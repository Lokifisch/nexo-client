//! Capes: the account's own Minecraft capes, and Nexo's custom ones.
//!
//! A bar of filters on top, like the skin grid underneath: every tile is the
//! player's own skin seen from behind, wearing that cape.

use super::skins::{figure, section, tile, tool, wrap, PLACEHOLDER};
use crate::lively::{button, scrollable, text, text_input};
use crate::skin3d::{self, SkinViewer, Stance};
use crate::theme;
use crate::{App, Message};
use iced::widget::{column, container, mouse_area, row, stack, svg, Space};
use iced::{Element, Fill};
use nexo_core::nexo_capes::CustomCape;
use std::sync::{Arc, LazyLock};

static HEART: LazyLock<svg::Handle> =
    LazyLock::new(|| super::icon("M12 20s-7-4.4-7-10a4 4 0 017-2.6A4 4 0 0119 10c0 5.6-7 10-7 10z"));
static COPY: LazyLock<svg::Handle> = LazyLock::new(|| super::icon("M9 9h11v11H9zM5 15V4h11"));
static FLAG: LazyLock<svg::Handle> = LazyLock::new(|| super::icon("M5 21V4M5 4h11l-2 4 2 4H5"));
static DELETE: LazyLock<svg::Handle> = LazyLock::new(|| super::icon("M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3"));

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CapeTab {
    Vanilla,
    /// Every approved custom cape, most favorited first.
    #[default]
    Custom,
    Mine,
    Favorites,
    /// The review queue: only shown to reviewers.
    Review,
}

impl CapeTab {
    fn label(self) -> &'static str {
        match self {
            CapeTab::Vanilla => "Vanilla",
            CapeTab::Custom => "Custom",
            CapeTab::Mine => "Your capes",
            CapeTab::Favorites => "Favorites",
            CapeTab::Review => "Review",
        }
    }
}

pub fn view(app: &App) -> Element<'_, Message> {
    if app.active_account().is_none() {
        return crate::empty_state(
            "Sign in to choose a cape",
            "Capes belong to the Minecraft account, so one has to be signed in first.",
        );
    }
    let mine = app.capes_mine();
    let admin = mine.is_some_and(|m| m.admin);

    let mut tabs = row![].spacing(6);
    let mut list = vec![CapeTab::Vanilla, CapeTab::Custom, CapeTab::Mine, CapeTab::Favorites];
    if admin {
        list.push(CapeTab::Review);
    }
    for tab in list {
        let label = match (tab, app.capes_review()) {
            (CapeTab::Review, Some(queue)) if !queue.pending.is_empty() || !queue.reported.is_empty() => {
                format!("Review ({})", queue.pending.len() + queue.reported.len())
            }
            // A dot until the player has looked: one of their uploads was decided.
            (CapeTab::Mine, _) if app.capes_news() => "Your capes •".to_string(),
            _ => tab.label().to_string(),
        };
        tabs = tabs.push(
            button(text(label).size(13).font(theme::bold()))
                .padding([8, 14])
                .style(if app.cape_tab == tab { theme::primary_button } else { theme::ghost_button })
                .on_press(Message::CapeTab(tab)),
        );
    }

    let busy = app.is_busy();
    let toolbar = row![
        tabs,
        Space::new().width(Fill),
        text_input("Search capes…", &app.cape_query).on_input(Message::CapeQuery).padding([8, 12]).size(13).width(200),
        button(text("Template").size(13)).padding([8, 14]).style(theme::ghost_button).on_press(Message::OpenCapeTemplate),
        button(text("+ Upload").size(13).font(theme::bold()))
            .padding([8, 16])
            .style(theme::primary_button)
            .on_press_maybe((!busy).then_some(Message::UploadNexoCape)),
    ]
    .spacing(8)
    .align_y(iced::Center);

    let (title, help, grid) = match app.cape_tab {
        CapeTab::Vanilla => (
            "Vanilla capes",
            "The capes your Minecraft account owns. Everyone sees these, with or without Nexo.",
            vanilla_grid(app),
        ),
        CapeTab::Custom => (
            "Custom capes",
            "Made by Nexo players. Players with Nexo Mod see the one you wear, on your back and your elytra. Hover one to see it move.",
            column![custom_bar(app), custom_grid(app, app.capes_catalog.iter().collect(), true)].spacing(12).into(),
        ),
        CapeTab::Favorites => {
            let favorites = mine.map(|m| m.favorites.as_slice()).unwrap_or_default();
            ("Favorite capes", "Capes you gave a heart.", custom_grid(app, app.capes_catalog.iter().filter(|c| favorites.contains(&c.id)).collect(), true))
        }
        CapeTab::Mine => ("Your capes", "", mine_grid(app)),
        CapeTab::Review => ("Review", "Nobody else sees a cape until it is approved here.", review_grid(app)),
    };
    let help: Element<'_, Message> = if app.cape_tab == CapeTab::Mine {
        let (used, limit) = mine.map_or((0, 5), |m| (m.mine.iter().filter(|c| c.status.as_deref() != Some("rejected")).count(), m.limit));
        column![
            text(title).size(18).font(theme::bold()).color(theme::text()),
            text(format!("{used} of {limit} uploads used. Each is reviewed before anyone else can see or wear it. Start from the template: it shows where the cape and the elytra go."))
                .size(12)
                .color(theme::muted()),
        ]
        .spacing(3)
        .width(Fill)
        .into()
    } else {
        section(title, help)
    };

    let page = column![help, grid]
        .spacing(14)
        .width(Fill)
        .padding(iced::Padding { right: 14.0, bottom: 8.0, ..Default::default() });
    let screen = column![toolbar, scrollable(page).height(Fill)].spacing(18).height(Fill);
    match &app.cape_draft {
        Some(draft) => stack![screen, upload_popup(app, draft)].into(),
        None => screen.into(),
    }
}

