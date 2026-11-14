//! A 3D player model, drawn with wgpu through iced's shader widget.
//!
//! iced has no 3D of its own, so this owns a small render pipeline: the
//! player's cuboids are built as geometry, textured with the account's skin,
//! and drawn with a depth buffer into the widget's bounds.
//!
//! Signed in, this is a single textured pass.
//!
//! Signed out, the figure becomes a hollow rainbow badge: the outline of its
//! *silhouette*, like a cast shadow, with nothing inside.
//!
//! This is done as a two-pass screen-space effect rather than with geometry.
//! The model is rendered into an offscreen coverage mask, then a fullscreen
//! pass walks that mask: a pixel outside the figure that has any covered
//! pixel within the outline radius is part of the ring, and everything else
//! is discarded.
//!
//! The obvious approach — an inverted hull, the model re-drawn expanded with
//! front faces culled — was tried first and cannot produce this. It outlines
//! every cuboid separately, so arms get their own rings inside the body's,
//! and where a limb overlaps the torso the two expanded shells z-fight. A
//! silhouette has no internal edges by definition, and only a mask over the
//! union of all parts gives that.
//!
//! Each cuboid expands about its own centre rather than the model's, or
//! limbs would splay outwards as the outline grew.

use iced::widget::shader;
use iced::wgpu;
use iced::{mouse, Rectangle};
use nexo_core::skin::Rgba;
use nexo_core::SkinModel;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Where the model rests when nothing is interacting with it: turned to its
/// right and tipped forward, so it reads as looking down and to the viewer's
/// bottom-left rather than staring straight out.
const REST_YAW: f32 = -0.62;
const REST_PITCH: f32 = 0.20;

/// Turned away from the viewer, so the cape is what's facing you. Half a turn
/// from the front pose, keeping the same slight lean.
const BACK_YAW: f32 = REST_YAW + std::f32::consts::PI;

/// How long a dragged pose is left alone before it eases back.
const HOLD: Duration = Duration::from_secs(5);

/// Fraction of the remaining distance closed per second while easing back.
/// Exponential rather than linear so it arrives gently instead of stopping
/// dead.
const RETURN_RATE: f32 = 2.2;

/// How fast the head turns toward the cursor, the same way.
const LOOK_RATE: f32 = 5.0;
/// How far the head turns from the body before the neck would look broken.
const MAX_LOOK_YAW: f32 = 1.0;
const MAX_LOOK_PITCH: f32 = 0.55;

/// Below this, the pose is close enough to rest to stop animating — without
/// it, exponential decay never quite arrives and redraws never stop.
const SETTLED: f32 = 0.001;

const DRAG_SENSITIVITY: f32 = 0.011;
/// Keeps the model from being tipped past vertical, where it reads as broken.
const MAX_PITCH: f32 = 0.9;

/// Outline thickness, in physical pixels. Screen-space now, not model
/// units — the ring is produced by dilating a mask, not by expanding
/// geometry.
const OUTLINE_PIXELS: f32 = 4.0;

/// Single channel is all the coverage mask needs.
const MASK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;

/// What the figure is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stance {
    /// Breathing, swaying its arms and following the cursor with its head.
    /// Can be dragged round, and eases back afterwards.
    Idle,
    /// One of [`POSES`] (any number picks one), held still, for a tile of
    /// the skin library.
    Showcase(u64),
    /// A still pose from behind, for showing a cape.
    Back,
}

/// How each limb is turned, in radians, plus how far the whole figure floats.
/// Swings are positive forwards and spreads positive away from the body,
/// whichever side the limb is on.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Rig {
    head_yaw: f32,
    head_pitch: f32,
    right_arm: (f32, f32),
    left_arm: (f32, f32),
    right_leg: f32,
    left_leg: f32,
    /// How far the cape is lifted off the back.
    cape: f32,
    bob: f32,
}

impl Rig {
    /// Minecraft's own idle, a little livelier: the arms drift out and back,
    /// and the figure rises and sinks as if breathing. `t` is seconds.
    fn idle(t: f32) -> Self {
        Self {
            right_arm: ((t * 1.34).sin() * 0.16, 0.09 + (t * 1.8).cos() * 0.08),
            left_arm: (-(t * 1.34).sin() * 0.16, 0.09 + (t * 1.8).cos() * 0.08),
            cape: 0.1 + (t * 1.3).sin() * 0.06,
            bob: (t * 1.8).sin() * 0.9,
            ..Self::default()
        }
    }

    /// The same pose the other way round.
    fn mirrored(self) -> Self {
        Self {
            head_yaw: -self.head_yaw,
            right_arm: self.left_arm,
            left_arm: self.right_arm,
            right_leg: self.left_leg,
            left_leg: self.right_leg,
            ..self
        }
    }

    fn back() -> Self {
        Self { right_arm: (0.0, 0.06), left_arm: (0.0, 0.06), cape: 0.14, ..Self::default() }
    }
}

/// Still poses for the library's tiles: how far the body is turned, then the
/// rig. Each also appears mirrored, so a page of skins isn't a row of twins.
const POSES: [(f32, Rig); 9] = {
    const fn pose(yaw: f32, head: (f32, f32), right_arm: (f32, f32), left_arm: (f32, f32), legs: (f32, f32), cape: f32) -> (f32, Rig) {
        (yaw, Rig { head_yaw: head.0, head_pitch: head.1, right_arm, left_arm, right_leg: legs.0, left_leg: legs.1, cape, bob: 0.0 })
    }
    [
        // Waving.
        pose(-0.45, (0.25, -0.1), (2.3, 0.7), (-0.35, 0.12), (0.45, -0.4), 0.25),
        // Cheering, both arms up.
        pose(-0.3, (0.0, -0.3), (2.9, 0.45), (2.9, 0.45), (0.1, -0.1), 0.2),
        // Mid-stride.
        pose(-0.6, (0.1, 0.05), (-0.6, 0.05), (0.6, 0.05), (0.6, -0.6), 0.3),
        // Pointing ahead.
        pose(-0.5, (0.15, 0.05), (1.5, 0.1), (-0.1, 0.08), (0.2, -0.15), 0.15),
        // Arms out wide.
        pose(-0.35, (0.0, 0.0), (0.1, 1.35), (0.1, 1.35), (0.0, 0.0), 0.1),
        // Standing easy, arms a little out, looking down.
        pose(-0.55, (-0.2, 0.15), (0.3, 0.45), (0.3, 0.45), (0.15, -0.1), 0.1),
        // Running.
        pose(-0.7, (0.0, 0.15), (1.0, 0.05), (-1.0, 0.05), (-0.9, 0.9), 0.55),
        // Thinking: a hand up by the chin, looking up.
        pose(-0.4, (0.3, -0.35), (1.9, -0.3), (0.2, 0.05), (0.05, 0.0), 0.1),
        // Kicking.
        pose(-0.5, (0.0, 0.2), (0.4, 0.2), (-0.4, 0.2), (1.1, -0.1), 0.3),
    ]
};

/// Drawn once per launch, so the library's tiles stand differently each time
/// the launcher starts but don't change pose while it's running.
static SESSION: std::sync::LazyLock<u64> = std::sync::LazyLock::new(|| {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
    slot(&nanos.to_string())
});

/// Pose `n` of the eighteen, mirrored for the second nine.
fn showcase(n: u64) -> (f32, Rig) {
    let (yaw, rig) = POSES[(n % POSES.len() as u64) as usize];
    if (n / POSES.len() as u64) % 2 == 1 { (-yaw, rig.mirrored()) } else { (yaw, rig) }
}

/// How long the idle figure looks at the cursor, and then away, in seconds.
const ATTENTION: std::ops::Range<f32> = 3.0..5.0;
const DISTRACTION: std::ops::Range<f32> = 2.5..6.0;

