// A launcher has no business flashing a console window on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Nexo — native Minecraft client.
//!
//! The UI is deliberately thin: every operation with real logic lives in
//! `nexo-core` and is invoked here as an async [`Task`]. `update` stays a
//! pure state transition, which is what keeps the window responsive while a
//! ~1,000-file install runs underneath it.

mod backdrop;
mod cape_sim;
mod dropdown;
mod lively;
mod motion;
mod pulse;
mod screens;
mod skin3d;
mod sky;
mod theme;

use iced::widget::image;
use iced::{Element, Fill, Task};
use nexo_core::content::ProjectKind;
use nexo_core::minecraft::Progress;
use nexo_core::skin;
use nexo_core::{Account, Instance, Loader, Nexo};
use std::sync::Arc;

/// Minecraft version new instances default to — the single version `Mod/`
/// targets for v1.
const DEFAULT_GAME_VERSION: &str = "26.3";

/// Where `App::clock` wraps, in seconds.
///
/// A whole multiple of every animation period built on it — the accent's
/// [`theme::RAINBOW_PERIOD`] (12s) and the live pulse's 2.4s ring — so both
/// pass through the wrap without a jump. Adding a third animation means
/// checking its period divides this, or raising this to suit.
const CLOCK_WRAP: f32 = theme::RAINBOW_PERIOD;

/// Desktop identity. Must stay in sync with `assets/nexo.desktop`'s filename
/// and its `StartupWMClass` key.
const APP_ID: &str = "nexo";

/// Embedded so the binary is self-sufficient — a `cargo run` from a source
/// checkout gets the right icon without anything being installed first.
const WINDOW_ICON: &[u8] = include_bytes!("../../../assets/icons/256.png");

/// Upscale factors for the two skin renders. Skins are tiny pixel art (an
/// 8×8 face, a 16×32 body), so these are integer multipliers applied with
/// nearest-neighbour sampling.
const FACE_SCALE: u32 = 4;


fn main() -> iced::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "nexo_app=info,nexo_core=info".into()),
        )
        .init();

    // Windows can't delete a running .exe, so an update there leaves the
    // previous binary next to the new one for the next start to clear. A
    // no-op on every other platform.
    nexo_core::self_update::clear_previous();

    let (fonts, default_font) = screens::settings::startup_fonts();
    theme::set_font(default_font);

    iced::application(App::boot, App::update, App::view)
        .title("Nexo")
        // The closure parameter must be annotated. Left to inference, it
        // breaks the higher-ranked lifetime resolution for `view` across the
        // whole builder chain, with an error that points at the chain rather
        // than at this line.
        .theme(|_state: &App| theme::current())
        // Stops entirely when the window loses focus — see `App::subscription`.
        .subscription(App::subscription)
        // Sets the Wayland `app_id` / X11 `WM_CLASS`. It has to match the
        // basename of the installed `nexo.desktop`, or desktop environments
        // can't tie the running window back to its launcher entry and show a
        // generic placeholder icon in the task switcher instead.
        .settings(iced::Settings {
            id: Some(APP_ID.to_string()),
            fonts,
            default_font,
            ..Default::default()
        })
        .window(iced::window::Settings {
            size: iced::Size::new(1080.0, 720.0),
            position: iced::window::Position::Centered,
            min_size: Some(iced::Size::new(880.0, 600.0)),
            // Used by X11 and by Windows for the titlebar. Wayland ignores
            // it and takes the icon from the .desktop file matched via app_id
            // above, which is why both mechanisms are set.
            icon: iced::window::icon::from_file_data(WINDOW_ICON, None).ok(),
            ..Default::default()
        })
        .antialiasing(true)
        .run()
}

/// Not `Copy`: the details screen carries which instance it's showing, so the
/// selected instance survives navigating away and back.
/// Which page is on show: the screen, and whichever tab or step within it.
type ShownPage = (
    Screen,
    Option<screens::instance::Tab>,
    Option<screens::hosting::Tab>,
    Option<screens::hosting::Step>,
);

/// The transitions that aren't per-control.
/// A Modrinth project waiting to go into an instance.
#[derive(Debug, Clone)]
pub struct QueuedInstall {
    pub instance: String,
    pub project: String,
    pub kind: ProjectKind,
}

pub mod motion_keys {
    use crate::motion::Key;
    pub const PAGE: Key = ("page", "");
    pub const RAIL: Key = ("rail", "");
    pub const STATUS: Key = ("status", "");
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Screen {
    Home,
    Instances,
    Accounts,
    Skins,
    /// Vanilla and Nexo custom capes.
    Capes,
    /// Name and skin history of any player, from crafty.gg.
    History,
    /// Every Paper server this machine knows about — see `Mod/ROADMAP.md`
    /// Phase 7. Cross-instance, unlike the other screens: a hosted server
    /// outlives the "open instance" concept the details screen is scoped to.
    Servers,
    /// The launcher's theme and background effect.
    Settings,
    /// Details for one instance, by id.
    Instance(String),
    /// Live log + RCON console for one running Paper server, by id.
    PaperConsole(String),
    /// The create-a-server wizard, then its setup progress.
    NewServer,
    /// One hosted server's dashboard, by id.
    Server(String),
}

impl Screen {
    /// Which sidebar entry should light up. The details screen belongs to
    /// Instances, so the rail doesn't go blank when you drill in.
    fn nav_group(&self) -> Screen {
        match self {
            Screen::Instance(_) => Screen::Instances,
            Screen::PaperConsole(_) | Screen::NewServer | Screen::Server(_) => Screen::Servers,
            other => other.clone(),
        }
    }
}

/// The open server form: which entry it edits, and what has been typed.
#[derive(Debug, Clone, Default)]
pub struct ServerForm {
    /// Position in `servers.dat`, or `None` for a server being added.
    pub editing: Option<usize>,
    pub name: String,
    pub address: String,
}

/// What a server answered, minus the icon.
///
/// The favicon is stripped out on the way in and turned into an
/// `image::Handle` immediately, so the megabyte-scale PNG bytes are not held
/// twice — once here and once in the widget — for every server in the list.
pub type ServerStatus = nexo_core::server_ping::Status;

/// A cape chosen for upload, shown in a popup to be named before it is sent.
#[derive(Debug, Clone)]
pub struct CapeDraft {
    pub path: std::path::PathBuf,
    /// The account it is for: a popup left open across an account switch must not upload to the other one.
    pub account: String,
    /// The file as it was checked and previewed, which is what gets sent; and the decoded
    /// image for the preview, or why it can't be a cape.
    pub bytes: Arc<Vec<u8>>,
    pub texture: Result<Arc<skin::Rgba>, String>,
    pub name: String,
    pub sending: bool,
    pub error: Option<String>,
}

/// Transient feedback shown in the header strip.
#[derive(Debug, Clone, Default)]
pub enum Status {
    #[default]
    Idle,
    /// A long-running operation with a human-readable stage.
    Busy(String),
    Error(String),
    /// A finished operation with something worth reading, e.g. a migration report.
    Notice(String),
}

/// A one-way action that needs a second click, and the instance it is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Armed {
    /// Migrating rewrites the worlds for a newer Minecraft; there is no going
    /// back to the old version with them.
    Migrate(String),
    /// Deleting removes the whole instance folder.
    Delete(String),
    /// Deleting a custom theme, by its place in `App::custom_themes`.
    DeleteTheme(usize),
}

pub struct App {
    /// `None` until [`Message::Booted`] lands; every screen renders a
    /// loading state until then rather than unwrapping.
    core: Option<Nexo>,
    screen: Screen,

    instances: Vec<Instance>,
    accounts: Vec<Account>,
    active_account: Option<String>,

    /// Minecraft releases offered when creating an instance.
    game_versions: Vec<String>,
    /// The one-way action whose button has been clicked once, waiting for
    /// the confirming second click. See [`Armed`].
    armed: Option<Armed>,
    /// Installs waiting their turn, the front one running. One at a time, since
    /// each reads the instance, adds to it and saves it. Not a `Status::Busy`:
    /// a banner would push the list down under the cursor mid bulk-install.
    pub install_queue: std::collections::VecDeque<QueuedInstall>,
    /// Bumped on every arm, so a timeout only disarms the arm that started
    /// it and not a later one.
    armed_generation: u64,

    status: Status,

    // New-instance form.
    new_name: String,
    /// Whether "+ New instance" has opened the create form. The form is
    /// also shown, regardless, while there are no instances at all.
    creating: bool,
    /// Filter over the instance list, by name.
    instance_query: String,
    new_version: String,
    /// Whether a new instance gets Nexo Mod, when its version can have it.
    new_with_nexo: bool,
    /// Open export picker: instance id and (top-level entry, included).
    export_pick: Option<(String, Vec<(String, bool)>)>,

    /// True while the browser sign-in is outstanding, so the button can't be
    /// pressed twice — the second attempt would fail to bind the callback
    /// port anyway.
    signing_in: bool,

    /// Small 2D avatars, keyed by account UUID. Every signed-in account gets
    /// one, not just the active one.
    faces: std::collections::HashMap<String, image::Handle>,

    /// Textures for the 3D model: the skin, and the cape when one is
    /// equipped. `Arc` because every frame's primitive clones them.
    skin_texture: Option<Arc<skin::Rgba>>,
    cape_texture: Option<Arc<skin::Rgba>>,
    skin_model: nexo_core::SkinModel,
    /// Bumped whenever the textures change, so the renderer knows to re-upload
    /// them instead of doing it every frame.
    skin_key: u64,

    /// Running game sessions (`nexo_core::running::session_key`: instance id
    /// plus account uuid), so Play can show Stop. Mirrors the core registry
    /// rather than querying it during `view`, which must stay free of locking.
    running: std::collections::HashSet<String>,
    /// Launches still in their prepare phase (Launch sent, no Done/Failed yet).
    preparing: usize,
    /// Account chosen in each instance's launch picker (instance id -> uuid).
    /// Per app session only; absent means "the active account".
    launch_account: std::collections::HashMap<String, String>,

    /// Latest published Nexo Mod release, once looked up. `None` while
    /// unknown; the error is surfaced through `status` instead.
    nexo_release: Option<nexo_core::nexo_mod::Release>,
    /// Why the last release lookup failed, if it did. Without this the card
    /// cannot tell "still asking" apart from "asked and it went wrong", and a
    /// failure leaves it reading "Checking for the latest release…" forever.
    nexo_release_error: Option<String>,
    /// Every JVM the user could pick for an instance — this machine's plus
    /// any Nexo downloaded. Loaded when an instance is opened, since probing
    /// each candidate means running it.
    java_options: Vec<nexo_core::java::JavaInstall>,
    /// The launcher's *own* update, once a check has found one newer than
    /// this build. `None` covers both "not asked yet" and "already current";
    /// `update_checked` is what tells those apart.
    update: Option<nexo_core::self_update::Update>,
    /// A check or an install is in flight.
    update_busy: bool,
    /// At least one check has finished. Without this the footer can't say
    /// "latest", and a check that finds nothing looks like a dead button.
    update_checked: bool,
    /// The new binary is on disk. The process running this is still the old
    /// one, so this stays set until restart rather than clearing on success.
    update_installed: bool,
    /// Edition picked in the injector card, if the user picked one. `None`
    /// falls back to whatever the instance already has, then to the release's
    /// own default, so the card is never in a state with nothing selected.
    /// Cleared when another instance is opened — it's a choice about that
    /// instance, not a global preference.
    nexo_edition: Option<nexo_core::nexo_mod::Edition>,

    /// Which of the instance screen's four tabs is open. Reset to Content
    /// when an instance is opened — it is a view onto that instance, not a
    /// preference that should follow the user between them.
    tab: screens::instance::Tab,
    /// Tabs that have read their directory at least once for the open
    /// instance. Without it a count badge cannot tell "none" from "haven't
    /// looked", and would show a confident 0 for both.
    tabs_loaded: std::collections::HashSet<screens::instance::Tab>,
    /// The app's animation clock, in seconds. Drives both the drifting accent
    /// colour and the live-log pulse — one clock rather than two, so the two
    /// animations can never disagree about what time it is.
    ///
    /// Wrapped at [`CLOCK_WRAP`], which is a whole multiple of both periods,
    /// so neither animation jumps when it comes round.
    clock: f32,
    /// Everything mid-transition. See `App::start_transitions`.
    pub motion: motion::Motion,
    /// What the transitions last saw, to tell when something changed.
    shown_page: ShownPage,
    shown_rail: f32,
    shown_status: std::mem::Discriminant<Status>,
    /// How far the current page transition starts below its place.
    pub page_rise: f32,
    /// Whether the transition is a tab change inside a screen, not a whole page.
    page_within: bool,
    /// Whether the window has focus. The clock stops when it doesn't.
    focused: bool,
    /// The particles behind the player model.
    pub effect: backdrop::Effect,
    pub custom_themes: Vec<theme::Custom>,
    pub theme_editor: screens::settings::Editor,
    /// Numbers slider moves, so only the latest one's timer saves.
    pub theme_save_due: u64,
    pub particles: backdrop::Tuning,
    pub reduce_motion: bool,
    pub settings: nexo_core::settings::Settings,
    /// The chosen font. The window's is `theme::regular`, which follows it.
    pub font: screens::settings::Typeface,
    pub custom_family: Option<&'static str>,
    pub settings_tab: screens::settings::Tab,
    /// Whether a launch minimised the window, so the game's exit can bring it back.
    pub minimized_for_game: bool,
    /// Sessions the Stop button ended, so that doesn't get reported as a dead game.
    pub stopped_by_user: std::collections::HashSet<String>,

    // Files tab.
    files: Vec<nexo_core::browse::Entry>,
    /// Where the browser is, relative to the instance root. Empty is the
    /// instance itself. Kept relative rather than absolute so the path that
    /// reaches `browse::list_dir` is always one it can bounds-check.
    files_at: std::path::PathBuf,
    files_error: Option<String>,

    // Worlds tab — singleplayer worlds and the multiplayer server list.
    worlds: Vec<nexo_core::browse::World>,
    /// Which world is one click away from being deleted, if any. Same
    /// two-step as the saved-skin grid, for the same reason.
    confirm_delete_world: Option<String>,
    servers: Vec<nexo_core::browse::Server>,
    /// Ping results, keyed by address. Absent means the ping is still out —
    /// the row shows that rather than pretending the server is down.
    server_status: std::collections::HashMap<String, Result<ServerStatus, String>>,
    /// Server icons, keyed by address. Sourced from the live ping when it
    /// answers and from `servers.dat`'s cached copy until then.
    server_icons: std::collections::HashMap<String, image::Handle>,
    /// The server form, when one is open. Adding and editing share it, which
    /// makes "only one open at a time" structural rather than a rule three
    /// separate flags would have to keep agreeing on.
    server_form: Option<ServerForm>,

    // Paper server hosting (Screen::Servers, plus the "Host as Paper Server"
    // action on a Worlds-tab row).
    paper_servers: Vec<nexo_core::paper_server::PaperServer>,
    /// The world one click away from being converted, if any — same
    /// two-step the world-delete button uses, except this step doubles as
    /// EULA consent rather than a plain "are you sure". `(instance id,
    /// world folder)` since the Worlds tab is scoped to whichever instance
    /// is open, but this state must survive being read from a world row.
    confirm_convert_world: Option<(String, String)>,

    /// Servers made from scratch: wizard, setup, dashboards.
    host: screens::hosting::HostState,

    // Paper server console (Screen::PaperConsole).
    /// The tail of `logs/latest.log`, and whether it was cut short. `None`
    /// while it is being read.
    paper_console_log: Option<(String, bool)>,
    paper_console_input: String,
    /// The last RCON command's response (or error), shown under the input.
    paper_console_response: Option<String>,

    // Logs tab.
    logs: Vec<nexo_core::browse::LogFile>,
    selected_log: Option<String>,
    /// The tail of the selected log, and whether it was cut short. `None`
    /// while it is being read.
    log_text: Option<(String, bool)>,

    // Content, on the instance details screen.
    /// Filters the *installed* list. Modrinth has its own separate query.
    content_query: String,
    content_kind: ProjectKind,
    /// True while the Modrinth browser is open in place of the instance's
    /// own content view.
    browsing: bool,
    modrinth_query: String,
    content_results: Vec<nexo_core::modrinth::SearchHit>,
    content_searching: bool,
    /// Project icons, keyed by project id. Fetched once and kept, since
    /// scrolling a result list would otherwise refetch constantly.
    icons: std::collections::HashMap<String, image::Handle>,

    // Skin and cape management.
    capes: Vec<nexo_core::cosmetics::Cape>,
    /// Each owned cape's texture, for drawing it on the player.
    cape_textures: std::collections::HashMap<String, Arc<skin::Rgba>>,

    // Nexo's custom capes.
    cape_tab: screens::capes::CapeTab,
    cape_query: String,
    capes_catalog: Vec<nexo_core::nexo_capes::CustomCape>,
    capes_catalog_error: Option<String>,
    /// The signed-in account's uploads and choices, with the UUID they belong to.
    capes_mine_for: Option<(String, nexo_core::nexo_capes::Mine)>,
    capes_mine_error: Option<String>,
    capes_review_for: Option<(String, nexo_core::nexo_capes::ReviewQueue)>,
    nexo_cape_textures: std::collections::HashMap<String, Arc<skin::Rgba>>,
    /// Which upload asks "delete?" in its tile.
    cape_delete_armed: Option<String>,
    cape_report_armed: Option<String>,
    /// The upload popup: a chosen file, waiting for its name.
    cape_draft: Option<CapeDraft>,
    /// What the shared file says this account wears, read at sign-in, until the service has said.
    nexo_worn_hint: Option<(String, Option<String>)>,
    cape_sort: nexo_core::nexo_capes::CapeSort,
    /// A friend's cape code, being typed.
    cape_code: String,
    /// The review status of each upload the player has already seen, so a new
    /// decision can be pointed out once. Read from disk on first need.
    capes_seen: Option<std::collections::BTreeMap<String, String>>,
    /// Which account's file shared with the game was last looked at, and when it had last changed.
    capes_share_seen: Option<(String, Option<std::time::SystemTime>)>,

    // Saved skins.
    saved_skins: Vec<nexo_core::skin_library::SavedSkin>,
    /// The library's textures, keyed by saved-skin id, with the arm width the
    /// texture itself shows.
    saved_textures: std::collections::HashMap<String, (Arc<skin::Rgba>, nexo_core::SkinModel)>,
    /// What the skin search box holds.
    skin_query: String,
    history_query: String,
    history: screens::history::State,
    /// The saved skin being renamed, and the name typed so far.
    renaming_skin: Option<(String, String)>,
    /// "Continue without an account" was pressed on the welcome screen.
    welcome_dismissed: bool,
    /// The account list has been read once, so an empty one means none.
    accounts_loaded: bool,
    /// Which tile the cursor is over, so its delete button can appear.
    hovered_skin: Option<String>,
    /// Awaiting a yes/no on deleting this one.
    confirm_delete: Option<String>,
}

#[derive(Clone)]
pub enum Message {
    /// Picks a theme, and the effect it comes with. Built-in themes first, then custom ones.
    SetTheme(usize),
    SetEffect(backdrop::Effect),
    ThemeEditor(screens::settings::Msg),
    Booted(Result<Nexo, String>),
    Loaded {
        instances: Vec<Instance>,
        accounts: Vec<Account>,
        active: Option<String>,
    },
    VersionsLoaded(Vec<String>),
    Navigate(Screen),
    DismissStatus,
    Noop,

    // Instances
    NewNameChanged(String),
    ToggleCreateForm,
    InstanceQueryChanged(String),
    NewVersionChanged(String),
    CreateInstance,
    InstanceCreated(Result<(), String>),
    DeleteInstance(String),
    /// An arm's timeout ran out; carries the generation it was started for.
    Disarm(u64),
    OpenInstance(String),
    /// Instance id, account uuid (`None` = the active account).
    Launch(String, Option<String>),
    /// The account to launch an instance with, picked on its screen.
    PickLaunchAccount(String, String),
    /// A session key, not an instance id.
    Stop(String),
    /// Stops every session of an instance (the list's Stop button).
    StopInstance(String),
    GameExited(String),
    LaunchProgress(Progress),

    // Java
    LoadJavaOptions,
    JavaOptionsLoaded(Vec<nexo_core::java::JavaInstall>),
    /// `None` restores automatic detection.
    SetInstanceJava {
        instance: String,
        path: Option<std::path::PathBuf>,
    },
    BrowseForJava(String),
    InstanceSaved(Result<(), String>),

    // Updating the launcher itself
    /// `announce` separates the check that runs at startup from one the user
    /// clicked. GitHub allows 60 unauthenticated requests an hour per IP, so a
    /// failed startup check is an ordinary event and must not throw a banner;
    /// a check someone asked for has to answer either way.
    CheckForUpdate {
        announce: bool,
    },
    UpdateChecked {
        result: Result<Option<nexo_core::self_update::Update>, String>,
        announce: bool,
    },
    InstallUpdate,
    UpdateInstalled(Result<(), String>),

