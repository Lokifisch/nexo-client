//! The create-a-server wizard (type → add-ons → world → performance →
//! review) and the setup screen that follows it. State and messages are in
//! `hosting`; this is only drawing.

use super::hosting::{self, DIFFICULTIES, Essential, GAME_MODES, Msg, Phase, Probe, Setup, Step, WORLD_TYPES, Wizard};
use crate::{App, Message, theme};
use crate::lively::button;
use crate::lively::checkbox;
use crate::dropdown::dropdown;
use crate::lively::text_input;
use crate::lively::toggler;
use crate::lively::Tint;
use iced::widget::{Space, column, container, image, row, scrollable, slider, stack};
use crate::lively::text;
use iced::{Color, Element, Fill, Font, Length};
use nexo_core::hosting::ServerKind;

fn host(msg: Msg) -> Message {
    Message::Host(msg)
}

pub fn view(app: &App) -> Element<'_, Message> {
    let body = match (&app.host.setup, &app.host.wizard) {
        (Some(setup), _) => setup_view(app, setup),
        (None, Some(w)) => wizard_view(app, w),
        (None, None) => crate::empty_state("Loading…", ""),
    };
    container(body).width(Fill).height(Fill).align_x(iced::Center).into()
}

fn wizard_view<'a>(app: &'a App, w: &'a Wizard) -> Element<'a, Message> {
    let body = match w.step {
        Step::Type => type_step(app, w),
        Step::Content => content_step(app, w),
        Step::World => world_step(w),
        Step::Performance => performance_step(app, w),
        Step::Review => review_step(app, w),
    };

    let back = button(text("←  Back").size(14))
        .padding([8, 16])
        .style(theme::ghost_button)
        .on_press(host(Msg::Back));
    let next: Element<'_, Message> = if w.step == Step::Review {
        Space::new().into()
    } else {
        button(text("Next  →").size(14))
            .padding([8, 20])
            .style(theme::primary_button)
            .on_press_maybe(step_ready(w).then_some(host(Msg::Next)))
            .into()
    };

    container(
        column![
            container(iced::widget::responsive(move |size| stepper(w, size.width >= 820.0)))
                .padding([18, 24])
                .height(64)
                .width(Fill),
            rule(),
            crate::lively::scrollable(container(body).padding([22, 32]).width(Fill)).height(Fill),
            rule(),
            container(row![back, Space::new().width(Fill), next].align_y(iced::Center)).padding([14, 24]),
        ]
        .height(Fill),
    )
    .max_width(1000)
    .height(Fill)
    .style(theme::card)
    .into()
}

/// Whether Next is allowed off this step.
fn step_ready(w: &Wizard) -> bool {
    match w.step {
        Step::World => !w.import || w.import_world.is_some(),
        Step::Performance => !w.name.trim().is_empty(),
        _ => true,
    }
}

fn rule<'a>() -> Element<'a, Message> {
    container(Space::new()).width(Fill).height(1).style(theme::divider).into()
}

/// The numbered steps. `roomy`: every label shows; otherwise only the
/// current one, so five steps still fit a narrow window.
fn stepper(w: &Wizard, roomy: bool) -> Element<'_, Message> {
    let mut steps = row![].spacing(if roomy { 26 } else { 12 }).align_y(iced::Center);
    for (i, step) in Step::ALL.into_iter().enumerate() {
        let current = step == w.step;
        let done = step < w.step;
        let dot = container(text(if done { "✓".to_string() } else { (i + 1).to_string() }).size(13))
            .width(28)
            .height(28)
            .align_x(iced::Center)
            .align_y(iced::Center)
            .tinted(theme::step_dot(current, done));
        let label = crate::lively::tinted_text(
            text(step.label()).size(14).font(if current { theme::bold() } else { theme::regular() }),
            if current { theme::text() } else { theme::muted() },
        );
        let content = if roomy || current { row![dot, label] } else { row![dot] };
        steps = steps.push(
            button(content.spacing(9).align_y(iced::Center))
                .padding(0)
                .style(theme::bare_button)
                // Only backwards: later steps depend on choices not made yet.
                .on_press_maybe(done.then_some(host(Msg::GoTo(step)))),
        );
    }
    container(steps).width(Fill).align_x(iced::Center).into()
}

