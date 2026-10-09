//! Shifts and rotates: `ASL`, `ASR`, `LSL`, `LSR`, `ROL`, `ROR`, `ROXL`,
//! `ROXR`.
//!
//! ```text
//!  LSL:  C,X <- [  value  ] <- 0          LSR:  0 -> [  value  ] -> C,X
//!  ASL:  C,X <- [  value  ] <- 0          ASR:  [s]-> [ value  ] -> C,X  (sign copied in)
//!  ROL:  C <- [  value  ] <-+             ROR:  +-> [  value  ] -> C
//!              +-------------+                  +-----------------+
//!  ROXL: C,X <- [ value ] <- X            ROXR: X -> [ value ] -> C,X  (X is a 33rd bit)
//! ```
//!
//! `ASL` and `LSL` produce the same result; they differ only in V, which
//! `ASL` sets if the sign bit changed at any point during the shift (i.e. if
//! the value no longer fits as a signed number).
//!
//! Register forms shift by 1–8 (encoded in the opcode) or by a data register
//! modulo 64. The 68000 shifts one bit per 2 cycles, so the time depends on
//! the count: 6 + 2n cycles for bytes and words, 8 + 2n for longs. Memory
//! forms always shift a word by one bit.
//!
//! With a count of zero nothing moves: C is cleared (except for `ROXL`/`ROXR`,
//! where C takes the value of X) and X is unchanged.

use crate::Bus;
use crate::cpu::M68k;
use crate::decode::{Instr, ShiftKind};
use crate::ea::{Mode, Size};
use crate::exceptions::Exec;

/// Bit `n` of `value` (0 for `n` ≥ 32).
#[inline]
fn bit(value: u32, n: u32) -> bool {
    n < 32 && (value >> n) & 1 != 0
}

impl M68k {
    /// Shift `value` (already masked to `size`) by `count`, setting the
    /// flags, and return the result.
    pub(crate) fn shift(&mut self, kind: ShiftKind, value: u32, count: u32, size: Size) -> u32 {
        let bits = size.bits();
        let mask = size.mask();
        let msb = size.msb();
        let mut v = false;
        let result = if count == 0 {
            self.c = match kind {
                ShiftKind::Roxl | ShiftKind::Roxr => self.x,
                _ => false,
            };
            value
        } else {
            match kind {
                ShiftKind::Asl | ShiftKind::Lsl => {
                    let carry = count <= bits && bit(value, bits - count);
                    let result = if count < bits { (value << count) & mask } else { 0 };
                    if kind == ShiftKind::Asl {
                        // V: did the top `count + 1` bits differ from each other?
                        v = if count >= bits {
                            value != 0
                        } else {
                            let top = (mask << (bits - 1 - count)) & mask;
                            let t = value & top;
                            t != 0 && t != top
                        };
                    }
                    self.c = carry;
                    self.x = carry;
                    result
                }
                ShiftKind::Asr => {
                    let negative = value & msb != 0;
                    let carry = if count <= bits { bit(value, count - 1) } else { negative };
                    let signed = size.sign_extend(value) as i32;
                    let result = (signed >> count.min(31)) as u32 & mask;
                    self.c = carry;
                    self.x = carry;
                    result
                }
                ShiftKind::Lsr => {
                    let carry = count <= bits && bit(value, count - 1);
                    let result = if count < bits { value >> count } else { 0 };
                    self.c = carry;
                    self.x = carry;
                    result
                }
                ShiftKind::Rol => {
                    let n = count % bits;
                    let result = if n == 0 { value } else { ((value << n) | (value >> (bits - n))) & mask };
                    self.c = result & 1 != 0;
                    result
                }
                ShiftKind::Ror => {
                    let n = count % bits;
                    let result = if n == 0 { value } else { ((value >> n) | (value << (bits - n))) & mask };
                    self.c = result & msb != 0;
                    result
                }
                ShiftKind::Roxl | ShiftKind::Roxr => {
                    // Rotate through a (bits + 1)-bit quantity X:value.
                    let n = count % (bits + 1);
                    let wide = u64::from(self.x) << bits | u64::from(value);
                    let width_mask = (1u64 << (bits + 1)) - 1;
                    let rotated = if n == 0 {
                        wide
                    } else if kind == ShiftKind::Roxl {
                        ((wide << n) | (wide >> (bits + 1 - n))) & width_mask
                    } else {
                        ((wide >> n) | (wide << (bits + 1 - n))) & width_mask
                    };
                    self.x = (rotated >> bits) & 1 != 0;
                    self.c = self.x;
                    rotated as u32 & mask
                }
            }
        };
        self.v = v;
        self.set_nz(result, size);
        result
    }

    /// `<shift> #n,Dy` / `<shift> Dx,Dy`.
    pub(crate) fn op_shift_register<B: Bus>(&mut self, bus: &mut B, i: Instr, kind: ShiftKind) -> Exec {
        let count = if i.src.mode == Mode::DataReg {
            self.d[i.src.reg as usize] % 64
        } else {
            u32::from(i.src.reg)
        };
        let r = i.dst.reg;
        let value = self.d[r as usize] & i.size.mask();
        let result = self.shift(kind, value, count, i.size);
        self.set_d(r, i.size, result);
        self.prefetch(bus);
        self.idle(if i.size == Size::Long { 4 } else { 2 } + 2 * count);
        Ok(())
    }

    /// `<shift> <ea>`: shift a word in memory by one bit.
    pub(crate) fn op_shift_memory<B: Bus>(&mut self, bus: &mut B, i: Instr, kind: ShiftKind) -> Exec {
        let operand = self.resolve(bus, i.dst, Size::Word);
        let value = self.read_operand(bus, operand, Size::Word)?;
        let result = self.shift(kind, value, 1, Size::Word);
        self.prefetch(bus);
        self.write_operand(bus, operand, Size::Word, result)
    }
}
