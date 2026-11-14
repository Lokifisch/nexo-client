use crate::theme;
use crate::{App, Message, Screen};
use crate::lively::button;
use iced::widget::{column, container, Space};
use crate::lively::text;
use iced::{Element, Fill};

/// The whole setup as one button, for someone who has nothing yet. Anything they
/// already have is left alone.
fn setup_prompt(app: &App) -> Element<'_, Message> {
    if let Some(reason) = &app.nexo_release_error {
        return column![
            text("Couldn't reach the Nexo download. Check your internet connection.").size(13).color(theme::text()),
            text(reason).size(11).color(theme::muted()),
            button(text("Try again").size(14))
                .padding([8, 18])
                .style(theme::ghost_button)
                .on_press(Message::FetchNexoRelease),
        ]
        .spacing(8)
        .align_x(iced::Center)
        .into();
    }

    let ready = app.nexo_release.is_some() && !app.is_busy();
    let mut prompt = column![
        button(text("Install Nexo & Play").size(16).font(theme::bold()))
            .padding([12, 28])
            .style(theme::hero_button)
            .on_press_maybe(ready.then_some(Message::QuickSetup)),
        text(match &app.nexo_release {
            Some(release) => format!(
                "One click. Installs the {} edition. The first start downloads Minecraft, so give it a few minutes.",
                release.default_edition(),
            ),
            None => "One click. The first start downloads Minecraft, so give it a few minutes.".to_string(),
        })
        .size(12)
        .color(theme::muted()),
    ]
    .spacing(8)
    .align_x(iced::Center);

    if let (label, Some(launch)) = crate::screens::last_instance_launch(app) {
        prompt = prompt.push(
            button(text(format!("{label} without Nexo")).size(13))
                .padding([6, 14])
                .style(theme::ghost_button)
                .on_press(launch),
        );
    }
    prompt.into()
}

/// Landing screen: the active account's character, front-on, with their name
/// above it. Signed out, it shows the placeholder silhouette and turns into a
/// prompt to sign in.
pub fn view(app: &App) -> Element<'_, Message> {
    let account = app.active_account();

    let name: Element<'_, Message> = match account {
        Some(account) => text(&account.username).size(26).color(theme::text()).into(),
        None => text("Not signed in").size(22).color(theme::muted()).into(),
    };

    // A Nexo cape, when worn, over the Mojang one; its own key so changing cape re-uploads it.
    let (cape, cape_key) = app.home_cape();
    let character: Element<'_, Message> = match &app.skin_texture {
        Some(texture) => iced::widget::shader(
            crate::skin3d::SkinViewer::new(
                std::sync::Arc::clone(texture),
                cape,
                app.skin_model,
                app.skin_key ^ cape_key,
                0,
                crate::skin3d::Stance::Idle,
            )
            // Only the signed-out placeholder wears the border.
            .outlined(account.is_none())
            .live(app.focused),
        )
        .width(280)
        .height(380)
        .into(),
        // Textures load a frame or two after boot; reserve the space so the
        // layout doesn't jump when they arrive.
        None => Space::new().width(280).height(380).into(),
    };

    let mut content = column![name, Space::new().height(4), character]
        .spacing(8)
        .align_x(iced::Center);

    let has_nexo = app.instances.iter().any(|i| nexo_core::nexo_mod::installed(i).is_some());
    if account.is_some() && !has_nexo {
        content = content.push(Space::new().height(16));
        content = content.push(setup_prompt(app));
    } else if account.is_none() {
        content = content.push(Space::new().height(16));
        content = content.push(
            button(text("Sign in with Microsoft").size(15))
                .padding([10, 22])
                .style(theme::primary_button)
                .on_press_maybe((!app.signing_in && app.core.is_some()).then_some(Message::StartSignIn)),
        );
        content = content.push(
            text("Opens your browser — nothing to type in.")
                .size(12)
                .color(theme::muted()),
        );
    } else if app.instances.is_empty() {
        // Nothing to start yet, so the way forward is making one.
        content = content.push(Space::new().height(16));
        content = content.push(
            button(text("Go to instances").size(14))
                .padding([8, 18])
                .style(theme::ghost_button)
                .on_press(Message::Navigate(Screen::Instances)),
        );
    } else {
        // Straight back into the last game, named so it's clear which one.
        let (label, launch) = crate::screens::last_instance_launch(app);
        content = content.push(Space::new().height(16));
        content = content.push(
            button(text(label).size(15).font(theme::bold()))
                .padding([10, 22])
                .style(theme::hero_button)
                .on_press_maybe(launch),
        );
    }

    container(content)
        .width(Fill)
        .height(Fill)
        .center_x(Fill)
        .center_y(Fill)
        .into()
}