fn title<'a>(heading: &'a str, sub: impl iced::widget::text::IntoFragment<'a>) -> Element<'a, Message> {
    column![
        text(heading).size(24).font(theme::bold()).color(theme::text()),
        text(sub).size(13).color(theme::muted()),
    ]
    .spacing(6)
    .width(Fill)
    .align_x(iced::Center)
    .into()
}

fn field_label(label: &str) -> Element<'_, Message> {
    text(label).size(14).font(theme::bold()).color(theme::text()).into()
}

// ------------------------------------------------------------------- step 1

fn type_step<'a>(app: &'a App, w: &'a Wizard) -> Element<'a, Message> {
    let cards = row(ServerKind::ALL.into_iter().map(|kind| kind_card(kind, w.kind == kind)))
        .spacing(14)
        .width(Fill);

    let lag_note = (w.kind == ServerKind::Paper).then(|| {
        text("Paper usually supports a new Minecraft release a few days after Mojang ships it.")
            .size(12)
            .color(theme::muted())
    });

    column![
        title("Server type", "Pick what the server runs. You can't switch type later, but everything else stays editable."),
        cards,
        column![
            field_label("Minecraft version"),
            dropdown(app.game_versions.as_slice(), Some(&w.version), |v| host(Msg::Version(v)))
                .padding(10)
                .width(Fill),
        ]
        .push(lag_note)
        .spacing(8),
    ]
    .spacing(22)
    .into()
}

fn kind_card(kind: ServerKind, selected: bool) -> Element<'static, Message> {
    let (blurb, score, rating, plugins, mods) = match kind {
        ServerKind::Paper => (
            "A fast, configurable fork of vanilla with fixes for lag and exploits. Runs Bukkit/Paper plugins.",
            5,
            "Excellent",
            true,
            false,
        ),
        ServerKind::Vanilla => (
            "Exactly the server Mojang ships — nothing added, nothing changed. Every vanilla mechanic behaves as in singleplayer.",
            3,
            "Good",
            false,
            false,
        ),
        ServerKind::Fabric => (
            "Runs Fabric mods server-side — performance mods like Lithium, or content mods your friends install too.",
            4,
            "Great",
            false,
            true,
        ),
    };

    let meter = row((0..5).map(|i| {
        container(Space::new())
            .width(Fill)
            .height(5)
            .style(theme::fill(if i < score { theme::accent() } else { theme::control() }))
            .into()
    }))
    .spacing(5);

    let feature = |ok: bool, label: &'static str| {
        text(format!("{} {label}", if ok { "✓" } else { "✕" }))
            .size(12)
            .color(if ok { theme::mint() } else { theme::muted() })
    };

    button(
        column![
            text(kind.label()).size(20).font(theme::bold()).color(theme::text()),
            text(blurb).size(13).color(theme::muted()),
            Space::new().height(Fill),
            row![text("Performance").size(12).color(theme::muted()), Space::new().width(Fill), text(rating).size(12)],
            meter,
            row![
                feature(plugins, "Plugins"),
                Space::new().width(Fill),
                feature(mods, "Mods")
            ],
        ]
        .spacing(10),
    )
    .padding(18)
    .width(Fill)
    .height(270)
    .style(if selected { theme::selected_tile } else { theme::tile })
    .on_press(host(Msg::Kind(kind)))
    .into()
}

// ------------------------------------------------------------------- step 2

