//! Modrinth Labrinth (v2) API client.
//!
//! Read-only and unauthenticated: search, version listing, and file
//! downloads are all public endpoints, so v1 needs no API token. Modrinth
//! asks third-party consumers to send an identifying `User-Agent`
//! (<https://docs.modrinth.com/api/#user-agents>); anonymous traffic gets
//! rate-limited harder, so [`USER_AGENT`] is not optional politeness.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};

const API_BASE: &str = "https://api.modrinth.com/v2";

/// Modrinth's documented format is `owner/project/version (contact)`.
const USER_AGENT: &str = concat!(
    "Lokifisch/nexo-client/",
    env!("CARGO_PKG_VERSION"),
    " (github.com/Lokifisch/nexo-client)"
);

#[derive(Debug, Clone)]
pub struct Modrinth {
    http: reqwest::Client,
}

impl Modrinth {
    pub fn new() -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .build()?;
        Ok(Self { http })
    }

    /// Shares an existing client's connection pool. Prefer this over
    /// [`Modrinth::new`] when the caller already has one.
    pub fn with_client(http: reqwest::Client) -> Self {
        Self { http }
    }

    /// Searches projects, narrowed to what a given instance can actually
    /// install. Passing the loader and game version as facets (rather than
    /// filtering client-side) matters: an unfiltered search is mostly
    /// results the user can't use.
    pub async fn search(&self, query: &SearchQuery<'_>) -> Result<SearchResults> {
        let facets = query.facets();
        let limit = query.limit.to_string();
        let offset = query.offset.to_string();

        let mut params: Vec<(&str, &str)> = vec![
            ("query", query.text),
            ("limit", &limit),
            ("offset", &offset),
            ("index", query.sort.as_str()),
        ];
        if !facets.is_empty() {
            params.push(("facets", &facets));
        }

        let response = self
            .http
            .get(format!("{API_BASE}/search"))
            .query(&params)
            .send()
            .await?
            .error_for_status()?;

        Ok(response.json().await?)
    }

    /// Versions of one project, newest first, filtered to a loader and game
    /// version.
    pub async fn versions(
        &self,
        project_id: &str,
        loader: &str,
        game_version: &str,
    ) -> Result<Vec<Version>> {
        // These two are JSON-array-encoded query params, not repeated keys.
        let loaders = format!("[\"{loader}\"]");
        let game_versions = format!("[\"{game_version}\"]");

        let response = self
            .http
            .get(format!("{API_BASE}/project/{project_id}/version"))
            .query(&[("loaders", &loaders), ("game_versions", &game_versions)])
            .send()
            .await?
            .error_for_status()?;

        Ok(response.json().await?)
    }

    /// The version a fresh install should get: Modrinth returns versions
    /// newest-first, so this is the first entry, preferring a stable release
    /// over an alpha/beta when both are offered.
    pub async fn latest_version(
        &self,
        project_id: &str,
        loader: &str,
        game_version: &str,
    ) -> Result<Version> {
        let versions = self.versions(project_id, loader, game_version).await?;
        versions
            .iter()
            .find(|v| v.version_type == VersionType::Release)
            .or_else(|| versions.first())
            .cloned()
            .ok_or_else(|| {
                Error::invalid(format!(
                    "no {loader} build of this project supports Minecraft {game_version}"
                ))
            })
    }

    pub async fn project(&self, id_or_slug: &str) -> Result<Project> {
        let response = self
            .http
            .get(format!("{API_BASE}/project/{id_or_slug}"))
            .send()
            .await?
            .error_for_status()?;
        Ok(response.json().await?)
    }

    /// Downloads a version's primary file into memory. Jars are single-digit
    /// MB, so buffering avoids a half-written jar in the mods folder if the
    /// transfer dies partway.
    pub async fn download(&self, file: &VersionFile) -> Result<Vec<u8>> {
        let bytes = self
            .http
            .get(&file.url)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;

        if let Some(expected) = &file.hashes.sha1 {
            let actual = crate::util::sha1_hex(&bytes);
            if &actual != expected {
                return Err(Error::invalid(format!(
                    "downloaded {} is corrupt (sha1 {actual}, expected {expected})",
                    file.filename
                )));
            }
        }

        Ok(bytes.to_vec())
    }
}

#[derive(Debug, Clone)]
pub struct SearchQuery<'a> {
    pub text: &'a str,
    /// Restricts results to mods that run on this loader.
    pub loader: Option<&'a str>,
    pub game_version: Option<&'a str>,
    pub project_type: Option<&'a str>,
    pub sort: SortIndex,
    pub limit: u32,
    pub offset: u32,
}

