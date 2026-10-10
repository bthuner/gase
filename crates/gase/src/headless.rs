//! Running without a window: for automated tests, CI, benchmarks and
//! recording.

use std::fs::File;
use std::io::BufWriter;
use std::time::Instant;

use std::path::Path;

use gase_app::{App, Capabilities, Event, Key};
use gase_core::{Debugger, Stop};

use crate::cli::Options;
use crate::debugger;
use crate::desktop::Desktop;
use crate::media::WavWriter;
use crate::session::Session;

const SAMPLE_RATE: u32 = 48_000;

pub fn run(options: &Options) -> Result<(), String> {
    if let Some(path) = &options.dump_ui {
        return dump_ui(options, path);
    }
    let mut session = Session::open(options, SAMPLE_RATE)?;
    eprintln!("{}", session.describe());

    let mut wav = match &options.wav {
        Some(path) => {
            let file =
                File::create(path).map_err(|e| format!("cannot create {}: {e}", path.display()))?;
            Some(WavWriter::new(BufWriter::new(file), SAMPLE_RATE).map_err(|e| e.to_string())?)
        }
        None => None,
    };

    let mut debug = Debugger::new();
    for &addr in &options.breakpoints {
        debug.add_breakpoint(addr);
    }

    let mut audio = Vec::new();
    let mut frames = 0;
    let start = Instant::now();
    while frames < options.frames {
        // The debugger's frame loop checks breakpoints; the normal one is
        // a little faster, so it is used when there are none.
        let stop = if debug.breakpoints().is_empty() {
            session.genesis.run_frame();
            Stop::FrameEnd
        } else {
            debug.run_frame(&mut session.genesis)
        };
        session.print_trace();
        audio.clear();
        session.genesis.drain_audio(&mut audio);
        if let Some(wav) = &mut wav {
            wav.write(&audio).map_err(|e| e.to_string())?;
        }
        if let Stop::Breakpoint(pc) = stop {
            println!(
                "Breakpoint at ${pc:06X} in frame {}, line {}",
                session.genesis.frame_count(),
                debug.line().unwrap_or(0)
            );
            print!("{}", debugger::cpu::report(&session.genesis, &debug));
            break;
        }
        frames += 1;
    }
    let elapsed = start.elapsed();
    if !debug.breakpoints().is_empty() && frames == options.frames {
        eprintln!("No breakpoint was reached in {frames} frames");
    }

    if let Some(wav) = wav {
        wav.finish().map_err(|e| e.to_string())?;
    }
    if let Some(path) = &options.screenshot {
        let path = session.screenshot(Some(path))?;
        eprintln!("Saved {}", path.display());
    }
    let genesis = &session.genesis;
    if let Some(path) = &options.dump_vram {
        let colors = debugger::vdp::palette_rgb(genesis);
        let sheet = debugger::vdp::tile_sheet(&genesis.hw.vdp.vram, &colors[..16]);
        write_png(path, &sheet)?;
    }
    if let Some(path) = &options.dump_cram {
        let colors = debugger::vdp::palette_rgb(genesis);
        write_png(path, &debugger::vdp::cram_swatches(&colors, 16))?;
    }
    if let Some(path) = &options.dump_debugger {
        let mut panel = debugger::Panel::new();
        write_png(path, panel.draw(genesis, &debug, true))?;
    }
    if options.bench {
        let fps = frames as f64 / elapsed.as_secs_f64();
        eprintln!(
            "{} frames in {:.2?}: {fps:.1} fps ({:.1}x real time)",
            frames,
            elapsed,
            fps / session.genesis.frame_rate()
        );
    }
    Ok(())
}

fn write_png(path: &std::path::Path, canvas: &debugger::Canvas) -> Result<(), String> {
    std::fs::write(path, canvas.to_png())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    eprintln!("Saved {}", path.display());
    Ok(())
}

/// `--dump-ui`: run the app without a window and save what it would show.
///
/// This is the same [`App`] the window runs, driven by the same calls; only
/// the pixels go to a PNG file instead of the screen. It makes pictures of
/// the menus for documentation, and lets anyone check a layout at a phone's
/// size and density from a desktop.
fn dump_ui(options: &Options, path: &Path) -> Result<(), String> {
    let mut platform = Desktop::new();
    let caps = Capabilities {
        sample_rate: SAMPLE_RATE,
        touch_screen: options.touch,
        file_browser: true,
        drop_files: true,
        can_quit: true,
        fullscreen: true,
        debugger: true,
        ..Capabilities::default()
    };
    let mut app = App::new(&mut platform, caps);
    crate::apply_overrides(&mut app, options);
    let (width, height) = options.ui_size;
    app.handle(
        &mut platform,
        Event::Resized {
            width,
            height,
            pixels_per_point: options.ui_density,
        },
    );
    if let Some(rom) = &options.rom {
        app.open_rom(&mut platform, &rom.to_string_lossy())?;
        if let Some(game) = app.game_mut() {
            game.genesis.set_trace(options.trace);
        }
        for _ in 0..options.frames {
            app.update(&mut platform);
        }
    }
    let default = if options.rom.is_some() {
        "pause"
    } else {
        "home"
    };
    for name in options.ui_screens.as_deref().unwrap_or(default).split(',') {
        let name = name.trim();
        if let Some(key) = name.strip_prefix("key:") {
            // Press and release a key, e.g. `key:F5` to save a state first.
            let key =
                Key::from_name(key).ok_or_else(|| format!("--ui-screen: unknown key '{key}'"))?;
            for pressed in [true, false] {
                let event = Event::Key {
                    key,
                    pressed,
                    repeat: false,
                };
                app.handle(&mut platform, event);
                app.update(&mut platform);
            }
            continue;
        }
        if name != "game" && !app.show_screen(&mut platform, name) {
            return Err(format!("--ui-screen: cannot show '{name}' here"));
        }
    }
    // Two updates: the first lays the screen out, the second draws it
    // settled (and lets touch controls appear over a running game).
    app.update(&mut platform);
    app.update(&mut platform);
    write_png(path, &app.compose())
}
