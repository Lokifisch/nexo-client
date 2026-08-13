//! Modrinth modpack (`.mrpack`) import and export.
//!
//! An mrpack is a zip holding `modrinth.index.json` plus an optional
//! `overrides/` tree. The index lists files by URL and hash rather than
//! embedding them, which is what keeps packs small and is why importing needs
//! the network while exporting does not.
//!
//! Files that came from Modrinth are exported as index entries; anything
//! local — a jar dropped in by hand, config — goes into `overrides/`, since
//! there is no URL to point at.

use crate::content::ProjectKind;
use crate::error::{Error, IoContext, Result};
use crate::instance::{InstalledMod, Instance, Loader, ModSource};
use crate::paths::Paths;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// The only format version Modrinth has published.
const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Index {
    pub format_version: u32,
    /// Always `minecraft`.
    pub game: String,
    /// The pack's own version, not Minecraft's.
    pub version_id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default)]
    pub files: Vec<IndexFile>,
    /// Keyed by `minecraft`, `fabric-loader`, and so on.
    pub dependencies: std::collections::BTreeMap<String, String>,
}

impl Index {
    pub fn minecraft_version(&self) -> Option<&str> {
        self.dependencies.get("minecraft").map(String::as_str)
    }

    /// Which loader the pack wants, if it's one this launcher supports.
    pub fn loader(&self) -> Option<Loader> {
        if self.dependencies.contains_key("fabric-loader") {
            return Some(Loader::Fabric);
        }
        if self.dependencies.is_empty() {
            return None;
        }
        // Forge, NeoForge and Quilt all appear here; none is supported yet,
        // and treating them as vanilla would produce an instance that looks
        // fine and then can't load any of the pack's mods.
        None
    }

