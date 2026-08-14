//! Microsoft account sign-in via the OAuth2 authorization-code flow.
//!
//! The user clicks sign in, their normal browser opens on Microsoft's login
//! page, and when they finish, Microsoft redirects to a one-shot HTTP server
//! this process runs on loopback. Nothing to read off the screen and retype —
//! the same shape Modrinth's launcher and `Mod/`'s in-game sign-in use.
//!
//! Four hops, all required, in this order:
//!
//! 1. **MSA** — browser login, redirect captured, code exchanged for a token.
//! 2. **Xbox Live** — the MSA token is exchanged for an XBL token.
//! 3. **XSTS** — the XBL token is authorized against Minecraft's relying
//!    party. This is the step that rejects accounts with no Xbox profile or
//!    a child account, and it reports why via an `XErr` code.
//! 4. **Minecraft Services** — the XSTS token becomes a Minecraft bearer
//!    token, which finally lets us read the profile the game launches with.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Azure application id, shared with `Mod/`'s in-game sign-in so both halves
/// of the project present the same consent screen.
const CLIENT_ID: &str = "e16699bb-2aa8-46da-b5e3-45cbcce29091";

/// **Fixed, not arbitrary.** Azure matches redirect URIs exactly, and this
/// exact host/port/path is what's registered against [`CLIENT_ID`]. Changing
/// the port here without changing the registration breaks sign-in. It also
/// means only one sign-in can be in flight per machine — a second one, or a
/// concurrent in-game sign-in from `Mod/`, will fail to bind.
const CALLBACK_PORT: u16 = 25585;
const REDIRECT_URI: &str = "http://localhost:25585/callback";

/// `consumers` (not `common`) because Minecraft accounts are personal
/// Microsoft accounts; the tenant endpoints reject them.
const AUTHORIZE_URL: &str =
    "https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize";
const TOKEN_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
const XBL_AUTH_URL: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS_AUTH_URL: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const MC_LOGIN_URL: &str = "https://api.minecraftservices.com/authentication/login_with_xbox";
const MC_PROFILE_URL: &str = "https://api.minecraftservices.com/minecraft/profile";

/// `offline_access` is what gets us a refresh token; without it the user
/// would have to sign in again on every launch.
const SCOPE: &str = "XboxLive.signin offline_access";

/// Always show the account picker, even when the browser already has a live
/// Microsoft session.
///
/// Without this, Microsoft silently signs the browser's current account in
/// again, so adding a *second* account meant leaving Nexo, signing out at
/// microsoft.com, coming back, and signing in — every time. Since Nexo keeps
/// several accounts and exists partly to switch between them, picking one is
/// the normal case here, not the exception.
///
/// Deliberately not `login`, which would force a full re-authentication and
/// make people retype a password they have already given.
///
/// Must stay in step with `Mod/`'s `MicrosoftAuth.java`: both halves sign into
/// the same account store, and an inconsistency would be the kind of thing
/// nobody notices until they hit it in-game.
const PROMPT: &str = "select_account";

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
    /// URL of the account's active skin texture, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skin_url: Option<String>,
    /// `CLASSIC` (4px arms) or `SLIM` (3px arms) — changes how the body is
    /// drawn, so it has to survive alongside the URL.
    #[serde(default)]
    pub skin_model: SkinModel,
    /// Active cape texture, when the account has one equipped. Most accounts
    /// have none, and an equipped cape can be unequipped, so this is
    /// genuinely optional rather than merely sometimes-unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cape_url: Option<String>,
}

impl Account {
    /// Treats a token as expired slightly early so a launch can't start with
    /// a token that dies mid-handshake.
    pub fn is_expired(&self) -> bool {
        crate::instance::now() + 60 >= self.expires_at
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum SkinModel {
    #[default]
    Classic,
    Slim,
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

    /// Runs the whole sign-in and returns a ready-to-launch account.
    ///
    /// `open_url` is called once the loopback listener is actually bound,
    /// with the URL to send the user to. Taking it as a callback rather than
    /// opening a browser here keeps this crate free of any desktop
    /// dependency, and guarantees we're already listening before the browser
    /// can possibly redirect back.
    pub async fn login<F>(&self, open_url: F) -> Result<Account>
    where
        F: FnOnce(&str) + Send,
    {
        // Bind first: if the port is taken, fail before sending the user off
        // to a browser page whose redirect could never be received.
        let listener = TcpListener::bind(("127.0.0.1", CALLBACK_PORT))
            .await
            .map_err(|err| {
                Error::auth(format!(
                    "couldn't listen on port {CALLBACK_PORT} for the sign-in redirect \
                     ({err}) — another sign-in may already be in progress"
                ))
            })?;

        // Guards against a stray or forged request hitting the callback.
        let state = uuid::Uuid::new_v4().to_string();
        open_url(&authorize_url(&state));

        let code = wait_for_code(&listener, &state).await?;
        let msa = self.exchange_code(&code).await?;
        self.complete_login(msa).await
    }

    async fn exchange_code(&self, code: &str) -> Result<MsaToken> {
        let response = self
            .http
            .post(TOKEN_URL)
            .form(&[
                ("client_id", CLIENT_ID),
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", REDIRECT_URI),
            ])
            .send()
            .await?;

        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Error::auth(format!(
                "Microsoft rejected the sign-in: {body}"
            )));
        }
        Ok(response.json().await?)
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
        // Both read the profile by reference, so they must be taken before
        // the fields below are moved out of it.
        let skin = profile.active_skin();
        let cape_url = profile.active_cape().map(|c| c.url);

        Ok(Account {
            uuid: profile.id,
            username: profile.name,
            access_token: mc_token.access_token,
            refresh_token: msa.refresh_token,
            expires_at: crate::instance::now() + mc_token.expires_in,
            skin_url: skin.as_ref().map(|s| s.url.clone()),
            skin_model: skin.map(|s| s.model()).unwrap_or_default(),
            cape_url,
        })
    }

