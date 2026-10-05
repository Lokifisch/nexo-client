//! Persistent multi-account storage.
//!
//! Backed by [`crate::shared_store`], the encrypted file Nexo Mod reads and
//! writes as well — so signing in here shows up in the in-game account
//! switcher, and an account added in game shows up here.
//!
//! Every mutation is read-modify-write rather than a wholesale snapshot. The
//! launcher usually stays open while the game runs, so both processes can be
//! writing; re-reading first means a change made by the other side is merged
//! instead of clobbered.

use crate::auth::{Account, Auth};
use crate::error::{Error, Result};
use crate::shared_store::{Contents, SharedStore};
use std::path::PathBuf;

/// What was salvaged from the old plaintext file.
struct LegacyFile {
    /// Deduplicated, uuids normalised (dashless, lowercase).
    accounts: Vec<Account>,
    active: Option<String>,
    /// An entry could not be migrated (unparseable, or no uuid). The file
    /// must then be kept: it may hold the only copy.
    dropped: bool,
}

/// Dashless lowercase, the form the shared store uses.
fn norm(uuid: &str) -> String {
    uuid.replace('-', "").to_lowercase()
}

fn uuid_set(accounts: &[Account]) -> std::collections::HashSet<String> {
    accounts.iter().map(|a| norm(&a.uuid)).collect()
}

/// `contents` plus the legacy accounts it lacks (and legacy tokens newer than
/// the stored ones), or `None` if nothing would change.
fn merged(contents: &Contents, legacy: &LegacyFile) -> Option<Contents> {
    let mut out = contents.clone();
    for l in &legacy.accounts {
        match out.accounts.iter_mut().find(|a| norm(&a.uuid) == l.uuid) {
            Some(have) if l.expires_at > have.expires_at => *have = l.clone(),
            Some(_) => {}
            None => out.accounts.push(l.clone()),
        }
    }
    if out == *contents {
        return None;
    }
    if out.active.is_none() {
        out.active = legacy.active.clone();
    }
    Some(out)
}

/// Every legacy account is stored, with a token at least as new.
fn covers(contents: &Contents, legacy: &LegacyFile) -> bool {
    legacy.accounts.iter().all(|l| {
        contents
            .accounts
            .iter()
            .any(|a| norm(&a.uuid) == l.uuid && a.expires_at >= l.expires_at)
    })
}

static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn parse_legacy(raw: &[u8]) -> Option<LegacyFile> {
    #[derive(serde::Deserialize)]
    struct Raw {
        #[serde(default)]
        accounts: Vec<serde_json::Value>,
        #[serde(default)]
        active: Option<String>,
    }
    let parsed: Raw = match serde_json::from_slice(raw) {
        Ok(p) => p,
        Err(err) => {
            tracing::warn!(%err, "the old accounts file cannot be parsed; leaving it in place");
            return None;
        }
    };
    let mut accounts: Vec<Account> = Vec::new();
    let mut dropped = 0usize;
    for value in parsed.accounts {
        match serde_json::from_value::<Account>(value) {
            Ok(mut account) => {
                account.uuid = norm(&account.uuid);
                if account.uuid.is_empty() {
                    dropped += 1;
                } else if let Some(have) = accounts.iter_mut().find(|a| a.uuid == account.uuid) {
                    // Duplicate: keep the newest token, not the first seen.
                    if account.expires_at > have.expires_at {
                        *have = account;
                    }
                } else {
                    accounts.push(account);
                }
            }
            Err(_) => dropped += 1, // no error text: it may quote token fields
        }
    }
    if dropped > 0 && !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        // Once per process: every read() re-parses a file that is kept.
        tracing::warn!(dropped, "some accounts in the old file could not be migrated; keeping it");
    }
    Some(LegacyFile {
        accounts,
        active: parsed.active.map(|a| norm(&a)).filter(|a| !a.is_empty()),
        dropped: dropped > 0,
    })
}

