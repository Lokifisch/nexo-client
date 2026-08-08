//! Mojang's piston-meta schema, plus the rule engine that decides which
//! libraries and JVM arguments apply to the machine we're running on.
//!
//! Every version JSON is written for all platforms at once and filtered by
//! `rules` blocks at launch time. Getting that filtering wrong is the classic
//! launcher bug: include a Windows-only native on Linux and the game dies
//! with a linker error that looks nothing like the actual cause.

use serde::Deserialize;
use std::collections::HashMap;

/// Index of every published Minecraft version.
pub const VERSION_MANIFEST_URL: &str =
    "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";

/// Asset objects hang off this host, addressed by hash.
pub const RESOURCES_BASE: &str = "https://resources.download.minecraft.net";

#[derive(Debug, Clone, Deserialize)]
pub struct VersionManifest {
    pub latest: LatestVersions,
    pub versions: Vec<ManifestVersion>,
}

impl VersionManifest {
    pub fn find(&self, id: &str) -> Option<&ManifestVersion> {
        self.versions.iter().find(|v| v.id == id)
    }

    /// Stable releases only, newest first — snapshots would swamp the version
    /// picker.
    pub fn releases(&self) -> impl Iterator<Item = &ManifestVersion> {
        self.versions.iter().filter(|v| v.kind == "release")
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct LatestVersions {
    pub release: String,
    pub snapshot: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ManifestVersion {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    /// Where this version's full JSON lives.
    pub url: String,
    #[serde(default)]
    pub sha1: String,
}

/// A full version JSON. Fabric ships one of these too, with `inheritsFrom`
/// pointing at the vanilla version it layers onto.
#[derive(Debug, Clone, Deserialize)]
pub struct VersionData {
    pub id: String,
    #[serde(rename = "mainClass")]
    pub main_class: String,

    /// Set on loader profiles (Fabric); absent on vanilla.
    #[serde(rename = "inheritsFrom", default)]
    pub inherits_from: Option<String>,

    #[serde(default)]
    pub libraries: Vec<Library>,

    #[serde(default)]
    pub downloads: HashMap<String, Download>,

    #[serde(rename = "assetIndex", default)]
    pub asset_index: Option<AssetIndexRef>,

    #[serde(default)]
    pub assets: Option<String>,

    #[serde(default)]
    pub arguments: Option<Arguments>,

    /// Pre-1.13 single-string argument form. Fabric profiles sometimes still
    /// emit it.
    #[serde(rename = "minecraftArguments", default)]
    pub legacy_arguments: Option<String>,

    #[serde(rename = "javaVersion", default)]
    pub java_version: Option<JavaVersionSpec>,
}

impl VersionData {
    /// Layers a loader profile over the vanilla version it inherits from.
    ///
    /// Order matters in one specific way: the loader's libraries must come
    /// **first** on the classpath, because Fabric ships patched versions of
    /// classes that also exist in vanilla's jars, and the JVM takes the first
    /// match it finds.
    pub fn merge_onto(self, parent: VersionData) -> VersionData {
        let mut libraries = self.libraries;
        libraries.extend(parent.libraries);

        VersionData {
            id: self.id,
            main_class: self.main_class,
            inherits_from: None,
            libraries,
            downloads: if self.downloads.is_empty() {
                parent.downloads
            } else {
                self.downloads
            },
            asset_index: self.asset_index.or(parent.asset_index),
            assets: self.assets.or(parent.assets),
            arguments: match (self.arguments, parent.arguments) {
                (Some(child), Some(base)) => Some(base.extend(child)),
                (Some(child), None) => Some(child),
                (None, base) => base,
            },
            legacy_arguments: self.legacy_arguments.or(parent.legacy_arguments),
            java_version: self.java_version.or(parent.java_version),
        }
    }

    pub fn client_download(&self) -> Option<&Download> {
        self.downloads.get("client")
    }

    /// Libraries that apply to this machine, natives included.
    pub fn active_libraries(&self) -> impl Iterator<Item = &Library> {
        self.libraries.iter().filter(|lib| lib.applies())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct JavaVersionSpec {
    #[serde(rename = "majorVersion")]
    pub major_version: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssetIndexRef {
    pub id: String,
    pub sha1: String,
    pub url: String,
    #[serde(rename = "totalSize", default)]
    pub total_size: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Download {
    pub url: String,
    #[serde(default)]
    pub sha1: String,
    #[serde(default)]
    pub size: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Library {
    pub name: String,
    #[serde(default)]
    pub downloads: Option<LibraryDownloads>,
    #[serde(default)]
    pub rules: Vec<Rule>,
    /// Legacy natives mapping: os name → classifier key, with `${arch}`
    /// substituted at lookup time.
    #[serde(default)]
    pub natives: Option<HashMap<String, String>>,
    /// Fabric-style libraries carry no `downloads` block, just a Maven repo
    /// to resolve `name` against.
    #[serde(default)]
    pub url: Option<String>,
}

impl Library {
    pub fn applies(&self) -> bool {
        rules_allow(&self.rules)
    }

    /// The main jar for this library on this platform.
    pub fn artifact(&self) -> Option<&Artifact> {
        self.downloads.as_ref()?.artifact.as_ref()
    }

    /// The natives jar to extract, if this library has one for this OS.
    pub fn native_artifact(&self) -> Option<&Artifact> {
        let classifier = self.natives.as_ref()?.get(os_name())?;
        let classifier = classifier.replace("${arch}", if cfg!(target_pointer_width = "64") {
            "64"
        } else {
            "32"
        });
        self.downloads.as_ref()?.classifiers.get(&classifier)
    }

    /// Resolves `group:artifact:version` to the Maven-style relative path
    /// Mojang and Fabric both use: `group/with/slashes/artifact/version/artifact-version.jar`.
    pub fn maven_path(&self) -> Option<String> {
        let mut parts = self.name.split(':');
        let group = parts.next()?.replace('.', "/");
        let artifact = parts.next()?;
        let version = parts.next()?;
        // A 4th segment, when present, is a classifier.
        let classifier = parts.next().map(|c| format!("-{c}")).unwrap_or_default();
        Some(format!(
            "{group}/{artifact}/{version}/{artifact}-{version}{classifier}.jar"
        ))
    }

    /// Where to fetch this library from when it has no `downloads` block.
    pub fn maven_url(&self) -> Option<String> {
        let base = self.url.as_deref()?.trim_end_matches('/').to_string();
        Some(format!("{base}/{}", self.maven_path()?))
    }

    /// True for the separate `:natives-linux`-style entries modern versions
    /// use instead of a `natives` map.
    pub fn is_modern_native(&self) -> bool {
        self.name.contains(":natives-")
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct LibraryDownloads {
    #[serde(default)]
    pub artifact: Option<Artifact>,
    #[serde(default)]
    pub classifiers: HashMap<String, Artifact>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Artifact {
    /// Path relative to the shared libraries directory.
    #[serde(default)]
    pub path: String,
    pub url: String,
    #[serde(default)]
    pub sha1: String,
    #[serde(default)]
    pub size: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Arguments {
    #[serde(default)]
    pub game: Vec<Argument>,
    #[serde(default)]
    pub jvm: Vec<Argument>,
}

impl Arguments {
    /// Appends a loader profile's arguments after the base version's.
    fn extend(mut self, child: Arguments) -> Arguments {
        self.game.extend(child.game);
        self.jvm.extend(child.jvm);
        self
    }
}

/// Either a bare string or a conditional block.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Argument {
    Constant(String),
    Conditional {
        #[serde(default)]
        rules: Vec<Rule>,
        value: ArgumentValue,
    },
}

impl Argument {
    /// Yields the argument's tokens if its rules pass on this machine.
    ///
    /// `features` covers optional launcher capabilities (demo mode, custom
    /// window size, quick-play). We enable none of them in v1, so any
    /// argument gated behind a feature is correctly skipped.
    pub fn resolve(&self) -> Vec<String> {
        match self {
            Self::Constant(value) => vec![value.clone()],
            Self::Conditional { rules, value } => {
                if rules_allow_strict(rules) {
                    value.tokens()
                } else {
                    Vec::new()
                }
            }
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ArgumentValue {
    Single(String),
    Many(Vec<String>),
}

impl ArgumentValue {
    fn tokens(&self) -> Vec<String> {
        match self {
            Self::Single(value) => vec![value.clone()],
            Self::Many(values) => values.clone(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    pub action: RuleAction,
    #[serde(default)]
    pub os: Option<OsRule>,
    /// Optional launcher features; presence of any means we skip the
    /// argument, since v1 implements none of them.
    #[serde(default)]
    pub features: Option<HashMap<String, bool>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleAction {
    Allow,
    Disallow,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OsRule {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arch: Option<String>,
    /// A regex against the OS version. Matching it properly would need a
    /// regex engine for one rarely-used rule, so a present `version` is
    /// treated as non-matching — see [`OsRule::matches`].
    #[serde(default)]
    pub version: Option<String>,
}

impl OsRule {
    fn matches(&self) -> bool {
        if let Some(name) = &self.name
            && name != os_name()
        {
            return false;
        }
        if let Some(arch) = &self.arch
            && arch != os_arch()
        {
            return false;
        }
        // In practice `version` only appears on legacy Windows-specific
        // workarounds we don't want anyway, so declining to match is the
        // safe reading.
        self.version.is_none()
    }
}

/// Mojang's rule algorithm: start from "allowed only if unconstrained", then
/// let each matching rule overwrite the verdict. Last match wins.
pub fn rules_allow(rules: &[Rule]) -> bool {
    if rules.is_empty() {
        return true;
    }
    let mut allowed = false;
    for rule in rules {
        let os_ok = rule.os.as_ref().is_none_or(|os| os.matches());
        // Any feature gate counts as unmet — v1 enables no features.
        let features_ok = rule.features.as_ref().is_none_or(|f| f.is_empty());
        if os_ok && features_ok {
            allowed = rule.action == RuleAction::Allow;
        }
    }
    allowed
}

/// Same as [`rules_allow`], but for arguments, where a feature-gated entry
/// must be dropped rather than merely un-matched.
fn rules_allow_strict(rules: &[Rule]) -> bool {
    if rules.iter().any(|r| {
        r.features
            .as_ref()
            .is_some_and(|f| f.values().any(|enabled| *enabled))
    }) {
        return false;
    }
    rules_allow(rules)
}

/// Mojang's name for the current OS.
pub fn os_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "osx"
    } else {
        "linux"
    }
}

/// Mojang's name for the current architecture.
pub fn os_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86" => "x86",
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    }
}

/// One entry in an asset index.
#[derive(Debug, Clone, Deserialize)]
pub struct AssetObject {
    pub hash: String,
    pub size: u64,
}

impl AssetObject {
    /// Assets are stored under the first two characters of their hash, both
    /// remotely and in our cache.
    pub fn relative_path(&self) -> String {
        format!("{}/{}", &self.hash[..2], self.hash)
    }

    pub fn url(&self) -> String {
        format!("{RESOURCES_BASE}/{}", self.relative_path())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssetIndex {
    pub objects: HashMap<String, AssetObject>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os_rule(name: &str, action: RuleAction) -> Rule {
        Rule {
            action,
            os: Some(OsRule {
                name: Some(name.into()),
                arch: None,
                version: None,
            }),
            features: None,
        }
    }

    #[test]
    fn no_rules_means_allowed() {
        assert!(rules_allow(&[]));
    }

    #[test]
    fn allow_only_matching_os() {
        let rules = vec![os_rule(os_name(), RuleAction::Allow)];
        assert!(rules_allow(&rules));

        let other = if os_name() == "linux" { "windows" } else { "linux" };
        assert!(!rules_allow(&[os_rule(other, RuleAction::Allow)]));
    }

    #[test]
    fn later_disallow_overrides_earlier_allow() {
        let rules = vec![
            Rule {
                action: RuleAction::Allow,
                os: None,
                features: None,
            },
            os_rule(os_name(), RuleAction::Disallow),
        ];
        assert!(!rules_allow(&rules));
    }

    #[test]
    fn feature_gated_arguments_are_skipped() {
        let rules = vec![Rule {
            action: RuleAction::Allow,
            os: None,
            features: Some(HashMap::from([("is_demo_user".to_string(), true)])),
        }];
        assert!(!rules_allow_strict(&rules));
    }

    #[test]
    fn maven_path_from_coordinates() {
        let lib = Library {
            name: "net.fabricmc:fabric-loader:0.19.3".into(),
            downloads: None,
            rules: Vec::new(),
            natives: None,
            url: Some("https://maven.fabricmc.net/".into()),
        };
        assert_eq!(
            lib.maven_path().unwrap(),
            "net/fabricmc/fabric-loader/0.19.3/fabric-loader-0.19.3.jar"
        );
        assert_eq!(
            lib.maven_url().unwrap(),
            "https://maven.fabricmc.net/net/fabricmc/fabric-loader/0.19.3/fabric-loader-0.19.3.jar"
        );
    }

    #[test]
    fn loader_libraries_take_classpath_precedence() {
        let parent = VersionData {
            id: "26.1.2".into(),
            main_class: "net.minecraft.client.main.Main".into(),
            inherits_from: None,
            libraries: vec![Library {
                name: "vanilla:lib:1".into(),
                downloads: None,
                rules: vec![],
                natives: None,
                url: None,
            }],
            downloads: HashMap::new(),
            asset_index: None,
            assets: None,
            arguments: None,
            legacy_arguments: None,
            java_version: None,
        };
        let child = VersionData {
            id: "fabric-loader-26.1.2".into(),
            main_class: "net.fabricmc.loader.impl.launch.knot.KnotClient".into(),
            inherits_from: Some("26.1.2".into()),
            libraries: vec![Library {
                name: "fabric:lib:1".into(),
                downloads: None,
                rules: vec![],
                natives: None,
                url: None,
            }],
            downloads: HashMap::new(),
            asset_index: None,
            assets: None,
            arguments: None,
            legacy_arguments: None,
            java_version: None,
        };

        let merged = child.merge_onto(parent);
        assert_eq!(merged.libraries[0].name, "fabric:lib:1");
        assert_eq!(merged.main_class, "net.fabricmc.loader.impl.launch.knot.KnotClient");
    }
}
