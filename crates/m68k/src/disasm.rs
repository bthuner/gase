//! A disassembler producing Motorola syntax, for debuggers and traces.
//!
//! It reuses the CPU's own decode table, so the disassembly always agrees
//! with what the core executes: an opcode the CPU would treat as illegal is
//! shown as `dc.w`, never as a plausible-looking instruction.
//!
//! ```
//! let code = [0x4E71u16, 0x203C, 0x0000, 0x0100];
//! let read = |addr: u32| code[(addr / 2) as usize];
//! assert_eq!(gase_m68k::disasm::disassemble(0, read), ("nop".to_string(), 2));
//! assert_eq!(gase_m68k::disasm::disassemble(2, read), ("move.l #$100,d0".to_string(), 6));
//! ```

use std::fmt::Write as _;

use crate::decode::{DecodeTable, Instr, Op};
use crate::ea::{Ea, Mode, Size};

/// Disassemble the instruction at `addr`, fetching words with `read_word`.
///
/// Returns the text and the instruction length in bytes.
pub fn disassemble(addr: u32, read_word: impl FnMut(u32) -> u16) -> (String, u32) {
    let mut d = Disassembler {
        next: addr,
        read_word,
        out: String::new(),
    };
    let opcode = d.word();
    let instr = DecodeTable::get().lookup(opcode);
    d.instruction(opcode, instr);
    (d.out, d.next.wrapping_sub(addr))
}

struct Disassembler<F> {
    /// Address of the next unread word.
    next: u32,
    read_word: F,
    out: String,
}

/// `$1F`-style hex for a signed value: `-$10` rather than `$FFF0`.
fn signed_hex(value: i32) -> String {
    if value < 0 {
        format!("-${:X}", value.unsigned_abs())
    } else {
        format!("${value:X}")
    }
}

/// Condition code names, in encoding order.
const CONDITIONS: [&str; 16] = [
    "t", "f", "hi", "ls", "cc", "cs", "ne", "eq", "vc", "vs", "pl", "mi", "ge", "lt", "gt", "le",
];

fn data_reg(n: u8) -> String {
    format!("d{n}")
}

fn addr_reg(n: u8) -> String {
    if n == 7 { "sp".into() } else { format!("a{n}") }
}

impl<F: FnMut(u32) -> u16> Disassembler<F> {
    fn word(&mut self) -> u16 {
        let w = (self.read_word)(self.next & 0x00FF_FFFF);
        self.next = self.next.wrapping_add(2);
        w
    }

    fn long(&mut self) -> u32 {
        let high = self.word();
        u32::from(high) << 16 | u32::from(self.word())
    }

    /// The index part of a `(d8,An,Xn)` extension word, e.g. `d1.w`.
    fn index_reg(ext: u16) -> String {
        let n = ((ext >> 12) & 7) as u8;
        let reg = if ext & 0x8000 != 0 {
            addr_reg(n)
        } else {
            data_reg(n)
        };
        format!("{reg}.{}", if ext & 0x0800 != 0 { 'l' } else { 'w' })
    }

    /// Format an effective address, reading its extension words.
    fn ea(&mut self, ea: Ea, size: Size) -> String {
        let r = ea.reg;
        match ea.mode {
            Mode::DataReg => data_reg(r),
            Mode::AddrReg => addr_reg(r),
            Mode::Indirect => format!("({})", addr_reg(r)),
            Mode::PostInc => format!("({})+", addr_reg(r)),
            Mode::PreDec => format!("-({})", addr_reg(r)),
            Mode::Disp => {
                let d = self.word() as i16;
                format!("({},{})", signed_hex(d.into()), addr_reg(r))
            }
            Mode::Index => {
                let ext = self.word();
                format!(
                    "({},{},{})",
                    signed_hex((ext as i8).into()),
                    addr_reg(r),
                    Self::index_reg(ext)
                )
            }
            Mode::AbsShort => {
                let a = self.word() as i16;
                format!("(${:X}).w", a as u32 & 0x00FF_FFFF)
            }
            Mode::AbsLong => format!("(${:X}).l", self.long()),
            // PC-relative: show the address it resolves to.
            Mode::PcDisp => {
                let base = self.next;
                let d = self.word() as i16;
                format!("(${:X},pc)", base.wrapping_add(d as u32) & 0x00FF_FFFF)
            }
            Mode::PcIndex => {
                let base = self.next;
                let ext = self.word();
                let target = base.wrapping_add(ext as i8 as u32) & 0x00FF_FFFF;
                format!("(${target:X},pc,{})", Self::index_reg(ext))
            }
            Mode::Immediate => match size {
                Size::Byte => format!("#${:X}", self.word() & 0xFF),
                Size::Word => format!("#${:X}", self.word()),
                Size::Long => format!("#${:X}", self.long()),
            },
            Mode::Quick => format!("#{r}"),
        }
    }