/// The popup's name field, focused when it opens.
pub const NAME_FIELD: &str = "cape-upload-name";

/// Over the whole screen: the chosen cape on the player's back, and a name for it.
fn upload_popup<'a>(app: &'a App, draft: &'a crate::CapeDraft) -> Element<'a, Message> {
    let preview: Element<'_, Message> = match &draft.texture {
        // Keyed by the decoded image itself: choosing an edited file of the same name shows the edit.
        Ok(texture) => container(back(app, &format!("draft:{:p}", Arc::as_ptr(texture)), Some(Arc::clone(texture)))).width(220).into(),
        Err(_) => container(text("✕").size(40).color(theme::danger())).width(220).height(220).center_x(220).center_y(220).style(theme::well).into(),
    };
    let (used, limit) = app.capes_mine().map_or((0, 5), |m| (m.mine.iter().filter(|c| c.status.as_deref() != Some("rejected")).count(), m.limit));
    let ready = draft.texture.is_ok() && !draft.name.trim().is_empty() && !draft.sending;
    let problem = draft.texture.as_ref().err().or(draft.error.as_ref());
    let name = text_input("Cape name", &draft.name)
        .id(NAME_FIELD)
        .on_input_maybe((!draft.sending).then_some(Message::CapeDraftName))
        .on_submit_maybe(ready.then_some(Message::CapeDraftSend))
        .padding([9, 12])
        .size(14);
    let form = column![
        text("Name your cape").size(20).font(theme::bold()).color(theme::text()),
        text(format!("Shown under it to everyone, with your name, once it is approved. {used} of {limit} uploads used."))
            .size(12)
            .color(theme::muted()),
        Space::new().height(6),
        name,
        text(format!("{}/32", draft.name.chars().count())).size(11).color(theme::muted()),
        match problem {
            Some(problem) => Element::from(text(problem.clone()).size(12).color(theme::danger())),
            None => Space::new().height(0).into(),
        },
        Space::new().height(Fill),
        row![
            Space::new().width(Fill),
            button(text("Cancel").size(13)).padding([8, 16]).style(theme::ghost_button).on_press_maybe((!draft.sending).then_some(Message::CapeDraftCancel)),
            button(text(if draft.sending { "Uploading…" } else { "Upload for review" }).size(13).font(theme::bold()))
                .padding([8, 16])
                .style(theme::primary_button)
                .on_press_maybe(ready.then_some(Message::CapeDraftSend)),
        ]
        .spacing(8),
    ]
    .spacing(6)
    .width(Fill)
    .height(240);
    let card = container(row![preview, form].spacing(22).align_y(iced::Center))
        .padding(22)
        .width(640)
        .style(theme::panel);
    // The dimmed page underneath takes no clicks while the popup is open.
    iced::widget::opaque(
        container(crate::lively::appear(card))
            .width(Fill)
            .height(Fill)
            .center_x(Fill)
            .center_y(Fill)
            .style(theme::veil(0.72)),
    )
}

