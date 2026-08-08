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

/// Close enough to a pose to treat it as already there, in radians. Stops a
/// reveal re-triggering a move the model has effectively already made.
const ARRIVED: f32 = 0.02;

/// How long a dragged pose is left alone before it eases back.
const HOLD: Duration = Duration::from_secs(5);

/// Fraction of the remaining distance closed per second while easing back.
/// Exponential rather than linear so it arrives gently instead of stopping
/// dead.
const RETURN_RATE: f32 = 2.2;

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

/// Interaction state. Lives in the widget tree, so a redraw doesn't reset the
/// pose the user dragged to.
#[derive(Debug)]
pub struct Pose {
    yaw: f32,
    pitch: f32,
    /// Cursor position the current drag was last sampled at.
    drag_from: Option<iced::Point>,
    /// When the drag ended, which starts the hold before easing back.
    released_at: Option<Instant>,
    last_tick: Option<Instant>,
    /// Time base for the drifting rainbow. Set on the first frame rather than
    /// at construction, so the animation starts when the widget appears.
    started: Option<Instant>,
    /// Where the model settles. Normally the front pose, but a cape change
    /// moves it round the back and leaves it there — on a cape screen, the
    /// cape is the thing worth looking at.
    rest_yaw: f32,
    /// Last reveal request acted on, so one request moves the model once.
    reveal_seen: u64,
}

impl Default for Pose {
    fn default() -> Self {
        Self {
            yaw: REST_YAW,
            pitch: REST_PITCH,
            drag_from: None,
            released_at: None,
            last_tick: None,
            started: None,
            rest_yaw: REST_YAW,
            reveal_seen: 0,
        }
    }
}

impl Pose {
    fn at_rest(&self) -> bool {
        (self.yaw - self.rest_yaw).abs() < SETTLED && (self.pitch - REST_PITCH).abs() < SETTLED
    }

    /// Eases toward the rest pose, framerate-independently.
    fn ease_back(&mut self, dt: f32) {
        let t = 1.0 - (-RETURN_RATE * dt).exp();
        self.yaw += (self.rest_yaw - self.yaw) * t;
        self.pitch += (REST_PITCH - self.pitch) * t;
        if self.at_rest() {
            self.yaw = self.rest_yaw;
            self.pitch = REST_PITCH;
            self.released_at = None;
        }
    }

    /// Points the model at whichever side a reveal asked for, if it isn't
    /// effectively there already.
    ///
    /// Returns whether anything needs animating, so an unchanged pose doesn't
    /// start a redraw loop that has nothing to draw.
    fn reveal(&mut self, target_yaw: f32) -> bool {
        self.rest_yaw = target_yaw;

        // Compare on the shortest angle between the two, so a model sitting
        // at the target plus a full turn still counts as already there.
        let delta = shortest_angle(target_yaw - self.yaw);
        if delta.abs() < ARRIVED {
            self.yaw = target_yaw;
            return false;
        }

        // Rotate the short way round rather than unwinding several turns.
        self.yaw = target_yaw - delta;
        // Nothing is being held, so easing starts immediately.
        self.released_at = None;
        true
    }
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
    /// Draw the rainbow border. Only while signed out: it marks the
    /// placeholder figure, and once a real skin is shown it would just
    /// obscure it — and animating it costs a redraw every frame.
    outlined: bool,
    /// Bumped to ask the model to turn round and show its back. Carried as a
    /// counter rather than a flag so repeating the same request moves it
    /// again, while a redraw on its own does not.
    reveal_back: u64,
}

impl SkinViewer {
    pub fn new(
        skin: Arc<Rgba>,
        cape: Option<Arc<Rgba>>,
        model: SkinModel,
        key: u64,
        outlined: bool,
        reveal_back: u64,
    ) -> Self {
        Self {
            skin,
            cape,
            model,
            key,
            outlined,
            reveal_back,
        }
    }
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