    // Nexo Mod injector
    FetchNexoRelease,
    NexoReleaseLoaded(Result<nexo_core::nexo_mod::Release, String>),
    SelectNexoEdition(nexo_core::nexo_mod::Edition),
    NewWithNexoToggled(bool),
    /// The one button: a new instance with Nexo Mod, then straight into the game.
    QuickSetup,
    QuickSetupDone(Result<String, String>),
    /// Installs one edition. Carries it rather than reading the selection at
    /// handling time, so the button does exactly what its label says.
    InstallNexoMod {
        instance: String,
        edition: nexo_core::nexo_mod::Edition,
    },
    RemoveNexoMod(String),
    NexoModDone(Result<(), String>),

    // Instance tabs
    SelectTab(screens::instance::Tab),
    /// Navigates the file browser. The path is relative to the instance root.
    BrowseFiles(std::path::PathBuf),
    FilesLoaded(Result<Vec<nexo_core::browse::Entry>, String>),
    /// Hands a file or folder to whatever the desktop opens it with.
    OpenPath(std::path::PathBuf),
    LoadWorlds,
    WorldsLoaded(Vec<nexo_core::browse::World>),
    /// `None` backs out of the confirmation.
    AskDeleteWorld(Option<String>),
    DeleteWorld(String),

    // Servers, on the same tab as worlds
    LoadServers,
    ServersLoaded(Vec<nexo_core::browse::Server>),
    /// One server answered, or didn't. Carries the address rather than an
    /// index: the list can be reloaded while pings are still out, and an index
    /// would then land on a different server.
    ServerPinged {
        address: String,
        result: Result<ServerStatus, String>,
    },
    ServerIconLoaded {
        address: String,
        handle: image::Handle,
    },
    /// Opens the server form. `Some(index)` edits that entry, `None` adds a
    /// new one; the index is the position in `servers.dat` — see
    /// `browse::Server::index` for why nothing else can identify an entry.
    OpenServerForm(Option<usize>),
    CloseServerForm,
    ServerFormNameChanged(String),
    ServerFormAddressChanged(String),
    SubmitServerForm,
    /// Forgets every ping result so the list asks again.
    RepingServers,
    // Paper server hosting — see Mod/ROADMAP.md Phase 7.
    LoadPaperServers,
    PaperServersLoaded(Vec<nexo_core::paper_server::PaperServer>),
    /// `None` backs out of the confirmation. `Some((instance, world folder))`
    /// arms it — this doubles as the EULA-consent step, so there is no
    /// separate modal.
    AskConvertToPaperServer(Option<(String, String)>),
    ConvertToPaperServer {
        world_folder: String,
        world_path: std::path::PathBuf,
        game_version: String,
        world_mode: Option<&'static str>,
    },
    PaperServerConverted(Result<nexo_core::paper_server::PaperServer, String>),
    RevertPaperServer(String),
    PaperServerReverted(Result<std::path::PathBuf, String>),
    /// Re-reads `logs/latest.log` for the open `Screen::PaperConsole`. Fired
    /// once on opening the screen and then on the same 1s tick the instance
    /// Logs tab already uses.
    PaperConsoleFollowLog,
    PaperConsoleLogLoaded(Result<(String, bool), String>),
    PaperConsoleInputChanged(String),
    SendPaperConsoleCommand,
    PaperConsoleCommandSent(Result<String, String>),
    /// Everything for servers made from scratch — see `screens::hosting`.
    Host(screens::hosting::Msg),

    LoadLogs,
    LogsLoaded(Vec<nexo_core::browse::LogFile>),
    SelectLog(String),
    LogLoaded(Result<(String, bool), String>),
    /// Advances the animation clock. Carries the elapsed seconds rather than a
    /// frame count so the animations run at the same speed whatever the tick
    /// interval turns out to be on a loaded machine.
    Tick(f32),
    /// A frame was drawn while something is moving.
    Frame(std::time::Instant),
    /// The cursor entered or left a control whose hover state eases.
    Hover(motion::Key, bool),
    WindowFocused(bool),
    /// Re-reads the log being followed. Separate from [`Message::SelectLog`]
    /// because it must not clear the text first — blanking the viewer once a
    /// second is not "live", it is a strobe.
    FollowLog,
    LogFollowed(Result<(String, bool), String>),

    // Content
    ContentQueryChanged(String),
    ContentKindChanged(ProjectKind),
    OpenModrinthBrowser,
    CloseModrinthBrowser,
    ModrinthQueryChanged(String),
    SearchContent,
    IconLoaded {
        project: String,
        handle: image::Handle,
    },
    LoadJarIcons(String),
    ContentResults(Result<Vec<nexo_core::modrinth::SearchHit>, String>),
    InstallProject {
        instance: String,
        project: String,
    },
    InstallDone(Result<(), String>),
    AddFromFile(String),
    RemoveContent {
        instance: String,
        project: String,
    },
    SetContentEnabled {
        instance: String,
        project: String,
        enabled: bool,
    },

    // Skins and capes
    LoadCapes,
    CapesLoaded(Result<Vec<nexo_core::cosmetics::Cape>, String>),
    CapeTextureLoaded {
        cape: String,
        texture: Arc<skin::Rgba>,
    },
    UploadSkin,
    ResetSkin,
    WearCape(String),
    HideCape,
    CosmeticsDone(Result<(), String>),

    // Nexo's custom capes
    CapeTab(screens::capes::CapeTab),
    CapeQuery(String),
    LoadNexoCapes,
    /// The sort it was asked for, so a slow reply for an old sort can't replace a newer one.
    NexoCatalogLoaded(nexo_core::nexo_capes::CapeSort, Result<Vec<nexo_core::nexo_capes::CustomCape>, String>),
    NexoMineLoaded(String, Result<nexo_core::nexo_capes::Mine, String>),
    NexoReviewLoaded(String, Result<nexo_core::nexo_capes::ReviewQueue, String>),
    NexoCapeTexture(String, Arc<skin::Rgba>),
    UploadNexoCape,
    WearNexoCape(Option<String>),
    FavoriteNexoCape(String, bool),
    ArmCapeDelete(Option<String>),
    DeleteNexoCape(String),
    ReviewNexoCape(String, bool),
    OpenCapeTemplate,
    /// The file dialog's answer: a file and its preview, or nothing when cancelled.
    CapeDraftPicked(Option<(std::path::PathBuf, Arc<Vec<u8>>, Result<Arc<skin::Rgba>, String>)>),
    CapeDraftName(String),
    CapeDraftCancel,
    CapeDraftSend,
    CapeDraftSent(String, Result<nexo_core::nexo_capes::Mine, String>),
    CapeSort(nexo_core::nexo_capes::CapeSort),
    CapeCode(String),
    WearByCode,
    ArmCapeReport(Option<String>),
    ReportNexoCape(String),
    CopyCapeCode(String),
    CapeNotice(Result<String, String>),
    /// Once a second on the Capes screen: did the game change the shared file?
    CapesShareTick,

    // Saved skins
    LoadSavedSkins,
    SavedSkinsLoaded(Vec<nexo_core::skin_library::SavedSkin>),
    SkinTextureLoaded {
        skin: String,
        texture: Arc<skin::Rgba>,
        model: nexo_core::SkinModel,
    },
    SkinQuery(String),
    HistoryQuery(String),
    HistorySearch,
    HistoryLoaded(Result<Arc<screens::history::Loaded>, String>),
    /// Starts renaming a saved skin.
    RenameSkin(String),
    RenameDraft(String),
    /// Keeps the typed name.
    RenameDone,
    ContinueWithoutAccount,
    HoverSkin(Option<String>),
    /// The cursor left this skin's tile.
    UnhoverSkin(String),
    WearSavedSkin(String),
    AskDeleteSkin(String),
    CancelDeleteSkin,
    ConfirmDeleteSkin(String),

    // Modpacks
    OpenFolder(String),
    ImportPack,
    ExportPack(String),
    ExportToggle(usize),
    ExportCancel,
    ExportGo,
    PackImported(Result<String, String>),

    // Options sync — copies options.txt from one instance to every other.
    SyncOptions(String),
    /// Instance id, target Minecraft version.
    MigrateInstance(String, String),
    InstanceMigrated(Result<String, String>),
    OptionsSynced(Result<(String, usize), String>),
    /// The live watch loop for a since-closed instance has returned; nothing
    /// to show for it, but `Task::perform` needs a message to land on.
    OptionsWatchDone,

    // Accounts
    StartSignIn,
    SignInFinished(Result<Account, String>),
    SetActiveAccount(String),
    RemoveAccount(String),
    AccountActionDone(Result<(), String>),
    FaceLoaded {
        account: String,
        handle: image::Handle,
    },
    SkinLoaded {
        /// Account this was loaded for; a result for a no-longer-active account is dropped.
        uuid: Option<String>,
        face: image::Handle,
        skin: Arc<skin::Rgba>,
        cape: Option<Arc<skin::Rgba>>,
        model: nexo_core::SkinModel,
    },
}

impl App {
    fn boot() -> (Self, Task<Message>) {
        let appearance = screens::settings::load();
        let app = Self {
            core: None,
            screen: Screen::Home,
            instances: Vec::new(),
            accounts: Vec::new(),
            active_account: None,
            game_versions: vec![DEFAULT_GAME_VERSION.to_string()],
            armed: None,
            install_queue: Default::default(),
            armed_generation: 0,
            status: Status::Busy("Starting up".into()),
            new_name: String::new(),
            creating: false,
            instance_query: String::new(),
            new_version: DEFAULT_GAME_VERSION.to_string(),
            new_with_nexo: true,
            export_pick: None,
            signing_in: false,
            faces: std::collections::HashMap::new(),
            skin_texture: None,
            cape_texture: None,
            skin_model: nexo_core::SkinModel::Classic,
            skin_key: 0,
            running: std::collections::HashSet::new(),
            preparing: 0,
            launch_account: std::collections::HashMap::new(),
            nexo_release: None,
            nexo_release_error: None,
            java_options: Vec::new(),
            update: None,
            update_busy: false,
            update_checked: false,
            update_installed: false,
            nexo_edition: None,
            tab: screens::instance::Tab::Content,
            tabs_loaded: std::collections::HashSet::new(),
            clock: 0.0,
            motion: motion::Motion::new(),
            shown_page: (Screen::Home, None, None, None),
            shown_rail: screens::rail_y(&Screen::Home),
            shown_status: std::mem::discriminant(&Status::Idle),
            page_rise: 14.0,
            page_within: false,
            focused: true,
            effect: appearance.effect,
            theme_editor: appearance.editor,
            theme_save_due: 0,
            particles: appearance.particles,
            reduce_motion: appearance.reduce_motion,
            settings: appearance.settings,
            font: appearance.font,
            custom_family: appearance.custom_family,
            settings_tab: Default::default(),
            minimized_for_game: false,
            stopped_by_user: Default::default(),
            custom_themes: appearance.customs,
            files: Vec::new(),
            files_at: std::path::PathBuf::new(),
            files_error: None,
            worlds: Vec::new(),
            confirm_delete_world: None,
            servers: Vec::new(),
            server_status: std::collections::HashMap::new(),
            server_icons: std::collections::HashMap::new(),
            server_form: None,
            paper_servers: Vec::new(),
            confirm_convert_world: None,
            host: Default::default(),
            paper_console_log: None,
            paper_console_input: String::new(),
            paper_console_response: None,
            logs: Vec::new(),
            selected_log: None,
            log_text: None,
            content_query: String::new(),
            content_kind: ProjectKind::Mod,
            browsing: false,
            modrinth_query: String::new(),
            content_results: Vec::new(),
            content_searching: false,
            icons: std::collections::HashMap::new(),
            capes: Vec::new(),
            cape_textures: std::collections::HashMap::new(),
            cape_tab: Default::default(),
            cape_query: String::new(),
            capes_catalog: Vec::new(),
            capes_catalog_error: None,
            capes_mine_for: None,
            capes_mine_error: None,
            capes_review_for: None,
            nexo_cape_textures: std::collections::HashMap::new(),
            cape_delete_armed: None,
            cape_report_armed: None,
            cape_draft: None,
            nexo_worn_hint: None,
            cape_sort: Default::default(),
            cape_code: String::new(),
            capes_seen: None,
            capes_share_seen: None,
            saved_skins: Vec::new(),
            saved_textures: std::collections::HashMap::new(),
            skin_query: String::new(),
            history_query: String::new(),
            history: screens::history::State::default(),
            renaming_skin: None,
            welcome_dismissed: false,
            accounts_loaded: false,
            hovered_skin: None,
            confirm_delete: None,
        };

        let task = Task::batch([
            Task::perform(
                async { Nexo::new().await.map_err(|e| e.to_string()) },
                Message::Booted,
            ),
            // Draw the placeholder immediately rather than leaving a hole
            // until the account list has loaded.
            Task::done(placeholder_skin()),
        ]);

        (app, task)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        let frame = matches!(message, Message::Frame(_));
        let task = self.handle(message);
        // In one place, so no screen change can forget its transition. Skipped
        // for frames, which by definition change nothing it compares.
        if !frame {
            self.start_transitions();
        }
        task
    }

    /// Compares what is showing against what was, and starts whatever moved.
    fn start_transitions(&mut self) {
        let page = (
            self.screen.clone(),
            matches!(self.screen, Screen::Instance(_)).then_some(self.tab),
            matches!(self.screen, Screen::Server(_)).then_some(self.host.tab),
            self.host.wizard.as_ref().filter(|_| matches!(self.screen, Screen::NewServer)).map(|wizard| wizard.step),
        );
        if page != self.shown_page {
            let same_screen = page.0 == self.shown_page.0;
            // A new screen rises further than a new tab of the same one.
            self.page_rise = if same_screen { 6.0 } else { 14.0 };
            // A new tab moves only what is under the strip; the strip and the header stay put.
            self.page_within = same_screen && matches!(self.screen, Screen::Instance(_) | Screen::Server(_));
            self.motion.restart(motion_keys::PAGE, 0.0, 1.0, if same_screen { 260 } else { 420 });
            self.shown_page = page;
        }
        let rail = screens::rail_y(&self.screen.nav_group());
        if rail != self.shown_rail {
            self.motion.to(motion_keys::RAIL, self.shown_rail, rail, 280);
            self.shown_rail = rail;
        }
        for tab in screens::instance::Tab::ALL {
            self.motion.settle(("tab", tab.label()), if self.tab == tab { 1.0 } else { 0.0 }, 240);
        }
        for tab in screens::hosting::Tab::ALL {
            self.motion.settle(("hosttab", tab.label()), if self.host.tab == tab { 1.0 } else { 0.0 }, 240);
        }
        let status = std::mem::discriminant(&self.status);
        if status != self.shown_status {
            self.motion.restart(motion_keys::STATUS, 0.0, 1.0, 220);
            self.shown_status = status;
        }
    }

