//! # A Motorola 68000 interpreter
//!
//! The 68000 (1979) is the main CPU of the Mega Drive / Genesis, clocked at
//! ≈7.67 MHz. This crate emulates it instruction by instruction with
//! cycle-exact timing, and is validated against Tom Harte's single-step test
//! vectors (see `tests/tomharte.rs`).
//!
//! This page is a short tour of the chip for readers new to it. Each module
//! then explains one family of instructions in more depth.
//!
//! ## Registers
//!
//! The 68000 is a 32-bit design on a 16-bit data bus with 24 address lines:
//!
//! * eight **data registers** `D0`–`D7` (32 bits). Byte and word operations
//!   only touch the low 8 or 16 bits and leave the rest alone;
//! * eight **address registers** `A0`–`A7` (32 bits). Writes to them always
//!   affect the full 32 bits (a word is sign-extended), and they never change
//!   the condition codes;
//! * `A7` is the **stack pointer**. There are really two of them: the user
//!   stack pointer (USP) and the supervisor stack pointer (SSP), and `A7`
//!   names whichever matches the current privilege mode;
//! * the **program counter**, 32 bits wide internally, but only the low 24
//!   bits reach the address bus (16 MiB address space);
//! * the 16-bit **status register** (SR):
//!
//! ```text
//!   15  14  13  12  11  10   9   8   7   6   5   4   3   2   1   0
//!  [T ][  ][S ][  ][  ][I2][I1][I0][  ][  ][  ][X ][N ][Z ][V ][C ]
//!   |       |            \______/               \_________________/
//!   trace   supervisor   interrupt mask         condition codes (CCR)
//! ```
//!
//! The low byte is the **condition code register**: e**X**tend (a copy of
//! carry used by multi-precision arithmetic: `ADDX`, `SUBX`, `ROXL`, BCD),
//! **N**egative, **Z**ero, o**V**erflow (signed) and **C**arry (unsigned).
//! The high byte is the "system byte" and can only be changed in supervisor
//! mode.
//!
//! ## Instructions and addressing modes
//!
//! Every instruction is one 16-bit opcode word followed by 0–4 extension
//! words (immediates, displacements, absolute addresses). Most instructions
//! take one or two *effective addresses* (EAs), encoded in 6 bits as a 3-bit
//! mode and a 3-bit register (see [`ea`](crate) module docs):
//!
//! | Syntax        | Meaning                                         |
//! |---------------|-------------------------------------------------|
//! | `Dn`, `An`    | the register itself                             |
//! | `(An)`        | memory at the address in `An`                   |
//! | `(An)+`       | same, then increment `An` by the operand size   |
//! | `-(An)`       | decrement `An` first, then access memory        |
//! | `(d16,An)`    | `An` + signed 16-bit displacement               |
//! | `(d8,An,Xn)`  | `An` + signed 8-bit displacement + index reg    |
//! | `(xxx).W/.L`  | absolute address                                |
//! | `(d16,PC)`, `(d8,PC,Xn)` | PC-relative versions (read only)     |
//! | `#imm`        | immediate data from the extension words         |
//!
//! ## The prefetch queue
//!
//! The 68000 overlaps instruction fetch with execution through a two-word
//! queue: `IR` holds the opcode being decoded and `IRC` the word after it.
//! Every instruction ends by fetching the next word into the queue, and
//! consuming an extension word immediately triggers a refill. This is why
//! instruction timings look the way they do (e.g. `NOP` is a single 4-cycle
//! bus read: the fetch of the instruction after next), and why the PC value
//! pushed by some exceptions points "a little further on". This core models
//! the queue exactly; see [`M68k::prefetch_queue`].
//!
//! ## Timing
//!
//! Every bus access takes 4 clock cycles (assuming no wait states); the rest
//! of an instruction's time is internal ALU work. The core counts cycles the
//! same way: 4 per bus access plus explicit internal delays, so the reported
//! counts match Motorola's tables by construction rather than by lookup.
//!
//! ## Exceptions
//!
//! Exceptions are the 68000's unified mechanism for traps, errors and
//! interrupts. On an exception the CPU copies SR, enters supervisor mode,
//! clears trace, pushes the PC and the old SR onto the supervisor stack, then
//! loads a new PC from a 256-entry **vector table** at address 0. Three groups
//! exist, in priority order:
//!
//! * **group 0**: reset, bus error and address error (a word or long access
//!   to an odd address). These abort the current instruction mid-flight and
//!   push a larger 14-byte frame describing the faulting access;
//! * **group 1**: trace, interrupts, illegal / unimplemented opcodes
//!   (line A `$Axxx` and line F `$Fxxx`), privilege violations;
//! * **group 2**: exceptions raised by an instruction doing its job: `TRAP`,
//!   `TRAPV`, `CHK`, division by zero.
//!
//! Interrupts come in on three priority lines (levels 1–7). A level is
//! accepted when it is greater than the mask in SR; level 7 cannot be masked.
//! The Mega Drive uses *autovectors*: the vector number is simply 24 + level.

mod bcd;
mod bit;
mod branch;
mod cpu;
mod data;
mod decode;
pub mod disasm;
mod ea;
mod exceptions;
mod logic;
mod muldiv;
mod shift;
mod system;
mod arith;

pub use cpu::{M68k, RunState};

/// The CPU's view of the outside world.
///
/// Addresses passed are already masked to 24 bits. Word accesses are always
/// to even addresses: the CPU raises an address error itself before an odd
/// word access could reach the bus.
pub trait Bus {
    /// Read one byte.
    fn read_byte(&mut self, addr: u32) -> u8;
    /// Read one big-endian word (`addr` is always even).
    fn read_word(&mut self, addr: u32) -> u16;
    /// Write one byte.
    fn write_byte(&mut self, addr: u32, value: u8);
    /// Write one big-endian word (`addr` is always even).
    fn write_word(&mut self, addr: u32, value: u16);
    /// Interrupt acknowledge cycle for `level` (1..=7). Return `None` for an
    /// autovector (what the Mega Drive uses), or `Some(vector number)`.
    fn interrupt_acknowledge(&mut self, level: u8) -> Option<u8> {
        let _ = level;
        None
    }
    /// The `RESET` instruction pulses the reset line for external devices.
    fn reset_devices(&mut self) {}
}
