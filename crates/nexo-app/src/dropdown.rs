//! The launcher's own dropdown: the field eases into focus, its chevron turns,
//! the list unrolls with its rows arriving in order, and one highlight glides
//! from row to row. Closing plays the same thing backwards. Also takes the
//! arrow keys, Enter, Home, End and Escape while it is open.

use crate::lively::{num, step, tick, Tween};
use crate::theme;
use iced::advanced::layout::{self, Layout};
use iced::advanced::text::{self, Renderer as _, Text};
use iced::advanced::widget::{tree, Tree};
use iced::advanced::{clipboard::Clipboard, overlay, renderer, svg as adv_svg, Renderer as _, Shell, Widget};
use iced::widget::text_input as ti;
use iced::{alignment, keyboard, mouse, window, Background, Border, Color, Element, Event, Length, Padding, Pixels, Point, Rectangle, Renderer, Size, Theme, Vector};
use std::borrow::{Borrow, Cow};
use std::sync::LazyLock;
use std::time::Instant;

static CHEVRON: LazyLock<adv_svg::Handle> = LazyLock::new(|| {
    adv_svg::Handle::from_memory(
        &br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 12 12"><path d="M2.5 4.5 6 8l3.5-3.5" fill="none" stroke="#fff" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg>"##[..],
    )
});

/// Space between the field and the list, and around the rows inside it.
const GAP: f32 = 6.0;
const INSET: f32 = 4.0;
/// A long list scrolls inside this rather than filling the window.
const MAX_HEIGHT: f32 = 280.0;
/// Kept clear between the list and the window's edge.
const MARGIN: f32 = 8.0;

pub fn dropdown<'a, T, M>(
    options: impl Into<Cow<'a, [T]>>,
    selected: Option<impl Borrow<T>>,
    on_select: impl Fn(T) -> M + 'a,
) -> Dropdown<'a, T, M>
where
    T: ToString + PartialEq + Clone + 'a,
{
    let options = options.into();
    let selected = selected.as_ref().map(Borrow::borrow);
    Dropdown {
        // Shown even if it isn't one of the options: a version list reloaded
        // underneath must not blank a value that's still in use.
        label: selected.map(ToString::to_string),
        selected: selected.and_then(|s| options.iter().position(|o| o == s)),
        options,
        on_select: Box::new(on_select),
        padding: Padding::from([8, 10]),
        width: Length::Fixed(180.0),
        text_size: 14.0,
    }
}

pub struct Dropdown<'a, T: Clone, M> {
    options: Cow<'a, [T]>,
    label: Option<String>,
    selected: Option<usize>,
    on_select: Box<dyn Fn(T) -> M + 'a>,
    padding: Padding,
    width: Length,
    text_size: f32,
}

impl<T: Clone, M> Dropdown<'_, T, M> {
    pub fn padding(mut self, padding: impl Into<Padding>) -> Self {
        self.padding = padding.into();
        self
    }
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }
    pub fn text_size(mut self, size: impl Into<Pixels>) -> Self {
        self.text_size = size.into().0;
        self
    }
    fn row_height(&self) -> f32 {
        self.text_size + 16.0
    }
}

#[derive(Default)]
struct State {
    open: bool,
    /// How open the list is, 0 to 1. Still above 0 for a moment after closing.
    amount: f32,
    hovered: bool,
    look: Option<ti::Style>,
    /// The row the cursor or arrow keys are on.
    active: usize,
    /// Where the highlight is now, in rows, on its way to `active`.
    glide: f32,
    scroll: f32,
    scroll_to: f32,
    /// Just opened: the list centres on the chosen row once it knows its height.
    centre: bool,
    /// The label the field last showed, and how far it has faded in.
    showing: Option<String>,
    label_in: f32,
    modifiers: keyboard::Modifiers,
    /// Wheel travel not yet worth a step, so a trackpad's stream of tiny
    /// movements doesn't race through the options, changing real settings.
    wheel: f32,
    last: Option<Instant>,
}

/// Where the list goes: under the field, unless it fits better above.
/// Returns the list's bounds and whether it opened upward.
fn place(field: Rectangle, count: usize, row: f32, screen: Size) -> (Rectangle, bool) {
    let natural = (count as f32 * row + INSET * 2.0).min(MAX_HEIGHT);
    let below = screen.height - (field.y + field.height) - GAP - MARGIN;
    let above = field.y - GAP - MARGIN;
    let up = natural > below && above > below;
    let height = natural.min(if up { above } else { below }).max(row + INSET * 2.0);
    let y = if up { field.y - GAP - height } else { field.y + field.height + GAP };
    (Rectangle { x: field.x, y, width: field.width, height }, up)
}

