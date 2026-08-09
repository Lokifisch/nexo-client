//! Every skin the account has worn, kept locally so it can be worn again.
//!
//! Mojang stores only the skin currently in use — change it and the previous
//! one is gone unless you kept the file. This keeps a copy of each one as it
//! is seen, so switching back is a click rather than a hunt through
//! downloads.
//!
//! Entries are keyed by the SHA-1 of the PNG itself, which makes re-saving
//! the same skin a no-op no matter how many times it comes round.

use crate::auth::SkinModel;
use crate::error::{IoContext, Result};
use crate::paths::Paths;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One saved skin.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedSkin {
    /// SHA-1 of the PNG, and the filename it's stored under.
    pub id: String,
    /// Arm width the texture is drawn for.
    pub model: SkinModel,
    /// Unix seconds, used only for ordering.
    pub added_at: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Index {
    #[serde(default)]
    skins: Vec<SavedSkin>,
}

#[derive(Debug, Clone)]
pub struct SkinLibrary {
    dir: PathBuf,
    index: PathBuf,
}

impl SkinLibrary {
    pub fn new(paths: &Paths) -> Self {
        let dir = paths.root().join("skins");
        Self {
            index: dir.join("index.json"),
            dir,
        }
    }

    pub fn png_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.png"))
    }

    /// Saved skins, most recently added first.
    pub async fn list(&self) -> Result<Vec<SavedSkin>> {
        let mut index = self.read_index().await?;
        index.skins.sort_by_key(|skin| std::cmp::Reverse(skin.added_at));

        // Drop entries whose file has gone — deleted by hand, or a partial
        // write — rather than showing tiles that can't be worn.
        index.skins.retain(|skin| self.png_path(&skin.id).exists());
        Ok(index.skins)
    }

    async fn read_index(&self) -> Result<Index> {
        if !self.index.exists() {
            return Ok(Index::default());
        }
        let raw = tokio::fs::read(&self.index).await.ctx(&self.index)?;
        // A corrupt index shouldn't cost the user their skins; the PNGs are
        // the real content and are still on disk.
        Ok(serde_json::from_slice(&raw).unwrap_or_default())
    }

    async fn write_index(&self, index: &Index) -> Result<()> {
        tokio::fs::create_dir_all(&self.dir).await.ctx(&self.dir)?;
        let json = serde_json::to_vec_pretty(index)?;
        let temp = self.index.with_extension("json.tmp");
        tokio::fs::write(&temp, &json).await.ctx(&temp)?;
        tokio::fs::rename(&temp, &self.index).await.ctx(&self.index)
    }

    /// Records a skin, or returns the existing entry if it's already here.
    ///
    /// The model is refreshed on a repeat: the same texture can legitimately
    /// be worn as classic once and slim later.
    pub async fn save(&self, png: &[u8], model: SkinModel) -> Result<SavedSkin> {
        let id = crate::util::sha1_hex(png);
        let mut index = self.read_index().await?;

        tokio::fs::create_dir_all(&self.dir).await.ctx(&self.dir)?;
        let path = self.png_path(&id);
        if !path.exists() {
            tokio::fs::write(&path, png).await.ctx(&path)?;
        }

        if let Some(existing) = index.skins.iter_mut().find(|s| s.id == id) {
            existing.model = model;
            let saved = existing.clone();
            self.write_index(&index).await?;
            return Ok(saved);
        }

        let saved = SavedSkin {
            id,
            model,
            added_at: crate::instance::now(),
        };
        index.skins.push(saved.clone());
        self.write_index(&index).await?;
        Ok(saved)
    }

    /// Forgets a skin and deletes its file.
    pub async fn remove(&self, id: &str) -> Result<()> {
        let mut index = self.read_index().await?;
        index.skins.retain(|skin| skin.id != id);
        self.write_index(&index).await?;

        let path = self.png_path(id);
        if path.exists() {
            tokio::fs::remove_file(&path).await.ctx(&path)?;
        }
        Ok(())
    }

    pub async fn read(&self, id: &str) -> Result<Vec<u8>> {
        let path = self.png_path(id);
        tokio::fs::read(&path).await.ctx(&path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library() -> (SkinLibrary, PathBuf) {
        let root = std::env::temp_dir().join(format!("nexo-skins-{}", uuid::Uuid::new_v4()));
        (SkinLibrary::new(&Paths::with_root(&root)), root)
    }

    #[tokio::test]
    async fn saving_the_same_skin_twice_keeps_one_entry() {
        let (library, root) = library();

        let first = library.save(b"pretend png", SkinModel::Classic).await.unwrap();
        let again = library.save(b"pretend png", SkinModel::Classic).await.unwrap();

        assert_eq!(first.id, again.id, "the id is the content's hash");
        assert_eq!(library.list().await.unwrap().len(), 1);

        tokio::fs::remove_dir_all(&root).await.ok();
    }

    #[tokio::test]
    async fn re_saving_updates_the_model() {
        let (library, root) = library();

        library.save(b"pretend png", SkinModel::Classic).await.unwrap();
        // Same texture, worn as slim this time.
        library.save(b"pretend png", SkinModel::Slim).await.unwrap();

        let saved = library.list().await.unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].model, SkinModel::Slim);

        tokio::fs::remove_dir_all(&root).await.ok();
    }

    #[tokio::test]
    async fn newest_first_and_removal_takes_the_file_with_it() {
        let (library, root) = library();

        let older = library.save(b"one", SkinModel::Classic).await.unwrap();
        // `added_at` has one-second resolution, so order it explicitly.
        let mut newer = library.save(b"two", SkinModel::Classic).await.unwrap();
        newer.added_at = older.added_at + 10;
        library
            .write_index(&Index {
                skins: vec![older.clone(), newer.clone()],
            })
            .await
            .unwrap();

        let listed = library.list().await.unwrap();
        assert_eq!(listed[0].id, newer.id, "newest should come first");

        library.remove(&newer.id).await.unwrap();
        assert!(!library.png_path(&newer.id).exists());
        assert_eq!(library.list().await.unwrap().len(), 1);

        tokio::fs::remove_dir_all(&root).await.ok();
    }

    #[tokio::test]
    async fn entries_whose_file_vanished_are_not_listed() {
        let (library, root) = library();

        let saved = library.save(b"one", SkinModel::Classic).await.unwrap();
        // Deleted by hand, behind the library's back.
        tokio::fs::remove_file(library.png_path(&saved.id)).await.unwrap();

        assert!(library.list().await.unwrap().is_empty());

        tokio::fs::remove_dir_all(&root).await.ok();
    }
}