    /// Renews an account without user interaction. The error here should be
    /// treated as "make them sign in again" — it means the refresh token was
    /// revoked, e.g. by a password change.
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
            return Err(Error::auth("this account's sign-in expired — sign in again"));
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

        let response = self.http.post(MC_LOGIN_URL).json(&body).send().await?;

        if !response.status().is_success() {
            return Err(Error::auth("Minecraft services rejected this account"));
        }
        Ok(response.json().await?)
    }

    /// Re-reads the account's cosmetics and returns it updated.
    ///
    /// Skins and capes change independently of tokens — equipping a cape
    /// should show up without signing in again — and an account stored by an
    /// older build may predate these fields entirely.
    pub async fn sync_profile(&self, account: &Account) -> Result<Account> {
        let profile = self.profile(&account.access_token).await?;
        let skin = profile.active_skin();

        Ok(Account {
            skin_url: skin.as_ref().map(|s| s.url.clone()),
            skin_model: skin.map(|s| s.model()).unwrap_or_default(),
            // Deliberately overwrites with `None` when nothing is equipped, so
            // taking a cape off in game is reflected here too.
            cape_url: profile.active_cape().map(|c| c.url),
            ..account.clone()
        })
    }

    async fn profile(&self, mc_access_token: &str) -> Result<McProfile> {
        let response = self
            .http
            .get(MC_PROFILE_URL)
            .bearer_auth(mc_access_token)
            .send()
            .await?;

        // A 404 means the account authenticated fine but owns no copy of the
        // game — worth saying plainly rather than "profile missing".
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(Error::auth("this account doesn't own Minecraft: Java Edition"));
        }
        if !response.status().is_success() {
            return Err(Error::auth("could not read the Minecraft profile"));
        }
        Ok(response.json().await?)
    }
}

fn authorize_url(state: &str) -> String {
    format!(
        "{AUTHORIZE_URL}?client_id={}&response_type=code&redirect_uri={}&scope={}&state={}&prompt={PROMPT}",
        urlencode(CLIENT_ID),
        urlencode(REDIRECT_URI),
        urlencode(SCOPE),
        urlencode(state),
    )
}

/// Percent-encodes everything outside the unreserved set. Small enough to not
/// warrant a dependency, and the inputs here are all known-simple.
fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Accepts connections until one is Microsoft's redirect carrying a matching
/// `state`, then returns its authorization code.
///
/// Loops rather than taking the first connection because browsers open
/// speculative and favicon requests that would otherwise be mistaken for the
/// callback.
async fn wait_for_code(listener: &TcpListener, expected_state: &str) -> Result<String> {
    loop {
        let (mut socket, _) = listener
            .accept()
            .await
            .map_err(|err| Error::auth(format!("sign-in listener failed: {err}")))?;

        let mut buffer = vec![0u8; 8192];
        let read = socket.read(&mut buffer).await.unwrap_or(0);
        if read == 0 {
            continue;
        }

        let request = String::from_utf8_lossy(&buffer[..read]);
        let Some(target) = request_target(&request) else {
            continue;
        };
        if !target.starts_with("/callback") {
            let _ = respond(&mut socket, 404, "Not found").await;
            continue;
        }

        let params = query_params(target);
        let result = match (
            params.get("code"),
            params.get("state"),
            params.get("error_description").or(params.get("error")),
        ) {
            (_, _, Some(error)) => Err(Error::auth(error.clone())),
            (Some(code), Some(state), _) if state == expected_state => Ok(code.clone()),
            // A mismatched state means this redirect didn't originate from
            // the request we made, so the code must not be trusted.
            (Some(_), _, _) => Err(Error::auth("sign-in state mismatch — please try again")),
            _ => Err(Error::auth("Microsoft didn't return an authorization code")),
        };

        let page = match &result {
            Ok(_) => success_page(),
            Err(err) => error_page(&err.to_string()),
        };
        let _ = respond(&mut socket, 200, &page).await;

        return result;
    }
}

