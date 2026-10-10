//! On-screen controls for touch screens: a d-pad, the A B C (and X Y Z)
//! buttons, Start, Mode and a menu button, drawn over the game.
//!
//! # Multi-touch
//!
//! A phone reports every finger separately, each with an id that stays the
//! same from the moment it touches the screen until it lifts. The controls
//! keep a small table of the fingers they own and what each one started
//! on, and every frame the console buttons are simply the union of what
//! every finger presses. That gives multi-touch for free: a thumb on the
//! d-pad and another on B, or a thumb rolling from B onto C.
//!
//! Two rules make the controls feel right:
//!
//! * A finger that went down on the **d-pad keeps steering it** wherever
//!   it slides, even off the drawn circle: thumbs drift, and losing the
//!   direction mid-jump is infuriating. Its direction is the angle from
//!   the d-pad's centre (see [`dpad_directions`]).
//! * A finger that went down on a **button can slide** to other buttons,
//!   and presses every button whose (slightly enlarged) circle it is in,
//!   so the gap between two buttons presses both: B+C with one thumb.
//!
//! # Layout
//!
//! In landscape the game fills the middle and the controls sit on either
//! side, over the borders. In portrait the game is pinned to the top of
//! the screen and the controls fill the space below it. All sizes derive
//! from the screen size so the same code serves a 5" phone and a tablet.

use gase_core::Buttons;

use crate::canvas::{Canvas, Rect, with_alpha};
use crate::font::{text_scaled, text_width};
use crate::input::{ConsoleButton, PointerPhase};
use crate::theme;

/// The shape of a control, for drawing and hit testing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Circle {
        radius: i32,
    },
    /// A rounded rectangle centred on the control's position.
    Pill {
        w: i32,
        h: i32,
    },
}

/// What a control does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Button(ConsoleButton),
    /// Opens the pause menu.
    Menu,
}

/// A button on the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TouchButton {
    pub control: Control,
    pub center: (i32, i32),
    pub shape: Shape,
    pub label: &'static str,
}

impl TouchButton {
    /// Is (`x`, `y`) on the button? Circles are enlarged by a fifth: a
    /// fingertip is wide and imprecise, and missing a button is worse than
    /// pressing a neighbour too.
    #[must_use]
    pub fn hit(&self, x: i32, y: i32) -> bool {
        let (dx, dy) = (x - self.center.0, y - self.center.1);
        match self.shape {
            Shape::Circle { radius } => {
                let r = radius * 6 / 5;
                dx * dx + dy * dy <= r * r
            }
            Shape::Pill { w, h } => dx.abs() <= w / 2 + 4 && dy.abs() <= h / 2 + 4,
        }
    }
}

/// Where every control is, for one screen size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub dpad_center: (i32, i32),
    pub dpad_radius: i32,
    pub buttons: Vec<TouchButton>,
}

