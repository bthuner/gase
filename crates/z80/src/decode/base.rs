//! The unprefixed page, also executed after `DD`/`FD` with `HL` swapped for
//! an index register.

use super::Index;
use crate::{Bus, Z80};

impl Z80 {
    /// Address of the memory operand `(HL)`, or `(IX+d)`/`(IY+d)` after a
    /// prefix. Fetching `d` and adding it costs 3 + 5 T-states, and the sum
    /// is latched in WZ.
    #[inline]
    pub(crate) fn mem_operand<B: Bus>(&mut self, bus: &mut B, index: Index) -> u16 {
        match index {
            Index::Hl => self.hl(),
            Index::Ix | Index::Iy => {
                let addr = self.fetch_displaced(bus, self.index_reg(index));
                self.internal(5);
                self.wz = addr;
                addr
            }
        }
    }

    /// `JR`/`DJNZ`: the displacement is always read; if the jump is taken,
    /// 5 more T-states go to adding it to PC.
    #[inline]
    fn jump_relative<B: Bus>(&mut self, bus: &mut B, taken: bool) {
        let d = self.fetch_byte(bus) as i8;
        if taken {
            self.internal(5);
            self.pc = self.pc.wrapping_add_signed(d.into());
            self.wz = self.pc;
        }
    }

