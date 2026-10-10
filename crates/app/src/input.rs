//! Input: the events platforms send, and how they become console buttons
//! or menu navigation.
//!
//! # From a key press to a console button
//!
//! Every platform names keys and buttons differently: SDL has scancodes,
//! browsers have `KeyboardEvent.code` strings, Android has `KEYCODE_*`
//! numbers. A platform shell translates its own names into the small,
//! platform-independent vocabulary of this module ([`Key`], [`PadButton`],
//! [`PadAxis`], pointer events) and sends an [`Event`] to the app.
//! From there everything is shared code:
//!
//! ```text
//!  platform event ──► Event::Key { key: Key::Z, pressed: true }
//!                         │
//!          menu shown? ───┤ yes ─► Nav::… (fixed keys: arrows, Enter, Esc)
//!                         │ no
//!                         ├─► Bindings: Z is player 1's "A" ─► Buttons::A
//!                         └─► otherwise a hotkey (F5 save state, Tab …)
//! ```
//!
//! Keys are **physical positions** (like SDL scancodes and the web's
//! `KeyboardEvent.code`), not the letters printed on them: the default
//! Z/X/C layout stays a row of three neighbouring keys on an AZERTY or
//! QWERTZ keyboard.
//!
//! Gamepad buttons use the **standard layout** popularised by the Xbox
//! controller and adopted by SDL's GameController API, the web Gamepad API
//! ("standard" mapping) and Android: four face buttons named by position
//! ([`PadButton::South`] is the bottom one, labelled A on Xbox and × on
//! PlayStation pads), a d-pad, two shoulders, two triggers, Start, Back
//! and Guide. A platform that maps its controllers to this layout gets
//! every pad working with the same bindings.
//!
//! The Mega Drive pad has three face buttons in a row, A B C, with X Y Z
//! above them on the six-button model. On a modern pad the default puts
//! A B C on the bottom row as your thumb finds it (West, South, East) and
//! X Y Z on the shoulders and North, like most Mega Drive collections.

use gase_core::Buttons;

macro_rules! named_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident { $($variant:ident = $text:literal),* $(,)? }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum $name { $($variant),* }

        impl $name {
            /// Every value, in declaration order.
            pub const ALL: &'static [$name] = &[$($name::$variant),*];

            /// The name used in the settings file (and shown in menus).
            #[must_use]
            pub fn name(self) -> &'static str {
                match self { $($name::$variant => $text),* }
            }

            /// Parse a name written by [`Self::name`] (case-insensitive).
            #[must_use]
            pub fn from_name(text: &str) -> Option<Self> {
                Self::ALL.iter().copied().find(|v| v.name().eq_ignore_ascii_case(text))
            }
        }
    };
}

named_enum! {
    /// A keyboard key, by physical position (US layout names).
    pub enum Key {
        A = "A", B = "B", C = "C", D = "D", E = "E", F = "F", G = "G", H = "H",
        I = "I", J = "J", K = "K", L = "L", M = "M", N = "N", O = "O", P = "P",
        Q = "Q", R = "R", S = "S", T = "T", U = "U", V = "V", W = "W", X = "X",
        Y = "Y", Z = "Z",
        Num0 = "0", Num1 = "1", Num2 = "2", Num3 = "3", Num4 = "4",
        Num5 = "5", Num6 = "6", Num7 = "7", Num8 = "8", Num9 = "9",
        F1 = "F1", F2 = "F2", F3 = "F3", F4 = "F4", F5 = "F5", F6 = "F6",
        F7 = "F7", F8 = "F8", F9 = "F9", F10 = "F10", F11 = "F11", F12 = "F12",
        Up = "Up", Down = "Down", Left = "Left", Right = "Right",
        Enter = "Enter", Escape = "Escape", Backspace = "Backspace", Tab = "Tab",
        Space = "Space", Insert = "Insert", Delete = "Delete", Home = "Home",
        End = "End", PageUp = "PageUp", PageDown = "PageDown",
        LeftShift = "LeftShift", RightShift = "RightShift",
        LeftCtrl = "LeftCtrl", RightCtrl = "RightCtrl",
        LeftAlt = "LeftAlt", RightAlt = "RightAlt",
        Minus = "Minus", Equals = "Equals", LeftBracket = "LeftBracket",
        RightBracket = "RightBracket", Backslash = "Backslash",
        Semicolon = "Semicolon", Apostrophe = "Apostrophe", Grave = "Grave",
        Comma = "Comma", Period = "Period", Slash = "Slash",
        Kp0 = "Keypad0", Kp1 = "Keypad1", Kp2 = "Keypad2", Kp3 = "Keypad3",
        Kp4 = "Keypad4", Kp5 = "Keypad5", Kp6 = "Keypad6", Kp7 = "Keypad7",
        Kp8 = "Keypad8", Kp9 = "Keypad9", KpEnter = "KeypadEnter",
        KpPlus = "KeypadPlus", KpMinus = "KeypadMinus",
        KpMultiply = "KeypadMultiply", KpDivide = "KeypadDivide",
        KpPeriod = "KeypadPeriod",
    }
}