    /// A MOVEM register list such as `d0-d3/a0/a6`. `mask` bit 0 is D0.
    fn register_list(mask: u16) -> String {
        let mut parts = Vec::new();
        for (bank, prefix) in [(0, 'd'), (8, 'a')] {
            let mut n = 0;
            while n < 8 {
                if mask & (1 << (bank + n)) == 0 {
                    n += 1;
                    continue;
                }
                let start = n;
                while n < 8 && mask & (1 << (bank + n)) != 0 {
                    n += 1;
                }
                let name = |k: u16| {
                    if prefix == 'a' && k == 7 {
                        "sp".into()
                    } else {
                        format!("{prefix}{k}")
                    }
                };
                parts.push(if n - start == 1 {
                    name(start)
                } else {
                    format!("{}-{}", name(start), name(n - 1))
                });
            }
        }
        parts.join("/")
    }

    fn emit(&mut self, mnemonic: &str, operands: &str) {
        self.out.push_str(mnemonic);
        if !operands.is_empty() {
            self.out.push(' ');
            self.out.push_str(operands);
        }
    }

    /// `op.s src,dst`.
    fn two(&mut self, mnemonic: &str, i: Instr) {
        let src = self.ea(i.src, i.size);
        let dst = self.ea(i.dst, i.size);
        self.emit(
            &format!("{mnemonic}{}", i.size.suffix()),
            &format!("{src},{dst}"),
        );
    }

    /// `op.s dst`.
    fn one(&mut self, mnemonic: &str, i: Instr) {
        let dst = self.ea(i.dst, i.size);
        self.emit(&format!("{mnemonic}{}", i.size.suffix()), &dst);
    }

    /// ADD/SUB/CMP/AND/OR/EOR and their immediate and quick forms.
    fn alu(&mut self, base: &str, i: Instr) {
        let mnemonic = match i.src.mode {
            Mode::Immediate => format!("{base}i"),
            Mode::Quick => format!("{base}q"),
            _ => base.to_owned(),
        };
        self.two(&mnemonic, i);
    }