        match event {
            E::Mouse(Mouse::ButtonPressed(Button::Left)) => {
                let position = cursor.position_over(bounds)?;
                state.drag_from = Some(position);
                state.released_at = None;
                Some(shader::Action::request_redraw())
            }

            E::Mouse(Mouse::CursorMoved { .. }) => {
                let from = state.drag_from?;
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

            E::Mouse(Mouse::ButtonReleased(Button::Left)) => {
                state.drag_from.take()?;
                let now = Instant::now();
                state.released_at = Some(now);
                // Nothing to draw until the hold expires, so ask to be woken
                // then instead of animating through it.
                Some(shader::Action::request_redraw_at(now + HOLD))
            }

            E::Window(Window::RedrawRequested(now)) => {
                let previous = state.last_tick.replace(*now);
                state.started.get_or_insert(*now);

                // A visible outline drifts continuously, so it needs a frame
                // every time regardless of what the pose is doing.
                let animating = self.outlined;

                // A new reveal request retargets the resting pose.
                if state.reveal_seen != self.reveal_back {
                    state.reveal_seen = self.reveal_back;
                    // The very first frame carries the initial counter and
                    // must not spin the model on load.
                    if self.reveal_back != 0 {
                        state.reveal(BACK_YAW);
                    }
                }

                if state.drag_from.is_some() {
                    return animating.then(shader::Action::request_redraw);
                }

                // Hold a dragged pose before easing away from it. A reveal
                // clears this, so switching a cape doesn't wait five seconds.
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

                let dt = previous
                    .map(|previous| now.duration_since(previous).as_secs_f32())
                    // First animated frame has no previous tick to measure.
                    .unwrap_or(1.0 / 60.0)
                    .min(0.1);

                state.ease_back(dt);
                Some(shader::Action::request_redraw())
            }

            _ => None,
        }
    }

    fn draw(&self, state: &Self::State, _cursor: mouse::Cursor, bounds: Rectangle) -> Scene {
        Scene {
            mvp: mvp(state.yaw, state.pitch, bounds),
            geometry: Arc::new(build_model(self.model, self.cape.is_some())),
            skin: Arc::clone(&self.skin),
            cape: self.cape.clone(),
            key: self.key,
            bounds,
            outlined: self.outlined,
            time: state
                .started
                .map(|start| start.elapsed().as_secs_f32())
                .unwrap_or(0.0),
        }
    }

    fn mouse_interaction(
        &self,
        state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if state.drag_from.is_some() {
            mouse::Interaction::Grabbing
        } else if cursor.is_over(bounds) {
            mouse::Interaction::Grab
        } else {
            mouse::Interaction::default()
        }
    }
}

/// Model-view-projection for the current pose.
fn mvp(yaw: f32, pitch: f32, bounds: Rectangle) -> [[f32; 4]; 4] {
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
    let model = Mat4::from_rotation_x(pitch)
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
    let bottom = [u + d + w, v, w, d];

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
    face(
        [p(-1., -1., 1.), p(-1., -1., -1.), p(1., -1., -1.), p(1., -1., 1.)],
        [[-1., -1., 1.], [-1., -1., -1.], [1., -1., -1.], [1., -1., 1.]],
        bottom,
        SHADE_BOTTOM,
    );
}

