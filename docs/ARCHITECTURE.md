# How gase works

This guide walks through the emulator from the outside in. Each section
points to the module whose documentation goes deeper.

## 1. The big picture

```text
   keyboard, pads,   ┌──── shell: gase (SDL2 desktop) · web · Android · iOS ────┐
   mouse, touch ───► │ window, sound device, files, input devices              │ ───► screen, speakers
                     └───────────────┬────────────────────────────▲────────────┘
                                     │ Event                      │ Platform trait,
                     ┌───────────────▼──── gase-app ──────────────┴────────────┐  Video
                     │ App: home, file browser, menus, settings, input mapping,  │
                     │ touch controls, save states, rewind, pacing               │
                     └───────────────┬───────────────────────────────────────────┘
                                     │ run_frame(), frame(), drain_audio(), set_buttons()
                      ┌──────────────▼───────────────────────────────── gase-core ┐
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
portable to any frontend. The user interface is built the same way: a
library without I/O (`gase-app`, section 10) that every platform's thin
shell drives.

### Loading ROMs and archives

`Cartridge::from_bytes` (`crates/core/src/cartridge.rs`) is the single
entry point for ROM bytes, so every frontend gets the same formats: plain
images (`.bin`, `.md`, `.gen`), interleaved `.smd` dumps (deinterleaved on
load), and either of them inside a `.zip` archive. A zip file is
recognised by its first bytes (`PK\3\4`), the ROM inside is picked by
its extension (the largest `.md`/`.bin`/`.gen`/`.smd`/`.68k`/`.sgd` file)
and extracted, with its CRC-32 checked, by `crates/zip`, a dependency-free
ZIP reader and DEFLATE decoder whose module docs explain both formats:
LZ77 back-references, canonical Huffman codes and how a compressed block
describes its own codes. Frontends name save files after the file they
opened, so `game.zip` saves to `game.srm` like `game.bin` does.

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
(`crates/app/src/font.rs`, shared with the menus; the views are in
`crates/gase/src/debugger/`), so the same views can be saved as PNG from
the headless runner (`--dump-vram`, `--dump-cram`, `--dump-debugger`,
`--break`).

## 9. Performance

At the time of writing gase runs the test ROMs at 1000-1800 frames per
second on one core of a modest 2.1 GHz Xeon, 17-30 times real time. The
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

## 10. The user interface and the platform contract

An emulator frontend has to do the same things on every platform: show a
picture, play sound, read buttons, open files, offer menus. Only the
*how* differs. gase splits the two:

* **`gase-app`** (`crates/app`) decides *what* happens: the home screen,
  the file browser, the pause menu, save states, settings, how a key or a
  finger becomes a console button, how fast to run. It has no
  dependencies and no I/O, like the core.
* A **shell** per platform does the *how*: `crates/gase/src/sdl.rs` (with
  `desktop.rs` for files) on desktop and, the same file with `mobile.rs`
  for files, on Android and iOS (see "Mobile shells" below); a web shell
  (JavaScript + WASM) follows the same pattern.
  `desktop.rs` for files) on desktop; `crates/web` with the page in
  `web/` in a browser (section 11); Android/iOS shells (SDL2) can follow
  the same pattern.

The contract between them is the `Platform` trait plus three flows
(`crates/app/src/platform.rs` documents it with a diagram):

```rust
pub trait Platform {
    fn now_ms(&self) -> u64;
    fn log(&mut self, message: &str) {}
    fn load(&mut self, file: FileKey<'_>) -> Option<Vec<u8>>;
    fn store(&mut self, file: FileKey<'_>, data: &[u8]) -> Result<String, String>;
    fn read_rom(&mut self, path: &str) -> Result<Vec<u8>, String>;
    fn list_dir(&mut self, dir: Option<&str>) -> Result<Listing, String> { /* unsupported */ }
    fn audio_queued(&self) -> Option<usize> { None }
    fn queue_audio(&mut self, samples: &[i16]) {}
    fn request(&mut self, request: Request) {}
}
```

* **Events flow in**: the shell translates its system's input into
  `Event`s (physical keys, standard-layout gamepad buttons, pointers with
  ids for multi-touch, resizes with the display density, dropped files).
* **Audio is pushed** to `queue_audio` as it is produced; the app reads
  `audio_queued` to fine-tune the audio speed (section 5).
* **Video is pulled**: `App::video()` returns the game frame with the
  rectangle to draw it in, and an *overlay* with alpha for menus, touch
  controls and messages. The overlay is drawn small (one font pixel = one
  overlay pixel) and enlarged by a whole factor by the GPU, so it costs
  a few hundred thousand pixels at most, and only while something is
  shown: while playing with a keyboard or pad there is no overlay at all.
* **Storage** says *what*, not *where*: `FileKey::State { rom, slot }`
  becomes `game.state3` next to the ROM on desktop, a browser storage key
  on the web.
* **Slow or optional things are requests answered later**: the web's
  file picker answers with an `Event::RomData` whenever the user is done.
  `Capabilities` tell the app what the platform can do, so it only offers
  what works (no "Quit" on the web, a native picker instead of the
  built-in browser on phones).

Inside the app, three ideas are worth reading about in the code:

* **Immediate-mode UI** (`crates/app/src/ui.rs`): each screen is a function
  that draws itself and returns what was chosen, every frame. Only the
  focus and scroll position persist. Focus moves by *spatial navigation*
  (to the nearest item in the pressed direction), so lists and grids work
  with a d-pad without special code.
* **Software rendering** with the 8×8 font (`canvas.rs`, `font.rs`): one
  pixel buffer works everywhere and can be tested; `gase --headless
  --dump-ui` saves the interface as PNG.
* **Input mapping** (`input.rs`, `touch.rs`): bindings per player for keys
  and pad buttons; the on-screen d-pad picks one of eight sectors by
  comparing the finger's offsets with tan 22.5°, and every finger is
  tracked separately, so the console buttons are simply the union of what
  each one presses.

### Mobile shells

SDL2 runs on Android and iOS too, so the phone apps reuse the desktop's
SDL shell instead of a framework of their own: `gase::sdl::run_sdl`
takes `Shell::Desktop(options)` or `Shell::Mobile(hooks)`, and the few
differences are decided in one place:

| | Desktop | Phone |
|---|---|---|
| Files | config folder, saves next to the ROM | the app's sandbox (`SDL_GetPrefPath`), ROM copies in `roms/` |
| Opening ROMs | built-in browser, drag and drop | the system's document picker, "Open with" |
| Window | resizable, fullscreen on F11 | the whole screen, rotates (portrait/landscape layouts) |
| Density | drawable ÷ window size | the same on iOS; display DPI ÷ 160 on Android |
| Lifecycle | runs until closed | background: write saves, pause, stop drawing; foreground: resume |

How it starts is the interesting part. Neither system calls `main`: on
Android the Java VM starts SDL's `SDLActivity`, which loads `libSDL2.so`
and `libmain.so` and calls the C function `SDL_main` on a thread of its
own; on iOS a tiny Objective-C `main` hands control to UIKit through
`SDL_UIKitRunApp`, which calls `SDL_main` once the app has launched. The
`gase-mobile` crate (`crates/mobile`) exports `SDL_main` — compiled as a
`cdylib` for Android, a `staticlib` linked into the iOS executable — and
it is the only place in the workspace allowed to use `unsafe`, for that
export and two calls into native code. What only Java or Objective-C can
do (the document picker, "Open with", copying the file into the sandbox)
is a few dozen lines of native glue that hands the copy's path back as an
ordinary SDL drop-file event, which the shell already understood.
`mobile/README.md` walks through the whole path, the build and the
lifecycle.

Two rules came with the touch screen: on-screen controls hide while a
gamepad is being used (the last input wins), and `Event::Suspend` writes
the save and settings immediately, because a phone may kill a backgrounded
app without warning.
## 11. The web shell

`crates/web` (gase-web) is the platform contract implemented for a
browser tab, and `web/` is the page that hosts it. It is written to show
how a Rust program runs in a browser with nothing in between: no
wasm-bindgen, no web-sys, no bundler.

* **One address space.** The module is built for `wasm32-unknown-unknown`
  as a `cdylib`. Its *linear memory* is one `ArrayBuffer` that the page
  can see; a Rust pointer is an offset into it. So pictures and sound are
  never serialised: Rust returns `vec.as_ptr()` as a number, the page
  wraps `new Uint8ClampedArray(memory.buffer, ptr, len)` and hands it to
  `putImageData`, or copies samples out of an `Int16Array` view. Views
  are made afresh each time, because growing the memory replaces the
  buffer.
* **A numbers-only boundary** (`crates/web/src/abi.rs`). About twenty
  exports (`gase_key`, `gase_pointer`, `gase_update`, `gase_video` …) and
  nine imports (`file_read`, `file_write`, `audio_push`, `request` …),
  all taking integers and floats. Text and ROMs from the page go through
  an *inbox* buffer the module sizes on request. After each frame the
  module fills a seventeen-word *frame description* (picture addresses and
  sizes, where to draw them, how to pace), which the page reads through
  one `Uint32Array`, like a C struct. The workspace forbids unsafe code;
  this crate denies it instead, because exporting under a fixed name
  needs `#[unsafe(no_mangle)]` and importing needs an `unsafe extern`
  block with `safe fn` declarations. Both stay in `abi.rs`, which has no
  `unsafe { }` block. Everything else is a `Shell` generic over a `Host`
  trait, tested natively with a fake page.
* **Storage** is synchronous for the app but IndexedDB is asynchronous:
  the page reads every stored file into a `Map` at startup and writes
  behind, one transaction per frame. Opened ROMs are kept (the eight most
  recently used) so the recent list works after a reload.
* **Audio** runs in an `AudioWorklet` on the browser's real-time thread,
  fed from the main thread either through a ring buffer in a
  `SharedArrayBuffer` (exact queue level, but only allowed on
  cross-origin-isolated pages, which needs COOP/COEP headers that GitHub
  Pages cannot send) or through posted chunks with the queue level
  estimated from the worklet's reports and the audio clock (works
  everywhere). Sound starts at the first click or key press, as browsers
  require; until then the app paces by the clock.
* **Pacing** follows the app's `Pacing` inside `requestAnimationFrame`.
  Each display frame owes `elapsed × the console's rate` emulated frames
  (rounded, the remainder carried over, so a 60 Hz display gets a steady
  one per refresh whatever its jitter). With sound the audio queue
  steers too: more than two frames' worth below its 50 ms target adds a
  frame, above it skips one; small differences between the sound card's
  clock and the console's are left to the app's dynamic rate control,
  which stretches the audio by up to 0.5 %. The queue level is only an
  estimate without SharedArrayBuffer and audio devices consume in bursts,
  so a tight rule ("run until the queue is full") would alternate 0 and 2
  frames per refresh. Fast-forward runs as many updates as fit in 12 ms.
* **Input** is translated in Rust tables: `KeyboardEvent.code` (a
  physical key, like the app's `Key`), the Gamepad API's "standard"
  mapping (polled each frame, changes sent as events, triggers as axes),
  Pointer Events with their `pointerId` for multi-touch, and
  `devicePixelRatio` for the UI scale.

## Where to start reading

1. `crates/core/src/system.rs` — the main loop.
2. `crates/core/src/bus.rs` — the memory maps.
3. `crates/vdp/src/lib.rs` — what the VDP is.
4. `crates/m68k/src/lib.rs` and `crates/z80/src/lib.rs` — the CPUs.
5. `crates/sound/src/lib.rs` — FM synthesis.
6. `crates/app/src/lib.rs` — the user interface, and `platform.rs` for
   how it reaches any platform.
7. `crates/zip/src/inflate.rs` — DEFLATE, for a break from hardware.
8. `crates/gase/src/sdl.rs` and `crates/mobile/src/lib.rs` — one shell for
   desktops and phones, and how a Rust library becomes an app.
9. `crates/web/src/lib.rs`, then `web/gase.js` — the emulator in a web
   page.
