//! Translation tables: what the browser reports → the app's vocabulary.
//!
//! Browsers already describe input in a platform-independent way, so these
//! tables are short and close to one-to-one:
//!
//! * **Keys.** `KeyboardEvent.code` names the *physical* key (`"KeyZ"` is
//!   the key left of X on any layout), exactly like the app's [`Key`]. Do
//!   not confuse it with `KeyboardEvent.key`, which is the *character*
//!   typed (`"z"`, `"y"` on a German keyboard, `"ω"` with a Greek layout).
//! * **Gamepads.** The Gamepad API reports buttons and axes as arrays. When
//!   the browser recognises a controller it sets `gamepad.mapping` to
//!   `"standard"` and orders the arrays the way the W3C "standard gamepad"
//!   figure shows: the same Xbox-style layout as the app's [`PadButton`].
//!   Triggers are buttons there (6 and 7) but with an analogue `value`;
//!   the app wants them as axes, so the page sends them as axis 4 and 5.
//! * **Wheel.** `WheelEvent.deltaY` comes in pixels, lines or pages
//!   depending on `deltaMode`, with the opposite sign to the app's
//!   [`Event::Wheel`](gase_app::Event::Wheel).

use gase_app::{Key, PadAxis, PadButton};

/// `KeyboardEvent.code` → [`Key`], for every key the app knows.
///
/// A plain table: easy to read, easy to check (see the tests), and a
/// linear search over a hundred short strings per key press costs nothing.
pub const KEY_CODES: &[(&str, Key)] = &[
    ("KeyA", Key::A),
    ("KeyB", Key::B),
    ("KeyC", Key::C),
    ("KeyD", Key::D),
    ("KeyE", Key::E),
    ("KeyF", Key::F),
    ("KeyG", Key::G),
    ("KeyH", Key::H),
    ("KeyI", Key::I),
    ("KeyJ", Key::J),
    ("KeyK", Key::K),
    ("KeyL", Key::L),
    ("KeyM", Key::M),
    ("KeyN", Key::N),
    ("KeyO", Key::O),
    ("KeyP", Key::P),
    ("KeyQ", Key::Q),
    ("KeyR", Key::R),
    ("KeyS", Key::S),
    ("KeyT", Key::T),
    ("KeyU", Key::U),
    ("KeyV", Key::V),
    ("KeyW", Key::W),
    ("KeyX", Key::X),
    ("KeyY", Key::Y),
    ("KeyZ", Key::Z),
    ("Digit0", Key::Num0),
    ("Digit1", Key::Num1),
    ("Digit2", Key::Num2),
    ("Digit3", Key::Num3),
    ("Digit4", Key::Num4),
    ("Digit5", Key::Num5),
    ("Digit6", Key::Num6),
    ("Digit7", Key::Num7),
    ("Digit8", Key::Num8),
    ("Digit9", Key::Num9),
    ("F1", Key::F1),
    ("F2", Key::F2),
    ("F3", Key::F3),
    ("F4", Key::F4),
    ("F5", Key::F5),
    ("F6", Key::F6),
    ("F7", Key::F7),
    ("F8", Key::F8),
    ("F9", Key::F9),
    ("F10", Key::F10),
    ("F11", Key::F11),
    ("F12", Key::F12),
    ("ArrowUp", Key::Up),
    ("ArrowDown", Key::Down),
    ("ArrowLeft", Key::Left),
    ("ArrowRight", Key::Right),
    ("Enter", Key::Enter),
    ("Escape", Key::Escape),
    ("Backspace", Key::Backspace),
    ("Tab", Key::Tab),
    ("Space", Key::Space),
    ("Insert", Key::Insert),
    ("Delete", Key::Delete),
    ("Home", Key::Home),
    ("End", Key::End),
    ("PageUp", Key::PageUp),
    ("PageDown", Key::PageDown),
    ("ShiftLeft", Key::LeftShift),
    ("ShiftRight", Key::RightShift),
    ("ControlLeft", Key::LeftCtrl),
    ("ControlRight", Key::RightCtrl),
    ("AltLeft", Key::LeftAlt),
    ("AltRight", Key::RightAlt),
    ("Minus", Key::Minus),
    ("Equal", Key::Equals),
    ("BracketLeft", Key::LeftBracket),
    ("BracketRight", Key::RightBracket),
    ("Backslash", Key::Backslash),
    ("Semicolon", Key::Semicolon),
    ("Quote", Key::Apostrophe),
    ("Backquote", Key::Grave),
    ("Comma", Key::Comma),
    ("Period", Key::Period),
    ("Slash", Key::Slash),
    ("Numpad0", Key::Kp0),
    ("Numpad1", Key::Kp1),
    ("Numpad2", Key::Kp2),
    ("Numpad3", Key::Kp3),
    ("Numpad4", Key::Kp4),
    ("Numpad5", Key::Kp5),
    ("Numpad6", Key::Kp6),
    ("Numpad7", Key::Kp7),
    ("Numpad8", Key::Kp8),
    ("Numpad9", Key::Kp9),
    ("NumpadEnter", Key::KpEnter),
    ("NumpadAdd", Key::KpPlus),
    ("NumpadSubtract", Key::KpMinus),
    ("NumpadMultiply", Key::KpMultiply),
    ("NumpadDivide", Key::KpDivide),
    ("NumpadDecimal", Key::KpPeriod),
];

