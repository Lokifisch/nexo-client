use crate::theme;
use crate::{App, Message, Screen};
use iced::widget::{button, column, container, image, row, scrollable, text, text_input, Space};
use iced::{Element, Fill};
use nexo_core::instance::InstalledMod;
use nexo_core::nexo_mod;
use nexo_core::Instance;

/// Details for a single instance: what it is, how to launch it, its content,
/// and the Nexo Mod injector.
pub fn view<'a>(app: &'a App, instance: &'a Instance) -> Element<'a, Message> {
    let running = app.running.contains(&instance.id);

    let back = button(text("‹ Instances").size(13))
        .padding([6, 12])
        .style(theme::ghost_button)
        .on_press(Message::Navigate(Screen::Instances));

    let heading = column![
        text(&instance.name).size(28).color(theme::TEXT),
        text(format!(
            "{} {}{}",
            instance.loader,
            instance.game_version,
            instance
                .loader_version
                .as_deref()
                .map(|v| format!(" · loader {v}"))
                .unwrap_or_default()
        ))
        .size(13)
        .color(theme::MUTED),
    ]
    .spacing(4)
    .width(Fill);

    let content = column![
        back,
        row![heading, launch_control(app, instance, running)]
            .spacing(16)
            .align_y(iced::Center),
        scrollable(if app.browsing {
            column![browser_card(app, instance)].spacing(14).width(Fill)
        } else {
            column![
                nexo_mod_card(app, instance),
                installed_card(app, instance),
                details_card(app, instance),
                danger_card(instance, running),
            ]
            .spacing(14)
            .width(Fill)
        })
        .height(Fill),
    ]
    .spacing(18)
    .height(Fill);

    content.into()
}

/// Play, or Stop while the game is up. Red and relabelled rather than a
/// separate control, so there's one obvious thing to press either way.
fn launch_control<'a>(
    app: &'a App,
    instance: &'a Instance,
    running: bool,
) -> Element<'a, Message> {
    let signed_in = app.active_account().is_some();

    if running {
        return button(text("Stop").size(15))
            .padding([12, 32])
            .style(theme::stop_button)
            .on_press(Message::Stop(instance.id.clone()))
            .into();
    }

    let label = if app.is_busy() { "Preparing…" } else { "Play" };

    let play = button(text(label).size(15))
        .padding([12, 32])
        .style(theme::primary_button)
        // Launching without an account fails deep in the pipeline, so the
        // button is disabled until there is one.
        .on_press_maybe(
            (!app.is_busy() && signed_in).then(|| Message::Launch(instance.id.clone())),
        );

    if signed_in {
        play.into()
    } else {
        column![
            play,
            text("Sign in first").size(11).color(theme::MUTED),
        ]
        .spacing(4)
        .align_x(iced::Center)
        .into()
    }
}

/// The injector. Compatibility is decided by the release's own manifest, so
/// this states what the published build targets rather than assuming.
fn nexo_mod_card<'a>(app: &'a App, instance: &'a Instance) -> Element<'a, Message> {
    let installed = nexo_mod::installed(instance);

    let mut body = column![
        row![
            column![
                text("Nexo Mod").size(17).color(theme::TEXT),
                text("The client-side half — cosmetics, position obscuring, macros.")
                    .size(12)
                    .color(theme::MUTED),
            ]
            .spacing(3)
            .width(Fill),
        ]
        .align_y(iced::Center),
    ]
    .spacing(12);

    // Bound with an explicit type: the arms produce different widget types,
    // so inference can't pick one for `push`.
    let section: Element<'a, Message> = match &app.nexo_release {
        Some(release) => release_section(app, instance, release, installed),

        // Release lookup hasn't landed, or it failed — offer a retry rather
        // than a dead card.
        //
        // The failure has to be said out loud. Reporting the pending line
        // after a failed lookup leaves the card claiming to be working on
        // something it gave up on, and the user waits on it indefinitely
        // rather than pressing the retry sitting right underneath.
        None => {
            let (line, tone) = match (&app.nexo_release_error, installed) {
                (Some(err), _) => (err.clone(), theme::DANGER),
                (None, Some(current)) => {
                    (format!("Installed — {}", current.version_number), theme::MUTED)
                }
                (None, None) => ("Checking for the latest release…".to_string(), theme::MUTED),
            };
            let retry = if app.nexo_release_error.is_some() {
                "Try again"
            } else {
                "Check again"
            };
            column![
                text(line).size(12).color(tone),
                button(text(retry).size(13))
                    .padding([8, 16])
                    .style(theme::ghost_button)
                    .on_press(Message::FetchNexoRelease),
            ]
            .spacing(10)
            .into()
        }
    };
    body = body.push(section);

    container(body)
        .padding(18)
        .width(Fill)
        .style(theme::card)
        .into()
}

