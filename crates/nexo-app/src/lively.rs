//! Stock widgets that ease between looks instead of snapping.
//!
//! The screens keep writing `button(...).style(theme::tile)`. Each wrapper asks
//! that same style function what the widget should look like right now and moves
//! what is shown toward the answer, so every change of look animates, whatever
//! caused it: hover, press, focus, disabled, or a different style function
//! because a row became selected.
//!
//! Each widget runs its own animation from `RedrawRequested` and asks for another
//! frame only until it has arrived, so a settled window paints nothing.

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{tree, Operation, Tree};
use iced::advanced::{clipboard::Clipboard, overlay, renderer, text as adv_text, Shell, Widget};
use iced::widget::{button as b, checkbox as c, container as k, scrollable as sc, slider as sl, text_input as ti, toggler as tg};
use iced::{border, gradient, mouse, window, Background, Border, Color, Element, Event, Gradient, Length, Padding, Rectangle, Renderer, Shadow, Size, Theme, Vector};
use std::cell::Cell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::time::Instant;

/// How long a change takes to mostly happen, in seconds. About 180 ms to look finished.
const TAU: f32 = 0.055;

/// The share of the remaining way to cover after `dt` seconds. Depends on time,
/// not frames, so a slow frame doesn't slow the animation down.
pub fn step(dt: f32) -> f32 {
    if crate::motion::reduced() {
        return 1.0;
    }
    1.0 - (-dt.clamp(0.0, 0.1) / TAU).exp()
}

/// Seconds since the last frame this widget saw. Zero after a rest, so an
/// animation starting now doesn't jump by however long the widget sat idle.
pub fn tick(last: &mut Option<Instant>, now: Instant, moving: bool) -> f32 {
    let dt = last.map_or(0.0, |then| now.saturating_duration_since(then).as_secs_f32());
    *last = moving.then_some(now);
    if dt == 0.0 { 1.0 / 60.0 } else { dt }
}

pub fn num(a: f32, b: f32, k: f32, eps: f32) -> f32 {
    if (a - b).abs() < eps { b } else { a + (b - a) * k }
}

/// A look that can be moved toward another.
pub trait Tween: Copy + PartialEq + 'static {
    fn toward(self, target: Self, k: f32) -> Self;
}

impl Tween for Color {
    fn toward(self, t: Self, k: f32) -> Self {
        const E: f32 = 0.004;
        Color { r: num(self.r, t.r, k, E), g: num(self.g, t.g, k, E), b: num(self.b, t.b, k, E), a: num(self.a, t.a, k, E) }
    }
}

impl Tween for Background {
    fn toward(self, target: Self, k: f32) -> Self {
        match (self, target) {
            (Background::Color(a), Background::Color(b)) => Background::Color(a.toward(b, k)),
            (Background::Gradient(a), Background::Gradient(b)) => Background::Gradient(a.toward_gradient(b, k)),
            // A flat colour and a ramp cross-fade by treating the colour as a ramp of one colour.
            (Background::Color(a), Background::Gradient(b)) => Background::Gradient(flat_like(a, b).toward_gradient(b, k)),
            (Background::Gradient(a), Background::Color(b)) => {
                let moved = a.toward_gradient(flat_like(b, a), k);
                if moved == flat_like(b, a) { Background::Color(b) } else { Background::Gradient(moved) }
            }
        }
    }
}

trait GradientExt {
    fn toward_gradient(self, target: Self, k: f32) -> Self;
}

impl GradientExt for Gradient {
    fn toward_gradient(self, target: Self, k: f32) -> Self {
        let (Gradient::Linear(a), Gradient::Linear(b)) = (self, target);
        let mut stops = b.stops;
        for (stop, from) in stops.iter_mut().zip(a.stops) {
            if let (Some(stop), Some(from)) = (stop.as_mut(), from) {
                stop.color = from.color.toward(stop.color, k);
            }
        }
        Gradient::Linear(gradient::Linear { stops, ..b })
    }
}

fn flat_like(color: Color, like: Gradient) -> Gradient {
    let Gradient::Linear(mut linear) = like;
    for stop in linear.stops.iter_mut().flatten() {
        stop.color = color;
    }
    Gradient::Linear(linear)
}

fn clear(background: Background) -> Background {
    match background {
        Background::Color(c) => Background::Color(Color { a: 0.0, ..c }),
        Background::Gradient(g) => {
            let Gradient::Linear(mut linear) = g;
            for stop in linear.stops.iter_mut().flatten() {
                stop.color.a = 0.0;
            }
            Background::Gradient(Gradient::Linear(linear))
        }
    }
}

fn opt_bg(shown: Option<Background>, target: Option<Background>, k: f32) -> Option<Background> {
    match (shown, target) {
        (None, None) => None,
        (Some(s), Some(t)) => Some(s.toward(t, k)),
        (None, Some(t)) => Some(clear(t).toward(t, k)),
        (Some(s), None) => {
            let gone = clear(s);
            let moved = s.toward(gone, k);
            (moved != gone).then_some(moved)
        }
    }
}

impl Tween for Border {
    fn toward(self, t: Self, k: f32) -> Self {
        let (a, b) = (self.radius, t.radius);
        let r = |x, y| num(x, y, k, 0.05);
        Border {
            color: self.color.toward(t.color, k),
            width: num(self.width, t.width, k, 0.02),
            radius: border::Radius {
                top_left: r(a.top_left, b.top_left),
                top_right: r(a.top_right, b.top_right),
                bottom_right: r(a.bottom_right, b.bottom_right),
                bottom_left: r(a.bottom_left, b.bottom_left),
            },
        }
    }
}

impl Tween for Shadow {
    fn toward(self, t: Self, k: f32) -> Self {
        Shadow {
            color: self.color.toward(t.color, k),
            offset: Vector::new(num(self.offset.x, t.offset.x, k, 0.05), num(self.offset.y, t.offset.y, k, 0.05)),
            blur_radius: num(self.blur_radius, t.blur_radius, k, 0.05),
        }
    }
}

