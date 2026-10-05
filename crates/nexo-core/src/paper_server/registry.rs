//! `nexo-paper-server.json` — read and written by both Nexo Client (this
//! module) and Nexo Mod (`PaperServerRecord.java`), so either product can
//! start, observe, or stop a server the other one created. The wire format
//! is normative in `Mod/docs/PAPER-SERVER-REGISTRY.md`; this mirrors it
//! field-for-field, camelCase preserved via `serde(rename_all)`.
//!
//! `status` is a plain `String`, not a typed enum — see the [`status`]
//! module. A typed enum would fail to deserialize a value the *other*
//! product introduced in a newer release before this one updated, which is
//! exactly the failure mode a plain string with well-known constants avoids
//! (same reasoning `PaperServerRecord.java` documents for its own `status`).

use crate::error::{IoContext, Result};
use crate::paths::Paths;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const MANIFEST: &str = "nexo-paper-server.json";
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// Well-known `status` values. Keep in sync by hand with
/// `PaperServerRecord.Status` — there's no shared source of truth for the
/// set beyond the two implementations and the contract doc.
pub mod status {
    pub const CONVERTING: &str = "converting";
    pub const STOPPED: &str = "stopped";
    pub const STARTING: &str = "starting";
    pub const RUNNING: &str = "running";
    pub const STOPPING: &str = "stopping";
    pub const ERROR: &str = "error";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaperServer {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub minecraft_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paper_build: Option<u32>,
    pub level_name: String,
    pub port: u16,
    pub rcon: Rcon,
    pub eula: Eula,
    pub status: String,
    pub owner: Owner,
    pub source_world: SourceWorld,
    pub friend_hosting: FriendHosting,
    pub created_at_epoch_second: u64,
    pub updated_at_epoch_second: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rcon {
    pub port: u16,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Eula {
    pub accepted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accepted_at_epoch_second: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accepted_by: Option<String>,
}

impl Eula {
    pub fn unaccepted() -> Self {
        Self {
            accepted: false,
            accepted_at_epoch_second: None,
            accepted_by: None,
        }
    }
}

/// `started_by` is `"mod"` or `"launcher"` (or absent when stopped). `pid`
/// is a same-machine diagnostic/fallback only — the real control path is
/// RCON (see `paper_server::rcon`), never a `kill` by pid as the first
/// resort.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Owner {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at_epoch_second: Option<u64>,
}

impl Owner {
    pub fn none() -> Self {
        Self {
            started_by: None,
            pid: None,
            hostname: None,
            started_at_epoch_second: None,
        }
    }
}

/// `original_save_path` is absolute and platform-native — `original_save_id`
/// alone (the world folder's bare name) isn't enough to find the save back,
/// since this side manages multiple instances, each with its own `saves/`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceWorld {
    pub original_save_id: String,
    pub original_save_path: PathBuf,
    pub converted_at_epoch_second: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FriendHosting {
    pub tunnel_active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
}

impl FriendHosting {
    pub fn inactive() -> Self {
        Self {
            tunnel_active: false,
            domain: None,
        }
    }
}

impl PaperServer {
    /// A fresh, unconverted, unstarted record — the state a new server
    /// registration starts in.
    pub fn new(
        id: String,
        name: String,
        minecraft_version: String,
        level_name: String,
        port: u16,
        rcon: Rcon,
        source_world: SourceWorld,
    ) -> Self {
        let now = now_secs();
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            id,
            name,
            minecraft_version,
            paper_build: None,
            level_name,
            port,
            rcon,
            eula: Eula::unaccepted(),
            status: status::CONVERTING.to_string(),
            owner: Owner::none(),
            source_world,
            friend_hosting: FriendHosting::inactive(),
            created_at_epoch_second: now,
            updated_at_epoch_second: now,
        }
    }

    pub fn with_status(mut self, new_status: &str) -> Self {
        self.status = new_status.to_string();
        self.updated_at_epoch_second = now_secs();
        self
    }

    pub fn with_paper_build(mut self, build: u32) -> Self {
        self.paper_build = Some(build);
        self.updated_at_epoch_second = now_secs();
        self
    }

    pub fn with_eula_accepted(mut self, accepted_by: &str) -> Self {
        self.eula = Eula {
            accepted: true,
            accepted_at_epoch_second: Some(now_secs()),
            accepted_by: Some(accepted_by.to_string()),
        };
        self.updated_at_epoch_second = now_secs();
        self
    }

    pub fn with_owner(mut self, owner: Owner) -> Self {
        self.owner = owner;
        self.updated_at_epoch_second = now_secs();
        self
    }
}

/// Reads and writes `nexo-paper-server.json`, one per hosted server, under
/// `<sharedDataDir>/paper-servers/<id>/`.
#[derive(Debug, Clone)]
pub struct PaperServerStore {
    paths: Paths,
}

impl PaperServerStore {
    pub fn new(paths: Paths) -> Self {
        Self { paths }
    }