impl Layout {
    /// Lay the controls out on a `w` × `h` overlay (UI pixels) where the
    /// game picture covers `game`. `six_button` adds X, Y, Z and Mode.
    #[must_use]
    pub fn new(w: i32, h: i32, game: Rect, six_button: bool) -> Self {
        let portrait = h > w && h - game.bottom() > h / 3;
        let (area, radius) = if portrait {
            let top = game.bottom() + 4;
            let area = Rect::new(0, top, w, h - top);
            (area, ((w * 18 / 100).min(area.h * 28 / 100)).clamp(22, 60))
        } else {
            (Rect::new(0, 0, w, h), (h * 17 / 100).clamp(22, 56))
        };
        let m = (radius / 3).max(8);
        // In landscape, keep the buttons within the border beside the game
        // where possible; never smaller than a fingertip.
        let side = if portrait {
            w
        } else {
            game.x.max(w - game.right())
        };
        let b = (radius * 45 / 100).min(side * 10 / 66).max(13);
        let pill = (b * 3, 12);

        // Start/Mode: a bottom row in portrait; the side borders in
        // landscape (see below).
        let start_y = area.bottom() - m - pill.1 / 2;
        let dpad_y = if portrait {
            area.y + (area.h - pill.1 - m) / 2
        } else {
            h - m - radius - 4
        };
        let dpad_center = (m + radius, dpad_y);

        let mut buttons = Vec::new();
        let mut push = |control, center, shape, label| {
            buttons.push(TouchButton {
                control,
                center,
                shape,
                label,
            });
        };
        // A B C rise to the right like on the Mega Drive pad, with X Y Z
        // above them on the six-button model.
        let step = b * 22 / 10;
        let c_x = w - m - b;
        let c_y = dpad_y - b / 2 + if six_button { step / 2 } else { 0 };
        let row = [
            (ConsoleButton::A, "A", ConsoleButton::X, "X"),
            (ConsoleButton::B, "B", ConsoleButton::Y, "Y"),
            (ConsoleButton::C, "C", ConsoleButton::Z, "Z"),
        ];
        for (i, &(low, low_label, high, high_label)) in row.iter().enumerate() {
            let k = 2 - i as i32; // distance from C
            let x = c_x - k * step;
            let y = c_y + k * b / 2;
            let circle = Shape::Circle { radius: b };
            push(Control::Button(low), (x, y), circle, low_label);
            if six_button {
                push(Control::Button(high), (x, y - step), circle, high_label);
            }
        }
        let pill_shape = Shape::Pill {
            w: pill.0,
            h: pill.1,
        };
        if portrait {
            let gap = pill.0 / 2 + 6;
            push(
                Control::Button(ConsoleButton::Start),
                (w / 2 + gap, start_y),
                pill_shape,
                "START",
            );
            if six_button {
                push(
                    Control::Button(ConsoleButton::Mode),
                    (w / 2 - gap, start_y),
                    pill_shape,
                    "MODE",
                );
            }
        } else {
            // In the side borders, above the thumbs' resting places: Start
            // over the face buttons, Mode over the d-pad. (At the bottom
            // centre they would cover the picture.)
            // C (or Z above it) is the highest face button.
            let cluster_top = c_y - if six_button { step } else { 0 } - b;
            let start_y = cluster_top - m - pill.1 / 2;
            push(
                Control::Button(ConsoleButton::Start),
                (c_x - step, start_y),
                pill_shape,
                "START",
            );
            if six_button {
                let mode_y = dpad_y - radius - m - pill.1 / 2;
                push(
                    Control::Button(ConsoleButton::Mode),
                    (dpad_center.0, mode_y),
                    pill_shape,
                    "MODE",
                );
            }
        }
        let menu_r = (b * 3 / 4).max(10);
        let menu_y = if portrait {
            area.y + m + menu_r
        } else {
            m + menu_r
        };
        push(
            Control::Menu,
            (w - m - menu_r, menu_y),
            Shape::Circle { radius: menu_r },
            "",
        );
        Self {
            dpad_center,
            dpad_radius: radius,
            buttons,
        }
    }

    /// Is (`x`, `y`) close enough to the d-pad to grab it?
    #[must_use]
    pub fn dpad_hit(&self, x: i32, y: i32) -> bool {
        let (dx, dy) = (x - self.dpad_center.0, y - self.dpad_center.1);
        let r = self.dpad_radius * 5 / 4;
        dx * dx + dy * dy <= r * r
    }
}

/// The directions pressed by a finger at (`dx`, `dy`) from the centre of
/// a d-pad of `radius`.
///
/// The circle is cut into eight 45° sectors: four straight directions
/// centred on the axes and four diagonals between them. Instead of
/// computing an angle with `atan2`, compare the two offsets: the finger is
/// in a straight sector when one offset is less than tan 22.5° ≈ 0.414
/// times the other. A small dead zone in the middle presses nothing.
#[must_use]
pub fn dpad_directions(dx: i32, dy: i32, radius: i32) -> Buttons {
    let dead = radius / 5;
    if dx * dx + dy * dy < dead * dead {
        return Buttons::default();
    }
    let (ax, ay) = (i64::from(dx.abs()), i64::from(dy.abs()));
    // Straight sectors press one direction; diagonal sectors both.
    let (h, v) = if ay * 1000 <= ax * 414 {
        (true, false)
    } else if ax * 1000 <= ay * 414 {
        (false, true)
    } else {
        (true, true)
    };
    let mut b = Buttons::default();
    if h {
        b.set(
            if dx < 0 {
                Buttons::LEFT
            } else {
                Buttons::RIGHT
            },
            true,
        );
    }
    if v {
        b.set(if dy < 0 { Buttons::UP } else { Buttons::DOWN }, true);
    }
    b
}

