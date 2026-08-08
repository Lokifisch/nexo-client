//! Turning resolved version data into an actual `java` invocation.
//!
//! Mojang's version JSON stores argument lists with `${placeholder}` tokens
//! that the launcher fills in. Substituting them as whole tokens — never by
//! string-splicing a joined command line — is what keeps a username or
//! directory containing spaces from silently breaking the launch.

use super::install::Installer;
use super::meta::VersionData;
use crate::auth::Account;
use crate::error::{IoContext, Result};
use crate::instance::Instance;
use crate::paths::Paths;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::process::{Child, Command};

/// Reported to the game (and to servers) as the launcher's identity.
const LAUNCHER_NAME: &str = "nexo";
const LAUNCHER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Default heap when neither the instance nor global settings say otherwise.
pub const DEFAULT_MEMORY_MB: u32 = 4096;

#[derive(Debug, Clone)]
pub struct LaunchOptions {
    pub account: Account,
    pub java: PathBuf,
    pub memory_mb: u32,
    /// Appended verbatim after the generated JVM arguments.
    pub extra_jvm_args: Vec<String>,
}

pub struct Launcher {
    paths: Paths,
    installer: Installer,
}

impl Launcher {
    pub fn new(paths: Paths, installer: Installer) -> Self {
        Self { paths, installer }
    }

    /// Assembles the full command line without running it. Separated from
    /// [`Launcher::launch`] so it can be unit-tested and shown to the user
    /// for debugging.
    pub fn build_command(
        &self,
        instance: &Instance,
        version: &VersionData,
        options: &LaunchOptions,
    ) -> Result<Command> {
        let vars = self.variables(instance, version, options);

        let mut command = Command::new(&options.java);
        // Working directory is the instance folder, so the game writes
        // saves/config/screenshots inside it rather than into a shared root.
        command.current_dir(self.paths.instance(&instance.id));

        // Heap first so a version-supplied -Xmx can still override it.
        command.arg(format!("-Xmx{}M", options.memory_mb));
        command.arg(format!("-Djava.library.path={}", vars["natives_directory"]));

        for token in self.jvm_arguments(version) {
            command.arg(substitute(&token, &vars));
        }
        for arg in &options.extra_jvm_args {
            command.arg(arg);
        }

        command.arg(&version.main_class);

        for token in self.game_arguments(version) {
            command.arg(substitute(&token, &vars));
        }

        Ok(command)
    }

    /// Installs anything missing, then starts the game.
    pub async fn launch(
        &self,
        instance: &Instance,
        version: &VersionData,
        options: &LaunchOptions,
    ) -> Result<Child> {
        let dir = self.paths.instance(&instance.id);
        tokio::fs::create_dir_all(&dir).await.ctx(&dir)?;

        let mut command = self.build_command(instance, version, options)?;
        // Inherit stdio rather than piping: the caller is free to drop the
        // returned handle once the game is up, and a dropped `Child` closes
        // its pipes — which would hand the JVM a broken stdout mid-run. An
        // in-app log console will need a reader task draining the pipes
        // before this can become `piped()`.
        command
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(false);

        Ok(command.spawn()?)
    }

    /// Every library jar plus the client jar, in classpath order.
    fn classpath(&self, instance: &Instance, version: &VersionData) -> String {
        let mut entries: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for library in version.active_libraries() {
            // Natives are extracted, not put on the classpath.
            if library.is_modern_native() {
                continue;
            }
            if let Some(path) = self.installer.library_path(library) {
                let path = path.to_string_lossy().to_string();
                if seen.insert(path.clone()) {
                    entries.push(path);
                }
            }
        }

        // Vanilla's jar goes last: loader-patched classes must win.
        entries.push(
            self.installer
                .client_jar(&instance.game_version)
                .to_string_lossy()
                .to_string(),
        );

        entries.join(classpath_separator())
    }

