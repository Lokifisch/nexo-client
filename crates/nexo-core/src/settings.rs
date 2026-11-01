//! The launcher's global settings: what an instance falls back to when it
//! doesn't name its own Java or memory, and what happens after a launch.

use crate::instance::Instance;
use crate::minecraft::DEFAULT_MEMORY_MB;
use crate::paths::Paths;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

/// A heap the JVM could plausibly start with, in MiB. Anything else came from a
/// hand edit and would only stop the game with a confusing error.
fn plausible_memory(mb: &u32) -> bool {
    (512..=1 << 20).contains(mb)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AfterLaunch {
    #[default]
    KeepOpen,
    Minimize,
    Close,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// `None` is automatic: see [`crate::java::ensure`].
    pub java_path: Option<PathBuf>,
    pub memory_mb: Option<u32>,
    /// As typed, split on whitespace at launch.
    // ponytail: no shell-style quoting, so a value can't contain a space. Use a
    // real tokenizer if somebody needs one.
    pub jvm_args: String,
    pub after_launch: AfterLaunch,
    /// Brings Nexo Mod up to date when an instance is launched.
    pub auto_update_mod: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            java_path: None,
            memory_mb: None,
            jvm_args: String::new(),
            after_launch: AfterLaunch::default(),
            auto_update_mod: true,
        }
    }
}

impl Settings {
    fn file(paths: &Paths) -> PathBuf {
        paths.root().join("settings.json")
    }

    /// Missing or damaged reads as the defaults: nothing here is worth refusing
    /// to start over, and every value can be set again.
    pub fn load(paths: &Paths) -> Self {
        let backup = || std::fs::copy(Self::file(paths), Self::file(paths).with_extension("json.bak"));
        let text = match std::fs::read_to_string(Self::file(paths)) {
            Ok(text) => text,
            Err(err) => {
                if err.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(%err, "could not read settings.json, using the defaults");
                    let _ = backup();
                }
                return Self::default();
            }
        };
        serde_json::from_str(&text).unwrap_or_else(|err| {
            tracing::warn!(%err, "settings.json is damaged, using the defaults");
            // The next save overwrites it, and it may be someone's careful JVM arguments.
            let _ = backup();
            Self::default()
        })
    }

    /// By way of a temporary file, so a crash mid-write can't leave half of one.
    pub fn save(&self, paths: &Paths) -> std::io::Result<()> {
        let file = Self::file(paths);
        let temporary = file.with_extension("json.tmp");
        std::fs::create_dir_all(paths.root())?;
        // Owner-only like the account tokens: JVM arguments can hold a proxy password.
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        options.open(&temporary)?.write_all(&serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&temporary, &file)
    }

    /// An instance's own choice wins; this only fills in what it left open.
    pub fn java_for<'a>(&'a self, instance: &'a Instance) -> Option<&'a Path> {
        instance.java_path.as_deref().or(self.java_path.as_deref())
    }

    pub fn memory_for(&self, instance: &Instance) -> u32 {
        instance
            .memory_mb
            .filter(plausible_memory)
            .or(self.memory_mb.filter(plausible_memory))
            .unwrap_or(DEFAULT_MEMORY_MB)
    }

    pub fn jvm_arg_list(&self) -> Vec<String> {
        self.jvm_args.split_whitespace().map(str::to_string).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance::Loader;

    fn temp_paths() -> Paths {
        Paths::with_root(std::env::temp_dir().join(format!("nexo-test-{}", uuid::Uuid::new_v4())))
    }

    #[test]
    fn saves_and_loads() {
        let paths = temp_paths();
        assert_eq!(Settings::load(&paths), Settings::default());
        let settings = Settings {
            java_path: Some("/opt/jdk/bin/java".into()),
            memory_mb: Some(6144),
            jvm_args: "-XX:+UseG1GC".into(),
            after_launch: AfterLaunch::Minimize,
            auto_update_mod: false,
        };
        settings.save(&paths).unwrap();
        assert_eq!(Settings::load(&paths), settings);
        std::fs::remove_dir_all(paths.root()).unwrap();
    }

    #[test]
    fn damage_means_defaults_and_unknown_keys_are_ignored() {
        let paths = temp_paths();
        std::fs::create_dir_all(paths.root()).unwrap();
        std::fs::write(Settings::file(&paths), "{not json").unwrap();
        assert_eq!(Settings::load(&paths), Settings::default());
        assert_eq!(std::fs::read_to_string(Settings::file(&paths).with_extension("json.bak")).unwrap(), "{not json");
        std::fs::write(Settings::file(&paths), r#"{"memory_mb":2048,"from_the_future":1}"#).unwrap();
        assert_eq!(Settings::load(&paths).memory_mb, Some(2048));
        assert!(Settings::load(&paths).auto_update_mod, "on unless turned off, also in an older file");
        std::fs::remove_dir_all(paths.root()).unwrap();
    }

    #[test]
    fn an_instance_overrides_the_defaults() {
        let settings = Settings { java_path: Some("/global".into()), memory_mb: Some(2048), ..Default::default() };
        let mut instance = Instance::new("x", "26.3", Loader::Fabric);
        assert_eq!(settings.java_for(&instance), Some(Path::new("/global")));
        assert_eq!(settings.memory_for(&instance), 2048);
        instance.java_path = Some("/own".into());
        instance.memory_mb = Some(8192);
        assert_eq!(settings.java_for(&instance), Some(Path::new("/own")));
        assert_eq!(settings.memory_for(&instance), 8192);
        instance.memory_mb = Some(0);
        assert_eq!(settings.memory_for(&instance), 2048, "nonsense is ignored");
        assert_eq!(Settings { memory_mb: Some(u32::MAX), ..Default::default() }.memory_for(&instance), DEFAULT_MEMORY_MB);
        assert_eq!(Settings::default().memory_for(&Instance::new("y", "26.3", Loader::Fabric)), DEFAULT_MEMORY_MB);
    }

    #[test]
    fn arguments_split_on_whitespace() {
        let settings = Settings { jvm_args: "  -XX:+UseG1GC\t-Dfoo=bar \n".into(), ..Default::default() };
        assert_eq!(settings.jvm_arg_list(), ["-XX:+UseG1GC", "-Dfoo=bar"]);
    }
}
