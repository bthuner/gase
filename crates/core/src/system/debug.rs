//! Debugger support: breakpoints, single-stepping and side-effect-free
//! inspection of memory.
//!
//! # Why a separate scheduler?
//!
//! [`Genesis::run_frame`] is the fast path: it runs a whole frame without
//! ever looking at breakpoints, so playing a game costs nothing extra when
//! no debugger is in use. A debugger, however, must be able to stop
//! *anywhere* — after one instruction, in the middle of a scanline — and
//! resume later from exactly that point.
//!
//! [`Debugger`] therefore drives the console with its own copy of the frame
//! loop, written as a resumable state machine: it remembers which scanline
//! the console is in and which event of that line (vertical interrupt,
//! horizontal blanking, end of line) comes next. Its steps are exactly the
//! ones `run_frame` performs, in the same order, so a frame run through the
//! debugger is bit-for-bit identical to a frame run normally (the tests
//! check this by comparing save states).
//!
//! While the debugger has stopped the console mid-frame
//! ([`Debugger::in_frame`]), keep using the debugger to run it until the
//! frame ends: `run_frame` always starts a frame from its first line.
//!
//! # Breakpoints
//!
//! A breakpoint stops the console *before* the instruction at its address
//! executes. The first instruction run by every debugger call never
//! triggers a breakpoint, so "continue" after a breakpoint does not stop at
//! the same place again.
//!
//! # Inspecting memory
//!
//! Reading some addresses has side effects (the VDP's data port advances its
//! address, the controller ports latch, the YM2612 status depends on time).
//! [`Genesis::peek_word`] and [`Genesis::peek_z80`] only read memories
//! (cartridge, work RAM, Z80 RAM) and return open-bus values elsewhere, so a
//! debugger can show memory and disassembly without disturbing the game.

use gase_vdp::{HBLANK_START_CYCLE, MASTER_CYCLES_PER_LINE, VINT_CYCLE};

use super::Genesis;

/// The next scheduler event in the current scanline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Event {
    /// The line has not started yet: the VDP must render it first.
    LineStart,
    /// The vertical interrupt (only on the first line of vertical blanking).
    VInt,
    /// The start of horizontal blanking.
    HBlank,
    /// The end of the line.
    LineEnd,
}

/// Where in the frame the debugger stopped the console.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Position {
    line: u16,
    next: Event,
}

/// Why a [`Debugger`] call returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// One 68000 instruction (or exception) was executed.
    Stepped,
    /// The frame is complete: the console is at a frame boundary.
    FrameEnd,
    /// The 68000 is about to execute the instruction at this address, which
    /// has a breakpoint.
    Breakpoint(u32),
    /// The VDP has just raised the vertical interrupt (the start of vertical
    /// blanking). Unless interrupts are masked, the next step enters the
    /// game's VBlank handler.
    VBlank,
}

/// What a call to [`Debugger::advance`] runs until.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Goal {
    Instruction,
    FrameEnd,
    VBlank,
}

/// Breakpoints and a resumable frame loop. See the [module docs](self).
#[derive(Clone, Debug, Default)]
pub struct Debugger {
    breakpoints: Vec<u32>,
    /// `None` at a frame boundary.
    position: Option<Position>,
}

impl Debugger {
    /// A debugger with no breakpoints, at a frame boundary.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The breakpoint addresses, in the order they were set.
    #[must_use]
    pub fn breakpoints(&self) -> &[u32] {
        &self.breakpoints
    }

    /// Set a breakpoint at `addr` (a 24-bit 68000 address).
    pub fn add_breakpoint(&mut self, addr: u32) {
        let addr = addr & 0xFF_FFFF;
        if !self.breakpoints.contains(&addr) {
            self.breakpoints.push(addr);
        }
    }

    /// Remove the breakpoint at `addr`; returns whether there was one.
    pub fn remove_breakpoint(&mut self, addr: u32) -> bool {
        let addr = addr & 0xFF_FFFF;
        let before = self.breakpoints.len();
        self.breakpoints.retain(|&a| a != addr);
        self.breakpoints.len() != before
    }

    /// Set or clear the breakpoint at `addr`; returns whether it is now set.
    pub fn toggle_breakpoint(&mut self, addr: u32) -> bool {
        if self.remove_breakpoint(addr) {
            false
        } else {
            self.add_breakpoint(addr);
            true
        }
    }

