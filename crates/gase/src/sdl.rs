//! The SDL2 shell: a window, sound, and input devices around
//! [`gase_app::App`], for the desktop **and** for Android and iOS.
//!
//! Everything the user sees and touches is decided by the app; this file
//! only translates. Each pass of the main loop:
//!
//! 1. **Events in.** SDL's events become [`gase_app::Event`]s: scancodes
//!    become [`Key`]s, SDL GameController buttons (already in the standard
//!    layout, for any pad SDL knows) become [`PadButton`]s, mouse and
//!    fingers become pointers, dropped files become ROMs to open.
//! 2. **Update.** [`App::update`] runs the menus or the emulator and
//!    queues the sound (`SdlPlatform::queue_audio`).
//! 3. **Requests.** What the app asked for (window title, fullscreen,
//!    quit, the debugger, the system's file picker) is carried out.
//! 4. **Video out.** The game frame and, when there is one, the overlay are
//!    uploaded to two streaming textures and drawn: the game stretched into
//!    its rectangle, the overlay enlarged by a whole factor and blended on
//!    top (GPU work, so menus cost almost nothing).
//! 5. **Wait** as the app's [`Pacing`] says.
//!
//! # One shell, three systems
//!
//! SDL2 runs on Linux, Windows and macOS, and also on Android and iOS: it
//! hides each system's window, OpenGL/Metal surface, sound, touch and
//! gamepad APIs behind the same functions. So the phone apps run *this*
//! loop too, through [`run_sdl`] with [`Shell::Mobile`]; only a few
//! things differ, all decided in this file:
//!
//! | | [`Shell::Desktop`] | [`Shell::Mobile`] |
//! |---|---|---|
//! | Files | [`Desktop`]: saves next to the ROM | [`MobileFiles`]: the app's sandbox |
//! | Window | resizable, `--scale`, F11 fullscreen | always full screen, rotates with the phone |
//! | Opening ROMs | built-in file browser, drag and drop | the system's document picker ([`Mobile::pick_rom`]) and "Open with" |
//! | Density | drawable size ÷ window size (Retina, HiDPI) | the same on iOS; the display's DPI ÷ 160 on Android |
//! | Lifecycle | runs until closed | goes to the background, see below |
//! | Extras | debugger window, `--trace`, breakpoints | Back button |
//!
//! # The mobile lifecycle
//!
//! A phone app does not decide when it stops. The user switches apps, a
//! call comes in, the screen locks: the app is sent to the *background*,
//! where it must not draw (iOS kills an app that uses the GPU in the
//! background) and may be killed at any moment without further notice to
//! free memory. SDL reports this with application events:
//!
//! ```text
//!  SDL_APP_WILLENTERBACKGROUND ─► write the save and settings NOW,
//!                                 open the pause menu, pause the sound,
//!                                 stop drawing, sleep in SDL_WaitEvent
//!  … the app may be killed here; nothing is lost …
//!  SDL_APP_DIDENTERFOREGROUND  ─► drop stale sound, resume it, re-read
//!                                 the screen size (it may have rotated);
//!                                 the game waits in the pause menu
//!  SDL_APP_TERMINATING         ─► the system is closing the app: write
//!                                 everything and leave
//! ```
//!
//! On Android SDL's own Java activity guarantees that the main loop sees
//! the background event before it blocks the native thread; on iOS the
//! Objective-C app delegate asks the system for a moment of background
//! time for the same purpose (see `mobile/ios/Sources/GaseAppDelegate.m`).
//!
//! # Audio/video synchronisation
//!
//! The console produces ≈59.92 frames per second; the host display usually
//! refreshes at 60 Hz, and the sound card consumes samples at its own pace.
//! We let **audio drive timing**: after each frame we wait until the sound
//! queue has drained to a target level, then emulate the next frame. To keep
//! the queue from slowly over- or under-flowing, the app nudges the
//! resampler's ratio by up to ±0.5% depending on how full the queue is
//! (dynamic rate control) — far too little to hear, but enough to absorb
//! clock drift.
//!
//! # The debugger window
//!
//! F1 (or the backtick key) opens a second window with the debugger
//! (`debugger::Panel`). While it has the keyboard focus, keys go to
//! the debugger instead of the game. The debugger is a desktop feature (a
//! phone has one window), so it lives here and works on the app's
//! [`gase_app::Game`].

use std::time::{Duration, Instant};

use gase_app::{
    App, Capabilities, DebugRun, Event, FileKey, Key, Listing, Pacing, PadAxis, PadButton,
    Platform, PointerKind, PointerPhase, Request,
};
use gase_core::Stop;
use sdl2::audio::{AudioQueue, AudioSpecDesired};
use sdl2::controller::{Axis, Button, GameController};
use sdl2::event::{Event as SdlEvent, WindowEvent};
use sdl2::keyboard::{Keycode, Scancode};
use sdl2::mouse::MouseButton;
use sdl2::pixels::PixelFormatEnum;
use sdl2::rect::Rect as SdlRect;
use sdl2::render::{BlendMode, Texture, TextureCreator, WindowCanvas};
use sdl2::video::{FullscreenType, WindowContext};

use crate::cli::Options;
use crate::debugger::{self, Action, Panel};
use crate::desktop::Desktop;
use crate::mobile::MobileFiles;

const SAMPLE_RATE: u32 = 48_000;
/// SDL reports mouse events it synthesises from touches with this id; the
/// touches themselves arrive as finger events.
const TOUCH_MOUSE_ID: u32 = u32::MAX;
/// Android's baseline density: 160 dots per inch is one density-independent
/// pixel ("dp") per physical pixel.
const ANDROID_BASELINE_DPI: f32 = 160.0;

