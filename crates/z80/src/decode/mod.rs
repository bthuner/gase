//! Instruction decoding and execution, one module per opcode page.
//!
//! The Z80 instruction set is the 8080's 256 opcodes plus four prefix bytes
//! that open further pages:
//!
//! | Prefix  | Page                                         | Module        |
//! |---------|----------------------------------------------|---------------|
//! | (none)  | 8080-compatible base set, plus `EX`, `DJNZ`, `JR` | [`base`] |
//! | `CB`    | rotates/shifts, `BIT`, `RES`, `SET`          | [`cb`]        |
//! | `ED`    | 16-bit ADC/SBC, block moves, I/O, `IM`, ...  | [`ed`]        |
//! | `DD`/`FD` | the base page with `HL` replaced by `IX`/`IY` | [`base`]   |
//! | `DD CB`/`FD CB` | `CB` page on `(IX+d)`/`(IY+d)`       | [`cb`]        |
//!
//! Opcodes are commonly read as octal fields, which makes the regular
//! structure of the instruction set visible:
//!
//! ```text
//!   7 6 5 4 3 2 1 0
//!   x x y y y z z z        y = p p q   (p: register pair, q: 0/1 variant)
//! ```
//!
//! For example `01 yyy zzz` is `LD r[y],r[z]` and `10 yyy zzz` is
//! `ALU[y] A,r[z]`, where the register code is
//! `0=B 1=C 2=D 3=E 4=H 5=L 6=(HL) 7=A`.
//!
//! Each page is a plain `match` on the opcode byte, which the compiler turns
//! into a jump table, so decoding costs one indirect branch per byte.

mod base;
mod cb;
mod ed;

use crate::{Bus, Z80};

/// Which register plays the role of `HL` in the current instruction.
///
/// A `DD` or `FD` prefix does not select new instructions; it makes the next
/// instruction use `IX` or `IY` wherever it would use `HL`, `H` or `L`,
/// and turns `(HL)` into `(IX+d)` / `(IY+d)`. Using `H`/`L` with a prefix
/// gives the undocumented halves `IXH`, `IXL`, `IYH`, `IYL`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Index {
    Hl,
    Ix,
    Iy,
}

impl Z80 {
    /// Execute the instruction whose first byte `op` has just been fetched.
    #[inline]
    pub(crate) fn execute<B: Bus>(&mut self, bus: &mut B, op: u8) {
        match op {
            0xCB => {
                let op = self.fetch_opcode(bus);
                self.execute_cb(bus, op);
            }
            0xED => {
                let op = self.fetch_opcode(bus);
                self.execute_ed(bus, op);
            }
            0xDD => self.execute_indexed(bus, Index::Ix),
            0xFD => self.execute_indexed(bus, Index::Iy),
            _ => self.execute_base(bus, op, Index::Hl),
        }
    }

    /// After a `DD` or `FD` prefix.
    fn execute_indexed<B: Bus>(&mut self, bus: &mut B, mut index: Index) {
        // As far as the Q latch is concerned a prefix is an instruction of
        // its own, one that does not touch the flags: `DD SCF` sees Q = 0.
        self.q = 0;
        loop {
            let op = self.fetch_opcode(bus);
            match op {
                // A run of prefixes: only the last one counts, the others
                // behave as 4 T-state NOPs (and interrupts are not accepted
                // in between).
                0xDD => index = Index::Ix,
                0xFD => index = Index::Iy,
                // DD/FD have no effect on the ED page.
                0xED => {
                    let op = self.fetch_opcode(bus);
                    return self.execute_ed(bus, op);
                }
                0xCB => return self.execute_index_cb(bus, index),
                _ => return self.execute_base(bus, op, index),
            }
        }
    }

    /// The register that stands in for `HL`.
    #[inline]
    pub(crate) fn index_reg(&self, index: Index) -> u16 {
        match index {
            Index::Hl => self.hl(),
            Index::Ix => self.ix,
            Index::Iy => self.iy,
        }
    }

    #[inline]
    pub(crate) fn set_index_reg(&mut self, index: Index, v: u16) {
        match index {
            Index::Hl => self.set_hl(v),
            Index::Ix => self.ix = v,
            Index::Iy => self.iy = v,
        }
    }

    /// Read 8-bit register `code` (0..=7 except 6, see the module docs), with
    /// `H`/`L` replaced by the halves of the index register.
    #[inline]
    pub(crate) fn reg8(&self, code: u8, index: Index) -> u8 {
        match code {
            0 => self.b,
            1 => self.c,
            2 => self.d,
            3 => self.e,
            4 => (self.index_reg(index) >> 8) as u8,
            5 => self.index_reg(index) as u8,
            7 => self.a,
            _ => unreachable!("register code 6 is a memory operand"),
        }
    }

    #[inline]
    pub(crate) fn set_reg8(&mut self, code: u8, index: Index, v: u8) {
        match code {
            0 => self.b = v,
            1 => self.c = v,
            2 => self.d = v,
            3 => self.e = v,
            4 => {
                let r = self.index_reg(index);
                self.set_index_reg(index, (r & 0x00FF) | ((v as u16) << 8));
            }
            5 => {
                let r = self.index_reg(index);
                self.set_index_reg(index, (r & 0xFF00) | v as u16);
            }
            7 => self.a = v,
            _ => unreachable!("register code 6 is a memory operand"),
        }
    }

    /// 16-bit register pair `p` as used by `LD rp,nn`, `INC rp`, `ADD HL,rp`:
    /// `0=BC 1=DE 2=HL 3=SP`.
    #[inline]
    pub(crate) fn rp(&self, p: u8, index: Index) -> u16 {
        match p & 3 {
            0 => self.bc(),
            1 => self.de(),
            2 => self.index_reg(index),
            _ => self.sp,
        }
    }

    #[inline]
    pub(crate) fn set_rp(&mut self, p: u8, index: Index, v: u16) {
        match p & 3 {
            0 => self.set_bc(v),
            1 => self.set_de(v),
            2 => self.set_index_reg(index, v),
            _ => self.sp = v,
        }
    }

    /// Condition `y` of `JP cc`, `CALL cc`, `RET cc`:
    /// `NZ Z NC C PO PE P M`.
    #[inline]
    pub(crate) fn condition(&self, y: u8) -> bool {
        use crate::flags::{C, PV, S, Z};
        let flag = [Z, C, PV, S][(y >> 1) as usize & 3];
        (self.f & flag != 0) == (y & 1 != 0)
    }
}
