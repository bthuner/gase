//! Single-bit operations: `BTST`, `BCHG`, `BCLR`, `BSET`, and `TAS`.
//!
//! The bit number comes from a data register ("dynamic") or from an
//! extension word ("static"). On a data register all 32 bits are reachable
//! (bit number modulo 32); in memory the operand is a single byte (bit number
//! modulo 8). Z is set to the *inverse* of the bit's previous value, so
//! `BTST` followed by `BEQ` branches when the bit was clear. No other flags
//! change.
//!
//! On a register the 68000 takes 2 extra cycles to change a bit in the upper
//! word, and `BCLR` takes 2 more than `BCHG`/`BSET` (it is microcoded
//! differently).
//!
//! `TAS` ("test and set") tests a byte, then sets its top bit, using an
//! *indivisible* read-modify-write bus cycle that no other bus master can
//! interrupt: a primitive for multiprocessor locks. (On the Mega Drive the
//! bus arbiter does not support that cycle for main RAM, so the write is
//! lost; that is the system's business, not the CPU's.)

use crate::Bus;
use crate::cpu::M68k;
use crate::decode::{Instr, Op};
use crate::ea::{Mode, Size};
use crate::exceptions::Exec;

impl M68k {
    pub(crate) fn op_bit<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        let bit_number = if i.src.mode == Mode::DataReg {
            self.d[i.src.reg as usize]
        } else {
            u32::from(self.read_ext(bus) & 0xFF)
        };

        if i.dst.mode == Mode::DataReg {
            let r = i.dst.reg as usize;
            let bit = bit_number % 32;
            let mask = 1u32 << bit;
            self.z = self.d[r] & mask == 0;
            self.prefetch(bus);
            let upper = u32::from(bit >= 16) * 2;
            match i.op {
                Op::Btst => self.idle(2),
                Op::Bchg => {
                    self.d[r] ^= mask;
                    self.idle(2 + upper);
                }
                Op::Bclr => {
                    self.d[r] &= !mask;
                    self.idle(4 + upper);
                }
                _ => {
                    self.d[r] |= mask;
                    self.idle(2 + upper);
                }
            }
            return Ok(());
        }

        let mask = 1u32 << (bit_number % 8);
        let operand = self.resolve(bus, i.dst, Size::Byte);
        let value = self.read_operand(bus, operand, Size::Byte)?;
        self.z = value & mask == 0;
        let result = match i.op {
            Op::Btst => {
                self.prefetch(bus);
                if i.dst.mode == Mode::Immediate {
                    self.idle(2);
                }
                return Ok(());
            }
            Op::Bchg => value ^ mask,
            Op::Bclr => value & !mask,
            _ => value | mask,
        };
        self.prefetch(bus);
        self.write_operand(bus, operand, Size::Byte, result)
    }

    pub(crate) fn op_tas<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        let operand = self.resolve(bus, i.dst, Size::Byte);
        let value = self.read_operand(bus, operand, Size::Byte)?;
        self.set_logic_flags(value, Size::Byte);
        if i.dst.is_memory() {
            // The read-modify-write cycle: 2 cycles between read and write.
            self.idle(2);
            self.write_operand(bus, operand, Size::Byte, value | 0x80)?;
            self.prefetch(bus);
        } else {
            self.write_operand(bus, operand, Size::Byte, value | 0x80)?;
            self.prefetch(bus);
        }
        Ok(())
    }
}