/// What a finger went down on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    Dpad,
    Buttons,
    Menu,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Finger {
    id: u64,
    target: Target,
    x: i32,
    y: i32,
}

/// The on-screen controls and the fingers on them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TouchControls {
    layout: Option<Layout>,
    fingers: Vec<Finger>,
    menu_tapped: bool,
}

impl TouchControls {
    /// Use `layout` (after a resize). Fingers are released.
    pub fn set_layout(&mut self, layout: Layout) {
        self.layout = Some(layout);
        self.fingers.clear();
    }

    #[must_use]
    pub fn layout(&self) -> Option<&Layout> {
        self.layout.as_ref()
    }

    /// Feed a touch (UI pixels). Returns true if the controls took it.
    pub fn pointer(&mut self, id: u64, phase: PointerPhase, x: i32, y: i32) -> bool {
        let Some(layout) = &self.layout else {
            return false;
        };
        let known = self.fingers.iter().position(|f| f.id == id);
        match (phase, known) {
            (PointerPhase::Down, _) => {
                let target = if layout.dpad_hit(x, y) {
                    Target::Dpad
                } else if let Some(b) = layout.buttons.iter().find(|b| b.hit(x, y)) {
                    if b.control == Control::Menu {
                        Target::Menu
                    } else {
                        Target::Buttons
                    }
                } else {
                    return false;
                };
                if let Some(i) = known {
                    self.fingers.remove(i);
                }
                self.fingers.push(Finger { id, target, x, y });
                true
            }
            (PointerPhase::Move, Some(i)) => {
                self.fingers[i].x = x;
                self.fingers[i].y = y;
                true
            }
            (PointerPhase::Up | PointerPhase::Cancel, Some(i)) => {
                let finger = self.fingers.remove(i);
                if finger.target == Target::Menu && phase == PointerPhase::Up {
                    let on_menu = layout
                        .buttons
                        .iter()
                        .any(|b| b.control == Control::Menu && b.hit(x, y));
                    self.menu_tapped |= on_menu;
                }
                true
            }
            _ => false,
        }
    }

    /// The console buttons held by all fingers together.
    #[must_use]
    pub fn buttons(&self) -> Buttons {
        let Some(layout) = &self.layout else {
            return Buttons::default();
        };
        let mut held = Buttons::default();
        for f in &self.fingers {
            match f.target {
                Target::Dpad => {
                    let (cx, cy) = layout.dpad_center;
                    held = held | dpad_directions(f.x - cx, f.y - cy, layout.dpad_radius);
                }
                Target::Buttons => {
                    for b in &layout.buttons {
                        if let Control::Button(button) = b.control {
                            if b.hit(f.x, f.y) {
                                held = held | button.bits();
                            }
                        }
                    }
                }
                Target::Menu => {}
            }
        }
        held
    }

    /// Was the menu button tapped since the last call?
    pub fn take_menu_tap(&mut self) -> bool {
        std::mem::take(&mut self.menu_tapped)
    }

    /// Forget all fingers (e.g. when a menu opens).
    pub fn release_all(&mut self) {
        self.fingers.clear();
    }

