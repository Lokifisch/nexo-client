//! The animated particles behind the player model.
//!
//! Every particle is a pure function of its index and the clock, so there is
//! nothing to simulate or store, and the frame costs only what it draws. Square
//! pixels throughout, to look like Minecraft's own particles; the glyphs are
//! drawn as pixels so they don't depend on which fonts a machine has.
//!
//! Kept sparse and translucent: this sits behind the screen's content.

use crate::theme;
use iced::mouse;
use iced::widget::canvas::{self, gradient, Frame, Geometry, Gradient, Path, Stroke, Style};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Size, Theme};
use std::f32::consts::TAU;
use std::time::{Duration, Instant};

/// 30 fps. Slow drifting pixels look no different at 60.
pub const FRAME: Duration = Duration::from_millis(33);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    Enchant,
    Rain,
    Embers,
    Portal,
    Snow,
    Petals,
    Voxels,
    Lightning,
    Grid,
    Chrome,
    Off,
}

impl Effect {
    pub const ALL: [Effect; 11] = [
        Effect::Enchant,
        Effect::Rain,
        Effect::Embers,
        Effect::Portal,
        Effect::Snow,
        Effect::Petals,
        Effect::Voxels,
        Effect::Lightning,
        Effect::Grid,
        Effect::Chrome,
        Effect::Off,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Effect::Enchant => "Enchantment",
            Effect::Rain => "Matrix rain",
            Effect::Embers => "Embers",
            Effect::Portal => "Portal",
            Effect::Snow => "Snow",
            Effect::Petals => "Petals",
            Effect::Voxels => "Nebula voxels",
            Effect::Lightning => "Nebula lightning",
            Effect::Grid => "Nebula grid",
            Effect::Chrome => "Liquid chrome",
            Effect::Off => "Off",
        }
    }
}

/// 5x5 pixel runes in the spirit of the enchanting table's, one row per byte.
const RUNES: [[u8; 5]; 10] = [
    [0b10001, 0b10001, 0b11111, 0b00100, 0b00100],
    [0b11111, 0b00001, 0b00010, 0b00100, 0b01000],
    [0b00100, 0b01010, 0b10001, 0b00000, 0b11111],
    [0b10000, 0b10000, 0b11110, 0b10010, 0b11110],
    [0b01110, 0b10001, 0b00100, 0b00100, 0b00100],
    [0b10101, 0b10101, 0b00000, 0b10101, 0b10101],
    [0b11111, 0b10000, 0b11100, 0b10000, 0b10000],
    [0b00001, 0b00011, 0b00101, 0b01001, 0b10001],
    [0b10010, 0b10010, 0b11111, 0b00010, 0b00010],
    [0b01010, 0b00000, 0b10001, 0b01010, 0b00100],
];

/// A repeatable random number in 0..1 for a particle and a purpose.
fn random(index: u32, salt: u32) -> f32 {
    let mut x = index.wrapping_mul(0x9E37_79B1) ^ salt.wrapping_mul(0x85EB_CA77);
    x ^= x >> 15;
    x = x.wrapping_mul(0xC2B2_AE3D);
    x ^= x >> 13;
    (x & 0xFFFF) as f32 / 65536.0
}

/// One particle's place in its own loop, 0..1, each starting somewhere else.
fn phase(index: u32, seconds_per_loop: f32, t: f32) -> f32 {
    let spread = 0.75 + random(index, 7) * 0.5;
    (t * spread / seconds_per_loop + random(index, 1)).fract()
}

/// Fades in over the start of a loop and out over the end, so nothing pops.
fn fade(phase: f32) -> f32 {
    (phase * 6.0).min((1.0 - phase) * 6.0).clamp(0.0, 1.0)
}

fn dot(frame: &mut Frame, x: f32, y: f32, size: f32, color: Color, opacity: f32) {
    frame.fill_rectangle(Point::new(x.round(), y.round()), Size::new(size, size), theme::alpha(color, opacity));
}

fn rune(frame: &mut Frame, which: u32, x: f32, y: f32, pixel: f32, color: Color, opacity: f32) {
    let color = theme::alpha(color, opacity);
    let (x, y) = (x.round(), y.round());
    for (row, bits) in RUNES[which as usize % RUNES.len()].iter().enumerate() {
        for column in 0..5 {
            if bits & (0b10000 >> column) != 0 {
                frame.fill_rectangle(
                    Point::new(x + column as f32 * pixel, y + row as f32 * pixel),
                    Size::new(pixel, pixel),
                    color,
                );
            }
        }
    }
}

