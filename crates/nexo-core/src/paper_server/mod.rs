//! Convert a singleplayer world into a locally-run Paper server, and back —
//! see `Mod/ROADMAP.md` Phase 7 for the full design and
//! `Mod/docs/PAPER-SERVER-REGISTRY.md` for the on-disk contract this module
//! shares with `dev.nexoclient.nexomod.paperserver` on the Mod side.
//!
//! Deliberately holds almost no state beyond what [`registry::PaperServerStore`]
//! already persists to disk: the only in-memory piece is a
//! [`running::RunningGames`] entry for a server *this* process just started,
//! used solely to register an exit callback. Stopping never depends on
//! holding that handle — it goes through RCON, so a server this process
//! didn't start (left running from a previous session, or started by the
//! Mod) can still be stopped and reverted from here.

pub mod convert;
pub mod files;
pub mod hangar;
pub mod paper_api;
pub mod process;
pub mod rcon;
pub mod registry;

pub use registry::{
    Eula, FriendHosting, Owner, PaperServer, PaperServerStore, Rcon, SourceWorld, status,
};

use crate::error::{Error, Result};
use crate::minecraft::download::Downloader;
use crate::paths::Paths;
use crate::running::RunningGames;
use crate::util::slugify;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::time::sleep;

const DEFAULT_PORT: u16 = 25566;
const DEFAULT_RCON_PORT: u16 = 25576;
const DEFAULT_HEAP_MB: u32 = 2048;
/// Generous on purpose: Paper does a mandatory, non-configurable one-time
/// "world storage migration" pass (with its own hardcoded 30s warning
/// delay) the first time it opens a save that wasn't already in its native
/// format — which is every single conversion this feature does. Measured
/// ~164s from process spawn to RCON actually answering on ordinary dev
/// hardware; 120s was cutting it close enough to fail outright, and gave up
/// without killing the process, which is what let a *second* conversion
/// attempt fail on a port collision with the still-running first one.
const RCON_START_TIMEOUT: Duration = Duration::from_secs(300);
const RCON_POLL_INTERVAL: Duration = Duration::from_secs(2);
const RCON_CALL_TIMEOUT: Duration = Duration::from_secs(5);
const STOP_POLL_TIMEOUT: Duration = Duration::from_secs(60);
const STOP_POLL_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub struct PaperServers {
    http: reqwest::Client,
    paths: Paths,
    store: PaperServerStore,
    /// Servers this process has itself started. Separate from the
    /// game-launch registry on [`crate::Nexo::running`] — different id
    /// namespace, and mixing them would make "what's running" ambiguous.
    running: RunningGames,
}

impl PaperServers {
    pub fn new(http: reqwest::Client, paths: Paths) -> Self {
        let store = PaperServerStore::new(paths.clone());
        Self {
            http,
            paths,
            store,
            running: RunningGames::new(),
        }
    }

    pub fn store(&self) -> &PaperServerStore {
        &self.store
    }

    pub fn idfor(world_name: &str) -> String {
        let id = slugify(world_name);
        if id.is_empty() {
            "world".to_string()
        } else {
            id
        }
    }

    /// Converts `save` into a fresh Paper server directory and starts it.
    /// Must only be called after real, explicit EULA consent — this does
    /// not ask, it assumes the caller already did (see [`registry::Eula`]).
    pub async fn convert_and_host(
        &self,
        save: &Path,
        world_name: &str,
        minecraft_version: &str,
        accepted_by: &str,
        game_mode: &str,
        host_player: &str,
    ) -> Result<PaperServer> {
        let id = Self::idfor(world_name);
        let rcon = Rcon {
            port: DEFAULT_RCON_PORT,
            password: files::random_rcon_password(),
        };
        let source_world = SourceWorld {
            original_save_id: world_name.to_string(),
            original_save_path: save.to_path_buf(),
            converted_at_epoch_second: registry_now(),
        };
        let record = PaperServer::new(
            id.clone(),
            world_name.to_string(),
            minecraft_version.to_string(),
            "world".to_string(),
            DEFAULT_PORT,
            rcon,
            source_world,
        )
        .with_eula_accepted(accepted_by);
        self.store.write(&record).await?;

        match self
            .do_convert_and_host(record.clone(), save, game_mode, host_player)
            .await
        {
            Ok(running) => Ok(running),
            Err(err) => {
                if let Some(current) = self.store.read(&id).await {
                    let _ = self.store.write(&current.with_status(status::ERROR)).await;
                }
                Err(err)
            }
        }
    }

