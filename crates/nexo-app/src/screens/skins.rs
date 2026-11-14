use crate::lively::{button, scrollable, text, text_input};
use crate::skin3d::{self, SkinViewer, Stance};
use crate::theme;
use crate::{App, Message};
use iced::widget::{column, container, mouse_area, row, shader, stack, svg, Space};
use iced::{Element, Fill};
use nexo_core::SkinModel;
use std::sync::{Arc, LazyLock};

static RENAME: LazyLock<svg::Handle> = LazyLock::new(|| super::icon("M4 20h4L19 9l-4-4L4 16v4zM14 6l4 4"));
static DELETE: LazyLock<svg::Handle> = LazyLock::new(|| super::icon("M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3"));
static DONE: LazyLock<svg::Handle> = LazyLock::new(|| super::icon("M5 12l5 5L20 7"));
pub(super) static PLACEHOLDER: LazyLock<Arc<nexo_core::skin::Rgba>> = LazyLock::new(|| Arc::new(nexo_core::skin::placeholder_texture()));

/// A small icon button over a tile.
pub(super) fn tool<'a>(icon: &LazyLock<svg::Handle>, style: fn(&iced::Theme, iced::widget::button::Status) -> iced::widget::button::Style, message: Message) -> Element<'a, Message> {
    button(svg((*icon).clone()).width(14).height(14).style(theme::nav_icon(1.0))).padding(6).style(style).on_press(message).into()
}

/// Every tile is the same size, so the grids line up and wrap cleanly.
pub(super) const TILE_WIDTH: f32 = 196.0;
pub(super) const FIGURE_HEIGHT: f32 = 220.0;

/// Skin management for the active account. Capes have their own screen.
///
/// Everything here is a server-side change to the Minecraft account, not a
/// launcher setting — it applies wherever the account is used, so the copy
/// says so rather than implying it is local.
pub fn view(app: &App) -> Element<'_, Message> {
    if app.active_account().is_none() {
        return crate::empty_state(
            "Sign in to change your skin",
            "Skins and capes belong to the Minecraft account, so one has to be signed in first.",
        );
    }

    let busy = app.is_busy();
    let toolbar = row![
        text_input("Search skins…", &app.skin_query)
            .on_input(Message::SkinQuery)
            .padding([9, 14])
            .size(14)
            .width(360),
        Space::new().width(Fill),
        button(text("Reset to default").size(13))
            .padding([8, 14])
            .style(theme::ghost_button)
            .on_press_maybe((!busy).then_some(Message::ResetSkin)),
        button(text("+ Add skin").size(13).font(theme::bold()))
            .padding([8, 16])
            .style(theme::primary_button)
            .on_press_maybe((!busy).then_some(Message::UploadSkin)),
    ]
    .spacing(10)
    .align_y(iced::Center);

    let page = column![
        section("Skins", "Changes apply to your Minecraft account everywhere, not just here. Every skin you wear is kept here."),
        skins_grid(app),
    ]
    .spacing(14)
    .width(Fill)
    .padding(iced::Padding { right: 14.0, bottom: 8.0, ..Default::default() });

    column![toolbar, scrollable(page).height(Fill)].spacing(18).height(Fill).into()
}

pub(super) fn section<'a>(title: &'a str, help: &'a str) -> Element<'a, Message> {
    column![
        text(title).size(18).font(theme::bold()).color(theme::text()),
        text(help).size(12).color(theme::muted()),
    ]
    .spacing(3)
    .width(Fill)
    .into()
}

/// Tiles in rows as wide as the window allows. Each is held to its width
/// here: the arrival animation and the hover tools fill whatever they're given, and a
/// tile that fills the row wraps alone.
pub(super) fn wrap<'a>(tiles: Vec<Element<'a, Message>>) -> Element<'a, Message> {
    let tiles = tiles.into_iter().map(|tile| container(tile).width(TILE_WIDTH).into());
    row(tiles).spacing(14).width(Fill).wrap().vertical_spacing(14).into()
}

pub(super) fn figure<'a>(viewer: SkinViewer) -> Element<'a, Message> {
    shader(viewer).width(Fill).height(FIGURE_HEIGHT).into()
}

/// A tile: the figure, a name and a line under it. `chosen` outlines it.
pub(super) fn tile<'a>(figure: Element<'a, Message>, name: Element<'a, Message>, detail: Element<'a, Message>, chosen: bool, on_press: Option<Message>) -> Element<'a, Message> {
    button(column![figure, name, detail].spacing(4).align_x(iced::Center))
        .padding([10, 10])
        .width(TILE_WIDTH)
        .style(if chosen { theme::selected_tile } else { theme::tile })
        .on_press_maybe(on_press)
        .into()
}

fn model_label(model: SkinModel) -> &'static str {
    match model {
        SkinModel::Classic => "Classic",
        SkinModel::Slim => "Slim",
    }
}