/// A number in `range` from `n`, the same every time for the same `n`: enough
/// to keep the figure's timing from looking like a metronome.
fn vary(n: u32, range: std::ops::Range<f32>) -> f32 {
    let noise = ((n as f32 * 12.9898).sin() * 43_758.547).fract().abs();
    range.start + (range.end - range.start) * noise
}

/// Where the head drifts while not looking at the cursor: a slow glance
/// round, never quite still.
fn wander(t: f32) -> (f32, f32) {
    ((t * 0.45).sin() * 0.35 + (t * 0.17).sin() * 0.15, (t * 0.31).sin() * 0.12 - 0.05)
}

/// Interaction state. Lives in the widget tree, so a redraw doesn't reset the
/// pose the user dragged to.
#[derive(Debug)]
pub struct Pose {
    yaw: f32,
    pitch: f32,
    /// The head's turn toward the cursor, eased so it follows rather than snaps.
    look: (f32, f32),
    /// Where the cursor last was, anywhere in the window.
    cursor: Option<iced::Point>,
    /// Whether the head is following the cursor right now, until when, and
    /// how many times that has changed.
    attending: bool,
    switch_at: Option<Instant>,
    switches: u32,
    /// Cursor position the current drag was last sampled at.
    drag_from: Option<iced::Point>,
    /// When the drag ended, which starts the hold before easing back.
    released_at: Option<Instant>,
    last_tick: Option<Instant>,
    /// Time base for the drifting rainbow and the idle motion. Set on the
    /// first frame rather than at construction, so both start when the
    /// widget appears.
    started: Option<Instant>,
    /// A cape shown from behind: its physics, whether the cursor is over it
    /// (which blows a breeze at it), and its last step.
    cape: Option<crate::cape_sim::CapeSim>,
    hovered: bool,
    cape_tick: Option<Instant>,
}

impl Default for Pose {
    fn default() -> Self {
        Self {
            yaw: REST_YAW,
            pitch: REST_PITCH,
            look: (0.0, 0.0),
            cursor: None,
            attending: false,
            switch_at: None,
            switches: 0,
            drag_from: None,
            released_at: None,
            last_tick: None,
            started: None,
            cape: None,
            hovered: false,
            cape_tick: None,
        }
    }
}

impl Pose {
    fn at_rest(&self) -> bool {
        (self.yaw - REST_YAW).abs() < SETTLED && (self.pitch - REST_PITCH).abs() < SETTLED
    }

    /// Eases toward the rest pose, framerate-independently.
    fn ease_back(&mut self, dt: f32) {
        let t = 1.0 - (-RETURN_RATE * dt).exp();
        self.yaw += (REST_YAW - self.yaw) * t;
        self.pitch += (REST_PITCH - self.pitch) * t;
        if self.at_rest() {
            self.yaw = REST_YAW;
            self.pitch = REST_PITCH;
            self.released_at = None;
        }
    }

    /// Turns the head a step toward the cursor.
    fn ease_look(&mut self, target: (f32, f32), dt: f32) {
        let t = 1.0 - (-LOOK_RATE * dt).exp();
        self.look.0 += (target.0 - self.look.0) * t;
        self.look.1 += (target.1 - self.look.1) * t;
    }
}

/// The head turn, relative to the body, that faces `cursor`. The head is
/// treated as sitting a little way in front of the screen, so a cursor right
/// beside it turns it fully and one across the window less than that.
fn look_at(cursor: iced::Point, bounds: Rectangle, yaw: f32, pitch: f32) -> (f32, f32) {
    // The head's centre on screen, from the same projection the figure is drawn with.
    let clip = glam::Mat4::from_cols_array_2d(&mvp(yaw, pitch, 0.0, bounds)) * glam::Vec4::new(0.0, 28.0, 0.0, 1.0);
    let ndc = clip.truncate() / clip.w.max(f32::EPSILON);
    let head = iced::Point::new(
        bounds.x + (ndc.x + 1.0) / 2.0 * bounds.width,
        bounds.y + (1.0 - ndc.y) / 2.0 * bounds.height,
    );
    let depth = bounds.height.max(1.0) * 0.9;
    let toward_yaw = ((cursor.x - head.x) / depth).atan();
    let toward_pitch = ((cursor.y - head.y) / depth).atan();
    (
        shortest_angle(toward_yaw - yaw).clamp(-MAX_LOOK_YAW, MAX_LOOK_YAW),
        (toward_pitch - pitch).clamp(-MAX_LOOK_PITCH, MAX_LOOK_PITCH),
    )
}

/// Wraps an angle to (-pi, pi].
fn shortest_angle(mut radians: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    while radians > PI {
        radians -= TAU;
    }
    while radians <= -PI {
        radians += TAU;
    }
    radians
}

/// The widget program. Cheap to construct each `view`; the GPU resources live
/// in [`ModelPipeline`], which iced keeps between frames.
#[derive(Debug)]
pub struct SkinViewer {
    skin: Arc<Rgba>,
    cape: Option<Arc<Rgba>>,
    model: SkinModel,
    /// Changes when the textures change, so they're only uploaded then rather
    /// than every frame.
    key: u64,
    /// Which set of GPU buffers this viewer draws from. iced keeps one
    /// pipeline for every viewer on screen, and buffer writes all land before
    /// any of them draws, so viewers sharing buffers would all show the last.
    slot: u64,
    stance: Stance,
    /// Draw the rainbow border. Only while signed out: it marks the
    /// placeholder figure, and once a real skin is shown it would just
    /// obscure it — and animating it costs a redraw every frame.
    outlined: bool,
    /// Whether it may keep animating. False while the window is unfocused,
    /// so a launcher left behind a game stops repainting.
    live: bool,
}

impl SkinViewer {
    pub fn new(skin: Arc<Rgba>, cape: Option<Arc<Rgba>>, model: SkinModel, key: u64, slot: u64, stance: Stance) -> Self {
        Self { skin, cape, model, key, slot, stance, outlined: false, live: false }
    }

    /// The rainbow border of the signed-out placeholder.
    pub fn outlined(mut self, outlined: bool) -> Self {
        self.outlined = outlined;
        self
    }

    /// Lets an idle figure move. Off while the window is unfocused, and with
    /// reduced motion.
    pub fn live(mut self, live: bool) -> Self {
        self.live = live;
        self
    }

    /// A cape from behind moves while hovered, and settles after.
    fn cape_moves(&self) -> bool {
        self.stance == Stance::Back && self.cape.is_some() && self.live && !crate::motion::reduced()
    }

    fn idle_moves(&self) -> bool {
        self.stance == Stance::Idle && self.live && !crate::motion::reduced()
    }
}

/// A slot number for a viewer, from anything that names it uniquely.
pub fn slot(name: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut hasher);
    hasher.finish()
}

impl<Message> shader::Program<Message> for SkinViewer {
    type State = Pose;
    type Primitive = Scene;

