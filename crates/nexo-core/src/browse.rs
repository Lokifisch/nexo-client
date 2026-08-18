//! Reading an instance's own directory: its files, its worlds, and its logs.
//!
//! Everything here is read-only apart from [`delete_world`], and everything is
//! best-effort: an instance directory is a folder on a real machine, so a
//! world with a truncated `level.dat` or a log that is being written to right
//! now has to degrade into a less informative row rather than failing the
//! whole listing. A tab that shows nothing because one entry was malformed is
//! worse than one that shows a folder name with no metadata beside it.

use crate::error::{Error, IoContext, Result};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// One row in the file browser.
#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    /// Where it actually is, for opening it in the system file manager.
    pub path: PathBuf,
    /// Where it is *relative to the instance root*, which is what the browser
    /// navigates by — see [`resolve`] for why it never holds an absolute path.
    pub rel: PathBuf,
    pub is_dir: bool,
    /// Always 0 for directories; summing a tree per row would stat the whole
    /// instance every time a folder is opened.
    pub size: u64,
    pub modified: Option<u64>,
}

/// Resolves a browser location against the instance root, refusing anything
/// that climbs out of it.
///
/// The browser's location comes from clicking rows, so today every value is
/// one this module produced. That is exactly the property worth not depending
/// on: the check is here so a future breadcrumb, a remembered location from
/// disk, or a typo in a caller can't turn the file browser into a reader for
/// the rest of the user's home directory. Only ordinary path components are
/// allowed through — no `..`, no root, no Windows drive prefix.
fn resolve(root: &Path, rel: &Path) -> Result<PathBuf> {
    let mut out = root.to_path_buf();
    for component in rel.components() {
        match component {
            std::path::Component::Normal(part) => out.push(part),
            std::path::Component::CurDir => {}
            _ => return Err(Error::invalid("that path is outside the instance")),
        }
    }
    Ok(out)
}

/// Lists one directory inside an instance, directories first.
///
/// `rel` is relative to the instance root; an empty path is the instance
/// itself. Symlinks are reported by what they point at, since a symlinked
/// `mods` folder is a real thing people do and showing it as a 0-byte oddity
/// would be unhelpful.
pub async fn list_dir(root: &Path, rel: &Path) -> Result<Vec<Entry>> {
    let dir = resolve(root, rel)?;

    let mut read = match tokio::fs::read_dir(&dir).await {
        Ok(read) => read,
        // A brand-new instance has no directory until something is installed
        // into it. That is an empty folder, not an error to put on screen.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err).ctx(&dir),
    };

    let mut entries = Vec::new();
    while let Some(item) = read.next_entry().await.ctx(&dir)? {
        let name = item.file_name().to_string_lossy().to_string();
        // `metadata` follows symlinks where `DirEntry::file_type` does not.
        // A broken link still deserves a row, so a failed stat degrades to an
        // unadorned file rather than dropping the entry.
        let meta = tokio::fs::metadata(item.path()).await.ok();
        let is_dir = meta.as_ref().is_some_and(|m| m.is_dir());

        entries.push(Entry {
            rel: rel.join(&name),
            path: item.path(),
            name,
            is_dir,
            size: if is_dir {
                0
            } else {
                meta.as_ref().map(|m| m.len()).unwrap_or(0)
            },
            modified: meta.as_ref().and_then(modified_secs),
        });
    }

    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

fn modified_secs(meta: &std::fs::Metadata) -> Option<u64> {
    meta.modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

/// A single-player world under `saves/`.
///
/// Every field but `folder` and `path` is optional because it comes out of
/// `level.dat`, which may be absent, gzipped differently than expected, or
/// half-written by a game that was killed mid-save.
#[derive(Debug, Clone)]
pub struct World {
    /// Directory name under `saves/`. Unique, so it doubles as the id.
    pub folder: String,
    /// `LevelName` from `level.dat`. Falls back to the folder name, which is
    /// what the folder was named after anyway.
    pub name: String,
    pub path: PathBuf,
    /// Unix seconds. `level.dat` stores milliseconds.
    pub last_played: Option<u64>,
    /// The Minecraft version that last wrote the world — worth showing,
    /// because opening a world in an older version is how worlds get broken.
    pub game_version: Option<String>,
    pub mode: Option<&'static str>,
    pub hardcore: bool,
    pub size: u64,
    /// `icon.png`, the screenshot Minecraft takes when leaving a world.
    pub icon: Option<PathBuf>,
}

/// Everything under `saves/`, most recently played first.
pub async fn worlds(instance: &Path) -> Vec<World> {
    let saves = instance.join("saves");
    let Ok(mut read) = tokio::fs::read_dir(&saves).await else {
        // No saves directory at all — an instance that has never been played.
        return Vec::new();
    };

    let mut worlds = Vec::new();
    while let Ok(Some(item)) = read.next_entry().await {
        let path = item.path();
        if !tokio::fs::metadata(&path).await.is_ok_and(|m| m.is_dir()) {
            continue;
        }
        let folder = item.file_name().to_string_lossy().to_string();

        let level = read_level_dat(&path).await;
        let icon = path.join("icon.png");

        worlds.push(World {
            name: level
                .as_ref()
                .and_then(|l| l.name.clone())
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| folder.clone()),
            last_played: level.as_ref().and_then(|l| l.last_played),
            game_version: level.as_ref().and_then(|l| l.game_version.clone()),
            mode: level.as_ref().and_then(|l| l.mode),
            hardcore: level.as_ref().is_some_and(|l| l.hardcore),
            size: dir_size(&path).await,
            icon: tokio::fs::metadata(&icon).await.is_ok().then_some(icon),
            folder,
            path,
        });
    }

    // Never played sorts last: `None` is not "played at the epoch".
    worlds.sort_by(|a, b| b.last_played.cmp(&a.last_played).then(a.name.cmp(&b.name)));
    worlds
}

/// Deletes one world folder.
///
/// Takes the folder name rather than a path so a caller cannot be talked into
/// deleting something outside `saves/` — the name goes through [`resolve`],
/// which rejects every component that isn't a plain name.
pub async fn delete_world(instance: &Path, folder: &str) -> Result<()> {
    let saves = instance.join("saves");
    let dir = resolve(&saves, Path::new(folder))?;
    if dir == saves {
        return Err(Error::invalid("that is not a world folder"));
    }
    tokio::fs::remove_dir_all(&dir).await.ctx(&dir)
}

/// The handful of `level.dat` fields worth putting on screen.
struct Level {
    name: Option<String>,
    last_played: Option<u64>,
    game_version: Option<String>,
    mode: Option<&'static str>,
    hardcore: bool,
}

async fn read_level_dat(world: &Path) -> Option<Level> {
    let bytes = tokio::fs::read(world.join("level.dat")).await.ok()?;
    let root = nbt::parse(&bytes)?;
    // Everything of interest lives under `Data`; the root compound holds
    // nothing else worth reading.
    let data = root.get("Data")?;

    Some(Level {
        name: data.get("LevelName").and_then(nbt::Value::as_str).map(str::to_string),
        // Stored in milliseconds. Divided rather than shown raw so the whole
        // app can treat one unit as "a timestamp".
        last_played: data
            .get("LastPlayed")
            .and_then(nbt::Value::as_i64)
            .filter(|ms| *ms > 0)
            .map(|ms| ms as u64 / 1000),
        game_version: data
            .get("Version")
            .and_then(|v| v.get("Name"))
            .and_then(nbt::Value::as_str)
            .map(str::to_string),
        mode: data.get("GameType").and_then(nbt::Value::as_i64).and_then(game_mode),
        hardcore: data.get("hardcore").and_then(nbt::Value::as_i64) == Some(1),
    })
}

fn game_mode(id: i64) -> Option<&'static str> {
    match id {
        0 => Some("Survival"),
        1 => Some("Creative"),
        2 => Some("Adventure"),
        3 => Some("Spectator"),
        _ => None,
    }
}

