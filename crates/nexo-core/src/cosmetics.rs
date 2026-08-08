//! Changing the account's skin and cape through Minecraft Services.
//!
//! These are the same endpoints the official launcher uses, and they all
//! authenticate with the account's Minecraft bearer token — not the Microsoft
//! one. A 401 here means the token expired, and the caller should refresh and
//! retry rather than treat it as a rejection.
//!
//! Every change is server-side and immediate: it affects the account
//! everywhere, not just in this launcher.

use crate::auth::{Account, SkinModel};
use crate::error::{Error, IoContext, Result};
use serde::Deserialize;
use std::path::Path;

const SKINS_URL: &str = "https://api.minecraftservices.com/minecraft/profile/skins";
const ACTIVE_SKIN_URL: &str = "https://api.minecraftservices.com/minecraft/profile/skins/active";
const ACTIVE_CAPE_URL: &str = "https://api.minecraftservices.com/minecraft/profile/capes/active";
const PROFILE_URL: &str = "https://api.minecraftservices.com/minecraft/profile";

/// A cape the account owns. Owning is not wearing — exactly one can be
/// active, and none has to be.
#[derive(Debug, Clone, Deserialize)]
pub struct Cape {
    pub id: String,
    /// Human-readable name, e.g. "Migrator". Not always present.
    #[serde(default)]
    pub alias: String,
    pub url: String,
    #[serde(default)]
    state: String,
}

impl Cape {
    pub fn is_active(&self) -> bool {
        self.state.eq_ignore_ascii_case("ACTIVE")
    }

    /// Something to label it with, even when Mojang sends no alias.
    pub fn label(&self) -> &str {
        if self.alias.is_empty() {
            "Cape"
        } else {
            &self.alias
        }
    }
}

#[derive(Debug, Deserialize)]
struct ProfileCapes {
    #[serde(default)]
    capes: Vec<Cape>,
}

#[derive(Debug, Clone)]
pub struct Cosmetics {
    http: reqwest::Client,
}

impl Cosmetics {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }

    /// Every cape the account owns, active one included.
    pub async fn capes(&self, account: &Account) -> Result<Vec<Cape>> {
        let response = self
            .http
            .get(PROFILE_URL)
            .bearer_auth(&account.access_token)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(Error::auth("could not read the account's capes"));
        }

        let profile: ProfileCapes = response.json().await?;
        Ok(profile.capes)
    }

    /// Wears `cape_id`.
    pub async fn wear_cape(&self, account: &Account, cape_id: &str) -> Result<()> {
        let response = self
            .http
            .put(ACTIVE_CAPE_URL)
            .bearer_auth(&account.access_token)
            .json(&serde_json::json!({ "capeId": cape_id }))
            .send()
            .await?;

        Self::check(response, "could not put that cape on").await
    }

    /// Takes off whatever cape is being worn. Not a deletion — the cape stays
    /// owned and can be worn again.
    pub async fn hide_cape(&self, account: &Account) -> Result<()> {
        let response = self
            .http
            .delete(ACTIVE_CAPE_URL)
            .bearer_auth(&account.access_token)
            .send()
            .await?;

        Self::check(response, "could not take that cape off").await
    }

    /// Uploads a skin file.
    ///
    /// Sent as multipart rather than by URL, so a local PNG works without
    /// having to host it somewhere first.
    pub async fn upload_skin(
        &self,
        account: &Account,
        file: &Path,
        model: SkinModel,
    ) -> Result<()> {
        let bytes = tokio::fs::read(file).await.ctx(file)?;

        // Rejected server-side otherwise, with a far less helpful message.
        if crate::skin::Skin::decode(&bytes, model).is_err() {
            return Err(Error::invalid(
                "that file isn't a valid Minecraft skin — it must be a 64×64 PNG",
            ));
        }

        let file_name = file
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "skin.png".to_string());

        let form = reqwest::multipart::Form::new()
            .text(
                "variant",
                match model {
                    SkinModel::Classic => "classic",
                    SkinModel::Slim => "slim",
                },
            )
            .part(
                "file",
                reqwest::multipart::Part::bytes(bytes)
                    .file_name(file_name)
                    // Mojang rejects the upload without this.
                    .mime_str("image/png")
                    .map_err(|err| Error::invalid(format!("invalid skin upload: {err}")))?,
            );

        let response = self
            .http
            .post(SKINS_URL)
            .bearer_auth(&account.access_token)
            .multipart(form)
            .send()
            .await?;

        Self::check(response, "could not change the skin").await
    }

    /// Puts the account back on its default skin.
    pub async fn reset_skin(&self, account: &Account) -> Result<()> {
        let response = self
            .http
            .delete(ACTIVE_SKIN_URL)
            .bearer_auth(&account.access_token)
            .send()
            .await?;

        Self::check(response, "could not reset the skin").await
    }

    /// Turns a non-success response into a message worth showing, keeping the
    /// expired-token case distinct so the caller can refresh and retry.
    async fn check(response: reqwest::Response, context: &str) -> Result<()> {
        if response.status().is_success() {
            return Ok(());
        }
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(Error::auth("this account's session expired — sign in again"));
        }

        let detail = response.text().await.unwrap_or_default();
        Err(Error::invalid(if detail.is_empty() {
            context.to_string()
        } else {
            format!("{context}: {detail}")
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cape(state: &str, alias: &str) -> Cape {
        Cape {
            id: "abc".into(),
            alias: alias.into(),
            url: "https://example.invalid/cape.png".into(),
            state: state.into(),
        }
    }

    #[test]
    fn only_an_active_cape_counts_as_worn() {
        assert!(cape("ACTIVE", "Migrator").is_active());
        assert!(cape("active", "Migrator").is_active());
        // Owned but not worn.
        assert!(!cape("INACTIVE", "Migrator").is_active());
        assert!(!cape("", "Migrator").is_active());
    }

    #[test]
    fn capes_without_an_alias_still_have_a_label() {
        assert_eq!(cape("ACTIVE", "Migrator").label(), "Migrator");
        assert_eq!(cape("ACTIVE", "").label(), "Cape");
    }
}