/// The part of the injector card that needs a resolved release: what's
/// installed, which edition to install, and the one button that acts on it.
fn release_section<'a>(
    app: &'a App,
    instance: &'a Instance,
    release: &'a nexo_mod::Release,
    installed: Option<&'a InstalledMod>,
) -> Element<'a, Message> {
    // Refuses rather than adapting: installing never changes an instance's
    // loader or Minecraft version. Shown instead of the picker, since there
    // is nothing to pick between if nothing can be installed.
    if installed.is_none()
        && let Some(reason) = release.manifest.incompatibility(instance)
    {
        return column![
            text(reason).size(12).color(theme::MUTED),
            text("Create an instance on that version to use Nexo Mod.")
                .size(11)
                .color(theme::MUTED),
        ]
        .spacing(4)
        .into();
    }

    let installed_edition = nexo_mod::installed_edition(instance);
    // What every control below acts on: the user's pick, else what's already
    // installed, else what the release itself prefers. Filtered against what
    // the release actually publishes, so a stale pick can't point at nothing.
    let selected = app
        .nexo_edition
        .or(installed_edition)
        .filter(|edition| release.edition(*edition).is_some())
        .unwrap_or_else(|| release.default_edition());

    let up_to_date = installed.is_some_and(|m| m.version_number == release.manifest.mod_version);

    let status: Element<'a, Message> = match installed {
        Some(current) => {
            let edition = installed_edition.unwrap_or_default();
            if up_to_date {
                text(format!(
                    "Installed — {edition} {} (latest)",
                    current.version_number
                ))
                .size(12)
                .color(theme::MINT)
                .into()
            } else {
                text(format!(
                    "Installed — {edition} {} · {} is available",
                    current.version_number, release.manifest.mod_version
                ))
                .size(12)
                .color(theme::TEXT)
                .into()
            }
        }
        None => text(format!(
            "Compatible — version {} targets {} {}",
            release.manifest.mod_version,
            release.manifest.loader,
            release.manifest.minecraft_version
        ))
        .size(12)
        .color(theme::MINT)
        .into(),
    };

    let mut section = column![status].spacing(12);

    // Installed into an instance the published release no longer fits. Every
    // button below is gone in that case, so say why rather than leaving a
    // card that looks broken.
    if let Some(reason) = release.manifest.incompatibility(instance) {
        section = section.push(text(reason).size(11).color(theme::MUTED));
    }

    // Only worth a picker when the release actually publishes more than one
    // build. Pre-0.5.0 releases have a single jar and get no chooser.
    if release.offers_a_choice() {
        let mut tiles = row![].spacing(10);
        for build in release.editions() {
            tiles = tiles.push(edition_tile(build, selected, installed_edition));
        }

        section = section.push(
            column![
                // Frames the choice as what it is. Which features sit in
                // which jar is the manifest's business — this line is about
                // the consequence, and holds no matter what a release ships.
                text("Pick an edition. This decides what the mod is allowed to do on a server, not how much of it you get.")
                    .size(11)
                    .color(theme::MUTED),
                tiles,
            ]
            .spacing(8),
        );
    }

    // The two jars declare `breaks` on each other, so this is never an "also
    // install" — say so before the button is pressed.
    if let Some(current) = installed_edition
        && current != selected
    {
        section = section.push(
            text(format!(
                "Installing {selected} removes the {current} jar — Minecraft won't start with both."
            ))
            .size(11)
            .color(theme::MUTED),
        );
    }

    let action = match installed_edition {
        // Same edition already there: only an update is left to offer.
        Some(current) if current == selected => (!up_to_date).then(|| "Update".to_string()),
        Some(_) => Some(format!("Switch to {selected}")),
        None if release.offers_a_choice() => Some(format!("Install {selected}")),
        None => Some("Install Nexo Mod".to_string()),
    };

    let mut actions = row![].spacing(10);
    if let Some(label) = action
        && release.manifest.supports(instance)
    {
        actions = actions.push(
            button(text(label).size(14))
                .padding([10, 20])
                .style(theme::primary_button)
                .on_press_maybe((!app.is_busy()).then(|| Message::InstallNexoMod {
                    instance: instance.id.clone(),
                    edition: selected,
                })),
        );
    }
    if installed.is_some() {
        actions = actions.push(
            button(text("Remove").size(13))
                .padding([8, 16])
                .style(theme::danger_button)
                .on_press(Message::RemoveNexoMod(instance.id.clone())),
        );
    }

    section.push(actions).into()
}

