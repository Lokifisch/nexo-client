//! Name and skin history for any player, from crafty.gg.
//!
//! The only thing sent is the name the user typed, and only when they press
//! search. The reply is untrusted: sizes are capped and every field is optional.

use crate::error::{Error, Result};
use base64::Engine;
use serde::Deserialize;

const API: &str = "https://api.crafty.gg/api/v2/players/";
/// A real reply is well under 100 KB (six skins as base64); this is an attack, not a player.
const MAX_BODY: usize = 2 * 1024 * 1024;
const MAX_SKINS: usize = 64;
const MAX_NAMES: usize = 100;
/// One skin PNG; a real one is a few KB.
const MAX_PNG: usize = 64 * 1024;
const LOOKUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// JSON `null` where a list or number is expected reads as empty rather than failing the lookup.
fn nullable<'de, D, T>(d: D) -> std::result::Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

#[derive(Debug, Clone)]
pub struct NameEntry {
    pub name: String,
    /// `None` is the original name.
    pub changed_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SkinEntry {
    pub hash: String,
    pub png: Vec<u8>,
    pub slim: bool,
    pub changed_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CapeEntry {
    pub name: String,
    pub current: bool,
}

#[derive(Debug, Clone)]
pub struct Profile {
    pub uuid: String,
    pub name: String,
    pub created_at: Option<String>,
    pub name_changes: u32,
    pub rarity: Option<String>,
    /// Newest first, the original name last.
    pub names: Vec<NameEntry>,
    /// Newest first; the first is the one being worn.
    pub skins: Vec<SkinEntry>,
    pub capes: Vec<CapeEntry>,
}

#[derive(Deserialize)]
struct Reply {
    #[serde(default)]
    success: bool,
    data: Option<Data>,
}

#[derive(Deserialize)]
struct Data {
    uuid: Option<String>,
    username: Option<String>,
    created_at: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    name_changes: i64,
    #[serde(default, deserialize_with = "nullable")]
    usernames: Vec<Username>,
    #[serde(default, deserialize_with = "nullable")]
    skins: Vec<Skin>,
    #[serde(default, deserialize_with = "nullable")]
    capes: Vec<Cape>,
    rarity: Option<Rarity>,
}

#[derive(Deserialize)]
struct Username {
    username: Option<String>,
    changed_at: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    hidden: bool,
}

#[derive(Deserialize)]
struct Skin {
    hash: Option<String>,
    texture: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    slim: bool,
    changed_at: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    hidden: bool,
}

#[derive(Deserialize)]
struct Cape {
    name: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    current: bool,
}

#[derive(Deserialize)]
struct Rarity {
    tier: Option<String>,
}

/// Minecraft names are 3-16 of these; anything else would only reach the URL.
fn valid_name(name: &str) -> bool {
    (1..=16).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

pub async fn lookup(http: &reqwest::Client, name: &str) -> Result<Profile> {
    let name = name.trim();
    if !valid_name(name) {
        return Err(Error::invalid("That is not a valid Minecraft name."));
    }
    tokio::time::timeout(LOOKUP_TIMEOUT, fetch(http, name))
        .await
        .map_err(|_| Error::invalid("crafty.gg took too long to answer."))?
}

async fn fetch(http: &reqwest::Client, name: &str) -> Result<Profile> {
    let mut response = http.get(format!("{API}{name}")).header("Accept", "application/json").send().await?;
    match response.status().as_u16() {
        200 => {}
        404 => return Err(Error::invalid(format!("No player called {name}."))),
        403 | 429 => return Err(Error::invalid("crafty.gg refused the request. Try again later.")),
        other => return Err(Error::invalid(format!("crafty.gg answered with an error ({other})."))),
    }
    // Read in pieces and stop at the cap: `bytes()` would buffer whatever the host sends.
    let too_large = || Error::invalid("crafty.gg sent a reply that is too large.");
    if response.content_length().is_some_and(|n| n > MAX_BODY as u64) {
        return Err(too_large());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len() + chunk.len() > MAX_BODY {
            return Err(too_large());
        }
        body.extend_from_slice(&chunk);
    }
    parse(&body).map_err(|e| match e {
        Error::Json(_) => Error::invalid("crafty.gg sent a reply this version does not understand."),
        other => other,
    })
}

fn parse(body: &[u8]) -> Result<Profile> {
    let reply: Reply = serde_json::from_slice(body)?;
    let data = reply.data.filter(|_| reply.success).ok_or_else(|| Error::invalid("No such player."))?;

    let mut names: Vec<NameEntry> = data
        .usernames
        .into_iter()
        .filter(|u| !u.hidden)
        .filter_map(|u| Some(NameEntry { name: u.username.filter(|n| valid_name(n))?, changed_at: u.changed_at.map(clean) }))
        .take(MAX_NAMES)
        .collect();
    // The original name has no date and sorts oldest, so it ends up last.
    names.sort_by_key(|n| std::cmp::Reverse(instant(n.changed_at.as_deref())));

    let engine = base64::engine::general_purpose::STANDARD;
    let mut skins: Vec<SkinEntry> = data
        .skins
        .into_iter()
        .filter(|s| !s.hidden)
        .filter_map(|s| {
            let texture = s.texture?;
            if texture.len() > MAX_PNG * 2 {
                return None;
            }
            let png = engine.decode(texture).ok()?;
            if !skin_header_ok(&png) {
                return None;
            }
            Some(SkinEntry { hash: s.hash.unwrap_or_default(), png, slim: s.slim, changed_at: s.changed_at.map(clean) })
        })
        .take(MAX_SKINS)
        .collect();
    skins.sort_by_key(|s| std::cmp::Reverse(instant(s.changed_at.as_deref())));

    let uuid = data.uuid.unwrap_or_default();
    let name = data.username.unwrap_or_default();
    if !valid_name(&name) || !(32..=36).contains(&uuid.len()) || !uuid.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
        return Err(Error::invalid("crafty.gg sent a reply this version does not understand."));
    }
    Ok(Profile {
        uuid,
        name,
        created_at: data.created_at.map(clean),
        name_changes: data.name_changes.clamp(0, 100_000) as u32,
        rarity: data.rarity.and_then(|r| r.tier).map(clean),
        names,
        skins,
        capes: data
            .capes
            .into_iter()
            .filter_map(|c| Some(CapeEntry { name: clean(c.name?), current: c.current }))
            .take(MAX_NAMES)
            .collect(),
    })
}

/// Server text shown in the UI: no control characters, bounded.
fn clean(text: String) -> String {
    text.chars().filter(|c| !c.is_control()).take(64).collect()
}

/// A PNG whose header says 64x64 or 64x32, checked before any decoder allocates for it.
fn skin_header_ok(png: &[u8]) -> bool {
    png.len() <= MAX_PNG
        && png.len() > 24
        && png.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A])
        && png[16..20] == 64u32.to_be_bytes()
        && (png[20..24] == 64u32.to_be_bytes() || png[20..24] == 32u32.to_be_bytes())
}

