//! The web shell: gase in a browser tab, as WebAssembly.
//!
//! The whole emulator and its user interface ([`gase_app::App`]) are
//! compiled for `wasm32-unknown-unknown`; a page written in plain
//! JavaScript (`web/` at the top of the repository) loads the module,
//! feeds it input and shows what it draws. No wasm-bindgen, no web-sys, no
//! bundler: the boundary is a few dozen numbers-only functions, so every
//! byte that crosses it is visible and explained.
//!
//! # How a Rust program runs in a web page
//!
//! ```text
//!  ┌──────────────────────── the page (web/*.js) ─────────────────────────┐
//!  │ KeyboardEvent.code   Gamepad API   PointerEvent   <input type=file>   │
//!  │        │ (polled each frame)  │           │              │ IndexedDB  │
//!  │        ▼                      ▼           ▼              ▼            │
//!  │   exports: gase_key, gase_pad_button, gase_pointer, gase_rom …        │
//!  └────────┬──────────────────────────────────────────────────────▲──────┘
//!           │ calls with numbers (+ bytes in the inbox)             │ imports: env.*
//!  ┌────────▼───────────── WebAssembly instance ───────────────────┴──────┐
//!  │  Shell ── App ── Genesis              file_read, file_write, log,     │
//!  │  linear memory: one big ArrayBuffer    audio_push, request, now_ms    │
//!  │  [ … inbox | game RGBA | overlay RGBA | info[16] | samples … ]        │
//!  └──────────────────────────────────────────────────────────────────────┘
//!           ▲ JS reads pictures and samples *in place* through typed-array views
//! ```
//!
//! * **Memory.** A WebAssembly instance has one *linear memory*, a
//!   resizable `ArrayBuffer` that is the Rust program's whole address
//!   space. A pointer is just an offset into it, so Rust can hand the page
//!   a pointer as a plain number and the page reads the bytes with
//!   `new Uint8Array(memory.buffer, ptr, len)`: no copy, no serialisation.
//!   Taking a pointer's address is safe Rust ([`Vec::as_ptr`], then
//!   `expose_provenance()`, which also tells the compiler that the memory
//!   behind it may be touched from outside); only *dereferencing* a
//!   pointer is unsafe, and Rust never does that here: the page does it
//!   on its side. Two rules keep the page
//!   correct: a pointer is valid only until the next call into the module
//!   (a `Vec` may move when it grows), and when the memory grows its old
//!   `ArrayBuffer` is detached, so views must be made afresh from
//!   `memory.buffer` each time.
//! * **Calls.** Exports and imports may only take and return numbers
//!   (`i32`, `i64`, `f32`, `f64`). Strings and byte arrays travel as
//!   pointer + length: the page writes them into the [`Shell`]'s *inbox*
//!   (a `Vec<u8>` it asks the module to size), Rust writes outgoing ones
//!   anywhere and passes their address. `src/abi.rs` lists every function.
//! * **State.** An export is a free function, so the shell lives in a
//!   `thread_local!` (a WebAssembly instance without threads has exactly
//!   one thread) and each export borrows it for the duration of the call.
//!   The page's imports never call back into the module, so the borrow can
//!   never be taken twice.
//!
//! # One frame
//!
//! The page's `requestAnimationFrame` callback is the main loop
//! ([`gase_app::platform`] describes its shape):
//!
//! 1. poll the gamepads (the Gamepad API has no events for buttons) and
//!    send what changed; keys and pointers were already sent by their
//!    event listeners;
//! 2. decide how many emulated frames this display frame needs (the
//!    app's [`Pacing`]: by the audio queue's level, by the clock, or as
//!    many as fit for fast-forward) and call [`Shell::update`] that often;
//!    each update pushes its sound to the audio worklet through the
//!    `audio_push` import;
//! 3. call [`Shell::video`] and `putImageData` the game picture and, if it
//!    changed, the overlay into their canvases; the browser's compositor
//!    scales them (CSS `image-rendering: pixelated`).
//!
//! # What is tested where
//!
//! Everything in this crate but `abi.rs` is ordinary Rust that does not
//! know it runs in a browser: [`Shell`] is generic over a [`Host`] (the
//! JavaScript side), so `cargo test` drives it natively with a fake host.
//! The page itself is tested in a headless browser (`web/tests/`).

pub mod input;
pub mod storage;
pub mod video;

#[cfg(target_arch = "wasm32")]
mod abi;

use gase_app::{App, Capabilities, Event, FileKey, Pacing, Platform, Request};