    fn update(
        &self,
        state: &mut Self::State,
        event: &iced::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<shader::Action<Message>> {
        use iced::mouse::{Button, Event as Mouse};
        use iced::window::Event as Window;
        use iced::Event as E;

        if self.stance == Stance::Back {
            return self.update_cape(state, event, bounds, cursor);
        }
        // The still poses don't respond to anything.
        if self.stance != Stance::Idle {
            return None;
        }

        match event {
            E::Mouse(Mouse::ButtonPressed(Button::Left)) => {
                let position = cursor.position_over(bounds)?;
                state.drag_from = Some(position);
                state.released_at = None;
                Some(shader::Action::request_redraw())
            }

            E::Mouse(Mouse::CursorMoved { .. }) => {
                state.cursor = cursor.position();
                let Some(from) = state.drag_from else {
                    // The head follows on the next frame, which an idle figure
                    // asks for anyway.
                    return None;
                };
                // Position is read from the cursor rather than the event so a
                // drag that leaves the widget keeps tracking instead of
                // sticking.
                let to = cursor.position()?;
                state.yaw += (to.x - from.x) * DRAG_SENSITIVITY;
                state.pitch =
                    (state.pitch + (to.y - from.y) * DRAG_SENSITIVITY).clamp(-MAX_PITCH, MAX_PITCH);
                state.drag_from = Some(to);
                Some(shader::Action::request_redraw())
            }

            E::Mouse(Mouse::CursorLeft) => {
                state.cursor = None;
                None
            }

            E::Mouse(Mouse::ButtonReleased(Button::Left)) => {
                state.drag_from.take()?;
                // Spun round several times, it would unwind every turn on its way back.
                state.yaw = REST_YAW + shortest_angle(state.yaw - REST_YAW);
                let now = Instant::now();
                state.released_at = Some(now);
                // Nothing to draw until the hold expires, so ask to be woken
                // then instead of animating through it.
                Some(shader::Action::request_redraw_at(now + HOLD))
            }

            E::Window(Window::RedrawRequested(now)) => {
                let previous = state.last_tick.replace(*now);
                state.started.get_or_insert(*now);
                let dt = previous
                    .map(|previous| now.duration_since(previous).as_secs_f32())
                    // First animated frame has no previous tick to measure.
                    .unwrap_or(1.0 / 60.0)
                    .min(0.1);

                // Breathing, the arms and the outline all move continuously,
                // so they need a frame every time whatever the pose is doing.
                let animating = (self.outlined || self.idle_moves()) && self.live;
                if !self.idle_moves() {
                    // Still faces forward, so coming back doesn't snap to an old glance.
                    state.look = (0.0, 0.0);
                    state.switch_at = None;
                }
                if self.idle_moves() {
                    // Looks at the cursor a while, then away a while, and back.
                    let due = *state.switch_at.get_or_insert(*now + Duration::from_secs_f32(1.5));
                    if *now >= due {
                        state.attending = !state.attending;
                        state.switches = state.switches.wrapping_add(1);
                        let span = vary(state.switches, if state.attending { ATTENTION } else { DISTRACTION });
                        state.switch_at = Some(*now + Duration::from_secs_f32(span));
                    }
                    let time = state.started.map_or(0.0, |start| now.duration_since(start).as_secs_f32());
                    let target = match state.cursor {
                        Some(cursor) if state.attending => look_at(cursor, bounds, state.yaw, state.pitch),
                        _ => wander(time),
                    };
                    state.ease_look(target, dt);
                }

                if state.drag_from.is_some() {
                    return animating.then(shader::Action::request_redraw);
                }

                // Hold a dragged pose before easing away from it.
                if let Some(released_at) = state.released_at
                    && now.duration_since(released_at) < HOLD
                {
                    return Some(if animating {
                        shader::Action::request_redraw()
                    } else {
                        shader::Action::request_redraw_at(released_at + HOLD)
                    });
                }

                if state.at_rest() {
                    return animating.then(shader::Action::request_redraw);
                }

                if crate::motion::reduced() {
                    // Straight back, as everything else does with reduced motion.
                    *state = Pose { started: state.started, cursor: state.cursor, ..Pose::default() };
                    return animating.then(shader::Action::request_redraw);
                }
                state.ease_back(dt);
                Some(shader::Action::request_redraw())
            }

            _ => None,
        }
    }

    fn draw(&self, state: &Self::State, _cursor: mouse::Cursor, bounds: Rectangle) -> Scene {
        let time = state.started.map(|start| start.elapsed().as_secs_f32()).unwrap_or(0.0);
        let (yaw, pitch, rig) = match self.stance {
            Stance::Idle => {
                let mut rig = Rig::default();
                // Still, the head faces where the body does rather than staying
                // wherever the cursor last left it.
                if self.idle_moves() {
                    rig = Rig::idle(time);
                    (rig.head_yaw, rig.head_pitch) = state.look;
                }
                (state.yaw, state.pitch, rig)
            }
            Stance::Showcase(n) => {
                let (yaw, rig) = showcase(slot(&format!("{n}:{}", *SESSION)));
                (yaw, 0.12, rig)
            }
            Stance::Back => (BACK_YAW, REST_PITCH, Rig::back()),
        };
        // A cape that has been blown about keeps the shape its physics left it in.
        let strips = state.cape.as_ref().filter(|_| self.stance == Stance::Back).map(crate::cape_sim::CapeSim::angles);

        Scene {
            mvp: mvp(yaw, pitch, rig.bob, bounds),
            geometry: Arc::new(build_model(self.model, self.cape.is_some(), rig, strips.as_ref())),
            skin: Arc::clone(&self.skin),
            cape: self.cape.clone(),
            key: self.key,
            slot: self.slot,
            outlined: self.outlined,
            time,
        }
    }

    fn mouse_interaction(
        &self,
        state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if self.stance != Stance::Idle {
            mouse::Interaction::default()
        } else if state.drag_from.is_some() {
            mouse::Interaction::Grabbing
        } else if cursor.is_over(bounds) {
            mouse::Interaction::Grab
        } else {
            mouse::Interaction::default()
        }
    }
}

impl SkinViewer {
    /// The Back stance's only motion: the cape, in a breeze while the cursor is over it.
    fn update_cape<Message>(&self, state: &mut Pose, event: &iced::Event, bounds: Rectangle, cursor: mouse::Cursor) -> Option<shader::Action<Message>> {
        use iced::mouse::Event as Mouse;
        use iced::window::Event as Window;
        use iced::Event as E;

        if !self.cape_moves() {
            state.cape_tick = None;
            return None;
        }
        match event {
            E::Mouse(Mouse::CursorMoved { .. }) | E::Mouse(Mouse::CursorLeft) => {
                let over = cursor.is_over(bounds);
                if over == state.hovered {
                    return None;
                }
                state.hovered = over;
                Some(shader::Action::request_redraw())
            }
            E::Window(Window::RedrawRequested(now)) => {
                // Again here: a tile scrolled out from under a still cursor gets no move event.
                state.hovered = cursor.is_over(bounds);
                let previous = state.cape_tick.replace(*now);
                let started = *state.started.get_or_insert(*now);
                let dt = previous.map_or(1.0 / 60.0, |p| now.duration_since(p).as_secs_f32()).min(0.1);
                let wind = if state.hovered { crate::cape_sim::CapeSim::breeze(now.duration_since(started).as_secs_f32()) } else { 0.0 };
                let sim = state.cape.get_or_insert_with(Default::default);
                sim.advance(dt, wind);
                if state.hovered || !sim.settled() {
                    Some(shader::Action::request_redraw())
                } else {
                    // Still: no more frames until the cursor comes back.
                    state.cape_tick = None;
                    None
                }
            }
            _ => None,
        }
    }
}

/// Model-view-projection for the current pose. `bob` lifts the whole figure,
/// in model units.
fn mvp(yaw: f32, pitch: f32, bob: f32, bounds: Rectangle) -> [[f32; 4]; 4] {
    use glam::{Mat4, Vec3};

    let aspect = (bounds.width / bounds.height.max(1.0)).max(0.01);
    // The `directx` projection is the right one for wgpu: its clip space puts
    // depth in 0..1, unlike OpenGL's -1..1, and a mismatch here shows up as
    // everything failing the depth test rather than as an error.
    let projection =
        glam::camera::rh::proj::directx::perspective(30f32.to_radians(), aspect, 1.0, 400.0);
    let view = glam::camera::rh::view::look_at_mat4(
        Vec3::new(0.0, 0.0, 68.0),
        Vec3::ZERO,
        Vec3::new(0.0, 1.0, 0.0),
    );
    // Pitch is applied after yaw so tipping stays relative to the viewer
    // rather than to the model's own turned axis.
    let model = Mat4::from_translation(Vec3::new(0.0, bob, 0.0))
        * Mat4::from_rotation_x(pitch)
        * Mat4::from_rotation_y(yaw)
        // The model is built standing on y=0; recentre it on its waist.
        * Mat4::from_translation(Vec3::new(0.0, -16.0, 0.0));

    (projection * view * model).to_cols_array_2d()
}

// --- geometry -------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 3],
    uv: [f32; 2],
    /// Direction this vertex moves in for the outline hulls — away from its
    /// own cuboid's centre.
    expand: [f32; 3],
    /// Baked face brightness, standing in for lighting.
    shade: f32,
}

