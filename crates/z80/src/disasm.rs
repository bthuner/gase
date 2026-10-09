//! A Zilog-syntax disassembler, for debuggers and trace logs.
//!
//! It follows the same opcode structure as the executor (see
//! [`crate::decode`](crate#instruction-encoding)) and names undocumented
//! instructions the way most tools do: `SLL`, `IXH`/`IXL`, `IN (C)`,
//! `OUT (C),0`, and `RLC (IX+d),B` for the register-copying `DD CB` forms.
//!
//! ```
//! use gase_z80::disasm::disassemble;
//! let code = [0xDD, 0x7E, 0xFB]; // LD A,(IX-5)
//! let (text, len) = disassemble(0, |a| code[a as usize]);
//! assert_eq!((text.as_str(), len), ("LD A,(IX-$05)", 3));
//! ```

use std::fmt::Write;

const R: [&str; 8] = ["B", "C", "D", "E", "H", "L", "(HL)", "A"];
const RP: [&str; 4] = ["BC", "DE", "HL", "SP"];
const RP2: [&str; 4] = ["BC", "DE", "HL", "AF"];
const CC: [&str; 8] = ["NZ", "Z", "NC", "C", "PO", "PE", "P", "M"];
const ALU: [&str; 8] = [
    "ADD A,", "ADC A,", "SUB ", "SBC A,", "AND ", "XOR ", "OR ", "CP ",
];
const ROT: [&str; 8] = ["RLC", "RRC", "RL", "RR", "SLA", "SRA", "SLL", "SRL"];

/// Reads instruction bytes and remembers how many were consumed.
struct Cursor<F> {
    start: u16,
    len: u16,
    read: F,
}

impl<F: FnMut(u16) -> u8> Cursor<F> {
    fn byte(&mut self) -> u8 {
        let v = (self.read)(self.start.wrapping_add(self.len));
        self.len += 1;
        v
    }
    fn word(&mut self) -> u16 {
        let lo = self.byte();
        u16::from_le_bytes([lo, self.byte()])
    }
    /// Address after the instruction, the base of relative jumps.
    fn next_pc(&self) -> u16 {
        self.start.wrapping_add(self.len)
    }
}

/// `(IX+$05)`, `(IY-$80)`.
fn displaced(reg: &str, d: i8) -> String {
    let sign = if d < 0 { '-' } else { '+' };
    format!("({reg}{sign}${:02X})", d.unsigned_abs())
}

/// Operand names for one instruction, given its (optional) index prefix.
struct Names {
    /// "HL", "IX" or "IY".
    hl: &'static str,
    /// Names of r[4] and r[5]: H/L or the index register halves.
    h: &'static str,
    l: &'static str,
    /// The memory operand, already including the displacement.
    mem: String,
}

impl Names {
    fn r(&self, code: u8) -> String {
        match code {
            4 => self.h.to_string(),
            5 => self.l.to_string(),
            6 => self.mem.clone(),
            _ => R[code as usize].to_string(),
        }
    }
    fn rp(&self, p: u8) -> &'static str {
        if p == 2 { self.hl } else { RP[p as usize] }
    }
    fn rp2(&self, p: u8) -> &'static str {
        if p == 2 { self.hl } else { RP2[p as usize] }
    }
}

/// Disassemble the instruction at `addr`, reading bytes through `read`.
/// Returns the text and the instruction length in bytes.
pub fn disassemble(addr: u16, read: impl FnMut(u16) -> u8) -> (String, u16) {
    let mut cur = Cursor {
        start: addr,
        len: 0,
        read,
    };
    let text = decode(&mut cur);
    (text, cur.len)
}

