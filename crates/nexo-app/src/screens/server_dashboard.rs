//! One hosted server's dashboard: header with lifecycle and address, a tab
//! column (performance, console, players, settings, files), and the online
//! players beside it. State and messages are in `hosting`.

use super::hosting::{self, DIFFICULTIES, Filter, GAME_MODES, Msg, Phase, Range, Tab};
use super::server_wizard::{address_row, ram_picker};
use crate::{App, Message, theme};
use iced::mouse;
use iced::widget::canvas::{self, Frame, Geometry, Path, Stroke};
use crate::lively::button;
use crate::dropdown::dropdown;
use crate::lively::scrollable;
use crate::lively::text_input;
use crate::lively::toggler;
use crate::lively::Tint;
use iced::widget::{Space, column, container, row};
use crate::lively::text;
use iced::{Color, Element, Fill, Font, Point, Rectangle, Renderer, Theme};
use nexo_core::hosting::{HostedServer, Sample};
use std::time::Instant;

fn host(msg: Msg) -> Message {
    Message::Host(msg)
}

pub fn view<'a>(app: &'a App, server: &'a HostedServer) -> Element<'a, Message> {
    let body = move || -> Element<'a, Message> {
        let content = match app.host.tab {
            Tab::Overview => overview(app, server),
            Tab::Console => console(app, server),
            Tab::Players => players(app, server),
            Tab::Settings => settings(app, server),
            Tab::Files => files(app, server),
        };
        container(content).padding(20).width(Fill).height(Fill).style(theme::card).into()
    };

    column![
        header(app, server),
        tab_bar(app),
        // The players panel only where it leaves the tab room to breathe; a
        // narrow window still has them on the Players tab.
        super::reveal(
            iced::widget::responsive(move |size| {
                if size.width >= 900.0 {
                    row![body(), side_panel(app, server)].spacing(14).height(Fill).into()
                } else {
                    body()
                }
            }),
            app.tab_content_progress(),
            app.page_rise,
        ),
    ]
    .spacing(14)
    .height(Fill)
    .into()
}

/// Same strip as the instance screen's tabs, so the two read as one family.
fn tab_bar(app: &App) -> Element<'_, Message> {
    let mut bar = row![].spacing(4).align_y(iced::Bottom);
    for tab in Tab::ALL {
        let amount = app.motion.get(("hosttab", tab.label()), if app.host.tab == tab { 1.0 } else { 0.0 });
        bar = bar.push(
            column![
                button(text(tab.label()).size(13).font(theme::bold()))
                    .padding([9, 16])
                    .style(theme::tab_button(amount))
                    .on_press(host(Msg::Tab(tab))),
                super::tab_underline(amount),
            ]
            .width(iced::Shrink),
        );
    }
    column![bar, container(Space::new().width(Fill).height(1)).style(theme::divider)].into()
}

