//! Binary-coded decimal: `ABCD`, `SBCD`, `NBCD`.
//!
//! In BCD each nibble of a byte holds one decimal digit, so `$42` means
//! forty-two. Games use it for scores and timers because it is trivial to
//! display. The 68000 adds and subtracts BCD bytes in two steps: a plain
//! binary operation, then a *decimal correction* that adds (or subtracts) 6
//! to each digit that went past 9, since a nibble holds 16 values but a
//! digit only 10.
//!
//! X is the decimal carry/borrow (so multi-byte numbers can be processed with
//! `-(An)` operands, from least to most significant byte), and as with `ADDX`
//! Z is only ever cleared.
//!
//! For invalid BCD inputs (nibbles above 9) the result and the officially
//! "undefined" N and V flags follow from exactly how the correction is
//! wired. The formulation below reproduces the real chip bit for bit: the
//! correction is derived from the binary carries out of bits 3 and 7, and
//! V reports whether the correction flipped the top bit.

use crate::Bus;
use crate::cpu::M68k;
use crate::decode::Instr;
use crate::ea::{Mode, Size};
use crate::exceptions::Exec;

/// A BCD result and its carry/overflow flags.
struct Bcd {
    result: u8,
    carry: bool,
    overflow: bool,
}

/// `dst + src + x` in BCD.
fn bcd_add(dst: u8, src: u8, x: bool) -> Bcd {
    let (dst, src) = (u32::from(dst), u32::from(src));
    let binary = dst + src + u32::from(x);
    // Carries out of bit 3 (half carry) and bit 7, from the binary add.
    let carries = ((src & dst) | (!binary & src) | (!binary & dst)) & 0x88;
    // Digits that are above 9 even without a carry.
    let too_big = (((binary + 0x66) ^ binary) & 0x110) >> 1;
    // A 6 for each digit needing correction (0x08 -> 0x06, 0x80 -> 0x60).
    let correction = (carries | too_big) - ((carries | too_big) >> 2);
    let result = binary + correction;
    Bcd {
        result: result as u8,
        carry: (carries | (binary & !result)) & 0x80 != 0,
        overflow: !binary & result & 0x80 != 0,
    }
}

/// `dst - src - x` in BCD.
fn bcd_sub(dst: u8, src: u8, x: bool) -> Bcd {
    let (dst, src) = (u32::from(dst), u32::from(src));
    let binary = dst.wrapping_sub(src).wrapping_sub(u32::from(x));
    // Borrows into bit 3 and bit 7.
    let borrows = ((!dst & src) | (binary & !dst) | (binary & src)) & 0x88;
    let correction = borrows - (borrows >> 2);
    let result = binary.wrapping_sub(correction);
    Bcd {
        result: result as u8,
        carry: (borrows | (!binary & result)) & 0x80 != 0,
        overflow: binary & !result & 0x80 != 0,
    }
}

impl M68k {
    fn set_bcd_flags(&mut self, bcd: &Bcd) {
        self.c = bcd.carry;
        self.x = bcd.carry;
        self.v = bcd.overflow;
        self.n = bcd.result & 0x80 != 0;
        if bcd.result != 0 {
            self.z = false;
        }
    }

    /// ABCD/SBCD share the `Dy,Dx` / `-(Ay),-(Ax)` forms of ADDX.
    fn bcd_pair<B: Bus>(&mut self, bus: &mut B, i: Instr, op: fn(u8, u8, bool) -> Bcd) -> Exec {
        if i.src.mode == Mode::DataReg {
            let src = self.d[i.src.reg as usize] as u8;
            let dst = self.d[i.dst.reg as usize] as u8;
            let bcd = op(dst, src, self.x);
            self.set_bcd_flags(&bcd);
            self.prefetch(bus);
            self.idle(2);
            self.set_d(i.dst.reg, Size::Byte, u32::from(bcd.result));
        } else {
            self.idle(2);
            let src_addr = self.predecrement(i.src.reg, Size::Byte);
            let src = self.read_byte(bus, src_addr);
            let dst_addr = self.predecrement(i.dst.reg, Size::Byte);
            let dst = self.read_byte(bus, dst_addr);
            let bcd = op(dst, src, self.x);
            self.set_bcd_flags(&bcd);
            self.prefetch(bus);
            self.write_byte(bus, dst_addr, bcd.result);
        }
        Ok(())
    }

    pub(crate) fn op_abcd<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        self.bcd_pair(bus, i, bcd_add)
    }

    pub(crate) fn op_sbcd<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        self.bcd_pair(bus, i, bcd_sub)
    }

    /// `NBCD <ea>`: `0 - <ea> - X` in BCD (decimal negate).
    pub(crate) fn op_nbcd<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        let operand = self.resolve(bus, i.dst, Size::Byte);
        let value = self.read_operand(bus, operand, Size::Byte)?;
        let bcd = bcd_sub(0, value as u8, self.x);
        self.set_bcd_flags(&bcd);
        self.prefetch(bus);
        if i.dst.mode == Mode::DataReg {
            self.idle(2);
        }
        self.write_operand(bus, operand, Size::Byte, u32::from(bcd.result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_arithmetic() {
        let r = bcd_add(0x19, 0x28, false);
        assert_eq!((r.result, r.carry), (0x47, false));
        let r = bcd_add(0x99, 0x01, false);
        assert_eq!((r.result, r.carry), (0x00, true));
        let r = bcd_sub(0x42, 0x13, false);
        assert_eq!((r.result, r.carry), (0x29, false));
        let r = bcd_sub(0x00, 0x01, false);
        assert_eq!((r.result, r.carry), (0x99, true));
    }
}
