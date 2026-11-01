//! The full-window backgrounds a canvas can't draw well: a nebula's clouds and
//! liquid chrome. Fragment shaders, so every pixel is worked out smoothly instead
//! of being built from shapes.

use crate::backdrop::{Clock, Tuning};
use crate::theme;
use iced::mouse;
use iced::wgpu;
use iced::widget::shader;
use iced::{Color, Element, Length, Rectangle};
use std::collections::HashMap;

/// What to draw. The numbers are `mode` in the shader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Nebula = 0,
    Chrome = 1,
}

pub struct Sky {
    kind: Kind,
    tuning: Tuning,
    live: bool,
}

impl<Message> shader::Program<Message> for Sky {
    type State = Clock;
    type Primitive = Frame;

    fn update(&self, clock: &mut Clock, event: &iced::Event, _: Rectangle, _: mouse::Cursor) -> Option<shader::Action<Message>> {
        let iced::Event::Window(iced::window::Event::RedrawRequested(now)) = event else {
            return None;
        };
        clock.advance(*now, self.live, self.tuning).then(|| shader::Action::request_redraw_at(*now + crate::backdrop::FRAME))
    }

    fn draw(&self, clock: &Clock, _: mouse::Cursor, bounds: Rectangle) -> Frame {
        // Linear values on a surface that doesn't convert them, which darkens them.
        // The themes' clouds and metal are tuned to look right this way.
        let linear = |color: Color| color.into_linear();
        let palette = theme::palette();
        Frame {
            slot: (bounds.x as i32, bounds.y as i32),
            uniforms: Uniforms {
                origin: [0.0; 2],
                size: [0.0; 2],
                time: clock.seconds() + 60.0,
                mode: self.kind as u32 as f32,
                amount: self.tuning.amount as f32 / 100.0,
                _padding: 0.0,
                ramp: palette.ramp.map(linear),
                accent: linear(palette.accent),
                text: linear(palette.text),
            },
        }
    }
}

pub fn view<'a, Message: 'a>(kind: Kind, tuning: Tuning, live: bool, height: impl Into<Length>) -> Element<'a, Message> {
    iced::widget::shader(Sky { kind, tuning, live }).width(Length::Fill).height(height).into()
}

#[derive(Debug)]
pub struct Frame {
    /// Tells one widget's uniforms from another's: the Settings page has several.
    slot: (i32, i32),
    uniforms: Uniforms,
}

#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    origin: [f32; 2],
    size: [f32; 2],
    time: f32,
    mode: f32,
    amount: f32,
    _padding: f32,
    ramp: [[f32; 4]; 4],
    accent: [f32; 4],
    text: [f32; 4],
}

/// Uniform offsets have to be aligned to this.
const STRIDE: u64 = 256;
/// More than any screen shows at once: one big background and a tile per effect.
const SLOTS: u32 = 16;

impl shader::Primitive for Frame {
    type Pipeline = Pipeline;

    fn prepare(
        &self,
        pipeline: &mut Pipeline,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        viewport: &shader::Viewport,
    ) {
        let next = pipeline.slots.len() as u32;
        if next == SLOTS && !pipeline.slots.contains_key(&self.slot) {
            return;
        }
        let index = *pipeline.slots.entry(self.slot).or_insert(next);
        let scale = viewport.scale_factor();
        let uniforms = Uniforms {
            origin: [bounds.x * scale, bounds.y * scale],
            size: [bounds.width * scale, bounds.height * scale],
            ..self.uniforms
        };
        queue.write_buffer(&pipeline.uniforms, index as u64 * STRIDE, bytemuck::bytes_of(&uniforms));
    }

    fn render(
        &self,
        pipeline: &Pipeline,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        clip_bounds: &Rectangle<u32>,
    ) {
        let Some(&index) = pipeline.slots.get(&self.slot) else { return };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("nexo.sky"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        // The whole target as the viewport, with the shader placing itself by pixel
        // position: a tile scrolled half out of view can't make an invalid viewport.
        pass.set_scissor_rect(clip_bounds.x, clip_bounds.y, clip_bounds.width.max(1), clip_bounds.height.max(1));
        pass.set_pipeline(&pipeline.pipeline);
        pass.set_bind_group(0, &pipeline.bind_group, &[index * STRIDE as u32]);
        pass.draw(0..3, 0..1);
    }
}

pub struct Pipeline {
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    slots: HashMap<(i32, i32), u32>,
}

impl shader::Pipeline for Pipeline {
    /// Every frame: moving tiles would otherwise use up the slots with old positions.
    fn trim(&mut self) {
        self.slots.clear();
    }

    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("nexo.sky"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("nexo.sky.uniforms"),
            size: STRIDE * SLOTS as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("nexo.sky.layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<Uniforms>() as u64),
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("nexo.sky.bind"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &uniforms,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<Uniforms>() as u64),
                }),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("nexo.sky.pipeline_layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("nexo.sky"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            multiview: None,
            cache: None,
        });
        Self { pipeline, uniforms, bind_group, slots: HashMap::new() }
    }
}

const SHADER: &str = r#"
struct Uniforms {
    origin: vec2f,
    size: vec2f,
    time: f32,
    mode: f32,
    amount: f32,
    padding: f32,
    ramp: array<vec4f, 4>,
    accent: vec4f,
    text: vec4f,
}
@group(0) @binding(0) var<uniform> u: Uniforms;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    let x = f32(i32(i & 1u) * 4 - 1);
    let y = f32(i32(i >> 1u) * 4 - 1);
    return vec4f(x, y, 0.0, 1.0);
}

