//! Keeping the launcher itself up to date.
//!
//! # What a release has to publish
//!
//! One raw executable per platform, named for the platform rather than the
//! version, plus a `SHA256SUMS` in `sha256sum` format listing them:
//!
//! ```text
//! nexo-linux-x86_64
//! nexo-windows-x86_64.exe
//! nexo-macos-aarch64
//! SHA256SUMS
//! ```
//!
//! The name carries no version on purpose — [`asset_name`] has to be able to
//! compute it without knowing what the newest release is called.
//!
//! A Windows *installer* may ship alongside these, but it is for first-time
//! installs. Updating replaces the executable directly, so the update path is
//! one mechanism on every platform instead of one per packaging format.
//!
//! # What this deliberately does not do
//!
//! **It never updates an install it doesn't own.** A copy under `/usr` belongs
//! to whichever package manager put it there; overwriting it corrupts that
//! package's file list and the next system upgrade quietly reverts the update.
//! [`ownership`] classifies the install first and the whole path refuses
//! rather than guessing — see [`Ownership::Blocked`].
//!
//! **It never downgrades.** Running a build newer than the last release is
//! normal while developing and is not a state to be repaired.
//!
//! **It never applies anything unverified.** `SHA256SUMS` is required, not
//! preferred: this writes an executable that the user will run, and "the
//! release forgot its checksums" is not a good enough reason to skip checking.

use crate::error::{Error, IoContext, Result};
use crate::github;
use crate::util::sha256_hex;
use semver::Version;
use std::path::{Path, PathBuf};

/// `owner/repo` the launcher's own releases come from. Not the mod's repo —
/// see [`crate::nexo_mod`] for that one.
const REPO: &str = "Lokifisch/nexo-client";

/// The version this build reports. Shared across the workspace, so it is the
/// same number `nexo-app` was compiled with.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// Checksum manifest asset name, in `sha256sum` output format.
const CHECKSUMS: &str = "SHA256SUMS";

/// The release asset holding a build for the running platform, or `None` on a
/// platform Nexo publishes nothing for.
///
/// `EXE_SUFFIX` rather than a literal `.exe` so the Windows name can't drift
/// away from what the binary is actually called.
pub fn asset_name() -> Option<String> {
    let os = match std::env::consts::OS {
        "linux" => "linux",
        "windows" => "windows",
        "macos" => "macos",
        _ => return None,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        _ => return None,
    };
    Some(format!("nexo-{os}-{arch}{}", std::env::consts::EXE_SUFFIX))
}

/// Whether this install is one the launcher may replace in place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ownership {
    /// Nexo owns its binary and can swap it.
    Replaceable,
    /// It must not, with the reason stated for the user. Every message names
    /// what to do instead — a refusal with no alternative just looks broken.
    Blocked(String),
}

impl Ownership {
    pub fn is_replaceable(&self) -> bool {
        matches!(self, Self::Replaceable)
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Replaceable => None,
            Self::Blocked(why) => Some(why),
        }
    }
}

/// Classifies where the running binary lives.
///
/// Checked in order of confidence: an explicit opt-out beats a path guess,
/// and a path guess beats a permission probe, because a packager running as
/// root would otherwise pass the probe and corrupt their own package.
pub fn ownership(exe: &Path) -> Ownership {
    // Distro packagers build with `NEXO_SELF_UPDATE=off`. This is the
    // supported way to turn the updater off and it outranks everything else,
    // including a binary that happens to sit somewhere writable.
    if option_env!("NEXO_SELF_UPDATE") == Some("off") {
        return Ownership::Blocked(
            "This build was packaged with its updater disabled. Update Nexo the same way you installed it."
                .into(),
        );
    }

    // A cargo build directory is writable and replacing the binary there
    // would "work", but the next `cargo build` undoes it — which looks like
    // the update silently failing.
    let mut components = exe.components().rev().skip(1);
    if let (Some(profile), Some(target)) = (components.next(), components.next())
        && matches!(
            profile.as_os_str().to_str(),
            Some("debug") | Some("release")
        )
        && target.as_os_str() == "target"
    {
        return Ownership::Blocked(
            "Nexo is running out of a Cargo build directory. `cargo build --release` is the update here.".into(),
        );
    }

    // A system prefix means a package manager put it there. Its file list
    // would no longer match what's on disk, and the next system upgrade
    // reinstalls the packaged version over the update without saying so.
    #[cfg(unix)]
    if exe.starts_with("/usr") || exe.starts_with("/opt") {
        return Ownership::Blocked(
            "Nexo was installed system-wide, so updating itself would fight the package it came from. Update it through your package manager.".into(),
        );
    }

    let Some(dir) = exe.parent() else {
        return Ownership::Blocked("Nexo can't tell where its own binary lives.".into());
    };

    // Permission bits and ACLs both lie often enough that the only honest
    // test is to try. The probe file is created and removed immediately.
    if !can_write_in(dir) {
        return Ownership::Blocked(format!(
            "Nexo can't write to {}, so it can't replace itself there.",
            dir.display()
        ));
    }

    Ownership::Replaceable
}