/// How many particles this much room gets, so a small preview isn't a blizzard.
fn count(area: f32, per_particle: f32, most: u32, amount: f32) -> u32 {
    (((area / per_particle) as u32).clamp(12, most) as f32 * amount) as u32
}

/// Runes drifting in from all around to the middle, like at an enchanting table.
fn enchant(frame: &mut Frame, size: Size, t: f32, amount: f32) {
    let palette = theme::palette();
    let middle = Point::new(size.width / 2.0, size.height / 2.0);
    for i in 0..count(size.width * size.height, 16_000.0, 48, amount) {
        let at = phase(i, 7.0, t);
        let (from_x, from_y) = (random(i, 2) * size.width, random(i, 3) * size.height);
        // Accelerating, and curving on the way in.
        let pull = at * at;
        let swing = (at * TAU).sin() * 18.0 * (random(i, 4) - 0.5);
        let x = from_x + (middle.x - from_x) * pull + swing;
        let y = from_y + (middle.y - from_y) * pull - at * 14.0;
        let color = if i % 3 == 0 { palette.ramp[0] } else { palette.accent };
        let which = i + (t / 2.0 + random(i, 5)) as u32;
        rune(frame, which, x, y, 2.0, color, fade(at) * 0.45);
    }
}

/// Falling columns of glyphs, a bright head and a fading tail.
fn rain(frame: &mut Frame, size: Size, t: f32, amount: f32) {
    const CELL: f32 = 14.0;
    const TAIL: u32 = 12;
    let palette = theme::palette();
    let (columns, rows) = ((size.width / CELL) as u32 + 1, size.height / CELL);
    for column in 0..columns {
        // Most columns are empty at any time, or it stops being a background.
        if random(column, 11) > 0.42 * amount {
            continue;
        }
        let speed = 3.0 + random(column, 12) * 5.0;
        let travel = rows + TAIL as f32;
        let head = ((t * speed + random(column, 13) * travel) % travel).floor();
        for back in 0..TAIL {
            let row = head - back as f32;
            if row < 0.0 || row >= rows {
                continue;
            }
            // A glyph stays put as the head passes, and flickers now and then.
            let which = (column * 31 + row as u32 * 17) + (t * (0.4 + random(column, 14))) as u32;
            let (color, opacity) = if back == 0 {
                (palette.text, 0.42)
            } else {
                (palette.accent, 0.30 * (1.0 - back as f32 / TAIL as f32))
            };
            rune(frame, which, column as f32 * CELL + 2.0, row * CELL + 2.0, 2.0, color, opacity);
        }
    }
}

/// Sparks rising and swaying, cooling as they go.
fn embers(frame: &mut Frame, size: Size, t: f32, amount: f32) {
    let palette = theme::palette();
    for i in 0..count(size.width * size.height, 7_000.0, 110, amount) {
        let at = phase(i, 9.0, t);
        let sway = (t * (0.5 + random(i, 2)) + random(i, 3) * TAU).sin() * 14.0;
        let x = random(i, 4) * size.width + sway;
        let y = size.height * (1.05 - at * 1.1);
        let color = palette.ramp[(i % 4) as usize];
        let pixel = if i % 4 == 0 { 4.0 } else { 3.0 };
        dot(frame, x, y, pixel, color, fade(at) * (1.0 - at * 0.5) * 0.7);
    }
}

/// Specks spiralling into the middle, like a nether or end portal's.
fn portal(frame: &mut Frame, size: Size, t: f32, amount: f32) {
    let palette = theme::palette();
    let middle = Point::new(size.width / 2.0, size.height / 2.0);
    let reach = size.width.max(size.height) * 0.62;
    for i in 0..count(size.width * size.height, 6_000.0, 130, amount) {
        let at = phase(i, 8.0, t);
        let angle = random(i, 2) * TAU + at * 2.4;
        let radius = reach * (0.15 + random(i, 3) * 0.85) * (1.0 - at);
        let x = middle.x + angle.cos() * radius;
        // Flattened a little, a disc rather than a ball.
        let y = middle.y + angle.sin() * radius * 0.75;
        let color = if i % 2 == 0 { palette.magenta } else { palette.accent };
        dot(frame, x, y, if i % 4 == 0 { 4.0 } else { 3.0 }, color, fade(at) * 0.65);
    }
}

/// Falling flakes; the small ones fall slower, which reads as further away.
fn snow(frame: &mut Frame, size: Size, t: f32, amount: f32) {
    let palette = theme::palette();
    for i in 0..count(size.width * size.height, 6_000.0, 130, amount) {
        let near = i % 4 == 0;
        let at = phase(i, if near { 8.0 } else { 15.0 }, t);
        let sway = (t * 0.7 + random(i, 2) * TAU).sin() * 16.0;
        let x = random(i, 3) * size.width + sway;
        let y = at * size.height * 1.1 - size.height * 0.05;
        dot(frame, x, y, if near { 4.0 } else { 3.0 }, palette.text, fade(at) * if near { 0.55 } else { 0.32 });
    }
}