fn header<'a>(app: &'a App, server: &'a HostedServer) -> Element<'a, Message> {
    let phase = app.host.phase(&server.id);
    let (label, color) = hosting::phase_label(phase);
    let initial = server.kind.label().chars().next().unwrap_or('S').to_string();

    let mut actions = row![hosting::lifecycle_button(app, server)].spacing(8).align_y(iced::Center);
    if phase == Phase::Online {
        actions = actions.push(
            button(text("Restart").size(14))
                .padding([7, 14])
                .style(theme::ghost_button)
                .on_press(host(Msg::Restart(server.id.clone()))),
        );
    }

    let address = app.host.address(server);
    let public = app.host.live.get(&server.id).is_some_and(|l| l.domain.is_some());
    let chip = row![
        container(crate::lively::changed(public, text(if public { "Public" } else { "This PC" }).size(11)))
            .padding([2, 8])
            .tinted(theme::pill(if public { theme::mint() } else { theme::muted() })),
        text(address.clone()).size(13).font(Font::MONOSPACE).color(theme::text()),
        button(text("Copy").size(12)).padding([4, 10]).style(theme::ghost_button).on_press(host(Msg::Copy(address))),
    ]
    .spacing(10)
    .align_y(iced::Center);

    container(column![
        row![
            button(text("←").size(16))
                .padding([6, 12])
                .style(theme::ghost_button)
                .on_press(Message::Navigate(crate::Screen::Servers)),
            container(text(initial).size(20).font(theme::bold()))
                .width(48)
                .height(48)
                .align_x(iced::Center)
                .align_y(iced::Center)
                .tinted(theme::initial_tile(phase == Phase::Online)),
            column![
                row![
                    text(&server.name).size(22).font(theme::bold()).color(theme::text()),
                    container(crate::lively::changed(label, text(label).size(11))).padding([2, 9]).tinted(theme::pill(color)),
                ]
                .spacing(10)
                .align_y(iced::Center),
                text(format!("{} · {}", server.kind.label(), server.minecraft_version)).size(13).color(theme::muted()),
            ]
            .spacing(6)
            .width(Fill),
            actions,
        ]
        .spacing(14)
        .align_y(iced::Center),
        container(chip).padding([6, 12]).width(Fill).style(theme::well),
    ]
    .spacing(12))
    .padding([12, 16])
    .width(Fill)
    .style(theme::card)
    .into()
}

fn side_panel<'a>(app: &'a App, server: &'a HostedServer) -> Element<'a, Message> {
    let online = app.host.phase(&server.id) == Phase::Online;
    let count = match (&app.host.players, online) {
        (Some(p), true) => format!("{}/{}", p.online, p.max),
        _ => "–".into(),
    };
    let invite = button(container(text("Copy invite address").size(14)).width(Fill).align_x(iced::Center))
        .padding([9, 12])
        .width(Fill)
        .style(theme::primary_button)
        .on_press_maybe(online.then(|| host(Msg::Copy(app.host.address(server)))));

    let mut list = column![].spacing(6);
    match (&app.host.players, online) {
        (_, false) => list = list.push(text("The server is offline.").size(13).color(theme::muted())),
        (Some(p), true) if !p.names.is_empty() => {
            for name in &p.names {
                list = list.push(
                    container(text(name.as_str()).size(14).color(theme::text())).padding([7, 10]).width(Fill).style(theme::well),
                );
            }
        }
        _ => list = list.push(text("No players online.").size(13).color(theme::muted())),
    }

    let mut col = column![
        row![
            text("Online players").size(15).font(theme::bold()).color(theme::text()),
            Space::new().width(Fill),
            text(count).size(13).color(theme::muted()),
        ],
        invite,
        scrollable(list).height(Fill),
    ]
    .spacing(12);

    if let Some(live) = app.host.live.get(&server.id) {
        if let Some(err) = &live.error {
            col = col.push(crate::lively::appear(container(text(err.as_str()).size(12)).padding(10).width(Fill).style(theme::banner(true))));
        }
        if let Some(err) = &live.tunnel_error {
            col = col.push(
                container(
                    column![
                        text(format!("Public address: {err}")).size(12),
                        button(text("Reconnect").size(12))
                            .padding([4, 10])
                            .style(theme::ghost_button)
                            .on_press_maybe(online.then(|| host(Msg::SetTunnel(server.id.clone(), true)))),
                    ]
                    .spacing(6),
                )
                .padding(10)
                .width(Fill)
                .style(theme::banner(true)),
            );
        }
    }

    container(col).padding(16).width(270).height(Fill).style(theme::card).into()
}

fn section<'a>(heading: &'a str, sub: &'a str) -> Element<'a, Message> {
    column![
        text(heading).size(20).font(theme::bold()).color(theme::text()),
        text(sub).size(13).color(theme::muted())
    ]
    .spacing(4)
    .into()
}

// ----------------------------------------------------------------- overview

