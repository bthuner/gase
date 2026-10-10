//! The gase user interface, for every platform.
//!
//! `gase-core` emulates a console: ROM bytes and button states in, pixels
//! and samples out. Everything *around* that — the home screen, the file
//! browser, menus, settings, save states, mapping a keyboard or a gamepad
//! or a finger to console buttons, deciding how fast to run — is the job of
//! this crate. It is written once and shared by every platform gase runs
//! on: the SDL2 desktop frontend today, and web, Android and iOS shells.
//!
//! Like the core it has **no dependencies, no unsafe code and no I/O**.
//! What only a platform can do (files, sound, windows) goes through the
//! [`Platform`] trait; the [`platform`] module documents that contract,
//! which is what makes the multi-platform story work. Start there.
//!
//! # A tour
//!
//! | Module | What it teaches |
//! |---|---|
//! | [`platform`] | The contract with the platform: events in, audio pushed out, video pulled, storage and slow things as requests |
//! | [`app`] | The state machine: home, playing, a stack of menus; one [`App::update`] per frame |
//! | [`ui`] | An immediate-mode UI toolkit with spatial navigation, for keyboard, gamepad, mouse and touch |
//! | [`screens`](crate::App::show_screen) | Every menu as one function from state to pixels and actions |
//! | [`input`] | Platform-independent keys and pad buttons, bindings, hotkeys |
//! | [`touch`] | On-screen controls: layout, 8-way d-pad, multi-touch |
//! | [`settings`] | The settings and their forgiving `key = value` text format |
//! | [`browser`] | Listing, filtering and sorting folders for the built-in file browser |
//! | [`layout`] | UI scale and the game picture's rectangle |
//! | [`canvas`], [`font`], [`png`] | Drawing into a pixel buffer with an 8×8 bitmap font, and saving it |
//!
//! # Why draw the UI in software?
//!
//! The menus are drawn pixel by pixel into a [`Canvas`] with a bitmap font,
//! instead of using a GUI library or the GPU:
//!
//! * **It is the same everywhere.** A GUI library would be another large
//!   dependency per platform (and there is no good one that covers desktop,
//!   web, Android and iOS without its own runtime). A pixel buffer is the
//!   one thing every platform can show: SDL streams it into a texture, a
//!   web page `putImageData`s it.
//! * **It is testable.** A test can render the pause menu into memory and
//!   check its pixels, with no window and no GPU.
//! * **It is cheap.** The overlay is a few hundred thousand pixels at most
//!   (it is drawn small and enlarged by the GPU, see [`layout`]), only
//!   while a menu is open; while playing it is usually not drawn at all.
//! * **It suits the subject.** An 8×8 font and whole-pixel scaling look
//!   like the console the emulator is about.
//!
//! # From input to action
//!
//! ```text
//!  Event::Key / PadButton / PadAxis / Pointer
//!        │
//!        ├─ menu open ──► Nav (Up, Down, Confirm, Back …) or pointer ──► ui ──► Action
//!        │
//!        └─ playing ───► Bindings ──► Buttons per player ──► Genesis::set_buttons
//!                    └─► hotkeys (save state, fast-forward …)
//!                    └─► touch controls ──► Buttons for player 1
//! ```
//!
//! See [`input`] for the vocabulary and the defaults, [`touch`] for the
//! on-screen controls.
//!
//! # Example: a headless "shell"
//!
//! ```
//! use gase_app::{App, Capabilities, FileKey, Platform};
//!
//! /// A platform with no files, no sound and a fixed clock.
//! struct Nothing;
//! impl Platform for Nothing {
//!     fn now_ms(&self) -> u64 { 0 }
//!     fn load(&mut self, _: FileKey<'_>) -> Option<Vec<u8>> { None }
//!     fn store(&mut self, _: FileKey<'_>, _: &[u8]) -> Result<String, String> { Ok(String::new()) }
//!     fn read_rom(&mut self, path: &str) -> Result<Vec<u8>, String> { Err(format!("no {path}")) }
//! }
//!
//! let mut platform = Nothing;
//! let mut app = App::new(&mut platform, Capabilities::default());
//! app.update(&mut platform);                // draws the home screen
//! let picture = app.compose();              // what a window would show
//! assert_eq!(app.screen_name(), "home");
//! assert!(picture.width > 0);
//! ```

pub mod app;
pub mod browser;
pub mod canvas;
pub mod font;
pub mod game;
pub mod input;
pub mod layout;
pub mod platform;
pub mod png;
mod screens;
pub mod settings;
pub mod theme;
pub mod touch;
pub mod ui;

pub use app::{AUDIO_TARGET_MS, App, Pacing, Video};
pub use canvas::{Canvas, Image, Rect};
pub use game::{DebugRun, Game};
pub use input::{ConsoleButton, Event, Key, Nav, PadAxis, PadButton, PointerKind, PointerPhase};
pub use platform::{Capabilities, DirEntry, FileKey, Listing, Platform, Request};
pub use settings::{Aspect, Settings, TouchMode};
