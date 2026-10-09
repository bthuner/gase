//! A Zilog Z80 CPU core.
//!
//! In the Mega Drive the Z80 is the sound co-processor: it runs at
//! ≈3.58 MHz, has 8 KiB of private RAM, talks to the YM2612 and the PSG and
//! can peek into the 68000's address space through a banked window. This
//! crate knows nothing about that machine, though: it is a plain Z80 that
//! talks to the outside world through the [`Bus`] trait.
//!
//! ```
//! use gase_z80::{Bus, Z80};
//!
//! struct Ram([u8; 0x10000]);
//! impl Bus for Ram {
//!     fn read(&mut self, addr: u16) -> u8 { self.0[addr as usize] }
//!     fn write(&mut self, addr: u16, value: u8) { self.0[addr as usize] = value }
//! }
//!
//! let mut ram = Ram([0; 0x10000]);
//! ram.0[..4].copy_from_slice(&[0x3E, 0x2A, 0x3C, 0x76]); // LD A,42; INC A; HALT
//! let mut cpu = Z80::new();
//! let mut cycles = 0;
//! while !cpu.is_halted() {
//!     cycles += cpu.step(&mut ram);
//! }
//! assert_eq!((cpu.a, cycles), (43, 7 + 4 + 4));
//! ```
//!
//! # Registers
//!
//! ```text
//!   main set        shadow set       special purpose
//!   +----+----+     +----+----+      +---------+  +---------+
//!   | A  | F  |     | A' | F' |      |   IX    |  |   SP    |
//!   | B  | C  |     | B' | C' |      |   IY    |  |   PC    |
//!   | D  | E  |     | D' | E' |      +----+----+  +---------+
//!   | H  | L  |     | H' | L' |      | I  | R  |
//!   +----+----+     +----+----+      +----+----+
//!   hidden: WZ (MEMPTR), Q, IFF1, IFF2, IM
//! ```
//!
//! * `A` is the accumulator, the implicit operand of most arithmetic.
//!   `F` holds the [flags](flags).
//! * `BC`, `DE`, `HL` are general purpose and pair up as 16-bit registers.
//!   `HL` is the privileged memory pointer: `(HL)` is an operand of almost
//!   every 8-bit instruction.
//! * The shadow set is swapped in wholesale by `EXX` (BC/DE/HL) and
//!   `EX AF,AF'`. It lets an interrupt handler get fresh registers without
//!   pushing anything.
//! * `IX` and `IY` are index registers: `(IX+d)` addresses memory with a
//!   signed 8-bit displacement, ideal for structures.
//! * `I` is the high byte of the interrupt vector table in interrupt mode 2.
//! * `R` is the DRAM refresh counter. The CPU increments its low 7 bits on
//!   every opcode fetch (M1 cycle), so a prefixed instruction bumps it twice;
//!   bit 7 only changes with `LD R,A`. Programs use it as a cheap random
//!   number source.
//! * `WZ` (also called MEMPTR) is an internal temporary register used for
//!   16-bit address arithmetic. It is invisible, except that `BIT n,(HL)`
//!   copies bits 13 and 11 of it into the undocumented X/Y flags.
//! * `Q` is an internal latch holding the flags written by the last
//!   instruction (or 0 if it did not change them); it explains the X/Y
//!   flags of `SCF` and `CCF`.
//! * `IFF1`/`IFF2` are the interrupt enable flip-flops and `IM` the interrupt
//!   mode; see [the interrupt section](#interrupts).
//!
//! # Instruction encoding
//!
//! The base page has 256 opcodes. Four of them are *prefixes* that select
//! another page:
//!
//! * `CB`: rotates, shifts and single-bit operations (`BIT`, `SET`, `RES`).
//! * `ED`: "extended" instructions: 16-bit `ADC`/`SBC`, block copy/search/I-O
//!   (`LDIR`, `CPIR`, `OTIR`, ...), `IN r,(C)`, `NEG`, `IM n`, `RETI`/`RETN`
//!   and access to `I`/`R`. Unused `ED` opcodes act as 8 T-state NOPs.
//! * `DD`/`FD`: do not open a new page but modify the *next* instruction to
//!   use `IX`/`IY` instead of `HL`, and `(IX+d)` instead of `(HL)`. Applied
//!   to `H` or `L` they give the undocumented 8-bit halves `IXH`, `IXL`, ...
//!   Applied to an instruction that doesn't involve HL they do nothing but
//!   cost 4 T-states.
//! * `DD CB d op` / `FD CB d op`: bit operations on `(IX+d)`. The
//!   displacement comes before the opcode. Undocumented variants also copy
//!   the result into a register.
//!
//! See the [`decode`] module for how the executor is organised around
//! this structure.
//!
//! # Timing
//!
//! [`Z80::step`] returns the T-states (clock cycles) an instruction took.
//! The count is not looked up in a table: it is accumulated by the machine
//! cycles the instruction performs (4 per opcode fetch, 3 per memory access,
//! 4 per I/O access, plus internal cycles), which documents exactly where
//! the time goes.
//!
//! # Interrupts
//!
//! See the [`interrupts`] module for the details. In short: `/NMI` is
//! edge-triggered and always taken ([`Z80::nmi`]); `/INT` is
//! level-sensitive ([`Z80::set_irq`]) and taken only when `IFF1` is set. `EI`
//! takes effect only after the instruction following it. Three modes decide
//! where an `/INT` jumps: IM 0 executes an instruction supplied by the
//! device, IM 1 calls `0x0038`, IM 2 calls through a vector table at
//! `I * 256 + data`. The data byte comes from [`Bus::interrupt_data`].
//!
//! # Accuracy
//!
//! The core is validated against the SingleStepTests JSON vectors (every
//! documented and undocumented opcode, comparing all registers including
//! WZ and Q, memory, I/O and cycle counts) and against ZEXDOC/ZEXALL.
//! It models an NMOS Z80 (as found in the Mega Drive). It is instruction-
//! granular: bus accesses happen in the right order but not at the exact
//! T-state within the instruction.

#![forbid(unsafe_code)]

mod alu;
mod cpu;
pub mod decode;
pub mod disasm;
pub mod flags;
pub mod interrupts;
mod state;

#[cfg(test)]
mod tests;

pub use cpu::Z80;

/// The CPU's view of the outside world: memory and I/O ports.
///
/// The CPU core is generic over the bus (`step<B: Bus>`), so all calls are
/// statically dispatched and can be inlined into the instruction code.
pub trait Bus {
    /// Read a byte of memory.
    fn read(&mut self, addr: u16) -> u8;
    /// Write a byte of memory.
    fn write(&mut self, addr: u16, value: u8);
    /// Read an I/O port. The Z80 drives all 16 address lines during I/O:
    /// the low byte is the port number and the high byte is `A` (for
    /// `IN A,(n)`) or `B` (for `IN r,(C)` and block I/O).
    fn port_in(&mut self, port: u16) -> u8 {
        let _ = port;
        0xFF
    }
    /// Write an I/O port. The port's high byte is set as for [`Bus::port_in`].
    fn port_out(&mut self, port: u16, value: u8) {
        let _ = (port, value);
    }
    /// Byte placed on the data bus during an interrupt acknowledge (IM 0 opcode / IM 2 vector low byte).
    ///
    /// The default, `0xFF`, is what a floating (pulled-up) bus reads: `RST 38h`
    /// in IM 0.
    fn interrupt_data(&mut self) -> u8 {
        0xFF
    }
}