fn overview<'a>(app: &'a App, server: &'a HostedServer) -> Element<'a, Message> {
    let live = app.host.live.get(&server.id);
    let online = app.host.phase(&server.id) != Phase::Offline;
    let latest = live.and_then(|l| l.samples.back()).map(|(_, s)| *s).filter(|_| online);

    let uptime = match (online, server.started_at) {
        (true, Some(at)) => duration(now_secs().saturating_sub(at)),
        _ => "–".into(),
    };
    let tiles = row![
        tile(
            "CPU usage",
            latest.map_or("–".into(), |s| format!("{:.1}%", s.cpu_percent)),
            latest.map_or(0.0, |s| s.cpu_percent / 100.0),
            theme::magenta(),
        ),
        tile(
            "RAM usage",
            latest.map_or("–".into(), |s| format!("{} MB", s.memory_mb)),
            latest.map_or(0.0, |s| s.memory_mb as f32 / server.memory_mb.max(1) as f32),
            theme::mint(),
        ),
        tile("Uptime", uptime, if online { 1.0 } else { 0.0 }, theme::accent()),
    ]
    .spacing(12);

    let mut ranges = row![].spacing(4);
    for r in Range::ALL {
        ranges = ranges.push(
            button(text(r.label()).size(12))
                .padding([4, 10])
                .style(theme::row_button(app.host.range == r))
                .on_press(host(Msg::Range(r))),
        );
    }

    let samples: Vec<(f32, Sample)> = match live {
        Some(l) => {
            let now = Instant::now();
            l.samples.iter().map(|(t, s)| (now.duration_since(*t).as_secs_f32(), *s)).collect()
        }
        None => Vec::new(),
    };
    let graph: Element<'_, Message> = if samples.len() < 2 {
        container(text(if online { "Collecting samples…" } else { "Start the server to see its performance." }).size(13).color(theme::muted()))
            .width(Fill)
            .height(Fill)
            .align_x(iced::Center)
            .align_y(iced::Center)
            .into()
    } else {
        iced::widget::Canvas::new(Graph {
            samples,
            newest: live.and_then(|l| l.samples.back().map(|(t, _)| *t)),
            range: app.host.range.secs(),
            range_label: app.host.range.label(),
            memory_mb: server.memory_mb,
        })
            .width(Fill)
            .height(Fill)
            .into()
    };

    let legend = |c: Color, l: &'static str| {
        row![container(Space::new()).width(10).height(10).style(theme::fill(c)), text(l).size(12).color(theme::muted())]
            .spacing(6)
            .align_y(iced::Center)
    };

    column![
        section("Server performance", "Live CPU and memory, sampled every two seconds."),
        tiles,
        container(
            column![
                row![text("Performance graph").size(15).font(theme::bold()).color(theme::text()), Space::new().width(Fill), ranges]
                    .align_y(iced::Center),
                graph,
                row![legend(theme::magenta(), "CPU (% of this PC)"), legend(theme::mint(), "RAM (MB)")].spacing(18),
            ]
            .spacing(12),
        )
        .padding(16)
        .height(Fill)
        .style(theme::well),
    ]
    .spacing(16)
    .into()
}

fn tile<'a>(label: &'a str, value: String, share: f32, color: Color) -> Element<'a, Message> {
    let share = share.clamp(0.0, 1.0);
    let bar = crate::lively::meter(share, 4.0, color, theme::control());
    container(
        column![text(label).size(13).color(theme::muted()), crate::lively::ticking(value.clone(), text(value).size(26).font(theme::bold())), bar].spacing(8),
    )
    .padding(16)
    .width(Fill)
    .style(theme::well)
    .into()
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn duration(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m {}s", secs / 60, secs % 60),
        3600..86400 => format!("{}h {}m", secs / 3600, secs % 3600 / 60),
        _ => format!("{}d {}h", secs / 86400, secs % 86400 / 3600),
    }
}

