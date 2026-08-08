//! Neon palette and widget styling.
//!
//! Colors are sampled from `nexologo.svg` so the app and the in-game re-skin
//! read as one product: violet `#7b3cff` as the primary, magenta and mint as
//! the gradient poles, on the logo's near-black `#0d0d14` ground.
//!
//! iced has no stylesheet — every style is a function of `(theme, status)`.
//! That is more verbose than CSS, but it means a widget's appearance is
//! ordinary Rust that the compiler checks, and hover/press/disabled states
//! can't be silently forgotten.

use iced::widget::{button, container, text_input};
use iced::{Background, Border, Color, Shadow, Theme, Vector};

/// Logo violet — the primary accent.
pub const VIOLET: Color = rgb(0x7b, 0x3c, 0xff);
/// Logo magenta, the warm pole of the brand gradient.
pub const MAGENTA: Color = rgb(0xff, 0x3c, 0xac);
/// Logo mint, the cool pole — used for success/ready states.
pub const MINT: Color = rgb(0x3c, 0xff, 0xb0);
/// Logo background.
pub const INK: Color = rgb(0x0d, 0x0d, 0x14);
/// One step up from [`INK`], for panels that need to separate from the page.
pub const SURFACE: Color = rgb(0x16, 0x16, 0x22);
/// Two steps up — cards, inputs.
pub const RAISED: Color = rgb(0x1f, 0x1f, 0x2e);
pub const TEXT: Color = rgb(0xe8, 0xe8, 0xf2);
/// Secondary text; still clears 4.5:1 on [`INK`].
pub const MUTED: Color = rgb(0x9a, 0x9a, 0xb4);
pub const DANGER: Color = rgb(0xff, 0x4d, 0x6a);

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: 1.0,
    }
}

/// Same color at a different alpha — the basis of the glow effects below.
pub fn alpha(color: Color, a: f32) -> Color {
    Color { a, ..color }
}

/// The app-wide theme. Built from a custom palette so iced's generated
/// component colors (hover shades, contrast text) derive from the brand
/// rather than being hand-maintained per widget.
pub fn nexo() -> Theme {
    Theme::custom(
        "Nexo".to_string(),
        iced::theme::Palette {
            background: INK,
            text: TEXT,
            primary: VIOLET,
            success: MINT,
            warning: rgb(0xff, 0xd9, 0x3c),
            danger: DANGER,
        },
    )
}

/// The main call-to-action: filled violet with a soft glow that intensifies
/// on hover. The glow is a shadow rather than a border so it reads as light
/// spill instead of an outline.
pub fn primary_button(_theme: &Theme, status: button::Status) -> button::Style {
    let (background, glow) = match status {
        button::Status::Active => (VIOLET, 12.0),
        button::Status::Hovered => (lighten(VIOLET, 0.12), 22.0),
        button::Status::Pressed => (darken(VIOLET, 0.10), 8.0),
        button::Status::Disabled => (alpha(VIOLET, 0.25), 0.0),
    };

    button::Style {
        background: Some(Background::Color(background)),
        text_color: if matches!(status, button::Status::Disabled) {
            alpha(TEXT, 0.5)
        } else {
            Color::WHITE
        },
        border: Border {
            radius: 10.0.into(),
            ..Border::default()
        },
        shadow: Shadow {
            color: alpha(VIOLET, 0.55),
            offset: Vector::new(0.0, 0.0),
            blur_radius: glow,
        },
        ..Default::default()
    }
}

/// Quieter actions that sit next to a primary one.
pub fn ghost_button(_theme: &Theme, status: button::Status) -> button::Style {
    let (background, border_color) = match status {
        button::Status::Active => (Color::TRANSPARENT, alpha(VIOLET, 0.45)),
        button::Status::Hovered => (alpha(VIOLET, 0.14), VIOLET),
        button::Status::Pressed => (alpha(VIOLET, 0.22), VIOLET),
        button::Status::Disabled => (Color::TRANSPARENT, alpha(MUTED, 0.3)),
    };

    button::Style {
        background: Some(Background::Color(background)),
        text_color: match status {
            button::Status::Disabled => alpha(TEXT, 0.4),
            _ => TEXT,
        },
        border: Border {
            radius: 10.0.into(),
            width: 1.0,
            color: border_color,
        },
        shadow: Shadow::default(),
        ..Default::default()
    }
}