fn can_write_in(dir: &Path) -> bool {
    let probe = dir.join(format!(".nexo-update-probe-{}", std::process::id()));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// A newer release than the one running, resolved to a downloadable build.
#[derive(Debug, Clone)]
pub struct Update {
    pub version: Version,
    /// The release's git tag, which is what the user sees on GitHub.
    pub tag: String,
    /// Release notes as published, for the UI to show before updating.
    pub notes: Option<String>,
    pub url: String,
    /// Download size in bytes, so the UI can say how long this will take.
    pub size: u64,
    /// Where this release's `SHA256SUMS` lives. Resolved during the check
    /// rather than at install time: it costs no extra request there, and an
    /// update that couldn't be verified is better never offered than offered
    /// and refused after the download.
    checksums_url: String,
    /// Whether this install may be replaced. Carried along so the UI has one
    /// value to hold and can't offer a button that is guaranteed to fail.
    pub install: Ownership,
}

impl Update {
    /// Download size rounded to whole MB — enough precision for a progress
    /// label and never a misleading "0 MB" for a real file.
    pub fn size_mb(&self) -> u64 {
        self.size.div_ceil(1024 * 1024)
    }
}

#[derive(Debug, Clone)]
pub struct SelfUpdate {
    http: reqwest::Client,
}

impl SelfUpdate {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }

    /// The newest published release that is actually newer than this build,
    /// or `None` when already current.
    ///
    /// Callers should treat an error here as unremarkable: with no
    /// authentication GitHub allows 60 requests an hour per IP, so a failed
    /// check is a normal outcome and not something to interrupt the user
    /// over. Only a check the user asked for should surface it.
    pub async fn check(&self) -> Result<Option<Update>> {
        let current = Version::parse(CURRENT).map_err(|err| {
            Error::invalid(format!(
                "this build's own version ({CURRENT}) isn't valid semver: {err}"
            ))
        })?;
        let wanted = asset_name().ok_or_else(|| {
            Error::invalid(format!(
                "Nexo publishes no build for {}-{}",
                std::env::consts::OS,
                std::env::consts::ARCH
            ))
        })?;

        let exe = current_exe()?;

        for release in github::releases(&self.http, REPO, 10).await? {
            let Some(version) = parse_tag(&release.tag_name) else {
                tracing::warn!(tag = %release.tag_name, "ignoring a release whose tag isn't a version");
                continue;
            };

            // Releases arrive newest first, so once one is not newer than us,
            // no later one can be either.
            if version <= current {
                return Ok(None);
            }

            // A release that shipped only some platforms must not hide the
            // one before it that shipped all of them, so this keeps looking
            // rather than giving up on the newest tag.
            let Some((url, size)) = release
                .asset(&wanted)
                .map(|a| (a.browser_download_url.clone(), a.size))
            else {
                tracing::warn!(tag = %release.tag_name, asset = %wanted, "release publishes no build for this platform");
                continue;
            };

            // No checksums, no offer. Skipping here rather than failing in
            // `apply` means an incomplete release falls back to the last good
            // one instead of leaving a button that always errors.
            let Some(checksums_url) = release
                .asset(CHECKSUMS)
                .map(|a| a.browser_download_url.clone())
            else {
                tracing::warn!(tag = %release.tag_name, "release publishes no {CHECKSUMS}, so it can't be verified");
                continue;
            };

            return Ok(Some(Update {
                version,
                tag: release.tag_name,
                notes: release.body.filter(|b| !b.trim().is_empty()),
                url,
                size,
                checksums_url,
                install: ownership(&exe),
            }));
        }

        Ok(None)
    }

    /// Downloads `update`, checks it against the release's `SHA256SUMS`, and
    /// puts it in place of the running binary.
    ///
    /// Returns once the new binary is installed. The running process is still
    /// the old one — it has to be restarted, which is the caller's to prompt.
    pub async fn apply(&self, update: &Update) -> Result<()> {
        let exe = current_exe()?;
        if let Ownership::Blocked(why) = ownership(&exe) {
            return Err(Error::Invalid(why));
        }

        let wanted = asset_name()
            .ok_or_else(|| Error::invalid("this platform has no published Nexo build"))?;

        // Fetched before the binary: if the release can't vouch for what's
        // about to be downloaded, there is no reason to download it.
        let sums = self
            .http
            .get(&update.checksums_url)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        let expected = checksum_for(&sums, &wanted).ok_or_else(|| {
            Error::invalid(format!(
                "{CHECKSUMS} in release {} doesn't list {wanted}",
                update.tag
            ))
        })?;

        let bytes = self
            .http
            .get(&update.url)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;

        let actual = sha256_hex(&bytes);
        if actual != expected {
            return Err(Error::invalid(format!(
                "the downloaded {wanted} doesn't match the checksum {REPO} published for it \
                 (expected {expected}, got {actual}). Nothing was installed."
            )));
        }

        // Staged in the destination directory, not a temp dir: the swap below
        // is a rename, and a rename is only atomic within one filesystem.
        let staged = exe.with_file_name(format!(
            "{}.new",
            exe.file_name().unwrap_or_default().to_string_lossy()
        ));
        tokio::fs::write(&staged, &bytes).await.ctx(&staged)?;

        if let Err(err) = swap(&staged, &exe) {
            let _ = tokio::fs::remove_file(&staged).await;
            return Err(err);
        }
        tracing::info!(version = %update.version, "replaced the running binary; restart to use it");
        Ok(())
    }
}

