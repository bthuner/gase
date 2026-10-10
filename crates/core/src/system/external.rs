//! Driving the console from an *external* 68000: statically recompiled code.
//!
//! The normal scheduler ([`Genesis::run_frame`]) owns the 68000: it steps
//! the interpreter one instruction at a time and handles the scanline
//! events in between. A static recompiler such as
//! [gase-recomp](https://github.com/bthuner/gase-recomp) turns a game's
//! 68000 code into native code ahead of time; that code cannot be stepped,
//! it *runs*. So the roles are reversed: the external CPU runs freely, and
//! at its own "poll points" asks the console to **catch up** with the time
//! it has used. [`External`] is that inverted scheduler; everything else
//! (VDP, sound, Z80, I/O, cartridge) is the same as in normal mode.
//!
//! Between frames the interpreter's [`M68k`] stays the canonical CPU state:
//! an external CPU loads its registers from it when a frame starts and
//! stores them back when the frame ends. Save states, rewind and the
//! debugger's register views therefore keep working unchanged, and the
//! interpreter is right there for code the recompiler did not see.
//!
//! # Registering a recompiled game
//!
//! A recompiled game calls [`register`] at start-up with its ROM's CRC-32;
//! any [`Genesis`] created afterwards for that ROM, by any frontend, runs
//! the recompiled code. With nothing registered, nothing changes.

use std::fmt;
use std::sync::Mutex;

use gase_m68k::{Bus, M68k};
use gase_vdp::{HBLANK_START_CYCLE, MASTER_CYCLES_PER_LINE, VINT_CYCLE};
use gase_z80::Z80;

#[cfg(doc)]
use crate::Genesis;
use crate::bus::{Hardware, Z80Bus};

/// A replacement for the 68000 interpreter.
pub trait ExternalCpu: fmt::Debug + Send {
    /// Run the 68000 until [`External::frame_done`] says the frame is over.
    ///
    /// `cpu` holds the CPU state: load it first, store it back before
    /// returning. It is also an interpreter, for code you cannot run.
    fn run_frame(&mut self, cpu: &mut M68k, console: &mut External<'_>);

    /// A copy (consoles are cloned for save states and rewind).
    fn clone_box(&self) -> Box<dyn ExternalCpu>;

    /// A short description for logs, e.g. "recompiled by gase-recomp".
    fn describe(&self) -> String {
        "external 68000".to_string()
    }
}

impl Clone for Box<dyn ExternalCpu> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

/// Makes a fresh external CPU for a console.
pub type Factory = fn() -> Box<dyn ExternalCpu>;

static REGISTRY: Mutex<Vec<(u32, Factory)>> = Mutex::new(Vec::new());

/// Run `factory`'s CPU for every console created for the ROM whose CRC-32
/// (the zip/PNG one, over the flat ROM image) is `rom_crc32`.
pub fn register(rom_crc32: u32, factory: Factory) {
    let mut list = REGISTRY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    list.retain(|(crc, _)| *crc != rom_crc32);
    list.push((rom_crc32, factory));
}

/// The CPU registered for a ROM, if any.
pub(crate) fn registered(rom: &[u8]) -> Option<Box<dyn ExternalCpu>> {
    let list = REGISTRY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if list.is_empty() {
        return None;
    }
    let crc = gase_zip::crc32::crc32(rom);
    list.iter().find(|(c, _)| *c == crc).map(|(_, make)| make())
}

/// Which scanline event comes next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    /// The first blank line: the vertical interrupt fires 770 clocks in.
    Vint,
    /// Horizontal blanking starts 2650 clocks into the line.
    Hblank,
    /// The line ends after 3420 clocks.
    End,
}

/// The console as seen by an external 68000, for one frame.
///
/// Memory accesses go through [`Bus`], exactly as the interpreter's do.
/// Time is counted in **68000 cycles** since power-on (the master clock
/// divided by 7); the CPU reports it with [`External::set_clock`] before
/// accesses and hands control to the console with [`External::catch_up`].
#[derive(Debug)]
pub struct External<'a> {
    hw: &'a mut Hardware,
    z80: &'a mut Z80,
    z80_clock: &'a mut u64,
    /// The 68000's clock in master ticks.
    clock: u64,
    line: u16,
    stage: Stage,
    frame_done: bool,
}

