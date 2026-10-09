//! The arithmetic and logic unit: every operation that computes flags.
//!
//! A few tricks recur throughout:
//!
//! * **Half carry** (`H`): bit 4 of `a ^ b ^ result` is set exactly when a
//!   carry (or borrow) crossed from bit 3 into bit 4, because XOR-ing the
//!   operands out of the sum leaves only the carry bits.
//! * **Overflow** (`PV` for arithmetic): a signed overflow happened when both
//!   operands of an addition have the same sign and the result's sign
//!   differs. For a subtraction, when the operands' signs differ and the
//!   result's sign differs from the minuend's.
//! * **X/Y**: bits 3 and 5 of the result, taken from [`SZXY`] together with
//!   `S` and `Z`. The exceptions (`CP`, `BIT`, `SCF`/`CCF`, 16-bit adds, block
//!   instructions) are commented where they happen.

use crate::Z80;
use crate::flags::{C, H, N, PV, S, SZXY, SZXYP, X, XY, Y, Z, parity};

impl Z80 {
    /// `ADD A,v` (`carry` = 0) and `ADC A,v` (`carry` = 0 or 1).
    #[inline]
    pub(crate) fn add_a(&mut self, v: u8, carry: u8) {
        let a = self.a;
        let wide = a as u16 + v as u16 + carry as u16;
        let res = wide as u8;
        let overflow = ((a ^ res) & (v ^ res) & 0x80) >> 5;
        self.a = res;
        self.set_f(SZXY[res as usize] | ((a ^ v ^ res) & H) | overflow | (wide >> 8) as u8);
    }

    /// Shared core of `SUB`, `SBC`, `CP` and `NEG`: returns the difference and
    /// its flags, leaving `A` alone.
    #[inline]
    fn sub_flags(a: u8, v: u8, carry: u8) -> (u8, u8) {
        let wide = (a as u16).wrapping_sub(v as u16).wrapping_sub(carry as u16);
        let res = wide as u8;
        let overflow = ((a ^ v) & (a ^ res) & 0x80) >> 5;
        let borrow = (wide >> 8) as u8 & C;
        (
            res,
            SZXY[res as usize] | ((a ^ v ^ res) & H) | overflow | N | borrow,
        )
    }

    /// `SUB v` (`carry` = 0) and `SBC A,v`.
    #[inline]
    pub(crate) fn sub_a(&mut self, v: u8, carry: u8) {
        let (res, f) = Self::sub_flags(self.a, v, carry);
        self.a = res;
        self.set_f(f);
    }

    /// `CP v`: a subtraction that only keeps the flags. Unusually, X and Y are
    /// copied from the *operand*, not from the (discarded) result.
    #[inline]
    pub(crate) fn cp_a(&mut self, v: u8) {
        let (_, f) = Self::sub_flags(self.a, v, 0);
        self.set_f((f & !XY) | (v & XY));
    }

    #[inline]
    pub(crate) fn and_a(&mut self, v: u8) {
        self.a &= v;
        // AND sets H, OR and XOR clear it: an artefact of how the ALU shares
        // its carry logic between operations.
        self.set_f(SZXYP[self.a as usize] | H);
    }

    #[inline]
    pub(crate) fn xor_a(&mut self, v: u8) {
        self.a ^= v;
        self.set_f(SZXYP[self.a as usize]);
    }

    #[inline]
    pub(crate) fn or_a(&mut self, v: u8) {
        self.a |= v;
        self.set_f(SZXYP[self.a as usize]);
    }

    /// The eight accumulator operations, selected by bits 5..3 of the opcode
    /// (`ADD ADC SUB SBC AND XOR OR CP`).
    #[inline]
    pub(crate) fn alu(&mut self, op: u8, v: u8) {
        let carry = self.f & C;
        match op & 7 {
            0 => self.add_a(v, 0),
            1 => self.add_a(v, carry),
            2 => self.sub_a(v, 0),
            3 => self.sub_a(v, carry),
            4 => self.and_a(v),
            5 => self.xor_a(v),
            6 => self.or_a(v),
            _ => self.cp_a(v),
        }
    }

    /// `INC r`: like `ADD r,1` but the carry flag is preserved, so that `INC`
    /// can drive multi-byte loops without disturbing a running carry.
    #[inline]
    pub(crate) fn inc8(&mut self, v: u8) -> u8 {
        let res = v.wrapping_add(1);
        let mut f = (self.f & C) | SZXY[res as usize];
        if v & 0x0F == 0x0F {
            f |= H;
        }
        if v == 0x7F {
            f |= PV;
        }
        self.set_f(f);
        res
    }

