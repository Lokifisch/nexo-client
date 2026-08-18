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

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Decodes standard base64, as used for the server icons in `servers.dat` and
/// in a status response's `favicon` data URL.
///
/// Written rather than pulled in because it is needed in exactly two places
/// for exactly one alphabet. Whitespace is skipped — `servers.dat` icons are
/// sometimes stored wrapped — and anything else invalid returns `None` rather
/// than a partial decode, since a half-decoded PNG is not a picture.
pub fn base64_decode(encoded: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(encoded.len() / 4 * 3);
    let mut accumulator: u32 = 0;
    let mut bits = 0;

    for byte in encoded.bytes() {
        match byte {
            b'=' => break,
            b if b.is_ascii_whitespace() => continue,
            b => {
                let value = BASE64.iter().position(|c| *c == b)? as u32;
                accumulator = (accumulator << 6) | value;
                bits += 6;
                if bits >= 8 {
                    bits -= 8;
                    out.push((accumulator >> bits) as u8);
                }
            }
        }
    }

    // Leftover bits must be padding, and padding must be zero. Anything else
    // means the input was truncated mid-character.
    if accumulator & ((1 << bits) - 1) != 0 {
        return None;
    }
    Some(out)
}

/// The inverse, used to write an icon back into `servers.dat`.
pub fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);

    for chunk in bytes.chunks(3) {
        let mut block = [0u8; 3];
        block[..chunk.len()].copy_from_slice(chunk);
        let packed = u32::from_be_bytes([0, block[0], block[1], block[2]]);

        for index in 0..4 {
            if index <= chunk.len() {
                out.push(BASE64[((packed >> (18 - index * 6)) & 0x3F) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// File sizes for the browser and the worlds list.
///
/// Binary units, matching what a file manager shows for the same folder —
/// a launcher disagreeing with the OS about how big a world is would just
/// look wrong. Sub-KiB sizes stay exact, since "0.3 KiB" says less than
/// "412 B".
pub fn human_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    let units = ["KiB", "MiB", "GiB", "TiB"];

    if bytes < 1024 {
        return format!("{bytes} B");
    }

    let mut value = bytes as f64 / KIB;
    let mut unit = units[0];
    for next in &units[1..] {
        if value < 1024.0 {
            break;
        }
        value /= KIB;
        unit = next;
    }

    // One decimal below 10 and none above: "9.7 MiB" is a useful distinction,
    // "437.2 MiB" is noise.
    if value < 10.0 {
        format!("{value:.1} {unit}")
    } else {
        format!("{value:.0} {unit}")
    }
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
    fn base64_round_trips_every_padding_case() {
        // Lengths 0..=3 mod 3 cover all three padding shapes.
        for input in ["", "f", "fo", "foo", "foob", "fooba", "foobar"] {
            let encoded = base64_encode(input.as_bytes());
            assert_eq!(
                base64_decode(&encoded).as_deref(),
                Some(input.as_bytes()),
                "{input}"
            );
        }
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"f"), "Zg==");
    }

    #[test]
    fn base64_round_trips_arbitrary_bytes() {
        // A PNG is binary, so the high bytes and the 62/63 alphabet slots have
        // to survive — those are the ones a naive table gets wrong.
        let bytes: Vec<u8> = (0..=255u8).collect();
        assert_eq!(
            base64_decode(&base64_encode(&bytes)).as_deref(),
            Some(&bytes[..])
        );
    }

    #[test]
    fn base64_tolerates_wrapping_and_rejects_junk() {
        assert_eq!(base64_decode("Zm9v\nYmFy").as_deref(), Some(&b"foobar"[..]));
        assert_eq!(base64_decode("Zm9v YmFy").as_deref(), Some(&b"foobar"[..]));
        // Not in the alphabet at all.
        assert_eq!(base64_decode("not*valid"), None);
        // Truncated: a trailing character carrying bits that never complete a
        // byte is a damaged icon, not a shorter one.
        assert_eq!(base64_decode("Zm9vYmFyZ"), None);
        // Padded, but with bits set where the padding should be — the same
        // damage, one character further along.
        assert_eq!(base64_decode("Zh=="), None);
    }

    #[test]
    fn human_bytes_switches_unit_and_precision() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1023), "1023 B");
        assert_eq!(human_bytes(1024), "1.0 KiB");
        assert_eq!(human_bytes(1024 * 1024 * 9 + 700_000), "9.7 MiB");
        assert_eq!(human_bytes(1024 * 1024 * 437), "437 MiB");
        assert_eq!(human_bytes(1024 * 1024 * 1024 * 3), "3.0 GiB");
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