/// Which shell to run: see the [module documentation](self).
#[derive(Debug)]
pub enum Shell<'a> {
    /// A window on a desktop, with the command line's options.
    Desktop(&'a Options),
    /// A full-screen app on a phone or tablet (Android, iOS).
    Mobile(Mobile),
}

/// What the native side of a phone app gives the shell: a ROM to start
/// with, and the few things only Java or Objective-C can do.
///
/// The functions are plain `fn()`s so that the unsafe calls into native
/// code stay in the small `gase-mobile` crate (`crates/mobile`).
#[derive(Clone, Debug, Default)]
pub struct Mobile {
    /// A ROM to open at start: the app was launched by opening a file
    /// ("Open with" on Android).
    pub open: Option<String>,
    /// Show the system's document picker. The answer comes back later, as
    /// an SDL drop-file event with the path of a copy in the app's sandbox.
    pub pick_rom: Option<fn()>,
    /// The Back button was pressed on the home screen: do what Android
    /// does by default (leave the app).
    pub back_at_home: Option<fn()>,
    /// Trying the phone shell on a desktop (`gase --mobile`): a window of
    /// this size instead of the whole screen.
    pub window: Option<(u32, u32)>,
}

/// The platform for the app: files from [`Desktop`] or [`MobileFiles`],
/// sound from SDL, and the requests the main loop carries out after each
/// update.
struct SdlPlatform {
    files: Box<dyn Platform>,
    audio: Option<AudioQueue<i16>>,
    requests: Vec<Request>,
    /// Log through SDL (Android's logcat, the iOS console) instead of
    /// stderr, which goes nowhere on a phone.
    sdl_log: bool,
}

impl SdlPlatform {
    /// Stop the sound (going to the background).
    fn pause_audio(&mut self) {
        if let Some(queue) = &self.audio {
            queue.pause();
        }
    }

    /// Restart the sound, without what was queued before the pause.
    fn resume_audio(&mut self) {
        if let Some(queue) = &self.audio {
            queue.clear();
            queue.resume();
        }
    }
}

impl Platform for SdlPlatform {
    fn now_ms(&self) -> u64 {
        self.files.now_ms()
    }
    fn log(&mut self, message: &str) {
        if self.sdl_log {
            sdl2::log::log(message);
        } else {
            self.files.log(message);
        }
    }
    fn load(&mut self, file: FileKey<'_>) -> Option<Vec<u8>> {
        self.files.load(file)
    }
    fn store(&mut self, file: FileKey<'_>, data: &[u8]) -> Result<String, String> {
        self.files.store(file, data)
    }
    fn read_rom(&mut self, path: &str) -> Result<Vec<u8>, String> {
        self.files.read_rom(path)
    }
    fn list_dir(&mut self, dir: Option<&str>) -> Result<Listing, String> {
        self.files.list_dir(dir)
    }
    fn audio_queued(&self) -> Option<usize> {
        // The queue size is in bytes: 4 per stereo frame of i16.
        self.audio.as_ref().map(|q| q.size() as usize / 4)
    }
    fn queue_audio(&mut self, samples: &[i16]) {
        let result = self.audio.as_ref().map(|q| q.queue_audio(samples));
        if let Some(Err(e)) = result {
            self.log(&format!("audio: {e}"));
        }
    }
    fn request(&mut self, request: Request) {
        self.requests.push(request);
    }
}

/// SDL scancode → the app's physical key.
fn key(code: Scancode) -> Option<Key> {
    use Scancode as S;
    Some(match code {
        S::A => Key::A,
        S::B => Key::B,
        S::C => Key::C,
        S::D => Key::D,
        S::E => Key::E,
        S::F => Key::F,
        S::G => Key::G,
        S::H => Key::H,
        S::I => Key::I,
        S::J => Key::J,
        S::K => Key::K,
        S::L => Key::L,
        S::M => Key::M,
        S::N => Key::N,
        S::O => Key::O,
        S::P => Key::P,
        S::Q => Key::Q,
        S::R => Key::R,
        S::S => Key::S,
        S::T => Key::T,
        S::U => Key::U,
        S::V => Key::V,
        S::W => Key::W,
        S::X => Key::X,
        S::Y => Key::Y,
        S::Z => Key::Z,
        S::Num0 => Key::Num0,
        S::Num1 => Key::Num1,
        S::Num2 => Key::Num2,
        S::Num3 => Key::Num3,
        S::Num4 => Key::Num4,
        S::Num5 => Key::Num5,
        S::Num6 => Key::Num6,
        S::Num7 => Key::Num7,
        S::Num8 => Key::Num8,
        S::Num9 => Key::Num9,
        S::F1 => Key::F1,
        S::F2 => Key::F2,
        S::F3 => Key::F3,
        S::F4 => Key::F4,
        S::F5 => Key::F5,
        S::F6 => Key::F6,
        S::F7 => Key::F7,
        S::F8 => Key::F8,
        S::F9 => Key::F9,
        S::F10 => Key::F10,
        S::F11 => Key::F11,
        S::F12 => Key::F12,
        S::Up => Key::Up,
        S::Down => Key::Down,
        S::Left => Key::Left,
        S::Right => Key::Right,
        S::Return => Key::Enter,
        // Android's Back button (or gesture) is Escape: it opens and leaves
        // menus. (The mobile shell handles it on the home screen.)
        S::Escape | S::AcBack => Key::Escape,
        S::Backspace => Key::Backspace,
        S::Tab => Key::Tab,
        S::Space => Key::Space,
        S::Insert => Key::Insert,
        S::Delete => Key::Delete,
        S::Home => Key::Home,
        S::End => Key::End,
        S::PageUp => Key::PageUp,
        S::PageDown => Key::PageDown,
        S::LShift => Key::LeftShift,
        S::RShift => Key::RightShift,
        S::LCtrl => Key::LeftCtrl,
        S::RCtrl => Key::RightCtrl,
        S::LAlt => Key::LeftAlt,
        S::RAlt => Key::RightAlt,
        S::Minus => Key::Minus,
        S::Equals => Key::Equals,
        S::LeftBracket => Key::LeftBracket,
        S::RightBracket => Key::RightBracket,
        S::Backslash => Key::Backslash,
        S::Semicolon => Key::Semicolon,
        S::Apostrophe => Key::Apostrophe,
        S::Grave => Key::Grave,
        S::Comma => Key::Comma,
        S::Period => Key::Period,
        S::Slash => Key::Slash,
        S::Kp0 => Key::Kp0,
        S::Kp1 => Key::Kp1,
        S::Kp2 => Key::Kp2,
        S::Kp3 => Key::Kp3,
        S::Kp4 => Key::Kp4,
        S::Kp5 => Key::Kp5,
        S::Kp6 => Key::Kp6,
        S::Kp7 => Key::Kp7,
        S::Kp8 => Key::Kp8,
        S::Kp9 => Key::Kp9,
        S::KpEnter => Key::KpEnter,
        S::KpPlus => Key::KpPlus,
        S::KpMinus => Key::KpMinus,
        S::KpMultiply => Key::KpMultiply,
        S::KpDivide => Key::KpDivide,
        S::KpPeriod => Key::KpPeriod,
        _ => return None,
    })
}

