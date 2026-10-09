//! Multiplication and division: `MULU`, `MULS`, `DIVU`, `DIVS`.
//!
//! The 68000 multiplies 16 × 16 → 32 bits and divides 32 / 16 → 16-bit
//! quotient and 16-bit remainder (packed as `remainder:quotient` in the
//! destination register). There is no hardware multiplier: both run as
//! shift-and-add / shift-and-subtract loops in microcode, one step per
//! source bit, so their duration depends on the operand values.
//!
//! * `MULU` takes 38 + 2n cycles, n = number of 1 bits in the multiplier.
//! * `MULS` takes 38 + 2n cycles, n = number of `01`/`10` bit pairs in the
//!   multiplier with a 0 appended below it (Booth's algorithm only does work
//!   where the bit pattern changes).
//! * `DIVU`/`DIVS` timing depends on the quotient bits; the functions below
//!   replay the microcode's decisions (algorithm by Jorge Cwik, verified on
//!   hardware) rather than doing the division bit by bit.
//!
//! If the quotient does not fit in 16 bits, the division sets V and leaves
//! the destination unchanged. Division by zero raises an exception
//! (vector 5).

use crate::Bus;
use crate::cpu::M68k;
use crate::decode::Instr;
use crate::ea::Size;
use crate::exceptions::{Exec, vector};

/// Cycles taken by `DIVU`, excluding the effective address calculation.
fn divu_cycles(dividend: u32, divisor: u16) -> u32 {
    let divisor = u32::from(divisor);
    if dividend >> 16 >= divisor {
        // Overflow is detected up front by comparing the high word.
        return 10;
    }
    let mut micro_cycles = 38;
    let shifted_divisor = divisor << 16;
    let mut dividend = dividend;
    for _ in 0..15 {
        let top_bit_set = dividend & 0x8000_0000 != 0;
        dividend <<= 1;
        if top_bit_set {
            dividend = dividend.wrapping_sub(shifted_divisor);
        } else {
            micro_cycles += 2;
            if dividend >= shifted_divisor {
                dividend -= shifted_divisor;
                micro_cycles -= 1;
            }
        }
    }
    micro_cycles * 2
}

/// Cycles taken by `DIVS`, excluding the effective address calculation.
fn divs_cycles(dividend: i32, divisor: i16) -> u32 {
    let mut micro_cycles = 6;
    if dividend < 0 {
        micro_cycles += 1;
    }
    let abs_dividend = dividend.unsigned_abs();
    let abs_divisor = u32::from(divisor.unsigned_abs());
    if abs_dividend >> 16 >= abs_divisor {
        return (micro_cycles + 2) * 2;
    }
    let mut quotient = abs_dividend / abs_divisor;
    let negative = (dividend < 0) != (divisor < 0);
    if quotient > 0x7FFF && !(negative && quotient == 0x8000) {
        // The first quotient bit already shows a signed overflow.
        return (micro_cycles + 2) * 2;
    }
    micro_cycles += 55;
    if divisor >= 0 {
        if dividend >= 0 {
            micro_cycles -= 1;
        } else {
            micro_cycles += 1;
        }
    }
    // One extra micro cycle for each of the top 15 quotient bits that is 0.
    for _ in 0..15 {
        if quotient & 0x8000 == 0 {
            micro_cycles += 1;
        }
        quotient <<= 1;
    }
    micro_cycles * 2
}

impl M68k {
    pub(crate) fn op_mulu<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        let multiplier = self.read_ea(bus, i.src, Size::Word)?;
        let r = i.dst.reg as usize;
        let result = (self.d[r] & 0xFFFF) * multiplier;
        self.d[r] = result;
        self.set_logic_flags(result, Size::Long);
        self.prefetch(bus);
        self.idle(34 + 2 * multiplier.count_ones());
        Ok(())
    }

    pub(crate) fn op_muls<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        let multiplier = self.read_ea(bus, i.src, Size::Word)?;
        let r = i.dst.reg as usize;
        let result = (self.d[r] as i16 as i32).wrapping_mul(multiplier as i16 as i32) as u32;
        self.d[r] = result;
        self.set_logic_flags(result, Size::Long);
        self.prefetch(bus);
        // Bit pairs that differ in (multiplier << 1).
        let booth = (multiplier << 1) ^ (multiplier << 2);
        let changes = (booth & 0x1_FFFE).count_ones();
        self.idle(34 + 2 * changes);
        Ok(())
    }

    /// Division by zero: the exception is taken after 4 more internal cycles,
    /// stacking the address of the next instruction.
    fn divide_by_zero<B: Bus>(&mut self, bus: &mut B) -> Exec {
        self.idle(4);
        self.v = false;
        self.c = false;
        let pc = self.pc;
        self.exception(bus, vector::ZERO_DIVIDE, pc)
    }

    /// Quotient does not fit: V set, C cleared, destination untouched (N and
    /// Z keep their old values).
    fn divide_overflow(&mut self) {
        self.v = true;
        self.c = false;
    }

    pub(crate) fn op_divu<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        let divisor = self.read_ea(bus, i.src, Size::Word)? as u16;
        if divisor == 0 {
            return self.divide_by_zero(bus);
        }
        let r = i.dst.reg as usize;
        let dividend = self.d[r];
        let quotient = dividend / u32::from(divisor);
        if quotient > 0xFFFF {
            self.divide_overflow();
        } else {
            let remainder = dividend % u32::from(divisor);
            self.d[r] = remainder << 16 | quotient;
            self.set_logic_flags(quotient, Size::Word);
        }
        self.prefetch(bus);
        self.idle(divu_cycles(dividend, divisor) - 4);
        Ok(())
    }

    pub(crate) fn op_divs<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        let divisor = self.read_ea(bus, i.src, Size::Word)? as u16 as i16;
        if divisor == 0 {
            return self.divide_by_zero(bus);
        }
        let r = i.dst.reg as usize;
        let dividend = self.d[r] as i32;
        // i64 avoids the i32::MIN / -1 overflow trap.
        let quotient = i64::from(dividend) / i64::from(divisor);
        if quotient != i64::from(quotient as i16) {
            self.divide_overflow();
        } else {
            let remainder = i64::from(dividend) % i64::from(divisor);
            self.d[r] = (remainder as u32) << 16 | (quotient as u32 & 0xFFFF);
            self.set_logic_flags(quotient as u32, Size::Word);
        }
        self.prefetch(bus);
        self.idle(divs_cycles(dividend, divisor) - 4);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn division_timing_bounds() {
        // Documented best/worst cases.
        assert_eq!(divu_cycles(0x0001_0000, 1), 10); // overflow
        assert!(divu_cycles(0, 1) <= 140);
        assert!(divs_cycles(0, 1) <= 158);
    }
}
