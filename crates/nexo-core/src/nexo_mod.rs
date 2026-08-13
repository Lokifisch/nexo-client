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
//!
//! From 0.5.0 a release publishes *two* jars built from one source tree — see
//! [`Edition`]. They declare Fabric `breaks` on each other, so installing one
//! is always a switch: whatever else is in `mods/` comes out first.

use crate::error::{Error, IoContext, Result};
use crate::instance::{Instance, InstalledMod, Loader, ModSource};
use crate::paths::Paths;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// `owner/repo` the releases are fetched from.
const REPO: &str = "Lokifisch/nexo-mod";

/// Synthetic project id, since Nexo Mod isn't published on Modrinth. The
/// `nexo:` prefix keeps it from ever colliding with a real Modrinth id.
///
/// Deliberately *one* id for both editions: they are the same product and
/// only one can be installed, so the instance's content list has one entry
/// either way and the "is it installed / is it current" lookup stays a single
/// question.
pub const PROJECT_ID: &str = "nexo:nexo-mod";

/// Fabric mod id of the Tactical jar.
const TACTICAL_MOD_ID: &str = "nexomod";
/// Fabric mod id of the Legit jar.
const LEGIT_MOD_ID: &str = "nexomod-legit";

/// Which build of Nexo Mod. Both come out of one source tree and ship in the
/// same release.
///
/// The split is about what a server may object to, not about how much of the
/// mod you get: the Legit jar contains nothing that could hand a player
/// information or automation the server didn't send. The two jars declare
/// Fabric `breaks` on each other, so with both in `mods/` the game refuses to
/// start — which is why every path here treats them as mutually exclusive.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "lowercase")]
pub enum Edition {
    /// Everything the mod can do, including features some servers forbid.
    #[default]
    Tactical,
    /// Nothing a server could read as an advantage.
    Legit,
}

impl Edition {
    /// Both editions, in the order the UI offers them.
    pub const ALL: [Edition; 2] = [Edition::Tactical, Edition::Legit];

    /// The key this edition has in a manifest's `editions` map.
    pub fn key(self) -> &'static str {
        match self {
            Self::Tactical => "tactical",
            Self::Legit => "legit",
        }
    }

    /// The name this edition had before 0.5.0, still honoured when reading a
    /// manifest so an older release stays installable.
    pub fn legacy_key(self) -> &'static str {
        match self {
            Self::Tactical => "full",
            Self::Legit => "light",
        }
    }

    /// Parses a manifest key. Unknown keys are `None` rather than an error, so
    /// a future release that adds a third edition still resolves the two this
    /// launcher understands.
    ///
    /// `full` and `light` are what these editions were called before 0.5.0.
    /// They stay accepted because this same parser reads the *instance*
    /// manifest, where an older launcher may have written one — rejecting them
    /// would silently reset a user's recorded edition to the default, and the
    /// launcher would then offer to install what is already there.
    pub fn from_key(key: &str) -> Option<Self> {
        match key.trim().to_ascii_lowercase().as_str() {
            "tactical" | "full" => Some(Self::Tactical),
            "legit" | "light" => Some(Self::Legit),
            _ => None,
        }
    }

    pub fn other(self) -> Self {
        match self {
            Self::Tactical => Self::Legit,
            Self::Legit => Self::Tactical,
        }
    }

    /// Fabric mod id of this edition's jar, for releases whose manifest
    /// doesn't spell it out.
    pub fn default_mod_id(self) -> &'static str {
        match self {
            Self::Tactical => TACTICAL_MOD_ID,
            Self::Legit => LEGIT_MOD_ID,
        }
    }

    /// What choosing this edition means for the user, stated in terms of
    /// server rules rather than features.
    ///
    /// This is the one line the launcher owns instead of taking from the
    /// manifest: it has to stay true no matter which features a release adds
    /// or drops, and it's what makes the choice decidable without knowing the
    /// feature list. Everything feature-shaped comes from the manifest.
    pub fn rules_note(self) -> &'static str {
        match self {
            Self::Tactical => "Includes features some servers count as cheating.",
            Self::Legit => "Holds nothing a server could count as an advantage.",
        }
    }

    /// Which edition a jar file name belongs to, or `None` if it isn't one of
    /// ours. Handles the `.disabled` suffix.
    ///
    /// Used for instances installed before the edition was recorded, and to
    /// sweep `mods/` for jars nothing is tracking.
    pub fn from_file_name(file_name: &str) -> Option<Self> {
        let stem = jar_stem(file_name)?;
        // Longest base first, or `nexomod` would swallow `nexomod-legit`.
        //
        // `nexomod-light` is the name the Legit jar shipped under before 0.5.0.
        // It stays recognised so that switching editions still deletes it: an
        // unrecognised jar would be left in `mods/` next to the new one, and
        // the two declare Fabric `breaks` on each other, so the game would
        // refuse to start.
        for (edition, base) in [
            (Self::Legit, LEGIT_MOD_ID),
            (Self::Legit, "nexomod-light"),
            (Self::Tactical, TACTICAL_MOD_ID),
        ] {
            let Some(rest) = stem.strip_prefix(base) else {
                continue;
            };
            // Only a version may follow the base name. Without this,
            // `nexomod-addon-1.0.jar` — someone else's mod — would be read as
            // ours and deleted on the next install.
            let is_ours = rest.is_empty()
                || rest
                    .strip_prefix('-')
                    .is_some_and(|v| v.starts_with(|c: char| c.is_ascii_digit()));
            return is_ours.then_some(edition);
        }
        None
    }
}

