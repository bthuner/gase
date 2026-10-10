//! A tiny **immediate-mode** UI toolkit: buttons, choices, lists and grids
//! that work with a keyboard, a gamepad, a mouse or a finger.
//!
//! # Immediate mode versus retained mode
//!
//! Most desktop toolkits are *retained*: you build a tree of widget
//! objects once, keep it alive, and update it when your data changes; the
//! widgets call you back when clicked. The toolkit owns a copy of your UI.
//!
//! An *immediate-mode* UI keeps no widget objects at all. Every frame, the
//! code that draws a screen simply runs from top to bottom, and each widget
//! call draws itself **and** reports what happened to it, in one go:
//!
//! ```text
//! if ui.button("Resume") { action = Resume; }
//! let d = ui.choice("Volume", &format!("{volume}%"));   // -1, 0 or +1
//! volume = (volume + 10 * d).clamp(0, 100);
//! ```
//!
//! The screen *is* a function of the app's state; there is nothing to keep
//! in sync, menus that change shape (a slot that becomes "Empty", a
//! setting that only exists on touch screens) need no bookkeeping, and the
//! whole interaction can be tested by calling the function with some input
//! and looking at the result. The cost — redrawing everything every frame —
//! is irrelevant at this size: a menu is a few thousand pixels of text,
//! and it is only drawn while it is shown.
//!
//! The only state that must survive between frames is *which item has the
//! focus* and *how far the list is scrolled*: that is [`Focus`], one per
//! open screen, owned by the app.
//!
//! # Navigation without a mouse
//!
//! Keyboard and gamepad users move the focus with directions
//! ([`Nav`]). Instead of hard-coding "down = next item", the toolkit
//! remembers where every item was drawn in the previous frame and moves
//! to the nearest item in the pressed direction (*spatial navigation*, as
//! on game consoles and TVs). The same code then works for a vertical
//! list, a grid of save slots, or both on one screen. Left and right
//! change the value of an adjustable item (a choice) instead.
//!
//! # Pointers
//!
//! A mouse or a finger activates an item on **release** over the item it
//! was pressed on (so you can slide off to cancel). Dragging a list
//! vertically, or the mouse wheel, scrolls it. A mouse also moves the
//! focus by hovering, so keyboard and mouse can be mixed freely.

use crate::canvas::{Canvas, Image, Rect};
use crate::font::{self, text_scaled, text_width};
use crate::input::Nav;
use crate::theme;

/// Pointer activity during one frame, in UI pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PointerInput {
    /// Where a mouse moved to (hover focus); not set for touches.
    pub hover: Option<(i32, i32)>,
    /// Where the pointer went down this frame.
    pub press: Option<(i32, i32)>,
    /// Where it went up this frame (a click if over the pressed item).
    pub release: Option<(i32, i32)>,
    /// Current position while held down.
    pub held: Option<(i32, i32)>,
    /// Pixels to scroll the list by (drag or wheel; positive shows content
    /// further down).
    pub scroll: i32,
}

/// What the toolkit remembers about one item between frames.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ItemInfo {
    rect: Rect,
    adjustable: bool,
}

/// The state of one screen that outlives a frame: which item has the focus
/// and how far its list is scrolled.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Focus {
    /// Index of the focused item, counting focusable items in drawing order.
    pub index: usize,
    /// Vertical scroll of the screen's list, in UI pixels.
    pub scroll: i32,
    /// Items as drawn in the last frame (for navigation and hit testing).
    items: Vec<ItemInfo>,
    /// The item a pointer went down on.
    pressed: Option<usize>,
}

impl Focus {
    /// Focus on item `index` (e.g. to restore a selection).
    #[must_use]
    pub fn at(index: usize) -> Self {
        Self {
            index,
            ..Self::default()
        }
    }

    /// Number of items drawn in the last frame.
    #[must_use]
    pub fn item_count(&self) -> usize {
        self.items.len()
    }