impl Tween for b::Style {
    fn toward(self, t: Self, k: f32) -> Self {
        b::Style {
            background: opt_bg(self.background, t.background, k),
            text_color: self.text_color.toward(t.text_color, k),
            border: self.border.toward(t.border, k),
            shadow: self.shadow.toward(t.shadow, k),
            snap: t.snap,
        }
    }
}

impl Tween for k::Style {
    fn toward(self, t: Self, k: f32) -> Self {
        k::Style {
            text_color: match (self.text_color, t.text_color) {
                (Some(a), Some(b)) => Some(a.toward(b, k)),
                (_, b) => b,
            },
            background: opt_bg(self.background, t.background, k),
            border: self.border.toward(t.border, k),
            shadow: self.shadow.toward(t.shadow, k),
            snap: t.snap,
        }
    }
}

impl Tween for ti::Style {
    fn toward(self, t: Self, k: f32) -> Self {
        ti::Style {
            background: self.background.toward(t.background, k),
            border: self.border.toward(t.border, k),
            icon: self.icon.toward(t.icon, k),
            placeholder: self.placeholder.toward(t.placeholder, k),
            value: self.value.toward(t.value, k),
            selection: self.selection.toward(t.selection, k),
        }
    }
}

impl Tween for c::Style {
    fn toward(self, t: Self, k: f32) -> Self {
        c::Style {
            background: self.background.toward(t.background, k),
            icon_color: self.icon_color.toward(t.icon_color, k),
            border: self.border.toward(t.border, k),
            text_color: t.text_color,
        }
    }
}

impl Tween for sl::Style {
    fn toward(self, t: Self, k: f32) -> Self {
        sl::Style {
            rail: t.rail,
            handle: sl::Handle {
                shape: t.handle.shape,
                background: self.handle.background.toward(t.handle.background, k),
                border_width: num(self.handle.border_width, t.handle.border_width, k, 0.02),
                border_color: self.handle.border_color.toward(t.handle.border_color, k),
            },
        }
    }
}

impl Tween for tg::Style {
    fn toward(self, t: Self, k: f32) -> Self {
        tg::Style {
            background: self.background.toward(t.background, k),
            background_border_color: self.background_border_color.toward(t.background_border_color, k),
            foreground: self.foreground.toward(t.foreground, k),
            foreground_border_color: self.foreground_border_color.toward(t.foreground_border_color, k),
            ..t
        }
    }
}

impl Tween for iced::widget::text::Style {
    fn toward(self, t: Self, k: f32) -> Self {
        iced::widget::text::Style {
            color: match (self.color, t.color) {
                (Some(a), Some(b)) => Some(a.toward(b, k)),
                (_, b) => b,
            },
        }
    }
}

impl Tween for sc::Style {
    fn toward(self, t: Self, k: f32) -> Self {
        let rail = |a: sc::Rail, b: sc::Rail| sc::Rail {
            background: opt_bg(a.background, b.background, k),
            border: a.border.toward(b.border, k),
            scroller: sc::Scroller {
                background: a.scroller.background.toward(b.scroller.background, k),
                border: a.scroller.border.toward(b.scroller.border, k),
            },
        };
        sc::Style {
            vertical_rail: rail(self.vertical_rail, t.vertical_rail),
            horizontal_rail: rail(self.horizontal_rail, t.horizontal_rail),
            ..t
        }
    }
}

/// What the cursor and keyboard are doing to a widget.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Sense {
    hovered: bool,
    pressed: bool,
    focused: bool,
}

struct Anim<S> {
    shown: Option<S>,
    sense: Sense,
    press: f32,
    last: Option<Instant>,
}

/// Runs `inner`, whose style closure reads `look`, and keeps `look` moving toward `target`.
struct Eased<'a, M, S> {
    inner: Element<'a, M>,
    look: Rc<Cell<S>>,
    target: Box<dyn Fn(&Theme, Sense) -> S + 'a>,
    focused: Option<fn(&Tree) -> bool>,
    /// Sinks a pixel while held. Only for things you press.
    nudge: bool,
    /// The part of the widget that reacts to the cursor, when it isn't all of it.
    zone: Option<fn(Rectangle) -> Rectangle>,
}

impl<'a, M, S: Tween> Eased<'a, M, S> {
    fn new(
        look: Rc<Cell<S>>,
        inner: impl Into<Element<'a, M>>,
        target: impl Fn(&Theme, Sense) -> S + 'a,
    ) -> Self {
        Self { inner: inner.into(), look, target: Box::new(target), focused: None, nudge: false, zone: None }
    }
}

