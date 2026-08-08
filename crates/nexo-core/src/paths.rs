//! Where everything lives on disk.
//!
//! One root directory holds instances, the shared asset/library cache, and
//! account storage. Shared caching across instances is the whole reason
//! instances aren't self-contained folders: Minecraft's asset objects and
//! Maven libraries are identical across every instance on the same version,
//! and re-downloading ~200 MB per instance would be absurd.

use crate::error::{Error, IoContext, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Paths {
    root: PathBuf,
}

impl Paths {
    /// Resolves the platform data directory:
    /// - Linux: `~/.local/share/nexo`
    /// - Windows: `%APPDATA%\Nexo\data`
    /// - macOS: `~/Library/Application Support/dev.nexoclient.nexo`
    pub fn discover() -> Result<Self> {
        let dirs = directories::ProjectDirs::from("dev", "nexoclient", "nexo")
            .ok_or(Error::NoDataDir)?;
        Ok(Self {
            root: dirs.data_dir().to_path_buf(),
        })
    }

    /// Points every path at `root` instead of the platform default. Used by
    /// tests and by portable installs that keep their data next to the binary.
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// One subdirectory per instance, keyed by the instance's slug.
    pub fn instances(&self) -> PathBuf {
        self.root.join("instances")
    }

    pub fn instance(&self, id: &str) -> PathBuf {
        self.instances().join(id)
    }

    /// Vanilla launcher layout, so an instance directory is recognizable to
    /// anyone who has poked at `.minecraft` before.
    pub fn instance_mods(&self, id: &str) -> PathBuf {
        self.instance(id).join("mods")
    }

    /// Shared across instances — see the module note on why.
    pub fn libraries(&self) -> PathBuf {
        self.root.join("libraries")
    }

    pub fn assets(&self) -> PathBuf {
        self.root.join("assets")
    }

    pub fn asset_objects(&self) -> PathBuf {
        self.assets().join("objects")
    }

    pub fn asset_indexes(&self) -> PathBuf {
        self.assets().join("indexes")
    }

    /// Per-version client jars and their JSON manifests.
    pub fn versions(&self) -> PathBuf {
        self.root.join("versions")
    }

    /// Runtimes we downloaded ourselves, when no suitable system JDK exists.
    pub fn java_runtimes(&self) -> PathBuf {
        self.root.join("java")
    }

    pub fn accounts_file(&self) -> PathBuf {
        self.root.join("accounts.json")
    }

    pub fn settings_file(&self) -> PathBuf {
        self.root.join("settings.json")
    }

    /// Creates the directories that must exist before anything else runs.
    pub async fn ensure(&self) -> Result<()> {
        for dir in [
            self.root.clone(),
            self.instances(),
            self.libraries(),
            self.asset_objects(),
            self.asset_indexes(),
            self.versions(),
            self.java_runtimes(),
        ] {
            tokio::fs::create_dir_all(&dir).await.ctx(&dir)?;
        }
        Ok(())
    }
}