/// The player's skin from behind wearing `cape`; `name` keys the 3D view.
fn back<'a>(app: &'a App, name: &str, cape: Option<Arc<nexo_core::skin::Rgba>>) -> Element<'a, Message> {
    let skin = app.skin_texture.as_ref().unwrap_or(&PLACEHOLDER);
    let slot = skin3d::slot(&format!("{name}:{}", cape.is_some()));
    figure(SkinViewer::new(Arc::clone(skin), cape, app.skin_model, slot ^ app.skin_key, slot, Stance::Back).live(app.focused))
}

fn title<'a>(name: &str) -> Element<'a, Message> {
    text(name.to_string()).size(15).font(theme::bold()).color(theme::text()).into()
}

fn caption<'a>(line: String, highlight: bool) -> Element<'a, Message> {
    text(line).size(12).color(if highlight { theme::mint() } else { theme::muted() }).into()
}

fn matches(app: &App, name: &str) -> bool {
    let query = app.cape_query.trim().to_lowercase();
    query.is_empty() || name.to_lowercase().contains(&query)
}

/// Search by a custom cape's name, its uploader, or its code.
fn matches_cape(app: &App, cape: &CustomCape) -> bool {
    let query = app.cape_query.trim().to_lowercase();
    let code = |c: &str| c.replace('-', "").to_lowercase();
    query.is_empty()
        || cape.name.to_lowercase().contains(&query)
        || cape.uploader.as_deref().is_some_and(|u| u.to_lowercase().contains(&query))
        || (query.len() >= 4 && code(&cape.code).starts_with(&code(&query)))
}

/// Above Custom: the order, and a friend's code to wear straight away.
fn custom_bar(app: &App) -> Element<'_, Message> {
    let mut sorts = row![].spacing(6);
    for sort in nexo_core::nexo_capes::CapeSort::ALL {
        sorts = sorts.push(
            button(text(sort.label()).size(12))
                .padding([6, 12])
                .style(if app.cape_sort == sort { theme::primary_button } else { theme::ghost_button })
                .on_press(Message::CapeSort(sort)),
        );
    }
    let ready = nexo_core::nexo_capes::parse_code(&app.cape_code).is_some() && !app.is_busy();
    row![
        sorts,
        Space::new().width(Fill),
        text_input("Cape code, like 3F9A-2C1B", &app.cape_code)
            .on_input(Message::CapeCode)
            .on_submit_maybe(ready.then_some(Message::WearByCode))
            .padding([7, 12])
            .size(13)
            .width(220),
        button(text("Wear code").size(12).font(theme::bold()))
            .padding([7, 14])
            .style(theme::primary_button)
            .on_press_maybe(ready.then_some(Message::WearByCode)),
    ]
    .spacing(8)
    .align_y(iced::Center)
    .into()
}

/// Who made it and how it is doing, under a cape's name.
fn cape_line(cape: &CustomCape) -> String {
    let by = cape.uploader.as_deref().map(|u| format!("by {u} · ")).unwrap_or_default();
    format!("{by}♥ {} · {} wearing", cape.favorites, cape.uses)
}

/// One tile per cape the account owns, plus "No cape".
fn vanilla_grid(app: &App) -> Element<'_, Message> {
    let busy = app.is_busy();
    let none_worn = !app.capes.iter().any(|c| c.is_active());
    // "No cape" is a tile like any other, so taking one off is the same
    // gesture as putting one on.
    let mut tiles = vec![tile(
        back(app, "cape:", None),
        title("No cape"),
        caption(if none_worn { "Wearing".into() } else { "Click to wear".into() }, none_worn),
        none_worn,
        (!busy && !none_worn).then_some(Message::HideCape),
    )];
    for (index, cape) in app.capes.iter().filter(|c| matches(app, c.label())).enumerate() {
        let worn = cape.is_active();
        let texture = app.cape_textures.get(&cape.id).cloned();
        let tile = tile(
            back(app, &format!("cape:{}", cape.id), texture),
            title(cape.label()),
            caption(if worn { "Wearing".into() } else { "Click to wear".into() }, worn),
            worn,
            (!busy && !worn).then(|| Message::WearCape(cape.id.clone())),
        );
        tiles.push(super::arrive(app, index + 1, tile));
    }
    if app.capes.is_empty() {
        tiles.push(text("No capes on this account yet.").size(12).color(theme::muted()).into());
    }
    wrap(tiles)
}

