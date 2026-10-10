//! The `gase` command: the desktop emulator.
//!
//! Everything lives in the `gase` library next to this file (so the
//! Android and iOS apps can reuse the SDL shell, see
//! [`gase::sdl::run_sdl`]); this is only the command line. `--headless`
//! (or a build without the `sdl` feature) runs without any window, for
//! tests, CI and benchmarks.

use std::process::ExitCode;

use gase::{cli, headless};

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

    let result = if options.headless {
        headless::run(&options)
    } else {
        run_windowed(&options)
    };
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
    use gase::sdl::{Mobile, Shell, run_sdl};
    if options.mobile {
        // The phone app's shell, in a window: to try it without a phone.
        return run_sdl(Shell::Mobile(Mobile {
            open: options
                .rom
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            window: Some(options.ui_size),
            ..Mobile::default()
        }));
    }
    run_sdl(Shell::Desktop(options))
}

#[cfg(not(feature = "sdl"))]
fn run_windowed(_options: &cli::Options) -> Result<(), String> {
    Err("this build has no window support (built without the `sdl` feature); use --headless".into())
}