    /// Where item `index` was drawn in the last frame.
    #[must_use]
    pub fn item_rect(&self, index: usize) -> Option<Rect> {
        self.items.get(index).map(|i| i.rect)
    }
}

/// What happened to an item this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Response {
    pub rect: Rect,
    pub focused: bool,
    /// The pointer is held down on it.
    pub pressed: bool,
    /// Activated: Confirm while focused, or a click/tap.
    pub clicked: bool,
    /// Left/Right while focused (−1 or +1), for adjustable items.
    pub adjust: i32,
    /// Where the click landed (for items with several hot spots).
    pub click_at: Option<(i32, i32)>,
}

/// Direction of a spatial move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dir {
    Up,
    Down,
    Left,
    Right,
}

/// Pick the item to move to from `from` in direction `dir`, using the
/// previous frame's item rectangles. Vertical moves wrap around.
fn spatial_move(items: &[ItemInfo], from: usize, dir: Dir) -> usize {
    let Some(current) = items.get(from) else {
        return 0;
    };
    let (cx, cy) = current.rect.center();
    // Distance along the direction, plus a penalty for straying sideways,
    // so the item straight ahead wins over a closer one diagonally.
    let score = |r: Rect| -> Option<i32> {
        let (x, y) = r.center();
        let (along, across) = match dir {
            Dir::Down => (y - cy, x - cx),
            Dir::Up => (cy - y, x - cx),
            Dir::Right => (x - cx, y - cy),
            Dir::Left => (cx - x, y - cy),
        };
        (along > 0).then_some(along + 2 * across.abs())
    };
    let best = items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| score(item.rect).map(|s| (s, i)))
        .min();
    if let Some((_, i)) = best {
        return i;
    }
    // Nothing that way: wrap vertical moves to the far end of the column.
    let wrap_key = |r: Rect| {
        let (x, y) = r.center();
        let column = (x - cx).abs();
        match dir {
            Dir::Down => Some((y, column)),
            Dir::Up => Some((-y, column)),
            _ => None,
        }
    };
    items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| wrap_key(item.rect).map(|k| (k, i)))
        .min()
        .map_or(from, |(_, i)| i)
}

/// One frame of drawing and interaction for one screen.
#[derive(Debug)]
pub struct Ui<'a> {
    pub canvas: &'a mut Canvas,
    focus: &'a mut Focus,
    pointer: PointerInput,
    confirm: bool,
    back: bool,
    adjust: i32,
    typed: Option<char>,
    /// The focus was moved by a direction: scroll it into view.
    focus_moved: bool,
    items: Vec<ItemInfo>,
    /// The column widgets are laid out in (x and width).
    pub column: Rect,
    /// The next widget's top.
    pub y: i32,
    /// The scrolled list area, while one is open.
    list: Option<Rect>,
    list_top: i32,
    content_bottom: i32,
    /// The frame should be drawn again (the focus or scroll settled).
    pub redraw: bool,
}

impl<'a> Ui<'a> {
    /// Start a frame: apply this frame's navigation to the focus.
    pub fn new(
        canvas: &'a mut Canvas,
        focus: &'a mut Focus,
        navs: &[Nav],
        pointer: PointerInput,
    ) -> Self {
        let width = (canvas.width as i32 - 2 * theme::MARGIN).min(theme::COLUMN);
        let column = Rect::new(
            (canvas.width as i32 - width) / 2,
            0,
            width,
            canvas.height as i32,
        );
        let mut ui = Self {
            canvas,
            focus,
            pointer,
            confirm: false,
            back: false,
            adjust: 0,
            typed: None,
            focus_moved: false,
            items: Vec::new(),
            column,
            y: theme::MARGIN,
            list: None,
            list_top: 0,
            content_bottom: 0,
            redraw: false,
        };
        for &nav in navs {
            ui.navigate(nav);
        }
        ui.focus.scroll += pointer.scroll;
        if pointer.scroll != 0 {
            // Scrolling by hand cancels a pending tap.
            ui.focus.pressed = None;
        }
        ui
    }

