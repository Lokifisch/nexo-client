//! Nexo client core: instance management, Microsoft authentication, the
//! Minecraft install/launch pipeline, and content APIs.
//!
//! Deliberately free of any UI dependency — the iced frontend in `nexo-app`
//! is one consumer, and keeping this crate headless means the launch pipeline
//! stays testable without a window.

pub mod accounts;
pub mod auth;
pub mod browse;
pub mod content;
pub mod cosmetics;
pub mod error;
pub mod github;
pub mod hwkey;
pub mod instance;
pub mod java;
pub mod minecraft;
pub mod modrinth;
pub mod mrpack;
pub mod nexo_mod;
pub mod options_sync;
pub mod paper_server;
pub mod paths;
pub mod running;
pub mod self_update;
pub mod server_ping;
pub mod shared_store;
pub mod skin;
pub mod skin_library;
pub mod util;

pub use accounts::AccountStore;
pub use auth::{Account, Auth, SkinModel};
pub use error::{Error, Result};
pub use instance::{Instance, InstanceStore, Loader};
pub use paths::Paths;

use minecraft::{Installer, LaunchOptions, Launcher, Progress};
use tokio::sync::mpsc::UnboundedSender;

/// Wires the pieces together so the UI has one thing to hold.
#[derive(Clone)]
pub struct Nexo {
    pub paths: Paths,
    pub instances: InstanceStore,
    pub accounts: AccountStore,
    pub auth: Auth,
    pub installer: Installer,
    pub nexo_mod: nexo_mod::NexoMod,
    pub content: content::Content,
    pub cosmetics: cosmetics::Cosmetics,
    pub skins: skin_library::SkinLibrary,
    pub mrpack: mrpack::MrPack,
    /// Converts singleplayer worlds to locally-run Paper servers and back —
    /// see `Mod/ROADMAP.md` Phase 7.
    pub paper_servers: paper_server::PaperServers,
    /// Updates the launcher itself, as opposed to anything it installs.
    pub self_update: self_update::SelfUpdate,
    /// Games this launcher started. Shared, so every clone of `Nexo` sees the
    /// same set — the UI holds one clone and each async task another.
    pub running: running::RunningGames,
    launch_locks: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>>,
    http: reqwest::Client,
}

impl Nexo {
    /// Resolves paths, creates the directory tree, and builds a shared HTTP
    /// client. One `reqwest::Client` for the whole app on purpose — it owns
    /// the connection pool, and creating them per-request would give up
    /// keep-alive across the hundreds of requests an install makes.
    pub async fn new() -> Result<Self> {
        Self::with_paths(Paths::discover()?).await
    }