impl std::fmt::Display for Edition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Tactical => "Tactical",
            Self::Legit => "Legit",
        })
    }
}

/// Lowercased jar stem with any `.disabled` suffix stripped, or `None` for
/// something that isn't a jar at all.
fn jar_stem(file_name: &str) -> Option<String> {
    let lower = file_name.to_ascii_lowercase();
    let base = lower.strip_suffix(".disabled").unwrap_or(&lower);
    base.strip_suffix(".jar").map(str::to_string)
}

/// Whether a file in `mods/` is one of Nexo Mod's own jars — either edition,
/// any version, enabled or disabled.
fn is_nexo_jar(file_name: &str) -> bool {
    Edition::from_file_name(file_name).is_some()
}

/// Reads `Option<Edition>` without letting an unknown value fail the whole
/// document.
///
/// This sits on the *instance* manifest, which a newer launcher may have
/// written. A hard parse error there doesn't produce a warning — it makes the
/// instance vanish from the list, since [`crate::instance::InstanceStore::list`]
/// skips anything it can't read.
pub(crate) fn deserialize_edition<'de, D>(deserializer: D) -> std::result::Result<Option<Edition>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = Option::<String>::deserialize(deserializer)?;
    Ok(raw.as_deref().and_then(Edition::from_key))
}

/// One edition as the manifest declares it.
#[derive(Debug, Clone, Deserialize)]
pub struct EditionInfo {
    /// Exact release asset name. This is what makes resolution unambiguous —
    /// picking "the first .jar" out of a two-jar release is a coin flip.
    pub file: String,
    #[serde(default)]
    pub mod_id: Option<String>,
    /// Display label, e.g. "Full".
    #[serde(default)]
    pub name: Option<String>,
    /// Prose for the picker. Lives in the manifest so the launcher doesn't
    /// carry its own copy to drift out of date.
    #[serde(default)]
    pub description: Option<String>,
}

