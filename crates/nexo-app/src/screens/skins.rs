use crate::theme;
use crate::{App, Message};
use iced::widget::{button, column, container, image, mouse_area, row, scrollable, stack, text, Space};
use iced::{Element, Fill};
use nexo_core::SkinModel;

/// Skin and cape management for the active account.
///
/// Everything here is a server-side change to the Minecraft account, not a
/// launcher setting — it applies wherever the account is used, so the copy
/// says so rather than implying it is local.
pub fn view(app: &App) -> Element<'_, Message> {
    // Only presence matters here: which model to show comes from
    // `app.skin_model`, which the toggle owns.
    if app.active_account().is_none() {
        return crate::empty_state(
            "Sign in to change your skin",
            "Skins and capes belong to the Minecraft account, so one has to be signed in first.",
        );
    }

    signed_in(app)
}

fn signed_in(app: &App) -> Element<'_, Message> {
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
                app.cape_reveal,
            ))
            .width(240)
            .height(340),
        )
        .padding(16)
        .style(theme::card)
        .into(),
        None => Space::new().width(272).height(372).into(),
    };

    // Only the controls scroll. The shader widget draws straight to the
    // surface with its own render pass, so it does not follow a scroll
    // translation — inside a scrollable it stays pinned and then gets
    // clipped. Keeping it out is also better: the preview stays visible
    // while the controls beside it are used.
    let controls = scrollable(
        // `app.skin_model` rather than `account.skin_model`: the toggle
        // updates the former, and highlighting from the latter meant the
        // model changed while the buttons stayed put.
        column![skin_card(app, app.skin_model), library_card(app), capes_card(app)]
            .spacing(16)
            .width(Fill),
    )
    .height(Fill);

    column![
        heading,
        row![preview, controls].spacing(20).height(Fill),
    ]
    .spacing(16)
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

/// Capes as a grid of thumbnails. A list of full-width rows wasted most of
/// the width on a 10x16 image, and capes are picked by looking at them.
/// Every skin the account has worn, so an old one can be put back on.
fn library_card(app: &App) -> Element<'_, Message> {
    const COLUMNS: usize = 4;

    let body = column![
        text("Your skins").size(17).color(theme::TEXT),
        text("Kept automatically whenever a skin is worn. Click one to put it back on.")
            .size(12)
            .color(theme::MUTED),
    ]
    .spacing(3);

    let mut tiles: Vec<Element<'_, Message>> = Vec::new();
    for saved in &app.saved_skins {
        let preview: Element<'_, Message> = match app.skin_previews.get(&saved.id) {
            Some(handle) => image(handle.clone())
                .filter_method(image::FilterMethod::Nearest)
                .width(48)
                .height(96)
                .into(),
            None => Space::new().width(48).height(96).into(),
        };

        // Confirmation replaces the tile's own contents rather than appearing
        // elsewhere, so the thing being deleted is the thing you are looking
        // at.
        if app.confirm_delete.as_deref() == Some(saved.id.as_str()) {
            tiles.push(
                container(
                    column![
                        text("Delete?").size(12).color(theme::TEXT),
                        row![
                            button(text("Yes").size(11))
                                .padding([4, 10])
                                .style(theme::danger_button)
                                .on_press(Message::ConfirmDeleteSkin(saved.id.clone())),
                            button(text("No").size(11))
                                .padding([4, 10])
                                .style(theme::ghost_button)
                                .on_press(Message::CancelDeleteSkin),
                        ]
                        .spacing(6),
                    ]
                    .spacing(8)
                    .align_x(iced::Center),
                )
                .padding(8)
                .height(120)
                .width(Fill)
                .center_y(Fill)
                .style(theme::card)
                .into(),
            );
            continue;
        }

        let hovered = app.hovered_skin.as_deref() == Some(saved.id.as_str());

        let tile = button(container(preview).width(Fill).center_x(Fill))
            .padding(8)
            .width(Fill)
            .height(120)
            .style(if hovered { theme::selected_tile } else { theme::tile })
            .on_press_maybe(
                (!app.is_busy()).then(|| Message::WearSavedSkin(saved.id.clone())),
            );

        // The bin is stacked over the tile rather than laid out inside it, so
        // the tile keeps one shape whether or not the cursor is on it —
        // previously it grew a row and the grid jumped as the cursor moved.
        let overlaid: Element<'_, Message> = if hovered {
            stack![
                tile,
                container(
                    button(text("🗑").size(12))
                        .padding([2, 6])
                        .style(theme::danger_button)
                        .on_press(Message::AskDeleteSkin(saved.id.clone()))
                )
                .width(Fill)
                .height(Fill)
                .align_right(Fill)
                .align_bottom(Fill)
                .padding(6),
            ]
            .into()
        } else {
            tile.into()
        };

        tiles.push(
            mouse_area(overlaid)
                .on_enter(Message::HoverSkin(Some(saved.id.clone())))
                .on_exit(Message::HoverSkin(None))
                .into(),
        );
    }

    let note: Element<'_, Message> = if app.saved_skins.is_empty() {
        text("Nothing saved yet — the skin you're wearing is added automatically.")
            .size(12)
            .color(theme::MUTED)
            .into()
    } else {
        Space::new().height(0).into()
    };

    // Pad the last row so its tiles keep the width of the rest.
    let remainder = tiles.len() % COLUMNS;
    if remainder != 0 {
        for _ in 0..(COLUMNS - remainder) {
            tiles.push(Space::new().width(Fill).into());
        }
    }

    let mut grid = column![].spacing(10);
    let mut current: Vec<Element<'_, Message>> = Vec::with_capacity(COLUMNS);
    for tile in tiles {
        current.push(tile);
        if current.len() == COLUMNS {
            grid = grid.push(row(std::mem::take(&mut current)).spacing(10));
        }
    }

    container(body.push(note).push(grid).spacing(12))
        .padding(18)
        .width(Fill)
        .style(theme::card)
        .into()
}

