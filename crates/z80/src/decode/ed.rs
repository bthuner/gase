//! The `ED` page ("extended" instructions).
//!
//! Only two regions are populated:
//!
//! ```text
//!   01 yyy zzz   I/O on (C), 16-bit ADC/SBC, LD (nn),rp, NEG, RETN/RETI,
//!                IM, LD I/R, RRD/RLD
//!   10 1rr 0dd   block instructions: LDI CPI INI OUTI and the D/IR/DR forms
//! ```
//!
//! Everything else executes as an 8 T-state NOP. The 01 region is fully
//! mirrored: e.g. `NEG` appears 8 times, `IM` and `RETN` several times.

use super::Index;
use crate::alu::block_xy;
use crate::flags::{C, H, N, PV, S, SZXY, Z};
use crate::{Bus, Z80};

/// Interrupt mode set by `ED 46+8y`, indexed by y. The modes for y = 1 and 5
/// are undocumented and behave like IM 0.
const IM_TABLE: [u8; 8] = [0, 0, 1, 2, 0, 0, 1, 2];

impl Z80 {
    pub(crate) fn execute_ed<B: Bus>(&mut self, bus: &mut B, op: u8) {
        let y = (op >> 3) & 7;
        let p = y >> 1;
        match op {
            // IN r,(C). y = 6 is `IN (C)` / `IN F,(C)`: only the flags are kept.
            0x40 | 0x48 | 0x50 | 0x58 | 0x60 | 0x68 | 0x70 | 0x78 => {
                let port = self.bc();
                let v = self.port_in(bus, port);
                self.wz = port.wrapping_add(1);
                self.flags_szxyp_keep_c(v);
                if y != 6 {
                    self.set_reg8(y, Index::Hl, v);
                }
            }
            // OUT (C),r. y = 6 is `OUT (C),0` on NMOS parts (CMOS outputs 0xFF).
            0x41 | 0x49 | 0x51 | 0x59 | 0x61 | 0x69 | 0x71 | 0x79 => {
                let port = self.bc();
                let v = if y == 6 { 0 } else { self.reg8(y, Index::Hl) };
                self.port_out(bus, port, v);
                self.wz = port.wrapping_add(1);
            }

            // SBC HL,rp / ADC HL,rp
            0x42 | 0x52 | 0x62 | 0x72 => {
                self.internal(7);
                self.sbc16(self.rp(p, Index::Hl));
            }
            0x4A | 0x5A | 0x6A | 0x7A => {
                self.internal(7);
                self.adc16(self.rp(p, Index::Hl));
            }

            // LD (nn),rp / LD rp,(nn)
            0x43 | 0x53 | 0x63 | 0x73 => {
                let addr = self.fetch_word(bus);
                self.write_word(bus, addr, self.rp(p, Index::Hl));
                self.wz = addr.wrapping_add(1);
            }
            0x4B | 0x5B | 0x6B | 0x7B => {
                let addr = self.fetch_word(bus);
                let v = self.read_word(bus, addr);
                self.set_rp(p, Index::Hl, v);
                self.wz = addr.wrapping_add(1);
            }

            0x44 | 0x4C | 0x54 | 0x5C | 0x64 | 0x6C | 0x74 | 0x7C => self.neg(),

            // RETN / RETI: both restore IFF1 from IFF2. RETI differs only in
            // being recognised by Z80 peripheral chips watching the bus.
            0x45 | 0x4D | 0x55 | 0x5D | 0x65 | 0x6D | 0x75 | 0x7D => {
                self.iff1 = self.iff2;
                self.pc = self.pop(bus);
                self.wz = self.pc;
            }

            0x46 | 0x4E | 0x56 | 0x5E | 0x66 | 0x6E | 0x76 | 0x7E => self.im = IM_TABLE[y as usize],

            // LD I,A / LD R,A
            0x47 => {
                self.internal(1);
                self.i = self.a;
            }
            0x4F => {
                self.internal(1);
                self.r = self.a;
            }
            // LD A,I / LD A,R: P/V receives IFF2, the only way for software to
            // read the interrupt enable state.
            0x57 | 0x5F => {
                self.internal(1);
                self.a = if op == 0x57 { self.i } else { self.r };
                let pv = if self.iff2 { PV } else { 0 };
                self.set_f((SZXY[self.a as usize]) | pv | (self.f & C));
                self.ld_a_ir = true;
            }

            // RRD / RLD: rotate a 3-nibble number formed by A's low nibble and
            // (HL) right/left by one nibble. Handy for BCD arithmetic.
            0x67 | 0x6F => {
                let addr = self.hl();
                let v = self.read(bus, addr);
                self.internal(4);
                let (mem, a_low) = if op == 0x67 {
                    ((self.a << 4) | (v >> 4), v & 0x0F)
                } else {
                    ((v << 4) | (self.a & 0x0F), v >> 4)
                };
                self.write(bus, addr, mem);
                self.a = (self.a & 0xF0) | a_low;
                self.flags_szxyp_keep_c(self.a);
                self.wz = addr.wrapping_add(1);
            }

            // ED 77 and ED 7F would be "LD I,I"/"LD R,R"-like slots: NOPs.
            0x77 | 0x7F => {}

            // Block instructions. Bit 3 selects decrement, bit 4 repeat.
            0xA0 | 0xA8 | 0xB0 | 0xB8 => self.block_ld(bus, op),
            0xA1 | 0xA9 | 0xB1 | 0xB9 => self.block_cp(bus, op),
            0xA2 | 0xAA | 0xB2 | 0xBA => self.block_in(bus, op),
            0xA3 | 0xAB | 0xB3 | 0xBB => self.block_out(bus, op),

            // Unassigned: an 8 T-state NOP.
            _ => {}
        }
    }