impl Key {
    /// The character a letter or digit key types (lower case), for
    /// "type to jump" in lists.
    #[must_use]
    pub fn char(self) -> Option<char> {
        let name = self.name();
        let c = name.chars().next()?;
        (name.len() == 1 && c.is_ascii_alphanumeric()).then(|| c.to_ascii_lowercase())
    }
}

named_enum! {
    /// A button of a gamepad with the standard layout (named by position).
    pub enum PadButton {
        South = "South", East = "East", West = "West", North = "North",
        Back = "Back", Guide = "Guide", Start = "Start",
        LeftStick = "LeftStick", RightStick = "RightStick",
        LeftShoulder = "LeftShoulder", RightShoulder = "RightShoulder",
        DPadUp = "DPadUp", DPadDown = "DPadDown",
        DPadLeft = "DPadLeft", DPadRight = "DPadRight",
        Misc = "Misc",
    }
}

impl PadButton {
    /// A short label as printed on an Xbox-style pad.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            PadButton::South => "A",
            PadButton::East => "B",
            PadButton::West => "X",
            PadButton::North => "Y",
            PadButton::Back => "Back",
            PadButton::Guide => "Guide",
            PadButton::Start => "Start",
            PadButton::LeftStick => "L3",
            PadButton::RightStick => "R3",
            PadButton::LeftShoulder => "LB",
            PadButton::RightShoulder => "RB",
            PadButton::DPadUp => "D-pad up",
            PadButton::DPadDown => "D-pad down",
            PadButton::DPadLeft => "D-pad left",
            PadButton::DPadRight => "D-pad right",
            PadButton::Misc => "Misc",
        }
    }
}

/// An analogue axis of a standard gamepad.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PadAxis {
    /// Left stick, −1 (left) to 1 (right).
    LeftX,
    /// Left stick, −1 (up) to 1 (down).
    LeftY,
    RightX,
    RightY,
    /// Triggers, 0 (released) to 1 (fully pressed).
    LeftTrigger,
    RightTrigger,
}

/// Mouse or finger: touches drive the on-screen controls, a mouse only the
/// menus (and gets hover highlighting).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerKind {
    Mouse,
    Touch,
}

/// What happened to a pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerPhase {
    /// A finger touched the screen, or the main mouse button went down.
    Down,
    /// It moved (mouse: with or without the button held).
    Move,
    /// The finger lifted or the button was released.
    Up,
    /// The system took the touch away (e.g. a gesture); treat as released
    /// without a click.
    Cancel,
}

/// One input event, in the platform-independent vocabulary of this module.
///
/// Positions are in **physical pixels** of the window or screen, the same
/// unit as [`Event::Resized`]; the app converts them.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A key went down (`repeat`: generated by the system while it is held)
    /// or up.
    Key {
        key: Key,
        pressed: bool,
        repeat: bool,
    },
    /// A gamepad appeared. `pad` is any number that identifies it until it
    /// is disconnected (SDL's instance id, the web Gamepad `index`, …).
    PadConnected {
        pad: u32,
        name: String,
    },
    PadDisconnected {
        pad: u32,
    },
    PadButton {
        pad: u32,
        button: PadButton,
        pressed: bool,
    },
    PadAxis {
        pad: u32,
        axis: PadAxis,
        value: f32,
    },
    /// A mouse or touch pointer. `id` tells simultaneous touches apart.
    Pointer {
        id: u64,
        kind: PointerKind,
        phase: PointerPhase,
        x: f32,
        y: f32,
    },
    /// Mouse wheel or touchpad scroll, in lines (positive: content moves
    /// down, i.e. scroll towards the top).
    Wheel {
        lines: f32,
    },
    /// The drawable area changed size. `pixels_per_point` is the platform's
    /// display density: 1.0 on a classic desktop, `devicePixelRatio` on the
    /// web, `density` on Android, `UIScreen.scale` on iOS.
    Resized {
        width: u32,
        height: u32,
        pixels_per_point: f32,
    },
    /// The window lost the keyboard focus: release everything held.
    FocusLost,
    /// The app is going to the background (mobile) or the window was
    /// minimised: pause and write the game's saves now.
    Suspend,
    /// Open a ROM the platform can read again later by this name with
    /// [`Platform::read_rom`](crate::Platform::read_rom): a path from the
    /// command line or a file dropped on the window.
    OpenRom {
        path: String,
    },
    /// A ROM chosen with the platform's own file picker
    /// ([`Request::PickRom`](crate::Request::PickRom)) or downloaded,
    /// given as bytes. `name` should be a file name (it names the saves).
    RomData {
        name: String,
        data: Vec<u8>,
    },
}