impl<'a, M: 'a, S: Tween> Widget<M, Theme, Renderer> for Eased<'a, M, S> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<Anim<S>>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(Anim::<S> { shown: None, sense: Sense::default(), press: 0.0, last: None })
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.inner)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.inner));
    }

    fn size(&self) -> Size<Length> {
        self.inner.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.inner.as_widget().size_hint()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) -> layout::Node {
        self.inner.as_widget_mut().layout(&mut tree.children[0], renderer, limits)
    }

    fn operate(&mut self, tree: &mut Tree, layout: Layout<'_>, renderer: &Renderer, operation: &mut dyn Operation) {
        self.inner.as_widget_mut().operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        let over = cursor.is_over(self.zone.map_or(layout.bounds(), |zone| zone(layout.bounds())));
        self.inner.as_widget_mut().update(&mut tree.children[0], event, layout, cursor, renderer, clipboard, shell, viewport);

        let focused = self.focused.is_some_and(|read| read(&tree.children[0]));
        let anim = tree.state.downcast_mut::<Anim<S>>();
        let mut sense = Sense { hovered: over, focused, ..anim.sense };
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => sense.pressed = over,
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => sense.pressed = false,
            _ => {}
        }
        if sense != anim.sense {
            anim.sense = sense;
            shell.request_redraw();
        }

        if let Event::Window(window::Event::RedrawRequested(now)) = event {
            let target = (self.target)(&crate::theme::current(), sense);
            let press_to = if self.nudge && sense.pressed && sense.hovered { 1.0 } else { 0.0 };
            let was_moving = anim.shown.is_some_and(|s| s != target) || anim.press != press_to;
            let k = step(tick(&mut anim.last, *now, was_moving));
            // A widget seen for the first time simply looks right: nothing
            // animates just because a screen appeared.
            let shown = anim.shown.map_or(target, |s| s.toward(target, k));
            anim.press = num(anim.press, press_to, k, 0.02);
            anim.shown = Some(shown);
            if shown != target || anim.press != press_to {
                anim.last = Some(*now);
                shell.request_redraw();
            } else {
                anim.last = None;
            }
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let anim = tree.state.downcast_ref::<Anim<S>>();
        self.look.set(anim.shown.unwrap_or_else(|| (self.target)(theme, anim.sense)));
        let draw = |renderer: &mut Renderer| {
            self.inner.as_widget().draw(&tree.children[0], renderer, theme, style, layout, cursor, viewport);
        };
        if anim.press > 0.0 {
            iced::advanced::Renderer::with_translation(renderer, Vector::new(0.0, anim.press), draw);
        } else {
            draw(renderer);
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.inner.as_widget().mouse_interaction(&tree.children[0], layout, cursor, viewport, renderer)
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, M, Theme, Renderer>> {
        self.inner.as_widget_mut().overlay(&mut tree.children[0], layout, renderer, viewport, translation)
    }
}

impl<'a, M: 'a, S: Tween> From<Eased<'a, M, S>> for Element<'a, M> {
    fn from(eased: Eased<'a, M, S>) -> Self {
        Element::new(eased)
    }
}

type StyleFn<'a, St, S> = Box<dyn Fn(&Theme, St) -> S + 'a>;

/// `iced::widget::button`, easing between its looks.
pub fn button<'a, M: Clone + 'a>(content: impl Into<Element<'a, M>>) -> Button<'a, M> {
    Button { inner: b::Button::new(content), style: Box::new(crate::theme::ghost_button), enabled: false }
}

pub struct Button<'a, M> {
    inner: b::Button<'a, M>,
    style: StyleFn<'a, b::Status, b::Style>,
    enabled: bool,
}

impl<'a, M: Clone + 'a> Button<'a, M> {
    pub fn padding(mut self, padding: impl Into<Padding>) -> Self {
        self.inner = self.inner.padding(padding);
        self
    }
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.inner = self.inner.width(width);
        self
    }
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.inner = self.inner.height(height);
        self
    }
    pub fn clip(mut self, clip: bool) -> Self {
        self.inner = self.inner.clip(clip);
        self
    }
    pub fn style(mut self, style: impl Fn(&Theme, b::Status) -> b::Style + 'a) -> Self {
        self.style = Box::new(style);
        self
    }
    pub fn on_press(mut self, message: M) -> Self {
        self.inner = self.inner.on_press(message);
        self.enabled = true;
        self
    }
    pub fn on_press_maybe(mut self, message: Option<M>) -> Self {
        self.enabled = message.is_some();
        self.inner = self.inner.on_press_maybe(message);
        self
    }
}

impl<'a, M: Clone + 'a> From<Button<'a, M>> for Element<'a, M> {
    fn from(button: Button<'a, M>) -> Self {
        let Button { inner, style, enabled } = button;
        let look = Rc::new(Cell::new(style(&crate::theme::current(), b::Status::Active)));
        let read = look.clone();
        let inner = inner.style(move |_, _| read.get());
        let mut eased = Eased::new(look, inner, move |theme, sense: Sense| {
            style(theme, match (enabled, sense.hovered, sense.pressed) {
                (false, _, _) => b::Status::Disabled,
                (true, true, true) => b::Status::Pressed,
                (true, true, false) => b::Status::Hovered,
                (true, false, _) => b::Status::Active,
            })
        });
        eased.nudge = enabled;
        eased.into()
    }
}

/// `iced::widget::text_input`, easing into focus and hover.
pub fn text_input<'a, M: Clone + 'a>(placeholder: &str, value: &str) -> Input<'a, M> {
    Input { inner: ti::TextInput::new(placeholder, value), style: Box::new(crate::theme::input), enabled: false }
}

pub struct Input<'a, M> {
    inner: ti::TextInput<'a, M>,
    style: StyleFn<'a, ti::Status, ti::Style>,
    enabled: bool,
}

impl<'a, M: Clone + 'a> Input<'a, M> {
    pub fn on_input(mut self, on_input: impl Fn(String) -> M + 'a) -> Self {
        self.inner = self.inner.on_input(on_input);
        self.enabled = true;
        self
    }
    pub fn on_input_maybe(mut self, on_input: Option<impl Fn(String) -> M + 'a>) -> Self {
        self.enabled = on_input.is_some();
        self.inner = self.inner.on_input_maybe(on_input);
        self
    }
    pub fn on_submit(mut self, message: M) -> Self {
        self.inner = self.inner.on_submit(message);
        self
    }
    pub fn on_submit_maybe(mut self, message: Option<M>) -> Self {
        self.inner = self.inner.on_submit_maybe(message);
        self
    }
    pub fn padding(mut self, padding: impl Into<Padding>) -> Self {
        self.inner = self.inner.padding(padding);
        self
    }
    pub fn size(mut self, size: impl Into<iced::Pixels>) -> Self {
        self.inner = self.inner.size(size);
        self
    }
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.inner = self.inner.width(width);
        self
    }
    pub fn style(mut self, style: impl Fn(&Theme, ti::Status) -> ti::Style + 'a) -> Self {
        self.style = Box::new(style);
        self
    }
}