    fn handle(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Frame(now) => {
                self.motion.advance(now);
                Task::none()
            }

            Message::Hover(key, hovered) => {
                self.motion.to(key, 0.0, if hovered { 1.0 } else { 0.0 }, if hovered { 120 } else { 220 });
                Task::none()
            }

            Message::Booted(Ok(core)) => {
                self.core = Some(core.clone());
                self.status = Status::Idle;
                Task::batch([
                    reload(core.clone()),
                    load_versions(core),
                    // Quietly: see `Message::CheckForUpdate`.
                    Task::done(Message::CheckForUpdate { announce: false }),
                    // Also quietly: lets the instance list flag outdated Nexo Mod.
                    Task::done(Message::FetchNexoRelease),
                    dev_screen(),
                ])
            }
            Message::Booted(Err(err)) => {
                self.status = Status::Error(format!("Could not start: {err}"));
                Task::none()
            }

            Message::Loaded {
                instances,
                accounts,
                active,
            } => {
                let changed = self.active_account != active;
                self.instances = instances;
                self.accounts = accounts;
                self.active_account = active;
                self.accounts_loaded = true;

                let mut tasks = Vec::new();

                // Only re-fetch the 3D textures when the active account
                // actually changed; reload() runs after nearly every action.
                if changed && let Some(core) = self.core.clone() {
                    tasks.push(load_skin(core.clone(), self.active_account().cloned()));
                    // The Nexo cape for Home's figure, from the file the game shares: no sign-in needed.
                    if let Some(uuid) = self.active_account.clone() {
                        let worn = nexo_core::nexo_capes::shared::read(core.paths.root(), &uuid).and_then(|s| s.worn);
                        self.nexo_worn_hint = Some((uuid, worn));
                        tasks.push(self.fetch_worn_cape());
                    }
                }

                // Every account gets an avatar, not just the active one.
                for account in &self.accounts {
                    let (Some(url), false) = (
                        account.skin_url.clone(),
                        self.faces.contains_key(&account.uuid),
                    ) else {
                        continue;
                    };
                    let Some(core) = self.core.clone() else {
                        continue;
                    };
                    let (uuid, model) = (account.uuid.clone(), account.skin_model);

                    tasks.push(Task::perform(
                        async move {
                            skin::fetch(core.http(), &url, model)
                                .await
                                .ok()
                                .map(|decoded| (uuid, decoded.face(FACE_SCALE)))
                        },
                        |loaded| match loaded {
                            Some((account, face)) => Message::FaceLoaded {
                                account,
                                handle: to_handle(face),
                            },
                            None => Message::Noop,
                        },
                    ));
                }

                Task::batch(tasks)
            }

            Message::VersionsLoaded(versions) => {
                if !versions.is_empty() {
                    self.game_versions = versions;
                }
                Task::none()
            }

            Message::Navigate(screen) => {
                self.armed = None;
                let opening_skins = screen == Screen::Skins && self.screen != Screen::Skins;
                let opening_capes = screen == Screen::Capes && self.screen != Screen::Capes;
                let opening_servers = screen == Screen::Servers && self.screen != Screen::Servers;
                let opening_console = matches!(&screen, Screen::PaperConsole(_)) && screen != self.screen;
                let opening_settings = screen == Screen::Settings && self.screen != Screen::Settings;
                self.screen = screen.clone();
                let host_task = screens::hosting::on_navigate(self, &screen);
                if opening_settings {
                    return Task::batch([Task::done(Message::LoadJavaOptions), host_task, screens::hosting::load_total_memory(self)]);
                }
                if opening_console {
                    self.paper_console_log = None;
                    self.paper_console_input.clear();
                    self.paper_console_response = None;
                    return Task::done(Message::PaperConsoleFollowLog);
                }
                if opening_capes {
                    self.cape_delete_armed = None;
                    return Task::batch([Task::done(Message::LoadCapes), Task::done(Message::LoadNexoCapes)]);
                }
                if opening_skins {
                    return Task::done(Message::LoadSavedSkins);
                }
                if opening_servers {
                    // Re-read every visit rather than caching: a server can
                    // be started or stopped by the Mod between visits, and a
                    // stale list would show a Stop button for a server that
                    // already reverted.
                    return Task::batch([Task::done(Message::LoadPaperServers), host_task]);
                }
                host_task
            }

            Message::DismissStatus => {
                self.status = Status::Idle;
                Task::none()
            }

            Message::Noop => Task::none(),

            Message::Host(msg) => screens::hosting::update(self, msg),

            Message::NewNameChanged(name) => {
                self.new_name = name;
                Task::none()
            }

            Message::ToggleCreateForm => {
                self.creating = !self.creating;
                Task::none()
            }

            Message::InstanceQueryChanged(query) => {
                self.instance_query = query;
                Task::none()
            }

            Message::NewVersionChanged(version) => {
                self.new_version = version;
                Task::none()
            }

            Message::CreateInstance => {
                let Some(core) = self.core.clone() else {
                    return Task::none();
                };
                let name = match self.new_name.trim() {
                    "" => "Nexo".to_string(),
                    name => name.to_string(),
                };

                let version = self.new_version.clone();
                let with_nexo = self
                    .nexo_release
                    .clone()
                    .filter(|release| self.new_with_nexo && release.manifest.minecraft_version == version);
                self.new_name.clear();
                self.creating = false;
                self.status = Status::Busy(format!("Creating {name}"));

                Task::perform(
                    async move {
                        let mut instance = core
                            .instances
                            .create(&name, &version, Loader::Fabric)
                            .await
                            .map_err(|e| e.to_string())?;
                        if let Some(release) = with_nexo {
                            core.nexo_mod
                                .install(&mut instance, &release, release.default_edition())
                                .await
                                .map_err(|e| format!("{name} was created, but Nexo Mod couldn't be added: {e}"))?;
                            core.instances.save(&instance).await.map_err(|e| e.to_string())?;
                        }
                        Ok(())
                    },
                    Message::InstanceCreated,
                )
            }

            Message::InstanceCreated(Ok(())) => {
                self.status = Status::Idle;
                self.core.clone().map(reload).unwrap_or_else(Task::none)
            }
            Message::InstanceCreated(Err(err)) => {
                self.status = Status::Error(err);
                // The instance may exist even so.
                self.core.clone().map(reload).unwrap_or_else(Task::none)
            }

            Message::NewWithNexoToggled(on) => {
                self.new_with_nexo = on;
                Task::none()
            }

            Message::QuickSetup => {
                let (Some(core), Some(release)) = (self.core.clone(), self.nexo_release.clone()) else {
                    return Task::none();
                };
                if self.is_busy() {
                    return Task::none();
                }
                if self.active_account().is_none() {
                    self.status = Status::Error("Sign in with Microsoft first, on the Home screen.".into());
                    return Task::none();
                }
                // Already set up: there's no reason to make another.
                if let Some(existing) = self
                    .instances
                    .iter()
                    .find(|i| nexo_core::nexo_mod::installed(i).is_some() && release.manifest.supports(i))
                {
                    return Task::done(Message::Launch(existing.id.clone(), None));
                }
                self.status = Status::Busy("Setting up Nexo".into());
                Task::perform(
                    async move {
                        let mut instance = core
                            .instances
                            .create("Nexo", &release.manifest.minecraft_version, Loader::Fabric)
                            .await
                            .map_err(|e| e.to_string())?;
                        // Counts as most recently played from the start, or if the launch
                        // fails, Play would then go to whatever was played before.
                        instance.last_played = Some(nexo_core::instance::now());
                        let installed = match core.nexo_mod.install(&mut instance, &release, release.default_edition()).await {
                            Ok(()) => core.instances.save(&instance).await,
                            Err(err) => Err(err),
                        };
                        if let Err(err) = installed {
                            // Brand new and empty, so retries don't pile up husks.
                            core.instances.delete(&instance.id).await.ok();
                            return Err(err.to_string());
                        }
                        Ok(instance.id)
                    },
                    Message::QuickSetupDone,
                )
            }
            Message::QuickSetupDone(Ok(id)) => {
                // Stays Busy until the launch takes over, so the button can't be pressed again.
                let Some(core) = self.core.clone() else {
                    return Task::none();
                };
                reload(core).chain(Task::done(Message::Launch(id, None)))
            }
            Message::QuickSetupDone(Err(err)) => {
                self.status = Status::Error(format!("Couldn't set up Nexo: {err}"));
                self.core.clone().map(reload).unwrap_or_else(Task::none)
            }

            Message::DeleteInstance(id) => {
                let Some(core) = self.core.clone() else {
                    return Task::none();
                };
                // The whole instance folder goes, so the first click only
                // arms the button — the same two-step Migrate uses.
                let armed = Armed::Delete(id.clone());
                if self.armed.as_ref() != Some(&armed) {
                    return self.arm(armed);
                }
                self.armed = None;
                Task::perform(
                    async move {
                        core.instances
                            .delete(&id)
                            .await
                            .map(|_| ())
                            .map_err(|e| e.to_string())
                    },
                    Message::InstanceCreated,
                )
            }

            Message::LoadJarIcons(id) => {
                let Some(core) = self.core.clone() else {
                    return Task::none();
                };
                let Some(instance) = self.instances.iter().find(|i| i.id == id).cloned() else {
                    return Task::none();
                };

                // Read icons straight out of the installed jars. Modrinth's
                // are only known while a search is in memory, so without this
                // installed content is iconless after every restart.
                let mut tasks = Vec::new();
                for installed in &instance.mods {
                    if self.icons.contains_key(&installed.project_id) {
                        continue;
                    }
                    let core = core.clone();
                    let instance = instance.clone();
                    let (project, file_name) =
                        (installed.project_id.clone(), installed.file_name.clone());

                    tasks.push(Task::perform(
                        async move {
                            core.content
                                .jar_icon(&instance, &file_name)
                                .await
                                .map(|icon| (project, icon))
                        },
                        |loaded| match loaded {
                            Some((project, icon)) => Message::IconLoaded {
                                project,
                                handle: image::Handle::from_rgba(
                                    icon.width,
                                    icon.height,
                                    icon.pixels,
                                ),
                            },
                            None => Message::Noop,
                        },
                    ));
                }

                Task::batch(tasks)
            }

            Message::OpenInstance(id) => {
                self.armed = None;
                let icons = Task::done(Message::LoadJarIcons(id.clone()));
                self.screen = Screen::Instance(id);
                // The edition picker starts from this instance's own state,
                // not from what was chosen on the last one.
                self.nexo_edition = None;
                // Every tab shows one instance's directory. Carrying any of it
                // across would show the last instance's worlds under this
                // one's name until the reload landed.
                self.reset_tabs();
                let mut tasks = vec![icons, Task::done(Message::LoadJavaOptions)];
                // The details screen shows injector state, so look up what's
                // published the first time one is opened.
                if self.nexo_release.is_none() {
                    tasks.push(Task::done(Message::FetchNexoRelease));
                }
                Task::batch(tasks)
            }

            Message::PickLaunchAccount(id, uuid) => {
                self.launch_account.insert(id, uuid);
                Task::none()
            }

            Message::Stop(id) => {
                self.stopped_by_user.insert(id.clone());
                if let Some(core) = &self.core {
                    core.stop(&id);
                }
                // The registry deregisters asynchronously; GameExited flips
                // the button back once the process is actually gone.
                Task::none()
            }

            Message::StopInstance(id) => {
                let sessions = self.instance_sessions(&id);
                self.stopped_by_user.extend(sessions.into_iter().map(|(key, _)| key));
                if let Some(core) = &self.core {
                    core.stop_instance(&id);
                }
                Task::none()
            }

            Message::GameExited(id) => {
                self.running.remove(&id);
                // Never overwrites an Error, nor another launch's progress.
                if self.is_busy() && self.preparing == 0 {
                    self.status = Status::Idle;
                }
                screens::settings::after_game_exit(self)
            }

            Message::LoadJavaOptions => {
                let Some(core) = self.core.clone() else {
                    return Task::none();
                };
                // Includes any JVM a setting names outside the usual places, or the
                // pickers would call one that works "not found".
                let mut named: Vec<_> = self.instances.iter().filter_map(|i| i.java_path.clone()).collect();
                named.extend(self.settings.java_path.clone());
                named.retain(|path| nexo_core::java::looks_like_java(path));
                Task::perform(
                    async move {
                        let mut options = nexo_core::java::options(&core.paths).await;
                        for path in named {
                            if !options.iter().any(|java| java.path == path)
                                && let Some(java) = nexo_core::java::probe(&path).await
                            {
                                options.push(java);
                            }
                        }
                        options
                    },
                    Message::JavaOptionsLoaded,
                )
            }
            Message::JavaOptionsLoaded(options) => {
                self.java_options = options;
                Task::none()
            }

            Message::SetInstanceJava { instance, path } => {
                let Some(core) = self.core.clone() else {
                    return Task::none();
                };
                Task::perform(
                    async move {
                        let mut target = core
                            .instances
                            .get(&instance)
                            .await
                            .map_err(|e| e.to_string())?;
                        target.java_path = path;
                        core.instances
                            .save(&target)
                            .await
                            .map_err(|e| e.to_string())
                    },
                    Message::InstanceSaved,
                )
            }

            Message::BrowseForJava(instance) => {
                Task::perform(
                    async move {
                        // Awaited off the UI thread so the window keeps
                        // painting while the dialog is open.
                        let Some(handle) = rfd::AsyncFileDialog::new()
                            .set_title("Pick a Java executable")
                            .pick_file()
                            .await
                        else {
                            // Cancelled, which is not a failure.
                            return None;
                        };
                        Some((instance, handle.path().to_path_buf()))
                    },
                    |picked| match picked {
                        Some((instance, path)) => Message::SetInstanceJava {
                            instance,
                            path: Some(path),
                        },
                        None => Message::Noop,
                    },
                )
            }

            Message::InstanceSaved(result) => {
                let Some(core) = self.core.clone() else {
                    return Task::none();
                };
                match result {
                    // Re-reads from disk rather than patching the in-memory
                    // copy, so the screen can't drift from what was saved.
                    // In order: the list needs the path that was just saved.
                    Ok(()) => reload(core).chain(Task::done(Message::LoadJavaOptions)),
                    Err(err) => {
                        self.status = Status::Error(err);
                        Task::none()
                    }
                }
            }

            Message::CheckForUpdate { announce } => {
                let Some(core) = self.core.clone() else {
                    return Task::none();
                };
                self.update_busy = true;
                if announce {
                    self.status = Status::Busy("Checking for a Nexo update".into());
                }
                Task::perform(
                    async move { core.self_update.check().await.map_err(|e| e.to_string()) },
                    move |result| Message::UpdateChecked { result, announce },
                )
            }
            Message::UpdateChecked { result, announce } => {
                self.update_busy = false;
                self.update_checked = true;
                match result {
                    Ok(update) => {
                        self.update = update;
                        if announce {
                            self.status = Status::Idle;
                        }
                    }
                    Err(err) => {
                        tracing::warn!(%err, "could not check for a Nexo update");
                        // A silent check leaves the footer showing the plain
                        // version, which is the truth: nothing is known about
                        // a newer one.
                        self.update_checked = announce;
                        if announce {
                            self.status = Status::Error(err);
                        }
                    }
                }
                Task::none()
            }
            Message::InstallUpdate => {
                let (Some(core), Some(update)) = (self.core.clone(), self.update.clone()) else {
                    return Task::none();
                };
                self.update_busy = true;
                self.status = Status::Busy(format!(
                    "Downloading Nexo {} ({} MB)",
                    update.version,
                    update.size_mb()
                ));
                Task::perform(
                    async move {
                        core.self_update
                            .apply(&update)
                            .await
                            .map_err(|e| e.to_string())
                    },
                    Message::UpdateInstalled,
                )
            }
            Message::UpdateInstalled(Ok(())) => {
                self.update_busy = false;
                self.update_installed = true;
                self.status = Status::Idle;
                Task::none()
            }
            Message::UpdateInstalled(Err(err)) => {
                self.update_busy = false;
                self.status = Status::Error(err);
                Task::none()
            }

            Message::FetchNexoRelease => {
                let Some(core) = self.core.clone() else {
                    return Task::none();
                };
                // Clear first, so a retry shows the pending state instead of
                // the previous attempt's error sitting under a spinner.
                self.nexo_release_error = None;
                Task::perform(
                    async move {
                        core.nexo_mod
                            .latest_including_prereleases()
                            .await
                            .map_err(|e| e.to_string())
                    },
                    Message::NexoReleaseLoaded,
                )
            }

            Message::NexoReleaseLoaded(Ok(release)) => {
                // New instances default to what the mod runs on.
                if self.new_version == DEFAULT_GAME_VERSION {
                    self.new_version = release.manifest.minecraft_version.clone();
                }
                self.nexo_release = Some(release);
                self.nexo_release_error = None;
                // Updates happen by themselves on Play, so only what that can't do
                // is worth saying: an instance on another Minecraft version.
                let behind = self
                    .instances
                    .iter()
                    .filter(|i| self.nexo_mod_outdated(i))
                    .filter(|i| !self.settings.auto_update_mod || !self.nexo_release.as_ref().is_some_and(|r| r.manifest.supports(i)))
                    .count();
                if let Some(release) = &self.nexo_release
                    && behind > 0
                    && matches!(self.status, Status::Idle)
                {
                    let version = &release.manifest.mod_version;
                    self.status = Status::Notice(match behind {
                        1 => format!("Nexo Mod {version} is out, for Minecraft {}. 1 instance can't update until it's on that version.", release.manifest.minecraft_version),
                        n => format!("Nexo Mod {version} is out, for Minecraft {}. {n} instances can't update until they're on that version.", release.manifest.minecraft_version),
                    });
                }
                Task::none()
            }
            Message::NexoReleaseLoaded(Err(err)) => {
                tracing::warn!(%err, "could not look up the latest Nexo Mod release");
                self.nexo_release_error = Some(err);
                Task::none()
            }

            Message::SelectNexoEdition(edition) => {
                self.nexo_edition = Some(edition);
                Task::none()
            }

            Message::InstallNexoMod {
                instance: id,
                edition,
            } => {
                let (Some(core), Some(release)) = (self.core.clone(), self.nexo_release.clone())
                else {
                    return Task::none();
                };
                // Swapping the jar under a game, or under a launch being prepared.
                if self.instance_running(&id) || self.preparing > 0 {
                    return Task::none();
                }
                self.status = Status::Busy(format!("Installing Nexo Mod ({edition})"));

                Task::perform(
                    async move {
                        let mut instance =
                            core.instances.get(&id).await.map_err(|e| e.to_string())?;
                        // Installs one edition and takes the other out on the
                        // way — they can't share a mods/ folder.
                        core.nexo_mod
                            .install(&mut instance, &release, edition)
                            .await
                            .map_err(|e| e.to_string())?;
                        // Persist the content list, or the install is
                        // invisible after a restart.
                        core.instances
                            .save(&instance)
                            .await
                            .map_err(|e| e.to_string())
                    },
                    Message::NexoModDone,
                )
            }

            Message::RemoveNexoMod(id) => {
                let Some(core) = self.core.clone() else {
                    return Task::none();
                };
                self.status = Status::Busy("Removing Nexo Mod".into());

                Task::perform(
                    async move {
                        let mut instance =
                            core.instances.get(&id).await.map_err(|e| e.to_string())?;
                        core.nexo_mod
                            .remove(&mut instance)
                            .await
                            .map_err(|e| e.to_string())?;
                        core.instances
                            .save(&instance)
                            .await
                            .map_err(|e| e.to_string())
                    },
                    Message::NexoModDone,
                )
            }

            Message::NexoModDone(Ok(())) => {
                self.status = Status::Idle;
                if let Screen::Instance(id) = self.screen.clone() {
                    let refresh = Task::done(Message::LoadJarIcons(id));
                    return match self.core.clone() {
                        Some(core) => Task::batch([reload(core), refresh]),
                        None => refresh,
                    };
                }
                // Deliberately stays in the browser. Installing one thing is
                // rarely the whole job, and closing it forced a re-open and a
                // fresh search for every single mod.
                self.core.clone().map(reload).unwrap_or_else(Task::none)
            }
            Message::NexoModDone(Err(err)) => {
                self.status = Status::Error(err);
                Task::none()
            }

            Message::SelectTab(tab) => {
                self.tab = tab;
                // The armed buttons live under Overview, so an armed click
                // could otherwise sit out of sight and fire on the next visit.
                self.armed = None;
                // Re-read on every visit rather than caching. The game writes
                // into this directory while it runs, so a cached listing is
                // stale the moment it matters — and a directory read is
                // cheap next to being wrong about what is on disk.
                match tab {
                    screens::instance::Tab::Overview | screens::instance::Tab::Content => {
                        Task::none()
                    }
                    screens::instance::Tab::Files => {
                        Task::done(Message::BrowseFiles(self.files_at.clone()))
                    }
                    // One tab, two lists: the worlds on disk and the servers
                    // in the multiplayer list.
                    screens::instance::Tab::Worlds => Task::batch([
                        Task::done(Message::LoadWorlds),
                        Task::done(Message::LoadServers),
                    ]),
                    screens::instance::Tab::Logs => Task::done(Message::LoadLogs),
                }
            }

            Message::BrowseFiles(rel) => {
                let Some(dir) = self.open_instance_dir() else {
                    return Task::none();
                };
                // Moved before the read lands, so the breadcrumb follows the
                // click immediately instead of after the directory answers.
                self.files_at = rel.clone();
                self.files_error = None;
                Task::perform(
                    async move {
                        nexo_core::browse::list_dir(&dir, &rel)
                            .await
                            .map_err(|e| e.to_string())
                    },
                    Message::FilesLoaded,
                )
            }

            Message::FilesLoaded(Ok(entries)) => {
                self.files = entries;
                self.files_error = None;
                self.tabs_loaded.insert(screens::instance::Tab::Files);
                Task::none()
            }
            Message::FilesLoaded(Err(err)) => {
                // Shown in the tab rather than the header banner: it is about
                // this one folder, not about the app.
                self.files.clear();
                self.files_error = Some(err);
                self.tabs_loaded.remove(&screens::instance::Tab::Files);
                Task::none()
            }

            Message::OpenPath(path) => {
                if let Err(err) = open::that_detached(&path) {
                    self.status = Status::Error(format!("Couldn't open that: {err}"));
                }
                Task::none()
            }

            Message::LoadWorlds => {
                let Some(dir) = self.open_instance_dir() else {
                    return Task::none();
                };
                Task::perform(
                    async move { nexo_core::browse::worlds(&dir).await },
                    Message::WorldsLoaded,
                )
            }
            Message::WorldsLoaded(worlds) => {
                // A world that was deleted while the listing was in flight
                // must not leave the confirmation armed against a name that
                // no longer exists.
                if let Some(pending) = &self.confirm_delete_world
                    && !worlds.iter().any(|w| &w.folder == pending)
                {
                    self.confirm_delete_world = None;
                }
                if let Some((_, folder)) = &self.confirm_convert_world
                    && !worlds.iter().any(|w| &w.folder == folder)
                {
                    self.confirm_convert_world = None;
                }
                self.worlds = worlds;
                self.tabs_loaded.insert(screens::instance::Tab::Worlds);
                Task::none()
            }

            Message::AskDeleteWorld(folder) => {
                self.confirm_delete_world = folder;
                Task::none()
            }
            Message::DeleteWorld(folder) => {
                let Some(dir) = self.open_instance_dir() else {
                    return Task::none();
                };
                self.confirm_delete_world = None;
                Task::perform(
                    async move {
                        nexo_core::browse::delete_world(&dir, &folder)
                            .await
                            .map_err(|e| e.to_string())
                    },
                    // Reuses the generic "an action finished" handler, which
                    // reports the error and reloads. The reload re-reads the
                    // instance list; the worlds themselves come from the
                    // follow-up below.
                    |result| match result {
                        Ok(()) => Message::LoadWorlds,
                        Err(err) => Message::InstanceSaved(Err(err)),
                    },
                )
            }

            Message::LoadServers => {
                let Some(dir) = self.open_instance_dir() else {
                    return Task::none();
                };
                Task::perform(
                    async move { nexo_core::browse::servers(&dir).await },
                    Message::ServersLoaded,
                )
            }

            Message::ServersLoaded(servers) => {
                let mut tasks = Vec::new();

                for server in &servers {
                    // The icon the game cached, shown until — or instead of —
                    // a live one. A server that is down still gets its face.
                    if let Some(icon) = &server.icon
                        && !self.server_icons.contains_key(&server.address)
                    {
                        self.server_icons.insert(
                            server.address.clone(),
                            image::Handle::from_bytes(icon.clone()),
                        );
                    }

                    // Re-pinging every server on every visit to the tab would
                    // be a burst of connections to other people's machines
                    // each time someone clicks between tabs. One ping per
                    // server per open instance is enough to fill the list.
                    if !self.server_status.contains_key(&server.address) {
                        let address = server.address.clone();
                        tasks.push(Task::perform(
                            async move {
                                let result = nexo_core::server_ping::ping(&address)
                                    .await
                                    .map_err(|e| e.to_string());
                                (address, result)
                            },
                            |(address, result)| Message::ServerPinged { address, result },
                        ));
                    }
                }

                self.servers = servers;
                self.tabs_loaded.insert(screens::instance::Tab::Worlds);
                Task::batch(tasks)
            }

            Message::ServerPinged { address, mut result } => {
                // Decoded off the UI thread and handed over as a handle. The
                // favicon is taken out of the status either way, so the stored
                // copy never carries the PNG bytes around with it.
                let favicon = match &mut result {
                    Ok(status) => status.favicon.take(),
                    Err(_) => None,
                };
                self.server_status.insert(address.clone(), result);

                match favicon {
                    Some(bytes) => Task::done(Message::ServerIconLoaded {
                        address,
                        handle: image::Handle::from_bytes(bytes),
                    }),
                    None => Task::none(),
                }
            }

            Message::ServerIconLoaded { address, handle } => {
                self.server_icons.insert(address, handle);
                Task::none()
            }

            Message::OpenServerForm(editing) => {
                // Editing starts from what is already there, so a rename does
                // not mean retyping the address — and an empty address field
                // would disable the very button that saves it.
                let existing = editing
                    .and_then(|index| self.servers.iter().find(|s| s.index == index));

                self.server_form = Some(ServerForm {
                    editing,
                    name: existing.map(|s| s.name.clone()).unwrap_or_default(),
                    address: existing.map(|s| s.address.clone()).unwrap_or_default(),
                });
                Task::none()
            }
            Message::CloseServerForm => {
                self.server_form = None;
                Task::none()
            }
            Message::ServerFormNameChanged(name) => {
                if let Some(form) = &mut self.server_form {
                    form.name = name;
                }
                Task::none()
            }
            Message::ServerFormAddressChanged(address) => {
                if let Some(form) = &mut self.server_form {
                    form.address = address;
                }
                Task::none()
            }

            Message::RepingServers => {
                // Only the results are dropped; the icons stay, so the list
                // keeps its faces while it re-pings instead of flashing empty.
                self.server_status.clear();
                Task::done(Message::LoadServers)
            }

            Message::SubmitServerForm => {
                let Some(dir) = self.open_instance_dir() else {
                    return Task::none();
                };
                let Screen::Instance(id) = &self.screen else {
                    return Task::none();
                };
                let Some(form) = self.server_form.take() else {
                    return Task::none();
                };

                // Minecraft rewrites the whole of servers.dat when it closes
                // the multiplayer screen, so anything written underneath a
                // running game is discarded without a word. Refusing is the
                // only honest answer; the button is disabled too, and this is
                // the backstop for the race between the two.
                if self.instance_running(id) {
                    self.status = Status::Error(
                        "Close the game first — Minecraft overwrites its server list on exit."
                            .into(),
                    );
                    return Task::none();
                }

                // The address may have changed, so the old ping result no
                // longer describes this row. Dropping it puts the row back to
                // "Pinging…" rather than showing the previous server's MOTD
                // under the new address.
                if let Some(index) = form.editing
                    && let Some(server) = self.servers.iter().find(|s| s.index == index)
                {
                    self.server_status.remove(&server.address);
                    self.server_icons.remove(&server.address);
                }

                Task::perform(
                    async move {
                        match form.editing {
                            Some(index) => {
                                nexo_core::browse::update_server(
                                    &dir,
                                    index,
                                    &form.name,
                                    &form.address,
                                )
                                .await
                            }
                            None => {
                                nexo_core::browse::add_server(&dir, &form.name, &form.address).await
                            }
                        }
                        .map_err(|e| e.to_string())
                    },
                    |result| match result {
                        Ok(()) => Message::LoadServers,
                        Err(err) => Message::InstanceSaved(Err(err)),
                    },
                )
            }

            Message::LoadPaperServers => {
                let Some(core) = self.core.clone() else {
                    return Task::none();
                };
                Task::perform(
                    async move { core.paper_servers.store().list().await },
                    Message::PaperServersLoaded,
                )
            }
            Message::PaperServersLoaded(servers) => {
                self.paper_servers = servers;
                Task::none()
            }

            Message::AskConvertToPaperServer(pending) => {
                self.confirm_convert_world = pending;
                Task::none()
            }

            Message::ConvertToPaperServer { world_folder, world_path, game_version, world_mode } => {
                let Some(core) = self.core.clone() else {
                    return Task::none();
                };
                let Some(account) = self.active_account().cloned() else {
                    return Task::none();
                };
                self.confirm_convert_world = None;
                self.status = Status::Busy(format!(
                    "Converting \"{world_folder}\" to a Paper server — this can take a few minutes, \
                     Paper does a one-time migration pass the first time it opens a world"
                ));

                let game_mode = world_mode.unwrap_or("Survival").to_lowercase();
                Task::perform(
                    async move {
                        core.paper_servers
                            .convert_and_host(&world_path, &world_folder, &game_version, "launcher", &game_mode, &account.username)
                            .await
                            .map_err(|e| e.to_string())
                    },
                    Message::PaperServerConverted,
                )
            }
            Message::PaperServerConverted(Ok(_)) => {
                self.status = Status::Idle;
                // Jumps to the cross-instance list so the result of the
                // conversion is immediately visible, not left on the Worlds
                // tab it was started from.
                Task::batch([
                    Task::done(Message::Navigate(Screen::Servers)),
                    Task::done(Message::LoadPaperServers),
                ])
            }
            Message::PaperServerConverted(Err(err)) => {
                self.status = Status::Error(err);
                Task::none()
            }

            Message::RevertPaperServer(id) => {
                let Some(core) = self.core.clone() else {
                    return Task::none();
                };
                self.status = Status::Busy(format!("Stopping and reverting \"{id}\""));
                Task::perform(
                    async move { core.paper_servers.stop_and_revert(&id).await.map_err(|e| e.to_string()) },
                    Message::PaperServerReverted,
                )
            }
            Message::PaperServerReverted(Ok(_backup)) => {
                self.status = Status::Idle;
                Task::done(Message::LoadPaperServers)
            }
            Message::PaperServerReverted(Err(err)) => {
                self.status = Status::Error(err);
                Task::none()
            }

            Message::PaperConsoleFollowLog => {
                let Some(core) = self.core.clone() else {
                    return Task::none();
                };
                let Screen::PaperConsole(id) = &self.screen else {
                    return Task::none();
                };
                let id = id.clone();
                Task::perform(
                    async move {
                        let dir = core.paper_servers.store().server_dir(&id);
                        let logs = nexo_core::browse::logs(&dir).await;
                        let Some(latest) = logs.into_iter().find(|l| l.name == "latest.log") else {
                            return Err("No log yet".to_string());
                        };
                        nexo_core::browse::read_log(&latest).await.map_err(|e| e.to_string())
                    },
                    Message::PaperConsoleLogLoaded,
                )
            }
            Message::PaperConsoleLogLoaded(Ok(read)) => {
                self.paper_console_log = Some(read);
                Task::none()
            }
            // A failed re-read is dropped, same as the instance Logs tab's
            // `LogFollowed`: the file may not exist yet right after
            // conversion, and replacing the view with an error on every
            // missed tick would be worse than showing slightly stale text.
            Message::PaperConsoleLogLoaded(Err(_)) => Task::none(),

            Message::PaperConsoleInputChanged(text) => {
                self.paper_console_input = text;
                Task::none()
            }
            Message::SendPaperConsoleCommand => {
                let Screen::PaperConsole(id) = &self.screen else {
                    return Task::none();
                };
                let Some(server) = self.paper_servers.iter().find(|s| &s.id == id).cloned() else {
                    return Task::none();
                };
                let command = std::mem::take(&mut self.paper_console_input);
                if command.trim().is_empty() {
                    return Task::none();
                }
                Task::perform(
                    async move {
                        let mut client = nexo_core::paper_server::rcon::RconClient::connect(
                            "127.0.0.1",
                            server.rcon.port,
                            &server.rcon.password,
                            std::time::Duration::from_secs(5),
                        )
                        .await
                        .map_err(|e| e.to_string())?;
                        client.command(&command).await.map_err(|e| e.to_string())
                    },
                    Message::PaperConsoleCommandSent,
                )
            }
            Message::PaperConsoleCommandSent(Ok(response)) => {
                self.paper_console_response = Some(response);
                // The command likely just changed something worth seeing in
                // the log too (e.g. an op/gamemode grant).
                Task::done(Message::PaperConsoleFollowLog)
            }
            Message::PaperConsoleCommandSent(Err(err)) => {
                self.paper_console_response = Some(format!("Error: {err}"));
                Task::none()
            }

            Message::LoadLogs => {
                let Some(dir) = self.open_instance_dir() else {
                    return Task::none();
                };
                Task::perform(
                    async move { nexo_core::browse::logs(&dir).await },
                    Message::LogsLoaded,
                )
            }
            Message::LogsLoaded(logs) => {
                // Opening the tab with the newest log already showing saves
                // the one click that is right nearly every time — that file
                // is the session someone just came back from.
                let first = logs.first().map(|l| l.name.clone());
                let still_there = self
                    .selected_log
                    .as_ref()
                    .filter(|name| logs.iter().any(|l| &&l.name == name))
                    .cloned();
                self.logs = logs;
                self.tabs_loaded.insert(screens::instance::Tab::Logs);

                match still_there.or(first) {
                    Some(name) if self.selected_log.as_ref() != Some(&name) => {
                        Task::done(Message::SelectLog(name))
                    }
                    Some(_) => Task::none(),
                    None => {
                        self.selected_log = None;
                        self.log_text = None;
                        Task::none()
                    }
                }
            }

            Message::SelectLog(name) => {
                let Some(file) = self.logs.iter().find(|l| l.name == name).cloned() else {
                    return Task::none();
                };
                self.selected_log = Some(name);
                // Cleared so the viewer says "Reading…" instead of leaving the
                // previous file's text under the new file's name.
                self.log_text = None;
                Task::perform(
                    async move {
                        nexo_core::browse::read_log(&file)
                            .await
                            .map_err(|e| e.to_string())
                    },
                    Message::LogLoaded,
                )
            }
            Message::LogLoaded(Ok(read)) => {
                self.log_text = Some(read);
                Task::none()
            }
            Message::LogLoaded(Err(err)) => {
                self.log_text = Some((err, false));
                Task::none()
            }

            Message::Tick(seconds) => {
                // Wrapped rather than left to grow. An f32 accumulating 30
                // times a second loses its fractional precision within a day
                // of uptime, and both animations would start visibly stepping.
                self.clock = (self.clock + seconds) % CLOCK_WRAP;
                Task::none()
            }

            Message::SetTheme(index) => {
                // Picking the theme already on leaves a chosen background alone.
                if index != theme::selected() {
                    screens::settings::pick(self, index);
                }
                Task::none()
            }
            Message::SetEffect(effect) => {
                screens::settings::set_effect(self, effect);
                Task::none()
            }
            Message::ThemeEditor(msg) => screens::settings::update(self, msg),
            Message::WindowFocused(focused) => {
                self.focused = focused;
                Task::none()
            }

            Message::FollowLog => {
                let Some(file) = self
                    .selected_log
                    .as_ref()
                    .and_then(|name| self.logs.iter().find(|l| &l.name == name))
                    .cloned()
                else {
                    return Task::none();
                };
                Task::perform(
                    async move {
                        nexo_core::browse::read_log(&file)
                            .await
                            .map_err(|e| e.to_string())
                    },
                    Message::LogFollowed,
                )
            }
            Message::LogFollowed(Ok(read)) => {
                // A failed re-read is dropped on purpose: the file is being
                // written to, so a transient read error is expected, and
                // replacing a screen of log with an error message every time
                // one happens would be worse than showing slightly stale text.
                self.log_text = Some(read);
                Task::none()
            }
            Message::LogFollowed(Err(_)) => Task::none(),

            Message::ContentQueryChanged(query) => {
                // Filters the installed list in `view`; no work to do here.
                self.content_query = query;
                Task::none()
            }

            Message::ModrinthQueryChanged(query) => {
                self.modrinth_query = query;
                Task::none()
            }

            Message::OpenModrinthBrowser => {
                self.browsing = true;
                // Opens on the popular projects for this instance rather than
                // an empty list waiting to be typed into.
                Task::done(Message::SearchContent)
            }

            Message::CloseModrinthBrowser => {
                self.browsing = false;
                Task::none()
            }

            Message::ContentKindChanged(kind) => {
                self.content_kind = kind;
                // Results are kind-specific, so re-run rather than showing
                // mods under a "Shaders" filter.
                if self.browsing {
                    return Task::done(Message::SearchContent);
                }
                Task::none()
            }

            Message::IconLoaded { project, handle } => {
                self.icons.insert(project, handle);
                Task::none()
            }

            Message::SearchContent => {
                let (Some(core), Screen::Instance(id)) = (self.core.clone(), self.screen.clone())
                else {
                    return Task::none();
                };
                let Some(instance) = self.instances.iter().find(|i| i.id == id).cloned() else {
                    return Task::none();
                };

                self.content_searching = true;
                let query = self.modrinth_query.clone();
                let kind = self.content_kind;

                Task::perform(
                    async move {
                        use nexo_core::modrinth::{SearchQuery, SortIndex};
                        // Narrowed to what this instance can actually install;
                        // an unfiltered list is mostly results that won't work.
                        let results = core
                            .content
                            .modrinth()
                            .search(&SearchQuery {
                                text: &query,
                                loader: match kind {
                                    ProjectKind::Mod => instance.loader.modrinth_facet(),
                                    _ => None,
                                },
                                game_version: Some(&instance.game_version),
                                project_type: Some(kind.facet()),
                                sort: if query.trim().is_empty() {
                                    SortIndex::Downloads
                                } else {
                                    SortIndex::Relevance
                                },
                                limit: 30,
                                offset: 0,
                            })
                            .await
                            .map_err(|e| e.to_string())?;
                        Ok(results.hits)
                    },
                    Message::ContentResults,
                )
            }

            Message::ContentResults(Ok(hits)) => {
                self.content_searching = false;

                let fetches = fetch_icons(self.core.clone(), &hits, &self.icons);
                self.content_results = hits;
                fetches
            }
            Message::ContentResults(Err(err)) => {
                self.content_searching = false;
                self.status = Status::Error(err);
                Task::none()
            }

            Message::InstallProject { instance, project } => {
                if self.core.is_none() || self.install_queue.iter().any(|q| q.instance == instance && q.project == project) {
                    return Task::none();
                }
                self.install_queue.push_back(QueuedInstall { instance, project, kind: self.content_kind });
                // Otherwise the one running starts this when it finishes.
                if self.install_queue.len() == 1 { self.next_install() } else { Task::none() }
            }

            Message::InstallDone(result) => {
                self.install_queue.pop_front();
                // The rest of the queue still goes: one mod without a build for
                // this version shouldn't cancel the others.
                if let Err(err) = result {
                    self.status = Status::Error(err);
                }
                let mut tasks = vec![self.next_install()];
                tasks.extend(self.core.clone().map(reload));
                if self.install_queue.is_empty() && let Screen::Instance(id) = self.screen.clone() {
                    tasks.push(Task::done(Message::LoadJarIcons(id)));
                }
                Task::batch(tasks)
            }

            Message::AddFromFile(instance) => {
                let Some(core) = self.core.clone() else {
                    return Task::none();
                };
                let kind = self.content_kind;

                Task::perform(
                    async move {
                        // Portal dialog, awaited off the UI thread so the
                        // window keeps painting while it is open.
                        let Some(handle) = rfd::AsyncFileDialog::new()
                            .set_title("Add content")
                            .add_filter("Minecraft content", &["jar", "zip"])
                            .pick_file()
                            .await
                        else {
                            // Cancelled, which is not a failure.
                            return Ok(());
                        };

                        let mut target = core
                            .instances
                            .get(&instance)
                            .await
                            .map_err(|e| e.to_string())?;
                        core.content
                            .install_file(&mut target, handle.path(), Some(kind))
                            .await
                            .map_err(|e| e.to_string())?;
                        core.instances
                            .save(&target)
                            .await
                            .map_err(|e| e.to_string())
                    },
                    Message::NexoModDone,
                )
            }

            Message::RemoveContent { instance, project } => {
                let Some(core) = self.core.clone() else {
                    return Task::none();
                };
                Task::perform(
                    async move {
                        let mut target = core
                            .instances
                            .get(&instance)
                            .await
                            .map_err(|e| e.to_string())?;
                        core.content
                            .remove(&mut target, &project)
                            .await
                            .map_err(|e| e.to_string())?;
                        core.instances
                            .save(&target)
                            .await
                            .map_err(|e| e.to_string())
                    },
                    Message::NexoModDone,
                )
            }

            Message::SetContentEnabled {
                instance,
                project,
                enabled,
            } => {
                let Some(core) = self.core.clone() else {
                    return Task::none();
                };
                Task::perform(
                    async move {
                        let mut target = core
                            .instances
                            .get(&instance)
                            .await
                            .map_err(|e| e.to_string())?;
                        core.content
                            .set_enabled(&mut target, &project, enabled)
                            .await
                            .map_err(|e| e.to_string())?;
                        core.instances
                            .save(&target)
                            .await
                            .map_err(|e| e.to_string())
                    },
                    Message::NexoModDone,
                )
            }

            Message::OpenFolder(id) => {
                if let Some(core) = &self.core {
                    let folder = core.paths.instance(&id);
                    // The folder only exists once something has been written
                    // into it, and opening a missing path fails silently in
                    // most file managers.
                    if let Err(err) = std::fs::create_dir_all(&folder) {
                        self.status = Status::Error(format!("Couldn't open the folder: {err}"));
                    } else if let Err(err) = open::that_detached(&folder) {
                        self.status = Status::Error(format!("Couldn't open the folder: {err}"));
                    }
                }
                Task::none()
            }

            Message::ImportPack => {
                let Some(core) = self.core.clone() else {
                    return Task::none();
                };
                self.status = Status::Busy("Importing modpack".into());

                Task::perform(
                    async move {
                        let Some(handle) = rfd::AsyncFileDialog::new()
                            .set_title("Import a Modrinth modpack")
                            .add_filter("Modrinth modpack", &["mrpack"])
                            .pick_file()
                            .await
                        else {
                            // Cancelled, which is not a failure.
                            return Ok(String::new());
                        };

                        let imported = core
                            .mrpack
                            .import(handle.path(), &core.instances)
                            .await
                            .map_err(|e| e.to_string())?;

                        Ok(if imported.skipped.is_empty() {
                            format!("Imported {} — {} files", imported.name, imported.files)
                        } else {
                            // Named rather than counted: knowing which mod is
                            // missing is what makes it fixable. Past 10, only
                            // the first few, so the message stays readable.
                            let n = imported.skipped.len();
                            // Paths come from the pack: strip control chars, cap length.
                            let clean = |p: &String| -> String {
                                p.chars()
                                    .filter(|c| {
                                        !c.is_control()
                                            && !matches!(*c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}')
                                    })
                                    .take(120)
                                    .collect()
                            };
                            let shown = if n > 10 {
                                let first: Vec<_> = imported.skipped[..5].iter().map(clean).collect();
                                format!("{}, and {} more", first.join(", "), n - 5)
                            } else {
                                imported.skipped.iter().map(clean).collect::<Vec<_>>().join(", ")
                            };
                            format!(
                                "Imported {} — {} files, {} unavailable ({})",
                                imported.name, imported.files, n, shown
                            )
                        })
                    },
                    Message::PackImported,
                )
            }

            Message::ExportPack(id) => {
                let Some(core) = self.core.as_ref() else {
                    return Task::none();
                };
                let root = core.paths.instance(&id);
                let mut names: Vec<String> = std::fs::read_dir(&root)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| n != "nexo-instance.json")
                    .collect();
                names.sort();
                // Allowlist: config can hold tokens, so it starts unticked.
                let on_by_default = ["mods", "resourcepacks", "shaderpacks", "datapacks", "options.txt"];
                self.export_pick = Some((
                    id,
                    names
                        .into_iter()
                        .map(|n| { let on = on_by_default.contains(&n.as_str()); (n, on) })
                        .collect(),
                ));
                Task::none()
            }
            Message::ExportToggle(i) => {
                if let Some((_, rows)) = self.export_pick.as_mut() {
                    if let Some(r) = rows.get_mut(i) {
                        r.1 = !r.1;
                    }
                }
                Task::none()
            }
            Message::ExportCancel => {
                self.export_pick = None;
                Task::none()
            }
            Message::ExportGo => {
                let Some((id, rows)) = self.export_pick.take() else {
                    return Task::none();
                };
                let Some(core) = self.core.clone() else {
                    return Task::none();
                };
                let Some(instance) = self.instances.iter().find(|i| i.id == id).cloned() else {
                    return Task::none();
                };
                let include: Vec<String> = rows.into_iter().filter(|r| r.1).map(|r| r.0).collect();
                self.status = Status::Busy("Exporting modpack".into());

                Task::perform(
                    async move {
                        let Some(handle) = rfd::AsyncFileDialog::new()
                            .set_title("Export as a Modrinth modpack")
                            .set_file_name(format!("{}.mrpack", instance.id))
                            .add_filter("Modrinth modpack", &["mrpack"])
                            .save_file()
                            .await
                        else {
                            return Ok(String::new());
                        };

                        core.mrpack
                            .export(&instance, handle.path(), &include)
                            .await
                            .map_err(|e| e.to_string())?;
                        Ok(format!("Exported {}", instance.name))
                    },
                    Message::PackImported,
                )
            }