/// Petals blowing down and across, tumbling as they fall.
fn petals(frame: &mut Frame, size: Size, t: f32, amount: f32) {
    let palette = theme::palette();
    for i in 0..count(size.width * size.height, 12_000.0, 70, amount) {
        let at = phase(i, 12.0, t);
        let drift = at * size.width * 0.3;
        let x = (random(i, 2) * size.width * 1.3 - size.width * 0.3) + drift + (t + random(i, 3) * TAU).sin() * 10.0;
        let y = at * size.height * 1.1 - size.height * 0.05;
        // Narrowing and widening is what makes a flat rectangle look like it turns.
        let turn = (t * (1.0 + random(i, 4)) + random(i, 5) * TAU).cos().abs();
        let color = if i % 3 == 0 { palette.ramp[2] } else { palette.accent };
        frame.fill_rectangle(
            Point::new(x.round(), y.round()),
            Size::new((2.0 + 6.0 * turn).round(), 4.0),
            theme::alpha(color, fade(at) * 0.6),
        );
    }
}

fn polygon(points: &[(f32, f32)]) -> Path {
    Path::new(|path| {
        let mut points = points.iter().map(|&(x, y)| Point::new(x, y));
        if let Some(first) = points.next() {
            path.move_to(first);
        }
        points.for_each(|point| path.line_to(point));
        path.close();
    })
}

/// The faces of a turning cube a camera sees, around its middle: each as its
/// corners and how brightly it's lit, the top always and the sides facing front.
fn cube_faces(side: f32, yaw: f32) -> Vec<([(f32, f32); 4], f32)> {
    const TILT: f32 = 0.6;
    let half = side / 2.0;
    let corner = |k: f32, up: f32| {
        let angle = yaw + std::f32::consts::FRAC_PI_4 + k * std::f32::consts::FRAC_PI_2;
        let (x, z) = (angle.cos() * half * std::f32::consts::SQRT_2, angle.sin() * half * std::f32::consts::SQRT_2);
        (x, z * TILT.sin() - up * half * TILT.cos())
    };
    let mut faces = vec![([corner(0.0, 1.0), corner(1.0, 1.0), corner(2.0, 1.0), corner(3.0, 1.0)], 1.0)];
    for k in 0..4 {
        let k = k as f32;
        let facing = yaw + std::f32::consts::FRAC_PI_2 + k * std::f32::consts::FRAC_PI_2;
        if facing.sin() > 0.0 {
            faces.push(([corner(k, 1.0), corner(k + 1.0, 1.0), corner(k + 1.0, -1.0), corner(k, -1.0)], 0.4 + 0.3 * (facing.cos() + 1.0) / 2.0));
        }
    }
    faces
}

/// Small cubes turning as they rise through a nebula.
fn voxels(frame: &mut Frame, size: Size, t: f32, amount: f32) {
    let palette = theme::palette();
    for i in 0..count(size.width * size.height, 22_000.0, 30, amount) {
        let at = phase(i, 20.0, t);
        let depth = 0.35 + random(i, 2) * 0.65;
        let side = 8.0 + 20.0 * depth;
        let x = random(i, 3) * size.width + (t * 0.3 + random(i, 4) * TAU).sin() * 18.0;
        let y = size.height * (1.1 - at * 1.2);
        let yaw = t * (0.15 + random(i, 5) * 0.4) + random(i, 6) * TAU;
        let color = palette.ramp[i as usize % 4];
        let opacity = fade(at) * (0.2 + 0.4 * depth);
        for (corners, light) in cube_faces(side, yaw) {
            let corners = corners.map(|(cx, cy)| (x + cx, y + cy));
            let lit = Color { r: color.r * light, g: color.g * light, b: color.b * light, a: opacity };
            let shape = polygon(&corners);
            frame.fill(&shape, lit);
            frame.stroke(&shape, Stroke::default().with_color(theme::alpha(color, opacity * 0.9)).with_width(1.0));
        }
    }
}

/// A bolt's points from the top downward, the same for the same `seed`.
fn bolt_points(seed: u32, size: Size) -> Vec<(f32, f32)> {
    const STEPS: u32 = 14;
    let mut x = (0.1 + random(seed, 20) * 0.8) * size.width;
    let reach = size.height * (0.5 + random(seed, 21) * 0.4);
    (0..=STEPS)
        .map(|step| {
            x += (random(seed, 100 + step) - 0.5) * size.width * 0.07;
            (x, reach * step as f32 / STEPS as f32 - 4.0)
        })
        .collect()
}

