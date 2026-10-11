//! The native entry point of the gase Android and iOS apps.
//!
//! # How a Rust program becomes a phone app
//!
//! Neither Android nor iOS starts a program at `main` the way a desktop
//! does. An app is a *bundle* that the system launches through its own
//! framework, and native code is a library inside it:
//!
//! ```text
//!  Android                                  iOS
//!  ───────                                  ───
//!  launcher taps the icon                   springboard taps the icon
//!       │                                        │
//!  ART (the Java VM) starts                 the executable's C `main`
//!  io.github.bthuner.gase.GaseActivity      (mobile/ios/Sources/main.m)
//!  = SDL's Java SDLActivity + our glue           │
//!       │ System.loadLibrary("SDL2")        SDL_UIKitRunApp → UIApplicationMain
//!       │ System.loadLibrary("main")        with SDL's app delegate (ours
//!       │ starts a thread "SDLThread"       subclasses it: GaseAppDelegate.m)
//!       │ which calls, through JNI,              │ didFinishLaunching
//!       ▼                                        ▼
//!  SDL_main(argc, argv) in libmain.so       SDL_main(argc, argv), linked in
//!       └────────────── both are this crate's [`SDL_main`] ─────────────┘
//!                                   │
//!                      gase::sdl::run_sdl(Shell::Mobile(…))
//!                      the same main loop as the desktop
//! ```
//!
//! * On **Android** this crate is compiled as a `cdylib` named
//!   `libmain.so` for each processor family ("ABI": `arm64-v8a`,
//!   `armeabi-v7a`, `x86_64`), and packed into the APK next to
//!   `libSDL2.so`. Java and native code talk through **JNI** (the Java
//!   Native Interface): SDL's Java classes call C functions in `libSDL2.so`
//!   for every touch, key and resize, and C calls Java methods to open the
//!   sound device or change the orientation. All of that is SDL's; gase
//!   only adds a few lines of Java (`GaseActivity.java`) for the document
//!   picker, and reaches it with one SDL function, see `native.rs`.
//! * On **iOS** it is a `staticlib`, linked into the app's single
//!   executable together with SDL2 (also static) and a few lines of
//!   Objective-C, because iOS apps cannot load their own dynamic
//!   libraries except as embedded frameworks.
//!
//! Either way, `SDL_main` runs the shared SDL shell ([`gase::sdl`]) with
//! the phone's settings; everything after that is the same Rust code as on
//! the desktop.
//!
//! # Unsafe code
//!
//! The workspace forbids `unsafe`. Talking to C needs a little of it, so
//! this crate only denies it, and the one module that needs it
//! (`native.rs`) allows it, with a justification for every use: exporting
//! `SDL_main` under its C name, reading the C argument strings, and
//! declaring the two native functions it calls.

mod native;

#[cfg(feature = "sdl-main")]
pub use native::SDL_main;
pub use native::args;

use std::path::PathBuf;

pub use gase::sdl::Notices;
use gase::sdl::{Mobile, Shell, run_sdl};

/// What an app built on this crate adds to gase (all off in gase's own
/// app).
#[derive(Clone, Copy, Debug, Default)]
pub struct Hooks {
    /// Offer "Make an app of this game…" in the pause menu, and do it:
    /// see [`gase::sdl::Mobile::recompile`]. gase-recomp's phone app
    /// asks a studio on the network to build the app, then hands it to
    /// [`install_apk`].
    pub recompile: Option<fn(rom: String, notices: Notices)>,
}

/// The ROM to open at start, from `SDL_main`'s arguments.
///
/// `GaseActivity` passes the path of a ROM copied from an "Open with"
/// request as the only argument. iOS passes none of ours, but Xcode may
/// add options such as `-NSDocumentRevisionsDebugMode YES`: anything that
/// looks like an option is not a ROM.
fn rom_argument(args: &[String]) -> Option<String> {
    args.iter()
        .skip(1)
        .find(|a| !a.is_empty() && !a.starts_with('-'))
        .cloned()
}

/// Run the app (called by `SDL_main`); returns the process's exit status.
#[must_use]
pub fn run(args: &[String]) -> i32 {
    run_with(args, Hooks::default())
}

/// Run the app with some [`Hooks`] (called by an app's own `SDL_main`).
#[must_use]
pub fn run_with(args: &[String], hooks: Hooks) -> i32 {
    run_opening(rom_argument(args), hooks)
}

