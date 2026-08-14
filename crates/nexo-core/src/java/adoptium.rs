//! Installing a JVM when the machine hasn't got a new enough one.
//!
//! Requiring people to go and install a JDK before the launcher will start a
//! game is the kind of step a "client" is supposed to remove, so Nexo fetches
//! one itself from [Eclipse Temurin](https://adoptium.net) — the same builds
//! the AUR package suggests as an optdepend.
//!
//! Two rules shape everything here:
//!
//! * **A working system JVM is never displaced.** [`super::resolve`] only
//!   reaches this after discovery has come up empty, and an instance with an
//!   explicit `java_path` never reaches it at all.
//! * **A half-extracted runtime must never be found.** Extraction goes to a
//!   staging directory and is renamed into place only once it has been probed
//!   and actually runs. Otherwise an interrupted download leaves something
//!   that looks installed and fails at launch, which is far harder to
//!   diagnose than nothing being there.

use super::{JavaInstall, probe};
use crate::error::{Error, IoContext, Result};
use crate::minecraft::Progress;
use crate::paths::Paths;
use crate::util::sha256_hex;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc::UnboundedSender;

const API: &str = "https://api.adoptium.net/v3";

/// A JRE rather than a full JDK: Minecraft and its mod loaders only need a
/// runtime, and it is roughly a quarter of the download.
const IMAGE_TYPE: &str = "jre";

/// Adoptium's names for the running platform, or `None` where it publishes
/// no build.
fn platform() -> Option<(&'static str, &'static str)> {
    let os = match std::env::consts::OS {
        "linux" => "linux",
        "windows" => "windows",
        "macos" => "mac",
        _ => return None,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "aarch64",
        _ => return None,
    };
    Some((os, arch))
}

#[derive(Debug, Deserialize)]
struct Asset {
    binary: Binary,
    release_name: String,
}

#[derive(Debug, Deserialize)]
struct Binary {
    package: Package,
}

#[derive(Debug, Deserialize)]
struct Package {
    name: String,
    link: String,
    /// SHA-256, hex. Adoptium publishes it for every package.
    #[serde(default)]
    checksum: String,
}

/// A JVM Nexo installed itself, if one is already present and usable.
///
/// Probes rather than trusting the directory listing: a runtime whose files
/// were partly deleted, or one left by an older Nexo that no longer runs,
/// must not be offered as though it worked.
pub async fn installed(paths: &Paths, major: u32) -> Option<JavaInstall> {
    let root = paths.java_runtimes();
    let mut entries = tokio::fs::read_dir(&root).await.ok()?;

    while let Ok(Some(entry)) = entries.next_entry().await {
        // Staging directories are in-progress extractions, never candidates.
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let Some(binary) = find_java_binary(&entry.path()) else {
            continue;
        };
        if let Some(install) = probe(&binary).await
            && install.major >= major
        {
            return Some(install);
        }
    }
    None
}

/// Every JVM Nexo manages, whatever their version. For the picker, which
/// should show what is there rather than only what is new enough.
pub async fn all_installed(paths: &Paths) -> Vec<JavaInstall> {
    let mut found = Vec::new();
    let Ok(mut entries) = tokio::fs::read_dir(paths.java_runtimes()).await else {
        return found;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if let Some(binary) = find_java_binary(&entry.path())
            && let Some(install) = probe(&binary).await
        {
            found.push(install);
        }
    }
    found.sort_by_key(|j| std::cmp::Reverse(j.major));
    found
}