/// Approved custom capes: click to wear, heart to favorite.
fn custom_grid<'a>(app: &'a App, capes: Vec<&'a CustomCape>, with_none: bool) -> Element<'a, Message> {
    let busy = app.is_busy();
    let mine = app.capes_mine();
    let worn = mine.and_then(|m| m.worn.as_deref());
    let mut tiles = Vec::new();
    if with_none && app.cape_query.trim().is_empty() {
        tiles.push(tile(
            back(app, "ncape:", None),
            title("No custom cape"),
            caption(if worn.is_none() { "Your vanilla cape shows".into() } else { "Click to take it off".into() }, worn.is_none()),
            worn.is_none(),
            (!busy && worn.is_some() && mine.is_some()).then_some(Message::WearNexoCape(None)),
        ));
    }
    for (index, cape) in capes.into_iter().filter(|c| matches_cape(app, c)).enumerate() {
        let wearing = worn == Some(cape.id.as_str());
        let liked = mine.is_some_and(|m| m.favorites.contains(&cape.id));
        let line = if wearing { format!("{} · you", cape_line(cape)) } else { cape_line(cape) };
        let body = tile(
            back(app, &format!("ncape:{}", cape.id), app.nexo_cape_textures.get(&cape.id).cloned()),
            title(&cape.name),
            caption(line, wearing),
            wearing,
            (!busy && !wearing && mine.is_some()).then(|| Message::WearNexoCape(Some(cape.id.clone()))),
        );
        let heart = button(svg(HEART.clone()).width(14).height(14).style(theme::nav_icon(if liked { 1.0 } else { 0.35 })))
            .padding(6)
            .style(if liked { theme::primary_button } else { theme::ghost_button })
            .on_press_maybe(mine.is_some().then(|| Message::FavoriteNexoCape(cape.id.clone(), !liked)));
        let tools: Element<'_, Message> = if app.cape_report_armed.as_deref() == Some(cape.id.as_str()) {
            row![
                button(text("Report").size(12)).padding([5, 10]).style(theme::danger_button).on_press(Message::ReportNexoCape(cape.id.clone())),
                button(text("Keep").size(12)).padding([5, 10]).style(theme::ghost_button).on_press(Message::ArmCapeReport(None)),
            ]
            .spacing(6)
            .into()
        } else {
            column![
                heart,
                tool(&COPY, theme::ghost_button, Message::CopyCapeCode(cape.code.clone())),
                tool(&FLAG, theme::ghost_button, Message::ArmCapeReport(Some(cape.id.clone()))),
            ]
            .spacing(6)
            .into()
        };
        let overlay = container(tools).width(Fill).align_right(Fill).padding(8);
        tiles.push(super::arrive(app, index + 1, stack![body, overlay]));
    }
    if tiles.len() <= usize::from(with_none) {
        let empty = match app.cape_tab {
            CapeTab::Favorites => "No favorites yet. Give a cape a heart in Custom.",
            _ if !app.cape_query.trim().is_empty() => "No cape has that name.",
            _ if app.capes_catalog_error.is_some() => "Could not reach the cape service.",
            _ => "No custom capes yet. Be the first: upload one.",
        };
        tiles.push(text(empty).size(12).color(theme::muted()).into());
    }
    wrap(tiles)
}

fn status_line(cape: &CustomCape) -> (String, bool) {
    match cape.status.as_deref() {
        Some("pending") => ("Waiting for review".into(), false),
        Some("rejected") => (format!("Rejected{}", cape.reason.as_deref().map(|r| format!(": {r}")).unwrap_or_default()), false),
        _ => (format!("Approved · {} · ♥ {} · {} wearing", cape.code, cape.favorites, cape.uses), true),
    }
}

