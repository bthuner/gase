# How gase works

This guide walks through the emulator from the outside in. Each section
points to the module whose documentation goes deeper.

## 1. The big picture

```text
                      ┌───────────── gase (frontend) ─────────────┐
   keyboard/pads ───► │ Session: ROM, .srm saves, states, rewind  │ ───► window, speakers
                      └──────────────────┬────────────────────────┘
                                         │ run_frame(), frame(), drain_audio()
                      ┌──────────────────▼──────────────── gase-core ┐
                      │ Genesis                                       │
                      │   ├─ M68k (gase-m68k)  ──┐                     │
                      │   ├─ Z80  (gase-z80)   ──┤ Bus traits          │
                      │   └─ Hardware ◄──────────┘                     │
                      │        ├─ Cartridge (ROM, SRAM/EEPROM, mapper) │
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
| VDP | 3420 per scanline | lines; access slots in master clocks within a line |
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

The VDP is busy reading VRAM to draw the picture, so outside writes can only
happen in a few **access slots** per line: 18 in active H40 display, 205
when the VDP is not drawing (vertical blanking, or display turned off).
Data-port writes wait in a 4-entry **FIFO** for their slot; a fifth write
stalls the 68000. A 68000-to-VDP DMA pushes words through the same FIFO and
freezes the 68000 until it is done; fills and copies run in the background
while status bit 1 (DMA busy) is set. This is why games upload graphics
during vertical blanking or with the display off: the same transfer is
about eleven times faster there.

The VDP's memory side is caught up lazily like the sound chips:
`Vdp::advance` uses the slots up to the current time before every port
access and at the end of each line. While a 68000 DMA runs, the scheduler
stops executing 68000 instructions and just lets the VDP progress up to the
next line event (see `crates/vdp/src/dma.rs` and `Genesis::run_until`).

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

## 7. Cartridge saves

Games keep progress in one of two kinds of chip. Most have **battery-backed
SRAM**, ordinary memory mapped at `0x200000` (`crates/core/src/cartridge.rs`).
A few dozen use a **serial EEPROM**: a tiny I²C chip with only a clock and
a data pin, which the game drives bit by bit by writing to a latch at a
board-specific address. `crates/core/src/eeprom.rs` explains the protocol
(START/STOP, ACK, device and word addresses, page writes) and emulates the
chip edge by edge; `crates/core/src/eeprom/boards.rs` lists which games use
which chip and wiring, since nothing in the ROM says so.

Frontends do not care which kind it is: `Cartridge::save_data()` and
`take_save_dirty()` give them bytes to store in the `.srm` file.

## 8. Correctness

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

## 9. Performance

At the time of writing gase runs the test ROMs at 1000-1700 frames per
second on one core of a modest 2.1 GHz Xeon, 16-28 times real time. The
numbers, the scripts that produce them and how to profile are in
[`benchmarks/`](../benchmarks/README.md). The main design choices:

* **Catch-up scheduling** (section 3). Nothing runs that nobody can
  observe: the sound chips and the VDP's memory side are only brought up
  to date when a CPU touches them or a line or frame ends.
* **Pre-decoded CPUs.** Every 68000 opcode is decoded once into a 65 536
  entry table, so executing an instruction is a table lookup and one
  `match`; the effective-address helpers are inlined into each handler so
  operands stay in registers.
* **Fast paths for the common case, full map behind them.** A program
  fetch from cartridge ROM is one compare and a load; the Z80's sound RAM
  likewise. Everything else (SRAM, EEPROM, mapper, I/O) goes through the
  full memory map, out of line.
* **A renderer that works in tile rows, not pixels.** A plane is drawn in
  runs of up to 8 pixels that share one name table entry and one pattern
  row; the name table row is found once per scroll column. Priorities are
  small numbers compared with byte arithmetic that the compiler turns into
  SIMD instructions, 16 pixels at a time.
* **Skip what cannot change.** A YM2612 channel whose operators have all
  faded out after key-off only advances its oscillators; the PSG jumps
  from one counter reload to the next instead of ticking every 16 clocks.

The rule for all of these: **a readable reference, and a tested fast
path.** The straightforward version stays in the code as the documented
definition (`Vdp::pick`, `plane_pixel_reference`, `Psg::tick` +
`Psg::output`, `Channel::calc_operators`, `Operator::eg_step`,
`Cartridge::read_word_mapped`, `State::save_slice`'s default), and a unit
test checks that the fast path gives exactly the same results, usually on
thousands of random inputs. Read the reference to learn how the hardware
works; read the fast path to learn how to make it quick. On top of that,
every optimisation must leave the test-ROM frame hashes, the WAV output
and the CPU test vectors bit-identical.

## Where to start reading

1. `crates/core/src/system.rs` — the main loop.
2. `crates/core/src/bus.rs` — the memory maps.
3. `crates/vdp/src/lib.rs` — what the VDP is.
4. `crates/m68k/src/lib.rs` and `crates/z80/src/lib.rs` — the CPUs.
5. `crates/sound/src/lib.rs` — FM synthesis.
