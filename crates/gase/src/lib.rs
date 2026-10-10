//! gase: a Sega Mega Drive / Genesis emulator — the shells around the
//! user interface.
//!
//! `gase-core` emulates the console and `gase-app` provides everything the
//! user sees and touches (menus, settings, input mapping). What is left
//! here is how gase meets an operating system:
//!
//! | Module | What |
//! |---|---|
//! | [`sdl`] | The SDL2 shell: a window, sound and input devices around the app. **Shared by the desktop and the Android and iOS apps**: [`sdl::run_sdl`] with [`sdl::Shell::Desktop`] or [`sdl::Shell::Mobile`] |
//! | [`desktop`] | Files on a desktop: settings in the configuration folder, saves next to the ROM, a file browser |
//! | [`mobile`] | Files on a phone: everything in the app's private folder (its *sandbox*) |
//! | [`cli`], [`headless`] | The `gase` command line, and running without a window (tests, CI, benchmarks) |
//!
//! The binary (`src/main.rs`) only calls [`run_command_line`]: so can a
//! statically recompiled game (see `gase_core::system::external`), which
//! registers its code first and then gets the whole desktop emulator
//! around it. The Android and iOS
//! apps call [`sdl::run_sdl`] from the `gase-mobile` crate
//! (`crates/mobile`), which is what their native code starts. See
//! `mobile/README.md` for how a Rust library becomes a phone app.

pub mod cli;
mod debugger;
pub mod desktop;
pub mod headless;
mod media;
pub mod mobile;
#[cfg(feature = "sdl")]
pub mod sdl;
mod session;

use std::process::ExitCode;

/// The `gase` command: parse `args` (without the program name) and run.
///
/// This is all of `gase`'s `main`. A statically recompiled game calls it
/// too, after `gase_core::system::external::register`: any console created
/// for its ROM then runs the recompiled code, in the same window, menus,
/// save states and controls as any other game.
#[must_use]
pub fn run_command_line(args: impl IntoIterator<Item = String>) -> ExitCode {
    let options = match cli::parse(args) {
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
    use sdl::{Mobile, Shell, run_sdl};
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
