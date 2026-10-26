//! Neon palette and widget styling.
//!
//! The rules (written down in the umbrella project's `design/STYLE.md`): flat near-black panels separated by
//! 1px lines, and colour spent only on what is on or primary. Primary buttons
//! are filled with the logo's rainbow ramp; everything else stays dark until
//! the cursor reaches it.
//!
//! iced has no stylesheet — every style is a function of `(theme, status)`.
//! That is more verbose than CSS, but it means a widget's appearance is
//! ordinary Rust that the compiler checks, and hover/press/disabled states
//! can't be silently forgotten.

use iced::widget::{button, checkbox, container, svg, text_input, toggler};
use iced::{border, gradient, Background, Border, Color, Font, Shadow, Theme, Vector};

/// Logo violet — the primary accent.
pub const VIOLET: Color = rgb(0x7b, 0x3c, 0xff);
/// Logo magenta, the warm pole of the brand gradient.
pub const MAGENTA: Color = rgb(0xff, 0x3c, 0xac);
/// Logo mint, the cool pole — used for success/ready states.
pub const MINT: Color = rgb(0x3c, 0xff, 0xb0);
/// The page.
pub const INK: Color = rgb(0x0a, 0x0a, 0x0d);
/// The nav rail — one step up from [`INK`].
pub const SURFACE: Color = rgb(0x0d, 0x0d, 0x11);
/// Cards and list rows.
pub const RAISED: Color = rgb(0x11, 0x11, 0x16);
/// Inputs and unlit buttons.
pub const FIELD: Color = rgb(0x10, 0x10, 0x15);
/// Lines between panels.
pub const LINE: Color = rgb(0x1d, 0x1d, 0x25);
/// The outline of an unlit control — a step brighter than [`LINE`].
pub const CONTROL: Color = rgb(0x23, 0x23, 0x2c);
pub const TEXT: Color = rgb(0xf2, 0xf2, 0xf5);
/// Secondary text; still clears 4.5:1 on [`RAISED`].
pub const MUTED: Color = rgb(0x8b, 0x8b, 0x99);
pub const DANGER: Color = rgb(0xff, 0x54, 0x70);

/// Headings and button labels. Weight, not size, carries the hierarchy.
pub const BOLD: Font = Font {
    weight: iced::font::Weight::Bold,
    ..Font::DEFAULT
};

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

/// Gradient angles. iced measures radians clockwise from straight up, which
/// is easy to get backwards — these are named so a style never has to.
pub mod angle {
    /// Top to bottom.
    pub const DOWN: f32 = std::f32::consts::PI;
    /// Top-left to bottom-right, the way the logo's ramp runs.
    pub const DIAGONAL: f32 = std::f32::consts::FRAC_PI_4 * 3.0;
}

/// Lays `top` over `base` at `amount` opacity.
///
/// Buttons need an *opaque* result: they sit on the page in some places and
/// on a raised card in others, and a translucent tint would come out a
/// different colour in each. Blending against the app's own black keeps every
/// button the same black wherever it lands.
fn blend(base: Color, top: Color, amount: f32) -> Color {
    Color {
        r: base.r + (top.r - base.r) * amount,
        g: base.g + (top.g - base.g) * amount,
        b: base.b + (top.b - base.b) * amount,
        a: 1.0,
    }
}

/// A halo around a control. No offset: a glow that falls to one side reads as
/// an object lit from elsewhere rather than as one that is lit.
fn halo(color: Color, blur_radius: f32) -> Shadow {
    Shadow {
        color,
        offset: Vector::new(0.0, 0.0),
        blur_radius,
    }
}

/// The logo's own ramp — pink, violet, blue, mint — at the logo's own
/// colours. Fixed, not derived from the spectrum: the spectrum's evenly lit
/// hues turned a small button into broad flat bands, where the logo's deeper
/// violet and blue in the middle keep it reading as one gradient.
pub fn rainbow(_theme: &Theme) -> Background {
    Background::from(
        gradient::Linear::new(angle::DIAGONAL)
            .add_stop(0.0, MAGENTA)
            // Lighter than the logo's #7b3cff: dark button labels cross this
            // band, and #7b3cff only gives them ~3.7:1.
            .add_stop(0.35, rgb(0x9a, 0x6b, 0xff))
            .add_stop(0.7, rgb(0x22, 0xb8, 0xff))
            .add_stop(1.0, MINT),
    )
}

