//! Running without a window: for automated tests, CI, benchmarks and
//! recording.

use std::fs::File;
use std::io::BufWriter;
use std::time::Instant;

use gase_core::{Debugger, Stop};

use crate::cli::Options;
use crate::debugger;
use crate::media::WavWriter;
use crate::session::Session;

const SAMPLE_RATE: u32 = 48_000;

pub fn run(options: &Options) -> Result<(), String> {
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

fn write_png(path: &std::path::Path, canvas: &debugger::canvas::Canvas) -> Result<(), String> {
    std::fs::write(path, canvas.to_png())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    eprintln!("Saved {}", path.display());
    Ok(())
}