fn content_step<'a>(app: &'a App, w: &'a Wizard) -> Element<'a, Message> {
    let Some((_, loader, _)) = w.kind.content() else {
        return column![
            title("Add-ons", "Plugins and mods for your server."),
            crate::empty_state(
                "Vanilla runs no add-ons",
                "It's the server exactly as Mojang ships it, so only players on exactly this version can join. Pick Paper or Fabric for plugins, mods, or ViaVersion to let other versions in.",
            ),
        ]
        .spacing(22)
        .into();
    };
    let noun = if w.kind == ServerKind::Paper { "plugins" } else { "mods" };

    let search = row![
        text_input(&format!("Search {noun} on Modrinth…"), &w.query)
            .on_input(|q| host(Msg::Query(q)))
            .on_submit(host(Msg::Search))
            .padding(10)
            .style(theme::input)
            .width(Fill),
        button(text("Search").size(14))
            .padding([10, 18])
            .style(theme::ghost_button)
            .on_press_maybe((!w.searching).then_some(host(Msg::Search))),
    ]
    .spacing(8);

    let filters = text(format!("Modrinth · {loader} · Minecraft {}", w.version)).size(12).color(theme::muted());

    let mut picked = row![text(format!("{} selected", w.picked.len())).size(12).color(theme::muted())]
        .spacing(6)
        .align_y(iced::Center);
    for (id, name) in &w.picked {
        picked = picked.push(
            button(text(format!("{name}  ✕")).size(12))
                .padding([4, 10])
                .style(theme::lit_button)
                .on_press(host(Msg::TogglePick(id.clone(), name.clone()))),
        );
    }

    let results: Element<'_, Message> = if w.searching {
        note("Searching Modrinth…")
    } else if let Some(err) = &w.search_error {
        note(err)
    } else if w.results.is_empty() {
        note("Nothing found for that search.")
    } else {
        let cards: Vec<Element<'_, Message>> = w.results.iter().enumerate().map(|(index, hit)| super::arrive(app, index, hit_card(app, w, hit))).collect();
        let mut grid = column![].spacing(12);
        let mut cards = cards.into_iter();
        while let Some(left) = cards.next() {
            let right = cards.next().unwrap_or_else(|| Space::new().width(Fill).into());
            grid = grid.push(row![left, right].spacing(12));
        }
        grid.into()
    };

    let mut col = column![
        title("Add-ons", format!("Pick {noun} to install. The right build for Minecraft {} is downloaded when the server is made, with anything it needs.", w.version)),
        essentials_section(app, w),
        text(format!("More {noun}")).size(16).font(theme::bold()).color(theme::text()),
        column![search, filters].spacing(6),
        crate::lively::eased_scrollbar(
            scrollable(picked).direction(scrollable::Direction::Horizontal(scrollable::Scrollbar::new().width(3).scroller_width(3))),
            scrollable::default,
            true,
        ),
    ]
    .spacing(16);
    if w.kind == ServerKind::Fabric {
        col = col.push(text("Fabric API is added automatically — almost every server mod needs it.").size(12).color(theme::muted()));
    }
    col.push(results).into()
}

/// The curated list: checkable cards, each saying which build it will get.
fn essentials_section<'a>(app: &'a App, w: &'a Wizard) -> Element<'a, Message> {
    let cards: Vec<Element<'_, Message>> =
        hosting::essentials(w.kind).iter().enumerate().map(|(index, e)| super::arrive(app, index, essential_card(app, w, e))).collect();
    let mut grid = column![].spacing(10);
    let mut cards = cards.into_iter();
    while let Some(left) = cards.next() {
        let right = cards.next().unwrap_or_else(|| Space::new().width(Fill).into());
        grid = grid.push(row![left, right].spacing(10));
    }
    column![
        row![
            column![
                text("Essentials").size(16).font(theme::bold()).color(theme::text()),
                text("Hand-picked, and checked against your version as you look.").size(12).color(theme::muted()),
            ]
            .spacing(3)
            .width(Fill),
            button(text("Recommended").size(12))
                .padding([5, 12])
                .style(theme::ghost_button)
                .on_press(host(Msg::RecommendedEssentials)),
        ]
        .align_y(iced::Center),
        grid,
    ]
    .spacing(12)
    .into()
}

