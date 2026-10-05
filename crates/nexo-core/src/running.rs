//! Tracking of games this launcher started, so the UI can show a running
//! instance as running and stop it again.
//!
//! The launch path used to drop the [`Child`] as soon as the JVM was up,
//! which is fine for letting the game run detached but leaves nothing to
//! stop it with. Each running game now gets a watcher task that owns its
//! `Child` — ownership has to live in exactly one place, since both waiting
//! for exit and killing need `&mut Child`, and holding a lock across the
//! `wait()` await would block the kill it's meant to allow.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::process::Child;
use tokio::sync::{oneshot, watch};

/// Separates instance id from account uuid in a session key.
const SEP: char = '#';

/// Registry key for one instance launched with one account.
pub fn session_key(instance_id: &str, account_uuid: &str) -> String {
    format!("{instance_id}{SEP}{account_uuid}")
}

/// Splits a session key into (instance id, account uuid).
pub fn split_session_key(key: &str) -> Option<(&str, &str)> {
    key.split_once(SEP)
}

/// Message of the error `Core::play_as` returns when a Stop landed during the
/// prepare phase; the UI treats it as a quiet cancel, not a failure.
pub const LAUNCH_CANCELLED: &str = "launch cancelled";

/// Only `{id}#{uuid}` sessions belong to an instance (Paper servers register
/// as `paper:<id>`, never a plain id).
fn in_instance(key: &str, instance_id: &str) -> bool {
    key.strip_prefix(instance_id)
        .is_some_and(|rest| rest.starts_with(SEP))
}

type Preparing = Arc<Mutex<HashMap<String, bool>>>;

/// A session in its prepare phase (install, before spawn). Dropping removes it.
pub struct PrepareGuard {
    key: String,
    preparing: Preparing,
}

impl PrepareGuard {
    pub fn is_cancelled(&self) -> bool {
        self.preparing
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&self.key)
            .copied()
            .unwrap_or(false)
    }
}

impl Drop for PrepareGuard {
    fn drop(&mut self) {
        self.preparing.lock().unwrap_or_else(|p| p.into_inner()).remove(&self.key);
    }
}

#[derive(Debug)]
struct RunningGame {
    /// Taken when a stop is requested; the watcher kills on receipt.
    stop: Option<oneshot::Sender<()>>,
    /// Flips to `true` once the process is gone. A `watch` rather than a
    /// `Notify` because a waiter that arrives *after* exit must still resolve
    /// immediately instead of hanging forever.
    exited: watch::Receiver<bool>,
}

/// Registry of games started by this launcher. Game launches are keyed by
/// [`session_key`] (one instance can run once per account); Paper servers use
/// `paper:<id>`. `stop`/`wait_for_exit` take an exact key.
#[derive(Debug, Clone, Default)]
pub struct RunningGames {
    games: Arc<Mutex<HashMap<String, RunningGame>>>,
    /// Sessions still preparing -> whether a stop was requested.
    preparing: Preparing,
}

impl RunningGames {
    pub fn new() -> Self {
        Self::default()
    }

    /// Marks `key` as preparing so a stop can cancel it before the game
    /// exists. `None` if that key is already preparing.
    pub fn begin_prepare(&self, key: &str) -> Option<PrepareGuard> {
        let mut p = self.preparing.lock().unwrap_or_else(|p| p.into_inner());
        if p.contains_key(key) {
            return None;
        }
        p.insert(key.to_string(), false);
        Some(PrepareGuard { key: key.to_string(), preparing: Arc::clone(&self.preparing) })
    }

    /// Takes ownership of a freshly spawned child and starts watching it.
    pub fn register(&self, instance_id: &str, mut child: Child) {
        let (stop_tx, stop_rx) = oneshot::channel();
        let (exited_tx, exited_rx) = watch::channel(false);

        {
            let mut games = self.lock();
            // Replacing an entry for the same instance drops the old stop
            // sender, which makes its watcher fall through to `wait()` —
            // harmless, since a stale entry means that process already ended.
            games.insert(
                instance_id.to_string(),
                RunningGame {
                    stop: Some(stop_tx),
                    exited: exited_rx,
                },
            );
        }

        let games = Arc::clone(&self.games);
        let id = instance_id.to_string();

        tokio::spawn(async move {
            tokio::select! {
                result = child.wait() => {
                    match result {
                        Ok(status) => tracing::info!(instance = %id, ?status, "game exited"),
                        Err(err) => tracing::warn!(instance = %id, %err, "lost track of the game process"),
                    }
                }
                _ = stop_rx => {
                    tracing::info!(instance = %id, "stopping game");
                    if let Err(err) = child.kill().await {
                        tracing::warn!(instance = %id, %err, "could not stop the game");
                    }
                }
            }

            if let Ok(mut games) = games.lock() {
                games.remove(&id);
            }
            // Signalled last, so anything woken by it sees a registry that
            // already reflects the exit.
            let _ = exited_tx.send(true);
        });
    }

    /// True if the exact session key is registered or any session of that
    /// instance runs.
    pub fn is_running(&self, instance_id: &str) -> bool {
        let games = self.lock();
        games.contains_key(instance_id) || games.keys().any(|k| in_instance(k, instance_id))
    }

    /// Account uuids of the sessions currently running for `instance_id`.
    pub fn sessions(&self, instance_id: &str) -> Vec<String> {
        let prefix = format!("{instance_id}{SEP}");
        self.lock()
            .keys()
            .filter_map(|k| k.strip_prefix(&prefix).map(str::to_string))
            .collect()
    }

