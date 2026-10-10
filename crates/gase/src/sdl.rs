//! The windowed frontend, built on SDL2 (window, audio, keyboard, gamepads).
//!
//! # Audio/video synchronisation
//!
//! The console produces ≈59.92 frames per second; the host display usually
//! refreshes at 60 Hz, and the sound card consumes samples at its own pace.
//! We let **audio drive timing**: after each frame we wait until the sound
//! queue has drained to a target level, then emulate the next frame. To keep
//! the queue from slowly over- or under-flowing, the resampler's ratio is
//! nudged by up to ±0.5% depending on how full the queue is (dynamic rate
//! control) — far too little to hear, but enough to absorb clock drift.
//!
//! # The debugger window
//!
//! F1 (or the backtick key) opens a second window with the debugger
//! ([`crate::debugger::Panel`]). While it has the keyboard focus, keys go to
//! the debugger instead of the game. When breakpoints are set, or the
//! debugger has stopped the console in the middle of a frame, frames run
//! through [`gase_core::Debugger`] instead of [`gase_core::Genesis::run_frame`].

use std::time::{Duration, Instant};

use gase_core::{Buttons, Debugger, Rewind, Stop};
use sdl2::audio::{AudioQueue, AudioSpecDesired};
use sdl2::controller::{Axis, Button, GameController};
use sdl2::event::{Event, WindowEvent};
use sdl2::keyboard::{Keycode, Scancode};
use sdl2::pixels::PixelFormatEnum;
use sdl2::rect::Rect;
use sdl2::video::FullscreenType;

use crate::cli::Options;
use crate::debugger::{self, Action, Key, Panel};
use crate::session::Session;

const SAMPLE_RATE: u32 = 48_000;
/// Audio queue level to aim for: 50 ms.
const TARGET_QUEUE: u32 = SAMPLE_RATE / 20;
/// Frames run per displayed frame while fast-forwarding.
const FAST_FORWARD_FRAMES: u32 = 4;
/// How long the window title shows a status message.
const MESSAGE_TIME: Duration = Duration::from_secs(2);

fn keyboard_button(code: Scancode) -> Option<Buttons> {
    Some(match code {
        Scancode::Up => Buttons::UP,
        Scancode::Down => Buttons::DOWN,
        Scancode::Left => Buttons::LEFT,
        Scancode::Right => Buttons::RIGHT,
        Scancode::Z => Buttons::A,
        Scancode::X => Buttons::B,
        Scancode::C => Buttons::C,
        Scancode::A => Buttons::X,
        Scancode::S => Buttons::Y,
        Scancode::D => Buttons::Z,
        Scancode::Return => Buttons::START,
        Scancode::Q => Buttons::MODE,
        _ => return None,
    })
}

/// Pad buttons follow the physical layout: the bottom row of a modern pad
/// (X, A, B on an Xbox layout) is the Mega Drive's A, B, C.
fn pad_button(button: Button) -> Option<Buttons> {
    Some(match button {
        Button::DPadUp => Buttons::UP,
        Button::DPadDown => Buttons::DOWN,
        Button::DPadLeft => Buttons::LEFT,
        Button::DPadRight => Buttons::RIGHT,
        Button::X => Buttons::A,
        Button::A => Buttons::B,
        Button::B => Buttons::C,
        Button::LeftShoulder => Buttons::X,
        Button::Y => Buttons::Y,
        Button::RightShoulder => Buttons::Z,
        Button::Start => Buttons::START,
        Button::Back => Buttons::MODE,
        _ => return None,
    })
}

/// A connected game controller and the player it controls.
struct Pad {
    controller: GameController,
    buttons: Buttons,
    stick: Buttons,
}

/// Frontend state that is not part of the emulated console.
struct Frontend {
    session: Session,
    keyboard: Buttons,
    pads: Vec<Pad>,
    paused: bool,
    step_frame: bool,
    fast_forward: bool,
    rewinding: bool,
    muted: bool,
    slot: u8,
    rewind: Rewind,
    message: Option<(String, Instant)>,
    debug: Debugger,
    panel: Panel,
    /// The debugger window is shown.
    debug_open: bool,
    /// A debugger command to run while paused.
    debug_command: Option<Action>,
}