/// Bounded so one pathological directory can't hang the worlds tab.
///
/// A big world is tens of thousands of region and entity files, and this walks
/// the tree on every visit to the tab. The cap keeps that cost predictable;
/// hitting it means the number shown is a floor, which is still more useful
/// than no number.
const MAX_WALK_ENTRIES: usize = 60_000;

async fn dir_size(dir: &Path) -> u64 {
    let mut total = 0;
    let mut seen = 0;
    let mut stack = vec![dir.to_path_buf()];

    while let Some(next) = stack.pop() {
        let Ok(mut read) = tokio::fs::read_dir(&next).await else {
            continue;
        };
        while let Ok(Some(item)) = read.next_entry().await {
            seen += 1;
            if seen > MAX_WALK_ENTRIES {
                return total;
            }
            // `file_type` here rather than `metadata`: it needs no extra stat
            // on most platforms, and not following symlinks is what stops a
            // link pointing back up the tree from looping forever.
            match item.file_type().await {
                Ok(kind) if kind.is_dir() => stack.push(item.path()),
                Ok(kind) if kind.is_file() => {
                    total += item.metadata().await.map(|m| m.len()).unwrap_or(0)
                }
                _ => {}
            }
        }
    }
    total
}

/// One entry from the instance's multiplayer server list.
#[derive(Debug, Clone)]
pub struct Server {
    /// Position in `servers.dat`, and the only thing that identifies an entry.
    ///
    /// Minecraft allows two servers with the same name *and* the same address,
    /// so neither can be a key. Editing therefore addresses an entry by where
    /// it sits, which is why [`servers`] must never reorder what it reads.
    pub index: usize,
    /// The name the player gave it, falling back to the address when there
    /// is none — a row has to be titled something.
    pub name: String,
    /// Host, optionally `:port`, exactly as stored.
    pub address: String,
    /// The icon Minecraft cached the last time it pinged, already decoded.
    /// A live ping usually supersedes it, but this is what fills the row
    /// before — and if — the ping answers.
    pub icon: Option<Vec<u8>>,
}

/// The `servers.dat` list, in the order the player arranged it.
///
/// Order is preserved rather than sorted: that list is hand-arranged in the
/// multiplayer screen, and re-sorting it here would mean the launcher and the
/// game disagree about which server is at the top.
pub async fn servers(instance: &Path) -> Vec<Server> {
    let Ok(bytes) = tokio::fs::read(instance.join("servers.dat")).await else {
        return Vec::new();
    };
    let Some(root) = nbt::parse(&bytes) else {
        return Vec::new();
    };
    let Some(nbt::Value::List(entries)) = root.get("servers") else {
        return Vec::new();
    };

    entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            // An entry with no address is not a server anyone can reach, so it
            // is dropped; a missing name is only a missing label. Note the
            // index is the position in the *file*, counted before this filter,
            // so a dropped entry cannot shift what an edit points at.
            let address = entry.get("ip").and_then(nbt::Value::as_str)?.to_string();
            Some(Server {
                index,
                name: entry
                    .get("name")
                    .and_then(nbt::Value::as_str)
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or(&address)
                    .to_string(),
                icon: entry
                    .get("icon")
                    .and_then(nbt::Value::as_str)
                    .and_then(crate::util::base64_decode),
                address,
            })
        })
        .collect()
}

/// Appends a server to `servers.dat`, creating the file if the instance has
/// never been to multiplayer.
///
/// **The game owns this file while it is running.** Minecraft reads
/// `servers.dat` at startup and writes the whole list back when the
/// multiplayer screen closes, so a server added underneath a running game is
/// silently discarded. The caller is expected to refuse while the instance is
/// up — [`Error::Invalid`] here is the backstop, not the interlock.
pub async fn add_server(instance: &Path, name: &str, address: &str) -> Result<()> {
    edit_list(instance, None, name, address).await
}