    /// Draw the controls, highlighting what is pressed.
    pub fn draw(&self, canvas: &mut Canvas) {
        let Some(layout) = &self.layout else {
            return;
        };
        let held = self.buttons();
        let base = with_alpha(0x000000, 0x58);
        let rim = with_alpha(0xFFFFFF, 0x50);
        let ink = with_alpha(0xFFFFFF, 0xB0);
        let lit = with_alpha(theme::ACCENT, 0xC0);

        // D-pad: a disc with a cross, the pressed arms lit.
        let (cx, cy) = layout.dpad_center;
        let r = layout.dpad_radius;
        canvas.disc(cx, cy, r, base);
        canvas.ring(cx, cy, r, r - 1, rim);
        let arm = r * 30 / 100;
        let reach = r * 80 / 100;
        let cross = with_alpha(0xFFFFFF, 0x30);
        canvas.rounded(
            Rect::new(cx - reach, cy - arm, 2 * reach, 2 * arm),
            3,
            cross,
        );
        canvas.rounded(
            Rect::new(cx - arm, cy - reach, 2 * arm, reach - arm),
            3,
            cross,
        );
        canvas.rounded(
            Rect::new(cx - arm, cy + arm, 2 * arm, reach - arm),
            3,
            cross,
        );
        let arms = [
            (
                Buttons::UP,
                Rect::new(cx - arm, cy - reach, 2 * arm, reach - arm),
            ),
            (
                Buttons::DOWN,
                Rect::new(cx - arm, cy + arm, 2 * arm, reach - arm),
            ),
            (
                Buttons::LEFT,
                Rect::new(cx - reach, cy - arm, reach - arm, 2 * arm),
            ),
            (
                Buttons::RIGHT,
                Rect::new(cx + arm, cy - arm, reach - arm, 2 * arm),
            ),
        ];
        for (dir, rect) in arms {
            if held.contains(dir) {
                canvas.rounded(rect, 3, lit);
            }
        }
        // Small arrow heads on the arms.
        for (label, dx, dy) in [("▲", 0, -1), ("▼", 0, 1), ("◀", -1, 0), ("▶", 1, 0)] {
            let d = (reach + arm) / 2;
            text_scaled(canvas, cx + dx * d - 4, cy + dy * d - 4, label, ink, 1);
        }

        for b in &layout.buttons {
            let pressed = match b.control {
                Control::Button(button) => held.contains(button.bits()),
                Control::Menu => self.fingers.iter().any(|f| f.target == Target::Menu),
            };
            let (x, y) = b.center;
            match b.shape {
                Shape::Circle { radius } => {
                    canvas.disc(x, y, radius, if pressed { lit } else { base });
                    canvas.ring(x, y, radius, radius - 1, rim);
                }
                Shape::Pill { w, h } => {
                    let rect = Rect::new(x - w / 2, y - h / 2, w, h);
                    canvas.rounded(rect, h / 2, if pressed { lit } else { base });
                }
            }
            if b.control == Control::Menu {
                // Three bars: the universal "menu" sign.
                for i in -1..=1 {
                    canvas.blend(Rect::new(x - 5, y + i * 4 - 1, 10, 2), ink);
                }
            } else {
                text_scaled(
                    canvas,
                    x - text_width(b.label, 1) / 2,
                    y - 4,
                    b.label,
                    ink,
                    1,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use PointerPhase::{Down, Move, Up};

    #[test]
    fn eight_way_dpad() {
        let r = 40;
        assert_eq!(dpad_directions(0, 0, r), Buttons::default(), "dead zone");
        assert_eq!(dpad_directions(30, 0, r), Buttons::RIGHT);
        assert_eq!(dpad_directions(-30, 5, r), Buttons::LEFT);
        assert_eq!(dpad_directions(0, -30, r), Buttons::UP);
        assert_eq!(dpad_directions(20, 20, r), Buttons::RIGHT | Buttons::DOWN);
        assert_eq!(dpad_directions(-20, -20, r), Buttons::LEFT | Buttons::UP);
        // The sector boundary is at 22.5°: 30° is diagonal, 15° straight.
        assert_eq!(dpad_directions(26, 15, r), Buttons::RIGHT | Buttons::DOWN);
        assert_eq!(dpad_directions(29, 8, r), Buttons::RIGHT);
        // Far outside the circle still steers.
        assert_eq!(dpad_directions(0, 500, r), Buttons::DOWN);
    }

    fn controls(w: i32, h: i32, six: bool) -> TouchControls {
        let game = if w > h {
            Rect::new((w - h * 10 / 7) / 2, 0, h * 10 / 7, h)
        } else {
            Rect::new(0, 0, w, w * 7 / 10)
        };
        let mut t = TouchControls::default();
        t.set_layout(Layout::new(w, h, game, six));
        t
    }

    fn center_of(t: &TouchControls, control: Control) -> (i32, i32) {
        t.layout()
            .unwrap()
            .buttons
            .iter()
            .find(|b| b.control == control)
            .unwrap()
            .center
    }

    #[test]
    fn multi_touch_dpad_and_button() {
        for (w, h) in [(480, 216), (216, 480)] {
            let mut t = controls(w, h, true);
            let (cx, cy) = t.layout().unwrap().dpad_center;
            let r = t.layout().unwrap().dpad_radius;
            assert!(t.pointer(1, Down, cx + r / 2, cy - r / 2));
            let (bx, by) = center_of(&t, Control::Button(ConsoleButton::B));
            assert!(t.pointer(2, Down, bx, by));
            assert_eq!(
                t.buttons(),
                Buttons::RIGHT | Buttons::UP | Buttons::B,
                "{w}x{h}"
            );
            // The d-pad finger slides far left, off the pad: still steering.
            t.pointer(1, Move, cx - 3 * r, cy);
            assert_eq!(t.buttons(), Buttons::LEFT | Buttons::B);
            t.pointer(2, Up, bx, by);
            assert_eq!(t.buttons(), Buttons::LEFT);
            t.pointer(1, Up, 0, 0);
            assert_eq!(t.buttons(), Buttons::default());
        }
    }

    #[test]
    fn sliding_between_buttons() {
        let mut t = controls(480, 216, false);
        let b = center_of(&t, Control::Button(ConsoleButton::B));
        let c = center_of(&t, Control::Button(ConsoleButton::C));
        t.pointer(7, Down, b.0, b.1);
        assert_eq!(t.buttons(), Buttons::B);
        // Halfway between B and C: both.
        t.pointer(7, Move, (b.0 + c.0) / 2, (b.1 + c.1) / 2);
        assert_eq!(t.buttons(), Buttons::B | Buttons::C);
        t.pointer(7, Move, c.0, c.1);
        assert_eq!(t.buttons(), Buttons::C);
        // Slid off everything: nothing pressed, finger still tracked.
        t.pointer(7, Move, 240, 20);
        assert_eq!(t.buttons(), Buttons::default());
        assert!(t.pointer(7, Up, 240, 20));
    }

    #[test]
    fn touches_elsewhere_are_not_taken() {
        let mut t = controls(480, 216, false);
        assert!(!t.pointer(1, Down, 240, 100), "the middle of the game");
        assert!(!t.pointer(1, Up, 240, 100));
        // Six-button buttons only exist in six-button layouts.
        assert!(t.layout().unwrap().buttons.iter().all(|b| b.label != "X"));
    }

    #[test]
    fn menu_button_needs_a_full_tap() {
        let mut t = controls(480, 216, true);
        let (mx, my) = center_of(&t, Control::Menu);
        t.pointer(3, Down, mx, my);
        assert!(!t.take_menu_tap());
        t.pointer(3, Up, mx, my);
        assert!(t.take_menu_tap());
        assert!(!t.take_menu_tap(), "taken once");
        // Pressed then slid away: no tap.
        t.pointer(3, Down, mx, my);
        t.pointer(3, Up, 100, 100);
        assert!(!t.take_menu_tap());
    }

    #[test]
    fn landscape_start_and_mode_stay_off_the_picture() {
        // A 19.5:9 phone held sideways. The face buttons may reach a little
        // into the picture when the border is narrow, but Start and Mode
        // are small and must never cover the game (they once sat at the
        // bottom centre, over the score of many games).
        for six in [false, true] {
            let (w, h) = (844, 390);
            let game = Rect::new((w - h * 10 / 7) / 2, 0, h * 10 / 7, h);
            let l = Layout::new(w, h, game, six);
            for b in &l.buttons {
                let Shape::Pill { w: pill_w, .. } = b.shape else {
                    continue;
                };
                let (x, _) = b.center;
                assert!(
                    x + pill_w / 2 <= game.x || x - pill_w / 2 >= game.right(),
                    "{b:?} covers the picture (six buttons: {six})"
                );
            }
        }
    }

    #[test]
    fn layouts_fit_on_screen_without_overlaps() {
        for (w, h, six) in [
            (480, 216, true),
            (216, 480, true),
            (400, 260, false),
            (270, 600, true),
        ] {
            let t = controls(w, h, six);
            let l = t.layout().unwrap();
            let (cx, cy) = l.dpad_center;
            assert!(
                cx - l.dpad_radius >= 0 && cy + l.dpad_radius <= h,
                "{w}x{h}"
            );
            for (i, a) in l.buttons.iter().enumerate() {
                let (x, y) = a.center;
                assert!((0..w).contains(&x) && (0..h).contains(&y), "{w}x{h} {a:?}");
                for b in &l.buttons[i + 1..] {
                    assert_ne!(a.center, b.center);
                }
                assert!(!l.dpad_hit(x, y), "{a:?} overlaps the d-pad in {w}x{h}");
            }
        }
    }
}