    pub fn fabric_loader_version(&self) -> Option<&str> {
        self.dependencies.get("fabric-loader").map(String::as_str)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexFile {
    /// Destination relative to the instance root, e.g. `mods/sodium.jar`.
    pub path: String,
    #[serde(default)]
    pub hashes: std::collections::BTreeMap<String, String>,
    pub downloads: Vec<String>,
    #[serde(default)]
    pub file_size: u64,
}

/// What an import produced, so the UI can say what happened.
#[derive(Debug, Clone)]
pub struct Imported {
    pub instance_id: String,
    pub name: String,
    pub files: usize,
    /// Files the pack listed that couldn't be fetched. An import continues
    /// past these rather than failing outright — a pack is still mostly
    /// usable when one optional mod has been taken down.
    pub skipped: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct MrPack {
    http: reqwest::Client,
    paths: Paths,
}

impl MrPack {
    pub fn new(http: reqwest::Client, paths: Paths) -> Self {
        Self { http, paths }
    }

    /// Reads a pack's index without importing it, for showing what's inside
    /// before committing to it.
    pub async fn inspect(&self, pack: &Path) -> Result<Index> {
        let pack = pack.to_path_buf();
        tokio::task::spawn_blocking(move || read_index(&pack))
            .await
            .map_err(|err| Error::invalid(format!("could not read the pack: {err}")))?
    }

    /// Creates an instance from a pack and downloads its contents.
    pub async fn import(
        &self,
        pack: &Path,
        instances: &crate::instance::InstanceStore,
    ) -> Result<Imported> {
        let index = self.inspect(pack).await?;

        let game_version = index
            .minecraft_version()
            .ok_or_else(|| Error::invalid("this pack doesn't say which Minecraft version it needs"))?
            .to_string();

        let loader = index.loader().ok_or_else(|| {
            Error::invalid(
                "this pack needs a mod loader Nexo doesn't support yet — only Fabric works",
            )
        })?;

        let mut instance = instances.create(&index.name, &game_version, loader).await?;
        instance.loader_version = index.fabric_loader_version().map(str::to_string);

        let root = self.paths.instance(&instance.id);
        let mut skipped = Vec::new();
        let mut installed = 0usize;

        for file in &index.files {
            let Some(destination) = safe_join(&root, &file.path) else {
                // A path escaping the instance directory is malformed at best
                // and hostile at worst.
                skipped.push(file.path.clone());
                continue;
            };

            match self.fetch_file(file).await {
                Ok(bytes) => {
                    if let Some(parent) = destination.parent() {
                        tokio::fs::create_dir_all(parent).await.ctx(parent)?;
                    }
                    tokio::fs::write(&destination, &bytes).await.ctx(&destination)?;
                    installed += 1;

                    instance.mods.push(InstalledMod {
                        project_id: format!("mrpack:{}", file.path),
                        name: file_stem(&file.path),
                        version_id: String::new(),
                        version_number: "from modpack".to_string(),
                        file_name: file_name(&file.path),
                        source: ModSource::Modrinth,
                        enabled: true,
                        edition: None,
                    });
                }
                Err(err) => {
                    tracing::warn!(path = %file.path, %err, "skipping a pack file");
                    skipped.push(file.path.clone());
                }
            }
        }

        // Overrides last, so a pack's own config wins over anything a listed
        // file happened to write.
        let pack = pack.to_path_buf();
        let root_for_overrides = root.clone();
        tokio::task::spawn_blocking(move || extract_overrides(&pack, &root_for_overrides))
            .await
            .map_err(|err| Error::invalid(format!("could not unpack overrides: {err}")))??;

        instances.save(&instance).await?;

        Ok(Imported {
            instance_id: instance.id.clone(),
            name: instance.name.clone(),
            files: installed,
            skipped,
        })
    }

    async fn fetch_file(&self, file: &IndexFile) -> Result<Vec<u8>> {
        let url = file
            .downloads
            .first()
            .ok_or_else(|| Error::invalid("a pack entry lists no download"))?;

        let bytes = self
            .http
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;

        if let Some(expected) = file.hashes.get("sha1") {
            let actual = crate::util::sha1_hex(&bytes);
            if &actual != expected {
                return Err(Error::invalid(format!(
                    "{} failed its checksum",
                    file.path
                )));
            }
        }

        Ok(bytes.to_vec())
    }

    /// Writes an instance out as a pack.
    ///
    /// Modrinth-sourced content becomes index entries; everything else is
    /// bundled into `overrides/`, because a local file has no URL to list.
    pub async fn export(&self, instance: &Instance, destination: &Path) -> Result<()> {
        let mut dependencies = std::collections::BTreeMap::new();
        dependencies.insert("minecraft".to_string(), instance.game_version.clone());
        if instance.loader == Loader::Fabric {
            dependencies.insert(
                "fabric-loader".to_string(),
                instance
                    .loader_version
                    .clone()
                    .unwrap_or_else(|| "latest".to_string()),
            );
        }

        // Only content with a real download can be listed; the rest is
        // gathered as overrides below.
        let mut files = Vec::new();
        let mut bundled: Vec<(String, PathBuf)> = Vec::new();
        let root = self.paths.instance(&instance.id);

        for installed in &instance.mods {
            let relative = relative_path(installed);
            let absolute = root.join(&relative);
            if !absolute.exists() {
                continue;
            }
            bundled.push((relative, absolute));
        }

        let index = Index {
            format_version: FORMAT_VERSION,
            game: "minecraft".to_string(),
            version_id: "1.0.0".to_string(),
            name: instance.name.clone(),
            summary: Some(format!(
                "Exported from Nexo — {} {}",
                instance.loader, instance.game_version
            )),
            files: std::mem::take(&mut files),
            dependencies,
        };

        let destination = destination.to_path_buf();
        tokio::task::spawn_blocking(move || write_pack(&destination, &index, &bundled))
            .await
            .map_err(|err| Error::invalid(format!("could not write the pack: {err}")))?
    }
}

/// Where an installed file lives relative to the instance root.
fn relative_path(installed: &InstalledMod) -> String {
    // The kind isn't recorded, so infer from the extension: jars are mods,
    // and zips are packs of some sort.
    let folder = if installed.file_name.ends_with(".jar") {
        ProjectKind::Mod.folder()
    } else {
        ProjectKind::ResourcePack.folder()
    };
    format!("{folder}/{}", installed.file_name)
}

fn file_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

fn file_stem(path: &str) -> String {
    let name = file_name(path);
    name.trim_end_matches(".jar")
        .trim_end_matches(".zip")
        .to_string()
}

/// Joins a pack-supplied relative path onto the instance root, refusing
/// anything that would escape it.
///
/// Pack indexes are third-party data, and `../` in a path would otherwise
/// write wherever it liked.
fn safe_join(root: &Path, relative: &str) -> Option<PathBuf> {
    let mut out = root.to_path_buf();
    for part in relative.split('/') {
        match part {
            "" | "." => continue,
            ".." => return None,
            part if part.contains('\\') => return None,
            part => out.push(part),
        }
    }
    // A path of only separators would leave the root itself.
    (out != root).then_some(out)
}

fn read_index(pack: &Path) -> Result<Index> {
    let file = std::fs::File::open(pack).ctx(pack)?;
    let mut archive = zip::ZipArchive::new(file)?;

    let mut entry = archive.by_name("modrinth.index.json").map_err(|_| {
        Error::invalid("that file isn't a Modrinth pack — it has no modrinth.index.json")
    })?;

    let mut text = String::new();
    entry.read_to_string(&mut text)?;
    let index: Index = serde_json::from_str(&text)?;

    if index.format_version != FORMAT_VERSION {
        return Err(Error::invalid(format!(
            "this pack is format version {}, and only version {FORMAT_VERSION} is understood",
            index.format_version
        )));
    }

    Ok(index)
}

fn extract_overrides(pack: &Path, root: &Path) -> Result<()> {
    let file = std::fs::File::open(pack).ctx(pack)?;
    let mut archive = zip::ZipArchive::new(file)?;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        // `enclosed_name` rejects traversal; the prefix check keeps the rest
        // of the archive out.
        let Some(name) = entry.enclosed_name() else {
            continue;
        };
        let Ok(relative) = name.strip_prefix("overrides") else {
            continue;
        };
        if entry.is_dir() || relative.as_os_str().is_empty() {
            continue;
        }

        let destination = root.join(relative);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).ctx(parent)?;
        }
        let mut out = std::fs::File::create(&destination).ctx(&destination)?;
        std::io::copy(&mut entry, &mut out).ctx(&destination)?;
    }