            Message::PackImported(Ok(message)) => {
                self.status = Status::Idle;
                if !message.is_empty() {
                    tracing::info!("{message}");
                }
                self.core.clone().map(reload).unwrap_or_else(Task::none)
            }
            Message::PackImported(Err(err)) => {
                self.status = Status::Error(err);
                Task::none()
            }

            Message::Disarm(generation) => {
                if generation == self.armed_generation {
                    self.armed = None;
                }
                Task::none()
            }

            Message::MigrateInstance(id, to) => {
                let Some(core) = self.core.clone() else {
                    return Task::none();
                };
                // One-way for the worlds: the first click only arms the button.
                let armed = Armed::Migrate(id.clone());
                if self.armed.as_ref() != Some(&armed) {
                    return self.arm(armed);
                }
                self.armed = None;
                self.status = Status::Busy(format!("Migrating to {to}"));
                Task::perform(
                    async move {
                        let report = core
                            .migrate_instance(&id, &to)
                            .await
                            .map_err(|e| e.to_string())?;
                        let mut summary = format!(
                            "Migrated to {to}: {} updated",
                            report.updated.len()
                        );
                        if !report.disabled.is_empty() {
                            summary += &format!(
                                "; disabled, no {to} build yet: {}",
                                report.disabled.join(", ")
                            );
                        }
                        if !report.failed.is_empty() {
                            summary += &format!(
                                "; couldn't check, disabled: {}",
                                report.failed.join(", ")
                            );
                        }
                        if !report.untouched.is_empty() {
                            summary += &format!("; left as-is: {}", report.untouched.join(", "));
                        }
                        if let Some(backup) = report.backup.as_ref().and_then(|b| b.file_name()) {
                            summary += &format!(
                                ". Old worlds and mods are in {}",
                                backup.to_string_lossy()
                            );
                        }
                        Ok(summary)
                    },
                    Message::InstanceMigrated,
                )
            }
            Message::InstanceMigrated(result) => {
                self.status = match result {
                    Ok(summary) => Status::Notice(summary),
                    Err(err) => Status::Error(format!("Migration failed: {err}")),
                };
                self.core.clone().map(reload).unwrap_or_else(Task::none)
            }
            Message::SyncOptions(id) => {
                let Some(core) = self.core.clone() else {
                    return Task::none();
                };
                self.status = Status::Busy("Syncing options".into());
                let running_now = self.instance_running(&id);

                let sync_id = id.clone();
                let sync_core = core.clone();
                let sync = Task::perform(
                    async move {
                        nexo_core::options_sync::sync_to_all(
                            &sync_core.paths,
                            &sync_core.instances,
                            &sync_id,
                        )
                        .await
                        .map(|n| (sync_id, n))
                        .map_err(|e| e.to_string())
                    },
                    Message::OptionsSynced,
                );

                // Still running: keep re-syncing every time this instance's
                // options.txt changes, until it closes. A one-shot sync
                // would otherwise miss anything changed after this click.
                if running_now {
                    let watch = Task::perform(
                        nexo_core::options_sync::watch_and_sync(
                            core.paths,
                            core.instances,
                            core.running,
                            id,
                        ),
                        |()| Message::OptionsWatchDone,
                    );
                    Task::batch([sync, watch])
                } else {
                    sync
                }
            }
            Message::OptionsSynced(Ok((id, count))) => {
                self.status = if count == 0 {
                    Status::Error(format!("\"{id}\" has no options.txt yet — play it once first"))
                } else {
                    Status::Idle
                };
                Task::none()
            }
            Message::OptionsSynced(Err(err)) => {
                self.status = Status::Error(err);
                Task::none()
            }
            Message::OptionsWatchDone => Task::none(),