/// How bright a strike is, `age` seconds in: a flash, a dip, a second flash, gone.
/// A stroke rather than the whole page lighting up, so it never flashes the window.
fn strike_brightness(age: f32) -> f32 {
    const LIFE: f32 = 0.5;
    if !(0.0..LIFE).contains(&age) {
        return 0.0;
    }
    let flicker = if age < 0.07 { 1.0 } else if age < 0.14 { 0.3 } else if age < 0.24 { 0.85 } else { 0.5 };
    flicker * (1.0 - age / LIFE)
}

/// Now and then a bolt forks through a nebula.
fn lightning(frame: &mut Frame, size: Size, t: f32, amount: f32) {
    let palette = theme::palette();
    let line = |points: &[(f32, f32)]| {
        Path::new(|path| {
            let mut points = points.iter().map(|&(x, y)| Point::new(x, y));
            if let Some(first) = points.next() {
                path.move_to(first);
            }
            points.for_each(|point| path.line_to(point));
        })
    };
    for lane in 0..((3.0 * amount).ceil() as u32).max(1) {
        let period = 3.5 + random(lane, 30) * 3.0;
        let clock = t + random(lane, 31) * period;
        let bright = strike_brightness(clock % period);
        if bright == 0.0 {
            continue;
        }
        let seed = lane * 97 + (clock / period) as u32 * 13;
        let main = bolt_points(seed, size);
        let fork_at = 4 + (random(seed, 22) * 6.0) as usize;
        let lean = if random(seed, 23) > 0.5 { 1.0 } else { -1.0 };
        let fork: Vec<(f32, f32)> = main[fork_at..]
            .iter()
            .take(6)
            .enumerate()
            .map(|(i, &(x, y))| (x + lean * i as f32 * size.width * 0.018, y))
            .collect();
        for (points, weight) in [(&main, 1.0), (&fork, 0.55)] {
            let path = line(points);
            for (width, color, opacity) in [(7.0, palette.accent, 0.16), (3.0, palette.accent, 0.4), (1.2, palette.text, 0.9)] {
                frame.stroke(&path, Stroke::default().with_color(theme::alpha(color, opacity * bright * weight)).with_width(width * weight));
            }
        }
    }
}

/// A perspective floor rolling toward the viewer, under stars and a nebula.
fn grid(frame: &mut Frame, size: Size, t: f32, amount: f32) {
    let palette = theme::palette();
    let horizon = size.height * 0.56;
    let middle = size.width / 2.0;
    frame.fill_rectangle(Point::new(0.0, horizon), Size::new(size.width, size.height - horizon), theme::alpha(palette.ink, 0.72));
    let lines = ((14.0 * amount) as u32).max(4);
    for i in 0..lines {
        let along = (i as f32 / lines as f32 + t * 0.07).fract();
        let y = horizon + (size.height - horizon) * along * along * along;
        frame.fill_rectangle(Point::new(0.0, y), Size::new(size.width, 1.5), theme::alpha(palette.accent, 0.08 + along * 0.4));
    }
    let spokes = ((12.0 * amount) as i32).max(3);
    let spacing = size.width * 1.6 / spokes as f32;
    let ramp = gradient::Linear::new(Point::new(0.0, horizon), Point::new(0.0, size.height))
        .add_stop(0.0, theme::alpha(palette.magenta, 0.06))
        .add_stop(1.0, theme::alpha(palette.magenta, 0.5));
    for spoke in -spokes..=spokes {
        let path = Path::line(
            Point::new(middle + spoke as f32 * spacing * 0.08, horizon),
            Point::new(middle + spoke as f32 * spacing, size.height),
        );
        frame.stroke(&path, Stroke { style: Style::Gradient(Gradient::Linear(ramp)), width: 1.2, ..Stroke::default() });
    }
    frame.fill_rectangle(Point::new(0.0, horizon - 1.0), Size::new(size.width, 2.0), theme::alpha(palette.accent, 0.5));
}

/// An effect as a canvas. `live` false freezes it, without giving up its place.
pub struct Backdrop {
    pub effect: Effect,
    pub tuning: Tuning,
    pub live: bool,
}

/// How much of an effect, as percentages of what it does by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tuning {
    pub amount: u32,
    pub speed: u32,
}

impl Tuning {
    pub const RANGE: std::ops::RangeInclusive<u32> = 25..=200;
}