fn essential_card<'a>(app: &'a App, w: &'a Wizard, e: &'a Essential) -> Element<'a, Message> {
    let chosen = w.chosen.contains(&e.slug);
    let probe = w.probes.get(e.slug).cloned().unwrap_or(Probe::Checking);
    let (status, color, available) = match &probe {
        Probe::Checking => ("Checking Modrinth…".to_string(), theme::muted(), true),
        Probe::Ready { version, downloads } => {
            (format!("{version} for {} · ↓ {}", w.version, compact(*downloads)), theme::mint(), true)
        }
        Probe::Missing => (format!("No build for {} yet", w.version), theme::amber(), false),
        Probe::Failed => ("Couldn't reach Modrinth — checked again when you come back".to_string(), theme::amber(), true),
    };
    let icon: Element<'_, Message> = match app.icons.get(e.slug) {
        Some(handle) => crate::lively::appear(image(handle.clone()).width(36).height(36)),
        None => container(Space::new()).width(36).height(36).style(theme::fill(theme::control())).into(),
    };
    let check = container(text(if chosen { "✓" } else { "" }).size(13).color(theme::ink()))
        .width(20)
        .height(20)
        .align_x(iced::Center)
        .align_y(iced::Center)
        .tinted(theme::fill(if chosen { theme::mint() } else { theme::control() }));

    button(
        row![
            icon,
            column![
                row![
                    text(e.name).size(14).font(theme::bold()).color(theme::text()),
                    container(text(e.tag).size(10)).padding([1, 7]).tinted(theme::pill(theme::accent())),
                ]
                .spacing(8)
                .align_y(iced::Center),
                text(e.why).size(12).color(theme::muted()),
                text(status).size(11).color(color),
            ]
            .spacing(4)
            .width(Fill),
            check,
        ]
        .spacing(12)
        .align_y(iced::Center),
    )
    .padding(12)
    .width(Fill)
    .style(if chosen { theme::selected_tile } else { theme::tile })
    .on_press_maybe(available.then_some(host(Msg::ToggleEssential(e.slug))))
    .into()
}

fn hit_card<'a>(app: &'a App, w: &'a Wizard, hit: &'a nexo_core::modrinth::SearchHit) -> Element<'a, Message> {
    let picked = w.picked.iter().any(|(id, _)| *id == hit.project_id);
    let icon: Element<'_, Message> = match app.icons.get(&hit.project_id) {
        Some(handle) => crate::lively::appear(image(handle.clone()).width(48).height(48)),
        None => container(Space::new()).width(48).height(48).style(theme::fill(theme::control())).into(),
    };
    let toggle = button(text(if picked { "Added ✓" } else { "Add" }).size(13))
        .padding([5, 12])
        .style(if picked { theme::lit_button } else { theme::ghost_button })
        .on_press(host(Msg::TogglePick(hit.project_id.clone(), hit.title.clone())));

    container(
        row![
            icon,
            column![
                row![text(&hit.title).size(15).font(theme::bold()).color(theme::text()).width(Fill), toggle]
                    .align_y(iced::Center),
                text(clip(&hit.description, 80)).size(12).color(theme::muted()),
                text(format!("↓ {}   ♥ {}", compact(hit.downloads), compact(hit.follows))).size(11).color(theme::muted()),
            ]
            .spacing(5)
            .width(Fill),
        ]
        .spacing(12),
    )
    .padding(14)
    .width(Fill)
    .style(if picked { theme::highlight } else { theme::panel })
    .into()
}

fn clip(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", s[..i].trim_end()),
        None => s.to_string(),
    }
}

/// 103573148 → "103.6M".
pub fn compact(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.1}k", n as f64 / 1e3),
        _ => format!("{:.1}M", n as f64 / 1e6),
    }
}

fn note(message: &str) -> Element<'_, Message> {
    container(text(message).size(13).color(theme::muted()))
        .width(Fill)
        .padding(30)
        .align_x(iced::Center)
        .into()
}