/// Returns a JVM of at least `major`, downloading one if necessary.
///
/// Reuses an already-downloaded runtime when there is one, so this is cheap to
/// call on every launch.
pub async fn ensure(
    http: &reqwest::Client,
    paths: &Paths,
    major: u32,
    progress: Option<&UnboundedSender<Progress>>,
) -> Result<JavaInstall> {
    if let Some(existing) = installed(paths, major).await {
        return Ok(existing);
    }

    let (os, arch) = platform().ok_or_else(|| {
        Error::invalid(format!(
            "no Java build is published for {}-{}, so one has to be installed by hand",
            std::env::consts::OS,
            std::env::consts::ARCH
        ))
    })?;

    let stage = |label: &str| {
        if let Some(tx) = progress {
            let _ = tx.send(Progress::Stage(label.to_string()));
        }
    };

    stage(&format!("Looking up Java {major}"));
    let url = format!(
        "{API}/assets/latest/{major}/hotspot?architecture={arch}&image_type={IMAGE_TYPE}&os={os}&vendor=eclipse"
    );
    let assets: Vec<Asset> = http
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let asset = assets.into_iter().next().ok_or_else(|| {
        Error::invalid(format!(
            "Adoptium publishes no Java {major} {IMAGE_TYPE} for {os}-{arch}"
        ))
    })?;

    stage(&format!("Downloading Java {major}"));
    let bytes = http
        .get(&asset.binary.package.link)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;

    // Adoptium publishes a digest for every package. Treating a missing one as
    // acceptable would make the check trivially skippable by anything sitting
    // between us and them, and this archive becomes an executable.
    let expected = asset.binary.package.checksum.trim().to_ascii_lowercase();
    if expected.is_empty() {
        return Err(Error::invalid(format!(
            "Adoptium published no checksum for {}, so it can't be verified",
            asset.binary.package.name
        )));
    }
    let actual = sha256_hex(&bytes);
    if actual != expected {
        return Err(Error::invalid(format!(
            "the downloaded {} doesn't match Adoptium's checksum (expected {expected}, got {actual}). \
             Nothing was installed.",
            asset.binary.package.name
        )));
    }

    stage(&format!("Installing Java {major}"));
    let root = paths.java_runtimes();
    tokio::fs::create_dir_all(&root).await.ctx(&root)?;

    // Staged under a dot name so `installed` skips it while it fills up.
    let staging = root.join(format!(".staging-{}", std::process::id()));
    let _ = tokio::fs::remove_dir_all(&staging).await;
    tokio::fs::create_dir_all(&staging).await.ctx(&staging)?;

    let archive_name = asset.binary.package.name.clone();
    let target = staging.clone();
    // Extraction is CPU- and syscall-bound and the archive is ~45 MB, which is
    // long enough to visibly stall the UI's runtime if it ran inline.
    let extracted = tokio::task::spawn_blocking(move || extract(&bytes, &archive_name, &target))
        .await
        .map_err(|e| Error::invalid(format!("unpacking Java panicked: {e}")))?;

    if let Err(err) = extracted {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(err);
    }

    let binary = find_java_binary(&staging).ok_or_else(|| {
        Error::invalid("the downloaded Java archive contains no bin/java".to_string())
    })?;

    // Probed *before* it is moved into place, so a runtime that cannot run
    // never becomes something `installed` will hand out later.
    let Some(install) = probe(&binary).await else {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(Error::invalid(
            "the downloaded Java runtime doesn't run on this machine".to_string(),
        ));
    };

    let final_dir = root.join(safe_dir_name(&asset.release_name));
    let _ = tokio::fs::remove_dir_all(&final_dir).await;
    tokio::fs::rename(&staging, &final_dir)
        .await
        .ctx(&final_dir)?;

    // The path changed under it, so re-derive rather than returning the
    // staging path the probe was run against.
    let binary = find_java_binary(&final_dir)
        .ok_or_else(|| Error::invalid("the installed Java runtime moved out from under us"))?;

    tracing::info!(version = %install.version, path = %binary.display(), "installed a Java runtime");
    Ok(JavaInstall {
        path: binary,
        ..install
    })
}

/// Keeps a release name from turning into a path. Adoptium's are tame
/// (`jdk-25.0.1+9`), but this writes a directory from a remote string.
fn safe_dir_name(release: &str) -> String {
    let cleaned: String = release
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+' | '_') {
                c
            } else {
                '-'
            }
        })
        .collect();
    // Dots are kept because real release names contain them, which leaves `.`
    // and `..` able to survive the filter intact — and `join("..")` climbs out
    // of the runtimes directory.
    if cleaned.is_empty() || cleaned.chars().all(|c| c == '.') {
        "runtime".to_string()
    } else {
        cleaned
    }
}