impl Default for Tuning {
    fn default() -> Self {
        Self { amount: 100, speed: 100 }
    }
}

#[derive(Default)]
pub struct Clock {
    last: Option<Instant>,
    seconds: f32,
}

impl Clock {
    /// Moves on to the frame at `now`, and says whether to keep asking for frames.
    /// Added up, not measured from a start time, so time spent frozen doesn't count
    /// and particles carry on from where they stopped.
    pub fn advance(&mut self, now: Instant, live: bool, tuning: Tuning) -> bool {
        if !live {
            self.last = None;
            return false;
        }
        if let Some(last) = self.last {
            self.seconds += now.duration_since(last).min(FRAME * 3).as_secs_f32() * tuning.speed as f32 / 100.0;
        }
        self.last = Some(now);
        true
    }

    pub fn seconds(&self) -> f32 {
        self.seconds
    }
}

impl<Message> canvas::Program<Message> for Backdrop {
    type State = Clock;

    fn update(&self, clock: &mut Clock, event: &iced::Event, _: Rectangle, _: mouse::Cursor) -> Option<canvas::Action<Message>> {
        let iced::Event::Window(iced::window::Event::RedrawRequested(now)) = event else {
            return None;
        };
        clock
            .advance(*now, self.live && self.effect != Effect::Off, self.tuning)
            .then(|| canvas::Action::request_redraw_at(*now + FRAME))
    }

    fn draw(&self, clock: &Clock, renderer: &Renderer, _: &Theme, bounds: Rectangle, _: mouse::Cursor) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        // A fixed head start, so nothing begins with an empty sky.
        let t = clock.seconds + 60.0;
        let size = bounds.size();
        let amount = self.tuning.amount as f32 / 100.0;
        match self.effect {
            Effect::Enchant => enchant(&mut frame, size, t, amount),
            Effect::Rain => rain(&mut frame, size, t, amount),
            Effect::Embers => embers(&mut frame, size, t, amount),
            Effect::Portal => portal(&mut frame, size, t, amount),
            Effect::Snow => snow(&mut frame, size, t, amount),
            Effect::Petals => petals(&mut frame, size, t, amount),
            Effect::Voxels => voxels(&mut frame, size, t, amount),
            Effect::Lightning => lightning(&mut frame, size, t, amount),
            Effect::Grid => grid(&mut frame, size, t, amount),
            Effect::Chrome => {}
            Effect::Off => {}
        }
        vec![frame.into_geometry()]
    }
}

pub fn view<'a, Message: 'a>(effect: Effect, tuning: Tuning, live: bool, height: impl Into<Length> + Copy) -> Element<'a, Message> {
    use crate::sky::Kind;
    let sky = match effect {
        Effect::Voxels | Effect::Lightning | Effect::Grid => Some(Kind::Nebula),
        Effect::Chrome => Some(Kind::Chrome),
        _ => None,
    };
    let particles = iced::widget::Canvas::new(Backdrop { effect, tuning, live }).width(Length::Fill).height(height);
    match sky {
        // In a stack so the particles land in a later layer, over the clouds.
        Some(Kind::Chrome) => crate::sky::view(Kind::Chrome, tuning, live, height),
        Some(kind) => iced::widget::stack![crate::sky::view(kind, tuning, live, height), particles].into(),
        None => particles.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cube_always_shows_its_top_and_a_side_or_two() {
        for step in 0..360 {
            let faces = cube_faces(10.0, (step as f32).to_radians());
            assert!((2..=3).contains(&faces.len()), "{step}: {}", faces.len());
            assert_eq!(faces[0].1, 1.0, "the top comes first and is brightest");
            assert!(faces[1..].iter().all(|&(_, light)| light < 1.0));
        }
    }

    #[test]
    fn strikes_are_brief_and_bolts_repeatable() {
        assert_eq!(strike_brightness(-0.1), 0.0);
        assert!(strike_brightness(0.01) > 0.9);
        assert_eq!(strike_brightness(0.5), 0.0);
        let size = Size::new(800.0, 600.0);
        let bolt = bolt_points(7, size);
        assert_eq!(bolt, bolt_points(7, size));
        assert!(bolt.len() > 9, "the fork starts at step 9 at the latest");
        assert!(bolt.iter().all(|&(_, y)| y <= size.height));
    }

    #[test]
    fn particle_numbers_stay_in_range() {
        for index in 0..1000 {
            for salt in 0..16 {
                assert!((0.0..1.0).contains(&random(index, salt)));
            }
            let at = phase(index, 7.0, 12_345.6);
            assert!((0.0..1.0).contains(&at));
            assert!((0.0..=1.0).contains(&fade(at)));
        }
    }
}