// ------------------------------------------------------------------- step 3

fn world_step(w: &Wizard) -> Element<'_, Message> {
    let segment = |label: &'static str, import: bool| {
        button(container(text(label).size(14)).width(Fill).align_x(iced::Center))
            .padding([9, 12])
            .width(Fill)
            .style(if w.import == import { theme::selected_tile } else { theme::tile })
            .on_press(host(Msg::Import(import)))
    };

    let fields: Element<'_, Message> = if w.import {
        if w.worlds.is_empty() {
            note("No singleplayer worlds in any instance yet.")
        } else {
            column![
                field_label("World to copy in"),
                dropdown(w.worlds.as_slice(), w.import_world.as_ref(), |c| host(Msg::ImportWorld(c)))
                    .padding(10)
                    .width(Fill),
                text("It's copied — the original save stays exactly as it is. Game mode and difficulty come from the world.")
                    .size(12)
                    .color(theme::muted()),
            ]
            .spacing(8)
            .into()
        }
    } else {
        column![
            field_label("World seed (optional)"),
            text_input("Leave blank for a random seed", &w.seed)
                .on_input(|s| host(Msg::Seed(s)))
                .padding(10)
                .style(theme::input),
            row![
                column![
                    field_label("World type"),
                    dropdown(&WORLD_TYPES[..], Some(w.world_type), |t| host(Msg::WorldType(t))).padding(10).width(Fill)
                ]
                .spacing(8)
                .width(Fill),
                column![
                    field_label("Game mode"),
                    dropdown(&GAME_MODES[..], Some(w.game_mode), |m| host(Msg::GameMode(m))).padding(10).width(Fill)
                ]
                .spacing(8)
                .width(Fill),
                column![
                    field_label("Difficulty"),
                    dropdown(&DIFFICULTIES[..], Some(w.difficulty), |d| host(Msg::Difficulty(d))).padding(10).width(Fill)
                ]
                .spacing(8)
                .width(Fill),
            ]
            .spacing(14),
            row![
                toggler(w.hardcore).size(18).style(theme::toggle).on_toggle(|on| host(Msg::Hardcore(on))),
                text("Hardcore — one life, difficulty locked to Hard").size(13).color(theme::text()),
            ]
            .spacing(10)
            .align_y(iced::Center),
        ]
        .spacing(10)
        .into()
    };

    column![
        title("World", "Start a fresh world, or bring one of your singleplayer worlds along."),
        row![segment("Create new world", false), segment("Import existing world", true)].spacing(8),
        fields,
    ]
    .spacing(20)
    .into()
}

// ------------------------------------------------------------------- step 4

fn performance_step<'a>(app: &'a App, w: &'a Wizard) -> Element<'a, Message> {
    let tunnel = container(
        row![
            column![
                text("Public address").size(14).font(theme::bold()).color(theme::text()),
                text("Friends join from anywhere through e4mc's relay — no port forwarding, no account. Open while Nexo runs. The whitelist starts on: add friends under Players.")
                    .size(12)
                    .color(theme::muted()),
            ]
            .spacing(3)
            .width(Fill),
            toggler(w.tunnel).size(20).style(theme::toggle).on_toggle(|on| host(Msg::Tunnel(on))),
        ]
        .spacing(14)
        .align_y(iced::Center),
    )
    .padding(16)
    .style(theme::panel);

    column![
        title("Performance", "Name the server and decide how much memory it gets."),
        field_label("Server name"),
        text_input("My server", &w.name)
            .on_input(|n| host(Msg::Name(n)))
            .padding(10)
            .style(theme::input),
        Space::new().height(4),
        field_label("Memory"),
        ram_picker(app, w.kind, w.memory_mb, |mb| host(Msg::Memory(mb))),
        Space::new().height(4),
        text("CONNECTED SERVICES").size(11).color(theme::muted()),
        tunnel,
    ]
    .spacing(10)
    .into()
}