named_enum! {
    /// A button of the Mega Drive controller.
    pub enum ConsoleButton {
        Up = "up", Down = "down", Left = "left", Right = "right",
        A = "a", B = "b", C = "c", X = "x", Y = "y", Z = "z",
        Start = "start", Mode = "mode",
    }
}

impl ConsoleButton {
    /// The bit in [`gase_core::Buttons`].
    #[must_use]
    pub fn bits(self) -> Buttons {
        match self {
            ConsoleButton::Up => Buttons::UP,
            ConsoleButton::Down => Buttons::DOWN,
            ConsoleButton::Left => Buttons::LEFT,
            ConsoleButton::Right => Buttons::RIGHT,
            ConsoleButton::A => Buttons::A,
            ConsoleButton::B => Buttons::B,
            ConsoleButton::C => Buttons::C,
            ConsoleButton::X => Buttons::X,
            ConsoleButton::Y => Buttons::Y,
            ConsoleButton::Z => Buttons::Z,
            ConsoleButton::Start => Buttons::START,
            ConsoleButton::Mode => Buttons::MODE,
        }
    }

    /// The label for menus.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            ConsoleButton::Up => "Up",
            ConsoleButton::Down => "Down",
            ConsoleButton::Left => "Left",
            ConsoleButton::Right => "Right",
            ConsoleButton::A => "A",
            ConsoleButton::B => "B",
            ConsoleButton::C => "C",
            ConsoleButton::X => "X",
            ConsoleButton::Y => "Y",
            ConsoleButton::Z => "Z",
            ConsoleButton::Start => "Start",
            ConsoleButton::Mode => "Mode",
        }
    }

    /// Only six-button pads have X, Y, Z and Mode.
    #[must_use]
    pub fn six_button_only(self) -> bool {
        matches!(
            self,
            ConsoleButton::X | ConsoleButton::Y | ConsoleButton::Z | ConsoleButton::Mode
        )
    }

    /// Position in [`ConsoleButton::ALL`] (and in binding tables).
    #[must_use]
    pub fn index(self) -> usize {
        self as usize
    }
}

/// Number of console buttons (the size of a binding table).
pub const BUTTON_COUNT: usize = 12;

/// Which keys and pad buttons one player uses, indexed by
/// [`ConsoleButton::index`]. `None`: not bound.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerBindings {
    pub keys: [Option<Key>; BUTTON_COUNT],
    pub pad: [Option<PadButton>; BUTTON_COUNT],
}

impl PlayerBindings {
    /// Player 1: arrows, Z X C / A S D, Enter and Q on the keyboard.
    #[must_use]
    pub fn player1() -> Self {
        use Key as K;
        Self {
            // Up Down Left Right A B C X Y Z Start Mode
            keys: [
                Some(K::Up),
                Some(K::Down),
                Some(K::Left),
                Some(K::Right),
                Some(K::Z),
                Some(K::X),
                Some(K::C),
                Some(K::A),
                Some(K::S),
                Some(K::D),
                Some(K::Enter),
                Some(K::Q),
            ],
            pad: Self::default_pad(),
        }
    }

    /// Player 2: no keyboard keys (most people use a pad), same pad layout.
    #[must_use]
    pub fn player2() -> Self {
        Self {
            keys: [None; BUTTON_COUNT],
            pad: Self::default_pad(),
        }
    }

    /// The defaults for `player` (0 or 1).
    #[must_use]
    pub fn default_for(player: usize) -> Self {
        if player == 0 {
            Self::player1()
        } else {
            Self::player2()
        }
    }

    fn default_pad() -> [Option<PadButton>; BUTTON_COUNT] {
        use PadButton as P;
        [
            Some(P::DPadUp),
            Some(P::DPadDown),
            Some(P::DPadLeft),
            Some(P::DPadRight),
            Some(P::West),
            Some(P::South),
            Some(P::East),
            Some(P::LeftShoulder),
            Some(P::North),
            Some(P::RightShoulder),
            Some(P::Start),
            Some(P::Back),
        ]
    }