#[derive(Debug)]
pub struct Geometry {
    vertices: Vec<Vertex>,
    /// Where the cape's vertices start; everything before is body.
    cape_start: u32,
}

/// Per-face brightness. Flat cuboids look completely flat without it.
const SHADE_TOP: f32 = 1.0;
const SHADE_FRONT: f32 = 1.0;
const SHADE_SIDE: f32 = 0.93;
const SHADE_BACK: f32 = 0.88;
const SHADE_BOTTOM: f32 = 0.82;

/// Appends one cuboid's six faces.
///
/// `uv` is the box's origin in the texture, laid out the way Minecraft does
/// it: a cross unwrap where the top row is top/bottom and the second row is
/// right/front/left/back.
#[allow(clippy::too_many_arguments)]
fn push_box(
    out: &mut Vec<Vertex>,
    centre: [f32; 3],
    size: [f32; 3],
    uv: [f32; 2],
    texture: [f32; 2],
    inflate: f32,
    // Cape textures put the visible design where a body box puts its front.
    flip_faces: bool,
) {
    let (w, h, d) = (size[0], size[1], size[2]);
    let (hx, hy, hz) = (
        w / 2.0 + inflate,
        h / 2.0 + inflate,
        d / 2.0 + inflate,
    );
    let (cx, cy, cz) = (centre[0], centre[1], centre[2]);
    let (tw, th) = (texture[0], texture[1]);
    let (u, v) = (uv[0], uv[1]);

    // Texel rectangles for each face, in the standard unwrap.
    let right = [u, v + d, d, h];
    let front = [u + d, v + d, w, h];
    let left = [u + d + w, v + d, d, h];
    let back = [u + d + w + d, v + d, w, h];
    let top = [u + d, v, w, d];
    // Mirrored: the game lays the bottom out as if seen from above, through
    // the box, which seen from below is a mirror image.
    let bottom = [u + d + w + w, v, -w, d];

    let (front_uv, back_uv) = if flip_faces {
        (back, front)
    } else {
        (front, back)
    };

    // Corners, named by sign on each axis.
    let p = |sx: f32, sy: f32, sz: f32| [cx + sx * hx, cy + sy * hy, cz + sz * hz];
    // Outline direction: away from this box's own centre.
    let e = |sx: f32, sy: f32, sz: f32| {
        let v = glam::Vec3::new(sx, sy, sz).normalize_or_zero();
        [v.x, v.y, v.z]
    };

    let mut face = |corners: [[f32; 3]; 4], signs: [[f32; 3]; 4], rect: [f32; 4], shade: f32| {
        let [ru, rv, rw, rh] = rect;
        // Texture coordinates, normalized. Corners are ordered
        // top-left, bottom-left, bottom-right, top-right.
        let uvs = [
            [ru / tw, rv / th],
            [ru / tw, (rv + rh) / th],
            [(ru + rw) / tw, (rv + rh) / th],
            [(ru + rw) / tw, rv / th],
        ];
        // Two counter-clockwise triangles, so back-face culling keeps the
        // outward side.
        for &i in &[0usize, 1, 2, 0, 2, 3] {
            out.push(Vertex {
                position: corners[i],
                uv: uvs[i],
                expand: e(signs[i][0], signs[i][1], signs[i][2]),
                shade,
            });
        }
    };

    // +Z faces the viewer at rest.
    face(
        [p(-1., 1., 1.), p(-1., -1., 1.), p(1., -1., 1.), p(1., 1., 1.)],
        [[-1., 1., 1.], [-1., -1., 1.], [1., -1., 1.], [1., 1., 1.]],
        front_uv,
        SHADE_FRONT,
    );
    face(
        [p(1., 1., -1.), p(1., -1., -1.), p(-1., -1., -1.), p(-1., 1., -1.)],
        [[1., 1., -1.], [1., -1., -1.], [-1., -1., -1.], [-1., 1., -1.]],
        back_uv,
        SHADE_BACK,
    );
    // The player's right side is -X, which is the viewer's left.
    face(
        [p(-1., 1., -1.), p(-1., -1., -1.), p(-1., -1., 1.), p(-1., 1., 1.)],
        [[-1., 1., -1.], [-1., -1., -1.], [-1., -1., 1.], [-1., 1., 1.]],
        right,
        SHADE_SIDE,
    );
    face(
        [p(1., 1., 1.), p(1., -1., 1.), p(1., -1., -1.), p(1., 1., -1.)],
        [[1., 1., 1.], [1., -1., 1.], [1., -1., -1.], [1., 1., -1.]],
        left,
        SHADE_SIDE,
    );
    face(
        [p(-1., 1., -1.), p(-1., 1., 1.), p(1., 1., 1.), p(1., 1., -1.)],
        [[-1., 1., -1.], [-1., 1., 1.], [1., 1., 1.], [1., 1., -1.]],
        top,
        SHADE_TOP,
    );
    // Starting from the back corner, so the texture's top row is the back
    // edge as in the game; starting at the front showed it upside down.
    face(
        [p(1., -1., -1.), p(1., -1., 1.), p(-1., -1., 1.), p(-1., -1., -1.)],
        [[1., -1., -1.], [1., -1., 1.], [-1., -1., 1.], [-1., -1., -1.]],
        bottom,
        SHADE_BOTTOM,
    );
}

/// Turns the vertices from `from` onwards about `pivot`: the head about the
/// neck, a limb about its shoulder or hip.
fn pose_part(vertices: &mut [Vertex], pivot: [f32; 3], rotation: glam::Mat4) {
    let pivot = glam::Vec3::from(pivot);
    let transform = glam::Mat4::from_translation(pivot) * rotation * glam::Mat4::from_translation(-pivot);
    for vertex in vertices {
        vertex.position = transform.transform_point3(vertex.position.into()).into();
        vertex.expand = rotation.transform_vector3(vertex.expand.into()).into();
    }
}

