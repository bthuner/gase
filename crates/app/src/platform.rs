//! The contract between the app and the platform it runs on.
//!
//! The app never touches a file, a window or a sound card itself. It
//! *asks*, through the [`Platform`] trait, and a **shell** — a few hundred
//! lines specific to one platform — answers:
//!
//! ```text
//!   ┌──────────────── shell (SDL2 desktop, web, Android, iOS) ─────────────┐
//!   │  window + GPU texture   sound device   files / storage   input APIs │
//!   └─────┬───────────────────────▲──────────────▲─────────────────┬──────┘
//!         │ Event (keys, pads,    │ queue_audio  │ load / store    │ Video:
//!         │ touches, resize, ROM) │              │ read_rom        │ game frame +
//!         ▼                       │              │ list_dir        │ overlay
//!   ┌─────────────────────────── gase-app: App ────────────────────▼──────┐
//!   │  menus · settings · input mapping · touch controls · saves · pacing │
//!   └────────────────────────────────┬────────────────────────────────────┘
//!                                    │ run_frame, set_buttons, save_state …
//!                              gase-core: Genesis
//! ```
//!
//! The direction of each arrow is deliberate:
//!
//! * **Events flow in** ([`App::handle`](crate::App::handle)): the shell
//!   translates whatever its system delivers into [`Event`](crate::Event)s.
//! * **Audio is pushed out** ([`Platform::queue_audio`]): sound is a
//!   continuous stream, and the app produces it as it emulates.
//! * **Video is pulled** ([`App::video`](crate::App::video)): a picture is
//!   a snapshot, so the shell takes the latest one whenever it is ready to
//!   draw.
//! * **Storage is a request** ([`Platform::load`], [`Platform::store`]):
//!   the app says *what* it keeps ([`FileKey`]), the platform decides
//!   *where*: next to the ROM on desktop, in browser storage on the web, in
//!   the app's sandbox on a phone.
//! * **Everything slow or optional is a request with a later answer.**
//!   Opening the system's file picker ([`Request::PickRom`]) cannot block:
//!   on the web it is an `<input type=file>` that answers some time later,
//!   on Android an `Intent`. The shell answers by sending
//!   [`Event::RomData`](crate::Event::RomData) whenever the user is done.
//!   A request the platform cannot fulfil is simply ignored, and the
//!   [`Capabilities`] tell the app which ones are worth offering.
//!
//! # Writing a shell
//!
//! A shell's main loop is the same everywhere:
//!
//! ```text
//! let mut app = App::new(&mut platform, capabilities);
//! loop {
//!     for each system event: app.handle(&mut platform, event);
//!     let pacing = app.update(&mut platform);    // menus + emulation + audio
//!     let video = app.video();                   // draw game + overlay
//!     upload video.game into a texture, draw it at video.game_rect;
//!     if let Some(overlay) = video.overlay: upload (if changed) and draw it,
//!         alpha-blended, scaled by overlay.scale with nearest filtering;
//!     wait according to `pacing` (see Pacing);
//!     if app.wants_quit() { app.shutdown(&mut platform); break }
//! }
//! ```
//!
//! On the web the loop body becomes a `requestAnimationFrame` callback;
//! on Android and iOS an SDL2 shell can keep exactly this shape.

/// Something the app stores. The platform maps it to a file name, a
/// browser storage key, …
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKey<'a> {
    /// The settings text ([`Settings::to_text`](crate::Settings::to_text)).
    Settings,
    /// The game's own save memory (battery SRAM or EEPROM) for the ROM
    /// `rom` (the same string the ROM was opened with). Desktop: `game.srm`
    /// next to the ROM.
    Save { rom: &'a str },
    /// Save state `slot` (0-9). Desktop: `game.state3`.
    State { rom: &'a str, slot: u8 },
    /// A small PNG picture of save state `slot`. Desktop: `game.state3.png`.
    StatePreview { rom: &'a str, slot: u8 },
    /// A PNG screenshot taken at `frame`. Desktop: `game-1234.png`.
    Screenshot { rom: &'a str, frame: u64 },
}

/// One entry of a folder listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    /// What to show (the file name).
    pub name: String,
    /// What to pass back to [`Platform::list_dir`] (folders) or
    /// [`Platform::read_rom`] (files).
    pub path: String,
    pub is_dir: bool,
}

/// The contents of a folder.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Listing {
    /// The folder itself, as shown in the browser's title.
    pub path: String,
    /// The folder above, if any.
    pub parent: Option<String>,
    /// In any order; the app sorts and filters them.
    pub entries: Vec<DirEntry>,
}

/// Something only the platform can do. Platforms ignore requests they
/// cannot honour.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Show the system's own file picker; answer later with
    /// [`Event::RomData`](crate::Event::RomData) (or
    /// [`Event::OpenRom`](crate::Event::OpenRom)), or not at all if the
    /// user cancels.
    PickRom,
    /// Enter or leave fullscreen.
    SetFullscreen(bool),
    /// Show this as the window title.
    SetTitle(String),
    /// The user chose "Quit" (only offered when [`Capabilities::can_quit`]).
    Quit,
    /// Show or hide the debugger (only when [`Capabilities::debugger`]).
    ToggleDebugger,
    /// The game stopped at a breakpoint at this 68000 address: show the
    /// debugger.
    Breakpoint(u32),
}