            Message::LoadSavedSkins => {
                let Some(core) = self.core.clone() else {
                    return Task::none();
                };
                Task::perform(
                    async move { core.skins.list().await.unwrap_or_default() },
                    Message::SavedSkinsLoaded,
                )
            }

            Message::SavedSkinsLoaded(skins) => {
                let mut fetches = Vec::new();
                for saved in &skins {
                    if self.saved_textures.contains_key(&saved.id) {
                        continue;
                    }
                    let Some(core) = self.core.clone() else {
                        continue;
                    };
                    let (id, model) = (saved.id.clone(), saved.model);

                    fetches.push(Task::perform(
                        async move {
                            let bytes = core.skins.read(&id).await.ok()?;
                            let mut decoded = skin::Skin::decode(&bytes, model).ok()?;
                            // The stored model can be wrong; the texture
                            // can't be, and slim arms drawn as classic come
                            // out full of holes.
                            let model = decoded.use_detected_model();
                            Some((id, Arc::new(decoded.texture().clone()), model))
                        },
                        |loaded| match loaded {
                            Some((skin, texture, model)) => Message::SkinTextureLoaded { skin, texture, model },
                            None => Message::Noop,
                        },
                    ));
                }

                self.saved_skins = skins;
                Task::batch(fetches)
            }

            Message::SkinTextureLoaded { skin, texture, model } => {
                self.saved_textures.insert(skin, (texture, model));
                Task::none()
            }

            Message::HistoryQuery(query) => {
                self.history_query = query;
                Task::none()
            }

            Message::HistorySearch => {
                let (Some(core), false) = (self.core.clone(), matches!(self.history, screens::history::State::Loading)) else {
                    return Task::none();
                };
                self.history = screens::history::State::Loading;
                let name = self.history_query.trim().to_string();
                Task::perform(screens::history::load(core, name), Message::HistoryLoaded)
            }

            Message::HistoryLoaded(result) => {
                self.history = match result {
                    Ok(loaded) => screens::history::State::Ready(loaded),
                    Err(message) => screens::history::State::Failed(message),
                };
                Task::none()
            }

            Message::SkinQuery(query) => {
                self.skin_query = query;
                // A rename the search hides would have no way to finish.
                self.renaming_skin = None;
                Task::none()
            }

            Message::RenameSkin(id) => {
                let current = self.saved_skins.iter().find(|s| s.id == id).and_then(|s| s.name.clone()).unwrap_or_default();
                self.renaming_skin = Some((id, current));
                Task::none()
            }

            Message::RenameDraft(draft) => {
                if let Some((_, name)) = &mut self.renaming_skin {
                    *name = draft.chars().take(nexo_core::skin_library::NAME_LIMIT).collect();
                }
                Task::none()
            }

            Message::RenameDone => {
                let Some(core) = self.core.clone() else { return Task::none() };
                let Some((id, name)) = self.renaming_skin.take() else { return Task::none() };
                // Shown at once; the list read back afterwards says the same.
                if let Some(saved) = self.saved_skins.iter_mut().find(|s| s.id == id) {
                    saved.name = nexo_core::skin_library::clean_name(&name);
                }
                Task::perform(
                    async move {
                        if let Err(err) = core.skins.rename(&id, &name).await {
                            tracing::warn!(%err, "could not rename the skin");
                        }
                    },
                    |()| Message::LoadSavedSkins,
                )
            }

            Message::ContinueWithoutAccount => {
                self.welcome_dismissed = true;
                Task::none()
            }

            Message::UnhoverSkin(id) => {
                // Only if nothing else has been entered since: leaving one tile
                // and entering the next can arrive in either order.
                if self.hovered_skin.as_deref() == Some(id.as_str()) {
                    self.hovered_skin = None;
                }
                Task::none()
            }

            Message::HoverSkin(id) => {
                self.hovered_skin = id;
                Task::none()
            }

            Message::AskDeleteSkin(id) => {
                self.confirm_delete = Some(id);
                Task::none()
            }

            Message::CancelDeleteSkin => {
                self.confirm_delete = None;
                Task::none()
            }

            Message::ConfirmDeleteSkin(id) => {
                self.confirm_delete = None;
                self.saved_textures.remove(&id);
                let Some(core) = self.core.clone() else {
                    return Task::none();
                };
                Task::perform(
                    async move {
                        let _ = core.skins.remove(&id).await;
                    },
                    |()| Message::LoadSavedSkins,
                )
            }

            Message::WearSavedSkin(id) => {
                let (Some(core), Some(account)) =
                    (self.core.clone(), self.active_account().cloned())
                else {
                    return Task::none();
                };
                // The model read off the texture, which is what its tile says;
                // the stored one came from the profile and can be wrong.
                let model = self
                    .saved_textures
                    .get(&id)
                    .map(|(_, model)| *model)
                    .or_else(|| self.saved_skins.iter().find(|s| s.id == id).map(|s| s.model))
                    .unwrap_or_default();
                self.status = Status::Busy("Changing skin".into());

                Task::perform(
                    async move {
                        // Uploaded from the stored file rather than by URL:
                        // these are local copies with nowhere to link to.
                        core.cosmetics
                            .upload_skin(&account, &core.skins.png_path(&id), model)
                            .await
                            .map_err(|e| e.to_string())
                    },
                    Message::CosmeticsDone,
                )
            }

            Message::LoadCapes => {
                let Some(core) = self.core.clone() else {
                    return Task::none();
                };
                let Some(account) = self.active_account().cloned() else {
                    return Task::none();
                };

                Task::perform(
                    async move {
                        // A stale token reads no capes and looks identical to
                        // owning none, so renew before asking. `account` is
                        // only used to prove somebody is signed in.
                        let _ = &account;
                        let valid = core
                            .accounts
                            .active_valid(&core.auth)
                            .await
                            .map_err(|e| e.to_string())?;
                        core.cosmetics
                            .capes(&valid)
                            .await
                            .map_err(|e| e.to_string())
                    },
                    Message::CapesLoaded,
                )
            }

            Message::CapesLoaded(Err(err)) => {
                // Deliberately keeps whatever list is already shown. Blanking
                // it claimed the account owns no capes, which is a different
                // and wrong statement — and the change of shape reset the 3D
                // viewer's pose along with it.
                self.status = Status::Error(format!("Could not read capes: {err}"));
                Task::none()
            }

            Message::CapesLoaded(Ok(capes)) => {
                // Fetch previews for any cape not already cached, one task
                // each so a slow texture doesn't hold up the others.
                let mut fetches = Vec::new();
                for cape in &capes {
                    if self.cape_textures.contains_key(&cape.id) {
                        continue;
                    }
                    let Some(core) = self.core.clone() else {
                        continue;
                    };
                    let (id, url) = (cape.id.clone(), cape.url.clone());

                    fetches.push(Task::perform(
                        async move {
                            nexo_core::skin::fetch_texture(core.http(), &url)
                                .await
                                .ok()
                                .map(|texture| (id, texture))
                        },
                        |loaded| match loaded {
                            Some((cape, texture)) => Message::CapeTextureLoaded { cape, texture: Arc::new(texture) },
                            None => Message::Noop,
                        },
                    ));
                }

                self.capes = capes;
                Task::batch(fetches)
            }

            Message::FaceLoaded { account, handle } => {
                self.faces.insert(account, handle);
                Task::none()
            }

            Message::CapeTextureLoaded { cape, texture } => {
                self.cape_textures.insert(cape, texture);
                Task::none()
            }

            Message::UploadSkin => {
                let Some(core) = self.core.clone() else {
                    return Task::none();
                };
                let Some(account) = self.active_account().cloned() else {
                    return Task::none();
                };
                self.status = Status::Busy("Uploading skin".into());

                Task::perform(
                    async move {
                        let Some(handle) = rfd::AsyncFileDialog::new()
                            .set_title("Choose a skin")
                            .add_filter("Minecraft skin", &["png"])
                            .pick_file()
                            .await
                        else {
                            // Cancelled, which is not a failure.
                            return Ok(());
                        };
                        // The arm width is read off the texture, the way the
                        // library and the 3D model already do, rather than asked.
                        let model = tokio::fs::read(handle.path())
                            .await
                            .ok()
                            .and_then(|bytes| skin::Skin::decode(&bytes, nexo_core::SkinModel::Classic).ok())
                            .map_or(nexo_core::SkinModel::Classic, |decoded| decoded.detect_model());

                        core.cosmetics
                            .upload_skin(&account, handle.path(), model)
                            .await
                            .map_err(|e| e.to_string())
                    },
                    Message::CosmeticsDone,
                )
            }

            Message::ResetSkin => {
                let (Some(core), Some(account)) =
                    (self.core.clone(), self.active_account().cloned())
                else {
                    return Task::none();
                };
                self.status = Status::Busy("Resetting skin".into());

                Task::perform(
                    async move {
                        core.cosmetics
                            .reset_skin(&account)
                            .await
                            .map_err(|e| e.to_string())
                    },
                    Message::CosmeticsDone,
                )
            }

            Message::WearCape(cape) => {
                let (Some(core), Some(account)) =
                    (self.core.clone(), self.active_account().cloned())
                else {
                    return Task::none();
                };
                self.status = Status::Busy("Changing cape".into());

                Task::perform(
                    async move {
                        core.cosmetics
                            .wear_cape(&account, &cape)
                            .await
                            .map_err(|e| e.to_string())
                    },
                    Message::CosmeticsDone,
                )
            }

            Message::HideCape => {
                let (Some(core), Some(account)) =
                    (self.core.clone(), self.active_account().cloned())
                else {
                    return Task::none();
                };
                self.status = Status::Busy("Removing cape".into());

                Task::perform(
                    async move {
                        core.cosmetics
                            .hide_cape(&account)
                            .await
                            .map_err(|e| e.to_string())
                    },
                    Message::CosmeticsDone,
                )
            }

            Message::CosmeticsDone(Ok(())) => {
                self.status = Status::Idle;
                let reload_capes = Task::done(Message::LoadCapes);
                let Some(core) = self.core.clone() else {
                    return reload_capes;
                };

                // `reload` alone is not enough: it only re-fetches the skin
                // when the *active account* changes, and putting a cape on
                // doesn't change which account is active — so the 3D textures
                // were never refreshed. Ask for them explicitly. `load_skin`
                // re-reads the profile itself, so it picks up the cape that
                // just landed server-side.
                let account = self.active_account().cloned();
                Task::batch([reload(core.clone()), reload_capes, load_skin(core, account)])
            }
            Message::CapeTab(tab) => {
                self.cape_tab = tab;
                self.cape_delete_armed = None;
                self.cape_report_armed = None;
                if tab == screens::capes::CapeTab::Mine {
                    self.mark_capes_seen();
                }
                if tab == screens::capes::CapeTab::Review {
                    return self.load_cape_review();
                }
                Task::none()
            }
            Message::CapeQuery(query) => {
                self.cape_query = query;
                Task::none()
            }
            Message::LoadNexoCapes => {
                let Some(core) = self.core.clone() else {
                    return Task::none();
                };
                // The capes dev screen shows fake lists; the real service would replace them.
                if cfg!(debug_assertions) && std::env::var("NEXO_DEV_SCREEN").is_ok_and(|s| s.starts_with("capes")) {
                    return Task::none();
                }
                let sort = self.cape_sort;
                let catalog = Task::perform(
                    { let core = core.clone(); async move { core.nexo_capes.catalog(sort).await.map_err(|e| e.to_string()) } },
                    move |result| Message::NexoCatalogLoaded(sort, result),
                );
                let Some(uuid) = self.active_account.clone() else {
                    return catalog;
                };
                let mine = Task::perform(
                    with_valid_account(core, |core, account| async move { core.nexo_capes.me(&account).await }),
                    move |result| Message::NexoMineLoaded(uuid.clone(), result),
                );
                Task::batch([catalog, mine])
            }
            Message::NexoCatalogLoaded(sort, _) if sort != self.cape_sort => Task::none(),
            Message::NexoCatalogLoaded(_, Ok(capes)) => {
                self.capes_catalog_error = None;
                // ponytail: the first 60 up front, which is several screens of tiles; fetch
                // per visible tile (and page the catalog) once there are hundreds of capes.
                let fetch = self.fetch_nexo_textures(capes.iter().take(60), false);
                self.capes_catalog = capes;
                fetch
            }
            Message::NexoCatalogLoaded(_, Err(err)) => {
                // Keeps whatever list is shown: a failed refresh is not "no capes".
                self.capes_catalog_error = Some(err);
                Task::none()
            }
            Message::NexoMineLoaded(uuid, Ok(mine)) => {
                self.capes_mine_error = None;
                self.share_capes(&uuid, &mine);
                // An answer for an account no longer active (an upload that finished after a switch)
                // is written to that account's file and goes no further.
                if Some(uuid.as_str()) != self.active_account.as_deref() {
                    return Task::none();
                }
                self.note_cape_decisions(&mine);
                self.nexo_worn_hint = Some((uuid.clone(), mine.worn.clone()));
                let worn_fetch = self.fetch_worn_cape();
                let fetch = self.fetch_nexo_textures(mine.mine.iter(), true);
                let review = mine.admin;
                self.capes_mine_for = Some((uuid, mine));
                if review { Task::batch([fetch, worn_fetch, self.load_cape_review()]) } else { Task::batch([fetch, worn_fetch]) }
            }
            Message::NexoMineLoaded(_, Err(err)) => {
                self.capes_mine_error = Some(format!("Could not sign in to the cape service: {err}"));
                Task::none()
            }
            Message::NexoReviewLoaded(uuid, Ok(queue)) => {
                let fetch = Task::batch([self.fetch_nexo_textures(queue.pending.iter(), true), self.fetch_nexo_textures(queue.approved.iter(), false)]);
                self.capes_review_for = Some((uuid, queue));
                fetch
            }
            Message::NexoReviewLoaded(_, Err(err)) => {
                self.status = Status::Error(format!("Could not read the review queue: {err}"));
                Task::none()
            }
            Message::NexoCapeTexture(id, texture) => {
                self.nexo_cape_textures.insert(id, texture);
                Task::none()
            }
            Message::UploadNexoCape => Task::perform(
                async move {
                    let handle = rfd::AsyncFileDialog::new().set_title("Choose a cape").add_filter("Cape PNG", &["png"]).pick_file().await?;
                    let path = handle.path().to_path_buf();
                    let (bytes, texture) = check_cape_file(&path).await;
                    Some((path, Arc::new(bytes), texture.map(Arc::new)))
                },
                Message::CapeDraftPicked,
            ),
            Message::CapeDraftPicked(None) => Task::none(),
            // A second dialog's answer while the first cape is on its way: that one finishes first.
            Message::CapeDraftPicked(Some(_)) if self.cape_draft.as_ref().is_some_and(|d| d.sending) => Task::none(),
            Message::CapeDraftPicked(Some((path, bytes, texture))) => {
                let Some(account) = self.active_account.clone() else { return Task::none() };
                // Named after its file to start with; the popup is where it gets its real name.
                let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                let name: String = stem.chars().filter(|c| !c.is_control()).take(32).collect();
                self.cape_draft = Some(CapeDraft { path, account, bytes, texture, name: name.trim().to_string(), sending: false, error: None });
                self.cape_tab = screens::capes::CapeTab::Mine;
                iced::widget::operation::focus(screens::capes::NAME_FIELD)
            }
            Message::CapeDraftName(name) => {
                if let Some(draft) = self.cape_draft.as_mut() {
                    draft.name = name.chars().take(32).collect();
                    draft.error = None;
                }
                Task::none()
            }
            Message::CapeDraftCancel => {
                // Not while it is on its way: the answer still has to land somewhere.
                if self.cape_draft.as_ref().is_some_and(|d| !d.sending) {
                    self.cape_draft = None;
                }
                Task::none()
            }
            Message::CapeDraftSend => {
                let (Some(core), Some(uuid)) = (self.core.clone(), self.active_account.clone()) else {
                    return Task::none();
                };
                let Some(draft) = self.cape_draft.as_mut().filter(|d| d.texture.is_ok() && !d.name.trim().is_empty() && !d.sending) else {
                    return Task::none();
                };
                if draft.account != uuid {
                    draft.error = Some("You switched accounts since choosing this file. Choose it again for this account.".into());
                    return Task::none();
                }
                draft.sending = true;
                draft.error = None;
                let (name, bytes) = (draft.name.trim().to_string(), draft.bytes.as_ref().clone());
                Task::perform(
                    with_valid_account(core, move |core, account| async move {
                        // Refused rather than sent to whoever is signed in by the time the token is renewed.
                        if account.uuid != uuid {
                            return Err(nexo_core::Error::invalid("the account changed while uploading"));
                        }
                        core.nexo_capes.upload(&account, &name, bytes).await.map(|mine| (account.uuid, mine))
                    }),
                    |result| match result {
                        Ok((uuid, mine)) => Message::CapeDraftSent(uuid, Ok(mine)),
                        Err(err) => Message::CapeDraftSent(String::new(), Err(err)),
                    },
                )
            }
            Message::CapeDraftSent(uuid, Ok(mine)) => {
                let name = self.cape_draft.take().map(|d| d.name).unwrap_or_default();
                self.status = Status::Notice(format!("“{}” is uploaded and waiting for review.", name.trim()));
                Task::done(Message::NexoMineLoaded(uuid, Ok(mine)))
            }
            Message::CapeDraftSent(_, Err(err)) => {
                // The popup stays, with the reason, so the name or the file can be fixed.
                if let Some(draft) = self.cape_draft.as_mut() {
                    draft.sending = false;
                    draft.error = Some(err);
                }
                Task::none()
            }
            Message::WearNexoCape(cape) => self.cape_change(move |core, account| async move { core.nexo_capes.wear(&account, cape.as_deref()).await }),
            Message::FavoriteNexoCape(cape, on) => self.cape_change(move |core, account| async move { core.nexo_capes.favorite(&account, &cape, on).await }),
            Message::ArmCapeDelete(cape) => {
                self.cape_delete_armed = cape;
                Task::none()
            }
            Message::DeleteNexoCape(cape) => {
                self.cape_delete_armed = None;
                self.cape_change(move |core, account| async move { core.nexo_capes.delete(&account, &cape).await })
            }
            Message::ReviewNexoCape(cape, approve) => {
                let (Some(core), Some(uuid)) = (self.core.clone(), self.active_account.clone()) else {
                    return Task::none();
                };
                Task::perform(
                    with_valid_account(core, move |core, account| async move { core.nexo_capes.review(&account, &cape, approve).await }),
                    move |result| Message::NexoReviewLoaded(uuid.clone(), result),
                )
                .chain(Task::done(Message::LoadNexoCapes))
            }
            Message::CapeSort(sort) => {
                self.cape_sort = sort;
                let Some(core) = self.core.clone() else { return Task::none() };
                Task::perform(async move { core.nexo_capes.catalog(sort).await.map_err(|e| e.to_string()) }, move |result| Message::NexoCatalogLoaded(sort, result))
            }
            Message::CapeCode(code) => {
                self.cape_code = code;
                Task::none()
            }
            Message::WearByCode => {
                let code = self.cape_code.clone();
                self.cape_change(move |core, account| async move {
                    let cape = core.nexo_capes.by_code(&code).await?;
                    core.nexo_capes.wear(&account, Some(&cape.id)).await
                })
            }
            Message::ArmCapeReport(cape) => {
                self.cape_report_armed = cape;
                Task::none()
            }
            Message::ReportNexoCape(cape) => {
                self.cape_report_armed = None;
                let Some(core) = self.core.clone() else { return Task::none() };
                Task::perform(
                    with_valid_account(core, move |core, account| async move { core.nexo_capes.report(&account, &cape).await }),
                    |result| Message::CapeNotice(result.map(|()| "Reported. A reviewer will look at it.".to_string())),
                )
            }
            Message::CopyCapeCode(code) => {
                self.status = Status::Notice(format!("Copied cape code {code}. A friend can paste it into Wear code."));
                iced::clipboard::write(code)
            }
            Message::CapeNotice(Ok(text)) => {
                self.status = Status::Notice(text);
                Task::none()
            }
            Message::CapeNotice(Err(err)) => {
                self.status = Status::Error(err);
                Task::none()
            }
            Message::CapesShareTick => {
                let (Some(core), Some(uuid)) = (self.core.as_ref(), self.active_account.clone()) else { return Task::none() };
                let modified = nexo_core::nexo_capes::shared::modified(core.paths.root(), &uuid);
                // The first look at an account's file only learns its time; after that a change came from the game.
                let changed = matches!(&self.capes_share_seen, Some((seen, time)) if *seen == uuid && *time != modified);
                self.capes_share_seen = Some((uuid, modified));
                if changed { Task::done(Message::LoadNexoCapes) } else { Task::none() }
            }
            Message::OpenCapeTemplate => {
                if let Err(err) = open::that_detached(nexo_core::nexo_capes::TEMPLATE_URL) {
                    self.status = Status::Error(format!("Could not open the template: {err}"));
                }
                Task::none()
            }
            Message::CosmeticsDone(Err(err)) => {
                self.status = Status::Error(err);
                Task::none()
            }