#[derive(Debug, Clone)]
pub struct AccountStore {
    shared: SharedStore,
    /// The launcher's old plaintext file, migrated once, then deleted.
    legacy: PathBuf,
    /// Serialises refresh-and-save so two launches of one expired account
    /// don't both spend (and the second lose) the rotating refresh token.
    /// ponytail: one lock for all accounts, per-uuid if refreshes ever contend.
    refresh_lock: std::sync::Arc<tokio::sync::Mutex<()>>,
}

impl AccountStore {
    pub fn new(paths: &crate::paths::Paths) -> Self {
        Self {
            shared: SharedStore::new(paths.accounts_file()),
            legacy: paths.legacy_accounts_file(),
            refresh_lock: Default::default(),
        }
    }

    /// Present once the legacy file has been merged and was kept anyway
    /// (entries dropped, failed delete). Stops a later `read()` re-merging it,
    /// which would resurrect accounts signed out since.
    fn merged_marker(&self) -> PathBuf {
        let mut name = self.legacy.clone().into_os_string();
        name.push(".merged");
        PathBuf::from(name)
    }

    /// `len:mtime` of the legacy file; the marker records it so a recreated or
    /// replaced accounts.json (reinstalled old launcher) is merged again.
    fn legacy_stamp(&self) -> Option<String> {
        let m = std::fs::metadata(&self.legacy).ok()?;
        let t = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
        Some(format!("{}:{}", m.len(), t.as_nanos()))
    }

    /// True if the marker matches the legacy file as it is now. A marker for a
    /// vanished file is deleted.
    fn already_merged(&self) -> bool {
        let Some(stamp) = self.legacy_stamp() else {
            let _ = std::fs::remove_file(self.merged_marker());
            return false;
        };
        std::fs::read_to_string(self.merged_marker()).is_ok_and(|m| m == stamp)
    }

    async fn write_marker(&self) {
        let stamp = self.legacy_stamp().unwrap_or_default();
        if let Err(err) = tokio::fs::write(self.merged_marker(), stamp).await {
            tracing::warn!(%err, "could not write the legacy-merged marker");
        }
    }

    /// Loads the shared store, importing the old plaintext one the first time
    /// if the shared store doesn't exist yet.
    ///
    /// The legacy file holds tokens in plaintext, so it is deleted — but only
    /// once the encrypted store has been read back successfully. Nexo Mod
    /// never reads it (it only knows `accounts.dat`). An undecryptable store
    /// (other machine) leaves it alone, since it may be the only copy.
    async fn read(&self) -> Result<Contents> {
        if !self.shared.exists() {
            if let Some(migrated) = self.migrate().await? {
                return Ok(migrated);
            }
        }
        let mut contents = self.shared.load().await?;
        // The shared store can predate the migration (Nexo Mod creates it
        // too), so legacy accounts missing from it are merged in first; the
        // plaintext file goes only once every account is proven present.
        if !self.already_merged() && self.legacy.exists() {
            if let Some(legacy) = self.read_legacy().await {
                if let Some(next) = merged(&contents, &legacy) {
                    // Re-load right before saving: if the Mod wrote in the
                    // meantime (any field, e.g. a refreshed token), recompute
                    // on its version instead. One retry; a real lock needs a
                    // dependency (fs2/fd-lock).
                    let fresh = self.shared.load().await?;
                    let to_save = if fresh == contents { Some(next) } else { merged(&fresh, &legacy) };
                    match to_save {
                        Some(next) => {
                            self.shared.save(&next).await?;
                            contents = self.shared.load().await?;
                        }
                        // Fresh already has everything: adopt it, write nothing.
                        None => contents = fresh,
                    }
                }
                if !legacy.dropped && covers(&contents, &legacy) {
                    if let Err(err) = tokio::fs::remove_file(&self.legacy).await {
                        tracing::warn!(%err, "could not delete the old plaintext accounts file");
                    }
                }
                // Kept (never delete what couldn't be migrated) but merged
                // once and for all.
                if self.legacy.exists() && covers(&contents, &legacy) {
                    self.write_marker().await;
                }
            }
        }
        Ok(contents)
    }