    pub async fn with_paths(paths: Paths) -> Result<Self> {
        paths.ensure().await?;

        let http = reqwest::Client::builder()
            .user_agent(concat!("Lokifisch/nexo-client/", env!("CARGO_PKG_VERSION"),))
            // No total timeout (large jars): only stalls and dead hosts fail.
            .connect_timeout(std::time::Duration::from_secs(15))
            .read_timeout(std::time::Duration::from_secs(60))
            .build()?;

        Ok(Self {
            instances: InstanceStore::new(paths.clone()),
            accounts: AccountStore::new(&paths),
            auth: Auth::with_client(http.clone()),
            installer: Installer::new(http.clone(), paths.clone()),
            nexo_mod: nexo_mod::NexoMod::new(http.clone(), paths.clone()),
            content: content::Content::new(http.clone(), paths.clone()),
            cosmetics: cosmetics::Cosmetics::new(http.clone()),
            skins: skin_library::SkinLibrary::new(&paths),
            mrpack: mrpack::MrPack::new(http.clone(), paths.clone()),
            paper_servers: paper_server::PaperServers::new(http.clone(), paths.clone()),
            self_update: self_update::SelfUpdate::new(http.clone()),
            running: running::RunningGames::new(),
            launch_locks: Default::default(),
            paths,
            http,
        })
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    pub fn launcher(&self) -> Launcher {
        Launcher::new(self.paths.clone(), self.installer.clone())
    }

    /// Full play path: refresh the account, install anything missing, then
    /// spawn the game and register it as running.
    ///
    /// Returns once the JVM is up, not when the game exits — await
    /// [`running::RunningGames::wait_for_exit`] for that. The child itself is
    /// handed to the registry, which is the only place that can stop it.
    pub async fn play(
        &self,
        instance_id: &str,
        progress: Option<&UnboundedSender<Progress>>,
    ) -> Result<()> {
        self.play_as(instance_id, None, progress).await.map(|_| ())
    }

    /// Like [`Core::play`] but with a chosen account (`None` = the active one),
    /// so one instance can run several times at once, one per account. The
    /// sessions share the instance's game dir (saves, options.txt, mods) on
    /// purpose. The returned session key is `running::session_key(id, uuid)`.
    pub async fn play_as(
        &self,
        instance_id: &str,
        account_uuid: Option<&str>,
        progress: Option<&UnboundedSender<Progress>>,
    ) -> Result<String> {
        // Registered before the launch lock so a Stop works even while this
        // launch is still queued behind another one's install.
        let mut prepare = match account_uuid {
            Some(uuid) => Some(
                self.running
                    .begin_prepare(&running::session_key(instance_id, uuid))
                    .ok_or_else(|| Error::invalid("this account is already launching this instance"))?,
            ),
            None => None,
        };
        let cancelled = |p: &Option<running::PrepareGuard>| p.as_ref().is_some_and(|p| p.is_cancelled());
        // Serialises install + instance.save + spawn per instance, so a second
        // launch during the first one's install waits instead of racing it.
        // The wait for *exit* happens outside, so running games don't block.
        let lock = {
            let mut locks = self.launch_locks.lock().unwrap_or_else(|p| p.into_inner());
            std::sync::Arc::clone(locks.entry(instance_id.to_string()).or_default())
        };
        let _guard = lock.lock().await;

        if cancelled(&prepare) {
            return Err(Error::invalid(running::LAUNCH_CANCELLED));
        }
        let mut instance = self.instances.get(instance_id).await?;

        // Before anything expensive: a launch with a dead token wastes a
        // whole install pass before failing.
        let account = match account_uuid {
            Some(uuid) => self.accounts.valid_for(uuid, &self.auth).await?,
            None => self.accounts.active_valid(&self.auth).await?,
        };
        // A second client with the same account would be kicked by the server.
        let key = running::session_key(instance_id, &account.uuid);
        if prepare.is_none() {
            prepare = Some(
                self.running
                    .begin_prepare(&key)
                    .ok_or_else(|| Error::invalid("this account is already launching this instance"))?,
            );
        }
        if cancelled(&prepare) {
            return Err(Error::invalid(running::LAUNCH_CANCELLED));
        }
        if self.running.is_running(&key) {
            return Err(Error::invalid(format!(
                "{} is already playing this instance — pick another account",
                account.username
            )));
        }
        // Downloads a runtime if the machine has none new enough, so a fresh
        // install doesn't stop at "go and install Java first". An existing
        // system JVM is always preferred, and an instance that names its own
        // is never overridden.
        let java = java::ensure(
            &self.http,
            &self.paths,
            instance.java_path.as_deref(),
            progress,
        )
        .await?;

        let version = self.installer.install(&instance, progress).await?;
        if cancelled(&prepare) {
            return Err(Error::invalid(running::LAUNCH_CANCELLED));
        }

        // Record what Fabric build actually got installed, so later launches
        // are reproducible instead of silently drifting to a newer loader.
        if instance.loader == Loader::Fabric && instance.loader_version.is_none() {
            instance.loader_version =
                Some(minecraft::fabric::latest_stable(&self.http, &instance.game_version).await?);
        }
        // Nearly every Fabric mod needs Fabric API, and its absence surfaces
        // as a startup crash that names nothing useful — so it is installed
        // with the loader rather than left for the user to discover.
        if let Err(err) = self.content.ensure_fabric_api(&mut instance).await {
            tracing::warn!(%err, "could not install Fabric API");
        }

        instance.last_played = Some(instance::now());
        self.instances.save(&instance).await?;

        let options = LaunchOptions {
            account,
            java: java.path,
            memory_mb: instance.memory_mb.unwrap_or(minecraft::DEFAULT_MEMORY_MB),
            extra_jvm_args: Vec::new(),
        };

        if cancelled(&prepare) {
            return Err(Error::invalid(running::LAUNCH_CANCELLED));
        }
        let child = self
            .launcher()
            .launch(&instance, &version, &options)
            .await?;
        self.running.register(&key, child);
        // A Stop between the last check and register found only the prepare
        // entry (nothing to kill); honour it now that there is a game.
        if cancelled(&prepare) {
            self.running.stop(&key);
            return Err(Error::invalid(running::LAUNCH_CANCELLED));
        }
        drop(prepare);
        Ok(key)
    }

    /// Brings the active account's cosmetics up to date and persists them.
    ///
    /// Returns `None` when nobody is signed in. Failures are the caller's to
    /// treat as non-fatal: the stored values stay usable, they're just stale.
    pub async fn sync_active_profile(&self) -> Result<Option<Account>> {
        self.accounts.refresh_active(&self.auth).await
    }

    /// Stops one running session by key. Returns `false` if it wasn't running.
    pub fn stop(&self, session_key: &str) -> bool {
        self.running.stop(session_key)
    }

    /// Stops every session of an instance.
    pub fn stop_instance(&self, instance_id: &str) -> bool {
        self.running.stop_instance(instance_id)
    }
}
