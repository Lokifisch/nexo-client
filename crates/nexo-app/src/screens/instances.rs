use crate::theme;
use crate::{empty_state, App, Message};
use iced::widget::{button, column, container, pick_list, row, scrollable, text, text_input};
use iced::{Element, Fill};
use nexo_core::Instance;

pub fn view(app: &App) -> Element<'_, Message> {
    let list: Element<'_, Message> = if app.instances.is_empty() {
        empty_state(
            "No instances yet",
            "Name one below and hit Create — Fabric and the right Minecraft build get set up for you.",
        )
    } else {
        scrollable(
            column(app.instances.iter().map(|instance| card(app, instance)))
                .spacing(12)
                .width(Fill),
        )
        .height(Fill)
        .into()
    };

    column![
        crate::screens::header("Instances", "Each one is its own game directory, mods, and saves.", None),
        create_form(app),
        list,
    ]
    .spacing(20)
    .height(Fill)
    .into()
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
        .padding([10, 20])
        .style(theme::primary_button)
        .on_press_maybe(can_create.then_some(Message::CreateInstance));

    let import = button(text("Import .mrpack").size(13))
        .padding([10, 16])
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
    let loader_line = match &instance.loader_version {
        Some(version) => format!("{} {} · loader {version}", instance.loader, instance.game_version),
        None => format!("{} {} · not installed yet", instance.loader, instance.game_version),
    };

    let last_played = match instance.last_played {
        Some(_) => "Played before".to_string(),
        None => "Never played".to_string(),
    };

    let running = app.running.contains(&instance.id);

    // The whole card is the way into the details screen; a plain text button
    // keeps it looking like a card rather than a control.
    let details = button(
        column![
            text(&instance.name).size(17).color(theme::TEXT),
            text(loader_line).size(12).color(theme::MUTED),
            // Owned, not borrowed: the widget outlives this function.
            text(if running {
                "Running".to_string()
            } else {
                last_played
            })
            .size(11)
            .color(if running { theme::MINT } else { theme::MUTED }),
        ]
        .spacing(3)
        .width(Fill),
    )
    .padding(0)
    .width(Fill)
    .style(theme::bare_button)
    .on_press(Message::OpenInstance(instance.id.clone()));

    // Play becomes Stop while the game is up, mirroring the details screen so
    // the same instance never shows two different states.
    let action: iced::Element<'a, Message> = if running {
        button(text("Stop").size(14))
            .padding([9, 22])
            .style(theme::stop_button)
            .on_press(Message::Stop(instance.id.clone()))
            .into()
    } else {
        let can_launch = !app.is_busy() && app.active_account().is_some();
        button(text("Play").size(14))
            .padding([9, 22])
            .style(theme::primary_button)
            .on_press_maybe(can_launch.then(|| Message::Launch(instance.id.clone())))
            .into()
    };

    container(row![details, action].spacing(12).align_y(iced::Center))
        .padding(16)
        .width(Fill)
        .style(theme::card)
        .into()
}