    pub fn server_dir(&self, id: &str) -> PathBuf {
        self.paths.paper_server(id)
    }

    fn manifest_path(&self, id: &str) -> PathBuf {
        self.server_dir(id).join(MANIFEST)
    }

    /// `None` on any read/parse failure — a missing or unreadable record
    /// just means "no such server," not an error worth surfacing.
    pub async fn read(&self, id: &str) -> Option<PaperServer> {
        if !crate::util::is_safe_id(id) {
            return None;
        }
        let manifest = self.manifest_path(id);
        let raw = tokio::fs::read(&manifest).await.ok()?;
        let record: PaperServer = serde_json::from_slice(&raw).ok()?;
        // The record is shared, hand-editable JSON: its id must be the
        // directory it lives in, or later paths are built from a lie.
        (record.id == id).then_some(record)
    }

    /// ponytail: no cross-process lock — start/stop are user-triggered and
    /// sequential in practice, not truly concurrent. Upgrade to a real lock
    /// file if testing ever shows the mod and launcher racing on a write.
    pub async fn write(&self, record: &PaperServer) -> Result<()> {
        if !crate::util::is_safe_id(&record.id) {
            return Err(crate::error::Error::invalid("unsafe Paper server id"));
        }
        let dir = self.server_dir(&record.id);
        tokio::fs::create_dir_all(&dir).await.ctx(&dir)?;

        let manifest = dir.join(MANIFEST);
        let json = serde_json::to_vec_pretty(record)?;
        super::files::write_private(&manifest, json).await
    }