use crate::video::{INFO_LEN, info};

/// What the JavaScript side provides: the module's imports, as a trait so
/// that tests can stand in for the browser.
pub trait Host {
    /// Milliseconds since the page loaded (`performance.now()`).
    fn now_ms(&self) -> f64;
    /// Print to the browser console.
    fn log(&mut self, message: &str);
    /// Read a stored file (from the page's in-memory copy of IndexedDB).
    fn read(&mut self, key: &str) -> Option<Vec<u8>>;
    /// Store a file (written to IndexedDB behind the scenes) or, for
    /// screenshots, hand it to the user as a download.
    fn write(&mut self, key: &str, data: &[u8]) -> Written;
    /// Stereo frames waiting in the audio worklet's queue; `None` while
    /// the browser has not allowed sound yet (no click or key press so
    /// far), which makes the app pace by the clock instead.
    fn audio_queued(&self) -> Option<usize>;
    /// Send interleaved stereo samples to the audio worklet.
    fn audio_push(&mut self, samples: &[i16]);
    /// Carry out a request (see [`encode_request`]).
    fn request(&mut self, request: &Request);
}

/// What became of a [`Host::write`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Written {
    Stored,
    Downloaded,
    Failed,
}

/// The app's [`Platform`], implemented with a [`Host`].
#[derive(Debug)]
pub struct WebPlatform<H> {
    pub host: H,
}

impl<H: Host> Platform for WebPlatform<H> {
    fn now_ms(&self) -> u64 {
        self.host.now_ms() as u64
    }

    fn log(&mut self, message: &str) {
        self.host.log(message);
    }

    fn load(&mut self, file: FileKey<'_>) -> Option<Vec<u8>> {
        self.host.read(&storage::key(file))
    }

    fn store(&mut self, file: FileKey<'_>, data: &[u8]) -> Result<String, String> {
        let key = storage::key(file);
        match self.host.write(&key, data) {
            Written::Stored => Ok("in browser storage".to_string()),
            Written::Downloaded => {
                let name = key.strip_prefix(storage::DOWNLOAD_PREFIX).unwrap_or(&key);
                Ok(format!("{name} (downloaded)"))
            }
            Written::Failed => Err("the browser refused to store it (storage full?)".to_string()),
        }
    }

    fn read_rom(&mut self, path: &str) -> Result<Vec<u8>, String> {
        self.host
            .read(&storage::rom_key(path))
            .ok_or_else(|| format!("{path} is no longer stored in this browser: open it again"))
    }

    fn audio_queued(&self) -> Option<usize> {
        self.host.audio_queued()
    }

    fn queue_audio(&mut self, samples: &[i16]) {
        self.host.audio_push(samples);
    }

    fn request(&mut self, request: Request) {
        self.host.request(&request);
    }
}

/// Request codes for the page's `request(kind, arg, ptr, len)` import.
pub const REQUEST_PICK_ROM: u32 = 0;
/// `arg`: 1 to enter fullscreen, 0 to leave.
pub const REQUEST_FULLSCREEN: u32 = 1;
/// The text is the new title.
pub const REQUEST_TITLE: u32 = 2;

/// Encode a request as (kind, numeric argument, text) for the page, or
/// `None` for requests a browser cannot honour (quit, the debugger).
#[must_use]
pub fn encode_request(request: &Request) -> Option<(u32, u32, &str)> {
    Some(match request {
        Request::PickRom => (REQUEST_PICK_ROM, 0, ""),
        Request::SetFullscreen(on) => (REQUEST_FULLSCREEN, u32::from(*on), ""),
        Request::SetTitle(title) => (REQUEST_TITLE, 0, title.as_str()),
        Request::Quit | Request::ToggleDebugger | Request::Breakpoint(_) => return None,
    })
}

/// Capability bits the page passes to [`Shell::new`].
pub const CAP_TOUCH: u32 = 1;
pub const CAP_KEYBOARD: u32 = 2;
pub const CAP_FULLSCREEN: u32 = 4;
pub const CAP_DROP_FILES: u32 = 8;

/// What a browser can do, from the page's capability bits.
#[must_use]
pub fn capabilities(sample_rate: u32, bits: u32) -> Capabilities {
    Capabilities {
        sample_rate,
        touch_screen: bits & CAP_TOUCH != 0,
        // No folders to browse; the system's file picker instead.
        file_browser: false,
        rom_picker: true,
        drop_files: bits & CAP_DROP_FILES != 0,
        // A tab is closed by the browser, not by the page.
        can_quit: false,
        fullscreen: bits & CAP_FULLSCREEN != 0,
        debugger: false,
        keyboard: bits & CAP_KEYBOARD != 0,
    }
}