/// CPU (left axis, 0-100%) and RAM (right axis, 0 to the allocation) over the
/// chosen window, newest at the right edge.
struct Graph {
    /// (age in seconds, sample), oldest first.
    samples: Vec<(f32, Sample)>,
    newest: Option<Instant>,
    range: f32,
    range_label: &'static str,
    memory_mb: u32,
}

/// What the graph is showing now, on its way to what it should.
#[derive(Default)]
struct GraphMotion {
    /// When the newest sample it has seen was taken, to spot a new one arriving.
    newest: Option<Instant>,
    /// 0 when the newest sample just arrived, 1 once the line has reached it.
    grow: f32,
    /// The window on screen; slides to a newly chosen range.
    span: f32,
    last: Option<Instant>,
}

impl<Message> canvas::Program<Message> for Graph {
    type State = GraphMotion;

    fn update(&self, state: &mut GraphMotion, event: &iced::Event, _: Rectangle, _: mouse::Cursor) -> Option<canvas::Action<Message>> {
        let iced::Event::Window(iced::window::Event::RedrawRequested(now)) = event else {
            return None;
        };
        // Seen for the first time, it shows what it has: nothing to catch up on.
        if state.span == 0.0 {
            *state = GraphMotion { newest: self.newest, grow: 1.0, span: self.range, last: None };
        } else if self.newest != state.newest {
            state.newest = self.newest;
            state.grow = 0.0;
        }
        if state.grow == 1.0 && state.span == self.range {
            state.last = None;
            return None;
        }
        let k = crate::lively::step(crate::lively::tick(&mut state.last, *now, true));
        state.grow = crate::lively::num(state.grow, 1.0, k, 0.004);
        state.span = crate::lively::num(state.span, self.range, k, 0.5);
        Some(canvas::Action::request_redraw())
    }

    fn draw(&self, state: &GraphMotion, renderer: &Renderer, _: &Theme, bounds: Rectangle, _: mouse::Cursor) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let (left, right, top, bottom) = (44.0, bounds.width - 62.0, 8.0, bounds.height - 20.0);
        let (w, h) = ((right - left).max(1.0), (bottom - top).max(1.0));

        for i in 0..=4 {
            let y = top + h * i as f32 / 4.0;
            frame.stroke(
                &Path::line(Point::new(left, y), Point::new(right, y)),
                Stroke::default().with_color(theme::line()).with_width(1.0),
            );
            let share = 1.0 - i as f32 / 4.0;
            label(&mut frame, format!("{:.0}%", share * 100.0), Point::new(4.0, y - 7.0));
            label(&mut frame, format!("{:.0} MB", share * self.memory_mb as f32), Point::new(right + 6.0, y - 7.0));
        }
        let span = if state.span > 0.0 { state.span } else { self.range };
        label(&mut frame, format!("{} ago", self.range_label), Point::new(left, bottom + 4.0));
        label(&mut frame, "now".into(), Point::new(right - 22.0, bottom + 4.0));

