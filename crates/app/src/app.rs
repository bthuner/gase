//! The app: a state machine over screens and the running game.
//!
//! ```text
//!              open a ROM                        Esc, Guide, ≡ button
//!   ┌──────┐ ───────────────►  ┌──────────┐ ─────────────────────────► ┌────────────┐
//!   │ Home │                   │ Playing  │                            │ Pause menu │
//!   └──────┘ ◄─────────────── └──────────┘ ◄───────────────────────── └────────────┘
//!     │  ▲      Close game                     Resume, Esc, Load state     │  ▲
//!     ▼  │                                                                  ▼  │
//!   Browser, Settings …                                  Save/Load state, Settings …
//! ```
//!
//! The menus are a **stack** of screens: opening one pushes it, Back pops
//! it, so every screen returns to wherever it was opened from (Settings
//! from Home goes back to Home; from the pause menu, back to the pause
//! menu). With a game loaded and an empty stack, the game is playing.
//! Without a game the stack always holds at least the home screen.
//!
//! Each [`App::update`] does, in order:
//!
//! 1. menus, if any are open: one immediate-mode pass ([`crate::ui`]) that
//!    draws the top screen into the overlay and handles this frame's input;
//! 2. otherwise emulation: map the inputs to console buttons, run one frame
//!    (several when fast-forwarding, one step back when rewinding), push the
//!    audio to the platform, and tell the shell how to pace the next frame;
//! 3. the in-game overlay (touch controls, messages), redrawn only when
//!    something on it changed.

use gase_core::{Buttons, Stop};

use crate::canvas::{Canvas, Image, Rect, opaque, with_alpha};
use crate::font::{text_scaled, text_width};
use crate::game::{self, DebugRun, Game, SLOTS};
use crate::input::{
    ConsoleButton, Event, Hotkey, Key, Nav, PadAxis, PadButton, PointerKind, PointerPhase,
    STICK_DEAD_ZONE, TRIGGER_THRESHOLD, hotkey_for_key, nav_for_key, nav_for_pad,
};
use crate::layout::{game_rect, ui_scale};
use crate::platform::{Capabilities, FileKey, Platform, Request};
use crate::screens::{self, Action, Ctx, InputMode, REMAP_TIMEOUT_MS, Remap, Screen};
use crate::settings::{Settings, TouchMode};
use crate::theme;
use crate::touch::{Layout, TouchControls};
use crate::ui::{Focus, PointerInput, Ui};

/// How long a message stays on screen.
const TOAST_MS: u64 = 2_500;
/// Gamepad directions held in menus repeat after this delay …
const REPEAT_DELAY_MS: u64 = 400;
/// … at this interval.
const REPEAT_MS: u64 = 90;
/// How long the mouse's menu button stays after the mouse last moved.
const MOUSE_MENU_MS: u64 = 2_500;
/// How far a finger must move before a press becomes a scroll.
const DRAG_THRESHOLD: i32 = 6;
/// The audio queue level the shell should keep, in milliseconds.
pub const AUDIO_TARGET_MS: u32 = 50;

/// How the shell should wait before the next [`App::update`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pacing {
    /// The game runs at normal speed with sound: wait until the sound
    /// queue has drained to `target` stereo frames
    /// ([`Platform::audio_queued`]). The sound card's clock then paces the
    /// emulation, and the app nudges the audio speed to keep the queue
    /// level steady (dynamic rate control), so there are no crackles.
    Audio { target: usize },
    /// No sound to follow (menus, paused, rewinding, no audio device):
    /// show `fps` frames per second by the system clock, or simply wait
    /// for the display's vertical sync.
    Timer { fps: f64 },
    /// Fast-forward: draw and come back immediately.
    Unthrottled,
}

/// What the shell draws: the game picture and the overlay over it.
#[derive(Debug)]
pub struct Video<'a> {
    /// The console's picture, if a game is loaded.
    pub game: Option<Image<'a>>,
    /// Where to draw it, in screen pixels (nearest or linear filtering:
    /// the shell's choice).
    pub game_rect: Rect,
    /// Menus, on-screen controls and messages, with alpha; `None` when
    /// there is nothing to show (the common case while playing).
    pub overlay: Option<&'a Canvas>,
    /// Draw the overlay enlarged by this factor (nearest filtering), from
    /// the top-left corner.
    pub overlay_scale: i32,
    /// The overlay changed since the last call: upload it again.
    pub overlay_changed: bool,
    /// Colour for the parts of the screen nothing covers.
    pub background: u32,
}

/// A screen on the menu stack with its focus.
#[derive(Debug)]
struct Layer {
    screen: Screen,
    focus: Focus,
}

/// A connected gamepad.
#[derive(Debug)]
struct Pad {
    id: u32,
    name: String,
    player: usize,
    /// Console buttons held through bindings.
    buttons: Buttons,
    /// Directions from the left stick.
    stick: Buttons,
    stick_xy: (f32, f32),
    /// Pad buttons held (bit = `PadButton as u32`).
    held: u32,
    fast_forward: bool,
    rewind: bool,
}

/// A short message.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Toast {
    text: String,
    error: bool,
    until_ms: u64,
}

/// The primary pointer as the menus see it, collected between frames.
#[derive(Debug, Default)]
struct UiPointer {
    id: Option<u64>,
    frame: PointerInput,
    down_at: (i32, i32),
    last: (i32, i32),
    dragging: bool,
}

/// Everything drawn on the in-game overlay; redrawn only when it changes.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Hud {
    touch: Option<(u16, bool)>,
    mouse_menu: bool,
    toast: Option<Toast>,
    size: (usize, usize),
}

