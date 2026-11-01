//! The theme palettes and widget styling.
//!
//! The rules (written down in the umbrella project's `design/STYLE.md`): flat near-black panels separated by
//! 1px lines, and colour spent only on what is on or primary. Primary buttons
//! are filled with the theme's ramp; everything else stays dark until
//! the cursor reaches it.
//!
//! iced has no stylesheet — every style is a function of `(theme, status)`.
//! That is more verbose than CSS, but it means a widget's appearance is
//! ordinary Rust that the compiler checks, and hover/press/disabled states
//! can't be silently forgotten.

use iced::widget::{button, checkbox, container, slider, svg, text_input, toggler};
use crate::backdrop::Effect;
use iced::{border, gradient, Background, Border, Color, Font, Shadow, Theme, Vector};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LazyLock, RwLock};

/// One complete look. Every colour a screen uses comes out of the selected one.
/// The exceptions are brand marks: the logo, and the signed-out placeholder
/// figure with its rainbow outline (`skin3d`).
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    /// Empty on a custom theme, whose name lives in its [`Custom`].
    pub name: &'static str,
    /// The background effect this theme comes with.
    pub effect: Effect,
    /// The page.
    pub ink: Color,
    /// The nav rail: one step up from `ink`.
    pub surface: Color,
    /// Cards and list rows.
    pub raised: Color,
    /// Inputs and unlit buttons.
    pub field: Color,
    /// Lines between panels.
    pub line: Color,
    /// The outline of an unlit control: a step brighter than `line`.
    pub control: Color,
    pub text: Color,
    /// Secondary text; has to clear 4.5:1 on `raised`.
    pub muted: Color,
    pub danger: Color,
    /// "Works, but not ideal": the RAM slider's edges, warnings in a log.
    pub amber: Color,
    /// Glows, outlines and the selected state.
    pub accent: Color,
    /// The warm pole of the brand gradient.
    pub magenta: Color,
    /// Deepest hue of the gradient.
    pub violet: Color,
    /// Success and ready states.
    pub mint: Color,
    /// The fill of a primary button, under a dark label: see [`rainbow`].
    pub ramp: [Color; 4],
}