fn skins_grid(app: &App) -> Element<'_, Message> {
    let busy = app.is_busy();
    let query = app.skin_query.trim().to_lowercase();
    let worn_pixels = app.skin_texture.as_ref().map(|texture| &texture.pixels);

    let mut tiles: Vec<Element<'_, Message>> = Vec::new();
    // The way in for a new one is a tile like the rest.
    if query.is_empty() {
        tiles.push(add_tile(busy));
    }

    let mut index = 0;
    for saved in &app.saved_skins {
        let name = saved.name.as_deref().unwrap_or("Saved skin");
        if !query.is_empty() && !name.to_lowercase().contains(&query) {
            continue;
        }
        let Some((texture, model)) = app.saved_textures.get(&saved.id) else { continue };
        let worn = worn_pixels == Some(&texture.pixels);
        tiles.push(super::arrive(app, index, skin_tile(app, saved, name, texture, *model, worn, busy)));
        index += 1;
    }

    if tiles.is_empty() {
        return text("No saved skin has that name.").size(13).color(theme::muted()).into();
    }
    wrap(tiles)
}

fn add_tile<'a>(busy: bool) -> Element<'a, Message> {
    let plus = container(text("+").size(30).font(theme::bold()).color(theme::accent()))
        .width(64)
        .height(64)
        .center_x(64)
        .center_y(64)
        .style(theme::well);
    let figure = container(plus).width(Fill).height(FIGURE_HEIGHT).center_x(Fill).center_y(FIGURE_HEIGHT);
    tile(
        figure.into(),
        text("Add new skin").size(15).font(theme::bold()).color(theme::text()).into(),
        text("Upload a 64×64 PNG").size(12).color(theme::muted()).into(),
        false,
        (!busy).then_some(Message::UploadSkin),
    )
}

#[allow(clippy::too_many_arguments)]
fn skin_tile<'a>(
    app: &'a App,
    saved: &'a nexo_core::skin_library::SavedSkin,
    name: &'a str,
    texture: &Arc<nexo_core::skin::Rgba>,
    model: SkinModel,
    worn: bool,
    busy: bool,
) -> Element<'a, Message> {
    let slot = skin3d::slot(&saved.id);
    // The pose comes from the skin's id, mixed with a number drawn at launch.
    let figure = figure(SkinViewer::new(Arc::clone(texture), None, model, slot, slot, Stance::Showcase(slot)));

    // Deleting asks first, in the tile itself, so the thing being deleted is
    // the thing you are looking at.
    if app.confirm_delete.as_deref() == Some(saved.id.as_str()) {
        let ask = row![
            button(text("Delete").size(12))
                .padding([5, 10])
                .style(theme::danger_button)
                .on_press(Message::ConfirmDeleteSkin(saved.id.clone())),
            button(text("Keep").size(12))
                .padding([5, 10])
                .style(theme::ghost_button)
                .on_press(Message::CancelDeleteSkin),
        ]
        .spacing(6);
        return tile(figure, text(format!("Delete {name}?")).size(13).color(theme::text()).into(), ask.into(), false, None);
    }

    let renaming = app.renaming_skin.as_ref().filter(|(id, _)| id == &saved.id);
    let title: Element<'_, Message> = match renaming {
        Some((_, draft)) => text_input("Name", draft)
            .on_input(Message::RenameDraft)
            .on_submit(Message::RenameDone)
            .size(14)
            .padding([4, 8])
            .into(),
        None => text(name).size(15).font(theme::bold()).color(theme::text()).into(),
    };
    let detail: Element<'_, Message> = if worn {
        text(format!("{} · Wearing", model_label(model))).size(12).color(theme::mint()).into()
    } else {
        text(model_label(model)).size(12).color(theme::muted()).into()
    };
    let wear = (!busy && !worn && renaming.is_none()).then(|| Message::WearSavedSkin(saved.id.clone()));
    let body = tile(figure, title, detail, worn, wear);

    // The buttons sit over the tile rather than in it, so the tile keeps one
    // shape whether or not the cursor is on it.
    let hovered = app.hovered_skin.as_deref() == Some(saved.id.as_str());
    let tools: Element<'_, Message> = if hovered && renaming.is_none() {
        container(
            column![
                tool(&RENAME, theme::ghost_button, Message::RenameSkin(saved.id.clone())),
                tool(&DELETE, theme::danger_button, Message::AskDeleteSkin(saved.id.clone())),
            ]
            .spacing(6),
        )
        .width(Fill)
        .align_right(Fill)
        .padding(8)
        .into()
    } else if renaming.is_some() {
        container(tool(&DONE, theme::primary_button, Message::RenameDone))
            .width(Fill)
            .align_right(Fill)
            .padding(8)
            .into()
    } else {
        Space::new().into()
    };

    mouse_area(stack![body, tools])
        .on_enter(Message::HoverSkin(Some(saved.id.clone())))
        .on_exit(Message::UnhoverSkin(saved.id.clone()))
        .into()
}