    /// `DEC r`; carry preserved like `INC`.
    #[inline]
    pub(crate) fn dec8(&mut self, v: u8) -> u8 {
        let res = v.wrapping_sub(1);
        let mut f = (self.f & C) | SZXY[res as usize] | N;
        if v & 0x0F == 0 {
            f |= H;
        }
        if v == 0x80 {
            f |= PV;
        }
        self.set_f(f);
        res
    }

    /// `ADD HL,rp` (and `ADD IX,rp`). Only H, N, C and X/Y change; X/Y come
    /// from the high byte of the result, because the CPU performs the
    /// addition as two 8-bit additions and the second one sets the flags.
    #[inline]
    pub(crate) fn add16(&mut self, a: u16, b: u16) -> u16 {
        let wide = a as u32 + b as u32;
        let res = wide as u16;
        let hi = (res >> 8) as u8;
        let half = (((a ^ b ^ res) >> 8) as u8) & H;
        self.wz = a.wrapping_add(1);
        self.set_f((self.f & (S | Z | PV)) | (hi & XY) | half | (wide >> 16) as u8);
        res
    }

    /// `ADC HL,rp`: unlike `ADD HL,rp` it sets every flag, with S, Z and
    /// overflow computed on 16 bits.
    #[inline]
    pub(crate) fn adc16(&mut self, b: u16) {
        let a = self.hl();
        let wide = a as u32 + b as u32 + (self.f & C) as u32;
        let res = wide as u16;
        let hi = (res >> 8) as u8;
        let mut f = (hi & (S | XY)) | ((((a ^ b ^ res) >> 8) as u8) & H) | (wide >> 16) as u8;
        if res == 0 {
            f |= Z;
        }
        if (a ^ res) & (b ^ res) & 0x8000 != 0 {
            f |= PV;
        }
        self.wz = a.wrapping_add(1);
        self.set_hl(res);
        self.set_f(f);
    }

    /// `SBC HL,rp`.
    #[inline]
    pub(crate) fn sbc16(&mut self, b: u16) {
        let a = self.hl();
        let wide = (a as u32)
            .wrapping_sub(b as u32)
            .wrapping_sub((self.f & C) as u32);
        let res = wide as u16;
        let hi = (res >> 8) as u8;
        let mut f = (hi & (S | XY)) | ((((a ^ b ^ res) >> 8) as u8) & H) | N;
        if wide > 0xFFFF {
            f |= C;
        }
        if res == 0 {
            f |= Z;
        }
        if (a ^ b) & (a ^ res) & 0x8000 != 0 {
            f |= PV;
        }
        self.wz = a.wrapping_add(1);
        self.set_hl(res);
        self.set_f(f);
    }

    /// `DAA`: turn the binary result of a BCD addition or subtraction (as
    /// remembered by N, H and C) back into valid BCD.
    pub(crate) fn daa(&mut self) {
        let a = self.a;
        let mut correction = 0;
        let mut carry = self.f & C;
        if self.f & H != 0 || a & 0x0F > 9 {
            correction |= 0x06;
        }
        if carry != 0 || a > 0x99 {
            correction |= 0x60;
            carry = C;
        }
        let res = if self.f & N != 0 {
            a.wrapping_sub(correction)
        } else {
            a.wrapping_add(correction)
        };
        self.a = res;
        self.set_f(SZXYP[res as usize] | ((a ^ res) & H) | (self.f & N) | carry);
    }

    /// `CPL`: complement A.
    pub(crate) fn cpl(&mut self) {
        self.a = !self.a;
        self.set_f((self.f & (S | Z | PV | C)) | H | N | (self.a & XY));
    }

    /// `NEG`: A = 0 - A.
    pub(crate) fn neg(&mut self) {
        let (res, f) = Self::sub_flags(0, self.a, 0);
        self.a = res;
        self.set_f(f);
    }

    /// X/Y for `SCF` and `CCF`. If the previous instruction changed the flags
    /// (`Q` = F), X/Y are copied from A. Otherwise they are the OR of A and the
    /// old flags. Real silicon does this because the flag latches are only
    /// reloaded from the ALU bus when an instruction updated them; see Patrik
    /// Rak's 2018 research on the "Q" register.
    fn scf_ccf_xy(&self) -> u8 {
        ((self.q ^ self.f) | self.a) & XY
    }

    /// `SCF`: set carry.
    pub(crate) fn scf(&mut self) {
        self.set_f((self.f & (S | Z | PV)) | self.scf_ccf_xy() | C);
    }

    /// `CCF`: complement carry; the old carry goes to H.
    pub(crate) fn ccf(&mut self) {
        let old_c = self.f & C;
        self.set_f((self.f & (S | Z | PV)) | self.scf_ccf_xy() | (old_c << 4) | (old_c ^ C));
    }