/// The app with its platform and the buffers shared with the page. Each
/// method is one export of `abi.rs`.
#[derive(Debug)]
pub struct Shell<H: Host> {
    pub app: App,
    pub platform: WebPlatform<H>,
    /// Bytes from the page: key codes, gamepad names, ROMs.
    inbox: Vec<u8>,
    /// The pictures as canvas RGBA (see [`video`]).
    game_rgba: Vec<u32>,
    overlay_rgba: Vec<u32>,
    /// The frame description (see [`video::info`]).
    info: [u32; INFO_LEN],
}

impl<H: Host> Shell<H> {
    /// Start the app (reading the settings through the host).
    pub fn new(host: H, sample_rate: u32, capability_bits: u32) -> Self {
        let mut platform = WebPlatform { host };
        let mut app = App::new(&mut platform, capabilities(sample_rate, capability_bits));
        // A page cannot start in fullscreen: browsers only allow it in
        // response to a click or key press.
        app.settings_mut().video.fullscreen = false;
        let mut shell = Self {
            app,
            platform,
            inbox: Vec::new(),
            game_rgba: Vec::new(),
            overlay_rgba: Vec::new(),
            info: [0; INFO_LEN],
        };
        shell.set_pacing(Pacing::Timer { fps: 60.0 });
        shell
    }

    fn handle(&mut self, event: Event) {
        self.app.handle(&mut self.platform, event);
    }

    /// Make the inbox `len` bytes long and return its address, for the
    /// page to write into.
    pub fn inbox(&mut self, len: usize) -> u32 {
        self.inbox.clear();
        self.inbox.resize(len, 0);
        self.inbox.as_mut_ptr().expose_provenance() as u32
    }

    /// The first `len` bytes of the inbox as text.
    fn inbox_text(&self, len: usize) -> String {
        String::from_utf8_lossy(&self.inbox[..len.min(self.inbox.len())]).into_owned()
    }

    /// A key, by its `KeyboardEvent.code` (in the inbox). Returns whether
    /// the app knows the key, so the page can stop the browser's default
    /// action (scrolling on Space, going back on Backspace…).
    pub fn key(&mut self, code_len: usize, pressed: bool, repeat: bool) -> bool {
        let Some(key) = input::key_for_code(&self.inbox_text(code_len)) else {
            return false;
        };
        self.handle(Event::Key {
            key,
            pressed,
            repeat,
        });
        true
    }

    /// A gamepad appeared; its `Gamepad.id` is in the inbox.
    pub fn pad_connected(&mut self, pad: u32, id_len: usize) {
        let name = input::pad_name(&self.inbox_text(id_len));
        self.handle(Event::PadConnected { pad, name });
    }

    pub fn pad_disconnected(&mut self, pad: u32) {
        self.handle(Event::PadDisconnected { pad });
    }

    /// A button of the standard mapping changed.
    pub fn pad_button(&mut self, pad: u32, index: u32, pressed: bool) {
        if let Some(button) = input::standard_button(index) {
            self.handle(Event::PadButton {
                pad,
                button,
                pressed,
            });
        }
    }

    /// An axis (0-3 sticks, 4-5 triggers) changed.
    pub fn pad_axis(&mut self, pad: u32, index: u32, value: f32) {
        if let Some(axis) = input::standard_axis(index) {
            self.handle(Event::PadAxis { pad, axis, value });
        }
    }

    /// A pointer, in physical pixels of the page's drawing area.
    pub fn pointer(&mut self, id: u32, kind: u32, phase: u32, x: f32, y: f32) {
        if let Some(phase) = input::pointer_phase(phase) {
            self.handle(Event::Pointer {
                id: u64::from(id),
                kind: input::pointer_kind(kind),
                phase,
                x,
                y,
            });
        }
    }

    pub fn wheel(&mut self, delta_y: f64, mode: u32) {
        let lines = input::wheel_lines(delta_y, mode);
        self.handle(Event::Wheel { lines });
    }

    /// The drawing area is `width` × `height` physical pixels, with
    /// `devicePixelRatio` `density`.
    pub fn resize(&mut self, width: u32, height: u32, density: f32) {
        self.handle(Event::Resized {
            width,
            height,
            pixels_per_point: density,
        });
    }