/// What a published release declares about itself.
#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub minecraft_version: String,
    pub loader: String,
    pub mod_version: String,

    /// Which edition to preselect. Absent on releases from before the split.
    ///
    /// A `String` rather than an [`Edition`] on purpose: an unrecognised value
    /// falls back to the default instead of failing the whole manifest and
    /// making the release uninstallable.
    #[serde(default)]
    pub default_edition: Option<String>,

    /// Declared editions keyed by `"tactical"` / `"legit"`. Empty on pre-0.5.0
    /// releases, which published exactly one jar and named it nowhere — see
    /// [`assemble`] for how those still resolve.
    #[serde(default)]
    pub editions: BTreeMap<String, EditionInfo>,
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
        self.loader() == Some(instance.loader) && self.minecraft_version == instance.game_version
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

    /// True for a manifest that names its jars, i.e. 0.5.0 and later.
    pub fn declares_editions(&self) -> bool {
        !self.editions.is_empty()
    }

    /// Falls back to the pre-0.5.0 spelling, because [`Edition::from_key`]
    /// accepts it and a lookup that didn't would make that acceptance a lie:
    /// `preferred_edition` would resolve `"full"` to Tactical and then this
    /// would fail to find Tactical's asset.
    pub fn edition(&self, edition: Edition) -> Option<&EditionInfo> {
        self.editions
            .get(edition.key())
            .or_else(|| self.editions.get(edition.legacy_key()))
    }

    /// The edition the manifest wants preselected.
    pub fn preferred_edition(&self) -> Edition {
        self.default_edition
            .as_deref()
            .and_then(Edition::from_key)
            .unwrap_or_default()
    }
}

/// One downloadable jar, resolved from a declared edition to a real asset.
#[derive(Debug, Clone)]
pub struct ReleaseEdition {
    pub edition: Edition,
    /// Display label from the manifest, falling back to the edition's name.
    pub name: String,
    /// Prose from the manifest. `None` on pre-0.5.0 releases, which said
    /// nothing about editions at all.
    pub description: Option<String>,
    /// Fabric mod id that will end up in the instance.
    pub mod_id: String,
    pub jar_name: String,
    url: String,
}

/// A release, resolved to the manifest and every edition it actually
/// publishes.
#[derive(Debug, Clone)]
pub struct Release {
    pub tag: String,
    pub manifest: Manifest,
    /// Never empty — [`assemble`] fails rather than producing a release with
    /// nothing to install.
    editions: Vec<ReleaseEdition>,
}

impl Release {
    pub fn version(&self) -> &str {
        &self.manifest.mod_version
    }

    /// Everything installable in this release, in [`Edition::ALL`] order.
    pub fn editions(&self) -> &[ReleaseEdition] {
        &self.editions
    }

    pub fn edition(&self, edition: Edition) -> Option<&ReleaseEdition> {
        self.editions.iter().find(|e| e.edition == edition)
    }

    /// Whether there is actually a choice to present.
    pub fn offers_a_choice(&self) -> bool {
        self.editions.len() > 1
    }