impl<'a, M: Clone + 'a> From<Input<'a, M>> for Element<'a, M> {
    fn from(input: Input<'a, M>) -> Self {
        let Input { inner, style, enabled } = input;
        let look = Rc::new(Cell::new(style(&crate::theme::current(), ti::Status::Active)));
        let read = look.clone();
        let inner = inner.style(move |_, _| read.get());
        let mut eased = Eased::new(look, inner, move |theme, sense: Sense| {
            style(theme, match (enabled, sense.focused, sense.hovered) {
                (false, _, _) => ti::Status::Disabled,
                (true, true, is_hovered) => ti::Status::Focused { is_hovered },
                (true, false, true) => ti::Status::Hovered,
                (true, false, false) => ti::Status::Active,
            })
        });
        eased.focused = Some(|tree: &Tree| {
            tree.state.downcast_ref::<ti::State<<Renderer as adv_text::Renderer>::Paragraph>>().is_focused()
        });
        eased.into()
    }
}

/// `iced::widget::checkbox`, its tick fading in instead of appearing.
pub fn checkbox<'a, M: 'a>(checked: bool) -> Check<'a, M> {
    Check { inner: c::Checkbox::new(checked), checked, enabled: false, style: Box::new(crate::theme::check) }
}

pub struct Check<'a, M> {
    inner: c::Checkbox<'a, M>,
    checked: bool,
    enabled: bool,
    style: StyleFn<'a, c::Status, c::Style>,
}

impl<'a, M: 'a> Check<'a, M> {
    pub fn size(mut self, size: impl Into<iced::Pixels>) -> Self {
        self.inner = self.inner.size(size);
        self
    }
    pub fn spacing(mut self, spacing: impl Into<iced::Pixels>) -> Self {
        self.inner = self.inner.spacing(spacing);
        self
    }
    pub fn text_size(mut self, size: impl Into<iced::Pixels>) -> Self {
        self.inner = self.inner.text_size(size);
        self
    }
    pub fn label(mut self, label: impl iced::widget::text::IntoFragment<'a>) -> Self {
        self.inner = self.inner.label(label);
        self
    }
    pub fn on_toggle(mut self, on_toggle: impl Fn(bool) -> M + 'a) -> Self {
        self.inner = self.inner.on_toggle(on_toggle);
        self.enabled = true;
        self
    }
    pub fn style(mut self, style: impl Fn(&Theme, c::Status) -> c::Style + 'a) -> Self {
        self.style = Box::new(style);
        self
    }
}

impl<'a, M: 'a> From<Check<'a, M>> for Element<'a, M> {
    fn from(check: Check<'a, M>) -> Self {
        let Check { inner, checked: is_checked, enabled, style } = check;
        let look = Rc::new(Cell::new(style(&crate::theme::current(), c::Status::Active { is_checked })));
        let read = look.clone();
        let inner = inner.style(move |_, _| read.get());
        Eased::new(look, inner, move |theme, sense: Sense| {
            style(theme, match (enabled, sense.hovered) {
                (false, _) => c::Status::Disabled { is_checked },
                (true, true) => c::Status::Hovered { is_checked },
                (true, false) => c::Status::Active { is_checked },
            })
        })
        .into()
    }
}

/// A finished slider, its knob easing between looks.
pub fn slider<'a, M: Clone + 'a>(inner: sl::Slider<'a, u32, M>, style: impl Fn(&Theme, sl::Status) -> sl::Style + 'a) -> Element<'a, M> {
    let look = Rc::new(Cell::new(style(&crate::theme::current(), sl::Status::Active)));
    let read = look.clone();
    let inner = inner.style(move |_, _| read.get());
    Eased::new(look, inner, move |theme, sense: Sense| {
        style(theme, match (sense.pressed, sense.hovered) {
            (true, _) => sl::Status::Dragged,
            (false, true) => sl::Status::Hovered,
            (false, false) => sl::Status::Active,
        })
    })
    .into()
}

/// A container whose colours ease when its style changes: a status pill, a
/// step dot, a tile that lights up when its server comes online.
pub trait Tint<'a, M> {
    fn tinted(self, style: impl Fn(&Theme) -> k::Style + 'a) -> Element<'a, M>;
}

impl<'a, M: 'a> Tint<'a, M> for k::Container<'a, M> {
    fn tinted(self, style: impl Fn(&Theme) -> k::Style + 'a) -> Element<'a, M> {
        let look = Rc::new(Cell::new(style(&crate::theme::current())));
        let read = look.clone();
        let inner = self.style(move |_| read.get());
        Eased::new(look, inner, move |theme, _| style(theme)).into()
    }
}

/// A bar filled to `share` (0 to 1) that slides to each new value.
pub fn meter<'a, M: 'a>(share: f32, height: f32, fill: Color, track: Color) -> Element<'a, M> {
    // NaN would never equal itself, and the bar would ask for frames forever.
    let share = if share.is_finite() { share.clamp(0.0, 1.0) } else { 0.0 };
    Element::new(Meter { share, height, fill, track })
}

struct Meter {
    share: f32,
    height: f32,
    fill: Color,
    track: Color,
}

struct MeterAnim {
    shown: f32,
    last: Option<Instant>,
}

impl<M> Widget<M, Theme, Renderer> for Meter {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<MeterAnim>()
    }
    fn state(&self) -> tree::State {
        tree::State::new(MeterAnim { shown: self.share, last: None })
    }
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fixed(self.height))
    }
    fn layout(&mut self, _tree: &mut Tree, _renderer: &Renderer, limits: &layout::Limits) -> layout::Node {
        layout::Node::new(limits.height(self.height).resolve(Length::Fill, self.height, Size::ZERO))
    }
    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        _layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        _viewport: &Rectangle,
    ) {
        if let Event::Window(window::Event::RedrawRequested(now)) = event {
            let anim = tree.state.downcast_mut::<MeterAnim>();
            if anim.shown != self.share {
                let k = step(tick(&mut anim.last, *now, true));
                anim.shown = num(anim.shown, self.share, k, 0.001);
                shell.request_redraw();
            } else {
                anim.last = None;
            }
        }
    }
    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        let bounds = layout.bounds();
        let shown = tree.state.downcast_ref::<MeterAnim>().shown;
        renderer.fill_quad(renderer::Quad { bounds, ..Default::default() }, self.track);
        renderer.fill_quad(
            renderer::Quad { bounds: Rectangle { width: bounds.width * shown, ..bounds }, ..Default::default() },
            self.fill,
        );
    }
}