/// The whole user-facing application. See the [crate documentation](crate).
#[derive(Debug)]
pub struct App {
    caps: Capabilities,
    settings: Settings,
    settings_dirty: bool,
    game: Option<Game>,
    stack: Vec<Layer>,

    size: (u32, u32),
    pixels_per_point: f32,
    scale: i32,
    overlay: Canvas,
    overlay_shown: bool,
    overlay_changed: bool,
    hud: Option<Hud>,

    keyboard: [Buttons; 2],
    pads: Vec<Pad>,
    touch: TouchControls,
    /// A touch screen was used, even if the platform did not report one.
    touch_seen: bool,
    pointer: UiPointer,
    navs: Vec<Nav>,
    repeat: Option<(Nav, u64)>,
    mode: InputMode,
    fast_forward_key: bool,
    rewind_key: bool,
    mouse_menu_until: u64,

    audio: Vec<i16>,
    toast: Option<Toast>,
    title: String,
    quit: bool,
    now: u64,
}

impl App {
    /// Start the app: read the settings and show the home screen.
    pub fn new(platform: &mut dyn Platform, caps: Capabilities) -> Self {
        let settings = match platform.load(FileKey::Settings) {
            Some(bytes) => {
                let (settings, errors) = Settings::parse(&String::from_utf8_lossy(&bytes));
                for e in errors {
                    platform.log(&format!("settings: {e}"));
                }
                settings
            }
            None => Settings::default(),
        };
        let mut app = Self {
            caps,
            settings,
            settings_dirty: false,
            game: None,
            stack: vec![Layer {
                screen: Screen::Home,
                focus: Focus::default(),
            }],
            size: (0, 0),
            pixels_per_point: 1.0,
            scale: 1,
            overlay: Canvas::new(0, 0, 0),
            overlay_shown: false,
            overlay_changed: true,
            hud: None,
            keyboard: [Buttons::default(); 2],
            pads: Vec::new(),
            touch: TouchControls::default(),
            touch_seen: false,
            pointer: UiPointer::default(),
            navs: Vec::new(),
            repeat: None,
            mode: InputMode::Keyboard,
            fast_forward_key: false,
            rewind_key: false,
            mouse_menu_until: 0,
            audio: Vec::with_capacity(4096),
            toast: None,
            title: String::new(),
            quit: false,
            now: platform.now_ms(),
        };
        app.mode = if app.caps.keyboard {
            InputMode::Keyboard
        } else {
            InputMode::Pointer
        };
        let s = &app.settings.video;
        app.resize(320 * s.scale, 224 * s.scale, 1.0);
        app
    }

    // --- Accessors ----------------------------------------------------------------

    #[must_use]
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Change settings from outside the menus (e.g. command-line options).
    /// Such changes are only written out if the user also changes
    /// something in the menus.
    pub fn settings_mut(&mut self) -> &mut Settings {
        &mut self.settings
    }

    #[must_use]
    pub fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    /// The running game.
    #[must_use]
    pub fn game(&self) -> Option<&Game> {
        self.game.as_ref()
    }

    /// The running game, for a debugger.
    pub fn game_mut(&mut self) -> Option<&mut Game> {
        self.game.as_mut()
    }

    /// Is a menu shown?
    #[must_use]
    pub fn menu_open(&self) -> bool {
        !self.stack.is_empty()
    }

    /// The user asked to quit.
    #[must_use]
    pub fn wants_quit(&self) -> bool {
        self.quit
    }

    /// Show a message for a few seconds.
    pub fn notify(&mut self, text: impl Into<String>) {
        self.toast = Some(Toast {
            text: text.into(),
            error: false,
            until_ms: self.now + TOAST_MS,
        });
    }

    fn error(&mut self, platform: &mut dyn Platform, text: impl Into<String>) {
        let text = text.into();
        platform.log(&text);
        self.toast = Some(Toast {
            text,
            error: true,
            until_ms: self.now + 2 * TOAST_MS,
        });
    }

    // --- Games -----------------------------------------------------------------------

    /// Open a ROM by path (read through the platform).
    ///
    /// # Errors
    ///
    /// A message (also shown to the user) if it cannot be read or used.
    pub fn open_rom(&mut self, platform: &mut dyn Platform, path: &str) -> Result<(), String> {
        let data = platform
            .read_rom(path)
            .inspect_err(|e| self.error(platform, e.clone()))?;
        self.open_rom_data(platform, path, &data)
    }

    /// Open a ROM given as bytes, named `name` (for its saves and the
    /// recent list).
    ///
    /// # Errors
    ///
    /// A message (also shown to the user) if it is not a usable ROM.
    pub fn open_rom_data(
        &mut self,
        platform: &mut dyn Platform,
        name: &str,
        data: &[u8],
    ) -> Result<(), String> {
        let game = Game::open(name, data, &self.settings, self.caps.sample_rate, platform)
            .inspect_err(|e| self.error(platform, e.clone()))?;
        platform.log(&game.describe());
        // Only now that the new game works, put the old one away.
        self.close_game(platform);
        self.settings.add_recent(name);
        self.settings_dirty = true;
        self.save_settings(platform);
        self.game = Some(game);
        self.stack.clear();
        self.keyboard = [Buttons::default(); 2];
        self.relayout();
        Ok(())
    }

    /// Write the save and return to the home screen.
    pub fn close_game(&mut self, platform: &mut dyn Platform) {
        if let Some(mut game) = self.game.take() {
            game.flush_save(platform);
        }
        self.stack = vec![Layer {
            screen: Screen::Home,
            focus: Focus::default(),
        }];
        self.relayout();
    }

    /// Run the debugger (desktop debugger window). Returns how it stopped.
    pub fn debug_run(&mut self, what: DebugRun) -> Option<Stop> {
        self.game.as_mut().map(|g| g.debug_run(what))
    }