/// Builds the player model, standing on y = 0, in skin-pixel units, with
/// each part turned as `rig` says.
fn build_model(model: SkinModel, with_cape: bool, rig: Rig, strips: Option<&[f32; crate::cape_sim::LINES - 1]>) -> Geometry {
    use glam::Mat4;
    let mut v = Vec::new();
    let skin = [64.0, 64.0];

    // Slim skins have 3px arms, which also moves where they hang.
    let arm_w = match model {
        SkinModel::Classic => 4.0,
        SkinModel::Slim => 3.0,
    };
    let arm_x = 4.0 + arm_w / 2.0;

    // Each part with its overlay, slightly inflated so it sits just proud of
    // the base. Hair and jacket detail live in the overlay; skipping it is the
    // usual reason a rendered skin looks subtly wrong.
    let part = |v: &mut Vec<Vertex>, centre, size, base, overlay, inflate, pivot, rotation: Mat4| {
        let from = v.len();
        push_box(v, centre, size, base, skin, 0.0, false);
        push_box(v, centre, size, overlay, skin, inflate, false);
        pose_part(&mut v[from..], pivot, rotation);
    };
    // A swing forwards turns the limb's foot towards +Z, which is the
    // negative way round X; a spread turns it away from the body. Spread
    // first: spread after a big swing would push a raised arm inwards.
    let limb = |(swing, spread): (f32, f32), side: f32| Mat4::from_rotation_x(-swing) * Mat4::from_rotation_z(side * spread);

    part(&mut v, [0.0, 28.0, 0.0], [8.0, 8.0, 8.0], [0.0, 0.0], [32.0, 0.0], 0.5, [0.0, 24.0, 0.0],
        Mat4::from_rotation_y(rig.head_yaw) * Mat4::from_rotation_x(rig.head_pitch));
    part(&mut v, [0.0, 18.0, 0.0], [8.0, 12.0, 4.0], [16.0, 16.0], [16.0, 32.0], 0.25, [0.0, 18.0, 0.0], Mat4::IDENTITY);
    // The player's right is -X, so spreading it out turns it the negative way.
    part(&mut v, [-arm_x, 18.0, 0.0], [arm_w, 12.0, 4.0], [40.0, 16.0], [40.0, 32.0], 0.25, [-arm_x, 22.0, 0.0], limb(rig.right_arm, -1.0));
    part(&mut v, [arm_x, 18.0, 0.0], [arm_w, 12.0, 4.0], [32.0, 48.0], [48.0, 48.0], 0.25, [arm_x, 22.0, 0.0], limb(rig.left_arm, 1.0));
    part(&mut v, [-2.0, 6.0, 0.0], [4.0, 12.0, 4.0], [0.0, 16.0], [0.0, 32.0], 0.25, [-2.0, 12.0, 0.0], limb((rig.right_leg, 0.0), -1.0));
    part(&mut v, [2.0, 6.0, 0.0], [4.0, 12.0, 4.0], [16.0, 48.0], [0.0, 48.0], 0.25, [2.0, 12.0, 0.0], limb((rig.left_leg, 0.0), 1.0));

    let cape_start = v.len() as u32;

    if let (true, Some(strips)) = (with_cape, strips) {
        // One pixel-high box per texture row, each hung from the bottom of the one above
        // and turned by its own angle: the cape bends where the physics bent it. A box's
        // texture origin one row down picks that row of every face.
        let mut joint = glam::Vec3::new(0.0, 24.0, -3.2);
        for (row, angle) in strips.iter().enumerate() {
            let from = v.len();
            let top = 24.0 - row as f32;
            push_box(&mut v, [0.0, top - 0.5, -3.2], [10.0, 1.0, 1.0], [0.0, row as f32], [64.0, 32.0], 0.0, true);
            let rotation = Mat4::from_rotation_x(*angle);
            let pivot = glam::Vec3::new(0.0, top, -3.2);
            pose_part(&mut v[from..], pivot.into(), Mat4::from_translation(joint - pivot) * rotation);
            joint += rotation.transform_vector3(glam::Vec3::new(0.0, -1.0, 0.0));
        }
    } else if with_cape {
        // Hangs off the back of the torso, top edge level with the shoulders.
        // Cape textures are 64x32, and the visible design sits where a body
        // box would put its *back*, hence the face flip.
        push_box(
            &mut v,
            // Far enough behind the torso's back face (z = -2) to read as
            // hanging off it rather than being tucked into it.
            [0.0, 16.0, -3.2],
            [10.0, 16.0, 1.0],
            [0.0, 0.0],
            [64.0, 32.0],
            0.0,
            true,
        );
        // Lifting it swings the bottom edge out behind, about the top edge.
        pose_part(&mut v[cape_start as usize..], [0.0, 24.0, -3.2], Mat4::from_rotation_x(rig.cape));
    }

    Geometry {
        vertices: v,
        cape_start,
    }
}

// --- rendering ------------------------------------------------------------

#[derive(Debug)]
pub struct Scene {
    mvp: [[f32; 4]; 4],
    geometry: Arc<Geometry>,
    skin: Arc<Rgba>,
    cape: Option<Arc<Rgba>>,
    key: u64,
    slot: u64,
    outlined: bool,
    time: f32,
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    mvp: [[f32; 4]; 4],
    colour: [f32; 4],
    expand: f32,
    textured: f32,
    /// Non-zero on the outer hull, which derives its colour from position
    /// instead of using `colour`.
    rainbow: f32,
    /// Seconds since the viewer appeared, so the rainbow can drift.
    time: f32,
}

impl shader::Primitive for Scene {
    type Pipeline = ModelPipeline;

    fn prepare(
        &self,
        pipeline: &mut Self::Pipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        viewport: &shader::Viewport,
    ) {
        // The one place both the logical bounds and the scale factor are in
        // scope; `render` has neither.
        let scale = viewport.scale_factor();

        pipeline.resize_depth(device, viewport.physical_size());
        let shared = Shared { device, queue, layout: &pipeline.texture_layout, uniform_layout: &pipeline.uniform_layout, sampler: &pipeline.sampler };
        let instance = pipeline.instances.entry(self.slot).or_insert_with(|| Instance::new(&shared));
        instance.upload_geometry(&shared, &self.geometry);
        instance.upload_textures(&shared, self.key, &self.skin, self.cape.as_deref());
        instance.viewport = Rectangle {
            x: bounds.x * scale,
            y: bounds.y * scale,
            width: bounds.width * scale,
            height: bounds.height * scale,
        };

        // Used by both the textured pass and the coverage pass; the vertex
        // stage is shared, so the geometry must match exactly.
        let uniforms = Uniforms {
            mvp: self.mvp,
            colour: [1.0; 4],
            expand: 0.0,
            textured: 1.0,
            rainbow: 0.0,
            time: 0.0,
        };
        queue.write_buffer(&instance.uniforms, 0, bytemuck::bytes_of(&uniforms));

        queue.write_buffer(
            &pipeline.outline_uniform,
            0,
            bytemuck::bytes_of(&OutlineUniforms {
                radius: OUTLINE_PIXELS,
                time: self.time,
                _padding: [0.0; 2],
            }),
        );

        if self.outlined {
            pipeline.resize_mask(device, viewport.physical_size());
        }
    }

    fn render(
        &self,
        pipeline: &Self::Pipeline,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        clip_bounds: &Rectangle<u32>,
    ) {
        let (Some(depth), Some(instance)) = (pipeline.depth.as_ref(), pipeline.instances.get(&self.slot)) else {
            return;
        };
        if instance.vertex_count == 0 {
            return;
        }
        let viewport = instance.viewport;

        let body = 0..self.geometry.cape_start;
        let cape = self.geometry.cape_start..instance.vertex_count;

        // Draws the figure's geometry, whatever the current pass is.
        let draw_figure = |pass: &mut wgpu::RenderPass<'_>| {
            pass.set_vertex_buffer(0, instance.vertices.slice(..));
            pass.set_bind_group(0, &instance.uniform_binding, &[]);
            pass.set_bind_group(1, &instance.skin_binding, &[]);
            pass.draw(body.clone(), 0..1);

            if !cape.is_empty()
                && let Some(cape_binding) = instance.cape_binding.as_ref()
            {
                pass.set_bind_group(1, cape_binding, &[]);
                pass.draw(cape.clone(), 0..1);
            }
        };

        if self.outlined {
            let (Some(mask), Some(mask_binding)) =
                (pipeline.mask.as_ref(), pipeline.mask_binding.as_ref())
            else {
                return;
            };

            // Pass one: coverage into the mask. Cleared to zero, so anything
            // left over from a previous frame cannot leak into the ring.
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("nexo.skin3d.mask"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: mask,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: depth,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Discard,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });

