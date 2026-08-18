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
use iced::{border, gradient, Background, Border, Color, Shadow, Theme, Vector};

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

/// Gradient angles. iced measures radians clockwise from straight up, which
/// is easy to get backwards — these are named so a style never has to.
pub mod angle {
    /// Left to right.
    pub const ACROSS: f32 = std::f32::consts::FRAC_PI_2;
    /// Top to bottom.
    pub const DOWN: f32 = std::f32::consts::PI;
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

/// The neon-sign construction every button in the app is built from: black
/// glass, a lit outline, and the glow it throws onto the page around it.
///
/// The colour is in the *light*, never in the fill. That is what separates a
/// neon sign from a painted one, and it is why the accent arrives as a border,
/// a label and a halo while the surface itself stays near-black — a filled
/// swatch of the same colour reads as plastic no matter how bright it is.
///
/// `quiet` holds the glow back until the cursor arrives. A toolbar of six lit
/// buttons is a toolbar with no emphasis left to give the one that matters, so
/// only the primary action on a screen glows at rest.
fn lit(accent: Color, status: button::Status, quiet: bool) -> button::Style {
    // Neon is a bright line on a dark ground, so the outline and the label
    // take a lightened accent rather than the raw brand colour: #7b3cff on
    // near-black sits around 4:1, which is thin for a control label.
    let bright = lighten(accent, 0.22);

    let (border, glow, wash) = match status {
        button::Status::Active if quiet => (0.4, 0.0, 0.05),
        button::Status::Active => (0.9, 15.0, 0.09),
        button::Status::Hovered => (1.0, 27.0, 0.15),
        // Dimmer and tighter, not brighter: a neon tube pressed against the
        // page should look like it moved closer to it.
        button::Status::Pressed => (1.0, 9.0, 0.22),
        button::Status::Disabled => (0.16, 0.0, 0.0),
    };

    button::Style {
        background: Some(Background::Color(blend(INK, accent, wash))),
        text_color: match status {
            button::Status::Disabled => alpha(MUTED, 0.45),
            _ => bright,
        },
        border: Border {
            radius: 10.0.into(),
            width: if quiet { 1.0 } else { 1.5 },
            color: alpha(bright, border),
        },
        shadow: Shadow {
            color: alpha(accent, 0.6),
            // No offset: a halo, not a drop shadow. A glow that falls to one
            // side reads as an object lit from elsewhere rather than as one
            // that is lit.
            offset: Vector::new(0.0, 0.0),
            blur_radius: glow,
        },
        ..Default::default()
    }
}

/// How long the accent takes to travel once around the colour wheel.
///
/// Slow on purpose. The glow should look like it is drifting when you happen
/// to notice it, not flashing while you are trying to read a log — and a fast
/// cycle turns every button into a distraction competing with the content.
pub const RAINBOW_PERIOD: f32 = 12.0;

/// A fully saturated neon at `turns` around the colour wheel; wraps, so the
/// caller can hand it a raw elapsed time over [`RAINBOW_PERIOD`].
///
/// Lightness sits at 0.66 rather than full: a pure hue at maximum lightness
/// goes white at yellow and cyan, so a cycling accent would visibly flatten
/// twice per revolution.
pub fn spectrum(turns: f32) -> Color {
    let hue = turns.rem_euclid(1.0) * 6.0;
    let sector = hue.floor();
    let f = hue - sector;

    // HSL at S = 1, L = 0.66, expanded by hand: the general conversion is
    // mostly branches that fall away once saturation is fixed.
    const L: f32 = 0.66;
    const C: f32 = (1.0 - 2.0 * L + 1.0) * 1.0; // chroma at S=1 → 2(1-L)
    let (min, max) = (L - C / 2.0, L + C / 2.0);
    let rise = min + (max - min) * f;
    let fall = max - (max - min) * f;

    let (r, g, b) = match sector as u32 % 6 {
        0 => (max, rise, min),
        1 => (fall, max, min),
        2 => (min, max, rise),
        3 => (min, fall, max),
        4 => (rise, min, max),
        _ => (max, min, fall),
    };
    Color { r, g, b, a: 1.0 }
}

/// Where a colour sits on the wheel, in turns.
///
/// Lets a style build a *ramp* around the accent it was handed — the tab bar
/// needs three hues spaced across the spectrum, and the only thing it receives
/// is the one colour in the palette. Deriving the neighbours instead of
/// threading the animation clock through every style function is what keeps
/// [`nexo`] the single place the animation exists.
fn hue_of(color: Color) -> f32 {
    let max = color.r.max(color.g).max(color.b);
    let min = color.r.min(color.g).min(color.b);
    let span = max - min;
    if span <= f32::EPSILON {
        return 0.0;
    }

    let hue = if max == color.r {
        (color.g - color.b) / span
    } else if max == color.g {
        2.0 + (color.b - color.r) / span
    } else {
        4.0 + (color.r - color.g) / span
    };
    (hue / 6.0).rem_euclid(1.0)
}

/// The app-wide theme. Built from a custom palette so iced's generated
/// component colors (hover shades, contrast text) derive from the brand
/// rather than being hand-maintained per widget.
///
/// `clock` is elapsed seconds, and it is the *whole* animation: the accent is
/// carried in `primary`, so every style below reads its colour out of the
/// theme it is already handed and no widget, screen, or call site knows that
/// the accent moves. Rebuilt each frame, which costs one palette generation —
/// a handful of colour conversions, against a frame that was going to be
/// drawn anyway.
///
/// `success` and `danger` deliberately do not move. Mint means *ready* and red
/// means *this destroys something*; a Delete button that drifts through green
/// is a Delete button that has stopped warning anyone.
pub fn nexo(clock: f32) -> Theme {
    Theme::custom(
        "Nexo".to_string(),
        iced::theme::Palette {
            background: INK,
            text: TEXT,
            primary: spectrum(clock / RAINBOW_PERIOD),
            success: MINT,
            warning: rgb(0xff, 0xd9, 0x3c),
            danger: DANGER,
        },
    )
}

/// The accent as it stands this frame. Every style takes its colour from here
/// rather than from [`VIOLET`], which is now only the resting brand colour for
/// anything that must not move.
fn accent(theme: &Theme) -> Color {
    theme.palette().primary
}

/// The main call-to-action: violet neon, lit at rest.
pub fn primary_button(theme: &Theme, status: button::Status) -> button::Style {
    lit(accent(theme), status, false)
}

/// Play — the one control that must be found without looking.
///
/// Mint rather than violet, and the only mint button in the app. It is the
/// palette's "ready" signal, so spending it here means *go* is the one word
/// the colour ever says; a second mint button anywhere would cost that.
pub fn hero_button(_theme: &Theme, status: button::Status) -> button::Style {
    let mut style = lit(MINT, status, false);

    // A touch more light than the others carry, so it wins the screen even
    // sitting beside a lit primary.
    style.shadow.blur_radius *= 1.35;
    style
}

/// Quieter actions that sit next to a primary one. Unlit until the cursor
/// arrives — see [`lit`] on why they have to be.
pub fn ghost_button(theme: &Theme, status: button::Status) -> button::Style {
    let mut style = lit(accent(theme), status, true);

    // Plain text rather than violet: these are labels like "Open folder" and
    // "Browse…", and a row of them in accent colour would look like a row of
    // primary actions.
    style.text_color = match status {
        button::Status::Disabled => alpha(MUTED, 0.45),
        _ => TEXT,
    };
    style
}

/// Stop, shown in place of Play while a game is running. Lit at rest, since it
/// occupies the primary action's slot and needs the same weight while clearly
/// not being "go".
pub fn stop_button(_theme: &Theme, status: button::Status) -> button::Style {
    lit(DANGER, status, false)
}

/// Destructive actions. Red, and dark until touched: a Delete that glows on
/// its own draws the eye to exactly the button nobody should press by
/// accident.
pub fn danger_button(_theme: &Theme, status: button::Status) -> button::Style {
    let mut style = lit(DANGER, status, true);
    style.text_color = match status {
        button::Status::Disabled => alpha(MUTED, 0.45),
        _ => DANGER,
    };
    style
}

/// Sidebar navigation entries. The selected one is a lit tube; the rest are
/// unlit glass, which is what makes "where you are" readable at a glance
/// without a separate indicator.
pub fn nav_button(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);