        let visible: Vec<&(f32, Sample)> = self.samples.iter().filter(|(age, _)| *age <= span).collect();
        if visible.len() < 2 {
            return vec![frame.into_geometry()];
        }
        let x = |age: f32| left + w * (1.0 - age / span);
        let series = |value: &dyn Fn(&Sample) -> f32, color: Color, frame: &mut Frame| {
            let at = |(age, s): &(f32, Sample)| Point::new(x(*age), bottom - h * value(s).clamp(0.0, 1.0));
            // The newest point travels out from the one before it, drawing the line on.
            let newest = visible[visible.len() - 1] as *const (f32, Sample);
            let before = at(visible[visible.len() - 2]);
            let point = |sample: &(f32, Sample)| {
                let there = at(sample);
                if std::ptr::eq(sample, newest) {
                    Point::new(before.x + (there.x - before.x) * state.grow, before.y + (there.y - before.y) * state.grow)
                } else {
                    there
                }
            };
            let line = Path::new(|b| {
                b.move_to(point(visible[0]));
                for p in &visible[1..] {
                    b.line_to(point(p));
                }
            });
            let area = Path::new(|b| {
                b.move_to(Point::new(x(visible[0].0), bottom));
                for p in &visible {
                    b.line_to(point(p));
                }
                b.line_to(Point::new(point(visible[visible.len() - 1]).x, bottom));
                b.close();
            });
            frame.fill(&area, theme::alpha(color, 0.12));
            frame.stroke(&line, Stroke::default().with_color(color).with_width(2.0));
        };
        let mem = self.memory_mb.max(1) as f32;
        series(&|s| s.memory_mb as f32 / mem, theme::mint(), &mut frame);
        series(&|s| s.cpu_percent / 100.0, theme::magenta(), &mut frame);
        vec![frame.into_geometry()]
    }
}

fn label(frame: &mut Frame, content: String, at: Point) {
    frame.fill_text(canvas::Text {
        content,
        position: at,
        color: theme::muted(),
        size: 11.0.into(),
        font: theme::regular(),
        ..Default::default()
    });
}

// ------------------------------------------------------------------ console

fn level(line: &str) -> Filter {
    if line.contains("ERROR]") || line.contains("FATAL]") || line.contains("Exception") {
        Filter::Errors
    } else if line.contains("WARN]") {
        Filter::Warnings
    } else {
        Filter::Info
    }
}

fn console<'a>(app: &'a App, server: &'a HostedServer) -> Element<'a, Message> {
    let raw = app.host.console.as_deref().unwrap_or("");
    // Every RCON call the dashboard makes (the player list, every five
    // seconds) logs a connect and a disconnect line. They'd bury the log.
    let lines: Vec<(usize, &str, Filter)> = raw
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.contains("Thread RCON Client"))
        .map(|(i, l)| (i + 1, l, level(l)))
        .collect();
    let count = |f: Filter| lines.iter().filter(|(_, _, l)| *l == f).count();

    let chip = |f: Filter, label: String, color: Color| {
        button(text(label).size(12).color(color))
            .padding([5, 10])
            .style(theme::row_button(app.host.filter == f))
            .on_press(host(Msg::Filter(f)))
    };
    let chips = row![
        chip(Filter::All, format!("All ({})", lines.len()), theme::text()),
        chip(Filter::Errors, format!("Errors ({})", count(Filter::Errors)), theme::danger()),
        chip(Filter::Warnings, format!("Warnings ({})", count(Filter::Warnings)), theme::amber()),
        chip(Filter::Info, format!("Info ({})", count(Filter::Info)), theme::accent()),
    ]
    .spacing(6);

    // One text block, not a widget per line: a busy log is thousands of lines.
    let matching: Vec<&(usize, &str, Filter)> = lines
        .iter()
        .filter(|(_, _, l)| app.host.filter == Filter::All || *l == app.host.filter)
        .collect();
    let shown: Vec<String> = matching[matching.len().saturating_sub(600)..]
        .iter()
        .map(|(n, l, _)| format!("{n:>5}  {l}"))
        .collect();
    let body = if app.host.console.is_none() {
        "Reading…".to_string()
    } else if shown.is_empty() {
        "Nothing here yet.".to_string()
    } else {
        shown.join("\n")
    };

    let online = app.host.phase(&server.id) == Phase::Online;
    let input = text_input(if online { "Type a command — e.g. say hello, time set day" } else { "Start the server to send commands" }, &app.host.command)
        .on_input_maybe(online.then_some(|c| host(Msg::Command(c))))
        .on_submit(host(Msg::SendCommand))
        .padding(10)
        .style(theme::input)
        .width(Fill);
    let send = button(text("Send").size(14))
        .padding([10, 18])
        .style(theme::primary_button)
        .on_press_maybe((online && !app.host.command.trim().is_empty()).then_some(host(Msg::SendCommand)));

    let mut col = column![
        row![chips, Space::new().width(Fill), text(hosting_console_name()).size(12).color(theme::muted())].align_y(iced::Center),
        container(
            scrollable(container(text(body).size(11).font(Font::MONOSPACE).color(theme::text())).padding(12))
                .anchor_bottom()
                .width(Fill)
                .height(Fill),
        )
        .height(Fill)
        .style(theme::well),
        row![input, send].spacing(8).align_y(iced::Center),
    ]
    .spacing(12)
    .height(Fill);
    if let Some(reply) = &app.host.reply {
        col = col.push(container(text(reply.as_str()).size(12).font(Font::MONOSPACE).color(theme::text())).padding(10).width(Fill).style(theme::well));
    }
    col.into()
}

