//! The live indicator: a dot with rings expanding out of it.
//!
//! A canvas rather than a stack of styled containers. Rings overlap, are
//! stroked at fractional radii, and fade as they travel — a container can be
//! made round, but it cannot be made to sit *inside* another one's bounds and
//! draw at 40% alpha halfway between two integer sizes without the whole thing
//! turning into arithmetic on layout units.
//!
//! It is driven from outside: `elapsed` comes from app state, not from a clock
//! read during `draw`. Only ticking it while something is actually live is
//! what keeps this from repainting the window forever — see `App::subscription`.

use crate::theme;
use iced::mouse;
use iced::widget::canvas::{self, Frame, Geometry, Path, Stroke};
use iced::{Color, Element, Point, Rectangle, Renderer, Theme};

/// How long one ring takes to travel from the dot to the outer edge.
const PERIOD: f32 = 2.4;
/// Rings in flight at once, evenly spread across the period. Three reads as a
/// steady outward flow; one reads as a blink.
const RINGS: usize = 3;

/// The widget's box. The dot sits at the centre and the outermost ring stops
/// just inside the edge, so this can be dropped into a text row without the
/// rings being clipped by a neighbour.
pub const SIZE: f32 = 22.0;

const DOT_RADIUS: f32 = 3.5;
const MAX_RADIUS: f32 = SIZE / 2.0 - 1.0;

pub struct Pulse {
    /// Seconds the indicator has been live. Ignored when `live` is false.
    elapsed: f32,
    live: bool,
}

impl Pulse {
    /// A live indicator: green, with rings.
    pub fn live(elapsed: f32) -> Self {
        Self {
            elapsed,
            live: true,
        }
    }

    /// The same dot, dimmed and still. Kept as one widget rather than two so
    /// the row it sits in doesn't reflow when the game starts or stops.
    pub fn idle() -> Self {
        Self {
            elapsed: 0.0,
            live: false,
        }
    }
}

impl<Message> canvas::Program<Message> for Pulse {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let center = Point::new(bounds.width / 2.0, bounds.height / 2.0);

        if !self.live {
            frame.fill(
                &Path::circle(center, DOT_RADIUS),
                theme::alpha(theme::MUTED, 0.55),
            );
            return vec![frame.into_geometry()];
        }

        for ring in 0..RINGS {
            // Offsetting each ring by a fraction of the period is what spaces
            // them out; they are one animation sampled at three phases rather
            // than three animations that have to be kept in step.
            let phase = (self.elapsed / PERIOD + ring as f32 / RINGS as f32).fract();
            let radius = DOT_RADIUS + phase * (MAX_RADIUS - DOT_RADIUS);

            // Squared rather than linear so a ring is still clearly visible
            // when it leaves the dot and has faded out well before the edge,
            // where it would otherwise end on a hard stop.
            let fade = (1.0 - phase).powi(2);

            frame.stroke(
                &Path::circle(center, radius),
                Stroke::default()
                    .with_color(theme::alpha(theme::MINT, fade * 0.7))
                    .with_width(1.0 + fade),
            );
        }

        // A soft halo under the dot, so the centre reads as a source the rings
        // come out of rather than as a fourth ring.
        frame.fill(
            &Path::circle(center, DOT_RADIUS * 1.8),
            theme::alpha(theme::MINT, 0.18),
        );
        frame.fill(&Path::circle(center, DOT_RADIUS), theme::MINT);

        vec![frame.into_geometry()]
    }
}

/// The indicator, sized to [`SIZE`].
pub fn view<'a, Message: 'a>(pulse: Pulse) -> Element<'a, Message> {
    // Spelled out because `canvas` is both the module imported above and the
    // helper function, and the module wins in a bare call.
    iced::widget::Canvas::new(pulse)
        .width(SIZE)
        .height(SIZE)
        .into()
}

/// The colour the label beside it should take, so the two can't drift apart.
pub fn label_color(live: bool) -> Color {
    if live { theme::MINT } else { theme::MUTED }
}