/// A thin line with a highlight running along it, for "something is happening"
/// when there's no number to show. Asks for frames only while it's on screen.
pub fn working<'a, M: 'a>(height: f32, color: Color) -> Element<'a, M> {
    Element::new(Working { height, color })
}

struct Working {
    height: f32,
    color: Color,
}

#[derive(Default)]
struct WorkingAnim {
    start: Option<Instant>,
    now: Option<Instant>,
    unfocused: bool,
}

impl<M> Widget<M, Theme, Renderer> for Working {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<WorkingAnim>()
    }
    fn state(&self) -> tree::State {
        tree::State::new(WorkingAnim::default())
    }
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fixed(self.height))
    }
    fn layout(&mut self, _tree: &mut Tree, _renderer: &Renderer, limits: &layout::Limits) -> layout::Node {
        layout::Node::new(limits.height(self.height).resolve(Length::Fill, self.height, Size::ZERO))
    }
    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        _layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        _viewport: &Rectangle,
    ) {
        let anim = tree.state.downcast_mut::<WorkingAnim>();
        match event {
            Event::Window(window::Event::RedrawRequested(now)) => {
                anim.start.get_or_insert(*now);
                anim.now = Some(*now);
            }
            Event::Window(window::Event::Unfocused) => anim.unfocused = true,
            Event::Window(window::Event::Focused) => anim.unfocused = false,
            _ => {}
        }
        // At half speed and not behind another window: waiting on a browser
        // sign-in or a long download shouldn't repaint the launcher all the while.
        if !anim.unfocused {
            shell.request_redraw_at(Instant::now() + std::time::Duration::from_millis(33));
        }
    }
    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        let bounds = layout.bounds();
        let anim = tree.state.downcast_ref::<WorkingAnim>();
        let seconds = match (anim.start, anim.now) {
            (Some(start), Some(now)) => now.saturating_duration_since(start).as_secs_f32(),
            _ => 0.0,
        };
        let radius = border::Radius::from(self.height / 2.0);
        let rounded = Border { radius, ..Default::default() };
        renderer.fill_quad(
            renderer::Quad { bounds, border: rounded, ..Default::default() },
            Color { a: 0.15, ..self.color },
        );
        // Enters at the left, leaves at the right: a third of the track wide,
        // cut down to the track at both ends.
        const PERIOD: f32 = 1.4;
        let phase = (seconds / PERIOD).fract();
        let width = bounds.width / 3.0;
        let left = (-width + (bounds.width + width) * phase).max(0.0);
        let right = (-width + (bounds.width + width) * phase + width).min(bounds.width);
        if right > left {
            renderer.fill_quad(
                renderer::Quad {
                    bounds: Rectangle { x: bounds.x + left, width: right - left, ..bounds },
                    border: rounded,
                    ..Default::default()
                },
                self.color,
            );
        }
    }
}

/// Content that comes into place the first time it's on screen. For panels that
/// come and go within a page, like a form or an error, where `screens::reveal`'s
/// page clock isn't running.
pub fn appear<'a, M: 'a>(content: impl Into<Element<'a, M>>) -> Element<'a, M> {
    Element::new(Appear { inner: content.into(), key: None, floor: 0.0 })
}

/// Text that fades in again whenever `key` changes: a label saying something
/// new. Only for text taking its colour from its parent; one with a `.color()`
/// of its own doesn't fade. The first time on screen it just shows, since the
/// page around it is already coming in.
pub fn changed<'a, M: 'a>(key: impl Hash, content: impl Into<Element<'a, M>>) -> Element<'a, M> {
    Element::new(Appear { inner: content.into(), key: Some(hash_of(key)), floor: 0.0 })
}

/// The same for a live number, ticking over every second or two: it only dips,
/// because fading from nothing would make the figure flicker.
pub fn ticking<'a, M: 'a>(key: impl Hash, content: impl Into<Element<'a, M>>) -> Element<'a, M> {
    Element::new(Appear { inner: content.into(), key: Some(hash_of(key)), floor: 0.65 })
}

fn hash_of(key: impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish()
}

struct Appear<'a, M> {
    inner: Element<'a, M>,
    key: Option<u64>,
    /// The opacity a keyed fade starts from.
    floor: f32,
}

struct AppearAnim {
    amount: f32,
    key: Option<u64>,
    last: Option<Instant>,
}

impl<'a, M: 'a> Widget<M, Theme, Renderer> for Appear<'a, M> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<AppearAnim>()
    }
    fn state(&self) -> tree::State {
        let amount = if self.key.is_some() { 1.0 } else { 0.0 };
        tree::State::new(AppearAnim { amount, key: self.key, last: None })
    }
    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.inner)]
    }
    fn diff(&self, tree: &mut Tree) {
        let anim = tree.state.downcast_mut::<AppearAnim>();
        if anim.key != self.key {
            anim.key = self.key;
            anim.amount = 0.0;
            // Not the time since the last change, or the fade is over in one frame.
            anim.last = None;
        }
        tree.diff_children(std::slice::from_ref(&self.inner));
    }
    fn size(&self) -> Size<Length> {
        self.inner.as_widget().size()
    }
    fn size_hint(&self) -> Size<Length> {
        self.inner.as_widget().size_hint()
    }
    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) -> layout::Node {
        self.inner.as_widget_mut().layout(&mut tree.children[0], renderer, limits)
    }
    fn operate(&mut self, tree: &mut Tree, layout: Layout<'_>, renderer: &Renderer, operation: &mut dyn Operation) {
        self.inner.as_widget_mut().operate(&mut tree.children[0], layout, renderer, operation);
    }
    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        self.inner.as_widget_mut().update(&mut tree.children[0], event, layout, cursor, renderer, clipboard, shell, viewport);
        let anim = tree.state.downcast_mut::<AppearAnim>();
        if anim.amount < 1.0 {
            if let Event::Window(window::Event::RedrawRequested(now)) = event {
                let k = step(tick(&mut anim.last, *now, true));
                anim.amount = num(anim.amount, 1.0, k, 0.004);
            }
            // Until the first frame arrives, this is what asks for it.
            shell.request_redraw();
        }
    }
    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        let hidden = 1.0 - tree.state.downcast_ref::<AppearAnim>().amount;
        if self.key.is_some() {
            let shown = self.floor + (1.0 - self.floor) * (1.0 - hidden);
            let color = Color { a: style.text_color.a * shown, ..style.text_color };
            let style = renderer::Style { text_color: color };
            self.inner.as_widget().draw(&tree.children[0], renderer, theme, &style, layout, cursor, viewport);
            return;
        }
        renderer.with_translation(Vector::new(0.0, hidden * 8.0), |renderer| {
            self.inner.as_widget().draw(&tree.children[0], renderer, theme, style, layout, cursor, viewport);
        });
        if hidden > 0.0 {
            // The page colour over it: iced has no opacity for a whole subtree.
            // A layer of its own, or text and images would be drawn over it.
            let veil = crate::theme::veil(hidden)(theme);
            let bounds = layout.bounds();
            renderer.with_layer(bounds, |renderer| {
                renderer.fill_quad(
                    renderer::Quad { bounds, ..Default::default() },
                    veil.background.unwrap_or(Background::Color(Color::TRANSPARENT)),
                );
            });
        }
    }
    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.inner.as_widget().mouse_interaction(&tree.children[0], layout, cursor, viewport, renderer)
    }
    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, M, Theme, Renderer>> {
        self.inner.as_widget_mut().overlay(&mut tree.children[0], layout, renderer, viewport, translation)
    }
}