/// Ten dark looks, the launcher's own first.
pub static PALETTES: [Palette; 10] = [
    Palette {
        name: "Nexo",
        effect: Effect::Enchant,
        ink: rgb(0x0a, 0x0a, 0x0d),
        surface: rgb(0x0d, 0x0d, 0x11),
        raised: rgb(0x11, 0x11, 0x16),
        field: rgb(0x10, 0x10, 0x15),
        line: rgb(0x1d, 0x1d, 0x25),
        control: rgb(0x23, 0x23, 0x2c),
        text: rgb(0xf2, 0xf2, 0xf5),
        muted: rgb(0x8b, 0x8b, 0x99),
        danger: rgb(0xff, 0x54, 0x70),
        amber: rgb(0xff, 0xb4, 0x3c),
        // A violet-pink between the logo's magenta and violet, so the glow
        // matches the fill it sits around.
        accent: rgb(0xb1, 0x4c, 0xff),
        magenta: rgb(0xff, 0x3c, 0xac),
        violet: rgb(0x7b, 0x3c, 0xff),
        mint: rgb(0x3c, 0xff, 0xb0),
        // Lighter than the logo's #7b3cff in the second stop: dark button
        // labels cross this band, and #7b3cff only gives them ~3.7:1.
        ramp: [rgb(0xff, 0x3c, 0xac), rgb(0x9a, 0x6b, 0xff), rgb(0x22, 0xb8, 0xff), rgb(0x3c, 0xff, 0xb0)],
    },
    Palette {
        name: "Matrix",
        effect: Effect::Rain,
        ink: rgb(0x05, 0x08, 0x06),
        surface: rgb(0x07, 0x0b, 0x08),
        raised: rgb(0x0a, 0x10, 0x0b),
        field: rgb(0x09, 0x10, 0x09),
        line: rgb(0x14, 0x20, 0x18),
        control: rgb(0x1a, 0x2a, 0x1f),
        text: rgb(0xe6, 0xff, 0xe9),
        muted: rgb(0x7d, 0xa2, 0x87),
        danger: rgb(0xff, 0x54, 0x70),
        amber: rgb(0xff, 0xb4, 0x3c),
        accent: rgb(0x39, 0xff, 0x7a),
        magenta: rgb(0x9d, 0xff, 0x3c),
        violet: rgb(0x12, 0xb8, 0x5a),
        mint: rgb(0x3c, 0xff, 0xb0),
        ramp: [rgb(0xb6, 0xff, 0x3c), rgb(0x39, 0xff, 0x7a), rgb(0x14, 0xd9, 0x8a), rgb(0x7d, 0xff, 0xd0)],
    },
    Palette {
        name: "Nether",
        effect: Effect::Embers,
        ink: rgb(0x0d, 0x07, 0x06),
        surface: rgb(0x12, 0x09, 0x08),
        raised: rgb(0x17, 0x0c, 0x0a),
        field: rgb(0x15, 0x0b, 0x09),
        line: rgb(0x2a, 0x16, 0x12),
        control: rgb(0x34, 0x20, 0x1a),
        text: rgb(0xf9, 0xee, 0xe8),
        muted: rgb(0xa8, 0x8a, 0x7e),
        danger: rgb(0xff, 0x4d, 0x5e),
        amber: rgb(0xff, 0xc2, 0x3c),
        accent: rgb(0xff, 0x6a, 0x2b),
        magenta: rgb(0xff, 0x3c, 0x3c),
        violet: rgb(0xc8, 0x32, 0x1e),
        mint: rgb(0x7d, 0xff, 0x6a),
        ramp: [rgb(0xff, 0x4d, 0x3c), rgb(0xff, 0x7a, 0x2b), rgb(0xff, 0xae, 0x2b), rgb(0xff, 0xe0, 0x6a)],
    },
    Palette {
        name: "End",
        effect: Effect::Portal,
        ink: rgb(0x07, 0x07, 0x0f),
        surface: rgb(0x0a, 0x0a, 0x16),
        raised: rgb(0x0f, 0x0f, 0x1e),
        field: rgb(0x0e, 0x0e, 0x1b),
        line: rgb(0x1c, 0x1c, 0x33),
        control: rgb(0x25, 0x25, 0x3f),
        text: rgb(0xef, 0xee, 0xfb),
        muted: rgb(0x8f, 0x8f, 0xb0),
        danger: rgb(0xff, 0x54, 0x70),
        amber: rgb(0xff, 0xcf, 0x6b),
        accent: rgb(0x2e, 0xe6, 0xc5),
        magenta: rgb(0xb6, 0x4c, 0xff),
        violet: rgb(0x6a, 0x3c, 0xff),
        mint: rgb(0x5c, 0xff, 0xb0),
        ramp: [rgb(0xc7, 0x7d, 0xff), rgb(0x8f, 0x8b, 0xff), rgb(0x4f, 0xc3, 0xff), rgb(0x2e, 0xe6, 0xc5)],
    },
    Palette {
        name: "Frost",
        effect: Effect::Snow,
        ink: rgb(0x07, 0x0b, 0x10),
        surface: rgb(0x0a, 0x0f, 0x16),
        raised: rgb(0x0e, 0x14, 0x1d),
        field: rgb(0x0d, 0x13, 0x1b),
        line: rgb(0x1a, 0x25, 0x33),
        control: rgb(0x22, 0x30, 0x41),
        text: rgb(0xee, 0xf6, 0xff),
        muted: rgb(0x88, 0xa0, 0xb5),
        danger: rgb(0xff, 0x5d, 0x73),
        amber: rgb(0xff, 0xc0, 0x4d),
        accent: rgb(0x5c, 0xc8, 0xff),
        magenta: rgb(0xa5, 0xb8, 0xff),
        violet: rgb(0x3c, 0x7b, 0xff),
        mint: rgb(0x5c, 0xff, 0xd6),
        ramp: [rgb(0xc6, 0xd8, 0xff), rgb(0x7d, 0xb9, 0xff), rgb(0x4f, 0xd3, 0xff), rgb(0x8f, 0xf5, 0xf0)],
    },
    Palette {
        name: "Cherry",
        effect: Effect::Petals,
        ink: rgb(0x0f, 0x09, 0x0c),
        surface: rgb(0x14, 0x0c, 0x10),
        raised: rgb(0x1a, 0x10, 0x15),
        field: rgb(0x18, 0x0e, 0x13),
        line: rgb(0x2d, 0x1c, 0x24),
        control: rgb(0x38, 0x23, 0x2d),
        text: rgb(0xfb, 0xf0, 0xf4),
        muted: rgb(0xac, 0x8d, 0x9a),
        danger: rgb(0xff, 0x5a, 0x5a),
        amber: rgb(0xff, 0xc0, 0x5c),
        accent: rgb(0xff, 0x7a, 0xb8),
        magenta: rgb(0xff, 0x4f, 0xa3),
        violet: rgb(0xd4, 0x5f, 0xa0),
        mint: rgb(0x7d, 0xff, 0xb8),
        ramp: [rgb(0xff, 0x5f, 0xa8), rgb(0xff, 0x8c, 0xc4), rgb(0xff, 0xb3, 0xd6), rgb(0xff, 0xd9, 0xb0)],
    },
    Palette {
        name: "Nebula Voxels",
        effect: Effect::Voxels,
        ink: rgb(0x0a, 0x06, 0x18),
        surface: rgb(0x0e, 0x09, 0x20),
        raised: rgb(0x13, 0x0d, 0x29),
        field: rgb(0x11, 0x0b, 0x25),
        line: rgb(0x24, 0x1a, 0x42),
        control: rgb(0x2e, 0x22, 0x50),
        text: rgb(0xf5, 0xef, 0xff),
        muted: rgb(0x9d, 0x90, 0xc0),
        danger: rgb(0xff, 0x5c, 0x7a),
        amber: rgb(0xff, 0xc8, 0x6b),
        accent: rgb(0xff, 0x5c, 0xe1),
        magenta: rgb(0xff, 0x6a, 0xd5),
        violet: rgb(0xad, 0x8c, 0xff),
        mint: rgb(0x5c, 0xff, 0xc8),
        ramp: [rgb(0xff, 0x6a, 0xd5), rgb(0xc7, 0x74, 0xe8), rgb(0xad, 0x8c, 0xff), rgb(0x87, 0xb5, 0xff)],
    },
    Palette {
        name: "Nebula Lightning",
        effect: Effect::Lightning,
        ink: rgb(0x06, 0x0a, 0x1c),
        surface: rgb(0x09, 0x0e, 0x24),
        raised: rgb(0x0d, 0x13, 0x2d),
        field: rgb(0x0b, 0x11, 0x29),
        line: rgb(0x1a, 0x24, 0x48),
        control: rgb(0x23, 0x2f, 0x57),
        text: rgb(0xee, 0xf3, 0xff),
        muted: rgb(0x8e, 0x9b, 0xc4),
        danger: rgb(0xff, 0x5c, 0x7a),
        amber: rgb(0xff, 0xd2, 0x5c),
        accent: rgb(0x7c, 0xb0, 0xff),
        magenta: rgb(0xc7, 0x8b, 0xff),
        violet: rgb(0x6c, 0x7b, 0xff),
        mint: rgb(0x5c, 0xff, 0xd0),
        ramp: [rgb(0xe2, 0x7b, 0xff), rgb(0xa7, 0x8b, 0xff), rgb(0x7f, 0xb2, 0xff), rgb(0xd6, 0xe8, 0xff)],
    },
    Palette {
        name: "Nebula Grid",
        effect: Effect::Grid,
        ink: rgb(0x0b, 0x04, 0x20),
        surface: rgb(0x10, 0x07, 0x29),
        raised: rgb(0x15, 0x0b, 0x32),
        field: rgb(0x13, 0x09, 0x2e),
        line: rgb(0x26, 0x1a, 0x4f),
        control: rgb(0x31, 0x23, 0x60),
        text: rgb(0xf2, 0xf0, 0xff),
        muted: rgb(0x98, 0x92, 0xc8),
        danger: rgb(0xff, 0x5c, 0x7a),
        amber: rgb(0xff, 0xc8, 0x4d),
        accent: rgb(0x2b, 0xe7, 0xff),
        magenta: rgb(0xff, 0x4f, 0xd8),
        violet: rgb(0x8f, 0x6b, 0xff),
        mint: rgb(0x4d, 0xff, 0xc0),
        ramp: [rgb(0xff, 0x4f, 0xd8), rgb(0xc5, 0x6b, 0xff), rgb(0x6f, 0x9d, 0xff), rgb(0x2b, 0xe7, 0xff)],
    },
    Palette {
        name: "Liquid Chrome",
        effect: Effect::Chrome,
        ink: rgb(0x0b, 0x0c, 0x0e),
        surface: rgb(0x10, 0x11, 0x14),
        raised: rgb(0x15, 0x17, 0x1a),
        field: rgb(0x13, 0x15, 0x18),
        line: rgb(0x27, 0x2a, 0x2f),
        control: rgb(0x32, 0x36, 0x3c),
        text: rgb(0xf4, 0xf6, 0xf8),
        muted: rgb(0x9a, 0xa1, 0xaa),
        danger: rgb(0xff, 0x5c, 0x70),
        amber: rgb(0xe8, 0xbd, 0x6a),
        accent: rgb(0xd8, 0xde, 0xe6),
        magenta: rgb(0xb8, 0xc0, 0xca),
        violet: rgb(0x8b, 0x94, 0x9e),
        mint: rgb(0x6f, 0xe3, 0xb8),
        ramp: [rgb(0xf5, 0xf7, 0xfa), rgb(0xae, 0xb6, 0xbf), rgb(0xe4, 0xe8, 0xed), rgb(0x8d, 0x96, 0xa0)],
    },
];

