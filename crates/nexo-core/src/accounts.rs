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

#[derive(Debug, Clone)]
pub struct AccountStore {
    shared: SharedStore,
    /// The launcher's old plaintext file, migrated once then left alone.
    legacy: PathBuf,
}

impl AccountStore {
    pub fn new(paths: &crate::paths::Paths) -> Self {
        Self {
            shared: SharedStore::new(paths.accounts_file()),
            legacy: paths.legacy_accounts_file(),
        }
    }

    /// Loads the shared store, importing the old plaintext one the first time
    /// if the shared store doesn't exist yet.
    ///
    /// The legacy file is deliberately left in place rather than deleted: it
    /// costs nothing, and losing accounts to a migration bug would be far
    /// worse than a stale file sitting there.
    async fn read(&self) -> Result<Contents> {
        if !self.shared.exists()
            && let Some(migrated) = self.migrate().await?
        {
            return Ok(migrated);
        }
        self.shared.load().await
    }

    async fn migrate(&self) -> Result<Option<Contents>> {
        if !self.legacy.exists() {
            return Ok(None);
        }

        let Ok(raw) = tokio::fs::read(&self.legacy).await else {
            return Ok(None);
        };

        #[derive(serde::Deserialize)]
        struct LegacyFile {
            #[serde(default)]
            accounts: Vec<Account>,
            #[serde(default)]
            active: Option<String>,
        }

        let Ok(legacy) = serde_json::from_slice::<LegacyFile>(&raw) else {
            tracing::warn!("the old accounts file is unreadable; starting fresh");
            return Ok(None);
        };

        let contents = Contents {
            accounts: legacy.accounts,
            active: legacy.active,
            ..Contents::default()
        };

        tracing::info!(
            accounts = contents.accounts.len(),
            "migrating accounts into the store shared with Nexo Mod"
        );
        self.shared.save(&contents).await?;
        Ok(Some(contents))
    }

    pub async fn list(&self) -> Result<Vec<Account>> {
        Ok(self.read().await?.accounts)
    }

    pub async fn active(&self) -> Result<Option<Account>> {
        let contents = self.read().await?;
        let Some(active) = contents.active.clone() else {
            return Ok(contents.accounts.into_iter().next());
        };
        Ok(contents
            .accounts
            .into_iter()
            .find(|a| a.uuid == active))
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

        if !account.is_expired() {
            return Ok(account);
        }

        let refreshed = auth.refresh(&account).await?;
        self.upsert(refreshed.clone()).await?;
        Ok(refreshed)
    }
}
