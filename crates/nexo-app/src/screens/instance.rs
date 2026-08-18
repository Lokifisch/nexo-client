use crate::{pulse, theme};
use crate::{App, Message, Screen};
use iced::widget::{
    button, column, container, image, pick_list, row, scrollable, text, text_input, Space,
};
use iced::{Element, Fill, Font};
use nexo_core::browse;
use nexo_core::instance::InstalledMod;
use nexo_core::nexo_mod;
use nexo_core::util::human_bytes;
use nexo_core::Instance;
use std::path::PathBuf;

/// The four views onto an instance.
///
/// Deliberately the same set the other launchers offer, in the same order:
/// this is the one screen where someone arrives already knowing what they are
/// looking for, and inventing a different vocabulary for it would only make
/// them hunt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Tab {
    #[default]
    Content,
    Files,
    Worlds,
    Logs,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Content, Tab::Files, Tab::Worlds, Tab::Logs];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Content => "Content",
            Tab::Files => "Files",
            Tab::Worlds => "Worlds",
            Tab::Logs => "Logs",
        }
    }

    /// A glyph rather than an icon font: these are all in the fonts every
    /// target already ships, so the tab strip needs no asset and cannot come
    /// up as tofu on a machine missing one.
    pub fn glyph(self) -> &'static str {
        match self {
            Tab::Content => "◈",
            Tab::Files => "▤",
            Tab::Worlds => "◍",
            Tab::Logs => "≡",
        }
    }
}

/// Details for a single instance: what it is, how to launch it, and the four
/// tabs onto what is inside it.
///
/// The details sit above the tabs rather than inside one of them because they
/// describe the instance itself — which Minecraft, which loader, which JVM —
/// and stay true whichever tab is open. Filing them under a tab would mean
/// leaving the file you are looking at to check what version it belongs to.
pub fn view<'a>(app: &'a App, instance: &'a Instance) -> Element<'a, Message> {
    let running = app.running.contains(&instance.id);

    let back = button(text("‹ Instances").size(13))
        .padding([5, 10])
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

    let body = match app.tab {
        Tab::Content => content_tab(app, instance),
        Tab::Files => files_tab(app, instance),
        Tab::Worlds => worlds_tab(app, instance),
        Tab::Logs => logs_tab(app, instance),
    };

    column![
        back,
        row![heading, launch_control(app, instance, running)]
            .spacing(16)
            .align_y(iced::Center),
        details_card(app, instance, running),
        tab_bar(app, instance),
        body,
    ]
    .spacing(14)
    .height(Fill)
    .into()
}

/// The tab strip. Every tab carries its own underline, so the unselected ones
/// join into one rule and the selected one breaks it.
fn tab_bar<'a>(app: &'a App, instance: &'a Instance) -> Element<'a, Message> {
    let mut bar = row![].spacing(2).align_y(iced::Bottom);

    for tab in Tab::ALL {
        let selected = app.tab == tab;

        let mut label = row![
            text(tab.glyph())
                .size(13)
                .color(if selected { app.accent() } else { theme::MUTED }),
            text(tab.label()).size(14),
        ]
        .spacing(8)
        .align_y(iced::Center);

        // Only for a tab that has actually counted. A badge reading 0 on a
        // directory nobody has opened yet would be a claim, not a number.
        if let Some(count) = app.tab_count(tab, instance) {
            label = label.push(
                container(text(count.to_string()).size(10))
                    .padding([1, 7])
                    .style(theme::tab_badge(selected)),
            );
        }

        bar = bar.push(
            column![
                button(label)
                    .padding([9, 16])
                    .style(theme::tab_button(selected))
                    .on_press(Message::SelectTab(tab)),
                container(Space::new().width(Fill).height(if selected { 3 } else { 2 }))
                    .style(theme::tab_underline(selected)),
            ]
            .spacing(0),
        );
    }

    // Carries the rule out to the edge of the screen, so the strip reads as a
    // divider the content hangs from rather than as four floating buttons.
    bar.push(container(Space::new().width(Fill).height(2)).style(theme::tab_underline(false)))
        .into()
}