/// Which theme is showing: an index into [`PALETTES`], then on into the custom
/// themes the app holds.
static SELECTED: AtomicUsize = AtomicUsize::new(0);

struct Look {
    palette: Palette,
    /// iced's theme for it, kept because [`current`] runs on every frame of
    /// every animating widget.
    theme: Theme,
}

static LOOK: LazyLock<RwLock<Look>> = LazyLock::new(|| {
    RwLock::new(Look { palette: PALETTES[0], theme: iced_theme(&PALETTES[0]) })
});

fn iced_theme(p: &Palette) -> Theme {
    Theme::custom(
        p.name.to_string(),
        iced::theme::Palette {
            background: p.ink,
            text: p.text,
            primary: p.accent,
            success: p.mint,
            warning: rgb(0xff, 0xd9, 0x3c),
            danger: p.danger,
        },
    )
}

pub fn selected() -> usize {
    SELECTED.load(Ordering::Relaxed)
}

/// Switches the whole launcher. Animated controls ease to the new look by
/// themselves, since their targets come from the same palette.
pub fn apply(index: usize, palette: Palette) {
    SELECTED.store(index, Ordering::Relaxed);
    *LOOK.write().unwrap() = Look { palette, theme: iced_theme(&palette) };
}

pub fn palette() -> Palette {
    LOOK.read().unwrap().palette
}