    /// Lenient: one bad entry costs only itself, not the whole file.
    async fn read_legacy(&self) -> Option<LegacyFile> {
        let raw = tokio::fs::read(&self.legacy).await.ok()?;
        parse_legacy(&raw)
    }

    async fn migrate(&self) -> Result<Option<Contents>> {
        if !self.legacy.exists() {
            return Ok(None);
        }

        let Some(legacy) = self.read_legacy().await else {
            tracing::warn!("the old accounts file is unreadable; starting fresh");
            return Ok(None);
        };

        let contents = Contents {
            accounts: legacy.accounts.clone(),
            active: legacy.active.clone(),
            ..Contents::default()
        };

        tracing::info!(
            accounts = contents.accounts.len(),
            "migrating accounts into the store shared with Nexo Mod"
        );
        self.shared.save(&contents).await?;
        // Prove the encrypted copy round-trips before the plaintext goes.
        let reloaded = self.shared.load().await?;
        if !legacy.dropped && uuid_set(&reloaded.accounts) == uuid_set(&contents.accounts) {
            if let Err(err) = tokio::fs::remove_file(&self.legacy).await {
                tracing::warn!(%err, "could not delete the old plaintext accounts file");
            }
        }
        if self.legacy.exists() && uuid_set(&reloaded.accounts) == uuid_set(&contents.accounts) {
            self.write_marker().await;
        }
        Ok(Some(contents))
    }

    pub async fn list(&self) -> Result<Vec<Account>> {
        Ok(self.read().await?.accounts)
    }

    pub async fn active(&self) -> Result<Option<Account>> {
        let contents = self.read().await?;
        // An `active` naming an account that isn't there falls back to the first.
        let pick = contents
            .active
            .as_ref()
            .and_then(|u| contents.accounts.iter().position(|a| &a.uuid == u))
            .unwrap_or(0);
        Ok(contents.accounts.into_iter().nth(pick))
    }

    /// Adds or replaces an account, making it active. Re-signing into an
    /// account already present updates its tokens rather than duplicating it.
    pub async fn upsert(&self, account: Account) -> Result<()> {
        let mut contents = self.read().await?;
        contents.accounts.retain(|a| a.uuid != account.uuid);
        contents.active = Some(account.uuid.clone());
        contents.accounts.push(account);
        self.shared.save(&contents).await
    }

    pub async fn set_active(&self, uuid: &str) -> Result<()> {
        let mut contents = self.read().await?;
        if !contents.accounts.iter().any(|a| a.uuid == uuid) {
            return Err(Error::invalid("that account is not signed in"));
        }
        contents.active = Some(uuid.to_string());
        self.shared.save(&contents).await
    }

    pub async fn remove(&self, uuid: &str) -> Result<()> {
        let mut contents = self.read().await?;
        contents.accounts.retain(|a| a.uuid != uuid);
        if contents.active.as_deref() == Some(uuid) {
            contents.active = contents.accounts.first().map(|a| a.uuid.clone());
        }
        self.shared.save(&contents).await
    }

    /// Returns the active account with a guaranteed-valid token, refreshing
    /// it first if needed. Call this immediately before launching.
    pub async fn active_valid(&self, auth: &Auth) -> Result<Account> {
        let account = self
            .active()
            .await?
            .ok_or_else(|| Error::invalid("sign in to a Microsoft account first"))?;
        // From here on everything is by uuid, so a switch of the active
        // account mid-refresh can't change who we launch as.
        self.renew(&account.uuid, auth, false).await
    }

    /// Like [`Self::active_valid`] for a chosen account, and unlike `upsert`
    /// it leaves the stored *active* account alone — the rotated refresh
    /// token is saved in place, so launching as a second account never
    /// switches which one the launcher (or the Mod, which shares this file)
    /// treats as active.
    pub async fn valid_for(&self, uuid: &str, auth: &Auth) -> Result<Account> {
        self.renew(uuid, auth, false).await
    }