/// The app's key for a `KeyboardEvent.code`, if it has one.
#[must_use]
pub fn key_for_code(code: &str) -> Option<Key> {
    KEY_CODES.iter().find(|(c, _)| *c == code).map(|&(_, k)| k)
}

/// The buttons of the W3C "standard" gamepad mapping, by index.
///
/// Indices 6 and 7 (the triggers) are missing on purpose: they are sent
/// as axes ([`standard_axis`]).
pub const STANDARD_BUTTONS: [Option<PadButton>; 17] = [
    Some(PadButton::South),         // 0: bottom face button (A, ×)
    Some(PadButton::East),          // 1: right (B, ○)
    Some(PadButton::West),          // 2: left (X, □)
    Some(PadButton::North),         // 3: top (Y, △)
    Some(PadButton::LeftShoulder),  // 4
    Some(PadButton::RightShoulder), // 5
    None,                           // 6: left trigger → axis 4
    None,                           // 7: right trigger → axis 5
    Some(PadButton::Back),          // 8: Back / Select / Share
    Some(PadButton::Start),         // 9: Start / Options
    Some(PadButton::LeftStick),     // 10
    Some(PadButton::RightStick),    // 11
    Some(PadButton::DPadUp),        // 12
    Some(PadButton::DPadDown),      // 13
    Some(PadButton::DPadLeft),      // 14
    Some(PadButton::DPadRight),     // 15
    Some(PadButton::Guide),         // 16: Xbox / PS / Home button
];

/// A standard-mapping button index → [`PadButton`]. Index 17 and beyond
/// (a touchpad click, a capture button…) become [`PadButton::Misc`].
#[must_use]
pub fn standard_button(index: u32) -> Option<PadButton> {
    match STANDARD_BUTTONS.get(index as usize) {
        Some(&button) => button,
        None => Some(PadButton::Misc),
    }
}

/// The axis numbering the page uses: the standard mapping's four stick
/// axes, then the two triggers (the analogue `value` of buttons 6 and 7).
#[must_use]
pub fn standard_axis(index: u32) -> Option<PadAxis> {
    Some(match index {
        0 => PadAxis::LeftX,
        1 => PadAxis::LeftY,
        2 => PadAxis::RightX,
        3 => PadAxis::RightY,
        4 => PadAxis::LeftTrigger,
        5 => PadAxis::RightTrigger,
        _ => return None,
    })
}

/// A friendlier name for a controller than `Gamepad.id`.
///
/// Chrome reports `"Xbox 360 Controller (XInput STANDARD GAMEPAD)"` or
/// `"Wireless Controller (STANDARD GAMEPAD Vendor: 054c Product: 09cc)"`,
/// Firefox `"054c-09cc-Wireless Controller"`. Keep the human part.
#[must_use]
pub fn pad_name(id: &str) -> String {
    let mut name = id;
    if let Some(paren) = name.find(" (") {
        name = &name[..paren];
    }
    // Firefox: "vendor-product-Name", both four hex digits.
    let bytes = name.as_bytes();
    if bytes.len() > 10
        && bytes[4] == b'-'
        && bytes[9] == b'-'
        && name[..4].chars().all(|c| c.is_ascii_hexdigit())
        && name[5..9].chars().all(|c| c.is_ascii_hexdigit())
    {
        name = &name[10..];
    }
    let name = name.trim();
    if name.is_empty() {
        "Gamepad".to_string()
    } else {
        name.chars().take(40).collect()
    }
}

/// `WheelEvent.deltaMode` values.
pub const DELTA_PIXEL: u32 = 0;
pub const DELTA_LINE: u32 = 1;
pub const DELTA_PAGE: u32 = 2;

/// Convert a `WheelEvent.deltaY` to the app's lines (positive: towards the
/// top). A "line" in pixel mode is taken as 40 CSS pixels, about what one
/// notch of a mouse wheel scrolls in browsers.
#[must_use]
pub fn wheel_lines(delta_y: f64, mode: u32) -> f32 {
    let lines = match mode {
        DELTA_LINE => delta_y,
        DELTA_PAGE => delta_y * 10.0,
        _ => delta_y / 40.0,
    };
    -lines as f32
}

