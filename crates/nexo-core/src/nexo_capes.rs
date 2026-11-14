//! Nexo's own capes: the catalog everyone uploads to, and the review queue.
//!
//! The service is the emote service's, so it shares its sign-in: the account
//! proves itself to Mojang with the same `join` a server login uses, and the
//! access token only ever goes to Mojang. What comes back is a Nexo session
//! token, kept in memory per account for this run.
//!
//! Wearing a cape here changes what *Nexo Mod players* see, not the Mojang
//! profile: it is not a vanilla cape, and the rest of the game never sees it.

use crate::auth::Account;
use crate::error::{Error, Result};
use crate::skin::Rgba;
use serde::Deserialize;
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const BASE_URL: &str = "https://nexomc.dev/nexo/emotes/api/v1";
const JOIN_URL: &str = "https://sessionserver.mojang.com/session/minecraft/join";
pub const TEMPLATE_URL: &str = "https://nexomc.dev/nexo/emotes/api/v1/capes/template.png";

/// A cape someone uploaded. `status` and `reason` are only sent to its owner
/// and to reviewers.
#[derive(Debug, Clone, Deserialize)]
pub struct CustomCape {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub favorites: u32,
    #[serde(default)]
    pub uses: u32,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    /// Short and shareable, like `3F9A-2C1B`: typing it finds this cape.
    #[serde(default)]
    pub code: String,
    /// The uploader's Minecraft name as Mojang confirmed it at upload, if known.
    #[serde(default)]
    pub uploader: Option<String>,
    /// How many players reported it; only sent to reviewers.
    #[serde(default)]
    pub reports: u32,
}

impl CustomCape {
    pub fn approved(&self) -> bool {
        self.status.as_deref().is_none_or(|s| s == "approved")
    }
}

/// This account's side: its uploads, favorites, and what it wears.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Mine {
    pub mine: Vec<CustomCape>,
    pub favorites: Vec<String>,
    pub worn: Option<String>,
    pub limit: u32,
    pub admin: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ReviewQueue {
    pub pending: Vec<CustomCape>,
    pub approved: Vec<CustomCape>,
    #[serde(default)]
    pub reported: Vec<CustomCape>,
}

/// How the Custom list is ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CapeSort {
    #[default]
    Favorites,
    Newest,
    MostWorn,
}

impl CapeSort {
    pub const ALL: [CapeSort; 3] = [CapeSort::Favorites, CapeSort::Newest, CapeSort::MostWorn];

    fn query(self) -> &'static str {
        match self {
            CapeSort::Favorites => "favorites",
            CapeSort::Newest => "new",
            CapeSort::MostWorn => "worn",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CapeSort::Favorites => "Most favorited",
            CapeSort::Newest => "Newest",
            CapeSort::MostWorn => "Most worn",
        }
    }
}

/// A typed code as the id prefix it stands for: 8 hex digits, dashes, spaces and case ignored.
pub fn parse_code(text: &str) -> Option<String> {
    let raw: String = text.chars().filter(|c| !c.is_whitespace() && *c != '-').collect::<String>().to_ascii_lowercase();
    (raw.len() == 8 && raw.chars().all(|c| c.is_ascii_hexdigit())).then_some(raw)
}

#[derive(Clone)]
pub struct NexoCapes {
    http: reqwest::Client,
    base: String,
    /// Session tokens by account UUID. Not persisted: signing in costs one
    /// Mojang call, and a token on disk is one more thing to protect.
    sessions: Arc<Mutex<HashMap<String, String>>>,
    /// A grid of capes asks for every texture at once; this lets a few through at
    /// a time, so it never trips the service's per-address limit for a whole house.
    downloads: Arc<tokio::sync::Semaphore>,
    /// One sign-in at a time: an expired session meets several requests at once, and each
    /// signing in for itself would spend a Mojang join and a sign-in from the hourly budget.
    signing: Arc<tokio::sync::Mutex<()>>,
}

impl std::fmt::Debug for NexoCapes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NexoCapes").field("base", &self.base).finish_non_exhaustive()
    }
}

/// What Minecraft sends Mojang for a server id: SHA-1 read as a signed number, in hex.
pub fn join_id(challenge: &str) -> String {
    let mut digest: [u8; 20] = Sha1::digest(challenge.as_bytes()).into();
    let negative = digest[0] & 0x80 != 0;
    if negative {
        // Two's complement: invert, then add one.
        let mut carry = true;
        for byte in digest.iter_mut().rev() {
            *byte = !*byte;
            if carry {
                let (sum, overflow) = byte.overflowing_add(1);
                *byte = sum;
                carry = overflow;
            }
        }
    }
    let hex = hex::encode(digest);
    let trimmed = hex.trim_start_matches('0');
    let body = if trimmed.is_empty() { "0" } else { trimmed };
    if negative { format!("-{body}") } else { body.to_string() }
}

