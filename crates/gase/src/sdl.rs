//! The desktop shell, built on SDL2: a window, sound, and input devices
//! around [`gase_app::App`].
//!
//! Everything the user sees and touches is decided by the app; this file
//! only translates. Each pass of the main loop:
//!
//! 1. **Events in.** SDL's events become [`gase_app::Event`]s: scancodes
//!    become [`Key`]s, SDL GameController buttons (already in the standard
//!    layout, for any pad SDL knows) become [`PadButton`]s, mouse and
//!    fingers become pointers, dropped files become ROMs to open.
//! 2. **Update.** [`App::update`] runs the menus or the emulator and
//!    queues the sound ([`SdlPlatform::queue_audio`]).
//! 3. **Requests.** What the app asked for (window title, fullscreen,
//!    quit, the debugger) is carried out.
//! 4. **Video out.** The game frame and, when there is one, the overlay are
//!    uploaded to two streaming textures and drawn: the game stretched into
//!    its rectangle, the overlay enlarged by a whole factor and blended on
//!    top (GPU work, so menus cost almost nothing).
//! 5. **Wait** as the app's [`Pacing`] says.
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
//! ([`crate::debugger::Panel`]). While it has the keyboard focus, keys go to
//! the debugger instead of the game. The debugger is a desktop feature, so
//! it lives here and works on the app's [`gase_app::Game`].

use std::time::{Duration, Instant};

use gase_app::{
    App, Capabilities, DebugRun, Event, Key, Pacing, PadAxis, PadButton, Platform, PointerKind,
    PointerPhase, Request,
};
use gase_core::Stop;
use sdl2::audio::{AudioQueue, AudioSpecDesired};
use sdl2::controller::{Axis, Button, GameController};
use sdl2::event::{Event as SdlEvent, WindowEvent};
use sdl2::keyboard::{Keycode, Scancode};
use sdl2::mouse::MouseButton;
use sdl2::pixels::PixelFormatEnum;
use sdl2::rect::Rect as SdlRect;
use sdl2::render::{BlendMode, Texture, WindowCanvas};
use sdl2::video::FullscreenType;

use crate::cli::Options;
use crate::debugger::{self, Action, Panel};
use crate::desktop::Desktop;

const SAMPLE_RATE: u32 = 48_000;
/// SDL reports mouse events it synthesises from touches with this id; the
/// touches themselves arrive as finger events.
const TOUCH_MOUSE_ID: u32 = u32::MAX;

/// The desktop platform: files from [`Desktop`], sound from SDL, and the
/// requests the main loop carries out after each update.
struct SdlPlatform {
    desktop: Desktop,
    audio: Option<AudioQueue<i16>>,
    requests: Vec<Request>,
}