/// Offer `apk` to Android's package installer, which asks the user to
/// confirm (and, the first time, to allow this app to install apps).
/// Returns once the request is sent; the installer's window follows.
///
/// The APK is written to `<app folder>/builds/app.apk`, where
/// `GaseActivity.java` reads it. Any thread may call this.
///
/// # Errors
///
/// When the file cannot be written, or on a system other than Android
/// (iOS only installs apps from its store).
pub fn install_apk(apk: &[u8]) -> Result<(), String> {
    let Some(install) = native::INSTALL_APK else {
        return Err("only Android installs apps this way".into());
    };
    let data = sdl2::filesystem::pref_path("", "gase").map_err(|e| e.to_string())?;
    let dir = PathBuf::from(data).join("builds");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("app.apk");
    let part = path.with_extension("part");
    std::fs::write(&part, apk).map_err(|e| e.to_string())?;
    std::fs::rename(&part, &path).map_err(|e| e.to_string())?;
    if install() {
        Ok(())
    } else {
        Err("cannot reach the activity to install the app".into())
    }
}

/// Run a game shipped inside the app: a statically recompiled game, which
/// brings its own `SDL_main` (with this crate's `sdl-main` feature off):
///
/// ```ignore
/// #[unsafe(no_mangle)]
/// pub unsafe extern "C" fn SDL_main(argc: c_int, argv: *mut *mut c_char) -> c_int {
///     gase_recomp_runtime::register(recomp::ROM_CRC32, recomp::cpu);
///     let args = unsafe { gase_mobile::args(argc, argv) };
///     gase_mobile::run_game(&args, "My Game", include_bytes!("../../rom.bin"))
/// }
/// ```
///
/// The ROM is copied once into the app's ROM folder as `<name>.bin`, and
/// opened at start (unless the app was launched to open another file).
/// The menus still open any other ROM.
#[must_use]
pub fn run_game(args: &[String], name: &str, rom: &[u8]) -> i32 {
    let open = rom_argument(args).or_else(|| match install_rom(name, rom) {
        Ok(path) => Some(path.to_string_lossy().into_owned()),
        Err(e) => {
            sdl2::log::log(&format!("gase: cannot install {name}: {e}"));
            None
        }
    });
    run_opening(open, Hooks::default())
}

/// Copy `rom` to `<app folder>/roms/<name>.bin`, unless the same is there.
fn install_rom(name: &str, rom: &[u8]) -> Result<PathBuf, String> {
    let data = sdl2::filesystem::pref_path("", "gase").map_err(|e| e.to_string())?;
    let dir = PathBuf::from(data).join("roms");
    let path = dir.join(format!("{}.bin", file_name(name)));
    if std::fs::read(&path).is_ok_and(|have| have == rom) {
        return Ok(path);
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // A temporary name, then a rename: an app killed mid-copy leaves no
    // half-written ROM.
    let part = path.with_extension("part");
    std::fs::write(&part, rom).map_err(|e| e.to_string())?;
    std::fs::rename(&part, &path).map_err(|e| e.to_string())?;
    Ok(path)
}

/// A game title as a file name: no path separators or reserved characters.
fn file_name(title: &str) -> String {
    let name: String = title
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || " -_().".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let name = name.trim().trim_matches('.');
    if name.is_empty() {
        "game".into()
    } else {
        name.into()
    }
}

fn run_opening(open: Option<String>, hooks: Hooks) -> i32 {
    // A panic message on stderr is lost on a phone; send it to the system
    // log (`adb logcat`, Xcode's console) where it can be found.
    std::panic::set_hook(Box::new(|info| {
        sdl2::log::log(&format!("gase panicked: {info}"));
    }));
    let mobile = Mobile {
        open,
        pick_rom: native::PICK_ROM,
        back_at_home: native::BACK_AT_HOME,
        window: None,
        recompile: hooks.recompile,
    };
    match run_sdl(Shell::Mobile(mobile)) {
        Ok(()) => 0,
        Err(e) => {
            sdl2::log::log(&format!("gase: {e}"));
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn the_rom_comes_from_the_arguments() {
        assert_eq!(rom_argument(&args(&["app"])), None);
        assert_eq!(
            rom_argument(&args(&["app", "/data/files/roms/Game.md"])),
            Some("/data/files/roms/Game.md".into())
        );
        assert_eq!(
            rom_argument(&args(&["app", "-NSDocumentRevisionsDebugMode", "YES"])),
            Some("YES".into()),
            "only option-looking words are skipped"
        );
        assert_eq!(rom_argument(&args(&["app", "", "-x"])), None);
    }

    #[test]
    fn game_titles_become_file_names() {
        assert_eq!(file_name("ALADDIN"), "ALADDIN");
        assert_eq!(file_name("Sonic / Knuckles: 2"), "Sonic _ Knuckles_ 2");
        assert_eq!(file_name("../.."), "_", "no way out of the folder");
        assert_eq!(file_name("  "), "game");
    }

    #[test]
    fn the_desktop_has_no_native_hooks() {
        // The hooks exist only where there is native code to call.
        if cfg!(not(any(target_os = "android", target_os = "ios"))) {
            assert!(native::PICK_ROM.is_none());
            assert!(native::BACK_AT_HOME.is_none());
            assert!(install_apk(b"PK").is_err(), "no installer to hand it to");
        }
    }
}
