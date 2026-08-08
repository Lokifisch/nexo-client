//! Installing Nexo Mod into an instance from its GitHub releases.
//!
//! "Injection" here means *automated installation tooling* — dropping the
//! mod's jar into a target instance's `mods/` folder, the way Essential's or
//! Lunar's installers do. Nothing hooks another running process.
//!
//! What a given release targets is never hardcoded: every release publishes a
//! `manifest.json` asset next to the jar declaring its Minecraft version and
//! loader, and compatibility is checked against that. A constant here would
//! drift out of sync with what's actually published.
//!
//! Installation refuses rather than adapts. If the instance is on a different
//! Minecraft version or loader, it fails with an explanation — it never
//! changes an instance's loader or version on the user's behalf.

use crate::error::{Error, IoContext, Result};
use crate::instance::{Instance, InstalledMod, Loader, ModSource};
use crate::paths::Paths;
use serde::Deserialize;

/// `owner/repo` the releases are fetched from.
const REPO: &str = "Lokifisch/nexo-mod";

/// Synthetic project id, since Nexo Mod isn't published on Modrinth. The
/// `nexo:` prefix keeps it from ever colliding with a real Modrinth id.
pub const PROJECT_ID: &str = "nexo:nexo-mod";

/// What a published release declares about itself.
#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub minecraft_version: String,
    pub loader: String,
    pub mod_version: String,
}

impl Manifest {
    fn loader(&self) -> Option<Loader> {
        match self.loader.to_ascii_lowercase().as_str() {
            "fabric" => Some(Loader::Fabric),
            "vanilla" => Some(Loader::Vanilla),
            _ => None,
        }
    }

    /// Whether this release can be installed into `instance` as it stands.
    pub fn supports(&self, instance: &Instance) -> bool {
        self.loader() == Some(instance.loader)
            && self.minecraft_version == instance.game_version
    }

    /// Why it can't be installed, phrased for the UI.
    pub fn incompatibility(&self, instance: &Instance) -> Option<String> {
        if self.supports(instance) {
            return None;
        }
        Some(format!(
            "Nexo Mod {} targets {} {} — this instance is {} {}.",
            self.mod_version,
            self.loader,
            self.minecraft_version,
            instance.loader,
            instance.game_version,
        ))
    }
}

/// A release, resolved to the two assets that matter.
#[derive(Debug, Clone)]
pub struct Release {
    pub tag: String,
    pub manifest: Manifest,
    jar_url: String,
    pub jar_name: String,
}

impl Release {
    pub fn version(&self) -> &str {
        &self.manifest.mod_version
    }
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Debug, Clone)]
pub struct NexoMod {
    http: reqwest::Client,
    paths: Paths,
}

impl NexoMod {
    pub fn new(http: reqwest::Client, paths: Paths) -> Self {
        Self { http, paths }
    }

    /// Fetches the newest release and reads its manifest.
    ///
    /// Uses `/releases/latest`, which skips prereleases — so an alpha-only
    /// repo needs [`NexoMod::latest_including_prereleases`] instead.
    pub async fn latest(&self) -> Result<Release> {
        self.resolve(&format!("https://api.github.com/repos/{REPO}/releases/latest"))
            .await
    }

    /// The newest release of any kind, prereleases included. Nexo Mod ships
    /// alpha builds, so this is the one the UI wants.
    pub async fn latest_including_prereleases(&self) -> Result<Release> {
        let releases: Vec<GithubRelease> = self
            .http
            .get(format!("https://api.github.com/repos/{REPO}/releases?per_page=10"))
            .header("Accept", "application/vnd.github+json")
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        // GitHub returns newest first; take the first one carrying both
        // required assets rather than assuming the newest is complete.
        for release in releases {
            if let Ok(resolved) = self.resolve_release(release).await {
                return Ok(resolved);
            }
        }
        Err(Error::invalid(
            "no Nexo Mod release publishes both a jar and a manifest.json yet",
        ))
    }

    async fn resolve(&self, url: &str) -> Result<Release> {
        let release: GithubRelease = self
            .http
            .get(url)
            .header("Accept", "application/vnd.github+json")
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        self.resolve_release(release).await
    }