/// A rainbow-filled button: the one thing on a screen that should be pressed.
/// Dark label on the bright fill. `glow` scales the halo, so Play can outshine
/// an ordinary primary beside it.
fn filled(theme: &Theme, status: button::Status, glow: f32) -> button::Style {
    let blur = match status {
        button::Status::Active => 16.0,
        button::Status::Hovered => 26.0,
        // Tighter, not brighter: pressed should look closer to the page.
        button::Status::Pressed => 8.0,
        button::Status::Disabled => {
            return button::Style {
                background: Some(Background::Color(FIELD)),
                text_color: alpha(MUTED, 0.6),
                border: Border {
                    radius: 8.0.into(),
                    width: 1.0,
                    color: CONTROL,
                },
                ..Default::default()
            };
        }
    };

    button::Style {
        background: Some(rainbow(theme)),
        text_color: INK,
        border: Border {
            radius: 8.0.into(),
            ..Default::default()
        },
        shadow: halo(alpha(accent(theme), 0.5), blur * glow),
        ..Default::default()
    }
}

/// An unlit button: dark field, 1px line, and only the outline reacts to the
/// cursor. `tone` colours the label and the hovered outline — the accent for
/// ordinary actions, red for destructive ones.
fn outline(tone: Option<Color>, theme: &Theme, status: button::Status) -> button::Style {
    let edge = tone.unwrap_or(accent(theme));
    let (background, line) = match status {
        button::Status::Active => (FIELD, CONTROL),
        button::Status::Hovered => (FIELD, alpha(edge, 0.7)),
        button::Status::Pressed => (blend(FIELD, edge, 0.12), edge),
        button::Status::Disabled => (FIELD, alpha(CONTROL, 0.6)),
    };

    button::Style {
        background: Some(Background::Color(background)),
        text_color: match status {
            button::Status::Disabled => alpha(MUTED, 0.6),
            _ => tone.unwrap_or(TEXT),
        },
        border: Border {
            radius: 8.0.into(),
            width: 1.0,
            color: line,
        },
        ..Default::default()
    }
}

/// Where the animation clock wraps, in seconds — a whole multiple of the
/// live-log pulse's 2.4s ring, so the pulse passes the wrap without a jump.
/// (Named for the accent's old 12s trip round the colour wheel; the accent no
/// longer moves.)
pub const RAINBOW_PERIOD: f32 = 12.0;

/// The one accent: glows, outlines and the selected state. A violet-pink
/// between the logo's magenta and violet, so the glow matches the rainbow
/// fill it sits around.
///
/// It used to drift round the whole colour wheel. That made the rail and
/// every lit control turn green, then yellow, in turn — big blocks of colour
/// the mockup never had — so it holds still now.
pub const ACCENT: Color = rgb(0xb1, 0x4c, 0xff);

/// The app-wide theme. Built from a custom palette so iced's generated
/// component colors (hover shades, contrast text) derive from the brand
/// rather than being hand-maintained per widget. Every style below reads the
/// accent out of `primary`, so this is the one place it is chosen.
pub fn nexo() -> Theme {
    Theme::custom(
        "Nexo".to_string(),
        iced::theme::Palette {
            background: INK,
            text: TEXT,
            primary: ACCENT,
            success: MINT,
            warning: rgb(0xff, 0xd9, 0x3c),
            danger: DANGER,
        },
    )
}

/// The accent ([`ACCENT`]), read out of the theme each style is handed.
fn accent(theme: &Theme) -> Color {
    theme.palette().primary
}

/// The main call-to-action: rainbow-filled and glowing at rest. One per
/// screen — a second one leaves the screen with no emphasis left to give.
pub fn primary_button(theme: &Theme, status: button::Status) -> button::Style {
    filled(theme, status, 1.0)
}

/// Play — the one control that must be found without looking. The same fill
/// as a primary, with more light, so it wins the screen even beside one.
pub fn hero_button(theme: &Theme, status: button::Status) -> button::Style {
    filled(theme, status, 1.4)
}

/// Quieter actions that sit next to a primary one, like "Open folder" and
/// "Browse…". Unlit until the cursor arrives.
pub fn ghost_button(theme: &Theme, status: button::Status) -> button::Style {
    outline(None, theme, status)
}