                // Same viewport as the final pass, so mask pixels and screen
                // pixels are the same pixels.
                pass.set_viewport(
                    viewport.x,
                    viewport.y,
                    viewport.width,
                    viewport.height,
                    0.0,
                    1.0,
                );
                pass.set_pipeline(&pipeline.mask_pipeline);
                draw_figure(&mut pass);
            }

            // Pass two: dilate the mask into the ring, straight onto the UI.
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("nexo.skin3d.outline"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            pass.set_scissor_rect(
                clip_bounds.x,
                clip_bounds.y,
                clip_bounds.width.max(1),
                clip_bounds.height.max(1),
            );
            pass.set_viewport(
                viewport.x,
                viewport.y,
                viewport.width,
                viewport.height,
                0.0,
                1.0,
            );
            pass.set_pipeline(&pipeline.outline_pipeline);
            pass.set_bind_group(0, &pipeline.outline_uniform_binding, &[]);
            pass.set_bind_group(1, mask_binding, &[]);
            // Three vertices: one oversized triangle covering the viewport.
            pass.draw(0..3, 0..1);
            return;
        }

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("nexo.skin3d"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    // Load, not Clear: the rest of the interface is already
                    // drawn into this target.
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        pass.set_scissor_rect(
            clip_bounds.x,
            clip_bounds.y,
            clip_bounds.width.max(1),
            clip_bounds.height.max(1),
        );
        pass.set_viewport(
            viewport.x,
            viewport.y,
            viewport.width,
            viewport.height,
            0.0,
            1.0,
        );
        pass.set_pipeline(&pipeline.model);
        draw_figure(&mut pass);
    }
}

/// GPU resources, created once and reused across frames.
#[derive(Debug)]
pub struct ModelPipeline {
    model: wgpu::RenderPipeline,
    /// Renders the figure's coverage into [`Self::mask`].
    mask_pipeline: wgpu::RenderPipeline,
    /// Fullscreen pass that turns the mask into a ring.
    outline_pipeline: wgpu::RenderPipeline,
    mask: Option<wgpu::TextureView>,
    mask_binding: Option<wgpu::BindGroup>,
    mask_layout: wgpu::BindGroupLayout,
    outline_uniform: wgpu::Buffer,
    outline_uniform_binding: wgpu::BindGroup,
    uniform_layout: wgpu::BindGroupLayout,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// Each viewer's own buffers, by its slot.
    // ponytail: never pruned; bounded by how many skins and capes have been shown.
    instances: std::collections::HashMap<u64, Instance>,
    /// Shared: each viewer's pass clears it before drawing.
    depth: Option<wgpu::TextureView>,
    depth_size: (u32, u32),
}

/// The pipeline's device-side helpers, borrowed for one `prepare`.
struct Shared<'a> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    layout: &'a wgpu::BindGroupLayout,
    uniform_layout: &'a wgpu::BindGroupLayout,
    sampler: &'a wgpu::Sampler,
}

/// One viewer's geometry, textures and uniforms.
#[derive(Debug)]
pub struct Instance {
    vertices: wgpu::Buffer,
    vertex_capacity: u32,
    vertex_count: u32,
    uniforms: wgpu::Buffer,
    uniform_binding: wgpu::BindGroup,
    skin_binding: wgpu::BindGroup,
    cape_binding: Option<wgpu::BindGroup>,
    /// Which textures are currently uploaded, so they aren't re-sent each
    /// frame.
    uploaded_key: Option<u64>,
    /// The widget's rectangle in **physical** pixels, computed in `prepare`
    /// where the scale factor is available and consumed in `render`, which
    /// only gets the clip rectangle.
    ///
    /// `wgpu`'s viewport and scissor are both physical, but iced hands the
    /// primitive its bounds in logical units. On a 100%-scaled display the two
    /// are identical, which is why this was invisible on Linux and broke on a
    /// Windows desktop at 125%: the viewport landed at bounds/scale — up and
    /// to the left of the widget, and smaller — so the figure was clipped to
    /// whatever corner still overlapped, or vanished entirely.
    viewport: Rectangle,
}

impl shader::Pipeline for ModelPipeline {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("nexo.skin3d.shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("nexo.skin3d.uniforms"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("nexo.skin3d.texture"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    // Non-filtering: skins are pixel art and must not be
                    // smoothed on the way to the screen.
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                    count: None,
                },
            ],
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("nexo.skin3d.layout"),
            bind_group_layouts: &[&uniform_layout, &texture_layout],
            push_constant_ranges: &[],
        });

        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: 12,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x2,
                },
                wgpu::VertexAttribute {
                    offset: 20,
                    shader_location: 2,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: 32,
                    shader_location: 3,
                    format: wgpu::VertexFormat::Float32,
                },
            ],
        };

        let make_pipeline = |label: &str, cull: wgpu::Face, writes: wgpu::ColorWrites| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: std::slice::from_ref(&vertex_layout),
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: writes,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: Some(cull),
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: true,
                    depth_compare: wgpu::CompareFunction::Less,
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview: None,
                cache: None,
            })
        };

        let model = make_pipeline("nexo.skin3d.model", wgpu::Face::Back, wgpu::ColorWrites::ALL);

        // Coverage pass. Single-channel target, and the same vertex stage as
        // the model so the mask lines up with it exactly.
        let mask_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("nexo.skin3d.mask"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: std::slice::from_ref(&vertex_layout),
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_mask"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: MASK_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });

        let outline_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("nexo.skin3d.outline.shader"),
            source: wgpu::ShaderSource::Wgsl(OUTLINE_SHADER.into()),
        });

        // Read with textureLoad rather than a sampler: the mask is queried at
        // exact pixels, so there is nothing to filter.
        let mask_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("nexo.skin3d.mask.layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });

        let outline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("nexo.skin3d.outline.layout"),
            bind_group_layouts: &[&uniform_layout, &mask_layout],
            push_constant_ranges: &[],
        });

        let outline_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("nexo.skin3d.outline"),
            layout: Some(&outline_layout),
            vertex: wgpu::VertexState {
                module: &outline_shader,
                entry_point: Some("vs_outline"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &outline_shader,
                entry_point: Some("fs_outline"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            // No depth: it reads the mask instead.
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });

        let outline_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("nexo.skin3d.outline.uniform"),
            size: std::mem::size_of::<OutlineUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let outline_uniform_binding = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("nexo.skin3d.outline.uniform.bind"),
            layout: &uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: outline_uniform.as_entire_binding(),
            }],
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("nexo.skin3d.sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        Self {
            model,
            mask_pipeline,
            outline_pipeline,
            mask: None,
            mask_binding: None,
            mask_layout,
            outline_uniform,
            outline_uniform_binding,
            uniform_layout,
            texture_layout,
            sampler,
            instances: std::collections::HashMap::new(),
            depth: None,
            depth_size: (0, 0),
        }
    }
}

