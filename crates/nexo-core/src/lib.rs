//! Nexo client core: instance management, Microsoft authentication, the
//! Minecraft install/launch pipeline, and content APIs.
//!
//! Deliberately free of any UI dependency — the iced frontend in `nexo-app`
//! is one consumer, and keeping this crate headless means the launch pipeline
//! stays testable without a window.

pub mod accounts;
pub mod auth;
pub mod error;
pub mod instance;
pub mod java;
pub mod minecraft;
pub mod modrinth;
pub mod paths;
pub mod skin;
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
            .user_agent(concat!(
                "Lokifisch/nexo-client/",
                env!("CARGO_PKG_VERSION"),
            ))
            .build()?;

        Ok(Self {
            instances: InstanceStore::new(paths.clone()),
            accounts: AccountStore::new(&paths),
            auth: Auth::with_client(http.clone()),
            installer: Installer::new(http.clone(), paths.clone()),
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
    /// spawn the game. Returns the child so the caller can stream its log.
    pub async fn play(
        &self,
        instance_id: &str,
        progress: Option<&UnboundedSender<Progress>>,
    ) -> Result<tokio::process::Child> {
        let mut instance = self.instances.get(instance_id).await?;

        // Before anything expensive: a launch with a dead token wastes a
        // whole install pass before failing.
        let account = self.accounts.active_valid(&self.auth).await?;
        let java = java::resolve(instance.java_path.as_deref()).await?;

        let version = self.installer.install(&instance, progress).await?;

        // Record what Fabric build actually got installed, so later launches
        // are reproducible instead of silently drifting to a newer loader.
        if instance.loader == Loader::Fabric && instance.loader_version.is_none() {
            instance.loader_version = Some(
                minecraft::fabric::latest_stable(&self.http, &instance.game_version).await?,
            );
        }
        instance.last_played = Some(instance::now());
        self.instances.save(&instance).await?;

        let options = LaunchOptions {
            account,
            java: java.path,
            memory_mb: instance.memory_mb.unwrap_or(minecraft::DEFAULT_MEMORY_MB),
            extra_jvm_args: Vec::new(),
        };

        self.launcher().launch(&instance, &version, &options).await
    }
}