    fn navigate(&mut self, nav: Nav) {
        let f = &mut *self.focus;
        let adjustable = f.items.get(f.index).is_some_and(|i| i.adjustable);
        let mut go = |dir: Dir| f.index = spatial_move(&f.items, f.index, dir);
        match nav {
            Nav::Left | Nav::Right if adjustable => {
                self.adjust += if nav == Nav::Left { -1 } else { 1 };
            }
            Nav::Up => go(Dir::Up),
            Nav::Down => go(Dir::Down),
            Nav::Left => go(Dir::Left),
            Nav::Right => go(Dir::Right),
            Nav::PageUp | Nav::PageDown => {
                // Several rows at once, without wrapping around.
                let step = if nav == Nav::PageUp { -8 } else { 8 };
                let last = self.focus.items.len().saturating_sub(1) as i64;
                self.focus.index = (self.focus.index as i64 + step).clamp(0, last) as usize;
            }
            Nav::Home => self.focus.index = 0,
            Nav::End => self.focus.index = self.focus.items.len().saturating_sub(1),
            Nav::Confirm => self.confirm = true,
            Nav::Back => self.back = true,
            Nav::Char(c) => self.typed = Some(c),
        }
        if matches!(
            nav,
            Nav::Up
                | Nav::Down
                | Nav::Left
                | Nav::Right
                | Nav::PageUp
                | Nav::PageDown
                | Nav::Home
                | Nav::End
        ) {
            self.focus_moved = true;
        }
    }

    /// Back was pressed this frame (the screen decides what it means).
    #[must_use]
    pub fn back(&self) -> bool {
        self.back
    }

    /// A letter or digit typed this frame.
    #[must_use]
    pub fn typed(&self) -> Option<char> {
        self.typed
    }

    /// The focused item's index.
    #[must_use]
    pub fn focused(&self) -> usize {
        self.focus.index
    }

    /// Move the focus (e.g. type-to-jump) and scroll to it.
    pub fn set_focus(&mut self, index: usize) {
        self.focus.index = index;
        self.focus_moved = true;
    }

    /// Register a focusable item at `rect` and find out what happened to it.
    /// This is the one primitive all widgets are built on.
    pub fn item(&mut self, rect: Rect, adjustable: bool) -> Response {
        let id = self.items.len();
        self.items.push(ItemInfo { rect, adjustable });
        self.content_bottom = self.content_bottom.max(rect.bottom());
        // Only the visible part of an item in a scrolled list can be hit.
        let hit = self.list.map_or(rect, |list| rect.intersect(list));
        let inside = |p: Option<(i32, i32)>| p.is_some_and(|(x, y)| hit.contains(x, y));
        if inside(self.pointer.press) {
            self.focus.pressed = Some(id);
            self.focus.index = id;
        }
        if inside(self.pointer.hover) {
            self.focus.index = id;
        }
        let mut r = Response {
            rect,
            focused: self.focus.index == id,
            ..Response::default()
        };
        if self.focus.pressed == Some(id) && inside(self.pointer.release) {
            r.clicked = true;
            r.click_at = self.pointer.release;
        }
        r.pressed = self.focus.pressed == Some(id) && inside(self.pointer.held);
        if r.focused {
            r.clicked |= self.confirm;
            r.adjust = if adjustable { self.adjust } else { 0 };
        }
        r
    }

    /// End the frame: remember the items and settle the focus. Returns
    /// true if the frame should be drawn again because the focus or the
    /// scroll position changed while drawing (immediate-mode UIs settle in
    /// at most one extra frame).
    pub fn finish(self) -> bool {
        let Ui {
            focus,
            items,
            pointer,
            mut redraw,
            ..
        } = self;
        if pointer.release.is_some() || pointer.held.is_none() && pointer.press.is_none() {
            focus.pressed = None;
        }
        if !items.is_empty() && focus.index >= items.len() {
            focus.index = items.len() - 1;
            redraw = true;
        }
        focus.items = items;
        redraw
    }

