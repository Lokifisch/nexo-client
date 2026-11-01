//! The Settings screen: the theme, custom themes, the background effect and its
//! tuning, then what games start with and what happens when they do.

use crate::backdrop::{self, Effect, Tuning};
use crate::lively::{button, scrollable, text_input, toggler};
use crate::theme::{self, Custom, Palette, PALETTES};
use crate::{App, Armed, Message};
use iced::widget::{column, container, row, slider, stack, text, Space};
use iced::{Color, Element, Fill, Task};
use nexo_core::settings::{AfterLaunch, Settings};

/// The file the choice is kept in, under the data directory.
const FILE: &str = "appearance.txt";
const NAME_LIMIT: usize = 32;
/// How long after a slider last moved its colour is written to disk.
const SAVE_AFTER: std::time::Duration = std::time::Duration::from_millis(400);

/// What `appearance.txt` holds.
#[derive(Debug, PartialEq)]
struct Saved {
    selected: usize,
    effect: Option<Effect>,
    customs: Vec<Custom>,
    particles: Tuning,
    reduce_motion: bool,
}

// Built-in themes and effects are stored by name, so renaming one makes
// existing installs fall back to the default. Custom themes go by position, and
// the name comes last on its line so it may contain anything but a line break.
fn serialize(selected: usize, effect: Effect, customs: &[Custom], particles: Tuning, reduce_motion: bool) -> String {
    let theme = match PALETTES.get(selected) {
        Some(palette) => palette.name.to_string(),
        None => format!("custom:{}", selected - PALETTES.len()),
    };
    let mut out = format!(
        "theme={theme}\nbackground={}\nparticle_amount={}\nparticle_speed={}\nreduce_motion={reduce_motion}\n",
        effect.label(),
        particles.amount,
        particles.speed,
    );
    for custom in customs {
        let colors: Vec<String> = custom.colors.iter().map(|&color| theme::to_hex(color)).collect();
        out += &format!("custom={}|{}|{}\n", custom.effect.label(), colors.join("|"), custom.name);
    }
    out
}

fn effect_named(name: &str) -> Option<Effect> {
    Effect::ALL.into_iter().find(|effect| effect.label() == name)
}

fn parse_custom(line: &str) -> Option<Custom> {
    let mut parts = line.splitn(9, '|');
    // A background that no longer exists costs the background, not the theme.
    let effect = effect_named(parts.next()?).unwrap_or(Effect::Off);
    let mut colors = [Color::BLACK; 7];
    for color in &mut colors {
        *color = theme::from_hex(parts.next()?)?;
    }
    Some(Custom { name: clean_name(parts.next()?), effect, colors })
}

fn parse(contents: &str) -> Saved {
    let values = |key: &'static str| {
        contents.lines().filter_map(move |line| line.strip_prefix(key)?.strip_prefix('='))
    };
    let customs: Vec<Custom> = values("custom").filter_map(parse_custom).collect();
    let selected = values("theme")
        .next()
        .and_then(|name| match name.strip_prefix("custom:") {
            Some(at) => at.parse::<usize>().ok().filter(|&at| at < customs.len()).map(|at| at + PALETTES.len()),
            None => PALETTES.iter().position(|p| p.name == name),
        })
        .unwrap_or(0);
    let effect = values("background").next().and_then(effect_named);
    let percent = |key, default| {
        values(key).next().and_then(|v| v.parse::<u32>().ok()).unwrap_or(default).clamp(*Tuning::RANGE.start(), *Tuning::RANGE.end())
    };
    let particles = Tuning { amount: percent("particle_amount", 100), speed: percent("particle_speed", 100) };
    let reduce_motion = values("reduce_motion").next() == Some("true");
    Saved { selected, effect, customs, particles, reduce_motion }
}

fn clean_name(name: &str) -> String {
    name.chars().filter(|c| !c.is_control()).take(NAME_LIMIT).collect()
}

/// The theme at `index`, in the numbering of [`theme::selected`].
fn palette_at(customs: &[Custom], index: usize) -> Palette {
    match PALETTES.get(index) {
        Some(palette) => *palette,
        None => customs[index - PALETTES.len()].palette(),
    }
}

/// The custom theme that is showing, if one is.
fn showing_custom() -> Option<usize> {
    theme::selected().checked_sub(PALETTES.len())
}

/// The colour being edited, as the sliders and the hex box show it. Held apart
/// from the colour itself because black has no hue: worked out from the colour,
/// dragging brightness to zero would throw away the hue the slider was on.
#[derive(Debug, Default)]
pub struct Editor {
    slot: usize,
    hue: u32,
    saturation: u32,
    brightness: u32,
    hex: String,
}

impl Editor {
    fn of(slot: usize, color: Color) -> Self {
        let (hue, saturation, brightness) = theme::to_hsv(color);
        Self {
            slot,
            hue: hue.round() as u32,
            saturation: (saturation * 100.0).round() as u32,
            brightness: (brightness * 100.0).round() as u32,
            hex: theme::to_hex(color),
        }
    }

