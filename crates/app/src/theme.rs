//! Colours and sizes shared by every screen.
//!
//! The palette borrows from the Mega Drive itself: a black, slightly cool
//! body, the red of the "16-BIT" lettering, and the gold of the Japanese
//! logo. Few colours, used consistently, make a UI look deliberate:
//!
//! * red marks *where you are* (the focused item, the title rule);
//! * gold marks *values* (a setting's current choice, a binding);
//! * grey is for everything secondary (hints, captions, empty slots).
//!
//! Sizes are in UI pixels (one font pixel; see [`crate::layout`]).

use crate::canvas::{opaque, with_alpha};

/// Screen background behind menus without a game.
pub const BACKGROUND: u32 = opaque(0x0C0D11);
/// The same, translucent, laid over a paused game.
pub const SCRIM: u32 = with_alpha(0x0C0D11, 0xB0);
/// Panels and unfocused rows.
pub const PANEL: u32 = opaque(0x17191F);
/// The focused row.
pub const FOCUS: u32 = opaque(0x262931);
/// Pressed (pointer held on it).
pub const PRESSED: u32 = opaque(0x33363F);
/// Thin separators and the console's grooves.
pub const LINE: u32 = opaque(0x2A2D35);
pub const TEXT: u32 = opaque(0xE8E8EC);
pub const DIM: u32 = opaque(0x8A8F99);
pub const FAINT: u32 = opaque(0x4A4E58);
/// Mega Drive red.
pub const ACCENT: u32 = opaque(0xE3262E);
/// Logo gold.
pub const GOLD: u32 = opaque(0xF0B840);
/// Error messages.
pub const ERROR: u32 = opaque(0xFF6A5C);

/// Height of a menu row.
pub const ROW: i32 = 16;
/// Space between rows.
pub const GAP: i32 = 2;
/// Margin around screens.
pub const MARGIN: i32 = 12;
/// Widest a menu column gets.
pub const COLUMN: i32 = 300;
/// Height of the font.
pub const GLYPH: i32 = 8;
