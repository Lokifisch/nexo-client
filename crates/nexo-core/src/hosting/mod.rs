//! Servers made from scratch — Paper, vanilla or Fabric — run on this
//! machine, optionally made public through Nexo's relay ([`tunnel`]).
//!
//! Lives beside `paper_server` rather than inside it: that module's on-disk
//! record is a contract with Nexo Mod, and every record there is a converted
//! singleplayer world the Mod knows how to revert. A server made here has no
//! source world, so it gets its own folder (`<data>/servers/<id>/`) and its
//! own record (`nexo-server.json`), which only the launcher reads.
//!
//! Control follows the same rule as `paper_server`: commands and stop go over
//! RCON, so a server left running by an earlier launcher session can still be
//! driven. The process's stdout/stderr go to `nexo-console.log` in the server
//! folder — unlike `logs/latest.log`, it also holds whatever a server prints
//! before its logger is up, which is where a wrong-Java crash ends up.

pub mod properties;
pub mod tunnel;

use crate::error::{Error, IoContext, Result};
use crate::minecraft::download::{DownloadTask, Downloader};
use crate::minecraft::{Installer, fabric};
use crate::modrinth::Modrinth;
use crate::paper_server::rcon::RconClient;
use crate::paper_server::{Rcon, files, paper_api};
use crate::paths::Paths;
use crate::running::RunningGames;
use crate::util::{is_bare_basename, is_safe_id, no_window_async, slugify};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const RECORD: &str = "nexo-server.json";
pub const CONSOLE_LOG: &str = "nexo-console.log";
const JAR: &str = "server.jar";
/// Paper's first start on a fresh world is a minute or two on ordinary
/// hardware; Fabric's first start also downloads the vanilla server.
const READY_TIMEOUT: Duration = Duration::from_secs(300);
const STOP_TIMEOUT: Duration = Duration::from_secs(60);
const RCON_CALL_TIMEOUT: Duration = Duration::from_secs(5);
const CONSOLE_TAIL_BYTES: u64 = 192 * 1024;
const MAX_ADDONS: usize = 64;