/// Stop, shown in place of Play while a game is running. Red outline with a
/// faint red glow at rest: it holds the primary slot, so it needs weight, but
/// a filled red button would read as an alarm.
pub fn stop_button(theme: &Theme, status: button::Status) -> button::Style {
    let mut style = outline(Some(DANGER), theme, status);
    let glow = match status {
        button::Status::Active => 12.0,
        button::Status::Hovered => 22.0,
        button::Status::Pressed => 8.0,
        button::Status::Disabled => return style,
    };
    style.border.color = DANGER;
    style.shadow = halo(alpha(DANGER, 0.35), glow);
    style
}

/// Destructive actions. Red label, dark until touched: a Delete that glows on
/// its own draws the eye to exactly the button nobody should press by
/// accident.
pub fn danger_button(theme: &Theme, status: button::Status) -> button::Style {
    outline(Some(DANGER), theme, status)
}

/// A square icon entry in the nav rail. The selected one is tinted, outlined
/// and glowing; the rest are bare until hovered, which is what makes "where
/// you are" readable at a glance. The icon itself is tinted by [`nav_icon`].
pub fn nav_button(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let accent = accent(theme);

        if selected {
            return button::Style {
                background: Some(Background::Color(blend(SURFACE, accent, 0.16))),
                text_color: accent,
                border: Border {
                    radius: 8.0.into(),
                    width: 1.0,
                    color: accent,
                },
                shadow: halo(alpha(accent, 0.45), 16.0),
                ..Default::default()
            };
        }

        button::Style {
            background: Some(Background::Color(if hovered { FIELD } else { Color::TRANSPARENT })),
            text_color: if hovered { TEXT } else { MUTED },
            border: Border {
                radius: 8.0.into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }
}

/// Tints a rail icon to match its [`nav_button`]: accent when selected, text
/// when hovered, muted otherwise.
pub fn nav_icon(selected: bool) -> impl Fn(&Theme, svg::Status) -> svg::Style {
    move |theme, status| svg::Style {
        color: Some(match (selected, status) {
            (true, _) => accent(theme),
            (false, svg::Status::Hovered) => TEXT,
            (false, svg::Status::Idle) => MUTED,
        }),
    }
}

/// An icon in the label colour of the [`filled`] button it sits on: dark on
/// the rainbow, muted on the dark field a disabled one falls back to.
pub fn icon_on_fill(enabled: bool) -> impl Fn(&Theme, svg::Status) -> svg::Style {
    move |_theme, _status| svg::Style {
        color: Some(if enabled { INK } else { alpha(MUTED, 0.6) }),
    }
}

/// Chromeless button, for making a block of content clickable without it
/// reading as a control.
/// `status` is deliberately ignored: the hover affordance belongs to the card
/// this sits inside, and tinting here would double up on it.
pub fn bare_button(_theme: &Theme, _status: button::Status) -> button::Style {
    button::Style {
        background: Some(Background::Color(Color::TRANSPARENT)),
        text_color: TEXT,
        border: Border::default(),
        shadow: Shadow::default(),
        ..Default::default()
    }
}

/// A selectable tile, used for the cape grid and the edition picker.
pub fn tile(theme: &Theme, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: Some(Background::Color(if hovered {
            blend(INK, accent(theme), 0.12)
        } else {
            SURFACE
        })),
        text_color: TEXT,
        border: Border {
            radius: 10.0.into(),
            width: 1.0,
            color: alpha(if hovered { accent(theme) } else { MUTED }, 0.35),
        },
        shadow: Shadow::default(),
        ..Default::default()
    }
}

/// The tile that's currently chosen.
///
/// Stays mint rather than taking the brand ramp: everywhere else in the app
/// mint means *this is the one that is ready/chosen*, and a violet tile would
/// read as merely hovered next to the violet washes on either side of it.
pub fn selected_tile(_theme: &Theme, _status: button::Status) -> button::Style {
    button::Style {
        background: Some(Background::Color(blend(INK, MINT, 0.1))),
        text_color: TEXT,
        border: Border {
            radius: 10.0.into(),
            width: 1.0,
            color: MINT,
        },
        shadow: Shadow {
            color: alpha(MINT, 0.25),
            offset: Vector::new(0.0, 1.0),
            blur_radius: 14.0,
        },
        ..Default::default()
    }
}