fn extract(bytes: &[u8], archive_name: &str, dest: &Path) -> Result<()> {
    if archive_name.ends_with(".zip") {
        extract_zip(bytes, dest)
    } else {
        extract_tar_gz(bytes, dest)
    }
}

fn extract_zip(bytes: &[u8], dest: &Path) -> Result<()> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        // Rejects `../` traversal — nothing may be written outside `dest`.
        let Some(name) = entry.enclosed_name() else {
            continue;
        };
        let out = dest.join(&name);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).ctx(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).ctx(parent)?;
        }
        let mut writer = std::fs::File::create(&out).ctx(&out)?;
        std::io::copy(&mut entry, &mut writer).ctx(&out)?;
    }
    Ok(())
}

fn extract_tar_gz(bytes: &[u8], dest: &Path) -> Result<()> {
    let decoder = flate2::read::GzDecoder::new(std::io::Cursor::new(bytes));
    let mut archive = tar::Archive::new(decoder);
    // Modes have to survive: without the executable bit on `bin/java` the
    // runtime extracts perfectly and then cannot be started.
    archive.set_preserve_permissions(true);
    // `tar`'s unpack refuses entries that would escape the destination.
    archive.unpack(dest).ctx(dest)?;
    Ok(())
}

/// Finds `bin/java` under `root`, however deeply the archive nests it.
///
/// Archives wrap everything in a versioned directory, and macOS adds a
/// `Contents/Home` layer on top, so the location is not fixed. The search is
/// depth-limited because this walks a freshly extracted tree.
fn find_java_binary(root: &Path) -> Option<PathBuf> {
    fn walk(dir: &Path, depth: usize) -> Option<PathBuf> {
        if depth == 0 {
            return None;
        }
        let candidate = dir
            .join("bin")
            .join(if cfg!(windows) { "java.exe" } else { "java" });
        if candidate.is_file() {
            return Some(candidate);
        }
        for entry in std::fs::read_dir(dir).ok()?.flatten() {
            if entry.file_type().ok()?.is_dir()
                && let Some(found) = walk(&entry.path(), depth - 1)
            {
                return Some(found);
            }
        }
        None
    }
    walk(root, 4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_names_cannot_become_paths() {
        assert_eq!(safe_dir_name("jdk-25.0.1+9"), "jdk-25.0.1+9");

        // Separators are what makes a name a path, so those are what has to
        // go; the dots may stay because real release names are full of them.
        assert_eq!(safe_dir_name("../../etc/cron.d/x"), "..-..-etc-cron.d-x");
        assert!(!safe_dir_name("a/../../b").contains(['/', '\\']));

        // Which leaves the one case dots alone can still escape with.
        for climb in ["..", ".", "..."] {
            assert_eq!(
                safe_dir_name(climb),
                "runtime",
                "{climb} would resolve outside the runtimes directory"
            );
        }
        assert_eq!(safe_dir_name(""), "runtime");
    }

    #[test]
    fn this_platform_has_a_published_build() {
        // Every platform Nexo itself ships for must be one Adoptium builds,
        // or auto-install silently becomes unavailable exactly where it is
        // needed most.
        assert!(
            platform().is_some(),
            "no Adoptium mapping for {}-{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        );
    }

    #[test]
    fn a_binary_is_found_through_the_wrapper_directory() {
        let root = std::env::temp_dir().join(format!("nexo-java-find-{}", std::process::id()));
        let bin = root.join("jdk-25.0.1+9-jre").join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let exe = bin.join(if cfg!(windows) { "java.exe" } else { "java" });
        std::fs::write(&exe, b"#!/bin/false\n").unwrap();

        let found = find_java_binary(&root);
        std::fs::remove_dir_all(&root).ok();
        assert_eq!(found.as_deref(), Some(exe.as_path()));
    }

    #[test]
    fn an_archive_without_a_runtime_is_not_mistaken_for_one() {
        let root = std::env::temp_dir().join(format!("nexo-java-empty-{}", std::process::id()));
        std::fs::create_dir_all(root.join("jdk-25").join("lib")).unwrap();
        let found = find_java_binary(&root);
        std::fs::remove_dir_all(&root).ok();
        assert_eq!(found, None);
    }
}
