//! Instance model and on-disk store.
//!
//! Each instance is a self-describing directory: its metadata lives in a
//! `nexo-instance.json` inside it rather than in one central registry file.
//! That means an instance can be copied, backed up, or hand-edited without
//! the app losing track of it, and a corrupt instance can't take the whole
//! list down with it.

use crate::error::{Error, IoContext, Result};
use crate::util::slugify;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Filename of the per-instance metadata document.
const MANIFEST: &str = "nexo-instance.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Loader {
    Vanilla,
    #[default]
    Fabric,
}

impl Loader {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Vanilla => "vanilla",
            Self::Fabric => "fabric",
        }
    }

    /// What Modrinth calls this loader in search facets. Vanilla instances
    /// can't take mods at all, hence the `Option`.
    pub fn modrinth_facet(self) -> Option<&'static str> {
        match self {
            Self::Vanilla => None,
            Self::Fabric => Some("fabric"),
        }
    }
}

impl std::fmt::Display for Loader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Vanilla => "Vanilla",
            Self::Fabric => "Fabric",
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instance {
    /// Slug, and also the directory name — see [`crate::paths::Paths::instance`].
    pub id: String,
    pub name: String,
    pub game_version: String,
    pub loader: Loader,
    /// Resolved Fabric loader build. `None` until the instance is installed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loader_version: Option<String>,

    pub created_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_played: Option<u64>,

    /// Heap ceiling in MiB. `None` follows the global setting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_mb: Option<u32>,
    /// Overrides Java autodetection for this instance only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub java_path: Option<PathBuf>,

    /// Content installed into `mods/`, tracked so the UI can list, update,
    /// and remove mods without re-reading jar metadata off disk.
    #[serde(default)]
    pub mods: Vec<InstalledMod>,
}

impl Instance {
    pub fn new(name: impl Into<String>, game_version: impl Into<String>, loader: Loader) -> Self {
        let name = name.into();
        Self {
            id: slugify(&name),
            name,
            game_version: game_version.into(),
            loader,
            loader_version: None,
            created_at: now(),
            last_played: None,
            memory_mb: None,
            java_path: None,
            mods: Vec::new(),
        }
    }

    /// True once the instance has everything it needs to launch.
    pub fn is_installed(&self) -> bool {
        self.loader == Loader::Vanilla || self.loader_version.is_some()
    }

    pub fn find_mod(&self, project_id: &str) -> Option<&InstalledMod> {
        self.mods.iter().find(|m| m.project_id == project_id)
    }

    pub fn has_mod(&self, project_id: &str) -> bool {
        self.find_mod(project_id).is_some()
    }
}

/// One jar in the instance's `mods/` folder, plus where it came from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledMod {
    /// Modrinth project id, or a `nexo:`-prefixed synthetic id for
    /// first-party content that isn't published there.
    pub project_id: String,
    pub name: String,
    pub version_id: String,
    pub version_number: String,
    /// Filename within `mods/`.
    pub file_name: String,
    pub source: ModSource,
    /// Disabled mods keep their jar but get a `.disabled` suffix, the
    /// convention every Fabric-aware launcher uses.
    #[serde(default)]
    pub enabled: bool,
    /// Which Nexo Mod edition this jar is, for [`ModSource::NexoMod`] entries.
    /// `None` on everything else, and on entries written before the editions
    /// existed — [`crate::nexo_mod::installed_edition`] falls back to the file
    /// name for those.
    #[serde(
        default,
        deserialize_with = "crate::nexo_mod::deserialize_edition",
        skip_serializing_if = "Option::is_none"
    )]
    pub edition: Option<crate::nexo_mod::Edition>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModSource {
    Modrinth,
    /// Nexo Mod itself, installed from its GitHub releases.
    NexoMod,
    /// Dropped in by the user from a local file.
    Local,
}

#[derive(Debug, Clone)]
pub struct InstanceStore {
    paths: crate::paths::Paths,
}

impl InstanceStore {
    pub fn new(paths: crate::paths::Paths) -> Self {
        Self { paths }
    }