/// This account's uploads, whatever their state.
fn mine_grid(app: &App) -> Element<'_, Message> {
    let Some(mine) = app.capes_mine() else {
        return text(app.capes_mine_error.as_deref().unwrap_or("Signing in to the cape service…")).size(12).color(theme::muted()).into();
    };
    let busy = app.is_busy();
    let mut tiles = Vec::new();
    for (index, cape) in mine.mine.iter().filter(|c| matches(app, &c.name)).enumerate() {
        let (line, approved) = status_line(cape);
        let wearing = mine.worn.as_deref() == Some(cape.id.as_str());
        let body = tile(
            back(app, &format!("ncape:{}", cape.id), app.nexo_cape_textures.get(&cape.id).cloned()),
            title(&cape.name),
            caption(line, approved),
            wearing,
            (!busy && approved && !wearing).then(|| Message::WearNexoCape(Some(cape.id.clone()))),
        );
        let armed = app.cape_delete_armed.as_deref() == Some(cape.id.as_str());
        let tools: Element<'_, Message> = if armed {
            container(
                row![
                    button(text("Delete").size(12)).padding([5, 10]).style(theme::danger_button).on_press(Message::DeleteNexoCape(cape.id.clone())),
                    button(text("Keep").size(12)).padding([5, 10]).style(theme::ghost_button).on_press(Message::ArmCapeDelete(None)),
                ]
                .spacing(6),
            )
            .width(Fill)
            .align_right(Fill)
            .padding(8)
            .into()
        } else {
            container(tool(&DELETE, theme::danger_button, Message::ArmCapeDelete(Some(cape.id.clone())))).width(Fill).align_right(Fill).padding(8).into()
        };
        tiles.push(super::arrive(app, index, mouse_area(stack![body, tools])));
    }
    if tiles.is_empty() {
        tiles.push(text("You have not uploaded a cape yet. Download the template, paint it, then upload it.").size(12).color(theme::muted()).into());
    }
    wrap(tiles)
}

/// Waiting capes with Approve and Reject; approved ones below, to take down.
fn review_grid(app: &App) -> Element<'_, Message> {
    let Some(queue) = app.capes_review() else {
        return text("Loading the queue…").size(12).color(theme::muted()).into();
    };
    let busy = app.is_busy();
    let card = |cape: &CustomCape, waiting: bool| -> Element<'_, Message> {
        let actions = if waiting && cape.reports > 0 {
            row![
                button(text("Keep").size(12)).padding([5, 10]).style(theme::primary_button).on_press_maybe((!busy).then(|| Message::ReviewNexoCape(cape.id.clone(), true))),
                button(text("Take down").size(12)).padding([5, 10]).style(theme::danger_button).on_press_maybe((!busy).then(|| Message::ReviewNexoCape(cape.id.clone(), false))),
            ]
        } else if waiting {
            row![
                button(text("Approve").size(12)).padding([5, 10]).style(theme::primary_button).on_press_maybe((!busy).then(|| Message::ReviewNexoCape(cape.id.clone(), true))),
                button(text("Reject").size(12)).padding([5, 10]).style(theme::danger_button).on_press_maybe((!busy).then(|| Message::ReviewNexoCape(cape.id.clone(), false))),
            ]
        } else {
            row![button(text("Take down").size(12)).padding([5, 10]).style(theme::danger_button).on_press_maybe((!busy).then(|| Message::ReviewNexoCape(cape.id.clone(), false)))]
        };
        column![
            tile(
                back(app, &format!("ncape:{}", cape.id), app.nexo_cape_textures.get(&cape.id).cloned()),
                title(&cape.name),
                caption(
                    if cape.reports > 0 {
                        format!("{} reports · {}", cape.reports, cape.uploader.as_deref().unwrap_or("unknown uploader"))
                    } else {
                        format!("{} · by {}", cape.code, cape.uploader.as_deref().unwrap_or("unknown"))
                    },
                    false,
                ),
                false,
                None,
            ),
            container(actions.spacing(6)).center_x(Fill),
        ]
        .spacing(6)
        .into()
    };
    let pending: Vec<_> = queue.pending.iter().map(|c| card(c, true)).collect();
    let reported: Vec<_> = queue.reported.iter().map(|c| card(c, true)).collect();
    let approved: Vec<_> = queue.approved.iter().filter(|c| matches(app, &c.name)).map(|c| card(c, false)).collect();
    let none = |what: &'static str| -> Element<'_, Message> { text(what).size(12).color(theme::muted()).into() };
    column![
        section("Waiting", "Approve to show it in Custom; reject to keep it from everyone but its owner."),
        if pending.is_empty() { none("Nothing waiting.") } else { wrap(pending) },
        Space::new().height(10),
        section("Reported", "Approved capes players flagged. Keep clears the reports; Take down removes it from everyone."),
        if reported.is_empty() { none("Nothing reported.") } else { wrap(reported) },
        Space::new().height(10),
        section("Approved", "Taking one down takes it off everyone wearing it."),
        if approved.is_empty() { none("None yet.") } else { wrap(approved) },
    ]
    .spacing(14)
    .into()
}