    fn color(&self) -> Color {
        theme::from_hsv(self.hue as f32, self.saturation as f32 / 100.0, self.brightness as f32 / 100.0)
    }
}

/// What boot needs out of the saved settings.
pub struct Appearance {
    pub effect: Effect,
    pub customs: Vec<Custom>,
    pub editor: Editor,
    pub particles: Tuning,
    pub reduce_motion: bool,
    pub settings: Settings,
}

/// Applies the saved theme. A missing or damaged file just means the defaults.
pub fn load() -> Appearance {
    // Tests start from the defaults, whatever this machine's user has chosen.
    let path = nexo_core::Paths::discover().ok().filter(|_| !cfg!(test)).map(|paths| paths.root().join(FILE));
    // The next save overwrites the file, so anything that can't be understood
    // is kept aside first: custom themes exist nowhere else.
    let keep_aside = || {
        if let Some(path) = &path {
            let _ = std::fs::copy(path, path.with_extension("txt.bak"));
        }
    };
    let contents = match path.as_ref().map(std::fs::read_to_string) {
        Some(Ok(contents)) => contents,
        Some(Err(error)) if error.kind() != std::io::ErrorKind::NotFound => {
            tracing::warn!("could not read the appearance settings: {error}");
            keep_aside();
            String::new()
        }
        _ => String::new(),
    };
    let saved = parse(&contents);
    if contents.lines().filter(|line| line.starts_with("custom=")).count() != saved.customs.len() {
        tracing::warn!("some custom themes could not be read; the file is kept as appearance.txt.bak");
        keep_aside();
    }
    let palette = palette_at(&saved.customs, saved.selected);
    theme::apply(saved.selected, palette);
    let editor = match saved.selected.checked_sub(PALETTES.len()) {
        Some(at) => Editor::of(0, saved.customs[at].colors[0]),
        None => Editor::default(),
    };
    crate::motion::set_reduced(saved.reduce_motion);
    let settings = nexo_core::Paths::discover().ok().filter(|_| !cfg!(test)).map_or_else(Settings::default, |paths| Settings::load(&paths));
    Appearance {
        effect: saved.effect.unwrap_or(palette.effect),
        customs: saved.customs,
        editor,
        particles: saved.particles,
        reduce_motion: saved.reduce_motion,
        settings,
    }
}

/// Best effort: failing to remember a colour isn't worth interrupting anyone.
fn save(app: &App) {
    let Ok(paths) = nexo_core::Paths::discover() else { return };
    if let Err(error) = app.settings.save(&paths) {
        tracing::warn!("could not save the settings: {error}");
    }
    let contents = serialize(theme::selected(), app.effect, &app.custom_themes, app.particles, app.reduce_motion);
    let path = paths.root().join(FILE);
    // By way of a temporary file, so a crash mid-write can't lose the themes.
    let temporary = path.with_extension("txt.tmp");
    let written = std::fs::create_dir_all(paths.root())
        .and_then(|()| std::fs::write(&temporary, contents))
        .and_then(|()| std::fs::rename(&temporary, &path));
    if let Err(error) = written {
        tracing::warn!("could not save the appearance settings: {error}");
    }
}

/// Switches to a theme and the background it comes with.
pub fn pick(app: &mut App, index: usize) {
    // A click from a view drawn before a delete can name a theme now gone.
    if index >= PALETTES.len() + app.custom_themes.len() {
        return;
    }
    app.armed = None;
    let palette = palette_at(&app.custom_themes, index);
    theme::apply(index, palette);
    app.effect = palette.effect;
    if let Some(custom) = showing_custom().map(|at| &app.custom_themes[at]) {
        app.theme_editor = Editor::of(0, custom.colors[0]);
    }
    save(app);
}

pub fn set_effect(app: &mut App, effect: Effect) {
    app.effect = effect;
    // A custom theme keeps it, for coming back to that theme later.
    if let Some(at) = showing_custom() {
        app.custom_themes[at].effect = effect;
    }
    save(app);
}

#[derive(Debug, Clone)]
pub enum Msg {
    Tab(Tab),
    Amount(u32),
    Speed(u32),
    ReduceMotion(bool),
    DefaultJava(Option<std::path::PathBuf>),
    BrowseJava,
    Memory(u32),
    JvmArgs(String),
    AfterLaunch(AfterLaunch),
    New,
    Delete,
    Name(String),
    Slot(usize),
    Hue(u32),
    Saturation(u32),
    Brightness(u32),
    Hex(String),
    /// A game has been up for [`GAME_SURVIVED_AFTER`].
    AfterLaunchDue(String),
    JavaRefused(String),
    /// A slider has been still for [`SAVE_AFTER`], if this is still the latest move.
    SaveDue(u64),
}

/// Puts `color` in the slot being edited, on screen straight away.
fn set_color(app: &mut App, at: usize, color: Color) {
    app.custom_themes[at].colors[app.theme_editor.slot] = color;
    theme::apply(theme::selected(), app.custom_themes[at].palette());
}