/// One edition in the picker. The label and the prose come from the release
/// manifest; only the rules line is the launcher's own, because that is the
/// part that has to stay true across releases.
fn edition_tile<'a>(
    build: &'a nexo_mod::ReleaseEdition,
    selected: nexo_mod::Edition,
    installed: Option<nexo_mod::Edition>,
) -> Element<'a, Message> {
    let is_selected = build.edition == selected;

    let mut inner = column![
        text(build.name.as_str()).size(14).color(theme::TEXT),
        text(build.edition.rules_note())
            .size(11)
            .color(if is_selected { theme::TEXT } else { theme::MUTED }),
    ]
    .spacing(4);

    // Straight from the manifest rather than a copy kept here, which would
    // start lying the first time a release changes what's in a jar.
    if let Some(description) = &build.description {
        inner = inner.push(text(description.as_str()).size(11).color(theme::MUTED));
    }
    if installed == Some(build.edition) {
        inner = inner.push(text("Currently installed").size(10).color(theme::MINT));
    }

    button(inner.width(Fill))
        .padding(12)
        .width(Fill)
        .style(if is_selected {
            theme::selected_tile
        } else {
            theme::tile
        })
        .on_press(Message::SelectNexoEdition(build.edition))
        .into()
}

/// What's already in the instance: a filter over the installed list, the two
/// ways to add more, and the entries themselves.
fn installed_card<'a>(app: &'a App, instance: &'a Instance) -> Element<'a, Message> {
    let needle = app.content_query.trim().to_lowercase();
    let matches: Vec<_> = instance
        .mods
        .iter()
        .filter(|m| {
            needle.is_empty()
                || m.name.to_lowercase().contains(&needle)
                || m.file_name.to_lowercase().contains(&needle)
        })
        .collect();

    // Searches what is installed, not Modrinth — adding is a separate action.
    let search = text_input("Search installed content…", &app.content_query)
        .on_input(Message::ContentQueryChanged)
        .padding(10)
        .style(theme::input)
        .width(Fill);

    let actions = row![
        button(text("Install from Modrinth").size(13))
            .padding([8, 16])
            .style(theme::primary_button)
            .on_press(Message::OpenModrinthBrowser),
        button(text("Add from file").size(13))
            .padding([8, 16])
            .style(theme::ghost_button)
            .on_press_maybe(
                (!app.is_busy()).then(|| Message::AddFromFile(instance.id.clone()))
            ),
    ]
    .spacing(10);

    let mut body = column![
        row![
            text(format!("Content ({})", instance.mods.len()))
                .size(16)
                .color(theme::TEXT)
                .width(Fill),
            actions,
        ]
        .spacing(12)
        .align_y(iced::Center),
        search,
    ]
    .spacing(12);

    if instance.mods.is_empty() {
        body = body.push(
            text("Nothing installed yet.")
                .size(12)
                .color(theme::MUTED),
        );
    } else if matches.is_empty() {
        body = body.push(
            text(format!("Nothing installed matches \"{}\".", app.content_query.trim()))
                .size(12)
                .color(theme::MUTED),
        );
    } else {
        for installed in matches {
            body = body.push(
                row![
                    icon_or_placeholder(app, &installed.project_id, 32.0),
                    column![
                        text(&installed.name).size(14).color(theme::TEXT),
                        text(&installed.file_name).size(11).color(theme::MUTED),
                    ]
                    .spacing(2)
                    .width(Fill),
                    text(&installed.version_number).size(12).color(theme::MUTED),
                    button(text("Remove").size(12))
                        .padding([6, 12])
                        .style(theme::danger_button)
                        .on_press_maybe((!app.is_busy()).then(|| Message::RemoveContent {
                            instance: instance.id.clone(),
                            project: installed.project_id.clone(),
                        })),
                ]
                .spacing(12)
                .align_y(iced::Center),
            );
        }
    }

    container(body)
        .padding(18)
        .width(Fill)
        .style(theme::card)
        .into()
}