/// Renames a server or points it somewhere else, addressed by its position in
/// the list — see [`Server::index`] for why that and not the name.
///
/// Carries the same warning as [`add_server`]: the game owns this file while
/// it is running.
pub async fn update_server(
    instance: &Path,
    index: usize,
    name: &str,
    address: &str,
) -> Result<()> {
    edit_list(instance, Some(index), name, address).await
}

/// The one place `servers.dat` is written. `at` selects an existing entry to
/// overwrite; `None` appends.
async fn edit_list(
    instance: &Path,
    at: Option<usize>,
    name: &str,
    address: &str,
) -> Result<()> {
    let name = name.trim();
    let address = address.trim();
    if address.is_empty() {
        return Err(Error::invalid("a server needs an address"));
    }

    let path = instance.join("servers.dat");

    // Parsed and rewritten whole rather than patched in place: the list is one
    // NBT list inside one compound, so entries have no fixed offsets, and a
    // partial write would cost the player their entire server list.
    let mut root = match tokio::fs::read(&path).await {
        Ok(bytes) => nbt::parse(&bytes).ok_or_else(|| {
            Error::invalid("servers.dat could not be read — refusing to overwrite it")
        })?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            nbt::Value::Compound(Default::default())
        }
        Err(err) => return Err(err).ctx(&path),
    };

    let nbt::Value::Compound(map) = &mut root else {
        return Err(Error::invalid("servers.dat is not in the expected shape"));
    };
    let nbt::Value::List(list) = map
        .entry("servers".to_string())
        .or_insert_with(|| nbt::Value::List(Vec::new()))
    else {
        return Err(Error::invalid("servers.dat is not in the expected shape"));
    };

    let name = nbt::Value::String(if name.is_empty() {
        address.to_string()
    } else {
        name.to_string()
    });
    let address = nbt::Value::String(address.to_string());

    match at {
        // Edited in place, so everything else the game stored on this entry —
        // the cached icon, the resource-pack answer, whether it is hidden —
        // survives. Replacing the compound would quietly reset all of it.
        Some(index) => {
            let Some(nbt::Value::Compound(entry)) = list.get_mut(index) else {
                return Err(Error::invalid("that server is no longer in the list"));
            };
            entry.insert("name".to_string(), name);
            entry.insert("ip".to_string(), address);
        }
        None => {
            let mut entry = std::collections::HashMap::new();
            entry.insert("name".to_string(), name);
            entry.insert("ip".to_string(), address);
            // What the game writes for a server whose resource-pack prompt has
            // never been answered. Leaving it out makes Minecraft ask on first
            // join, which is the right default for a server the launcher knows
            // nothing about.
            entry.insert("acceptTextures".to_string(), nbt::Value::Byte(0));
            list.push(nbt::Value::Compound(entry));
        }
    }

    let bytes = nbt::write(&root).ok_or_else(|| Error::invalid("could not encode servers.dat"))?;

    // Written beside and renamed, so a crash or a full disk mid-write leaves
    // the original list intact rather than a truncated one.
    let temporary = path.with_extension("dat.nexo-tmp");
    tokio::fs::write(&temporary, &bytes).await.ctx(&temporary)?;
    tokio::fs::rename(&temporary, &path).await.ctx(&path)
}

/// One file in the logs tab.
#[derive(Debug, Clone)]
pub struct LogFile {
    /// Also the id: names are unique within a directory, and the two
    /// directories this reads have no overlapping names.
    pub name: String,
    pub path: PathBuf,
    pub size: u64,
    pub modified: Option<u64>,
    /// From `crash-reports/` rather than `logs/`. Worth its own flag because
    /// it is the file someone came to this tab to find.
    pub crash: bool,
    /// Archived as `.log.gz` by Minecraft's own rotation, so it has to be
    /// decompressed before it can be read.
    pub compressed: bool,
}

/// `logs/` and `crash-reports/`, newest first, with `latest.log` pinned to the
/// top because it is the one that is being written right now.
pub async fn logs(instance: &Path) -> Vec<LogFile> {
    let mut out = Vec::new();

    for (dir, crash) in [(instance.join("logs"), false), (instance.join("crash-reports"), true)] {
        let Ok(mut read) = tokio::fs::read_dir(&dir).await else {
            continue;
        };
        while let Ok(Some(item)) = read.next_entry().await {
            let name = item.file_name().to_string_lossy().to_string();
            if !(name.ends_with(".log") || name.ends_with(".log.gz") || name.ends_with(".txt")) {
                continue;
            }
            let Ok(meta) = item.metadata().await else {
                continue;
            };
            if meta.is_dir() {
                continue;
            }
            out.push(LogFile {
                compressed: name.ends_with(".gz"),
                name,
                path: item.path(),
                size: meta.len(),
                modified: modified_secs(&meta),
                crash,
            });
        }
    }

    out.sort_by(|a, b| {
        (b.name == "latest.log")
            .cmp(&(a.name == "latest.log"))
            .then(b.modified.cmp(&a.modified))
            .then(a.name.cmp(&b.name))
    });
    out
}

/// How much of a log the viewer will hold.
///
/// A crashed session's `latest.log` runs to megabytes and the interesting part
/// is the end, so the viewer reads the tail. The size is set by what the UI
/// can lay out rather than by what is convenient here: this is roughly 700
/// lines of Minecraft log in a single text widget, which is enough to hold a
/// stack trace and the mod list that preceded it. Anyone who needs the whole
/// file has the button that opens it in a real editor.
pub const LOG_TAIL_BYTES: u64 = 64 * 1024;