    /// The console button `key` is bound to.
    #[must_use]
    pub fn button_for_key(&self, key: Key) -> Option<ConsoleButton> {
        let i = self.keys.iter().position(|&k| k == Some(key))?;
        Some(ConsoleButton::ALL[i])
    }

    /// The console button `button` is bound to.
    #[must_use]
    pub fn button_for_pad(&self, button: PadButton) -> Option<ConsoleButton> {
        let i = self.pad.iter().position(|&b| b == Some(button))?;
        Some(ConsoleButton::ALL[i])
    }

    /// Bind `key` to `button`. A key drives one button only, so if it was
    /// bound elsewhere that binding moves here, and the button's previous
    /// key takes the vacated place (a swap), so nothing is lost silently.
    pub fn bind_key(&mut self, button: ConsoleButton, key: Option<Key>) {
        swap_bind(&mut self.keys, button.index(), key);
    }

    /// Bind a pad button, with the same swapping rule as [`Self::bind_key`].
    pub fn bind_pad(&mut self, button: ConsoleButton, pad: Option<PadButton>) {
        swap_bind(&mut self.pad, button.index(), pad);
    }
}

fn swap_bind<T: Copy + PartialEq>(table: &mut [Option<T>], index: usize, value: Option<T>) {
    if let Some(v) = value {
        if let Some(other) = table.iter().position(|&t| t == Some(v)) {
            table[other] = table[index];
        }
    }
    table[index] = value;
}

/// A menu navigation command. Menus only understand these, whatever the
/// device: arrow keys, the d-pad and the left stick all produce `Up` …
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nav {
    Up,
    Down,
    Left,
    Right,
    /// Activate the focused item (Enter, Space, pad South).
    Confirm,
    /// Go back (Escape, Backspace, pad East).
    Back,
    PageUp,
    PageDown,
    Home,
    End,
    /// A letter or digit was typed (jump in lists).
    Char(char),
}

/// Menu navigation from the keyboard. These keys are fixed (not
/// remappable), so the menus can always be operated.
#[must_use]
pub fn nav_for_key(key: Key) -> Option<Nav> {
    Some(match key {
        Key::Up | Key::Kp8 => Nav::Up,
        Key::Down | Key::Kp2 => Nav::Down,
        Key::Left | Key::Kp4 => Nav::Left,
        Key::Right | Key::Kp6 => Nav::Right,
        Key::Enter | Key::KpEnter | Key::Space => Nav::Confirm,
        Key::Escape | Key::Backspace => Nav::Back,
        Key::PageUp => Nav::PageUp,
        Key::PageDown => Nav::PageDown,
        Key::Home => Nav::Home,
        Key::End => Nav::End,
        other => Nav::Char(other.char()?),
    })
}

/// Menu navigation from a gamepad button.
#[must_use]
pub fn nav_for_pad(button: PadButton) -> Option<Nav> {
    Some(match button {
        PadButton::DPadUp => Nav::Up,
        PadButton::DPadDown => Nav::Down,
        PadButton::DPadLeft => Nav::Left,
        PadButton::DPadRight => Nav::Right,
        PadButton::South | PadButton::Start => Nav::Confirm,
        PadButton::East | PadButton::Back => Nav::Back,
        PadButton::LeftShoulder => Nav::PageUp,
        PadButton::RightShoulder => Nav::PageDown,
        _ => return None,
    })
}

/// Things the player can do while a game runs, besides pressing console
/// buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hotkey {
    /// Open the pause menu.
    Menu,
    /// Pause or continue without the menu (for watching a frame).
    Pause,
    /// While paused: emulate one frame.
    FrameStep,
    /// Held: run several frames per displayed frame.
    FastForward,
    /// Held: step back through recent history.
    Rewind,
    SaveState,
    LoadState,
    PreviousSlot,
    NextSlot,
    Reset,
    Fullscreen,
    Screenshot,
    Mute,
    /// Show or hide the debugger (where the platform has one).
    Debugger,
}

/// The keyboard hotkeys. A key bound to a console button takes precedence,
/// so remapping onto one of these keys disables its hotkey.
pub const HOTKEYS: &[(Key, Hotkey, &str)] = &[
    (Key::Escape, Hotkey::Menu, "Menu"),
    (Key::P, Hotkey::Pause, "Pause"),
    (Key::N, Hotkey::FrameStep, "Next frame (paused)"),
    (Key::Tab, Hotkey::FastForward, "Fast forward (hold)"),
    (Key::Backspace, Hotkey::Rewind, "Rewind (hold)"),
    (Key::F5, Hotkey::SaveState, "Save state"),
    (Key::F8, Hotkey::LoadState, "Load state"),
    (Key::F6, Hotkey::PreviousSlot, "Previous slot"),
    (Key::F7, Hotkey::NextSlot, "Next slot"),
    (Key::F9, Hotkey::Reset, "Reset"),
    (Key::F11, Hotkey::Fullscreen, "Fullscreen"),
    (Key::F12, Hotkey::Screenshot, "Screenshot"),
    (Key::M, Hotkey::Mute, "Mute"),
    (Key::F1, Hotkey::Debugger, "Debugger"),
    (Key::Grave, Hotkey::Debugger, "Debugger"),
];

