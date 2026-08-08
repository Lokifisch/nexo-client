pub mod accounts;
pub mod instances;

use crate::theme;
use crate::{App, Message, Screen, Status};
use iced::widget::{button, column, container, row, rule, text, Space};
use iced::{Element, Fill};

/// Left navigation rail: brand mark, screens, and who we'd launch as.
pub fn sidebar(app: &App) -> Element<'_, Message> {
    let nav_entry = |label: &'static str, screen: Screen| {
        button(text(label).size(15))
            .width(Fill)
            .padding([10, 14])
            .style(theme::nav_button(app.screen == screen))
            .on_press(Message::Navigate(screen))
    };

    let account_footer: Element<'_, Message> = match app.active_account() {
        Some(account) => column![
            text("Signed in as").size(12).color(theme::MUTED),
            text(&account.username).size(14).color(theme::MINT),
        ]
        .spacing(2)
        .into(),
        None => column![
            text("Not signed in").size(13).color(theme::MUTED),
            button(text("Sign in").size(13))
                .padding([6, 12])
                .style(theme::ghost_button)
                .on_press(Message::Navigate(Screen::Accounts)),
        ]
        .spacing(8)
        .into(),
    };

    container(
        column![
            // Wordmark stands in for the logo until the SVG is embedded.
            text("NEXO").size(26).color(theme::VIOLET),
            text("native client").size(11).color(theme::MUTED),
            Space::new().height(24),
            nav_entry("Instances", Screen::Instances),
            nav_entry("Accounts", Screen::Accounts),
            Space::new().height(Fill),
            rule::horizontal(1),
            Space::new().height(12),
            account_footer,
        ]
        .spacing(6)
        .padding(20),
    )
    .width(220)
    .height(Fill)
    .style(theme::sidebar)
    .into()
}

/// Header strip. Occupies no space when idle so screens aren't pushed around
/// by transient messages.
pub fn status_bar(status: &Status) -> Element<'_, Message> {
    match status {
        Status::Idle => Space::new().height(0).into(),
        Status::Busy(label) => container(text(label).size(14))
            .padding([10, 16])
            .width(Fill)
            .style(theme::banner(false))
            .into(),
        Status::Error(message) => container(
            row![
                text(message).size(14).width(Fill),
                button(text("Dismiss").size(13))
                    .padding([4, 10])
                    .style(theme::ghost_button)
                    .on_press(Message::DismissStatus),
            ]
            .spacing(12)
            .align_y(iced::Center),
        )
        .padding([10, 16])
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