/// Writes settings once a slider has been still for a moment. On a timer rather
/// than on release: a slider also moves by arrow keys and Ctrl+wheel, which never
/// send a release.
fn save_soon(app: &mut App) -> Task<Message> {
    app.theme_save_due = app.theme_save_due.wrapping_add(1);
    let due = app.theme_save_due;
    Task::perform(tokio::time::sleep(SAVE_AFTER), move |_| Message::ThemeEditor(Msg::SaveDue(due)))
}

/// How long a game must have stayed up before the launcher gets out of its way.
/// A spawned process isn't a working game: a bad JVM argument dies right away.
const GAME_SURVIVED_AFTER: std::time::Duration = std::time::Duration::from_secs(5);

/// Once `game_up` says a game started, waits, then checks on that same session.
pub fn after_launch_soon(key: String, game_up: tokio::sync::oneshot::Receiver<()>) -> Task<Message> {
    Task::perform(
        async move {
            game_up.await.ok()?;
            tokio::time::sleep(GAME_SURVIVED_AFTER).await;
            Some(key)
        },
        |key| key.map_or(Message::Noop, |key| Message::ThemeEditor(Msg::AfterLaunchDue(key))),
    )
}

#[derive(Debug, PartialEq, Eq)]
enum Plan {
    Nothing,
    /// The game is already gone, so the launcher has to stay and say so.
    ReportDeadGame,
    Minimize,
    Exit,
}

/// `busy`: something on screen still needs watching, a launch being prepared or an
/// error, so the window stays. `launcher_needed`: work that closing would drop.
fn plan(after: AfterLaunch, game_running: bool, busy: bool, launcher_needed: bool) -> Plan {
    match after {
        AfterLaunch::KeepOpen => Plan::Nothing,
        _ if !game_running => Plan::ReportDeadGame,
        _ if busy => Plan::Nothing,
        AfterLaunch::Close if !launcher_needed => Plan::Exit,
        AfterLaunch::Minimize | AfterLaunch::Close => Plan::Minimize,
    }
}

fn after_launch(app: &mut App, key: &str) -> Task<Message> {
    let busy = app.preparing > 0 || matches!(app.status, crate::Status::Error(_));
    // Closing drops whatever only the launcher keeps going: an install or update in
    // progress, and a server's public address.
    let launcher_needed = app.is_busy()
        || app.update_busy
        || app.host.live.values().any(|live| live.phase != super::hosting::Phase::Offline);
    match plan(app.settings.after_launch, app.running.contains(key), busy, launcher_needed) {
        Plan::Nothing => Task::none(),
        Plan::ReportDeadGame => {
            // Pressing Stop within those seconds isn't a game that died.
            if matches!(app.status, crate::Status::Idle) && !app.stopped_by_user.remove(key) {
                app.status = crate::Status::Error(
                    "The game ended right after starting, so the launcher stayed open. Its log is on the instance's Logs tab.".into(),
                );
            }
            Task::none()
        }
        Plan::Exit => iced::exit(),
        Plan::Minimize => {
            app.minimized_for_game = true;
            iced::window::latest().and_then(|id| iced::window::minimize(id, true))
        }
    }
}

/// Brings back a window a launch minimised. Wayland doesn't let a program restore
/// or focus its own window, so there this only flags it in the taskbar.
pub fn restore(app: &mut App) -> Task<Message> {
    if !app.minimized_for_game {
        return Task::none();
    }
    app.minimized_for_game = false;
    iced::window::latest().and_then(|id| {
        iced::window::minimize(id, false)
            .chain(iced::window::gain_focus(id))
            .chain(iced::window::request_user_attention(id, Some(iced::window::UserAttention::Informational)))
    })
}

/// A game ended: once the last one has, the window comes back.
pub fn after_game_exit(app: &mut App) -> Task<Message> {
    if app.running.is_empty() { restore(app) } else { Task::none() }
}