impl Instance {
    fn new(shared: &Shared<'_>) -> Self {
        let uniforms = shared.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("nexo.skin3d.uniform"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_binding = shared.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("nexo.skin3d.uniform.bind"),
            layout: shared.uniform_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() }],
        });
        Self {
            vertices: shared.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("nexo.skin3d.vertices"),
                size: 1024,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            vertex_capacity: 0,
            vertex_count: 0,
            uniforms,
            uniform_binding,
            // A 1x1 placeholder keeps the bind group valid before any skin has
            // been uploaded.
            skin_binding: blank_binding(shared.device, shared.layout, shared.sampler),
            cape_binding: None,
            uploaded_key: None,
            viewport: Rectangle::default(),
        }
    }

    fn upload_geometry(&mut self, shared: &Shared<'_>, geometry: &Geometry) {
        let needed = geometry.vertices.len() as u32;
        if needed > self.vertex_capacity {
            self.vertices = shared.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("nexo.skin3d.vertices"),
                size: (std::mem::size_of::<Vertex>() * geometry.vertices.len()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.vertex_capacity = needed;
        }
        shared.queue.write_buffer(&self.vertices, 0, bytemuck::cast_slice(&geometry.vertices));
        self.vertex_count = needed;
    }

    fn upload_textures(&mut self, shared: &Shared<'_>, key: u64, skin: &Rgba, cape: Option<&Rgba>) {
        if self.uploaded_key == Some(key) {
            return;
        }
        self.skin_binding = texture_binding(shared.device, shared.queue, shared.layout, shared.sampler, skin);
        self.cape_binding = cape.map(|cape| texture_binding(shared.device, shared.queue, shared.layout, shared.sampler, cape));
        self.uploaded_key = Some(key);
    }
}

impl ModelPipeline {
    /// The depth buffer has to match the surface it's paired with, so it's
    /// rebuilt whenever the window changes size.
    fn resize_depth(&mut self, device: &wgpu::Device, size: iced::Size<u32>) {
        let size = (size.width.max(1), size.height.max(1));
        if self.depth.is_some() && self.depth_size == size {
            return;
        }

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("nexo.skin3d.depth"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });

        self.depth = Some(texture.create_view(&wgpu::TextureViewDescriptor::default()));
        self.depth_size = size;
        // The mask is addressed in the same pixel coordinates as the surface,
        // so it has to be rebuilt alongside the depth buffer.
        self.mask = None;
    }

    /// Coverage mask, matching the surface pixel for pixel so the fullscreen
    /// pass can read it at `frag_coord` without any remapping.
    fn resize_mask(&mut self, device: &wgpu::Device, size: iced::Size<u32>) {
        let size = (size.width.max(1), size.height.max(1));
        if self.mask.is_some() && self.depth_size == size {
            return;
        }

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("nexo.skin3d.mask"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: MASK_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.mask_binding = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("nexo.skin3d.mask.bind"),
            layout: &self.mask_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            }],
        }));
        self.mask = Some(view);
    }
}

fn texture_binding(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    image: &Rgba,
) -> wgpu::BindGroup {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("nexo.skin3d.texture"),
        size: wgpu::Extent3d {
            width: image.width,
            height: image.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        // Unorm, not UnormSrgb. Iced's surface is not an sRGB format, so an
        // sRGB texture would be converted to linear on sample and then stored
        // without being converted back — which is exactly what made the model
        // render far darker than the skin actually is.
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });

    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &image.pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(image.width * 4),
            rows_per_image: Some(image.height),
        },
        wgpu::Extent3d {
            width: image.width,
            height: image.height,
            depth_or_array_layers: 1,
        },
    );

    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("nexo.skin3d.texture.bind"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

fn blank_binding(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("nexo.skin3d.blank"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("nexo.skin3d.blank.bind"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

const SHADER: &str = r#"
struct Uniforms {
    mvp: mat4x4<f32>,
    colour: vec4<f32>,
    expand: f32,
    textured: f32,
    rainbow: f32,
    time: f32,
};

// How quickly the outline's hue cycles across the model, per unit of model
// space. Tuned so a 32-tall figure spans roughly one full sweep.
const RAINBOW_SCALE: f32 = 0.026;

// Full hue cycles per second. Fast enough that the colour is visibly
// moving at a glance, slow enough not to strobe.
const RAINBOW_DRIFT: f32 = 0.35;

// Hue to RGB at full saturation and value. Cheaper than a general HSV
// conversion, and the outline only ever wants fully saturated colour.
fn hue_to_rgb(h: f32) -> vec3<f32> {
    let r = abs(h * 6.0 - 3.0) - 1.0;
    let g = 2.0 - abs(h * 6.0 - 2.0);
    let b = 2.0 - abs(h * 6.0 - 4.0);
    return clamp(vec3<f32>(r, g, b), vec3<f32>(0.0), vec3<f32>(1.0));
}

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) expand: vec3<f32>,
    @location(3) shade: f32,
};

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) shade: f32,
    @location(2) local: vec3<f32>,
};

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    var out: VertexOut;
    // Outline passes push each vertex away from its own cuboid's centre.
    let position = in.position + in.expand * u.expand;
    out.clip = u.mvp * vec4<f32>(position, 1.0);
    out.uv = in.uv;
    out.shade = in.shade;
    // Model space, not clip space: the hue must stay fixed to the figure so
    // it doesn't crawl over the surface while the model is rotated.
    out.local = position;
    return out;
}

// Coverage only: writes 1 wherever the figure is solid. Shares the vertex
// stage, so the mask lines up exactly with where the model would have drawn.
@fragment
fn fs_mask(in: VertexOut) -> @location(0) vec4<f32> {
    let sampled = textureSample(tex, samp, in.uv);
    // Same cutout as the model pass, or transparent overlay boxes would make
    // the silhouette a set of solid slabs.
    if (sampled.a < 0.05) {
        discard;
    }
    return vec4<f32>(1.0, 0.0, 0.0, 1.0);
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    if (u.textured < 0.5) {
        if (u.rainbow > 0.5) {
            // Diagonal sweep, so the bands run across the figure rather than
            // as flat horizontal stripes.
            let hue = fract((in.local.y + in.local.x) * RAINBOW_SCALE + u.time * RAINBOW_DRIFT);
            return vec4<f32>(hue_to_rgb(hue), 1.0);
        }
        return u.colour;
    }

    let sampled = textureSample(tex, samp, in.uv);
    // Overlay layers are mostly transparent; drawing them would otherwise
    // block the base layer behind via the depth buffer.
    if (sampled.a < 0.05) {
        discard;
    }
    return vec4<f32>(sampled.rgb * in.shade, sampled.a);
}
"#;

/// Screen-space outline pass. Reads the coverage mask and paints the ring
/// around it.
const OUTLINE_SHADER: &str = r#"
struct Outline {
    // Radius in physical pixels.
    radius: f32,
    time: f32,
    padding: vec2<f32>,
};

@group(0) @binding(0) var<uniform> o: Outline;
@group(1) @binding(0) var mask: texture_2d<f32>;

// Full hue cycles per second.
const DRIFT: f32 = 0.35;
// Hue change per pixel, so the band sweeps across the figure rather than
// flashing as one flat colour.
const SPREAD: f32 = 0.0016;
// Samples around the ring. Enough that a circle of this radius has no gaps,
// few enough to stay cheap at every pixel of the border.
const STEPS: i32 = 24;

fn hue_to_rgb(h: f32) -> vec3<f32> {
    let r = abs(h * 6.0 - 3.0) - 1.0;
    let g = 2.0 - abs(h * 6.0 - 2.0);
    let b = 2.0 - abs(h * 6.0 - 4.0);
    return clamp(vec3<f32>(r, g, b), vec3<f32>(0.0), vec3<f32>(1.0));
}

