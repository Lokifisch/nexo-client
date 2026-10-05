//! In-launcher console for a running Paper server: tails `logs/latest.log`
//! and sends RCON commands. The Mod's own `NexoPaperConsoleScreen` does the
//! same thing in-game (see `Mod/ROADMAP.md` Phase 7) — this is the launcher
//! side of that, reusing the same log-tail core
//! (`nexo_core::browse::logs`/`read_log`) the per-instance Logs tab already
//! uses.

use crate::theme;
use crate::{App, Message};
use iced::widget::{Space, button, column, container, row, scrollable, text, text_input};
use iced::{Element, Fill, Font};
use nexo_core::paper_server::PaperServer;

pub fn view<'a>(app: &'a App, server: &'a PaperServer) -> Element<'a, Message> {
    let log: Element<'_, Message> = match &app.paper_console_log {
        Some((body, truncated)) => {
            let mut head = row![Space::new().width(Fill)];
            if *truncated {
                head = head.push(text("showing the end only").size(11).color(theme::MUTED));
            }
            column![
                head,
                container(
                    scrollable(
                        container(text(body.as_str()).size(11).font(Font::MONOSPACE).color(theme::TEXT))
                            .padding(12)
                    )
                    // Same reasoning as the instance Logs tab: a live log
                    // should keep its newest line in view, not push it off
                    // the bottom.
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
        None => centered_note("Reading…"),
    };

    let can_send = !app.paper_console_input.trim().is_empty();
    let input = text_input("Command — e.g. list", &app.paper_console_input)
        .on_input(Message::PaperConsoleInputChanged)
        .on_submit(Message::SendPaperConsoleCommand)
        .padding(10)
        .style(theme::input)
        .width(Fill);
    let send = button(text("Send").size(14))
        .padding([10, 18])
        .style(theme::primary_button)
        .on_press_maybe(can_send.then_some(Message::SendPaperConsoleCommand));

    let mut body = column![
        row![
            text(format!("{} — Console", server.name)).size(24).color(theme::TEXT),
            Space::new().width(Fill),
            button(text("Back").size(13))
                .padding([6, 12])
                .style(theme::ghost_button)
                .on_press(Message::Navigate(crate::Screen::Servers)),
        ]
        .align_y(iced::Center),
        log,
        row![input, send].spacing(8).align_y(iced::Center),
    ]
    .spacing(14)
    .height(Fill);

    if let Some(response) = &app.paper_console_response {
        body = body.push(
            container(text(response.as_str()).size(12).font(Font::MONOSPACE).color(theme::TEXT))
                .padding(10)
                .width(Fill)
                .style(theme::well),
        );
    }

    body.into()
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
