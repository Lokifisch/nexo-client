use crate::theme;
use crate::{App, Message};
use iced::widget::{button, column, container, image, row, scrollable, text, Space};
use iced::{Element, Fill};
use nexo_core::SkinModel;

/// Skin and cape management for the active account.
///
/// Everything here is a server-side change to the Minecraft account, not a
/// launcher setting — it applies wherever the account is used, so the copy
/// says so rather than implying it is local.
pub fn view(app: &App) -> Element<'_, Message> {
    let Some(account) = app.active_account() else {
        return crate::empty_state(
            "Sign in to change your skin",
            "Skins and capes belong to the Minecraft account, so one has to be signed in first.",
        );
    };

    let heading = crate::screens::header(
        "Skin & capes",
        "Changes apply to your Minecraft account everywhere, not just here.",
        None,
    );

    // Two columns: the character on the left, everything that changes it on
    // the right — so the effect of a change is visible next to the control
    // that made it.
    let preview: Element<'_, Message> = match &app.skin_texture {
        Some(texture) => container(
            iced::widget::shader(crate::skin3d::SkinViewer::new(
                std::sync::Arc::clone(texture),
                app.cape_texture.clone(),
                app.skin_model,
                app.skin_key,
                // No border here: this is a real skin, and the ring exists to
                // mark the signed-out placeholder.
                false,
            ))
            .width(240)
            .height(340),
        )
        .padding(16)
        .style(theme::card)
        .into(),
        None => Space::new().width(272).height(372).into(),
    };

    let controls = column![
        skin_card(app, account.skin_model),
        capes_card(app),
    ]
    .spacing(16)
    .width(Fill);

    scrollable(
        column![
            heading,
            row![preview, controls].spacing(20),
        ]
        .spacing(16)
        .width(Fill),
    )
    .height(Fill)
    .into()
}

fn skin_card(app: &App, model: SkinModel) -> Element<'_, Message> {
    // The arm width is part of the skin, not a separate preference — the same
    // texture reads wrong on the other model, so it is chosen at upload time.
    let variant = row![
        text("Model").size(13).color(theme::MUTED).width(90),
        button(text("Classic").size(13))
            .padding([7, 16])
            .style(theme::nav_button(model == SkinModel::Classic))
            .on_press(Message::SetSkinModel(SkinModel::Classic)),
        button(text("Slim").size(13))
            .padding([7, 16])
            .style(theme::nav_button(model == SkinModel::Slim))
            .on_press(Message::SetSkinModel(SkinModel::Slim)),
    ]
    .spacing(8)
    .align_y(iced::Center);

    container(
        column![
            text("Skin").size(17).color(theme::TEXT),
            text("Drag the model to turn it. Upload a 64×64 PNG; slim gives the narrower three-pixel arms.")
                .size(12)
                .color(theme::MUTED),
            Space::new().height(4),
            variant,
            Space::new().height(4),
            row![
                button(text("Upload skin…").size(13))
                    .padding([9, 18])
                    .style(theme::primary_button)
                    .on_press_maybe((!app.is_busy()).then_some(Message::UploadSkin)),
                button(text("Reset to default").size(13))
                    .padding([9, 18])
                    .style(theme::ghost_button)
                    .on_press_maybe((!app.is_busy()).then_some(Message::ResetSkin)),
            ]
            .spacing(10),
        ]
        .spacing(8),
    )
    .padding(18)
    .width(Fill)
    .style(theme::card)
    .into()
}

fn capes_card(app: &App) -> Element<'_, Message> {
    let mut body = column![
        row![
            column![
                text("Capes").size(17).color(theme::TEXT),
                text("Only capes your account already owns can be worn.")
                    .size(12)
                    .color(theme::MUTED),
            ]
            .spacing(3)
            .width(Fill),
            button(text("Refresh").size(12))
                .padding([7, 14])
                .style(theme::ghost_button)
                .on_press(Message::LoadCapes),
        ]
        .align_y(iced::Center),
    ]
    .spacing(12);

    if app.capes.is_empty() {
        body = body.push(
            text("No capes on this account.")
                .size(12)
                .color(theme::MUTED),
        );
        return container(body)
            .padding(18)
            .width(Fill)
            .style(theme::card)
            .into();
    }

    let none_worn = !app.capes.iter().any(|c| c.is_active());
    body = body.push(
        button(
            row![
                Space::new().width(40).height(64),
                text("No cape").size(14).color(theme::TEXT).width(Fill),
                if none_worn {
                    text("Worn").size(12).color(theme::MINT)
                } else {
                    text("").size(12)
                },
            ]
            .spacing(12)
            .align_y(iced::Center),
        )
        .padding(10)
        .width(Fill)
        .style(theme::bare_button)
        .on_press_maybe((!app.is_busy() && !none_worn).then_some(Message::HideCape)),
    );

    for cape in &app.capes {
        let worn = cape.is_active();
        // Cape textures are 64×32 with the design in one corner, so the whole
        // texture is shown rather than trying to crop the visible panel out.
        let preview: Element<'_, Message> = match app.cape_previews.get(&cape.id) {
            Some(handle) => image(handle.clone())
                .filter_method(image::FilterMethod::Nearest)
                .width(40)
                .height(64)
                .into(),
            None => Space::new().width(40).height(64).into(),
        };

        body = body.push(
            button(
                row![
                    preview,
                    text(cape.label()).size(14).color(theme::TEXT).width(Fill),
                    if worn {
                        text("Worn").size(12).color(theme::MINT)
                    } else {
                        text("Wear").size(12).color(theme::MUTED)
                    },
                ]
                .spacing(12)
                .align_y(iced::Center),
            )
            .padding(10)
            .width(Fill)
            .style(theme::bare_button)
            .on_press_maybe(
                (!app.is_busy() && !worn).then(|| Message::WearCape(cape.id.clone())),
            ),
        );
    }

    container(body)
        .padding(18)
        .width(Fill)
        .style(theme::card)
        .into()
}