/// A switch whose knob slides. Same style function as `iced::widget::toggler`.
pub fn toggler<'a, M: 'a>(on: bool) -> Toggle<'a, M> {
    Toggle { on, on_toggle: None, size: 18.0, style: Box::new(crate::theme::toggle) }
}

pub struct Toggle<'a, M> {
    on: bool,
    on_toggle: Option<Box<dyn Fn(bool) -> M + 'a>>,
    size: f32,
    style: StyleFn<'a, tg::Status, tg::Style>,
}

impl<'a, M: 'a> Toggle<'a, M> {
    pub fn size(mut self, size: impl Into<iced::Pixels>) -> Self {
        self.size = size.into().0;
        self
    }
    pub fn style(mut self, style: impl Fn(&Theme, tg::Status) -> tg::Style + 'a) -> Self {
        self.style = Box::new(style);
        self
    }
    pub fn on_toggle(mut self, on_toggle: impl Fn(bool) -> M + 'a) -> Self {
        self.on_toggle = Some(Box::new(on_toggle));
        self
    }
    pub fn on_toggle_maybe(mut self, on_toggle: Option<impl Fn(bool) -> M + 'a>) -> Self {
        self.on_toggle = on_toggle.map(|f| Box::new(f) as Box<dyn Fn(bool) -> M + 'a>);
        self
    }
}

struct ToggleAnim {
    /// 0 is off, 1 is on.
    knob: f32,
    /// Grows under the cursor and stretches while held.
    stretch: f32,
    shown: Option<tg::Style>,
    hovered: bool,
    pressed: bool,
    last: Option<Instant>,
}

impl<'a, M: 'a> Widget<M, Theme, Renderer> for Toggle<'a, M> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<ToggleAnim>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(ToggleAnim {
            knob: if self.on { 1.0 } else { 0.0 },
            stretch: 0.0,
            shown: None,
            hovered: false,
            pressed: false,
            last: None,
        })
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(self.size * 2.0), Length::Fixed(self.size))
    }

    fn layout(&mut self, _tree: &mut Tree, _renderer: &Renderer, _limits: &layout::Limits) -> layout::Node {
        layout::Node::new(Size::new(self.size * 2.0, self.size))
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        _viewport: &Rectangle,
    ) {
        let anim = tree.state.downcast_mut::<ToggleAnim>();
        let over = cursor.is_over(layout.bounds());
        if over != anim.hovered {
            anim.hovered = over;
            shell.request_redraw();
        }
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) if over && !shell.is_event_captured() => {
                if let Some(on_toggle) = &self.on_toggle {
                    anim.pressed = true;
                    shell.publish(on_toggle(!self.on));
                    shell.capture_event();
                    shell.request_redraw();
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) if anim.pressed => {
                anim.pressed = false;
                shell.request_redraw();
            }
            Event::Window(window::Event::RedrawRequested(now)) => {
                let enabled = self.on_toggle.is_some();
                let status = match (enabled, over) {
                    (false, _) => tg::Status::Disabled { is_toggled: self.on },
                    (true, true) => tg::Status::Hovered { is_toggled: self.on },
                    (true, false) => tg::Status::Active { is_toggled: self.on },
                };
                let target = (self.style)(&crate::theme::current(), status);
                let knob_to = if self.on { 1.0 } else { 0.0 };
                let stretch_to = match (enabled, anim.pressed, over) {
                    (true, true, _) => 1.0,
                    (true, false, true) => 0.35,
                    _ => 0.0,
                };
                let k = step(tick(&mut anim.last, *now, true));
                anim.knob = num(anim.knob, knob_to, k, 0.002);
                anim.stretch = num(anim.stretch, stretch_to, k, 0.005);
                let shown = anim.shown.map_or(target, |s| s.toward(target, k));
                anim.shown = Some(shown);
                if shown != target || anim.knob != knob_to || anim.stretch != stretch_to {
                    shell.request_redraw();
                } else {
                    anim.last = None;
                }
            }
            _ => {}
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        let anim = tree.state.downcast_ref::<ToggleAnim>();
        let bounds = layout.bounds();
        let style = anim.shown.unwrap_or_else(|| {
            (self.style)(theme, tg::Status::Active { is_toggled: self.on })
        });
        renderer.fill_quad(
            renderer::Quad {
                bounds,
                border: Border {
                    radius: (bounds.height / 2.0).into(),
                    width: style.background_border_width,
                    color: style.background_border_color,
                },
                ..Default::default()
            },
            style.background,
        );

        let gap = (bounds.height * style.padding_ratio).max(2.0);
        let height = bounds.height - gap * 2.0;
        let width = height * (1.0 + 0.3 * anim.stretch);
        let travel = bounds.width - gap * 2.0 - width;
        renderer.fill_quad(
            renderer::Quad {
                bounds: Rectangle {
                    x: bounds.x + gap + travel * anim.knob,
                    y: bounds.y + gap,
                    width,
                    height,
                },
                border: Border { radius: (height / 2.0).into(), ..Default::default() },
                ..Default::default()
            },
            style.foreground,
        );
    }

    fn mouse_interaction(
        &self,
        _tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &Renderer,
    ) -> mouse::Interaction {
        match (cursor.is_over(layout.bounds()), self.on_toggle.is_some()) {
            (true, true) => mouse::Interaction::Pointer,
            (true, false) => mouse::Interaction::NotAllowed,
            (false, _) => mouse::Interaction::default(),
        }
    }
}

