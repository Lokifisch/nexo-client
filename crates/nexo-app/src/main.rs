// A launcher has no business flashing a console window on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Nexo — native Minecraft client.
//!
//! The UI is deliberately thin: every operation with real logic lives in
//! `nexo-core` and is invoked here as an async [`Task`]. `update` stays a
//! pure state transition, which is what keeps the window responsive while a
//! ~1,000-file install runs underneath it.

mod screens;
mod theme;

use iced::widget::{column, container, row, text};
use iced::{Element, Fill, Task};
use nexo_core::minecraft::Progress;
use nexo_core::{Account, DeviceCode, Instance, Loader, Nexo};

/// Minecraft version new instances default to — the single version `Mod/`
/// targets for v1.
const DEFAULT_GAME_VERSION: &str = "26.1.2";

/// Desktop identity. Must stay in sync with `assets/nexo.desktop`'s filename
/// and its `StartupWMClass` key.
const APP_ID: &str = "nexo";

/// Embedded so the binary is self-sufficient — a `cargo run` from a source
/// checkout gets the right icon without anything being installed first.
const WINDOW_ICON: &[u8] = include_bytes!("../../../assets/icons/256.png");

fn main() -> iced::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "nexo_app=info,nexo_core=info".into()),
        )
        .init();

    iced::application(App::boot, App::update, App::view)
        .title("Nexo")
        // The closure parameter must be annotated. Left to inference, it
        // breaks the higher-ranked lifetime resolution for `view` across the
        // whole builder chain, with an error that points at the chain rather
        // than at this line.
        .theme(|_state: &App| theme::nexo())
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
            min_size: Some(iced::Size::new(820.0, 560.0)),
            // Used by X11 and by Windows for the titlebar. Wayland ignores
            // it and takes the icon from the .desktop file matched via app_id
            // above, which is why both mechanisms are set.
            icon: iced::window::icon::from_file_data(WINDOW_ICON, None).ok(),
            ..Default::default()
        })
        .antialiasing(true)
        .run()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Instances,
    Accounts,
}

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

    /// Present while a device-code sign-in is waiting on the user.
    pending_code: Option<DeviceCode>,
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
    Launch(String),
    LaunchProgress(Progress),

    // Accounts
    StartSignIn,
    DeviceCodeReady(Result<DeviceCode, String>),
    OpenVerificationUrl,
    SignInFinished(Result<Account, String>),
    SetActiveAccount(String),
    RemoveAccount(String),
    AccountActionDone(Result<(), String>),
}

impl App {
    fn boot() -> (Self, Task<Message>) {
        let app = Self {
            core: None,
            screen: Screen::Instances,
            instances: Vec::new(),
            accounts: Vec::new(),
            active_account: None,
            game_versions: vec![DEFAULT_GAME_VERSION.to_string()],
            status: Status::Busy("Starting up".into()),
            new_name: String::new(),
            new_version: DEFAULT_GAME_VERSION.to_string(),
            pending_code: None,
        };

        let task = Task::perform(
            async { Nexo::new().await.map_err(|e| e.to_string()) },
            Message::Booted,
        );

        (app, task)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Booted(Ok(core)) => {
                self.core = Some(core.clone());
                self.status = Status::Idle;
                Task::batch([reload(core.clone()), load_versions(core)])
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
                self.instances = instances;
                self.accounts = accounts;
                self.active_account = active;
                Task::none()
            }

            Message::VersionsLoaded(versions) => {
                if !versions.is_empty() {
                    self.game_versions = versions;
                }
                Task::none()
            }

            Message::Navigate(screen) => {
                self.screen = screen;
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

                let run = Task::perform(
                    async move {
                        match core.play(&id, Some(&tx)).await {
                            // The child is intentionally dropped: stdio is
                            // inherited, so the game keeps running on its own
                            // once spawned.
                            Ok(_child) => {
                                let _ = tx.send(Progress::Done);
                            }
                            Err(err) => {
                                let _ = tx.send(Progress::Failed(err.to_string()));
                            }
                        }
                    },
                    |()| Message::Noop,
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
                self.status = Status::Busy("Asking Microsoft for a code".into());

                Task::perform(
                    async move { core.auth.start_device_code().await.map_err(|e| e.to_string()) },
                    Message::DeviceCodeReady,
                )
            }

            Message::DeviceCodeReady(Ok(code)) => {
                let Some(core) = self.core.clone() else {
                    return Task::none();
                };
                self.status = Status::Idle;
                self.pending_code = Some(code.clone());

                // Opening the browser is a convenience; the code and URL stay
                // on screen so a failed open isn't a dead end.
                let _ = open::that_detached(&code.verification_uri);

                Task::perform(
                    async move {
                        let account = core
                            .auth
                            .poll_for_account(&code, || true)
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
            Message::DeviceCodeReady(Err(err)) => {
                self.status = Status::Error(err);
                Task::none()
            }

            Message::OpenVerificationUrl => {
                if let Some(code) = &self.pending_code {
                    let _ = open::that_detached(&code.verification_uri);
                }
                Task::none()
            }

            Message::SignInFinished(Ok(account)) => {
                self.pending_code = None;
                self.status = Status::Idle;
                tracing::info!(user = %account.username, "signed in");
                self.core.clone().map(reload).unwrap_or_else(Task::none)
            }
            Message::SignInFinished(Err(err)) => {
                self.pending_code = None;
                self.status = Status::Error(err);
                Task::none()
            }

            Message::SetActiveAccount(uuid) => {
                let Some(core) = self.core.clone() else {
                    return Task::none();
                };
                Task::perform(
                    async move { core.accounts.set_active(&uuid).await.map_err(|e| e.to_string()) },
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
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let body = match self.screen {
            Screen::Instances => screens::instances::view(self),
            Screen::Accounts => screens::accounts::view(self),
        };

        let content = column![screens::status_bar(&self.status), body]
            .spacing(16)
            .padding(24)
            .width(Fill)
            .height(Fill);

        row![screens::sidebar(self), content]
            .height(Fill)
            .into()
    }

    /// The account launches will use, for display in the sidebar.
    fn active_account(&self) -> Option<&Account> {
        let uuid = self.active_account.as_deref()?;
        self.accounts.iter().find(|a| a.uuid == uuid)
    }

    fn is_busy(&self) -> bool {
        matches!(self.status, Status::Busy(_))
    }
}

/// Re-reads instances and accounts from disk. Cheap, and simpler to reason
/// about than mutating the in-memory lists at every call site.
fn reload(core: Nexo) -> Task<Message> {
    Task::perform(
        async move {
            let instances = core.instances.list().await.unwrap_or_default();
            let accounts = core.accounts.list().await.unwrap_or_default();
            let active = core
                .accounts
                .active()
                .await
                .ok()
                .flatten()
                .map(|a| a.uuid);
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
                Ok(manifest) => manifest
                    .releases()
                    .map(|v| v.id.clone())
                    .take(60)
                    .collect(),
                Err(err) => {
                    tracing::warn!(%err, "could not load the Minecraft version list");
                    Vec::new()
                }
            }
        },
        Message::VersionsLoaded,
    )
}

/// Shared empty-state block, used by both screens.
fn empty_state<'a>(title: &'a str, hint: &'a str) -> Element<'a, Message> {
    container(
        column![
            text(title).size(18).color(theme::TEXT),
            text(hint).size(14).color(theme::MUTED),
        ]
        .spacing(8),
    )
    .padding(32)
    .width(Fill)
    .style(theme::card)
    .into()
}