/// What is installed, and the two ways to install more.
fn content_tab<'a>(app: &'a App, instance: &'a Instance) -> Element<'a, Message> {
    scrollable(if app.browsing {
        column![browser_card(app, instance)].spacing(14).width(Fill)
    } else {
        column![nexo_mod_card(app, instance), installed_card(app, instance)]
            .spacing(14)
            .width(Fill)
    })
    .height(Fill)
    .into()
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
            .padding([10, 26])
            .style(theme::stop_button)
            .on_press(Message::Stop(instance.id.clone()))
            .into();
    }

    let label = if app.is_busy() { "Preparing…" } else { "Play" };

    let play = button(text(label).size(15))
        .padding([10, 26])
        // The one place the full ramp is spent — see `theme::hero_button`.
        .style(theme::hero_button)
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
                    .padding([7, 13])
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
                .padding([8, 17])
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
                .padding([7, 13])
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
            .padding([7, 13])
            .style(theme::primary_button)
            .on_press(Message::OpenModrinthBrowser),
        button(text("Add from file").size(13))
            .padding([7, 13])
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
                        .padding([5, 10])
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
                .padding([6, 12])
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
                .padding([7, 13])
                .style(theme::primary_button)
                .on_press(Message::SearchContent),
            button(text("Done").size(13))
                .padding([7, 13])
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
                                .padding([6, 12])
                                .style(theme::ghost_button)
                        } else {
                            button(text("Install").size(12))
                                .padding([6, 12])
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

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

/// The instance directory, browsable in place.
///
/// Read-only apart from opening things: this exists so someone can check
/// whether a config landed where they think it did without leaving the
/// launcher, and a file manager is one button away for anything more.
fn files_tab<'a>(app: &'a App, instance: &'a Instance) -> Element<'a, Message> {
    let mut header = row![breadcrumb(app)].spacing(10).align_y(iced::Center);

    header = header.push(
        button(text("Open in file manager").size(13))
            .padding([7, 13])
            .style(theme::ghost_button)
            .on_press(Message::OpenFolder(instance.id.clone())),
    );

    let mut list = column![].spacing(2).width(Fill);

    // A folder that has vanished under us is worth saying out loud — the
    // alternative is a listing that silently reads as empty.
    if let Some(err) = &app.files_error {
        list = list.push(text(err.as_str()).size(12).color(theme::DANGER));
    } else if app.files.is_empty() {
        list = list.push(
            text(if app.files_at.as_os_str().is_empty() {
                "This instance has no files yet. Installing something or launching it once will fill it in."
            } else {
                "This folder is empty."
            })
            .size(12)
            .color(theme::MUTED),
        );
    }

    for entry in &app.files {
        let meta = row![
            text(if entry.is_dir {
                String::new()
            } else {
                human_bytes(entry.size)
            })
            .size(11)
            .color(theme::MUTED)
            .width(80),
            text(entry.modified.map(ago).unwrap_or_default())
                .size(11)
                .color(theme::MUTED)
                .width(110),
        ]
        .spacing(10)
        .align_y(iced::Center);

        let label = row![
            // A glyph rather than an icon: it is the one distinction the list
            // has to make at a glance, and it costs no asset.
            text(if entry.is_dir { "▸" } else { "·" })
                .size(13)
                .color(if entry.is_dir {
                    theme::VIOLET
                } else {
                    theme::MUTED
                })
                .width(14),
            text(entry.name.as_str())
                .size(13)
                .color(theme::TEXT)
                .width(Fill),
            meta,
        ]
        .spacing(10)
        .align_y(iced::Center);

        // Directories navigate; files hand off to whatever the OS uses for
        // them. Both are the whole row, since a row-sized target is easier to
        // hit than a word.
        list = list.push(
            button(label)
                .padding([7, 10])
                .width(Fill)
                .style(theme::row_button(false))
                .on_press(if entry.is_dir {
                    Message::BrowseFiles(entry.rel.clone())
                } else {
                    Message::OpenPath(entry.path.clone())
                }),
        );
    }

    column![
        header,
        // Room on the right for the overlay scrollbar, which otherwise sits
        // on top of each row's size and timestamp.
        scrollable(container(list).padding(iced::Padding {
            top: 4.0,
            right: 12.0,
            bottom: 4.0,
            left: 0.0,
        }))
        .height(Fill),
    ]
    .spacing(12)
    .height(Fill)
    .into()
}

/// Where the browser is, one clickable segment per level.
fn breadcrumb(app: &App) -> Element<'_, Message> {
    let mut trail = row![].spacing(4).align_y(iced::Center);

    let at_root = app.files_at.as_os_str().is_empty();
    trail = trail.push(
        button(text("Instance").size(13))
            .padding([5, 8])
            .style(theme::row_button(at_root))
            .on_press(Message::BrowseFiles(PathBuf::new())),
    );

    // Each segment gets the path *up to and including itself*, so clicking
    // one goes there rather than to wherever the browser happens to be.
    let mut so_far = PathBuf::new();
    let last = app.files_at.components().count();
    for (index, component) in app.files_at.components().enumerate() {
        so_far.push(component);
        trail = trail.push(text("/").size(12).color(theme::MUTED));
        trail = trail.push(
            button(text(component.as_os_str().to_string_lossy().into_owned()).size(13))
                .padding([5, 8])
                .style(theme::row_button(index + 1 == last))
                .on_press(Message::BrowseFiles(so_far.clone())),
        );
    }

    trail.width(Fill).into()
}