impl<'a, M: 'a> From<Toggle<'a, M>> for Element<'a, M> {
    fn from(toggle: Toggle<'a, M>) -> Self {
        Element::new(toggle)
    }
}

/// `iced::widget::scrollable` for vertical lists, gliding to where the wheel
/// sent it instead of jumping. Trackpads report pixels and already glide, so
/// only notched wheels are smoothed.
pub fn scrollable<'a, M: 'a>(content: impl Into<Element<'a, M>>) -> Glide<'a, M> {
    Glide { inner: sc::Scrollable::new(content), style: Box::new(sc::default) }
}

pub struct Glide<'a, M> {
    inner: sc::Scrollable<'a, M>,
    style: StyleFn<'a, sc::Status, sc::Style>,
}

/// `inner` with a scrollbar that eases into hover and drag. iced doesn't say
/// where the scroller is, so the whole strip it runs along counts as hovering it.
pub fn eased_scrollbar<'a, M: 'a>(
    inner: sc::Scrollable<'a, M>,
    style: impl Fn(&Theme, sc::Status) -> sc::Style + 'a,
    sideways: bool,
) -> Element<'a, M> {
    let status = move |sense: Sense| {
        let on = (sense.pressed, sense.hovered);
        let (x, y) = (sideways, !sideways);
        match on {
            (true, _) => sc::Status::Dragged {
                is_horizontal_scrollbar_dragged: x,
                is_vertical_scrollbar_dragged: y,
                is_horizontal_scrollbar_disabled: false,
                is_vertical_scrollbar_disabled: false,
            },
            (false, true) => sc::Status::Hovered {
                is_horizontal_scrollbar_hovered: x,
                is_vertical_scrollbar_hovered: y,
                is_horizontal_scrollbar_disabled: false,
                is_vertical_scrollbar_disabled: false,
            },
            (false, false) => sc::Status::Active {
                is_horizontal_scrollbar_disabled: false,
                is_vertical_scrollbar_disabled: false,
            },
        }
    };
    let look = Rc::new(Cell::new(style(&crate::theme::current(), status(Sense::default()))));
    let read = look.clone();
    let inner = inner.style(move |_, _| read.get());
    let mut eased = Eased::new(look, inner, move |theme, sense| style(theme, status(sense)));
    eased.zone = Some(if sideways {
        // The one sideways bar is thin, and buttons sit right above it.
        |r: Rectangle| Rectangle { y: r.y + r.height - 5.0, height: 5.0, ..r }
    } else {
        |r: Rectangle| Rectangle { x: r.x + r.width - 12.0, width: 12.0, ..r }
    });
    eased.into()
}

/// A text's colour easing from one to another, like a step becoming the current one.
pub fn tinted_text<'a, M: 'a>(inner: iced::widget::Text<'a>, color: Color) -> Element<'a, M> {
    let target = iced::widget::text::Style { color: Some(color) };
    let look = Rc::new(Cell::new(target));
    let read = look.clone();
    let inner = inner.style(move |_| read.get());
    Eased::new(look, inner, move |_, _| target).into()
}

impl<'a, M: 'a> Glide<'a, M> {
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.inner = self.inner.width(width);
        self
    }
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.inner = self.inner.height(height);
        self
    }
    pub fn spacing(mut self, spacing: impl Into<iced::Pixels>) -> Self {
        self.inner = self.inner.spacing(spacing);
        self
    }
    pub fn anchor_bottom(mut self) -> Self {
        self.inner = self.inner.anchor_bottom();
        self
    }
    pub fn style(mut self, style: impl Fn(&Theme, sc::Status) -> sc::Style + 'a) -> Self {
        self.style = Box::new(style);
        self
    }
}

#[derive(Default)]
struct GlideState {
    left: f32,
    /// Where the wheel was turned. The rest of the glide lands there, even if
    /// the cursor has since moved on.
    at: Option<iced::Point>,
    modifiers: iced::keyboard::Modifiers,
    last: Option<Instant>,
}

struct GlideWidget<'a, M> {
    inner: Element<'a, M>,
}

/// What one notch of the wheel scrolls, the same as a stock scrollable.
const NOTCH: f32 = 60.0;