    /// Every server this machine knows about, newest-created first. Skips
    /// any directory whose record fails to parse.
    pub async fn list(&self) -> Vec<PaperServer> {
        let mut records = Vec::new();
        let dir = self.paths.paper_servers();
        let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
            return records;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let Ok(file_type) = entry.file_type().await else {
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
            let Some(id) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if let Some(record) = self.read(&id).await {
                records.push(record);
            }
        }
        records.sort_by(|a, b| b.created_at_epoch_second.cmp(&a.created_at_epoch_second));
        records
    }
}

pub(crate) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The actual risk this format sharing carries isn't "does serde work,"
    /// it's "does this struct still agree with what `PaperServerRecord.java`
    /// (Gson) emits." This is exactly what that record looks like once
    /// fully populated — field names, nesting, and null-vs-absent handling
    /// all copied from a real write, not from re-reading the Rust struct and
    /// assuming it matches. A full cross-process round trip (write from a
    /// real JVM, read here, and back) is the stronger check
    /// `PAPER-SERVER-REGISTRY.md` asks for; it needs a Java-side interop
    /// tool that doesn't exist yet, same gap `SHARED-ACCOUNT-STORE.md`'s own
    /// interop test currently sits in.
    const JAVA_SHAPED_JSON: &str = r#"{
      "schemaVersion": 1,
      "id": "my-world-server",
      "name": "My World (Paper)",
      "minecraftVersion": "26.1.2",
      "paperBuild": 74,
      "levelName": "world",
      "port": 25566,
      "rcon": { "port": 25576, "password": "s3cr3t" },
      "eula": { "accepted": true, "acceptedAtEpochSecond": 1786300000, "acceptedBy": "mod" },
      "status": "running",
      "owner": { "startedBy": "mod", "pid": 12345, "hostname": "desktop", "startedAtEpochSecond": 1786300000 },
      "sourceWorld": { "originalSaveId": "my-world", "originalSavePath": "/home/user/.minecraft/saves/my-world", "convertedAtEpochSecond": 1786300000 },
      "friendHosting": { "tunnelActive": false, "domain": null },
      "createdAtEpochSecond": 1786299000,
      "updatedAtEpochSecond": 1786300000
    }"#;

    #[test]
    fn parses_the_exact_shape_java_writes() {
        let record: PaperServer =
            serde_json::from_str(JAVA_SHAPED_JSON).expect("Java-shaped JSON must parse");
        assert_eq!(record.id, "my-world-server");
        assert_eq!(record.paper_build, Some(74));
        assert_eq!(record.rcon.password, "s3cr3t");
        assert!(record.eula.accepted);
        assert_eq!(record.eula.accepted_by.as_deref(), Some("mod"));
        assert_eq!(record.status, status::RUNNING);
        assert_eq!(record.owner.pid, Some(12345));
        assert_eq!(
            record.source_world.original_save_path,
            PathBuf::from("/home/user/.minecraft/saves/my-world")
        );
        assert!(!record.friend_hosting.tunnel_active);
        assert_eq!(record.friend_hosting.domain, None);
    }

    #[test]
    fn a_record_with_null_optionals_parses_too() {
        // The state a brand-new record is in before a build is resolved or
        // ownership is claimed — every "or None" field actually absent.
        let json = r#"{
          "schemaVersion": 1,
          "id": "fresh",
          "name": "Fresh World",
          "minecraftVersion": "26.1.2",
          "levelName": "world",
          "port": 25566,
          "rcon": { "port": 25576, "password": "x" },
          "eula": { "accepted": false },
          "status": "converting",
          "owner": {},
          "sourceWorld": { "originalSaveId": "fresh", "originalSavePath": "/x/saves/fresh", "convertedAtEpochSecond": 1 },
          "friendHosting": { "tunnelActive": false },
          "createdAtEpochSecond": 1,
          "updatedAtEpochSecond": 1
        }"#;
        let record: PaperServer =
            serde_json::from_str(json).expect("a record with no owner/build yet must still parse");
        assert_eq!(record.paper_build, None);
        assert_eq!(record.owner.pid, None);
        assert_eq!(record.eula.accepted_by, None);
    }

    #[test]
    fn round_trips_through_json() {
        let record = PaperServer::new(
            "roundtrip".into(),
            "Roundtrip World".into(),
            "26.1.2".into(),
            "world".into(),
            25566,
            Rcon {
                port: 25576,
                password: "pw".into(),
            },
            SourceWorld {
                original_save_id: "roundtrip".into(),
                original_save_path: PathBuf::from("/save/roundtrip"),
                converted_at_epoch_second: 1,
            },
        )
        .with_eula_accepted("launcher")
        .with_paper_build(1)
        .with_owner(Owner {
            started_by: Some("launcher".into()),
            pid: Some(1),
            hostname: Some("host".into()),
            started_at_epoch_second: Some(1),
        })
        .with_status(status::RUNNING);

        let json = serde_json::to_string(&record).unwrap();
        let parsed: PaperServer = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.id, record.id);
        assert_eq!(parsed.status, status::RUNNING);
        assert_eq!(parsed.paper_build, Some(1));
        assert_eq!(parsed.owner.pid, Some(1));
        assert_eq!(
            parsed.source_world.original_save_path,
            PathBuf::from("/save/roundtrip")
        );
    }
}