/// The Modrinth browser, shown in place of the instance's own content view.
fn browser_card<'a>(app: &'a App, instance: &'a Instance) -> Element<'a, Message> {
    use nexo_core::content::ProjectKind;

    let search = text_input("Search Modrinth…", &app.modrinth_query)
        .on_input(Message::ModrinthQueryChanged)
        .on_submit(Message::SearchContent)
        .padding(10)
        .style(theme::input)
        .width(Fill);

    let mut filters = row![].spacing(8);
    for kind in ProjectKind::ALL {
        filters = filters.push(
            button(text(kind.label()).size(13))
                .padding([8, 14])
                // Selected filter is filled, so the current scope is obvious
                // without a separate label.
                .style(theme::nav_button(app.content_kind == kind))
                .on_press(Message::ContentKindChanged(kind)),
        );
    }

    let mut body = column![
        row![
            text("Install from Modrinth")
                .size(16)
                .color(theme::TEXT)
                .width(Fill),
            button(text("Search").size(13))
                .padding([8, 16])
                .style(theme::primary_button)
                .on_press(Message::SearchContent),
            button(text("Done").size(13))
                .padding([8, 16])
                .style(theme::ghost_button)
                .on_press(Message::CloseModrinthBrowser),
        ]
        .spacing(10)
        .align_y(iced::Center),
        search,
        filters,
    ]
    .spacing(12);

    if app.content_searching {
        body = body.push(text("Searching…").size(12).color(theme::MUTED));
    } else if app.content_results.is_empty() {
        body = body.push(
            text("Nothing found for this instance's version and loader.")
                .size(12)
                .color(theme::MUTED),
        );
    } else {
        for hit in &app.content_results {
            let installed = instance.mods.iter().any(|m| m.project_id == hit.project_id);

            body = body.push(
                container(
                    row![
                        icon_or_placeholder(app, &hit.project_id, 48.0),
                        column![
                            text(&hit.title).size(14).color(theme::TEXT),
                            text(&hit.description).size(11).color(theme::MUTED),
                            row![
                                // Downloads are the quickest signal of whether
                                // a project is the well-known one, so they get
                                // their own emphasis rather than being buried
                                // in a grey byline.
                                text(format!("↓ {}", compact(hit.downloads)))
                                    .size(11)
                                    .color(theme::MINT),
                                text(
                                    hit.author
                                        .as_deref()
                                        .map(|a| format!("by {a}"))
                                        .unwrap_or_default()
                                )
                                .size(11)
                                .color(theme::MUTED),
                            ]
                            .spacing(10),
                        ]
                        .spacing(3)
                        .width(Fill),
                        if installed {
                            button(text("Installed").size(12))
                                .padding([7, 14])
                                .style(theme::ghost_button)
                        } else {
                            button(text("Install").size(12))
                                .padding([7, 14])
                                .style(theme::primary_button)
                                .on_press_maybe((!app.is_busy()).then(|| {
                                    Message::InstallProject {
                                        instance: instance.id.clone(),
                                        project: hit.project_id.clone(),
                                    }
                                }))
                        },
                    ]
                    .spacing(12)
                    .align_y(iced::Center),
                )
                .padding(12)
                .width(Fill)
                .style(theme::card),
            );
        }
    }

    container(body)
        .padding(18)
        .width(Fill)
        .style(theme::card)
        .into()
}