/// The selected theme, also for widgets working out their next look outside
/// `draw`, the one place iced hands them a theme.
pub fn current() -> Theme {
    LOOK.read().unwrap().theme.clone()
}

/// A theme somebody made: a few chosen colours, with the rest worked out from
/// them so a new theme can't end up half-finished.
#[derive(Debug, Clone, PartialEq)]
pub struct Custom {
    pub name: String,
    pub effect: Effect,
    /// In the order of [`Custom::SLOTS`].
    pub colors: [Color; 7],
}

impl Custom {
    pub const SLOTS: [&'static str; 7] =
        ["Background", "Text", "Accent", "Gradient 1", "Gradient 2", "Gradient 3", "Gradient 4"];

    /// A copy of an existing look, as the starting point for a new theme.
    pub fn from_palette(name: String, effect: Effect, p: &Palette) -> Self {
        let [a, b, c, d] = p.ramp;
        Self { name, effect, colors: [p.ink, p.text, p.accent, a, b, c, d] }
    }

    pub fn palette(&self) -> Palette {
        let [ink, text, accent, a, b, c, d] = self.colors;
        // The same steps from page to text that the Nexo palette takes, so this
        // works from either direction. A light page is allowed, though the fixed
        // status colours are made for dark ones: `problems` says so.
        let step = |amount| blend(ink, text, amount);
        Palette {
            name: "",
            effect: self.effect,
            ink,
            surface: step(0.013),
            raised: step(0.03),
            field: step(0.026),
            line: step(0.082),
            control: step(0.108),
            text,
            muted: step(0.556),
            // Meanings, not decoration: red stays danger in every custom theme.
            danger: PALETTES[0].danger,
            amber: PALETTES[0].amber,
            accent,
            magenta: a,
            violet: b,
            mint: PALETTES[0].mint,
            ramp: [a, b, c, d],
        }
    }

    /// What is going to be hard to read. Warned about, not refused: it's their
    /// theme, and any built-in one is a click away.
    pub fn problems(&self) -> Vec<&'static str> {
        let [ink, text, accent, ramp @ ..] = self.colors;
        let mut problems = Vec::new();
        if contrast(text, ink) < 4.5 {
            problems.push("Text is hard to read on this background.");
        }
        if contrast(accent, ink) < 3.0 {
            problems.push("The accent barely shows against the background.");
        }
        let status = &PALETTES[0];
        if [status.danger, status.amber, status.mint].iter().any(|&color| contrast(color, ink) < 3.0) {
            problems.push("Warnings and status colours are hard to read on this background.");
        }
        if ramp.iter().any(|&stop| contrast(ink, stop) < 3.0) {
            problems.push("Button labels use the background colour: part of the gradient is too close to it.");
        }
        problems
    }
}

fn luminance(c: Color) -> f32 {
    let channel = |v: f32| if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
    0.2126 * channel(c.r) + 0.7152 * channel(c.g) + 0.0722 * channel(c.b)
}

fn contrast(a: Color, b: Color) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

pub fn to_hex(color: Color) -> String {
    let [r, g, b, _] = color.into_rgba8();
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// `#rrggbb`, always opaque.
pub fn from_hex(hex: &str) -> Option<Color> {
    let digits = hex.trim().trim_start_matches('#');
    if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).ok();
    Some(Color::from_rgb8(byte(0)?, byte(2)?, byte(4)?))
}

/// Hue in degrees, saturation and brightness 0 to 1.
pub fn from_hsv(hue: f32, saturation: f32, value: f32) -> Color {
    let channel = |n: f32| {
        let k = (n + hue / 60.0) % 6.0;
        value - value * saturation * k.min(4.0 - k).clamp(0.0, 1.0)
    };
    Color::from_rgb(channel(5.0), channel(3.0), channel(1.0))
}

pub fn to_hsv(color: Color) -> (f32, f32, f32) {
    let max = color.r.max(color.g).max(color.b);
    let spread = max - color.r.min(color.g).min(color.b);
    let hue = if spread == 0.0 {
        0.0
    } else if max == color.r {
        60.0 * ((color.g - color.b) / spread).rem_euclid(6.0)
    } else if max == color.g {
        60.0 * ((color.b - color.r) / spread + 2.0)
    } else {
        60.0 * ((color.r - color.g) / spread + 4.0)
    };
    (hue, if max == 0.0 { 0.0 } else { spread / max }, max)
}