    /// Write everything that is not saved yet; call before exiting.
    pub fn shutdown(&mut self, platform: &mut dyn Platform) {
        if let Some(game) = &mut self.game {
            game.flush_save(platform);
        }
        self.save_settings(platform);
    }

    fn save_settings(&mut self, platform: &mut dyn Platform) {
        if !self.settings_dirty {
            return;
        }
        self.settings_dirty = false;
        if let Err(e) = platform.store(FileKey::Settings, self.settings.to_text().as_bytes()) {
            platform.log(&format!("Cannot save the settings: {e}"));
        }
    }

    // --- Layout --------------------------------------------------------------------

    fn resize(&mut self, width: u32, height: u32, pixels_per_point: f32) {
        self.size = (width.max(1), height.max(1));
        self.pixels_per_point = pixels_per_point;
        self.relayout();
    }

    /// Recompute everything that depends on the screen size or settings.
    fn relayout(&mut self) {
        let (w, h) = self.size;
        self.scale = ui_scale(w, h, self.pixels_per_point);
        let s = self.scale as u32;
        self.overlay
            .resize(w.div_ceil(s) as usize, h.div_ceil(s) as usize);
        let game = self.game_rect();
        let s = self.scale;
        let logical = Rect::new(game.x / s, game.y / s, game.w / s, game.h / s);
        let six = self.settings.players[0].device == gase_core::Device::SixButton;
        self.touch.set_layout(Layout::new(
            self.overlay.width as i32,
            self.overlay.height as i32,
            logical,
            six,
        ));
        self.hud = None;
        self.overlay_changed = true;
    }

    fn touch_visible(&self) -> bool {
        match self.settings.touch {
            TouchMode::On => true,
            TouchMode::Off => false,
            TouchMode::Auto => self.caps.touch_screen || self.touch_seen,
        }
    }

    /// Where the game picture goes, in screen pixels.
    fn game_rect(&self) -> Rect {
        let (w, h) = self.size;
        let area = Rect::new(0, 0, w as i32, h as i32);
        let frame = self.game.as_ref().map_or((320, 224), |g| {
            let f = g.genesis.frame();
            (f.width, f.height)
        });
        let v = &self.settings.video;
        let portrait_controls = self.touch_visible() && h > w;
        game_rect(area, frame, v.aspect, v.integer_scale, portrait_controls)
    }

    // --- Events ----------------------------------------------------------------------

    /// Feed one input event.
    pub fn handle(&mut self, platform: &mut dyn Platform, event: Event) {
        self.now = platform.now_ms();
        match event {
            Event::Resized {
                width,
                height,
                pixels_per_point,
            } => {
                if (width, height, pixels_per_point)
                    != (self.size.0, self.size.1, self.pixels_per_point)
                {
                    self.resize(width, height, pixels_per_point);
                }
            }
            Event::Key {
                key,
                pressed,
                repeat,
            } => self.key(platform, key, pressed, repeat),
            Event::PadConnected { pad, name } => {
                let taken = |p: usize| self.pads.iter().any(|x| x.player == p);
                let player = if !taken(0) {
                    0
                } else if !taken(1) {
                    1
                } else {
                    0
                };
                self.notify(format!("{name}: player {}", player + 1));
                self.pads.push(Pad {
                    id: pad,
                    name,
                    player,
                    buttons: Buttons::default(),
                    stick: Buttons::default(),
                    stick_xy: (0.0, 0.0),
                    held: 0,
                    fast_forward: false,
                    rewind: false,
                });
            }
            Event::PadDisconnected { pad } => {
                if let Some(i) = self.pads.iter().position(|p| p.id == pad) {
                    let p = self.pads.remove(i);
                    self.notify(format!("{} disconnected", p.name));
                }
            }
            Event::PadButton {
                pad,
                button,
                pressed,
            } => self.pad_button(platform, pad, button, pressed),
            Event::PadAxis { pad, axis, value } => self.pad_axis(pad, axis, value),
            Event::Pointer {
                id,
                kind,
                phase,
                x,
                y,
            } => {
                let (x, y) = (x as i32 / self.scale, y as i32 / self.scale);
                self.pointer_event(id, kind, phase, x, y);
            }
            Event::Wheel { lines } => {
                self.pointer.frame.scroll -= (lines * 3.0 * theme::ROW as f32) as i32;
            }
            Event::FocusLost => self.release_all(),
            Event::Suspend => {
                if let Some(game) = &mut self.game {
                    game.flush_save(platform);
                }
                self.save_settings(platform);
                self.open_menu();
            }
            Event::OpenRom { path } => {
                let _ = self.open_rom(platform, &path);
            }
            Event::RomData { name, data } => {
                let _ = self.open_rom_data(platform, &name, &data);
            }
        }
    }

    fn release_all(&mut self) {
        self.keyboard = [Buttons::default(); 2];
        for pad in &mut self.pads {
            pad.buttons = Buttons::default();
            pad.held = 0;
        }
        self.fast_forward_key = false;
        self.rewind_key = false;
        self.touch.release_all();
        self.repeat = None;
    }

    /// Open the pause menu over the game.
    pub fn open_menu(&mut self) {
        if self.game.is_some() && self.stack.is_empty() {
            self.release_all();
            self.stack.push(Layer {
                screen: Screen::Pause,
                focus: Focus::default(),
            });
        }
    }

    /// The remapping screen waiting for a key, if any.
    fn capturing(&mut self) -> Option<&mut Remap> {
        match self.stack.last_mut() {
            Some(Layer {
                screen: Screen::Remap(r),
                ..
            }) if r.waiting.is_some() => Some(r),
            _ => None,
        }
    }

