//! Microsoft account sign-in via the OAuth2 device-code flow.
//!
//! Four hops, all required, in this order:
//!
//! 1. **MSA** — device code issued, user approves in a browser, we poll for
//!    an access token.
//! 2. **Xbox Live** — the MSA token is exchanged for an XBL token.
//! 3. **XSTS** — the XBL token is authorized against Minecraft's relying
//!    party. This is the step that rejects accounts with no Xbox profile or
//!    an unmigrated/child account, and it reports why via an `XErr` code.
//! 4. **Minecraft Services** — the XSTS token becomes a Minecraft bearer
//!    token, which finally lets us read the profile (uuid + username) the
//!    game is launched with.
//!
//! Device code rather than a redirect-URI flow specifically because it needs
//! no loopback HTTP server and no custom URL scheme registration — the user
//! opens a page, types a short code, and we poll. Same flow the vanilla
//! launcher uses on consoles and the same one `Mod/`'s in-game sign-in uses.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Azure application id. Shared with `Mod/`'s in-game sign-in so both halves
/// of the project present the same consent screen. Registering a separate
/// app id for the launcher is fine too — it only has to be an Azure app with
/// the `XboxLive.signin` delegated permission and device-code flow enabled.
const CLIENT_ID: &str = "e16699bb-2aa8-46da-b5e3-45cbcce29091";

/// `consumers` (not `common`) because Minecraft accounts are personal
/// Microsoft accounts; the tenant endpoints reject them.
const DEVICE_CODE_URL: &str =
    "https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode";
const TOKEN_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
const XBL_AUTH_URL: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS_AUTH_URL: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const MC_LOGIN_URL: &str = "https://api.minecraftservices.com/authentication/login_with_xbox";
const MC_PROFILE_URL: &str = "https://api.minecraftservices.com/minecraft/profile";

/// `offline_access` is what gets us a refresh token; without it the user
/// would have to re-approve on every launch.
const SCOPE: &str = "XboxLive.signin offline_access";

/// What the UI shows the user while we poll.
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    /// The short code the user types, e.g. `A1B2C3D4`.
    pub user_code: String,
    /// Where they type it — normally <https://microsoft.com/link>.
    pub verification_uri: String,
    pub expires_in: u64,
    /// Seconds Microsoft asks us to wait between polls.
    pub interval: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    /// Minecraft profile UUID, dashless as the game expects it.
    pub uuid: String,
    pub username: String,
    pub access_token: String,
    /// MSA refresh token, used to renew without user interaction.
    pub refresh_token: String,
    /// Unix seconds at which `access_token` stops working.
    pub expires_at: u64,
}

impl Account {
    /// Treats a token as expired slightly early so a launch can't start with
    /// a token that dies mid-handshake.
    pub fn is_expired(&self) -> bool {
        crate::instance::now() + 60 >= self.expires_at
    }
}

#[derive(Debug, Clone)]
pub struct Auth {
    http: reqwest::Client,
}

impl Default for Auth {
    fn default() -> Self {
        Self::new()
    }
}

