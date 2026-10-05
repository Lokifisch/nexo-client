//! Vanilla singleplayer save <-> Paper server world.
//!
//! Verified 2026-08-22 against a real Paper 26.1.2 build and the real
//! vanilla 26.1.2 dedicated server (see `Mod/ROADMAP.md` Phase 7 and
//! `WorldConverter.java`, the Java twin of this module): MC 26.1.2 unified
//! every dimension under `<world>/dimensions/<namespace>/<dim>/`, and a
//! singleplayer save is structurally identical to a dedicated server's
//! `level-name` directory. Conversion is therefore a plain recursive
//! directory copy with a renamed top-level folder, not a per-dimension
//! remap — there is no shared binary format to keep in sync here, just two
//! independently-correct implementations of the same well-known layout.

use crate::error::{Error, IoContext, Result};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Copies `save` into `server_dir/level_name`. The original save is never
/// touched — copy into a temp dir, verify, then rename into place, so a
/// failed or interrupted copy never leaves a half-written world at the
/// final path.
pub async fn to_server(save: &Path, server_dir: &Path, level_name: &str) -> Result<()> {
    if tokio::fs::metadata(save.join("level.dat")).await.is_err() {
        return Err(Error::invalid(format!(
            "not a Minecraft save (no level.dat): {}",
            save.display()
        )));
    }

    let final_dest = server_dir.join(level_name);
    let temp_dest = server_dir.join(format!("{level_name}.converting-{}", now_nanos()));
    copy_tree(save, &temp_dest).await?;

    if tokio::fs::metadata(temp_dest.join("level.dat"))
        .await
        .is_err()
    {
        let _ = remove_tree(&temp_dest).await;
        return Err(Error::invalid(format!(
            "copy of {} is missing level.dat after copying",
            save.display()
        )));
    }

    if tokio::fs::metadata(&final_dest).await.is_ok() {
        remove_tree(&final_dest).await?;
    }
    tokio::fs::rename(&temp_dest, &final_dest)
        .await
        .ctx(&final_dest)?;
    Ok(())
}

/// Backs up the current singleplayer save, then overwrites it with
/// `server_dir/level_name`. The backup happens unconditionally and first —
/// even if the rest of this fails, nothing is lost.
///
/// The backup ends up nested at `save/backups/<timestamp>/` — inside the
/// world it's a backup *of*, once that world is back in place. It can't be
/// copied straight there before the overwrite (that would copy `save` into
/// a directory living inside itself), so it's staged as a sibling temp dir
/// first and only moved inside `save` after the revert has completed.
///
/// Returns the path the pre-conversion save was backed up to.
pub async fn to_singleplayer(server_dir: &Path, level_name: &str, save: &Path) -> Result<PathBuf> {
    let source = server_dir.join(level_name);
    if tokio::fs::metadata(source.join("level.dat")).await.is_err() {
        return Err(Error::invalid(format!(
            "not a Minecraft world (no level.dat): {}",
            source.display()
        )));
    }

    let backup_staging = sibling(save, &format!(".backup-staging-{}", now_nanos()));
    copy_tree(save, &backup_staging).await?;

    let temp_dest = sibling(save, &format!(".restoring-{}", now_nanos()));
    copy_tree(&source, &temp_dest).await?;
    remove_tree(save).await?;
    tokio::fs::rename(&temp_dest, save).await.ctx(save)?;

    let backups_dir = save.join("backups");
    tokio::fs::create_dir_all(&backups_dir)
        .await
        .ctx(&backups_dir)?;
    let backup = backups_dir.join(now_secs().to_string());
    tokio::fs::rename(&backup_staging, &backup)
        .await
        .ctx(&backup)?;

    Ok(backup)
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("world");
    path.with_file_name(format!("{name}{suffix}"))
}

/// Recursive copy. `tokio::fs` has no built-in tree copy, and pulling in a
/// crate for a bounded recursive directory walk isn't earning its keep.
/// Boxed because an `async fn` can't call itself directly (its own future
/// would have infinite size).
fn copy_tree<'a>(
    source: &'a Path,
    dest: &'a Path,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
    Box::pin(async move {
        tokio::fs::create_dir_all(dest).await.ctx(dest)?;
        let mut entries = tokio::fs::read_dir(source).await.ctx(source)?;
        while let Some(entry) = entries.next_entry().await.ctx(source)? {
            let file_type = entry.file_type().await.ctx(entry.path())?;
            let dest_path = dest.join(entry.file_name());
            if file_type.is_dir() {
                copy_tree(&entry.path(), &dest_path).await?;
            } else if file_type.is_file() {
                tokio::fs::copy(entry.path(), &dest_path)
                    .await
                    .ctx(&dest_path)?;
            }
            // Symlinks (rare inside a save) are skipped rather than followed
            // or copied as links — Minecraft never creates one on its own.
        }
        Ok(())
    })
}

async fn remove_tree(dir: &Path) -> Result<()> {
    if tokio::fs::metadata(dir).await.is_ok() {
        tokio::fs::remove_dir_all(dir).await.ctx(dir)?;
    }
    Ok(())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default()
}