    pub fn focus_lost(&mut self) {
        self.handle(Event::FocusLost);
    }

    /// The page was hidden (tab switched, phone locked, app switched).
    pub fn suspend(&mut self) {
        self.handle(Event::Suspend);
    }

    /// The browser left (or entered) fullscreen by itself, e.g. on Esc.
    pub fn fullscreen_changed(&mut self, on: bool) {
        self.app.settings_mut().video.fullscreen = on;
    }

    /// A ROM: its name is the first `name_len` bytes of the inbox, its
    /// contents the rest. (The page has already stored it, so that
    /// [`Platform::read_rom`] finds it again from the recent list.)
    pub fn rom(&mut self, name_len: usize) {
        let inbox = std::mem::take(&mut self.inbox);
        let split = name_len.min(inbox.len());
        let name = String::from_utf8_lossy(&inbox[..split]).into_owned();
        let data = inbox[split..].to_vec();
        drop(inbox);
        self.handle(Event::RomData { name, data });
    }

    /// Run one frame: menus or emulation. Returns the pacing kind
    /// ([`video::PACE_TIMER`] …); the details are in the frame description.
    pub fn update(&mut self) -> u32 {
        let pacing = self.app.update(&mut self.platform);
        self.set_pacing(pacing)
    }

    fn set_pacing(&mut self, pacing: Pacing) -> u32 {
        let (kind, value) = video::encode_pacing(pacing);
        self.info[info::PACE] = kind;
        self.info[info::PACE_VALUE] = value;
        kind
    }

    /// Prepare the pictures for drawing and return the address of the
    /// frame description.
    pub fn video(&mut self) -> u32 {
        let v = self.app.video();
        let mut flags = 0;
        let i = &mut self.info;
        if let Some(game) = &v.game {
            flags |= video::HAS_GAME;
            video::to_rgba(game, &mut self.game_rgba, true);
            i[info::GAME_PTR] = self.game_rgba.as_ptr().expose_provenance() as u32;
            i[info::GAME_W] = game.width as u32;
            i[info::GAME_H] = game.height as u32;
        }
        let r = v.game_rect;
        i[info::RECT_X] = r.x as u32;
        i[info::RECT_Y] = r.y as u32;
        i[info::RECT_W] = r.w as u32;
        i[info::RECT_H] = r.h as u32;
        if let Some(overlay) = v.overlay {
            flags |= video::HAS_OVERLAY;
            let size = (overlay.width as u32, overlay.height as u32);
            let stale = self.overlay_rgba.len() != overlay.pixels.len();
            // Converted only when the app redrew it: while playing with a
            // keyboard or pad that is almost never.
            if v.overlay_changed || stale {
                flags |= video::OVERLAY_CHANGED;
                video::to_rgba(&overlay.image(), &mut self.overlay_rgba, false);
            }
            i[info::OVERLAY_PTR] = self.overlay_rgba.as_ptr().expose_provenance() as u32;
            (i[info::OVERLAY_W], i[info::OVERLAY_H]) = size;
            i[info::OVERLAY_SCALE] = v.overlay_scale as u32;
        }
        i[info::BACKGROUND] = v.background & 0xFF_FFFF;
        i[info::FLAGS] = flags;
        i[info::FRAME] = self
            .app
            .game()
            .map_or(0, |g| g.genesis.frame_count() as u32);
        self.info_ptr()
    }

    /// The address of the frame description.
    pub fn info_ptr(&self) -> u32 {
        self.info.as_ptr().expose_provenance() as u32
    }

    /// The frame description as last written (for tests).
    #[must_use]
    pub fn info(&self) -> &[u32; INFO_LEN] {
        &self.info
    }

    /// The game picture as last converted (for tests).
    #[must_use]
    pub fn game_rgba(&self) -> &[u32] {
        &self.game_rgba
    }

    /// Emulate `frames` frames as fast as possible, without sound or
    /// pacing, for measuring speed in the browser. Returns the frames run.
    pub fn bench(&mut self, frames: u32) -> u32 {
        let Some(game) = self.app.game_mut() else {
            return 0;
        };
        let mut sink = Vec::new();
        for _ in 0..frames {
            game.run_frames(1);
            sink.clear();
            game.genesis.drain_audio(&mut sink);
        }
        frames
    }

    /// Write everything not yet saved (the page is going away).
    pub fn shutdown(&mut self) {
        self.app.shutdown(&mut self.platform);
    }
}

#[cfg(test)]
mod tests;