fn hosting_console_name() -> &'static str {
    nexo_core::hosting::CONSOLE_LOG
}

// ------------------------------------------------------------------ players

fn players<'a>(app: &'a App, server: &'a HostedServer) -> Element<'a, Message> {
    let online = app.host.phase(&server.id) == Phase::Online;
    let name = app.host.player_input.trim().to_string();
    let action = |label: &'static str, verb: &'static str| {
        button(text(label).size(13))
            .padding([8, 12])
            .style(theme::ghost_button)
            .on_press_maybe((online && !name.is_empty()).then(|| host(Msg::PlayerAction(verb, name.clone()))))
    };

    let mut list = column![].spacing(8);
    match &app.host.players {
        Some(p) if online && !p.names.is_empty() => {
            for player in &p.names {
                let act = |label: &'static str, verb: &'static str, danger: bool| {
                    button(text(label).size(12))
                        .padding([5, 10])
                        .style(if danger { theme::danger_button } else { theme::ghost_button })
                        .on_press(host(Msg::PlayerAction(verb, player.clone())))
                };
                list = list.push(
                    container(
                        row![
                            text(player.as_str()).size(15).color(theme::text()).width(Fill),
                            act("Make operator", "op", false),
                            act("Remove operator", "deop", false),
                            act("Kick", "kick", true),
                            act("Ban", "ban", true),
                        ]
                        .spacing(8)
                        .align_y(iced::Center),
                    )
                    .padding([10, 14])
                    .style(theme::well),
                );
            }
        }
        _ => {
            list = list.push(
                text(if online { "Nobody is online right now." } else { "Start the server to manage players." })
                    .size(13)
                    .color(theme::muted()),
            )
        }
    }

    let mut col = column![
        section("Players", "Who's on, and who may join. Every action is a console command, so it shows up in the console too."),
        container(
            column![
                text("Any player, online or not").size(14).font(theme::bold()).color(theme::text()),
                row![
                    text_input("Minecraft username", &app.host.player_input)
                        .on_input(|s| host(Msg::PlayerInput(s)))
                        .padding(9)
                        .style(theme::input)
                        .width(Fill),
                    action("Whitelist", "whitelist add"),
                    action("Operator", "op"),
                    action("Pardon", "pardon"),
                ]
                .spacing(8)
                .align_y(iced::Center),
            ]
            .spacing(10),
        )
        .padding(16)
        .style(theme::well),
        text("Online now").size(14).font(theme::bold()).color(theme::text()),
        scrollable(list).height(Fill),
    ]
    .spacing(14);
    if let Some(reply) = &app.host.reply {
        col = col.push(text(reply.as_str()).size(12).color(theme::muted()));
    }
    col.into()
}

// ----------------------------------------------------------------- settings

