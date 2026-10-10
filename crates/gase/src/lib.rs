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
//! The binary (`src/main.rs`) is only the command line; the Android and iOS
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