    // --- Layout -------------------------------------------------------------

    /// Big title, optional caption, and the red rule under them.
    pub fn title(&mut self, title: &str, caption: Option<&str>) {
        let x = self.column.x;
        text_scaled(self.canvas, x, self.y, title, theme::TEXT, 2);
        self.y += 16 + 4;
        if let Some(caption) = caption {
            let max = (self.column.w / 8) as usize;
            text_scaled(
                self.canvas,
                x,
                self.y,
                &font::ellipsize(caption, max, false),
                theme::DIM,
                1,
            );
            self.y += 8 + 4;
        }
        self.canvas.fill(Rect::new(x, self.y, 24, 2), theme::ACCENT);
        self.canvas.fill(
            Rect::new(x + 24, self.y, self.column.w - 24, 1),
            theme::LINE,
        );
        self.y += 2 + 8;
    }

    /// Start a scrolling list that ends at `bottom` (exclusive).
    pub fn begin_list(&mut self, bottom: i32) {
        let top = self.y;
        let area = Rect::new(0, top, self.canvas.width as i32, (bottom - top).max(0));
        self.list = Some(area);
        self.list_top = top;
        self.content_bottom = top;
        self.canvas.set_clip(Some(area));
        self.y = top - self.focus.scroll;
    }

    /// End the list: clamp the scroll, bring the focused item into view,
    /// and draw ▲ ▼ marks when there is more to see.
    pub fn end_list(&mut self) {
        let Some(area) = self.list.take() else {
            return;
        };
        self.canvas.set_clip(None);
        let content = self.content_bottom + self.focus.scroll - self.list_top;
        let max_scroll = (content - area.h).max(0);
        let mut scroll = self.focus.scroll.clamp(0, max_scroll);
        if self.focus_moved {
            if let Some(item) = self.items.get(self.focus.index) {
                // `item.rect` was drawn with the old scroll.
                let top = item.rect.y + self.focus.scroll - scroll;
                let margin = theme::ROW / 2;
                if top < area.y + margin {
                    scroll -= area.y + margin - top;
                } else if top + item.rect.h > area.bottom() - margin {
                    scroll += top + item.rect.h - (area.bottom() - margin);
                }
                scroll = scroll.clamp(0, max_scroll);
            }
        }
        if scroll != self.focus.scroll {
            self.focus.scroll = scroll;
            self.redraw = true;
        }
        let x = self.column.right() - 8;
        if scroll > 0 {
            text_scaled(self.canvas, x, area.y - 9, "▲", theme::DIM, 1);
        }
        if scroll < max_scroll {
            text_scaled(self.canvas, x, area.bottom() + 1, "▼", theme::DIM, 1);
        }
        self.y = area.bottom();
    }

    /// Vertical space.
    pub fn space(&mut self, pixels: i32) {
        self.y += pixels;
    }

    /// A small caption above a group of items.
    pub fn section(&mut self, label: &str) {
        self.y += 4;
        text_scaled(self.canvas, self.column.x + 2, self.y, label, theme::DIM, 1);
        self.y += 8 + 6;
    }

    /// A line of text (not focusable).
    pub fn text(&mut self, line: &str, color: u32) {
        let max = (self.column.w / 8) as usize;
        text_scaled(
            self.canvas,
            self.column.x + 2,
            self.y,
            &font::ellipsize(line, max, false),
            color,
            1,
        );
        self.y += 8 + 4;
    }

