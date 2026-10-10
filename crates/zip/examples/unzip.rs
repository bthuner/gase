//! List a ZIP archive, extract its ROM, and time the extraction.
//!
//! ```sh
//! cargo run --release -p gase-zip --example unzip -- game.zip            # list
//! cargo run --release -p gase-zip --example unzip -- game.zip out.bin    # extract the ROM
//! cargo run --release -p gase-zip --example unzip -- game.zip --bench 20 # time it
//! ```

use std::process::ExitCode;
use std::time::{Duration, Instant};

use gase_zip::crc32::crc32;
use gase_zip::{Archive, find_rom, inflate, inflate_reference};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("unzip: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let path = args
        .first()
        .ok_or("usage: unzip <file.zip> [out | --bench N]")?;
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let archive = Archive::parse(&bytes).map_err(|e| format!("{path}: {e}"))?;
    for entry in archive.entries() {
        println!(
            "{:>9} {:>9} {:?} {}",
            entry.size(),
            entry.compressed_size(),
            entry.method(),
            entry.name()
        );
    }
    let rom = find_rom(&archive).ok_or("no ROM in the archive")?;
    println!("ROM: {}", rom.name());

    match args.get(1).map(String::as_str) {
        None => Ok(()),
        Some("--bench") => {
            let rounds: u32 = args.get(2).and_then(|n| n.parse().ok()).unwrap_or(10);
            let fast = time(rounds, || archive.read(rom).map(|d| d.len()));
            println!("Archive::read      {}", report(fast?, rom.size()));
            // Where the time goes: decompression and checksum alone, and
            // the bit-at-a-time reference decoder for comparison.
            let packed = archive.compressed_data(rom).map_err(|e| e.to_string())?;
            let size = rom.size() as usize;
            let fast = time(rounds, || inflate(packed, size).map(|d| d.len()));
            println!("  inflate          {}", report(fast?, rom.size()));
            let data = archive.read(rom).map_err(|e| e.to_string())?;
            let crc = time(rounds, || {
                Ok::<_, String>(crc32(std::hint::black_box(&data)) as usize)
            });
            println!("  crc32            {}", report(crc?, rom.size()));
            let reference = time(rounds.min(3), || {
                inflate_reference(packed, size).map(|d| d.len())
            });
            println!("inflate_reference  {}", report(reference?, rom.size()));
            Ok(())
        }
        Some(out) => {
            let data = archive.read(rom).map_err(|e| e.to_string())?;
            std::fs::write(out, data).map_err(|e| format!("{out}: {e}"))
        }
    }
}

/// Median time of `rounds` runs.
fn time<E: std::fmt::Display>(
    rounds: u32,
    mut f: impl FnMut() -> Result<usize, E>,
) -> Result<Duration, String> {
    let mut times = Vec::new();
    for _ in 0..rounds.max(1) {
        let start = Instant::now();
        std::hint::black_box(f().map_err(|e| e.to_string())?);
        times.push(start.elapsed());
    }
    times.sort();
    Ok(times[times.len() / 2])
}

fn report(t: Duration, size: u64) -> String {
    let mib = size as f64 / (1024.0 * 1024.0);
    format!(
        "{:8.2} ms  ({:.0} MiB/s)",
        t.as_secs_f64() * 1000.0,
        mib / t.as_secs_f64()
    )
}