/// `PointerEvent.pointerType` as the page sends it: 0 mouse, 1 touch,
/// 2 pen. A pen behaves like a finger (it should press the on-screen
/// controls), so only the mouse is a mouse.
#[must_use]
pub fn pointer_kind(kind: u32) -> gase_app::PointerKind {
    if kind == 0 {
        gase_app::PointerKind::Mouse
    } else {
        gase_app::PointerKind::Touch
    }
}

/// The page's pointer phases: 0 down, 1 move, 2 up, 3 cancel.
#[must_use]
pub fn pointer_phase(phase: u32) -> Option<gase_app::PointerPhase> {
    use gase_app::PointerPhase as P;
    Some(match phase {
        0 => P::Down,
        1 => P::Move,
        2 => P::Up,
        3 => P::Cancel,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_has_exactly_one_code() {
        for &key in Key::ALL {
            let codes: Vec<_> = KEY_CODES.iter().filter(|(_, k)| *k == key).collect();
            assert_eq!(codes.len(), 1, "{key:?}");
        }
        assert_eq!(KEY_CODES.len(), Key::ALL.len());
        for (i, (a, _)) in KEY_CODES.iter().enumerate() {
            assert!(KEY_CODES[i + 1..].iter().all(|(b, _)| a != b), "{a}");
        }
    }

    #[test]
    fn codes_are_physical_positions() {
        assert_eq!(key_for_code("KeyZ"), Some(Key::Z));
        assert_eq!(key_for_code("Digit7"), Some(Key::Num7));
        assert_eq!(key_for_code("Numpad7"), Some(Key::Kp7));
        assert_eq!(key_for_code("Equal"), Some(Key::Equals));
        assert_eq!(key_for_code("Quote"), Some(Key::Apostrophe));
        assert_eq!(key_for_code("Backquote"), Some(Key::Grave));
        // Character names (KeyboardEvent.key) are not codes.
        assert_eq!(key_for_code("z"), None);
        assert_eq!(key_for_code("MetaLeft"), None);
        assert_eq!(key_for_code(""), None);
    }

    #[test]
    fn standard_gamepad_layout() {
        assert_eq!(standard_button(0), Some(PadButton::South));
        assert_eq!(standard_button(3), Some(PadButton::North));
        assert_eq!(standard_button(6), None);
        assert_eq!(standard_button(7), None);
        assert_eq!(standard_button(9), Some(PadButton::Start));
        assert_eq!(standard_button(12), Some(PadButton::DPadUp));
        assert_eq!(standard_button(16), Some(PadButton::Guide));
        assert_eq!(standard_button(17), Some(PadButton::Misc));
        // Every pad button but Misc has exactly one index.
        for &b in PadButton::ALL {
            let n = STANDARD_BUTTONS.iter().filter(|x| **x == Some(b)).count();
            assert_eq!(n, usize::from(b != PadButton::Misc), "{b:?}");
        }
        assert_eq!(standard_axis(1), Some(PadAxis::LeftY));
        assert_eq!(standard_axis(4), Some(PadAxis::LeftTrigger));
        assert_eq!(standard_axis(5), Some(PadAxis::RightTrigger));
        assert_eq!(standard_axis(6), None);
    }

    #[test]
    fn pad_names() {
        assert_eq!(
            pad_name("Xbox 360 Controller (XInput STANDARD GAMEPAD)"),
            "Xbox 360 Controller"
        );
        assert_eq!(
            pad_name("Wireless Controller (STANDARD GAMEPAD Vendor: 054c Product: 09cc)"),
            "Wireless Controller"
        );
        assert_eq!(
            pad_name("054c-09cc-Wireless Controller"),
            "Wireless Controller"
        );
        assert_eq!(pad_name("   "), "Gamepad");
        assert_eq!(pad_name(&"x".repeat(100)).len(), 40);
    }

    #[test]
    fn wheel_direction_and_units() {
        // Scrolling down in the browser (positive deltaY) moves the content
        // up: negative lines for the app.
        assert_eq!(wheel_lines(120.0, DELTA_PIXEL), -3.0);
        assert_eq!(wheel_lines(-3.0, DELTA_LINE), 3.0);
        assert_eq!(wheel_lines(1.0, DELTA_PAGE), -10.0);
    }

    #[test]
    fn pointers() {
        use gase_app::{PointerKind, PointerPhase};
        assert_eq!(pointer_kind(0), PointerKind::Mouse);
        assert_eq!(pointer_kind(1), PointerKind::Touch);
        assert_eq!(pointer_kind(2), PointerKind::Touch);
        assert_eq!(pointer_phase(0), Some(PointerPhase::Down));
        assert_eq!(pointer_phase(3), Some(PointerPhase::Cancel));
        assert_eq!(pointer_phase(4), None);
    }
}
