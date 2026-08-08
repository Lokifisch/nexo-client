use crate::theme;
use crate::{empty_state, App, Message};
use iced::widget::{button, column, container, row, scrollable, text};
use iced::{Element, Fill};
use nexo_core::Account;

pub fn view(app: &App) -> Element<'_, Message> {
    let sign_in = button(text("Add account").size(14))
        .padding([10, 20])
        .style(theme::primary_button)
        .on_press_maybe(
            (app.core.is_some() && app.pending_code.is_none()).then_some(Message::StartSignIn),
        );

    let list: Element<'_, Message> = if app.accounts.is_empty() {
        empty_state(
            "No accounts signed in",
            "Sign in with the Microsoft account that owns Minecraft: Java Edition.",
        )
    } else {
        scrollable(
            column(app.accounts.iter().map(|account| card(app, account)))
                .spacing(12)
                .width(Fill),
        )
        .height(Fill)
        .into()
    };

    let mut content = column![crate::screens::header(
        "Accounts",
        "Signed in through Microsoft's device-code flow.",
        Some(sign_in.into()),
    )]
    .spacing(20)
    .height(Fill);

    if let Some(code) = &app.pending_code {
        content = content.push(device_code_prompt(code));
    }

    content.push(list).into()
}

/// The waiting state: shows the code prominently, since the user has to read
/// it off the screen and type it into a browser on any device.
fn device_code_prompt(code: &nexo_core::DeviceCode) -> Element<'_, Message> {
    container(
        column![
            text("Finish signing in").size(16).color(theme::TEXT),
            text("Go to this page and enter the code below. This window keeps waiting.")
                .size(13)
                .color(theme::MUTED),
            row![
                text(&code.verification_uri).size(14).color(theme::MINT),
                button(text("Open in browser").size(13))
                    .padding([6, 12])
                    .style(theme::ghost_button)
                    .on_press(Message::OpenVerificationUrl),
            ]
            .spacing(12)
            .align_y(iced::Center),
            // Oversized because it gets transcribed by hand, often onto a
            // phone; magenta rather than the primary violet so it reads as
            // the thing to act on, not just another accented label.
            text(&code.user_code).size(34).color(theme::MAGENTA),
        ]
        .spacing(10),
    )
    .padding(20)
    .width(Fill)
    .style(theme::highlight)
    .into()
}

fn card<'a>(app: &'a App, account: &'a Account) -> Element<'a, Message> {
    let is_active = app.active_account.as_deref() == Some(account.uuid.as_str());

    let status = if is_active {
        text("Active — launches use this account")
            .size(12)
            .color(theme::MINT)
    } else if account.is_expired() {
        // Not an error: the token is refreshed silently at launch. Saying so
        // avoids the user re-adding an account that works fine.
        text("Session expired — renews automatically on next launch")
            .size(12)
            .color(theme::MUTED)
    } else {
        text("Signed in").size(12).color(theme::MUTED)
    };

    let details = column![
        text(&account.username).size(16).color(theme::TEXT),
        status,
    ]
    .spacing(3)
    .width(Fill);

    let mut actions = row![].spacing(10).align_y(iced::Center);

    if !is_active {
        actions = actions.push(
            button(text("Use this one").size(13))
                .padding([8, 14])
                .style(theme::ghost_button)
                .on_press(Message::SetActiveAccount(account.uuid.clone())),
        );
    }

    actions = actions.push(
        button(text("Sign out").size(13))
            .padding([8, 14])
            .style(theme::danger_button)
            .on_press(Message::RemoveAccount(account.uuid.clone())),
    );

    container(row![details, actions].spacing(12).align_y(iced::Center))
        .padding(16)
        .width(Fill)
        .style(theme::card)
        .into()
}