    fn key(&mut self, platform: &mut dyn Platform, key: Key, pressed: bool, repeat: bool) {
        self.mode = InputMode::Keyboard;
        if let Some(remap) = self.capturing() {
            if !pressed || repeat {
                return;
            }
            let (player, pad, button) = (
                remap.player,
                remap.pad,
                remap.waiting.take().unwrap_or(ConsoleButton::A),
            );
            let bindings = &mut self.settings.players[player].bindings;
            match key {
                Key::Escape => {}
                Key::Backspace | Key::Delete if pad => bindings.bind_pad(button, None),
                Key::Backspace | Key::Delete => bindings.bind_key(button, None),
                _ if pad => {}
                _ => bindings.bind_key(button, Some(key)),
            }
            self.settings_dirty = true;
            return;
        }
        if self.menu_open() {
            if pressed {
                if key == Key::F11 && !repeat {
                    self.hotkey(platform, Hotkey::Fullscreen);
                } else if let Some(nav) = nav_for_key(key) {
                    self.navs.push(nav);
                }
            }
            return;
        }
        // Playing: a bound key is a console button, otherwise maybe a hotkey.
        let bound = (0..2).find_map(|p| {
            self.settings.players[p]
                .bindings
                .button_for_key(key)
                .map(|b| (p, b))
        });
        if let Some((player, button)) = bound {
            if !repeat {
                self.keyboard[player].set(button.bits(), pressed);
            }
            return;
        }
        match hotkey_for_key(key) {
            Some(Hotkey::FastForward) => self.fast_forward_key = pressed,
            Some(Hotkey::Rewind) => self.rewind_key = pressed,
            Some(hotkey) if pressed && !repeat => self.hotkey(platform, hotkey),
            _ => {}
        }
    }

    fn hotkey(&mut self, platform: &mut dyn Platform, hotkey: Hotkey) {
        match hotkey {
            Hotkey::Menu => self.open_menu(),
            Hotkey::Fullscreen => {
                if self.caps.fullscreen {
                    self.settings.video.fullscreen = !self.settings.video.fullscreen;
                    self.settings_dirty = true;
                    platform.request(Request::SetFullscreen(self.settings.video.fullscreen));
                }
            }
            Hotkey::Mute => {
                self.settings.audio.mute = !self.settings.audio.mute;
                self.settings_dirty = true;
                self.notify(if self.settings.audio.mute {
                    "Sound off"
                } else {
                    "Sound on"
                });
            }
            Hotkey::Debugger => {
                if self.caps.debugger {
                    platform.request(Request::ToggleDebugger);
                }
            }
            Hotkey::FastForward | Hotkey::Rewind => {}
            _ => self.game_hotkey(platform, hotkey),
        }
    }

    fn game_hotkey(&mut self, platform: &mut dyn Platform, hotkey: Hotkey) {
        let Some(game) = &mut self.game else {
            return;
        };
        let message = match hotkey {
            Hotkey::Pause => {
                game.paused = !game.paused;
                Ok(if game.paused { "Paused" } else { "Resumed" }.to_string())
            }
            Hotkey::FrameStep => {
                game.step_frame = game.paused;
                return;
            }
            Hotkey::SaveState => game
                .save_state(game.slot, platform)
                .map(|()| format!("Saved state to slot {}", game.slot)),
            Hotkey::LoadState => game
                .load_state(game.slot, platform)
                .map(|()| format!("Loaded state from slot {}", game.slot)),
            Hotkey::PreviousSlot | Hotkey::NextSlot => {
                let step = if hotkey == Hotkey::NextSlot {
                    1
                } else {
                    SLOTS - 1
                };
                game.slot = (game.slot + step) % SLOTS;
                Ok(format!("Slot {}", game.slot))
            }
            Hotkey::Reset => {
                game.genesis.reset();
                Ok("Reset".to_string())
            }
            Hotkey::Screenshot => {
                let frame = game.genesis.frame();
                let png = crate::png::encode(frame.pixels, frame.width, frame.height, frame.stride);
                let key = FileKey::Screenshot {
                    rom: &game.rom,
                    frame: game.genesis.frame_count(),
                };
                platform
                    .store(key, &png)
                    .map(|place| format!("Saved {place}"))
            }
            _ => return,
        };
        match message {
            Ok(text) => {
                platform.log(&text);
                self.notify(text);
            }
            Err(e) => self.error(platform, e),
        }
    }

    fn pad_button(
        &mut self,
        platform: &mut dyn Platform,
        id: u32,
        button: PadButton,
        pressed: bool,
    ) {
        self.mode = InputMode::Pad;
        let Some(index) = self.pads.iter().position(|p| p.id == id) else {
            return;
        };
        let bit = 1u32 << button as u32;
        let pad = &mut self.pads[index];
        if pressed {
            pad.held |= bit;
        } else {
            pad.held &= !bit;
        }
        let held = pad.held;
        if let Some(remap) = self.capturing() {
            if pressed && remap.pad {
                let button_to_bind = remap.waiting.take().unwrap_or(ConsoleButton::A);
                let player = remap.player;
                self.settings.players[player]
                    .bindings
                    .bind_pad(button_to_bind, Some(button));
                self.settings_dirty = true;
            }
            return;
        }
        if self.menu_open() {
            if button == PadButton::Guide && pressed {
                if matches!(
                    self.stack.last(),
                    Some(Layer {
                        screen: Screen::Pause,
                        ..
                    })
                ) {
                    self.navs.push(Nav::Back);
                }
            } else if let Some(nav) = nav_for_pad(button) {
                if pressed {
                    self.navs.push(nav);
                    if matches!(nav, Nav::Up | Nav::Down | Nav::Left | Nav::Right) {
                        self.repeat = Some((nav, self.now + REPEAT_DELAY_MS));
                    }
                } else if self.repeat.is_some_and(|(n, _)| n == nav) {
                    self.repeat = None;
                }
            }
            return;
        }
        // Playing.
        let chord = (1 << PadButton::Back as u32) | (1 << PadButton::Start as u32);
        if pressed && (button == PadButton::Guide || held & chord == chord) {
            self.open_menu();
            return;
        }
        let pad = &mut self.pads[index];
        if let Some(b) = self.settings.players[pad.player]
            .bindings
            .button_for_pad(button)
        {
            pad.buttons.set(b.bits(), pressed);
        }
        let _ = platform;
    }