@vertex
fn vs_outline(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    // One oversized triangle covering the viewport — cheaper than a quad and
    // with no seam down the diagonal. Indices 0,1,2 must give (-1,-1),
    // (3,-1) and (-1,3); note `& 2u` on both terms, since using `& 1u` for y
    // collapses two corners onto each other and the triangle degenerates to a
    // line that rasterises nothing.
    let x = f32((index << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(index & 2u) * 2.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

@fragment
fn fs_outline(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let here = vec2<i32>(frag.xy);

    // Inside the figure: this is the "invisible skin" — the silhouette's
    // interior is left untouched.
    if (textureLoad(mask, here, 0).r > 0.5) {
        discard;
    }

    // Outside: part of the ring if anything solid lies within the radius.
    // Because the mask is the union of every cuboid, there are no internal
    // edges to outline — exactly what a cast shadow looks like.
    var found = false;
    for (var step = 0; step < STEPS; step = step + 1) {
        let angle = f32(step) * (6.2831853 / f32(STEPS));
        let offset = vec2<f32>(cos(angle), sin(angle)) * o.radius;
        if (textureLoad(mask, here + vec2<i32>(offset), 0).r > 0.5) {
            found = true;
            break;
        }
    }
    if (!found) {
        discard;
    }

    let hue = fract((frag.x + frag.y) * SPREAD + o.time * DRIFT);
    return vec4<f32>(hue_to_rgb(hue), 1.0);
}
"#;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct OutlineUniforms {
    radius: f32,
    time: f32,
    _padding: [f32; 2],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_has_geometry_for_every_part() {
        let model = build_model(SkinModel::Classic, false, Rig::default(), None);
        // Six base parts plus six overlays, six faces each, two triangles a
        // face, three vertices a triangle.
        assert_eq!(model.vertices.len(), 12 * 6 * 6);
        assert_eq!(model.cape_start, model.vertices.len() as u32);
    }

    #[test]
    fn cape_is_appended_after_the_body() {
        let without = build_model(SkinModel::Classic, false, Rig::default(), None);
        let with = build_model(SkinModel::Classic, true, Rig::default(), None);

        assert_eq!(with.cape_start, without.vertices.len() as u32);
        assert_eq!(with.vertices.len(), without.vertices.len() + 36);
    }

    #[test]
    fn slim_arms_are_narrower_and_hang_closer() {
        let classic = build_model(SkinModel::Classic, false, Rig::default(), None);
        let slim = build_model(SkinModel::Slim, false, Rig::default(), None);
        assert_eq!(classic.vertices.len(), slim.vertices.len());

        let widest = |g: &Geometry| {
            g.vertices
                .iter()
                .map(|v| v.position[0].abs())
                .fold(0.0f32, f32::max)
        };
        assert!(widest(&slim) < widest(&classic));
    }

    #[test]
    fn expand_directions_point_away_from_their_own_box() {
        let model = build_model(SkinModel::Classic, false, Rig::default(), None);
        // A normalized direction, or zero — never anything longer, which
        // would make the outline uneven.
        for vertex in &model.vertices {
            let length = glam::Vec3::from(vertex.expand).length();
            assert!(length <= 1.001, "expand direction was not normalized");
        }
    }

    /// Mirrors `vs_outline` in OUTLINE_SHADER. Kept in step by hand, because
    /// a wrong formula here produces no output at all rather than an error —
    /// the triangle degenerates and rasterises nothing, which looks exactly
    /// like the outline being switched off.
    fn fullscreen_corner(index: u32) -> (f32, f32) {
        let x = ((index << 1) & 2) as f32 * 2.0 - 1.0;
        let y = (index & 2) as f32 * 2.0 - 1.0;
        (x, y)
    }

    #[test]
    fn fullscreen_triangle_covers_the_viewport() {
        let corners: Vec<_> = (0..3).map(fullscreen_corner).collect();
        assert_eq!(corners, vec![(-1.0, -1.0), (3.0, -1.0), (-1.0, 3.0)]);

        // All three distinct, or the triangle collapses to a line.
        assert_ne!(corners[0], corners[1]);
        assert_ne!(corners[1], corners[2]);
        assert_ne!(corners[0], corners[2]);

        // Non-zero area, and large enough to cover clip space in both axes.
        let area = ((corners[1].0 - corners[0].0) * (corners[2].1 - corners[0].1)
            - (corners[2].0 - corners[0].0) * (corners[1].1 - corners[0].1))
            .abs()
            / 2.0;
        assert!(area >= 8.0, "triangle does not cover clip space, area {area}");
    }

    /// The waving arm goes up and out to the side, not across the face: the
    /// order the two turns are applied in decides which.
    #[test]
    fn a_raised_arm_stays_out_to_the_side() {
        let model = build_model(SkinModel::Classic, false, POSES[0].1, None);
        // The right arm's base box is the third part: head and torso come first,
        // each a base and an overlay box of 36 vertices.
        let arm = &model.vertices[4 * 36..5 * 36];
        let hand_height = arm.iter().map(|v| v.position[1]).fold(f32::MIN, f32::max);
        let innermost = arm.iter().map(|v| v.position[0]).fold(f32::MIN, f32::max);
        assert!(hand_height > 26.0, "the arm should be raised above the shoulder, reached {hand_height}");
        assert!(innermost < -3.0, "the arm crossed in front of the body, reaching x = {innermost}");
    }

    #[test]
    fn there_are_eighteen_poses_and_the_mirrors_differ() {
        let poses: Vec<_> = (0..18).map(showcase).collect();
        for (i, a) in poses.iter().enumerate() {
            for b in &poses[i + 1..] {
                assert_ne!(a, b, "two tiles would stand the same way");
            }
        }
        assert_eq!(showcase(18), showcase(0), "numbers past the end wrap round");
    }

    #[test]
    fn looking_away_takes_its_time() {
        for n in 0..50 {
            assert!(ATTENTION.contains(&vary(n, ATTENTION)));
            assert!(DISTRACTION.contains(&vary(n, DISTRACTION)));
        }
        // Not the same length every time.
        assert!((0..10).map(|n| (vary(n, ATTENTION) * 10.0) as i32).collect::<std::collections::HashSet<_>>().len() > 3);
    }

    /// The game's bottom face: the texture's top row is the back edge and its
    /// left column the player's left (+X), seen from below.
    #[test]
    fn the_underside_is_laid_out_as_the_game_does() {
        let model = build_model(SkinModel::Classic, false, Rig::default(), None);
        // The head's base box, sixth face; its first corner is (+X, bottom, back).
        let corner = model.vertices[5 * 6];
        assert_eq!(corner.position, [4.0, 24.0, -4.0]);
        assert_eq!(corner.uv, [24.0 / 64.0, 0.0]);
    }

    #[test]
    fn the_cape_lifts_away_from_the_back() {
        let lifted = Rig { cape: 0.3, ..Rig::default() };
        let cape = build_model(SkinModel::Classic, true, lifted, None);
        let deepest = cape.vertices[cape.cape_start as usize..].iter().map(|v| v.position[2]).fold(f32::MAX, f32::min);
        assert!(deepest < -6.0, "a lifted cape swings out behind, reached z = {deepest}");
    }

    #[test]
    fn shortest_angle_wraps_into_half_turns() {
        use std::f32::consts::{PI, TAU};
        assert!((shortest_angle(0.5) - 0.5).abs() < 1e-5);
        assert!((shortest_angle(TAU + 0.5) - 0.5).abs() < 1e-5);
        assert!((shortest_angle(-TAU - 0.5) + 0.5).abs() < 1e-5);
        assert!(shortest_angle(PI + 0.1) < 0.0, "just past half a turn goes negative");
    }

    #[test]
    fn pose_eases_back_to_rest_and_stops() {
        let mut pose = Pose {
            yaw: REST_YAW + 1.5,
            pitch: REST_PITCH - 0.5,
            ..Pose::default()
        };
        pose.released_at = Some(Instant::now());

        // A few seconds of 60fps steps should settle it.
        for _ in 0..600 {
            pose.ease_back(1.0 / 60.0);
        }

        assert!(pose.at_rest(), "pose never settled");
        // Settling clears the timer, so no further redraws are requested.
        assert!(pose.released_at.is_none());
    }
}