    async fn resolve_release(&self, release: GithubRelease) -> Result<Release> {
        let manifest_asset = release
            .assets
            .iter()
            .find(|a| a.name == "manifest.json")
            .ok_or_else(|| {
                Error::invalid(format!(
                    "release {} publishes no manifest.json, so what it targets is unknown",
                    release.tag_name
                ))
            })?;

        let jar = release
            .assets
            .iter()
            .find(|a| a.name.ends_with(".jar") && !a.name.ends_with("-sources.jar"))
            .ok_or_else(|| {
                Error::invalid(format!("release {} publishes no jar", release.tag_name))
            })?;

        let manifest: Manifest = self
            .http
            .get(&manifest_asset.browser_download_url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        Ok(Release {
            tag: release.tag_name,
            manifest,
            jar_url: jar.browser_download_url.clone(),
            jar_name: jar.name.clone(),
        })
    }

    /// Downloads and installs `release` into `instance`.
    ///
    /// Fails without writing anything if the instance isn't already on the
    /// loader and Minecraft version the release targets.
    pub async fn install(&self, instance: &mut Instance, release: &Release) -> Result<()> {
        if let Some(reason) = release.manifest.incompatibility(instance) {
            return Err(Error::invalid(format!(
                "{reason} Switch the instance to a supported version first."
            )));
        }

        let bytes = self
            .http
            .get(&release.jar_url)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;

        let mods = self.paths.instance_mods(&instance.id);
        tokio::fs::create_dir_all(&mods).await.ctx(&mods)?;

        // Remove a previously installed build first, or the instance ends up
        // with two Nexo Mod jars and Fabric refuses to start.
        self.remove(instance).await?;

        let destination = mods.join(&release.jar_name);
        tokio::fs::write(&destination, &bytes)
            .await
            .ctx(&destination)?;

        instance.mods.push(InstalledMod {
            project_id: PROJECT_ID.to_string(),
            name: "Nexo Mod".to_string(),
            version_id: release.tag.clone(),
            version_number: release.manifest.mod_version.clone(),
            file_name: release.jar_name.clone(),
            source: ModSource::NexoMod,
            enabled: true,
        });

        Ok(())
    }

    /// Deletes the installed jar and forgets it. Safe to call when nothing is
    /// installed.
    pub async fn remove(&self, instance: &mut Instance) -> Result<()> {
        let mods = self.paths.instance_mods(&instance.id);

        for installed in instance.mods.iter().filter(|m| m.source == ModSource::NexoMod) {
            for name in [
                installed.file_name.clone(),
                // Disabled mods carry the conventional suffix.
                format!("{}.disabled", installed.file_name),
            ] {
                let path = mods.join(name);
                if path.exists() {
                    tokio::fs::remove_file(&path).await.ctx(&path)?;
                }
            }
        }

        instance.mods.retain(|m| m.source != ModSource::NexoMod);
        Ok(())
    }

    /// The installed build, if any.
    pub fn installed<'a>(&self, instance: &'a Instance) -> Option<&'a InstalledMod> {
        instance.mods.iter().find(|m| m.source == ModSource::NexoMod)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(version: &str, loader: &str) -> Manifest {
        Manifest {
            minecraft_version: version.into(),
            loader: loader.into(),
            mod_version: "0.2.0".into(),
        }
    }

    #[test]
    fn supports_only_an_exact_loader_and_version_match() {
        let instance = Instance::new("test", "26.1.2", Loader::Fabric);

        assert!(manifest("26.1.2", "fabric").supports(&instance));
        assert!(manifest("26.1.2", "Fabric").supports(&instance));
        // A different Minecraft version is not "close enough".
        assert!(!manifest("26.2", "fabric").supports(&instance));
        assert!(!manifest("26.1.2", "vanilla").supports(&instance));
        assert!(!manifest("26.1.2", "forge").supports(&instance));
    }

    #[test]
    fn incompatibility_names_both_sides() {
        let instance = Instance::new("test", "26.2", Loader::Fabric);
        let reason = manifest("26.1.2", "fabric").incompatibility(&instance).unwrap();

        assert!(reason.contains("26.1.2"), "should name what the mod targets");
        assert!(reason.contains("26.2"), "should name what the instance is");
        assert!(manifest("26.2", "fabric").incompatibility(&instance).is_none());
    }
}
