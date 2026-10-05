//! PaperMC's own plugin repository (hangar.papermc.io) — verified live
//! against the real API on 2026-08-22. Mirrors `modrinth.rs`'s shape;
//! mirrors `HangarClient.java` field-for-field on the wire.
//!
//! Headless only for now, same as the rest of `nexo-core` — no
//! `nexo-app` screen consumes this yet. The Mod side has a working plugin
//! browser (`NexoPaperPluginBrowserScreen`); the launcher's own UI for it is
//! deferred rather than rushed to parity, the same call already made for
//! the live RCON console (see `screens::paper_servers` doc comment).
//! Installing a plugin is arbitrary code execution — any UI built on this
//! must surface `Version::author`/`review_state` before installing, not
//! just before downloading.

use crate::error::{Error, Result};
use crate::minecraft::download::DownloadTask;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

const API_BASE: &str = "https://hangar.papermc.io/api/v1";
const USER_AGENT: &str = concat!(
    "Lokifisch/nexo-client/",
    env!("CARGO_PKG_VERSION"),
    " (github.com/Lokifisch/nexo-client)"
);

#[derive(Debug, Clone)]
pub struct Hangar {
    http: reqwest::Client,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Namespace {
    pub owner: String,
    pub slug: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Project {
    pub id: u64,
    pub name: String,
    pub namespace: Namespace,
    pub category: String,
    pub description: String,
    pub visibility: String,
}

#[derive(Debug, Clone, Deserialize)]
struct SearchResult {
    result: Vec<Project>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileInfo {
    pub name: String,
    pub size_bytes: u64,
    pub sha256_hash: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Download {
    pub file_info: FileInfo,
    pub download_url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Version {
    pub id: u64,
    pub name: String,
    pub author: String,
    pub review_state: String,
    pub visibility: String,
    pub downloads: HashMap<String, Download>,
}

impl Version {
    pub fn paper_download(&self) -> Result<&Download> {
        self.downloads.get("PAPER").ok_or_else(|| {
            Error::invalid(format!(
                "Hangar version {} has no PAPER download",
                self.name
            ))
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
struct VersionSearchResult {
    result: Vec<Version>,
}

impl Hangar {
    pub fn new() -> Result<Self> {
        let http = reqwest::Client::builder().user_agent(USER_AGENT).build()?;
        Ok(Self { http })
    }

    pub fn with_client(http: reqwest::Client) -> Self {
        Self { http }
    }

    pub async fn search(&self, query: &str, limit: u32) -> Result<Vec<Project>> {
        let response = self
            .http
            .get(format!("{API_BASE}/projects"))
            .query(&[("query", query), ("limit", &limit.to_string())])
            .send()
            .await?
            .error_for_status()?;
        Ok(response.json::<SearchResult>().await?.result)
    }

    /// Newest first, per Hangar's own ordering.
    pub async fn versions(&self, owner: &str, slug: &str, limit: u32) -> Result<Vec<Version>> {
        let response = self
            .http
            .get(format!("{API_BASE}/projects/{owner}/{slug}/versions"))
            .query(&[("limit", limit.to_string())])
            .send()
            .await?
            .error_for_status()?;
        Ok(response.json::<VersionSearchResult>().await?.result)
    }

    /// The first version (newest) that actually has a PAPER build.
    pub async fn latest_paper_version(&self, owner: &str, slug: &str) -> Result<Version> {
        self.versions(owner, slug, 10)
            .await?
            .into_iter()
            .find(|v| v.downloads.contains_key("PAPER"))
            .ok_or_else(|| Error::invalid(format!("no build of {owner}/{slug} supports Paper")))
    }

    /// A `DownloadTask` for this version's plugin jar, ready to feed to
    /// `Downloader::run` — same pipeline every other download in this crate
    /// uses, verified checksum and atomic write included.
    pub fn download_task(&self, version: &Version, dest: PathBuf) -> Result<DownloadTask> {
        let download = version.paper_download()?;
        Ok(DownloadTask {
            url: download.download_url.clone(),
            dest,
            sha1: None,
            sha256: Some(download.file_info.sha256_hash.clone()),
            size: download.file_info.size_bytes,
        })
    }
}