/// The hotkey on `key`, if any.
#[must_use]
pub fn hotkey_for_key(key: Key) -> Option<Hotkey> {
    HOTKEYS.iter().find(|(k, ..)| *k == key).map(|&(_, h, _)| h)
}

/// How far a stick must be pushed to count as a d-pad direction. Sticks
/// rest slightly off-centre; a dead zone keeps that from moving anything.
pub const STICK_DEAD_ZONE: f32 = 0.4;
/// How far a trigger must be pressed to count.
pub const TRIGGER_THRESHOLD: f32 = 0.5;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for &k in Key::ALL {
            assert_eq!(Key::from_name(k.name()), Some(k));
        }
        for &b in PadButton::ALL {
            assert_eq!(PadButton::from_name(b.name()), Some(b));
        }
        assert_eq!(Key::from_name("escape"), Some(Key::Escape));
        assert_eq!(Key::from_name("nope"), None);
        assert_eq!(ConsoleButton::ALL.len(), BUTTON_COUNT);
        for (i, b) in ConsoleButton::ALL.iter().enumerate() {
            assert_eq!(b.index(), i);
        }
    }

    #[test]
    fn default_keyboard_mapping() {
        let p1 = PlayerBindings::player1();
        assert_eq!(p1.button_for_key(Key::Z), Some(ConsoleButton::A));
        assert_eq!(p1.button_for_key(Key::Enter), Some(ConsoleButton::Start));
        assert_eq!(p1.button_for_key(Key::Up), Some(ConsoleButton::Up));
        assert_eq!(p1.button_for_key(Key::F5), None);
        assert_eq!(PlayerBindings::player2().button_for_key(Key::Z), None);
        // No default binding shadows a hotkey.
        for (key, ..) in HOTKEYS {
            assert_eq!(p1.button_for_key(*key), None, "{key:?}");
        }
    }

    #[test]
    fn default_pad_mapping_follows_the_physical_layout() {
        let p = PlayerBindings::player1();
        assert_eq!(p.button_for_pad(PadButton::West), Some(ConsoleButton::A));
        assert_eq!(p.button_for_pad(PadButton::South), Some(ConsoleButton::B));
        assert_eq!(p.button_for_pad(PadButton::East), Some(ConsoleButton::C));
        assert_eq!(p.button_for_pad(PadButton::Guide), None);
        assert_eq!(ConsoleButton::C.bits(), Buttons::C);
    }

    #[test]
    fn rebinding_swaps_instead_of_duplicating() {
        let mut p = PlayerBindings::player1();
        // Put "X" (the key bound to B) on A: B gets A's old key, Z.
        p.bind_key(ConsoleButton::A, Some(Key::X));
        assert_eq!(p.button_for_key(Key::X), Some(ConsoleButton::A));
        assert_eq!(p.button_for_key(Key::Z), Some(ConsoleButton::B));
        // Clearing.
        p.bind_key(ConsoleButton::A, None);
        assert_eq!(p.button_for_key(Key::X), None);
        p.bind_pad(ConsoleButton::Start, Some(PadButton::South));
        assert_eq!(
            p.button_for_pad(PadButton::South),
            Some(ConsoleButton::Start)
        );
        assert_eq!(p.button_for_pad(PadButton::Start), Some(ConsoleButton::B));
    }

    #[test]
    fn navigation_keys() {
        assert_eq!(nav_for_key(Key::Enter), Some(Nav::Confirm));
        assert_eq!(nav_for_key(Key::Escape), Some(Nav::Back));
        assert_eq!(nav_for_key(Key::G), Some(Nav::Char('g')));
        assert_eq!(nav_for_key(Key::Num7), Some(Nav::Char('7')));
        assert_eq!(nav_for_key(Key::F3), None);
        assert_eq!(nav_for_pad(PadButton::South), Some(Nav::Confirm));
        assert_eq!(nav_for_pad(PadButton::East), Some(Nav::Back));
        assert_eq!(hotkey_for_key(Key::F5), Some(Hotkey::SaveState));
        assert_eq!(hotkey_for_key(Key::Z), None);
    }
}