    /// +1 or -1 depending on bit 3 of a block instruction opcode.
    #[inline]
    fn block_step(op: u8) -> u16 {
        if op & 0x08 == 0 { 1 } else { 0xFFFF }
    }

    /// Common tail of a repeating block instruction that has to go round
    /// again: rewind PC to re-execute it (which is what lets interrupts in
    /// between iterations) at a cost of 5 extra T-states.
    #[inline]
    fn block_repeat(&mut self) {
        self.internal(5);
        self.pc = self.pc.wrapping_sub(2);
        self.wz = self.pc.wrapping_add(1);
    }

    /// `LDI LDD LDIR LDDR`: copy (HL) to (DE), step both, decrement BC.
    fn block_ld<B: Bus>(&mut self, bus: &mut B, op: u8) {
        let step = Self::block_step(op);
        let v = self.read(bus, self.hl());
        self.write(bus, self.de(), v);
        self.internal(2);
        self.set_hl(self.hl().wrapping_add(step));
        self.set_de(self.de().wrapping_add(step));
        let bc = self.bc().wrapping_sub(1);
        self.set_bc(bc);

        // X/Y from bits 3 and 1 of (value + A): the ALU computes this sum
        // for no documented reason.
        let n = v.wrapping_add(self.a);
        let pv = if bc != 0 { PV } else { 0 };
        self.set_f((self.f & (S | Z | C)) | block_xy(n) | pv);

        if op & 0x10 != 0 && bc != 0 {
            self.block_repeat();
            self.block_repeat_xy();
        }
    }

    /// `CPI CPD CPIR CPDR`: compare A with (HL), step HL, decrement BC.
    fn block_cp<B: Bus>(&mut self, bus: &mut B, op: u8) {
        let step = Self::block_step(op);
        let v = self.read(bus, self.hl());
        self.internal(5);
        self.set_hl(self.hl().wrapping_add(step));
        let bc = self.bc().wrapping_sub(1);
        self.set_bc(bc);
        self.wz = self.wz.wrapping_add(step);

        let res = self.a.wrapping_sub(v);
        let half = (self.a ^ v ^ res) & H;
        // X/Y come from `A - (HL) - H`, as if the half borrow was applied.
        let n = res.wrapping_sub(half >> 4);
        let pv = if bc != 0 { PV } else { 0 };
        self.set_f((self.f & C) | (SZXY[res as usize] & (S | Z)) | half | N | block_xy(n) | pv);

        if op & 0x10 != 0 && bc != 0 && res != 0 {
            self.block_repeat();
            self.block_repeat_xy();
        }
    }

    /// `INI IND INIR INDR`: input from port (C) to (HL), step HL, decrement B.
    fn block_in<B: Bus>(&mut self, bus: &mut B, op: u8) {
        let step = Self::block_step(op);
        self.internal(1);
        let port = self.bc();
        let v = self.port_in(bus, port);
        self.wz = port.wrapping_add(step);
        self.b = self.b.wrapping_sub(1);
        self.write(bus, self.hl(), v);
        self.set_hl(self.hl().wrapping_add(step));

        let k = v as u16 + self.c.wrapping_add(step as u8) as u16;
        self.block_io_flags(v, k);

        if op & 0x10 != 0 && self.b != 0 {
            self.block_repeat();
            self.block_io_repeat_flags(v);
        }
    }

    /// `OUTI OUTD OTIR OTDR`: output (HL) to port (C), step HL, decrement B.
    /// B is decremented *before* the output, so the port's high byte is the
    /// new B.
    fn block_out<B: Bus>(&mut self, bus: &mut B, op: u8) {
        let step = Self::block_step(op);
        self.internal(1);
        let v = self.read(bus, self.hl());
        self.b = self.b.wrapping_sub(1);
        let port = self.bc();
        self.port_out(bus, port, v);
        self.wz = port.wrapping_add(step);
        self.set_hl(self.hl().wrapping_add(step));

        let k = v as u16 + self.l as u16;
        self.block_io_flags(v, k);

        if op & 0x10 != 0 && self.b != 0 {
            self.block_repeat();
            self.block_io_repeat_flags(v);
        }
    }
}
