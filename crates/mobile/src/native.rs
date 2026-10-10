//! Everything that crosses into C, Java or Objective-C: the only module of
//! the workspace that may use `unsafe`.
//!
//! Three things need it:
//!
//! 1. **Exporting `SDL_main`.** SDL looks the entry point up by its C name
//!    (with `dlsym` on Android, through the linker on iOS). Rust mangles
//!    names, so the function needs `#[unsafe(no_mangle)]`, which is unsafe
//!    because two exported symbols with the same name would clash at link
//!    time. There is exactly one `SDL_main` in the app: this one.
//! 2. **Reading `argv`.** C passes the arguments as raw pointers to
//!    NUL-terminated strings; turning them into Rust strings trusts that
//!    they are what C promises.
//! 3. **Declaring native functions** (an `unsafe extern` block): Rust
//!    cannot check that a C function has the signature we write down, so
//!    the declaration is a promise. The functions are marked `safe` because
//!    calling them has no further requirement once the promise holds.
//!
//! What the native side does with these calls is in the glue:
//! `mobile/android/app/src/main/java/io/github/bthuner/gase/GaseActivity.java`
//! and `mobile/ios/Sources/GaseAppDelegate.m`.
#![allow(unsafe_code)]

use std::ffi::{CStr, c_char, c_int};

/// The entry point SDL calls (see the [crate documentation](crate)).
///
/// # Safety
///
/// `argv` must point to `argc` pointers, each null or pointing to a
/// NUL-terminated string, as C's `main` receives them. SDL guarantees this
/// on both systems (on Android it builds them from the Java `String[]` of
/// `GaseActivity.getArguments()`; on iOS they are the process's own).
#[allow(non_snake_case)] // the name is SDL's
#[unsafe(no_mangle)]
pub unsafe extern "C" fn SDL_main(argc: c_int, argv: *mut *mut c_char) -> c_int {
    let count = if argv.is_null() {
        0
    } else {
        usize::try_from(argc).unwrap_or(0)
    };
    let args: Vec<String> = (0..count)
        .filter_map(|i| {
            // SAFETY: `i < argc`, and the caller promises `argc` pointers.
            let arg = unsafe { *argv.add(i) };
            // SAFETY: non-null pointers point to NUL-terminated strings
            // that live as long as this call.
            (!arg.is_null()).then(|| {
                unsafe { CStr::from_ptr(arg) }
                    .to_string_lossy()
                    .into_owned()
            })
        })
        .collect();
    crate::run(&args)
}

#[cfg(target_os = "android")]
mod android {
    use std::ffi::c_int;

    /// The message `GaseActivity.onUnhandledMessage` answers by opening
    /// the document picker. SDL reserves numbers from 0x8000 for apps
    /// (`SDLActivity.COMMAND_USER`); keep it equal to
    /// `GaseActivity.COMMAND_PICK_ROM`.
    const COMMAND_PICK_ROM: u32 = 0x8000 + 1;

    // SAFETY: these are the signatures declared in SDL2's `SDL_system.h`
    // (available on Android since SDL 2.0.9 and 2.0.22), and `libSDL2.so`,
    // which `libmain.so` links against, defines them.
    unsafe extern "C" {
        /// Posts a message to the Java activity's UI thread; returns 0 on
        /// success. Safe to call from any thread.
        safe fn SDL_AndroidSendMessage(command: u32, param: c_int) -> c_int;
        /// Does what the Back button does without SDL trapping it: leaves
        /// the activity.
        safe fn SDL_AndroidBackButton();
    }

    pub fn pick_rom() {
        if SDL_AndroidSendMessage(COMMAND_PICK_ROM, 0) != 0 {
            sdl2::log::log("gase: cannot reach the activity to pick a ROM");
        }
    }

    pub fn back() {
        SDL_AndroidBackButton();
    }
}

#[cfg(target_os = "ios")]
mod ios {
    // SAFETY: defined in mobile/ios/Sources/GaseAppDelegate.m with this
    // signature (`void gase_ios_pick_rom(void)`), and linked into the same
    // executable. It must be called on the main thread, where SDL_main
    // runs on iOS.
    unsafe extern "C" {
        /// Shows UIDocumentPickerViewController; the chosen ROM comes back
        /// as an SDL drop-file event.
        safe fn gase_ios_pick_rom();
    }

    pub fn pick_rom() {
        gase_ios_pick_rom();
    }
}

/// Show the system's document picker, where there is one.
#[cfg(target_os = "android")]
pub const PICK_ROM: Option<fn()> = Some(android::pick_rom);
#[cfg(target_os = "ios")]
pub const PICK_ROM: Option<fn()> = Some(ios::pick_rom);
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub const PICK_ROM: Option<fn()> = None;

/// Leave the app from the home screen (Android's Back button).
#[cfg(target_os = "android")]
pub const BACK_AT_HOME: Option<fn()> = Some(android::back);
#[cfg(not(target_os = "android"))]
pub const BACK_AT_HOME: Option<fn()> = None;
