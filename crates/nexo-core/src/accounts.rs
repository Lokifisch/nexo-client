//! Persistent multi-account storage.
//!
//! Tokens are secrets: the file is written with owner-only permissions on
//! Unix. That is deliberately weaker than `Mod/`'s AES-256-GCM at-rest
//! encryption — matching that is worth doing before any public release, and
//! is tracked as such. File permissions at least keep other local users out.

use crate::auth::{Account, Auth};
use crate::error::{Error, IoContext, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Default, Serialize, Deserialize)]
struct AccountsFile {
    #[serde(default)]
    accounts: Vec<Account>,
    /// UUID of the account launches use by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    active: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AccountStore {
    path: PathBuf,
}

impl AccountStore {
    pub fn new(paths: &crate::paths::Paths) -> Self {
        Self {
            path: paths.accounts_file(),
        }
    }

    async fn read(&self) -> Result<AccountsFile> {
        if !self.path.exists() {
            return Ok(AccountsFile::default());
        }
        let raw = tokio::fs::read(&self.path).await.ctx(&self.path)?;
        // A corrupt accounts file shouldn't brick the app — worst case the
        // user signs in again.
        Ok(serde_json::from_slice(&raw).unwrap_or_default())
    }

    async fn write(&self, file: &AccountsFile) -> Result<()> {
        let json = serde_json::to_vec_pretty(file)?;
        tokio::fs::write(&self.path, &json).await.ctx(&self.path)?;
        self.restrict_permissions().await
    }

    #[cfg(unix)]
    async fn restrict_permissions(&self) -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        tokio::fs::set_permissions(&self.path, perms)
            .await
            .ctx(&self.path)
    }

    #[cfg(not(unix))]
    async fn restrict_permissions(&self) -> Result<()> {
        // Windows inherits the user-profile ACL, which is already
        // owner-scoped for %APPDATA%.
        Ok(())
    }

    pub async fn list(&self) -> Result<Vec<Account>> {
        Ok(self.read().await?.accounts)
    }

    pub async fn active(&self) -> Result<Option<Account>> {
        let file = self.read().await?;
        let Some(active) = file.active else {
            return Ok(file.accounts.into_iter().next());
        };
        Ok(file
            .accounts
            .into_iter()
            .find(|a| a.uuid == active))
    }

    /// Adds or replaces an account, making it active. Re-signing into an
    /// account already present updates its tokens rather than duplicating it.
    pub async fn upsert(&self, account: Account) -> Result<()> {
        let mut file = self.read().await?;
        file.accounts.retain(|a| a.uuid != account.uuid);
        file.active = Some(account.uuid.clone());
        file.accounts.push(account);
        self.write(&file).await
    }

    pub async fn set_active(&self, uuid: &str) -> Result<()> {
        let mut file = self.read().await?;
        if !file.accounts.iter().any(|a| a.uuid == uuid) {
            return Err(Error::invalid("that account is not signed in"));
        }
        file.active = Some(uuid.to_string());
        self.write(&file).await
    }

    pub async fn remove(&self, uuid: &str) -> Result<()> {
        let mut file = self.read().await?;
        file.accounts.retain(|a| a.uuid != uuid);
        if file.active.as_deref() == Some(uuid) {
            file.active = file.accounts.first().map(|a| a.uuid.clone());
        }
        self.write(&file).await
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