/// The track of a colour slider, showing what moving along it does.
pub fn color_track(stops: Vec<Color>) -> impl Fn(&Theme) -> container::Style {
    move |_theme| {
        let last = (stops.len() - 1).max(1) as f32;
        let ramp = stops
            .iter()
            .enumerate()
            .fold(gradient::Linear::new(angle::RIGHT), |ramp, (i, &stop)| ramp.add_stop(i as f32 / last, stop));
        container::Style {
            background: Some(Background::from(ramp)),
            border: Border { radius: 4.0.into(), width: 1.0, color: control() },
            ..Default::default()
        }
    }
}

pub fn ink() -> Color { palette().ink }
pub fn surface() -> Color { palette().surface }
pub fn raised() -> Color { palette().raised }
pub fn field() -> Color { palette().field }
pub fn line() -> Color { palette().line }
pub fn control() -> Color { palette().control }
pub fn text() -> Color { palette().text }
pub fn muted() -> Color { palette().muted }
pub fn danger() -> Color { palette().danger }
pub fn amber() -> Color { palette().amber }
pub fn accent() -> Color { palette().accent }
pub fn magenta() -> Color { palette().magenta }
pub fn violet() -> Color { palette().violet }
pub fn mint() -> Color { palette().mint }
/// Unchecked boxes, off switches, idle tiles: a little brighter than `control`.
fn dim_edge() -> Color { blend(control(), muted(), 0.07) }

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
    /// Left to right.
    pub const RIGHT: f32 = std::f32::consts::FRAC_PI_2;
    /// Top to bottom.
    pub const DOWN: f32 = std::f32::consts::PI;
    /// Top-left to bottom-right, the way the logo's ramp runs.
    pub const DIAGONAL: f32 = std::f32::consts::FRAC_PI_4 * 3.0;
}

/// Lays `top` over `base` at `amount` opacity.
///
/// Buttons need an *opaque* result: they sit on the page in some places and
/// on a raised card in others, and a translucent tint would come out a
/// different colour in each. Blending against the page's own colour keeps every
/// button the same colour wherever it lands.
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

/// The selected theme's ramp as a fill. Fixed stops, not derived from the
/// spectrum: the spectrum's evenly lit hues turned a small button into broad
/// flat bands, where a hand-picked ramp keeps it reading as one gradient.
pub fn rainbow(_theme: &Theme) -> Background {
    ramp_of(&palette())
}

fn ramp_of(palette: &Palette) -> Background {
    let [a, b, c, d] = palette.ramp;
    Background::from(
        gradient::Linear::new(angle::DIAGONAL)
            .add_stop(0.0, a)
            .add_stop(0.35, b)
            .add_stop(0.7, c)
            .add_stop(1.0, d),
    )
}

/// A theme's sample on its picker card, in that theme's colours whichever is
/// selected.
pub fn swatch(palette: Palette) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(ramp_of(&palette)),
        border: Border { radius: 6.0.into(), ..Default::default() },
        ..Default::default()
    }
}

/// The ground of a theme's picker card.
pub fn swatch_ground(palette: Palette) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(palette.ink)),
        border: Border { radius: 8.0.into(), width: 1.0, color: palette.line },
        ..Default::default()
    }
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
                background: Some(Background::Color(field())),
                text_color: alpha(muted(), 0.6),
                border: Border {
                    radius: 8.0.into(),
                    width: 1.0,
                    color: control(),
                },
                ..Default::default()
            };
        }
    };

    button::Style {
        background: Some(rainbow(theme)),
        text_color: ink(),
        border: Border {
            radius: 8.0.into(),
            ..Default::default()
        },
        shadow: halo(alpha(accent_of(theme), 0.5), blur * glow),
        ..Default::default()
    }
}

