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

#[derive(Debug)]
struct RunningGame {
    /// Taken when a stop is requested; the watcher kills on receipt.
    stop: Option<oneshot::Sender<()>>,
    /// Flips to `true` once the process is gone. A `watch` rather than a
    /// `Notify` because a waiter that arrives *after* exit must still resolve
    /// immediately instead of hanging forever.
    exited: watch::Receiver<bool>,
}

/// Registry of games started by this launcher, keyed by instance id.
#[derive(Debug, Clone, Default)]
pub struct RunningGames {
    games: Arc<Mutex<HashMap<String, RunningGame>>>,
}

impl RunningGames {
    pub fn new() -> Self {
        Self::default()
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

    pub fn is_running(&self, instance_id: &str) -> bool {
        self.lock().contains_key(instance_id)
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
            None => false,
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
        self.games.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
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
        games.register("demo", spawn_sleeper("30"));
        assert!(games.is_running("demo"));

        assert!(games.stop("demo"));
        games.wait_for_exit("demo").await;
        assert!(!games.is_running("demo"));
    }

    #[tokio::test]
    async fn deregisters_when_the_game_exits_on_its_own() {
        let games = RunningGames::new();
        games.register("quick", spawn_sleeper("0"));

        games.wait_for_exit("quick").await;
        assert!(!games.is_running("quick"));
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
