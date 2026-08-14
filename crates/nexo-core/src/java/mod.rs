//! Locating a JVM to launch the game with.
//!
//! Modern Minecraft is strict about Java major version, so "any `java` on
//! PATH" is not good enough — a too-old JVM fails with an unhelpful
//! `UnsupportedClassVersionError`. We probe candidates, read each one's real
//! version, and only accept ones that clear the game's floor.

pub mod adoptium;

use crate::error::{Error, Result};
use crate::minecraft::Progress;
use crate::paths::Paths;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc::UnboundedSender;

/// Minimum Java major version for the Minecraft versions v1 targets.
/// `Mod/README.md` lists Java 25+ for MC 26.1.2.
pub const MIN_JAVA_MAJOR: u32 = 25;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JavaInstall {
    pub path: PathBuf,
    pub major: u32,
    /// Full version string as reported by the JVM, e.g. `25.0.1`.
    pub version: String,
}

/// Finds every usable JVM, best (highest version) first.
pub async fn discover() -> Vec<JavaInstall> {
    let mut found: Vec<JavaInstall> = Vec::new();

    for candidate in candidates().await {
        if found.iter().any(|j| j.path == candidate) {
            continue;
        }
        if let Some(install) = probe(&candidate).await {
            found.push(install);
        }
    }

    found.sort_by_key(|j| std::cmp::Reverse(j.major));
    found.dedup_by(|a, b| a.path == b.path);
    found
}

/// The best JVM that meets [`MIN_JAVA_MAJOR`], if any.
pub async fn find_suitable() -> Option<JavaInstall> {
    discover()
        .await
        .into_iter()
        .find(|j| j.major >= MIN_JAVA_MAJOR)
}

/// Runs `java -version` and parses what comes back. Returns `None` for
/// anything that isn't a working JVM, so callers can throw candidate paths
/// at this freely.
pub async fn probe(java: &Path) -> Option<JavaInstall> {
    // Discovery probes every candidate path, so on Windows this is the spawn
    // that ran most often — and each one flashed its own console window.
    let output = crate::util::no_window_async(tokio::process::Command::new(java).arg("-version"))
        .output()
        .await
        .ok()?;

    // `java -version` writes to stderr, not stdout — a long-standing quirk.
    let text = String::from_utf8_lossy(&output.stderr);
    let version = parse_version(&text)?;
    let major = parse_major(&version)?;

    Some(JavaInstall {
        path: java.to_path_buf(),
        major,
        version,
    })
}

/// Pulls `25.0.1` out of `openjdk version "25.0.1" 2025-10-21`.
fn parse_version(output: &str) -> Option<String> {
    let line = output.lines().next()?;
    let start = line.find('"')? + 1;
    let rest = &line[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Handles both modern (`25.0.1` → 25) and legacy (`1.8.0_402` → 8) schemes.
fn parse_major(version: &str) -> Option<u32> {
    let mut parts = version.split(['.', '_', '-']);
    let first: u32 = parts.next()?.parse().ok()?;
    if first == 1 {
        parts.next()?.parse().ok()
    } else {
        Some(first)
    }
}

/// Every path worth probing on this platform.
async fn candidates() -> Vec<PathBuf> {
    let exe = if cfg!(windows) { "java.exe" } else { "java" };
    let mut paths = Vec::new();

    if let Ok(java_home) = std::env::var("JAVA_HOME") {
        paths.push(PathBuf::from(java_home).join("bin").join(exe));
    }

    // Whatever `java` resolves to on PATH.
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join(exe);
            if candidate.exists() {
                paths.push(candidate);
            }
        }
    }

    // Distribution-standard install roots, each holding one directory per
    // installed JDK.
    let roots: &[&str] = if cfg!(target_os = "windows") {
        &[
            r"C:\Program Files\Java",
            r"C:\Program Files\Eclipse Adoptium",
            r"C:\Program Files\Microsoft\jdk",
            r"C:\Program Files\Zulu",
        ]
    } else if cfg!(target_os = "macos") {
        &["/Library/Java/JavaVirtualMachines"]
    } else {
        &["/usr/lib/jvm", "/usr/lib64/jvm", "/opt/java"]
    };

    for root in roots {
        let Ok(mut entries) = tokio::fs::read_dir(root).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let base = entry.path();
            // macOS buries the binary an extra two levels down.
            for suffix in [
                PathBuf::from("bin").join(exe),
                PathBuf::from("Contents").join("Home").join("bin").join(exe),
            ] {
                let candidate = base.join(&suffix);
                if candidate.exists() {
                    paths.push(candidate);
                }
            }
        }
    }

    paths
}

/// Turns "no suitable JVM" into a message that says what to do about it.
pub fn missing_java_error() -> Error {
    Error::invalid(format!(
        "no Java {MIN_JAVA_MAJOR}+ runtime is available and one couldn't be downloaded — \
         install Temurin {MIN_JAVA_MAJOR} yourself, or point the instance at an existing one"
    ))
}

/// Resolves the JVM to launch with, without installing anything.
///
/// Kept separate from [`ensure`] so callers that must not touch the network —
/// or must not silently download 45 MB — have a way to ask.
pub async fn resolve(instance_override: Option<&Path>) -> Result<JavaInstall> {
    if let Some(path) = instance_override {
        return probe(path).await.ok_or_else(|| {
            Error::invalid(format!("{} is not a working Java runtime", path.display()))
        });
    }
    find_suitable().await.ok_or_else(missing_java_error)
}

/// Resolves the JVM to launch with, downloading one if the machine has none
/// new enough.
///
/// Order matters and is deliberate:
///
/// 1. **The instance's own setting**, if it has one. An explicit choice is
///    never second-guessed and never silently replaced — if it is broken, that
///    is an error, not a reason to go and fetch something else.
/// 2. **A system JVM.** Anything already installed and new enough wins over
///    downloading, so Nexo doesn't accumulate a copy of what the machine
///    already has.
/// 3. **A runtime Nexo manages**, reused if present and downloaded if not.
pub async fn ensure(
    http: &reqwest::Client,
    paths: &Paths,
    instance_override: Option<&Path>,
    progress: Option<&UnboundedSender<Progress>>,
) -> Result<JavaInstall> {
    if let Some(path) = instance_override {
        return probe(path).await.ok_or_else(|| {
            Error::invalid(format!("{} is not a working Java runtime", path.display()))
        });
    }

    if let Some(system) = find_suitable().await {
        return Ok(system);
    }

    adoptium::ensure(http, paths, MIN_JAVA_MAJOR, progress).await
}

/// Every JVM the user could pick for an instance: the ones on this machine,
/// then the ones Nexo downloaded, newest first and without duplicates.
pub async fn options(paths: &Paths) -> Vec<JavaInstall> {
    let mut all = discover().await;
    for managed in adoptium::all_installed(paths).await {
        if !all.iter().any(|j| j.path == managed.path) {
            all.push(managed);
        }
    }
    all.sort_by_key(|j| std::cmp::Reverse(j.major));
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modern_and_legacy_versions() {
        assert_eq!(parse_major("25.0.1"), Some(25));
        assert_eq!(parse_major("21"), Some(21));
        assert_eq!(parse_major("1.8.0_402"), Some(8));
    }

    #[test]
    fn extracts_version_from_java_output() {
        let output = "openjdk version \"25.0.1\" 2025-10-21\nOpenJDK Runtime Environment";
        assert_eq!(parse_version(output).as_deref(), Some("25.0.1"));
    }

    #[test]
    fn rejects_output_without_quoted_version() {
        assert_eq!(parse_version("command not found"), None);
    }
}