    fn pad_axis(&mut self, id: u32, axis: PadAxis, value: f32) {
        let Some(pad) = self.pads.iter_mut().find(|p| p.id == id) else {
            return;
        };
        match axis {
            PadAxis::LeftX => pad.stick_xy.0 = value,
            PadAxis::LeftY => pad.stick_xy.1 = value,
            PadAxis::LeftTrigger => pad.rewind = value > TRIGGER_THRESHOLD,
            PadAxis::RightTrigger => pad.fast_forward = value > TRIGGER_THRESHOLD,
            PadAxis::RightX | PadAxis::RightY => {}
        }
        if !matches!(axis, PadAxis::LeftX | PadAxis::LeftY) {
            return;
        }
        // The stick as a d-pad, with a dead zone around the centre.
        let (x, y) = pad.stick_xy;
        let old = pad.stick;
        let mut dirs = Buttons::default();
        dirs.set(Buttons::LEFT, x < -STICK_DEAD_ZONE);
        dirs.set(Buttons::RIGHT, x > STICK_DEAD_ZONE);
        dirs.set(Buttons::UP, y < -STICK_DEAD_ZONE);
        dirs.set(Buttons::DOWN, y > STICK_DEAD_ZONE);
        pad.stick = dirs;
        if dirs != old && self.menu_open() {
            self.mode = InputMode::Pad;
            // In menus, the dominant direction of the stick navigates.
            let nav = if dirs == Buttons::default() {
                None
            } else if x.abs() > y.abs() {
                Some(if x < 0.0 { Nav::Left } else { Nav::Right })
            } else {
                Some(if y < 0.0 { Nav::Up } else { Nav::Down })
            };
            match nav {
                Some(nav) if self.repeat.map(|(n, _)| n) != Some(nav) => {
                    self.navs.push(nav);
                    self.repeat = Some((nav, self.now + REPEAT_DELAY_MS));
                }
                None => self.repeat = None,
                _ => {}
            }
        }
    }

    fn pointer_event(&mut self, id: u64, kind: PointerKind, phase: PointerPhase, x: i32, y: i32) {
        if kind == PointerKind::Touch {
            self.mode = InputMode::Pointer;
            if !self.touch_seen {
                self.touch_seen = true;
                self.relayout();
            }
        } else if phase != PointerPhase::Move {
            self.mode = InputMode::Pointer;
        }
        if self.menu_open() {
            if let Some(remap) = self.capturing() {
                // Tapping anywhere cancels waiting for a key.
                if phase == PointerPhase::Down {
                    remap.waiting = None;
                }
                return;
            }
            self.ui_pointer(id, kind, phase, x, y);
            return;
        }
        // Playing.
        let touch_controls = self.touch_visible();
        if touch_controls && (kind == PointerKind::Touch || self.settings.touch == TouchMode::On) {
            self.touch.pointer(id, phase, x, y);
            return;
        }
        if kind == PointerKind::Mouse {
            self.mouse_menu_until = self.now + MOUSE_MENU_MS;
            if phase == PointerPhase::Down && self.mouse_menu_rect().contains(x, y) {
                self.open_menu();
            }
        }
    }

    fn ui_pointer(&mut self, id: u64, kind: PointerKind, phase: PointerPhase, x: i32, y: i32) {
        let p = &mut self.pointer;
        match phase {
            PointerPhase::Down if p.id.is_none() => {
                p.id = Some(id);
                p.frame.press = Some((x, y));
                p.frame.held = Some((x, y));
                p.down_at = (x, y);
                p.last = (x, y);
                p.dragging = false;
            }
            PointerPhase::Move if p.id == Some(id) => {
                p.frame.held = Some((x, y));
                if !p.dragging && (y - p.down_at.1).abs() > DRAG_THRESHOLD {
                    p.dragging = true;
                }
                if p.dragging {
                    p.frame.scroll += p.last.1 - y;
                }
                p.last = (x, y);
            }
            PointerPhase::Move if kind == PointerKind::Mouse && p.id.is_none() => {
                p.frame.hover = Some((x, y));
            }
            PointerPhase::Up if p.id == Some(id) => {
                if !p.dragging {
                    p.frame.release = Some((x, y));
                }
                p.frame.held = None;
                p.id = None;
            }
            PointerPhase::Cancel if p.id == Some(id) => {
                p.frame.held = None;
                p.frame.press = None;
                p.id = None;
            }
            _ => {}
        }
    }

    // --- The frame ---------------------------------------------------------------------

    /// Run one frame: menus or emulation. Returns how to wait before the
    /// next call.
    pub fn update(&mut self, platform: &mut dyn Platform) -> Pacing {
        self.now = platform.now_ms();
        if self.toast.as_ref().is_some_and(|t| t.until_ms <= self.now) {
            self.toast = None;
        }
        if self.touch.take_menu_tap() {
            self.open_menu();
        }
        let now = self.now;
        if let Some(remap) = self.capturing() {
            if now.saturating_sub(remap.since_ms) > REMAP_TIMEOUT_MS {
                remap.waiting = None;
            }
        }
        let pacing = if self.menu_open() {
            if let Some((nav, at)) = self.repeat {
                if self.now >= at {
                    self.navs.push(nav);
                    self.repeat = Some((nav, self.now + REPEAT_MS));
                }
            }
            self.run_menus(platform);
            self.hud = None;
            // Keep the sound device fed with silence-free nothing: just pace.
            Pacing::Timer { fps: 60.0 }
        } else {
            self.repeat = None;
            self.navs.clear();
            let pacing = self.emulate(platform);
            self.draw_hud();
            pacing
        };
        if let Some(game) = &mut self.game {
            game.flush_save_periodically(platform);
        }
        self.update_title(platform);
        pacing
    }

