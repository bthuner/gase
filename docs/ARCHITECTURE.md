# How gase works

This guide walks through the emulator from the outside in. Each section
points to the module whose documentation goes deeper.

## 1. The big picture

```text
                      ┌───────────── gase (frontend) ─────────────┐
   keyboard/pads ───► │ Session: ROM, SRAM, save states, rewind   │ ───► window, speakers
                      └──────────────────┬────────────────────────┘
                                         │ run_frame(), frame(), drain_audio()
                      ┌──────────────────▼──────────────── gase-core ┐
                      │ Genesis                                       │
                      │   ├─ M68k (gase-m68k)  ──┐                     │
                      │   ├─ Z80  (gase-z80)   ──┤ Bus traits          │
                      │   └─ Hardware ◄──────────┘                     │
                      │        ├─ Cartridge (ROM, SRAM, mapper)        │
                      │        ├─ 64 KiB RAM, 8 KiB Z80 RAM            │
                      │        ├─ Vdp (gase-vdp)                        │
                      │        ├─ Ym2612, Psg (gase-sound)              │
                      │        └─ Io (controllers, region)              │
                      └───────────────────────────────────────────────┘
```

The core is a library with no I/O: it takes ROM bytes and button states and
returns pixels and audio samples. That keeps it testable (see
`crates/core/tests/smoke.rs`, which runs hand-assembled programs) and
portable to any frontend.

## 2. The CPUs and their buses

Each CPU crate defines a small `Bus` trait describing what the CPU needs
from the outside world:

```rust
// gase-m68k
pub trait Bus {
    fn read_byte(&mut self, addr: u32) -> u8;
    fn read_word(&mut self, addr: u32) -> u16;
    fn write_byte(&mut self, addr: u32, value: u8);
    fn write_word(&mut self, addr: u32, value: u16);
    // ...
}
```

`M68k::step<B: Bus>(&mut self, bus: &mut B)` is generic, so the compiler
generates a version specialised for the real `Hardware` type: memory
accesses are direct calls, often inlined. Tests use a different bus (a flat
RAM array) with the same CPU code.

The CPUs are kept **outside** `Hardware` on purpose. While the 68000 runs,
it holds `&mut Hardware`; if the CPU were inside `Hardware` that borrow
would be impossible in safe Rust. The Z80 sees the same hardware through a
thin wrapper, `Z80Bus<'a>(&'a mut Hardware)`, which implements the Z80's
`Bus` trait with the Z80's memory map (`crates/core/src/bus.rs`).

## 3. Time

Everything is derived from one crystal: the **master clock** (53.69 MHz on
NTSC consoles).

| Chip | Divider | Unit used in code |
|---|---|---|
| 68000 | ÷7 | CPU cycles × 7 = master clocks |
| Z80 | ÷15 | T-states × 15 |
| VDP | 3420 per scanline | lines |
| YM2612 | ÷7, one sample per 144 cycles | 1008 master clocks per sample |
| PSG | ÷15, ticks every 16 cycles | 240 master clocks per tick |

`Genesis::run_frame` (`crates/core/src/system.rs`) loops over scanlines. In
each line it:

1. asks the VDP to render the line,
2. runs the 68000 instruction by instruction up to the next event
   (vertical interrupt, horizontal blank, end of line), and after each
   instruction lets the Z80 catch up to the same master-clock time,
3. raises interrupts at the right moments.

Sound chips are updated **lazily**: they only run when a CPU writes to them
(so the write lands at the correct sample) or at the end of the frame. This
"catch-up" approach costs almost nothing when the sound chips are idle.

## 4. Video

The VDP (`crates/vdp`) is driven entirely through two ports: a control port
for registers and addresses and a data port for memory contents. Games
mostly use **DMA** to copy graphics from the 68000's memory into VRAM.
Because only the system bus can read 68000 memory, the VDP flags a pending
DMA and the scheduler performs it, charging the 68000 for the time it is
frozen.

Rendering happens one scanline at a time into a 32-bit frame buffer.
Each line composites plane B, plane A/window and sprites by priority —
see the module docs in `crates/vdp/src/render.rs` for the exact rules,
including shadow/highlight mode.

## 5. Audio

The YM2612 and PSG produce ~53 kHz samples which are mixed, optionally
low-pass filtered (the model 1 console's analogue filter) and resampled to
the host rate (48 kHz). The frontend lets the audio device pace the
emulator and nudges the resampling ratio by up to ±0.5% to keep its queue
from drifting (dynamic rate control) — inaudible, and it avoids crackles
without dropping or repeating frames.

## 6. Save states and rewind

Every component implements `gase_savestate::State`: `save` writes its fields
in a fixed order, `load` reads them back. The format is plain little-endian
binary with a magic number, a version and a fingerprint of the game. Rewind
(`crates/core/src/rewind.rs`) is simply a ring buffer of save states taken
every few frames. Loading is transactional: a damaged state leaves the
console untouched.

## 7. Correctness

* The CPU cores are checked against the TomHarte / SingleStepTests suites:
  hundreds of thousands of single-instruction tests each, comparing every
  register, memory byte and cycle count.
* Chips have unit tests for their documented behaviours.
* The core has whole-system tests using tiny hand-assembled programs.
* CI runs formatting, clippy, all tests and a dependency-free build on every
  pull request, and the big vector suites weekly.

## 8. Using the debugger to learn

The debugger (F1 in the window, see the README for its keys) shows the
console's state as the chips see it. A few experiments that make the
sections above concrete:

* **Follow the boot.** Start with `gase --debug game.bin` and press `S`
  repeatedly: the first instructions of most games read the console's
  version register (`A10001`), set up the VDP registers (watch the decoded
  register list change) and clear RAM.
* **Find the VBlank handler.** Press `V` to run until the VDP raises the
  vertical interrupt, then `S`: the 68000 takes the level-6 interrupt and
  the listing jumps to the handler whose address is stored at `$000078`.
  Most games do all their VRAM updates (DMA from RAM) here.
* **See DMA at work.** Set a breakpoint (`B`) on an instruction that writes
  the VDP control port, step over it and watch tiles appear in the VRAM view.
* **Read a frame apart.** Switch the picture view (`T`) between the tiles,
  plane A, plane B and the window; compare with the sprite list to see how
  the final picture is composited (section 4). Changing the tile palette
  (`[` `]`) shows which palette a group of tiles was drawn for.
* **Hear the channels.** Mute FM channels (`1`-`6`) and the PSG (`7`-`0`)
  one by one to hear how the music is arranged across the chips.

How it works: `gase_core::debug::Debugger` runs the console with a resumable
copy of the frame loop of section 3, so it can stop after any instruction
and continue later, while `Genesis::run_frame` itself never looks at
breakpoints. Memory is inspected with `Genesis::peek_*`, which never touch
I/O registers (reading those has side effects). The windows are drawn into
plain pixel buffers with a public-domain 8×8 font
(`crates/gase/src/debugger/`), so the same views can be saved as PNG from
the headless runner (`--dump-vram`, `--dump-cram`, `--dump-debugger`,
`--break`).

## Where to start reading

1. `crates/core/src/system.rs` — the main loop.
2. `crates/core/src/bus.rs` — the memory maps.
3. `crates/vdp/src/lib.rs` — what the VDP is.
4. `crates/m68k/src/lib.rs` and `crates/z80/src/lib.rs` — the CPUs.
5. `crates/sound/src/lib.rs` — FM synthesis.