// ---------------------------------------------------------------------------
// Worlds
// ---------------------------------------------------------------------------

/// Everywhere this instance can be played: single-player worlds under
/// `saves/`, and the multiplayer list out of `servers.dat`.
///
/// One tab rather than two because that is the question being asked — *where
/// can I go* — and splitting it would mean knowing whether a place is a folder
/// or an address before you can look for it.
fn worlds_tab<'a>(app: &'a App, instance: &'a Instance) -> Element<'a, Message> {
    let running = app.running.contains(&instance.id);

    let mut list = column![section_heading(
        "Singleplayer",
        app.worlds.len(),
        None,
    )]
    .spacing(10)
    .width(Fill);

    if app.worlds.is_empty() {
        list = list.push(empty_note(
            "No worlds yet",
            "Worlds appear here once this instance has been played.",
        ));
    }
    for world in &app.worlds {
        list = list.push(world_row(app, instance, world));
    }

    // The form is open for a *new* server when it carries no index; an open
    // edit belongs against its own row further down.
    let adding = matches!(&app.server_form, Some(form) if form.editing.is_none());

    list = list.push(Space::new().height(8));
    list = list.push(section_heading(
        "Servers",
        app.servers.len(),
        Some(
            row![
                // Pings happen once per instance, so a server that was down
                // when the tab opened would read as down until the instance is
                // reopened. This is the way to ask again.
                button(text("Refresh").size(12))
                    .padding([6, 12])
                    .style(theme::ghost_button)
                    .on_press(Message::RepingServers),
                button(text(if adding { "Cancel" } else { "Add server" }).size(12))
                    .padding([6, 12])
                    .style(theme::ghost_button)
                    .on_press(if adding {
                        Message::CloseServerForm
                    } else {
                        Message::OpenServerForm(None)
                    }),
            ]
            .spacing(8)
            .into(),
        ),
    ));

    if adding && let Some(form) = &app.server_form {
        list = list.push(server_form(form, running));
    }

    if app.servers.is_empty() && !adding {
        list = list.push(empty_note(
            "No servers yet",
            "Servers you add in-game show up here, and so does anything added above.",
        ));
    }

    for server in &app.servers {
        // The form takes the row's place while that row is being edited, so
        // the fields sit exactly where the values they replace were.
        match &app.server_form {
            Some(form) if form.editing == Some(server.index) => {
                list = list.push(server_form(form, running))
            }
            _ => list = list.push(server_row(app, server)),
        }
    }

    // The scrollbar is an overlay, so without room reserved for it on the
    // right it sits on top of each row's buttons.
    scrollable(container(list).padding([0, 12]))
        .height(Fill)
        .into()
}

