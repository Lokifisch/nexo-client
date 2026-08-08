//! Resolving a version and getting everything it needs onto disk.

use super::download::{DownloadTask, Downloader, Progress};
use super::meta::{AssetIndex, Library, VersionData, VersionManifest};
use super::{fabric, meta};
use crate::error::{Error, IoContext, Result};
use crate::instance::{Instance, Loader};
use crate::paths::Paths;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug, Clone)]
pub struct Installer {
    http: reqwest::Client,
    paths: Paths,
    downloader: Downloader,
}

impl Installer {
    pub fn new(http: reqwest::Client, paths: Paths) -> Self {
        Self {
            downloader: Downloader::new(http.clone()),
            http,
            paths,
        }
    }

    pub async fn version_manifest(&self) -> Result<VersionManifest> {
        Ok(self
            .http
            .get(meta::VERSION_MANIFEST_URL)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    /// Builds the fully-merged version data for an instance, fetching from
    /// the network only what isn't cached. Cheap enough to call before every
    /// launch, which is what makes offline launching work.
    pub async fn resolve(&self, instance: &Instance) -> Result<VersionData> {
        let vanilla = self.vanilla_version(&instance.game_version).await?;

        match instance.loader {
            Loader::Vanilla => Ok(vanilla),
            Loader::Fabric => {
                let loader_version = match &instance.loader_version {
                    Some(v) => v.clone(),
                    None => fabric::latest_stable(&self.http, &instance.game_version).await?,
                };
                let profile =
                    fabric::profile(&self.http, &instance.game_version, &loader_version).await?;
                Ok(profile.merge_onto(vanilla))
            }
        }
    }

    /// Vanilla version JSON, cached under `versions/<id>/<id>.json`.
    async fn vanilla_version(&self, game_version: &str) -> Result<VersionData> {
        let cached = self
            .paths
            .versions()
            .join(game_version)
            .join(format!("{game_version}.json"));

        if cached.exists()
            && let Ok(raw) = tokio::fs::read(&cached).await
            && let Ok(parsed) = serde_json::from_slice::<VersionData>(&raw)
        {
            return Ok(parsed);
        }

        let manifest = self.version_manifest().await?;
        let entry = manifest.find(game_version).ok_or_else(|| {
            Error::invalid(format!("Minecraft {game_version} does not exist"))
        })?;

        let raw = self
            .http
            .get(&entry.url)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;

        if let Some(parent) = cached.parent() {
            tokio::fs::create_dir_all(parent).await.ctx(parent)?;
        }
        tokio::fs::write(&cached, &raw).await.ctx(&cached)?;

        Ok(serde_json::from_slice(&raw)?)
    }

    /// Downloads the client jar, libraries, and assets, then unpacks natives.
    /// Safe to re-run: everything already present and the right size is
    /// skipped.
    pub async fn install(
        &self,
        instance: &Instance,
        progress: Option<&UnboundedSender<Progress>>,
    ) -> Result<VersionData> {
        let stage = |label: &str| {
            if let Some(tx) = progress {
                let _ = tx.send(Progress::Stage(label.to_string()));
            }
        };

        stage("Resolving version");
        let version = self.resolve(instance).await?;

        stage("Downloading game files");
        let mut tasks = Vec::new();

        if let Some(client) = version.client_download() {
            tasks.push(DownloadTask {
                url: client.url.clone(),
                dest: self.client_jar(&instance.game_version),
                sha1: non_empty(&client.sha1),
                size: client.size,
            });
        }

        tasks.extend(self.library_tasks(&version)?);

        // Assets need their index fetched first, since the index *is* the
        // list of what to download.
        let asset_tasks = self.asset_tasks(&version).await?;
        let has_assets = !asset_tasks.is_empty();
        tasks.extend(asset_tasks);

        if has_assets {
            stage("Downloading game files and assets");
        }
        self.downloader.run(tasks, progress).await?;

        stage("Unpacking natives");
        self.extract_natives(&version, &instance.game_version).await?;

        // Deliberately no `Progress::Done` here — install is a step inside a
        // larger play flow, and the caller owns announcing the terminal state.
        Ok(version)
    }

    pub fn client_jar(&self, game_version: &str) -> PathBuf {
        self.paths
            .versions()
            .join(game_version)
            .join(format!("{game_version}.jar"))
    }

    /// Natives are unpacked per game version, not per instance — they're
    /// identical across instances on the same version.
    pub fn natives_dir(&self, game_version: &str) -> PathBuf {
        self.paths.versions().join(game_version).join("natives")
    }

    /// Where a library's jar belongs in the shared library tree.
    pub fn library_path(&self, library: &Library) -> Option<PathBuf> {
        let relative = match library.artifact() {
            Some(artifact) if !artifact.path.is_empty() => artifact.path.clone(),
            // Fabric's libraries have no `downloads` block, only Maven
            // coordinates.
            _ => library.maven_path()?,
        };
        Some(self.paths.libraries().join(relative))
    }

    fn library_tasks(&self, version: &VersionData) -> Result<Vec<DownloadTask>> {
        let mut tasks = Vec::new();

        for library in version.active_libraries() {
            if let Some(artifact) = library.artifact() {
                if let Some(dest) = self.library_path(library) {
                    tasks.push(DownloadTask {
                        url: artifact.url.clone(),
                        dest,
                        sha1: non_empty(&artifact.sha1),
                        size: artifact.size,
                    });
                }
            } else if let (Some(url), Some(dest)) =
                (library.maven_url(), self.library_path(library))
            {
                // No checksum published for Maven-resolved libraries.
                tasks.push(DownloadTask {
                    url,
                    dest,
                    sha1: None,
                    size: 0,
                });
            }

            // Legacy classifier-style natives are a second jar for the same
            // library entry.
            if let Some(native) = library.native_artifact() {
                tasks.push(DownloadTask {
                    url: native.url.clone(),
                    dest: self.paths.libraries().join(&native.path),
                    sha1: non_empty(&native.sha1),
                    size: native.size,
                });
            }
        }

        Ok(tasks)
    }

    async fn asset_tasks(&self, version: &VersionData) -> Result<Vec<DownloadTask>> {
        let Some(index_ref) = &version.asset_index else {
            return Ok(Vec::new());
        };

        let index_path = self
            .paths
            .asset_indexes()
            .join(format!("{}.json", index_ref.id));

        let raw = if index_path.exists() {
            tokio::fs::read(&index_path).await.ctx(&index_path)?
        } else {
            let bytes = self
                .http
                .get(&index_ref.url)
                .send()
                .await?
                .error_for_status()?
                .bytes()
                .await?;
            if let Some(parent) = index_path.parent() {
                tokio::fs::create_dir_all(parent).await.ctx(parent)?;
            }
            tokio::fs::write(&index_path, &bytes).await.ctx(&index_path)?;
            bytes.to_vec()
        };

        let index: AssetIndex = serde_json::from_slice(&raw)?;

        // Many logical asset paths share one hash; dedupe so we don't queue
        // the same object repeatedly.
        let mut seen = std::collections::HashSet::new();
        let mut tasks = Vec::new();
        for object in index.objects.values() {
            if !seen.insert(object.hash.clone()) {
                continue;
            }
            tasks.push(DownloadTask {
                url: object.url(),
                dest: self.paths.asset_objects().join(object.relative_path()),
                sha1: Some(object.hash.clone()),
                size: object.size,
            });
        }

        Ok(tasks)
    }

    /// Unpacks every native jar into the version's natives directory, which
    /// is what `-Djava.library.path` will point at.
    async fn extract_natives(&self, version: &VersionData, game_version: &str) -> Result<()> {
        // Keyed on the vanilla game version, not `version.id`: after a Fabric
        // merge the id reads `fabric-loader-<x>-<mc>`, and natives are
        // identical across loaders anyway.
        let dest = self.natives_dir(game_version);
        tokio::fs::create_dir_all(&dest).await.ctx(&dest)?;

        let mut jars = Vec::new();
        for library in version.active_libraries() {
            if let Some(native) = library.native_artifact() {
                jars.push(self.paths.libraries().join(&native.path));
            } else if library.is_modern_native()
                && let Some(path) = self.library_path(library)
            {
                jars.push(path);
            }
        }

        if jars.is_empty() {
            return Ok(());
        }

        // The zip crate is synchronous, so this belongs off the async
        // runtime's worker threads.
        tokio::task::spawn_blocking(move || -> Result<()> {
            for jar in jars {
                if !jar.exists() {
                    continue;
                }
                extract_jar(&jar, &dest)?;
            }
            Ok(())
        })
        .await
        .map_err(|e| Error::invalid(format!("native extraction panicked: {e}")))?
    }
}

/// Extracts the shared libraries from one natives jar, ignoring metadata and
/// anything that would escape `dest`.
fn extract_jar(jar: &Path, dest: &Path) -> Result<()> {
    let file = std::fs::File::open(jar).ctx(jar)?;
    let mut archive = zip::ZipArchive::new(file)?;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        // `enclosed_name` rejects `../` traversal — a malicious or malformed
        // jar must not be able to write outside the natives directory.
        let Some(name) = entry.enclosed_name() else {
            continue;
        };
        if entry.is_dir() || name.starts_with("META-INF") {
            continue;
        }
        // Only the actual shared objects matter; jars also carry class files.
        let is_native = name
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| matches!(e, "so" | "dll" | "dylib" | "jnilib"));
        if !is_native {
            continue;
        }

        let Some(file_name) = name.file_name() else {
            continue;
        };
        let out = dest.join(file_name);
        if out.exists() {
            continue;
        }
        let mut writer = std::fs::File::create(&out).ctx(&out)?;
        std::io::copy(&mut entry, &mut writer).ctx(&out)?;
    }

    Ok(())
}

fn non_empty(sha1: &str) -> Option<String> {
    (!sha1.is_empty()).then(|| sha1.to_string())
}
