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
| Compatibility | CPU cores validated against ~2.6 million test vectors, SRAM, SSF2 mapper, 6-button pads, PAL/NTSC |
| Quality of life | Save states, rewind, fast-forward, screenshots, gamepads, dynamic audio rate control |
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

```text
Arrows        D-pad            Enter         Start
Z / X / C     A / B / C        A / S / D     X / Y / Z
Q             Mode
P             Pause            N             Next frame (while paused)
Tab (hold)    Fast forward     Backspace     Rewind (hold)
F5 / F8       Save / load state               F6 / F7    Previous / next slot
F9            Reset            F11           Fullscreen
F12           Screenshot       M             Mute
F1 or `       Debugger         Esc           Quit
```

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

Game controllers are detected automatically (first controller = player 1).
Battery saves (`game.srm`), save states (`game.state0`..`9`) and screenshots
are stored next to the ROM. Run `gase --help` for all options.

## Layout

```text
crates/
  savestate/  tiny binary serialisation used by save states and rewind
  m68k/       Motorola 68000 (main CPU)
  z80/        Zilog Z80 (sound CPU)
  vdp/        315-5313 video display processor
  sound/      YM2612 FM synthesiser, SN76489 PSG, resampler
  core/       the console: memory maps, cartridge, controllers, scheduler
  gase/       the frontend binary (SDL2 window or headless)
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

## Contributing

Work happens on `feature/…` and `fix/…` branches, merged into `develop`
through pull requests; `main` follows releases.

## License

MIT
