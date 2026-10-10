# gase on Android and iOS

The phone apps are the desktop emulator in another wrapping. The menus,
touch controls, settings and save states are `gase-app`'s; the main loop is
the desktop's SDL2 shell (`crates/gase/src/sdl.rs`); only a few dozen lines
of Java and Objective-C are specific to each system. This page explains
how that works, then how to build, install and debug the apps.

- [How a Rust program becomes a phone app](#how-a-rust-program-becomes-a-phone-app)
- [Building for Android](#building-for-android)
- [Building for iOS](#building-for-ios)
- [Trying the phone shell on a desktop](#trying-the-phone-shell-on-a-desktop)
- [What is tested where](#what-is-tested-where)
- [Troubleshooting](#troubleshooting)
- [Limitations](#limitations)

## How a Rust program becomes a phone app

### No `main`: the system starts the app

On a desktop, the operating system runs a program from its `main`. A phone
app is a *bundle* (an APK on Android, a `.app` folder on iOS) that the
system launches through its own application framework, and native code is a
library inside it. SDL2 provides that framework glue for both systems, and
calls one C function when everything is ready: `SDL_main`.

```text
 Android                                      iOS
 ───────                                      ───
 tap the icon                                 tap the icon
   │ the Java VM (ART) creates                  │ the executable's main()
   │ io.github.bthuner.gase.GaseActivity        │ (ios/Sources/main.m)
   │ = SDL's SDLActivity + our glue             │ SDL_UIKitRunApp → UIApplicationMain,
   │ loads libSDL2.so, libmain.so               │ with SDL's app delegate
   │ starts the thread "SDLThread"              │ (our subclass: GaseAppDelegate.m)
   ▼ which calls through JNI                    ▼ after didFinishLaunching
 SDL_main(argc, argv)  in libmain.so          SDL_main(argc, argv), linked in
   └────────── crates/mobile/src/native.rs: the same Rust function ──────────┘
                                    │
               gase::sdl::run_sdl(Shell::Mobile(…)) — the desktop's main loop
```

* **Android.** Apps are Java (or Kotlin) programs running in a virtual
  machine. Native code comes as shared libraries (`.so`) in the APK, one
  set per processor family, called an **ABI**: `arm64-v8a` (nearly every
  phone), `armeabi-v7a` (old 32-bit phones), `x86_64` (the emulator,
  Chromebooks). Java calls C, and C calls Java, through **JNI** (the Java
  Native Interface): SDL's Java classes forward every touch, key, gamepad
  event, resize and lifecycle change to C functions in `libSDL2.so`, and
  SDL's C code calls Java to open the audio device or change the
  orientation. The Rust code is built as a `cdylib` named `libmain.so`;
  `SDLActivity.getLibraries()` loads `SDL2` then `main` and looks up
  `SDL_main` with `dlsym`.
* **iOS.** An app is one executable plus resources. Apps may not load
  their own dynamic libraries (except as embedded frameworks), so the Rust
  code is built as a `staticlib` and linked into the executable together
  with SDL2 (also static) and the Objective-C glue. `main` runs UIKit's
  event loop; SDL's app delegate calls `SDL_main` once the app has
  launched, on the main thread, and SDL keeps UIKit's run loop turning
  every time the Rust loop polls for events.

`crates/mobile` (`gase-mobile`) is that `SDL_main`. It is a normal Rust
library in the workspace (so it builds and is tested on any desktop); the
build scripts below turn it into a `cdylib` or a `staticlib` with
`cargo rustc --crate-type …`.

### Unsafe code, in one place

The workspace forbids `unsafe`. Exporting a function under a C name
(`#[unsafe(no_mangle)]`), reading C's `argv`, and declaring C functions
need it, so `gase-mobile` denies it instead and its module
`src/native.rs` allows it, with a justification for each use. The native
functions Rust calls are declared `safe fn` inside `unsafe extern "C"`:
the unsafety is in promising their signature, not in calling them.

| Rust calls | Implemented by | For |
|---|---|---|
| `SDL_AndroidSendMessage(0x8001, 0)` | SDL → `GaseActivity.onUnhandledMessage` on the UI thread | the "Open ROM…" button on Android |
| `SDL_AndroidBackButton()` | SDL → `Activity.onBackPressed` | Back on the home screen leaves the app |
| `gase_ios_pick_rom()` | `ios/Sources/GaseAppDelegate.m` | the "Open ROM…" button on iOS |

Everything coming back from native code uses an ordinary SDL event: a ROM
is a `SDL_DROPFILE` event with the path of a copy in the app's sandbox,
which the shell already turned into "open this ROM" for drag and drop on
the desktop.

### File sandboxes and document pickers

A phone app may only read and write its own folder (its *sandbox*);
other files are reachable only through the system's document picker, one
file at a time, with the user's consent. So:

* **Storage.** Settings, saves, save states and screenshots go to the
  folder `SDL_GetPrefPath` returns: `files/` of the app's internal storage
  on Android, `Library/Application Support/gase/` on iOS
  (`crates/gase/src/mobile.rs` has the details). Files are written to a
  temporary name and renamed, so a save survives the app being killed in
  the middle.
* **ROMs are copied in.** Android's picker (`ACTION_OPEN_DOCUMENT`, the
  Storage Access Framework) answers with a `content://` URI, not a path;
  iOS's (`UIDocumentPickerViewController`) with a temporary copy. The glue
  copies the ROM into `files/roms/` (Android) or `Documents/roms/` (iOS) on
  a background thread and hands Rust the copy's path, so the recent list
  can open it again without asking. ZIP archives work like ROMs (the core
  unpacks them).
* **"Open with" / "Share".** `AndroidManifest.xml` declares `VIEW` and
  `SEND` intent filters for generic binaries and ZIP archives (Android
  knows no MIME type for Mega Drive ROMs); `Info.plist` declares the `.md`,
  `.gen`, `.smd`, `.bin` extensions and ZIP archives (`CFBundleDocumentTypes`,
  `UTImportedTypeDeclarations`). When the app is started by such a request,
  native code is not running yet, so Android passes the copied ROM as
  `argv[1]` (`GaseActivity.getArguments()` runs on SDL's thread, a fine
  place for slow I/O) and iOS retries the event until SDL is up.
* On iOS, `Documents` is visible in the Files app ("On My iPhone > gase")
  and in Finder (`UIFileSharingEnabled`): copy ROMs into `roms/` there.

### The lifecycle: background and foreground

A phone app does not decide when it stops. When the user switches apps, a
call comes in or the screen locks, the app goes to the *background*,
where it must not draw (iOS kills apps that use the GPU in the
background) and may be killed at any time to free memory. SDL turns the
system's callbacks (`onPause`/`onResume`, `applicationWillResignActive`/
`applicationDidBecomeActive` …) into events, and the shell reacts:

| SDL event | The shell |
|---|---|
| `SDL_APP_WILLENTERBACKGROUND` | `Event::Suspend`: writes the game's save and the settings at once, opens the pause menu; pauses the sound; stops drawing and emulating; naps 50 ms between looks at the events (no CPU used) |
| `SDL_APP_DIDENTERFOREGROUND` | drops the stale sound and resumes it, re-reads the screen size (it may have rotated); the game waits in the pause menu |
| `SDL_APP_TERMINATING` | writes everything and leaves |
| `SDL_RENDER_DEVICE_RESET` | recreates the GPU textures (Android may lose them in the background) |

On Android, SDL's activity makes sure the native thread has seen the
background event before it blocks it. On iOS the app delegate asks for a
moment of background time (`beginBackgroundTask`) so the Rust loop gets
to write the save before the app is suspended.

### Screen, touch, density, rotation

* The window is the whole screen, without status or navigation bars
  (Android's immersive mode), and follows the phone's rotation
  (`SDL_IOS_ORIENTATIONS`, read by both systems). The app has a portrait
  layout (picture at the top, controls below) and a landscape one
  (controls beside the picture).
* **Density.** UI sizes are in *points*. iOS reports windows in points
  and the drawable in pixels, so their ratio is the density (3 on most
  iPhones; `allow_highdpi` is needed or iOS renders at point resolution
  and blurs). SDL2 on Android has no points: the density is the display's
  DPI over Android's 160-DPI baseline (2.75 on a typical 1080 × 2340
  phone), the same value as Android's own `DisplayMetrics.density`.
* **Touch** arrives as SDL finger events with an id per finger, so the
  d-pad and the buttons work at the same time. Synthetic mouse events from
  touches are turned off (`SDL_TOUCH_MOUSE_EVENTS=0`).
* **Back** (Android button or gesture) is trapped
  (`SDL_ANDROID_TRAP_BACK_BUTTON`) and acts as Escape: it opens the pause
  menu and leaves menus; on the home screen it leaves the app.

### Gamepads

SDL's GameController API covers Bluetooth and USB pads on Android
(through Android's input system, plus SDL's own HID drivers) and MFi,
Xbox and PlayStation pads on iOS (through Apple's GameController
framework; `GCSupportsControllerUserInteraction` in `Info.plist`). Pads
can be connected at any time (hot-plug, the first is player 1). While a
pad is in use the on-screen controls step aside and the picture is
centred; touching the screen brings them back.

### Audio

SDL opens the system's audio path (AAudio or OpenSL ES on Android, Core
Audio with an `AVAudioSession` on iOS). The shell asks for 48 kHz stereo
with a 1024-frame buffer (21 ms; twice the desktop's, to ride out the
hiccups of phone power management), and the app's dynamic rate control
keeps the queue level, as on the desktop. On iOS the sound follows the
silent switch (SDL's default "solo ambient" audio session).

## Building for Android

### What you need

* Rust with the Android targets:
  `rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android`
* JDK 17 or newer.
* The Android SDK with platform 35 and **NDK 27.2.12479018** (Android
  Studio: Settings → Languages & Frameworks → Android SDK → SDK Tools →
  "NDK (Side by side)", tick "Show package details"), or on the command
  line: `sdkmanager "platforms;android-35" "ndk;27.2.12479018"`.
* CMake 3.16+ on the `PATH` (to build SDL2), and `curl`.
* Gradle 8.9 or newer (`gradle` on the `PATH`; the repository has no Gradle
  wrapper jar). Android Studio brings its own.
* Linux or macOS (on Windows, use WSL): the native build is a shell
  script.

Set `ANDROID_HOME` (or write `sdk.dir=/path/to/sdk` into
`mobile/android/local.properties`).

### Build and install

```sh
cd mobile/android
gradle assembleDebug                          # app/build/outputs/apk/debug/app-debug.apk
gradle installDebug                           # onto the phone or emulator adb sees
gradle assembleDebug -PgaseAbis=arm64-v8a     # only one ABI: much faster while developing
```

Or open `mobile/android` in Android Studio and press Run.

Gradle's `buildNative` task (`app/build.gradle`) runs
`build-native.sh` before packaging, which:

1. downloads the SDL2 2.32.10 source release and checks its SHA-256
   (`mobile/fetch-sdl.sh`; nothing of SDL is in the repository, the
   download is kept in `mobile/build/`);
2. builds `libSDL2.so` per ABI with the NDK's CMake toolchain
   (`ANDROID_STL=c++_static`, flexible page sizes for 16 KB-page devices);
3. builds the Rust code per ABI — this is all `cargo-ndk` would do:

   ```sh
   CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android21-clang \
   CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS="-L native=<libSDL2.so's folder> -C link-arg=-Wl,-z,max-page-size=16384" \
   cargo rustc -p gase-mobile --release --target aarch64-linux-android --crate-type cdylib
   ```

   and copies `libgase_mobile.so` as `libmain.so`.

Gradle also compiles SDL's Java classes straight from the downloaded
release (`android-project/app/src/main/java`), so Java and native SDL
always have the same version. The Rust code is always built optimised
(`CARGO_PROFILE=release`), also for debug APKs: an emulator built without
optimisation is far too slow to play.

`gradle assembleRelease` makes an APK signed with the debug key
(installable, not publishable); for a store, configure your own
`signingConfig`.

### Debugging

```sh
adb logcat -s SDL SDL/APP gase      # the app's messages, SDL's, and panics
adb shell run-as io.github.bthuner.gase ls -R files   # the sandbox: settings, saves, roms
```

## Building for iOS

### What you need

* A Mac with Xcode 15 or newer (Apple silicon for the simulator; see
  below for Intel).
* Rust with the iOS targets: `rustup target add aarch64-apple-ios aarch64-apple-ios-sim`
* XcodeGen: `brew install xcodegen`.

### Build

```sh
mobile/ios/build.sh
# → mobile/build/ios/app/Release-iphonesimulator/gase.app
# → mobile/build/ios/app/Release-iphoneos/gase.app   (unsigned)
```

`build.sh` runs, in order:

1. `build-sdl.sh`: SDL2's own Xcode project (`Xcode/SDL/SDL.xcodeproj`,
   target "Static Library-iOS") builds `libSDL2.a` for the device and the
   simulator;
2. `xcodegen generate`: makes `Gase.xcodeproj` from `project.yml` (the
   project file is generated, not kept in git);
3. `xcodebuild` for each platform, without code signing. The Xcode project
   runs `build-rust.sh` before compiling, which builds the Rust staticlib
   (`cargo rustc -p gase-mobile --target aarch64-apple-ios[-sim]
   --crate-type staticlib`).

### Run in the simulator

```sh
xcrun simctl boot "iPhone 16"   # any simulator from `xcrun simctl list devices`
open -a Simulator
xcrun simctl install booted mobile/build/ios/app/Release-iphonesimulator/gase.app
xcrun simctl launch --console booted io.github.bthuner.gase
```

Drag a ROM onto the simulator window to "Open in" gase, or copy one into
the app's `Documents/roms` (`xcrun simctl get_app_container booted
io.github.bthuner.gase data`).

### Run on an iPhone or iPad

Devices only run signed apps. Run `build-sdl.sh` once and `xcodegen
generate`, open `mobile/ios/Gase.xcodeproj`, choose your team under
Signing & Capabilities (a free Apple ID works for your own devices; change
the bundle identifier if Xcode says it is taken), plug in the device and
press Run. The Xcode console shows the app's and SDL's messages.

## Trying the phone shell on a desktop

The phone shell runs on a desktop too (it is the same code), which is the
fastest way to see a change:

```sh
gase --mobile --ui-size 540x1170 game.md        # portrait phone; 1170x540 for landscape
gase --headless --mobile --ui-size 1080x2340 --ui-density 2.75 --dump-ui phone.png
gase --headless --mobile --ui-screen game --frames 120 --ui-size 2340x1080 --ui-density 2.75 --dump-ui landscape.png game.md
```

`--mobile` uses the phone capabilities (document picker instead of the
file browser, no Quit, touch controls) and the phone storage layout, in
SDL's data folder for the desktop (`~/.local/share/gase/` on Linux).
There is no picker on a desktop, so open ROMs from the command line or
the recent list. An SDL2 built with
`-DSDL_BACKGROUNDING_SIGNAL=10 -DSDL_FOREGROUNDING_SIGNAL=12` (from the
same source release) even lets you send the lifecycle events with
`kill -USR1` / `kill -USR2`.

## What is tested where

| Check | Where |
|---|---|
| The Rust side: shell, phone storage, lifecycle mapping, density, Back, the gamepad-hides-touch rule, argument parsing (unit and app tests) | `cargo test --workspace`, any desktop, CI |
| `gase-mobile` type-checks and passes clippy for all five phone targets (with the Android- and iOS-only code) | `mobile.yml` job "Rust for phone targets"; also locally, no SDK needed |
| The phone shell's main loop, with phone layouts in portrait and landscape, runs under SDL's dummy drivers | `gase --mobile` locally (`scripts/smoke-sdl.sh` on an optimised build) |
| The background/foreground path with real SDL application events | locally, with SDL built with backgrounding signals |
| `libSDL2.so` builds with the NDK; `libmain.so` links; the APK contains both for three ABIs; `SDL_main` is exported; 16 KB alignment | `mobile.yml` job "Android APK" |
| `libSDL2.a` builds with SDL's Xcode project; the app links and is packaged for the simulator and devices | `mobile.yml` job "iOS app" |
| Running on a phone, emulator or simulator: touch, picker, Open with, gamepads, rotation, sound | by hand (see above) |

## Troubleshooting

* **`NDK not configured` / `ANDROID_NDK_HOME`**: install NDK
  27.2.12479018 (above), or change `ndkVersion` in `app/build.gradle` to
  one you have (r26 and later work; flexible page sizes need r27).
* **`linker ... -clang not found`**: the NDK has no prebuilt toolchain for
  your host; build on Linux or macOS.
* **`cannot find -lSDL2`** while linking `libmain.so`: SDL2's CMake build
  for that ABI failed earlier; scroll up, often a missing CMake.
* **The app closes at once on Android**: `adb logcat -s SDL SDL/APP`.
  `dlopen failed: library "libmain.so" not found` means the native build
  did not run (Gradle's `buildNative`); `cannot locate symbol` means a
  `libSDL2.so` and `libmain.so` from different builds, run `gradle clean`.
* **SHA-256 mismatch** in `fetch-sdl.sh`: the download is damaged or
  someone changed the version without the checksum; delete `mobile/build/`
  and try again.
* **iOS: `building for iOS Simulator, but linking in object file built for
  iOS`**: a stale library in `mobile/build/ios/`; delete that folder.
* **iOS: `ld: library 'SDL2' not found`**: run `mobile/ios/build-sdl.sh`
  first (Xcode only builds the Rust part itself).
* **iOS simulator on an Intel Mac**: add `x86_64` to `ARCHS` in
  `project.yml` (and remove it from `EXCLUDED_ARCHS`), `rustup target add
  x86_64-apple-ios`, and build SDL with `ARCHS="arm64 x86_64"`.
* **Cargo not found from Xcode**: `build-rust.sh` adds `~/.cargo/bin` to
  `PATH`; with another Rust installation, edit that line.

## Limitations

* No safe-area handling: SDL2 has no API for display cutouts, so on
  phones with a notch or rounded corners, controls near the edges may sit
  partly under them (the layouts keep a margin).
* Screenshots are stored in the app's sandbox (Android) or its
  Application Support folder (iOS), not in the photo gallery.
* No rumble, no on-screen keyboard (gase needs neither), no Android TV
  banner artwork (the icon is used).
* A ROM with the same file name as another replaces its copy in `roms/`,
  and saves are named after the ROM's file name.
* The APK built here is signed with the debug key and the iOS app is not
  signed; publishing needs your own keys and store metadata.
