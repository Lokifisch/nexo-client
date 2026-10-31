//! Server hosting: a five-step wizard that makes a Paper, vanilla or Fabric
//! server from scratch, a setup screen that watches it come up, and a
//! dashboard for running it (performance graph, console, players, settings).
//! The engine is `nexo_core::hosting`; this is state and messages.
//!
//! Kept out of `main.rs` behind one `Message::Host` variant. The views live in
//! `server_wizard` and `server_dashboard`; the server list (shared with the
//! Mod's converted worlds) is [`list_view`].

use crate::{App, Message, Screen, Status, empty_state, theme};
use crate::lively::button;
use crate::lively::scrollable;
use crate::lively::Tint;
use iced::widget::{Space, column, container, row, text};
use iced::{Element, Fill, Task};
use nexo_core::hosting::{Created, HostedServer, Players, Sample, ServerKind, ServerSpec, WorldSource};
use nexo_core::modrinth::SearchHit;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::time::Instant;

pub const GAME_MODES: [&str; 3] = ["Survival", "Creative", "Adventure"];
pub const DIFFICULTIES: [&str; 4] = ["Peaceful", "Easy", "Normal", "Hard"];
pub const WORLD_TYPES: [&str; 4] = ["Default", "Flat", "Large Biomes", "Amplified"];
/// One add-on in the curated list on the add-ons step.
pub struct Essential {
    /// Modrinth slug.
    pub slug: &'static str,
    pub name: &'static str,
    pub why: &'static str,
    pub tag: &'static str,
    /// Checked from the start.
    pub default: bool,
}

const fn e(slug: &'static str, name: &'static str, tag: &'static str, why: &'static str, default: bool) -> Essential {
    Essential { slug, name, why, tag, default }
}

/// Picked by hand and checked on Modrinth (2026-10-28) for a `paper` build.
/// Left out on purpose: Geyser and Simple Voice Chat — both need UDP, and
/// e4mc's relay carries TCP only, so neither works for friends joining
/// through the public address.
pub const PAPER_ESSENTIALS: [Essential; 8] = [
    e("viaversion", "ViaVersion", "Compatibility", "Lets players on newer Minecraft versions join.", true),
    e("viabackwards", "ViaBackwards", "Compatibility", "Lets players on older versions join too.", true),
    e("luckperms", "LuckPerms", "Admin", "Permissions and ranks — who may use which command.", false),
    e("essentialsx", "EssentialsX", "Admin", "/home, /spawn, /tpa, kits, warps and the rest of the basics.", false),
    e("coreprotect", "CoreProtect", "Admin", "Logs every block change; roll back griefing in one command.", false),
    e("worldedit", "WorldEdit", "Building", "Select, copy, paste and reshape terrain in bulk.", false),
    e("worldguard", "WorldGuard", "Building", "Protect regions — spawn, builds — from other players.", false),
    e("chunky", "Chunky", "Performance", "Pre-generates the world so exploring doesn't lag.", false),
];

/// The same for Fabric. Fabric API is added regardless (see core).
pub const FABRIC_ESSENTIALS: [Essential; 9] = [
    e("viafabric", "ViaFabric", "Compatibility", "Lets players on newer Minecraft versions join.", true),
    e("viabackwards", "ViaBackwards", "Compatibility", "Lets players on older versions join too.", true),
    e("lithium", "Lithium", "Performance", "Faster game logic with identical behaviour.", true),
    e("ferrite-core", "FerriteCore", "Performance", "Cuts the server's memory use.", true),
    e("krypton", "Krypton", "Performance", "Lighter networking stack.", false),
    e("c2me-fabric", "C2ME", "Performance", "Generates and loads chunks on every core.", false),
    e("chunky", "Chunky", "Performance", "Pre-generates the world so exploring doesn't lag.", false),
    e("luckperms", "LuckPerms", "Admin", "Permissions and ranks — who may use which command.", false),
    e("styled-chat", "Styled Chat", "Social", "Chat formatting and colours.", false),
];

pub fn essentials(kind: ServerKind) -> &'static [Essential] {
    match kind {
        ServerKind::Paper => &PAPER_ESSENTIALS,
        ServerKind::Fabric => &FABRIC_ESSENTIALS,
        ServerKind::Vanilla => &[],
    }
}

fn defaults(kind: ServerKind) -> Vec<&'static str> {
    essentials(kind).iter().filter(|e| e.default).map(|e| e.slug).collect()
}

/// An essential's version number for this Minecraft version (`None`: no
/// build), and its project's downloads and icon URL when those could be read.
/// `Err` is "couldn't ask", which must not read as "doesn't exist".
pub type ProbeResult = Result<(Option<String>, Option<(u64, Option<String>)>), String>;

/// Whether an essential has a build for the chosen version and loader.
#[derive(Debug, Clone)]
pub enum Probe {
    Checking,
    Ready { version: String, downloads: u64 },
    Missing,
    /// Modrinth couldn't be reached; asked again on the next visit.
    Failed,
}

/// An hour of graph at one sample every two seconds.
const HISTORY: usize = 1800;

fn level_type(label: &str) -> &'static str {
    match label {
        "Flat" => "minecraft:flat",
        "Large Biomes" => "minecraft:large_biomes",
        "Amplified" => "minecraft:amplified",
        _ => "minecraft:normal",
    }
}

