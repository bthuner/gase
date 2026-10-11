# Changelog

All notable changes to gase are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[semantic versioning](https://semver.org/).

## [Unreleased]

## [0.3.1] - 2026-10-11

### Changed
- **The Android project brings its own Gradle**: a Gradle wrapper
  (`mobile/android/gradlew`) pinned to Gradle 8.10.2, with its SHA-256.
  Build with `./gradlew` instead of `gradle`: the installed Gradle, any
  version or none, no longer matters (Gradle 9 cannot run the Android
  Gradle plugin 8.7). CI builds with the wrapper too.

## [0.3.0] - 2026-10-11

Natively recompiled games: gase becomes the shell for games that
[gase-recomp](https://github.com/bthuner/gase-recomp) turned into native
code, on the desktop and on phones, and for its JIT. Save states from 0.2.0
still load (the format is unchanged).

### Added
- **Recompile to native…** in the pause menu, on desktops where
  gase-recomp's studio is installed (`gase-recomp-studio` on the `PATH`,
  or `GASE_RECOMP_STUDIO`): it opens the studio set to build the game for
  this computer. `Capabilities::recompile` and `Request::Recompile` carry
  it; phones and the browser never offer it.
- `external::register_any`: one external CPU for every ROM that has none
  of its own, such as gase-recomp's JIT. A ROM's own CPU (a statically
  recompiled game) still wins. The Android and iOS build scripts build the
  crate's library only (`--lib`), so a game crate may also have programs.
- **Statically recompiled games** (`gase_core::system::external`): a
  console can be driven by an external 68000, such as a game recompiled to
  native code by [gase-recomp](https://github.com/bthuner/gase-recomp).
  The external CPU runs freely and the console catches up at its poll
  points (an inverted form of the same scheduler, checked against the
  normal one by running the interpreter through it). The interpreter's
  state stays the CPU state between frames, so save states, rewind and the
  debugger keep working. A recompiled game registers its code for its
  ROM's CRC-32 and calls `gase::run_command_line`: every frontend then runs
  it like any other game.
- **Make an app of this game…** on phones, for apps that bring a way to
  build one: `gase_mobile::run_with` takes `Hooks { recompile }`, and
  `gase::sdl::Notices` carries toasts from any thread.
  `gase_mobile::install_apk` hands an APK to Android's package installer
  (`GaseActivity` grows a `PackageInstaller` session); apps built with
  `-PgasePhoneBuilds` ask for the `INTERNET` and
  `REQUEST_INSTALL_PACKAGES` permissions, gase's own app still asks for
  none. `ExternalCpu::ahead_of_time` tells a JIT from a recompiled game, so
  a game run by a JIT can still be recompiled.
- **Recompiled games on phones**: `gase-mobile` gets an `sdl-main` feature
  (on by default) and `run_game`, so a game crate can export its own
  `SDL_main`, register its code and ship its ROM inside the app. The
  Android and iOS builds take the crate to build (`-PgaseCrate`,
  `GASE_MOBILE_MANIFEST`/`GASE_MOBILE_PACKAGE`) and the app's name and
  identifier (`-PgaseAppId`/`-PgaseAppName`, `GASE_APP_ID`/`GASE_APP_NAME`),
  so the game installs as its own app next to gase.
- Release files for every platform: a `Release` workflow builds Linux,
  Windows and macOS (universal) programs with SDL2 linked in, the browser
  version, the Android APK and an unsigned iOS `.ipa`, and attaches them
  with checksums to a GitHub release. It runs when a release is published,
  or by hand to create a release (tag included) or refill an existing one.
  Older versions get the platforms they have (0.1.0: desktop only).

## [0.2.0] - 2026-10-10

Quality of life: menus and settings, every platform, ZIP files. Save
states from 0.1.0 still load (the format is unchanged).

### Added
- **A user interface** (`gase-app`, new crate, no dependencies): a home
  screen with recent games, a built-in file browser (*Open ROM…*), drag
  and drop, an in-game menu (resume, save and load states with picture
  previews in ten slots, reset, settings, close), and settings for the
  picture, sound, console and controls, saved to a plain `key = value`
  file (`settings.cfg`, `$GASE_CONFIG_DIR` overrides its folder). Drawn in
  software with its own bitmap font, so every platform shows the same
  interface.
- **Controls everywhere**: keyboard, any number of gamepads (hot-plugged)
  and multi-touch on-screen controls, all mapped to the console pads at
  once; 3- or 6-button pads per player; remapping of every key and pad
  button for two players. With touch controls on *Auto*, using a gamepad
  hides them.
- **ZIP ROMs** (`gase-zip`, new crate): `.zip` files open directly on
  every platform, with an own DEFLATE decoder and CRC-32 check (readable
  reference plus tested fast path). `game.zip` saves to `game.srm`.
- **In the browser** (`gase-web` and `web/`): the emulator and interface
  compiled to WebAssembly without wasm-bindgen or a bundler; keyboard,
  Gamepad API and touch; saves and settings in IndexedDB; an installable,
  offline-capable PWA; a one-click download of the 240p Test Suite,
  checked by SHA-256. About 169 KB gzipped.
- **Android and iOS apps** (`gase-mobile` and `mobile/`): the desktop's
  SDL2 shell on phones, with the system document picker, "Open with",
  rotation, and saves written as soon as the app goes to the background.
  CI builds an Android APK and an unsigned iOS app; not yet run on a
  device.
- `--mobile` runs the phone shell in a desktop window; `--touch` shows
  the touch controls; `--dump-ui`, `--ui-screen`, `--ui-size` and
  `--ui-density` save pictures of any screen without a window.
- **The making of gase** (`docs/index.html`): a 19-chapter static site
  telling how the emulator was built, with pictures made by the emulator
  (`scripts/docs-screenshots.sh`). The `Web` workflow can publish it on
  GitHub Pages with the player under `play/` (opt-in: repository variable
  `DEPLOY_PAGES=true`).

### Changed
- **Esc opens the in-game menu** instead of quitting. *Close game* in
  the menu returns to the home screen, which has *Quit*; closing the
  window still quits at once.
- Running `gase` without a ROM opens the home screen instead of exiting
  with "no ROM given".
- The `gase` crate is now a library (the SDL2 shell shared by desktop and
  phones) plus a thin binary.
- `unsafe` is still forbidden everywhere except two modules that must
  name foreign functions, where it is denied and allowed once each: the
  web module's exports/imports (no `unsafe` block) and the phone entry
  point `SDL_main` (two blocks reading the C `argv`).

### Fixed
- A crash of optimised builds as soon as the window opened: `sdl2` 0.37's
  `Texture::with_lock` with a rectangle passes SDL a pointer to a dropped
  temporary. gase now always locks the whole texture.
- In landscape, the touch controls' Start and Mode buttons were drawn
  over the game picture; they now sit in the side borders.

### Quality
- CI also runs clippy for WebAssembly and the five phone targets, and
  smoke-runs the optimised SDL program with dummy drivers
  (`scripts/smoke-sdl.sh`). New workflows build the web version and the
  phone apps.
- 331 tests in `cargo test --workspace` (200 in 0.1.0).

## [0.1.0] - 2026-10-10

The first release: a complete, tested Mega Drive / Genesis emulator.

### Emulation
- **Motorola 68000** (`gase-m68k`): full instruction set with exact cycle
  timing, 2-word prefetch queue, microcode-accurate address-error frames,
  interrupts, trace, STOP/RESET/TAS, pre-decoded dispatch table and a
  Motorola-syntax disassembler. Validated against the MAME-derived
  SingleStepTests suite (310,000 vectors) and Tom Harte's suite.
- **Zilog Z80** (`gase-z80`): full documented and undocumented behaviour
  (WZ/MEMPTR, Q, X/Y flags), interrupt modes 0/1/2, disassembler. Passes all
  1,604,000 SingleStepTests vectors, ZEXDOC and ZEXALL.
- **VDP** (`gase-vdp`): planes, window, sprites (limits, masking,
  collision), shadow/highlight, interlace mode 2, HV counter, interrupts,
  4-entry write FIFO, per-line access slots and time-accurate DMA.
- **Sound** (`gase-sound`): YM2612 (log-sin/exp tables, envelope
  generator, all algorithms, SSG-EG, LFO, CSM, DAC, ladder effect), SN76489
  PSG, polyphase sinc resampler, model-1 low-pass filter, DC blocker.
- **System** (`gase-core`): memory maps for both CPUs, master-clock
  scheduler, cartridges (header, `.smd`, battery SRAM, serial EEPROM for
  Sega/Acclaim/EA/Codemasters boards, SSF2 mapper), 3- and 6-button pads,
  NTSC/PAL regions, save states, rewind.
- Optional lenient mode for 68000 address errors (`--no-address-errors`)
  for buggy homebrew.

### Frontend
- SDL2 window: aspect-correct scaling, fullscreen, gamepads, save-state
  slots, rewind, fast-forward, pause/frame advance, screenshots, battery-save
  autosave, audio-driven pacing with dynamic rate control.
- Built-in learner debugger: CPU registers and disassembly, decoded VDP
  registers, palettes, tile/plane viewers, sprite list, breakpoints,
  stepping, per-channel audio mutes.
- Headless runner without any external dependency: benchmarks, PNG
  screenshots, WAV recording, instruction traces, VRAM/CRAM dumps,
  breakpoints.

### Performance
- 1.5–1.9× faster than the first working version with bit-identical
  output; about 25× real time on one core. Every fast path keeps a readable
  reference implementation and a test proving they match. Measurements and
  scripts live in `benchmarks/`.

### Quality
- Zero external dependencies outside the optional SDL2 frontend,
  `forbid(unsafe_code)`, clippy-clean, documented crates
  (`docs/ARCHITECTURE.md`).
- CI on every pull request (fmt, clippy, tests, docs, dependency-free
  build) and weekly runs of the CPU test vectors and the test-ROM frame-hash
  regression suite.

[0.3.1]: https://github.com/bthuner/gase/releases/tag/v0.3.1
[0.3.0]: https://github.com/bthuner/gase/releases/tag/v0.3.0
[0.2.0]: https://github.com/bthuner/gase/releases/tag/v0.2.0
[0.1.0]: https://github.com/bthuner/gase/releases/tag/v0.1.0
