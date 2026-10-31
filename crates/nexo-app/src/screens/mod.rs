pub mod accounts;
pub mod home;
pub mod hosting;
pub mod instance;
pub mod instances;
pub mod paper_console;
pub mod paper_servers;
pub mod server_dashboard;
pub mod server_wizard;
pub mod skins;

use crate::theme;
use crate::{App, Message, Screen, Status};
use iced::widget::{mouse_area, stack, Space, button, column, container, image, row, svg, text, tooltip};
use iced::{Element, Fill};
use std::sync::LazyLock;

/// The logo, redrawn with strokes thick enough for 40px.
static LOGO: LazyLock<svg::Handle> =
    LazyLock::new(|| svg::Handle::from_memory(include_bytes!("../../../../assets/nexo-mark.svg")));

/// A 24px line icon. Drawn in white and recoloured by the svg style, so one
/// handle serves every state.
fn icon(path: &str) -> svg::Handle {
    svg::Handle::from_memory(format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="white" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="{path}"/></svg>"#
    ).into_bytes())
}

static HOME: LazyLock<svg::Handle> = LazyLock::new(|| icon("M3 11l9-8 9 8M5 10v10h14V10"));
static INSTANCES: LazyLock<svg::Handle> =
    LazyLock::new(|| icon("M4 4h7v7H4zM13 4h7v7h-7zM4 13h7v7H4zM13 13h7v7h-7z"));
static ACCOUNTS: LazyLock<svg::Handle> =
    LazyLock::new(|| icon("M12 12a4 4 0 100-8 4 4 0 000 8zM4 21a8 8 0 0116 0"));
static SKINS: LazyLock<svg::Handle> =
    LazyLock::new(|| icon("M8 3l-5 3 2 4 3-1v12h8V9l3 1 2-4-5-3c-1 2-3 2-4 2s-3 0-4-2z"));
static SERVERS: LazyLock<svg::Handle> =
    LazyLock::new(|| icon("M4 4h16v6H4zM4 14h16v6H4zM8 7h.01M8 17h.01"));
static PLAY: LazyLock<svg::Handle> = LazyLock::new(|| {
    svg::Handle::from_memory(
        &br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="white"><path d="M7 4.5v15l12-7.5z"/></svg>"#[..],
    )
});

/// A rail entry's hover label, to the right of it.
fn labelled<'a>(content: impl Into<Element<'a, Message>>, label: String) -> Element<'a, Message> {
    tooltip(
        content,
        container(text(label).size(13)).padding([5, 9]).style(theme::tooltip),
        tooltip::Position::Right,
    )
    .gap(8)
    .into()
}

/// Where a section's highlight sits on the rail. Follows the column in
/// [`sidebar`]: top padding, logo, its gap, the spacer, then 44px entries.
pub fn rail_y(section: &Screen) -> f32 {
    const FIRST: f32 = 14.0 + 40.0 + 10.0 + 10.0 + 10.0;
    const STEP: f32 = 44.0 + 10.0;
    let index = match section {
        Screen::Home => 0.0,
        Screen::Instances => 1.0,
        Screen::Accounts => 2.0,
        Screen::Skins => 3.0,
        _ => 4.0,
    };
    FIRST + index * STEP
}

/// Content coming into place: `progress` from 0 to 1, starting `rise` pixels low
/// and hidden behind a veil of the window's own colour. A veil because iced has
/// no opacity for a whole subtree. Same structure at every progress, so widget
/// state like scroll positions and focus survives the animation.
pub fn reveal<'a>(content: impl Into<Element<'a, Message>>, progress: f32, rise: f32) -> Element<'a, Message> {
    let hidden = 1.0 - progress.clamp(0.0, 1.0);
    stack![
        container(content).padding(iced::Padding { top: hidden * rise, ..Default::default() }).width(Fill),
        container(Space::new()).width(Fill).height(Fill).style(theme::veil(hidden)),
    ]
    .into()
}

/// A tab's underline: grows out from its centre as the tab becomes selected.
pub fn tab_underline<'a>(amount: f32) -> Element<'a, Message> {
    let bar = (amount * 100.0).round() as u16;
    let side = (100 - bar.min(100)) / 2;
    // Fixed at zero rather than a portion when fully grown, or the bar stops short.
    let edge = || if side == 0 { iced::Length::Fixed(0.0) } else { iced::Length::FillPortion(side) };
    row![
        Space::new().width(edge()),
        container(Space::new().width(Fill).height(2))
            .width(iced::Length::FillPortion(bar.max(1)))
            .style(theme::tab_underline(bar > 0)),
        Space::new().width(edge()),
    ]
    .width(Fill)
    .into()
}

