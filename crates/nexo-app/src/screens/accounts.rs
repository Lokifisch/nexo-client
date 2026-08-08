use crate::theme;
use crate::{empty_state, App, Message};
use iced::widget::{button, column, container, image, row, scrollable, text, Space};
use iced::{Element, Fill};
use nexo_core::Account;

pub fn view(app: &App) -> Element<'_, Message> {
    let add = button(
        text(if app.signing_in {
            "Waiting for browser…"
        } else {
            "Add account"
        })
        .size(14),
    )
    .padding([10, 20])
    .style(theme::primary_button)
    .on_press_maybe((!app.signing_in && app.core.is_some()).then_some(Message::StartSignIn));

    let list: Element<'_, Message> = if app.accounts.is_empty() {
        empty_state(
            "No accounts signed in",
            "Sign in with the Microsoft account that owns Minecraft: Java Edition. \
             Your browser opens on Microsoft's page — there's no code to type.",
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
        "Signed in through Microsoft in your browser.",
        Some(add.into()),
    )]
    .spacing(20)
    .height(Fill);

    if app.signing_in {
        content = content.push(waiting_notice());
    }

    content.push(list).into()
}

/// Shown while the browser tab is open. The sign-in happens entirely out
/// there, so this exists to explain why the app appears to be idling.
fn waiting_notice() -> Element<'static, Message> {
    container(
        column![
            text("Finish signing in in your browser")
                .size(16)
                .color(theme::TEXT),
            text(
                "A tab should have opened on Microsoft's sign-in page. \
                 This window picks up automatically once you're done."
            )
            .size(13)
            .color(theme::MUTED),
        ]
        .spacing(8),
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

    // Only the active account's skin is rendered, so the face is shown just
    // for that row; the rest would need their own fetches for little gain.
    let avatar: Element<'a, Message> = match (&app.face, is_active) {
        (Some(handle), true) => image(handle.clone())
            .filter_method(image::FilterMethod::Nearest)
            .into(),
        _ => Space::new().width(32).height(32).into(),
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

    container(
        row![avatar, details, actions]
            .spacing(14)
            .align_y(iced::Center),
    )
    .padding(16)
    .width(Fill)
    .style(theme::card)
    .into()
}