fn max_scroll(count: usize, row: f32, height: f32) -> f32 {
    (count as f32 * row + INSET * 2.0 - height).max(0.0)
}

/// Which row is under `y`, measured from the top of the list.
fn row_at(y: f32, scroll: f32, row: f32, count: usize) -> Option<usize> {
    let index = ((y - INSET + scroll) / row).floor();
    (index >= 0.0 && (index as usize) < count).then_some(index as usize)
}

/// The scroll that brings `index` into view, moving as little as it can.
fn reveal(scroll: f32, index: usize, row: f32, height: f32, most: f32) -> f32 {
    let top = index as f32 * row + INSET;
    let bottom = top + row;
    let wanted = if top - INSET < scroll {
        top - INSET
    } else if bottom + INSET > scroll + height {
        bottom + INSET - height
    } else {
        scroll
    };
    wanted.clamp(0.0, most)
}

fn faded(color: Color, amount: f32) -> Color {
    Color { a: color.a * amount, ..color }
}

fn text_of(content: String, size: f32, bounds: Size, font: iced::Font, align_x: text::Alignment) -> Text<String, iced::Font> {
    Text {
        content,
        bounds,
        size: Pixels(size),
        line_height: text::LineHeight::default(),
        font,
        align_x,
        align_y: alignment::Vertical::Center,
        shaping: text::Shaping::Advanced,
        wrapping: text::Wrapping::None,
    }
}

