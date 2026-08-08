//! Fabric loader resolution via meta.fabricmc.net.
//!
//! Fabric publishes a ready-made launch profile per (game version, loader
//! version) pair — a version JSON with `inheritsFrom` set to the vanilla
//! version. That means we never have to synthesize Fabric's classpath or
//! main class ourselves; we fetch their profile and merge it onto vanilla's
//! via [`super::meta::VersionData::merge_onto`].

use crate::error::{Error, Result};
use serde::Deserialize;

const META_BASE: &str = "https://meta.fabricmc.net/v2";

#[derive(Debug, Clone, Deserialize)]
pub struct LoaderEntry {
    pub loader: LoaderVersion,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoaderVersion {
    pub version: String,
    #[serde(default)]
    pub build: u32,
    /// Fabric marks pre-release loaders unstable; a launcher should default
    /// to stable ones.
    #[serde(default)]
    pub stable: bool,
}

/// Every loader build compatible with `game_version`, newest first.
pub async fn loaders(http: &reqwest::Client, game_version: &str) -> Result<Vec<LoaderVersion>> {
    let response = http
        .get(format!("{META_BASE}/versions/loader/{game_version}"))
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(Error::invalid(format!(
            "Fabric has no builds for Minecraft {game_version}"
        )));
    }

    let entries: Vec<LoaderEntry> = response.json().await?;
    Ok(entries.into_iter().map(|e| e.loader).collect())
}

/// The loader build a new instance should get: newest stable, falling back to
/// newest of any kind if Fabric has published no stable build yet (which does
/// happen in the days after a Minecraft release).
pub async fn latest_stable(http: &reqwest::Client, game_version: &str) -> Result<String> {
    let loaders = loaders(http, game_version).await?;
    loaders
        .iter()
        .find(|l| l.stable)
        .or_else(|| loaders.first())
        .map(|l| l.version.clone())
        .ok_or_else(|| {
            Error::invalid(format!(
                "Fabric has no loader builds for Minecraft {game_version}"
            ))
        })
}

/// Fetches the launch profile for a specific loader build.
pub async fn profile(
    http: &reqwest::Client,
    game_version: &str,
    loader_version: &str,
) -> Result<super::meta::VersionData> {
    let url =
        format!("{META_BASE}/versions/loader/{game_version}/{loader_version}/profile/json");

    let response = http.get(&url).send().await?;
    if !response.status().is_success() {
        return Err(Error::invalid(format!(
            "Fabric loader {loader_version} has no profile for Minecraft {game_version}"
        )));
    }

    Ok(response.json().await?)
}