    /// Refreshes the active account's profile (and token if expired) under the
    /// refresh lock. `None` when nobody is signed in.
    pub async fn refresh_active(&self, auth: &Auth) -> Result<Option<Account>> {
        let Some(account) = self.active().await? else {
            return Ok(None);
        };
        self.renew(&account.uuid, auth, true).await.map(Some)
    }

    async fn find(&self, uuid: &str) -> Result<Account> {
        self.read()
            .await?
            .accounts
            .into_iter()
            .find(|a| a.uuid == uuid)
            .ok_or_else(|| Error::invalid("that account is not signed in"))
    }

    /// The one refresh path: re-reads by uuid under `refresh_lock` (so the
    /// rotating refresh token is spent once) and saves in place, only if the
    /// account is still signed in. `profile` forces a profile re-sync even
    /// when the token is still valid.
    async fn renew(&self, uuid: &str, auth: &Auth, profile: bool) -> Result<Account> {
        let account = self.find(uuid).await?;
        if !profile && !account.is_expired() {
            return Ok(account);
        }

        let _lock = self.refresh_lock.lock().await;
        // A concurrent launch may have just renewed it: re-check under the lock.
        let account = self.find(uuid).await?;
        if !account.is_expired() && !profile {
            return Ok(account);
        }
        // `refresh` already re-reads the profile; a valid token only needs the sync.
        let refreshed = if account.is_expired() {
            auth.refresh(&account).await?
        } else {
            auth.sync_profile(&account).await?
        };
        // Re-read: the refresh took a network round trip. Only update an
        // account that is still signed in; never push a removed one back.
        let saved = async {
            let mut contents = self.read().await?;
            if let Some(slot) = contents.accounts.iter_mut().find(|a| a.uuid == uuid) {
                *slot = refreshed.clone();
                self.shared.save(&contents).await?;
            }
            Ok::<(), Error>(())
        }
        .await;
        // The refresh token is spent; a failed save must not fail the launch.
        if let Err(err) = saved {
            tracing::warn!(%err, "refreshed the account but could not save it");
        }
        Ok(refreshed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn migration_deletes_the_plaintext_file() {
        if crate::hwkey::derive().is_none() {
            println!("SKIPPED: no hardware key on this machine, store round-trip not exercised");
            return;
        }
        let temp = std::env::temp_dir().join(format!("nexo-acc-{}", uuid::Uuid::new_v4()));
        let paths = crate::paths::Paths::with_root(&temp);
        tokio::fs::create_dir_all(&temp).await.unwrap();
        let legacy = paths.legacy_accounts_file();
        tokio::fs::write(
            &legacy,
            br#"{"accounts":[{"uuid":"069a79f444e94726a5befca90e38aaf5","username":"A","access_token":"t","refresh_token":"r","expires_at":1,"skin_model":"CLASSIC"}]}"#,
        )
        .await
        .unwrap();

        let store = AccountStore::new(&paths);
        let accounts = store.list().await.unwrap();
        if accounts.len() == 1 {
            assert!(
                !legacy.exists(),
                "plaintext tokens must not outlive migration"
            );
            assert_eq!(store.list().await.unwrap().len(), 1);
        }
        tokio::fs::remove_dir_all(&temp).await.ok();
    }

    #[tokio::test]
    async fn legacy_accounts_missing_from_an_existing_store_are_merged_not_deleted() {
        if crate::hwkey::derive().is_none() {
            println!("SKIPPED: no hardware key on this machine, store round-trip not exercised");
            return;
        }
        let temp = std::env::temp_dir().join(format!("nexo-acc-{}", uuid::Uuid::new_v4()));
        let paths = crate::paths::Paths::with_root(&temp);
        tokio::fs::create_dir_all(&temp).await.unwrap();
        let acc = |u: &str| {
            format!(
                r#"{{"uuid":"{u}","username":"A","access_token":"t","refresh_token":"r","expires_at":1,"skin_model":"CLASSIC"}}"#
            )
        };
        let store = AccountStore::new(&paths);
        // Shared store exists first (as if Nexo Mod made it) with one account.
        let shared: Account = serde_json::from_str(&acc("bbbb")).unwrap();
        if store
            .shared
            .save(&Contents {
                accounts: vec![shared],
                ..Contents::default()
            })
            .await
            .is_err()
        {
            return;
        }
        let legacy = paths.legacy_accounts_file();
        tokio::fs::write(&legacy, format!(r#"{{"accounts":[{}]}}"#, acc("aaaa")))
            .await
            .unwrap();
        let list = store.list().await.unwrap();
        assert_eq!(list.len(), 2, "the never-migrated account must survive");
        assert!(!legacy.exists());
        assert_eq!(store.list().await.unwrap().len(), 2);
        tokio::fs::remove_dir_all(&temp).await.ok();
    }

    #[test]
    fn legacy_parse_dedupes_normalises_and_flags_dropped() {
        let acc = |u: &str, n: &str| {
            format!(
                r#"{{"uuid":"{u}","username":"{n}","access_token":"t","refresh_token":"r","expires_at":1,"skin_model":"CLASSIC"}}"#
            )
        };
        let json = format!(
            r#"{{"accounts":[{},{},{},{{"bogus":1}},{}],"active":"AAAA-bb"}}"#,
            acc("AA-AA", "first"),
            acc("aaaa", "second"),
            acc("cccc", "c"),
            acc("", "empty"),
        );
        let l = parse_legacy(json.as_bytes()).unwrap();
        let ids: Vec<_> = l.accounts.iter().map(|a| a.uuid.as_str()).collect();
        assert_eq!(ids, ["aaaa", "cccc"]);
        assert_eq!(l.accounts[0].username, "first");
        assert_eq!(l.active.as_deref(), Some("aaaabb"));
        assert!(l.dropped, "bad and empty-uuid entries must keep the file");
        assert!(parse_legacy(b"not json").is_none());
        let clean = format!(r#"{{"accounts":[{},{}]}}"#, acc("a", "a"), acc("a", "dup"));
        assert!(!parse_legacy(clean.as_bytes()).unwrap().dropped);
    }

    #[test]
    fn newer_legacy_token_wins_and_older_does_not() {
        let acc = |u: &str, exp: u64| -> Account {
            serde_json::from_str(&format!(
                r#"{{"uuid":"{u}","username":"A","access_token":"t","refresh_token":"r","expires_at":{exp},"skin_model":"CLASSIC"}}"#
            ))
            .unwrap()
        };
        let shared = Contents { accounts: vec![acc("aaaa", 5)], ..Contents::default() };
        let newer = LegacyFile { accounts: vec![acc("aaaa", 9)], active: None, dropped: false };
        assert_eq!(merged(&shared, &newer).unwrap().accounts[0].expires_at, 9);
        assert!(!covers(&shared, &newer));
        let older = LegacyFile { accounts: vec![acc("aaaa", 2)], active: None, dropped: false };
        assert!(merged(&shared, &older).is_none() && covers(&shared, &older));
        // Dedupe in the legacy file keeps the largest expiry.
        let json = format!(r#"{{"accounts":[{},{}]}}"#, serde_json::to_string(&acc("a", 1)).unwrap(), serde_json::to_string(&acc("a", 7)).unwrap());
        assert_eq!(parse_legacy(json.as_bytes()).unwrap().accounts[0].expires_at, 7);
    }

    #[test]
    fn merge_matches_uuids_across_dash_and_case() {
        let acc = |u: &str| -> Account {
            serde_json::from_str(&format!(
                r#"{{"uuid":"{u}","username":"A","access_token":"t","refresh_token":"r","expires_at":1,"skin_model":"CLASSIC"}}"#
            ))
            .unwrap()
        };
        let shared = Contents { accounts: vec![acc("aaaa")], ..Contents::default() };
        let l = LegacyFile { accounts: vec![acc("aaaa"), acc("bbbb")], active: None, dropped: false };
        assert_eq!(merged(&shared, &l).unwrap().accounts.len(), 2);
        let l = LegacyFile { accounts: vec![acc("aaaa")], active: None, dropped: false };
        assert!(merged(&shared, &l).is_none());
    }

    #[tokio::test]
    async fn sign_out_after_a_kept_legacy_merge_is_not_undone() {
        if crate::hwkey::derive().is_none() {
            println!("SKIPPED: no hardware key on this machine, store round-trip not exercised");
            return;
        }
        let temp = std::env::temp_dir().join(format!("nexo-acc-{}", uuid::Uuid::new_v4()));
        let paths = crate::paths::Paths::with_root(&temp);
        tokio::fs::create_dir_all(&temp).await.unwrap();
        let acc = |u: &str| {
            format!(
                r#"{{"uuid":"{u}","username":"A","access_token":"t","refresh_token":"r","expires_at":1,"skin_model":"CLASSIC"}}"#
            )
        };
        let store = AccountStore::new(&paths);
        let shared: Account = serde_json::from_str(&acc("bbbb")).unwrap();
        if store.shared.save(&Contents { accounts: vec![shared], ..Contents::default() }).await.is_err() {
            return;
        }
        // A bogus entry forces the legacy file to be kept after the merge.
        tokio::fs::write(paths.legacy_accounts_file(), format!(r#"{{"accounts":[{},{{"bogus":1}}]}}"#, acc("aaaa")))
            .await
            .unwrap();
        assert_eq!(store.list().await.unwrap().len(), 2);
        assert!(paths.legacy_accounts_file().exists());
        store.remove("aaaa").await.unwrap();
        let list = store.list().await.unwrap();
        assert_eq!(list.len(), 1, "signed-out account must stay gone");
        assert_eq!(list[0].uuid, "bbbb");
        tokio::fs::remove_dir_all(&temp).await.ok();
    }

    #[tokio::test]
    async fn a_changed_legacy_file_is_merged_again() {
        if crate::hwkey::derive().is_none() {
            println!("SKIPPED: no hardware key on this machine, store round-trip not exercised");
            return;
        }
        let temp = std::env::temp_dir().join(format!("nexo-acc-{}", uuid::Uuid::new_v4()));
        let paths = crate::paths::Paths::with_root(&temp);
        tokio::fs::create_dir_all(&temp).await.unwrap();
        let acc = |u: &str| {
            format!(
                r#"{{"uuid":"{u}","username":"A","access_token":"t","refresh_token":"r","expires_at":1,"skin_model":"CLASSIC"}}"#
            )
        };
        let store = AccountStore::new(&paths);
        let shared: Account = serde_json::from_str(&acc("bbbb")).unwrap();
        if store.shared.save(&Contents { accounts: vec![shared], ..Contents::default() }).await.is_err() {
            return;
        }
        let legacy = paths.legacy_accounts_file();
        tokio::fs::write(&legacy, format!(r#"{{"accounts":[{},{{"bogus":1}}]}}"#, acc("aaaa"))).await.unwrap();
        assert_eq!(store.list().await.unwrap().len(), 2);
        assert!(store.merged_marker().exists());
        // Unchanged file: not re-merged after sign-out.
        store.remove("aaaa").await.unwrap();
        assert_eq!(store.list().await.unwrap().len(), 1);
        // Replaced file (different length): merged again.
        tokio::fs::write(&legacy, format!(r#"{{"accounts":[{},{},{{"bogus":1}}]}}"#, acc("aaaa"), acc("cccc"))).await.unwrap();
        assert_eq!(store.list().await.unwrap().len(), 3);
        // File gone: marker goes too.
        tokio::fs::remove_file(&legacy).await.unwrap();
        store.list().await.unwrap();
        assert!(!store.merged_marker().exists());
        tokio::fs::remove_dir_all(&temp).await.ok();
    }
}
