//! The first thing someone sees with no account yet: what Nexo is on the left,
//! over the theme's background, and signing in on the right.

use super::{HOME, INSTANCES, LOGO, SERVERS, SKINS};
use crate::lively::{button, text};
use crate::{theme, App, Message, Status};
use iced::widget::{column, container, row, stack, svg, Space};
use iced::{Element, Fill};
use std::sync::LazyLock;

pub fn view(app: &App) -> Element<'_, Message> {
    let chip = |icon: &LazyLock<svg::Handle>, label: &'static str| -> Element<'_, Message> {
        container(
            row![
                svg((*icon).clone()).width(16).height(16).style(theme::nav_icon(1.0)),
                text(label).size(13).color(theme::text()),
            ]
            .spacing(9)
            .align_y(iced::Center),
        )
        .padding([9, 14])
        .style(theme::card)
        .into()
    };

    let brand = row![
        svg(LOGO.clone()).width(34).height(34),
        text("NEXO").size(22).font(theme::bold()).color(theme::text()),
    ]
    .spacing(12)
    .align_y(iced::Center);

    let pitch = column![
        text("MINECRAFT,").size(56).font(theme::bold()).color(theme::text()),
        text("YOUR WAY.").size(56).font(theme::bold()).color(theme::accent()),
        Space::new().height(22),
        row![
            chip(&HOME, "One-click Nexo setup"),
            chip(&SKINS, "Skins & capes"),
            chip(&INSTANCES, "Mods from Modrinth"),
            chip(&SERVERS, "Host your own server"),
        ]
        .spacing(10)
        .wrap()
        .vertical_spacing(10),
    ];

    let left = stack![
        crate::backdrop::view(app.effect, app.particles, app.focused, Fill),
        container(column![brand, Space::new().height(Fill), pitch]).padding(48).width(Fill).height(Fill),
    ];

    let ready = !app.signing_in && app.core.is_some();
    let mut panel = column![
        svg(LOGO.clone()).width(52).height(52),
        Space::new().height(6),
        text("WELCOME").size(26).font(theme::bold()).color(theme::text()),
        text("Sign in with your Microsoft account to play, and to manage your skins and capes.")
            .size(14)
            .color(theme::muted())
            .center(),
        Space::new().height(14),
        button(text(if app.signing_in { "Waiting for the browser…" } else { "Sign in with Microsoft" }).size(15).font(theme::bold()).center())
            .padding([13, 22])
            .width(Fill)
            .style(theme::primary_button)
            .on_press_maybe(ready.then_some(Message::StartSignIn)),
        button(text("Continue without an account").size(13).color(theme::muted()).center())
            .padding([8, 14])
            .width(Fill)
            .style(theme::ghost_button)
            .on_press(Message::ContinueWithoutAccount),
    ]
    .spacing(10)
    .align_x(iced::Center)
    .max_width(380);

    if !matches!(app.status, Status::Idle) {
        panel = panel.push(Space::new().height(6)).push(super::status_bar(&app.status));
    }
    panel = panel
        .push(Space::new().height(14))
        .push(container(Space::new().height(1)).width(Fill).style(|_| container::Style {
            background: Some(theme::line().into()),
            ..Default::default()
        }))
        .push(
            text("Without an account you can look around and set up instances, but not start the game.")
                .size(11)
                .color(theme::muted())
                .center(),
        );

    let right = container(panel)
        .width(440)
        .height(Fill)
        .padding(40)
        .center_x(440)
        .center_y(Fill)
        .style(|_| container::Style {
            background: Some(theme::surface().into()),
            border: iced::Border { width: 1.0, color: theme::line(), ..Default::default() },
            ..Default::default()
        });

    row![left, right].height(Fill).into()
}