/// A heading over one of the tab's two lists, with its count and an optional
/// action on the right.
fn section_heading<'a>(
    label: &'a str,
    count: usize,
    action: Option<Element<'a, Message>>,
) -> Element<'a, Message> {
    let mut head = row![
        text(label).size(13).color(theme::TEXT),
        text(count.to_string()).size(12).color(theme::MUTED),
        Space::new().width(Fill),
    ]
    .spacing(8)
    .align_y(iced::Center);

    if let Some(action) = action {
        head = head.push(action);
    }
    head.into()
}

fn empty_note<'a>(title: &'a str, detail: &'a str) -> Element<'a, Message> {
    container(
        column![
            text(title).size(15).color(theme::TEXT),
            text(detail).size(12).color(theme::MUTED),
        ]
        .spacing(4),
    )
    .padding(18)
    .width(Fill)
    .style(theme::card)
    .into()
}

/// Name and address, written straight into `servers.dat`. One form for both
/// adding and editing — the only difference is where it is rendered and which
/// verb the button carries.
fn server_form(form: &crate::ServerForm, running: bool) -> Element<'_, Message> {
    let ready = !form.address.trim().is_empty() && !running;
    let editing = form.editing.is_some();

    let mut body = column![
        row![
            text_input("Name (optional)", &form.name)
                .on_input(Message::ServerFormNameChanged)
                .padding(9)
                .style(theme::input)
                .width(Fill),
            text_input("Address — mc.example.com or 192.168.1.9:25577", &form.address)
                .on_input(Message::ServerFormAddressChanged)
                .on_submit_maybe(ready.then_some(Message::SubmitServerForm))
                .padding(9)
                .style(theme::input)
                .width(Fill),
        ]
        .spacing(10),
    ]
    .spacing(10);

    // Minecraft writes the whole list back when it closes the multiplayer
    // screen, so anything added now would vanish on exit. Said out loud rather
    // than left as a mysteriously dead button.
    if running {
        body = body.push(
            text("Close the game first — Minecraft overwrites its server list on exit.")
                .size(11)
                .color(theme::DANGER),
        );
    }

    let mut actions = row![Space::new().width(Fill)].spacing(10);
    // An edit is rendered in place of its row, so without its own way out
    // there would be nothing on screen to dismiss it.
    if editing {
        actions = actions.push(
            button(text("Cancel").size(13))
                .padding([7, 16])
                .style(theme::ghost_button)
                .on_press(Message::CloseServerForm),
        );
    }
    body = body.push(
        actions.push(
            button(text(if editing { "Save" } else { "Add" }).size(13))
                .padding([7, 16])
                .style(theme::primary_button)
                .on_press_maybe(ready.then_some(Message::SubmitServerForm)),
        ),
    );

    container(body)
        .padding(14)
        .width(Fill)
        .style(theme::card)
        .into()
}

