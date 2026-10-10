# gase

Sega Mega Drive / Genesis emulator, rust flavour.

gase aims to be an emulator you can **learn from**: every chip lives in its
own small crate with documentation that explains how the hardware works, and
the whole emulator core has **no external dependencies**. It is also meant
to be fast, compatible and pleasant to use.

**[The making of gase](https://bthuner.github.io/gase/)** tells how it was
built, chip by chip, with pictures made by the emulator itself (the site's
source is [`docs/index.html`](docs/index.html)). Once the site is
published, **[gase runs in your browser](https://bthuner.github.io/gase/play/)**
too.

| Goal | How |
|---|---|
| Pedagogy | One crate per chip, module docs that teach the hardware, comments that explain *why* |
| Performance | Pre-decoded/static-dispatch CPU cores, scanline renderer, lazy audio catch-up |
| Compatibility | CPU cores validated against ~2.6 million test vectors, SRAM and serial EEPROM saves, SSF2 mapper, 6-button pads, PAL/NTSC |
| Quality of life | Menus for keyboard, gamepad, mouse and touch; save states with previews, rewind, fast-forward, remappable controls, gamepad hot-plug, dynamic audio rate control |
| Best practices | `forbid(unsafe_code)` (the web and phone entry points only *deny* it, in one documented module each), clippy-clean, CI, one external dependency (SDL2, optional) |

## Downloads

Every [release](https://github.com/bthuner/gase/releases) has ready-made
files: Linux, Windows and macOS programs (SDL2 built in, nothing to
install), the browser version, an Android APK and an unsigned iOS app,
plus checksums and the source code. The macOS program is not notarised:
after unpacking, run `xattr -d com.apple.quarantine gase` once. The APK is
signed with a debug key, and the iOS app must be signed (AltStore,
Sideloadly, Xcode) before an iPhone accepts it.

## Building

You need a Rust toolchain (1.85+) and, for the windowed frontend, the SDL2
development package (`libsdl2-dev` on Debian/Ubuntu, `sdl2` on Homebrew).

```sh
cargo build --release
./target/release/gase path/to/game.bin
```

The **Android and iOS apps** run the same SDL2 shell; see
[`mobile/README.md`](mobile/README.md) for how to build them (Android SDK +
NDK, or a Mac with Xcode) and how a Rust program becomes a phone app.
`gase --mobile --ui-size 540x1170` tries the phone app's shell on a desktop.

To build a program that does not need SDL2 installed where it runs, as
the releases do, let the `sdl2` crate compile the SDL source it ships
(needs CMake and a C compiler) and link it in:

```sh
cargo build --release -p gase --features sdl2/bundled,sdl2/static-link
```

Without SDL2, build the dependency-free headless runner:

```sh
cargo build --release -p gase --no-default-features
./target/release/gase --headless --frames 600 --screenshot shot.png game.bin
```

### In the browser

The same emulator and menus also run in a web page, compiled to
WebAssembly (no JavaScript framework, no npm, no wasm-bindgen):

```sh
rustup target add wasm32-unknown-unknown   # once
web/build.sh                               # → web/gase_web.wasm (~0.5 MB, ~160 KB gzipped)
python3 -m http.server -d web 8000         # then open http://localhost:8000/
```

Open a ROM (`.md`, `.bin`, `.gen`, `.smd` or `.zip`) with *Open ROM…* or
drop it on the page, or try the free 240p Test Suite from the link under
the home screen. Keyboard, gamepads (Gamepad API), mouse and multi-touch
on-screen controls all work. Games you opened, their saves, save states
and the settings stay in the browser's storage (IndexedDB); screenshots
(F12) are downloaded. The page is a **progressive web app**: in Chrome
on Android use *Install app* (or *Add to Home screen*), in Safari on iOS
*Share → Add to Home Screen*; it then opens full-screen and works
offline. Add `?stats` to the address for a speed meter.

`web/build.sh --out docs/play` assembles the playable site under the
project site for GitHub Pages; the `Web` workflow builds it on every push
and can publish it (opt-in, see `.github/workflows/web.yml`). The .wasm
is never committed. `web/tests/smoke.mjs` drives the page in a headless
Chromium (needs the `playwright` package; not part of CI yet). How the
shell works: [`crates/web/src/lib.rs`](crates/web/src/lib.rs) and section
12 of [ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Using it

```sh
gase                 # the home screen: open a ROM, recent games, settings
gase game.bin        # start a game directly
```

Without a ROM, gase opens its **home screen**: *Open ROM…* browses your
folders (type a letter to jump), recent games are one click away, and you
can also drop a ROM file onto the window. Everything works with the
keyboard, a gamepad, the mouse or a touch screen.

During a game, **Esc** (or a gamepad's Guide button, or Back+Start) opens
the **menu**: resume, save and load states (ten slots, with pictures),
reset, settings, close the game. **Settings** cover the picture (aspect
ratio, integer scaling, fullscreen, window size), sound (volume, mute,
the model 1 low-pass filter), the console (region, lenient address
errors, rewind, fast-forward speed) and controls: 3- or 6-button pads for
each player, **remapping** of every key and pad button for two players,
and on-screen touch controls (automatic on touch screens). Settings are
saved in `~/.config/gase/settings.cfg` (or `%APPDATA%\gase`, or
`~/Library/Application Support/gase`; `$GASE_CONFIG_DIR` overrides it), a
plain `key = value` file you can also edit by hand.

Keys during a game (the console buttons can be changed in the menu):

```text
Arrows        D-pad            Enter         Start
Z / X / C     A / B / C        A / S / D     X / Y / Z
Q             Mode             Esc           Menu
P             Pause            N             Next frame (while paused)
Tab (hold)    Fast forward     Backspace     Rewind (hold)
F5 / F8       Save / load state               F6 / F7    Previous / next slot
F9            Reset            F11           Fullscreen
F12           Screenshot       M             Mute
F1 or `       Debugger
```

In menus: arrows move, Enter selects, Left/Right change a setting, Esc
goes back.

Gamepads are detected automatically, also when plugged in later (first
pad = player 1, second = player 2). The defaults follow the buttons'
positions: the bottom row of a modern pad (X, A, B on an Xbox layout) is
the Mega Drive's A, B, C; LB, Y, RB are X, Y, Z; Start and Back are Start
and Mode; hold the left/right triggers to rewind/fast-forward. The D-pad
and left stick both steer.

### The debugger

F1 opens a second window showing the 68000 and Z80 registers and
disassembly, the VDP registers (decoded), the palettes, every tile in VRAM
(or plane A/B/window) and the sprite list. While it has the focus, its keys
are:

```text
Space / P     Pause / continue          S             Step one 68000 instruction
F / N         Run to the end of frame   V             Run to the next VBlank
Up / Down     Move the cursor           PgDn / Home   Next page / back to PC
B             Toggle breakpoint at cursor               C   Clear breakpoints
G             Go to an address (type hex, Enter)
T             Tiles / plane A / plane B / window        [ ]   Tile palette
, .           Scroll the sprite list
1-6           Mute FM channel 1-6       7 8 9 0       Mute PSG tone 1-3 / noise
Esc / F1      Close the debugger
```

The same tools work without a window, which is handy for scripts and CI:

```sh
# Stop at $000200, print registers and disassembly, save the tiles and palettes
gase --headless --break 200 --dump-vram vram.png --dump-cram cram.png game.bin
gase --headless --frames 120 --dump-debugger debugger.png game.bin
```

`gase --debug game.bin` starts paused with the debugger open.

ROMs can be plain images (`.bin`, `.md`, `.gen`), interleaved `.smd`
dumps, or either of them zipped: `gase game.zip` finds the ROM inside the
archive (gase reads ZIP files itself, with no extra library).

gase emulates the 68000 exactly, including *address errors*: a word access
at an odd address crashes the game, as on real hardware. A few homebrew
programs contain such bugs and only work in emulators that ignore them; run
those with `--no-address-errors`.

Game saves (`game.srm`, battery SRAM or EEPROM), save states
(`game.state0`..`9`, with `.png` previews) and screenshots are stored next
to the ROM, named after it (`game.zip` also saves to `game.srm`). Run `gase --help` for all options.

The headless runner can also take pictures of the interface, which is how
the screenshots in the docs are made, and how to check a layout at a
phone's size from a desktop:

```sh
gase --headless --dump-ui home.png
gase --headless --frames 300 --ui-screen pause,settings --dump-ui menu.png game.bin
gase --headless --frames 300 --ui-screen game --touch --ui-size 1080x2340 --ui-density 3 --dump-ui phone.png game.bin
```

## Layout

```text
crates/
  savestate/  tiny binary serialisation used by save states and rewind
  m68k/       Motorola 68000 (main CPU)
  z80/        Zilog Z80 (sound CPU)
  vdp/        315-5313 video display processor
  sound/      YM2612 FM synthesiser, SN76489 PSG, resampler
  zip/        ZIP archives and DEFLATE decompression, for zipped ROMs
  core/       the console: memory maps, cartridge, controllers, scheduler
  app/        the user interface for every platform: menus, settings, input
              mapping, touch controls, drawn in software (no dependencies)
  gase/       the SDL2 shell around app/ (desktop and phones, a library) and
              the desktop binary, or headless
  mobile/     the native entry point of the Android and iOS apps (SDL_main)
  web/        the browser shell: app/ as a WebAssembly module (gase-web)
mobile/
  android/    the Android app: Gradle project, GaseActivity.java
  ios/        the iOS app: XcodeGen project, Objective-C app delegate
  README.md   building and installing the phone apps
web/          the page around it: plain HTML, CSS and JavaScript modules,
              audio worklet, service worker; build.sh builds the module
docs/
  VIABILITY.md     why Rust, effort and risk analysis
  ARCHITECTURE.md  how the pieces fit together — start here to learn
  index.html       the project site: the making of gase
```

## Testing

```sh
cargo test --workspace                 # fast unit and integration tests

# CPU validation against the TomHarte / SingleStepTests JSON vectors
scripts/fetch-m68k-tests.sh
scripts/fetch-z80-tests.sh
cargo test -p gase-m68k -p gase-z80 --profile fast-test -- --ignored
```

## Benchmarks

```sh
scripts/fetch-test-roms.sh
benchmarks/run.sh            # frames per second over the test ROMs (medians)
benchmarks/callgrind.sh      # instruction counts, for profiling
```

See [`benchmarks/README.md`](benchmarks/README.md) for how to compare
versions and read profiles, and `benchmarks/results/` for measurements.

## The project site

[`docs/`](docs/index.html) is a static site (plain HTML, CSS and a little
JavaScript, no build step) that GitHub Pages can serve from the `docs/`
folder. Every picture of the emulator on it is generated, so after a change
that affects them, regenerate them and check the links:

```sh
scripts/fetch-test-roms.sh            # once
scripts/docs-screenshots.sh           # all pictures -> docs/assets/img/
scripts/check-docs-links.py           # no broken relative links
```

`scripts/docs-screenshots.sh` uses the headless release build
(`--screenshot`, `--dump-debugger`, `--dump-ui`, `--trace`, `--wav`) and a
small helper outside the workspace, `scripts/docs-shots`, for scripted
button presses and pictures of the VDP's individual layers. The picture of
the browser version needs Node.js and the `playwright` package (found
through `NODE_PATH`); without them that one step is skipped. The site's
"Play" links point to `play/`, which the `Web` workflow builds at deploy
time (`web/build.sh --out docs/play` locally).

## Contributing

Work happens on `feature/…` and `fix/…` branches, merged into `develop`
through pull requests; `main` follows releases. Publishing a release on
GitHub (or running the `Release` workflow by hand) builds every platform's
files and attaches them; see `.github/workflows/release.yml`.

## License

MIT