impl NexoCapes {
    pub fn new(http: reqwest::Client) -> Self {
        // Debug builds can point at a local service.
        let base = if cfg!(debug_assertions) { std::env::var("NEXO_CAPE_API").ok() } else { None };
        Self { http, base: base.unwrap_or_else(|| BASE_URL.into()), sessions: Default::default(), downloads: Arc::new(tokio::sync::Semaphore::new(6)), signing: Default::default() }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// Every approved cape, most favorited first. Needs no sign-in.
    pub async fn catalog(&self, sort: CapeSort) -> Result<Vec<CustomCape>> {
        #[derive(Deserialize)]
        struct Catalog {
            capes: Vec<CustomCape>,
        }
        let response = self.http.get(self.url("/capes")).query(&[("sort", sort.query())]).send().await?;
        Ok(Self::read::<Catalog>(response).await?.capes)
    }

    /// The approved cape a friend's code names.
    pub async fn by_code(&self, code: &str) -> Result<CustomCape> {
        let prefix = parse_code(code).ok_or_else(|| Error::invalid("a cape code is 8 letters and digits, like 3F9A-2C1B"))?;
        Self::read(self.http.get(self.url(&format!("/capes/code/{prefix}"))).send().await?).await
    }

    /// Flags an approved cape for the reviewers. Once per account.
    pub async fn report(&self, account: &Account, cape: &str) -> Result<()> {
        #[derive(Deserialize)]
        struct Done {}
        self.authed::<Done, _>(account, |http, url| http.post(url(&format!("/capes/{cape}/report")))).await.map(|_| ())
    }

    pub async fn me(&self, account: &Account) -> Result<Mine> {
        self.authed(account, |http, url| http.get(url("/capes/me"))).await
    }

    /// Uploads a PNG for review.
    /// Uploads exactly these bytes: the ones the caller checked and showed, not whatever the file holds by now.
    pub async fn upload(&self, account: &Account, name: &str, bytes: Vec<u8>) -> Result<Mine> {
        if bytes.len() > 1024 * 1024 {
            return Err(Error::invalid("that file is over 1 MB"));
        }
        self.authed(account, |http, url| {
            let form = reqwest::multipart::Form::new().text("name", name.to_string()).part(
                "file",
                reqwest::multipart::Part::bytes(bytes.clone()).file_name("cape.png").mime_str("image/png").expect("a valid mime type"),
            );
            http.post(url("/capes")).multipart(form)
        })
        .await
    }

    pub async fn delete(&self, account: &Account, cape: &str) -> Result<Mine> {
        self.authed(account, |http, url| http.delete(url(&format!("/capes/{cape}")))).await
    }

    pub async fn favorite(&self, account: &Account, cape: &str, on: bool) -> Result<Mine> {
        self.authed(account, |http, url| {
            let url = url(&format!("/capes/{cape}/favorite"));
            if on { http.put(url) } else { http.delete(url) }
        })
        .await
    }

    /// Wears an approved cape, or takes the Nexo cape off with `None`.
    pub async fn wear(&self, account: &Account, cape: Option<&str>) -> Result<Mine> {
        self.authed(account, |http, url| http.post(url("/capes/wear")).json(&serde_json::json!({ "cape": cape }))).await
    }

    pub async fn review_queue(&self, account: &Account) -> Result<ReviewQueue> {
        self.authed(account, |http, url| http.get(url("/capes/review"))).await
    }

    pub async fn review(&self, account: &Account, cape: &str, approve: bool) -> Result<ReviewQueue> {
        self.authed(account, |http, url| {
            http.post(url(&format!("/capes/{cape}/review"))).json(&serde_json::json!({ "approve": approve }))
        })
        .await
    }

    /// A cape's texture. Unreviewed ones need the owner's or a reviewer's session.
    pub async fn texture(&self, account: Option<&Account>, cape: &str) -> Result<Rgba> {
        let _turn = self.downloads.acquire().await.expect("the semaphore is never closed");
        let bytes = match account {
            Some(account) => self.authed_bytes(account, cape).await?,
            None => {
                let response = self.http.get(self.url(&format!("/capes/files/{cape}"))).send().await?;
                if !response.status().is_success() {
                    return Err(Error::invalid(format!("cape texture: HTTP {}", response.status())));
                }
                response.bytes().await?.to_vec()
            }
        };
        // Named by its own hash: anything else is not the cape that was asked for.
        if crate::util::sha256_hex(&bytes) != cape {
            return Err(Error::invalid("a cape texture did not match its name"));
        }
        crate::skin::decode_texture(&bytes)
    }

    async fn authed_bytes(&self, account: &Account, cape: &str) -> Result<Vec<u8>> {
        let mut failed: Option<String> = None;
        for attempt in 0..2 {
            let token = self.token(account, failed.as_deref()).await?;
            let response = self.http.get(self.url(&format!("/capes/files/{cape}"))).bearer_auth(&token).send().await?;
            if response.status() == reqwest::StatusCode::UNAUTHORIZED && attempt == 0 {
                failed = Some(token);
                continue;
            }
            if !response.status().is_success() {
                return Err(Error::invalid(format!("cape texture: HTTP {}", response.status())));
            }
            return Ok(response.bytes().await?.to_vec());
        }
        unreachable!("the loop returns on its second pass")
    }

    /// Sends a request with this account's session, signing in first if
    /// needed, and once more if the service says the session ended.
    async fn authed<T, F>(&self, account: &Account, build: F) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
        F: Fn(&reqwest::Client, &dyn Fn(&str) -> String) -> reqwest::RequestBuilder,
    {
        let url = |path: &str| self.url(path);
        let mut failed: Option<String> = None;
        for attempt in 0..2 {
            let token = self.token(account, failed.as_deref()).await?;
            let response = build(&self.http, &url).bearer_auth(&token).send().await?;
            if response.status() == reqwest::StatusCode::UNAUTHORIZED && attempt == 0 {
                failed = Some(token);
                continue;
            }
            return Self::read(response).await;
        }
        unreachable!("the loop returns on its second pass")
    }

    /// The session for this account; with `failed`, a newer one than that token, which the service refused.
    async fn token(&self, account: &Account, failed: Option<&str>) -> Result<String> {
        let cached = || self.sessions.lock().expect("not poisoned").get(&account.uuid).cloned();
        if let Some(token) = cached().filter(|t| Some(t.as_str()) != failed) {
            return Ok(token);
        }
        let _one_at_a_time = self.signing.lock().await;
        // Whoever held the lock may have just signed in.
        if let Some(token) = cached().filter(|t| Some(t.as_str()) != failed) {
            return Ok(token);
        }
        let token = self.sign_in(account).await?;
        self.sessions.lock().expect("not poisoned").insert(account.uuid.clone(), token.clone());
        Ok(token)
    }

    async fn sign_in(&self, account: &Account) -> Result<String> {
        #[derive(Deserialize)]
        struct Challenge {
            server_id: String,
        }
        #[derive(Deserialize)]
        struct Session {
            token: String,
        }
        let challenge: Challenge = Self::read(self.http.post(self.url("/challenge")).send().await?).await?;
        let joined = self
            .http
            .post(JOIN_URL)
            .json(&serde_json::json!({
                "accessToken": account.access_token,
                "selectedProfile": account.uuid.replace('-', ""),
                "serverId": join_id(&challenge.server_id),
            }))
            .send()
            .await?;
        if !joined.status().is_success() {
            return Err(Error::auth("Mojang did not accept this account's session — sign in again"));
        }
        let session: Session = Self::read(
            self.http
                .post(self.url("/session"))
                .json(&serde_json::json!({
                    "username": account.username,
                    "uuid": account.uuid,
                    "server_id": challenge.server_id,
                }))
                .send()
                .await?,
        )
        .await?;
        Ok(session.token)
    }

    /// The body as `T`, or the service's own `error` text when it gave one.
    async fn read<T: serde::de::DeserializeOwned>(response: reqwest::Response) -> Result<T> {
        let status = response.status();
        if status.is_success() {
            return Ok(response.json().await?);
        }
        #[derive(Deserialize)]
        struct Refusal {
            error: String,
        }
        let detail = response.json::<Refusal>().await.map(|r| r.error).ok();
        Err(Error::invalid(match (status.as_u16(), detail) {
            (_, Some(text)) => text,
            (403, None) => "only reviewers can do that".into(),
            (429, None) => "too many requests — try again in a minute".into(),
            (code, None) => format!("the cape service answered HTTP {code}"),
        }))
    }
}

/// Small files in the launcher's data folder that the launcher and Nexo Mod both read
/// and write, so a cape picked in one shows in the other at once instead of after the
/// next download of who wears what. One file per account (`capes-shared/<uuid>.json`,
/// dashless), so two programs signed in as different accounts never touch the same file.
pub mod shared {
    use serde::{Deserialize, Serialize};
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    #[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
    pub struct Shared {
        /// The Nexo cape the account wears, or null for none.
        #[serde(default)]
        pub worn: Option<String>,
        /// Uploads still waiting for review, so the game can tell the player when one is decided.
        #[serde(default)]
        pub pending: Vec<String>,
    }