impl Default for SearchQuery<'_> {
    fn default() -> Self {
        Self {
            text: "",
            loader: None,
            game_version: None,
            project_type: Some("mod"),
            sort: SortIndex::Relevance,
            limit: 20,
            offset: 0,
        }
    }
}

impl SearchQuery<'_> {
    /// Modrinth facets are `[[or, or], [and]]` — inner arrays are OR'd,
    /// outer entries are AND'd. Each filter here is its own outer entry
    /// because they must all hold at once.
    fn facets(&self) -> String {
        let mut groups: Vec<String> = Vec::new();
        if let Some(loader) = self.loader {
            groups.push(format!("[\"categories:{loader}\"]"));
        }
        if let Some(version) = self.game_version {
            groups.push(format!("[\"versions:{version}\"]"));
        }
        if let Some(kind) = self.project_type {
            groups.push(format!("[\"project_type:{kind}\"]"));
        }

        if groups.is_empty() {
            String::new()
        } else {
            format!("[{}]", groups.join(","))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortIndex {
    #[default]
    Relevance,
    Downloads,
    Follows,
    Newest,
    Updated,
}

impl SortIndex {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Relevance => "relevance",
            Self::Downloads => "downloads",
            Self::Follows => "follows",
            Self::Newest => "newest",
            Self::Updated => "updated",
        }
    }

    pub const ALL: [Self; 5] = [
        Self::Relevance,
        Self::Downloads,
        Self::Follows,
        Self::Newest,
        Self::Updated,
    ];
}

impl std::fmt::Display for SortIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::Relevance => "Relevance",
            Self::Downloads => "Downloads",
            Self::Follows => "Follows",
            Self::Newest => "Newest",
            Self::Updated => "Recently updated",
        };
        f.write_str(label)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchResults {
    pub hits: Vec<SearchHit>,
    pub total_hits: u32,
    pub offset: u32,
    pub limit: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchHit {
    pub project_id: String,
    pub slug: String,
    pub title: String,
    pub description: String,
    #[serde(default)]
    pub categories: Vec<String>,
    pub downloads: u64,
    #[serde(default)]
    pub follows: u64,
    pub icon_url: Option<String>,
    pub author: Option<String>,
    #[serde(default)]
    pub versions: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Project {
    pub id: String,
    pub slug: String,
    pub title: String,
    pub description: String,
    pub body: String,
    #[serde(default)]
    pub categories: Vec<String>,
    pub icon_url: Option<String>,
    pub downloads: u64,
    #[serde(default)]
    pub game_versions: Vec<String>,
    #[serde(default)]
    pub loaders: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Version {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub version_number: String,
    pub version_type: VersionType,
    pub files: Vec<VersionFile>,
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
    #[serde(default)]
    pub game_versions: Vec<String>,
    #[serde(default)]
    pub loaders: Vec<String>,
    pub date_published: String,
}

impl Version {
    /// The file to actually install. Multi-file versions exist (sources
    /// jars, extra artifacts); exactly one is flagged primary.
    pub fn primary_file(&self) -> Result<&VersionFile> {
        self.files
            .iter()
            .find(|f| f.primary)
            .or_else(|| self.files.first())
            .ok_or_else(|| {
                Error::invalid(format!("Modrinth version {} has no files", self.id))
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum VersionType {
    Release,
    Beta,
    Alpha,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VersionFile {
    pub url: String,
    pub filename: String,
    pub primary: bool,
    #[serde(default)]
    pub size: u64,
    pub hashes: FileHashes,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileHashes {
    pub sha1: Option<String>,
    pub sha512: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Dependency {
    pub project_id: Option<String>,
    pub version_id: Option<String>,
    pub file_name: Option<String>,
    pub dependency_type: DependencyType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DependencyType {
    Required,
    Optional,
    Incompatible,
    Embedded,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facets_and_together_one_group_each() {
        let query = SearchQuery {
            loader: Some("fabric"),
            game_version: Some("26.1.2"),
            ..Default::default()
        };
        assert_eq!(
            query.facets(),
            r#"[["categories:fabric"],["versions:26.1.2"],["project_type:mod"]]"#
        );
    }

    #[test]
    fn facets_empty_when_unfiltered() {
        let query = SearchQuery {
            project_type: None,
            ..Default::default()
        };
        assert_eq!(query.facets(), "");
    }
}