/// Removes the copy of itself Windows forced the last update to leave behind.
///
/// A no-op everywhere else, and safe to call unconditionally at startup: if
/// the file is somehow still locked, it stays and the next start tries again.
pub fn clear_previous() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let leftover = exe.with_file_name(format!(
        "{}.old",
        exe.file_name().unwrap_or_default().to_string_lossy()
    ));
    if leftover.exists() && std::fs::remove_file(&leftover).is_ok() {
        tracing::debug!(path = %leftover.display(), "cleaned up the previous version");
    }
}

fn current_exe() -> Result<PathBuf> {
    std::env::current_exe()
        .map_err(|err| Error::invalid(format!("Nexo can't locate its own binary: {err}")))
}

/// Parses a release tag into a version. Accepts a leading `v`, which is how
/// every Nexo tag is written, and nothing else — a tag that isn't a version
/// is skipped rather than guessed at.
fn parse_tag(tag: &str) -> Option<Version> {
    Version::parse(tag.strip_prefix('v').unwrap_or(tag)).ok()
}

/// Pulls one file's digest out of `sha256sum` output.
///
/// Handles both the text (`hash  name`) and binary (`hash *name`) forms, since
/// which one appears depends on how the release job invoked `sha256sum`.
fn checksum_for(sums: &str, asset: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, name) = line.trim().split_once(char::is_whitespace)?;
        let name = name.trim().trim_start_matches('*');
        (name == asset).then(|| hash.trim().to_ascii_lowercase())
    })
}

/// Puts `staged` where `target` is.
///
/// A running executable can't simply be written over on either platform, so
/// both arms go through a rename — atomic within a directory, which is why
/// `apply` stages next to the target rather than in a temp directory.
#[cfg(unix)]
fn swap(staged: &Path, target: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    // Executable *before* the rename, not after: in between, the file is
    // already the installed binary, and a crash in that window would leave a
    // Nexo behind that the desktop entry can no longer start.
    std::fs::set_permissions(staged, std::fs::Permissions::from_mode(0o755)).ctx(staged)?;
    std::fs::rename(staged, target).ctx(target)?;
    Ok(())
}

