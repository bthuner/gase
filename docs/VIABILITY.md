# Is a Mega Drive emulator in Rust viable?

Short answer: **yes**, and Rust is a very good fit. This note records why, what
the hard parts are, and the decisions that follow from the project goals
(in priority order: pedagogy, performance, compatibility, quality of life,
best practices with few dependencies).

## The machine

| Part | Chip | Clock (NTSC) | Role |
|---|---|---|---|
| Main CPU | Motorola 68000 | 53.693175 MHz / 7 ≈ 7.67 MHz | Runs the game |
| Sound CPU | Zilog Z80 | 53.693175 MHz / 15 ≈ 3.58 MHz | Drives the sound chips |
| Video | Sega 315-5313 "VDP" | master / 4 or / 5 per pixel | 2 tile planes, window, 80 sprites, 64 colours on screen out of 512 |
| FM sound | Yamaha YM2612 (OPN2) | master / 7 | 6 channels × 4 operators, DAC on channel 6 |
| PSG | TI SN76489 clone (in the VDP) | master / 15 | 3 square + 1 noise |
| RAM | 64 KiB 68k RAM, 8 KiB Z80 RAM, 64 KiB VRAM, 128 B CRAM, 80 B VSRAM | | |

Everything is driven from a single 53.693175 MHz master clock (50 Hz PAL:
53.203424 MHz). The emulator schedules every chip against that clock.

## Effort estimate (lines of Rust, order of magnitude)

| Component | Size | Difficulty | Validation |
|---|---|---|---|
| 68000 | ~3–4k | high (many addressing modes, exceptions, exact cycle counts) | TomHarte/SingleStepTests JSON vectors (~1M cases) |
| Z80 | ~2k | medium (undocumented flags, MEMPTR) | SingleStepTests JSON vectors, ZEXDOC/ZEXALL |
| VDP | ~1.5–2k | high (timing, DMA, sprite limits, interlace) | test ROMs, games |
| YM2612 | ~1k | high (accurate envelope/phase/LFO tables) | ear + reference captures |
| PSG | ~150 | low | |
| Bus / I/O / cart / mappers / SRAM | ~800 | low–medium | games |
| Frontend | ~600 | low | |

Total ≈ 10k lines for a solid, game-compatible emulator. That is well within
reach; the existence of several mature open emulators (Genesis Plus GX,
BlastEm, Exodus, clownmdemu) means the hardware is extremely well documented.

## Performance budget

A 7.67 MHz 68000 executes ~1–2 M instructions per second. A plain Rust
interpreter with a pre-decoded opcode table runs this at a few percent of one
modern core. The real costs are the VDP renderer (320×224 pixels × 60 fps,
fine) and the scheduler (how often chips synchronise). A scanline-based
scheduler with per-instruction cycle accounting comfortably runs at many
hundreds of frames per second. **Performance is not a risk**; we can spend it
on accuracy and on QoL features like rewind.

## Why Rust fits

* Bit manipulation and wrapping arithmetic are explicit (`wrapping_add`,
  `u16::from_be_bytes`, ...), which makes hardware behaviour visible in code.
* Generics with static dispatch (`fn step(&mut self, bus: &mut impl Bus)`) give
  zero-cost abstraction between CPU cores and the memory map — each core is a
  standalone, independently testable crate.
* `#![forbid(unsafe_code)]` is realistic: nothing here needs `unsafe`.
* Cargo workspaces make "one chip, one crate" natural, which is great for
  teaching: you can read the Z80 without knowing the VDP exists.

## Hard parts / risks

1. **68000 exactness** — mitigated by the JSON test vectors, run in CI.
2. **VDP timing tricks** (mid-line raster effects, DMA bandwidth, HV counter) —
   start with a scanline renderer (handles ~all games), keep the door open
   for finer granularity.
3. **YM2612 sound accuracy** — follow the well-documented die-shot-derived
   behaviour (logsin/exp tables, envelope generator rates).
4. **Copy-protection / odd mappers** (SSF2 mapper, EEPROM saves) — handled
   incrementally via a small mapper abstraction.
5. **Audio/video sync** — drive emulation from audio buffer fill level, with
   dynamic rate control to avoid crackles.

## Decisions

* **Workspace layout**: `savestate`, `m68k`, `z80`, `sound`, `core`, `gase`
  (frontend). Every crate except the frontend has **zero external
  dependencies**.
* **Frontend**: a single dependency, `sdl2`, which provides window, audio,
  keyboard *and* gamepads. It is behind a default feature; without it `gase`
  builds as a headless runner (screenshots, benchmarks, CI).
* **Scheduling**: master-clock based, scanline granularity for the VDP, CPUs
  interleaved within a line.
* **Save states**: tiny hand-written binary format (`gase-savestate`), also
  powering rewind.
* **Testing**: unit tests per chip, CPU test-vector harnesses (vectors are
  downloaded on demand, not committed), headless ROM smoke tests.
