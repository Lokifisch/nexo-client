use sha1::{Digest, Sha1};
use sha2::Sha256;

/// `CREATE_NO_WINDOW`, from the Win32 process creation flags.
#[cfg(windows)]
const NO_WINDOW: u32 = 0x0800_0000;

/// Keeps a child process from opening a console window on Windows.
///
/// Release builds are GUI-subsystem and so own no console. Windows reacts by
/// allocating a *fresh* one for any console-subsystem child — `java.exe`,
/// `powershell.exe` — which appears as a black window that flashes up, or in
/// the case of the game sits behind it for the whole session.
///
/// Every spawn in this crate goes through one of the two `no_window` helpers.
/// They are the reason a new spawn site can't quietly reintroduce the flicker:
/// there is no bare `Command::new(...).spawn()` left to copy from.
///
/// Deliberately not solved by using `javaw.exe` for Java: that detaches the
/// standard streams, and both the version probe and the planned log console
/// read them.
///
/// A no-op on every other platform.
pub fn no_window(command: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(NO_WINDOW);
    }
    command
}

/// [`no_window`] for the async variant.
pub fn no_window_async(command: &mut tokio::process::Command) -> &mut tokio::process::Command {
    #[cfg(windows)]
    {
        command.creation_flags(NO_WINDOW);
    }
    command
}

/// What Mojang publishes for every asset and library, so this is the digest
/// the install pipeline checks against.
pub fn sha1_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Used where *we* choose the digest rather than inheriting someone else's —
/// currently the launcher's own release checksums ([`crate::self_update`]).
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Turns a display name into something safe to use as a directory name on
/// every platform we target. Windows is the strict one: it rejects `<>:"/\|?*`
/// and trailing dots/spaces.
pub fn slugify(name: &str) -> String {
    let slug: String = name
        .chars()
        .map(|c| match c {
            'a'..='z' | '0'..='9' | '-' | '_' => c,
            'A'..='Z' => c.to_ascii_lowercase(),
            _ => '-',
        })
        .collect();

    // Collapse runs of separators so "My  Cool: Pack" doesn't become
    // "my--cool--pack".
    let mut out = String::with_capacity(slug.len());
    let mut prev_dash = false;
    for c in slug.chars() {
        if c == '-' {
            if !prev_dash && !out.is_empty() {
                out.push('-');
            }
            prev_dash = true;
        } else {
            out.push(c);
            prev_dash = false;
        }
    }

    let trimmed = out.trim_end_matches('-').to_string();
    if trimmed.is_empty() {
        "instance".to_string()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_collapses_and_lowercases() {
        assert_eq!(slugify("My  Cool: Pack"), "my-cool-pack");
        assert_eq!(slugify("Fabric 26.1.2"), "fabric-26-1-2");
    }

    #[test]
    fn slug_never_empty_or_trailing_dash() {
        assert_eq!(slugify("!!!"), "instance");
        assert_eq!(slugify("name!"), "name");
    }

    #[test]
    fn sha1_matches_known_digest() {
        assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn sha256_matches_known_digest() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