/// Minutes since 1970 (UTC) for `YYYY-MM-DDTHH:MM:SS[.fff]+HH:MM`; `None`/unreadable sorts oldest.
fn instant(iso: Option<&str>) -> i64 {
    let parse = |s: &str| -> Option<i64> {
        let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
        let (y, m, d, hh, mm) = (n(0..4)?, n(5..7)?, n(8..10)?, n(11..13)?, n(14..16)?);
        let offset = match s.get(s.len().checked_sub(6)?..)? {
            t if t.starts_with(['+', '-']) => {
                let sign = if t.starts_with('-') { -1 } else { 1 };
                sign * (t.get(1..3)?.parse::<i64>().ok()? * 60 + t.get(4..6)?.parse::<i64>().ok()?)
            }
            _ => 0,
        };
        // Days from civil (Howard Hinnant).
        let y = if m <= 2 { y - 1 } else { y };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
        let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
        Some(days * 1440 + hh * 60 + mm - offset)
    };
    iso.and_then(parse).unwrap_or(i64::MIN)
}

/// `2026-06-15T05:26:58.768+02:00` becomes `2026-06-15`; `None` is the original.
pub fn date(iso: Option<&str>) -> String {
    iso.map_or_else(|| "Original".to_string(), |s| s.chars().take(10).collect())
}

/// `pink_sheep` becomes `Pink Sheep`.
pub fn tier(slug: &str) -> String {
    slug.split('_')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next().map_or(String::new(), |f| f.to_uppercase().collect::<String>() + c.as_str())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_reply_and_orders_history() {
        let json = br#"{"success":true,"data":{"uuid":"4260c5e0-c7de-404b-9d3b-b1e14f7fb5f2","username":"Lokifisch","name_changes":1,
            "usernames":[{"username":"Old","changed_at":null},{"username":"Lokifisch","changed_at":"2024-01-02T00:00:00+01:00"}],
            "skins":[{"hash":"a","texture":"AAAA","slim":false,"changed_at":"2023-01-01T00:00:00+01:00"},
                     {"hash":"b","texture":"AAAA","slim":true,"changed_at":"2025-01-01T00:00:00+01:00"},
                     {"hash":"c","texture":"!!","changed_at":null}],
            "capes":[{"name":"Common","current":true}],"rarity":{"tier":"pink_sheep"}}}"#;
        let p = parse(json).unwrap();
        assert_eq!(p.names[0].name, "Lokifisch");
        assert_eq!(p.names[1].name, "Old");
        assert_eq!(p.skins.len(), 0, "textures that are not 64x64 PNGs are dropped");
        assert!(instant(Some("2024-01-02T00:00:00+01:00")) < instant(Some("2024-01-02T00:30:00+02:00")) + 90, "offsets compare as instants");
        assert_eq!(instant(Some("2025-01-01T00:00:00+01:00")), instant(Some("2024-12-31T23:00:00+00:00")));
        let nulls = br#"{"success":true,"data":{"uuid":"4260c5e0-c7de-404b-9d3b-b1e14f7fb5f2","username":"Lokifisch","skins":null,"name_changes":null,"capes":null}}"#;
        assert!(parse(nulls).unwrap().skins.is_empty());
        assert_eq!(tier(p.rarity.as_deref().unwrap()), "Pink Sheep");
        assert_eq!(date(Some("2026-06-15T05:26:58.768+02:00")), "2026-06-15");
        assert!(valid_name("Lokifisch") && !valid_name("a/b") && !valid_name(""));
    }
}
