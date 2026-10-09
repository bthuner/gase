//! gase: a Sega Mega Drive / Genesis emulator.
//!
//! This binary is a thin frontend over `gase-core`. With the default `sdl`
//! feature it opens a window; `--headless` (or a build without the feature)
//! runs without any window, for tests, CI and benchmarks.

mod cli;
mod headless;
mod media;
mod session;
#[cfg(feature = "sdl")]
mod sdl;

use std::process::ExitCode;

fn main() -> ExitCode {
    let options = match cli::parse(std::env::args().skip(1)) {
        Ok(cli::Command::Run(options)) => options,
        Ok(cli::Command::Help) => {
            print!("{}", cli::USAGE);
            return ExitCode::SUCCESS;
        }
        Ok(cli::Command::Version) => {
            println!("gase {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(2);
        }
    };

    let result = if options.headless { headless::run(&options) } else { run_windowed(&options) };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(feature = "sdl")]
fn run_windowed(options: &cli::Options) -> Result<(), String> {
    sdl::run(options)
}

#[cfg(not(feature = "sdl"))]
fn run_windowed(_options: &cli::Options) -> Result<(), String> {
    Err("this build has no window support (built without the `sdl` feature); use --headless".into())
}