pub fn update(app: &mut App, msg: Msg) -> Task<Message> {
    // The ones that have nothing to do with which theme is showing.
    match msg {
        Msg::Tab(tab) => {
            app.settings_tab = tab;
            return Task::none();
        }
        Msg::Amount(amount) => {
            app.particles.amount = amount;
            return save_soon(app);
        }
        Msg::Speed(speed) => {
            app.particles.speed = speed;
            return save_soon(app);
        }
        Msg::ReduceMotion(on) => {
            app.reduce_motion = on;
            crate::motion::set_reduced(on);
            save(app);
            return Task::none();
        }
        Msg::DefaultJava(path) => {
            app.settings.java_path = path;
            save(app);
            return Task::done(Message::LoadJavaOptions);
        }
        Msg::BrowseJava => {
            return Task::perform(
                // Awaited off the UI thread so the window keeps painting.
                async {
                    let file = rfd::AsyncFileDialog::new().set_title("Pick a Java executable").pick_file().await?;
                    let path = file.path().to_path_buf();
                    Some(match nexo_core::java::probe(&path).await {
                        Some(_) => Msg::DefaultJava(Some(path)),
                        None => Msg::JavaRefused(path.display().to_string()),
                    })
                },
                |picked| picked.map_or(Message::Noop, Message::ThemeEditor),
            );
        }
        Msg::JavaRefused(path) => {
            app.status = crate::Status::Error(format!("{path} is not a working Java runtime."));
            return Task::none();
        }
        Msg::AfterLaunchDue(key) => return after_launch(app, &key),
        Msg::Memory(mb) => {
            app.settings.memory_mb = Some(mb);
            return save_soon(app);
        }
        Msg::JvmArgs(args) => {
            app.settings.jvm_args = args;
            save(app);
            return Task::none();
        }
        Msg::AfterLaunch(after) => {
            app.settings.after_launch = after;
            save(app);
            return Task::none();
        }
        Msg::New => {
            let name = format!("My theme {}", app.custom_themes.len() + 1);
            app.custom_themes.push(Custom::from_palette(name, app.effect, &theme::palette()));
            pick(app, PALETTES.len() + app.custom_themes.len() - 1);
            return Task::none();
        }
        Msg::SaveDue(due) => {
            if due == app.theme_save_due {
                save(app);
            }
            return Task::none();
        }
        _ => {}
    }
    let Some(at) = showing_custom() else { return Task::none() };
    match msg {
        Msg::Delete => {
            let armed = Armed::DeleteTheme(at);
            if !app.is_armed(&armed) {
                return app.arm(armed);
            }
            app.armed = None;
            app.custom_themes.remove(at);
            pick(app, 0);
        }
        Msg::Name(name) => {
            app.custom_themes[at].name = clean_name(&name);
            save(app);
        }
        Msg::Slot(slot) => app.theme_editor = Editor::of(slot, app.custom_themes[at].colors[slot]),
        Msg::Hex(hex) => {
            app.theme_editor.hex = hex;
            if let Some(color) = theme::from_hex(&app.theme_editor.hex) {
                set_color(app, at, color);
                // Keeps what was typed, which may not be spelled the way we would.
                let hex = std::mem::take(&mut app.theme_editor.hex);
                app.theme_editor = Editor { hex, ..Editor::of(app.theme_editor.slot, color) };
                save(app);
            }
        }
        Msg::Hue(_) | Msg::Saturation(_) | Msg::Brightness(_) => {
            let editor = &mut app.theme_editor;
            match msg {
                Msg::Hue(value) => editor.hue = value,
                Msg::Saturation(value) => editor.saturation = value,
                Msg::Brightness(value) => editor.brightness = value,
                _ => {}
            }
            let color = editor.color();
            editor.hex = theme::to_hex(color);
            set_color(app, at, color);
            return save_soon(app);
        }
        // Dealt with above.
        _ => {}
    }
    Task::none()
}

/// A row of cards from a list, with a short last row kept to the same widths.
fn grid<'a>(cards: Vec<Element<'a, Message>>, per_row: usize) -> Element<'a, Message> {
    let mut cards = cards.into_iter();
    let mut rows = column![].spacing(12);
    loop {
        let chunk: Vec<_> = cards.by_ref().take(per_row).collect();
        if chunk.is_empty() {
            return rows.into();
        }
        let missing = per_row - chunk.len();
        let mut cells = row(chunk).spacing(12);
        for _ in 0..missing {
            cells = cells.push(Space::new().width(Fill));
        }
        rows = rows.push(cells);
    }
}

fn theme_card<'a>(index: usize, name: &'a str, palette: Palette) -> Element<'a, Message> {
    let sample = container(
        column![
            container(Space::new().width(Fill).height(16)).style(theme::swatch(palette)),
            row![
                text("Aa").size(14).color(palette.text),
                text("Text").size(12).color(palette.muted),
                Space::new().width(Fill),
                container(Space::new().width(10).height(10)).style(theme::fill(palette.accent)),
                container(Space::new().width(10).height(10)).style(theme::fill(palette.mint)),
            ]
            .spacing(8)
            .align_y(iced::Center),
        ]
        .spacing(10),
    )
    .padding(10)
    .width(Fill)
    .style(theme::swatch_ground(palette));

    button(column![sample, text(name).size(14).color(theme::text())].spacing(8))
        .width(Fill)
        .padding(10)
        .style(if theme::selected() == index { theme::selected_tile } else { theme::tile })
        .on_press(Message::SetTheme(index))
        .into()
}

fn new_theme_card<'a>() -> Element<'a, Message> {
    button(
        column![
            container(text("+").size(24).color(theme::accent())).width(Fill).height(64).center_x(Fill).center_y(64),
            text("New theme").size(14).color(theme::text()),
        ]
        .spacing(8),
    )
    .width(Fill)
    .padding(10)
    .style(theme::tile)
    .on_press(Message::ThemeEditor(Msg::New))
    .into()
}