impl Platform for SdlPlatform {
    fn now_ms(&self) -> u64 {
        self.desktop.now_ms()
    }
    fn log(&mut self, message: &str) {
        self.desktop.log(message);
    }
    fn load(&mut self, file: gase_app::FileKey<'_>) -> Option<Vec<u8>> {
        self.desktop.load(file)
    }
    fn store(&mut self, file: gase_app::FileKey<'_>, data: &[u8]) -> Result<String, String> {
        self.desktop.store(file, data)
    }
    fn read_rom(&mut self, path: &str) -> Result<Vec<u8>, String> {
        self.desktop.read_rom(path)
    }
    fn list_dir(&mut self, dir: Option<&str>) -> Result<gase_app::Listing, String> {
        self.desktop.list_dir(dir)
    }
    fn audio_queued(&self) -> Option<usize> {
        // The queue size is in bytes: 4 per stereo frame of i16.
        self.audio.as_ref().map(|q| q.size() as usize / 4)
    }
    fn queue_audio(&mut self, samples: &[i16]) {
        if let Some(queue) = &self.audio {
            if let Err(e) = queue.queue_audio(samples) {
                eprintln!("audio: {e}");
            }
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
        S::Escape => Key::Escape,
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

/// Copy `0xAARRGGBB` pixels into the top-left corner of a streaming
/// ARGB8888 texture (opaque pictures have alpha 0 in their top byte, so
/// `force_opaque` sets it).
///
/// The whole texture is locked (`None`) even when only part of it is
/// written: `sdl2` 0.37's `Texture::with_lock` keeps a pointer to a
/// temporary `Rect` after the temporary is gone, so passing a rectangle
/// makes SDL read a dead stack slot. In optimised builds that slot is
/// reused, SDL computes a bogus pixel address and the copy below writes
/// into freed memory. Locking everything costs nothing extra: SDL hands
/// back the same buffer either way.
fn upload(
    texture: &mut Texture<'_>,
    pixels: &[u32],
    (width, height, stride): (usize, usize, usize),
    force_opaque: bool,
) -> Result<(), String> {
    let mask = if force_opaque { 0xFF00_0000 } else { 0 };
    texture
        .with_lock(None, |buffer, pitch| {
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

/// The window's drawable size and density, as the app wants them.
fn resized(canvas: &WindowCanvas) -> Result<Event, String> {
    let (w, h) = canvas.output_size()?;
    let (points, _) = canvas.window().size();
    Ok(Event::Resized {
        width: w,
        height: h,
        pixels_per_point: w as f32 / points.max(1) as f32,
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
        SdlEvent::AppWillEnterBackground { .. } => Some(Event::Suspend),
        SdlEvent::Window {
            window_id,
            win_event,
            ..
        } if window_id == main_window => match win_event {
            WindowEvent::Close => return Err(()),
            WindowEvent::SizeChanged(..) | WindowEvent::Resized(..) => resized(canvas).ok(),
            WindowEvent::FocusLost => Some(Event::FocusLost),
            _ => None,
        },
        _ => None,
    })
}

pub fn run(options: &Options) -> Result<(), String> {
    let sdl = sdl2::init()?;
    let video = sdl.video()?;
    let controllers = sdl.game_controller()?;

    let audio = if options.audio {
        let spec = AudioSpecDesired {
            freq: Some(SAMPLE_RATE as i32),
            channels: Some(2),
            samples: Some(512),
        };
        let queue = sdl.audio()?.open_queue::<i16, _>(None, &spec)?;
        queue.resume();
        Some(queue)
    } else {
        None
    };
    let mut platform = SdlPlatform {
        desktop: Desktop::new(),
        audio,
        requests: Vec::new(),
    };
    let caps = Capabilities {
        sample_rate: SAMPLE_RATE,
        touch_screen: sdl2::touch::num_touch_devices() > 0,
        file_browser: true,
        rom_picker: false,
        drop_files: true,
        can_quit: true,
        fullscreen: true,
        debugger: true,
        keyboard: true,
    };
    let mut app = App::new(&mut platform, caps);
    crate::apply_overrides(&mut app, options);

    let scale = app.settings().video.scale;
    let window = video
        .window("gase", 320 * scale, 224 * scale)
        .position_centered()
        .resizable()
        .allow_highdpi()
        .build()
        .map_err(|e| e.to_string())?;
    let mut canvas = window.into_canvas().build().map_err(|e| e.to_string())?;
    if app.settings().video.fullscreen {
        canvas
            .window_mut()
            .set_fullscreen(FullscreenType::Desktop)?;
    }
    let creator = canvas.texture_creator();
    let mut game_texture = creator
        .create_texture_streaming(
            PixelFormatEnum::ARGB8888,
            gase_core::MAX_WIDTH as u32,
            gase_core::MAX_HEIGHT as u32,
        )
        .map_err(|e| e.to_string())?;
    // Created at the overlay's size, and again when that changes.
    let mut overlay_texture: Option<(Texture<'_>, (usize, usize))> = None;

    // The debugger window is created hidden and shown with F1.
    let debug_canvas = video
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
        .map_err(|e| e.to_string())?;
    let debug_creator = debug_canvas.texture_creator();
    let mut debug_texture = debug_creator
        .create_texture_streaming(
            PixelFormatEnum::ARGB8888,
            debugger::WIDTH as u32,
            debugger::HEIGHT as u32,
        )
        .map_err(|e| e.to_string())?;
    let debug_window_id = debug_canvas.window().id();
    let main_window_id = canvas.window().id();
    let mut dbg = DebugWindow {
        canvas: debug_canvas,
        panel: Panel::new(),
        open: false,
        shown: false,
    };

    app.handle(&mut platform, resized(&canvas)?);
    if let Some(rom) = &options.rom {
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
                dbg.open = true;
            }
        }
    }

    let mut events = sdl.event_pump()?;
    let mut pads: Vec<GameController> = Vec::new();
    let mut next_deadline = Instant::now();

    'main: loop {
        // --- 1. Events in ----------------------------------------------------------
        for event in events.poll_iter() {
            match event {
                // Keys typed into the debugger window are its own.
                SdlEvent::KeyDown {
                    scancode: Some(code),
                    keycode,
                    window_id,
                    ..
                } if dbg.open && window_id == debug_window_id => {
                    if matches!(code, Scancode::F1 | Scancode::Grave) {
                        dbg.open = false;
                    } else if let Some(k) = debugger_key(code, keycode) {
                        // Key repeat is welcome here: hold S to keep stepping.
                        dbg.key(&mut app, k);
                    }
                }
                SdlEvent::Window {
                    window_id,
                    win_event: WindowEvent::Close,
                    ..
                } if window_id == debug_window_id => dbg.open = false,
                // Pads must be opened (and kept) to receive their events.
                SdlEvent::ControllerDeviceAdded { which, .. } => match controllers.open(which) {
                    Ok(controller) => {
                        let event = Event::PadConnected {
                            pad: controller.instance_id(),
                            name: controller.name(),
                        };
                        pads.push(controller);
                        app.handle(&mut platform, event);
                    }
                    Err(e) => eprintln!("Cannot open controller: {e}"),
                },
                SdlEvent::ControllerDeviceRemoved { which, .. } => {
                    pads.retain(|p| p.instance_id() != which);
                    app.handle(&mut platform, Event::PadDisconnected { pad: which });
                }
                other => match translate(&other, &canvas, main_window_id) {
                    Ok(Some(event)) => app.handle(&mut platform, event),
                    Ok(None) => {}
                    Err(()) => break 'main,
                },
            }
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
                    dbg.open = !dbg.open;
                    dbg.panel.follow_pc();
                }
                Request::Breakpoint(pc) => {
                    dbg.open = true;
                    dbg.panel.follow_pc();
                    dbg.panel.set_status(format!("Breakpoint at ${pc:06X}"));
                }
                Request::PickRom => {}
            }
        }

        // --- 4. Video out -------------------------------------------------------------------
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
                &mut game_texture,
                frame.pixels,
                (frame.width, frame.height, frame.stride),
                true,
            )?;
            let r = video.game_rect;
            canvas.copy(
                &game_texture,
                SdlRect::new(0, 0, frame.width as u32, frame.height as u32),
                SdlRect::new(r.x, r.y, r.w.max(1) as u32, r.h.max(1) as u32),
            )?;
        }
        if let Some(overlay) = video.overlay {
            let size = (overlay.width, overlay.height);
            let stale = overlay_texture.as_ref().is_none_or(|(_, s)| *s != size);
            if stale {
                let mut t = creator
                    .create_texture_streaming(
                        PixelFormatEnum::ARGB8888,
                        size.0 as u32,
                        size.1 as u32,
                    )
                    .map_err(|e| e.to_string())?;
                t.set_blend_mode(BlendMode::Blend);
                overlay_texture = Some((t, size));
            }
            if let Some((texture, _)) = &mut overlay_texture {
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
        dbg.draw(&app, &mut debug_texture)?;
        if app.wants_quit() {
            break;
        }

        // --- 5. Wait ----------------------------------------------------------------------------
        match pacing {
            Pacing::Audio { target } => {
                while platform.audio_queued().is_some_and(|q| q > target) {
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