/// What a platform can do, so the app only offers what works.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capabilities {
    /// The audio output rate in Hz (the emulator resamples to it).
    pub sample_rate: u32,
    /// There is a touch screen: show on-screen controls (unless the
    /// settings say otherwise).
    pub touch_screen: bool,
    /// [`Platform::list_dir`] works: offer the built-in file browser.
    pub file_browser: bool,
    /// [`Request::PickRom`] works: offer the system's file picker.
    pub rom_picker: bool,
    /// Files can be dropped onto the window (shown as a hint).
    pub drop_files: bool,
    /// The app may offer "Quit" (not on the web or iOS).
    pub can_quit: bool,
    /// [`Request::SetFullscreen`] works.
    pub fullscreen: bool,
    /// [`Request::ToggleDebugger`] works.
    pub debugger: bool,
    /// A keyboard is likely present (show keyboard hints).
    pub keyboard: bool,
}

impl Default for Capabilities {
    /// A minimal platform: 48 kHz audio, keyboard, nothing else.
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            touch_screen: false,
            file_browser: false,
            rom_picker: false,
            drop_files: false,
            can_quit: false,
            fullscreen: false,
            debugger: false,
            keyboard: true,
        }
    }
}

/// Everything the app needs from the platform it runs on.
///
/// Only [`Platform::now_ms`], [`Platform::load`], [`Platform::store`] and
/// [`Platform::read_rom`] are required; the rest have defaults meaning
/// "not available".
pub trait Platform {
    /// Milliseconds since some fixed moment (monotonic). Used for toasts,
    /// key repeat on gamepads and periodic save-file writes; emulation
    /// timing itself comes from [`Pacing`](crate::Pacing).
    fn now_ms(&self) -> u64;

    /// A diagnostic message (stderr, the browser console, logcat …).
    fn log(&mut self, message: &str) {
        let _ = message;
    }

    /// Read a stored file; `None` if it does not exist or cannot be read.
    fn load(&mut self, file: FileKey<'_>) -> Option<Vec<u8>>;

    /// Store a file. On success, return a short description of where it
    /// went (shown to the user, e.g. a path).
    ///
    /// # Errors
    ///
    /// A message for the user when storing failed.
    fn store(&mut self, file: FileKey<'_>, data: &[u8]) -> Result<String, String>;

    /// Read a ROM by the path or name it was opened with: from a
    /// [`DirEntry::path`], [`Event::OpenRom`](crate::Event::OpenRom), the
    /// recent list, or the name of [`Event::RomData`](crate::Event::RomData)
    /// (a web or mobile platform may keep a copy of picked ROMs for this).
    ///
    /// # Errors
    ///
    /// A message for the user if the ROM cannot be read.
    fn read_rom(&mut self, path: &str) -> Result<Vec<u8>, String>;

    /// List a folder for the built-in file browser; `None` means "where to
    /// start" (the current or home folder).
    ///
    /// # Errors
    ///
    /// A message for the user (or "not supported").
    fn list_dir(&mut self, dir: Option<&str>) -> Result<Listing, String> {
        let _ = dir;
        Err("this platform has no file browser".into())
    }

    /// Stereo sample frames waiting in the sound device's queue, or `None`
    /// without sound. The app uses it to fine-tune the audio speed.
    fn audio_queued(&self) -> Option<usize> {
        None
    }

    /// Queue interleaved stereo 16-bit samples for playback.
    fn queue_audio(&mut self, samples: &[i16]) {
        let _ = samples;
    }

    /// Ask for something only the platform can do (see [`Request`]).
    fn request(&mut self, request: Request) {
        let _ = request;
    }
}