/// A Modrinth project id or slug: nothing that could reshape a URL path.
fn is_modrinth_id(s: &str) -> bool {
    (1..=64).contains(&s.len()) && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServerKind {
    Paper,
    Vanilla,
    Fabric,
}

impl ServerKind {
    pub const ALL: [ServerKind; 3] = [ServerKind::Paper, ServerKind::Vanilla, ServerKind::Fabric];

    pub fn label(self) -> &'static str {
        match self {
            ServerKind::Paper => "Paper",
            ServerKind::Vanilla => "Vanilla",
            ServerKind::Fabric => "Fabric",
        }
    }

    /// Where add-ons go, and how to find them on Modrinth. `None`: none at all.
    pub fn content(self) -> Option<(&'static str, &'static str, Option<&'static str>)> {
        // (folder, Modrinth loader facet, Modrinth project type). No type for
        // Paper: multi-platform plugins (ViaVersion, LuckPerms, WorldEdit)
        // are typed "mod" there, and the `paper` loader alone finds them all.
        match self {
            ServerKind::Paper => Some(("plugins", "paper", None)),
            ServerKind::Fabric => Some(("mods", "fabric", Some("mod"))),
            ServerKind::Vanilla => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostedServer {
    pub id: String,
    pub name: String,
    pub kind: ServerKind,
    pub minecraft_version: String,
    pub memory_mb: u32,
    pub port: u16,
    pub rcon: Rcon,
    /// Open a public relay address whenever the server starts.
    pub tunnel: bool,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub started_at: Option<u64>,
    pub created_at: u64,
}

#[derive(Debug, Clone)]
pub enum WorldSource {
    New {
        seed: String,
        /// A `level-type` value, e.g. `minecraft:flat`.
        level_type: String,
        game_mode: String,
        difficulty: String,
        hardcore: bool,
    },
    /// A singleplayer save to copy in. The save itself is never touched.
    Import(PathBuf),
}

#[derive(Debug, Clone)]
pub struct ServerSpec {
    pub name: String,
    pub kind: ServerKind,
    pub minecraft_version: String,
    pub world: WorldSource,
    pub memory_mb: u32,
    pub tunnel: bool,
    /// Modrinth project ids, installed into `plugins/` or `mods/`.
    pub content: Vec<String>,
    /// Must come from a real click on a EULA consent control.
    pub eula_accepted: bool,
}

#[derive(Debug, Clone)]
pub struct Created {
    pub server: HostedServer,
    /// Add-ons that couldn't be installed. The server is still made.
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct Sample {
    /// Share of the whole machine, 0-100.
    pub cpu_percent: f32,
    pub memory_mb: u32,
}

#[derive(Debug, Clone, Default)]
pub struct Players {
    pub online: u32,
    pub max: u32,
    pub names: Vec<String>,
}

#[derive(Clone)]
pub struct Hosting {
    http: reqwest::Client,
    paths: Paths,
    installer: Installer,
    modrinth: Modrinth,
    running: RunningGames,
    tunnels: Arc<Mutex<HashMap<String, tunnel::Tunnel>>>,
    /// Bumped by every close, so an open that was in flight when the server
    /// stopped (or the toggle went off) knows to shut itself down.
    tunnel_generation: Arc<Mutex<HashMap<String, u64>>>,
    system: Arc<Mutex<sysinfo::System>>,
}

impl Hosting {
    pub fn new(http: reqwest::Client, paths: Paths, installer: Installer) -> Self {
        Self {
            modrinth: Modrinth::with_client(http.clone()),
            http,
            paths,
            installer,
            running: RunningGames::new(),
            tunnels: Default::default(),
            tunnel_generation: Default::default(),
            system: Default::default(),
        }
    }

    pub fn server_dir(&self, id: &str) -> PathBuf {
        self.paths.servers().join(id)
    }

    pub fn modrinth(&self) -> &Modrinth {
        &self.modrinth
    }

    pub async fn list(&self) -> Vec<HostedServer> {
        let mut out = Vec::new();
        let Ok(mut entries) = tokio::fs::read_dir(self.paths.servers()).await else {
            return out;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            if let Some(id) = entry.file_name().to_str()
                && let Ok(server) = self.read(id).await
            {
                out.push(server);
            }
        }
        out.sort_by_key(|s| std::cmp::Reverse(s.created_at));
        out
    }

    pub async fn read(&self, id: &str) -> Result<HostedServer> {
        if !is_safe_id(id) {
            return Err(Error::invalid("unsafe server id"));
        }
        let path = self.server_dir(id).join(RECORD);
        let server: HostedServer = serde_json::from_slice(&tokio::fs::read(&path).await.ctx(&path)?)?;
        if server.id != id {
            return Err(Error::invalid("server record does not match its folder"));
        }
        Ok(server)
    }

    async fn write(&self, server: &HostedServer) -> Result<()> {
        if !is_safe_id(&server.id) {
            return Err(Error::invalid("unsafe server id"));
        }
        // Owner-only: holds the RCON password.
        files::write_private(
            &self.server_dir(&server.id).join(RECORD),
            serde_json::to_vec_pretty(server)?,
        )
        .await
    }

    /// Makes the server folder: world, `eula.txt`, `server.properties`, the
    /// server jar and any add-ons. Doesn't start it.
    pub async fn create(&self, spec: ServerSpec) -> Result<Created> {
        if !spec.eula_accepted {
            return Err(Error::invalid("the Minecraft EULA has to be accepted first"));
        }
        if !crate::util::is_safe_version(&spec.minecraft_version) {
            return Err(Error::invalid("invalid Minecraft version"));
        }
        let name = spec.name.trim();
        if name.is_empty() {
            return Err(Error::invalid("the server needs a name"));
        }

        let existing = self.list().await;
        tokio::fs::create_dir_all(self.paths.servers()).await.ctx(self.paths.servers())?;
        // `create_dir`, not `create_dir_all`: claiming the folder must fail if
        // it exists — one with an unreadable record is invisible to `list` —
        // or the cleanup below would delete somebody's world.
        let (id, dir) = {
            let base = base_id(name);
            let mut n = 1;
            loop {
                let id = if n == 1 { base.clone() } else { format!("{base}-{n}") };
                let dir = self.server_dir(&id);
                match tokio::fs::create_dir(&dir).await {
                    Ok(()) => break (id, dir),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && n < 1000 => n += 1,
                    Err(e) => return Err(e).ctx(&dir),
                }
            }
        };

        let result = self.fill(&id, &dir, name, &spec, &existing).await;
        if result.is_err() {
            let _ = tokio::fs::remove_dir_all(&dir).await;
        }
        result
    }

    async fn fill(
        &self,
        id: &str,
        dir: &Path,
        name: &str,
        spec: &ServerSpec,
        existing: &[HostedServer],
    ) -> Result<Created> {
        // Ports other servers already claim, running or not, so starting two
        // at once never collides. The Mod's converted worlds hold 25566/25576.
        let mut taken: Vec<u16> = existing.iter().flat_map(|s| [s.port, s.rcon.port]).collect();
        taken.extend([25566, 25576]);
        let port = free_port(25565, &taken)?;
        taken.push(port);
        let rcon = Rcon { port: free_port(25575, &taken)?, password: files::random_rcon_password() };

        let mut props: Vec<(String, String)> = [
            // Loopback only: RCON binds to the same address, and friends come
            // in through the tunnel, which dials 127.0.0.1.
            ("server-ip", "127.0.0.1".to_string()),
            ("server-port", port.to_string()),
            ("level-name", "world".into()),
            ("motd", name.to_string()),
            ("online-mode", "true".into()),
            ("enable-rcon", "true".into()),
            ("rcon.port", rcon.port.to_string()),
            ("rcon.password", rcon.password.clone()),
            ("max-players", "20".into()),
            ("view-distance", "10".into()),
            ("simulation-distance", "10".into()),
            // A public address can be found by anyone, and every tunnelled
            // player arrives as 127.0.0.1, so IP bans can't help. The host is
            // whitelisted on first start; friends are added from Players.
            ("white-list", spec.tunnel.to_string()),
            ("enforce-whitelist", spec.tunnel.to_string()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();

        match &spec.world {
            WorldSource::New { seed, level_type, game_mode, difficulty, hardcore } => {
                props.extend(
                    [
                        ("level-seed", seed.trim().to_string()),
                        ("level-type", level_type.clone()),
                        ("gamemode", game_mode.clone()),
                        ("difficulty", difficulty.clone()),
                        ("hardcore", hardcore.to_string()),
                    ]
                    .map(|(k, v)| (k.to_string(), v)),
                );
            }
            WorldSource::Import(save) => {
                crate::paper_server::convert::to_server(save, dir, "world").await?;
            }
        }

        files::write_eula(dir).await?;
        files::write_private(
            &dir.join("server.properties"),
            properties::patch("", &props).into_bytes(),
        )
        .await?;

        self.download_jar(spec.kind, &spec.minecraft_version, &dir.join(JAR)).await?;
        if spec.kind == ServerKind::Paper {
            // Paper bundles spark, whose always-on background profiler loads
            // async-profiler — which aborted the JVM at shutdown on Java 27
            // in every test run (core dump in libasyncProfiler), before the
            // world was certain to be saved. `/spark profiler` still works.
            let spark = dir.join("plugins").join("spark");
            tokio::fs::create_dir_all(&spark).await.ctx(&spark)?;
            let config = spark.join("config.json");
            tokio::fs::write(&config, br#"{"backgroundProfiler": false}"#).await.ctx(&config)?;
        }
        let warnings = self.install_content(spec, dir).await;

        let server = HostedServer {
            id: id.to_string(),
            name: name.to_string(),
            kind: spec.kind,
            minecraft_version: spec.minecraft_version.clone(),
            memory_mb: spec.memory_mb.clamp(512, 65536),
            port,
            rcon,
            tunnel: spec.tunnel,
            pid: None,
            started_at: None,
            created_at: crate::paper_server::registry::now_secs(),
        };
        self.write(&server).await?;
        Ok(Created { server, warnings })
    }

    async fn download_jar(&self, kind: ServerKind, version: &str, dest: &Path) -> Result<()> {
        let task = match kind {
            ServerKind::Paper => {
                let api = paper_api::PaperApi::with_client(self.http.clone());
                let build = api.latest_stable_build(version).await?;
                api.download_task(&build, dest.to_path_buf())?
            }
            ServerKind::Vanilla => {
                let data = self.installer.vanilla_version(version).await?;
                let jar = data.downloads.get("server").ok_or_else(|| {
                    Error::invalid(format!("Mojang publishes no server for Minecraft {version}"))
                })?;
                DownloadTask {
                    url: jar.url.clone(),
                    dest: dest.to_path_buf(),
                    sha1: Some(jar.sha1.clone()),
                    sha256: None,
                    size: jar.size,
                }
            }
            ServerKind::Fabric => {
                let bytes = fabric::server_launcher(&self.http, version).await?;
                return tokio::fs::write(dest, bytes).await.ctx(dest);
            }
        };
        Downloader::new(self.http.clone()).run(vec![task], None).await?;
        Ok(())
    }

    /// Best effort per add-on: one with no build for this version shouldn't
    /// cost the user the whole server.
    async fn install_content(&self, spec: &ServerSpec, dir: &Path) -> Vec<String> {
        let Some((folder, loader, _)) = spec.kind.content() else {
            return Vec::new();
        };
        let mut projects = spec.content.clone();
        // Nearly every Fabric server mod needs it, and forgetting it is the
        // classic first-start crash.
        if spec.kind == ServerKind::Fabric && !projects.iter().any(|p| p == crate::content::FABRIC_API) {
            projects.push(crate::content::FABRIC_API.to_string());
        }
        let folder = dir.join(folder);
        let mut warnings = Vec::new();
        // Canonical Modrinth ids already installed. Picks arrive as slugs
        // (the catalog) or ids (search), so dedupe on what Modrinth resolves.
        let mut installed = std::collections::HashSet::new();
        // Everything attempted, ok or not, so a failing dependency listed by
        // several projects is fetched (and warned about) once.
        let mut tried = std::collections::HashSet::new();
        let mut queue: std::collections::VecDeque<String> = projects.into();
        while let Some(project) = queue.pop_front() {
            if installed.contains(&project) || !tried.insert(project.clone()) {
                continue;
            }
            // Dependencies are other authors' choices; a list hundreds long
            // is either a mistake or an attack, and either way not installed.
            if installed.len() >= MAX_ADDONS {
                warnings.push(format!("stopped after {MAX_ADDONS} add-ons; {project} and the rest were skipped"));
                break;
            }
            // Ids from Modrinth's JSON go into an API path.
            if !is_modrinth_id(&project) {
                warnings.push(format!("skipped an add-on with a malformed id: {project:?}"));
                continue;
            }
            let result: Result<Vec<String>> = async {
                let version = self.modrinth.latest_version(&project, loader, &spec.minecraft_version).await?;
                if !installed.insert(version.project_id.clone()) {
                    return Ok(Vec::new());
                }
                let file = version.primary_file()?;
                if !is_bare_basename(&file.filename) {
                    return Err(Error::invalid(format!("refusing unsafe file name '{}'", file.filename)));
                }
                let bytes = self.modrinth.download(file).await?;
                tokio::fs::create_dir_all(&folder).await.ctx(&folder)?;
                let dest = folder.join(&file.filename);
                if tokio::fs::try_exists(&dest).await.unwrap_or(false) {
                    return Err(Error::invalid(format!("another add-on already uses the file name {}", file.filename)));
                }
                tokio::fs::write(&dest, bytes).await.ctx(&dest)?;
                // What it can't start without — WorldGuard needs WorldEdit,
                // ViaBackwards needs ViaVersion — comes along.
                Ok(version
                    .dependencies
                    .iter()
                    .filter(|d| d.dependency_type == crate::modrinth::DependencyType::Required)
                    .filter_map(|d| d.project_id.clone())
                    .collect())
            }
            .await;
            match result {
                Ok(deps) => queue.extend(deps),
                Err(err) => {
                    tracing::warn!(%project, %err, "could not install server add-on");
                    warnings.push(format!("{project}: {err}"));
                }
            }
        }
        warnings
    }

    fn key(id: &str) -> String {
        format!("host:{id}")
    }

    /// Spawns the server. Returns once the JVM is up; [`Hosting::wait_ready`]
    /// waits for it to accept commands.
    pub async fn start(&self, id: &str) -> Result<HostedServer> {
        let mut server = self.read(id).await?;
        if self.is_running(&server) {
            return Err(Error::invalid(format!("{} is already running", server.name)));
        }
        for port in [server.port, server.rcon.port] {
            if std::net::TcpListener::bind(("127.0.0.1", port)).is_err() {
                return Err(Error::invalid(format!(
                    "port {port} is in use by another program, so the server can't start"
                )));
            }
        }
        let java = crate::java::ensure(&self.http, &self.paths, None, None).await?;
        let dir = self.server_dir(id);
        let console = dir.join(CONSOLE_LOG);
        let out = {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                // Chat and command output, like the rest of this folder's secrets.
                options.mode(0o600);
            }
            options.open(&console).ctx(&console)?
        };
        let err = out.try_clone().ctx(&console)?;

        let mut command = tokio::process::Command::new(&java.path);
        command
            .arg(format!("-Xms{}M", server.memory_mb.min(1024)))
            .arg(format!("-Xmx{}M", server.memory_mb))
            .arg("-jar")
            .arg(JAR)
            .arg("nogui")
            .current_dir(&dir)
            .stdin(std::process::Stdio::null())
            .stdout(out)
            .stderr(err)
            .kill_on_drop(false);
        no_window_async(&mut command);
        let child = command.spawn().ctx(&dir)?;
        server.pid = child.id();
        server.started_at = Some(crate::paper_server::registry::now_secs());
        self.running.register(&Self::key(id), child);
        if let Err(err) = self.write(&server).await {
            // Unrecorded, it couldn't be stopped from the UI.
            self.running.stop(&Self::key(id));
            return Err(err);
        }
        Ok(server)
    }

    /// Waits until the server answers RCON, then makes `op` an operator. A
    /// server that dies or never answers is an error, and one that never
    /// answers is killed, or it would hold its ports for the next attempt.
    pub async fn wait_ready(&self, id: &str, op: Option<String>) -> Result<()> {
        let server = self.read(id).await?;
        let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
        loop {
            if let Ok(mut client) = self.rcon(&server).await {
                if let Some(player) = op.filter(|p| is_player_name(p)) {
                    let _ = client.command(&format!("op {player}")).await;
                    let _ = client.command(&format!("whitelist add {player}")).await;
                }
                return Ok(());
            }
            if !self.is_running(&server) {
                return Err(Error::invalid("the server stopped while starting — the console says why"));
            }
            if tokio::time::Instant::now() >= deadline {
                self.kill(&server);
                return Err(Error::invalid("the server never finished starting, so it was stopped"));
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    pub async fn stop(&self, id: &str) -> Result<()> {
        self.close_tunnel(id);
        let mut server = self.read(id).await?;
        if let Ok(mut client) = self.rcon(&server).await {
            let _ = client.command("stop").await;
        }
        let deadline = tokio::time::Instant::now() + STOP_TIMEOUT;
        while self.is_running(&server) {
            if tokio::time::Instant::now() >= deadline {
                tracing::warn!(id, "server ignored stop; killing it");
                self.kill(&server);
                self.running.wait_for_exit(&Self::key(id)).await;
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        server.pid = None;
        server.started_at = None;
        self.write(&server).await
    }

    fn kill(&self, server: &HostedServer) {
        if !self.running.stop(&Self::key(&server.id))
            && let Some(pid) = server.pid
        {
            let mut system = self.system.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(process) = self.server_process(&mut system, server, pid) {
                process.kill();
            }
        }
    }

    /// Ours from this session, or a Java process still alive under the pid an
    /// earlier session recorded.
    pub fn is_running(&self, server: &HostedServer) -> bool {
        if self.running.is_running(&Self::key(&server.id)) {
            return true;
        }
        let Some(pid) = server.pid else { return false };
        let mut system = self.system.lock().unwrap_or_else(|p| p.into_inner());
        self.server_process(&mut system, server, pid).is_some()
    }

    /// The process under a recorded pid, only if it is still *this* server: a
    /// Java process whose working directory is the server's folder. After a
    /// reboot the pid can belong to anything — the user's own game, which
    /// this launcher starts, is the likeliest Java process to get it — and
    /// that must never be mistaken for the server and killed.
    fn server_process<'s>(
        &self,
        system: &'s mut sysinfo::System,
        server: &HostedServer,
        pid: u32,
    ) -> Option<&'s sysinfo::Process> {
        let pid = sysinfo::Pid::from_u32(pid);
        system.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::Some(&[pid]),
            true,
            sysinfo::ProcessRefreshKind::nothing().with_cwd(sysinfo::UpdateKind::Always),
        );
        let dir = self.server_dir(&server.id);
        let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
        system
            .process(pid)
            .filter(|p| is_java(p) && p.cwd().is_some_and(|cwd| cwd == dir))
    }

    /// CPU and memory for each running server, keyed by id. CPU needs two
    /// readings to mean anything, so the first sample of a process reads 0.
    pub fn sample(&self, servers: &[HostedServer]) -> HashMap<String, Sample> {
        let pids: Vec<(String, sysinfo::Pid)> = servers
            .iter()
            .filter_map(|s| s.pid.map(|p| (s.id.clone(), sysinfo::Pid::from_u32(p))))
            .collect();
        let mut system = self.system.lock().unwrap_or_else(|p| p.into_inner());
        let only: Vec<sysinfo::Pid> = pids.iter().map(|(_, p)| *p).collect();
        system.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::Some(&only),
            true,
            sysinfo::ProcessRefreshKind::nothing().with_cpu().with_memory(),
        );
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get()) as f32;
        pids.into_iter()
            .filter_map(|(id, pid)| {
                let p = system.process(pid)?;
                Some((
                    id,
                    Sample {
                        cpu_percent: (p.cpu_usage() / cores).clamp(0.0, 100.0),
                        memory_mb: (p.memory() / (1024 * 1024)) as u32,
                    },
                ))
            })
            .collect()
    }

    /// Physical memory, for sizing the RAM slider.
    pub fn total_memory_mb(&self) -> u32 {
        let mut system = self.system.lock().unwrap_or_else(|p| p.into_inner());
        system.refresh_memory();
        (system.total_memory() / (1024 * 1024)) as u32
    }

    async fn rcon(&self, server: &HostedServer) -> Result<RconClient> {
        RconClient::connect("127.0.0.1", server.rcon.port, &server.rcon.password, RCON_CALL_TIMEOUT).await
    }

    pub async fn command(&self, id: &str, command: &str) -> Result<String> {
        let server = self.read(id).await?;
        let mut client = self.rcon(&server).await?;
        client.command(command).await.map(|r| strip_formatting(&r))
    }

    pub async fn players(&self, id: &str) -> Result<Players> {
        Ok(parse_list(&self.command(id, "list").await?))
    }

    /// The end of the console log, cut at a line boundary.
    pub async fn console(&self, id: &str) -> Result<String> {
        self.read(id).await?;
        use tokio::io::{AsyncReadExt, AsyncSeekExt};
        let path = self.server_dir(id).join(CONSOLE_LOG);
        let mut file = tokio::fs::File::open(&path).await.ctx(&path)?;
        let len = file.metadata().await.ctx(&path)?.len();
        let start = len.saturating_sub(CONSOLE_TAIL_BYTES);
        file.seek(std::io::SeekFrom::Start(start)).await.ctx(&path)?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf).await.ctx(&path)?;
        let text = String::from_utf8_lossy(&buf);
        Ok(match (start > 0, text.find('\n')) {
            (true, Some(i)) => text[i + 1..].to_string(),
            _ => text.into_owned(),
        })
    }

    pub async fn properties(&self, id: &str) -> Result<Vec<(String, String)>> {
        self.read(id).await?;
        let path = self.server_dir(id).join("server.properties");
        Ok(properties::parse(&tokio::fs::read_to_string(&path).await.ctx(&path)?))
    }

    /// Writes Settings-tab edits. Takes effect on the next start. The RCON
    /// and network keys are this module's to manage, so they're refused.
    pub async fn save_settings(&self, id: &str, updates: Vec<(String, String)>, memory_mb: u32) -> Result<()> {
        const MANAGED: [&str; 6] = ["server-ip", "server-port", "enable-rcon", "rcon.port", "rcon.password", "level-name"];
        let mut server = self.read(id).await?;
        let updates: Vec<_> = updates.into_iter().filter(|(k, _)| !MANAGED.contains(&k.as_str())).collect();
        let path = self.server_dir(id).join("server.properties");
        let text = tokio::fs::read_to_string(&path).await.ctx(&path)?;
        files::write_private(&path, properties::patch(&text, &updates).into_bytes()).await?;
        server.memory_mb = memory_mb.clamp(512, 65536);
        self.write(&server).await
    }

    pub async fn set_tunnel(&self, id: &str, on: bool) -> Result<()> {
        let mut server = self.read(id).await?;
        server.tunnel = on;
        self.write(&server).await
    }

    pub async fn delete(&self, id: &str) -> Result<()> {
        let server = self.read(id).await?;
        if self.is_running(&server) {
            return Err(Error::invalid("stop the server before deleting it"));
        }
        let dir = self.server_dir(id);
        tokio::fs::remove_dir_all(&dir).await.ctx(&dir)
    }

    /// Opens (or reuses) the public address for a running server.
    pub async fn open_tunnel(&self, id: &str) -> Result<String> {
        if let Some(domain) = self.tunnel_domain(id) {
            return Ok(domain);
        }
        let generation = self.generation(id);
        let server = self.read(id).await?;
        let tunnel = tunnel::open(server.port).await?;

        // Opening takes seconds. Only keep the result if nothing closed the
        // tunnel meanwhile, the server still wants one and still runs, and
        // no concurrent open got there first — an untracked tunnel would be a
        // public address nothing can close.
        let current = self.read(id).await.ok();
        let wanted = current.as_ref().is_some_and(|s| s.tunnel && self.is_running(s));
        let mut tunnels = self.tunnels.lock().unwrap_or_else(|p| p.into_inner());
        if !wanted || self.generation(id) != generation {
            tunnel.close();
            return Err(Error::invalid("the public address was closed while it opened"));
        }
        if let Some(existing) = tunnels.get(id).filter(|t| t.is_open()) {
            tunnel.close();
            return Ok(existing.domain.clone());
        }
        let domain = tunnel.domain.clone();
        tunnels.insert(id.to_string(), tunnel);
        Ok(domain)
    }

    fn generation(&self, id: &str) -> u64 {
        *self.tunnel_generation.lock().unwrap_or_else(|p| p.into_inner()).get(id).unwrap_or(&0)
    }

    pub fn close_tunnel(&self, id: &str) {
        *self
            .tunnel_generation
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(id.to_string())
            .or_default() += 1;
        if let Some(t) = self.tunnels.lock().unwrap_or_else(|p| p.into_inner()).remove(id) {
            t.close();
        }
    }

    /// The live public address, if the tunnel is up. A tunnel the relay
    /// dropped is forgotten here, so the UI can offer to reopen it.
    pub fn tunnel_domain(&self, id: &str) -> Option<String> {
        let mut tunnels = self.tunnels.lock().unwrap_or_else(|p| p.into_inner());
        match tunnels.get(id) {
            Some(t) if t.is_open() => Some(t.domain.clone()),
            Some(_) => {
                tunnels.remove(id);
                None
            }
            None => None,
        }
    }
}

fn is_java(process: &sysinfo::Process) -> bool {
    process.name().to_string_lossy().to_ascii_lowercase().contains("java")
}

/// What an RCON argument may be: a Minecraft username, so nothing that could
/// smuggle a second command in.
pub fn is_player_name(name: &str) -> bool {
    (1..=16).contains(&name.len()) && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn base_id(name: &str) -> String {
    match slugify(name) {
        s if s.is_empty() => "server".to_string(),
        s => s.chars().take(48).collect(),
    }
}

fn free_port(from: u16, taken: &[u16]) -> Result<u16> {
    (from..from.saturating_add(200))
        .find(|p| !taken.contains(p) && std::net::TcpListener::bind(("127.0.0.1", *p)).is_ok())
        .ok_or_else(|| Error::invalid("no free port for the server"))
}

/// Drops `§x` colour codes, which Paper leaves in RCON replies.
fn strip_formatting(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '§' {
            chars.next();
        } else {
            out.push(c);
        }
    }
    out
}

/// Parses `list`. Vanilla says "There are 1 of a max of 20 players online:
/// Steve"; Paper says "There are 1 out of maximum 20 players online." with
/// the names on the next line. Both: the first two numbers, then whatever
/// follows the first colon.
fn parse_list(reply: &str) -> Players {
    let (head, names) = reply.split_once(':').unwrap_or((reply, ""));
    let mut numbers = head
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|n| n.parse::<u32>().ok());
    Players {
        online: numbers.next().unwrap_or(0),
        max: numbers.next().unwrap_or(0),
        names: names
            .split([',', '\n'])
            .map(|n| n.trim().rsplit(' ').next().unwrap_or("").to_string())
            .filter(|n| is_player_name(n))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_vanilla_and_paper_list_replies() {
        let v = parse_list("There are 2 of a max of 20 players online: Steve, Alex_2");
        assert_eq!((v.online, v.max), (2, 20));
        assert_eq!(v.names, ["Steve", "Alex_2"]);

        let p = parse_list(&strip_formatting(
            "§6There are §c1§6 out of maximum §c8§6 players online.\n§6default§r: §fLokifisch",
        ));
        assert_eq!((p.online, p.max), (1, 8));
        assert_eq!(p.names, ["Lokifisch"]);

        let empty = parse_list("There are 0 of a max of 20 players online: ");
        assert!(empty.names.is_empty());
    }

    #[test]
    fn modrinth_ids_cannot_reshape_a_url() {
        assert!(is_modrinth_id("viabackwards"));
        assert!(is_modrinth_id("P7dR8mSH"));
        assert!(!is_modrinth_id("../../v3/x"));
        assert!(!is_modrinth_id("x?loaders=y"));
        assert!(!is_modrinth_id(""));
    }

    #[test]
    fn player_names_cannot_carry_a_second_command() {
        assert!(is_player_name("Notch_99"));
        assert!(!is_player_name("x\nstop"));
        assert!(!is_player_name("a b"));
        assert!(!is_player_name(""));
    }
}