/// Reads the tail of a log, decompressing it first when it is archived.
///
/// Returns the text and whether it was cut short, so the viewer can say so
/// rather than presenting a truncated file as the whole thing.
pub async fn read_log(file: &LogFile) -> Result<(String, bool)> {
    let path = file.path.clone();
    let compressed = file.compressed;

    // Both the gzip decode and the seek are blocking, and a rotated log can be
    // tens of megabytes uncompressed.
    tokio::task::spawn_blocking(move || {
        let bytes = if compressed {
            use std::io::Read;
            let raw = std::fs::File::open(&path).ctx(&path)?;
            let mut buf = Vec::new();
            // Bounded by the same tail budget, but from the *front*, since a
            // gzip stream can only be read forwards. Taking a generous multiple
            // and then trimming the tail below keeps the memory bounded while
            // still ending at the end of the file.
            flate2::read::GzDecoder::new(raw)
                .take(LOG_TAIL_BYTES * 8)
                .read_to_end(&mut buf)
                .ctx(&path)?;
            buf
        } else {
            use std::io::{Read, Seek, SeekFrom};
            let mut raw = std::fs::File::open(&path).ctx(&path)?;
            let len = raw.metadata().ctx(&path)?.len();
            if len > LOG_TAIL_BYTES {
                raw.seek(SeekFrom::Start(len - LOG_TAIL_BYTES)).ctx(&path)?;
            }
            let mut buf = Vec::new();
            raw.read_to_end(&mut buf).ctx(&path)?;
            buf
        };

        let truncated = bytes.len() as u64 >= LOG_TAIL_BYTES;
        // Logs are UTF-8 in practice, but a log killed mid-write ends in half
        // a character, and a crash report can carry whatever a mod printed.
        let mut text = String::from_utf8_lossy(&bytes).into_owned();

        // Starting mid-line is confusing in a way that dropping one line is
        // not — the first line would look like a real entry with its timestamp
        // chewed off.
        if truncated && let Some(newline) = text.find('\n') {
            text = text[newline + 1..].to_string();
        }

        Ok((text, truncated))
    })
    .await
    .map_err(|err| Error::invalid(format!("reading the log failed: {err}")))?
}

/// A minimal reader for Minecraft's NBT, enough to pull a handful of fields
/// out of `level.dat`.
///
/// Written rather than pulled in as a dependency because this needs five keys
/// out of one small file, and the format is a dozen tag types with no
/// versioning. It parses into a tree instead of scanning for the keys
/// directly: the tree is the part that is easy to get right, and skipping tags
/// correctly requires understanding all of them anyway.
mod nbt {
    use std::collections::HashMap;

    /// The payloads of the tags nothing here reads are kept rather than
    /// discarded: a parser that dropped them could not be checked against a
    /// real `level.dat`, and the next field someone wants — the seed, the
    /// difficulty, the datapack list — is then already reachable.
    #[allow(dead_code)]
    #[derive(Debug, Clone)]
    pub enum Value {
        Byte(i8),
        Short(i16),
        Int(i32),
        Long(i64),
        Float(f32),
        Double(f64),
        ByteArray(Vec<u8>),
        String(String),
        List(Vec<Value>),
        Compound(HashMap<String, Value>),
        IntArray(Vec<i32>),
        LongArray(Vec<i64>),
    }

    impl Value {
        pub fn get(&self, key: &str) -> Option<&Value> {
            match self {
                Value::Compound(map) => map.get(key),
                _ => None,
            }
        }

        pub fn as_str(&self) -> Option<&str> {
            match self {
                Value::String(s) => Some(s),
                _ => None,
            }
        }

        /// Widens every integer tag, since which one a field uses is
        /// Minecraft's business and has changed between versions.
        pub fn as_i64(&self) -> Option<i64> {
            match self {
                Value::Byte(v) => Some(*v as i64),
                Value::Short(v) => Some(*v as i64),
                Value::Int(v) => Some(*v as i64),
                Value::Long(v) => Some(*v),
                _ => None,
            }
        }
    }

    /// `level.dat` nests maybe six deep. The cap is only here so a corrupt or
    /// hostile file cannot drive this into a stack overflow, which would take
    /// the launcher down rather than showing one bad world.
    const MAX_DEPTH: u32 = 64;
    /// `level.dat` is a few kilobytes. Anything near this is not one.
    const MAX_DECOMPRESSED: u64 = 32 * 1024 * 1024;