    fn key(uuid: &str) -> String {
        uuid.replace('-', "").to_ascii_lowercase()
    }

    pub fn path(root: &Path, uuid: &str) -> PathBuf {
        root.join("capes-shared").join(format!("{}.json", key(uuid)))
    }

    /// The account's file, with anything that isn't a cape id dropped: the game writes it too,
    /// and an id ends up in a download URL.
    pub fn read(root: &Path, uuid: &str) -> Option<Shared> {
        let is_id = |id: &String| id.len() == 64 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        let mut shared: Shared = std::fs::read(path(root, uuid)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok())?;
        shared.worn = shared.worn.filter(is_id);
        shared.pending.retain(is_id);
        Some(shared)
    }

    /// Whole or not at all: the other program may read it at any moment.
    fn write_atomic(target: &Path, bytes: &[u8]) -> std::io::Result<()> {
        if let Some(dir) = target.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = target.with_extension(format!("{}.tmp", std::process::id()));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, target)
    }

    /// Records what this account wears and has waiting. Writes only on a change, so a
    /// watcher reacting to the file can't set off another write. Returns whether it wrote.
    pub fn record(root: &Path, uuid: &str, worn: Option<&str>, pending: Vec<String>) -> std::io::Result<bool> {
        let shared = Shared { worn: worn.map(str::to_string), pending };
        if read(root, uuid).as_ref() == Some(&shared) {
            return Ok(false);
        }
        write_atomic(&path(root, uuid), &serde_json::to_vec_pretty(&shared).expect("plain data serialises"))?;
        Ok(true)
    }