    fn instruction(&mut self, opcode: u16, i: Instr) {
        let cond = CONDITIONS[usize::from((opcode >> 8) & 0xF)];
        match i.op {
            Op::Add => self.alu("add", i),
            Op::Sub => self.alu("sub", i),
            Op::Cmp => self.alu("cmp", i),
            Op::And => self.alu("and", i),
            Op::Or => self.alu("or", i),
            Op::Eor => self.alu("eor", i),
            Op::Adda | Op::Suba => {
                let base = if i.op == Op::Adda { "add" } else { "sub" };
                if i.src.mode == Mode::Quick {
                    self.two(&format!("{base}q"), i);
                } else {
                    self.two(&format!("{base}a"), i);
                }
            }
            Op::Cmpa => self.two("cmpa", i),
            Op::Addx => self.two("addx", i),
            Op::Subx => self.two("subx", i),
            Op::Cmpm => self.two("cmpm", i),
            Op::Abcd => self.two("abcd", i),
            Op::Sbcd => self.two("sbcd", i),
            Op::Neg => self.one("neg", i),
            Op::Negx => self.one("negx", i),
            Op::Clr => self.one("clr", i),
            Op::Tst => self.one("tst", i),
            Op::Not => self.one("not", i),
            Op::Ext => self.one("ext", i),
            Op::Nbcd => {
                let dst = self.ea(i.dst, Size::Byte);
                self.emit("nbcd", &dst);
            }
            Op::Tas => {
                let dst = self.ea(i.dst, Size::Byte);
                self.emit("tas", &dst);
            }
            Op::Swap => {
                let dst = self.ea(i.dst, Size::Long);
                self.emit("swap", &dst);
            }
            Op::AndiToCcr
            | Op::OriToCcr
            | Op::EoriToCcr
            | Op::AndiToSr
            | Op::OriToSr
            | Op::EoriToSr => {
                let (mnemonic, reg) = match i.op {
                    Op::AndiToCcr => ("andi", "ccr"),
                    Op::OriToCcr => ("ori", "ccr"),
                    Op::EoriToCcr => ("eori", "ccr"),
                    Op::AndiToSr => ("andi", "sr"),
                    Op::OriToSr => ("ori", "sr"),
                    _ => ("eori", "sr"),
                };
                let imm = self.ea(i.src, i.size);
                self.emit(mnemonic, &format!("{imm},{reg}"));
            }
            Op::Shift(kind) => {
                let count = self.ea(i.src, Size::Byte);
                let dst = self.ea(i.dst, i.size);
                self.emit(
                    &format!("{}{}", kind.mnemonic(), i.size.suffix()),
                    &format!("{count},{dst}"),
                );
            }
            Op::ShiftMem(kind) => self.one(kind.mnemonic(), i),
            Op::Btst | Op::Bchg | Op::Bclr | Op::Bset => {
                let mnemonic = match i.op {
                    Op::Btst => "btst",
                    Op::Bchg => "bchg",
                    Op::Bclr => "bclr",
                    _ => "bset",
                };
                let bit = self.ea(i.src, Size::Byte);
                let dst = self.ea(i.dst, Size::Byte);
                self.emit(mnemonic, &format!("{bit},{dst}"));
            }
            Op::Bcc | Op::Bsr => {
                let base = self.next;
                let (disp, suffix) = match opcode as u8 {
                    0 => (u32::from(self.word()) as i16 as u32, ".w"),
                    d => (d as i8 as u32, ".s"),
                };
                let name = if i.op == Op::Bsr {
                    "bsr"
                } else if cond == "t" {
                    "bra"
                } else {
                    &format!("b{cond}")
                };
                let target = base.wrapping_add(disp) & 0x00FF_FFFF;
                self.emit(&format!("{name}{suffix}"), &format!("${target:X}"));
            }
            Op::Dbcc => {
                let base = self.next;
                let target = base.wrapping_add(self.word() as i16 as u32) & 0x00FF_FFFF;
                let dn = data_reg(i.dst.reg);
                self.emit(&format!("db{cond}"), &format!("{dn},${target:X}"));
            }
            Op::Scc => {
                let dst = self.ea(i.dst, Size::Byte);
                self.emit(&format!("s{cond}"), &dst);
            }
            Op::Jmp | Op::Jsr | Op::Pea => {
                let target = self.ea(i.src, Size::Long);
                let mnemonic = match i.op {
                    Op::Jmp => "jmp",
                    Op::Jsr => "jsr",
                    _ => "pea",
                };
                self.emit(mnemonic, &target);
            }
            Op::Lea => {
                let src = self.ea(i.src, Size::Long);
                self.emit("lea", &format!("{src},{}", addr_reg(i.dst.reg)));
            }
            Op::Mulu | Op::Muls | Op::Divu | Op::Divs | Op::Chk => {
                let mnemonic = match i.op {
                    Op::Mulu => "mulu.w",
                    Op::Muls => "muls.w",
                    Op::Divu => "divu.w",
                    Op::Divs => "divs.w",
                    _ => "chk.w",
                };
                let src = self.ea(i.src, Size::Word);
                self.emit(mnemonic, &format!("{src},{}", data_reg(i.dst.reg)));
            }
            Op::Move => self.two("move", i),
            Op::Movea => self.two("movea", i),
            Op::Moveq => {
                let value = signed_hex((i.src.reg as i8).into());
                self.emit("moveq", &format!("#{value},{}", data_reg(i.dst.reg)));
            }
            Op::MovemToMem => {
                let mask = self.word();
                // With -(An) the mask is stored reversed (bit 0 = A7).
                let mask = if i.dst.mode == Mode::PreDec {
                    mask.reverse_bits()
                } else {
                    mask
                };
                let dst = self.ea(i.dst, i.size);
                self.emit(
                    &format!("movem{}", i.size.suffix()),
                    &format!("{},{dst}", Self::register_list(mask)),
                );
            }
            Op::MovemToReg => {
                let mask = self.word();
                let src = self.ea(i.src, i.size);
                self.emit(
                    &format!("movem{}", i.size.suffix()),
                    &format!("{src},{}", Self::register_list(mask)),
                );
            }
            Op::Movep => self.two("movep", i),
            Op::Exg => {
                let x = ((opcode >> 9) & 7) as u8;
                let y = (opcode & 7) as u8;
                let operands = match (opcode >> 3) & 0x1F {
                    0x08 => format!("{},{}", data_reg(x), data_reg(y)),
                    0x09 => format!("{},{}", addr_reg(x), addr_reg(y)),
                    _ => format!("{},{}", data_reg(x), addr_reg(y)),
                };
                self.emit("exg", &operands);
            }
            Op::Link => {
                let disp = signed_hex(i32::from(self.word() as i16));
                self.emit("link", &format!("{},#{disp}", addr_reg(i.dst.reg)));
            }
            Op::Unlk => self.emit("unlk", &addr_reg(i.dst.reg)),
            Op::MoveFromSr => {
                let dst = self.ea(i.dst, Size::Word);
                self.emit("move", &format!("sr,{dst}"));
            }
            Op::MoveToCcr | Op::MoveToSr => {
                let src = self.ea(i.src, Size::Word);
                let reg = if i.op == Op::MoveToCcr { "ccr" } else { "sr" };
                self.emit("move", &format!("{src},{reg}"));
            }
            Op::MoveToUsp => self.emit("move", &format!("{},usp", addr_reg(i.dst.reg))),
            Op::MoveFromUsp => self.emit("move", &format!("usp,{}", addr_reg(i.dst.reg))),
            Op::Trap => self.emit("trap", &format!("#{}", opcode & 0xF)),
            Op::Stop => {
                let sr = self.word();
                self.emit("stop", &format!("#${sr:X}"));
            }
            Op::Rts => self.emit("rts", ""),
            Op::Rtr => self.emit("rtr", ""),
            Op::Rte => self.emit("rte", ""),
            Op::Trapv => self.emit("trapv", ""),
            Op::Reset => self.emit("reset", ""),
            Op::Nop => self.emit("nop", ""),
            Op::Illegal if opcode == 0x4AFC => self.emit("illegal", ""),
            Op::Illegal | Op::LineA | Op::LineF => {
                let _ = write!(self.out, "dc.w ${opcode:04X}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::disassemble;

    fn dis(words: &[u16]) -> (String, u32) {
        disassemble(0x1000, |addr| {
            words
                .get(((addr - 0x1000) / 2) as usize)
                .copied()
                .unwrap_or(0)
        })
    }

    #[test]
    fn common_instructions() {
        let cases: &[(&[u16], &str)] = &[
            (&[0x4E71], "nop"),
            (&[0x2C40], "movea.l d0,a6"),
            (&[0x2046], "movea.l d6,a0"),
            (&[0x3080], "move.w d0,(a0)"),
            (&[0x203C, 0x0000, 0x0100], "move.l #$100,d0"),
            (&[0x48E7, 0xC800], "movem.l d0-d1/d4,-(sp)"),
            (&[0x4CDF, 0x0013], "movem.l (sp)+,d0-d1/d4"),
            (&[0x4CD8, 0x06FC], "movem.l (a0)+,d2-d7/a1-a2"),
            (&[0x6700, 0x0014], "beq.w $1016"),
            (&[0x66FE], "bne.s $1000"),
            (&[0x51C8, 0xFFFE], "dbf d0,$1000"),
            (&[0x5279, 0x00FF, 0x0294], "addq.w #1,($FF0294).l"),
            (&[0x0280, 0x0000, 0x0001], "andi.l #$1,d0"),
            (&[0x7CFF], "moveq #-$1,d6"),
            (&[0xE51A], "rol.b #2,d2"),
            (&[0x41F9, 0x00FF, 0x026E], "lea ($FF026E).l,a0"),
            (&[0x4EB9, 0x0000, 0xF8C4], "jsr ($F8C4).l"),
            (&[0x46FC, 0x2700], "move #$2700,sr"),
            (&[0x4E57, 0xFFF0], "link sp,#-$10"),
            (&[0x303B, 0x1006], "move.w ($1008,pc,d1.w),d0"),
            (&[0x4AFC], "illegal"),
            (&[0xA000], "dc.w $A000"),
        ];
        for (words, text) in cases {
            assert_eq!(
                dis(words),
                ((*text).to_string(), 2 * words.len() as u32),
                "{words:04X?}"
            );
        }
    }
}