    fn run_menus(&mut self, platform: &mut dyn Platform) {
        let navs = std::mem::take(&mut self.navs);
        let pointer = std::mem::take(&mut self.pointer.frame);
        // Keep "held" for the next frame.
        self.pointer.frame.held = pointer.held;
        for pass in 0..3 {
            let Some(mut layer) = self.stack.pop() else {
                break;
            };
            if self.game.is_some() {
                self.overlay.clear(0);
                let bounds = self.overlay.bounds();
                self.overlay.blend(bounds, theme::SCRIM);
                // A sheet behind the menu column keeps text readable over
                // any picture, while the game stays visible around it.
                let width = (bounds.w - 2 * theme::MARGIN).min(theme::COLUMN) + 2 * theme::MARGIN;
                let sheet = Rect::new((bounds.w - width) / 2, 0, width, bounds.h);
                self.overlay.blend(sheet, with_alpha(0x15171C, 0xF4));
                self.overlay
                    .fill(Rect::new(sheet.x, 0, 1, bounds.h), theme::LINE);
                self.overlay
                    .fill(Rect::new(sheet.right() - 1, 0, 1, bounds.h), theme::LINE);
            } else {
                self.overlay.clear(theme::BACKGROUND);
            }
            let before = self.settings.clone();
            let (input_navs, input_pointer): (&[Nav], PointerInput) = if pass == 0 {
                (&navs, pointer)
            } else {
                (
                    &[],
                    PointerInput {
                        held: pointer.held,
                        ..PointerInput::default()
                    },
                )
            };
            let pads = self
                .pads
                .iter()
                .map(|p| (p.name.clone(), p.player))
                .collect();
            let (action, redraw) = {
                let mut ui = Ui::new(
                    &mut self.overlay,
                    &mut layer.focus,
                    input_navs,
                    input_pointer,
                );
                let mut ctx = Ctx {
                    settings: &mut self.settings,
                    caps: &self.caps,
                    game: self.game.as_ref(),
                    platform: &mut *platform,
                    mode: self.mode,
                    pads,
                    now_ms: self.now,
                };
                let action = screens::draw(&mut layer.screen, &mut ui, &mut ctx);
                let redraw = ui.redraw;
                (action, ui.finish() || redraw)
            };
            self.stack.push(layer);
            if let Some(toast) = self.toast.clone() {
                // Above the two lines of hints.
                let y = self.overlay.height as i32 - theme::MARGIN - 8 - 11 - 26;
                draw_toast(&mut self.overlay, &toast, y);
            }
            if self.settings != before {
                self.settings_changed(platform, &before);
            }
            let acted = !matches!(action, Action::None);
            self.apply(platform, action);
            if !(redraw || acted) || !self.menu_open() {
                break;
            }
        }
        self.overlay_shown = self.menu_open();
        self.overlay_changed = true;
    }

    /// React to settings edited in a menu.
    fn settings_changed(&mut self, platform: &mut dyn Platform, before: &Settings) {
        self.settings_dirty = true;
        let now = &self.settings;
        if now.video.fullscreen != before.video.fullscreen {
            platform.request(Request::SetFullscreen(now.video.fullscreen));
        }
        if let Some(game) = &mut self.game {
            for (port, player) in now.players.iter().enumerate() {
                game.genesis.set_device(port, player.device);
            }
            if now.emulation.rewind != before.emulation.rewind {
                game.rewind = now
                    .emulation
                    .rewind
                    .then(|| gase_core::Rewind::new(20, 4, game.genesis.frame_rate()));
            }
        }
        if now.video != before.video
            || now.touch != before.touch
            || now.players[0].device != before.players[0].device
        {
            self.relayout();
        }
    }

    fn apply(&mut self, platform: &mut dyn Platform, action: Action) {
        match action {
            Action::None => return,
            Action::Push(screen) => self.stack.push(Layer {
                screen,
                focus: Focus::default(),
            }),
            Action::Pop => {
                self.stack.pop();
                if self.stack.is_empty() && self.game.is_none() {
                    self.close_game(platform);
                }
            }
            Action::Resume => {
                if self.game.is_some() {
                    self.stack.clear();
                }
            }
            Action::Open(path) => {
                let _ = self.open_rom(platform, &path);
            }
            Action::Pick => platform.request(Request::PickRom),
            Action::CloseGame => self.close_game(platform),
            Action::Reset => {
                if let Some(game) = &mut self.game {
                    game.genesis.reset();
                    self.stack.clear();
                    self.notify("Reset");
                }
            }
            Action::SaveState(slot) => {
                if let Some(game) = &mut self.game {
                    match game.save_state(slot, platform) {
                        Ok(()) => {
                            let slots = game.slots(platform);
                            if let Some(Layer {
                                screen: Screen::States { slots: shown, .. },
                                ..
                            }) = self.stack.last_mut()
                            {
                                *shown = slots;
                            }
                            self.notify(format!("Saved state to slot {slot}"));
                        }
                        Err(e) => self.error(platform, e),
                    }
                }
            }
            Action::LoadState(slot) => {
                if let Some(game) = &mut self.game {
                    match game.load_state(slot, platform) {
                        Ok(()) => {
                            self.stack.clear();
                            self.notify(format!("Loaded state from slot {slot}"));
                        }
                        Err(e) => self.error(platform, e),
                    }
                }
            }
            Action::Quit => {
                self.quit = true;
                platform.request(Request::Quit);
            }
        }
        // Leaving a screen is a good moment to write changed settings.
        self.save_settings_if_leaving(platform);
    }