/// One tab on the instance screen.
///
/// Flat rather than a tile: the rail already uses lit tiles for "where you
/// are", and reusing that shape one level down would make the two compete.
/// The selected tab is white over a faint wash rising from its underline —
/// the mockup's glow is a text-shadow, which iced cannot draw, and this is
/// the nearest light that stays behind the label. See [`tab_underline`].
pub fn tab_button(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);

        button::Style {
            background: Some(if selected {
                Background::from(
                    gradient::Linear::new(angle::DOWN)
                        .add_stop(0.0, Color::TRANSPARENT)
                        .add_stop(1.0, alpha(accent(theme), 0.12)),
                )
            } else {
                Background::Color(Color::TRANSPARENT)
            }),
            // Hover only brightens the label: an unselected tab that lit up
            // under the cursor would read as a second selection.
            text_color: if selected || hovered { TEXT } else { MUTED },
            border: Border {
                // Square at the bottom so the button sits flush on its
                // underline instead of floating above a detached bar.
                radius: border::Radius::default().top(6.0),
                ..Default::default()
            },
            shadow: Shadow::default(),
            ..Default::default()
        }
    }
}

/// The bar under a tab: the logo ramp with a glow beneath it for the selected
/// one, nothing for the rest — the 1px [`divider`] under the whole strip is
/// the rule they sit on.
pub fn tab_underline(selected: bool) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        if !selected {
            return container::Style::default();
        }

        container::Style {
            background: Some(rainbow(theme)),
            shadow: Shadow {
                color: alpha(accent(theme), 0.6),
                offset: Vector::new(0.0, 1.0),
                blur_radius: 10.0,
            },
            ..Default::default()
        }
    }
}

/// The count beside a tab's label. Small, quiet, and only ever shown for a
/// number that has actually been counted — see `App::tabs_loaded`.
pub fn tab_badge(selected: bool) -> impl Fn(&Theme) -> container::Style {
    move |theme| container::Style {
        background: Some(Background::Color(blend(INK, accent(theme), 0.14))),
        text_color: Some(if selected {
            lighten(accent(theme), 0.28)
        } else {
            MUTED
        }),
        border: Border {
            radius: 100.0.into(),
            width: 1.0,
            color: alpha(if selected { accent(theme) } else { MUTED }, 0.35),
        },
        ..Default::default()
    }
}

/// A row that reacts to the cursor, for lists whose entries are clickable as
/// a whole — files, worlds, logs.
pub fn row_button(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let wash = match (selected, hovered) {
            (true, true) => 0.2,
            (true, false) => 0.15,
            (false, true) => 0.08,
            (false, false) => 0.0,
        };

        button::Style {
            background: Some(Background::Color(if wash > 0.0 {
                blend(INK, accent(theme), wash)
            } else {
                Color::TRANSPARENT
            })),
            text_color: if selected { TEXT } else { MUTED },
            border: Border {
                radius: 8.0.into(),
                width: if selected { 1.0 } else { 0.0 },
                color: alpha(accent(theme), 0.55),
            },
            // A list row is not a control, so it gets an outline and no halo.
            // Rows glowing down a scrolling list would be a light show.
            shadow: Shadow::default(),
            ..Default::default()
        }
    }
}

/// The ground under a log or any other block of preformatted text. Darker
/// than a card on purpose — it is a viewport onto a file, not a panel.
pub fn well(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(INK)),
        border: Border {
            radius: 10.0.into(),
            width: 1.0,
            color: alpha(MUTED, 0.15),
        },
        ..Default::default()
    }
}

/// A content card — instance tiles, account rows. A plain line, no accent:
/// cards are where content sits, and tinting every one of them is how a
/// screen ends up glowing all over.
pub fn card(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(RAISED)),
        border: Border {
            radius: 10.0.into(),
            width: 1.0,
            color: LINE,
        },
        ..Default::default()
    }
}

/// A side panel beside a list — the Modrinth browser's filters. One step
/// darker than [`card`], so the panel reads as furniture next to the results
/// rather than as one more result.
pub fn panel(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(SURFACE)),
        ..card(theme)
    }
}