    async fn do_convert_and_host(
        &self,
        record: PaperServer,
        save: &Path,
        game_mode: &str,
        host_player: &str,
    ) -> Result<PaperServer> {
        let server_dir = self.store.server_dir(&record.id);
        tracing::info!(save = %save.display(), server_dir = %server_dir.display(), "converting world to a Paper server");
        convert::to_server(save, &server_dir, &record.level_name).await?;

        files::write_eula(&server_dir).await?;
        files::write_server_properties(&server_dir, &record, game_mode).await?;

        let paper_api = paper_api::PaperApi::with_client(self.http.clone());
        let build = paper_api
            .latest_stable_build(&record.minecraft_version)
            .await?;
        tracing::info!(build = build.id, version = %record.minecraft_version, "downloading Paper");
        let task = paper_api.download_task(&build, server_dir.join("paper.jar"))?;
        Downloader::new(self.http.clone())
            .run(vec![task], None)
            .await?;

        let record = record.with_paper_build(build.id);
        self.store.write(&record).await?;

        let record = record.with_status(status::STARTING);
        self.store.write(&record).await?;
        let pid = process::start(
            &self.http,
            &self.paths,
            &server_dir,
            DEFAULT_HEAP_MB,
            &record.id,
            &self.running,
        )
        .await?;
        let record = record.with_owner(Owner {
            started_by: Some("launcher".to_string()),
            pid: Some(pid as u64),
            hostname: hostname(),
            started_at_epoch_second: Some(registry_now()),
        });
        self.store.write(&record).await?;

        if let Err(err) = wait_for_rcon(record.rcon.port, &record.rcon.password).await {
            // RCON is the only stop mechanism this codebase has, and it's
            // exactly what just failed — a hard kill is the only option
            // left. Leaving the process running would squat on the
            // hardcoded ports and fail every future conversion attempt with
            // a confusing "address already in use", the way this one did.
            tracing::warn!(id = %record.id, "killing the Paper process after it never accepted RCON");
            let key = format!("paper:{}", record.id);
            self.running.stop(&key);
            self.running.wait_for_exit(&key).await;
            return Err(err);
        }

        // Best-effort: the server is up either way, just without op granted.
        // Grantable even before the player has actually joined — `op`
        // resolves the name to a profile itself, unlike `/gamemode`.
        match rcon::RconClient::connect(
            "127.0.0.1",
            record.rcon.port,
            &record.rcon.password,
            RCON_CALL_TIMEOUT,
        )
        .await
        {
            Ok(mut client) => match client.command(&format!("op {host_player}")).await {
                Ok(_) => tracing::info!(id = %record.id, host_player, "granted operator"),
                Err(err) => {
                    tracing::warn!(id = %record.id, host_player, %err, "could not grant operator")
                }
            },
            Err(err) => {
                tracing::warn!(id = %record.id, host_player, %err, "could not grant operator")
            }
        }

        let record = record.with_status(status::RUNNING);
        self.store.write(&record).await?;
        tracing::info!(id = %record.id, port = record.port, "Paper server is up");
        Ok(record)
    }

    /// Stops the server over RCON — regardless of whether this process is
    /// the one that started it — and converts its world back into the
    /// original save path recorded at conversion time.
    ///
    /// Returns the path the pre-conversion save was backed up to.
    pub async fn stop_and_revert(&self, id: &str) -> Result<PathBuf> {
        let Some(record) = self.store.read(id).await else {
            return Err(Error::invalid(format!(
                "no Paper server registered for \"{id}\""
            )));
        };
        // The save path comes from shared, hand-editable JSON and is about to
        // be deleted and replaced: it must be a world inside an instance.
        let saves_root = tokio::fs::canonicalize(self.paths.instances()).await.ok();
        let save = tokio::fs::canonicalize(&record.source_world.original_save_path)
            .await
            .ok();
        let inside = matches!((&saves_root, &save), (Some(root), Some(save))
            if save.starts_with(root)
                && save.components().any(|c| c.as_os_str() == "saves"));
        if !inside {
            return Err(Error::invalid(
                "the recorded original world is not inside an instance's saves folder",
            ));
        }
        self.store
            .write(&record.clone().with_status(status::STOPPING))
            .await?;

        match self.do_stop_and_revert(&record).await {
            Ok(backup) => Ok(backup),
            Err(err) => {
                if let Some(current) = self.store.read(id).await {
                    let _ = self.store.write(&current.with_status(status::ERROR)).await;
                }
                Err(err)
            }
        }
    }

    async fn do_stop_and_revert(&self, record: &PaperServer) -> Result<PathBuf> {
        {
            let mut client = rcon::RconClient::connect(
                "127.0.0.1",
                record.rcon.port,
                &record.rcon.password,
                RCON_CALL_TIMEOUT,
            )
            .await?;
            client.command("stop").await?;
        }
        wait_for_rcon_refusal(record.rcon.port, &record.rcon.password).await;

        let server_dir = self.store.server_dir(&record.id);
        let backup = convert::to_singleplayer(
            &server_dir,
            &record.level_name,
            &record.source_world.original_save_path,
        )
        .await?;
        tracing::info!(id = %record.id, backup = %backup.display(), "converted back to singleplayer");

        self.store
            .write(
                &record
                    .clone()
                    .with_status(status::STOPPED)
                    .with_owner(Owner::none()),
            )
            .await?;
        Ok(backup)
    }
}

async fn wait_for_rcon(port: u16, password: &str) -> Result<()> {
    let deadline = tokio::time::Instant::now() + RCON_START_TIMEOUT;
    let mut last_err = None;
    while tokio::time::Instant::now() < deadline {
        match rcon::RconClient::connect("127.0.0.1", port, password, RCON_CALL_TIMEOUT).await {
            Ok(_) => return Ok(()),
            Err(err) => {
                last_err = Some(err);
                sleep(RCON_POLL_INTERVAL).await;
            }
        }
    }
    Err(Error::invalid(format!(
        "Paper server did not accept RCON connections within {RCON_START_TIMEOUT:?}{}",
        last_err
            .map(|e| format!(" (last error: {e})"))
            .unwrap_or_default()
    )))
}

/// Waits for the server to actually go down after "stop", by polling until
/// RCON refuses the connection.
async fn wait_for_rcon_refusal(port: u16, password: &str) {
    let deadline = tokio::time::Instant::now() + STOP_POLL_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        if rcon::RconClient::connect("127.0.0.1", port, password, RCON_CALL_TIMEOUT)
            .await
            .is_err()
        {
            return; // Connection refused (or reset) - the server is down.
        }
        sleep(STOP_POLL_INTERVAL).await;
    }
    tracing::warn!(
        port,
        "Paper server still accepted RCON after the stop timeout; converting its world back anyway"
    );
}

fn hostname() -> Option<String> {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
}

fn registry_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}