/// SDL GameController buttons are already the standard layout.
fn pad_button(button: Button) -> Option<PadButton> {
    Some(match button {
        Button::A => PadButton::South,
        Button::B => PadButton::East,
        Button::X => PadButton::West,
        Button::Y => PadButton::North,
        Button::Back => PadButton::Back,
        Button::Guide => PadButton::Guide,
        Button::Start => PadButton::Start,
        Button::LeftStick => PadButton::LeftStick,
        Button::RightStick => PadButton::RightStick,
        Button::LeftShoulder => PadButton::LeftShoulder,
        Button::RightShoulder => PadButton::RightShoulder,
        Button::DPadUp => PadButton::DPadUp,
        Button::DPadDown => PadButton::DPadDown,
        Button::DPadLeft => PadButton::DPadLeft,
        Button::DPadRight => PadButton::DPadRight,
        Button::Misc1 => PadButton::Misc,
        _ => return None,
    })
}

fn pad_axis(axis: Axis) -> PadAxis {
    match axis {
        Axis::LeftX => PadAxis::LeftX,
        Axis::LeftY => PadAxis::LeftY,
        Axis::RightX => PadAxis::RightX,
        Axis::RightY => PadAxis::RightY,
        Axis::TriggerLeft => PadAxis::LeftTrigger,
        Axis::TriggerRight => PadAxis::RightTrigger,
    }
}

/// Translate a key for the debugger panel.
fn debugger_key(code: Scancode, keycode: Option<Keycode>) -> Option<debugger::Key> {
    use debugger::Key as K;
    Some(match code {
        Scancode::Up => K::Up,
        Scancode::Down => K::Down,
        Scancode::PageDown => K::PageDown,
        Scancode::Home => K::Home,
        Scancode::Return | Scancode::KpEnter => K::Enter,
        Scancode::Backspace => K::Backspace,
        Scancode::Escape => K::Escape,
        _ => {
            // Printable keys: SDL key codes are their (lower-case) characters,
            // so this follows the keyboard layout.
            let c = char::from_u32(keycode?.into_i32() as u32)?;
            if !(' '..='~').contains(&c) {
                return None;
            }
            K::Char(c.to_ascii_lowercase())
        }
    })
}

/// The debugger window and its state.
struct DebugWindow {
    canvas: WindowCanvas,
    panel: Panel,
    open: bool,
    shown: bool,
}

impl DebugWindow {
    /// Handle a key pressed in the debugger window.
    fn key(&mut self, app: &mut App, key: debugger::Key) {
        let Some(game) = app.game_mut() else {
            return;
        };
        let what = match self.panel.key(key, &mut game.genesis, &mut game.debugger) {
            Action::None => return,
            Action::Close => {
                self.open = false;
                return;
            }
            Action::TogglePause => {
                game.paused = !game.paused;
                return;
            }
            Action::Step => DebugRun::Instruction,
            Action::StepFrame => DebugRun::Frame,
            Action::RunToVBlank => DebugRun::VBlank,
        };
        let stop = game.debug_run(what);
        for line in game.genesis.take_trace() {
            println!("{line}");
        }
        self.panel.set_status(match stop {
            Stop::Stepped => "Stepped one instruction".to_string(),
            Stop::FrameEnd => format!("Frame {} done", game.genesis.frame_count()),
            Stop::Breakpoint(pc) => format!("Breakpoint at ${pc:06X}"),
            Stop::VBlank => "Vertical interrupt raised: S steps into the handler".to_string(),
        });
        self.panel.follow_pc();
    }

