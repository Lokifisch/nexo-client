//! `eula.txt` and `server.properties` for a Paper server directory. See
//! `PaperServerFiles.java`'s doc comment for why `online-mode=true`: the
//! owner's singleplayer playerdata is keyed by their real Microsoft-account
//! UUID, and an offline-mode server computes a different, deterministic UUID
//! from the username instead — joining as the same person then gets them a
//! fresh player, not their copied inventory/position/gamemode.

use super::registry::PaperServer;
use crate::error::{IoContext, Result};
use std::path::Path;

/// Only ever called after a real, explicit user click on a EULA consent
/// screen (see `registry::Eula`) — never as a side effect of anything else.
pub async fn write_eula(server_dir: &Path) -> Result<()> {
    let path = server_dir.join("eula.txt");
    tokio::fs::write(&path, "eula=true\n").await.ctx(&path)
}

pub async fn write_server_properties(
    server_dir: &Path,
    record: &PaperServer,
    game_mode: &str,
) -> Result<()> {
    let properties = format!(
        "server-ip=127.0.0.1\nlevel-name={}\nserver-port={}\nonline-mode=true\ngamemode={}\nenable-rcon=true\nrcon.port={}\nrcon.password={}\nmotd={}\n",
        esc(&record.level_name),
        record.port,
        game_mode,
        record.rcon.port,
        record.rcon.password,
        esc(&record.name)
    );
    let path = server_dir.join("server.properties");
    write_private(&path, properties.into_bytes()).await
}

/// Strips control characters so a world folder name can't inject extra
/// `server.properties` lines.
fn esc(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}

/// Writes `bytes` to `path` owner-only (0600 on unix) via a uniquely named
/// exclusive temp file and a rename, so an existing file or symlink at `path`
/// is replaced, never followed. Holds RCON secrets.
pub async fn write_private(path: &Path, bytes: Vec<u8>) -> Result<()> {
    let path = path.to_path_buf();
    let result = tokio::task::spawn_blocking(move || {
        use std::io::Write;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let temp = path.with_extension(format!("tmp-{nanos}"));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(&temp, &path).inspect_err(|_| {
            let _ = std::fs::remove_file(&temp);
        })
    })
    .await;
    match result {
        Ok(r) => r.map_err(|e| crate::error::Error::invalid(format!("write failed: {e}"))),
        Err(e) => Err(crate::error::Error::invalid(format!("write failed: {e}"))),
    }
}

/// A fresh per-server RCON password — never reused across servers, never
/// logged. Reuses the same "random UUID as a token" idiom `auth.rs` already
/// uses for OAuth state, rather than adding a `rand` dependency for one value.
pub fn random_rcon_password() -> String {
    uuid::Uuid::new_v4().to_string()
}
