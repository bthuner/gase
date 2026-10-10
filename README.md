# gase

Sega Mega Drive / Genesis emulator, rust flavour.

gase aims to be an emulator you can **learn from**: every chip lives in its
own small crate with documentation that explains how the hardware works, and
the whole emulator core has **no external dependencies**. It is also meant
to be fast, compatible and pleasant to use.

| Goal | How |
|---|---|
| Pedagogy | One crate per chip, module docs that teach the hardware, comments that explain *why* |
| Performance | Pre-decoded/static-dispatch CPU cores, scanline renderer, lazy audio catch-up |
| Compatibility | CPU cores validated against ~2.6 million test vectors, SRAM and serial EEPROM saves, SSF2 mapper, 6-button pads, PAL/NTSC |
| Quality of life | Menus for keyboard, gamepad, mouse and touch; save states with previews, rewind, fast-forward, remappable controls, gamepad hot-plug, dynamic audio rate control |
| Best practices | `forbid(unsafe_code)`, clippy-clean, CI, one external dependency (SDL2, optional) |

## Building

You need a Rust toolchain (1.85+) and, for the windowed frontend, the SDL2
development package (`libsdl2-dev` on Debian/Ubuntu, `sdl2` on Homebrew).

```sh
cargo build --release
./target/release/gase path/to/game.bin
```

Without SDL2, build the dependency-free headless runner:

```sh
cargo build --release -p gase --no-default-features
./target/release/gase --headless --frames 600 --screenshot shot.png game.bin
```

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

gase emulates the 68000 exactly, including *address errors*: a word access
at an odd address crashes the game, as on real hardware. A few homebrew
programs contain such bugs and only work in emulators that ignore them; run
those with `--no-address-errors`.

Game saves (`game.srm`, battery SRAM or EEPROM), save states
(`game.state0`..`9`, with `.png` previews) and screenshots are stored next
to the ROM. Run `gase --help` for all options.

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
  core/       the console: memory maps, cartridge, controllers, scheduler
  app/        the user interface for every platform: menus, settings, input
              mapping, touch controls, drawn in software (no dependencies)
  gase/       the desktop binary: an SDL2 shell around app/, or headless
docs/
  VIABILITY.md     why Rust, effort and risk analysis
  ARCHITECTURE.md  how the pieces fit together — start here to learn
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

## Contributing

Work happens on `feature/…` and `fix/…` branches, merged into `develop`
through pull requests; `main` follows releases.

## License

MIT