/// One entry from the multiplayer list, filled in by a live ping.
fn server_row<'a>(app: &'a App, server: &'a browse::Server) -> Element<'a, Message> {
    // The icon a server publishes is 64×64, so this is a downscale rather than
    // the blur an upscale would give.
    let icon: Element<'a, Message> = match app.server_icons.get(&server.address) {
        Some(handle) => image(handle.clone()).width(48).height(48).into(),
        None => container(Space::new().width(48).height(48))
            .style(theme::well)
            .into(),
    };

    // Three states, and they have to stay distinguishable: still waiting, up,
    // or unreachable. Collapsing "waiting" into "down" would report every
    // server as offline for the first few seconds the tab is open.
    let detail: Element<'a, Message> = match app.server_status.get(&server.address) {
        None => text("Pinging…").size(11).color(theme::MUTED).into(),
        Some(Err(err)) => row![
            text("Unreachable").size(11).color(theme::DANGER),
            text(err.as_str()).size(11).color(theme::MUTED),
        ]
        .spacing(8)
        .into(),
        Some(Ok(status)) => column![
            text(status.motd.as_str()).size(12).color(theme::TEXT),
            row![
                text(format!(
                    "{}/{} online",
                    status.players_online, status.players_max
                ))
                .size(11)
                .color(theme::MINT),
                text(status.version.as_str()).size(11).color(theme::MUTED),
                text(format!("{} ms", status.latency_ms))
                    .size(11)
                    .color(theme::MUTED),
            ]
            .spacing(10),
        ]
        .spacing(3)
        .into(),
    };

    container(
        row![
            icon,
            column![
                row![
                    text(server.name.as_str()).size(15).color(theme::TEXT),
                    text(server.address.as_str()).size(11).color(theme::MUTED),
                ]
                .spacing(8)
                .align_y(iced::Center),
                detail,
            ]
            .spacing(4)
            .width(Fill),
            // Addressed by index rather than by name: two servers may share
            // both a name and an address, so position is the only identity
            // an edit can safely act on.
            button(text("✎").size(14))
                .padding([6, 11])
                .style(theme::ghost_button)
                .on_press(Message::OpenServerForm(Some(server.index))),
        ]
        .spacing(14)
        .align_y(iced::Center),
    )
    .padding(14)
    .width(Fill)
    .style(theme::card)
    .into()
}

fn world_row<'a>(
    app: &'a App,
    instance: &'a Instance,
    world: &'a browse::World,
) -> Element<'a, Message> {
    // Minecraft's own world icon, straight off disk — iced loads it lazily and
    // caches it, so there is no reason to route it through app state the way
    // the network-fetched project icons are.
    let icon: Element<'a, Message> = match &world.icon {
        Some(path) => image(path).width(56).height(56).into(),
        None => container(Space::new().width(56).height(56))
            .style(theme::well)
            .into(),
    };

    let mut tags = row![].spacing(10).align_y(iced::Center);
    if world.hardcore {
        tags = tags.push(text("Hardcore").size(11).color(theme::DANGER));
    }
    if let Some(mode) = world.mode {
        tags = tags.push(text(mode).size(11).color(theme::MUTED));
    }
    if let Some(version) = &world.game_version {
        // The version that last *wrote* the world, which is not always the
        // instance's — opening a world in an older Minecraft is how they get
        // broken, so the mismatch is worth colouring.
        let matches_instance = *version == instance.game_version;
        tags = tags.push(text(version.as_str()).size(11).color(if matches_instance {
            theme::MUTED
        } else {
            theme::MAGENTA
        }));
    }
    tags = tags.push(text(human_bytes(world.size)).size(11).color(theme::MUTED));
    tags = tags.push(
        text(
            world
                .last_played
                .map(|when| format!("played {}", ago(when)))
                .unwrap_or_else(|| "never played".to_string()),
        )
        .size(11)
        .color(theme::MUTED),
    );

    let mut details = column![text(world.name.as_str()).size(15).color(theme::TEXT)]
        .spacing(4)
        .width(Fill);

    // Only when it says something the name doesn't. Minecraft names the folder
    // after the world, so for most worlds these two lines are identical and
    // the second is pure noise; it earns its place exactly when a world has
    // been renamed and the folder no longer matches.
    if world.folder != world.name {
        details = details.push(text(world.folder.as_str()).size(11).color(theme::MUTED));
    }
    details = details.push(tags);

    // Two steps, like the saved-skin grid: this is the only control in the
    // launcher that can destroy a world, and the folder is gone for good.
    let actions: Element<'a, Message> = if app.confirm_delete_world.as_deref() == Some(&world.folder)
    {
        row![
            text("Delete for good?").size(12).color(theme::DANGER),
            button(text("Cancel").size(12))
                .padding([6, 12])
                .style(theme::ghost_button)
                .on_press(Message::AskDeleteWorld(None)),
            button(text("Delete").size(12))
                .padding([6, 12])
                .style(theme::danger_button)
                .on_press(Message::DeleteWorld(world.folder.clone())),
        ]
        .spacing(8)
        .align_y(iced::Center)
        .into()
    } else {
        row![
            button(text("Open folder").size(12))
                .padding([6, 12])
                .style(theme::ghost_button)
                .on_press(Message::OpenPath(world.path.clone())),
            button(text("Delete").size(12))
                .padding([6, 12])
                .style(theme::ghost_button)
                .on_press(Message::AskDeleteWorld(Some(world.folder.clone()))),
        ]
        .spacing(8)
        .align_y(iced::Center)
        .into()
    };

    container(
        row![icon, details, actions]
            .spacing(14)
            .align_y(iced::Center),
    )
    .padding(14)
    .width(Fill)
    .style(theme::card)
    .into()
}