impl<'a> External<'a> {
    pub(crate) fn new(
        hw: &'a mut Hardware,
        z80: &'a mut Z80,
        z80_clock: &'a mut u64,
        clock: u64,
        frame_start: u64,
    ) -> Self {
        let mut console = Self {
            hw,
            z80,
            z80_clock,
            clock,
            line: 0,
            stage: Stage::Hblank,
            frame_done: false,
        };
        console.start_line(0, frame_start);
        console
    }

    /// Run the rest of the frame with the 68000 idle (an external CPU that
    /// returned early).
    pub(crate) fn finish_frame(&mut self) {
        while !self.frame_done {
            self.clock = self.next_event();
            self.run_to(self.clock);
        }
    }

    /// The 68000's clock in master ticks (for the console's bookkeeping).
    pub(crate) fn master_clock(&self) -> u64 {
        self.clock
    }

    /// The 68000's clock, in CPU cycles.
    #[must_use]
    pub fn cycles(&self) -> u64 {
        self.clock / 7
    }

    /// Tell the console the 68000's time (in CPU cycles) before an access:
    /// the VDP's FIFO and the sound chips need to know *when* it happens.
    #[inline]
    pub fn set_clock(&mut self, cycles: u64) {
        self.clock = self.clock.max(cycles * 7);
    }

    /// Run every other chip up to `cycles` (scanlines, Z80, sound) and
    /// return the cycle at which the console next needs attention.
    pub fn catch_up(&mut self, cycles: u64) -> u64 {
        self.set_clock(cycles);
        self.run_to(self.clock);
        self.next_event().div_ceil(7)
    }

    /// The CPU's clock once the waits the hardware imposed are counted
    /// (VDP FIFO full, Z80 bus arbitration, a 68000-to-VDP DMA).
    #[must_use]
    pub fn stalled_clock(&self, cycles: u64) -> u64 {
        cycles.max(self.clock.div_ceil(7))
    }

    /// Is the frame over? The CPU should return from `run_frame` promptly.
    #[must_use]
    pub fn frame_done(&self) -> bool {
        self.frame_done
    }

    /// The interrupt level the 68000 sees (0 = none).
    #[must_use]
    pub fn interrupt_level(&self) -> u8 {
        self.hw.vdp.interrupt_level()
    }

    // ---- the inverted scheduler --------------------------------------------
    //
    // The same events as `Genesis::run_frame`, in the same order: a line
    // starts (the VDP renders it), the vertical interrupt fires on the
    // first blank line, horizontal blanking starts, the line ends. The Z80
    // is run up to each event.

    fn start_line(&mut self, line: u16, start: u64) {
        self.line = line;
        self.hw.line_start = start;
        self.hw.vdp.begin_line(line);
        let active = self.hw.vdp.active_lines();
        if line == active + 1 {
            self.hw.z80_irq = false;
        }
        self.stage = if line == active {
            Stage::Vint
        } else {
            Stage::Hblank
        };
    }

    fn next_event(&self) -> u64 {
        self.hw.line_start
            + u64::from(match self.stage {
                Stage::Vint => VINT_CYCLE,
                Stage::Hblank => HBLANK_START_CYCLE,
                Stage::End => MASTER_CYCLES_PER_LINE,
            })
    }

    fn run_to(&mut self, now: u64) {
        while !self.frame_done {
            let event = self.next_event();
            if now < event {
                self.run_z80(now);
                return;
            }
            self.run_z80(event);
            self.hw.now = event;
            match self.stage {
                Stage::Vint => {
                    self.hw.vdp.trigger_vint();
                    // The Z80 also receives the vertical interrupt, for about one line.
                    self.hw.z80_irq = true;
                    self.stage = Stage::Hblank;
                }
                Stage::Hblank => {
                    self.hw.vdp.hblank();
                    self.stage = Stage::End;
                }
                Stage::End => {
                    if self.line + 1 == self.hw.vdp.total_lines() {
                        // The next frame starts here; Genesis::run_frame
                        // does the end-of-frame bookkeeping.
                        self.hw.line_start = event;
                        self.frame_done = true;
                    } else {
                        self.start_line(self.line + 1, event);
                    }
                }
            }
        }
    }