    /// Centred hint text whose last line is at height `y`. Hints are
    /// phrases separated by ` · `; they wrap onto a second line (upwards)
    /// rather than run past the column.
    pub fn hint(&mut self, y: i32, line: &str) {
        let max = ((self.column.w + 24) / 8).max(8) as usize;
        let mut lines: Vec<String> = Vec::new();
        for part in line.split(" · ") {
            match lines.last_mut() {
                Some(last) if last.chars().count() + 3 + part.chars().count() <= max => {
                    last.push_str(" · ");
                    last.push_str(part);
                }
                _ => lines.push(font::ellipsize(part, max, false)),
            }
        }
        let n = lines.len() as i32;
        for (i, l) in lines.iter().enumerate() {
            let x = (self.canvas.width as i32 - text_width(l, 1)) / 2;
            text_scaled(
                self.canvas,
                x,
                y - (n - 1 - i as i32) * 11,
                l,
                theme::DIM,
                1,
            );
        }
    }

    /// The rectangle of the next row.
    fn next_row(&mut self) -> Rect {
        let r = Rect::new(self.column.x, self.y, self.column.w, theme::ROW);
        self.y += theme::ROW + theme::GAP;
        r
    }

    /// Draw a row's background according to its state.
    fn row_background(&mut self, r: &Response) {
        if r.pressed {
            self.canvas.fill(r.rect, theme::PRESSED);
        } else if r.focused {
            self.canvas.fill(r.rect, theme::FOCUS);
        }
        if r.focused {
            self.canvas
                .fill(Rect::new(r.rect.x, r.rect.y, 2, r.rect.h), theme::ACCENT);
        }
    }

    /// A button; true when activated.
    pub fn button(&mut self, label: &str) -> bool {
        self.button_with(None, label, None, theme::TEXT)
    }

    /// A button with an optional icon, label and right-aligned detail.
    pub fn button_with(
        &mut self,
        icon: Option<char>,
        label: &str,
        detail: Option<&str>,
        color: u32,
    ) -> bool {
        let rect = self.next_row();
        let r = self.item(rect, false);
        self.row_background(&r);
        let mut x = rect.x + 10;
        let ty = rect.y + (theme::ROW - 8) / 2;
        if let Some(icon) = icon {
            let icon_color = if r.focused { theme::ACCENT } else { theme::DIM };
            text_scaled(self.canvas, x, ty, &icon.to_string(), icon_color, 1);
            x += 14;
        }
        let detail_w = detail.map_or(0, |d| text_width(d, 1) + 10);
        let max = ((rect.right() - x - detail_w - 6) / 8).max(1) as usize;
        text_scaled(
            self.canvas,
            x,
            ty,
            &font::ellipsize(label, max, false),
            color,
            1,
        );
        if let Some(detail) = detail {
            let dx = rect.right() - 8 - text_width(detail, 1);
            text_scaled(self.canvas, dx, ty, detail, theme::DIM, 1);
        }
        r.clicked
    }

    /// A row with a label and a value (gold), activated as a whole: for
    /// lists of bindings.
    pub fn value_row(&mut self, label: &str, value: &str) -> bool {
        let rect = self.next_row();
        let r = self.item(rect, false);
        self.row_background(&r);
        let ty = rect.y + (theme::ROW - 8) / 2;
        text_scaled(self.canvas, rect.x + 10, ty, label, theme::TEXT, 1);
        let vx = rect.right() - 8 - text_width(value, 1);
        text_scaled(self.canvas, vx, ty, value, theme::GOLD, 1);
        r.clicked
    }

    /// A setting with a value: Left/Right (or the arrows) return −1/+1,
    /// activating returns +1.
    pub fn choice(&mut self, label: &str, value: &str) -> i32 {
        let rect = self.next_row();
        let r = self.item(rect, true);
        self.row_background(&r);
        let ty = rect.y + (theme::ROW - 8) / 2;
        text_scaled(self.canvas, rect.x + 10, ty, label, theme::TEXT, 1);
        // "◀ value ▶", right-aligned.
        let right = rect.right() - 8;
        let value_x = right - 8 - 6 - text_width(value, 1);
        let left_arrow = value_x - 6 - 8;
        let arrows = if r.focused {
            theme::ACCENT
        } else {
            theme::FAINT
        };
        text_scaled(self.canvas, left_arrow, ty, "◀", arrows, 1);
        text_scaled(self.canvas, value_x, ty, value, theme::GOLD, 1);
        text_scaled(self.canvas, right - 8, ty, "▶", arrows, 1);
        if r.clicked {
            // A tap on the left arrow goes back; anywhere else forward.
            return match r.click_at {
                Some((x, _)) if x < value_x - 2 && x >= left_arrow - 8 => -1,
                _ => 1,
            };
        }
        r.adjust
    }

