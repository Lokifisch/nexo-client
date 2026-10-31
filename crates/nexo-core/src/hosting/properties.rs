//! `server.properties` read/patch. Java's `Properties` format: `key=value`
//! lines, `#` comments, `\` escapes, non-Latin-1 as `\uXXXX`. Patching edits
//! values in place so comments, ordering and keys this launcher doesn't know
//! about survive a save from the Settings tab.

/// Every `key=value` pair, in file order, unescaped.
pub fn parse(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim_start();
            if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
                return None;
            }
            let (k, v) = split(line);
            Some((unescape(k), unescape(v)))
        })
        .collect()
}

/// `text` with each key in `updates` set to its new value: replaced where the
/// key already exists, appended where it doesn't.
pub fn patch(text: &str, updates: &[(String, String)]) -> String {
    let mut done = vec![false; updates.len()];
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let trimmed = line.trim_start();
        let replaced = (!trimmed.is_empty() && !trimmed.starts_with('#') && !trimmed.starts_with('!'))
            .then(|| unescape(split(trimmed).0))
            .and_then(|key| updates.iter().position(|(k, _)| *k == key));
        match replaced {
            Some(i) => {
                done[i] = true;
                out.push_str(&line_for(&updates[i].0, &updates[i].1));
            }
            None => out.push_str(line),
        }
        out.push('\n');
    }
    for (i, (k, v)) in updates.iter().enumerate() {
        if !done[i] {
            out.push_str(&line_for(k, v));
            out.push('\n');
        }
    }
    out
}

pub fn line_for(key: &str, value: &str) -> String {
    format!("{}={}", escape(key, true), escape(value, false))
}

fn split(line: &str) -> (&str, &str) {
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        match c {
            '\\' if !escaped => escaped = true,
            '=' | ':' if !escaped => return (line[..i].trim_end(), line[i + 1..].trim_start()),
            _ => escaped = false,
        }
    }
    (line, "")
}

fn escape(s: &str, key: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '=' | ':' | '#' | '!' => {
                out.push('\\');
                out.push(c);
            }
            ' ' if key => out.push_str("\\ "),
            c if c.is_control() => {}
            c if (c as u32) > 0x7e => {
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out
}

fn unescape(s: &str) -> String {
    let mut units: Vec<u16> = Vec::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            let mut buf = [0u16; 2];
            units.extend_from_slice(c.encode_utf16(&mut buf));
            continue;
        }
        match chars.next() {
            Some('n') => units.push('\n' as u16),
            Some('r') => units.push('\r' as u16),
            Some('t') => units.push('\t' as u16),
            Some('u') => {
                let hex: String = chars.by_ref().take(4).collect();
                if let Ok(unit) = u16::from_str_radix(&hex, 16) {
                    units.push(unit);
                }
            }
            Some(other) => {
                let mut buf = [0u16; 2];
                units.extend_from_slice(other.encode_utf16(&mut buf));
            }
            None => {}
        }
    }
    String::from_utf16_lossy(&units)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_awkward_values_and_keeps_other_lines() {
        let original = "#Minecraft server properties\nmotd=A Minecraft Server\npvp=true\nlevel-seed=\n";
        let motd = "Lokis Welt: §6gold = ünïcødé 🙂 \\o/";
        let patched = patch(
            original,
            &[("motd".into(), motd.into()), ("max-players".into(), "8".into())],
        );
        assert!(patched.starts_with("#Minecraft server properties\n"));
        let parsed = parse(&patched);
        assert_eq!(
            parsed,
            vec![
                ("motd".to_string(), motd.to_string()),
                ("pvp".into(), "true".into()),
                ("level-seed".into(), "".into()),
                ("max-players".into(), "8".into()),
            ]
        );
        // Nothing outside ASCII reaches the file, whatever charset the server reads it in.
        assert!(patched.is_ascii());
    }
}
