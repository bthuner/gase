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

pub use native::SDL_main;

use gase::sdl::{Mobile, Shell, run_sdl};

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

/// Run the app (called by [`SDL_main`]); returns the process's exit status.
#[must_use]
pub fn run(args: &[String]) -> i32 {
    // A panic message on stderr is lost on a phone; send it to the system
    // log (`adb logcat`, Xcode's console) where it can be found.
    std::panic::set_hook(Box::new(|info| {
        sdl2::log::log(&format!("gase panicked: {info}"));
    }));
    let mobile = Mobile {
        open: rom_argument(args),
        pick_rom: native::PICK_ROM,
        back_at_home: native::BACK_AT_HOME,
        window: None,
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
    fn the_desktop_has_no_native_hooks() {
        // The hooks exist only where there is native code to call.
        if cfg!(not(any(target_os = "android", target_os = "ios"))) {
            assert!(native::PICK_ROM.is_none());
            assert!(native::BACK_AT_HOME.is_none());
        }
    }
}
