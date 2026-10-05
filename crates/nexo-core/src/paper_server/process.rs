//! Spawns a Paper server subprocess and hands it to [`RunningGames`] for
//! supervision.
//!
//! Reuses `running::RunningGames` rather than a second process-tracking
//! type: it already has no game-specific logic in it, just named-child
//! supervision keyed by a string id (oneshot stop, `watch` exited channel).
//! A Paper server's id and an instance's id live in different namespaces
//! (`paper-servers/<id>` vs `instances/<id>`), so a separate `RunningGames`
//! instance is used rather than sharing the game-launch one, purely to keep
//! "what's running" queries from mixing the two kinds of process.

use crate::error::{IoContext, Result};
use crate::paths::Paths;
use crate::running::RunningGames;
use crate::util::no_window_async;
use std::path::Path;

/// Starts `paper.jar` in `server_dir`, using a JVM resolved the same way
/// the game launch path does (`java::ensure`) rather than a bare `"java"`
/// on `PATH`.
pub async fn start(
    http: &reqwest::Client,
    paths: &Paths,
    server_dir: &Path,
    heap_mb: u32,
    id: &str,
    running: &RunningGames,
) -> Result<u32> {
    let java = crate::java::ensure(http, paths, None, None).await?;

    let mut command = tokio::process::Command::new(&java.path);
    command
        .arg(format!("-Xmx{heap_mb}M"))
        .arg("-jar")
        .arg("paper.jar")
        .arg("--nogui")
        .current_dir(server_dir)
        .kill_on_drop(false);
    no_window_async(&mut command);

    let child = command.spawn().ctx(server_dir)?;
    let pid = child.id().unwrap_or_default();
    running.register(&format!("paper:{id}"), child);
    Ok(pid)
}