// ---------------------------------------------------------------------------
// Logs
// ---------------------------------------------------------------------------

/// `logs/` and `crash-reports/`, with the selected file's tail beside them.
fn logs_tab<'a>(app: &'a App, instance: &'a Instance) -> Element<'a, Message> {
    let running = app.running.contains(&instance.id);
    let mut list = column![].spacing(2).width(Fill);

    if app.logs.is_empty() {
        list = list.push(
            text("No logs yet — they appear after the first launch.")
                .size(12)
                .color(theme::MUTED),
        );
    }

    for log in &app.logs {
        let selected = app.selected_log.as_deref() == Some(&log.name);

        let mut title = row![text(log.name.as_str()).size(12).color(if log.crash {
            // The file someone came to this tab to find.
            theme::DANGER
        } else {
            theme::TEXT
        })]
        .spacing(6)
        .align_y(iced::Center);

        // The same dot as the viewer's, so the file that is being written is
        // recognisable in the list without selecting it first.
        if is_live_file(log) {
            title = title.push(pulse::view(if running {
                pulse::Pulse::live(app.clock)
            } else {
                pulse::Pulse::idle()
            }));
        }

        list = list.push(
            button(
                column![
                    title,
                    text(format!(
                        "{}{}",
                        human_bytes(log.size),
                        log.modified.map(|w| format!(" · {}", ago(w))).unwrap_or_default()
                    ))
                    .size(10)
                    .color(theme::MUTED),
                ]
                .spacing(2)
                .width(Fill),
            )
            .padding([7, 10])
            .width(Fill)
            .style(theme::row_button(selected))
            .on_press(Message::SelectLog(log.name.clone())),
        );
    }

    let selected = app
        .selected_log
        .as_ref()
        .and_then(|name| app.logs.iter().find(|l| &l.name == name));

    let viewer: Element<'_, Message> = match (selected, &app.log_text) {
        (Some(log), Some((body, truncated))) => {
            let live = running && is_live_file(log);

            let mut head = row![text(log.name.as_str()).size(14).color(theme::TEXT)]
                .spacing(8)
                .align_y(iced::Center);

            // `latest.log` is the session's own log, so it gets the indicator
            // either way: pulsing while the game writes to it, dimmed once it
            // stops. Two states rather than one, because a dot that simply
            // disappears reads as a bug in the dot.
            if is_live_file(log) {
                head = head.push(pulse::view(if live {
                    pulse::Pulse::live(app.clock)
                } else {
                    pulse::Pulse::idle()
                }));
                head = head.push(
                    text(if live { "Live" } else { "Last session" })
                        .size(11)
                        .color(pulse::label_color(live)),
                );
            }

            head = head.push(Space::new().width(Fill));

            // Saying so matters: without it the first visible line looks like
            // the start of the session, and someone would go looking for a
            // startup error that is simply off the top.
            if *truncated {
                head = head.push(text("showing the end only").size(11).color(theme::MUTED));
            }

            head = head.push(
                button(text("Open file").size(12))
                    .padding([6, 12])
                    .style(theme::ghost_button)
                    .on_press(Message::OpenPath(log.path.clone())),
            );

            column![
                head,
                container(
                    scrollable(
                        container(
                            text(body.as_str())
                                .size(11)
                                .font(Font::MONOSPACE)
                                .color(theme::TEXT)
                        )
                        .padding(12)
                    )
                    // Measured from the bottom, so a log that grows under the
                    // viewer keeps its newest line in view instead of pushing
                    // it off the end and making the reader chase it.
                    .anchor_bottom()
                    .height(Fill)
                    .width(Fill)
                )
                .style(theme::well)
                .height(Fill),
            ]
            .spacing(10)
            .height(Fill)
            .into()
        }
        (Some(_), None) => centered_note("Reading…"),
        (None, _) if app.logs.is_empty() => centered_note("Nothing to show."),
        (None, _) => centered_note("Pick a log to read it."),
    };

    row![
        container(scrollable(list).height(Fill))
            .width(250)
            .height(Fill),
        container(viewer).width(Fill).height(Fill),
    ]
    .spacing(14)
    .height(Fill)
    .into()
}