        if selected {
            let mut style = lit(accent(theme), status, false);
            style.border.radius = 8.0.into();
            return style;
        }

        button::Style {
            background: Some(Background::Color(if hovered {
                blend(INK, accent(theme), 0.1)
            } else {
                Color::TRANSPARENT
            })),
            text_color: if hovered { TEXT } else { MUTED },
            border: Border {
                radius: 8.0.into(),
                ..Default::default()
            },
            shadow: Shadow::default(),
            ..Default::default()
        }
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
/// Flat rather than a pill: the sidebar already uses filled pills for "where
/// you are", and reusing that shape one level down would make the two rails
/// compete. The selected tab is lit from below instead — a wash that fades
/// upward out of its underline, so the two read as one lamp rather than as a
/// panel with a line under it. See [`tab_underline`].
pub fn tab_button(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let lit_accent = accent(theme);

        // The light from the bar below, spilling up onto the tab and falling
        // off. Transparent at the top so the tab itself stays black — the
        // colour is in the glow, not in the surface.
        let spill = |strength: f32| {
            Background::from(
                gradient::Linear::new(angle::DOWN)
                    .add_stop(0.0, Color::TRANSPARENT)
                    .add_stop(0.6, alpha(lit_accent, strength * 0.25))
                    .add_stop(1.0, alpha(lit_accent, strength)),
            )
        };

        button::Style {
            background: Some(match (selected, hovered) {
                (true, _) => spill(0.2),
                (false, true) => spill(0.1),
                (false, false) => Background::Color(Color::TRANSPARENT),
            }),
            text_color: if selected || hovered { TEXT } else { MUTED },
            border: Border {
                // Square at the bottom so the button sits flush on its
                // underline instead of floating above a detached bar.
                radius: border::Radius::default().top(8.0),
                ..Default::default()
            },
            shadow: Shadow::default(),
            ..Default::default()
        }
    }
}