fn settings<'a>(app: &'a App, server: &'a HostedServer) -> Element<'a, Message> {
    let Some(s) = &app.host.settings else {
        return text("Reading server.properties…").size(13).color(theme::muted()).into();
    };

    let flag = |label: &'static str, key: &'static str| {
        row![
            toggler(s.flag(key))
                .size(18)
                .style(theme::toggle)
                .on_toggle(move |on| host(Msg::SetProp(key, on.to_string()))),
            text(label).size(14).color(theme::text()),
        ]
        .spacing(10)
        .align_y(iced::Center)
    };
    let number = |label: &'static str, key: &'static str, hint: &'static str| {
        column![
            text(label).size(13).color(theme::text()),
            text_input(hint, s.get(key))
                .on_input(move |v| host(Msg::SetProp(key, v.chars().filter(char::is_ascii_digit).take(4).collect())))
                .padding(9)
                .style(theme::input),
        ]
        .spacing(6)
    };
    let pick = |label: &'static str, key: &'static str, options: &'static [&'static str]| {
        column![
            text(label).size(13).color(theme::text()),
            dropdown(options, hosting::label_of(options, s.get(key)), move |v: &'static str| {
                host(Msg::SetProp(key, v.to_lowercase()))
            })
            .padding(9)
            .width(Fill),
        ]
        .spacing(6)
    };

    let gameplay = container(
        column![
            text("Gameplay").size(16).font(theme::bold()).color(theme::text()),
            pick("Game mode", "gamemode", &GAME_MODES),
            flag("Force game mode on join", "force-gamemode"),
            pick("Difficulty", "difficulty", &DIFFICULTIES),
            flag("PvP", "pvp"),
            flag("Allow flight (stops fly-kicks with some mods)", "allow-flight"),
            flag("Whitelist — only listed players may join", "white-list"),
            column![
                text("Server message (MOTD)").size(13).color(theme::text()),
                text_input("Shown in the server list", s.get("motd"))
                    .on_input(|v| host(Msg::SetProp("motd", v)))
                    .padding(9)
                    .style(theme::input),
            ]
            .spacing(6),
        ]
        .spacing(14),
    )
    .padding(18)
    .width(Fill)
    .style(theme::well);

    let performance = container(
        column![
            text("Performance").size(16).font(theme::bold()).color(theme::text()),
            row![number("Max players", "max-players", "20"), number("Spawn protection", "spawn-protection", "16")].spacing(12),
            row![number("View distance", "view-distance", "10"), number("Simulation distance", "simulation-distance", "10")]
                .spacing(12),
            text("Memory").size(13).color(theme::text()),
            ram_picker(app, server.kind, s.memory_mb, |mb| host(Msg::SettingsMemory(mb))),
        ]
        .spacing(14),
    )
    .padding(18)
    .width(Fill)
    .style(theme::well);

    let online = app.host.phase(&server.id) == Phase::Online;
    let mut bar = row![
        button(text("Open server.properties").size(13))
            .padding([8, 12])
            .style(theme::ghost_button)
            .on_press_maybe(app.core.as_ref().map(|c| Message::OpenPath(c.hosting.server_dir(&server.id).join("server.properties")))),
        button(text("Copy texture pack to local").size(13))
            .padding([8, 12])
            .style(theme::ghost_button)
            // Reads server.properties from disk, so unsaved edits would be ignored.
            .on_press_maybe((!s.dirty).then_some(host(Msg::SyncPack))),
        button(text("Save changes").size(14))
            .padding([8, 16])
            .style(theme::primary_button)
            .on_press_maybe(s.dirty.then_some(host(Msg::SaveSettings))),
    ]
    .spacing(10)
    .align_y(iced::Center);
    if online && !s.dirty {
        bar = bar.push(
            button(text("Restart to apply").size(13))
                .padding([8, 12])
                .style(theme::ghost_button)
                .on_press(host(Msg::Restart(server.id.clone()))),
        );
    }

    column![
        section("Server settings", "Saved to server.properties. Changes apply the next time the server starts."),
        bar,
        scrollable(row![gameplay, performance].spacing(14)).height(Fill),
    ]
    .spacing(14)
    .into()
}