    /// Remove every breakpoint.
    pub fn clear_breakpoints(&mut self) {
        self.breakpoints.clear();
    }

    /// Has the debugger stopped the console in the middle of a frame?
    #[must_use]
    pub fn in_frame(&self) -> bool {
        self.position.is_some()
    }

    /// The scanline the console is in, when stopped mid-frame.
    #[must_use]
    pub fn line(&self) -> Option<u16> {
        self.position.map(|p| p.line)
    }

    /// Forget the mid-frame position. Call this after the console was put
    /// back at a frame boundary behind the debugger's back (loading a save
    /// state, rewinding).
    pub fn forget_position(&mut self) {
        self.position = None;
    }

    /// Execute exactly one 68000 instruction (or exception, or 4 idle cycles
    /// while the CPU is stopped), with everything else that happens in that
    /// time. Breakpoints are ignored: stepping always moves forward.
    pub fn step_instruction(&mut self, genesis: &mut Genesis) -> Stop {
        self.advance(genesis, Goal::Instruction)
    }

    /// Run until the end of the current frame, or until a breakpoint.
    pub fn run_frame(&mut self, genesis: &mut Genesis) -> Stop {
        self.advance(genesis, Goal::FrameEnd)
    }

    /// Run until the VDP raises the next vertical interrupt, or until a
    /// breakpoint. This may cross into the next frame.
    pub fn run_to_vblank(&mut self, genesis: &mut Genesis) -> Stop {
        self.advance(genesis, Goal::VBlank)
    }

    /// The resumable frame loop. It must perform the same steps, in the
    /// same order, as [`Genesis::run_frame`] and `Genesis::run_until`.
    fn advance(&mut self, g: &mut Genesis, goal: Goal) -> Stop {
        let mut executed = 0u32;
        loop {
            let mut pos = self.position.unwrap_or(Position {
                line: 0,
                next: Event::LineStart,
            });
            // While a line is in progress, `next_line` is still its start.
            let start = g.next_line;
            let target = match pos.next {
                Event::LineStart => {
                    g.hw.line_start = start;
                    g.hw.vdp.begin_line(pos.line);
                    let active = g.hw.vdp.active_lines();
                    pos.next = if pos.line == active {
                        Event::VInt
                    } else {
                        if pos.line == active + 1 {
                            g.hw.z80_irq = false;
                        }
                        Event::HBlank
                    };
                    self.position = Some(pos);
                    continue;
                }
                Event::VInt => start + u64::from(VINT_CYCLE),
                Event::HBlank => start + u64::from(HBLANK_START_CYCLE),
                Event::LineEnd => start + u64::from(MASTER_CYCLES_PER_LINE),
            };

            while g.m68k_clock < target {
                if executed > 0 {
                    if goal == Goal::Instruction {
                        return Stop::Stepped;
                    }
                    if !self.breakpoints.is_empty() {
                        let pc = g.m68k.pc() & 0xFF_FFFF;
                        if self.breakpoints.contains(&pc) {
                            return Stop::Breakpoint(pc);
                        }
                    }
                }
                step_68k(g);
                executed += 1;
            }

            match pos.next {
                Event::LineStart => unreachable!("handled above"),
                Event::VInt => {
                    g.hw.vdp.trigger_vint();
                    // The Z80 also receives the vertical interrupt, for about one line.
                    g.hw.z80_irq = true;
                    pos.next = Event::HBlank;
                    self.position = Some(pos);
                    if goal == Goal::VBlank {
                        return Stop::VBlank;
                    }
                }
                Event::HBlank => {
                    g.hw.vdp.hblank();
                    pos.next = Event::LineEnd;
                    self.position = Some(pos);
                }
                Event::LineEnd => {
                    g.next_line = start + u64::from(MASTER_CYCLES_PER_LINE);
                    pos.line += 1;
                    pos.next = Event::LineStart;
                    self.position = Some(pos);
                    if pos.line == g.hw.vdp.total_lines() {
                        self.position = None;
                        g.hw.now = g.next_line;
                        g.hw.sync_audio();
                        g.resample_audio();
                        g.frame_count += 1;
                        if goal == Goal::FrameEnd {
                            return Stop::FrameEnd;
                        }
                    }
                }
            }
        }
    }
}