    fn save_settings_if_leaving(&mut self, platform: &mut dyn Platform) {
        let in_settings = self.stack.iter().any(|l| {
            matches!(
                l.screen,
                Screen::Settings
                    | Screen::Video
                    | Screen::Audio
                    | Screen::Emulation
                    | Screen::Controls
                    | Screen::Remap(_)
            )
        });
        if !in_settings {
            self.save_settings(platform);
        }
    }

    /// The console buttons each player holds, from every source.
    fn player_buttons(&self) -> [Buttons; 2] {
        let mut players = self.keyboard;
        for pad in &self.pads {
            players[pad.player] = players[pad.player] | pad.buttons | pad.stick;
        }
        if self.touch_visible() {
            players[0] = players[0] | self.touch.buttons();
        }
        players
    }

    fn emulate(&mut self, platform: &mut dyn Platform) -> Pacing {
        let buttons = self.player_buttons();
        let fast_forward = self.fast_forward_key || self.pads.iter().any(|p| p.fast_forward);
        let rewinding = self.rewind_key || self.pads.iter().any(|p| p.rewind);
        let Some(game) = &mut self.game else {
            return Pacing::Timer { fps: 60.0 };
        };
        for (port, b) in buttons.into_iter().enumerate() {
            game.genesis.set_buttons(port, b);
        }
        let fps = game.genesis.frame_rate();
        let run = !game.paused || std::mem::take(&mut game.step_frame);
        let mut breakpoint = None;
        if run {
            if rewinding {
                game.rewind_step();
            } else {
                let count = if fast_forward {
                    self.settings.emulation.fast_forward
                } else {
                    1
                };
                breakpoint = game.run_frames(count);
            }
        }
        self.audio.clear();
        game.genesis.drain_audio(&mut self.audio);
        let normal_speed = run && !fast_forward && !rewinding;
        let pacing = match platform.audio_queued() {
            Some(queued) if normal_speed => {
                // Dynamic rate control: ±0.5 % depending on the fill level.
                let target = (self.caps.sample_rate * AUDIO_TARGET_MS / 1000) as usize;
                let error = (target as f64 - queued as f64) / target as f64;
                game.genesis
                    .set_audio_speed(1.0 + 0.005 * error.clamp(-1.0, 1.0));
                apply_volume(&mut self.audio, &self.settings);
                platform.queue_audio(&self.audio);
                Pacing::Audio { target }
            }
            _ if run && fast_forward => Pacing::Unthrottled,
            _ => Pacing::Timer { fps },
        };
        if let Some(pc) = breakpoint {
            platform.request(Request::Breakpoint(pc));
            self.notify(format!("Breakpoint at ${pc:06X}"));
        }
        pacing
    }

    /// The mouse's menu button (top right), in UI pixels.
    fn mouse_menu_rect(&self) -> Rect {
        let w = text_width("Menu", 1) + 24;
        Rect::new(
            self.overlay.width as i32 - theme::MARGIN / 2 - w,
            theme::MARGIN / 2,
            w,
            16,
        )
    }

    /// Redraw the in-game overlay if something on it changed.
    fn draw_hud(&mut self) {
        let touch = self.touch_visible();
        let hud = Hud {
            touch: touch.then(|| (self.touch.buttons().0, self.touch.layout().is_some())),
            mouse_menu: !touch && self.now < self.mouse_menu_until,
            toast: self.toast.clone(),
            size: (self.overlay.width, self.overlay.height),
        };
        if self.hud.as_ref() == Some(&hud) {
            return;
        }
        self.overlay_shown = hud.touch.is_some() || hud.mouse_menu || hud.toast.is_some();
        if self.overlay_shown {
            self.overlay.clear(0);
            if touch {
                self.touch.draw(&mut self.overlay);
            }
            if hud.mouse_menu {
                let r = self.mouse_menu_rect();
                self.overlay.rounded(r, 3, with_alpha(0x0C0D11, 0xC0));
                for i in -1..=1 {
                    self.overlay
                        .fill(Rect::new(r.x + 7, r.y + 7 + i * 3, 8, 1), theme::TEXT);
                }
                text_scaled(&mut self.overlay, r.x + 19, r.y + 4, "Menu", theme::TEXT, 1);
            }
            if let Some(toast) = &hud.toast {
                draw_toast(
                    &mut self.overlay,
                    toast,
                    theme::MARGIN / 2 + if hud.mouse_menu { 20 } else { 0 },
                );
            }
        }
        self.overlay_changed = true;
        self.hud = Some(hud);
    }

    fn update_title(&mut self, platform: &mut dyn Platform) {
        let mut title = String::from("gase");
        if let Some(game) = &self.game {
            title = format!("gase - {}", game.title);
            if game.paused {
                title.push_str(" [paused]");
            } else if self.menu_open() {
                title.push_str(" [menu]");
            } else if self.fast_forward_key {
                title.push_str(" [fast forward]");
            } else if self.rewind_key {
                title.push_str(" [rewind]");
            }
        }
        if title != self.title {
            platform.request(Request::SetTitle(title.clone()));
            self.title = title;
        }
    }

    // --- Output ----------------------------------------------------------------------

