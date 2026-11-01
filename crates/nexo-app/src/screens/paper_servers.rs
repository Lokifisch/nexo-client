//! The card for one converted-world Paper server — see `Mod/ROADMAP.md`
//! Phase 7. Shown under "Converted worlds" on the Servers screen
//! (`hosting::list_view`). Conversion itself starts from a world row on the
//! instance details screen's Worlds tab, where the source world's path is
//! already at hand; a card shows a server regardless of which instance or
//! product created it.
//!
//! Deliberately does not have a live RCON console or a Hangar plugin-browser
//! tab yet — `nexo_core::paper_server::hangar` and `rcon::RconClient` are
//! both implemented and ready, but the UI for them is scoped out of this
//! pass rather than rushed. The Mod's own settings screen already has a
//! working plugin browser (`NexoPaperPluginBrowserScreen`); a server started
//! from here can still be managed from there in the meantime.

use crate::theme;
use crate::{App, Message};
use crate::lively::button;
use iced::widget::{column, container, row, text};
use iced::{Element, Fill};
use nexo_core::paper_server::{status, PaperServer};

pub fn card<'a>(app: &'a App, server: &'a PaperServer) -> Element<'a, Message> {
    let (status_label, status_color) = if server.status == status::RUNNING {
        (format!("Running on localhost:{}", server.port), theme::mint())
    } else if server.status == status::ERROR {
        ("Error — check the log".to_string(), theme::danger())
    } else {
        (status_label_for(&server.status), theme::muted())
    };

    let mut details = column![
        text(&server.name).size(17).color(theme::text()),
        text(status_label).size(12).color(status_color),
        text(format!(
            "Minecraft {}{}",
            server.minecraft_version,
            server
                .paper_build
                .map(|b| format!(" · Paper build {b}"))
                .unwrap_or_default()
        ))
        .size(11)
        .color(theme::muted()),
    ]
    .spacing(3)
    .width(Fill);

    // Read-only here on purpose: the tunnel itself is Mod-owned (it needs
    // the QUIC relay stack `lantunnel/` already has, which this launcher
    // doesn't carry a second copy of) — see Mod/ROADMAP.md Phase 7. This
    // just reflects whatever the Mod's own toggle last wrote.
    if server.friend_hosting.tunnel_active
        && let Some(domain) = &server.friend_hosting.domain
    {
        details = details.push(text(format!("Shared with friends: {domain}")).size(11).color(theme::mint()));
    }

    let action: Element<'_, Message> = if server.status == status::RUNNING {
        row![
            button(text("Console").size(14))
                .padding([7, 14])
                .style(theme::ghost_button)
                .on_press(Message::Navigate(crate::Screen::PaperConsole(server.id.clone()))),
            button(text("Stop & revert").size(14))
                .padding([7, 18])
                .style(theme::stop_button)
                .on_press_maybe((!app.is_busy()).then(|| Message::RevertPaperServer(server.id.clone()))),
        ]
        .spacing(8)
        .into()
    } else {
        text("").into()
    };

    container(row![details, action].spacing(12).align_y(iced::Center))
        .padding(16)
        .width(Fill)
        .style(theme::card)
        .into()
}

/// A transitional status (converting/starting/stopping) reads the same way
/// this screen already talks about idle work elsewhere: present participle,
/// no punctuation.
fn status_label_for(raw_status: &str) -> String {
    match raw_status {
        s if s == status::CONVERTING => "Converting…".to_string(),
        s if s == status::STARTING => "Starting…".to_string(),
        s if s == status::STOPPING => "Stopping…".to_string(),
        s if s == status::STOPPED => "Stopped".to_string(),
        other => other.to_string(),
    }
}