/// An unlit button: dark field, 1px line, and only the outline reacts to the
/// cursor. `tone` colours the label and the hovered outline — the accent for
/// ordinary actions, red for destructive ones.
fn outline(tone: Option<Color>, theme: &Theme, status: button::Status) -> button::Style {
    let edge = tone.unwrap_or(accent_of(theme));
    let (background, line) = match status {
        button::Status::Active => (field(), control()),
        button::Status::Hovered => (field(), alpha(edge, 0.7)),
        button::Status::Pressed => (blend(field(), edge, 0.12), edge),
        button::Status::Disabled => (field(), alpha(control(), 0.6)),
    };

    button::Style {
        background: Some(Background::Color(background)),
        text_color: match status {
            button::Status::Disabled => alpha(muted(), 0.6),
            _ => tone.unwrap_or(text()),
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

/// The accent, read out of the theme each style is handed.
fn accent_of(theme: &Theme) -> Color {
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
    let mut style = outline(Some(danger()), theme, status);
    let glow = match status {
        button::Status::Active => 12.0,
        button::Status::Hovered => 22.0,
        button::Status::Pressed => 8.0,
        button::Status::Disabled => return style,
    };
    style.border.color = danger();
    style.shadow = halo(alpha(danger(), 0.35), glow);
    style
}

/// A secondary action that still has to be found — "+ New instance". Lit
/// at rest (accent edge, faint accent fill, soft glow) but with a plain text label
/// and no rainbow, so it never competes with a Play beside it.
pub fn lit_button(theme: &Theme, status: button::Status) -> button::Style {
    let accent = accent_of(theme);
    let (fill, glow) = match status {
        button::Status::Active => (0.14, 16.0),
        button::Status::Hovered => (0.2, 22.0),
        button::Status::Pressed => (0.24, 8.0),
        button::Status::Disabled => return outline(None, theme, status),
    };
    button::Style {
        background: Some(Background::Color(blend(ink(), accent, fill))),
        text_color: text(),
        border: Border {
            radius: 8.0.into(),
            width: 1.0,
            color: accent,
        },
        shadow: halo(alpha(accent, 0.35), glow),
        ..Default::default()
    }
}

/// Destructive actions. Red label, dark until touched: a Delete that glows on
/// its own draws the eye to exactly the button nobody should press by
/// accident.
pub fn danger_button(theme: &Theme, status: button::Status) -> button::Style {
    outline(Some(danger()), theme, status)
}

/// The rail's selected look, on the one highlight that slides between entries.
pub fn nav_highlight(theme: &Theme) -> container::Style {
    let accent = accent_of(theme);
    container::Style {
        background: Some(Background::Color(blend(surface(), accent, 0.16))),
        border: Border {
            radius: 8.0.into(),
            width: 1.0,
            color: accent,
        },
        shadow: halo(alpha(accent, 0.45), 16.0),
        ..Default::default()
    }
}

/// The window's colour at `amount` opacity: see `screens::reveal`.
pub fn veil(amount: f32) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: (amount > 0.0).then(|| Background::Color(alpha(ink(), amount))),
        ..Default::default()
    }
}

/// A square icon entry in the nav rail. The selected one is tinted, outlined
/// and glowing; the rest are bare until hovered, which is what makes "where
/// you are" readable at a glance. The icon itself is tinted by [`nav_icon`].
pub fn nav_button(selected: bool, hover: f32) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, _status| {
        let accent = accent_of(theme);

        // The selected look itself is `nav_highlight`, sitting behind this.
        if selected {
            return button::Style {
                background: None,
                text_color: accent,
                border: Border {
                    radius: 8.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            };
        }

        button::Style {
            background: Some(Background::Color(alpha(field(), hover))),
            text_color: blend(muted(), text(), hover),
            border: Border {
                radius: 8.0.into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }
}

/// A small selectable button off the rail, with the selected look built in.
pub fn nav_chip(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        if !selected {
            let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
            return nav_button(false, if hovered { 1.0 } else { 0.0 })(theme, status);
        }
        let highlight = nav_highlight(theme);
        button::Style {
            background: highlight.background,
            text_color: accent_of(theme),
            border: highlight.border,
            shadow: highlight.shadow,
            ..Default::default()
        }
    }
}

/// Tints a rail icon to match its [`nav_button`]: accent when selected, text
/// when hovered, muted otherwise.
///
/// `lit` is how close the sliding highlight is, 0 to 1, so an icon takes the
/// accent as the highlight reaches it rather than ahead of it.
pub fn nav_icon(lit: f32) -> impl Fn(&Theme, svg::Status) -> svg::Style {
    move |theme, status| {
        let base = if matches!(status, svg::Status::Hovered) { text() } else { muted() };
        svg::Style {
            color: Some(blend(base, accent_of(theme), lit)),
        }
    }
}

/// An icon in the label colour of the [`filled`] button it sits on: dark on
/// the rainbow, muted on the dark field a disabled one falls back to.
pub fn icon_on_fill(enabled: bool) -> impl Fn(&Theme, svg::Status) -> svg::Style {
    move |_theme, _status| svg::Style {
        color: Some(if enabled { ink() } else { alpha(muted(), 0.6) }),
    }
}

/// Chromeless button, for making a block of content clickable without it
/// reading as a control.
/// `status` is deliberately ignored: the hover affordance belongs to the card
/// this sits inside, and tinting here would double up on it.
pub fn bare_button(_theme: &Theme, _status: button::Status) -> button::Style {
    button::Style {
        background: Some(Background::Color(Color::TRANSPARENT)),
        text_color: text(),
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
            blend(ink(), accent_of(theme), 0.12)
        } else {
            surface()
        })),
        text_color: text(),
        border: Border {
            radius: 10.0.into(),
            width: 1.0,
            color: alpha(if hovered { accent_of(theme) } else { muted() }, 0.35),
        },
        shadow: Shadow::default(),
        ..Default::default()
    }
}