    /// The edition to preselect: what the manifest asks for when this release
    /// really publishes it, otherwise whatever it does publish.
    pub fn default_edition(&self) -> Edition {
        let preferred = self.manifest.preferred_edition();
        if self.edition(preferred).is_some() {
            return preferred;
        }
        self.editions
            .first()
            .map(|e| e.edition)
            .unwrap_or_default()
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

/// Turns a manifest plus a release's asset list into a [`Release`].
///
/// Two shapes have to work:
///
/// * **0.5.0 and later** declare `editions`, and each one names its asset.
///   The name is matched exactly. A declared asset that isn't in the release
///   is an error naming the missing file — falling back to "some other jar"
///   would install an edition the user didn't ask for, which is precisely the
///   bug the declaration exists to prevent.
/// * **0.4.0-alpha and earlier** declare nothing. The single non-sources jar
///   is the whole product and is treated as [`Edition::Tactical`], exactly as it
///   was before editions existed. Releases already on GitHub have to stay
///   installable.
///
/// Kept free of IO so both shapes can be tested against real manifests.
fn assemble(tag: String, manifest: Manifest, assets: &[GithubAsset]) -> Result<Release> {
    let editions = if manifest.declares_editions() {
        for key in manifest.editions.keys() {
            if Edition::from_key(key).is_none() {
                // Newer than us. Ignored rather than fatal, so a release that
                // adds an edition doesn't take the known ones down with it.
                tracing::warn!(edition = %key, release = %tag, "manifest declares an edition this launcher doesn't know");
            }
        }

        let mut resolved = Vec::new();
        for edition in Edition::ALL {
            let Some(info) = manifest.edition(edition) else {
                continue;
            };
            let asset = assets.iter().find(|a| a.name == info.file).ok_or_else(|| {
                Error::invalid(format!(
                    "release {tag} declares its {edition} edition as '{}', but publishes no asset by that name",
                    info.file
                ))
            })?;

            resolved.push(ReleaseEdition {
                edition,
                name: info.name.clone().unwrap_or_else(|| edition.to_string()),
                description: info.description.clone(),
                mod_id: info
                    .mod_id
                    .clone()
                    .unwrap_or_else(|| edition.default_mod_id().to_string()),
                jar_name: asset.name.clone(),
                url: asset.browser_download_url.clone(),
            });
        }
        resolved
    } else {
        let jar = assets
            .iter()
            .find(|a| a.name.ends_with(".jar") && !a.name.ends_with("-sources.jar"))
            .ok_or_else(|| Error::invalid(format!("release {tag} publishes no jar")))?;

        vec![ReleaseEdition {
            edition: Edition::Tactical,
            name: Edition::Tactical.to_string(),
            description: None,
            mod_id: TACTICAL_MOD_ID.to_string(),
            jar_name: jar.name.clone(),
            url: jar.browser_download_url.clone(),
        }]
    };

    if editions.is_empty() {
        return Err(Error::invalid(format!(
            "release {tag} declares no edition this launcher understands"
        )));
    }

    Ok(Release {
        tag,
        manifest,
        editions,
    })
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
        self.resolve(&format!(
            "https://api.github.com/repos/{REPO}/releases/latest"
        ))
        .await
    }

    /// The newest release of any kind, prereleases included. Nexo Mod ships
    /// alpha builds, so this is the one the UI wants.
    pub async fn latest_including_prereleases(&self) -> Result<Release> {
        let releases: Vec<GithubRelease> = self
            .http
            .get(format!(
                "https://api.github.com/repos/{REPO}/releases?per_page=10"
            ))
            .header("Accept", "application/vnd.github+json")
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        // GitHub returns newest first; take the first one carrying both
        // required assets rather than assuming the newest is complete. A
        // release whose manifest promises a jar it never uploaded counts as
        // incomplete and is skipped with a warning, not silently patched up
        // with whichever jar did make it.
        for release in releases {
            let tag = release.tag_name.clone();
            match self.resolve_release(release).await {
                Ok(resolved) => return Ok(resolved),
                Err(err) => tracing::warn!(%tag, %err, "skipping an unusable Nexo Mod release"),
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

        let manifest: Manifest = self
            .http
            .get(&manifest_asset.browser_download_url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        assemble(release.tag_name.clone(), manifest, &release.assets)
    }

    /// Downloads and installs one edition of `release` into `instance`.
    ///
    /// Fails without writing anything if the instance isn't already on the
    /// loader and Minecraft version the release targets, or if the release
    /// doesn't publish the edition asked for.
    ///
    /// Installing is always a switch: the other edition is removed first, and
    /// so is any Nexo Mod jar lying in `mods/` untracked.
    pub async fn install(
        &self,
        instance: &mut Instance,
        release: &Release,
        edition: Edition,
    ) -> Result<()> {
        if let Some(reason) = release.manifest.incompatibility(instance) {
            return Err(Error::invalid(format!(
                "{reason} Switch the instance to a supported version first."
            )));
        }

        let build = release.edition(edition).ok_or_else(|| {
            Error::invalid(format!(
                "release {} doesn't publish a {edition} build of Nexo Mod",
                release.tag
            ))
        })?;

        let bytes = self
            .http
            .get(&build.url)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;

        self.place(instance, release, build, &bytes).await
    }

    /// The disk half of [`NexoMod::install`], split out so the switch
    /// behaviour is testable without a download.
    async fn place(
        &self,
        instance: &mut Instance,
        release: &Release,
        build: &ReleaseEdition,
        bytes: &[u8],
    ) -> Result<()> {
        let mods = self.paths.instance_mods(&instance.id);
        tokio::fs::create_dir_all(&mods).await.ctx(&mods)?;

        // Remove first, always. The two editions declare `breaks` on each
        // other, so a leftover jar isn't a duplicate — it's a game that
        // won't start.
        self.remove(instance).await?;

        let destination = mods.join(&build.jar_name);
        tokio::fs::write(&destination, bytes)
            .await
            .ctx(&destination)?;

        instance.mods.push(InstalledMod {
            project_id: PROJECT_ID.to_string(),
            name: format!("Nexo Mod ({})", build.name),
            version_id: release.tag.clone(),
            version_number: release.manifest.mod_version.clone(),
            file_name: build.jar_name.clone(),
            source: ModSource::NexoMod,
            enabled: true,
            edition: Some(build.edition),
        });

        Ok(())
    }

    /// Deletes every Nexo Mod jar and forgets them. Safe to call when nothing
    /// is installed.
    ///
    /// Sweeps the folder as well as the tracked list: a jar can be in `mods/`
    /// without the instance knowing — put there by an older launcher build,
    /// copied in by hand, or left behind by a switch that died halfway. Any
    /// one of them stops the game from starting once a second Nexo Mod jar
    /// lands beside it, so removal can't be limited to what we wrote.
    pub async fn remove(&self, instance: &mut Instance) -> Result<()> {
        let mods = self.paths.instance_mods(&instance.id);

        let mut names: BTreeSet<String> = instance
            .mods
            .iter()
            .filter(|m| m.source == ModSource::NexoMod)
            .map(|m| m.file_name.clone())
            .collect();

        if mods.exists() {
            let mut entries = tokio::fs::read_dir(&mods).await.ctx(&mods)?;
            while let Some(entry) = entries.next_entry().await.ctx(&mods)? {
                let name = entry.file_name().to_string_lossy().into_owned();
                if is_nexo_jar(&name) {
                    names.insert(name);
                }
            }
        }

        for name in names {
            for candidate in [name.clone(), format!("{name}.disabled")] {
                let path = mods.join(candidate);
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
        installed(instance)
    }
}

/// The installed build, if any. Free function so the UI can ask without
/// holding a [`NexoMod`].
pub fn installed(instance: &Instance) -> Option<&InstalledMod> {
    instance.mods.iter().find(|m| m.source == ModSource::NexoMod)
}

/// Which edition is installed, if any.
///
/// Falls back to reading the jar name for entries written before the edition
/// was recorded — an instance set up by an older launcher build says nothing
/// about editions, but its file name still does.
pub fn installed_edition(instance: &Instance) -> Option<Edition> {
    let installed = installed(instance)?;
    Some(
        installed
            .edition
            .or_else(|| Edition::from_file_name(&installed.file_name))
            .unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exactly what `Lokifisch/nexo-mod` publishes as the v0.4.0-alpha
    /// manifest.json — fetched from the release, not written from memory.
    const LEGACY_MANIFEST: &str = r#"{
	"minecraft_version": "26.1.2",
	"loader": "fabric",
	"mod_version": "0.4.0"
}"#;

    /// The two-edition shape from 0.5.0 on.
    const EDITION_MANIFEST: &str = r#"{
      "mod_version": "0.5.0",
      "minecraft_version": "26.1.2",
      "loader": "fabric",
      "default_edition": "tactical",
      "editions": {
        "tactical": { "file": "nexomod-0.5.0.jar",       "mod_id": "nexomod",       "name": "Tactical", "description": "Every feature, including the ones some servers forbid." },
        "legit":    { "file": "nexomod-legit-0.5.0.jar", "mod_id": "nexomod-legit", "name": "Legit",    "description": "Nothing a server could read as an advantage." }
      }
    }"#;

    fn manifest_from(json: &str) -> Manifest {
        serde_json::from_str(json).expect("manifest fixture should parse")
    }

    fn assets(names: &[&str]) -> Vec<GithubAsset> {
        let json: String = names
            .iter()
            .map(|name| {
                format!(
                    r#"{{"name":"{name}","browser_download_url":"https://example.invalid/{name}"}}"#
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        serde_json::from_str(&format!("[{json}]")).expect("asset fixture should parse")
    }

    fn manifest(version: &str, loader: &str) -> Manifest {
        manifest_from(&format!(
            r#"{{"minecraft_version":"{version}","loader":"{loader}","mod_version":"0.2.0"}}"#
        ))
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
        let reason = manifest("26.1.2", "fabric")
            .incompatibility(&instance)
            .unwrap();

        assert!(reason.contains("26.1.2"), "should name what the mod targets");
        assert!(reason.contains("26.2"), "should name what the instance is");
        assert!(manifest("26.2", "fabric")
            .incompatibility(&instance)
            .is_none());
    }

    #[test]
    fn resolves_each_edition_to_the_asset_the_manifest_names() {
        // Legit listed first, so "the first jar" would pick the wrong one.
        let assets = assets(&[
            "manifest.json",
            "nexomod-legit-0.5.0.jar",
            "nexomod-0.5.0.jar",
            "nexomod-0.5.0-sources.jar",
        ]);
        let release =
            assemble("v0.5.0".into(), manifest_from(EDITION_MANIFEST), &assets).unwrap();

        assert!(release.offers_a_choice());
        assert_eq!(release.default_edition(), Edition::Tactical);

        let tactical = release.edition(Edition::Tactical).unwrap();
        assert_eq!(tactical.jar_name, "nexomod-0.5.0.jar");
        assert_eq!(tactical.mod_id, "nexomod");
        assert_eq!(tactical.name, "Tactical");
        assert!(tactical.description.is_some(), "prose comes from the manifest");

        let legit = release.edition(Edition::Legit).unwrap();
        assert_eq!(legit.jar_name, "nexomod-legit-0.5.0.jar");
        assert_eq!(legit.mod_id, "nexomod-legit");

        // Order is the UI's, not the release's.
        assert_eq!(
            release.editions().iter().map(|e| e.edition).collect::<Vec<_>>(),
            vec![Edition::Tactical, Edition::Legit]
        );
    }

    #[test]
    fn a_manifest_without_editions_still_resolves_its_single_jar() {
        let assets = assets(&["manifest.json", "nexomod-0.4.0.jar"]);
        let release =
            assemble("v0.4.0-alpha".into(), manifest_from(LEGACY_MANIFEST), &assets).unwrap();

        assert!(!release.manifest.declares_editions());
        assert!(!release.offers_a_choice(), "nothing to choose between");
        assert_eq!(release.default_edition(), Edition::Tactical);

        let only = release.edition(Edition::Tactical).unwrap();
        assert_eq!(only.jar_name, "nexomod-0.4.0.jar");
        assert_eq!(only.mod_id, TACTICAL_MOD_ID);
        assert!(release.edition(Edition::Legit).is_none());
    }

    #[test]
    fn a_declared_asset_that_is_missing_is_an_error_naming_the_file() {
        // The light jar never finished uploading.
        let assets = assets(&["manifest.json", "nexomod-0.5.0.jar"]);
        let err = assemble("v0.5.0".into(), manifest_from(EDITION_MANIFEST), &assets)
            .expect_err("a half-published release must not resolve");

        let message = err.to_string();
        assert!(
            message.contains("nexomod-legit-0.5.0.jar"),
            "the error has to name the missing file, got: {message}"
        );
        // The full jar is right there; taking it would install an edition the
        // user never picked.
        assert!(!message.contains("no jar"), "got: {message}");
    }

    #[test]
    fn an_unknown_edition_key_is_ignored_rather_than_fatal() {
        let manifest = manifest_from(
            r#"{
              "mod_version": "0.9.0",
              "minecraft_version": "26.1.2",
              "loader": "fabric",
              "default_edition": "server",
              "editions": {
                "full":   { "file": "nexomod-0.9.0.jar" },
                "server": { "file": "nexomod-server-0.9.0.jar" }
              }
            }"#,
        );
        let assets = assets(&["manifest.json", "nexomod-0.9.0.jar"]);
        let release = assemble("v0.9.0".into(), manifest, &assets).unwrap();

        assert_eq!(release.editions().len(), 1);
        // The unresolvable default falls back to what is actually published.
        assert_eq!(release.default_edition(), Edition::Tactical);
    }

    #[test]
    fn edition_is_read_off_a_jar_name_only_for_our_own_jars() {
        assert_eq!(
            Edition::from_file_name("nexomod-0.4.0.jar"),
            Some(Edition::Tactical)
        );
        assert_eq!(
            Edition::from_file_name("nexomod-light-0.5.0.jar"),
            Some(Edition::Legit)
        );
        assert_eq!(
            Edition::from_file_name("NexoMod-Light-0.5.0.Jar.disabled"),
            Some(Edition::Legit)
        );
        // Someone else's mod that merely starts the same way.
        assert_eq!(Edition::from_file_name("nexomod-addon-1.0.jar"), None);
        assert_eq!(Edition::from_file_name("nexomodular-2.jar"), None);
        assert_eq!(Edition::from_file_name("sodium-0.6.jar"), None);
        assert_eq!(Edition::from_file_name("nexomod-0.4.0.zip"), None);
    }

    async fn temp_instance() -> (Paths, Instance) {
        let temp = std::env::temp_dir().join(format!("nexo-test-{}", uuid::Uuid::new_v4()));
        let paths = Paths::with_root(&temp);
        paths.ensure().await.unwrap();
        let instance = Instance::new("edition-test", "26.1.2", Loader::Fabric);
        let mods = paths.instance_mods(&instance.id);
        tokio::fs::create_dir_all(&mods).await.unwrap();
        (paths, instance)
    }

    fn jars_in(paths: &Paths, instance: &Instance) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(paths.instance_mods(&instance.id))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[tokio::test]
    async fn installing_the_other_edition_switches_instead_of_stacking() {
        let (paths, mut instance) = temp_instance().await;
        let nexo = NexoMod::new(reqwest::Client::new(), paths.clone());

        let release = assemble(
            "v0.5.0".into(),
            manifest_from(EDITION_MANIFEST),
            &assets(&[
                "manifest.json",
                "nexomod-0.5.0.jar",
                "nexomod-legit-0.5.0.jar",
            ]),
        )
        .unwrap();

        let full = release.edition(Edition::Tactical).unwrap();
        nexo.place(&mut instance, &release, full, b"full").await.unwrap();
        assert_eq!(jars_in(&paths, &instance), vec!["nexomod-0.5.0.jar"]);
        assert_eq!(installed_edition(&instance), Some(Edition::Tactical));

        let legit = release.edition(Edition::Legit).unwrap();
        nexo.place(&mut instance, &release, legit, b"legit").await.unwrap();

        // The whole point: one jar, the one that was asked for.
        assert_eq!(jars_in(&paths, &instance), vec!["nexomod-legit-0.5.0.jar"]);
        assert_eq!(installed_edition(&instance), Some(Edition::Legit));
        assert_eq!(
            instance.mods.iter().filter(|m| m.source == ModSource::NexoMod).count(),
            1,
            "the content list must not grow an entry per switch"
        );

        tokio::fs::remove_dir_all(paths.root()).await.ok();
    }

    #[tokio::test]
    async fn a_switch_clears_jars_the_instance_never_tracked() {
        let (paths, mut instance) = temp_instance().await;
        let nexo = NexoMod::new(reqwest::Client::new(), paths.clone());
        let mods = paths.instance_mods(&instance.id);

        // The state that breaks the game: both jars present, neither of them
        // in the instance's content list — an older launcher build, or a jar
        // dropped in by hand.
        for name in [
            "nexomod-0.4.0.jar",
            "nexomod-light-0.4.0.jar.disabled",
            "sodium-0.6.jar",
        ] {
            tokio::fs::write(mods.join(name), b"x").await.unwrap();
        }

        let release = assemble(
            "v0.5.0".into(),
            manifest_from(EDITION_MANIFEST),
            &assets(&[
                "manifest.json",
                "nexomod-0.5.0.jar",
                "nexomod-legit-0.5.0.jar",
            ]),
        )
        .unwrap();
        let legit = release.edition(Edition::Legit).unwrap();
        nexo.place(&mut instance, &release, legit, b"legit").await.unwrap();

        assert_eq!(
            jars_in(&paths, &instance),
            vec!["nexomod-legit-0.5.0.jar", "sodium-0.6.jar"],
            "both stale Nexo jars go, everyone else's mods stay"
        );

        tokio::fs::remove_dir_all(paths.root()).await.ok();
    }

    #[tokio::test]
    async fn remove_takes_both_editions_and_leaves_other_mods_alone() {
        let (paths, mut instance) = temp_instance().await;
        let nexo = NexoMod::new(reqwest::Client::new(), paths.clone());
        let mods = paths.instance_mods(&instance.id);

        for name in [
            "nexomod-0.5.0.jar",
            "nexomod-legit-0.5.0.jar",
            "iris-1.8.jar",
        ] {
            tokio::fs::write(mods.join(name), b"x").await.unwrap();
        }
        instance.mods.push(InstalledMod {
            project_id: PROJECT_ID.to_string(),
            name: "Nexo Mod (Full)".into(),
            version_id: "v0.5.0".into(),
            version_number: "0.5.0".into(),
            file_name: "nexomod-0.5.0.jar".into(),
            source: ModSource::NexoMod,
            enabled: true,
            edition: Some(Edition::Tactical),
        });

        nexo.remove(&mut instance).await.unwrap();

        assert_eq!(jars_in(&paths, &instance), vec!["iris-1.8.jar"]);
        assert!(installed(&instance).is_none());

        tokio::fs::remove_dir_all(paths.root()).await.ok();
    }

    #[test]
    fn an_instance_from_an_older_launcher_still_reports_its_edition() {
        let mut instance = Instance::new("legacy", "26.1.2", Loader::Fabric);
        // No `edition` field — written before editions existed.
        instance.mods.push(InstalledMod {
            project_id: PROJECT_ID.to_string(),
            name: "Nexo Mod".into(),
            version_id: "v0.4.0-alpha".into(),
            version_number: "0.4.0".into(),
            file_name: "nexomod-0.4.0.jar".into(),
            source: ModSource::NexoMod,
            enabled: true,
            edition: None,
        });

        assert_eq!(installed_edition(&instance), Some(Edition::Tactical));
    }

    #[test]
    fn an_instance_manifest_from_a_newer_launcher_still_loads() {
        // An edition this build doesn't know must not take the whole instance
        // document down — an unreadable instance disappears from the list.
        let json = r#"{
          "id": "future",
          "name": "Future",
          "game_version": "26.1.2",
          "loader": "fabric",
          "created_at": 0,
          "mods": [{
            "project_id": "nexo:nexo-mod",
            "name": "Nexo Mod (Server)",
            "version_id": "v9.0.0",
            "version_number": "9.0.0",
            "file_name": "nexomod-server-9.0.0.jar",
            "source": "nexo_mod",
            "enabled": true,
            "edition": "server"
          }]
        }"#;

        let instance: Instance = serde_json::from_str(json).expect("must still parse");
        assert_eq!(installed(&instance).unwrap().edition, None);
    }
}