fn capes_card(app: &App) -> Element<'_, Message> {
    /// Tiles per row. Four fits the controls column without the tiles
    /// becoming too small to recognise a cape by.
    const COLUMNS: usize = 4;

    let body = column![row![
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
    .align_y(iced::Center)]
    .spacing(12);

    // Deliberately no early return with a different shape here. Swapping the
    // card's structure reassigns widget state further up the tree, which reset
    // the 3D viewer's pose — the model snapped to the front mid-animation.
    // The grid is simply empty instead.
    let none_worn = !app.capes.iter().any(|c| c.is_active());

    // "No cape" is a tile like any other, so taking one off is the same
    // gesture as putting one on rather than a differently-shaped action.
    let mut tiles: Vec<Element<'_, Message>> = vec![cape_tile(
        Space::new().width(60).height(96).into(),
        "No cape",
        none_worn,
        (!app.is_busy() && !none_worn).then_some(Message::HideCape),
    )];

    for cape in &app.capes {
        let worn = cape.is_active();
        let preview: Element<'_, Message> = match app.cape_previews.get(&cape.id) {
            Some(handle) => image(handle.clone())
                .filter_method(image::FilterMethod::Nearest)
                .width(60)
                .height(96)
                .into(),
            None => Space::new().width(60).height(96).into(),
        };

        tiles.push(cape_tile(
            preview,
            cape.label(),
            worn,
            (!app.is_busy() && !worn).then(|| Message::WearCape(cape.id.clone())),
        ));
    }

    // Pad the last row so its tiles keep the same width as the rest rather
    // than stretching across the gap.
    let remainder = tiles.len() % COLUMNS;
    if remainder != 0 {
        for _ in 0..(COLUMNS - remainder) {
            tiles.push(Space::new().width(Fill).into());
        }
    }

    let mut grid = column![].spacing(10);
    let mut current: Vec<Element<'_, Message>> = Vec::with_capacity(COLUMNS);
    for tile in tiles {
        current.push(tile);
        if current.len() == COLUMNS {
            grid = grid.push(row(std::mem::take(&mut current)).spacing(10));
        }
    }

    let note: Element<'_, Message> = if app.capes.is_empty() {
        text("No capes on this account yet.")
            .size(12)
            .color(theme::MUTED)
            .into()
    } else {
        Space::new().height(0).into()
    };

    container(body.push(note).push(grid))
        .padding(18)
        .width(Fill)
        .style(theme::card)
        .into()
}

fn cape_tile<'a>(
    preview: Element<'a, Message>,
    label: &'a str,
    worn: bool,
    on_press: Option<Message>,
) -> Element<'a, Message> {
    let caption = if worn {
        text("Worn").size(11).color(theme::MINT)
    } else {
        text(label).size(11).color(theme::MUTED)
    };

    button(
        column![preview, caption]
            .spacing(6)
            .align_x(iced::Center),
    )
    .padding(8)
    .width(Fill)
    // The worn one is outlined so it reads as selected without needing the
    // caption to be read.
    .style(if worn {
        theme::selected_tile
    } else {
        theme::tile
    })
    .on_press_maybe(on_press)
    .into()
}