    fn draw(&mut self, app: &App, texture: &mut Texture<'_>) -> Result<(), String> {
        if self.open != self.shown {
            let window = self.canvas.window_mut();
            if self.open {
                window.show();
                window.raise();
            } else {
                window.hide();
            }
            self.shown = self.open;
        }
        let Some(game) = app.game().filter(|_| self.open) else {
            return Ok(());
        };
        let picture = self.panel.draw(&game.genesis, &game.debugger, game.paused);
        upload(
            texture,
            &picture.pixels,
            (picture.width, picture.height, picture.width),
            true,
        )?;
        let (ww, wh) = self.canvas.output_size()?;
        let scale =
            (f64::from(ww) / debugger::WIDTH as f64).min(f64::from(wh) / debugger::HEIGHT as f64);
        let (w, h) = (
            (debugger::WIDTH as f64 * scale) as u32,
            (debugger::HEIGHT as f64 * scale) as u32,
        );
        self.canvas.set_draw_color(sdl2::pixels::Color::BLACK);
        self.canvas.clear();
        self.canvas.copy(
            texture,
            None,
            SdlRect::new(
                ((ww - w.min(ww)) / 2) as i32,
                ((wh - h.min(wh)) / 2) as i32,
                w.max(1),
                h.max(1),
            ),
        )?;
        self.canvas.present();
        Ok(())
    }
}

/// Copy `0xAARRGGBB` pixels into a streaming ARGB8888 texture (opaque
/// pictures have alpha 0 in their top byte, so `force_opaque` sets it).
fn upload(
    texture: &mut Texture<'_>,
    pixels: &[u32],
    (width, height, stride): (usize, usize, usize),
    force_opaque: bool,
) -> Result<(), String> {
    let mask = if force_opaque { 0xFF00_0000 } else { 0 };
    let area = SdlRect::new(0, 0, width as u32, height as u32);
    texture
        .with_lock(area, |buffer, pitch| {
            for (src, dst) in pixels
                .chunks(stride)
                .zip(buffer.chunks_mut(pitch))
                .take(height)
            {
                for (d, &s) in dst.chunks_exact_mut(4).zip(&src[..width]) {
                    d.copy_from_slice(&(s | mask).to_ne_bytes());
                }
            }
        })
        .map_err(|e| e.to_string())
}

/// Pixels per point (see [`Event::Resized`]) from what SDL knows.
///
/// On desktops and iOS, SDL measures windows in *points* and the drawable
/// in pixels, so their ratio is the density: 2 on a Retina Mac, 3 on most
/// iPhones (with `allow_highdpi`; without it iOS would render at the point
/// size and blur the picture up). Android has no points in SDL2: window and
/// drawable are both in pixels, and the density is the screen's DPI
/// relative to Android's 160-DPI baseline, the same number Android's own
/// `DisplayMetrics.density` gives (2.75 on a typical 1080 × 2340 phone).
fn density(drawable_width: u32, window_width: u32, android_dpi: Option<f32>) -> f32 {
    let ratio = drawable_width as f32 / window_width.max(1) as f32;
    match android_dpi {
        Some(dpi) if ratio < 1.01 && dpi > 0.0 => (dpi / ANDROID_BASELINE_DPI).max(1.0),
        _ => ratio,
    }
}

/// The window's drawable size and density, as the app wants them.
fn resized(canvas: &WindowCanvas, android_dpi: Option<f32>) -> Result<Event, String> {
    let (w, h) = canvas.output_size()?;
    let (points, _) = canvas.window().size();
    Ok(Event::Resized {
        width: w,
        height: h,
        pixels_per_point: density(w, points, android_dpi),
    })
}

/// A mouse event; SDL gives window coordinates in points, the app wants
/// pixels (they differ on high-density displays).
fn mouse(canvas: &WindowCanvas, phase: PointerPhase, x: i32, y: i32) -> Event {
    let (pw, ph) = canvas.output_size().unwrap_or((1, 1));
    let (ww, wh) = canvas.window().size();
    Event::Pointer {
        id: 0,
        kind: PointerKind::Mouse,
        phase,
        x: x as f32 * pw as f32 / ww.max(1) as f32,
        y: y as f32 * ph as f32 / wh.max(1) as f32,
    }
}

/// A finger event; SDL gives positions from 0 to 1 across the window.
fn finger(canvas: &WindowCanvas, phase: PointerPhase, id: i64, x: f32, y: f32) -> Option<Event> {
    let (w, h) = canvas.output_size().ok()?;
    Some(Event::Pointer {
        id: id as u64,
        kind: PointerKind::Touch,
        phase,
        x: x * w as f32,
        y: y * h as f32,
    })
}