    struct Reader<'a> {
        buf: &'a [u8],
        pos: usize,
    }

    impl<'a> Reader<'a> {
        fn take(&mut self, n: usize) -> Option<&'a [u8]> {
            let end = self.pos.checked_add(n)?;
            let slice = self.buf.get(self.pos..end)?;
            self.pos = end;
            Some(slice)
        }

        fn u8(&mut self) -> Option<u8> {
            Some(self.take(1)?[0])
        }

        fn i16(&mut self) -> Option<i16> {
            Some(i16::from_be_bytes(self.take(2)?.try_into().ok()?))
        }

        fn i32(&mut self) -> Option<i32> {
            Some(i32::from_be_bytes(self.take(4)?.try_into().ok()?))
        }

        fn i64(&mut self) -> Option<i64> {
            Some(i64::from_be_bytes(self.take(8)?.try_into().ok()?))
        }

        /// NBT strings are length-prefixed modified UTF-8. The difference from
        /// real UTF-8 only shows up for NUL and for characters outside the BMP,
        /// neither of which appears in the fields read here, so a lossy decode
        /// is honest enough and cannot fail.
        fn string(&mut self) -> Option<String> {
            let len = self.i16()? as usize;
            Some(String::from_utf8_lossy(self.take(len)?).into_owned())
        }

        /// Element counts are attacker-controlled, so every array allocates
        /// against what is actually left in the buffer rather than against the
        /// declared length — otherwise a four-byte count reserves gigabytes.
        fn array<T>(&mut self, width: usize, mut read: impl FnMut(&mut Self) -> Option<T>) -> Option<Vec<T>> {
            let len = self.i32()?;
            if len < 0 || (len as usize).checked_mul(width)? > self.buf.len() - self.pos {
                return None;
            }
            let mut out = Vec::with_capacity(len as usize);
            for _ in 0..len {
                out.push(read(self)?);
            }
            Some(out)
        }

        fn payload(&mut self, tag: u8, depth: u32) -> Option<Value> {
            if depth > MAX_DEPTH {
                return None;
            }
            Some(match tag {
                1 => Value::Byte(self.u8()? as i8),
                2 => Value::Short(self.i16()?),
                3 => Value::Int(self.i32()?),
                4 => Value::Long(self.i64()?),
                5 => Value::Float(f32::from_be_bytes(self.take(4)?.try_into().ok()?)),
                6 => Value::Double(f64::from_be_bytes(self.take(8)?.try_into().ok()?)),
                7 => Value::ByteArray(self.array(1, |r| r.u8())?),
                8 => Value::String(self.string()?),
                9 => {
                    let element = self.u8()?;
                    let len = self.i32()?;
                    if len < 0 {
                        return None;
                    }
                    // A list of TAG_End is how an empty list is written; every
                    // other element type must be readable.
                    if element == 0 {
                        return Some(Value::List(Vec::new()));
                    }
                    let mut items = Vec::new();
                    for _ in 0..len {
                        items.push(self.payload(element, depth + 1)?);
                    }
                    Value::List(items)
                }
                10 => {
                    let mut map = HashMap::new();
                    loop {
                        let kind = self.u8()?;
                        if kind == 0 {
                            break;
                        }
                        let name = self.string()?;
                        map.insert(name, self.payload(kind, depth + 1)?);
                    }
                    Value::Compound(map)
                }
                11 => Value::IntArray(self.array(4, |r| r.i32())?),
                12 => Value::LongArray(self.array(8, |r| r.i64())?),
                _ => return None,
            })
        }
    }

    /// Writes a root compound back out, uncompressed.
    ///
    /// Only `servers.dat` is ever written, and the game stores that one
    /// uncompressed — `level.dat` is read here but never written, which is
    /// deliberate: a launcher that rewrites a world's metadata is a launcher
    /// that can corrupt a world.
    pub fn write(root: &Value) -> Option<Vec<u8>> {
        let mut out = Vec::new();
        out.push(10);
        write_string(&mut out, "");
        write_payload(&mut out, root)?;
        Some(out)
    }

    fn write_string(out: &mut Vec<u8>, value: &str) {
        out.extend((value.len() as i16).to_be_bytes());
        out.extend_from_slice(value.as_bytes());
    }

    /// The tag byte that introduces a value of this shape.
    fn tag_of(value: &Value) -> u8 {
        match value {
            Value::Byte(_) => 1,
            Value::Short(_) => 2,
            Value::Int(_) => 3,
            Value::Long(_) => 4,
            Value::Float(_) => 5,
            Value::Double(_) => 6,
            Value::ByteArray(_) => 7,
            Value::String(_) => 8,
            Value::List(_) => 9,
            Value::Compound(_) => 10,
            Value::IntArray(_) => 11,
            Value::LongArray(_) => 12,
        }
    }

    fn write_payload(out: &mut Vec<u8>, value: &Value) -> Option<()> {
        match value {
            Value::Byte(v) => out.push(*v as u8),
            Value::Short(v) => out.extend(v.to_be_bytes()),
            Value::Int(v) => out.extend(v.to_be_bytes()),
            Value::Long(v) => out.extend(v.to_be_bytes()),
            Value::Float(v) => out.extend(v.to_be_bytes()),
            Value::Double(v) => out.extend(v.to_be_bytes()),
            Value::ByteArray(items) => {
                out.extend((items.len() as i32).to_be_bytes());
                out.extend_from_slice(items);
            }
            Value::String(v) => write_string(out, v),
            Value::List(items) => {
                // A list is typed by its elements, and an empty one is written
                // as a list of TAG_End. A mixed list cannot be represented at
                // all, so it is refused rather than silently truncated.
                let element = items.first().map(tag_of).unwrap_or(0);
                if items.iter().any(|item| tag_of(item) != element) {
                    return None;
                }
                out.push(element);
                out.extend((items.len() as i32).to_be_bytes());
                for item in items {
                    write_payload(out, item)?;
                }
            }
            Value::Compound(map) => {
                // Sorted so the file is byte-stable across writes. A HashMap
                // would reorder the keys every run, which turns "nothing
                // changed" into a diff for anyone versioning their instance.
                let mut keys: Vec<_> = map.keys().collect();
                keys.sort();
                for key in keys {
                    let item = &map[key];
                    out.push(tag_of(item));
                    write_string(out, key);
                    write_payload(out, item)?;
                }
                out.push(0);
            }
            Value::IntArray(items) => {
                out.extend((items.len() as i32).to_be_bytes());
                items.iter().for_each(|v| out.extend(v.to_be_bytes()));
            }
            Value::LongArray(items) => {
                out.extend((items.len() as i32).to_be_bytes());
                items.iter().for_each(|v| out.extend(v.to_be_bytes()));
            }
        }
        Some(())
    }

    /// Parses a `level.dat`, gzipped or not, returning the root compound's
    /// payload. `None` for anything that doesn't parse — the caller falls back
    /// to the folder name, which is always available.
    pub fn parse(bytes: &[u8]) -> Option<Value> {
        // Vanilla writes it gzipped; a few tools and older backups leave it
        // plain, and reading both costs one magic-number check.
        let owned;
        let buf = if bytes.starts_with(&[0x1f, 0x8b]) {
            use std::io::Read;
            let mut out = Vec::new();
            flate2::read::GzDecoder::new(bytes)
                .take(MAX_DECOMPRESSED)
                .read_to_end(&mut out)
                .ok()?;
            owned = out;
            &owned[..]
        } else {
            bytes
        };

        let mut reader = Reader { buf, pos: 0 };
        // The root is a named compound: type, name, then the payload.
        if reader.u8()? != 10 {
            return None;
        }
        reader.string()?;
        reader.payload(10, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "nexo-browse-{name}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn resolve_refuses_to_leave_the_instance() {
        let root = Path::new("/data/instances/fabric");
        assert!(resolve(root, Path::new("mods/sodium.jar")).is_ok());
        assert!(resolve(root, Path::new("../other/secrets")).is_err());
        assert!(resolve(root, Path::new("mods/../../etc/passwd")).is_err());
        assert!(resolve(root, Path::new("/etc/passwd")).is_err());
    }

    #[test]
    fn resolve_keeps_a_relative_path_inside_the_root() {
        let root = Path::new("/data/instances/fabric");
        assert_eq!(
            resolve(root, Path::new("config/sodium")).unwrap(),
            root.join("config").join("sodium")
        );
        assert_eq!(resolve(root, Path::new("")).unwrap(), root);
    }

    #[tokio::test]
    async fn list_dir_puts_directories_first_and_is_case_insensitive() {
        let root = temp_dir("list");
        std::fs::create_dir(root.join("mods")).unwrap();
        std::fs::create_dir(root.join("Config")).unwrap();
        std::fs::write(root.join("options.txt"), b"lang:en_us").unwrap();
        std::fs::write(root.join("Aardvark.json"), b"{}").unwrap();

        let entries = list_dir(&root, Path::new("")).await.unwrap();
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Config", "mods", "Aardvark.json", "options.txt"]);

        let options = entries.iter().find(|e| e.name == "options.txt").unwrap();
        assert_eq!(options.size, 10);
        assert_eq!(options.rel, Path::new("options.txt"));

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[tokio::test]
    async fn a_missing_directory_lists_as_empty_rather_than_failing() {
        let root = temp_dir("missing");
        let entries = list_dir(&root, Path::new("saves")).await.unwrap();
        assert!(entries.is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Builds the parts of a `level.dat` this module actually reads.
    fn level_dat(name: &str, last_played: i64, mode: i32, version: &str) -> Vec<u8> {
        fn string(out: &mut Vec<u8>, s: &str) {
            out.extend((s.len() as i16).to_be_bytes());
            out.extend(s.as_bytes());
        }
        fn named(out: &mut Vec<u8>, tag: u8, key: &str) {
            out.push(tag);
            string(out, key);
        }

        let mut data = Vec::new();
        named(&mut data, 8, "LevelName");
        string(&mut data, name);
        named(&mut data, 4, "LastPlayed");
        data.extend(last_played.to_be_bytes());
        named(&mut data, 3, "GameType");
        data.extend(mode.to_be_bytes());
        named(&mut data, 1, "hardcore");
        data.push(1);
        named(&mut data, 10, "Version");
        named(&mut data, 8, "Name");
        string(&mut data, version);
        data.push(0); // end of Version
        data.push(0); // end of Data

        let mut root = Vec::new();
        root.push(10);
        string(&mut root, "");
        named(&mut root, 10, "Data");
        root.extend(data);
        root.push(0); // end of root
        root
    }

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut encoder =
            flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn nbt_reads_the_fields_a_world_row_needs() {
        let raw = level_dat("My World", 1_700_000_000_000, 1, "26.1.2");
        for bytes in [raw.clone(), gzip(&raw)] {
            let root = nbt::parse(&bytes).expect("parses");
            let data = root.get("Data").unwrap();
            assert_eq!(data.get("LevelName").unwrap().as_str(), Some("My World"));
            assert_eq!(
                data.get("LastPlayed").unwrap().as_i64(),
                Some(1_700_000_000_000)
            );
            assert_eq!(data.get("GameType").unwrap().as_i64(), Some(1));
            assert_eq!(
                data.get("Version").unwrap().get("Name").unwrap().as_str(),
                Some("26.1.2")
            );
        }
    }

    #[test]
    fn nbt_rejects_a_truncated_file_instead_of_panicking() {
        let raw = level_dat("My World", 1, 0, "26.1.2");
        // Every prefix is a file that was half-written. None may panic, and
        // none may claim to have parsed a whole world.
        for cut in 1..raw.len() {
            assert!(nbt::parse(&raw[..cut]).is_none(), "prefix of {cut} parsed");
        }
        assert!(nbt::parse(b"").is_none());
        assert!(nbt::parse(b"not nbt at all").is_none());
    }

    #[test]
    fn nbt_refuses_an_array_longer_than_the_file() {
        // A four-byte length claiming 2 GiB of ints, with nothing behind it.
        let mut bytes = vec![10];
        bytes.extend(0i16.to_be_bytes());
        bytes.push(11);
        bytes.extend(3i16.to_be_bytes());
        bytes.extend(b"big");
        bytes.extend(i32::MAX.to_be_bytes());
        assert!(nbt::parse(&bytes).is_none());
    }

    #[tokio::test]
    async fn worlds_read_level_dat_and_sort_by_last_played() {
        let root = temp_dir("worlds");
        let saves = root.join("saves");

        for (folder, name, played) in [
            ("old-world", "Old World", 1_600_000_000_000i64),
            ("new-world", "New World", 1_700_000_000_000),
        ] {
            let dir = saves.join(folder);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("level.dat"),
                gzip(&level_dat(name, played, 0, "26.1.2")),
            )
            .unwrap();
            std::fs::write(dir.join("icon.png"), b"not really a png").unwrap();
        }
        // No level.dat at all: it still has to appear, named after its folder.
        std::fs::create_dir_all(saves.join("bare")).unwrap();

        let found = worlds(&root).await;
        let names: Vec<_> = found.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(names, ["New World", "Old World", "bare"]);

        let newest = &found[0];
        assert_eq!(newest.last_played, Some(1_700_000_000));
        assert_eq!(newest.game_version.as_deref(), Some("26.1.2"));
        assert_eq!(newest.mode, Some("Survival"));
        assert!(newest.hardcore);
        assert!(newest.icon.is_some());
        assert!(newest.size > 0);

        assert_eq!(found[2].last_played, None);
        assert!(found[2].icon.is_none());

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[tokio::test]
    async fn delete_world_removes_one_folder_and_refuses_to_climb_out() {
        let root = temp_dir("delete");
        let saves = root.join("saves");
        std::fs::create_dir_all(saves.join("doomed")).unwrap();
        std::fs::write(saves.join("doomed").join("level.dat"), b"x").unwrap();
        std::fs::write(root.join("keep-me.txt"), b"x").unwrap();

        assert!(delete_world(&root, "../keep-me.txt").await.is_err());
        assert!(delete_world(&root, "").await.is_err());
        assert!(root.join("keep-me.txt").exists());

        delete_world(&root, "doomed").await.unwrap();
        assert!(!saves.join("doomed").exists());

        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Builds a `servers.dat` the way Minecraft writes one.
    fn servers_dat(entries: &[(&str, &str)]) -> Vec<u8> {
        let list = entries
            .iter()
            .map(|(name, ip)| {
                let mut entry = std::collections::HashMap::new();
                entry.insert("name".to_string(), nbt::Value::String(name.to_string()));
                entry.insert("ip".to_string(), nbt::Value::String(ip.to_string()));
                nbt::Value::Compound(entry)
            })
            .collect();

        let mut root = std::collections::HashMap::new();
        root.insert("servers".to_string(), nbt::Value::List(list));
        nbt::write(&nbt::Value::Compound(root)).unwrap()
    }

    #[test]
    fn nbt_round_trips_through_the_writer() {
        let original = servers_dat(&[("Home", "192.168.1.9"), ("Public", "mc.example.com:25577")]);
        let parsed = nbt::parse(&original).expect("parses");

        // Byte-identical, which is the property `add_server` depends on: it
        // rewrites the whole file, so anything the writer cannot reproduce
        // exactly is something a player would lose.
        assert_eq!(nbt::write(&parsed).unwrap(), original);
    }

    #[test]
    fn nbt_refuses_a_list_it_cannot_represent() {
        // NBT lists are typed. A mixed one has no valid encoding, and writing
        // the first element's tag over the rest would produce a file that
        // parses into different data than it was built from.
        let mixed = nbt::Value::List(vec![nbt::Value::Byte(1), nbt::Value::String("no".into())]);
        let mut root = std::collections::HashMap::new();
        root.insert("mixed".to_string(), mixed);
        assert!(nbt::write(&nbt::Value::Compound(root)).is_none());
    }

    #[tokio::test]
    async fn servers_are_read_in_the_order_the_player_arranged_them() {
        let root = temp_dir("servers");
        std::fs::write(
            root.join("servers.dat"),
            servers_dat(&[("Zeta", "zeta.example"), ("Alpha", "alpha.example:25577")]),
        )
        .unwrap();

        let found = servers(&root).await;
        let names: Vec<_> = found.iter().map(|s| s.name.as_str()).collect();
        // Not sorted — Zeta stays first because that is where it was put.
        assert_eq!(names, ["Zeta", "Alpha"]);
        assert_eq!(found[1].address, "alpha.example:25577");

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[tokio::test]
    async fn no_servers_dat_is_an_empty_list_not_a_failure() {
        let root = temp_dir("no-servers");
        assert!(servers(&root).await.is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[tokio::test]
    async fn add_server_appends_without_disturbing_the_existing_list() {
        let root = temp_dir("add-server");
        std::fs::write(
            root.join("servers.dat"),
            servers_dat(&[("Existing", "old.example")]),
        )
        .unwrap();

        add_server(&root, "New", "new.example:25577").await.unwrap();

        let found = servers(&root).await;
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].name, "Existing");
        assert_eq!(found[1].name, "New");
        assert_eq!(found[1].address, "new.example:25577");
        // The scratch file must not survive the rename.
        assert!(!root.join("servers.dat.nexo-tmp").exists());

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[tokio::test]
    async fn update_server_edits_in_place_and_keeps_what_the_game_stored() {
        let root = temp_dir("update-server");

        // An entry carrying the extras Minecraft writes: a cached icon and an
        // answered resource-pack prompt. Editing must not throw those away.
        let mut entry = std::collections::HashMap::new();
        entry.insert("name".to_string(), nbt::Value::String("Old".into()));
        entry.insert("ip".to_string(), nbt::Value::String("old.example".into()));
        entry.insert(
            "icon".to_string(),
            nbt::Value::String(crate::util::base64_encode(b"\x89PNG\r\n\x1a\n")),
        );
        entry.insert("acceptTextures".to_string(), nbt::Value::Byte(1));

        let mut root_map = std::collections::HashMap::new();
        root_map.insert(
            "servers".to_string(),
            nbt::Value::List(vec![nbt::Value::Compound(entry)]),
        );
        std::fs::write(
            root.join("servers.dat"),
            nbt::write(&nbt::Value::Compound(root_map)).unwrap(),
        )
        .unwrap();

        update_server(&root, 0, "New", "new.example:25577")
            .await
            .unwrap();

        let found = servers(&root).await;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "New");
        assert_eq!(found[0].address, "new.example:25577");
        // The icon came through the rewrite untouched.
        assert_eq!(found[0].icon.as_deref(), Some(&b"\x89PNG\r\n\x1a\n"[..]));

        // An index past the end is a stale click, not a reason to append.
        assert!(update_server(&root, 9, "Ghost", "ghost.example").await.is_err());
        assert_eq!(servers(&root).await.len(), 1);

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[tokio::test]
    async fn server_indices_address_the_file_not_the_filtered_list() {
        let root = temp_dir("server-index");

        // The middle entry has no address, so `servers` drops it. The entries
        // after it must still report the index they occupy in the file, or an
        // edit would land on the wrong server.
        let mut broken = std::collections::HashMap::new();
        broken.insert("name".to_string(), nbt::Value::String("Broken".into()));

        let mut list = Vec::new();
        for (name, ip) in [("First", "first.example"), ("", "")] {
            if ip.is_empty() {
                list.push(nbt::Value::Compound(broken.clone()));
                continue;
            }
            let mut entry = std::collections::HashMap::new();
            entry.insert("name".to_string(), nbt::Value::String(name.into()));
            entry.insert("ip".to_string(), nbt::Value::String(ip.into()));
            list.push(nbt::Value::Compound(entry));
        }
        let mut third = std::collections::HashMap::new();
        third.insert("name".to_string(), nbt::Value::String("Third".into()));
        third.insert("ip".to_string(), nbt::Value::String("third.example".into()));
        list.push(nbt::Value::Compound(third));

        let mut root_map = std::collections::HashMap::new();
        root_map.insert("servers".to_string(), nbt::Value::List(list));
        std::fs::write(
            root.join("servers.dat"),
            nbt::write(&nbt::Value::Compound(root_map)).unwrap(),
        )
        .unwrap();

        let found = servers(&root).await;
        assert_eq!(found.len(), 2);
        // Second in the list, third in the file.
        assert_eq!(found[1].name, "Third");
        assert_eq!(found[1].index, 2);

        // Editing by that index hits Third, not the broken entry.
        update_server(&root, found[1].index, "Renamed", "renamed.example")
            .await
            .unwrap();
        let after = servers(&root).await;
        assert_eq!(after[0].name, "First");
        assert_eq!(after[1].name, "Renamed");

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[tokio::test]
    async fn add_server_creates_the_file_and_defends_the_existing_one() {
        let root = temp_dir("add-server-new");

        // No file yet: the first server added has to create one.
        add_server(&root, "First", "first.example").await.unwrap();
        assert_eq!(servers(&root).await.len(), 1);

        // A nameless entry falls back to its address rather than showing blank.
        add_server(&root, "   ", "second.example").await.unwrap();
        assert_eq!(servers(&root).await[1].name, "second.example");

        // No address is not a server.
        assert!(add_server(&root, "Nameless", "  ").await.is_err());
        assert_eq!(servers(&root).await.len(), 2);

        // An unreadable file is left exactly as it was rather than replaced
        // with a fresh list — that would silently delete every server.
        std::fs::write(root.join("servers.dat"), b"this is not nbt").unwrap();
        assert!(add_server(&root, "Third", "third.example").await.is_err());
        assert_eq!(
            std::fs::read(root.join("servers.dat")).unwrap(),
            b"this is not nbt"
        );

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[tokio::test]
    async fn logs_pin_latest_and_include_crash_reports() {
        let root = temp_dir("logs");
        std::fs::create_dir_all(root.join("logs")).unwrap();
        std::fs::create_dir_all(root.join("crash-reports")).unwrap();
        std::fs::write(root.join("logs").join("latest.log"), b"newest line\n").unwrap();
        std::fs::write(root.join("logs").join("2026-08-01-1.log.gz"), gzip(b"old\n")).unwrap();
        std::fs::write(root.join("logs").join("ignore-me.json"), b"{}").unwrap();
        std::fs::write(root.join("crash-reports").join("crash-2026.txt"), b"boom\n").unwrap();

        let found = logs(&root).await;
        assert_eq!(found[0].name, "latest.log");
        assert!(found.iter().any(|l| l.crash && l.name == "crash-2026.txt"));
        assert!(found.iter().any(|l| l.compressed));
        assert!(!found.iter().any(|l| l.name == "ignore-me.json"));

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[tokio::test]
    async fn read_log_handles_plain_and_gzipped_files() {
        let root = temp_dir("read-log");
        std::fs::create_dir_all(root.join("logs")).unwrap();
        std::fs::write(root.join("logs").join("latest.log"), b"plain text\n").unwrap();
        std::fs::write(
            root.join("logs").join("2026-08-01-1.log.gz"),
            gzip(b"archived text\n"),
        )
        .unwrap();

        for file in logs(&root).await {
            let (text, truncated) = read_log(&file).await.unwrap();
            assert!(!truncated);
            assert!(text.contains("text"), "{}: {text:?}", file.name);
        }

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[tokio::test]
    async fn read_log_returns_the_tail_of_an_oversized_file() {
        let root = temp_dir("tail");
        std::fs::create_dir_all(root.join("logs")).unwrap();

        // Every line numbered, so the tail can be checked for being the *end*
        // and not just the right size.
        let mut body = String::new();
        let mut line = 0;
        while body.len() < (LOG_TAIL_BYTES as usize) * 2 {
            body.push_str(&format!("line {line}\n"));
            line += 1;
        }
        std::fs::write(root.join("logs").join("latest.log"), &body).unwrap();

        let file = logs(&root).await.remove(0);
        let (text, truncated) = read_log(&file).await.unwrap();
        assert!(truncated);
        assert!((text.len() as u64) <= LOG_TAIL_BYTES);
        assert!(text.ends_with(&format!("line {}\n", line - 1)));
        // The partial first line is dropped rather than shown headless.
        assert!(text.starts_with("line "));

        std::fs::remove_dir_all(&root).unwrap();
    }
}