            Message::Launch(id, account) => {
                let Some(core) = self.core.clone() else {
                    return Task::none();
                };
                let Some(uuid) = account.or_else(|| self.active_account.clone()) else {
                    return Task::none();
                };
                // The server would kick a second client on the same account;
                // refuse here too, since a failed launch ends in GameExited,
                // which would otherwise clear the *live* session's entry.
                let key = nexo_core::running::session_key(&id, &uuid);
                if self.running.contains(&key) {
                    self.status =
                        Status::Error("That account is already playing this instance.".into());
                    return Task::none();
                }
                self.status = Status::Busy("Preparing to launch".into());

                // Progress flows back over a channel while the install runs,
                // so the window keeps painting instead of freezing for the
                // duration of a ~1,000-file download.
                let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

                let progress = Task::run(
                    futures::stream::unfold(rx, |mut rx| async move {
                        rx.recv().await.map(|item| (item, rx))
                    }),
                    Message::LaunchProgress,
                );

                // Marked running up front so the button flips immediately
                // rather than after the install finishes. Any failure path
                // still ends in GameExited, which clears it again.
                self.running.insert(key.clone());
                self.preparing += 1;
                self.stopped_by_user.remove(&key);
                let (started, game_up) = tokio::sync::oneshot::channel();
                let after = screens::settings::after_launch_soon(key.clone(), game_up);

                let run = Task::perform(
                    async move {
                        // Spawned so a panic inside play_as still ends in
                        // Failed + GameExited instead of a stuck button.
                        let c2 = core.clone();
                        let (id2, uuid2, tx2) = (id.clone(), uuid.clone(), tx.clone());
                        let res = tokio::spawn(async move { c2.play_as(&id2, Some(&uuid2), Some(&tx2)).await })
                            .await
                            .unwrap_or_else(|e| Err(nexo_core::Error::invalid(format!("launch crashed: {e}"))));
                        if let Err(err) = res {
                            let _ = tx.send(Progress::Failed(err.to_string()));
                        } else {
                            let _ = tx.send(Progress::Done);
                            let _ = started.send(());
                            // Resolves when the JVM exits, however that
                            // happens — quit from the menu, crash, or Stop.
                            core.running.wait_for_exit(&key).await;
                        }
                        key
                    },
                    Message::GameExited,
                );

                Task::batch([progress, run, after])
            }

            Message::LaunchProgress(progress) => {
                match progress {
                    Progress::Stage(stage) => self.status = Status::Busy(stage),
                    Progress::Advanced { completed, total } if total > 0 => {
                        let percent = (completed * 100) / total;
                        self.status = Status::Busy(format!(
                            "Downloading files — {completed}/{total} ({percent}%)"
                        ));
                    }
                    Progress::Advanced { .. } => {}
                    Progress::Done => {
                        self.preparing = self.preparing.saturating_sub(1);
                        if self.is_busy() && self.preparing == 0 {
                            self.status = Status::Idle;
                        }
                        return self.core.clone().map(reload).unwrap_or_else(Task::none);
                    }
                    Progress::Failed(err) => {
                        self.preparing = self.preparing.saturating_sub(1);
                        if err == nexo_core::running::LAUNCH_CANCELLED {
                            // A Stop during prepare: quiet, not an error.
                            if self.is_busy() && self.preparing == 0 {
                                self.status = Status::Idle;
                            }
                        } else {
                            self.status = Status::Error(err);
                            // Nobody reads an error on a hidden window.
                            return screens::settings::restore(self);
                        }
                    }
                }
                Task::none()
            }

            Message::StartSignIn => {
                let Some(core) = self.core.clone() else {
                    return Task::none();
                };
                if self.signing_in {
                    return Task::none();
                }

                self.signing_in = true;
                self.status = Status::Busy("Waiting for you to sign in in your browser".into());

                Task::perform(
                    async move {
                        let account = core
                            .auth
                            // The callback fires once the loopback listener
                            // is bound, so the browser can't beat us to the
                            // redirect.
                            .login(|url| {
                                if let Err(err) = open::that_detached(url) {
                                    tracing::warn!(%err, "couldn't open a browser");
                                }
                            })
                            .await
                            .map_err(|e| e.to_string())?;

                        core.accounts
                            .upsert(account.clone())
                            .await
                            .map_err(|e| e.to_string())?;
                        Ok(account)
                    },
                    Message::SignInFinished,
                )
            }

            Message::SignInFinished(Ok(account)) => {
                self.signing_in = false;
                self.status = Status::Idle;
                tracing::info!(user = %account.username, "signed in");
                self.core.clone().map(reload).unwrap_or_else(Task::none)
            }
            Message::SignInFinished(Err(err)) => {
                self.signing_in = false;
                self.status = Status::Error(err);
                Task::none()
            }

            Message::SetActiveAccount(uuid) => {
                // A chosen file belongs to the account it was chosen for.
                if self.cape_draft.as_ref().is_some_and(|d| !d.sending) {
                    self.cape_draft = None;
                }
                let Some(core) = self.core.clone() else {
                    return Task::none();
                };
                Task::perform(
                    async move {
                        core.accounts
                            .set_active(&uuid)
                            .await
                            .map_err(|e| e.to_string())
                    },
                    Message::AccountActionDone,
                )
            }

            Message::RemoveAccount(uuid) => {
                let Some(core) = self.core.clone() else {
                    return Task::none();
                };
                Task::perform(
                    async move {
                        write_skin_cache(&core, &uuid, "png", None).await;
                        write_skin_cache(&core, &uuid, "cape.png", None).await;
                        core.accounts.remove(&uuid).await.map_err(|e| e.to_string())
                    },
                    Message::AccountActionDone,
                )
            }

            Message::AccountActionDone(Ok(())) => {
                self.core.clone().map(reload).unwrap_or_else(Task::none)
            }
            Message::AccountActionDone(Err(err)) => {
                self.status = Status::Error(err);
                Task::none()
            }

            Message::SkinLoaded {
                uuid,
                face,
                skin,
                cape,
                model,
            } => {
                if uuid.is_some() && uuid != self.active_account {
                    return Task::none();
                }
                if let Some(uuid) = self.active_account.clone() {
                    self.faces.insert(uuid, face);
                }
                self.skin_texture = Some(skin);
                self.cape_texture = cape;
                self.skin_model = model;
                // Distinct from the last set, so the renderer re-uploads.
                self.skin_key = self.skin_key.wrapping_add(1);
                Task::none()
            }
        }
    }

    fn view(&self) -> Element<'_, Message> {
        // The first start, and anyone who signed out of every account. Not
        // before the list is read, or it would flash up for everyone.
        if self.accounts_loaded && self.accounts.is_empty() && !self.welcome_dismissed {
            return screens::welcome::view(self);
        }
        let body = match &self.screen {
            Screen::Home => screens::home::view(self),
            Screen::Instances => screens::instances::view(self),
            Screen::Accounts => screens::accounts::view(self),
            Screen::Skins => screens::skins::view(self),
            Screen::Capes => screens::capes::view(self),
            Screen::History => screens::history::view(self),
            Screen::Servers => screens::hosting::list_view(self),
            Screen::Settings => screens::settings::view(self),
            Screen::NewServer => screens::server_wizard::view(self),
            Screen::Server(id) => match self.host.server(id) {
                Some(server) => screens::server_dashboard::view(self, server),
                None => empty_state("That server is gone", "It was deleted, or its folder was removed."),
            },
            Screen::Instance(id) => match self.instances.iter().find(|i| &i.id == id) {
                Some(instance) => screens::instance::view(self, instance),
                // Deleted from under us, or an id that no longer exists.
                None => empty_state(
                    "That instance is gone",
                    "It was deleted, or its folder was removed outside the launcher.",
                ),
            },
            Screen::PaperConsole(id) => match self.paper_servers.iter().find(|s| &s.id == id) {
                Some(server) => screens::paper_console::view(self, server),
                None => empty_state(
                    "That server is gone",
                    "It was stopped and reverted, or its record was removed.",
                ),
            },
        };

        let page = self.motion.get(motion_keys::PAGE, 1.0);
        let on_home = self.screen == Screen::Home;
        let content = iced::widget::column![
            screens::top_bar(self),
            // In a column of its own, always there. A column drops zero-sized children
            // entirely, so an empty bar here would move the page one place along
            // whenever a banner came up, and iced would rebuild it as a new widget:
            // every list on the page lost its scroll position.
            if matches!(self.status, Status::Idle) {
                iced::widget::column![iced::widget::Space::new().height(16)]
            } else {
                iced::widget::column![
                    iced::widget::Space::new().height(16),
                    screens::reveal(screens::status_bar(&self.status), self.motion.get(motion_keys::STATUS, 1.0), 0.0),
                    iced::widget::Space::new().height(16),
                ]
            },
            // The page finishes before the list stagger that shares its clock.
            screens::reveal(body, if on_home { 1.0 } else if self.page_within { 1.0 } else { (page * 1.6).min(1.0) }, self.page_rise),
        ]
        .padding(24)
        .width(Fill)
        .height(Fill);

        // Behind everything right of the rail, not just the page, so it has no edge
        // of its own. Always a stack, so leaving Home doesn't rebuild the page's widgets.
        let backdrop: Element<'_, Message> = if on_home {
            backdrop::view(self.effect, self.particles, self.focused, Fill)
        } else {
            iced::widget::Space::new().width(Fill).height(Fill).into()
        };

        // Home arrives under one veil across all of it, because a veil on just the
        // page would show as a dark box against the background.
        let arriving = if on_home && !self.page_within { 1.0 - (page * 1.6).min(1.0) } else { 0.0 };
        let veil = iced::widget::container(iced::widget::Space::new()).width(Fill).height(Fill).style(theme::veil(arriving));

        iced::widget::row![screens::sidebar(self), iced::widget::stack![backdrop, content, veil]]
            .height(Fill)
            .into()
    }

    /// How far a tab's content has come in, for the screens with a tab strip.
    pub fn tab_content_progress(&self) -> f32 {
        if self.page_within {
            (self.motion.get(motion_keys::PAGE, 1.0) * 1.6).min(1.0)
        } else {
            1.0
        }
    }

    /// Arms a one-way button and schedules its disarm. Five seconds is long
    /// enough to read the relabelled button and click again, short enough
    /// that an arm forgotten about is gone before it can surprise anyone.
    fn arm(&mut self, armed: Armed) -> Task<Message> {
        // Replacing the previous arm is the point: arming Delete disarms a
        // pending Migrate, so only one confirmation is ever live.
        self.armed = Some(armed);
        self.armed_generation = self.armed_generation.wrapping_add(1);
        let generation = self.armed_generation;
        Task::perform(
            tokio::time::sleep(std::time::Duration::from_secs(5)),
            move |_| Message::Disarm(generation),
        )
    }

    /// Whether `armed` is the one waiting for its second click.
    pub fn is_armed(&self, armed: &Armed) -> bool {
        self.armed.as_ref() == Some(armed)
    }

    /// The accent, for the handful of places that colour a piece of text
    /// directly, where there is no style function to read it out of.
    pub fn accent(&self) -> iced::Color {
        theme::accent()
    }

    /// Whether any session of the instance is running.
    /// Has Nexo Mod installed, older than the newest release.
    fn nexo_mod_outdated(&self, instance: &Instance) -> bool {
        match (&self.nexo_release, nexo_core::nexo_mod::installed(instance)) {
            (Some(release), Some(installed)) => {
                nexo_core::nexo_mod::is_newer(&release.manifest.mod_version, &installed.version_number)
            }
            _ => false,
        }
    }

    /// `target` is a later release than `current`, by Mojang's own ordering
    /// (the list is newest first). Unknown versions count as not newer.
    fn is_newer_version(&self, target: &str, current: &str) -> bool {
        let position = |id: &str| self.game_versions.iter().position(|v| v == id);
        matches!((position(target), position(current)), (Some(t), Some(c)) if t < c)
    }

    fn instance_running(&self, id: &str) -> bool {
        !self.instance_sessions(id).is_empty()
    }

    /// (session key, account name) for each running session of the instance.
    fn instance_sessions(&self, id: &str) -> Vec<(String, String)> {
        let mut out: Vec<_> = self
            .running
            .iter()
            .filter_map(|key| {
                let (inst, uuid) = nexo_core::running::split_session_key(key)?;
                (inst == id).then(|| {
                    let name = self.accounts.iter().find(|a| a.uuid == uuid);
                    (key.clone(), name.map_or(uuid, |a| &a.username).to_string())
                })
            })
            .collect();
        out.sort();
        out
    }

    /// The account launches will use.
    fn active_account(&self) -> Option<&Account> {
        let uuid = self.active_account.as_deref()?;
        self.accounts.iter().find(|a| a.uuid == uuid)
    }

    /// This account's custom-cape state, if it has been read for the account now signed in.
    fn capes_mine(&self) -> Option<&nexo_core::nexo_capes::Mine> {
        let (uuid, mine) = self.capes_mine_for.as_ref()?;
        (Some(uuid.as_str()) == self.active_account.as_deref()).then_some(mine)
    }

    fn capes_review(&self) -> Option<&nexo_core::nexo_capes::ReviewQueue> {
        let (uuid, queue) = self.capes_review_for.as_ref()?;
        (Some(uuid.as_str()) == self.active_account.as_deref()).then_some(queue)
    }

    /// Writes what this account wears and has waiting to the file the game watches,
    /// and remembers the file's new time so this window doesn't react to its own write.
    fn share_capes(&mut self, uuid: &str, mine: &nexo_core::nexo_capes::Mine) {
        let Some(core) = self.core.as_ref() else { return };
        let pending = mine.mine.iter().filter(|c| c.status.as_deref() == Some("pending")).map(|c| c.id.clone()).collect();
        match nexo_core::nexo_capes::shared::record(core.paths.root(), uuid, mine.worn.as_deref(), pending) {
            Ok(true) => self.capes_share_seen = Some((uuid.to_string(), nexo_core::nexo_capes::shared::modified(core.paths.root(), uuid))),
            Ok(false) => {}
            Err(err) => tracing::warn!(%err, "could not write the cape file shared with the game"),
        }
    }

    /// Points out uploads decided since the player last looked. The first time,
    /// with nothing on record, it only records: every old decision is not news.
    fn note_cape_decisions(&mut self, mine: &nexo_core::nexo_capes::Mine) {
        let decided = |c: &&nexo_core::nexo_capes::CustomCape| matches!(c.status.as_deref(), Some("approved" | "rejected"));
        if self.capes_seen.is_none() {
            let stored = self.core.as_ref().and_then(|core| nexo_core::nexo_capes::shared::read_seen(core.paths.root()));
            match stored {
                Some(seen) => self.capes_seen = Some(seen),
                None => {
                    self.capes_seen = Some(mine.mine.iter().filter(decided).map(|c| (c.id.clone(), c.status.clone().unwrap_or_default())).collect());
                    self.save_capes_seen();
                    return;
                }
            }
        }
        let seen = self.capes_seen.as_ref().expect("filled above");
        let news: Vec<String> = mine
            .mine
            .iter()
            .filter(decided)
            .filter(|c| seen.get(&c.id) != c.status.as_ref())
            .map(|c| match c.status.as_deref() {
                Some("approved") => format!("Your cape “{}” was approved.", c.name),
                _ => format!("Your cape “{}” was rejected{}.", c.name, c.reason.as_deref().map(|r| format!(": {r}")).unwrap_or_default()),
            })
            .collect();
        if !news.is_empty() {
            self.status = Status::Notice(news.join(" "));
        }
        if self.cape_tab == screens::capes::CapeTab::Mine && self.screen == Screen::Capes {
            self.mark_capes_seen();
        }
    }

    fn mark_capes_seen(&mut self) {
        let Some((_, mine)) = self.capes_mine_for.as_ref() else { return };
        let decided: std::collections::BTreeMap<String, String> = mine
            .mine
            .iter()
            .filter(|c| matches!(c.status.as_deref(), Some("approved" | "rejected")))
            .map(|c| (c.id.clone(), c.status.clone().unwrap_or_default()))
            .collect();
        self.capes_seen.get_or_insert_with(Default::default).extend(decided);
        self.save_capes_seen();
    }

    fn save_capes_seen(&self) {
        if let (Some(core), Some(seen)) = (self.core.as_ref(), self.capes_seen.as_ref())
            && let Err(err) = nexo_core::nexo_capes::shared::write_seen(core.paths.root(), seen)
        {
            tracing::warn!(%err, "could not record which cape decisions were seen");
        }
    }

    /// The Nexo cape this account wears, as the service last said or else the shared file.
    fn worn_nexo_cape(&self) -> Option<&str> {
        if let Some(mine) = self.capes_mine() {
            return mine.worn.as_deref();
        }
        let (uuid, worn) = self.nexo_worn_hint.as_ref()?;
        (Some(uuid.as_str()) == self.active_account.as_deref()).then_some(worn.as_deref()).flatten()
    }

    /// The figure's cape on Home: the Nexo cape when one is worn and here, else the Mojang one.
    fn home_cape(&self) -> (Option<Arc<skin::Rgba>>, u64) {
        match self.worn_nexo_cape().and_then(|id| self.nexo_cape_textures.get(id).map(|t| (id, t))) {
            Some((id, texture)) => (Some(Arc::clone(texture)), skin3d::slot(&format!("home-cape:{id}"))),
            None => (self.cape_texture.clone(), 0),
        }
    }

    /// Downloads the worn Nexo cape's texture if it isn't here yet. Approved, so no sign-in.
    fn fetch_worn_cape(&self) -> Task<Message> {
        let (Some(core), Some(id)) = (self.core.clone(), self.worn_nexo_cape().map(str::to_string)) else {
            return Task::none();
        };
        if self.nexo_cape_textures.contains_key(&id) {
            return Task::none();
        }
        Task::perform(async move { core.nexo_capes.texture(None, &id).await.ok().map(|t| (id, t)) }, |loaded| match loaded {
            Some((id, texture)) => Message::NexoCapeTexture(id, Arc::new(texture)),
            None => Message::Noop,
        })
    }

    /// Whether an upload was decided since the player last opened Your capes.
    fn capes_news(&self) -> bool {
        let (Some(mine), Some(seen)) = (self.capes_mine(), self.capes_seen.as_ref()) else { return false };
        mine.mine.iter().any(|c| matches!(c.status.as_deref(), Some("approved" | "rejected")) && seen.get(&c.id) != c.status.as_ref())
    }

    /// A change to this account's capes, then the catalog again for its counts.
    fn cape_change<F, Fut>(&self, work: F) -> Task<Message>
    where
        F: FnOnce(nexo_core::Nexo, Account) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = nexo_core::Result<nexo_core::nexo_capes::Mine>> + Send + 'static,
    {
        let (Some(core), Some(uuid)) = (self.core.clone(), self.active_account.clone()) else {
            return Task::none();
        };
        let sort = self.cape_sort;
        let catalog = { let core = core.clone(); Task::perform(async move { core.nexo_capes.catalog(sort).await.map_err(|e| e.to_string()) }, move |result| Message::NexoCatalogLoaded(sort, result)) };
        Task::perform(with_valid_account(core, work), move |result| match result {
            Ok(mine) => Message::NexoMineLoaded(uuid.clone(), Ok(mine)),
            Err(err) => Message::CosmeticsDone(Err(err)),
        })
        .chain(catalog)
    }

    fn load_cape_review(&self) -> Task<Message> {
        let (Some(core), Some(uuid)) = (self.core.clone(), self.active_account.clone()) else {
            return Task::none();
        };
        if !self.capes_mine().is_some_and(|m| m.admin) {
            return Task::none();
        }
        Task::perform(
            with_valid_account(core, |core, account| async move { core.nexo_capes.review_queue(&account).await }),
            move |result| Message::NexoReviewLoaded(uuid.clone(), result),
        )
    }

    /// Textures for capes not drawn yet, one task each. `authed` for unreviewed
    /// ones, which only their owner and reviewers may download.
    fn fetch_nexo_textures<'a>(&self, capes: impl Iterator<Item = &'a nexo_core::nexo_capes::CustomCape>, authed: bool) -> Task<Message> {
        let Some(core) = self.core.clone() else {
            return Task::none();
        };
        let tasks: Vec<_> = capes
            .filter(|cape| !self.nexo_cape_textures.contains_key(&cape.id))
            .map(|cape| {
                let (core, id, authed) = (core.clone(), cape.id.clone(), authed && !cape.approved());
                Task::perform(
                    async move {
                        let texture = if authed {
                            with_valid_account(core, { let id = id.clone(); move |core, account| async move { core.nexo_capes.texture(Some(&account), &id).await } }).await.ok()
                        } else {
                            core.nexo_capes.texture(None, &id).await.ok()
                        };
                        texture.map(|texture| (id, texture))
                    },
                    |loaded| match loaded {
                        Some((id, texture)) => Message::NexoCapeTexture(id, Arc::new(texture)),
                        None => Message::Noop,
                    },
                )
            })
            .collect();
        Task::batch(tasks)
    }

    fn is_busy(&self) -> bool {
        self.other_work() || !self.install_queue.is_empty()
    }

    /// Busy with something an install has to wait out. Installs themselves
    /// don't count: the next one can always join the queue.
    fn other_work(&self) -> bool {
        matches!(self.status, Status::Busy(_))
    }

    /// Starts whatever is at the front of the install queue.
    fn next_install(&mut self) -> Task<Message> {
        let Some(QueuedInstall { instance, project, kind }) = self.install_queue.front().cloned() else {
            return Task::none();
        };
        let Some(core) = self.core.clone() else {
            return Task::none();
        };
        Task::perform(
            async move {
                let mut target = core.instances.get(&instance).await.map_err(|e| e.to_string())?;
                core.content.install_modrinth(&mut target, &project, kind).await.map_err(|e| e.to_string())?;
                core.instances.save(&target).await.map_err(|e| e.to_string())
            },
            Message::InstallDone,
        )
    }

    /// The instance the details screen is showing, if that is where we are.
    ///
    /// The tab messages carry no instance id: they are only reachable from
    /// this screen, and reading it here means a listing can never be applied
    /// to an instance other than the visible one — including when a slow
    /// directory walk lands after the user has already navigated away.
    fn open_instance_dir(&self) -> Option<std::path::PathBuf> {
        let Screen::Instance(id) = &self.screen else {
            return None;
        };
        Some(self.core.as_ref()?.paths.instance(id))
    }

    /// Drops everything the tabs hold. Called when an instance is opened,
    /// since all of it describes one instance's directory.
    fn reset_tabs(&mut self) {
        self.tab = screens::instance::Tab::Content;
        self.tabs_loaded.clear();
        self.browsing = false;
        self.content_query.clear();
        self.files.clear();
        self.files_at = std::path::PathBuf::new();
        self.files_error = None;
        self.worlds.clear();
        self.confirm_delete_world = None;
        self.confirm_convert_world = None;
        self.servers.clear();
        // Cleared too, so opening another instance re-pings rather than
        // showing this instance's results under the other one's addresses.
        self.server_status.clear();
        self.server_icons.clear();
        self.server_form = None;
        self.logs.clear();
        self.selected_log = None;
        self.log_text = None;
    }

    /// The number beside a tab's label, or `None` when there is nothing
    /// honest to put there yet.
    ///
    /// Content is the exception that is always known: it comes off the
    /// instance manifest, which is already in memory, where the other three
    /// have to read a directory first.
    fn tab_count(&self, tab: screens::instance::Tab, instance: &Instance) -> Option<usize> {
        use screens::instance::Tab;
        match tab {
            // Facts about the instance, not a list — nothing to count.
            Tab::Overview => None,
            Tab::Content => Some(instance.mods.len()),
            Tab::Files => self.tabs_loaded.contains(&tab).then_some(self.files.len()),
            // Both lists, since both live behind this one tab.
            Tab::Worlds => self
                .tabs_loaded
                .contains(&tab)
                .then_some(self.worlds.len() + self.servers.len()),
            Tab::Logs => self.tabs_loaded.contains(&tab).then_some(self.logs.len()),
        }
    }

    /// Whether the log on screen is one the game is writing to right now.
    ///
    /// Three things have to hold, and each rules out a way the indicator could
    /// lie: the Logs tab is what is on screen, the selected file is the open
    /// handle rather than a rotated archive, and *this* instance's game is the
    /// one running — another instance being up says nothing about this file.
    fn following_live_log(&self) -> bool {
        let Screen::Instance(id) = &self.screen else {
            return false;
        };
        self.tab == screens::instance::Tab::Logs
            && self.instance_running(id)
            && self
                .selected_log
                .as_ref()
                .and_then(|name| self.logs.iter().find(|l| &l.name == name))
                .is_some_and(screens::instance::is_live_file)
    }

    /// The animation clock, and the live log's re-read.
    ///
    /// Gated on focus, which is the whole reason this is a method and not a
    /// constant. A launcher left open behind a game would otherwise repaint 30
    /// times a second forever to animate a pulse nobody is looking at — on a
    /// laptop that is battery spent on an empty room. Returning an empty
    /// subscription lets iced park the event loop until something happens.
    fn subscription(&self) -> iced::Subscription<Message> {
        const FRAME: std::time::Duration = std::time::Duration::from_millis(33);
        const REREAD: std::time::Duration = std::time::Duration::from_secs(1);

        // Always listening, even unfocused — this is what turns the rest back
        // on, so it can never be the thing that got switched off.
        let focus = iced::event::listen_with(|event, _status, _window| match event {
            iced::Event::Window(iced::window::Event::Focused) => {
                Some(Message::WindowFocused(true))
            }
            iced::Event::Window(iced::window::Event::Unfocused) => {
                Some(Message::WindowFocused(false))
            }
            _ => None,
        });

        // Hosting's sampling keeps running unfocused; its own function decides.
        let hosting = screens::hosting::subscriptions(self, self.focused);
        let mut feeds = vec![focus];
        feeds.extend(hosting);

        // Only while something is moving, so a settled window paints nothing. Focused
        // or not: a transition left unfinished would hold its veil until refocus.
        if self.motion.active() {
            feeds.push(iced::window::frames().map(Message::Frame));
        }

        if !self.focused {
            return iced::Subscription::batch(feeds);
        }

        // Escape closes the cape upload popup, even while its name field has the focus (which
        // would otherwise take the first press), and only where the popup is shown.
        if self.cape_draft.is_some() && self.screen == Screen::Capes {
            feeds.push(iced::event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(iced::keyboard::Event::KeyPressed { key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape), .. }) => {
                    Some(Message::CapeDraftCancel)
                }
                _ => None,
            }));
        }

        // Escape closes the export popup.
        if self.export_pick.is_some() {
            feeds.push(iced::event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(iced::keyboard::Event::KeyPressed { key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape), .. }) => {
                    Some(Message::ExportCancel)
                }
                _ => None,
            }));
        }

        // A cape picked in the game shows here within a second. Only on this screen:
        // anywhere else the next visit reads it fresh.
        if self.screen == Screen::Capes {
            feeds.push(iced::time::every(REREAD).map(|_| Message::CapesShareTick));
        }

        // Widgets and the Home backdrop request their own frames. The live-log
        // pulse is the one animation on this clock, and it only moves on
        // the Logs tab of a running instance. Anywhere else a 30 Hz repaint
        // would redraw a screen that never changes.
        let pulse_on_screen = matches!(&self.screen, Screen::Instance(id)
            if self.tab == screens::instance::Tab::Logs && self.instance_running(id));
        if pulse_on_screen {
            feeds.push(iced::time::every(FRAME).map(|_| Message::Tick(FRAME.as_secs_f32())));
        }

        if self.following_live_log() {
            // Slower than the animation on purpose. The pulse has to be smooth
            // to look alive; re-reading 64 KiB off disk at 30 Hz to achieve
            // that would be absurd, and a second is well inside what reads as
            // immediate for a log.
            feeds.push(iced::time::every(REREAD).map(|_| Message::FollowLog));
        }

        if let Screen::PaperConsole(id) = &self.screen
            && self
                .paper_servers
                .iter()
                .any(|s| &s.id == id && s.status == nexo_core::paper_server::status::RUNNING)
        {
            feeds.push(iced::time::every(REREAD).map(|_| Message::PaperConsoleFollowLog));
        }

        iced::Subscription::batch(feeds)
    }
}