/// Left navigation rail: the logo, one icon per screen, and Play at the foot.
pub fn sidebar(app: &App) -> Element<'_, Message> {
    let nav_entry = |label: &'static str, handle: &LazyLock<svg::Handle>, screen: Screen| {
        let selected = app.screen.nav_group() == screen;
        let key = ("nav", label);
        let hover = app.motion.get(key, 0.0);
        let highlight_at = app.motion.get(crate::motion_keys::RAIL, rail_y(&app.screen.nav_group()));
        // One rail step away counts as unlit.
        let lit = (1.0 - (highlight_at - rail_y(&screen)).abs() / 54.0).clamp(0.0, 1.0);
        // The icon grows slightly under the cursor; the padding gives back what
        // it takes, so the button itself holds still.
        let grow = hover * 2.0;
        labelled(
            mouse_area(
                button(
                    svg((*handle).clone())
                        .width(22.0 + grow)
                        .height(22.0 + grow)
                        .style(theme::nav_icon(lit)),
                )
                .width(44)
                .height(44)
                .padding(11.0 - grow / 2.0)
                .style(theme::nav_button(selected, hover))
                .on_press(Message::Navigate(screen)),
            )
            .on_enter(Message::Hover(key, true))
            .on_exit(Message::Hover(key, false)),
            label.to_string(),
        )
    };

    let rail = column![
        svg(LOGO.clone()).width(40).height(40),
        Space::new().height(10),
        nav_entry("Home", &HOME, Screen::Home),
        nav_entry("Instances", &INSTANCES, Screen::Instances),
        nav_entry("Accounts", &ACCOUNTS, Screen::Accounts),
        nav_entry("Skin & capes", &SKINS, Screen::Skins),
        nav_entry("Servers", &SERVERS, Screen::Servers),
        Space::new().height(Fill),
        quick_play(app),
    ]
    .spacing(10)
    .padding([14, 12])
    .align_x(iced::Center);

    // The highlight is one element behind all the entries, not part of any of
    // them, which is what lets it travel from one to the next.
    let highlight = column![
        Space::new().height(app.motion.get(crate::motion_keys::RAIL, rail_y(&app.screen.nav_group()))),
        container(Space::new()).width(44).height(44).style(theme::nav_highlight),
    ]
    .width(Fill)
    .align_x(iced::Center);

    row![
        container(stack![highlight, rail].width(Fill).height(Fill)).width(68).height(Fill).style(theme::sidebar),
        container(Space::new()).width(1).height(Fill).style(theme::divider),
    ]
    .into()
}

/// What "play the last instance" means right now: a label naming it, and the
/// launch — `None` when it can't start (nothing to play, already running,
/// signed out, busy). Shared by the rail's Play and the Home screen button so
/// the two never disagree about which instance that is.
pub fn last_instance_launch(app: &App) -> (String, Option<Message>) {
    // The store already sorts most recently played first (then by name), so
    // this is also the instance the Instances screen shows on top.
    match app.instances.first() {
        None => ("No instances yet".to_string(), None),
        Some(i) if app.instance_running(&i.id) => (format!("{} is running", i.name), None),
        Some(_) if app.active_account().is_none() => ("Sign in to play".to_string(), None),
        Some(_) if app.is_busy() => ("Busy…".to_string(), None),
        Some(i) => (format!("Play {}", i.name), Some(Message::Launch(i.id.clone(), None))),
    }
}

/// Play at the foot of the rail: launches whichever instance was played most
/// recently, so the common case — "back into the game I was in" — is one
/// click from any screen.
fn quick_play(app: &App) -> Element<'_, Message> {
    let (label, message) = last_instance_launch(app);
    let key = ("nav", "play");
    let grow = if message.is_some() { app.motion.get(key, 0.0) * 3.0 } else { 0.0 };

    labelled(
        mouse_area(
            button(
                svg(PLAY.clone())
                    .width(20.0 + grow)
                    .height(20.0 + grow)
                    .style(theme::icon_on_fill(message.is_some())),
            )
            .width(46)
            .height(46)
            .padding(13.0 - grow / 2.0)
            .style(theme::hero_button)
            .on_press_maybe(message),
        )
        .on_enter(Message::Hover(key, true))
        .on_exit(Message::Hover(key, false)),
        label,
    )
}

