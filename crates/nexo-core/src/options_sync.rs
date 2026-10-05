//! Keeps `options.txt` (video, sound, keybinds, and everything else vanilla
//! Minecraft stores as flat `key:value` lines) the same across every
//! instance, so a setting tuned once doesn't need to be redone per instance.
//!
//! Deliberately one-directional and explicit: syncing copies one instance's
//! file over every other instance's rather than trying to merge two edited
//! copies, and only runs when asked — silently overwriting an instance's
//! settings just because another one launched would surprise someone who set
//! them differently on purpose. Minecraft ignores keys it doesn't recognise
//! and falls back to defaults for keys that are missing, so copying the raw
//! file across different versions/loaders is safe.

use crate::error::{IoContext, Result};
use crate::instance::InstanceStore;
use crate::paths::Paths;
use crate::running::RunningGames;
use std::time::Duration;

fn options_path(paths: &Paths, id: &str) -> std::path::PathBuf {
    paths.instance(id).join("options.txt")
}

/// Copies `source`'s `options.txt` over every other instance's. Returns how
/// many instances were updated. `Ok(0)` if `source` has no `options.txt` yet
/// (never launched) — there's nothing to spread.
pub async fn sync_to_all(paths: &Paths, instances: &InstanceStore, source: &str) -> Result<usize> {
    let src = options_path(paths, source);
    let content = match tokio::fs::read(&src).await {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(err).ctx(&src),
    };

    let mut synced = 0;
    for instance in instances.list().await? {
        if instance.id == source {
            continue;
        }
        let dest = options_path(paths, &instance.id);
        tokio::fs::write(&dest, &content).await.ctx(&dest)?;
        synced += 1;
    }
    Ok(synced)
}

/// How often to check `source`'s `options.txt` for changes while it's
/// running. Minecraft only rewrites the file when a settings screen closes,
/// so this trades a few seconds of latency for needing no filesystem-watch
/// dependency.
const POLL_INTERVAL: Duration = Duration::from_secs(3);

/// Re-syncs every time `source`'s `options.txt` changes while it's running
/// (so a setting flipped mid-session shows up in the other instances without
/// waiting for the game to close), then once more after it exits to catch
/// the last write. Returns once `source` is no longer running.
pub async fn watch_and_sync(
    paths: Paths,
    instances: InstanceStore,
    running: RunningGames,
    source: String,
) {
    let path = options_path(&paths, &source);
    let mut last_modified = None;

    while running.is_running(&source) {
        tokio::time::sleep(POLL_INTERVAL).await;

        if let Ok(meta) = tokio::fs::metadata(&path).await
            && let Ok(modified) = meta.modified()
            && last_modified != Some(modified)
        {
            last_modified = Some(modified);
            if let Err(err) = sync_to_all(&paths, &instances, &source).await {
                tracing::warn!(instance = %source, %err, "live options sync failed");
            }
        }
    }

    if let Err(err) = sync_to_all(&paths, &instances, &source).await {
        tracing::warn!(instance = %source, %err, "final options sync failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance::Loader;

    async fn temp_paths() -> Paths {
        let dir = std::env::temp_dir().join(format!("nexo-options-test-{}", uuid::Uuid::new_v4()));
        let paths = Paths::with_root(&dir);
        paths.ensure().await.unwrap();
        paths
    }

    #[tokio::test]
    async fn copies_source_options_to_every_other_instance() {
        let paths = temp_paths().await;
        let store = InstanceStore::new(paths.clone());

        let a = store.create("A", "26.1.2", Loader::Fabric).await.unwrap();
        let b = store.create("B", "26.1.2", Loader::Fabric).await.unwrap();
        let c = store.create("C", "26.1.2", Loader::Fabric).await.unwrap();

        tokio::fs::write(options_path(&paths, &a.id), b"fov:90\n")
            .await
            .unwrap();

        let synced = sync_to_all(&paths, &store, &a.id).await.unwrap();
        assert_eq!(synced, 2);

        assert_eq!(
            tokio::fs::read(options_path(&paths, &b.id)).await.unwrap(),
            b"fov:90\n"
        );
        assert_eq!(
            tokio::fs::read(options_path(&paths, &c.id)).await.unwrap(),
            b"fov:90\n"
        );

        tokio::fs::remove_dir_all(paths.root()).await.ok();
    }

    #[tokio::test]
    async fn source_with_no_options_yet_syncs_nothing() {
        let paths = temp_paths().await;
        let store = InstanceStore::new(paths.clone());
        let a = store.create("A", "26.1.2", Loader::Fabric).await.unwrap();

        assert_eq!(sync_to_all(&paths, &store, &a.id).await.unwrap(), 0);

        tokio::fs::remove_dir_all(paths.root()).await.ok();
    }
}