/// Debug builds only: `NEXO_DEV_SCREEN` opens a screen at boot, so a screen
/// can be screenshotted without driving the mouse. `wizard:<0-4>` is a wizard
/// step; `server:<id>:<0-4>` is a dashboard tab.
fn dev_screen() -> Task<Message> {
    if !cfg!(debug_assertions) {
        return Task::none();
    }
    use screens::hosting::{Msg, Step, Tab};
    let Ok(spec) = std::env::var("NEXO_DEV_SCREEN") else { return Task::none() };
    let mut parts = spec.split(':');
    let index = |p: Option<&str>| p.and_then(|n| n.parse::<usize>().ok()).unwrap_or(0);
    let later = |m: Message| Task::perform(tokio::time::sleep(std::time::Duration::from_millis(1500)), move |_| m.clone());
    match parts.next() {
        Some("servers") => Task::done(Message::Navigate(Screen::Servers)),
        // Home as a signed-in player with nothing set up, with no real account.
        Some("setup") => {
            let account = Account {
                uuid: "00000000000000000000000000000000".to_string(),
                username: "Friend".to_string(),
                access_token: String::new(),
                refresh_token: String::new(),
                expires_at: 0,
                skin_url: None,
                skin_model: Default::default(),
                cape_url: None,
            };
            let active = Some(account.uuid.clone());
            let run = parts.next() == Some("run");
            let signed_in = later(Message::Loaded { instances: Vec::new(), accounts: vec![account], active });
            if run { signed_in.chain(later(Message::QuickSetup)) } else { signed_in }
        }
        // The Skins page as a signed-in player: wearing the first skin of the
        // library on disk, with two capes made from the cached cape texture.
        Some("skins") | Some("home") | Some("capes") => {
            let home = spec.starts_with("home");
            let capes_tab = spec.strip_prefix("capes").map(|rest| index(rest.strip_prefix(':')));
            let account = Account {
                uuid: "00000000000000000000000000000000".to_string(),
                username: "Friend".to_string(),
                access_token: String::new(),
                refresh_token: String::new(),
                expires_at: 0,
                skin_url: None,
                skin_model: Default::default(),
                cape_url: None,
            };
            let active = Some(account.uuid.clone());
            let root = nexo_core::Paths::discover().map(|paths| paths.root().to_path_buf()).unwrap_or_default();
            let first_png = |dir: std::path::PathBuf, suffix: &str| {
                let mut files: Vec<_> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).collect();
                files.sort();
                files.into_iter().find(|p| p.to_string_lossy().ends_with(suffix)).and_then(|p| std::fs::read(p).ok())
            };
            let skin = first_png(root.join("skins"), ".png").and_then(|bytes| skin::Skin::decode(&bytes, Default::default()).ok());
            let cape = first_png(root.join("skin-cache"), ".cape.png").and_then(|bytes| skin::decode_texture(&bytes).ok()).map(Arc::new);
            use nexo_core::cosmetics::Cape;
            let capes = vec![Cape::new("a", "Migrator", "http://127.0.0.1:9/a", true), Cape::new("b", "Pan", "http://127.0.0.1:9/b", false)];
            let mut tasks = vec![later(Message::Loaded { instances: Vec::new(), accounts: vec![account], active })];
            if let Some(mut skin) = skin {
                let model = skin.use_detected_model();
                tasks.push(later(Message::SkinLoaded {
                    uuid: None,
                    face: to_handle(skin.face(FACE_SCALE)),
                    skin: Arc::new(skin.texture().clone()),
                    cape: cape.clone(),
                    model,
                }));
            }
            if !home {
                tasks.push(Task::done(Message::CapesLoaded(Ok(capes))));
                if let Some(cape) = cape {
                    for id in ["a", "b"] {
                        tasks.push(Task::done(Message::CapeTextureLoaded { cape: id.to_string(), texture: cape.clone() }));
                    }
                }
                if capes_tab.is_none() {
                    tasks.push(later(Message::Navigate(Screen::Skins)));
                }
            }
            if let Some(tab) = capes_tab {
                // Fake Nexo capes, all wearing the template, with this account as a reviewer.
                use nexo_core::nexo_capes::{CustomCape, Mine, ReviewQueue};
                let cape = |id: &str, name: &str, favorites, status: Option<&str>| CustomCape {
                    id: id.repeat(64),
                    name: name.into(),
                    favorites,
                    uses: favorites / 2,
                    status: status.map(Into::into),
                    reason: (status == Some("rejected")).then(|| "Too blurry".into()),
                    code: format!("{}-{}", id.repeat(4).to_uppercase(), id.repeat(4).to_uppercase()),
                    uploader: Some("Lokifisch".into()),
                    reports: 0,
                };
                let catalog = vec![cape("a", "Nebula", 42, None), cape("b", "Ember", 17, None), cape("c", "Pixel Fox", 3, None)];
                let mine = Mine {
                    mine: vec![cape("c", "Pixel Fox", 3, Some("approved")), cape("d", "Draft", 0, Some("pending")), cape("e", "Old one", 0, Some("rejected"))],
                    favorites: vec!["a".repeat(64)],
                    worn: Some("b".repeat(64)),
                    limit: 5,
                    admin: true,
                };
                let reported = CustomCape { reports: 3, ..cape("c", "Pixel Fox", 3, Some("approved")) };
                let review = ReviewQueue { pending: vec![cape("d", "Draft", 0, Some("pending"))], approved: catalog.clone(), reported: vec![reported] };
                let uuid = "00000000000000000000000000000000".to_string();
                let template = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../EmoteService/src/nexo_emotes/assets/cape_template.png"))
                    .ok()
                    .and_then(|bytes| skin::decode_texture(&bytes).ok())
                    .map(Arc::new);
                tasks.push(Task::done(Message::NexoCatalogLoaded(Default::default(), Ok(catalog))));
                tasks.push(Task::done(Message::NexoMineLoaded(uuid.clone(), Ok(mine))));
                tasks.push(Task::done(Message::NexoReviewLoaded(uuid, Ok(review))));
                if let Some(texture) = template {
                    for id in ["a", "b", "c", "d", "e"] {
                        tasks.push(Task::done(Message::NexoCapeTexture(id.repeat(64), texture.clone())));
                    }
                }
                let tabs = [screens::capes::CapeTab::Vanilla, screens::capes::CapeTab::Custom, screens::capes::CapeTab::Mine, screens::capes::CapeTab::Favorites, screens::capes::CapeTab::Review];
                tasks.push(later(Message::Navigate(Screen::Capes)));
                tasks.push(later(Message::CapeTab(tabs[tab.min(4)])));
                // `capes:<tab>:upload` also opens the upload popup on the template.
                if spec.ends_with(":upload") {
                    let path = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../EmoteService/src/nexo_emotes/assets/cape_template.png"));
                    let bytes = std::fs::read(&path).unwrap_or_default();
                    let texture = skin::decode_texture(&bytes).map_err(|e| e.to_string()).map(Arc::new);
                    tasks.push(later(Message::CapeDraftPicked(Some((path.with_file_name("nexo_cape.png"), Arc::new(bytes), texture)))));
                }
            }
            tasks.into_iter().reduce(Task::chain).unwrap_or_else(Task::none)
        }
        Some("settings") => {
            use screens::settings::{Msg, Tab};
            let tab = if parts.next() == Some("games") { Tab::Games } else { Tab::Appearance };
            Task::done(Message::Navigate(Screen::Settings)).chain(Task::done(Message::ThemeEditor(Msg::Tab(tab))))
        }
        Some("wizard") => {
            let step = Step::ALL[index(parts.next()).min(4)];
            Task::done(Message::Navigate(Screen::NewServer)).chain(later(Message::Host(Msg::GoTo(step))))
        }
        Some("server") => {
            let id = parts.next().unwrap_or_default().to_string();
            let tab = Tab::ALL[index(parts.next()).min(4)];
            Task::done(Message::Navigate(Screen::Servers))
                .chain(later(Message::Navigate(Screen::Server(id))))
                .chain(later(Message::Host(Msg::Tab(tab))))
        }
        _ => Task::none(),
    }
}

/// Fetches the Modrinth icon of every hit not already cached. One task each,
/// so a slow or missing icon never holds up the others.
fn fetch_icons(
    core: Option<Nexo>,
    hits: &[nexo_core::modrinth::SearchHit],
    icons: &std::collections::HashMap<String, image::Handle>,
) -> Task<Message> {
    let Some(core) = core else { return Task::none() };
    Task::batch(hits.iter().filter(|h| !icons.contains_key(&h.project_id)).filter_map(|hit| {
        let url = hit.icon_url.clone()?;
        let core = core.clone();
        let project = hit.project_id.clone();
        Some(Task::perform(
            async move {
                nexo_core::skin::fetch_texture(core.http(), &url)
                    .await
                    .ok()
                    .map(|icon| (project, icon))
            },
            |loaded| match loaded {
                Some((project, icon)) => Message::IconLoaded {
                    project,
                    handle: image::Handle::from_rgba(icon.width, icon.height, icon.pixels),
                },
                None => Message::Noop,
            },
        ))
    }))
}

/// Re-reads instances and accounts from disk. Cheap, and simpler to reason
/// about than mutating the in-memory lists at every call site.
fn reload(core: Nexo) -> Task<Message> {
    Task::perform(
        async move {
            let instances = core.instances.list().await.unwrap_or_default();
            let accounts = core.accounts.list().await.unwrap_or_default();
            let active = core.accounts.active().await.ok().flatten().map(|a| a.uuid);
            (instances, accounts, active)
        },
        |(instances, accounts, active)| Message::Loaded {
            instances,
            accounts,
            active,
        },
    )
}

/// Populates the version picker. Failure is non-fatal — the default version
/// stays selectable offline.
fn load_versions(core: Nexo) -> Task<Message> {
    Task::perform(
        async move {
            match core.installer.version_manifest().await {
                Ok(manifest) => manifest.releases().map(|v| v.id.clone()).take(60).collect(),
                Err(err) => {
                    tracing::warn!(%err, "could not load the Minecraft version list");
                    Vec::new()
                }
            }
        },
        Message::VersionsLoaded,
    )
}

/// A chosen file as a cape: its bytes, and its decoded image for the preview or why the
/// service would refuse it. The service checks again; this only saves a wasted upload.
async fn check_cape_file(path: &std::path::Path) -> (Vec<u8>, Result<skin::Rgba, String>) {
    let size = match tokio::fs::metadata(path).await {
        Ok(meta) => meta.len(),
        Err(err) => return (Vec::new(), Err(err.to_string())),
    };
    if size > 1024 * 1024 {
        return (Vec::new(), Err("This file is over 1 MB.".into()));
    }
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(err) => return (Vec::new(), Err(err.to_string())),
    };
    let checked = cape_png_size(&bytes).and_then(|_| skin::decode_texture(&bytes).map_err(|_| "This isn't a readable PNG.".to_string()));
    (bytes, checked)
}

/// A PNG's size from its header, refused before anything is decoded: a small file can
/// claim a huge image, and the right message needs the size anyway.
fn cape_png_size(bytes: &[u8]) -> Result<(u32, u32), String> {
    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 24 || &bytes[..8] != SIGNATURE || &bytes[12..16] != b"IHDR" {
        return Err("This isn't a PNG. Save the cape as a PNG and choose it again.".into());
    }
    let w = u32::from_be_bytes(bytes[16..20].try_into().expect("four bytes"));
    let h = u32::from_be_bytes(bytes[20..24].try_into().expect("four bytes"));
    if w != h.saturating_mul(2) || !(64..=1024).contains(&w) || !w.is_power_of_two() {
        return Err(format!("This image is {w}×{h}. A cape is 64×32, or that times 2, 4, 8 or 16 (the template is 512×256)."));
    }
    Ok((w, h))
}

/// Runs `work` with the active account, its token renewed first.
async fn with_valid_account<F, Fut, T>(core: nexo_core::Nexo, work: F) -> Result<T, String>
where
    F: FnOnce(nexo_core::Nexo, Account) -> Fut,
    Fut: std::future::Future<Output = nexo_core::Result<T>>,
{
    let account = core.accounts.active_valid(&core.auth).await.map_err(|e| e.to_string())?;
    work(core, account).await.map_err(|e| e.to_string())
}

/// Fetches the account's skin and cape, falling back to the placeholder for
/// a signed-out state, an account with no skin, or a failed download — none
/// of which is worth surfacing as an error.
fn load_skin(core: Nexo, account: Option<Account>) -> Task<Message> {
    // Paint the last-known skin from disk straight away, then refresh from the
    // network. Chained, not batched, so the older cached copy can never land
    // after the fresh one; and the refresh knows whether to keep what is shown
    // when Mojang is unreachable.
    let Some(uuid) = account.as_ref().map(|a| a.uuid.clone()) else {
        return fetch_skin(core, account, false);
    };
    Task::perform(cached_skin(core.clone(), uuid), |m| m).then(move |cached| {
        let had_cache = cached.is_some();
        let show = cached.map(Task::done).unwrap_or_else(Task::none);
        show.chain(fetch_skin(core.clone(), account.clone(), had_cache))
    })
}