/// The launcher's own version, and what can be done about it.
///
/// This sits at the left of the top bar rather than as a banner on purpose: an
/// update is worth offering, not worth interrupting anyone over. The version
/// label doubles as the manual check, so the control is present even when
/// there's nothing to report.
fn updater(app: &App) -> Element<'_, Message> {
    use nexo_core::self_update::CURRENT;

    // Outlives every other state: the binary on disk is already the new one,
    // and nothing the running process reports about updates is true any more.
    if app.update_installed {
        return column![
            text("Update installed").size(12).color(theme::MINT),
            text("Restart Nexo to use it").size(11).color(theme::MUTED),
        ]
        .spacing(2)
        .into();
    }

    if app.update_busy {
        return text("Checking…").size(12).color(theme::MUTED).into();
    }

    match &app.update {
        // An update this install may actually apply.
        Some(update) if update.install.is_replaceable() => {
            // Outlined, not filled: the screen's own primary and the rail's
            // Play already glow, and an update is an offer, not the main act.
            button(text(format!("Update to {}", update.version)).size(12))
                .padding([5, 9])
                .style(theme::ghost_button)
                .on_press(Message::InstallUpdate)
                .into()
        }
        // One it may not — a packaged install, say. Saying nothing would be
        // worse than saying "there's a newer one, here's why I can't take it".
        Some(update) => column![
            text(format!("Nexo {} is out", update.version))
                .size(12)
                .color(theme::TEXT),
            text(update.install.reason().unwrap_or_default())
                .size(11)
                .color(theme::MUTED),
        ]
        .spacing(2)
        .into(),
        None => button(
            text(if app.update_checked {
                format!("v{CURRENT} · latest")
            } else {
                format!("v{CURRENT}")
            })
            .size(12)
            .color(theme::MUTED),
        )
        .padding(0)
        .style(theme::bare_button)
        .on_press_maybe(
            app.core
                .is_some()
                .then_some(Message::CheckForUpdate { announce: true }),
        )
        .into(),
    }
}

/// Top bar: the launcher's version and updates on the left, and on the right
/// the active account's face and name, or a `+` to sign in when there's
/// nobody signed in yet.
pub fn top_bar(app: &App) -> Element<'_, Message> {
    row![updater(app), Space::new().width(Fill), account_button(app)]
        .align_y(iced::Center)
        .width(Fill)
        .into()
}

fn account_button(app: &App) -> Element<'_, Message> {
    match app.active_account() {
        Some(account) => {
            let face: Element<'_, Message> = match app.faces.get(&account.uuid) {
                Some(handle) => image(handle.clone())
                    .filter_method(image::FilterMethod::Nearest)
                    .width(32)
                    .height(32)
                    .into(),
                None => Space::new().width(32).height(32).into(),
            };

            button(
                row![face, text(&account.username).size(14).color(theme::TEXT)]
                    .spacing(10)
                    .align_y(iced::Center),
            )
            .padding([5, 10])
            .style(theme::ghost_button)
            .on_press(Message::Navigate(Screen::Accounts))
            .into()
        }
        // No face to show when signed out, so the control becomes the
        // affordance for adding one.
        None => button(
            row![
                // Magenta rather than the primary violet: it echoes the warm
                // end of the placeholder silhouette's gradient, so the two
                // signed-out affordances read as one thing.
                text("+").size(20).color(theme::MAGENTA),
                text(if app.signing_in {
                    "Signing in…"
                } else {
                    "Add account"
                })
                .size(14)
                .color(theme::TEXT),
            ]
            .spacing(8)
            .align_y(iced::Center),
        )
        .padding([5, 10])
        .style(theme::ghost_button)
        .on_press_maybe((!app.signing_in && app.core.is_some()).then_some(Message::StartSignIn))
        .into(),
    }
}

/// Status strip. Occupies no space when idle so screens aren't pushed around
/// by transient messages.
pub fn status_bar(status: &Status) -> Element<'_, Message> {
    match status {
        Status::Idle => Space::new().height(0).into(),
        Status::Busy(label) => container(text(label).size(14))
            .padding([8, 14])
            .width(Fill)
            .style(theme::banner(false))
            .into(),
        Status::Error(message) | Status::Notice(message) => container(
            row![
                text(message).size(14).width(Fill),
                button(text("Dismiss").size(13))
                    .padding([4, 8])
                    .style(theme::ghost_button)
                    .on_press(Message::DismissStatus),
            ]
            .spacing(12)
            .align_y(iced::Center),
        )
        .padding([8, 14])
        .width(Fill)
        .style(theme::banner(matches!(status, Status::Error(_))))
        .into(),
    }
}

/// Screen heading with an optional action on the right.
pub fn header<'a>(
    title: &'a str,
    subtitle: &'a str,
    action: Option<Element<'a, Message>>,
) -> Element<'a, Message> {
    let mut left = column![text(title).size(24).font(theme::BOLD).color(theme::TEXT)].spacing(4);
    if !subtitle.is_empty() {
        left = left.push(text(subtitle).size(13).color(theme::MUTED));
    }

    let mut bar = row![left.width(Fill)].align_y(iced::Center);
    if let Some(action) = action {
        bar = bar.push(action);
    }
    bar.into()
}