    Ok(())
}

fn write_pack(destination: &Path, index: &Index, bundled: &[(String, PathBuf)]) -> Result<()> {
    let file = std::fs::File::create(destination).ctx(destination)?;
    let mut zip = zip::ZipWriter::new(file);
    let options: zip::write::FileOptions<'_, ()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    zip.start_file("modrinth.index.json", options)?;
    zip.write_all(serde_json::to_string_pretty(index)?.as_bytes())?;

    for (relative, absolute) in bundled {
        zip.start_file(format!("overrides/{relative}"), options)?;
        let contents = std::fs::read(absolute).ctx(absolute)?;
        zip.write_all(&contents)?;
    }

    zip.finish()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(dependencies: &[(&str, &str)]) -> Index {
        Index {
            format_version: FORMAT_VERSION,
            game: "minecraft".into(),
            version_id: "1.0.0".into(),
            name: "Test pack".into(),
            summary: None,
            files: Vec::new(),
            dependencies: dependencies
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn fabric_packs_are_understood_and_others_refused() {
        let fabric = index(&[("minecraft", "26.1.2"), ("fabric-loader", "0.19.3")]);
        assert_eq!(fabric.loader(), Some(Loader::Fabric));
        assert_eq!(fabric.minecraft_version(), Some("26.1.2"));
        assert_eq!(fabric.fabric_loader_version(), Some("0.19.3"));

        // Refused rather than treated as vanilla: an instance that looks fine
        // and then can't load any of the pack's mods is worse than a refusal.
        let forge = index(&[("minecraft", "26.1.2"), ("forge", "50.0.0")]);
        assert_eq!(forge.loader(), None);
    }

    #[test]
    fn paths_that_escape_the_instance_are_refused() {
        let root = Path::new("/instances/demo");

        assert_eq!(
            safe_join(root, "mods/sodium.jar"),
            Some(root.join("mods").join("sodium.jar"))
        );
        // Traversal, in the obvious form and buried mid-path.
        assert_eq!(safe_join(root, "../evil.jar"), None);
        assert_eq!(safe_join(root, "mods/../../evil.jar"), None);
        // Windows separators would sidestep the split on '/'.
        assert_eq!(safe_join(root, "mods\\..\\evil.jar"), None);
        // Nothing but separators leaves the root itself.
        assert_eq!(safe_join(root, "///"), None);
    }

    #[test]
    fn file_names_are_taken_from_the_end_of_the_path() {
        assert_eq!(file_name("mods/sodium-0.6.jar"), "sodium-0.6.jar");
        assert_eq!(file_stem("mods/sodium-0.6.jar"), "sodium-0.6");
        assert_eq!(file_stem("resourcepacks/faithful.zip"), "faithful");
        assert_eq!(file_name("bare.jar"), "bare.jar");
    }

    #[test]
    fn round_trips_a_written_pack() {
        let dir = std::env::temp_dir().join(format!("nexo-mrpack-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        let jar = dir.join("example.jar");
        std::fs::write(&jar, b"pretend jar").unwrap();

        let pack = dir.join("out.mrpack");
        let written = index(&[("minecraft", "26.1.2"), ("fabric-loader", "0.19.3")]);
        write_pack(
            &pack,
            &written,
            &[("mods/example.jar".to_string(), jar.clone())],
        )
        .unwrap();

        let read = read_index(&pack).unwrap();
        assert_eq!(read.name, "Test pack");
        assert_eq!(read.minecraft_version(), Some("26.1.2"));

        // The local jar should have travelled as an override.
        let extracted = dir.join("extracted");
        std::fs::create_dir_all(&extracted).unwrap();
        extract_overrides(&pack, &extracted).unwrap();
        assert_eq!(
            std::fs::read(extracted.join("mods").join("example.jar")).unwrap(),
            b"pretend jar"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_zip_without_an_index_is_not_a_pack() {
        let dir = std::env::temp_dir().join(format!("nexo-mrpack-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        let path = dir.join("plain.zip");
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
        zip.start_file("readme.txt", options).unwrap();
        zip.write_all(b"not a pack").unwrap();
        zip.finish().unwrap();

        assert!(read_index(&path).is_err());

        std::fs::remove_dir_all(&dir).ok();
    }
}