/// The RAM slider over its zone track, with a verdict underneath. Shared with
/// the dashboard's Settings tab.
pub fn ram_picker<'a>(
    app: &App,
    kind: ServerKind,
    value: u32,
    on_change: impl Fn(u32) -> Message + 'a,
) -> Element<'a, Message> {
    const MIN: u32 = 512;
    let max = hosting::ram_max(app).max(value);
    let total = app.host.total_memory_mb;
    let (low, high) = hosting::ram_band(kind);
    // More than ~70% of the machine starves the game client running beside it.
    let danger = if total > 0 { (total * 7 / 10).max(high) } else { max };

    let span = (max - MIN) as f32;
    let portion = |from: u32, to: u32| ((to.clamp(MIN, max) - from.clamp(MIN, max)) as f32 / span * 1000.0) as u16;
    let zone = |p: u16, c: Color| -> Element<'a, Message> {
        container(Space::new())
            .width(Length::FillPortion(p.max(1)))
            .height(4)
            .style(theme::fill(c))
            .into()
    };
    let mut track = row![].spacing(0);
    for (p, c) in [
        (portion(MIN, low), theme::amber()),
        (portion(low, high), theme::mint()),
        (portion(high, danger), theme::amber()),
        (portion(danger, max), theme::danger()),
    ] {
        if p > 0 {
            track = track.push(zone(p, c));
        }
    }

    let picker = stack![
        container(track).height(20).align_y(iced::Center).padding([0, 8]),
        crate::lively::slider(slider(MIN..=max, value, on_change).step(256u32).height(20), theme::bare_slider),
    ];

    let available = if total > 0 { format!("This PC has {} MB", total) } else { String::new() };
    let scale = row![
        text(format!("{MIN} MB")).size(11).color(theme::muted()),
        Space::new().width(Fill),
        text(available).size(11).color(theme::muted()),
        Space::new().width(Fill),
        text(format!("{max} MB")).size(11).color(theme::muted()),
    ];

    let label = format!("{} {}", kind.label().to_lowercase(), "server");
    let (verdict, detail, good) = if value < low {
        ("A little tight", format!("Below the {low}–{high} MB a {label} is happiest with — fine for a couple of players."), false)
    } else if value <= high {
        ("Optimal memory", format!("Inside the {low}–{high} MB range that suits a {label}."), true)
    } else if value <= danger {
        ("More than it needs", format!("Past {high} MB a {label} rarely gets faster, and it's memory your game can't use."), false)
    } else {
        ("Too much for this PC", "This leaves too little for Minecraft itself and the rest of your system.".to_string(), false)
    };

    column![
        row![
            text(format!("{value} MB")).size(22).font(theme::bold()).color(theme::text()),
            text(format!("  ≈ {} GB", hosting::format_gb(value))).size(13).color(theme::muted()),
        ]
        .align_y(iced::Center),
        picker,
        scale,
        container(
            column![text(verdict).size(13).font(theme::bold()), text(detail).size(12)].spacing(3)
        )
        .padding([10, 14])
        .width(Fill)
        .tinted(theme::callout(match (good, value > danger) {
            (true, _) => theme::mint(),
            (false, false) => theme::amber(),
            (false, true) => theme::danger(),
        })),
    ]
    .spacing(8)
    .into()
}

// ------------------------------------------------------------------- step 5

