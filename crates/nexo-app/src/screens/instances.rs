use crate::theme;
use crate::{empty_state, App, Message};
use iced::widget::{button, column, container, pick_list, row, scrollable, text, text_input};
use iced::{Element, Fill};
use nexo_core::{nexo_mod, Instance};

pub fn view(app: &App) -> Element<'_, Message> {
    let needle = app.instance_query.trim().to_lowercase();
    let shown: Vec<&Instance> = app
        .instances
        .iter()
        .filter(|i| needle.is_empty() || i.name.to_lowercase().contains(&needle))
        .collect();

    let list: Element<'_, Message> = if app.instances.is_empty() {
        empty_state(
            "No instances yet",
            "Name one above and hit Create — Fabric and the right Minecraft build get set up for you.",
        )
    } else if shown.is_empty() {
        empty_state("No matches", "No instance has that in its name.")
    } else {
        scrollable(
            column(shown.into_iter().map(|instance| card(app, instance)))
                .spacing(8)
                .width(Fill),
        )
        .height(Fill)
        .into()
    };

    let header = row![
        text("Instances").size(26).font(theme::BOLD).color(theme::TEXT).width(Fill),
        text_input("Search instances", &app.instance_query)
            .on_input(Message::InstanceQueryChanged)
            .padding([10, 12])
            .size(13)
            .style(theme::input)
            .width(300),
        button(text(if app.creating { "CANCEL" } else { "+ NEW INSTANCE" }).size(12).font(theme::BOLD))
            .padding([11, 16])
            .style(theme::lit_button)
            .on_press(Message::ToggleCreateForm),
    ]
    .spacing(16)
    .align_y(iced::Center);

    let mut page = column![header].spacing(16).height(Fill);
    // Always open with nothing to list: the form is then the only way
    // forward, and hiding it behind a button would leave an empty screen.
    if app.creating || app.instances.is_empty() {
        page = page.push(create_form(app));
    }
    page.push(list).into()
}

/// New-instance form. Loader is fixed to Fabric in v1, matching the single
/// loader `Mod/` targets, so it's stated rather than offered as a choice.
fn create_form(app: &App) -> Element<'_, Message> {
    let can_create = app.core.is_some() && !app.is_busy();

    let name = text_input("Instance name", &app.new_name)
        .on_input_maybe(can_create.then_some(Message::NewNameChanged))
        .on_submit(Message::CreateInstance)
        .padding(10)
        .style(theme::input)
        .width(Fill);

    let version = pick_list(
        app.game_versions.as_slice(),
        Some(&app.new_version),
        Message::NewVersionChanged,
    )
    .padding(10)
    .width(160);

    let create = button(text("Create").size(14))
        .padding([8, 17])
        .style(theme::primary_button)
        .on_press_maybe(can_create.then_some(Message::CreateInstance));

    let import = button(text("Import .mrpack").size(13))
        .padding([8, 14])
        .style(theme::ghost_button)
        .on_press_maybe(can_create.then_some(Message::ImportPack));

    container(
        column![
            row![name, version, create, import]
                .spacing(12)
                .align_y(iced::Center),
            text("Fabric loader, installed automatically. Importing a modpack creates its own instance.")
                .size(12)
                .color(theme::MUTED),
        ]
        .spacing(10),
    )
    .padding(16)
    .width(Fill)
    .style(theme::card)
    .into()
}

fn card<'a>(app: &'a App, instance: &'a Instance) -> Element<'a, Message> {
    let running = app.instance_running(&instance.id);

    // Minecraft, loader, and Nexo Mod when it is there — the three versions
    // that decide whether an instance will start the way someone expects.
    let mut meta = format!("Minecraft {} · {}", instance.game_version, instance.loader);
    if let Some(version) = &instance.loader_version {
        meta.push_str(&format!(" {version}"));
    }
    if let Some(nexo) = nexo_mod::installed(instance) {
        meta.push_str(&format!(" · Nexo Mod {}", nexo.version_number));
        if let Some(edition) = nexo_mod::installed_edition(instance) {
            meta.push_str(&format!(" {edition}"));
        }
    }

    // RUNNING wins; otherwise whether the loader has actually been
    // installed, which is the difference between Play working now and Play
    // starting a download first.
    let (tag, lit) = if running {
        ("RUNNING", true)
    } else if instance.loader_version.is_some() {
        ("READY", false)
    } else {
        ("NOT INSTALLED", false)
    };

    let mut title = row![
        text(&instance.name).size(17).font(theme::BOLD).color(theme::TEXT),
        container(text(tag).size(10).font(theme::BOLD))
            .padding([3, 7])
            .style(theme::tag(lit)),
    ]
    .spacing(10)
    .align_y(iced::Center);
    if app.nexo_mod_outdated(instance) {
        title = title.push(
            container(text("NEXO UPDATE").size(10).font(theme::BOLD))
                .padding([3, 7])
                .style(theme::tag(true)),
        );
    }

    // Jars only: resource packs and shaders are content too, but "14 mods"
    // counting a texture pack would be wrong in exactly the way that matters.
    let mods = instance.mods.iter().filter(|m| m.file_name.ends_with(".jar")).count();
    let played = if running {
        "Running now".to_string()
    } else {
        match instance.last_played {
            Some(when) => format!("Played {}", super::instance::ago(when)),
            None => "Never played".to_string(),
        }
    };

    // The whole left side is the way into the details screen; a plain text
    // button keeps it looking like a card rather than a control.
    let details = button(
        column![title, text(meta).size(13).color(theme::MUTED)]
            .spacing(4)
            .width(Fill),
    )
    .padding(0)
    .width(Fill)
    .style(theme::bare_button)
    .on_press(Message::OpenInstance(instance.id.clone()));

    let stats = row![
        text(if mods == 1 { "1 mod".to_string() } else { format!("{mods} mods") })
            .size(12)
            .color(theme::MUTED),
        text(played).size(12).color(theme::MUTED),
    ]
    .spacing(22);

    // Play becomes Stop while the game is up, mirroring the details screen so
    // the same instance never shows two different states.
    let action: iced::Element<'a, Message> = if running {
        button(text("STOP").size(12).font(theme::BOLD).center())
            .width(96)
            .padding([11, 0])
            .style(theme::stop_button)
            .on_press(Message::StopInstance(instance.id.clone()))
            .into()
    } else {
        let can_launch = !app.is_busy() && app.active_account().is_some();
        button(text("PLAY").size(12).font(theme::BOLD).center())
            .width(96)
            .padding([11, 0])
            .style(theme::primary_button)
            .on_press_maybe(can_launch.then(|| Message::Launch(instance.id.clone(), None)))
            .into()
    };

    // First letter of the name, in the square at the left of the card.
    let initial = instance
        .name
        .chars()
        .next()
        .map(|c| c.to_uppercase().collect::<String>())
        .unwrap_or_default();
    let tile = container(text(initial).size(18).font(theme::BOLD))
        .width(52)
        .height(52)
        .center_x(52)
        .center_y(52)
        .style(theme::initial_tile(running));

    container(row![tile, details, stats, action].spacing(16).align_y(iced::Center))
        .padding([0, 16])
        .height(84)
        .align_y(iced::Center)
        .width(Fill)
        .style(theme::card)
        .into()
}