/// Translate a key for the debugger panel.
fn debugger_key(code: Scancode, keycode: Option<Keycode>) -> Option<Key> {
    Some(match code {
        Scancode::Up => Key::Up,
        Scancode::Down => Key::Down,
        Scancode::PageDown => Key::PageDown,
        Scancode::Home => Key::Home,
        Scancode::Return | Scancode::KpEnter => Key::Enter,
        Scancode::Backspace => Key::Backspace,
        Scancode::Escape => Key::Escape,
        _ => {
            // Printable keys: SDL key codes are their (lower-case) characters,
            // so this follows the keyboard layout.
            let c = char::from_u32(keycode?.into_i32() as u32)?;
            if !(' '..='~').contains(&c) {
                return None;
            }
            Key::Char(c.to_ascii_lowercase())
        }
    })
}

impl Frontend {
    fn notify(&mut self, text: impl Into<String>) {
        let text = text.into();
        eprintln!("{text}");
        self.message = Some((text, Instant::now()));
    }

    fn update_inputs(&mut self) {
        let mut players = [self.keyboard, Buttons::default()];
        for (i, pad) in self.pads.iter().enumerate().take(2) {
            players[i] = players[i] | pad.buttons | pad.stick;
        }
        for (port, buttons) in players.into_iter().enumerate() {
            self.session.genesis.set_buttons(port, buttons);
        }
    }

    fn toggle_debugger(&mut self) {
        self.debug_open = !self.debug_open;
        if self.debug_open {
            self.panel.follow_pc();
        }
    }

    /// Run a debugger command (the console is paused).
    fn run_debug_command(&mut self, action: Action) {
        let genesis = &mut self.session.genesis;
        let stop = match action {
            Action::Step => self.debug.step_instruction(genesis),
            Action::StepFrame => self.debug.run_frame(genesis),
            Action::RunToVBlank => self.debug.run_to_vblank(genesis),
            _ => return,
        };
        self.session.print_trace();
        if stop == Stop::FrameEnd {
            self.rewind.record(&self.session.genesis);
        }
        self.panel.set_status(match stop {
            Stop::Stepped => "Stepped one instruction".to_string(),
            Stop::FrameEnd => format!("Frame {} done", self.session.genesis.frame_count()),
            Stop::Breakpoint(pc) => format!("Breakpoint at ${pc:06X}"),
            Stop::VBlank => "Vertical interrupt raised: S steps into the handler".to_string(),
        });
        self.panel.follow_pc();
    }

    /// Handle a key pressed in the debugger window.
    fn debugger_key(&mut self, key: Key) {
        match self
            .panel
            .key(key, &mut self.session.genesis, &mut self.debug)
        {
            Action::None => {}
            Action::Close => self.debug_open = false,
            Action::TogglePause => self.toggle_pause(),
            command => {
                self.paused = true;
                self.debug_command = Some(command);
            }
        }
    }

    fn toggle_pause(&mut self) {
        self.paused = !self.paused;
        self.notify(if self.paused { "Paused" } else { "Resumed" });
    }

    fn pad_index(&self, instance: u32) -> Option<usize> {
        self.pads
            .iter()
            .position(|p| p.controller.instance_id() == instance)
    }

    /// Handle a hotkey; returns false to quit.
    fn hotkey(&mut self, code: Scancode, canvas: &mut sdl2::render::WindowCanvas) -> bool {
        match code {
            Scancode::Escape => return false,
            Scancode::P => self.toggle_pause(),
            Scancode::F1 | Scancode::Grave => self.toggle_debugger(),
            Scancode::N if self.paused => self.step_frame = true,
            Scancode::M => {
                self.muted = !self.muted;
                self.notify(if self.muted { "Sound off" } else { "Sound on" });
            }
            Scancode::F5 if self.debug.in_frame() => {
                self.notify("Stopped mid-frame: finish the frame first (F in the debugger)");
            }
            Scancode::F5 => match self.session.save_state(self.slot) {
                Ok(_) => self.notify(format!("Saved state to slot {}", self.slot)),
                Err(e) => self.notify(e),
            },
            Scancode::F8 => match self.session.load_state(self.slot) {
                Ok(_) => {
                    self.rewind.clear();
                    self.debug.forget_position();
                    self.notify(format!("Loaded state from slot {}", self.slot));
                }
                Err(e) => self.notify(e),
            },
            Scancode::F6 => {
                self.slot = (self.slot + 9) % 10;
                self.notify(format!("Slot {}", self.slot));
            }
            Scancode::F7 => {
                self.slot = (self.slot + 1) % 10;
                self.notify(format!("Slot {}", self.slot));
            }
            Scancode::F9 => {
                self.session.genesis.reset();
                self.notify("Reset");
            }
            Scancode::F11 => {
                let window = canvas.window_mut();
                let next = if window.fullscreen_state() == FullscreenType::Off {
                    FullscreenType::Desktop
                } else {
                    FullscreenType::Off
                };
                if let Err(e) = window.set_fullscreen(next) {
                    self.notify(format!("Fullscreen failed: {e}"));
                }
            }
            Scancode::F12 => match self.session.screenshot(None) {
                Ok(path) => self.notify(format!("Saved {}", path.display())),
                Err(e) => self.notify(e),
            },
            _ => {}
        }
        true
    }
}