    /// The fast accumulator rotates `RLCA RRCA RLA RRA` (selected by `op`
    /// 0..=3). They leave S, Z and PV alone, unlike their `CB`-prefixed
    /// cousins.
    pub(crate) fn rotate_a(&mut self, op: u8) {
        let a = self.a;
        let (res, carry) = match op {
            0 => (a.rotate_left(1), a >> 7),
            1 => (a.rotate_right(1), a & 1),
            2 => ((a << 1) | (self.f & C), a >> 7),
            _ => ((a >> 1) | ((self.f & C) << 7), a & 1),
        };
        self.a = res;
        self.set_f((self.f & (S | Z | PV)) | (res & XY) | carry);
    }

    /// The `CB`-prefixed rotates and shifts, selected by `op` 0..=7:
    /// `RLC RRC RL RR SLA SRA SLL SRL`. `SLL` is undocumented: it shifts left
    /// and sets bit 0 (it is "SLA with the wrong constant").
    #[inline]
    pub(crate) fn rot_shift(&mut self, op: u8, v: u8) -> u8 {
        let cin = self.f & C;
        let (res, carry) = match op & 7 {
            0 => (v.rotate_left(1), v >> 7),
            1 => (v.rotate_right(1), v & 1),
            2 => ((v << 1) | cin, v >> 7),
            3 => ((v >> 1) | (cin << 7), v & 1),
            4 => (v << 1, v >> 7),
            5 => ((v >> 1) | (v & 0x80), v & 1),
            6 => ((v << 1) | 1, v >> 7),
            _ => (v >> 1, v & 1),
        };
        self.set_f(SZXYP[res as usize] | carry);
        res
    }

    /// `BIT n,v`. Z and PV are both set when the bit is clear; S only when
    /// testing bit 7 and it is set. `xy_source` supplies X/Y: the register
    /// itself for `BIT n,r`, but for `BIT n,(HL)` the CPU has nothing better
    /// on its internal bus than the high byte of WZ.
    #[inline]
    pub(crate) fn bit(&mut self, n: u8, v: u8, xy_source: u8) {
        let masked = v & (1 << n);
        let mut f = (self.f & C) | H | (xy_source & XY) | (masked & S);
        if masked == 0 {
            f |= Z | PV;
        }
        self.set_f(f);
    }

    /// Flags for `IN r,(C)`, `LD A,I`-style loads and `RRD`/`RLD`: S, Z, X/Y
    /// (and parity, if wanted) from `v`; H and N cleared; C preserved.
    #[inline]
    pub(crate) fn flags_szxyp_keep_c(&mut self, v: u8) {
        self.set_f(SZXYP[v as usize] | (self.f & C));
    }

    /// Flags common to the block I/O instructions `INI IND OUTI OUTD` and
    /// their repeating forms. `value` is the byte transferred and `k` the sum
    /// the hardware computes from it (`value + (C±1)` for input, `value + L`
    /// for output). These formulas were reverse-engineered, not documented.
    pub(crate) fn block_io_flags(&mut self, value: u8, k: u16) {
        let mut f = SZXY[self.b as usize] | ((value >> 6) & N);
        if k > 0xFF {
            f |= H | C;
        }
        f |= parity((k as u8 & 7) ^ self.b);
        self.set_f(f);
    }

    /// Extra flag changes when a repeating block I/O instruction (`INIR`,
    /// `OTDR`, ...) loops: the interrupted instruction leaves the results of
    /// a further, internal B increment/decrement in H and PV. Research by
    /// David Banks and others (2018).
    pub(crate) fn block_io_repeat_flags(&mut self, value: u8) {
        let mut f = (self.f & !(XY | H)) | ((self.pc >> 8) as u8 & XY);
        let b = self.b;
        // Toggle PV when the parity of `x` is odd.
        let toggle_pv = |f: &mut u8, x: u8| *f ^= (parity(x) ^ PV) & PV;
        if self.f & C != 0 {
            if value & 0x80 != 0 {
                toggle_pv(&mut f, b.wrapping_sub(1) & 7);
                if b & 0x0F == 0x00 {
                    f |= H;
                }
            } else {
                toggle_pv(&mut f, b.wrapping_add(1) & 7);
                if b & 0x0F == 0x0F {
                    f |= H;
                }
            }
        } else {
            f |= self.f & H;
            toggle_pv(&mut f, b & 7);
        }
        self.set_f(f);
    }

    /// When a repeating block instruction loops, X and Y are taken from the
    /// high byte of PC (which by then points back at the instruction).
    #[inline]
    pub(crate) fn block_repeat_xy(&mut self) {
        let f = (self.f & !XY) | ((self.pc >> 8) as u8 & XY);
        self.set_f(f);
    }
}

/// X/Y of `LDI`/`CPI`-style instructions come from a strange sum `n`:
/// Y is bit 1 of `n` and X is bit 3.
#[inline]
pub(crate) fn block_xy(n: u8) -> u8 {
    (n & X) | ((n << 4) & Y)
}