impl Auth {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::new(),
        }
    }

    pub fn with_client(http: reqwest::Client) -> Self {
        Self { http }
    }

    /// Step 1. Ask Microsoft for a code to show the user.
    pub async fn start_device_code(&self) -> Result<DeviceCode> {
        let response = self
            .http
            .post(DEVICE_CODE_URL)
            .form(&[("client_id", CLIENT_ID), ("scope", SCOPE)])
            .send()
            .await?;

        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Error::auth(format!("Microsoft rejected the request: {body}")));
        }
        Ok(response.json().await?)
    }

    /// Steps 1b–4. Polls until the user approves, then runs the full token
    /// exchange and returns a ready-to-launch account.
    ///
    /// `on_tick` fires once per poll so the UI can show a countdown; return
    /// `false` from it to cancel.
    pub async fn poll_for_account<F>(&self, code: &DeviceCode, mut on_tick: F) -> Result<Account>
    where
        F: FnMut() -> bool,
    {
        let msa = self.poll_for_msa_token(code, &mut on_tick).await?;
        self.complete_login(msa).await
    }

    async fn poll_for_msa_token<F>(&self, code: &DeviceCode, on_tick: &mut F) -> Result<MsaToken>
    where
        F: FnMut() -> bool,
    {
        // Microsoft's `interval` is a floor, not a suggestion — polling
        // faster earns a `slow_down` and a longer wait.
        let mut interval = Duration::from_secs(code.interval.max(1));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(code.expires_in);

        loop {
            tokio::time::sleep(interval).await;

            if !on_tick() {
                return Err(Error::auth("sign-in cancelled"));
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::auth("the sign-in code expired — start again"));
            }

            let response = self
                .http
                .post(TOKEN_URL)
                .form(&[
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                    ("client_id", CLIENT_ID),
                    ("device_code", &code.device_code),
                ])
                .send()
                .await?;

            if response.status().is_success() {
                return Ok(response.json().await?);
            }

            let error: TokenError = response.json().await.unwrap_or(TokenError {
                error: "unknown_error".into(),
                error_description: None,
            });

            match error.error.as_str() {
                // Expected while the user is still typing the code.
                "authorization_pending" => continue,
                "slow_down" => {
                    interval += Duration::from_secs(5);
                    continue;
                }
                "expired_token" => {
                    return Err(Error::auth("the sign-in code expired — start again"));
                }
                "authorization_declined" => {
                    return Err(Error::auth("sign-in was declined"));
                }
                other => {
                    return Err(Error::auth(
                        error.error_description.unwrap_or_else(|| other.to_string()),
                    ));
                }
            }
        }
    }

    /// Steps 2–4, shared by first sign-in and silent refresh.
    async fn complete_login(&self, msa: MsaToken) -> Result<Account> {
        let xbl = self.xbox_live(&msa.access_token).await?;
        let user_hash = xbl
            .display_claims
            .user_hash()
            .ok_or_else(|| Error::auth("Xbox Live returned no user hash"))?
            .to_string();

        let xsts = self.xsts(&xbl.token).await?;
        let mc_token = self.minecraft_token(&user_hash, &xsts.token).await?;
        let profile = self.profile(&mc_token.access_token).await?;

        Ok(Account {
            uuid: profile.id,
            username: profile.name,
            access_token: mc_token.access_token,
            refresh_token: msa.refresh_token,
            expires_at: crate::instance::now() + mc_token.expires_in,
        })
    }

    /// Renews an account without user interaction. Falls back to an error the
    /// UI should treat as "make them sign in again" if the refresh token has
    /// been revoked (password change, consent withdrawn).
    pub async fn refresh(&self, account: &Account) -> Result<Account> {
        let response = self
            .http
            .post(TOKEN_URL)
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", CLIENT_ID),
                ("scope", SCOPE),
                ("refresh_token", &account.refresh_token),
            ])
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(Error::auth(
                "this account's sign-in expired — sign in again",
            ));
        }

        let msa: MsaToken = response.json().await?;
        self.complete_login(msa).await
    }

    async fn xbox_live(&self, msa_access_token: &str) -> Result<XboxResponse> {
        let body = serde_json::json!({
            "Properties": {
                "AuthMethod": "RPS",
                "SiteName": "user.auth.xboxlive.com",
                // The `d=` prefix marks this as a raw MSA access token.
                "RpsTicket": format!("d={msa_access_token}"),
            },
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT",
        });

        let response = self
            .http
            .post(XBL_AUTH_URL)
            .header("Accept", "application/json")
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(Error::auth("Xbox Live rejected this account"));
        }
        Ok(response.json().await?)
    }

    async fn xsts(&self, xbl_token: &str) -> Result<XboxResponse> {
        let body = serde_json::json!({
            "Properties": {
                "SandboxId": "RETAIL",
                "UserTokens": [xbl_token],
            },
            "RelyingParty": "rp://api.minecraftservices.com/",
            "TokenType": "JWT",
        });

        let response = self
            .http
            .post(XSTS_AUTH_URL)
            .header("Accept", "application/json")
            .json(&body)
            .send()
            .await?;

        if response.status().is_success() {
            return Ok(response.json().await?);
        }

        // XSTS is where the informative failures live; the numeric XErr is
        // the only way to tell "no Xbox account" from "child account".
        let err: XstsError = response.json().await.unwrap_or_default();
        Err(Error::auth(match err.x_err {
            2148916233 => "this Microsoft account has no Xbox profile — create one at xbox.com, then try again".to_string(),
            2148916235 => "Xbox Live isn't available in this account's country/region".to_string(),
            2148916236 | 2148916237 => "this account needs adult verification before it can sign in".to_string(),
            2148916238 => "this is a child account — it must be added to a Microsoft Family group before it can sign in".to_string(),
            other => format!("Xbox sign-in failed (error {other})"),
        }))
    }

    async fn minecraft_token(&self, user_hash: &str, xsts_token: &str) -> Result<McToken> {
        let body = serde_json::json!({
            "identityToken": format!("XBL3.0 x={user_hash};{xsts_token}"),
        });

        let response = self
            .http
            .post(MC_LOGIN_URL)
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(Error::auth("Minecraft services rejected this account"));
        }
        Ok(response.json().await?)
    }

    async fn profile(&self, mc_access_token: &str) -> Result<McProfile> {
        let response = self
            .http
            .get(MC_PROFILE_URL)
            .bearer_auth(mc_access_token)
            .send()
            .await?;

        // A 404 here means the account authenticated fine but owns no copy
        // of the game — worth saying plainly rather than "profile missing".
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(Error::auth(
                "this account doesn't own Minecraft: Java Edition",
            ));
        }
        if !response.status().is_success() {
            return Err(Error::auth("could not read the Minecraft profile"));
        }
        Ok(response.json().await?)
    }
}

#[derive(Debug, Deserialize)]
struct MsaToken {
    access_token: String,
    #[serde(default)]
    refresh_token: String,
}

#[derive(Debug, Deserialize)]
struct TokenError {
    error: String,
    error_description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct XboxResponse {
    #[serde(rename = "Token")]
    token: String,
    #[serde(rename = "DisplayClaims")]
    display_claims: DisplayClaims,
}

#[derive(Debug, Deserialize)]
struct DisplayClaims {
    #[serde(default)]
    xui: Vec<Xui>,
}

impl DisplayClaims {
    fn user_hash(&self) -> Option<&str> {
        self.xui.first().map(|x| x.uhs.as_str())
    }
}

#[derive(Debug, Deserialize)]
struct Xui {
    uhs: String,
}

#[derive(Debug, Default, Deserialize)]
struct XstsError {
    #[serde(rename = "XErr", default)]
    x_err: u64,
}

#[derive(Debug, Deserialize)]
struct McToken {
    access_token: String,
    expires_in: u64,
}

#[derive(Debug, Deserialize)]
struct McProfile {
    id: String,
    name: String,
}
