use crate::theme;
use crate::{App, Message, Screen};
use iced::widget::{button, column, container, text, Space};
use iced::{Element, Fill};

/// Landing screen: the active account's character, front-on, with their name
/// above it. Signed out, it shows the placeholder silhouette and turns into a
/// prompt to sign in.
pub fn view(app: &App) -> Element<'_, Message> {
    let account = app.active_account();

    let name: Element<'_, Message> = match account {
        Some(account) => text(&account.username).size(26).color(theme::TEXT).into(),
        None => text("Not signed in").size(22).color(theme::MUTED).into(),
    };

    let character: Element<'_, Message> = match &app.skin_texture {
        Some(texture) => iced::widget::shader(crate::skin3d::SkinViewer::new(
            std::sync::Arc::clone(texture),
            app.cape_texture.clone(),
            app.skin_model,
            app.skin_key,
            // Only the signed-out placeholder wears the border.
            account.is_none(),
            // Home never turns the model round; that belongs to the cape
            // screen, where the back is the thing being changed.
            0,
        ))
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

    if account.is_none() {
        content = content.push(Space::new().height(16));
        content = content.push(
            button(text("Sign in with Microsoft").size(15))
                .padding([12, 26])
                .style(theme::primary_button)
                .on_press_maybe((!app.signing_in && app.core.is_some()).then_some(Message::StartSignIn)),
        );
        content = content.push(
            text("Opens your browser — nothing to type in.")
                .size(12)
                .color(theme::MUTED),
        );
    } else {
        content = content.push(Space::new().height(16));
        content = content.push(
            button(text("Go to instances").size(14))
                .padding([10, 22])
                .style(theme::ghost_button)
                .on_press(Message::Navigate(Screen::Instances)),
        );
    }

    container(content)
        .width(Fill)
        .height(Fill)
        .center_x(Fill)
        .center_y(Fill)
        .into()
}
