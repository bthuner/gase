//! The `CB` page (bit operations) and its indexed form `DD CB d op` /
//! `FD CB d op`.
//!
//! ```text
//!   00 yyy zzz   rotate/shift[y] r[z]    RLC RRC RL RR SLA SRA SLL SRL
//!   01 yyy zzz   BIT y,r[z]
//!   10 yyy zzz   RES y,r[z]
//!   11 yyy zzz   SET y,r[z]
//! ```

use super::Index;
use crate::{Bus, Z80};

impl Z80 {
    /// Apply CB operation `op` to `v`. Returns the new value, or `None` for
    /// `BIT`, which writes nothing back.
    #[inline]
    fn cb_operation(&mut self, op: u8, v: u8, bit_xy_source: u8) -> Option<u8> {
        let y = (op >> 3) & 7;
        match op >> 6 {
            0 => Some(self.rot_shift(y, v)),
            1 => {
                self.bit(y, v, bit_xy_source);
                None
            }
            2 => Some(v & !(1 << y)),
            _ => Some(v | (1 << y)),
        }
    }

    pub(crate) fn execute_cb<B: Bus>(&mut self, bus: &mut B, op: u8) {
        let z = op & 7;
        if z == 6 {
            // (HL): read, 1 T-state to operate, write back. `BIT n,(HL)` takes
            // its X/Y flags from WZ: a famous leak of internal state.
            let addr = self.hl();
            let v = self.read(bus, addr);
            self.internal(1);
            let xy_source = (self.wz >> 8) as u8;
            if let Some(res) = self.cb_operation(op, v, xy_source) {
                self.write(bus, addr, res);
            }
        } else {
            let v = self.reg8(z, Index::Hl);
            if let Some(res) = self.cb_operation(op, v, v) {
                self.set_reg8(z, Index::Hl, res);
            }
        }
    }

    /// `DD CB d op`. Note the unusual byte order: the displacement comes
    /// *before* the final opcode byte, and that opcode byte is read as plain
    /// data (no M1 cycle, so `R` only advances twice for the whole
    /// instruction).
    ///
    /// Every operation works on `(IX+d)`. In the undocumented encodings where
    /// `z` is not 6, the result is *also* copied into register `r[z]`
    /// (the real H/L, not IXH/IXL), e.g. `DD CB 05 00` is
    /// `LD B,RLC (IX+5)`. `BIT` takes X/Y from the high byte of `IX+d`.
    pub(crate) fn execute_index_cb<B: Bus>(&mut self, bus: &mut B, index: Index) {
        let addr = self.fetch_displaced(bus, self.index_reg(index));
        let op = self.fetch_byte(bus);
        // The address addition overlaps the opcode read.
        self.internal(2);
        self.wz = addr;
        let v = self.read(bus, addr);
        self.internal(1);
        if let Some(res) = self.cb_operation(op, v, (addr >> 8) as u8) {
            self.write(bus, addr, res);
            let z = op & 7;
            if z != 6 {
                self.set_reg8(z, Index::Hl, res);
            }
        }
    }
}
