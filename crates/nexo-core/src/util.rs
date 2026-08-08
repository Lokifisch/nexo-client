use sha1::{Digest, Sha1};

pub fn sha1_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha1::new();
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
        assert_eq!(
            sha1_hex(b"abc"),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
    }
}