fn decode<F: FnMut(u16) -> u8>(cur: &mut Cursor<F>) -> String {
    let op = cur.byte();
    match op {
        0xCB => {
            let op = cur.byte();
            let names = Names {
                hl: "HL",
                h: "H",
                l: "L",
                mem: "(HL)".into(),
            };
            cb(op, &names, None)
        }
        0xED => {
            let op = cur.byte();
            ed(cur, op)
        }
        0xDD | 0xFD => {
            let (hl, h, l) = if op == 0xDD {
                ("IX", "IXH", "IXL")
            } else {
                ("IY", "IYH", "IYL")
            };
            let next_pc = cur.next_pc();
            let next = (cur.read)(next_pc);
            if matches!(next, 0xDD | 0xFD | 0xED) {
                // The prefix is overridden by the following one: on its own
                // it is a 4 T-state NOP.
                return format!("DB ${op:02X}");
            }
            let op = cur.byte();
            if op == 0xCB {
                let d = cur.byte() as i8;
                let op = cur.byte();
                let names = Names {
                    hl,
                    h,
                    l,
                    mem: displaced(hl, d),
                };
                return cb(op, &names, Some(()));
            }
            // Only instructions that use (HL) take a displacement byte.
            let uses_mem = match op {
                0x34..=0x36 => true,
                0x40..=0xBF => op != 0x76 && (op & 7 == 6 || (op & 0x38) == 0x30 && op < 0x80),
                _ => false,
            };
            let mem = if uses_mem {
                displaced(hl, cur.byte() as i8)
            } else {
                String::new()
            };
            // With a memory operand, the other register stays plain H/L.
            let names = if uses_mem {
                Names {
                    hl,
                    h: "H",
                    l: "L",
                    mem,
                }
            } else {
                Names { hl, h, l, mem }
            };
            base(cur, op, &names)
        }
        _ => {
            let names = Names {
                hl: "HL",
                h: "H",
                l: "L",
                mem: "(HL)".into(),
            };
            base(cur, op, &names)
        }
    }
}

fn base<F: FnMut(u16) -> u8>(cur: &mut Cursor<F>, op: u8, n: &Names) -> String {
    let y = (op >> 3) & 7;
    let z = op & 7;
    let p = y >> 1;
    let hl = n.hl;
    match op {
        0x00 => "NOP".into(),
        0x08 => "EX AF,AF'".into(),
        0x10 | 0x18 | 0x20 | 0x28 | 0x30 | 0x38 => {
            let d = cur.byte() as i8;
            let target = cur.next_pc().wrapping_add_signed(d.into());
            match op {
                0x10 => format!("DJNZ ${target:04X}"),
                0x18 => format!("JR ${target:04X}"),
                _ => format!("JR {},${target:04X}", CC[(y - 4) as usize]),
            }
        }
        0x01 | 0x11 | 0x21 | 0x31 => format!("LD {},${:04X}", n.rp(p), cur.word()),
        0x09 | 0x19 | 0x29 | 0x39 => format!("ADD {hl},{}", n.rp(p)),
        0x02 => "LD (BC),A".into(),
        0x12 => "LD (DE),A".into(),
        0x0A => "LD A,(BC)".into(),
        0x1A => "LD A,(DE)".into(),
        0x22 => format!("LD (${:04X}),{hl}", cur.word()),
        0x2A => format!("LD {hl},(${:04X})", cur.word()),
        0x32 => format!("LD (${:04X}),A", cur.word()),
        0x3A => format!("LD A,(${:04X})", cur.word()),
        0x03 | 0x13 | 0x23 | 0x33 => format!("INC {}", n.rp(p)),
        0x0B | 0x1B | 0x2B | 0x3B => format!("DEC {}", n.rp(p)),
        0x04 | 0x0C | 0x14 | 0x1C | 0x24 | 0x2C | 0x34 | 0x3C => format!("INC {}", n.r(y)),
        0x05 | 0x0D | 0x15 | 0x1D | 0x25 | 0x2D | 0x35 | 0x3D => format!("DEC {}", n.r(y)),
        0x06 | 0x0E | 0x16 | 0x1E | 0x26 | 0x2E | 0x36 | 0x3E => {
            format!("LD {},${:02X}", n.r(y), cur.byte())
        }
        0x07 | 0x0F | 0x17 | 0x1F | 0x27 | 0x2F | 0x37 | 0x3F => {
            ["RLCA", "RRCA", "RLA", "RRA", "DAA", "CPL", "SCF", "CCF"][y as usize].into()
        }
        0x76 => "HALT".into(),
        0x40..=0x7F => format!("LD {},{}", n.r(y), n.r(z)),
        0x80..=0xBF => format!("{}{}", ALU[y as usize], n.r(z)),
        0xC6 | 0xCE | 0xD6 | 0xDE | 0xE6 | 0xEE | 0xF6 | 0xFE => {
            format!("{}${:02X}", ALU[y as usize], cur.byte())
        }
        0xC0 | 0xC8 | 0xD0 | 0xD8 | 0xE0 | 0xE8 | 0xF0 | 0xF8 => format!("RET {}", CC[y as usize]),
        0xC1 | 0xD1 | 0xE1 | 0xF1 => format!("POP {}", n.rp2(p)),
        0xC5 | 0xD5 | 0xE5 | 0xF5 => format!("PUSH {}", n.rp2(p)),
        0xC9 => "RET".into(),
        0xD9 => "EXX".into(),
        0xE9 => format!("JP ({hl})"),
        0xF9 => format!("LD SP,{hl}"),
        0xC2 | 0xCA | 0xD2 | 0xDA | 0xE2 | 0xEA | 0xF2 | 0xFA => {
            format!("JP {},${:04X}", CC[y as usize], cur.word())
        }
        0xC3 => format!("JP ${:04X}", cur.word()),
        0xC4 | 0xCC | 0xD4 | 0xDC | 0xE4 | 0xEC | 0xF4 | 0xFC => {
            format!("CALL {},${:04X}", CC[y as usize], cur.word())
        }
        0xCD => format!("CALL ${:04X}", cur.word()),
        0xC7 | 0xCF | 0xD7 | 0xDF | 0xE7 | 0xEF | 0xF7 | 0xFF => format!("RST ${:02X}", y * 8),
        0xD3 => format!("OUT (${:02X}),A", cur.byte()),
        0xDB => format!("IN A,(${:02X})", cur.byte()),
        0xE3 => format!("EX (SP),{hl}"),
        0xEB => "EX DE,HL".into(),
        0xF3 => "DI".into(),
        0xFB => "EI".into(),
        // Prefixes are consumed by `decode`; reaching here means a prefix
        // inside an indexed instruction, which `decode` already rejected.
        0xCB | 0xDD | 0xED | 0xFD => format!("DB ${op:02X}"),
    }
}