    pub fn modified(root: &Path, uuid: &str) -> Option<std::time::SystemTime> {
        std::fs::metadata(path(root, uuid)).and_then(|m| m.modified()).ok()
    }

    /// The launcher's own record of which review decisions the player has seen.
    pub fn read_seen(root: &Path) -> Option<BTreeMap<String, String>> {
        std::fs::read(root.join("capes-seen.json")).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok())
    }

    pub fn write_seen(root: &Path, seen: &BTreeMap<String, String>) -> std::io::Result<()> {
        write_atomic(&root.join("capes-seen.json"), &serde_json::to_vec(seen).expect("plain data serialises"))
    }
}

#[cfg(test)]
mod tests {
    use super::join_id;

    #[test]
    fn codes_parse_however_they_are_typed() {
        assert_eq!(super::parse_code("3F9A-2C1B").as_deref(), Some("3f9a2c1b"));
        assert_eq!(super::parse_code(" 3f9a 2c1b ").as_deref(), Some("3f9a2c1b"));
        assert_eq!(super::parse_code("3f9a2c1"), None);
        assert_eq!(super::parse_code("zzzz-zzzz"), None);
    }

    #[test]
    fn the_shared_file_only_changes_when_something_does() {
        let dir = std::env::temp_dir().join(format!("nexo-capes-shared-{}", std::process::id()));
        let (x, p) = ("a".repeat(64), "b".repeat(64));
        assert!(super::shared::record(&dir, "ABC-def", Some(&x), vec![p.clone()]).unwrap());
        assert!(!super::shared::record(&dir, "abcdef", Some(&x), vec![p.clone()]).unwrap());
        let read = super::shared::read(&dir, "abcdef").unwrap();
        assert_eq!(read.worn.as_deref(), Some(x.as_str()));
        assert_eq!(read.pending, vec![p]);
        // Another account has its own file, untouched by this one's writes.
        assert!(super::shared::record(&dir, "other", None, vec![]).unwrap());
        assert_eq!(super::shared::read(&dir, "abcdef").unwrap().worn.as_deref(), Some(x.as_str()));
        // Anything that isn't a cape id never comes out, since it would end up in a URL.
        std::fs::write(super::shared::path(&dir, "bad"), r#"{"worn":"../../x?","pending":["nope"]}"#).unwrap();
        let bad = super::shared::read(&dir, "bad").unwrap();
        assert_eq!((bad.worn, bad.pending.len()), (None, 0));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn join_id_matches_minecrafts_server_hash() {
        // The well-known examples of Minecraft's signed hex digest.
        assert_eq!(join_id("Notch"), "4ed1f46bbe04bc756bcb17c0c7ce3e4632f06a48");
        assert_eq!(join_id("jeb_"), "-7c9d5b0044c130109a5d7b5fb5c317c02b4e28c1");
        assert_eq!(join_id("simon"), "88e16a1019277b15d58faf0541e11910eb756f6");
    }
}