    /// Stops every session of the instance. Returns `false` if none ran.
    pub fn stop_instance(&self, instance_id: &str) -> bool {
        let keys: Vec<String> = self
            .lock()
            .keys()
            .filter(|k| in_instance(k, instance_id))
            .cloned()
            .collect();
        let mut any = false;
        for k in &keys {
            any |= self.stop(k);
        }
        // Sessions still preparing have no process yet: flag them cancelled.
        for (k, cancelled) in self.preparing.lock().unwrap_or_else(|p| p.into_inner()).iter_mut() {
            if in_instance(k, instance_id) {
                *cancelled = true;
                any = true;
            }
        }
        any
    }

    pub fn running_ids(&self) -> Vec<String> {
        self.lock().keys().cloned().collect()
    }

    /// Asks the game to stop. Returns `false` if it wasn't running.
    pub fn stop(&self, instance_id: &str) -> bool {
        let mut games = self.lock();
        match games.get_mut(instance_id) {
            Some(game) => {
                // `send` fails only if the watcher is already gone, which
                // means the process ended on its own — still a success from
                // the caller's point of view.
                if let Some(stop) = game.stop.take() {
                    let _ = stop.send(());
                }
                true
            }
            None => match self.preparing.lock().unwrap_or_else(|p| p.into_inner()).get_mut(instance_id) {
                Some(cancelled) => {
                    *cancelled = true;
                    true
                }
                None => false,
            },
        }
    }

    /// Resolves when the game is no longer running, immediately if it never
    /// was.
    pub async fn wait_for_exit(&self, instance_id: &str) {
        let Some(mut exited) = self.lock().get(instance_id).map(|g| g.exited.clone()) else {
            return;
        };
        // `wait_for` checks the current value first, so an exit that lands
        // between the lookup above and this await is not missed.
        let _ = exited.wait_for(|done| *done).await;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, RunningGame>> {
        // A panic while holding this lock would only ever leave the registry
        // mid-insert; recovering is far better than poisoning every later
        // launch.
        self.games
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spawn_sleeper(seconds: &str) -> Child {
        tokio::process::Command::new("sleep")
            .arg(seconds)
            .kill_on_drop(true)
            .spawn()
            .expect("sleep should be available")
    }

    #[tokio::test]
    async fn tracks_and_stops_a_running_game() {
        let games = RunningGames::new();
        let k = session_key("demo", "A");
        games.register(&k, spawn_sleeper("30"));
        assert!(games.is_running("demo"));

        assert!(games.stop(&k));
        games.wait_for_exit(&k).await;
        assert!(!games.is_running("demo"));
    }

    #[tokio::test]
    async fn deregisters_when_the_game_exits_on_its_own() {
        let games = RunningGames::new();
        let k = session_key("quick", "A");
        games.register(&k, spawn_sleeper("0"));

        games.wait_for_exit(&k).await;
        assert!(!games.is_running("quick"));
    }

    #[tokio::test]
    async fn sessions_of_one_instance_are_independent() {
        let games = RunningGames::new();
        let (a, b, c) = (session_key("inst", "A"), session_key("inst", "B"), session_key("other", "A"));
        games.register(&a, spawn_sleeper("30"));
        games.register(&b, spawn_sleeper("30"));
        games.register(&c, spawn_sleeper("30"));

        assert!(games.is_running("inst") && games.is_running(&a) && games.is_running("other"));
        let mut s = games.sessions("inst");
        s.sort();
        assert_eq!(s, ["A", "B"]);
        // "inst" must not match "inst2"-style ids by prefix alone.
        assert!(!games.is_running("ins") && games.sessions("ins").is_empty());

        assert!(games.stop(&a));
        games.wait_for_exit(&a).await;
        assert_eq!(games.sessions("inst"), ["B"]);
        assert!(games.is_running("inst") && games.is_running("other"));

        assert!(games.stop_instance("inst"));
        games.wait_for_exit(&b).await;
        assert!(!games.is_running("inst") && games.is_running("other"));
        games.stop(&c);
        games.wait_for_exit(&c).await;
    }

    #[tokio::test]
    async fn paper_ids_do_not_belong_to_an_instance() {
        let games = RunningGames::new();
        games.register("paper:inst", spawn_sleeper("30"));
        assert!(!games.is_running("inst") && !games.stop_instance("inst"));
        assert!(games.stop("paper:inst"));
        games.wait_for_exit("paper:inst").await;
    }

    #[test]
    fn stop_cancels_a_preparing_session() {
        let games = RunningGames::new();
        let (a, b) = (session_key("inst", "A"), session_key("inst", "B"));
        let (ga, gb) = (games.begin_prepare(&a).unwrap(), games.begin_prepare(&b).unwrap());
        assert!(games.begin_prepare(&a).is_none());
        assert!(!ga.is_cancelled() && !gb.is_cancelled());
        assert!(games.stop(&a));
        assert!(ga.is_cancelled() && !gb.is_cancelled());
        assert!(games.stop_instance("inst") && gb.is_cancelled());
        drop(ga);
        assert!(games.begin_prepare(&a).is_some());
        assert!(!games.stop("nope#x"));
    }

    #[tokio::test]
    async fn waiting_on_an_unknown_instance_returns_immediately() {
        let games = RunningGames::new();
        // Would hang if a missing entry were treated as "not yet exited".
        games.wait_for_exit("never-started").await;
        assert!(!games.is_running("never-started"));
        assert!(!games.stop("never-started"));
    }
}