// -------------------------------------------------------------------- files

fn files<'a>(app: &'a App, server: &'a HostedServer) -> Element<'a, Message> {
    let dir = app.core.as_ref().map(|c| c.hosting.server_dir(&server.id));
    let running = app.host.phase(&server.id) != Phase::Offline;
    let armed = app.host.delete_armed.as_deref() == Some(server.id.as_str());

    let tunnel = container(
        row![
            column![
                text("Public address").size(15).font(theme::bold()).color(theme::text()),
                text("Through Nexo's relay, so friends join without port forwarding. The address changes each time it opens, and closes when Nexo does.")
                    .size(12)
                    .color(theme::muted()),
            ]
            .spacing(4)
            .width(Fill),
            toggler(server.tunnel)
                .size(20)
                .style(theme::toggle)
                .on_toggle(|on| host(Msg::SetTunnel(server.id.clone(), on))),
        ]
        .spacing(14)
        .align_y(iced::Center),
    )
    .padding(16)
    .style(theme::well);

    let mut addresses = column![address_row("This PC", format!("localhost:{}", server.port))].spacing(8);
    if let Some(domain) = app.host.live.get(&server.id).and_then(|l| l.domain.clone()) {
        addresses = addresses.push(address_row("Public", domain));
    }

    column![
        section("Files & sharing", "Where the server lives, and how people reach it."),
        container(
            row![
                text(dir.as_ref().map(|d| d.display().to_string()).unwrap_or_default())
                    .size(13)
                    .font(Font::MONOSPACE)
                    .color(theme::text())
                    .width(Fill),
                button(text("Open folder").size(13))
                    .padding([8, 14])
                    .style(theme::ghost_button)
                    .on_press_maybe(dir.map(Message::OpenPath)),
            ]
            .spacing(12)
            .align_y(iced::Center),
        )
        .padding(16)
        .style(theme::well),
        tunnel,
        addresses,
        Space::new().height(Fill),
        container(
            row![
                column![
                    text("Delete server").size(15).font(theme::bold()).color(theme::danger()),
                    text(if running { "Stop it first." } else { "Removes the folder, world included. There is no undo." })
                        .size(12)
                        .color(theme::muted()),
                ]
                .spacing(4)
                .width(Fill),
                button(text(if armed { "Click again to delete" } else { "Delete" }).size(13))
                    .padding([8, 14])
                    .style(theme::danger_button)
                    .on_press_maybe((!running).then(|| host(Msg::Delete(server.id.clone())))),
            ]
            .align_y(iced::Center),
        )
        .padding(16)
        .style(theme::well),
    ]
    .spacing(14)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas::Program;

    #[test]
    fn a_new_sample_draws_on_and_settles_even_with_a_full_history() {
        let frame = |at| iced::Event::Window(iced::window::Event::RedrawRequested(at));
        let t0 = Instant::now();
        let graph = |newest| Graph { samples: Vec::new(), newest: Some(newest), range: 60.0, range_label: "1m", memory_mb: 1024 };
        let mut state = GraphMotion::default();
        let run = |g: &Graph, state: &mut GraphMotion, at| {
            <Graph as Program<()>>::update(g, state, &frame(at), Rectangle::default(), mouse::Cursor::Unavailable).is_some()
        };
        assert!(!run(&graph(t0), &mut state, t0), "first sight doesn't animate");
        assert_eq!(state.grow, 1.0);
        let t1 = t0 + std::time::Duration::from_secs(1);
        assert!(run(&graph(t1), &mut state, t1), "same count, newer sample: still animates");
        assert!(state.grow < 1.0);
        let mut at = t1;
        for _ in 0..120 {
            at += std::time::Duration::from_millis(16);
            if !run(&graph(t1), &mut state, at) {
                return;
            }
        }
        panic!("must settle, or it repaints forever");
    }
}