fn review_step<'a>(app: &'a App, w: &'a Wizard) -> Element<'a, Message> {
    let line = |label: &'static str, value: String| {
        row![
            text(label).size(13).color(theme::muted()).width(170),
            text(value).size(14).color(theme::text()).width(Fill),
        ]
        .spacing(10)
    };

    let world = match (&w.import, &w.import_world) {
        (true, Some(choice)) => format!("Copy of {}", choice.label),
        _ => format!(
            "New {} world · {} · {}{}{}",
            w.world_type.to_lowercase(),
            w.game_mode,
            w.difficulty,
            if w.hardcore { " · hardcore" } else { "" },
            if w.seed.trim().is_empty() { String::new() } else { format!(" · seed {}", w.seed.trim()) },
        ),
    };
    let mut names: Vec<String> = hosting::essentials(w.kind)
        .iter()
        .filter(|e| w.chosen.contains(&e.slug))
        .filter_map(|e| w.ready_version(e.slug).map(|v| format!("{} {v}", e.name)))
        .collect();
    for (_, title) in &w.picked {
        // Already listed if it's also an essential.
        if !names.iter().any(|n| n.starts_with(title.as_str())) {
            names.push(title.clone());
        }
    }
    let addons = match w.kind.content() {
        None => "None — vanilla".to_string(),
        Some(_) if names.is_empty() => "None".to_string(),
        Some(_) => names.join(", "),
    };
    let location = app
        .core
        .as_ref()
        .map(|c| c.paths.servers().display().to_string())
        .unwrap_or_default();

    let summary = container(
        column![
            line("Server type", format!("{} {}", w.kind.label(), w.version)),
            line("Name", w.name.trim().to_string()),
            line("Memory", format!("{} MB", w.memory_mb)),
            line("World", world),
            line("Add-ons", addons),
            line("Public address", if w.tunnel { "On — e4mc relay, whitelist on".into() } else { "Off — this PC only".into() }),
            line("Saved under", location),
        ]
        .spacing(12),
    )
    .padding(20)
    .width(Fill)
    .style(theme::panel);

    let eula = row![
        checkbox(w.eula)
            .label("I accept the Minecraft End User License Agreement")
            .style(theme::check)
            .on_toggle(|on| host(Msg::Eula(on))),
        button(text("Read it").size(12)).padding([3, 8]).style(theme::ghost_button).on_press(host(Msg::OpenEula)),
    ]
    .spacing(10)
    .align_y(iced::Center);

    let ready = w.eula && !w.name.trim().is_empty() && app.core.is_some();
    column![
        title("Review", "Check everything, then make the server and start it."),
        summary,
        eula,
        container(
            button(text("Create & start server").size(15))
                .padding([11, 36])
                .style(theme::primary_button)
                .on_press_maybe(ready.then_some(host(Msg::Create)))
        )
        .width(Fill)
        .align_x(iced::Center),
    ]
    .spacing(18)
    .into()
}

// -------------------------------------------------------------------- setup

