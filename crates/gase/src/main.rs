//! The `gase` command: the desktop emulator.
//!
//! Everything lives in the `gase` library next to this file (so the
//! Android and iOS apps can reuse the SDL shell, see
//! [`gase::sdl::run_sdl`]); this is only the command line. `--headless`
//! (or a build without the `sdl` feature) runs without any window, for
//! tests, CI and benchmarks.

use std::process::ExitCode;

fn main() -> ExitCode {
    gase::run_command_line(std::env::args().skip(1))
}