fn effect_card<'a>(effect: Effect, shown: Effect, tuning: Tuning, live: bool) -> Element<'a, Message> {
    button(
        column![
            // The real thing, so choosing doesn't mean going back to Home to look.
            container(backdrop::view(effect, tuning, live, 72)).width(Fill).style(theme::well),
            text(effect.label()).size(14).color(theme::text()),
        ]
        .spacing(8),
    )
    .width(Fill)
    .padding(10)
    .style(if effect == shown { theme::selected_tile } else { theme::tile })
    .on_press(Message::SetEffect(effect))
    .into()
}

/// One colour slider, over a track showing what it does.
fn color_slider<'a>(
    label: &'a str,
    range: std::ops::RangeInclusive<u32>,
    value: u32,
    stops: Vec<Color>,
    on_change: impl Fn(u32) -> Msg + 'a,
) -> Element<'a, Message> {
    let track = container(container(Space::new().width(Fill).height(8)).style(theme::color_track(stops)))
        .height(20)
        .center_y(20);
    let knob = slider(range, value, move |value| Message::ThemeEditor(on_change(value))).height(20);
    row![
        text(label).size(13).color(theme::muted()).width(84),
        stack![track, crate::lively::slider(knob, theme::bare_slider)].width(Fill),
    ]
    .spacing(10)
    .align_y(iced::Center)
    .into()
}

fn editor<'a>(app: &'a App, at: usize) -> Element<'a, Message> {
    let custom = &app.custom_themes[at];
    let edit = &app.theme_editor;

    let delete = button(text(if app.is_armed(&Armed::DeleteTheme(at)) { "Really delete?" } else { "Delete" }).size(13))
        .padding([8, 12])
        .style(theme::danger_button)
        .on_press(Message::ThemeEditor(Msg::Delete));

    let slots = Custom::SLOTS.iter().enumerate().map(|(slot, &label)| {
        button(
            row![
                container(Space::new().width(14).height(14)).style(theme::fill(custom.colors[slot])),
                text(label).size(12),
            ]
            .spacing(6)
            .align_y(iced::Center),
        )
        .padding([6, 8])
        .style(theme::nav_chip(slot == edit.slot))
        .on_press(Message::ThemeEditor(Msg::Slot(slot)))
        .into()
    });

    let (h, s, v) = (edit.hue as f32, edit.saturation as f32 / 100.0, edit.brightness as f32 / 100.0);
    let spectrum = (0..=6).map(|step| theme::from_hsv(step as f32 * 60.0, 1.0, 1.0)).collect();

    let mut panel = column![
        row![
            text_input("Theme name", &custom.name)
                .on_input(|name| Message::ThemeEditor(Msg::Name(name)))
                .padding([8, 10])
                .size(14)
                .style(theme::input)
                .width(Fill),
            delete,
        ]
        .spacing(10)
        .align_y(iced::Center),
        row(slots).spacing(6).wrap().vertical_spacing(6),
        color_slider("Hue", 0..=360, edit.hue, spectrum, Msg::Hue),
        color_slider("Saturation", 0..=100, edit.saturation, vec![theme::from_hsv(h, 0.0, v), theme::from_hsv(h, 1.0, v)], Msg::Saturation),
        color_slider("Brightness", 0..=100, edit.brightness, vec![Color::BLACK, theme::from_hsv(h, s, 1.0)], Msg::Brightness),
        row![
            text("Hex").size(13).color(theme::muted()).width(84),
            text_input("#rrggbb", &edit.hex)
                .on_input(|hex| Message::ThemeEditor(Msg::Hex(hex)))
                .padding([6, 10])
                .size(13)
                .style(theme::input)
                .width(120),
        ]
        .spacing(10)
        .align_y(iced::Center),
    ]
    .spacing(12);

    for problem in custom.problems() {
        panel = panel.push(text(problem).size(13).color(theme::amber()));
    }
    container(panel).padding(14).width(Fill).style(theme::card).into()
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Appearance,
    Games,
}

fn heading(label: &str) -> iced::widget::Text<'_> {
    text(label).size(15).font(theme::BOLD).color(theme::text())
}

/// A labelled row: the label, the control, and a line of help under them.
fn field<'a>(label: &'a str, help: &'a str, control: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    column![
        row![text(label).size(14).color(theme::text()).width(160), control.into()]
            .spacing(10)
            .align_y(iced::Center),
        row![Space::new().width(160), text(help).size(12).color(theme::muted())].spacing(10),
    ]
    .spacing(4)
    .into()
}

/// A plain slider for a number, with what it's set to beside it.
fn number_slider<'a>(
    range: std::ops::RangeInclusive<u32>,
    step: u32,
    value: u32,
    shown: String,
    on_change: impl Fn(u32) -> Msg + 'a,
) -> Element<'a, Message> {
    let stops = vec![theme::control(), theme::accent()];
    let track = container(container(Space::new().width(Fill).height(8)).style(theme::color_track(stops)))
        .height(20)
        .center_y(20);
    let knob = slider(range, value, move |value| Message::ThemeEditor(on_change(value))).step(step).height(20);
    row![
        stack![track, crate::lively::slider(knob, theme::bare_slider)].width(Fill),
        text(shown).size(13).color(theme::text()).width(70),
    ]
    .spacing(10)
    .align_y(iced::Center)
    .into()
}