/// The tile that's currently chosen.
///
/// Stays mint rather than taking the brand ramp: everywhere else in the app
/// mint means *this is the one that is ready/chosen*, and an accent tile would
/// read as merely hovered next to the accent washes on either side of it.
pub fn selected_tile(_theme: &Theme, _status: button::Status) -> button::Style {
    button::Style {
        background: Some(Background::Color(blend(ink(), mint(), 0.1))),
        text_color: text(),
        border: Border {
            radius: 10.0.into(),
            width: 1.0,
            color: mint(),
        },
        shadow: Shadow {
            color: alpha(mint(), 0.25),
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
/// The selected tab is bright text over a faint wash rising from its underline —
/// the mockup's glow is a text-shadow, which iced cannot draw, and this is
/// the nearest light that stays behind the label. See [`tab_underline`].
pub fn tab_button(amount: f32) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);

        button::Style {
            background: Some(if amount > 0.0 {
                Background::from(
                    gradient::Linear::new(angle::DOWN)
                        .add_stop(0.0, Color::TRANSPARENT)
                        .add_stop(1.0, alpha(accent_of(theme), 0.12 * amount)),
                )
            } else {
                Background::Color(Color::TRANSPARENT)
            }),
            // Hover only brightens the label: an unselected tab that lit up
            // under the cursor would read as a second selection.
            text_color: if hovered { text() } else { blend(muted(), text(), amount) },
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
                color: alpha(accent_of(theme), 0.6),
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
        background: Some(Background::Color(blend(ink(), accent_of(theme), 0.14))),
        text_color: Some(if selected {
            lighten(accent_of(theme), 0.28)
        } else {
            muted()
        }),
        border: Border {
            radius: 100.0.into(),
            width: 1.0,
            color: alpha(if selected { accent_of(theme) } else { muted() }, 0.35),
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
                blend(ink(), accent_of(theme), wash)
            } else {
                Color::TRANSPARENT
            })),
            text_color: if selected { text() } else { muted() },
            border: Border {
                radius: 8.0.into(),
                width: if selected { 1.0 } else { 0.0 },
                color: alpha(accent_of(theme), 0.55),
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
        background: Some(Background::Color(ink())),
        border: Border {
            radius: 10.0.into(),
            width: 1.0,
            color: alpha(muted(), 0.15),
        },
        ..Default::default()
    }
}

/// A content card — instance tiles, account rows. A plain line, no accent:
/// cards are where content sits, and tinting every one of them is how a
/// screen ends up glowing all over.
pub fn card(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(raised())),
        border: Border {
            radius: 10.0.into(),
            width: 1.0,
            color: line(),
        },
        ..Default::default()
    }
}

/// A side panel beside a list — the Modrinth browser's filters. One step
/// darker than [`card`], so the panel reads as furniture next to the results
/// rather than as one more result.
pub fn panel(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(surface())),
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
        icon_color: ink(),
        border: Border {
            radius: 4.0.into(),
            width: 1.0,
            color: match (checked, hovered) {
                (true, _) => alpha(accent_of(theme), dim),
                (false, true) => alpha(accent_of(theme), 0.7),
                (false, false) => dim_edge(),
            },
        },
        text_color: Some(if checked && enabled { text() } else { muted() }),
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
            Background::Color(dim_edge())
        }),
        text_color: Some(if running { ink() } else { text() }),
        border: Border {
            radius: 8.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// The small state pill beside an instance's name. Mint only for RUNNING —
/// the one state worth spotting from across the list; everything else is a
/// flat grey chip.
pub fn tag(lit: bool) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(if lit { alpha(mint(), 0.14) } else { line() })),
        text_color: Some(if lit { mint() } else { muted() }),
        border: Border {
            radius: 4.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// The nav rail's ground.
pub fn sidebar(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(surface())),
        ..Default::default()
    }
}

/// The 1px line down the rail's edge. A container rather than a border on the
/// rail, because iced borders go all the way round.
pub fn divider(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(line())),
        ..Default::default()
    }
}

/// The label a rail icon shows on hover, since the rail has no text of its own.
pub fn tooltip(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(raised())),
        text_color: Some(text()),
        border: Border {
            radius: 6.0.into(),
            width: 1.0,
            color: control(),
        },
        ..Default::default()
    }
}

/// The same, at `amount` of its full strength, for fading it in.
pub fn tooltip_at(amount: f32) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        let full = tooltip(theme);
        container::Style {
            background: Some(Background::Color(alpha(raised(), amount))),
            text_color: Some(alpha(text(), amount)),
            border: Border { color: alpha(control(), amount), ..full.border },
            ..full
        }
    }
}

/// Inline status/error strip.
pub fn banner(is_error: bool) -> impl Fn(&Theme) -> container::Style {
    move |_theme| {
        let accent = if is_error { danger() } else { mint() };
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
        background: Some(Background::Color(alpha(accent_of(theme), 0.12))),
        border: Border {
            radius: 12.0.into(),
            width: 1.0,
            color: alpha(accent_of(theme), 0.5),
        },
        ..Default::default()
    }
}