/// `"survival"` → `"Survival"`, for showing a property value in a picker.
pub fn label_of(options: &[&'static str], value: &str) -> Option<&'static str> {
    options.iter().copied().find(|o| o.eq_ignore_ascii_case(value))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    #[default]
    Offline,
    Starting,
    Online,
    Stopping,
}

/// What this session knows about one server beyond its record.
#[derive(Default)]
pub struct Live {
    pub phase: Phase,
    pub samples: VecDeque<(Instant, Sample)>,
    pub domain: Option<String>,
    pub tunnel_error: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Step {
    #[default]
    Type,
    Content,
    World,
    Performance,
    Review,
}

impl Step {
    pub const ALL: [Step; 5] = [Step::Type, Step::Content, Step::World, Step::Performance, Step::Review];

    pub fn label(self) -> &'static str {
        match self {
            Step::Type => "Server Type",
            Step::Content => "Add-ons",
            Step::World => "World",
            Step::Performance => "Performance",
            Step::Review => "Review",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }
}

/// A singleplayer world that can be copied into a new server.
#[derive(Debug, Clone, PartialEq)]
pub struct WorldChoice {
    pub label: String,
    pub path: PathBuf,
}

impl std::fmt::Display for WorldChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

pub struct Wizard {
    pub step: Step,
    pub kind: ServerKind,
    pub version: String,
    pub query: String,
    pub results: Vec<SearchHit>,
    pub searching: bool,
    pub search_error: Option<String>,
    /// (project id, title), in the order picked.
    pub picked: Vec<(String, String)>,
    /// Essentials (by slug) that will be installed.
    pub chosen: Vec<&'static str>,
    pub probes: HashMap<&'static str, Probe>,
    /// What `probes` were checked against; a change of either re-checks.
    probed_for: Option<(ServerKind, String)>,
    pub import: bool,
    pub worlds: Vec<WorldChoice>,
    pub import_world: Option<WorldChoice>,
    pub seed: String,
    pub world_type: &'static str,
    pub game_mode: &'static str,
    pub difficulty: &'static str,
    pub hardcore: bool,
    pub name: String,
    pub memory_mb: u32,
    pub tunnel: bool,
    pub eula: bool,
}

impl Wizard {
    /// The build an essential will get — only if it was checked against the
    /// type and version chosen now, not ones the user has since moved off.
    pub fn ready_version(&self, slug: &str) -> Option<&str> {
        if self.probed_for.as_ref() != Some(&(self.kind, self.version.clone())) {
            return None;
        }
        match self.probes.get(slug) {
            Some(Probe::Ready { version, .. }) => Some(version),
            _ => None,
        }
    }

    fn new(version: String, count: usize) -> Self {
        Self {
            step: Step::Type,
            kind: ServerKind::Paper,
            version,
            query: String::new(),
            results: Vec::new(),
            searching: false,
            search_error: None,
            picked: Vec::new(),
            chosen: defaults(ServerKind::Paper),
            probes: HashMap::new(),
            probed_for: None,
            import: false,
            worlds: Vec::new(),
            import_world: None,
            seed: String::new(),
            world_type: WORLD_TYPES[0],
            game_mode: GAME_MODES[0],
            difficulty: DIFFICULTIES[2],
            hardcore: false,
            name: format!("Nexo Server #{:02}", count + 1),
            memory_mb: 4096,
            tunnel: true,
            eula: false,
        }
    }
}

/// The wizard's last screen: the new server coming up.
pub struct Setup {
    /// Set once the files exist.
    pub id: Option<String>,
    pub error: Option<String>,
    pub warnings: Vec<String>,
    pub tunnel: bool,
    pub show_console: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Overview,
    Console,
    Players,
    Settings,
    Files,
}

impl Tab {
    pub const ALL: [Tab; 5] = [Tab::Overview, Tab::Console, Tab::Players, Tab::Settings, Tab::Files];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Overview => "Performance",
            Tab::Console => "Console",
            Tab::Players => "Players",
            Tab::Settings => "Settings",
            Tab::Files => "Files",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Range {
    #[default]
    OneMin,
    FiveMin,
    HalfHour,
    Hour,
}

impl Range {
    pub const ALL: [Range; 4] = [Range::OneMin, Range::FiveMin, Range::HalfHour, Range::Hour];

    pub fn secs(self) -> f32 {
        match self {
            Range::OneMin => 60.0,
            Range::FiveMin => 300.0,
            Range::HalfHour => 1800.0,
            Range::Hour => 3600.0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Range::OneMin => "1m",
            Range::FiveMin => "5m",
            Range::HalfHour => "30m",
            Range::Hour => "1h",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Filter {
    #[default]
    All,
    Errors,
    Warnings,
    Info,
}

/// The Settings tab's form: `server.properties` values as read, then edited.
#[derive(Default)]
pub struct Settings {
    pub values: HashMap<String, String>,
    pub memory_mb: u32,
    pub dirty: bool,
}

impl Settings {
    pub fn get(&self, key: &str) -> &str {
        self.values.get(key).map(String::as_str).unwrap_or("")
    }

    pub fn flag(&self, key: &str) -> bool {
        self.get(key) == "true"
    }
}

/// Keys the Settings tab edits — and so the only ones it writes back.
pub const SETTINGS_KEYS: [&str; 11] = [
    "gamemode",
    "force-gamemode",
    "difficulty",
    "pvp",
    "allow-flight",
    "white-list",
    "motd",
    "max-players",
    "view-distance",
    "simulation-distance",
    "spawn-protection",
];

#[derive(Default)]
pub struct HostState {
    pub servers: Vec<HostedServer>,
    pub live: HashMap<String, Live>,
    pub wizard: Option<Wizard>,
    pub setup: Option<Setup>,
    pub tab: Tab,
    pub range: Range,
    pub console: Option<String>,
    pub filter: Filter,
    pub command: String,
    pub reply: Option<String>,
    pub players: Option<Players>,
    pub player_input: String,
    pub settings: Option<Settings>,
    pub total_memory_mb: u32,
    pub delete_armed: Option<String>,
    restarting: HashSet<String>,
}

impl HostState {
    pub fn server(&self, id: &str) -> Option<&HostedServer> {
        self.servers.iter().find(|s| s.id == id)
    }

    pub fn phase(&self, id: &str) -> Phase {
        self.live.get(id).map(|l| l.phase).unwrap_or_default()
    }

    /// The address friends use: the public one when it's up, else this PC's.
    pub fn address(&self, server: &HostedServer) -> String {
        self.live
            .get(&server.id)
            .and_then(|l| l.domain.clone())
            .unwrap_or_else(|| format!("localhost:{}", server.port))
    }
}

#[derive(Clone)]
pub enum Msg {
    Load,
    Loaded(Vec<(HostedServer, bool)>),

    OpenWizard,
    WorldsFound(Vec<WorldChoice>),
    MemoryFound(u32),
    GoTo(Step),
    Next,
    Back,
    Kind(ServerKind),
    Version(String),
    Query(String),
    Search,
    Results(Result<Vec<SearchHit>, String>),
    TogglePick(String, String),
    ToggleEssential(&'static str),
    RecommendedEssentials,
    Probed(ServerKind, String, &'static str, ProbeResult),
    Import(bool),
    ImportWorld(WorldChoice),
    Seed(String),
    WorldType(&'static str),
    GameMode(&'static str),
    Difficulty(&'static str),
    Hardcore(bool),
    Name(String),
    Memory(u32),
    Tunnel(bool),
    Eula(bool),
    OpenEula,
    Create,
    Created(Result<Created, String>),
    ToggleSetupConsole,
    LeaveSetup(Option<String>),

    Start(String),
    Started(String, Result<HostedServer, String>),
    Ready(String, Result<(), String>),
    TunnelOpened(String, Result<String, String>),
    SetTunnel(String, bool),
    Stop(String),
    Restart(String),
    Stopped(String, Result<(), String>),
    Sample,
    Sampled(HashMap<String, Sample>, HashSet<String>, HashMap<String, Option<String>>),

    Tab(Tab),
    Range(Range),
    Filter(Filter),
    FollowConsole,
    ConsoleRead(String, Result<String, String>),
    Command(String),
    SendCommand,
    CommandSent(Result<String, String>),
    RefreshPlayers,
    PlayersRead(String, Result<Players, String>),
    PlayerInput(String),
    /// An RCON verb (`op`, `kick`, …) and the player it applies to.
    PlayerAction(&'static str, String),
    SettingsRead(String, Result<(Vec<(String, String)>, u32), String>),
    SetProp(&'static str, String),
    SettingsMemory(u32),
    SaveSettings,
    SettingsSaved(Result<(), String>),
    Copy(String),
    Delete(String),
    Deleted(Result<(), String>),
}

fn host(msg: Msg) -> Message {
    Message::Host(msg)
}

/// Runs one core call and lands its result on `done`.
fn call<T, Fut>(
    app: &App,
    work: impl FnOnce(nexo_core::Nexo) -> Fut,
    done: impl Fn(Result<T, String>) -> Msg + Send + 'static,
) -> Task<Message>
where
    T: Send + 'static,
    Fut: std::future::Future<Output = Result<T, String>> + Send + 'static,
{
    let Some(core) = app.core.clone() else {
        return Task::none();
    };
    Task::perform(work(core), move |r| host(done(r)))
}

/// Whatever opening `screen` should load.
pub fn on_navigate(app: &mut App, screen: &Screen) -> Task<Message> {
    match screen {
        Screen::Servers => Task::done(host(Msg::Load)),
        Screen::NewServer if app.host.setup.is_none() => Task::done(host(Msg::OpenWizard)),
        Screen::Server(_) => {
            let h = &mut app.host;
            h.tab = Tab::Overview;
            h.console = None;
            h.reply = None;
            h.command.clear();
            h.players = None;
            h.settings = None;
            h.delete_armed = None;
            Task::batch([Task::done(host(Msg::RefreshPlayers)), Task::done(host(Msg::Load))])
        }
        _ => Task::none(),
    }
}

pub fn update(app: &mut App, msg: Msg) -> Task<Message> {
    match msg {
        Msg::Load => call(
            app,
            |core| async move {
                let servers = core.hosting.list().await;
                Ok(servers
                    .into_iter()
                    .map(|s| {
                        let running = core.hosting.is_running(&s);
                        (s, running)
                    })
                    .collect())
            },
            |r| Msg::Loaded(r.unwrap_or_default()),
        ),
        Msg::Loaded(list) => {
            let mut tasks = Vec::new();
            for (server, running) in &list {
                let live = app.host.live.entry(server.id.clone()).or_default();
                // Only Offline is overridden: a start or stop in flight owns
                // the phase until it reports back.
                if *running && live.phase == Phase::Offline {
                    // Left running by an earlier session: its tunnel died with
                    // that session, so reopen it.
                    live.phase = Phase::Online;
                    if server.tunnel {
                        tasks.push(open_tunnel(app, server.id.clone()));
                    }
                }
            }
            app.host.servers = list.into_iter().map(|(s, _)| s).collect();
            Task::batch(tasks)
        }

        Msg::OpenWizard => {
            let version = app
                .game_versions
                .first()
                .cloned()
                .unwrap_or_else(|| crate::DEFAULT_GAME_VERSION.to_string());
            app.host.wizard = Some(Wizard::new(version, app.host.servers.len()));
            let instances = app.instances.clone();
            Task::batch([
                call(
                    app,
                    move |core| async move {
                        let mut found = Vec::new();
                        for instance in instances {
                            for world in nexo_core::browse::worlds(&core.paths.instance(&instance.id)).await {
                                found.push(WorldChoice {
                                    label: format!("{} — {}", instance.name, world.name),
                                    path: world.path,
                                });
                            }
                        }
                        Ok(found)
                    },
                    |r| Msg::WorldsFound(r.unwrap_or_default()),
                ),
                call(app, |core| async move { Ok(core.hosting.total_memory_mb()) }, |r| {
                    Msg::MemoryFound(r.unwrap_or(0))
                }),
            ])
        }
        Msg::WorldsFound(worlds) => {
            if let Some(w) = &mut app.host.wizard {
                w.import_world = worlds.first().cloned();
                w.worlds = worlds;
            }
            Task::none()
        }
        Msg::MemoryFound(mb) => {
            app.host.total_memory_mb = mb;
            if let Some(w) = &mut app.host.wizard
                && mb > 0
            {
                // The low end of what the chosen type likes, but never more
                // than half the machine — the game runs beside it.
                let (low, _) = ram_band(w.kind);
                w.memory_mb = low.min(mb / 2).max(1024) / 256 * 256;
            }
            Task::none()
        }
        Msg::GoTo(step) => {
            wizard(app, |w| w.step = step);
            then_search(app)
        }
        Msg::Next => {
            wizard(app, |w| {
                w.step = Step::ALL[(w.step.index() + 1).min(Step::ALL.len() - 1)];
            });
            then_search(app)
        }
        Msg::Back => match app.host.wizard.as_ref().map(|w| w.step) {
            Some(Step::Type) | None => Task::done(Message::Navigate(Screen::Servers)),
            Some(step) => {
                wizard(app, |w| w.step = Step::ALL[step.index() - 1]);
                Task::none()
            }
        },
        Msg::Kind(kind) => {
            wizard(app, |w| {
                if w.kind != kind {
                    // Plugins and mods aren't interchangeable.
                    w.picked.clear();
                    w.results.clear();
                    w.chosen = defaults(kind);
                    w.probed_for = None;
                    w.probes.clear();
                }
                w.kind = kind;
            });
            Task::none()
        }
        Msg::Version(v) => {
            wizard(app, |w| {
                w.version = v;
                w.results.clear();
            });
            Task::none()
        }
        Msg::Query(q) => {
            wizard(app, |w| w.query = q);
            Task::none()
        }
        Msg::Search => {
            let Some(w) = &mut app.host.wizard else { return Task::none() };
            let Some((_, loader, kind)) = w.kind.content() else { return Task::none() };
            w.searching = true;
            w.search_error = None;
            let (text, version) = (w.query.clone(), w.version.clone());
            call(
                app,
                move |core| async move {
                    let query = nexo_core::modrinth::SearchQuery {
                        text: &text,
                        loader: Some(loader),
                        game_version: Some(&version),
                        project_type: kind,
                        sort: if text.trim().is_empty() {
                            nexo_core::modrinth::SortIndex::Downloads
                        } else {
                            nexo_core::modrinth::SortIndex::Relevance
                        },
                        limit: 30,
                        offset: 0,
                    };
                    core.hosting.modrinth().search(&query).await.map(|r| r.hits).map_err(|e| e.to_string())
                },
                Msg::Results,
            )
        }
        Msg::Results(result) => {
            let Some(w) = &mut app.host.wizard else { return Task::none() };
            w.searching = false;
            match result {
                Ok(hits) => {
                    let icons = crate::fetch_icons(app.core.clone(), &hits, &app.icons);
                    if let Some(w) = &mut app.host.wizard {
                        w.results = hits;
                    }
                    icons
                }
                Err(err) => {
                    w.search_error = Some(err);
                    Task::none()
                }
            }
        }
        Msg::ToggleEssential(slug) => set(app, |w| match w.chosen.iter().position(|s| *s == slug) {
            Some(i) => {
                w.chosen.remove(i);
            }
            None => w.chosen.push(slug),
        }),
        Msg::RecommendedEssentials => set(app, |w| {
            w.chosen = defaults(w.kind)
                .into_iter()
                .filter(|s| !matches!(w.probes.get(s), Some(Probe::Missing)))
                .collect();
        }),
        Msg::Probed(kind, version, slug, result) => {
            let Some(w) = &mut app.host.wizard else { return Task::none() };
            // A slower answer for a type or version the user has moved off.
            if w.probed_for.as_ref() != Some(&(kind, version)) {
                return Task::none();
            }
            match result {
                Ok((found, project)) => {
                    let (downloads, icon) = project.unwrap_or((0, None));
                    match found {
                        Some(version) => {
                            w.probes.insert(slug, Probe::Ready { version, downloads });
                        }
                        None => {
                            w.probes.insert(slug, Probe::Missing);
                            // Nothing to install, so don't promise it on the review.
                            w.chosen.retain(|s| *s != slug);
                        }
                    }
                    if let (Some(url), Some(core)) = (icon, app.core.clone())
                        && !app.icons.contains_key(slug)
                    {
                        return Task::perform(
                            async move { nexo_core::skin::fetch_texture(core.http(), &url).await.ok() },
                            move |icon| match icon {
                                Some(icon) => Message::IconLoaded {
                                    project: slug.to_string(),
                                    handle: iced::widget::image::Handle::from_rgba(icon.width, icon.height, icon.pixels),
                                },
                                None => Message::Noop,
                            },
                        );
                    }
                }
                // Stays chosen and clickable; re-asked on the next visit.
                Err(_) => {
                    w.probes.insert(slug, Probe::Failed);
                }
            }
            Task::none()
        }
        Msg::TogglePick(id, title) => {
            wizard(app, |w| match w.picked.iter().position(|(p, _)| *p == id) {
                Some(i) => {
                    w.picked.remove(i);
                }
                None => w.picked.push((id, title)),
            });
            Task::none()
        }
        Msg::Import(on) => set(app, |w| w.import = on),
        Msg::ImportWorld(world) => set(app, |w| w.import_world = Some(world)),
        Msg::Seed(s) => set(app, |w| w.seed = s),
        Msg::WorldType(t) => set(app, |w| w.world_type = t),
        Msg::GameMode(m) => set(app, |w| w.game_mode = m),
        Msg::Difficulty(d) => set(app, |w| w.difficulty = d),
        Msg::Hardcore(on) => set(app, |w| w.hardcore = on),
        Msg::Name(n) => set(app, |w| w.name = n),
        Msg::Memory(mb) => set(app, |w| w.memory_mb = mb),
        Msg::Tunnel(on) => set(app, |w| w.tunnel = on),
        Msg::Eula(on) => set(app, |w| w.eula = on),
        Msg::OpenEula => {
            let _ = open::that_detached("https://aka.ms/MinecraftEULA");
            Task::none()
        }
        Msg::Create => {
            let Some(w) = &app.host.wizard else { return Task::none() };
            if !w.eula || w.name.trim().is_empty() {
                return Task::none();
            }
            let world = match (w.import, &w.import_world) {
                (true, Some(choice)) => WorldSource::Import(choice.path.clone()),
                (true, None) => return Task::none(),
                (false, _) => WorldSource::New {
                    seed: w.seed.clone(),
                    level_type: level_type(w.world_type).to_string(),
                    game_mode: w.game_mode.to_lowercase(),
                    difficulty: w.difficulty.to_lowercase(),
                    hardcore: w.hardcore,
                },
            };
            let spec = ServerSpec {
                name: w.name.clone(),
                kind: w.kind,
                minecraft_version: w.version.clone(),
                world,
                memory_mb: w.memory_mb,
                tunnel: w.tunnel,
                content: if w.kind.content().is_some() {
                    w.chosen
                        .iter()
                        .filter(|s| w.ready_version(s).is_some())
                        .map(|s| s.to_string())
                        .chain(w.picked.iter().map(|(id, _)| id.clone()))
                        .collect()
                } else {
                    Vec::new()
                },
                eula_accepted: w.eula,
            };
            app.host.setup = Some(Setup {
                id: None,
                error: None,
                warnings: Vec::new(),
                tunnel: w.tunnel,
                show_console: true,
            });
            call(
                app,
                move |core| async move { core.hosting.create(spec).await.map_err(|e| e.to_string()) },
                Msg::Created,
            )
        }
        Msg::Created(result) => {
            let Some(setup) = &mut app.host.setup else { return Task::none() };
            match result {
                Ok(created) => {
                    let id = created.server.id.clone();
                    setup.id = Some(id.clone());
                    setup.warnings = created.warnings;
                    app.host.servers.insert(0, created.server);
                    app.host.live.insert(id.clone(), Live::default());
                    Task::done(host(Msg::Start(id)))
                }
                Err(err) => {
                    setup.error = Some(err);
                    Task::none()
                }
            }
        }
        Msg::ToggleSetupConsole => {
            if let Some(s) = &mut app.host.setup {
                s.show_console = !s.show_console;
            }
            Task::none()
        }
        Msg::LeaveSetup(open) => {
            let failed_early = app.host.setup.as_ref().is_some_and(|s| s.id.is_none());
            app.host.setup = None;
            match open {
                Some(id) => {
                    app.host.wizard = None;
                    Task::done(Message::Navigate(Screen::Server(id)))
                }
                // Back to the wizard, which still holds everything typed.
                None if failed_early => Task::none(),
                None => Task::done(Message::Navigate(Screen::Servers)),
            }
        }

        Msg::Start(id) => {
            let live = app.host.live.entry(id.clone()).or_default();
            live.phase = Phase::Starting;
            live.error = None;
            live.tunnel_error = None;
            call(
                app,
                {
                    let id = id.clone();
                    move |core| async move { core.hosting.start(&id).await.map_err(|e| e.to_string()) }
                },
                move |r| Msg::Started(id.clone(), r),
            )
        }
        Msg::Started(id, result) => match result {
            Ok(server) => {
                replace(app, server);
                // The player starting it gets operator, so /gamemode and
                // friends work for them on day one.
                let op = app.active_account().map(|a| a.username.clone());
                call(
                    app,
                    {
                        let id = id.clone();
                        move |core| async move { core.hosting.wait_ready(&id, op).await.map_err(|e| e.to_string()) }
                    },
                    move |r| Msg::Ready(id.clone(), r),
                )
            }
            Err(err) => failed(app, &id, err),
        },
        Msg::Ready(id, result) => match result {
            Ok(()) => {
                app.host.live.entry(id.clone()).or_default().phase = Phase::Online;
                let tunnel = app.host.server(&id).is_some_and(|s| s.tunnel);
                let mut tasks = vec![Task::done(host(Msg::RefreshPlayers))];
                if tunnel {
                    tasks.push(open_tunnel(app, id));
                }
                Task::batch(tasks)
            }
            Err(err) => failed(app, &id, err),
        },
        Msg::TunnelOpened(id, result) => {
            let live = app.host.live.entry(id.clone()).or_default();
            if live.phase != Phase::Online {
                // Stopped while it opened; the core already refused to keep it.
                if let Some(core) = &app.core {
                    core.hosting.close_tunnel(&id);
                }
                return Task::none();
            }
            match result {
                Ok(domain) => {
                    live.domain = Some(domain);
                    live.tunnel_error = None;
                }
                Err(err) => live.tunnel_error = Some(err),
            }
            Task::none()
        }
        Msg::SetTunnel(id, on) => {
            if let Some(s) = app.host.servers.iter_mut().find(|s| s.id == id) {
                s.tunnel = on;
            }
            let online = app.host.phase(&id) == Phase::Online;
            let persist = call(
                app,
                {
                    let id = id.clone();
                    move |core| async move {
                        if !on {
                            core.hosting.close_tunnel(&id);
                        }
                        core.hosting.set_tunnel(&id, on).await.map_err(|e| e.to_string())
                    }
                },
                |_| Msg::Load,
            );
            let live = app.host.live.entry(id.clone()).or_default();
            if !on {
                live.domain = None;
                live.tunnel_error = None;
                return persist;
            }
            if online {
                return Task::batch([persist, open_tunnel(app, id)]);
            }
            persist
        }
        Msg::Stop(id) => {
            app.host.live.entry(id.clone()).or_default().phase = Phase::Stopping;
            call(
                app,
                {
                    let id = id.clone();
                    move |core| async move { core.hosting.stop(&id).await.map_err(|e| e.to_string()) }
                },
                move |r| Msg::Stopped(id.clone(), r),
            )
        }
        Msg::Restart(id) => {
            app.host.restarting.insert(id.clone());
            Task::done(host(Msg::Stop(id)))
        }
        Msg::Stopped(id, result) => {
            let live = app.host.live.entry(id.clone()).or_default();
            live.phase = Phase::Offline;
            live.domain = None;
            if let Err(err) = result {
                live.error = Some(err);
            }
            app.host.players = None;
            let restart = app.host.restarting.remove(&id);
            Task::batch([
                Task::done(host(Msg::Load)),
                if restart { Task::done(host(Msg::Start(id))) } else { Task::none() },
            ])
        }
        Msg::Sample => {
            let servers: Vec<HostedServer> = app
                .host
                .servers
                .iter()
                .filter(|s| app.host.phase(&s.id) != Phase::Offline)
                .cloned()
                .collect();
            if servers.is_empty() {
                return Task::none();
            }
            call(
                app,
                move |core| async move {
                    let samples = core.hosting.sample(&servers);
                    let running = servers
                        .iter()
                        .filter(|s| core.hosting.is_running(s))
                        .map(|s| s.id.clone())
                        .collect();
                    let domains = servers
                        .iter()
                        .map(|s| (s.id.clone(), core.hosting.tunnel_domain(&s.id)))
                        .collect();
                    Ok((samples, running, domains))
                },
                |r| match r {
                    Ok((s, r, d)) => Msg::Sampled(s, r, d),
                    Err(_) => Msg::Sampled(HashMap::new(), HashSet::new(), HashMap::new()),
                },
            )
        }
        Msg::Sampled(samples, running, domains) => {
            let now = Instant::now();
            let mut tasks = Vec::new();
            for (id, live) in app.host.live.iter_mut() {
                if let Some(sample) = samples.get(id) {
                    live.samples.push_back((now, *sample));
                    while live.samples.len() > HISTORY {
                        live.samples.pop_front();
                    }
                }
                if live.phase == Phase::Online && !running.contains(id) {
                    // Exited on its own: a `stop` typed in the console, or a crash.
                    live.phase = Phase::Offline;
                    live.domain = None;
                    let id = id.clone();
                    if let Some(core) = app.core.clone() {
                        core.hosting.close_tunnel(&id);
                    }
                    tasks.push(Task::done(host(Msg::Load)));
                }
                if live.phase == Phase::Online
                    && let Some(domain) = domains.get(id)
                    && live.domain.is_some()
                    && domain.is_none()
                {
                    live.domain = None;
                    live.tunnel_error = Some("The relay dropped the connection.".into());
                }
            }
            Task::batch(tasks)
        }

        Msg::Tab(tab) => {
            app.host.tab = tab;
            app.host.delete_armed = None;
            let Screen::Server(id) = &app.screen else { return Task::none() };
            let id = id.clone();
            match tab {
                Tab::Console => Task::done(host(Msg::FollowConsole)),
                Tab::Settings => call(
                    app,
                    {
                        let id = id.clone();
                        move |core| async move {
                            let props = core.hosting.properties(&id).await.map_err(|e| e.to_string())?;
                            let memory = core.hosting.read(&id).await.map_err(|e| e.to_string())?.memory_mb;
                            Ok((props, memory))
                        }
                    },
                    move |r| Msg::SettingsRead(id.clone(), r),
                )
                .chain(call(app, |core| async move { Ok(core.hosting.total_memory_mb()) }, |r| {
                    Msg::MemoryFound(r.unwrap_or(0))
                })),
                _ => Task::none(),
            }
        }
        Msg::Range(r) => {
            app.host.range = r;
            Task::none()
        }
        Msg::Filter(f) => {
            app.host.filter = f;
            Task::none()
        }
        Msg::FollowConsole => {
            let Some(id) = console_target(app) else { return Task::none() };
            call(
                app,
                {
                    let id = id.clone();
                    move |core| async move { core.hosting.console(&id).await.map_err(|e| e.to_string()) }
                },
                move |r| Msg::ConsoleRead(id.clone(), r),
            )
        }
        Msg::ConsoleRead(id, result) => {
            // A read that lands after the user moved on describes another server.
            if console_target(app).as_deref() == Some(id.as_str())
                && let Ok(text) = result
            {
                app.host.console = Some(text);
            }
            Task::none()
        }
        Msg::Command(c) => {
            app.host.command = c;
            Task::none()
        }
        Msg::SendCommand => {
            let Screen::Server(id) = &app.screen else { return Task::none() };
            let command = std::mem::take(&mut app.host.command);
            let command = command.trim().trim_start_matches('/').to_string();
            if command.is_empty() {
                return Task::none();
            }
            let id = id.clone();
            call(
                app,
                move |core| async move { core.hosting.command(&id, &command).await.map_err(|e| e.to_string()) },
                Msg::CommandSent,
            )
        }
        Msg::CommandSent(result) => {
            app.host.reply = Some(match result {
                Ok(r) if r.trim().is_empty() => "Done.".to_string(),
                Ok(r) => r,
                Err(e) => format!("Couldn't send it: {e}"),
            });
            Task::batch([Task::done(host(Msg::FollowConsole)), Task::done(host(Msg::RefreshPlayers))])
        }
        Msg::RefreshPlayers => {
            let Screen::Server(id) = &app.screen else { return Task::none() };
            if app.host.phase(id) != Phase::Online {
                return Task::none();
            }
            let id = id.clone();
            call(
                app,
                {
                    let id = id.clone();
                    move |core| async move { core.hosting.players(&id).await.map_err(|e| e.to_string()) }
                },
                move |r| Msg::PlayersRead(id.clone(), r),
            )
        }
        Msg::PlayersRead(id, result) => {
            if matches!(&app.screen, Screen::Server(open) if *open == id)
                && let Ok(players) = result
            {
                app.host.players = Some(players);
            }
            Task::none()
        }
        Msg::PlayerInput(s) => {
            app.host.player_input = s;
            Task::none()
        }
        Msg::PlayerAction(verb, player) => {
            if !nexo_core::hosting::is_player_name(&player) {
                app.host.reply = Some(format!("\"{player}\" isn't a Minecraft username."));
                return Task::none();
            }
            app.host.command = format!("{verb} {player}");
            Task::done(host(Msg::SendCommand))
        }
        Msg::SettingsRead(id, result) => {
            if !matches!(&app.screen, Screen::Server(open) if *open == id) {
                return Task::none();
            }
            match result {
                Ok((props, memory_mb)) => {
                    app.host.settings = Some(Settings {
                        values: props.into_iter().filter(|(k, _)| SETTINGS_KEYS.contains(&k.as_str())).collect(),
                        memory_mb,
                        dirty: false,
                    })
                }
                Err(err) => app.status = Status::Error(format!("Couldn't read the settings: {err}")),
            }
            Task::none()
        }
        Msg::SetProp(key, value) => {
            if let Some(s) = &mut app.host.settings {
                s.values.insert(key.to_string(), value);
                s.dirty = true;
            }
            Task::none()
        }
        Msg::SettingsMemory(mb) => {
            if let Some(s) = &mut app.host.settings {
                s.memory_mb = mb;
                s.dirty = true;
            }
            Task::none()
        }
        Msg::SaveSettings => {
            let Screen::Server(id) = &app.screen else { return Task::none() };
            let Some(s) = &app.host.settings else { return Task::none() };
            let updates: Vec<(String, String)> = SETTINGS_KEYS
                .iter()
                .filter_map(|k| s.values.get(*k).map(|v| (k.to_string(), v.trim().to_string())))
                .collect();
            let (id, memory) = (id.clone(), s.memory_mb);
            call(
                app,
                move |core| async move { core.hosting.save_settings(&id, updates, memory).await.map_err(|e| e.to_string()) },
                Msg::SettingsSaved,
            )
        }
        Msg::SettingsSaved(result) => {
            match result {
                Ok(()) => {
                    if let Some(s) = &mut app.host.settings {
                        s.dirty = false;
                    }
                    app.status = Status::Notice("Settings saved. They apply the next time the server starts.".into());
                }
                Err(err) => app.status = Status::Error(format!("Couldn't save the settings: {err}")),
            }
            Task::done(host(Msg::Load))
        }
        Msg::Copy(text) => iced::clipboard::write(text),
        Msg::Delete(id) => {
            if app.host.delete_armed.as_deref() != Some(id.as_str()) {
                app.host.delete_armed = Some(id);
                return Task::none();
            }
            app.host.delete_armed = None;
            call(
                app,
                move |core| async move { core.hosting.delete(&id).await.map_err(|e| e.to_string()) },
                Msg::Deleted,
            )
        }
        Msg::Deleted(result) => {
            if let Err(err) = result {
                app.status = Status::Error(format!("Couldn't delete the server: {err}"));
                return Task::none();
            }
            Task::done(Message::Navigate(Screen::Servers))
        }
    }
}

/// Entering the add-ons step with nothing listed yet shows the most
/// downloaded ones, so the step is never an empty box.
fn then_search(app: &mut App) -> Task<Message> {
    let core = app.core.clone();
    let Some(w) = &mut app.host.wizard else { return Task::none() };
    if w.step != Step::Content {
        return Task::none();
    }
    let Some((_, loader, _)) = w.kind.content() else { return Task::none() };
    let mut tasks = Vec::new();
    if w.results.is_empty() && !w.searching {
        tasks.push(Task::done(host(Msg::Search)));
    }
    // Check every essential against this exact version and loader, so each
    // card can say which build it will get — or that there's none yet.
    let key = (w.kind, w.version.clone());
    let failed = w.probes.values().any(|p| matches!(p, Probe::Failed));
    if (w.probed_for.as_ref() != Some(&key) || failed)
        && let Some(core) = core
    {
        if w.probed_for.as_ref() != Some(&key) {
            // A new version may have builds the last one lacked: bring the
            // defaults back, minus whatever this round finds missing.
            for slug in defaults(w.kind) {
                if !w.chosen.contains(&slug) {
                    w.chosen.push(slug);
                }
            }
        }
        w.probed_for = Some(key.clone());
        w.probes.clear();
        for essential in essentials(w.kind) {
            let slug = essential.slug;
            w.probes.insert(slug, Probe::Checking);
            let (core, (kind, version)) = (core.clone(), key.clone());
            let wanted = version.clone();
            tasks.push(Task::perform(
                async move {
                    let modrinth = core.hosting.modrinth();
                    let versions = modrinth.versions(slug, loader, &wanted).await.map_err(|e| e.to_string())?;
                    // Same pick as the install: newest release, else newest.
                    let found = versions
                        .iter()
                        .find(|v| v.version_type == nexo_core::modrinth::VersionType::Release)
                        .or(versions.first())
                        .map(|v| v.version_number.clone());
                    let project = modrinth.project(slug).await.ok().map(|p| (p.downloads, p.icon_url));
                    Ok((found, project))
                },
                move |r| host(Msg::Probed(kind, version.clone(), slug, r)),
            ));
        }
    }
    Task::batch(tasks)
}

fn wizard(app: &mut App, f: impl FnOnce(&mut Wizard)) {
    if let Some(w) = &mut app.host.wizard {
        f(w);
    }
}

fn set(app: &mut App, f: impl FnOnce(&mut Wizard)) -> Task<Message> {
    wizard(app, f);
    Task::none()
}

fn replace(app: &mut App, server: HostedServer) {
    match app.host.servers.iter_mut().find(|s| s.id == server.id) {
        Some(slot) => *slot = server,
        None => app.host.servers.insert(0, server),
    }
}

fn failed(app: &mut App, id: &str, err: String) -> Task<Message> {
    let live = app.host.live.entry(id.to_string()).or_default();
    live.phase = Phase::Offline;
    live.error = Some(err.clone());
    match &mut app.host.setup {
        Some(setup) if setup.id.as_deref() == Some(id) => setup.error = Some(err),
        _ => app.status = Status::Error(err),
    }
    Task::done(host(Msg::Load))
}

fn open_tunnel(app: &App, id: String) -> Task<Message> {
    call(
        app,
        {
            let id = id.clone();
            move |core| async move { core.hosting.open_tunnel(&id).await.map_err(|e| e.to_string()) }
        },
        move |r| Msg::TunnelOpened(id.clone(), r),
    )
}

/// Whose console is on screen, if any.
pub fn console_target(app: &App) -> Option<String> {
    match &app.screen {
        Screen::Server(id) if app.host.tab == Tab::Console => Some(id.clone()),
        Screen::NewServer => app.host.setup.as_ref().filter(|s| s.show_console).and_then(|s| s.id.clone()),
        _ => None,
    }
}

/// The timers hosting needs. `sampling` runs even unfocused, so the graph has
/// no hole in it and a crashed server is noticed; the rest only while looked at.
pub fn subscriptions(app: &App, focused: bool) -> Vec<iced::Subscription<Message>> {
    use std::time::Duration;
    let mut feeds = Vec::new();
    if app.host.live.values().any(|l| l.phase != Phase::Offline) {
        feeds.push(iced::time::every(Duration::from_secs(2)).map(|_| host(Msg::Sample)));
    }
    if !focused {
        return feeds;
    }
    if console_target(app).is_some() {
        feeds.push(iced::time::every(Duration::from_secs(1)).map(|_| host(Msg::FollowConsole)));
    }
    if matches!(&app.screen, Screen::Server(id) if app.host.phase(id) == Phase::Online) {
        feeds.push(iced::time::every(Duration::from_secs(5)).map(|_| host(Msg::RefreshPlayers)));
    }
    feeds
}

/// RAM ranges by server type: (comfortable low, comfortable high) in MB.
pub fn ram_band(kind: ServerKind) -> (u32, u32) {
    match kind {
        ServerKind::Vanilla => (2048, 4096),
        ServerKind::Paper => (3072, 6144),
        ServerKind::Fabric => (4096, 8192),
    }
}

/// The slider's top end: the machine's memory, capped so the scale stays usable.
pub fn ram_max(app: &App) -> u32 {
    match app.host.total_memory_mb {
        0 => 16384,
        mb => mb.clamp(4096, 32768) / 256 * 256,
    }
}

// ---------------------------------------------------------------- list view

pub fn list_view(app: &App) -> Element<'_, Message> {
    let create = button(text("+ Create server").size(14))
        .padding([8, 16])
        .style(theme::lit_button)
        .on_press_maybe(app.core.is_some().then_some(Message::Navigate(Screen::NewServer)));

    let mut list = column![].spacing(12).width(Fill);
    for (index, server) in app.host.servers.iter().enumerate() {
        list = list.push(super::arrive(app, index, server_card(app, server)));
    }
    if !app.paper_servers.is_empty() {
        list = list.push(Space::new().height(8)).push(
            column![
                text("Converted worlds").size(15).font(theme::BOLD).color(theme::TEXT),
                text("Singleplayer worlds hosted with Paper from an instance's Worlds tab — stopping one copies the world back.")
                    .size(12)
                    .color(theme::MUTED),
            ]
            .spacing(3),
        );
        for (index, server) in app.paper_servers.iter().enumerate() {
            list = list.push(super::arrive(app, index, crate::screens::paper_servers::card(app, server)));
        }
    }

    let body: Element<'_, Message> = if app.host.servers.is_empty() && app.paper_servers.is_empty() {
        column![
            empty_state(
                "No servers yet",
                "Create one in five steps — Paper, vanilla or Fabric, with plugins or mods from Modrinth and a public address friends can join from anywhere.",
            ),
        ]
        .into()
    } else {
        scrollable(list).height(Fill).into()
    };

    column![
        crate::screens::header("Servers", "Host a Minecraft server on this PC — no port forwarding.", Some(create.into())),
        body,
    ]
    .spacing(20)
    .height(Fill)
    .into()
}

fn server_card<'a>(app: &'a App, server: &'a HostedServer) -> Element<'a, Message> {
    let phase = app.host.phase(&server.id);
    let (label, color) = phase_label(phase);
    let initial = server.kind.label().chars().next().unwrap_or('S').to_string();

    let actions = row![
        lifecycle_button(app, server),
        button(text("Open").size(14))
            .padding([7, 16])
            .style(theme::ghost_button)
            .on_press(Message::Navigate(Screen::Server(server.id.clone()))),
    ]
    .spacing(8);

    let mut details = column![
        row![
            text(&server.name).size(17).font(theme::BOLD).color(theme::TEXT),
            container(text(label).size(11)).padding([2, 8]).tinted(theme::pill(color)),
        ]
        .spacing(10)
        .align_y(iced::Center),
        text(format!(
            "{} · {} · {} GB RAM",
            server.kind.label(),
            server.minecraft_version,
            format_gb(server.memory_mb)
        ))
        .size(12)
        .color(theme::MUTED),
    ]
    .spacing(4)
    .width(Fill);
    if phase == Phase::Online {
        details = details.push(text(app.host.address(server)).size(12).font(iced::Font::MONOSPACE).color(theme::MINT));
    }

    container(
        row![
            container(text(initial).size(18).font(theme::BOLD))
                .width(44)
                .height(44)
                .align_x(iced::Center)
                .align_y(iced::Center)
                .tinted(theme::initial_tile(phase == Phase::Online)),
            details,
            actions,
        ]
        .spacing(14)
        .align_y(iced::Center),
    )
    .padding(16)
    .width(Fill)
    .style(theme::card)
    .into()
}

pub fn phase_label(phase: Phase) -> (&'static str, iced::Color) {
    match phase {
        Phase::Offline => ("Offline", theme::MUTED),
        Phase::Starting => ("Starting…", theme::AMBER),
        Phase::Online => ("Online", theme::MINT),
        Phase::Stopping => ("Stopping…", theme::AMBER),
    }
}

/// Start, or Stop, or a disabled label while either is in flight.
pub fn lifecycle_button<'a>(app: &'a App, server: &'a HostedServer) -> Element<'a, Message> {
    let id = server.id.clone();
    match app.host.phase(&server.id) {
        Phase::Offline => button(text("Start").size(14))
            .padding([7, 18])
            .style(theme::primary_button)
            .on_press(host(Msg::Start(id)))
            .into(),
        Phase::Online => button(text("Stop").size(14))
            .padding([7, 18])
            .style(theme::stop_button)
            .on_press(host(Msg::Stop(id)))
            .into(),
        Phase::Starting => button(text("Starting…").size(14)).padding([7, 18]).style(theme::ghost_button).into(),
        Phase::Stopping => button(text("Stopping…").size(14)).padding([7, 18]).style(theme::ghost_button).into(),
    }
}

pub fn format_gb(mb: u32) -> String {
    let gb = mb as f32 / 1024.0;
    if (gb - gb.round()).abs() < 0.05 {
        format!("{}", gb.round() as u32)
    } else {
        format!("{gb:.1}")
    }
}