#[cfg(windows)]
fn swap(staged: &Path, target: &Path) -> Result<()> {
    // Windows won't let a running .exe be deleted or overwritten, but it will
    // let it be *renamed* — the open handle follows the file. So the running
    // binary moves aside, the new one takes its name, and the leftover goes
    // at the next start via `clear_previous`.
    let aside = target.with_file_name(format!(
        "{}.old",
        target.file_name().unwrap_or_default().to_string_lossy()
    ));
    let _ = std::fs::remove_file(&aside);
    std::fs::rename(target, &aside).ctx(target)?;

    if let Err(err) = std::fs::rename(staged, target) {
        // Put the running binary back before reporting: leaving the install
        // with no `nexo.exe` at all is far worse than a failed update.
        let _ = std::fs::rename(&aside, target);
        return Err(Error::Io {
            path: target.to_path_buf(),
            source: err,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_names_are_platform_specific_and_unversioned() {
        let name = asset_name().expect("the test platform should have a published build");
        assert!(name.starts_with("nexo-"), "{name}");
        assert!(
            !name.contains(CURRENT),
            "the name has to be computable without knowing the release version: {name}"
        );
        #[cfg(windows)]
        assert!(name.ends_with(".exe"), "{name}");
        #[cfg(not(windows))]
        assert!(!name.ends_with(".exe"), "{name}");
    }

    #[test]
    fn tags_parse_with_or_without_the_v() {
        assert_eq!(parse_tag("v0.2.0"), Some(Version::new(0, 2, 0)));
        assert_eq!(parse_tag("0.2.0"), Some(Version::new(0, 2, 0)));
        assert!(parse_tag("v0.2.0-alpha").unwrap().pre.as_str() == "alpha");
        assert_eq!(parse_tag("nightly"), None);
    }

    /// The ordering that decides whether an update is offered at all. A
    /// prerelease sorting *above* its final would offer 0.2.0-alpha to
    /// someone already running 0.2.0.
    #[test]
    fn a_prerelease_is_older_than_its_release() {
        assert!(parse_tag("v0.2.0-alpha").unwrap() < parse_tag("v0.2.0").unwrap());
        assert!(parse_tag("v0.1.0").unwrap() < parse_tag("v0.2.0-alpha").unwrap());
    }

    #[test]
    fn checksums_parse_in_both_sha256sum_forms() {
        let sums = "\
d2f4a1  nexo-linux-x86_64
BEEF01 *nexo-windows-x86_64.exe
";
        assert_eq!(
            checksum_for(sums, "nexo-linux-x86_64"),
            Some("d2f4a1".into())
        );
        assert_eq!(
            checksum_for(sums, "nexo-windows-x86_64.exe"),
            Some("beef01".into()),
            "digests are compared lowercase, so parsing has to normalise"
        );
        assert_eq!(checksum_for(sums, "nexo-macos-aarch64"), None);
    }

    /// A prefix match here would accept the digest of a different file — the
    /// Windows build's line starts with the Linux build's name.
    #[test]
    fn checksum_lookup_is_not_a_prefix_match() {
        let sums = "aaaa  nexo-linux-x86_64-musl\nbbbb  nexo-linux-x86_64\n";
        assert_eq!(checksum_for(sums, "nexo-linux-x86_64"), Some("bbbb".into()));
    }

    #[test]
    fn a_cargo_build_directory_is_never_replaced() {
        let exe = PathBuf::from("/home/someone/Code/Nexoclient/Client/target/release/nexo");
        let verdict = ownership(&exe);
        assert!(!verdict.is_replaceable());
        assert!(
            verdict.reason().unwrap().contains("Cargo"),
            "the reason has to say what to do instead: {:?}",
            verdict.reason()
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_package_managed_install_is_never_replaced() {
        for path in ["/usr/bin/nexo", "/usr/local/bin/nexo", "/opt/nexo/nexo"] {
            let verdict = ownership(Path::new(path));
            assert!(!verdict.is_replaceable(), "{path} should be refused");
            assert!(
                verdict.reason().unwrap().contains("package manager"),
                "{path}: {:?}",
                verdict.reason()
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_user_local_install_is_replaceable() {
        // `~/.local/bin` is what `make install-user` writes to, and it is the
        // case the updater exists for.
        let dir = std::env::temp_dir().join(format!("nexo-own-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let verdict = ownership(&dir.join("nexo"));
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(verdict, Ownership::Replaceable);
    }

    #[test]
    fn a_missing_directory_is_refused_rather_than_panicking() {
        let exe = PathBuf::from("/definitely/not/here/nexo");
        assert!(!ownership(&exe).is_replaceable());
    }
}