/// Per-account copy of the worn skin and cape, kept so the next launch does
/// not wait on Mojang before drawing them. The uuid comes from the account
/// store, never from user input, but is checked anyway before touching a path.
fn skin_cache_path(core: &Nexo, uuid: &str, ext: &str) -> Option<std::path::PathBuf> {
    (!uuid.is_empty() && uuid.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        .then(|| core.paths.root().join("skin-cache").join(format!("{uuid}.{ext}")))
}

async fn cached_skin(core: Nexo, uuid: String) -> Option<Message> {
    let raw = tokio::fs::read(skin_cache_path(&core, &uuid, "png")?).await.ok()?;
    let mut decoded = skin::Skin::decode(&raw, nexo_core::SkinModel::Classic).ok()?;
    let model = decoded.use_detected_model();
    let cape = match tokio::fs::read(skin_cache_path(&core, &uuid, "cape.png")?).await {
        Ok(bytes) => skin::decode_texture(&bytes).ok().map(Arc::new),
        Err(_) => None,
    };
    Some(Message::SkinLoaded {
        uuid: Some(uuid),
        face: to_handle(decoded.face(FACE_SCALE)),
        skin: Arc::new(decoded.texture().clone()),
        cape,
        model,
    })
}

async fn write_skin_cache(core: &Nexo, uuid: &str, ext: &str, bytes: Option<&[u8]>) {
    let Some(path) = skin_cache_path(core, uuid, ext) else {
        return;
    };
    let result = match bytes {
        Some(bytes) => match path.parent() {
            Some(dir) => match tokio::fs::create_dir_all(dir).await {
                Ok(()) => tokio::fs::write(&path, bytes).await,
                Err(err) => Err(err),
            },
            None => Ok(()),
        },
        // Cape unequipped: drop the stale copy so it doesn't come back next launch.
        None => match tokio::fs::remove_file(&path).await {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err),
            _ => Ok(()),
        },
    };
    if let Err(err) = result {
        tracing::warn!(%err, "could not update the skin cache");
    }
}

/// `keep_shown`: a cached skin is already on screen, so a transient failure
/// (offline, Mojang outage) leaves it there instead of swapping in the placeholder.
fn fetch_skin(core: Nexo, account: Option<Account>, keep_shown: bool) -> Task<Message> {
    Task::perform(
        async move {
            let Some(account) = account else {
                return Some(placeholder_skin());
            };

            // Bring cosmetics up to date before drawing them. Skins and capes
            // change independently of tokens, and an account stored by an
            // older build has no cape recorded at all — which is exactly why
            // an equipped cape could otherwise never appear without signing
            // in again.
            let account = match core.sync_active_profile().await {
                Ok(Some(updated)) => updated,
                Ok(None) => account,
                Err(err) => {
                    tracing::warn!(%err, "could not refresh the profile, using stored cosmetics");
                    account
                }
            };

            let Some(url) = account.skin_url.clone() else {
                // The account has no skin (removed or reset): forget the old copy.
                write_skin_cache(&core, &account.uuid, "png", None).await;
                write_skin_cache(&core, &account.uuid, "cape.png", None).await;
                return Some(placeholder_skin_for(Some(account.uuid)));
            };

            // Fetched undecoded so the exact PNG can be kept and re-uploaded
            // later, rather than a re-encoding of its pixels.
            let raw = match skin::fetch_png(core.http(), &url).await {
                Ok(raw) => raw,
                Err(err) => {
                    tracing::warn!(%err, "could not load skin, using the placeholder");
                    return (!keep_shown).then(|| placeholder_skin_for(Some(account.uuid)));
                }
            };
            let mut decoded = match skin::Skin::decode(&raw, account.skin_model) {
                Ok(decoded) => decoded,
                Err(err) => {
                    tracing::warn!(%err, "skin texture is not usable, using the placeholder");
                    return (!keep_shown).then(|| placeholder_skin_for(Some(account.uuid)));
                }
            };
            write_skin_cache(&core, &account.uuid, "png", Some(&raw)).await;

            // A cape is genuinely optional, and a failure to fetch one should
            // not cost the user their skin.
            let cape = match &account.cape_url {
                Some(cape_url) => match skin::fetch_png(core.http(), cape_url).await {
                    Ok(bytes) => match skin::decode_texture(&bytes) {
                        Ok(cape) => {
                            write_skin_cache(&core, &account.uuid, "cape.png", Some(&bytes)).await;
                            Some(Arc::new(cape))
                        }
                        Err(err) => {
                            tracing::warn!(%err, "could not load cape");
                            None
                        }
                    },
                    Err(err) => {
                        tracing::warn!(%err, "could not load cape");
                        None
                    }
                },
                None => {
                    write_skin_cache(&core, &account.uuid, "cape.png", None).await;
                    None
                }
            };

            // The texture is authoritative, not the profile's `variant`,
            // which can be stale or simply wrong. Rendering a slim texture
            // with classic geometry samples arm columns that slim skins leave
            // empty, so the arms come out full of holes.
            let model = decoded.use_detected_model();

            // Keep a copy of whatever is being worn. Mojang only stores the
            // current one, so without this a skin is gone the moment it is
            // replaced.
            if let Err(err) = core.skins.save(&raw, model, &account.username).await {
                tracing::warn!(%err, "could not add this skin to the library");
            }

            Some(Message::SkinLoaded {
                uuid: Some(account.uuid.clone()),
                face: to_handle(decoded.face(FACE_SCALE)),
                skin: Arc::new(decoded.texture().clone()),
                cape,
                model,
            })
        },
        |message| message,
    )
    .then(|m| m.map(Task::done).unwrap_or_else(Task::none))
}

/// Signed-out stand-in. A real 64x64 texture rather than a special case, so
/// the 3D renderer always has exactly one path.
fn placeholder_skin() -> Message {
    placeholder_skin_for(None)
}

fn placeholder_skin_for(uuid: Option<String>) -> Message {
    Message::SkinLoaded {
        uuid,
        face: to_handle(skin::placeholder_face(FACE_SCALE)),
        skin: Arc::new(skin::placeholder_texture()),
        cape: None,
        model: nexo_core::SkinModel::Classic,
    }
}

fn to_handle(rgba: skin::Rgba) -> image::Handle {
    image::Handle::from_rgba(rgba.width, rgba.height, rgba.pixels)
}

/// Shared empty-state block, used by several screens.
fn empty_state<'a>(title: &'a str, hint: &'a str) -> Element<'a, Message> {
    iced::widget::container(
        iced::widget::column![
            crate::lively::text(title).size(18).color(theme::text()),
            crate::lively::text(hint).size(14).color(theme::muted()),
        ]
        .spacing(8),
    )
    .padding(32)
    .width(Fill)
    .style(theme::card)
    .into()
}

#[cfg(test)]
mod motion_frames {
    use super::*;
    use iced_test::simulator::Simulator;
    use std::time::{Duration, Instant};

    /// `NEXO_FRAME_DIR=<dir> cargo test -p nexo-app motion_frames -- --ignored`
    /// writes real rendered frames of a page transition, to look at. Needs no
    /// window, so it works on a locked or headless machine.
    fn dir() -> std::path::PathBuf {
        std::env::var("NEXO_FRAME_DIR").expect("set NEXO_FRAME_DIR").into()
    }

    fn shoot(app: &App, name: &str) {
        let settings = iced::Settings::default();
        let mut ui = Simulator::with_size(settings, iced::Size::new(1100.0, 640.0), app.view());
        let snapshot = ui.snapshot(&theme::current()).unwrap();
        snapshot.matches_image(dir().join(format!("{name}.png"))).unwrap();
    }

    /// The welcome, Home and Skins screens, drawn from the skins and cached
    /// cape in `NEXO_SHOT_DATA` (a Nexo data folder) as a pretend account.
    /// `NEXO_FRAME_DIR=<dir> NEXO_SHOT_DATA=~/.local/share/nexo cargo test -p nexo-app screens_offscreen -- --ignored`
    #[test]
    #[ignore = "writes images; run by hand"]
    fn screens_offscreen() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let data = std::path::PathBuf::from(std::env::var("NEXO_SHOT_DATA").expect("set NEXO_SHOT_DATA"));
        let (fonts, font) = screens::settings::startup_fonts();
        theme::set_font(font);
        let shoot = |app: &App, name: &str, frames: usize| {
            let settings = iced::Settings { fonts: fonts.clone(), default_font: font, ..Default::default() };
            // `NEXO_SHOT_SIZE=1600x864` for the website's pictures.
            let size = std::env::var("NEXO_SHOT_SIZE").ok().and_then(|size| {
                let (w, h) = size.split_once('x')?;
                Some(iced::Size::new(w.parse().ok()?, h.parse().ok()?))
            });
            // Tall for Skins, so the capes under the skins are in the picture.
            let size = size.unwrap_or(iced::Size::new(1280.0, if name == "skins" { 1500.0 } else { 800.0 }));
            let mut ui = Simulator::with_size(settings, size, app.view());
            if name == "capes" {
                // Down to the capes, under the skins.
                let at = iced::Point::new(size.width / 2.0, size.height / 2.0);
                ui.point_at(at);
                for _ in 0..3 {
                    let _ = ui.snapshot(&theme::current());
                    ui.simulate([iced::Event::Mouse(iced::mouse::Event::WheelScrolled { delta: iced::mouse::ScrollDelta::Pixels { x: 0.0, y: -2000.0 } })]);
                }
            }
            for frame in 0..frames {
                // Halfway, the cursor goes to the top left, for the head to follow.
                if frame == frames / 2 && frames > 1 {
                    let at = iced::Point::new(200.0, 120.0);
                    ui.point_at(at);
                    ui.simulate([iced::Event::Mouse(iced::mouse::Event::CursorMoved { position: at })]);
                }
                let snapshot = ui.snapshot(&theme::current()).unwrap();
                if frame + 1 == frames || frame % 15 == 0 {
                    snapshot.matches_image(dir().join(format!("{name}-{frame:02}.png"))).unwrap();
                }
                std::thread::sleep(Duration::from_millis(33));
            }
        };

        let (mut app, _) = App::boot();
        app.accounts_loaded = true;
        app.focused = true;
        // There's no core in a test, so boot's "Starting up" would stay up.
        app.status = Status::Idle;
        shoot(&app, "welcome", 1);

        app.accounts = vec![Account {
            uuid: "00000000000000000000000000000000".into(),
            username: "Friend".into(),
            access_token: String::new(),
            refresh_token: String::new(),
            expires_at: 0,
            skin_url: None,
            skin_model: Default::default(),
            cape_url: None,
        }];
        app.active_account = Some(app.accounts[0].uuid.clone());
        let mut pngs: Vec<_> = std::fs::read_dir(data.join("skins")).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "png")).collect();
        pngs.sort();
        // Skins to leave out of the pictures, by id.
        let skip = std::env::var("NEXO_SHOT_SKIP").unwrap_or_default();
        pngs.retain(|p| !skip.split(',').any(|id| !id.is_empty() && p.to_string_lossy().contains(id)));
        for (index, png) in pngs.iter().take(7).enumerate() {
            let mut decoded = skin::Skin::decode(&std::fs::read(png).unwrap(), Default::default()).unwrap();
            let model = decoded.use_detected_model();
            let id = png.file_stem().unwrap().to_string_lossy().to_string();
            let texture = Arc::new(decoded.texture().clone());
            if index == 0 {
                app.skin_texture = Some(Arc::clone(&texture));
                app.skin_model = model;
            }
            app.saved_textures.insert(id.clone(), (texture, model));
            app.saved_skins.push(nexo_core::skin_library::SavedSkin { id, model, added_at: 0, name: Some(["Friend", "Builder", "Night", "Summer"][index % 4].into()) });
        }
        let cape = std::fs::read_dir(data.join("skin-cache")).unwrap().flatten().map(|e| e.path())
            .find(|p| p.to_string_lossy().ends_with(".cape.png"))
            .map(|p| Arc::new(skin::decode_texture(&std::fs::read(p).unwrap()).unwrap()));
        app.cape_texture = cape.clone();
        use nexo_core::cosmetics::Cape;
        app.capes = vec![Cape::new("a", "Migrator", "", true), Cape::new("b", "Pan", "", false)];
        if let Some(cape) = cape {
            app.cape_textures.insert("a".into(), cape.clone());
            app.cape_textures.insert("b".into(), cape);
        }
        app.instances = Vec::new();
        shoot(&app, "home", 46);
        // Home in other themes, each with the background it comes with, for
        // the website's picture of them side by side.
        if std::env::var("NEXO_SHOT_THEMES").is_ok() {
            for (index, palette) in theme::PALETTES.iter().enumerate().skip(1) {
                screens::settings::pick(&mut app, index);
                shoot(&app, &format!("theme-{}", palette.name.to_lowercase().replace(' ', "-")), 40);
            }
            screens::settings::pick(&mut app, 0);
        }
        app.screen = Screen::Skins;
        app.hovered_skin = app.saved_skins.get(1).map(|s| s.id.clone());
        shoot(&app, "skins", 1);
        app.hovered_skin = None;
        shoot(&app, "capes", 1);
        // Page changes run on the app's own clock, which the simulator never
        // ticks; with reduced motion they finish at once.
        crate::motion::set_reduced(true);
        let _ = app.update(Message::Navigate(Screen::Settings));
        shoot(&app, "settings", 1);
        crate::motion::set_reduced(false);
    }

    /// Frames of the dropdown opening and a button lighting up, on a real clock.
    #[test]
    #[ignore = "writes images; run by hand"]
    fn widget_frames() {
        use iced::{Event, Point, mouse};
        use iced_test::simulator;
        let versions: Vec<String> = (0..40).map(|i| format!("1.{}.{}", 21 - i / 8, i % 8)).collect();
        let chosen = versions[6].clone();
        let bar = iced::widget::column![
            iced::widget::row![
                crate::dropdown::dropdown(versions.as_slice(), Some(&chosen), |_: String| Message::DismissStatus).width(220),
                crate::lively::button(crate::lively::text("Ghost")).padding([8, 14]).style(theme::ghost_button).on_press(Message::DismissStatus),
                crate::lively::button(crate::lively::text("PLAY")).padding([8, 24]).style(theme::hero_button).on_press(Message::DismissStatus),
                crate::lively::toggler(true).on_toggle(|_| Message::DismissStatus),
                crate::lively::toggler(false).on_toggle(|_| Message::DismissStatus),
            ]
            .spacing(14)
            .align_y(iced::Center),
            iced::widget::Space::new().height(iced::Fill),
            crate::dropdown::dropdown(&["Survival", "Creative", "Adventure"][..], Some("Creative"), |_: &str| Message::DismissStatus).width(220),
        ]
        .padding(16)
        .height(iced::Fill);
        let element: Element<'_, Message> = iced::widget::container(bar).width(iced::Fill).height(iced::Fill).style(theme::panel).into();
        let mut ui = Simulator::with_size(iced::Settings::default(), iced::Size::new(620.0, 420.0), element);
        let shot = |ui: &mut Simulator<'_, Message>, name: &str| {
            ui.snapshot(&theme::current()).unwrap().matches_image(dir().join(format!("{name}.png"))).unwrap();
        };
        shot(&mut ui, "w00-closed");
        ui.point_at(Point::new(60.0, 30.0));
        ui.simulate(simulator::click());
        for i in 1..=4 {
            std::thread::sleep(Duration::from_millis(35));
            shot(&mut ui, &format!("w0{i}-opening"));
        }
        for _ in 0..12 {
            std::thread::sleep(Duration::from_millis(35));
            let _ = ui.snapshot(&theme::current());
        }
        shot(&mut ui, "w05-open");
        // The highlight travels to the row under the cursor.
        ui.point_at(Point::new(90.0, 150.0));
        ui.simulate([Event::Mouse(mouse::Event::CursorMoved { position: Point::new(90.0, 150.0) })]);
        let _ = ui.snapshot(&theme::current());
        std::thread::sleep(Duration::from_millis(40));
        shot(&mut ui, "w06-gliding");
        for _ in 0..12 {
            std::thread::sleep(Duration::from_millis(35));
            let _ = ui.snapshot(&theme::current());
        }
        shot(&mut ui, "w07-hovered");
        // The bottom one closes the first and opens upward.
        ui.point_at(Point::new(60.0, 390.0));
        ui.simulate(simulator::click());
        let _ = ui.snapshot(&theme::current());
        std::thread::sleep(Duration::from_millis(40));
        shot(&mut ui, "w08-swap");
        ui.simulate(simulator::click());
        for _ in 0..12 {
            std::thread::sleep(Duration::from_millis(35));
            let _ = ui.snapshot(&theme::current());
        }
        shot(&mut ui, "w09-up");
    }

    #[test]
    #[ignore = "writes images; run by hand"]
    fn page_transition() {
        let (mut app, _boot) = App::boot();
        let _ = app.update(Message::Navigate(Screen::Home));
        let t0 = Instant::now();
        let _ = app.update(Message::Navigate(Screen::Accounts));
        for millis in [0u64, 60, 140, 280, 600] {
            let _ = app.update(Message::Frame(t0 + Duration::from_millis(millis)));
            shoot(&app, &format!("nav-{millis:03}"));
        }
        assert!(!app.motion.active(), "must stop asking for frames once settled");
    }

    fn instance(number: usize) -> Instance {
        Instance {
            id: format!("i{number}"),
            name: format!("Instance {number}"),
            game_version: "26.3".to_string(),
            loader: Default::default(),
            loader_version: None,
            created_at: 0,
            last_played: None,
            memory_mb: None,
            java_path: None,
            mods: Vec::new(),
        }
    }

    fn play(app: &mut App, name: &str, change: Message) {
        let t0 = Instant::now();
        let _ = app.update(change);
        for millis in [0u64, 60, 140, 280, 600] {
            let _ = app.update(Message::Frame(t0 + Duration::from_millis(millis)));
            shoot(app, &format!("{name}-{millis:03}"));
        }
        assert!(!app.motion.active(), "{name} must settle");
    }

    #[test]
    #[ignore = "writes images; run by hand"]
    fn instances_list_and_tabs() {
        let (mut app, _boot) = App::boot();
        app.instances = (0..6).map(instance).collect();
        play(&mut app, "list", Message::Navigate(Screen::Instances));
        play(&mut app, "open", Message::Navigate(Screen::Instance("i0".to_string())));
        play(&mut app, "tab", Message::SelectTab(screens::instance::Tab::Logs));
    }

    /// A scrollable's position lives in its widget state, so if the state at
    /// that spot survives, the list stays where it was scrolled to.
    #[test]
    fn installing_keeps_the_mod_list_where_it_was_scrolled() {
        use iced::advanced::widget::Tree;
        let (mut app, _boot) = App::boot();
        app.instances = (0..2).map(instance).collect();
        let _ = app.update(Message::Navigate(Screen::Instance("i0".to_string())));
        let _ = app.update(Message::SelectTab(screens::instance::Tab::Content));
        app.browsing = true;
        app.status = Status::Idle;
        let tree_of = |app: &App| Tree::new(&app.view());
        let mut tree = tree_of(&app);
        let list = crate::lively::scroll_states(&tree);
        assert_eq!(list.len(), 1, "the browser has one list");

        let project = |p: &str| Message::InstallProject { instance: "i0".to_string(), project: p.to_string() };
        for p in ["a", "b"] {
            app.install_queue.push_back(QueuedInstall { instance: "i0".to_string(), project: p.to_string(), kind: ProjectKind::Mod });
        }
        let _ = app.update(project("a"));
        assert_eq!(app.install_queue.len(), 2, "a second click on a queued mod adds nothing");
        assert!(matches!(app.status, Status::Idle), "no banner to push the list down");
        tree.diff(&app.view());
        assert_eq!(crate::lively::scroll_states(&tree), list);

        // One fails: the banner comes up, the list and the queue both carry on.
        let _ = app.update(Message::InstallDone(Err("no build".to_string())));
        assert_eq!(app.install_queue.len(), 1);
        tree.diff(&app.view());
        assert_eq!(crate::lively::scroll_states(&tree), list);

        let _ = app.update(Message::DismissStatus);
        let _ = app.update(Message::InstallDone(Ok(())));
        assert!(app.install_queue.is_empty());
        tree.diff(&app.view());
        assert_eq!(crate::lively::scroll_states(&tree), list);
    }

    #[test]
    #[ignore = "writes images; run by hand"]
    fn rail_hover() {
        let (mut app, _boot) = App::boot();
        let t0 = Instant::now();
        let _ = app.update(Message::Hover(("nav", "Skin & capes"), true));
        for millis in [0u64, 60, 200] {
            let _ = app.update(Message::Frame(t0 + Duration::from_millis(millis)));
            shoot(&app, &format!("hover-{millis:03}"));
        }
        let _ = app.update(Message::Frame(t0 + Duration::from_millis(600)));
        assert!(!app.motion.active());
    }

    #[test]
    #[ignore = "writes images; run by hand"]
    fn tab_underlines() {
        let bar = iced::widget::column(
            [0.0f32, 0.25, 0.5, 0.75, 1.0].into_iter().map(|amount| {
                iced::widget::container(screens::tab_underline(amount)).width(200).padding(8).into()
            }),
        );
        let element: Element<'_, Message> = bar.into();
        let mut ui = Simulator::with_size(iced::Settings::default(), iced::Size::new(216.0, 110.0), element);
        ui.snapshot(&theme::current()).unwrap().matches_image(dir().join("underline.png")).unwrap();
    }
}

#[cfg(test)]
mod cape_upload_tests {
    use super::cape_png_size;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        bytes.extend(w.to_be_bytes());
        bytes.extend(h.to_be_bytes());
        bytes
    }

    #[test]
    fn the_header_decides_before_any_decoding() {
        assert_eq!(cape_png_size(&png(512, 256)), Ok((512, 256)));
        assert_eq!(cape_png_size(&png(64, 32)), Ok((64, 32)));
        assert!(cape_png_size(&png(2048, 1024)).unwrap_err().contains("2048×1024"));
        assert!(cape_png_size(&png(96, 48)).is_err(), "not a power of two");
        assert!(cape_png_size(&png(64, 64)).is_err(), "not 2:1");
        assert!(cape_png_size(b"\xff\xd8\xff\xe0 a jpeg pretending").unwrap_err().contains("isn't a PNG"));
    }
}