fn setup_view<'a>(app: &'a App, setup: &'a Setup) -> Element<'a, Message> {
    let id = setup.id.as_deref();
    let live = id.and_then(|id| app.host.live.get(id));
    let phase = live.map(|l| l.phase).unwrap_or_default();
    let domain = live.and_then(|l| l.domain.as_deref());
    let tunnel_error = live.and_then(|l| l.tunnel_error.as_deref());
    let failed = setup.error.is_some();

    // (label, done, failed here)
    let mut steps = vec![
        ("Preparing server files", id.is_some(), failed && id.is_none()),
        ("Starting Minecraft server", phase == Phase::Online, failed && id.is_some()),
    ];
    if setup.tunnel {
        steps.push(("Opening the public address", domain.is_some(), tunnel_error.is_some()));
    }
    let finished = phase == Phase::Online && (!setup.tunnel || domain.is_some() || tunnel_error.is_some());
    let active = steps.iter().position(|(_, done, _)| !done);

    let mut list = column![].spacing(12);
    for (i, (label, done, broke)) in steps.iter().enumerate() {
        let (glyph, color) = if *broke {
            ("✕", theme::danger())
        } else if *done {
            ("✓", theme::mint())
        } else if Some(i) == active && !failed {
            ("◌", theme::accent())
        } else {
            ("○", theme::muted())
        };
        list = list.push(
            row![
                text(glyph).size(18).color(color).width(22),
                text(*label).size(16).color(if *done || Some(i) == active { theme::text() } else { theme::muted() }),
            ]
            .spacing(10)
            .align_y(iced::Center),
        );
    }

    let mut col = column![text("Setting up your server").size(20).font(theme::bold()).color(theme::text()), list].spacing(16);

    if let Some(err) = &setup.error {
        col = col.push(crate::lively::appear(container(text(err.as_str()).size(13)).padding([10, 14]).width(Fill).style(theme::banner(true))));
    }
    if let Some(err) = tunnel_error {
        col = col.push(crate::lively::appear(
            container(text(format!("The public address didn't open: {err}. The server still runs on this PC; retry from its dashboard.")).size(13))
                .padding([10, 14])
                .width(Fill)
                .style(theme::banner(true)),
        ));
    }
    if !setup.warnings.is_empty() {
        col = col.push(crate::lively::appear(
            container(text(format!("Some add-ons weren't installed:\n{}", setup.warnings.join("\n"))).size(12))
                .padding([10, 14])
                .width(Fill)
                .style(theme::banner(true)),
        ));
    }

    if finished && let Some(server) = id.and_then(|id| app.host.server(id)) {
        col = col.push(rule());
        if let Some(domain) = domain {
            col = col.push(address_row("Public", domain.to_string()));
        }
        col = col.push(address_row("This PC", format!("localhost:{}", server.port)));
        col = col.push(
            text(format!(
                "Anyone on Minecraft Java {} can paste the address into Multiplayer → Add Server.",
                server.minecraft_version
            ))
            .size(12)
            .color(theme::muted()),
        );
    }

    if id.is_some() && !finished {
        col = col.push(
            button(text(if setup.show_console { "›_  Hide console" } else { "›_  Show console" }).size(13))
                .padding([6, 12])
                .style(theme::ghost_button)
                .on_press(host(Msg::ToggleSetupConsole)),
        );
        if setup.show_console {
            let tail = app
                .host
                .console
                .as_deref()
                .map(|c| {
                    let lines: Vec<&str> = c.lines().collect();
                    lines[lines.len().saturating_sub(200)..].join("\n")
                })
                .unwrap_or_else(|| "Waiting for the server's first output…".into());
            col = col.push(
                container(
                    crate::lively::scrollable(container(text(tail).size(11).font(Font::MONOSPACE).color(theme::text())).padding(12))
                        .anchor_bottom()
                        .height(260)
                        .width(Fill),
                )
                .style(theme::well),
            );
        }
    }

    let actions: Element<'_, Message> = match (failed, finished, id) {
        (_, true, Some(id)) => button(text("Continue to dashboard").size(15))
            .padding([10, 30])
            .style(theme::primary_button)
            .on_press(host(Msg::LeaveSetup(Some(id.to_string()))))
            .into(),
        (true, _, None) => button(text("Back to the wizard").size(14))
            .padding([8, 18])
            .style(theme::ghost_button)
            .on_press(host(Msg::LeaveSetup(None)))
            .into(),
        (true, _, Some(id)) => button(text("Open its dashboard").size(14))
            .padding([8, 18])
            .style(theme::ghost_button)
            .on_press(host(Msg::LeaveSetup(Some(id.to_string()))))
            .into(),
        _ => text(if id.is_none() {
            "Downloading the server and add-ons…"
        } else {
            "The first start builds the world and can take a minute or two."
        })
        .size(12)
        .color(theme::muted())
        .into(),
    };
    col = col.push(container(actions).width(Fill).align_x(iced::Center));

    container(crate::lively::scrollable(container(col).padding(28)))
        .max_width(860)
        .style(theme::card)
        .into()
}

pub fn address_row(label: &str, address: String) -> Element<'static, Message> {
    row![
        container(text(label.to_string()).size(11)).padding([3, 10]).tinted(theme::pill(theme::accent())),
        container(text(address.clone()).size(14).font(Font::MONOSPACE).color(theme::text()))
            .padding([9, 12])
            .width(Fill)
            .style(theme::well),
        button(text("Copy").size(13)).padding([8, 14]).style(theme::ghost_button).on_press(host(Msg::Copy(address))),
    ]
    .spacing(10)
    .align_y(iced::Center)
    .into()
}