    /// Reads every instance directory. A directory whose manifest is missing
    /// or unparseable is skipped with a warning rather than failing the whole
    /// load — one hand-edited file shouldn't hide the user's other instances.
    pub async fn list(&self) -> Result<Vec<Instance>> {
        let dir = self.paths.instances();
        if !dir.exists() {
            return Ok(Vec::new());
        }

        let mut entries = tokio::fs::read_dir(&dir).await.ctx(&dir)?;
        let mut instances = Vec::new();

        while let Some(entry) = entries.next_entry().await.ctx(&dir)? {
            if !entry.file_type().await.ctx(entry.path())?.is_dir() {
                continue;
            }
            let manifest = entry.path().join(MANIFEST);
            if !manifest.exists() {
                continue;
            }
            match self.read_manifest(&manifest).await {
                Ok(instance) => instances.push(instance),
                Err(err) => {
                    tracing::warn!(path = %manifest.display(), %err, "skipping unreadable instance");
                }
            }
        }

        instances.sort_by(|a, b| {
            b.last_played
                .cmp(&a.last_played)
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(instances)
    }

    async fn read_manifest(&self, path: &std::path::Path) -> Result<Instance> {
        let raw = tokio::fs::read(path).await.ctx(path)?;
        Ok(serde_json::from_slice(&raw)?)
    }

    pub async fn get(&self, id: &str) -> Result<Instance> {
        let manifest = self.paths.instance(id).join(MANIFEST);
        if !manifest.exists() {
            return Err(Error::invalid(format!("no instance named '{id}'")));
        }
        self.read_manifest(&manifest).await
    }

    /// Writes the manifest, creating the instance directory tree if needed.
    pub async fn save(&self, instance: &Instance) -> Result<()> {
        let dir = self.paths.instance(&instance.id);
        let mods = self.paths.instance_mods(&instance.id);
        tokio::fs::create_dir_all(&mods).await.ctx(&mods)?;

        let manifest = dir.join(MANIFEST);
        let json = serde_json::to_vec_pretty(instance)?;
        // Write-then-rename so a crash mid-write can't leave a truncated
        // manifest that would make the instance vanish from the list.
        let temp = dir.join(format!("{MANIFEST}.tmp"));
        tokio::fs::write(&temp, &json).await.ctx(&temp)?;
        tokio::fs::rename(&temp, &manifest).await.ctx(&manifest)?;
        Ok(())
    }

    /// Reserves a unique id for `name`, suffixing `-2`, `-3`, … if the
    /// obvious slug is taken.
    pub async fn create(
        &self,
        name: &str,
        game_version: &str,
        loader: Loader,
    ) -> Result<Instance> {
        let mut instance = Instance::new(name, game_version, loader);
        let base = instance.id.clone();
        let mut n = 2;
        while self.paths.instance(&instance.id).exists() {
            instance.id = format!("{base}-{n}");
            n += 1;
        }
        self.save(&instance).await?;
        Ok(instance)
    }

    /// Deletes the instance directory and everything in it, including saves.
    pub async fn delete(&self, id: &str) -> Result<()> {
        let dir = self.paths.instance(id);
        if dir.exists() {
            tokio::fs::remove_dir_all(&dir).await.ctx(&dir)?;
        }
        Ok(())
    }
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn create_dedupes_ids_for_same_name() {
        let temp = std::env::temp_dir().join(format!("nexo-test-{}", uuid::Uuid::new_v4()));
        let paths = crate::paths::Paths::with_root(&temp);
        paths.ensure().await.unwrap();
        let store = InstanceStore::new(paths);

        let a = store.create("My Pack", "26.1.2", Loader::Fabric).await.unwrap();
        let b = store.create("My Pack", "26.1.2", Loader::Fabric).await.unwrap();

        assert_eq!(a.id, "my-pack");
        assert_eq!(b.id, "my-pack-2");
        assert_eq!(store.list().await.unwrap().len(), 2);

        tokio::fs::remove_dir_all(&temp).await.ok();
    }

    #[tokio::test]
    async fn roundtrips_through_disk() {
        let temp = std::env::temp_dir().join(format!("nexo-test-{}", uuid::Uuid::new_v4()));
        let paths = crate::paths::Paths::with_root(&temp);
        paths.ensure().await.unwrap();
        let store = InstanceStore::new(paths);

        let mut instance = store.create("Roundtrip", "26.1.2", Loader::Fabric).await.unwrap();
        instance.loader_version = Some("0.19.3".into());
        instance.memory_mb = Some(4096);
        store.save(&instance).await.unwrap();

        let loaded = store.get(&instance.id).await.unwrap();
        assert_eq!(loaded.loader_version.as_deref(), Some("0.19.3"));
        assert_eq!(loaded.memory_mb, Some(4096));
        assert!(loaded.is_installed());

        tokio::fs::remove_dir_all(&temp).await.ok();
    }
}