    fn variables(
        &self,
        instance: &Instance,
        version: &VersionData,
        options: &LaunchOptions,
    ) -> HashMap<String, String> {
        let assets_index = version
            .asset_index
            .as_ref()
            .map(|a| a.id.clone())
            .or_else(|| version.assets.clone())
            .unwrap_or_else(|| "legacy".to_string());

        HashMap::from([
            (
                "auth_player_name".to_string(),
                options.account.username.clone(),
            ),
            ("version_name".to_string(), version.id.clone()),
            (
                "game_directory".to_string(),
                self.paths
                    .instance(&instance.id)
                    .to_string_lossy()
                    .to_string(),
            ),
            (
                "assets_root".to_string(),
                self.paths.assets().to_string_lossy().to_string(),
            ),
            (
                "game_assets".to_string(),
                self.paths.assets().to_string_lossy().to_string(),
            ),
            ("assets_index_name".to_string(), assets_index),
            ("auth_uuid".to_string(), options.account.uuid.clone()),
            (
                "auth_access_token".to_string(),
                options.account.access_token.clone(),
            ),
            (
                "auth_session".to_string(),
                format!("token:{}", options.account.access_token),
            ),
            // Microsoft accounts only; "mojang"/"legacy" are dead.
            ("user_type".to_string(), "msa".to_string()),
            ("version_type".to_string(), "release".to_string()),
            ("user_properties".to_string(), "{}".to_string()),
            ("clientid".to_string(), String::new()),
            ("auth_xuid".to_string(), String::new()),
            (
                "natives_directory".to_string(),
                self.installer
                    .natives_dir(&instance.game_version)
                    .to_string_lossy()
                    .to_string(),
            ),
            (
                "library_directory".to_string(),
                self.paths.libraries().to_string_lossy().to_string(),
            ),
            ("launcher_name".to_string(), LAUNCHER_NAME.to_string()),
            (
                "launcher_version".to_string(),
                LAUNCHER_VERSION.to_string(),
            ),
            (
                "classpath_separator".to_string(),
                classpath_separator().to_string(),
            ),
            ("classpath".to_string(), self.classpath(instance, version)),
        ])
    }

    fn jvm_arguments(&self, version: &VersionData) -> Vec<String> {
        match &version.arguments {
            Some(arguments) => arguments.jvm.iter().flat_map(|a| a.resolve()).collect(),
            // Pre-1.13 versions declare no JVM arguments at all, so the
            // classpath has to be supplied by hand.
            None => vec!["-cp".to_string(), "${classpath}".to_string()],
        }
    }

    fn game_arguments(&self, version: &VersionData) -> Vec<String> {
        if let Some(arguments) = &version.arguments {
            return arguments.game.iter().flat_map(|a| a.resolve()).collect();
        }
        version
            .legacy_arguments
            .as_deref()
            .map(|raw| raw.split_whitespace().map(str::to_string).collect())
            .unwrap_or_default()
    }
}

/// Replaces every `${name}` in one argument token.
fn substitute(token: &str, vars: &HashMap<String, String>) -> String {
    let mut out = token.to_string();
    // Only scan when there's something to replace — most tokens are literal.
    if !out.contains("${") {
        return out;
    }
    for (key, value) in vars {
        let needle = format!("${{{key}}}");
        if out.contains(&needle) {
            out = out.replace(&needle, value);
        }
    }
    out
}

pub fn classpath_separator() -> &'static str {
    if cfg!(windows) { ";" } else { ":" }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars() -> HashMap<String, String> {
        HashMap::from([
            ("auth_player_name".to_string(), "Player One".to_string()),
            ("game_directory".to_string(), "/home/a b/inst".to_string()),
        ])
    }

    #[test]
    fn substitutes_placeholders_within_a_token() {
        assert_eq!(substitute("${auth_player_name}", &vars()), "Player One");
        assert_eq!(
            substitute("--dir=${game_directory}", &vars()),
            "--dir=/home/a b/inst"
        );
    }

    #[test]
    fn leaves_literals_and_unknown_placeholders_alone() {
        assert_eq!(substitute("--demo", &vars()), "--demo");
        assert_eq!(substitute("${nope}", &vars()), "${nope}");
    }
}
