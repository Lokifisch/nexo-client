// A launcher has no business flashing a console window on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Nexo — native Minecraft client.
//!
//! The UI is deliberately thin: every operation with real logic lives in
//! `nexo-core` and is invoked here as an async [`Task`]. `update` stays a
//! pure state transition, which is what keeps the window responsive while a
//! ~1,000-file install runs underneath it.

mod pulse;
mod screens;
mod skin3d;
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
const DEFAULT_GAME_VERSION: &str = "26.1.2";

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

/// Upscale for the saved-skin tiles. A body render is 16x32 texture pixels,
/// so this puts it at a size a skin is recognisable from.
const LIBRARY_SCALE: u32 = 3;

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

    iced::application(App::boot, App::update, App::view)
        .title("Nexo")
        // The closure parameter must be annotated. Left to inference, it
        // breaks the higher-ranked lifetime resolution for `view` across the
        // whole builder chain, with an error that points at the chain rather
        // than at this line.
        // The accent colour is a function of the clock, so the whole app
        // drifts through the spectrum from this one line. See `theme::nexo`.
        .theme(|state: &App| theme::nexo(state.clock))
        // Stops entirely when the window loses focus — see `App::subscription`.
        .subscription(App::subscription)
        // Sets the Wayland `app_id` / X11 `WM_CLASS`. It has to match the
        // basename of the installed `nexo.desktop`, or desktop environments
        // can't tie the running window back to its launcher entry and show a
        // generic placeholder icon in the task switcher instead.
        .settings(iced::Settings {
            id: Some(APP_ID.to_string()),
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Screen {
    Home,
    Instances,
    Accounts,
    Skins,
    /// Details for one instance, by id.
    Instance(String),
}

impl Screen {
    /// Which sidebar entry should light up. The details screen belongs to
    /// Instances, so the rail doesn't go blank when you drill in.
    fn nav_group(&self) -> Screen {
        match self {
            Screen::Instance(_) => Screen::Instances,
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

/// Transient feedback shown in the header strip.
#[derive(Debug, Clone, Default)]
pub enum Status {
    #[default]
    Idle,
    /// A long-running operation with a human-readable stage.
    Busy(String),
    Error(String),
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

    status: Status,

    // New-instance form.
    new_name: String,
    new_version: String,

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

    /// Instances currently running, so Play can become Stop. Mirrors the
    /// core registry rather than querying it during `view`, which must stay
    /// free of locking.
    running: std::collections::HashSet<String>,

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
    /// Whether the window has focus. The clock stops when it doesn't.
    focused: bool,

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
    cape_previews: std::collections::HashMap<String, image::Handle>,
    /// The cape reveal in progress, if any. The 3D model derives its turn
    /// from this, so the animation can't be knocked out of step.
    cape_reveal: Option<skin3d::Reveal>,

    // Saved skins.
    saved_skins: Vec<nexo_core::skin_library::SavedSkin>,
    /// Face previews for the library, keyed by saved-skin id.
    skin_previews: std::collections::HashMap<String, image::Handle>,
    /// Which tile the cursor is over, so its delete button can appear.
    hovered_skin: Option<String>,
    /// Awaiting a yes/no on deleting this one.
    confirm_delete: Option<String>,
}

#[derive(Clone)]
pub enum Message {
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
    NewVersionChanged(String),
    CreateInstance,
    InstanceCreated(Result<(), String>),
    DeleteInstance(String),
    OpenInstance(String),
    Launch(String),
    Stop(String),
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
    LoadLogs,
    LogsLoaded(Vec<nexo_core::browse::LogFile>),
    SelectLog(String),
    LogLoaded(Result<(String, bool), String>),
    /// Advances the animation clock. Carries the elapsed seconds rather than a
    /// frame count so the animations run at the same speed whatever the tick
    /// interval turns out to be on a loaded machine.
    Tick(f32),
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
    AddFromFile(String),
    RemoveContent {
        instance: String,
        project: String,
    },

    // Skins and capes
    LoadCapes,
    CapesLoaded(Result<Vec<nexo_core::cosmetics::Cape>, String>),
    CapePreviewLoaded {
        cape: String,
        handle: image::Handle,
    },
    SetSkinModel(nexo_core::SkinModel),
    UploadSkin,
    ResetSkin,
    WearCape(String),
    HideCape,
    CosmeticsDone(Result<(), String>),

    // Saved skins
    LoadSavedSkins,
    SavedSkinsLoaded(Vec<nexo_core::skin_library::SavedSkin>),
    SkinPreviewLoaded {
        skin: String,
        handle: image::Handle,
    },
    HoverSkin(Option<String>),
    WearSavedSkin(String),
    AskDeleteSkin(String),
    CancelDeleteSkin,
    ConfirmDeleteSkin(String),

    // Modpacks
    OpenFolder(String),
    ImportPack,
    ExportPack(String),
    PackImported(Result<String, String>),

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
        face: image::Handle,
        skin: Arc<skin::Rgba>,
        cape: Option<Arc<skin::Rgba>>,
        model: nexo_core::SkinModel,
    },
}

impl App {
    fn boot() -> (Self, Task<Message>) {
        let app = Self {
            core: None,
            screen: Screen::Home,
            instances: Vec::new(),
            accounts: Vec::new(),
            active_account: None,
            game_versions: vec![DEFAULT_GAME_VERSION.to_string()],
            status: Status::Busy("Starting up".into()),
            new_name: String::new(),
            new_version: DEFAULT_GAME_VERSION.to_string(),
            signing_in: false,
            faces: std::collections::HashMap::new(),
            skin_texture: None,
            cape_texture: None,
            skin_model: nexo_core::SkinModel::Classic,
            skin_key: 0,
            running: std::collections::HashSet::new(),
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
            focused: true,
            files: Vec::new(),
            files_at: std::path::PathBuf::new(),
            files_error: None,
            worlds: Vec::new(),
            confirm_delete_world: None,
            servers: Vec::new(),
            server_status: std::collections::HashMap::new(),
            server_icons: std::collections::HashMap::new(),
            server_form: None,
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
            cape_previews: std::collections::HashMap::new(),
            cape_reveal: None,
            saved_skins: Vec::new(),
            skin_previews: std::collections::HashMap::new(),
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
        match message {
            Message::Booted(Ok(core)) => {
                self.core = Some(core.clone());
                self.status = Status::Idle;
                Task::batch([
                    reload(core.clone()),
                    load_versions(core),
                    // Quietly: see `Message::CheckForUpdate`.
                    Task::done(Message::CheckForUpdate { announce: false }),
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

                let mut tasks = Vec::new();

                // Only re-fetch the 3D textures when the active account
                // actually changed; reload() runs after nearly every action.
                if changed && let Some(core) = self.core.clone() {
                    tasks.push(load_skin(core, self.active_account().cloned()));
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
                let opening_skins = screen == Screen::Skins && self.screen != Screen::Skins;
                self.screen = screen;
                if opening_skins {
                    let mut tasks = vec![Task::done(Message::LoadSavedSkins)];
                    if self.capes.is_empty() {
                        tasks.push(Task::done(Message::LoadCapes));
                    }
                    return Task::batch(tasks);
                }
                Task::none()
            }

            Message::DismissStatus => {
                self.status = Status::Idle;
                Task::none()
            }

            Message::Noop => Task::none(),

            Message::NewNameChanged(name) => {
                self.new_name = name;
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
                let name = self.new_name.trim().to_string();
                if name.is_empty() {
                    self.status = Status::Error("Give the instance a name first".into());
                    return Task::none();
                }

                let version = self.new_version.clone();
                self.new_name.clear();
                self.status = Status::Busy(format!("Creating {name}"));

                Task::perform(
                    async move {
                        core.instances
                            .create(&name, &version, Loader::Fabric)
                            .await
                            .map(|_| ())
                            .map_err(|e| e.to_string())
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
                Task::none()
            }

            Message::DeleteInstance(id) => {
                let Some(core) = self.core.clone() else {
                    return Task::none();
                };
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

            Message::Stop(id) => {
                if let Some(core) = &self.core {
                    core.stop(&id);
                }
                // The registry deregisters asynchronously; GameExited flips
                // the button back once the process is actually gone.
                Task::none()
            }

            Message::GameExited(id) => {
                self.running.remove(&id);
                if !self.is_busy() {
                    self.status = Status::Idle;
                }
                Task::none()
            }

            Message::LoadJavaOptions => {
                let Some(core) = self.core.clone() else {
                    return Task::none();
                };
                Task::perform(
                    async move { nexo_core::java::options(&core.paths).await },
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
                    Ok(()) => Task::batch([reload(core), Task::done(Message::LoadJavaOptions)]),
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
                self.nexo_release = Some(release);
                self.nexo_release_error = None;
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
                // Re-read on every visit rather than caching. The game writes
                // into this directory while it runs, so a cached listing is
                // stale the moment it matters — and a directory read is
                // cheap next to being wrong about what is on disk.
                match tab {
                    screens::instance::Tab::Content => Task::none(),
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
                if self.running.contains(id) {
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

                // Fetch any icon not already cached. One task each, so a slow
                // or missing icon never holds up the others.
                let mut fetches = Vec::new();
                for hit in &hits {
                    let (Some(url), false) = (
                        hit.icon_url.clone(),
                        self.icons.contains_key(&hit.project_id),
                    ) else {
                        continue;
                    };
                    let Some(core) = self.core.clone() else {
                        continue;
                    };
                    let project = hit.project_id.clone();

                    fetches.push(Task::perform(
                        async move {
                            nexo_core::skin::fetch_texture(core.http(), &url)
                                .await
                                .ok()
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

                self.content_results = hits;
                Task::batch(fetches)
            }
            Message::ContentResults(Err(err)) => {
                self.content_searching = false;
                self.status = Status::Error(err);
                Task::none()
            }

            Message::InstallProject { instance, project } => {
                let Some(core) = self.core.clone() else {
                    return Task::none();
                };
                let kind = self.content_kind;
                self.status = Status::Busy("Installing".into());

                Task::perform(
                    async move {
                        let mut target = core
                            .instances
                            .get(&instance)
                            .await
                            .map_err(|e| e.to_string())?;
                        core.content
                            .install_modrinth(&mut target, &project, kind)
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
                            // missing is what makes it fixable.
                            format!(
                                "Imported {} — {} files, {} unavailable ({})",
                                imported.name,
                                imported.files,
                                imported.skipped.len(),
                                imported.skipped.join(", ")
                            )
                        })
                    },
                    Message::PackImported,
                )
            }

            Message::ExportPack(id) => {
                let Some(core) = self.core.clone() else {
                    return Task::none();
                };
                let Some(instance) = self.instances.iter().find(|i| i.id == id).cloned() else {
                    return Task::none();
                };
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
                            .export(&instance, handle.path())
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
                    if self.skin_previews.contains_key(&saved.id) {
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
                            decoded.use_detected_model();
                            // A whole figure rather than a head: one static
                            // render per skin, all from the same angle.
                            Some((id, decoded.body(LIBRARY_SCALE)))
                        },
                        |loaded| match loaded {
                            Some((skin, face)) => Message::SkinPreviewLoaded {
                                skin,
                                handle: to_handle(face),
                            },
                            None => Message::Noop,
                        },
                    ));
                }

                self.saved_skins = skins;
                Task::batch(fetches)
            }

            Message::SkinPreviewLoaded { skin, handle } => {
                self.skin_previews.insert(skin, handle);
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
                self.skin_previews.remove(&id);
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
                let model = self
                    .saved_skins
                    .iter()
                    .find(|s| s.id == id)
                    .map(|s| s.model)
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
                    if self.cape_previews.contains_key(&cape.id) {
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
                            Some((cape, texture)) => Message::CapePreviewLoaded {
                                cape,
                                handle: {
                                    // Crop to the panel that actually hangs
                                    // off the back; the rest of a cape
                                    // texture is empty space.
                                    let panel = nexo_core::skin::cape_panel(&texture, 4);
                                    image::Handle::from_rgba(
                                        panel.width,
                                        panel.height,
                                        panel.pixels,
                                    )
                                },
                            },
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

            Message::CapePreviewLoaded { cape, handle } => {
                self.cape_previews.insert(cape, handle);
                Task::none()
            }

            Message::SetSkinModel(model) => {
                // Recorded locally; it is applied to the account with the next
                // upload, since the model is a property of the uploaded skin.
                self.skin_model = model;
                Task::none()
            }

            Message::UploadSkin => {
                let Some(core) = self.core.clone() else {
                    return Task::none();
                };
                let Some(account) = self.active_account().cloned() else {
                    return Task::none();
                };
                let model = self.skin_model;
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
                // Stamped on press, not on completion, so the model starts
                // turning while the request is still in flight. Extends a
                // reveal already on screen rather than restarting it, or
                // switching capes would snap the model to the front first.
                self.cape_reveal = Some(skin3d::Reveal::trigger(
                    self.cape_reveal,
                    std::time::Instant::now(),
                ));

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
                self.cape_reveal = Some(skin3d::Reveal::trigger(
                    self.cape_reveal,
                    std::time::Instant::now(),
                ));

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
            Message::CosmeticsDone(Err(err)) => {
                self.status = Status::Error(err);
                Task::none()
            }

            Message::Launch(id) => {
                let Some(core) = self.core.clone() else {
                    return Task::none();
                };
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
                self.running.insert(id.clone());

                let run = Task::perform(
                    async move {
                        if let Err(err) = core.play(&id, Some(&tx)).await {
                            let _ = tx.send(Progress::Failed(err.to_string()));
                        } else {
                            let _ = tx.send(Progress::Done);
                            // Resolves when the JVM exits, however that
                            // happens — quit from the menu, crash, or Stop.
                            core.running.wait_for_exit(&id).await;
                        }
                        id
                    },
                    Message::GameExited,
                );

                Task::batch([progress, run])
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
                        self.status = Status::Idle;
                        return self.core.clone().map(reload).unwrap_or_else(Task::none);
                    }
                    Progress::Failed(err) => self.status = Status::Error(err),
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
                    async move { core.accounts.remove(&uuid).await.map_err(|e| e.to_string()) },
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
                face,
                skin,
                cape,
                model,
            } => {
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
        let body = match &self.screen {
            Screen::Home => screens::home::view(self),
            Screen::Instances => screens::instances::view(self),
            Screen::Accounts => screens::accounts::view(self),
            Screen::Skins => screens::skins::view(self),
            Screen::Instance(id) => match self.instances.iter().find(|i| &i.id == id) {
                Some(instance) => screens::instance::view(self, instance),
                // Deleted from under us, or an id that no longer exists.
                None => empty_state(
                    "That instance is gone",
                    "It was deleted, or its folder was removed outside the launcher.",
                ),
            },
        };

        let content = iced::widget::column![
            screens::top_bar(self),
            screens::status_bar(&self.status),
            body
        ]
        .spacing(16)
        .padding(24)
        .width(Fill)
        .height(Fill);

        iced::widget::row![screens::sidebar(self), content]
            .height(Fill)
            .into()
    }

    /// The accent as it stands this frame.
    ///
    /// Widget *styles* get this from the theme they are handed; this is for
    /// the handful of places that colour a piece of text directly, where
    /// there is no style function to read it out of.
    pub fn accent(&self) -> iced::Color {
        theme::spectrum(self.clock / theme::RAINBOW_PERIOD)
    }

    /// The account launches will use.
    fn active_account(&self) -> Option<&Account> {
        let uuid = self.active_account.as_deref()?;
        self.accounts.iter().find(|a| a.uuid == uuid)
    }

    fn is_busy(&self) -> bool {
        matches!(self.status, Status::Busy(_))
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

    /// Drops everything the four tabs hold. Called when an instance is opened,
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
            && self.running.contains(id)
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
    /// times a second forever to animate colours nobody is looking at — on a
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

        if !self.focused {
            return focus;
        }

        let mut feeds = vec![
            focus,
            iced::time::every(FRAME).map(|_| Message::Tick(FRAME.as_secs_f32())),
        ];

        if self.following_live_log() {
            // Slower than the animation on purpose. The pulse has to be smooth
            // to look alive; re-reading 64 KiB off disk at 30 Hz to achieve
            // that would be absurd, and a second is well inside what reads as
            // immediate for a log.
            feeds.push(iced::time::every(REREAD).map(|_| Message::FollowLog));
        }

        iced::Subscription::batch(feeds)
    }
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

/// Fetches the account's skin and cape, falling back to the placeholder for
/// a signed-out state, an account with no skin, or a failed download — none
/// of which is worth surfacing as an error.
fn load_skin(core: Nexo, account: Option<Account>) -> Task<Message> {
    Task::perform(
        async move {
            let Some(account) = account else {
                return placeholder_skin();
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
                return placeholder_skin();
            };

            // Fetched undecoded so the exact PNG can be kept and re-uploaded
            // later, rather than a re-encoding of its pixels.
            let raw = match skin::fetch_png(core.http(), &url).await {
                Ok(raw) => raw,
                Err(err) => {
                    tracing::warn!(%err, "could not load skin, using the placeholder");
                    return placeholder_skin();
                }
            };
            let mut decoded = match skin::Skin::decode(&raw, account.skin_model) {
                Ok(decoded) => decoded,
                Err(err) => {
                    tracing::warn!(%err, "skin texture is not usable, using the placeholder");
                    return placeholder_skin();
                }
            };

            // A cape is genuinely optional, and a failure to fetch one should
            // not cost the user their skin.
            let cape = match &account.cape_url {
                Some(cape_url) => match skin::fetch_texture(core.http(), cape_url).await {
                    Ok(cape) => Some(Arc::new(cape)),
                    Err(err) => {
                        tracing::warn!(%err, "could not load cape");
                        None
                    }
                },
                None => None,
            };

            // The texture is authoritative, not the profile's `variant`,
            // which can be stale or simply wrong. Rendering a slim texture
            // with classic geometry samples arm columns that slim skins leave
            // empty, so the arms come out full of holes.
            let model = decoded.use_detected_model();

            // Keep a copy of whatever is being worn. Mojang only stores the
            // current one, so without this a skin is gone the moment it is
            // replaced.
            if let Err(err) = core.skins.save(&raw, model).await {
                tracing::warn!(%err, "could not add this skin to the library");
            }

            Message::SkinLoaded {
                face: to_handle(decoded.face(FACE_SCALE)),
                skin: Arc::new(decoded.texture().clone()),
                cape,
                model,
            }
        },
        |message| message,
    )
}

/// Signed-out stand-in. A real 64x64 texture rather than a special case, so
/// the 3D renderer always has exactly one path.
fn placeholder_skin() -> Message {
    Message::SkinLoaded {
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
            iced::widget::text(title).size(18).color(theme::TEXT),
            iced::widget::text(hint).size(14).color(theme::MUTED),
        ]
        .spacing(8),
    )
    .padding(32)
    .width(Fill)
    .style(theme::card)
    .into()
}