impl<'a, T, M> Widget<M, Theme, Renderer> for Dropdown<'a, T, M>
where
    T: ToString + Clone + 'a,
    M: 'a,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State { showing: self.label.clone(), label_in: 1.0, ..State::default() })
    }

    fn size(&self) -> Size<Length> {
        Size::new(self.width, Length::Shrink)
    }

    fn layout(&mut self, _tree: &mut Tree, _renderer: &Renderer, limits: &layout::Limits) -> layout::Node {
        let height = self.text_size * 1.3 + self.padding.y();
        let limits = limits.width(self.width).height(height);
        layout::Node::new(limits.resolve(self.width, Length::Shrink, Size::new(0.0, height)))
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
        let state = tree.state.downcast_mut::<State>();
        let over = cursor.is_over(layout.bounds());
        if over != state.hovered {
            state.hovered = over;
            shell.request_redraw();
        }

        match event {
            Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => state.modifiers = *modifiers,
            // Ctrl and the wheel step through the options without opening the list.
            Event::Mouse(mouse::Event::WheelScrolled { delta })
                if over && !state.open && state.modifiers.command() && !shell.is_event_captured() =>
            {
                state.wheel += match *delta {
                    mouse::ScrollDelta::Lines { y, .. } => y,
                    mouse::ScrollDelta::Pixels { y, .. } => y / 60.0,
                };
                if state.wheel.abs() >= 1.0 {
                    let down = state.wheel < 0.0;
                    state.wheel = 0.0;
                    let next = match self.selected {
                        Some(i) if down => Some((i + 1).min(self.options.len().saturating_sub(1))),
                        Some(i) => Some(i.saturating_sub(1)),
                        None => down.then_some(0),
                    };
                    if let Some(next) = next
                        && self.selected != Some(next)
                        && let Some(option) = self.options.get(next)
                    {
                        shell.publish((self.on_select)(option.clone()));
                    }
                }
                shell.capture_event();
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
                if over && !shell.is_event_captured() && !self.options.is_empty() =>
            {
                state.open = !state.open;
                if state.open {
                    state.active = self.selected.unwrap_or(0);
                    state.glide = state.active as f32;
                    state.centre = true;
                }
                shell.capture_event();
                shell.request_redraw();
            }
            Event::Window(window::Event::RedrawRequested(now)) => {
                let target = theme::input(&theme::current(), if state.open {
                    ti::Status::Focused { is_hovered: over }
                } else if over {
                    ti::Status::Hovered
                } else {
                    ti::Status::Active
                });
                let open_to = if state.open { 1.0 } else { 0.0 };
                let k = step(tick(&mut state.last, *now, true));
                state.amount = num(state.amount, open_to, k, 0.004);
                state.glide = num(state.glide, state.active as f32, (k * 1.4).min(1.0), 0.004);
                state.scroll = num(state.scroll, state.scroll_to, k, 0.3);
                if state.showing != self.label {
                    state.showing = self.label.clone();
                    state.label_in = 0.0;
                }
                state.label_in = num(state.label_in, 1.0, k, 0.004);
                let shown = state.look.map_or(target, |s| s.toward(target, k));
                state.look = Some(shown);
                if shown != target
                    || state.label_in != 1.0
                    || state.amount != open_to
                    || (state.amount > 0.0 && (state.glide != state.active as f32 || state.scroll != state.scroll_to))
                {
                    shell.request_redraw();
                } else {
                    state.last = None;
                }
            }
            _ => {}
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
        viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<State>();
        let bounds = layout.bounds();
        let look = state.look.unwrap_or_else(|| theme::input(&theme::current(), ti::Status::Active));
        renderer.fill_quad(renderer::Quad { bounds, border: look.border, ..Default::default() }, look.background);

        const CHEVRON_SIZE: f32 = 12.0;
        let room = (bounds.width - self.padding.x() - CHEVRON_SIZE - 6.0).max(0.0);
        if let Some(label) = &self.label {
            renderer.fill_text(
                text_of(label.clone(), self.text_size, Size::new(room, bounds.height), renderer.default_font(), text::Alignment::Left),
                Point::new(bounds.x + self.padding.left, bounds.center_y()),
                faded(look.value, state.label_in),
                Rectangle { width: room, x: bounds.x + self.padding.left, ..bounds }.intersection(viewport).unwrap_or(bounds),
            );
        }

        let icon = Rectangle {
            x: bounds.x + bounds.width - self.padding.right - CHEVRON_SIZE,
            y: bounds.center_y() - CHEVRON_SIZE / 2.0,
            width: CHEVRON_SIZE,
            height: CHEVRON_SIZE,
        };
        adv_svg::Renderer::draw_svg(
            renderer,
            adv_svg::Svg::new(CHEVRON.clone())
                .color(theme::muted().toward(theme::text(), state.amount))
                .rotation(std::f32::consts::PI * state.amount),
            icon,
            *viewport,
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
        if cursor.is_over(layout.bounds()) {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::default()
        }
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        _renderer: &Renderer,
        _viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, M, Theme, Renderer>> {
        let state = tree.state.downcast_mut::<State>();
        if !state.open && state.amount <= 0.0 {
            return None;
        }
        let bounds = layout.bounds();
        let row = self.row_height();
        Some(overlay::Element::new(Box::new(List {
            state,
            field: Rectangle { x: bounds.x + translation.x, y: bounds.y + translation.y, ..bounds },
            options: &self.options,
            selected: self.selected,
            on_select: &*self.on_select,
            row,
            text_size: self.text_size,
            pad_x: self.padding.left,
        })))
    }
}

struct List<'b, T, M> {
    state: &'b mut State,
    field: Rectangle,
    options: &'b [T],
    selected: Option<usize>,
    on_select: &'b dyn Fn(T) -> M,
    row: f32,
    text_size: f32,
    pad_x: f32,
}

impl<T: ToString + Clone, M> List<'_, T, M> {
    fn pick(&mut self, index: usize, shell: &mut Shell<'_, M>) {
        if let Some(option) = self.options.get(index) {
            shell.publish((self.on_select)(option.clone()));
        }
        self.state.open = false;
        shell.capture_event();
        shell.request_redraw();
    }

    fn move_to(&mut self, index: usize, height: f32, shell: &mut Shell<'_, M>) {
        self.state.active = index;
        let most = max_scroll(self.options.len(), self.row, height);
        self.state.scroll_to = reveal(self.state.scroll_to, index, self.row, height, most);
        shell.capture_event();
        shell.request_redraw();
    }
}

impl<T: ToString + Clone, M> overlay::Overlay<M, Theme, Renderer> for List<'_, T, M> {
    fn layout(&mut self, _renderer: &Renderer, screen: Size) -> layout::Node {
        let (list, _) = place(self.field, self.options.len(), self.row, screen);
        let most = max_scroll(self.options.len(), self.row, list.height);
        let state = &mut *self.state;
        if state.centre {
            // Starts with the chosen row near the middle, already there.
            state.centre = false;
            state.scroll_to = state.active as f32 * self.row + self.row / 2.0 - list.height / 2.0;
            state.scroll = state.scroll_to;
        }
        // Also catches the options shrinking, or the window, while it's open.
        state.scroll_to = state.scroll_to.clamp(0.0, most);
        state.scroll = state.scroll.clamp(0.0, most);
        layout::Node::new(list.size()).move_to(list.position())
    }

    fn update(
        &mut self,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
    ) {
        let count = self.options.len();
        if !self.state.open {
            return;
        }
        // The options can run out while it's open: a list reloading underneath.
        if count == 0 {
            self.state.open = false;
            shell.request_redraw();
            return;
        }
        let bounds = layout.bounds();

        let most = max_scroll(count, self.row, bounds.height);
        let under = cursor.position_over(bounds).and_then(|p| row_at(p.y - bounds.y, self.state.scroll_to, self.row, count));

        match event {
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if let Some(index) = under {
                    if index != self.state.active {
                        self.state.active = index;
                        shell.request_redraw();
                    }
                }
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) if cursor.is_over(bounds) => {
                let lines = match *delta {
                    mouse::ScrollDelta::Lines { y, .. } => y * self.row * 2.0,
                    mouse::ScrollDelta::Pixels { y, .. } => y,
                };
                self.state.scroll_to = (self.state.scroll_to - lines).clamp(0.0, most);
                if let Some(index) = cursor.position_over(bounds).and_then(|p| row_at(p.y - bounds.y, self.state.scroll_to, self.row, count)) {
                    self.state.active = index;
                }
                shell.capture_event();
                shell.request_redraw();
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) if cursor.is_over(bounds) => match under {
                Some(index) => self.pick(index, shell),
                None => shell.capture_event(),
            },
            // Handled up here, before anything else in the window hears it, so
            // dismissing a list over a Delete button deletes nothing, and no
            // second dropdown can open while this one is.
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                self.state.open = false;
                shell.capture_event();
                shell.request_redraw();
            }
            // The page still moves under this, and a list left behind would float.
            Event::Mouse(mouse::Event::WheelScrolled { .. }) => {
                self.state.open = false;
                shell.request_redraw();
            }
            Event::Keyboard(keyboard::Event::KeyPressed { key: keyboard::Key::Named(key), .. }) => {
                use keyboard::key::Named;
                let active = self.state.active;
                match key {
                    Named::ArrowDown => self.move_to((active + 1).min(count - 1), bounds.height, shell),
                    Named::ArrowUp => self.move_to(active.saturating_sub(1), bounds.height, shell),
                    Named::Home => self.move_to(0, bounds.height, shell),
                    Named::End => self.move_to(count - 1, bounds.height, shell),
                    Named::Enter => self.pick(active, shell),
                    Named::Escape => {
                        self.state.open = false;
                        shell.capture_event();
                        shell.request_redraw();
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn draw(
        &self,
        renderer: &mut Renderer,
        theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
    ) {
        let bounds = layout.bounds();
        let amount = self.state.amount;
        let accent = theme.palette().primary;
        let count = self.options.len();
        let up = bounds.y < self.field.y;

        // The list unrolls from the field's edge, wherever that edge is.
        let height = bounds.height * amount;
        let shown = Rectangle {
            y: if up { bounds.y + bounds.height - height } else { bounds.y },
            height,
            ..bounds
        };
        renderer.fill_quad(
            renderer::Quad {
                bounds: shown,
                border: Border { radius: 10.0.into(), width: 1.0, color: faded(theme::control(), amount) },
                ..Default::default()
            },
            // Solid well before the rows arrive, so nothing behind shows through them.
            Background::Color(faded(theme::raised(), (amount * 2.5).min(1.0))),
        );

        let scroll = self.state.scroll;
        let drift = (1.0 - amount) * if up { 6.0 } else { -6.0 };
        renderer.with_layer(shown, |renderer| {
            let y_of = |index: f32| bounds.y + INSET + index * self.row - scroll + drift;

            // One highlight for the whole list: it travels, the rows don't flash.
            renderer.fill_quad(
                renderer::Quad {
                    bounds: Rectangle {
                        x: bounds.x + INSET,
                        y: y_of(self.state.glide),
                        width: bounds.width - INSET * 2.0,
                        height: self.row,
                    },
                    border: Border { radius: 6.0.into(), ..Default::default() },
                    ..Default::default()
                },
                Background::Color(faded(accent, 0.2 * amount)),
            );

            let first = ((scroll - INSET) / self.row).floor().max(0.0) as usize;
            let visible = (bounds.height / self.row).ceil() as usize + 1;
            for (nth, index) in (first..count.min(first + visible)).enumerate() {
                // Each row arrives a touch after the one above it.
                let arrived = crate::motion::Motion::stagger(amount, nth);
                let y = y_of(index as f32);
                let picked = self.selected == Some(index);
                if picked {
                    renderer.fill_quad(
                        renderer::Quad {
                            bounds: Rectangle { x: bounds.x + INSET, y: y + 7.0, width: 3.0, height: self.row - 14.0 },
                            border: Border { radius: 1.5.into(), ..Default::default() },
                            ..Default::default()
                        },
                        Background::Color(faded(accent, arrived)),
                    );
                }
                let lit = (1.0 - (self.state.glide - index as f32).abs()).clamp(0.0, 1.0);
                let color = if picked { theme::text() } else { theme::muted().toward(theme::text(), lit) };
                let x = bounds.x + self.pad_x + INSET + (1.0 - arrived) * 8.0;
                renderer.fill_text(
                    text_of(
                        self.options[index].to_string(),
                        self.text_size,
                        Size::new(bounds.width - self.pad_x - INSET * 2.0 - 20.0, self.row),
                        renderer.default_font(),
                        text::Alignment::Left,
                    ),
                    Point::new(x, y + self.row / 2.0),
                    faded(color, arrived),
                    shown,
                );
                if picked {
                    renderer.fill_text(
                        text_of(
                            Renderer::CHECKMARK_ICON.to_string(),
                            self.text_size - 2.0,
                            Size::new(16.0, self.row),
                            Renderer::ICON_FONT,
                            text::Alignment::Center,
                        ),
                        Point::new(bounds.x + bounds.width - INSET - 12.0, y + self.row / 2.0),
                        faded(accent, arrived),
                        shown,
                    );
                }
            }

            let most = max_scroll(count, self.row, bounds.height);
            if most > 0.0 {
                let bar = (bounds.height * bounds.height / (bounds.height + most)).max(24.0);
                renderer.fill_quad(
                    renderer::Quad {
                        bounds: Rectangle {
                            x: bounds.x + bounds.width - 5.0,
                            y: bounds.y + 3.0 + (bounds.height - bar - 6.0) * (scroll / most),
                            width: 3.0,
                            height: bar,
                        },
                        border: Border { radius: 1.5.into(), ..Default::default() },
                        ..Default::default()
                    },
                    Background::Color(faded(theme::muted(), 0.5 * amount)),
                );
            }
        });
    }

    fn mouse_interaction(
        &self,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &Renderer,
    ) -> mouse::Interaction {
        if self.state.open && cursor.is_over(layout.bounds()) {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::default()
        }
    }

}

impl<'a, T: ToString + Clone + 'a, M: 'a> From<Dropdown<'a, T, M>> for Element<'a, M> {
    fn from(dropdown: Dropdown<'a, T, M>) -> Self {
        Element::new(dropdown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Size = Size::new(800.0, 600.0);

    fn field(y: f32) -> Rectangle {
        Rectangle { x: 10.0, y, width: 200.0, height: 36.0 }
    }

    #[test]
    fn opens_below_when_it_fits_and_above_when_only_that_fits() {
        let (below, up) = place(field(100.0), 5, 30.0, SCREEN);
        assert!(!up);
        assert_eq!(below.y, 142.0);
        let (above, up) = place(field(520.0), 5, 30.0, SCREEN);
        assert!(up);
        assert_eq!(above.y + above.height, 514.0);
    }

    #[test]
    fn a_long_list_stays_inside_the_window() {
        for y in [0.0, 100.0, 300.0, 560.0] {
            let (list, _) = place(field(y), 400, 30.0, SCREEN);
            assert!(list.height <= MAX_HEIGHT);
            assert!(list.y >= 0.0 && list.y + list.height <= SCREEN.height, "{list:?}");
        }
    }

    #[test]
    fn finds_the_row_under_the_cursor_after_scrolling() {
        assert_eq!(row_at(INSET + 1.0, 0.0, 30.0, 10), Some(0));
        assert_eq!(row_at(INSET + 31.0, 0.0, 30.0, 10), Some(1));
        assert_eq!(row_at(INSET + 1.0, 90.0, 30.0, 10), Some(3));
        assert_eq!(row_at(0.0, 0.0, 30.0, 10), None, "the padding is no row");
        assert_eq!(row_at(1000.0, 0.0, 30.0, 10), None);
    }

    #[test]
    fn keyboard_moves_scroll_only_as_far_as_needed() {
        let most = max_scroll(100, 30.0, 280.0);
        assert_eq!(reveal(0.0, 2, 30.0, 280.0, most), 0.0, "already in view");
        assert!(reveal(0.0, 50, 30.0, 280.0, most) > 0.0);
        assert_eq!(reveal(most, 0, 30.0, 280.0, most), 0.0);
        assert_eq!(reveal(0.0, 99, 30.0, 280.0, most), most);
    }

    #[test]
    fn a_click_outside_only_closes_it() {
        use iced_test::simulator::{self, Simulator};
        let options = vec!["one".to_string(), "two".to_string()];
        let element: Element<'_, Picked> = iced::widget::column![
            dropdown(options.as_slice(), None::<String>, Picked::Value).width(200),
            iced::widget::Space::new().height(150),
            dropdown(options.as_slice(), None::<String>, Picked::Value).width(200),
        ]
        .into();
        let mut ui = Simulator::with_size(iced::Settings::default(), Size::new(400.0, 400.0), element);
        ui.point_at(Point::new(50.0, 190.0));
        ui.simulate(simulator::click());
        let _ = ui.snapshot(&theme::current());
        ui.point_at(Point::new(50.0, 15.0));
        ui.simulate(simulator::click());
        let _ = ui.snapshot(&theme::current());
        // Neither open now: the click on the other field closed the first and no more.
        ui.tap_key(keyboard::Key::Named(keyboard::key::Named::Enter));
        assert_eq!(ui.into_messages().count(), 0);
    }

    #[test]
    fn ctrl_and_the_wheel_step_through_the_options() {
        use iced_test::simulator::Simulator;
        let options = vec!["one".to_string(), "two".to_string()];
        let element: Element<'_, Picked> = dropdown(options.as_slice(), Some(&options[0]), Picked::Value).width(200).into();
        let mut ui = Simulator::with_size(iced::Settings::default(), Size::new(400.0, 400.0), element);
        ui.point_at(Point::new(50.0, 15.0));
        let wheel = Event::Mouse(mouse::Event::WheelScrolled { delta: mouse::ScrollDelta::Lines { x: 0.0, y: -1.0 } });
        ui.simulate([wheel.clone()]);
        ui.simulate([Event::Keyboard(keyboard::Event::ModifiersChanged(keyboard::Modifiers::COMMAND)), wheel]);
        // A trackpad's tiny movements add up instead of each being a step.
        let nudge = Event::Mouse(mouse::Event::WheelScrolled { delta: mouse::ScrollDelta::Pixels { x: 0.0, y: -2.0 } });
        ui.simulate([nudge]);
        assert_eq!(ui.into_messages().collect::<Vec<_>>(), vec![Picked::Value("two".to_string())]);
    }

    #[derive(Debug, Clone, PartialEq)]
    enum Picked {
        Value(String),
    }

    #[test]
    fn clicking_opens_it_and_clicking_a_row_chooses_that_row() {
        use iced_test::simulator::{self, Simulator};
        let options = vec!["one".to_string(), "two".to_string(), "three".to_string()];
        let element: Element<'_, Picked> = iced::widget::container(
            dropdown(options.as_slice(), None::<String>, Picked::Value).width(200),
        )
        .padding(10)
        .into();
        let mut ui = Simulator::with_size(iced::Settings::default(), Size::new(400.0, 400.0), element);
        ui.point_at(Point::new(50.0, 25.0));
        ui.simulate(simulator::click());
        // Row height 30, the list starts 6 under the 34-high field, past its 4 of padding.
        ui.point_at(Point::new(50.0, 10.0 + 34.2 + GAP + INSET + 45.0));
        ui.simulate(simulator::click());
        let messages: Vec<_> = ui.into_messages().collect();
        assert_eq!(messages, vec![Picked::Value("two".to_string())]);
    }
}