/// One iteration of `Genesis::run_until`: a 68000 instruction, any DMA it
/// started, and the Z80 catching up.
fn step_68k(g: &mut Genesis) {
    g.hw.now = g.m68k_clock;
    g.m68k.set_interrupt_level(g.hw.vdp.interrupt_level());
    if g.trace_remaining > 0 {
        g.trace_instruction();
    }
    let mut cycles = g.m68k.step(&mut g.hw);
    cycles += std::mem::take(&mut g.hw.m68k_wait);
    if g.hw.vdp.dma_68k_pending() {
        cycles += g.run_dma();
    }
    g.hw.reset_requested = false;
    g.m68k_clock += u64::from(cycles) * 7;
    g.run_z80_until(g.m68k_clock);
}

/// Inspection helpers for debuggers. None of them changes the console.
impl Genesis {
    /// Read the 68000 word at `addr` without side effects.
    ///
    /// Only the cartridge, work RAM and Z80 RAM are visible; other areas
    /// (I/O, VDP ports) read as `0xFFFF` because reading them would disturb
    /// the hardware.
    #[must_use]
    pub fn peek_word(&self, addr: u32) -> u16 {
        let addr = addr & 0xFF_FFFE;
        match addr {
            0x00_0000..=0x3F_FFFF => self.hw.cart.read_word(addr),
            0xA0_0000..=0xA0_3FFF => {
                let byte = self.hw.zram[(addr & 0x1FFF) as usize];
                u16::from_be_bytes([byte, byte])
            }
            0xE0_0000..=0xFF_FFFF => {
                let i = (addr & 0xFFFF) as usize;
                u16::from_be_bytes([self.hw.ram[i], self.hw.ram[i + 1]])
            }
            _ => 0xFFFF,
        }
    }

    /// Read the 68000 byte at `addr` without side effects (see
    /// [`Genesis::peek_word`]).
    #[must_use]
    pub fn peek_byte(&self, addr: u32) -> u8 {
        let [high, low] = self.peek_word(addr).to_be_bytes();
        if addr & 1 == 0 { high } else { low }
    }

    /// Read the byte the Z80 sees at `addr` without side effects: its RAM
    /// and the banked window into 68000 memory. The YM2612, the bank
    /// register and the VDP read as `0xFF`.
    #[must_use]
    pub fn peek_z80(&self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x3FFF => self.hw.zram[usize::from(addr & 0x1FFF)],
            0x8000..=0xFFFF => {
                let addr68 = (u32::from(self.hw.z80_bank) << 15) | u32::from(addr & 0x7FFF);
                if (0xA0_0000..0xA1_0000).contains(&addr68) {
                    0xFF
                } else {
                    self.peek_byte(addr68)
                }
            }
            _ => 0xFF,
        }
    }

    /// Disassemble the 68000 instruction at `addr`. Returns the text and the
    /// instruction length in bytes.
    #[must_use]
    pub fn disassemble(&self, addr: u32) -> (String, u32) {
        gase_m68k::disasm::disassemble(addr, |a| self.peek_word(a))
    }

    /// Disassemble the Z80 instruction at `addr`. Returns the text and the
    /// instruction length in bytes.
    #[must_use]
    pub fn disassemble_z80(&self, addr: u16) -> (String, u16) {
        gase_z80::disasm::disassemble(addr, |a| self.peek_z80(a))
    }

    /// Silence individual sound channels, for listening to them one at a
    /// time: `fm` bit 0..5 mutes FM channel 1..6 (channel 6 includes the
    /// DAC), `psg` bit 0..3 mutes tone 1..3 and the noise channel.
    pub fn set_channel_mutes(&mut self, fm: u8, psg: u8) {
        // Produce the sound up to now with the old setting first.
        self.hw.sync_audio();
        self.hw.ym.set_muted_channels(fm);
        self.hw.psg.set_muted_channels(psg);
    }

    /// The masks set with [`Genesis::set_channel_mutes`]: `(fm, psg)`.
    #[must_use]
    pub fn channel_mutes(&self) -> (u8, u8) {
        (self.hw.ym.muted_channels(), self.hw.psg.muted_channels())
    }
}