/// Where to draw the picture inside a window of `window` size.
///
/// The console's pixels are not square: the picture always fills a
/// 320×224 (10:7) area whether the game uses 256 or 320 pixels per line, as
/// on a television.
fn display_rect(window: (u32, u32), lines: u32, integer: bool) -> Rect {
    let (ww, wh) = window;
    let (base_w, base_h) = (320.0, f64::from(lines));
    let mut scale = (f64::from(ww) / base_w).min(f64::from(wh) / base_h);
    if integer && scale >= 1.0 {
        scale = scale.floor();
    }
    let (w, h) = ((base_w * scale) as u32, (base_h * scale) as u32);
    Rect::new(
        ((ww - w) / 2) as i32,
        ((wh - h) / 2) as i32,
        w.max(1),
        h.max(1),
    )
}

pub fn run(options: &Options) -> Result<(), String> {
    let sdl = sdl2::init()?;
    let video = sdl.video()?;
    let controllers = sdl.game_controller()?;
    let session = Session::open(options, SAMPLE_RATE)?;
    eprintln!("{}", session.describe());
    let title = format!("gase - {}", session.title());

    let window = video
        .window(&title, 320 * options.scale, 224 * options.scale)
        .position_centered()
        .resizable()
        .allow_highdpi()
        .build()
        .map_err(|e| e.to_string())?;
    let mut canvas = window.into_canvas().build().map_err(|e| e.to_string())?;
    if options.fullscreen {
        canvas
            .window_mut()
            .set_fullscreen(FullscreenType::Desktop)?;
    }
    let creator = canvas.texture_creator();
    let mut texture = creator
        .create_texture_streaming(
            PixelFormatEnum::ARGB8888,
            gase_core::MAX_WIDTH as u32,
            gase_core::MAX_HEIGHT as u32,
        )
        .map_err(|e| e.to_string())?;

    let audio: Option<AudioQueue<i16>> = if options.audio {
        let audio = sdl.audio()?;
        let spec = AudioSpecDesired {
            freq: Some(SAMPLE_RATE as i32),
            channels: Some(2),
            samples: Some(512),
        };
        let queue = audio.open_queue::<i16, _>(None, &spec)?;
        queue.resume();
        Some(queue)
    } else {
        None
    };

    // The debugger window is created hidden and shown with F1.
    let mut debug_canvas = video
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
    let mut debug_shown = false;

    let frame_rate = session.genesis.frame_rate();
    let mut debug = Debugger::new();
    for &addr in &options.breakpoints {
        debug.add_breakpoint(addr);
    }
    let mut fe = Frontend {
        session,
        keyboard: Buttons::default(),
        pads: Vec::new(),
        paused: false,
        step_frame: false,
        fast_forward: false,
        rewinding: false,
        muted: false,
        slot: 0,
        rewind: Rewind::new(20, 4, frame_rate),
        message: None,
        debug,
        panel: Panel::new(),
        debug_open: options.debug,
        debug_command: None,
    };
    fe.paused = options.debug;

    let mut events = sdl.event_pump()?;
    let mut samples = Vec::with_capacity(4096);
    let frame_time = Duration::from_secs_f64(1.0 / frame_rate);
    let mut next_deadline = Instant::now();
    let mut last_save_flush = Instant::now();
    let mut shown_title = String::new();

    'main: loop {
        for event in events.poll_iter() {
            match event {
                Event::Quit { .. } => break 'main,
                Event::KeyDown {
                    scancode: Some(code),
                    keycode,
                    window_id,
                    ..
                } if fe.debug_open && window_id == debug_window_id => {
                    if matches!(code, Scancode::F1 | Scancode::Grave) {
                        fe.debug_open = false;
                    } else if let Some(key) = debugger_key(code, keycode) {
                        // Key repeat is welcome here: hold S to keep stepping.
                        fe.debugger_key(key);
                    }
                }
                Event::KeyDown {
                    scancode: Some(code),
                    repeat,
                    ..
                } => {
                    if let Some(b) = keyboard_button(code) {
                        fe.keyboard.set(b, true);
                    } else if code == Scancode::Tab {
                        fe.fast_forward = true;
                    } else if code == Scancode::Backspace {
                        fe.rewinding = true;
                    } else if !repeat && !fe.hotkey(code, &mut canvas) {
                        break 'main;
                    }
                }
                Event::KeyUp {
                    scancode: Some(code),
                    ..
                } => {
                    if let Some(b) = keyboard_button(code) {
                        fe.keyboard.set(b, false);
                    } else if code == Scancode::Tab {
                        fe.fast_forward = false;
                    } else if code == Scancode::Backspace {
                        fe.rewinding = false;
                    }
                }
                Event::ControllerDeviceAdded { which, .. } => {
                    if let Ok(controller) = controllers.open(which) {
                        let name = controller.name();
                        fe.pads.push(Pad {
                            controller,
                            buttons: Buttons::default(),
                            stick: Buttons::default(),
                        });
                        fe.notify(format!(
                            "Controller connected: {name} (player {})",
                            fe.pads.len().min(2)
                        ));
                    }
                }
                Event::ControllerDeviceRemoved { which, .. } => {
                    if let Some(i) = fe.pad_index(which) {
                        fe.pads.remove(i);
                        fe.notify("Controller disconnected");
                    }
                }
                Event::ControllerButtonDown { which, button, .. }
                | Event::ControllerButtonUp { which, button, .. } => {
                    let pressed = matches!(event, Event::ControllerButtonDown { .. });
                    if let (Some(i), Some(b)) = (fe.pad_index(which), pad_button(button)) {
                        fe.pads[i].buttons.set(b, pressed);
                    }
                }
                Event::ControllerAxisMotion {
                    which, axis, value, ..
                } => {
                    if let Some(i) = fe.pad_index(which) {
                        const DEAD_ZONE: i16 = 12_000;
                        let stick = &mut fe.pads[i].stick;
                        match axis {
                            Axis::LeftX => {
                                stick.set(Buttons::LEFT, value < -DEAD_ZONE);
                                stick.set(Buttons::RIGHT, value > DEAD_ZONE);
                            }
                            Axis::LeftY => {
                                stick.set(Buttons::UP, value < -DEAD_ZONE);
                                stick.set(Buttons::DOWN, value > DEAD_ZONE);
                            }
                            _ => {}
                        }
                    }
                }
                Event::Window {
                    window_id,
                    win_event: WindowEvent::Close,
                    ..
                } => {
                    if window_id == main_window_id {
                        break 'main;
                    }
                    fe.debug_open = false;
                }
                Event::Window {
                    win_event: WindowEvent::FocusLost,
                    ..
                } => {
                    // Avoid stuck keys when the window loses focus.
                    fe.keyboard = Buttons::default();
                }
                _ => {}
            }
        }
        fe.update_inputs();

        // --- Emulate ---------------------------------------------------------
        if let Some(command) = fe.debug_command.take() {
            fe.run_debug_command(command);
        }
        let run = !fe.paused || std::mem::take(&mut fe.step_frame);
        if run {
            if fe.rewinding {
                if fe.rewind.step_back(&mut fe.session.genesis) {
                    fe.debug.forget_position();
                    fe.session.genesis.run_frame();
                }
            } else {
                let count = if fe.fast_forward {
                    FAST_FORWARD_FRAMES
                } else {
                    1
                };
                for _ in 0..count {
                    // The fast path, unless the debugger has work to do.
                    if fe.debug.breakpoints().is_empty() && !fe.debug.in_frame() {
                        fe.session.genesis.run_frame();
                    } else if let Stop::Breakpoint(pc) = fe.debug.run_frame(&mut fe.session.genesis)
                    {
                        fe.session.print_trace();
                        fe.paused = true;
                        fe.debug_open = true;
                        fe.panel.follow_pc();
                        fe.panel.set_status(format!("Breakpoint at ${pc:06X}"));
                        fe.notify(format!("Breakpoint at ${pc:06X}"));
                        break;
                    }
                    fe.rewind.record(&fe.session.genesis);
                    fe.session.print_trace();
                }
            }
        }
        samples.clear();
        fe.session.genesis.drain_audio(&mut samples);

        // --- Audio and pacing --------------------------------------------------
        let normal_speed = run && !fe.fast_forward && !fe.rewinding;
        match &audio {
            Some(queue) if normal_speed => {
                let queued = queue.size() / 4; // stereo i16 frames
                // Dynamic rate control: ±0.5% depending on the fill level.
                let error = (f64::from(TARGET_QUEUE) - f64::from(queued)) / f64::from(TARGET_QUEUE);
                fe.session
                    .genesis
                    .set_audio_speed(1.0 + 0.005 * error.clamp(-1.0, 1.0));
                if fe.muted {
                    samples.iter_mut().for_each(|s| *s = 0);
                }
                queue.queue_audio(&samples)?;
                while queue.size() / 4 > TARGET_QUEUE {
                    std::thread::sleep(Duration::from_millis(1));
                }
                next_deadline = Instant::now();
            }
            _ => {
                if fe.fast_forward && run {
                    next_deadline = Instant::now();
                } else {
                    // No audio pacing: sleep until the next frame is due.
                    next_deadline += frame_time;
                    let now = Instant::now();
                    if next_deadline > now {
                        std::thread::sleep(next_deadline - now);
                    } else {
                        next_deadline = now;
                    }
                }
            }
        }

        // --- Video ----------------------------------------------------------------
        let frame = fe.session.genesis.frame();
        texture
            .with_lock(
                Rect::new(0, 0, frame.width as u32, frame.height as u32),
                |buffer, pitch| {
                    for (y, row) in frame
                        .pixels
                        .chunks(frame.stride)
                        .take(frame.height)
                        .enumerate()
                    {
                        let line = &mut buffer[y * pitch..y * pitch + frame.width * 4];
                        for (dst, &src) in line.chunks_exact_mut(4).zip(&row[..frame.width]) {
                            dst.copy_from_slice(&(src | 0xFF00_0000).to_ne_bytes());
                        }
                    }
                },
            )
            .map_err(|e| e.to_string())?;
        let interlaced = frame.height > 240;
        let lines = if interlaced {
            frame.height / 2
        } else {
            frame.height
        } as u32;
        let target = display_rect(canvas.output_size()?, lines, options.integer_scale);
        canvas.set_draw_color(sdl2::pixels::Color::BLACK);
        canvas.clear();
        canvas.copy(
            &texture,
            Rect::new(0, 0, frame.width as u32, frame.height as u32),
            target,
        )?;
        canvas.present();

        // --- Debugger window -------------------------------------------------------
        if fe.debug_open != debug_shown {
            let window = debug_canvas.window_mut();
            if fe.debug_open {
                window.show();
                window.raise();
            } else {
                window.hide();
            }
            debug_shown = fe.debug_open;
        }
        if fe.debug_open {
            let picture = fe.panel.draw(&fe.session.genesis, &fe.debug, fe.paused);
            debug_texture
                .with_lock(None, |buffer, pitch| {
                    for (row, line) in picture
                        .pixels
                        .chunks(picture.width)
                        .zip(buffer.chunks_mut(pitch))
                    {
                        for (dst, &src) in line.chunks_exact_mut(4).zip(row) {
                            dst.copy_from_slice(&(src | 0xFF00_0000).to_ne_bytes());
                        }
                    }
                })
                .map_err(|e| e.to_string())?;
            let (ww, wh) = debug_canvas.output_size()?;
            let scale = (f64::from(ww) / debugger::WIDTH as f64)
                .min(f64::from(wh) / debugger::HEIGHT as f64);
            let (w, h) = (
                (debugger::WIDTH as f64 * scale) as u32,
                (debugger::HEIGHT as f64 * scale) as u32,
            );
            debug_canvas.set_draw_color(sdl2::pixels::Color::BLACK);
            debug_canvas.clear();
            debug_canvas.copy(
                &debug_texture,
                None,
                Rect::new(
                    ((ww - w.min(ww)) / 2) as i32,
                    ((wh - h.min(wh)) / 2) as i32,
                    w.max(1),
                    h.max(1),
                ),
            )?;
            debug_canvas.present();
        }

        // --- Housekeeping ----------------------------------------------------------
        let mut status = title.clone();
        if fe.paused {
            status.push_str(" [paused]");
        } else if fe.fast_forward {
            status.push_str(" [fast forward]");
        } else if fe.rewinding {
            status.push_str(" [rewind]");
        }
        if let Some((text, since)) = &fe.message {
            if since.elapsed() < MESSAGE_TIME {
                status = format!("{status} - {text}");
            } else {
                fe.message = None;
            }
        }
        if status != shown_title {
            canvas
                .window_mut()
                .set_title(&status)
                .map_err(|e| e.to_string())?;
            shown_title = status;
        }
        if last_save_flush.elapsed() > Duration::from_secs(5) {
            fe.session.flush_save();
            last_save_flush = Instant::now();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_rect_letterboxes() {
        // A 4:3 window shows the 10:7 picture with bars above and below.
        let r = display_rect((1024, 768), 224, false);
        assert_eq!((r.width(), r.x()), (1024, 0));
        assert!(r.y() > 0);
        let r = display_rect((1000, 700), 224, true);
        assert_eq!((r.width(), r.height()), (960, 672));
    }
}
