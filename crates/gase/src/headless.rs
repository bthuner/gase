//! Running without a window: for automated tests, CI, benchmarks and
//! recording.

use std::fs::File;
use std::io::BufWriter;
use std::time::Instant;

use crate::cli::Options;
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

    let mut audio = Vec::new();
    let start = Instant::now();
    for _ in 0..options.frames {
        session.genesis.run_frame();
        session.print_trace();
        audio.clear();
        session.genesis.drain_audio(&mut audio);
        if let Some(wav) = &mut wav {
            wav.write(&audio).map_err(|e| e.to_string())?;
        }
    }
    let elapsed = start.elapsed();

    if let Some(wav) = wav {
        wav.finish().map_err(|e| e.to_string())?;
    }
    if let Some(path) = &options.screenshot {
        let path = session.screenshot(Some(path))?;
        eprintln!("Saved {}", path.display());
    }
    if options.bench {
        let fps = options.frames as f64 / elapsed.as_secs_f64();
        eprintln!(
            "{} frames in {:.2?}: {fps:.1} fps ({:.1}x real time)",
            options.frames,
            elapsed,
            fps / session.genesis.frame_rate()
        );
    }
    Ok(())
}