/// A filter checkbox. Checked is the "on" state STYLE.md allows neon for:
/// rainbow fill inside an accent edge. A disabled-but-checked box is a filter
/// the instance imposes (its Minecraft version, its loader) — shown so the
/// list explains itself, dimmed because it cannot be switched off.
pub fn check(theme: &Theme, status: checkbox::Status) -> checkbox::Style {
    let (checked, hovered, enabled) = match status {
        checkbox::Status::Active { is_checked } => (is_checked, false, true),
        checkbox::Status::Hovered { is_checked } => (is_checked, true, true),
        checkbox::Status::Disabled { is_checked } => (is_checked, false, false),
    };
    let dim = if enabled { 1.0 } else { 0.55 };

    checkbox::Style {
        background: if checked {
            rainbow(theme).scale_alpha(dim)
        } else {
            Background::Color(Color::TRANSPARENT)
        },
        icon_color: INK,
        border: Border {
            radius: 4.0.into(),
            width: 1.0,
            color: match (checked, hovered) {
                (true, _) => alpha(accent(theme), dim),
                (false, true) => alpha(accent(theme), 0.7),
                (false, false) => rgb(0x2a, 0x2a, 0x34),
            },
        },
        text_color: Some(if checked && enabled { TEXT } else { MUTED }),
    }
}

/// The square holding an instance's initial at the left of its card. Rainbow
/// while that instance is running, so a running game is visible from across
/// the list; a flat block otherwise.
pub fn initial_tile(running: bool) -> impl Fn(&Theme) -> container::Style {
    move |theme| container::Style {
        background: Some(if running {
            rainbow(theme)
        } else {
            Background::Color(rgb(0x2a, 0x2a, 0x34))
        }),
        text_color: Some(if running { INK } else { TEXT }),
        border: Border {
            radius: 8.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// The nav rail's ground.
pub fn sidebar(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(SURFACE)),
        ..Default::default()
    }
}

/// The 1px line down the rail's edge. A container rather than a border on the
/// rail, because iced borders go all the way round.
pub fn divider(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(LINE)),
        ..Default::default()
    }
}

/// The label a rail icon shows on hover, since the rail has no text of its own.
pub fn tooltip(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(RAISED)),
        text_color: Some(TEXT),
        border: Border {
            radius: 6.0.into(),
            width: 1.0,
            color: CONTROL,
        },
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
pub fn highlight(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(alpha(accent(theme), 0.12))),
        border: Border {
            radius: 12.0.into(),
            width: 1.0,
            color: alpha(accent(theme), 0.5),
        },
        ..Default::default()
    }
}

pub fn input(theme: &Theme, status: text_input::Status) -> text_input::Style {
    let border_color = match status {
        text_input::Status::Focused { .. } => accent(theme),
        text_input::Status::Hovered => alpha(accent(theme), 0.5),
        _ => CONTROL,
    };

    text_input::Style {
        background: Background::Color(FIELD),
        border: Border {
            radius: 8.0.into(),
            width: 1.0,
            color: border_color,
        },
        icon: MUTED,
        placeholder: MUTED,
        value: TEXT,
        selection: alpha(accent(theme), 0.4),
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


/// An on/off switch whose knob keeps one colour in every state. iced's default
/// recolours the whole switch when it is disabled, and the launcher disables
/// these while it is busy — so the knob flashed dark and white as that came
/// and went. A disabled switch is now the same switch, only dimmer.
pub fn toggle(theme: &Theme, status: toggler::Status) -> toggler::Style {
    let (on, dim) = match status {
        toggler::Status::Active { is_toggled } | toggler::Status::Hovered { is_toggled } => {
            (is_toggled, 1.0)
        }
        toggler::Status::Disabled { is_toggled } => (is_toggled, 0.6),
    };
    // On: a tinted track inside a lit outline, white knob. Off: a dark track
    // with a dim knob, so on and off differ in lightness, not only in hue.
    let (track, edge, knob) = if on {
        (blend(INK, accent(theme), 0.22), accent(theme), TEXT)
    } else {
        (rgb(0x16, 0x16, 0x1c), rgb(0x2a, 0x2a, 0x34), rgb(0x5a, 0x5a, 0x68))
    };
    toggler::Style {
        background: Background::Color(alpha(track, dim)),
        background_border_width: 1.0,
        background_border_color: alpha(edge, dim),
        foreground: Background::Color(alpha(knob, dim)),
        foreground_border_width: 0.0,
        foreground_border_color: Color::TRANSPARENT,
        text_color: None,
        border_radius: None,
        padding_ratio: 0.1,
    }
}
