//! PaperMC "Fill" API client (`fill.papermc.io/v3`). The older
//! `api.papermc.io/v2` is retired — confirmed 410 Gone live on 2026-08-22,
//! not assumed. See `Mod/ROADMAP.md` Phase 7 and `PaperApiClient.java`, the
//! Java twin of this module.

use crate::error::{Error, Result};
use crate::minecraft::download::DownloadTask;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

const API_BASE: &str = "https://fill.papermc.io/v3";
/// Fill's docs ask for a descriptive `User-Agent`; same format `modrinth.rs` sends.
const USER_AGENT: &str = concat!(
    "Lokifisch/nexo-client/",
    env!("CARGO_PKG_VERSION"),
    " (github.com/Lokifisch/nexo-client)"
);

#[derive(Debug, Clone)]
pub struct PaperApi {
    http: reqwest::Client,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Build {
    pub id: u32,
    pub channel: String,
    pub downloads: HashMap<String, Download>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Download {
    pub name: String,
    pub url: String,
    pub checksums: Checksums,
    pub size: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Checksums {
    pub sha256: String,
}

impl Build {
    pub fn server_jar(&self) -> Result<&Download> {
        self.downloads.get("server:default").ok_or_else(|| {
            Error::invalid(format!(
                "Paper build {} has no server:default download",
                self.id
            ))
        })
    }
}

impl PaperApi {
    pub fn new() -> Result<Self> {
        let http = reqwest::Client::builder().user_agent(USER_AGENT).build()?;
        Ok(Self { http })
    }

    /// Shares an existing client's connection pool.
    pub fn with_client(http: reqwest::Client) -> Self {
        Self { http }
    }

    /// Newest first. Empty (not an error) if this Minecraft version has no
    /// published Paper build yet — Paper genuinely lags Mojang releases,
    /// and this should be modelled as a real, surfaced state, not a crash.
    pub async fn stable_builds(&self, minecraft_version: &str) -> Result<Vec<Build>> {
        let response = self
            .http
            .get(format!(
                "{API_BASE}/projects/paper/versions/{minecraft_version}/builds"
            ))
            .query(&[("channel", "STABLE")])
            .send()
            .await?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }
        let mut builds: Vec<Build> = response.error_for_status()?.json().await?;
        builds.sort_by(|a, b| b.id.cmp(&a.id));
        Ok(builds)
    }

    pub async fn latest_stable_build(&self, minecraft_version: &str) -> Result<Build> {
        self.stable_builds(minecraft_version)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| {
                Error::invalid(format!(
                    "no published Paper build for Minecraft {minecraft_version} yet"
                ))
            })
    }

    /// A `DownloadTask` for this build's server jar, ready to feed to
    /// `Downloader::run` — the same pipeline every other download in this
    /// crate uses, verified checksum and atomic write included.
    pub fn download_task(&self, build: &Build, dest: PathBuf) -> Result<DownloadTask> {
        let jar = build.server_jar()?;
        Ok(DownloadTask {
            url: jar.url.clone(),
            dest,
            sha1: None,
            sha256: Some(jar.checksums.sha256.clone()),
            size: jar.size,
        })
    }
}