/// Translate one SDL event. `Err(())` means "quit".
fn translate(
    event: &SdlEvent,
    canvas: &WindowCanvas,
    main_window: u32,
    android_dpi: Option<f32>,
) -> Result<Option<Event>, ()> {
    Ok(match *event {
        SdlEvent::Quit { .. } => return Err(()),
        SdlEvent::KeyDown {
            scancode: Some(code),
            repeat,
            ..
        } => key(code).map(|key| Event::Key {
            key,
            pressed: true,
            repeat,
        }),
        SdlEvent::KeyUp {
            scancode: Some(code),
            ..
        } => key(code).map(|key| Event::Key {
            key,
            pressed: false,
            repeat: false,
        }),
        SdlEvent::ControllerButtonDown { which, button, .. } => {
            pad_button(button).map(|button| Event::PadButton {
                pad: which,
                button,
                pressed: true,
            })
        }
        SdlEvent::ControllerButtonUp { which, button, .. } => {
            pad_button(button).map(|button| Event::PadButton {
                pad: which,
                button,
                pressed: false,
            })
        }
        SdlEvent::ControllerAxisMotion {
            which, axis, value, ..
        } => Some(Event::PadAxis {
            pad: which,
            axis: pad_axis(axis),
            value: f32::from(value) / 32767.0,
        }),
        SdlEvent::MouseButtonDown {
            window_id,
            which,
            mouse_btn: MouseButton::Left,
            x,
            y,
            ..
        } if window_id == main_window && which != TOUCH_MOUSE_ID => {
            Some(mouse(canvas, PointerPhase::Down, x, y))
        }
        SdlEvent::MouseButtonUp {
            window_id,
            which,
            mouse_btn: MouseButton::Left,
            x,
            y,
            ..
        } if window_id == main_window && which != TOUCH_MOUSE_ID => {
            Some(mouse(canvas, PointerPhase::Up, x, y))
        }
        SdlEvent::MouseMotion {
            window_id,
            which,
            x,
            y,
            ..
        } if window_id == main_window && which != TOUCH_MOUSE_ID => {
            Some(mouse(canvas, PointerPhase::Move, x, y))
        }
        SdlEvent::MouseWheel {
            window_id,
            precise_y,
            ..
        } if window_id == main_window => Some(Event::Wheel { lines: precise_y }),
        SdlEvent::FingerDown {
            finger_id, x, y, ..
        } => finger(canvas, PointerPhase::Down, finger_id, x, y),
        SdlEvent::FingerMotion {
            finger_id, x, y, ..
        } => finger(canvas, PointerPhase::Move, finger_id, x, y),
        SdlEvent::FingerUp {
            finger_id, x, y, ..
        } => finger(canvas, PointerPhase::Up, finger_id, x, y),
        SdlEvent::DropFile { ref filename, .. } => Some(Event::OpenRom {
            path: filename.clone(),
        }),
        SdlEvent::Window {
            window_id,
            win_event,
            ..
        } if window_id == main_window => match win_event {
            WindowEvent::Close => return Err(()),
            WindowEvent::SizeChanged(..) | WindowEvent::Resized(..) => {
                resized(canvas, android_dpi).ok()
            }
            WindowEvent::FocusLost => Some(Event::FocusLost),
            _ => None,
        },
        _ => None,
    })
}

/// What an application event asks the shell to do (see "The mobile
/// lifecycle" in the [module documentation](self)).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lifecycle {
    /// Leaving the screen: save, pause, stop drawing.
    Background,
    /// Back on screen: draw and play sound again.
    Foreground,
    /// The system is closing the app.
    Terminate,
    /// The GPU lost its textures (Android may do this in the background).
    DeviceReset,
}

fn lifecycle(event: &SdlEvent) -> Option<Lifecycle> {
    Some(match event {
        // iOS sends "will" when the app stops being active (a call, the
        // control centre), "did" when it is really hidden; either way the
        // user cannot play now.
        SdlEvent::AppWillEnterBackground { .. } | SdlEvent::AppDidEnterBackground { .. } => {
            Lifecycle::Background
        }
        // Only draw again once the app is active ("did"), not while it is
        // still on its way ("will").
        SdlEvent::AppDidEnterForeground { .. } => Lifecycle::Foreground,
        SdlEvent::AppTerminating { .. } => Lifecycle::Terminate,
        SdlEvent::RenderDeviceReset { .. } => Lifecycle::DeviceReset,
        _ => return None,
    })
}

/// Hints for phones, set before SDL starts (SDL reads some at start-up).
fn mobile_hints() {
    // Rotate with the phone: the app has a portrait and a landscape layout.
    // (The hint is named after iOS but Android's SDLActivity reads it too.)
    sdl2::hint::set(
        "SDL_IOS_ORIENTATIONS",
        "LandscapeLeft LandscapeRight Portrait",
    );
    // Android's Back button arrives as a key (SDL_SCANCODE_AC_BACK) instead
    // of closing the app; it becomes Escape, see `key`.
    sdl2::hint::set("SDL_ANDROID_TRAP_BACK_BUTTON", "1");
    // Fingers are fingers: do not also report them as mouse clicks.
    sdl2::hint::set("SDL_TOUCH_MOUSE_EVENTS", "0");
    // Let the iPhone's home bar fade out while playing.
    sdl2::hint::set("SDL_IOS_HIDE_HOME_INDICATOR", "1");
}

/// What the app may offer on this kind of system.
fn capabilities(mobile: bool) -> Capabilities {
    if mobile {
        crate::mobile::phone_capabilities(SAMPLE_RATE)
    } else {
        Capabilities {
            sample_rate: SAMPLE_RATE,
            touch_screen: sdl2::touch::num_touch_devices() > 0,
            file_browser: true,
            rom_picker: false,
            drop_files: true,
            can_quit: true,
            fullscreen: true,
            debugger: true,
            keyboard: true,
        }
    }
}

/// Open the sound device: 48 kHz stereo, `samples` frames per buffer
/// (smaller is less latency, larger is safer against crackling).
fn open_audio(sdl: &sdl2::Sdl, samples: u16) -> Result<AudioQueue<i16>, String> {
    let spec = AudioSpecDesired {
        freq: Some(SAMPLE_RATE as i32),
        channels: Some(2),
        samples: Some(samples),
    };
    let queue = sdl.audio()?.open_queue::<i16, _>(None, &spec)?;
    queue.resume();
    Ok(queue)
}