    pub(crate) fn execute_base<B: Bus>(&mut self, bus: &mut B, op: u8, index: Index) {
        let y = (op >> 3) & 7;
        let z = op & 7;
        let p = y >> 1;
        match op {
            0x00 => {} // NOP

            // LD rp,nn
            0x01 | 0x11 | 0x21 | 0x31 => {
                let v = self.fetch_word(bus);
                self.set_rp(p, index, v);
            }

            // LD (BC),A / LD (DE),A. WZ's high byte receives A: the CPU puts
            // A on the data bus while computing the next address.
            0x02 | 0x12 => {
                let addr = self.rp(p, index);
                self.write(bus, addr, self.a);
                self.wz = u16::from_be_bytes([self.a, addr.wrapping_add(1) as u8]);
            }
            // LD A,(BC) / LD A,(DE)
            0x0A | 0x1A => {
                let addr = self.rp(p, index);
                self.a = self.read(bus, addr);
                self.wz = addr.wrapping_add(1);
            }

            // INC rp / DEC rp: the 16-bit incrementer needs 2 extra T-states
            // and does not touch the flags.
            0x03 | 0x13 | 0x23 | 0x33 => {
                self.internal(2);
                let v = self.rp(p, index).wrapping_add(1);
                self.set_rp(p, index, v);
            }
            0x0B | 0x1B | 0x2B | 0x3B => {
                self.internal(2);
                let v = self.rp(p, index).wrapping_sub(1);
                self.set_rp(p, index, v);
            }

            // INC r / DEC r / INC (HL) / DEC (HL)
            0x34 | 0x35 => {
                let addr = self.mem_operand(bus, index);
                let v = self.read(bus, addr);
                self.internal(1);
                let res = if op == 0x34 {
                    self.inc8(v)
                } else {
                    self.dec8(v)
                };
                self.write(bus, addr, res);
            }
            0x04 | 0x0C | 0x14 | 0x1C | 0x24 | 0x2C | 0x3C => {
                let v = self.reg8(y, index);
                let res = self.inc8(v);
                self.set_reg8(y, index, res);
            }
            0x05 | 0x0D | 0x15 | 0x1D | 0x25 | 0x2D | 0x3D => {
                let v = self.reg8(y, index);
                let res = self.dec8(v);
                self.set_reg8(y, index, res);
            }

            // LD (HL),n. With an index prefix, d and n are read back to back
            // and the address addition overlaps the read of n (2 T-states
            // instead of 5).
            0x36 => {
                let addr = match index {
                    Index::Hl => self.hl(),
                    Index::Ix | Index::Iy => {
                        let addr = self.fetch_displaced(bus, self.index_reg(index));
                        self.wz = addr;
                        addr
                    }
                };
                let n = self.fetch_byte(bus);
                if index != Index::Hl {
                    self.internal(2);
                }
                self.write(bus, addr, n);
            }
            // LD r,n
            0x06 | 0x0E | 0x16 | 0x1E | 0x26 | 0x2E | 0x3E => {
                let n = self.fetch_byte(bus);
                self.set_reg8(y, index, n);
            }

            // RLCA RRCA RLA RRA
            0x07 | 0x0F | 0x17 | 0x1F => self.rotate_a(y),
            0x27 => self.daa(),
            0x2F => self.cpl(),
            0x37 => self.scf(),
            0x3F => self.ccf(),

            // EX AF,AF'
            0x08 => {
                let af = self.af();
                self.set_af(self.af_alt);
                self.af_alt = af;
            }

            // ADD HL,rp: 7 internal T-states for two passes through the 8-bit ALU.
            0x09 | 0x19 | 0x29 | 0x39 => {
                self.internal(7);
                let res = self.add16(self.index_reg(index), self.rp(p, index));
                self.set_index_reg(index, res);
            }

            // DJNZ e: one extra T-state to decrement B.
            0x10 => {
                self.internal(1);
                self.b = self.b.wrapping_sub(1);
                self.jump_relative(bus, self.b != 0);
            }
            // JR e
            0x18 => self.jump_relative(bus, true),
            // JR NZ/Z/NC/C,e
            0x20 | 0x28 | 0x30 | 0x38 => self.jump_relative(bus, self.condition(y - 4)),

            // LD (nn),HL / LD HL,(nn)
            0x22 => {
                let addr = self.fetch_word(bus);
                self.write_word(bus, addr, self.index_reg(index));
                self.wz = addr.wrapping_add(1);
            }
            0x2A => {
                let addr = self.fetch_word(bus);
                let v = self.read_word(bus, addr);
                self.set_index_reg(index, v);
                self.wz = addr.wrapping_add(1);
            }
            // LD (nn),A / LD A,(nn)
            0x32 => {
                let addr = self.fetch_word(bus);
                self.write(bus, addr, self.a);
                self.wz = u16::from_be_bytes([self.a, addr.wrapping_add(1) as u8]);
            }
            0x3A => {
                let addr = self.fetch_word(bus);
                self.a = self.read(bus, addr);
                self.wz = addr.wrapping_add(1);
            }

            // HALT sits where `LD (HL),(HL)` would be.
            0x76 => self.halted = true,

            // LD r,r' block. When one side is (HL)/(IX+d) the other side is a
            // plain H or L, never IXH/IXL: the prefix has been "used up" by
            // the memory operand.
            0x40..=0x7F => {
                if z == 6 {
                    let addr = self.mem_operand(bus, index);
                    let v = self.read(bus, addr);
                    self.set_reg8(y, Index::Hl, v);
                } else if y == 6 {
                    let addr = self.mem_operand(bus, index);
                    let v = self.reg8(z, Index::Hl);
                    self.write(bus, addr, v);
                } else {
                    let v = self.reg8(z, index);
                    self.set_reg8(y, index, v);
                }
            }

            // ALU A,r block: ADD ADC SUB SBC AND XOR OR CP.
            0x80..=0xBF => {
                let v = if z == 6 {
                    let addr = self.mem_operand(bus, index);
                    self.read(bus, addr)
                } else {
                    self.reg8(z, index)
                };
                self.alu(y, v);
            }
            // ALU A,n
            0xC6 | 0xCE | 0xD6 | 0xDE | 0xE6 | 0xEE | 0xF6 | 0xFE => {
                let n = self.fetch_byte(bus);
                self.alu(y, n);
            }

            // RET cc: one extra T-state to evaluate the condition.
            0xC0 | 0xC8 | 0xD0 | 0xD8 | 0xE0 | 0xE8 | 0xF0 | 0xF8 => {
                self.internal(1);
                if self.condition(y) {
                    self.pc = self.pop(bus);
                    self.wz = self.pc;
                }
            }
            // RET
            0xC9 => {
                self.pc = self.pop(bus);
                self.wz = self.pc;
            }

            // POP rp2 / PUSH rp2, where pair 3 is AF instead of SP. POP AF
            // restores F but is not a flag computation, so Q is not set.
            0xC1 | 0xD1 | 0xE1 => {
                let v = self.pop(bus);
                self.set_rp(p, index, v);
            }
            0xF1 => {
                let v = self.pop(bus);
                self.set_af(v);
            }
            0xC5 | 0xD5 | 0xE5 | 0xF5 => {
                self.internal(1);
                let v = if op == 0xF5 {
                    self.af()
                } else {
                    self.rp(p, index)
                };
                self.push(bus, v);
            }

            // JP cc,nn / JP nn. The target is read (into WZ) whether or not
            // the jump is taken.
            0xC2 | 0xCA | 0xD2 | 0xDA | 0xE2 | 0xEA | 0xF2 | 0xFA => {
                self.wz = self.fetch_word(bus);
                if self.condition(y) {
                    self.pc = self.wz;
                }
            }
            0xC3 => {
                self.wz = self.fetch_word(bus);
                self.pc = self.wz;
            }

            // CALL cc,nn / CALL nn. The extra T-state happens only when the
            // call is taken: the CPU decides while reading the high byte.
            0xC4 | 0xCC | 0xD4 | 0xDC | 0xE4 | 0xEC | 0xF4 | 0xFC | 0xCD => {
                self.wz = self.fetch_word(bus);
                if op == 0xCD || self.condition(y) {
                    self.internal(1);
                    self.push(bus, self.pc);
                    self.pc = self.wz;
                }
            }

            // RST p: a one-byte call to address 8*p.
            0xC7 | 0xCF | 0xD7 | 0xDF | 0xE7 | 0xEF | 0xF7 | 0xFF => {
                self.internal(1);
                self.push(bus, self.pc);
                self.pc = (y * 8) as u16;
                self.wz = self.pc;
            }

            // OUT (n),A / IN A,(n): A provides the high byte of the port
            // address.
            0xD3 => {
                let n = self.fetch_byte(bus);
                let port = u16::from_be_bytes([self.a, n]);
                self.port_out(bus, port, self.a);
                self.wz = u16::from_be_bytes([self.a, n.wrapping_add(1)]);
            }
            0xDB => {
                let n = self.fetch_byte(bus);
                let port = u16::from_be_bytes([self.a, n]);
                self.a = self.port_in(bus, port);
                self.wz = port.wrapping_add(1);
            }

            // EXX: swap BC, DE, HL with their shadows. Not affected by DD/FD.
            0xD9 => {
                let (bc, de, hl) = (self.bc(), self.de(), self.hl());
                self.set_bc(self.bc_alt);
                self.set_de(self.de_alt);
                self.set_hl(self.hl_alt);
                (self.bc_alt, self.de_alt, self.hl_alt) = (bc, de, hl);
            }

            // EX (SP),HL
            0xE3 => {
                let v = self.read_word(bus, self.sp);
                self.internal(1);
                self.write(
                    bus,
                    self.sp.wrapping_add(1),
                    (self.index_reg(index) >> 8) as u8,
                );
                self.write(bus, self.sp, self.index_reg(index) as u8);
                self.internal(2);
                self.set_index_reg(index, v);
                self.wz = v;
            }

            // JP (HL): really "JP HL", no memory access.
            0xE9 => self.pc = self.index_reg(index),

            // EX DE,HL: always HL, even after a prefix.
            0xEB => {
                let de = self.de();
                self.set_de(self.hl());
                self.set_hl(de);
            }

            // DI / EI. EI takes effect after the next instruction.
            0xF3 => {
                self.iff1 = false;
                self.iff2 = false;
            }
            0xFB => {
                self.iff1 = true;
                self.iff2 = true;
                self.ei_delay = true;
            }

            // LD SP,HL
            0xF9 => {
                self.internal(2);
                self.sp = self.index_reg(index);
            }

            // Prefixes are handled by `execute` before reaching this page.
            0xCB | 0xDD | 0xED | 0xFD => unreachable!("prefix {op:#04x} reached execute_base"),
        }
    }
}