    /// An on/off setting; true when it should flip.
    pub fn toggle(&mut self, label: &str, on: bool) -> bool {
        self.choice(label, if on { "On" } else { "Off" }) != 0
    }

    /// A save-state slot in a grid: a picture (or "Empty") and a caption.
    pub fn slot(
        &mut self,
        rect: Rect,
        preview: Option<Image<'_>>,
        caption: &str,
        current: bool,
    ) -> bool {
        let r = self.item(rect, false);
        let pic = Rect::new(rect.x + 3, rect.y + 3, rect.w - 6, rect.h - 6 - 12);
        let border = if r.focused {
            theme::ACCENT
        } else if r.pressed {
            theme::DIM
        } else {
            theme::LINE
        };
        self.canvas.fill(
            rect,
            if r.focused {
                theme::FOCUS
            } else {
                theme::PANEL
            },
        );
        self.canvas
            .frame(rect, if r.focused { 2 } else { 1 }, border);
        match preview {
            Some(image) => self.canvas.draw_scaled(pic, image, false),
            None => {
                self.canvas.fill(pic, theme::BACKGROUND);
                let (cx, cy) = pic.center();
                text_scaled(self.canvas, cx - 20, cy - 4, "Empty", theme::FAINT, 1);
            }
        }
        let color = if current {
            theme::GOLD
        } else if r.focused {
            theme::TEXT
        } else {
            theme::DIM
        };
        let cw = text_width(caption, 1);
        text_scaled(
            self.canvas,
            rect.x + (rect.w - cw) / 2,
            pic.bottom() + 3,
            caption,
            color,
            1,
        );
        r.clicked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run one frame of a three-button menu; returns the clicked index.
    fn frame(focus: &mut Focus, navs: &[Nav], pointer: PointerInput) -> Option<usize> {
        let mut canvas = Canvas::new(320, 200, 0);
        let mut ui = Ui::new(&mut canvas, focus, navs, pointer);
        let mut clicked = None;
        for (i, label) in ["One", "Two", "Three"].iter().enumerate() {
            if ui.button(label) {
                clicked = Some(i);
            }
        }
        ui.finish();
        clicked
    }

    #[test]
    fn keyboard_moves_and_activates() {
        let mut f = Focus::default();
        frame(&mut f, &[], PointerInput::default()); // learn the layout
        assert_eq!(f.item_count(), 3);
        assert_eq!(frame(&mut f, &[Nav::Down], PointerInput::default()), None);
        assert_eq!(f.index, 1);
        assert_eq!(
            frame(&mut f, &[Nav::Down, Nav::Confirm], PointerInput::default()),
            Some(2)
        );
        // Down from the last item wraps to the first; Up wraps back.
        frame(&mut f, &[Nav::Down], PointerInput::default());
        assert_eq!(f.index, 0);
        frame(&mut f, &[Nav::Up], PointerInput::default());
        assert_eq!(f.index, 2);
        frame(&mut f, &[Nav::Home], PointerInput::default());
        assert_eq!(f.index, 0);
    }

    #[test]
    fn pointer_click_needs_press_and_release_on_the_same_item() {
        let mut f = Focus::default();
        frame(&mut f, &[], PointerInput::default());
        let second = f.item_rect(1).unwrap().center();
        let third = f.item_rect(2).unwrap().center();
        let press = PointerInput {
            press: Some(second),
            held: Some(second),
            ..PointerInput::default()
        };
        assert_eq!(frame(&mut f, &[], press), None);
        assert_eq!(f.index, 1, "pressing focuses");
        let release = PointerInput {
            release: Some(second),
            ..PointerInput::default()
        };
        assert_eq!(frame(&mut f, &[], release), Some(1));
        // Pressed on one item, released on another: nothing.
        frame(
            &mut f,
            &[],
            PointerInput {
                press: Some(second),
                held: Some(second),
                ..PointerInput::default()
            },
        );
        let moved = PointerInput {
            release: Some(third),
            ..PointerInput::default()
        };
        assert_eq!(frame(&mut f, &[], moved), None);
        // A quick tap within one frame works too.
        let tap = PointerInput {
            press: Some(third),
            release: Some(third),
            ..PointerInput::default()
        };
        assert_eq!(frame(&mut f, &[], tap), Some(2));
    }

    #[test]
    fn mouse_hover_moves_the_focus() {
        let mut f = Focus::default();
        frame(&mut f, &[], PointerInput::default());
        let third = f.item_rect(2).unwrap().center();
        frame(
            &mut f,
            &[],
            PointerInput {
                hover: Some(third),
                ..PointerInput::default()
            },
        );
        assert_eq!(f.index, 2);
    }

    #[test]
    fn spatial_navigation_in_a_grid() {
        // A 3 × 2 grid of 10 × 10 cells.
        let items: Vec<ItemInfo> = (0..6)
            .map(|i| ItemInfo {
                rect: Rect::new((i % 3) * 12, (i / 3) * 12, 10, 10),
                adjustable: false,
            })
            .collect();
        assert_eq!(spatial_move(&items, 0, Dir::Right), 1);
        assert_eq!(spatial_move(&items, 1, Dir::Down), 4);
        assert_eq!(spatial_move(&items, 4, Dir::Left), 3);
        assert_eq!(spatial_move(&items, 5, Dir::Up), 2);
        // No wrap sideways; vertical moves wrap within the column.
        assert_eq!(spatial_move(&items, 2, Dir::Right), 2);
        assert_eq!(spatial_move(&items, 4, Dir::Down), 1);
        assert_eq!(spatial_move(&items, 1, Dir::Up), 4);
    }

    #[test]
    fn choices_take_left_and_right() {
        let mut f = Focus::default();
        let mut value = 5;
        for navs in [
            &[][..],
            &[Nav::Right, Nav::Right][..],
            &[Nav::Left][..],
            &[Nav::Confirm][..],
        ] {
            let mut canvas = Canvas::new(320, 200, 0);
            let mut ui = Ui::new(&mut canvas, &mut f, navs, PointerInput::default());
            value += ui.choice("Volume", &value.to_string());
            ui.finish();
        }
        assert_eq!(value, 5 + 2 - 1 + 1);
    }

    #[test]
    fn long_lists_scroll_to_the_focus() {
        let mut f = Focus::default();
        let draw = |f: &mut Focus, navs: &[Nav]| {
            let mut canvas = Canvas::new(320, 120, 0);
            let mut ui = Ui::new(&mut canvas, f, navs, PointerInput::default());
            ui.begin_list(100);
            for i in 0..30 {
                ui.button(&format!("Item {i}"));
            }
            ui.end_list();
            let redraw = ui.redraw;
            ui.finish();
            redraw
        };
        draw(&mut f, &[]);
        assert_eq!(f.scroll, 0);
        assert!(
            draw(&mut f, &[Nav::End]),
            "the list scrolls, asking for a redraw"
        );
        assert!(f.scroll > 0);
        let rect = {
            draw(&mut f, &[]);
            f.item_rect(29).unwrap()
        };
        assert!(rect.bottom() <= 100, "the last item is visible: {rect:?}");
        draw(&mut f, &[Nav::Home]);
        assert_eq!(f.scroll, 0);
    }
}