/// Builds the player model, standing on y = 0, in skin-pixel units.
fn build_model(model: SkinModel, with_cape: bool) -> Geometry {
    let mut v = Vec::new();
    let skin = [64.0, 64.0];

    // Slim skins have 3px arms, which also moves where they hang.
    let arm_w = match model {
        SkinModel::Classic => 4.0,
        SkinModel::Slim => 3.0,
    };
    let arm_x = 4.0 + arm_w / 2.0;

    // Base layers.
    push_box(&mut v, [0.0, 28.0, 0.0], [8.0, 8.0, 8.0], [0.0, 0.0], skin, 0.0, false);
    push_box(&mut v, [0.0, 18.0, 0.0], [8.0, 12.0, 4.0], [16.0, 16.0], skin, 0.0, false);
    push_box(&mut v, [-arm_x, 18.0, 0.0], [arm_w, 12.0, 4.0], [40.0, 16.0], skin, 0.0, false);
    push_box(&mut v, [arm_x, 18.0, 0.0], [arm_w, 12.0, 4.0], [32.0, 48.0], skin, 0.0, false);
    push_box(&mut v, [-2.0, 6.0, 0.0], [4.0, 12.0, 4.0], [0.0, 16.0], skin, 0.0, false);
    push_box(&mut v, [2.0, 6.0, 0.0], [4.0, 12.0, 4.0], [16.0, 48.0], skin, 0.0, false);

    // Overlay layers, slightly inflated so they sit just proud of the base.
    // Hair and jacket detail live here; skipping them is the usual reason a
    // rendered skin looks subtly wrong.
    push_box(&mut v, [0.0, 28.0, 0.0], [8.0, 8.0, 8.0], [32.0, 0.0], skin, 0.5, false);
    push_box(&mut v, [0.0, 18.0, 0.0], [8.0, 12.0, 4.0], [16.0, 32.0], skin, 0.25, false);
    push_box(&mut v, [-arm_x, 18.0, 0.0], [arm_w, 12.0, 4.0], [40.0, 32.0], skin, 0.25, false);
    push_box(&mut v, [arm_x, 18.0, 0.0], [arm_w, 12.0, 4.0], [48.0, 48.0], skin, 0.25, false);
    push_box(&mut v, [-2.0, 6.0, 0.0], [4.0, 12.0, 4.0], [0.0, 32.0], skin, 0.25, false);
    push_box(&mut v, [2.0, 6.0, 0.0], [4.0, 12.0, 4.0], [0.0, 48.0], skin, 0.25, false);

    let cape_start = v.len() as u32;

    if with_cape {
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
    bounds: Rectangle,
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
        _bounds: &Rectangle,
        viewport: &shader::Viewport,
    ) {
        pipeline.upload_geometry(device, queue, &self.geometry);
        pipeline.upload_textures(device, queue, self.key, &self.skin, self.cape.as_deref());
        pipeline.resize_depth(device, viewport.physical_size());

        // One uniform block per pass; only the outline width, colour and
        // whether to sample the texture differ.
        let passes = [
            // Used by both the textured pass and the coverage pass; the
            // vertex stage is shared, so the geometry must match exactly.
            Uniforms {
                mvp: self.mvp,
                colour: [1.0; 4],
                expand: 0.0,
                textured: 1.0,
                rainbow: 0.0,
                time: 0.0,
            },
            Uniforms {
                mvp: self.mvp,
                colour: [1.0; 4],
                expand: 0.0,
                textured: 1.0,
                rainbow: 0.0,
                time: 0.0,
            },
        ];

        for (index, uniforms) in passes.iter().enumerate() {
            queue.write_buffer(
                &pipeline.uniforms[index],
                0,
                bytemuck::bytes_of(uniforms),
            );
        }

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
        let Some(depth) = pipeline.depth.as_ref() else {
            return;
        };
        if pipeline.vertex_count == 0 {
            return;
        }

        let body = 0..self.geometry.cape_start;
        let cape = self.geometry.cape_start..pipeline.vertex_count;

        // Draws the figure's geometry, whatever the current pass is.
        let draw_figure = |pass: &mut wgpu::RenderPass<'_>| {
            pass.set_vertex_buffer(0, pipeline.vertices.slice(..));
            pass.set_bind_group(0, &pipeline.uniform_bindings[0], &[]);
            pass.set_bind_group(1, &pipeline.skin_binding, &[]);
            pass.draw(body.clone(), 0..1);

            if !cape.is_empty()
                && let Some(cape_binding) = pipeline.cape_binding.as_ref()
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
                    self.bounds.x,
                    self.bounds.y,
                    self.bounds.width,
                    self.bounds.height,
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
                self.bounds.x,
                self.bounds.y,
                self.bounds.width,
                self.bounds.height,
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
            self.bounds.x,
            self.bounds.y,
            self.bounds.width,
            self.bounds.height,
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
    vertices: wgpu::Buffer,
    vertex_capacity: u32,
    vertex_count: u32,
    uniforms: [wgpu::Buffer; 2],
    uniform_bindings: [wgpu::BindGroup; 2],
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    skin_binding: wgpu::BindGroup,
    cape_binding: Option<wgpu::BindGroup>,
    /// Which textures are currently uploaded, so they aren't re-sent each
    /// frame.
    uploaded_key: Option<u64>,
    depth: Option<wgpu::TextureView>,
    depth_size: (u32, u32),
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

        let uniforms: [wgpu::Buffer; 2] = std::array::from_fn(|_| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("nexo.skin3d.uniform"),
                size: std::mem::size_of::<Uniforms>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });

        let uniform_bindings = std::array::from_fn(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("nexo.skin3d.uniform.bind"),
                layout: &uniform_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniforms[i].as_entire_binding(),
                }],
            })
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("nexo.skin3d.sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        // A 1x1 placeholder keeps the bind group valid before any skin has
        // been uploaded.
        let skin_binding = blank_binding(device, &texture_layout, &sampler);

        let vertices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("nexo.skin3d.vertices"),
            size: 1024,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
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
            vertices,
            vertex_capacity: 0,
            vertex_count: 0,
            uniforms,
            uniform_bindings,
            texture_layout,
            sampler,
            skin_binding,
            cape_binding: None,
            uploaded_key: None,
            depth: None,
            depth_size: (0, 0),
        }
    }
}

