use crate::lively::{button, scrollable, text, text_input};
use crate::skin3d::{self, SkinViewer, Stance};
use crate::theme;
use crate::{App, Message};
use iced::widget::{column, container, row, shader, Space};
use iced::{Element, Fill};
use nexo_core::skin::{Rgba, Skin};
use nexo_core::whois::{self, Profile};
use nexo_core::SkinModel;
use std::sync::Arc;

/// One skin the player has worn, ready to draw.
#[derive(Debug)]
pub struct Worn {
    pub changed_at: Option<String>,
    pub texture: Arc<Rgba>,
    pub model: SkinModel,
}

/// A finished lookup: the profile and its skins decoded for the 3D viewer.
#[derive(Debug)]
pub struct Loaded {
    pub profile: Profile,
    pub skins: Vec<Worn>,
}

#[derive(Debug, Default)]
pub enum State {
    #[default]
    Idle,
    Loading,
    Failed(String),
    Ready(Arc<Loaded>),
}

/// Runs the lookup, then decodes the skins (few, and header-checked in `whois`).
pub async fn load(core: crate::Nexo, name: String) -> Result<Arc<Loaded>, String> {
    let profile = whois::lookup(core.http(), &name).await.map_err(|e| e.to_string())?;
    let skins = profile
        .skins
        .iter()
        .filter_map(|s| {
            let model = if s.slim { SkinModel::Slim } else { SkinModel::Classic };
            let skin = Skin::decode(&s.png, model).ok()?;
            Some(Worn { changed_at: s.changed_at.clone(), texture: Arc::new(skin.texture().clone()), model })
        })
        .collect();
    Ok(Arc::new(Loaded { profile, skins }))
}

const TILE_WIDTH: f32 = 150.0;

fn card<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(content).padding(16).width(Fill).style(theme::card).into()
}

fn heading<'a>(title: &'a str) -> Element<'a, Message> {
    text(title).size(16).font(theme::bold()).color(theme::text()).into()
}

fn info_row<'a>(label: &'a str, value: String) -> Element<'a, Message> {
    container(
        row![text(label).size(13).color(theme::muted()), Space::new().width(Fill), text(value).size(13).color(theme::text())]
            .align_y(iced::Center),
    )
    .padding([10, 14])
    .width(Fill)
    .style(theme::well)
    .into()
}

/// Name and skin history for any player, in the layout of a profile page.
pub fn view(app: &App) -> Element<'_, Message> {
    let busy = matches!(app.history, State::Loading);
    let can_search = !busy && !app.history_query.trim().is_empty() && app.core.is_some();
    let search = row![
        text_input("Look up a player…", &app.history_query)
            .on_input(Message::HistoryQuery)
            .on_submit(Message::HistorySearch)
            .padding([9, 14])
            .size(14)
            .width(360),
        button(text(if busy { "Looking up…" } else { "Look up" }).size(13).font(theme::bold()))
            .padding([8, 16])
            .style(theme::primary_button)
            .on_press_maybe(can_search.then_some(Message::HistorySearch)),
    ]
    .spacing(10)
    .align_y(iced::Center);

    let body: Element<'_, Message> = match &app.history {
        State::Idle => crate::empty_state("Name and skin history", "Type a Minecraft name above to see every name and skin it has had. Data from crafty.gg."),
        State::Loading => crate::empty_state("Looking up…", "Asking crafty.gg."),
        State::Failed(message) => crate::empty_state("Could not look that up", message),
        State::Ready(loaded) => profile_view(loaded),
    };

    column![crate::screens::header("History", "Name and skin history, from crafty.gg.", Some(search.into())), body]
        .spacing(20)
        .height(Fill)
        .into()
}

fn profile_view<'a>(loaded: &'a Loaded) -> Element<'a, Message> {
    let p = &loaded.profile;

    // Left: the skin being worn now, turning with the cursor.
    let figure: Element<'_, Message> = match loaded.skins.first() {
        Some(now) => shader(SkinViewer::new(
            Arc::clone(&now.texture),
            None,
            now.model,
            skin3d::slot(&p.uuid),
            skin3d::slot(&p.uuid),
            Stance::Idle,
        ))
        .width(Fill)
        .height(360)
        .into(),
        None => Space::new().height(360).into(),
    };
    let left = card(column![figure, text(&p.name).size(20).font(theme::bold()).color(theme::text())].spacing(8).align_x(iced::Center));

    let mut info = column![heading("Player information")].spacing(8);
    info = info.push(info_row("UUID", p.uuid.clone()));
    info = info.push(info_row("First seen", whois::date(p.created_at.as_deref())));
    info = info.push(info_row("Name changes", p.name_changes.to_string()));
    if let Some(tier) = &p.rarity {
        info = info.push(info_row("Rarity", whois::tier(tier)));
    }

    let mut names = column![heading("Username history")].spacing(8);
    for n in &p.names {
        names = names.push(info_row_name(&n.name, whois::date(n.changed_at.as_deref())));
    }

    let mut capes = column![heading("Capes")].spacing(8);
    if p.capes.is_empty() {
        capes = capes.push(text("No capes.").size(13).color(theme::muted()));
    }
    for c in &p.capes {
        capes = capes.push(info_row_name(&c.name, if c.current { "Worn".into() } else { String::new() }));
    }

    let tiles = loaded.skins.iter().enumerate().map(|(i, s)| {
        let slot = skin3d::slot(&format!("hist:{}:{i}", p.uuid));
        let view = shader(SkinViewer::new(Arc::clone(&s.texture), None, s.model, slot, slot, Stance::Showcase(slot))).width(Fill).height(200);
        let caption = if i == 0 { text("Current").size(12).color(theme::mint()) } else { text(whois::date(s.changed_at.as_deref())).size(12).color(theme::muted()) };
        container(column![view, caption].spacing(4).align_x(iced::Center))
            .padding(10)
            .width(TILE_WIDTH)
            .style(theme::well)
            .into()
    });
    let skins = column![
        heading("Skin history"),
        iced::widget::row(tiles.map(|t: Element<'_, Message>| t)).spacing(12).width(Fill).wrap().vertical_spacing(12),
    ]
    .spacing(10);

    let right = column![card(info), card(names), card(capes), card(skins)].spacing(14).width(Fill);
    scrollable(row![container(left).width(300), right].spacing(14).width(Fill)).height(Fill).into()
}

fn info_row_name<'a>(name: &'a str, detail: String) -> Element<'a, Message> {
    container(
        row![text(name).size(13).color(theme::text()), Space::new().width(Fill), text(detail).size(13).color(theme::muted())]
            .align_y(iced::Center),
    )
    .padding([10, 14])
    .width(Fill)
    .style(theme::well)
    .into()
}