/// Whether this is the log the game writes into as it runs.
///
/// Minecraft's own naming, not a guess: `latest.log` is the open handle and
/// everything else in `logs/` has been rotated out and closed. Kept in one
/// place because the tab bar, the file list and the viewer all have to agree
/// on which row gets the indicator.
pub fn is_live_file(log: &browse::LogFile) -> bool {
    !log.crash && log.name == "latest.log"
}

fn centered_note(message: &str) -> Element<'_, Message> {
    container(text(message.to_string()).size(12).color(theme::MUTED))
        .width(Fill)
        .height(Fill)
        .align_x(iced::Center)
        .align_y(iced::Center)
        .style(theme::well)
        .into()
}

/// Coarse relative time, for rows where the exact second never matters.
///
/// Clamped at zero rather than going negative: a file's mtime can sit slightly
/// in the future after a clock correction or on a network share, and "in 3
/// seconds" would be a strange thing for a log to say.
fn ago(when: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(when);
    let seconds = now.saturating_sub(when);

    match seconds {
        s if s < 60 => "just now".to_string(),
        s if s < 3600 => format!("{}m ago", s / 60),
        s if s < 86_400 => format!("{}h ago", s / 3600),
        s if s < 86_400 * 30 => format!("{}d ago", s / 86_400),
        s => format!("{}mo ago", s / (86_400 * 30)),
    }
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

/// One entry in the Java picker.
///
/// `Automatic` is not "no Java" — it means "whatever `java::ensure` decides",
/// which prefers a system JVM and downloads one only when there is nothing
/// suitable. It stays the default because it is right for almost everyone;
/// the picker exists for the machine with four JDKs where it guesses wrong.
#[derive(Clone, PartialEq, Eq)]
pub enum JavaChoice {
    Automatic,
    Explicit(nexo_core::java::JavaInstall),
}

impl std::fmt::Display for JavaChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Automatic => f.write_str("Automatic"),
            Self::Explicit(java) => {
                // The version alone is ambiguous on a machine with several
                // builds of the same release, so the directory it lives in
                // comes along. The full path would be too wide for the row —
                // it is still shown underneath the picker.
                let where_ = java
                    .path
                    .parent()
                    .and_then(|p| p.parent())
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                if where_.is_empty() {
                    write!(f, "Java {}", java.version)
                } else {
                    write!(f, "Java {} · {where_}", java.version)
                }
            }
        }
    }
}