impl ModelPipeline {
    fn upload_geometry(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        geometry: &Geometry,
    ) {
        let needed = geometry.vertices.len() as u32;
        if needed > self.vertex_capacity {
            self.vertices = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("nexo.skin3d.vertices"),
                size: (std::mem::size_of::<Vertex>() * geometry.vertices.len()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.vertex_capacity = needed;
        }
        queue.write_buffer(&self.vertices, 0, bytemuck::cast_slice(&geometry.vertices));
        self.vertex_count = needed;
    }

    fn upload_textures(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: u64,
        skin: &Rgba,
        cape: Option<&Rgba>,
    ) {
        if self.uploaded_key == Some(key) {
            return;
        }

        self.skin_binding = texture_binding(device, queue, &self.texture_layout, &self.sampler, skin);
        self.cape_binding = cape
            .map(|cape| texture_binding(device, queue, &self.texture_layout, &self.sampler, cape));
        self.uploaded_key = Some(key);
    }

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
        let model = build_model(SkinModel::Classic, false);
        // Six base parts plus six overlays, six faces each, two triangles a
        // face, three vertices a triangle.
        assert_eq!(model.vertices.len(), 12 * 6 * 6);
        assert_eq!(model.cape_start, model.vertices.len() as u32);
    }

    #[test]
    fn cape_is_appended_after_the_body() {
        let without = build_model(SkinModel::Classic, false);
        let with = build_model(SkinModel::Classic, true);

        assert_eq!(with.cape_start, without.vertices.len() as u32);
        assert_eq!(with.vertices.len(), without.vertices.len() + 36);
    }

    #[test]
    fn slim_arms_are_narrower_and_hang_closer() {
        let classic = build_model(SkinModel::Classic, false);
        let slim = build_model(SkinModel::Slim, false);
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
        let model = build_model(SkinModel::Classic, false);
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

    #[test]
    fn reveal_moves_to_the_back_and_settles_there() {
        let mut pose = Pose::default();
        assert!(pose.reveal(BACK_YAW), "a turn from the front needs animating");

        for _ in 0..600 {
            pose.ease_back(1.0 / 60.0);
        }
        assert!(pose.at_rest());
        // The back is now where it settles, not the front it started from.
        assert!((pose.yaw - BACK_YAW).abs() < SETTLED);
    }

    #[test]
    fn revealing_a_pose_it_already_holds_does_nothing() {
        let mut pose = Pose {
            yaw: BACK_YAW,
            ..Pose::default()
        };

        assert!(
            !pose.reveal(BACK_YAW),
            "already facing away, so there is nothing to animate"
        );
        assert_eq!(pose.yaw, BACK_YAW);
    }

    #[test]
    fn reveal_takes_the_short_way_round() {
        // A full extra turn past the target still counts as being there.
        let mut pose = Pose {
            yaw: BACK_YAW + std::f32::consts::TAU,
            ..Pose::default()
        };
        assert!(!pose.reveal(BACK_YAW));

        // And a target just the other side of the wrap point moves a little,
        // not almost all the way round.
        let mut pose = Pose {
            yaw: BACK_YAW - 0.3,
            ..Pose::default()
        };
        assert!(pose.reveal(BACK_YAW));
        assert!(
            (pose.yaw - (BACK_YAW - 0.3)).abs() < 0.001,
            "should not have unwound a whole turn to get there"
        );
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