fn game_texture(creator: &TextureCreator<WindowContext>) -> Result<Texture<'_>, String> {
    creator
        .create_texture_streaming(
            PixelFormatEnum::ARGB8888,
            gase_core::MAX_WIDTH as u32,
            gase_core::MAX_HEIGHT as u32,
        )
        .map_err(|e| e.to_string())
}

/// Draw the game and the overlay (step 4 of the main loop).
fn draw<'a>(
    app: &mut App,
    canvas: &mut WindowCanvas,
    creator: &'a TextureCreator<WindowContext>,
    game_texture: &mut Texture<'a>,
    overlay_texture: &mut Option<(Texture<'a>, (usize, usize))>,
) -> Result<(), String> {
    let video = app.video();
    let bg = video.background;
    canvas.set_draw_color(sdl2::pixels::Color::RGB(
        (bg >> 16) as u8,
        (bg >> 8) as u8,
        bg as u8,
    ));
    canvas.clear();
    if let Some(frame) = video.game {
        upload(
            game_texture,
            frame.pixels,
            (frame.width, frame.height, frame.stride),
            true,
        )?;
        let r = video.game_rect;
        canvas.copy(
            game_texture,
            SdlRect::new(0, 0, frame.width as u32, frame.height as u32),
            SdlRect::new(r.x, r.y, r.w.max(1) as u32, r.h.max(1) as u32),
        )?;
    }
    if let Some(overlay) = video.overlay {
        let size = (overlay.width, overlay.height);
        let stale = overlay_texture.as_ref().is_none_or(|(_, s)| *s != size);
        if stale {
            let mut t = creator
                .create_texture_streaming(PixelFormatEnum::ARGB8888, size.0 as u32, size.1 as u32)
                .map_err(|e| e.to_string())?;
            t.set_blend_mode(BlendMode::Blend);
            *overlay_texture = Some((t, size));
        }
        if let Some((texture, _)) = overlay_texture {
            // Upload only when the app redrew it: usually never while
            // playing, once per frame while a menu is open.
            if stale || video.overlay_changed {
                upload(texture, &overlay.pixels, (size.0, size.1, size.0), false)?;
            }
            let s = video.overlay_scale as u32;
            canvas.copy(
                texture,
                None,
                SdlRect::new(0, 0, size.0 as u32 * s, size.1 as u32 * s),
            )?;
        }
    }
    canvas.present();
    Ok(())
}

