//! The slice of GitHub's REST API that release tracking needs.
//!
//! Nexo reads releases from two repositories: `Lokifisch/nexo-mod` for the mod
//! ([`crate::nexo_mod`]) and `Lokifisch/nexo-client` for the launcher itself
//! ([`crate::self_update`]). Both ask the same question of the same endpoint,
//! so the request shape, the JSON, and — the part worth not duplicating — the
//! rate-limit translation live here once.

use crate::error::{Error, Result};
use serde::Deserialize;

/// Requests are unauthenticated, which GitHub budgets at 60 an hour per IP.
/// Every call site should therefore treat a failed lookup as normal rather
/// than exceptional.
const API: &str = "https://api.github.com";

/// One published release, with the fields Nexo actually reads.
///
/// Everything past `tag_name` is `#[serde(default)]`: GitHub adds fields far
/// more often than it removes them, but a release created through some path
/// that omits one shouldn't take the whole lookup down.
#[derive(Debug, Clone, Deserialize)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
    #[serde(default)]
    pub size: u64,
}

impl Release {
    /// Exact-name asset lookup. Deliberately not a prefix or suffix match:
    /// both consumers resolve an asset they can name in advance, and "close
    /// enough" would mean downloading a file nobody asked for.
    pub fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == name)
    }
}

/// Every release of `repo`, newest first, prereleases included.
///
/// `/releases/latest` is deliberately not used anywhere in Nexo: it skips
/// prereleases, and every Nexo release so far is one, so it would report
/// "no releases" for a repo full of them.
pub async fn releases(http: &reqwest::Client, repo: &str, per_page: u32) -> Result<Vec<Release>> {
    Ok(status(
        http.get(format!("{API}/repos/{repo}/releases?per_page={per_page}"))
            .header("Accept", "application/vnd.github+json")
            .send()
            .await?,
    )?
    .json()
    .await?)
}

/// Turns GitHub's rate-limit refusal into something the user can act on.
///
/// Unauthenticated requests get 60 an hour per IP, and GitHub spends that
/// budget on a bare 403 whose body the UI never shows. Passed through
/// `error_for_status`, a condition that clears itself in a known number of
/// minutes reaches the user as "HTTP status client error (403 Forbidden)" —
/// indistinguishable from the repository having gone away.
///
/// Only a 403/429 that GitHub *also* marks as having no quota left is
/// rewritten. A genuine permission failure keeps its own error.
pub fn status(response: reqwest::Response) -> Result<reqwest::Response> {
    let code = response.status();
    let rate_limited = matches!(
        code,
        reqwest::StatusCode::FORBIDDEN | reqwest::StatusCode::TOO_MANY_REQUESTS
    ) && response
        .headers()
        .get("x-ratelimit-remaining")
        .and_then(|v| v.to_str().ok())
        == Some("0");

    if !rate_limited {
        return Ok(response.error_for_status()?);
    }

    // The reset header is advisory: if it's missing or already in the past,
    // say so without a number rather than printing "in 0 minutes".
    let retry_in = response
        .headers()
        .get("x-ratelimit-reset")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .and_then(|reset| {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_secs();
            reset.checked_sub(now).filter(|secs| *secs > 0)
        })
        .map(|secs| format!(" Try again in about {} minutes.", secs.div_ceil(60)))
        .unwrap_or_else(|| " Try again shortly.".to_string());

    Err(Error::Invalid(format!(
        "GitHub is rate-limiting this machine, so the release list can't be read.{retry_in}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Building the response by hand rather than waiting to actually be
    /// rate-limited: the live test only exercises this path on the unlucky
    /// run that trips the quota, which is exactly when nobody is looking.
    fn response(code: u16, remaining: &str, reset_in: i64) -> reqwest::Response {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let raw = http::Response::builder()
            .status(code)
            .header("x-ratelimit-remaining", remaining)
            .header("x-ratelimit-reset", (now + reset_in).to_string())
            .body("")
            .unwrap();
        reqwest::Response::from(raw)
    }

    #[test]
    fn an_exhausted_quota_says_so_and_says_when() {
        let err = status(response(403, "0", 11 * 60)).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("rate-limiting"),
            "a bare 403 is indistinguishable from the repo being gone: {message}"
        );
        assert!(
            message.contains("11 minutes"),
            "the wait is the actionable part: {message}"
        );
    }

    #[test]
    fn a_403_with_quota_left_keeps_its_own_error() {
        // A real permission failure must not be relabelled as a rate limit —
        // that would send the user off to wait for something that never clears.
        let err = status(response(403, "57", 600)).unwrap_err();
        assert!(
            !err.to_string().contains("rate-limiting"),
            "only an exhausted quota is a rate limit"
        );
    }

    #[test]
    fn a_429_counts_as_the_same_condition() {
        let err = status(response(429, "0", 60)).unwrap_err();
        assert!(err.to_string().contains("rate-limiting"));
    }
}