/// `indexed` is set for `DD CB d op`, where the register field names an
/// extra copy of the result (undocumented) rather than the operand.
fn cb(op: u8, n: &Names, indexed: Option<()>) -> String {
    let y = (op >> 3) & 7;
    let z = op & 7;
    let operand = if indexed.is_some() {
        n.mem.clone()
    } else {
        n.r(z)
    };
    let mut s = match op >> 6 {
        0 => format!("{} {operand}", ROT[y as usize]),
        1 => return format!("BIT {y},{operand}"),
        2 => format!("RES {y},{operand}"),
        _ => format!("SET {y},{operand}"),
    };
    if indexed.is_some() && z != 6 {
        let _ = write!(s, ",{}", R[z as usize]);
    }
    s
}

fn ed<F: FnMut(u16) -> u8>(cur: &mut Cursor<F>, op: u8) -> String {
    let y = (op >> 3) & 7;
    let p = y >> 1;
    match op {
        0x70 => "IN (C)".into(),
        0x71 => "OUT (C),0".into(),
        0x40 | 0x48 | 0x50 | 0x58 | 0x60 | 0x68 | 0x78 => format!("IN {},(C)", R[y as usize]),
        0x41 | 0x49 | 0x51 | 0x59 | 0x61 | 0x69 | 0x79 => format!("OUT (C),{}", R[y as usize]),
        0x42 | 0x52 | 0x62 | 0x72 => format!("SBC HL,{}", RP[p as usize]),
        0x4A | 0x5A | 0x6A | 0x7A => format!("ADC HL,{}", RP[p as usize]),
        0x43 | 0x53 | 0x63 | 0x73 => format!("LD (${:04X}),{}", cur.word(), RP[p as usize]),
        0x4B | 0x5B | 0x6B | 0x7B => format!("LD {},(${:04X})", RP[p as usize], cur.word()),
        0x44 | 0x4C | 0x54 | 0x5C | 0x64 | 0x6C | 0x74 | 0x7C => "NEG".into(),
        0x4D => "RETI".into(),
        0x45 | 0x55 | 0x5D | 0x65 | 0x6D | 0x75 | 0x7D => "RETN".into(),
        0x46 | 0x4E | 0x56 | 0x5E | 0x66 | 0x6E | 0x76 | 0x7E => {
            format!("IM {}", [0, 0, 1, 2][(y & 3) as usize])
        }
        0x47 => "LD I,A".into(),
        0x4F => "LD R,A".into(),
        0x57 => "LD A,I".into(),
        0x5F => "LD A,R".into(),
        0x67 => "RRD".into(),
        0x6F => "RLD".into(),
        0xA0..=0xBF if op & 7 <= 3 && y >= 4 => {
            const NAMES: [[&str; 4]; 4] = [
                ["LDI", "CPI", "INI", "OUTI"],
                ["LDD", "CPD", "IND", "OUTD"],
                ["LDIR", "CPIR", "INIR", "OTIR"],
                ["LDDR", "CPDR", "INDR", "OTDR"],
            ];
            NAMES[(y - 4) as usize][(op & 3) as usize].into()
        }
        _ => format!("NOP* (ED {op:02X})"),
    }
}