/// Run the app in an SDL window until the user (or, on a phone, the
/// system) closes it. See the [module documentation](self).
///
/// # Errors
///
/// A message if SDL cannot start (no display, no renderer), or, on the
/// desktop, if the ROM named on the command line cannot be opened.
pub fn run_sdl(shell: Shell<'_>) -> Result<(), String> {
    let (options, mobile) = match shell {
        Shell::Desktop(options) => (Some(options), None),
        Shell::Mobile(mobile) => (None, Some(mobile)),
    };
    let is_mobile = mobile.is_some();
    if is_mobile {
        mobile_hints();
    }
    let sdl = sdl2::init()?;
    let video = sdl.video()?;
    let controllers = sdl.game_controller()?;

    let (files, audio): (Box<dyn Platform>, _) = match options {
        Some(options) => {
            let audio = if options.audio {
                Some(open_audio(&sdl, 512)?)
            } else {
                None
            };
            (Box::new(Desktop::new()), audio)
        }
        None => {
            // The app's private folder (see `crate::mobile`).
            let data = sdl2::filesystem::pref_path("", "gase").map_err(|e| e.to_string())?;
            // A phone without sound still plays; a bigger buffer rides out
            // the hiccups of a phone's power management.
            let audio = open_audio(&sdl, 1024)
                .inspect_err(|e| sdl2::log::log(&format!("no sound: {e}")))
                .ok();
            (Box::new(MobileFiles::for_this_system(data.into())), audio)
        }
    };
    let mut platform = SdlPlatform {
        files,
        audio,
        requests: Vec::new(),
        sdl_log: is_mobile,
    };
    let mut app = App::new(&mut platform, capabilities(is_mobile));
    if let Some(options) = options {
        crate::cli::apply_overrides(&mut app, options);
    }

    let test_window = mobile.as_ref().and_then(|m| m.window);
    let window = if let Some((w, h)) = test_window {
        video
            .window("gase (mobile)", w, h)
            .position_centered()
            .resizable()
            .allow_highdpi()
            .build()
    } else if is_mobile {
        // A phone's window is the whole screen whatever we ask; "fullscreen"
        // also hides the status and navigation bars (Android's immersive
        // mode). Resizable lets it rotate.
        let mode = video.current_display_mode(0)?;
        video
            .window("gase", mode.w.max(1) as u32, mode.h.max(1) as u32)
            .fullscreen_desktop()
            .resizable()
            .allow_highdpi()
            .build()
    } else {
        let scale = app.settings().video.scale;
        video
            .window("gase", 320 * scale, 224 * scale)
            .position_centered()
            .resizable()
            .allow_highdpi()
            .build()
    }
    .map_err(|e| e.to_string())?;
    let mut canvas = window.into_canvas().build().map_err(|e| e.to_string())?;
    if !is_mobile && app.settings().video.fullscreen {
        canvas
            .window_mut()
            .set_fullscreen(FullscreenType::Desktop)?;
    }
    // Android's density, see `density`.
    let android_dpi = if is_mobile && cfg!(target_os = "android") {
        video.display_dpi(0).ok().map(|(ddpi, _, _)| ddpi)
    } else {
        None
    };
    let creator = canvas.texture_creator();
    let mut game_texture = game_texture(&creator)?;
    // Created at the overlay's size, and again when that changes.
    let mut overlay_texture: Option<(Texture<'_>, (usize, usize))> = None;

    // The debugger window (desktop only) is created hidden and shown with F1.
    let debug_canvas = match options {
        Some(_) => Some(
            video
                .window(
                    "gase debugger",
                    debugger::WIDTH as u32,
                    debugger::HEIGHT as u32,
                )
                .position_centered()
                .resizable()
                .allow_highdpi()
                .hidden()
                .build()
                .map_err(|e| e.to_string())?
                .into_canvas()
                .build()
                .map_err(|e| e.to_string())?,
        ),
        None => None,
    };
    let debug_creator = debug_canvas.as_ref().map(WindowCanvas::texture_creator);
    let mut debug_texture = match &debug_creator {
        Some(creator) => Some(
            creator
                .create_texture_streaming(
                    PixelFormatEnum::ARGB8888,
                    debugger::WIDTH as u32,
                    debugger::HEIGHT as u32,
                )
                .map_err(|e| e.to_string())?,
        ),
        None => None,
    };
    let debug_window_id = debug_canvas.as_ref().map(|c| c.window().id());
    let main_window_id = canvas.window().id();
    let mut dbg = debug_canvas.map(|canvas| DebugWindow {
        canvas,
        panel: Panel::new(),
        open: false,
        shown: false,
    });

    app.handle(&mut platform, resized(&canvas, android_dpi)?);
    if let Some((options, rom)) = options.and_then(|o| Some((o, o.rom.as_ref()?))) {
        // A ROM named on the command line that does not load is an error,
        // as it always was: the user is looking at a terminal.
        app.open_rom(&mut platform, &rom.to_string_lossy())?;
        if let Some(game) = app.game_mut() {
            game.genesis.set_trace(options.trace);
            for &addr in &options.breakpoints {
                game.debugger.add_breakpoint(addr);
            }
            if options.debug {
                game.paused = true;
                if let Some(dbg) = &mut dbg {
                    dbg.open = true;
                }
            }
        }
    }
    if let Some(path) = mobile.as_ref().and_then(|m| m.open.clone()) {
        // Opened with a ROM: errors are shown in the app, not fatal.
        app.handle(&mut platform, Event::OpenRom { path });
    }
    let hooks = mobile.unwrap_or_default();

    let mut events = sdl.event_pump()?;
    let mut pads: Vec<GameController> = Vec::new();
    let mut next_deadline = Instant::now();
    // In the background (phones): not drawing, not emulating, sleeping.
    let mut background = false;

    'main: loop {
        // --- 1. Events in ----------------------------------------------------------
        // In the background, sleep until something happens instead of
        // spinning (on Android SDL even blocks in there until we return).
        let waited = if background {
            events.wait_event_timeout(250)
        } else {
            None
        };
        for event in waited.into_iter().chain(events.poll_iter()) {
            if let Some(change) = lifecycle(&event) {
                match change {
                    Lifecycle::Background if !background => {
                        background = true;
                        // Writes the save and settings and opens the pause menu.
                        app.handle(&mut platform, Event::Suspend);
                        platform.pause_audio();
                        platform.log("gase: in the background, saves written");
                    }
                    Lifecycle::Foreground if background => {
                        background = false;
                        platform.resume_audio();
                        // The phone may have been turned while we were away.
                        if let Ok(size) = resized(&canvas, android_dpi) {
                            app.handle(&mut platform, size);
                        }
                        next_deadline = Instant::now();
                        platform.log("gase: back in the foreground");
                    }
                    Lifecycle::Terminate => break 'main,
                    Lifecycle::DeviceReset => {
                        game_texture = self::game_texture(&creator)?;
                        overlay_texture = None;
                    }
                    Lifecycle::Background | Lifecycle::Foreground => {}
                }
                continue;
            }
            match event {
                // Keys typed into the debugger window are its own.
                SdlEvent::KeyDown {
                    scancode: Some(code),
                    keycode,
                    window_id,
                    ..
                } if Some(window_id) == debug_window_id && dbg.as_ref().is_some_and(|d| d.open) => {
                    if let Some(dbg) = &mut dbg {
                        if matches!(code, Scancode::F1 | Scancode::Grave) {
                            dbg.open = false;
                        } else if let Some(k) = debugger_key(code, keycode) {
                            // Key repeat is welcome here: hold S to keep stepping.
                            dbg.key(&mut app, k);
                        }
                    }
                }
                SdlEvent::Window {
                    window_id,
                    win_event: WindowEvent::Close,
                    ..
                } if Some(window_id) == debug_window_id => {
                    if let Some(dbg) = &mut dbg {
                        dbg.open = false;
                    }
                }
                // Back on the home screen leaves the app, as on any Android
                // app; everywhere else it is Escape (menus, back).
                SdlEvent::KeyDown {
                    scancode: Some(Scancode::AcBack),
                    repeat: false,
                    ..
                } if app.screen_name() == "home" && hooks.back_at_home.is_some() => {
                    if let Some(back) = hooks.back_at_home {
                        back();
                    }
                }
                // Pads must be opened (and kept) to receive their events.
                // SDL reports the pads present at start this way too.
                SdlEvent::ControllerDeviceAdded { which, .. } => match controllers.open(which) {
                    Ok(controller) => {
                        let event = Event::PadConnected {
                            pad: controller.instance_id(),
                            name: controller.name(),
                        };
                        pads.push(controller);
                        app.handle(&mut platform, event);
                    }
                    Err(e) => platform.log(&format!("Cannot open controller: {e}")),
                },
                SdlEvent::ControllerDeviceRemoved { which, .. } => {
                    pads.retain(|p| p.instance_id() != which);
                    app.handle(&mut platform, Event::PadDisconnected { pad: which });
                }
                other => match translate(&other, &canvas, main_window_id, android_dpi) {
                    Ok(Some(event)) => app.handle(&mut platform, event),
                    Ok(None) => {}
                    Err(()) => break 'main,
                },
            }
        }
        if background {
            continue;
        }

        // --- 2. Update ----------------------------------------------------------------
        let pacing = app.update(&mut platform);
        if let Some(game) = app.game_mut() {
            for line in game.genesis.take_trace() {
                println!("{line}");
            }
        }

        // --- 3. Requests -----------------------------------------------------------------
        for request in std::mem::take(&mut platform.requests) {
            match request {
                Request::SetTitle(title) => canvas
                    .window_mut()
                    .set_title(&title)
                    .map_err(|e| e.to_string())?,
                Request::SetFullscreen(on) => {
                    let mode = if on {
                        FullscreenType::Desktop
                    } else {
                        FullscreenType::Off
                    };
                    if let Err(e) = canvas.window_mut().set_fullscreen(mode) {
                        app.notify(format!("Fullscreen failed: {e}"));
                    }
                }
                Request::Quit => break 'main,
                Request::ToggleDebugger => {
                    if let Some(dbg) = &mut dbg {
                        dbg.open = !dbg.open;
                        dbg.panel.follow_pc();
                    }
                }
                Request::Breakpoint(pc) => {
                    if let Some(dbg) = &mut dbg {
                        dbg.open = true;
                        dbg.panel.follow_pc();
                        dbg.panel.set_status(format!("Breakpoint at ${pc:06X}"));
                    }
                }
                Request::PickRom => {
                    if let Some(pick) = hooks.pick_rom {
                        pick();
                    }
                }
            }
        }

        // --- 4. Video out -------------------------------------------------------------------
        draw(
            &mut app,
            &mut canvas,
            &creator,
            &mut game_texture,
            &mut overlay_texture,
        )?;
        if let (Some(dbg), Some(texture)) = (&mut dbg, &mut debug_texture) {
            dbg.draw(&app, texture)?;
        }
        if app.wants_quit() {
            break;
        }

        // --- 5. Wait ----------------------------------------------------------------------------
        match pacing {
            Pacing::Audio { target } => {
                // Never longer than a few frames: a sound device that stops
                // playing (unplugged headphones) must not freeze the app.
                let give_up = Instant::now() + Duration::from_millis(100);
                while platform.audio_queued().is_some_and(|q| q > target)
                    && Instant::now() < give_up
                {
                    std::thread::sleep(Duration::from_millis(1));
                }
                next_deadline = Instant::now();
            }
            Pacing::Timer { fps } => {
                next_deadline += Duration::from_secs_f64(1.0 / fps);
                let now = Instant::now();
                if next_deadline > now {
                    std::thread::sleep(next_deadline - now);
                } else {
                    next_deadline = now;
                }
            }
            Pacing::Unthrottled => next_deadline = Instant::now(),
        }
    }
    app.shutdown(&mut platform);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn density_from_points_or_android_dpi() {
        // A classic desktop and a Retina one.
        assert!((density(960, 960, None) - 1.0).abs() < 1e-6);
        assert!((density(1920, 960, None) - 2.0).abs() < 1e-6);
        // An iPhone: pixels over points, whatever the DPI says.
        assert!((density(1179, 393, Some(460.0)) - 3.0).abs() < 1e-6);
        // Android: no points, the DPI decides (440 dpi → 2.75).
        assert!((density(1080, 1080, Some(440.0)) - 2.75).abs() < 1e-6);
        // Nonsense DPI or a low-density screen: at least 1.
        assert!((density(800, 800, Some(0.0)) - 1.0).abs() < 1e-6);
        assert!((density(800, 800, Some(120.0)) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn lifecycle_events() {
        let t = 0;
        assert_eq!(
            lifecycle(&SdlEvent::AppWillEnterBackground { timestamp: t }),
            Some(Lifecycle::Background)
        );
        assert_eq!(
            lifecycle(&SdlEvent::AppDidEnterBackground { timestamp: t }),
            Some(Lifecycle::Background)
        );
        assert_eq!(
            lifecycle(&SdlEvent::AppWillEnterForeground { timestamp: t }),
            None
        );
        assert_eq!(
            lifecycle(&SdlEvent::AppDidEnterForeground { timestamp: t }),
            Some(Lifecycle::Foreground)
        );
        assert_eq!(
            lifecycle(&SdlEvent::AppTerminating { timestamp: t }),
            Some(Lifecycle::Terminate)
        );
        assert_eq!(
            lifecycle(&SdlEvent::RenderDeviceReset { timestamp: t }),
            Some(Lifecycle::DeviceReset)
        );
        assert_eq!(lifecycle(&SdlEvent::Quit { timestamp: t }), None);
    }

    #[test]
    fn android_back_is_escape() {
        assert_eq!(key(Scancode::AcBack), Some(Key::Escape));
        assert_eq!(key(Scancode::Escape), Some(Key::Escape));
    }

    #[test]
    fn phones_offer_the_picker_not_the_browser() {
        let caps = capabilities(true);
        assert!(caps.touch_screen && caps.rom_picker);
        assert!(!caps.file_browser && !caps.can_quit && !caps.debugger);
    }
}