/// A project's icon, or reserved space while it loads. Keeping the space
/// stops rows from reflowing as icons arrive one by one.
fn icon_or_placeholder<'a>(app: &'a App, project: &str, size: f32) -> Element<'a, Message> {
    match app.icons.get(project) {
        Some(handle) => image(handle.clone())
            .width(size)
            .height(size)
            .into(),
        None => Space::new().width(size).height(size).into(),
    }
}

/// Download counts run to millions, which are unreadable in full.
fn compact(n: u64) -> String {
    match n {
        n if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1_000_000.0),
        n if n >= 1_000 => format!("{:.1}k", n as f64 / 1_000.0),
        n => n.to_string(),
    }
}

fn details_card<'a>(app: &'a App, instance: &'a Instance) -> Element<'a, Message> {
    let field = |label: &'static str, value: String| {
        row![
            text(label).size(12).color(theme::MUTED).width(140),
            text(value).size(12).color(theme::TEXT),
        ]
        .spacing(10)
    };

    container(
        column![
            text("Details").size(16).color(theme::TEXT),
            Space::new().height(4),
            field("Folder", instance.id.clone()),
            field("Minecraft", instance.game_version.clone()),
            field("Loader", instance.loader.to_string()),
            field(
                "Memory",
                instance
                    .memory_mb
                    .map(|mb| format!("{mb} MiB"))
                    .unwrap_or_else(|| "Default".to_string()),
            ),
            field(
                "Java",
                instance
                    .java_path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "Detected automatically".to_string()),
            ),
            Space::new().height(6),
            row![
                button(text("Open folder").size(13))
                    .padding([8, 16])
                    .style(theme::ghost_button)
                    .on_press(Message::OpenFolder(instance.id.clone())),
                button(text("Export .mrpack").size(13))
                    .padding([8, 16])
                    .style(theme::ghost_button)
                    .on_press_maybe(
                        (!app.is_busy()).then(|| Message::ExportPack(instance.id.clone()))
                    ),
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

fn danger_card(instance: &Instance, running: bool) -> Element<'_, Message> {
    container(
        row![
            column![
                text("Delete instance").size(14).color(theme::TEXT),
                text("Removes the folder and everything in it, saves included.")
                    .size(12)
                    .color(theme::MUTED),
            ]
            .spacing(3)
            .width(Fill),
            button(text("Delete").size(13))
                .padding([9, 16])
                .style(theme::danger_button)
                // Deleting the folder out from under a running game would
                // leave it writing into nothing.
                .on_press_maybe(
                    (!running).then(|| Message::DeleteInstance(instance.id.clone()))
                ),
        ]
        .spacing(12)
        .align_y(iced::Center),
    )
    .padding(18)
    .width(Fill)
    .style(theme::card)
    .into()
}