/// Destructive actions. Red rather than brand-violet so it can't be confused
/// with a primary action.
pub fn danger_button(_theme: &Theme, status: button::Status) -> button::Style {
    let background = match status {
        button::Status::Active => Color::TRANSPARENT,
        button::Status::Hovered => alpha(DANGER, 0.18),
        button::Status::Pressed => alpha(DANGER, 0.28),
        button::Status::Disabled => Color::TRANSPARENT,
    };

    button::Style {
        background: Some(Background::Color(background)),
        text_color: DANGER,
        border: Border {
            radius: 10.0.into(),
            width: 1.0,
            color: alpha(DANGER, 0.5),
        },
        shadow: Shadow::default(),
        ..Default::default()
    }
}

/// Sidebar navigation entries. The selected one gets a filled violet wash so
/// the current screen is obvious without a separate indicator.
pub fn nav_button(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let background = if selected {
            alpha(VIOLET, 0.22)
        } else {
            match status {
                button::Status::Hovered | button::Status::Pressed => alpha(VIOLET, 0.10),
                _ => Color::TRANSPARENT,
            }
        };

        button::Style {
            background: Some(Background::Color(background)),
            text_color: if selected { TEXT } else { MUTED },
            border: Border {
                radius: 8.0.into(),
                width: if selected { 1.0 } else { 0.0 },
                color: alpha(VIOLET, 0.5),
            },
            shadow: Shadow::default(),
            ..Default::default()
        }
    }
}

/// A content card — instance tiles, account rows.
pub fn card(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(RAISED)),
        border: Border {
            radius: 12.0.into(),
            width: 1.0,
            color: alpha(VIOLET, 0.18),
        },
        ..Default::default()
    }
}

/// The sidebar's ground.
pub fn sidebar(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(SURFACE)),
        ..Default::default()
    }
}

/// Inline status/error strip.
pub fn banner(is_error: bool) -> impl Fn(&Theme) -> container::Style {
    move |_theme| {
        let accent = if is_error { DANGER } else { MINT };
        container::Style {
            background: Some(Background::Color(alpha(accent, 0.12))),
            border: Border {
                radius: 10.0.into(),
                width: 1.0,
                color: alpha(accent, 0.45),
            },
            text_color: Some(accent),
            ..Default::default()
        }
    }
}

/// The device-code callout, which needs to draw the eye more than a card.
pub fn highlight(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(alpha(VIOLET, 0.12))),
        border: Border {
            radius: 12.0.into(),
            width: 1.0,
            color: alpha(VIOLET, 0.5),
        },
        ..Default::default()
    }
}

pub fn input(_theme: &Theme, status: text_input::Status) -> text_input::Style {
    let border_color = match status {
        text_input::Status::Focused { .. } => VIOLET,
        text_input::Status::Hovered => alpha(VIOLET, 0.5),
        _ => alpha(MUTED, 0.3),
    };

    text_input::Style {
        background: Background::Color(SURFACE),
        border: Border {
            radius: 10.0.into(),
            width: 1.0,
            color: border_color,
        },
        icon: MUTED,
        placeholder: MUTED,
        value: TEXT,
        selection: alpha(VIOLET, 0.4),
    }
}

fn lighten(color: Color, amount: f32) -> Color {
    Color {
        r: (color.r + amount).min(1.0),
        g: (color.g + amount).min(1.0),
        b: (color.b + amount).min(1.0),
        a: color.a,
    }
}

fn darken(color: Color, amount: f32) -> Color {
    Color {
        r: (color.r - amount).max(0.0),
        g: (color.g - amount).max(0.0),
        b: (color.b - amount).max(0.0),
        a: color.a,
    }
}