    fn run_z80(&mut self, target: u64) {
        if !self.hw.z80_running() {
            if self.hw.z80_reset {
                self.z80.reset();
            }
            *self.z80_clock = (*self.z80_clock).max(target);
            return;
        }
        while *self.z80_clock < target {
            self.hw.now = *self.z80_clock;
            self.z80.set_irq(self.hw.z80_irq);
            let cycles = self.z80.step(&mut Z80Bus(&mut *self.hw));
            *self.z80_clock += u64::from(cycles) * 15;
        }
    }

    /// Before an access to anything but ROM and work RAM: bring the console
    /// up to the 68000's time, so the VDP and the sound chips see it in order.
    fn sync(&mut self) {
        self.run_to(self.clock);
        self.hw.now = self.clock;
    }

    /// After such an access: the time the hardware made the 68000 wait.
    fn charge_waits(&mut self) {
        self.clock += u64::from(std::mem::take(&mut self.hw.vdp_stall))
            + 7 * u64::from(std::mem::take(&mut self.hw.m68k_wait));
    }

    /// A 68000-to-VDP DMA freezes the 68000 until the transfer is over.
    fn dma_freeze(&mut self) {
        if self.hw.vdp.dma_68k_pending() {
            self.hw.start_dma_68k();
        }
        while self.hw.vdp.dma_68k_active() && !self.frame_done {
            let event = self.next_event();
            self.hw.vdp.advance((event - self.hw.line_start) as u32);
            if self.hw.vdp.dma_68k_active() {
                self.clock = self.clock.max(event);
            } else {
                let done = self.hw.line_start + u64::from(self.hw.vdp.dma_68k_done_at());
                self.clock = self.clock.max(done);
            }
            self.run_to(self.clock);
        }
    }

    #[inline(never)]
    fn read_byte_slow(&mut self, addr: u32) -> u8 {
        if addr < 0x40_0000 {
            return self.hw.read_byte(addr);
        }
        self.sync();
        let v = self.hw.read_byte(addr);
        self.charge_waits();
        v
    }

    #[inline(never)]
    fn read_word_slow(&mut self, addr: u32) -> u16 {
        if addr < 0x40_0000 {
            return self.hw.read_word(addr);
        }
        self.sync();
        let v = self.hw.read_word(addr);
        self.charge_waits();
        v
    }

    #[inline(never)]
    fn write_slow(&mut self, addr: u32, value: u16, word: bool) {
        self.sync();
        if word {
            self.hw.write_word(addr, value);
        } else {
            self.hw.write_byte(addr, value as u8);
        }
        self.charge_waits();
        self.dma_freeze();
    }
}

// Recompiled code inlines memory accesses into every instruction, so only
// the work-RAM fast path is inlined here; everything else is out of line.
impl Bus for External<'_> {
    #[inline(always)]
    fn read_byte(&mut self, addr: u32) -> u8 {
        if addr >= 0xE0_0000 {
            return self.hw.ram[(addr & 0xFFFF) as usize];
        }
        self.read_byte_slow(addr)
    }

    #[inline(always)]
    fn read_word(&mut self, addr: u32) -> u16 {
        if addr >= 0xE0_0000 {
            let i = (addr & 0xFFFF) as usize;
            return u16::from_be_bytes([self.hw.ram[i], self.hw.ram[i + 1]]);
        }
        self.read_word_slow(addr)
    }

    #[inline(always)]
    fn write_byte(&mut self, addr: u32, value: u8) {
        if addr >= 0xE0_0000 {
            self.hw.ram[(addr & 0xFFFF) as usize] = value;
            return;
        }
        self.write_slow(addr, u16::from(value), false);
    }

    #[inline(always)]
    fn write_word(&mut self, addr: u32, value: u16) {
        if addr >= 0xE0_0000 {
            let i = (addr & 0xFFFF) as usize;
            self.hw.ram[i..i + 2].copy_from_slice(&value.to_be_bytes());
            return;
        }
        self.write_slow(addr, value, true);
    }

    fn interrupt_acknowledge(&mut self, level: u8) -> Option<u8> {
        self.hw.interrupt_acknowledge(level)
    }

    fn reset_devices(&mut self) {
        self.hw.reset_devices();
    }

    fn tas_write_byte(&mut self, addr: u32, value: u8) {
        // The bus arbiter drops the write half of TAS on work RAM.
        if addr < 0xE0_0000 {
            self.write_byte(addr, value);
        }
    }
}
