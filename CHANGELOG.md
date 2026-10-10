# Changelog

All notable changes to gase are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[semantic versioning](https://semver.org/).

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

[0.1.0]: https://github.com/bthuner/gase/releases/tag/v0.1.0