pub fn input(theme: &Theme, status: text_input::Status) -> text_input::Style {
    let border_color = match status {
        text_input::Status::Focused { .. } => accent_of(theme),
        text_input::Status::Hovered => alpha(accent_of(theme), 0.5),
        _ => control(),
    };

    text_input::Style {
        background: Background::Color(field()),
        border: Border {
            radius: 8.0.into(),
            width: 1.0,
            color: border_color,
        },
        icon: muted(),
        placeholder: muted(),
        value: text(),
        selection: alpha(accent_of(theme), 0.4),
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
    // On: a tinted track inside a lit outline, bright knob. Off: a dark track
    // with a dim knob, so on and off differ in lightness, not only in hue.
    let (track, edge, knob) = if on {
        (blend(ink(), accent_of(theme), 0.22), accent_of(theme), text())
    } else {
        (field(), dim_edge(), blend(muted(), ink(), 0.35))
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

/// A slider drawn over a track of its own (the RAM zones), so its rail is
/// invisible and only the knob shows.
pub fn bare_slider(_theme: &Theme, status: slider::Status) -> slider::Style {
    let ring = match status {
        slider::Status::Hovered | slider::Status::Dragged => 3.0,
        slider::Status::Active => 2.0,
    };
    slider::Style {
        rail: slider::Rail {
            backgrounds: (Background::Color(Color::TRANSPARENT), Background::Color(Color::TRANSPARENT)),
            width: 4.0,
            border: Border::default(),
        },
        handle: slider::Handle {
            shape: slider::HandleShape::Circle { radius: 8.0 },
            background: Background::Color(text()),
            border_width: ring,
            border_color: ink(),
        },
    }
}

/// A solid block of one colour: RAM zones, meter fills, graph legend keys.
pub fn fill(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(color)),
        border: Border { radius: 3.0.into(), ..Default::default() },
        ..Default::default()
    }
}

/// A callout in any colour — [`banner`] past its two states, for the RAM
/// verdict's mint / amber / red.
pub fn callout(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(alpha(color, 0.12))),
        border: Border { radius: 10.0.into(), width: 1.0, color: alpha(color, 0.45) },
        text_color: Some(color),
        ..Default::default()
    }
}

/// A state pill in any colour — [`tag`] generalised past mint/grey, for
/// server states that need amber and red too.
pub fn pill(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(alpha(color, 0.14))),
        text_color: Some(color),
        border: Border { radius: 100.0.into(), width: 1.0, color: alpha(color, 0.35) },
        ..Default::default()
    }
}

/// A numbered step in the server wizard: lit ring for the current one, mint
/// for done, a dim disc for the rest.
pub fn step_dot(current: bool, done: bool) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        let (fill, edge, label) = match (current, done) {
            (true, _) => (blend(ink(), accent_of(theme), 0.2), accent_of(theme), text()),
            (false, true) => (alpha(mint(), 0.14), alpha(mint(), 0.6), mint()),
            (false, false) => (control(), control(), muted()),
        };
        container::Style {
            background: Some(Background::Color(fill)),
            text_color: Some(label),
            border: Border { radius: 100.0.into(), width: if current { 2.0 } else { 1.0 }, color: edge },
            shadow: if current { halo(alpha(accent_of(theme), 0.4), 12.0) } else { Shadow::default() },
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A new palette is the likeliest way to ship unreadable text.
    #[test]
    fn every_palette_stays_readable() {
        for p in &PALETTES {
            for ground in [p.ink, p.surface, p.raised, p.field] {
                assert!(contrast(p.text, ground) >= 7.0, "{}: text", p.name);
                assert!(contrast(p.muted, ground) >= 4.5, "{}: muted", p.name);
                assert!(contrast(p.danger, ground) >= 4.5, "{}: danger", p.name);
                assert!(contrast(p.mint, ground) >= 4.5, "{}: mint", p.name);
            }
            // The dark label on a filled button.
            for stop in p.ramp {
                assert!(contrast(p.ink, stop) >= 4.5, "{}: ramp", p.name);
            }
        }
    }

    /// Built from a built-in's own colours, the derived ones must stay readable too.
    #[test]
    fn custom_themes_derive_readable_colours() {
        for p in &PALETTES {
            let custom = Custom::from_palette("x".into(), p.effect, p);
            let derived = custom.palette();
            for ground in [derived.ink, derived.surface, derived.raised, derived.field] {
                assert!(contrast(derived.muted, ground) >= 4.5, "{}: muted", p.name);
            }
            assert!(custom.problems().is_empty(), "{}: {:?}", p.name, custom.problems());
        }
    }

    #[test]
    fn unreadable_custom_themes_are_flagged() {
        let mut custom = Custom::from_palette("x".into(), Effect::Off, &PALETTES[0]);
        custom.colors[1] = custom.colors[0];
        assert!(!custom.problems().is_empty());
    }

    #[test]
    fn colours_survive_hex_and_hsv() {
        for p in &PALETTES {
            for color in p.ramp.into_iter().chain([p.ink, p.text, p.muted]) {
                assert_eq!(from_hex(&to_hex(color)), Some(color));
                let (h, s, v) = to_hsv(color);
                assert_eq!(to_hex(from_hsv(h, s, v)), to_hex(color));
            }
        }
        for bad in ["", "#12345", "#gggggg", "#1234567", "#ééé", "#+f+f+f"] {
            assert_eq!(from_hex(bad), None, "{bad}");
        }
    }
}