fn games_view(app: &App) -> Element<'_, Message> {
    use super::instance::{java_picker, JavaChoice};
    let settings = &app.settings;

    let java = row![
        java_picker(app, settings.java_path.as_deref(), JavaChoice::Automatic, |path| Message::ThemeEditor(Msg::DefaultJava(path))),
        button(text("Browse…").size(12))
            .padding([5, 9])
            .style(theme::ghost_button)
            .on_press(Message::ThemeEditor(Msg::BrowseJava)),
    ]
    .spacing(10)
    .align_y(iced::Center);
    let java_help = match &settings.java_path {
        Some(path) => path.to_str().unwrap_or("Java"),
        None => "Uses the newest Java on this PC, and downloads one if it has none new enough.",
    };

    let memory = settings.memory_mb.unwrap_or(nexo_core::minecraft::DEFAULT_MEMORY_MB);
    let memory_slider = number_slider(
        1024..=super::hosting::ram_max(app).max(memory),
        256,
        memory,
        format!("{} GB", super::hosting::format_gb(memory)),
        Msg::Memory,
    );
    let total = app.host.total_memory_mb;
    // The same line the server wizard draws: past this, the system itself starves.
    let memory_help = if total > 0 && memory > total * 7 / 10 {
        "This leaves too little for the rest of your system."
    } else {
        "How much memory a game may use. Most packs are happy with 4 to 8 GB."
    };

    let after = [
        (AfterLaunch::KeepOpen, "Keep open"),
        (AfterLaunch::Minimize, "Minimise"),
        (AfterLaunch::Close, "Close"),
    ]
    .map(|(choice, label)| {
        button(text(label).size(13))
            .padding([6, 12])
            .style(theme::nav_chip(settings.after_launch == choice))
            .on_press(Message::ThemeEditor(Msg::AfterLaunch(choice)))
            .into()
    });

    // Anything else would be taken for the class to run, and no game would start.
    let args_help = if settings.jvm_arg_list().iter().any(|arg| !arg.starts_with(['-', '@'])) {
        "Every argument has to start with a dash. Spaces inside a value aren't supported."
    } else {
        "Added to every game launch, separated by spaces. A wrong one stops games from starting."
    };

    column![
        heading("New and existing instances"),
        text("An instance only follows these while it doesn't set its own Java or memory.").size(13).color(theme::muted()),
        Space::new().height(4),
        field("Java", java_help, java),
        field("Memory", memory_help, memory_slider),
        field(
            "JVM arguments",
            args_help,
            text_input("-XX:+UseG1GC", &settings.jvm_args)
                .on_input(|args| Message::ThemeEditor(Msg::JvmArgs(args)))
                .padding([8, 10])
                .size(13)
                .style(theme::input)
                .width(Fill),
        ),
        Space::new().height(8),
        heading("Launcher"),
        field(
            "After launching",
            "Happens once the game has stayed up for a few seconds. After Close, reopening the launcher won't show that game as running. Close minimises instead while a server or install is running.",
            row(after).spacing(6),
        ),
    ]
    .spacing(12)
    .into()
}

pub fn view(app: &App) -> Element<'_, Message> {
    let tab = |label, tab| {
        button(text(label).size(13))
            .padding([6, 12])
            .style(theme::nav_chip(app.settings_tab == tab))
            .on_press(Message::ThemeEditor(Msg::Tab(tab)))
    };
    let tabs = row![tab("Appearance", Tab::Appearance), tab("Games", Tab::Games)].spacing(6);

    let page = match app.settings_tab {
        Tab::Appearance => appearance_view(app),
        Tab::Games => games_view(app),
    };

    column![
        super::header("Settings", "", None),
        Space::new().height(12),
        tabs,
        Space::new().height(16),
        scrollable(container(page).padding(iced::Padding { right: 14.0, ..Default::default() })).height(Fill),
    ]
    .into()
}