/// Which JVM this instance launches with.
fn java_field<'a>(app: &'a App, instance: &'a Instance) -> Element<'a, Message> {
    let mut choices = vec![JavaChoice::Automatic];
    choices.extend(app.java_options.iter().cloned().map(JavaChoice::Explicit));

    // An instance can name a JVM that discovery no longer finds — an uninstalled
    // JDK, an unplugged drive. Adding it keeps the picker showing what is
    // actually configured instead of silently snapping back to Automatic, which
    // would look like the setting had been lost.
    let selected = match &instance.java_path {
        None => JavaChoice::Automatic,
        Some(path) => {
            let known = app.java_options.iter().find(|j| &j.path == path).cloned();
            match known {
                Some(java) => JavaChoice::Explicit(java),
                None => {
                    let missing = nexo_core::java::JavaInstall {
                        path: path.clone(),
                        major: 0,
                        version: "not found".to_string(),
                    };
                    choices.push(JavaChoice::Explicit(missing.clone()));
                    JavaChoice::Explicit(missing)
                }
            }
        }
    };

    let id = instance.id.clone();
    let picker = pick_list(choices, Some(selected), move |choice| {
        Message::SetInstanceJava {
            instance: id.clone(),
            path: match choice {
                JavaChoice::Automatic => None,
                JavaChoice::Explicit(java) => Some(java.path),
            },
        }
    })
    .padding(6)
    .text_size(12)
    .width(260);

    let label_width = 42;
    let mut rows = column![
        row![
            text("Java").size(12).color(theme::MUTED).width(label_width),
            picker,
            button(text("Browse…").size(12))
                .padding([5, 9])
                .style(theme::ghost_button)
                .on_press(Message::BrowseForJava(instance.id.clone())),
        ]
        .spacing(10)
        .align_y(iced::Center)
    ]
    .spacing(4)
    .width(Fill);

    // The exact path matters when two entries read alike, and it is the only
    // way to see what Automatic actually resolved to.
    if let Some(path) = &instance.java_path {
        rows = rows.push(
            row![
                Space::new().width(label_width),
                text(path.display().to_string()).size(11).color(theme::MUTED),
            ]
            .spacing(10),
        );
    }

    rows.into()
}

/// The facts about the instance, and the actions that apply to all of it.
///
/// Laid out across rather than down: this sits above the tabs now, so every
/// row it takes is a row the tab below loses. The old stacked-label form cost
/// five, this costs two.
fn details_card<'a>(app: &'a App, instance: &'a Instance, running: bool) -> Element<'a, Message> {
    let fact = |label: &'static str, value: String| {
        column![
            text(label).size(10).color(theme::MUTED),
            text(value).size(13).color(theme::TEXT),
        ]
        .spacing(2)
    };

    let facts = row![
        fact("Minecraft", instance.game_version.clone()),
        fact(
            "Loader",
            match &instance.loader_version {
                Some(version) => format!("{} {version}", instance.loader),
                None => instance.loader.to_string(),
            }
        ),
        fact(
            "Memory",
            instance
                .memory_mb
                .map(|mb| format!("{mb} MiB"))
                .unwrap_or_else(|| "Default".to_string()),
        ),
        fact("Folder", instance.id.clone()),
        fact(
            "Content",
            match instance.mods.len() {
                1 => "1 item".to_string(),
                n => format!("{n} items"),
            }
        ),
    ]
    .spacing(28);

    container(
        column![
            facts,
            row![
                java_field(app, instance),
                row![
                    button(text("Open folder").size(13))
                        .padding([7, 13])
                        .style(theme::ghost_button)
                        .on_press(Message::OpenFolder(instance.id.clone())),
                    button(text("Export .mrpack").size(13))
                        .padding([7, 13])
                        .style(theme::ghost_button)
                        .on_press_maybe(
                            (!app.is_busy()).then(|| Message::ExportPack(instance.id.clone()))
                        ),
                    // Rehomed here when the tabs took over the card stack the
                    // danger card used to sit in. It belongs with the other
                    // whole-instance actions anyway — the tabs are all about
                    // what is *inside* the instance.
                    button(text("Delete").size(13))
                        .padding([7, 13])
                        .style(theme::danger_button)
                        // Deleting the folder out from under a running game
                        // would leave it writing into nothing.
                        .on_press_maybe(
                            (!running).then(|| Message::DeleteInstance(instance.id.clone()))
                        ),
                ]
                .spacing(10),
            ]
            .spacing(16)
            .align_y(iced::Center),
        ]
        .spacing(14),
    )
    .padding(16)
    .width(Fill)
    .style(theme::card)
    .into()
}