fn hash(p: vec2f) -> f32 {
    return fract(sin(dot(p, vec2f(127.1, 311.7))) * 43758.5453);
}

fn noise(p: vec2f) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let s = f * f * (3.0 - 2.0 * f);
    return mix(
        mix(hash(i), hash(i + vec2f(1.0, 0.0)), s.x),
        mix(hash(i + vec2f(0.0, 1.0)), hash(i + vec2f(1.0, 1.0)), s.x),
        s.y,
    );
}

fn fbm(start: vec2f) -> f32 {
    var p = start;
    var total = 0.0;
    var weight = 0.5;
    for (var octave = 0; octave < 4; octave++) {
        total += weight * noise(p);
        // Turned each octave, so the value noise's grid doesn't show.
        p = mat2x2f(1.6, 1.2, -1.2, 1.6) * p;
        weight *= 0.5;
    }
    return total;
}

fn nebula(uv: vec2f, pixel: vec2f) -> vec4f {
    let t = u.time;
    let p = uv * 2.6;
    let q = vec2f(fbm(p + vec2f(0.0, t * 0.012)), fbm(p + vec2f(5.2, 1.3) - t * 0.009));
    let v = fbm(p + 3.2 * q + vec2f(t * 0.01, 0.0));
    let cloud = smoothstep(0.32, 0.8, v);

    var color = mix(u.ramp[0].rgb, u.ramp[2].rgb, clamp(q.x * 1.9 - 0.3, 0.0, 1.0));
    color = mix(color, u.ramp[3].rgb, clamp(q.y * q.y * 2.0, 0.0, 1.0));
    color = mix(color, u.accent.rgb, smoothstep(0.62, 0.9, v) * 0.5);

    // Stars, in the darker parts: a sparse grid of cells, each maybe holding one.
    let cell = floor(pixel / 3.0);
    let star = step(0.9975, hash(cell));
    let twinkle = 0.55 + 0.45 * sin(t * (0.6 + hash(cell + 3.0) * 1.4) + hash(cell + 7.0) * 6.28);
    let stars = star * twinkle * (1.0 - cloud) * 0.8;

    let density = clamp(0.5 + 0.25 * u.amount, 0.0, 1.0);
    let alpha = clamp(cloud * density + stars, 0.0, 1.0);
    let lit = mix(color, u.text.rgb, stars / max(alpha, 0.001));
    return vec4f(lit, alpha);
}

fn molten(start: vec2f) -> f32 {
    let t = u.time;
    var p = start;
    p += 0.42 * vec2f(sin(p.y * 1.3 + t * 0.21), cos(p.x * 1.1 - t * 0.17));
    p += 0.2 * vec2f(sin(p.y * 2.9 - t * 0.13), cos(p.x * 2.3 + t * 0.19));
    let centre = vec2f(sin(t * 0.13) * 1.4, cos(t * 0.11) * 0.8);
    return sin(p.x * 1.35 + t * 0.2)
        + 0.62 * sin(p.y * 2.05 - t * 0.16 + p.x * 0.7)
        + 0.34 * sin((p.x + p.y) * 3.1 + t * 0.26)
        + 0.3 * sin(length(p - centre) * 4.1 - t * 0.34);
}

fn chrome(uv: vec2f) -> vec4f {
    let p = (uv - 0.5) * 3.2;
    let e = 0.012;
    let dx = molten(p + vec2f(e, 0.0)) - molten(p - vec2f(e, 0.0));
    let dy = molten(p + vec2f(0.0, e)) - molten(p - vec2f(0.0, e));
    let normal = normalize(vec3f(-dx, -dy, e * 5.0));
    let reflected = reflect(vec3f(0.0, 0.0, -1.0), normal);

    // What polished metal mirrors: bright sky fading upward, a hard dark line at
    // the horizon, dark ground lifting a little toward the far edge.
    let y = reflected.y + reflected.x * 0.25;
    let sky = mix(1.0, 0.34, pow(clamp(y, 0.0, 1.0), 0.55));
    let ground = mix(0.03, 0.3, pow(clamp(-y, 0.0, 1.0), 0.8));
    var tone = mix(ground, sky, smoothstep(-0.018, 0.018, y));
    let light = normalize(vec3f(-0.4, 0.7, 0.6));
    tone += pow(max(dot(reflected, light), 0.0), 90.0) * 0.8;

    let tint = mix(u.text.rgb, u.accent.rgb, 0.3);
    // Dimmed, and most in the middle where the player and the buttons are.
    let edge = length((uv - 0.5) * vec2f(1.1, 1.3));
    let dim = mix(0.2, 0.48, smoothstep(0.1, 0.75, edge));
    return vec4f(tint * tone * dim * (0.75 + 0.25 * u.amount), 1.0);
}

@fragment
fn fs_main(@builtin(position) position: vec4f) -> @location(0) vec4f {
    let pixel = position.xy - u.origin;
    // Square, so clouds and metal aren't stretched by a wide window. Based on the
    // height, so a small preview shows the same sort of shapes as the full page.
    let uv = pixel / max(u.size.y, 1.0);
    if (u.mode > 0.5) {
        let aspect = u.size.x / max(u.size.y, 1.0);
        return chrome(uv - vec2f((aspect - 1.0) * 0.5, 0.0));
    }
    return nebula(uv, pixel);
}
"#;
