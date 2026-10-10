//! gase: a Sega Mega Drive / Genesis emulator.
//!
//! This binary is a thin shell: `gase-core` emulates the console and
//! `gase-app` provides everything the user sees and touches (menus,
//! settings, input mapping). What is left here is desktop-specific: the
//! command line, files ([`desktop`]), and with the default `sdl` feature a
//! window, sound and input devices ([`sdl`]). `--headless` (or a build
//! without the feature) runs without any window, for tests, CI and
//! benchmarks.

mod cli;
mod debugger;
mod desktop;
mod headless;
mod media;
#[cfg(feature = "sdl")]
mod sdl;
mod session;

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
    sdl::run(options)
}

#[cfg(not(feature = "sdl"))]
fn run_windowed(_options: &cli::Options) -> Result<(), String> {
    Err("this build has no window support (built without the `sdl` feature); use --headless".into())
}

/// Command-line options override the settings for this run.
fn apply_overrides(app: &mut gase_app::App, options: &cli::Options) {
    let s = app.settings_mut();
    if let Some(scale) = options.scale {
        s.video.scale = scale;
    }
    s.video.fullscreen |= options.fullscreen;
    s.video.integer_scale |= options.integer_scale;
    s.audio.low_pass &= options.low_pass;
    if options.region.is_some() {
        s.emulation.region = options.region;
    }
    s.emulation.lenient_address_errors |= !options.address_errors;
    if options.touch {
        s.touch = gase_app::TouchMode::On;
    }
}