    /// What to draw now.
    pub fn video(&mut self) -> Video<'_> {
        let changed = std::mem::take(&mut self.overlay_changed);
        Video {
            game: self.game.as_ref().map(Game::frame),
            game_rect: self.game_rect(),
            overlay: self.overlay_shown.then_some(&self.overlay),
            overlay_scale: self.scale,
            overlay_changed: changed,
            background: opaque(0),
        }
    }

    /// The samples produced by the last [`App::update`] (already sent to
    /// [`Platform::queue_audio`] when playing at normal speed).
    #[must_use]
    pub fn audio(&self) -> &[i16] {
        &self.audio
    }

    /// The screen as one picture, game and overlay composited, as the shell
    /// would show it: for screenshots of the UI and tests.
    pub fn compose(&mut self) -> Canvas {
        let (w, h) = self.size;
        let mut out = Canvas::new(w as usize, h as usize, opaque(0));
        let video = self.video();
        if let Some(image) = video.game {
            out.draw_scaled(video.game_rect, image, false);
        }
        if let Some(overlay) = video.overlay {
            let s = video.overlay_scale;
            let dest = Rect::new(0, 0, overlay.width as i32 * s, overlay.height as i32 * s);
            out.draw_scaled(dest, overlay.image(), true);
        }
        out
    }

    /// Open a screen directly (tests and `--dump-ui`): `"home"`, `"pause"`,
    /// `"save"`, `"load"`, `"settings"`, `"video"`, `"audio"`,
    /// `"emulation"`, `"controls"`, `"remap"`, `"browser"`.
    /// Returns false for an unknown name.
    pub fn show_screen(&mut self, platform: &mut dyn Platform, name: &str) -> bool {
        let screen = match name {
            "home" => {
                self.stack.clear();
                if self.game.is_some() {
                    return false;
                }
                Screen::Home
            }
            "pause" => Screen::Pause,
            "save" | "load" => {
                let slots = self
                    .game
                    .as_ref()
                    .map(|g| g.slots(platform))
                    .unwrap_or_default();
                Screen::States {
                    save: name == "save",
                    slots,
                }
            }
            "settings" => Screen::Settings,
            "video" => Screen::Video,
            "audio" => Screen::Audio,
            "emulation" => Screen::Emulation,
            "controls" => Screen::Controls,
            "remap" | "remap-waiting" => Screen::Remap(Remap {
                player: 0,
                pad: false,
                waiting: (name == "remap-waiting").then_some(ConsoleButton::B),
                since_ms: self.now,
            }),
            "browser" => Screen::Browser(crate::browser::Browser::open(
                platform,
                self.settings.browse_dir.as_deref(),
            )),
            _ => return false,
        };
        if matches!(screen, Screen::Pause) && self.game.is_none() {
            return false;
        }
        self.stack.push(Layer {
            screen,
            focus: Focus::default(),
        });
        true
    }

    /// Feed a [`Nav`] command directly, as if from a key (for tests).
    pub fn nav(&mut self, nav: Nav) {
        self.navs.push(nav);
    }

    /// The name of the top screen (for tests and logs).
    #[must_use]
    pub fn screen_name(&self) -> &'static str {
        match self.stack.last().map(|l| &l.screen) {
            None => "game",
            Some(Screen::Home) => "home",
            Some(Screen::Browser(_)) => "browser",
            Some(Screen::Pause) => "pause",
            Some(Screen::States { save: true, .. }) => "save",
            Some(Screen::States { save: false, .. }) => "load",
            Some(Screen::Settings) => "settings",
            Some(Screen::Video) => "video",
            Some(Screen::Audio) => "audio",
            Some(Screen::Emulation) => "emulation",
            Some(Screen::Controls) => "controls",
            Some(Screen::Remap(r)) if r.waiting.is_some() => "remap-waiting",
            Some(Screen::Remap(_)) => "remap",
        }
    }

    /// The focused item of the top screen.
    #[must_use]
    pub fn focus_index(&self) -> Option<usize> {
        self.stack.last().map(|l| l.focus.index)
    }

    /// Where item `index` of the top screen was drawn (UI pixels).
    #[must_use]
    pub fn item_rect(&self, index: usize) -> Option<Rect> {
        self.stack.last()?.focus.item_rect(index)
    }

    /// Screen pixels per UI pixel.
    #[must_use]
    pub fn ui_scale(&self) -> i32 {
        self.scale
    }

    /// The file name of the running game's ROM.
    #[must_use]
    pub fn rom_name(&self) -> Option<&str> {
        self.game.as_ref().map(|g| game::file_name(&g.rom))
    }
}

/// Scale samples by the volume setting.
fn apply_volume(samples: &mut [i16], settings: &Settings) {
    let volume = if settings.audio.mute {
        0
    } else {
        i32::from(settings.audio.volume)
    };
    if volume < 100 {
        for s in samples {
            *s = (i32::from(*s) * volume / 100) as i16;
        }
    }
}

/// A message box centred horizontally at height `y`.
fn draw_toast(canvas: &mut Canvas, toast: &Toast, y: i32) {
    let max = ((canvas.width as i32 - 2 * theme::MARGIN - 16) / 8).max(4) as usize;
    let text = crate::font::ellipsize(&toast.text, max, false);
    let w = text_width(&text, 1) + 16;
    let r = Rect::new((canvas.width as i32 - w) / 2, y, w, 18);
    canvas.rounded(r, 3, with_alpha(0x17191F, 0xF0));
    let accent = if toast.error {
        theme::ERROR
    } else {
        theme::ACCENT
    };
    canvas.fill(Rect::new(r.x + 3, r.y + 4, 2, 10), accent);
    let color = if toast.error {
        theme::ERROR
    } else {
        theme::TEXT
    };
    text_scaled(canvas, r.x + 9, r.y + 5, &text, color, 1);
}
