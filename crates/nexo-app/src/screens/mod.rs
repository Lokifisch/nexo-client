pub mod accounts;
pub mod home;
pub mod instance;
pub mod instances;
pub mod skins;

use crate::theme;
use crate::{App, Message, Screen, Status};
use iced::widget::{Space, button, column, container, image, row, rule, text};
use iced::{Element, Fill};

/// Left navigation rail.
pub fn sidebar(app: &App) -> Element<'_, Message> {
    let nav_entry = |label: &'static str, screen: Screen| {
        button(text(label).size(15))
            .width(Fill)
            .padding([8, 12])
            .style(theme::nav_button(app.screen.nav_group() == screen))
            .on_press(Message::Navigate(screen))
    };

    container(
        column![
            // Wordmark stands in for the logo until the SVG is embedded.
            text("NEXO").size(26).color(theme::VIOLET),
            text("native client").size(11).color(theme::MUTED),
            Space::new().height(24),
            nav_entry("Home", Screen::Home),
            nav_entry("Instances", Screen::Instances),
            nav_entry("Accounts", Screen::Accounts),
            nav_entry("Skin & capes", Screen::Skins),
            Space::new().height(Fill),
            rule::horizontal(1),
            Space::new().height(12),
            text(format!("{} instance(s)", app.instances.len()))
                .size(12)
                .color(theme::MUTED),
            updater(app),
        ]
        .spacing(6)
        .padding(20),
    )
    .width(220)
    .height(Fill)
    .style(theme::sidebar)
    .into()
}

/// The launcher's own version, and what can be done about it.
///
/// This lives in the sidebar footer rather than as a banner on purpose: an
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
            button(text(format!("Update to {}", update.version)).size(12))
                .width(Fill)
                .padding([5, 9])
                .style(theme::primary_button)
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

/// Top bar. Its only occupant is the account control on the right: the
/// active account's face and name, or a `+` to sign in when there's nobody
/// signed in yet.
pub fn top_bar(app: &App) -> Element<'_, Message> {
    row![Space::new().width(Fill), account_button(app)]
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
        Status::Error(message) => container(
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
        .style(theme::banner(true))
        .into(),
    }
}

/// Screen heading with an optional action on the right.
pub fn header<'a>(
    title: &'a str,
    subtitle: &'a str,
    action: Option<Element<'a, Message>>,
) -> Element<'a, Message> {
    let mut left = column![text(title).size(24).color(theme::TEXT)].spacing(4);
    if !subtitle.is_empty() {
        left = left.push(text(subtitle).size(13).color(theme::MUTED));
    }

    let mut bar = row![left.width(Fill)].align_y(iced::Center);
    if let Some(action) = action {
        bar = bar.push(action);
    }
    bar.into()
}