fn appearance_view(app: &App) -> Element<'_, Message> {

    let mut themes: Vec<Element<'_, Message>> = PALETTES
        .iter()
        .enumerate()
        .map(|(index, palette)| theme_card(index, palette.name, *palette))
        .collect();
    themes.extend(app.custom_themes.iter().enumerate().map(|(at, custom)| {
        theme_card(PALETTES.len() + at, &custom.name, custom.palette())
    }));
    themes.push(new_theme_card());

    let effects = Effect::ALL.into_iter().map(|effect| effect_card(effect, app.effect, app.particles, app.focused)).collect();
    let percent = |value| format!("{value}%");

    let mut page = column![heading("Theme"), grid(themes, 3)].spacing(12);
    if let Some(at) = showing_custom() {
        page = page.push(editor(app, at));
    }
    page.push(Space::new().height(8))
        .push(heading("Background"))
        .push(text("A theme brings its own background; pick another here if you like.").size(13).color(theme::muted()))
        .push(grid(effects, 4))
        .push(field("Particle amount", "", number_slider(Tuning::RANGE, 5, app.particles.amount, percent(app.particles.amount), Msg::Amount)))
        .push(field("Particle speed", "", number_slider(Tuning::RANGE, 5, app.particles.speed, percent(app.particles.speed), Msg::Speed)))
        .push(Space::new().height(8))
        .push(heading("Motion"))
        .push(field(
            "Reduce motion",
            "Pages and buttons change straight away instead of animating. The background has its own Off.",
            toggler(app.reduce_motion).on_toggle(|on| Message::ThemeEditor(Msg::ReduceMotion(on))).style(theme::toggle),
        ))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn custom(name: &str) -> Custom {
        Custom::from_palette(name.to_string(), Effect::Snow, &PALETTES[3])
    }

    #[test]
    fn reads_what_it_writes() {
        let customs = vec![custom("One"), custom("pipes | are | fine")];
        for selected in [0, 5, PALETTES.len(), PALETTES.len() + 1] {
            for effect in Effect::ALL {
                let particles = Tuning { amount: 40, speed: 150 };
                let saved = parse(&serialize(selected, effect, &customs, particles, true));
                assert_eq!(saved, Saved { selected, effect: Some(effect), customs: customs.clone(), particles, reduce_motion: true });
            }
        }
    }

    #[test]
    fn survives_damage() {
        let nothing = || Saved { selected: 0, effect: None, customs: vec![], particles: Tuning::default(), reduce_motion: false };
        assert_eq!(parse(""), nothing());
        // Out of range or nonsense, from a hand edit.
        assert_eq!(parse("particle_amount=99999\nparticle_speed=x\n").particles, Tuning { amount: 200, speed: 100 });
        let damaged = parse("theme=Gone\nbackground=Gone\n\u{0}\ncustom=Snow|#nope\ncustom=Gone|x\n");
        assert_eq!(damaged, nothing());
        // An unknown background loses the background, not the theme.
        let line = serialize(0, Effect::Off, &[custom("Kept")], Tuning::default(), false).replace("custom=Snow", "custom=Gone");
        assert_eq!(parse(&line).customs, vec![Custom { effect: Effect::Off, ..custom("Kept") }]);
        // A theme pointing past the end, say after the file was edited by hand.
        assert_eq!(parse("theme=custom:9\n").selected, 0);
        assert_eq!(parse("theme=custom:-1\n").selected, 0);
    }

    #[test]
    fn names_stay_on_one_line() {
        assert_eq!(clean_name("a\nb\r\tc"), "abc");
        assert_eq!(clean_name(&"x".repeat(100)).len(), NAME_LIMIT);
    }

    #[test]
    fn what_happens_after_a_launch() {
        use AfterLaunch::*;
        assert_eq!(plan(KeepOpen, true, false, false), Plan::Nothing);
        assert_eq!(plan(KeepOpen, false, false, false), Plan::Nothing);
        assert_eq!(plan(Minimize, true, false, false), Plan::Minimize);
        assert_eq!(plan(Close, true, false, false), Plan::Exit);
        assert_eq!(plan(Close, true, false, true), Plan::Minimize, "work only the launcher keeps going");
        assert_eq!(plan(Close, false, false, false), Plan::ReportDeadGame);
        assert_eq!(plan(Minimize, false, false, true), Plan::ReportDeadGame);
        assert_eq!(plan(Minimize, false, true, false), Plan::ReportDeadGame, "this game's death still gets told");
        assert_eq!(plan(Minimize, true, true, false), Plan::Nothing, "a launch or an error on screen");
        assert_eq!(plan(Close, true, true, false), Plan::Nothing);
    }

    #[test]
    fn dragging_to_black_keeps_the_hue() {
        let editor = Editor { hue: 200, saturation: 80, brightness: 0, ..Default::default() };
        assert_eq!(editor.color(), Color::from_rgb(0.0, 0.0, 0.0));
        assert_eq!(editor.hue, 200);
        let back = Editor { brightness: 100, ..editor };
        assert_eq!(theme::to_hex(back.color()), theme::to_hex(theme::from_hsv(200.0, 0.8, 1.0)));
    }

    /// The whole round through `App::update`. Ignored because it writes the
    /// settings file and switches the global theme, so run it alone with a
    /// scratch data dir: `XDG_DATA_HOME=<dir> cargo test -p nexo-app
    /// custom_theme_flow -- --ignored`.
    #[test]
    #[ignore]
    fn custom_theme_flow() {
        let root = nexo_core::Paths::discover().unwrap();
        assert!(!root.root().starts_with(std::env::var("HOME").unwrap() + "/.local"), "set XDG_DATA_HOME");
        let on_disk = || parse(&std::fs::read_to_string(root.root().join(FILE)).unwrap());
        // Arming Delete starts its disarm timer.
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let (mut app, _) = App::boot();
        let edit = |app: &mut App, msg| { let _ = app.update(Message::ThemeEditor(msg)); };

        // A new theme starts as a copy of the look it was made from, and is selected.
        let before = theme::palette();
        edit(&mut app, Msg::New);
        assert_eq!(theme::selected(), PALETTES.len());
        assert_eq!(theme::palette().accent, before.accent);
        assert_eq!(on_disk().customs.len(), 1);

        // Moving a slider shows at once, and only the latest move gets written.
        edit(&mut app, Msg::Slot(2));
        edit(&mut app, Msg::Hue(120));
        edit(&mut app, Msg::Saturation(100));
        edit(&mut app, Msg::Brightness(100));
        assert_eq!(theme::to_hex(theme::accent()), "#00ff00");
        assert_eq!(theme::current().palette().primary, theme::accent());
        assert_ne!(theme::to_hex(on_disk().customs[0].colors[2]), "#00ff00");
        let latest = app.theme_save_due;
        edit(&mut app, Msg::SaveDue(latest - 1));
        assert_ne!(theme::to_hex(on_disk().customs[0].colors[2]), "#00ff00");
        edit(&mut app, Msg::SaveDue(latest));
        assert_eq!(theme::to_hex(on_disk().customs[0].colors[2]), "#00ff00");

        // Half-typed hex leaves the colour alone.
        edit(&mut app, Msg::Hex("#ff00".into()));
        assert_eq!(theme::to_hex(theme::accent()), "#00ff00");
        edit(&mut app, Msg::Hex("#ff0000".into()));
        assert_eq!(theme::to_hex(theme::accent()), "#ff0000");
        assert_eq!(app.theme_editor.hue, 0);

        // A custom theme remembers its background, past leaving and coming back.
        let _ = app.update(Message::SetEffect(Effect::Portal));
        let _ = app.update(Message::SetTheme(1));
        assert_eq!((theme::palette().name, app.effect), ("Matrix", Effect::Rain));
        let _ = app.update(Message::SetTheme(PALETTES.len()));
        assert_eq!((theme::to_hex(theme::accent()).as_str(), app.effect), ("#ff0000", Effect::Portal));

        // Saved and read back, it comes up the same.
        let saved = on_disk();
        assert_eq!((saved.selected, saved.effect), (PALETTES.len(), Some(Effect::Portal)));
        assert_eq!(saved.customs, app.custom_themes);

        // Settings that aren't colours work whichever theme is showing, and land on disk.
        edit(&mut app, Msg::ReduceMotion(true));
        assert!(crate::motion::reduced());
        assert_eq!(crate::lively::step(0.001), 1.0, "nothing eases");
        edit(&mut app, Msg::ReduceMotion(false));
        edit(&mut app, Msg::Amount(50));
        edit(&mut app, Msg::Memory(6144));
        edit(&mut app, Msg::JvmArgs("-XX:+UseG1GC".into()));
        edit(&mut app, Msg::AfterLaunch(AfterLaunch::Minimize));
        let latest = app.theme_save_due;
        edit(&mut app, Msg::SaveDue(latest));
        assert_eq!(on_disk().particles.amount, 50);
        assert_eq!(Settings::load(&root), app.settings);
        assert_eq!(Settings::load(&root).memory_for(&nexo_core::Instance::new("x", "26.3", nexo_core::Loader::Fabric)), 6144);

        // A game that died before the launcher got out of its way gets reported.
        assert!(app.running.is_empty());
        app.status = crate::Status::Idle;
        edit(&mut app, Msg::AfterLaunchDue("gone#player".into()));
        assert!(matches!(app.status, crate::Status::Error(_)));
        // Unless it was stopped on purpose.
        app.status = crate::Status::Idle;
        app.stopped_by_user.insert("gone#player".into());
        edit(&mut app, Msg::AfterLaunchDue("gone#player".into()));
        assert!(matches!(app.status, crate::Status::Idle));
        assert!(!app.minimized_for_game);

        // Switching away disarms a Delete; a click on a theme that's gone does nothing.
        edit(&mut app, Msg::Delete);
        let _ = app.update(Message::SetTheme(1));
        let _ = app.update(Message::SetTheme(PALETTES.len()));
        assert!(!app.is_armed(&Armed::DeleteTheme(0)));
        let _ = app.update(Message::SetTheme(PALETTES.len() + 5));
        assert_eq!(theme::selected(), PALETTES.len());

        // Delete takes two clicks, and leaves a theme that exists.
        edit(&mut app, Msg::Delete);
        assert_eq!(app.custom_themes.len(), 1);
        edit(&mut app, Msg::Delete);
        assert!(app.custom_themes.is_empty());
        assert_eq!(theme::selected(), 0);
        assert!(on_disk().customs.is_empty());
    }
}
