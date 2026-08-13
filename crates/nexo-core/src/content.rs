//! Installing content into an instance: from Modrinth, or from a local file.
//!
//! Every kind of content is a file dropped into a particular folder of the
//! instance — the launcher's job is picking the right folder, verifying the
//! download, and recording what went where so it can be listed and removed
//! again later.

use crate::error::{Error, IoContext, Result};
use crate::instance::{InstalledMod, Instance, ModSource};
use crate::modrinth::{Modrinth, Version};
use crate::paths::Paths;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Modrinth's slug for Fabric API. Slugs work anywhere an id does, and this
/// one is far more recognisable than `P7dR8mSH`.
pub const FABRIC_API: &str = "fabric-api";

/// What a project is, which decides where its file belongs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ProjectKind {
    #[default]
    Mod,
    ResourcePack,
    Shader,
}

impl ProjectKind {
    /// Modrinth's `project_type` facet value.
    pub fn facet(self) -> &'static str {
        match self {
            Self::Mod => "mod",
            Self::ResourcePack => "resourcepack",
            Self::Shader => "shader",
        }
    }

    /// Folder inside the instance this kind installs into. These names are
    /// Minecraft's, not ours — the game looks for exactly these.
    pub fn folder(self) -> &'static str {
        match self {
            Self::Mod => "mods",
            Self::ResourcePack => "resourcepacks",
            Self::Shader => "shaderpacks",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Mod => "Mods",
            Self::ResourcePack => "Resource packs",
            Self::Shader => "Shaders",
        }
    }

    pub const ALL: [Self; 3] = [Self::Mod, Self::ResourcePack, Self::Shader];

    /// Best guess for a file the user dropped in, from its extension.
    /// Resource packs and shaders are both zips, so a `.zip` can't be told
    /// apart and the caller has to say which it meant.
    fn from_extension(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "jar" => Some(Self::Mod),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Content {
    http: reqwest::Client,
    paths: Paths,
    modrinth: Modrinth,
}

impl Content {
    pub fn new(http: reqwest::Client, paths: Paths) -> Self {
        Self {
            modrinth: Modrinth::with_client(http.clone()),
            http,
            paths,
        }
    }

    pub fn modrinth(&self) -> &Modrinth {
        &self.modrinth
    }

    fn folder(&self, instance: &Instance, kind: ProjectKind) -> PathBuf {
        self.paths.instance(&instance.id).join(kind.folder())
    }

    /// Installs the newest release of a Modrinth project compatible with the
    /// instance.
    ///
    /// Resource packs and shaders aren't loader-specific, so only mods are
    /// filtered by loader — asking Modrinth for a "fabric" resource pack
    /// returns nothing.
    pub async fn install_modrinth(
        &self,
        instance: &mut Instance,
        project_id: &str,
        kind: ProjectKind,
    ) -> Result<()> {
        let loader = match kind {
            ProjectKind::Mod => instance
                .loader
                .modrinth_facet()
                .ok_or_else(|| Error::invalid("this instance has no mod loader"))?,
            // Modrinth still expects a loader facet for these; the pack
            // "loaders" are named after the format they target.
            ProjectKind::ResourcePack => "minecraft",
            ProjectKind::Shader => "iris",
        };

        let version = self
            .modrinth
            .latest_version(project_id, loader, &instance.game_version)
            .await?;

        self.install_version(instance, project_id, kind, &version)
            .await
    }

    /// Downloads and records one specific version.
    pub async fn install_version(
        &self,
        instance: &mut Instance,
        project_id: &str,
        kind: ProjectKind,
        version: &Version,
    ) -> Result<()> {
        let file = version.primary_file()?;
        // Checksum-verified inside `download`, so a corrupt transfer can't
        // land in the instance.
        let bytes = self.modrinth.download(file).await?;

        // Replacing an existing copy first; two versions of the same mod in
        // the folder is a startup failure.
        self.remove(instance, project_id).await?;

        let folder = self.folder(instance, kind);
        tokio::fs::create_dir_all(&folder).await.ctx(&folder)?;
        let destination = folder.join(&file.filename);
        tokio::fs::write(&destination, &bytes)
            .await
            .ctx(&destination)?;

        instance.mods.push(InstalledMod {
            project_id: project_id.to_string(),
            name: version.name.clone(),
            version_id: version.id.clone(),
            version_number: version.version_number.clone(),
            file_name: file.filename.clone(),
            source: ModSource::Modrinth,
            enabled: true,
            edition: None,
        });

        Ok(())
    }

    /// Copies a local file into the instance.
    ///
    /// Tracked under a `local:` project id so it can be listed and removed
    /// like anything else, while never colliding with a real Modrinth id.
    pub async fn install_file(
        &self,
        instance: &mut Instance,
        source: &Path,
        kind: Option<ProjectKind>,
    ) -> Result<()> {
        let kind = kind
            .or_else(|| ProjectKind::from_extension(source))
            .ok_or_else(|| {
                Error::invalid(
                    "couldn't tell what kind of content this file is — pick mod, \
                     resource pack, or shader explicitly",
                )
            })?;

        let file_name = source
            .file_name()
            .ok_or_else(|| Error::invalid("that path has no file name"))?
            .to_string_lossy()
            .to_string();

        let bytes = tokio::fs::read(source).await.ctx(source)?;

        let folder = self.folder(instance, kind);
        tokio::fs::create_dir_all(&folder).await.ctx(&folder)?;
        let destination = folder.join(&file_name);
        tokio::fs::write(&destination, &bytes)
            .await
            .ctx(&destination)?;

        let project_id = format!("local:{file_name}");
        instance.mods.retain(|m| m.project_id != project_id);
        instance.mods.push(InstalledMod {
            project_id,
            name: file_name
                .trim_end_matches(".jar")
                .trim_end_matches(".zip")
                .to_string(),
            version_id: String::new(),
            version_number: "local file".to_string(),
            file_name,
            source: ModSource::Local,
            enabled: true,
            edition: None,
        });

        Ok(())
    }

    /// Deletes an installed project's file and forgets it. Safe to call when
    /// nothing matches.
    pub async fn remove(&self, instance: &mut Instance, project_id: &str) -> Result<()> {
        let Some(installed) = instance
            .mods
            .iter()
            .find(|m| m.project_id == project_id)
            .cloned()
        else {
            return Ok(());
        };

        // The kind isn't recorded, so every folder is checked — cheaper and
        // more robust than guessing from the extension.
        for kind in ProjectKind::ALL {
            for name in [
                installed.file_name.clone(),
                format!("{}.disabled", installed.file_name),
            ] {
                let path = self.folder(instance, kind).join(name);
                if path.exists() {
                    tokio::fs::remove_file(&path).await.ctx(&path)?;
                }
            }
        }

        instance.mods.retain(|m| m.project_id != project_id);
        Ok(())
    }

    /// Installs Fabric API if the instance is on Fabric and doesn't have it.
    ///
    /// Nearly every Fabric mod depends on it, and its absence shows up as a
    /// crash on startup rather than as anything that names the real problem —
    /// so a Fabric instance gets it whether or not the user asked.
    pub async fn ensure_fabric_api(&self, instance: &mut Instance) -> Result<()> {
        if instance.loader != crate::instance::Loader::Fabric {
            return Ok(());
        }
        if instance.mods.iter().any(|m| m.project_id == FABRIC_API) {
            return Ok(());
        }

        self.install_modrinth(instance, FABRIC_API, ProjectKind::Mod)
            .await
    }

    /// Exposes the shared HTTP client so callers can reuse the pool.
    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// The icon a mod carries inside its own jar.
    ///
    /// Modrinth icons are only known while a search's results are in memory,
    /// so installed content lost its icons on every restart. A Fabric jar
    /// declares its own icon in `fabric.mod.json`, which is right there on
    /// disk — no network, and it works for mods that never came from Modrinth
    /// at all.
    pub async fn jar_icon(
        &self,
        instance: &Instance,
        file_name: &str,
    ) -> Option<crate::skin::Rgba> {
        // Which folder a mod landed in isn't recorded, and this is three
        // cheap existence checks.
        let path = ProjectKind::ALL
            .iter()
            .map(|kind| self.folder(instance, *kind).join(file_name))
            .find(|path| path.exists())?;

        // zip and image are synchronous and this reads a whole file, so it
        // belongs off the async runtime's worker threads.
        tokio::task::spawn_blocking(move || read_jar_icon(&path))
            .await
            .ok()
            .flatten()
    }
}

/// Pulls the icon out of a Fabric jar, or `None` if it hasn't got one.
///
/// Every failure here is ordinary rather than exceptional — not a zip, no
/// `fabric.mod.json`, no `icon` key, a path that isn't in the archive — so
/// they all collapse to `None` rather than an error the UI would have to
/// explain.
fn read_jar_icon(path: &Path) -> Option<crate::skin::Rgba> {
    use std::io::Read;

    let file = std::fs::File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;

    let metadata: serde_json::Value = {
        let mut entry = archive.by_name("fabric.mod.json").ok()?;
        let mut text = String::new();
        entry.read_to_string(&mut text).ok()?;
        serde_json::from_str(&text).ok()?
    };

    // `icon` is either a path, or a map of size to path for mods shipping
    // several. The largest is wanted, since it only ever gets scaled down.
    let icon_path = match metadata.get("icon")? {
        serde_json::Value::String(path) => path.clone(),
        serde_json::Value::Object(sizes) => sizes
            .iter()
            .filter_map(|(size, path)| Some((size.parse::<u32>().ok()?, path.as_str()?)))
            .max_by_key(|(size, _)| *size)
            .map(|(_, path)| path.to_string())?,
        _ => return None,
    };

    let mut icon = archive.by_name(&icon_path).ok()?;
    let mut bytes = Vec::new();
    icon.read_to_end(&mut bytes).ok()?;

    let decoded = image::load_from_memory(&bytes).ok()?.to_rgba8();
    let (width, height) = decoded.dimensions();
    Some(crate::skin::Rgba {
        width,
        height,
        pixels: decoded.into_raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_map_to_minecrafts_own_folder_names() {
        assert_eq!(ProjectKind::Mod.folder(), "mods");
        assert_eq!(ProjectKind::ResourcePack.folder(), "resourcepacks");
        assert_eq!(ProjectKind::Shader.folder(), "shaderpacks");
    }

    #[test]
    fn jars_are_recognised_but_ambiguous_zips_are_not() {
        assert_eq!(
            ProjectKind::from_extension(Path::new("sodium.jar")),
            Some(ProjectKind::Mod)
        );
        // A zip could be a resource pack or a shader, so the caller must say.
        assert_eq!(ProjectKind::from_extension(Path::new("pack.zip")), None);
        assert_eq!(ProjectKind::from_extension(Path::new("notes")), None);
    }

    #[tokio::test]
    async fn installing_a_local_file_records_and_removes_it() {
        let temp = std::env::temp_dir().join(format!("nexo-content-{}", uuid::Uuid::new_v4()));
        let paths = Paths::with_root(&temp);
        paths.ensure().await.unwrap();

        let source = temp.join("example.jar");
        tokio::fs::write(&source, b"not really a jar").await.unwrap();

        let content = Content::new(reqwest::Client::new(), paths.clone());
        let mut instance =
            Instance::new("local-test", "26.1.2", crate::instance::Loader::Fabric);

        content
            .install_file(&mut instance, &source, None)
            .await
            .unwrap();

        assert_eq!(instance.mods.len(), 1);
        let installed = &instance.mods[0];
        assert_eq!(installed.source, ModSource::Local);
        assert!(paths
            .instance_mods(&instance.id)
            .join("example.jar")
            .exists());

        let id = installed.project_id.clone();
        content.remove(&mut instance, &id).await.unwrap();
        assert!(instance.mods.is_empty());
        assert!(!paths
            .instance_mods(&instance.id)
            .join("example.jar")
            .exists());

        tokio::fs::remove_dir_all(&temp).await.ok();
    }

    #[tokio::test]
    async fn a_vanilla_instance_gets_no_fabric_api() {
        let temp = std::env::temp_dir().join(format!("nexo-content-{}", uuid::Uuid::new_v4()));
        let paths = Paths::with_root(&temp);
        paths.ensure().await.unwrap();

        let content = Content::new(reqwest::Client::new(), paths);
        let mut instance =
            Instance::new("vanilla", "26.1.2", crate::instance::Loader::Vanilla);

        // Returns without touching the network, which is why this test can
        // run offline.
        content.ensure_fabric_api(&mut instance).await.unwrap();
        assert!(instance.mods.is_empty());

        tokio::fs::remove_dir_all(&temp).await.ok();
    }
}