/// The bar under a tab. Every tab draws one, so the unselected ones join into
/// a continuous rule and the selected one reads as a break in it.
///
/// The selected bar runs violet → magenta along the logo's own gradient and
/// carries a glow beneath it, which is what makes the strip look lit rather
/// than merely coloured in.
pub fn tab_underline(selected: bool) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        if !selected {
            return container::Style {
                background: Some(Background::Color(alpha(MUTED, 0.15))),
                ..Default::default()
            };
        }

        // Left to right, so a row of tabs reads as one ramp rather than each
        // tab restarting the gradient. This is the only place a full three-hue
        // ramp is spent: it is a 3px sliver, which is exactly why it can carry
        // the whole spectrum without the screen turning into a paint chart.
        let base = hue_of(accent(theme));

        container::Style {
            background: Some(Background::from(
                gradient::Linear::new(angle::ACROSS)
                    .add_stop(0.0, spectrum(base))
                    .add_stop(0.5, spectrum(base + 0.1))
                    .add_stop(1.0, spectrum(base + 0.2)),
            )),
            border: Border {
                radius: 2.0.into(),
                ..Default::default()
            },
            shadow: Shadow {
                // Cast from the middle of the ramp, so the spill under the bar
                // matches the bar rather than naming one of its ends.
                color: alpha(spectrum(base + 0.1), 0.75),
                offset: Vector::new(0.0, 1.0),
                blur_radius: 11.0,
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

/// A content card — instance tiles, account rows.
pub fn card(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(RAISED)),
        border: Border {
            radius: 12.0.into(),
            width: 1.0,
            color: alpha(accent(theme), 0.18),
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