/// Pulls the path+query out of the request line (`GET /callback?... HTTP/1.1`).
fn request_target(request: &str) -> Option<&str> {
    request.lines().next()?.split_whitespace().nth(1)
}

fn query_params(target: &str) -> std::collections::HashMap<String, String> {
    let Some((_, query)) = target.split_once('?') else {
        return std::collections::HashMap::new();
    };

    query
        .split('&')
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            Some((key.to_string(), urldecode(value)))
        })
        .collect()
}

fn urldecode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

async fn respond(socket: &mut tokio::net::TcpStream, status: u16, body: &str) -> Result<()> {
    let reason = if status == 200 { "OK" } else { "Not Found" };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
    socket.write_all(response.as_bytes()).await?;
    socket.flush().await?;
    Ok(())
}

/// Styled to match the app, since this page is the last thing the user sees
/// before switching back to it.
fn success_page() -> String {
    landing_page(
        "#3cffb0",
        "Signed in",
        "You can close this tab and go back to Nexo.",
    )
}

fn error_page(message: &str) -> String {
    landing_page("#ff4d6a", "Sign-in failed", message)
}

fn landing_page(accent: &str, heading: &str, detail: &str) -> String {
    let detail = detail
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <title>Nexo</title></head>\
         <body style=\"margin:0;display:flex;align-items:center;justify-content:center;\
         height:100vh;background:#0d0d14;color:#e8e8f2;\
         font-family:system-ui,-apple-system,Segoe UI,sans-serif\">\
         <div style=\"text-align:center;padding:2rem\">\
         <div style=\"font-size:2rem;font-weight:600;color:{accent}\">{heading}</div>\
         <p style=\"color:#9a9ab4;margin-top:.75rem\">{detail}</p></div></body></html>"
    )
}

#[derive(Debug, Deserialize)]
struct MsaToken {
    access_token: String,
    #[serde(default)]
    refresh_token: String,
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
    #[serde(default)]
    skins: Vec<ProfileSkin>,
    #[serde(default)]
    capes: Vec<ProfileCape>,
}

impl McProfile {
    /// An account can carry several skins; exactly one is `ACTIVE`.
    fn active_skin(&self) -> Option<ProfileSkin> {
        self.skins
            .iter()
            .find(|s| s.state.eq_ignore_ascii_case("ACTIVE"))
            .or_else(|| self.skins.first())
            .cloned()
    }

    /// Owned capes are all listed; only an equipped one is `ACTIVE`. Unlike
    /// skins there's deliberately no fallback to the first entry — showing an
    /// unequipped cape would misrepresent what the player looks like in game.
    fn active_cape(&self) -> Option<ProfileCape> {
        self.capes
            .iter()
            .find(|c| c.state.eq_ignore_ascii_case("ACTIVE"))
            .cloned()
    }
}

#[derive(Debug, Clone, Deserialize)]
struct ProfileCape {
    url: String,
    #[serde(default)]
    state: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ProfileSkin {
    url: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    variant: String,
}

impl ProfileSkin {
    fn model(&self) -> SkinModel {
        if self.variant.eq_ignore_ascii_case("SLIM") {
            SkinModel::Slim
        } else {
            SkinModel::Classic
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_callback_query() {
        let params = query_params("/callback?code=abc123&state=xyz");
        assert_eq!(params.get("code").unwrap(), "abc123");
        assert_eq!(params.get("state").unwrap(), "xyz");
    }

    #[test]
    fn decodes_percent_escapes_in_callback() {
        let params = query_params("/callback?error_description=Bad+thing%20happened");
        assert_eq!(params.get("error_description").unwrap(), "Bad thing happened");
    }

    #[test]
    fn extracts_request_target() {
        assert_eq!(
            request_target("GET /callback?code=1 HTTP/1.1\r\nHost: x\r\n"),
            Some("/callback?code=1")
        );
    }

    #[test]
    fn authorize_url_encodes_redirect_and_scope() {
        let url = authorize_url("state-1");
        assert!(url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A25585%2Fcallback"));
        assert!(url.contains("scope=XboxLive.signin%20offline_access"));
        assert!(url.contains("response_type=code"));
    }

    /// Without this, a live browser session is reused silently and the only
    /// way to add a second account is to go and sign out at microsoft.com.
    #[test]
    fn authorize_url_always_offers_the_account_picker() {
        assert!(authorize_url("state-1").contains("prompt=select_account"));
    }

    #[test]
    fn error_page_escapes_injected_markup() {
        let page = error_page("<script>alert(1)</script>");
        assert!(!page.contains("<script>"));
        assert!(page.contains("&lt;script&gt;"));
    }
}