impl<'a, M: 'a> Widget<M, Theme, Renderer> for GlideWidget<'a, M> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<GlideState>()
    }
    fn state(&self) -> tree::State {
        tree::State::new(GlideState::default())
    }
    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.inner)]
    }
    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.inner));
    }
    fn size(&self) -> Size<Length> {
        self.inner.as_widget().size()
    }
    fn size_hint(&self) -> Size<Length> {
        self.inner.as_widget().size_hint()
    }
    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) -> layout::Node {
        self.inner.as_widget_mut().layout(&mut tree.children[0], renderer, limits)
    }
    fn operate(&mut self, tree: &mut Tree, layout: Layout<'_>, renderer: &Renderer, operation: &mut dyn Operation) {
        self.inner.as_widget_mut().operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        let (child, state) = (&mut tree.children[0], &mut tree.state);
        let state = state.downcast_mut::<GlideState>();

        if let Event::Keyboard(iced::keyboard::Event::ModifiersChanged(modifiers)) = event {
            state.modifiers = *modifiers;
        }

        // Sideways scrolling, by tilt or Shift, is left to the stock path, and so
        // is Ctrl, which a dropdown under the cursor uses.
        if let Event::Mouse(mouse::Event::WheelScrolled { delta: mouse::ScrollDelta::Lines { y, x } }) = event {
            let plain = !state.modifiers.shift() && !state.modifiers.command();
            if *x == 0.0 && plain && !shell.is_event_captured() && cursor.is_over(layout.bounds()) {
                state.at = cursor.position();
                // Turning the wheel the other way cancels what was still queued.
                if state.left * y < 0.0 {
                    state.left = 0.0;
                }
                state.left += y * NOTCH;
                // The content still hears a wheel turned, moving nothing yet:
                // an open dropdown in it closes on that.
                wheel(&mut self.inner, child, 0.0, layout, cursor, renderer, clipboard, shell, viewport);
                shell.capture_event();
                shell.request_redraw();
                return;
            }
        }

        self.inner.as_widget_mut().update(child, event, layout, cursor, renderer, clipboard, shell, viewport);

        if let Event::Window(window::Event::RedrawRequested(now)) = event {
            if state.left != 0.0 {
                let dt = tick(&mut state.last, *now, true);
                let mut part = state.left * step(dt);
                if (state.left - part).abs() < 0.5 {
                    part = state.left;
                }
                state.left -= part;
                let cursor = state.at.map_or(cursor, mouse::Cursor::Available);
                wheel(&mut self.inner, child, part, layout, cursor, renderer, clipboard, shell, viewport);
                shell.request_redraw();
            } else {
                state.last = None;
            }
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.inner.as_widget().draw(&tree.children[0], renderer, theme, style, layout, cursor, viewport);
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.inner.as_widget().mouse_interaction(&tree.children[0], layout, cursor, viewport, renderer)
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, M, Theme, Renderer>> {
        self.inner.as_widget_mut().overlay(&mut tree.children[0], layout, renderer, viewport, translation)
    }
}

/// Turns the wheel by `y` pixels. Through a shell of its own, so this can't mark
/// the event as handled for every widget after it, a frame's redraw above all.
#[allow(clippy::too_many_arguments)]
fn wheel<'a, M: 'a>(
    inner: &mut Element<'a, M>,
    tree: &mut Tree,
    y: f32,
    layout: Layout<'_>,
    cursor: mouse::Cursor,
    renderer: &Renderer,
    clipboard: &mut dyn Clipboard,
    shell: &mut Shell<'_, M>,
    viewport: &Rectangle,
) {
    let event = Event::Mouse(mouse::Event::WheelScrolled { delta: mouse::ScrollDelta::Pixels { x: 0.0, y } });
    let mut messages = Vec::new();
    let mut own = Shell::new(&mut messages);
    inner.as_widget_mut().update(tree, &event, layout, cursor, renderer, clipboard, &mut own, viewport);
    if own.is_layout_invalid() {
        shell.invalidate_layout();
    }
    if own.are_widgets_invalid() {
        shell.invalidate_widgets();
    }
    for message in messages {
        shell.publish(message);
    }
}

impl<'a, M: 'a> From<Glide<'a, M>> for Element<'a, M> {
    fn from(glide: Glide<'a, M>) -> Self {
        Element::new(GlideWidget { inner: eased_scrollbar(glide.inner, glide.style, false) })
    }
}

/// Where each gliding list's state lives, to check a list keeps it across views.
#[cfg(test)]
pub fn scroll_states(tree: &Tree) -> Vec<*const ()> {
    let mut found = Vec::new();
    if tree.tag == tree::Tag::of::<GlideState>() {
        found.push(std::ptr::from_ref(tree.state.downcast_ref::<GlideState>()).cast());
    }
    found.extend(tree.children.iter().flat_map(scroll_states));
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_changed_label_fades_from_the_start_every_time() {
        let label = |key: &str| changed::<()>(key, iced::widget::Space::new());
        let mut tree = Tree::new(&label("a"));
        assert_eq!(tree.state.downcast_ref::<AppearAnim>().amount, 1.0, "first sight just shows");
        tree.state.downcast_mut::<AppearAnim>().last = Some(Instant::now());
        tree.diff(&label("a"));
        assert_eq!(tree.state.downcast_ref::<AppearAnim>().amount, 1.0);
        tree.diff(&label("b"));
        let anim = tree.state.downcast_ref::<AppearAnim>();
        assert_eq!(anim.amount, 0.0);
        assert!(anim.last.is_none(), "idle time must not count toward the fade");
    }

    #[test]
    fn a_look_arrives_exactly_and_stops() {
        let mut shown = Color::BLACK;
        let target = Color::from_rgb(1.0, 0.5, 0.25);
        let mut frames = 0;
        while shown != target {
            shown = shown.toward(target, step(1.0 / 60.0));
            frames += 1;
            assert!(frames < 120, "must settle, or the window repaints forever");
        }
        assert!(frames > 5, "must actually animate, took {frames} frames");
    }

    #[test]
    fn a_missing_background_fades_out_and_ends_as_none() {
        let k = step(1.0 / 60.0);
        let mut shown = Some(Background::Color(Color::WHITE));
        for _ in 0..120 {
            shown = opt_bg(shown, None, k);
        }
        assert_eq!(shown, None);
    }

    #[test]
    fn a_colour_and_a_ramp_cross_fade_both_ways() {
        let ramp = Background::Gradient(Gradient::Linear(
            gradient::Linear::new(0.0).add_stop(0.0, Color::WHITE).add_stop(1.0, Color::BLACK),
        ));
        let flat = Background::Color(Color::from_rgb(0.5, 0.0, 0.5));
        let k = step(1.0 / 60.0);
        let (mut a, mut b) = (flat, ramp);
        for _ in 0..120 {
            a = a.toward(ramp, k);
            b = b.toward(flat, k);
        }
        assert_eq!(a, ramp);
        assert_eq!(b, flat);
    }
}
