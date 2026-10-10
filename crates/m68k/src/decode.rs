//! Opcode decoding.
//!
//! A 68000 opcode is 16 bits, so every possible opcode can be decoded once,
//! up front, into a 65 536-entry table of [`Instr`]. At run time `step` just
//! indexes the table and dispatches on [`Op`] with a single `match`; none of
//! the bit-field parsing below runs in the hot loop.
//!
//! The top four bits ("line") select a broad group:
//!
//! ```text
//!  0000  bit operations, MOVEP, immediate ops (ORI, ANDI, SUBI, ADDI, EORI, CMPI)
//!  0001  MOVE.B        0010  MOVE.L        0011  MOVE.W
//!  0100  miscellaneous (CLR, NEG, NOT, TST, LEA, JMP, MOVEM, TRAP, RTS, ...)
//!  0101  ADDQ, SUBQ, Scc, DBcc
//!  0110  Bcc, BRA, BSR
//!  0111  MOVEQ
//!  1000  OR, DIVU, DIVS, SBCD
//!  1001  SUB, SUBA, SUBX
//!  1010  (unassigned: "line A" exception, used for OS traps on the Mac)
//!  1011  CMP, CMPA, CMPM, EOR
//!  1100  AND, MULU, MULS, ABCD, EXG
//!  1101  ADD, ADDA, ADDX
//!  1110  shifts and rotates
//!  1111  (unassigned: "line F", later used for coprocessors)
//! ```
//!
//! Within a line, fields are mostly in the same places: bits 11..9 a
//! register, bits 8..6 an "opmode" (direction and size), bits 5..0 an
//! effective address. Each instruction only accepts some addressing modes;
//! an opcode with a disallowed mode is not that instruction at all, and
//! decodes as illegal unless another instruction claims the encoding (for
//! example `ADDX` lives in the `ADD Dn,<ea>` slots whose EA would be a
//! register, which `ADD` to memory cannot use).

use std::fmt;
use std::sync::OnceLock;

use crate::ea::{Ea, Mode, Size, class};

/// The kind of shift or rotate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShiftKind {
    Asl,
    Asr,
    Lsl,
    Lsr,
    Roxl,
    Roxr,
    Rol,
    Ror,
}

impl ShiftKind {
    /// From the 2-bit type field and the direction bit (1 = left).
    const fn new(kind: u16, left: bool) -> Self {
        match (kind & 3, left) {
            (0, true) => ShiftKind::Asl,
            (0, false) => ShiftKind::Asr,
            (1, true) => ShiftKind::Lsl,
            (1, false) => ShiftKind::Lsr,
            (2, true) => ShiftKind::Roxl,
            (2, false) => ShiftKind::Roxr,
            (3, true) => ShiftKind::Rol,
            _ => ShiftKind::Ror,
        }
    }

    pub(crate) const fn mnemonic(self) -> &'static str {
        match self {
            ShiftKind::Asl => "asl",
            ShiftKind::Asr => "asr",
            ShiftKind::Lsl => "lsl",
            ShiftKind::Lsr => "lsr",
            ShiftKind::Roxl => "roxl",
            ShiftKind::Roxr => "roxr",
            ShiftKind::Rol => "rol",
            ShiftKind::Ror => "ror",
        }
    }
}

/// Every operation the 68000 knows, at the granularity of one handler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Op {
    // arith.rs. ADDI/ADDQ are `Add` with an immediate/quick source, etc.
    Add,
    Sub,
    Cmp,
    Adda,
    Suba,
    Cmpa,
    Addx,
    Subx,
    Cmpm,
    Neg,
    Negx,
    Clr,
    Tst,
    Ext,
    // logic.rs
    And,
    Or,
    Eor,
    Not,
    AndiToCcr,
    OriToCcr,
    EoriToCcr,
    AndiToSr,
    OriToSr,
    EoriToSr,
    // shift.rs
    Shift(ShiftKind),
    ShiftMem(ShiftKind),
    // bit.rs
    Btst,
    Bchg,
    Bclr,
    Bset,
    Tas,
    // bcd.rs
    Abcd,
    Sbcd,
    Nbcd,
    // branch.rs
    Bcc,
    Bsr,
    Dbcc,
    Scc,
    Jmp,
    Jsr,
    Rts,
    Rtr,
    Rte,
    // muldiv.rs
    Mulu,
    Muls,
    Divu,
    Divs,
    // data.rs
    Move,
    Movea,
    Moveq,
    MovemToMem,
    MovemToReg,
    Movep,
    Lea,
    Pea,
    Exg,
    Swap,
    Link,
    Unlk,
    // system.rs
    MoveFromSr,
    MoveToCcr,
    MoveToSr,
    MoveToUsp,
    MoveFromUsp,
    Chk,
    Trap,
    Trapv,
    Reset,
    Stop,
    Nop,
    Illegal,
    LineA,
    LineF,
}

/// A decoded instruction: the operation and its operands.
///
/// For two-operand instructions `src` and `dst` are what the names say
/// (`ADD D1,(A0)` has `src = D1`, `dst = (A0)`); single-operand ones use
/// `dst`. Register fields that are not full effective addresses are stored
/// as `Dn`/`An` operands. Odd fields that only one instruction uses (branch
/// displacements, trap numbers) are read from the opcode by the handler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Instr {
    pub op: Op,
    pub size: Size,
    pub src: Ea,
    pub dst: Ea,
}

impl Instr {
    const fn new(op: Op, size: Size, src: Ea, dst: Ea) -> Self {
        Self { op, size, src, dst }
    }
    const fn unary(op: Op, size: Size, dst: Ea) -> Self {
        Self::new(op, size, Ea::NONE, dst)
    }
    const fn bare(op: Op) -> Self {
        Self::new(op, Size::Word, Ea::NONE, Ea::NONE)
    }
    const ILLEGAL: Instr = Instr::bare(Op::Illegal);
}

/// The table of all 65 536 decoded opcodes, built once per process.
///
/// A fixed-size array rather than a slice: indexed by a `u16` opcode it can
/// never be out of bounds, so the lookup at every instruction needs no
/// bounds check.
pub(crate) struct DecodeTable(Box<[Instr; 0x1_0000]>);

impl fmt::Debug for DecodeTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DecodeTable({} entries)", self.0.len())
    }
}

impl DecodeTable {
    /// The shared table, built on first use.
    pub(crate) fn get() -> &'static DecodeTable {
        static TABLE: OnceLock<DecodeTable> = OnceLock::new();
        TABLE.get_or_init(|| {
            let table: Box<[Instr]> = (0..=u16::MAX).map(decode).collect();
            DecodeTable(table.try_into().expect("one entry per opcode"))
        })
    }

    #[inline]
    pub(crate) fn lookup(&self, opcode: u16) -> Instr {
        self.0[opcode as usize]
    }
}

// ---------------------------------------------------------------------------
// Field helpers
// ---------------------------------------------------------------------------

/// Bits 11..9: the "register" field of most instructions.
const fn reg_hi(op: u16) -> u8 {
    ((op >> 9) & 7) as u8
}

/// Bits 2..0.
const fn reg_lo(op: u16) -> u8 {
    (op & 7) as u8
}

/// Bits 7..6 as a size, using the common 00=B 01=W 10=L encoding.
const fn size_76(op: u16) -> Option<Size> {
    match (op >> 6) & 3 {
        0 => Some(Size::Byte),
        1 => Some(Size::Word),
        2 => Some(Size::Long),
        _ => None,
    }
}

/// The effective address in bits 5..0, if it is in `allowed`.
fn ea_in(op: u16, allowed: u16) -> Option<Ea> {
    Ea::decode(op & 0x3F).filter(|ea| ea.class_bit() & allowed != 0)
}

/// Byte operations cannot use `An` directly (address registers have no
/// byte half).
fn ea_sized(op: u16, size: Size, allowed: u16) -> Option<Ea> {
    let allowed = if size == Size::Byte {
        allowed & class::DATA
    } else {
        allowed
    };
    ea_in(op, allowed)
}

/// Decode one opcode.
pub(crate) fn decode(op: u16) -> Instr {
    let decoded = match op >> 12 {
        0x0 => line_0(op),
        0x1 => line_move(op, Size::Byte),
        0x2 => line_move(op, Size::Long),
        0x3 => line_move(op, Size::Word),
        0x4 => line_4(op),
        0x5 => line_5(op),
        0x6 => line_6(op),
        0x7 => line_7(op),
        0x8 => line_8(op),
        0x9 => line_add_sub(op, Op::Sub, Op::Suba, Op::Subx),
        0xA => Some(Instr::bare(Op::LineA)),
        0xB => line_b(op),
        0xC => line_c(op),
        0xD => line_add_sub(op, Op::Add, Op::Adda, Op::Addx),
        0xE => line_e(op),
        _ => Some(Instr::bare(Op::LineF)),
    };
    decoded.unwrap_or(Instr::ILLEGAL)
}

/// Line 0: immediate operations, bit operations, MOVEP.
fn line_0(op: u16) -> Option<Instr> {
    if op & 0x0100 != 0 {
        // 0000 rrr1 xx ...: dynamic bit operations, or MOVEP when mode = An.
        let reg = Ea::data(reg_hi(op));
        if (op >> 3) & 7 == 1 {
            // MOVEP: bit 7 = direction (1 = register to memory), bit 6 = long.
            let size = if op & 0x40 != 0 {
                Size::Long
            } else {
                Size::Word
            };
            let mem = Ea::new(Mode::Disp, reg_lo(op));
            return Some(if op & 0x80 != 0 {
                Instr::new(Op::Movep, size, reg, mem)
            } else {
                Instr::new(Op::Movep, size, mem, reg)
            });
        }
        let (bit_op, allowed) = bit_op(op);
        // BTST Dn,#imm is allowed: testing a bit of a constant.
        let allowed = if bit_op == Op::Btst {
            allowed | (1 << Mode::Immediate as u16)
        } else {
            allowed
        };
        let dst = ea_in(op, allowed)?;
        return Some(Instr::new(bit_op, bit_size(dst), reg, dst));
    }

    // Immediate to CCR/SR: the EA field is "#imm" (111 100) in an
    // otherwise-unusable slot.
    match op {
        0x003C => {
            return Some(Instr::new(
                Op::OriToCcr,
                Size::Byte,
                Ea::new(Mode::Immediate, 4),
                Ea::NONE,
            ));
        }
        0x007C => {
            return Some(Instr::new(
                Op::OriToSr,
                Size::Word,
                Ea::new(Mode::Immediate, 4),
                Ea::NONE,
            ));
        }
        0x023C => {
            return Some(Instr::new(
                Op::AndiToCcr,
                Size::Byte,
                Ea::new(Mode::Immediate, 4),
                Ea::NONE,
            ));
        }
        0x027C => {
            return Some(Instr::new(
                Op::AndiToSr,
                Size::Word,
                Ea::new(Mode::Immediate, 4),
                Ea::NONE,
            ));
        }
        0x0A3C => {
            return Some(Instr::new(
                Op::EoriToCcr,
                Size::Byte,
                Ea::new(Mode::Immediate, 4),
                Ea::NONE,
            ));
        }
        0x0A7C => {
            return Some(Instr::new(
                Op::EoriToSr,
                Size::Word,
                Ea::new(Mode::Immediate, 4),
                Ea::NONE,
            ));
        }
        _ => {}
    }

    let imm = Ea::new(Mode::Immediate, 4);
    let kind = (op >> 9) & 7;
    if kind == 4 {
        // Static bit operations: bit number in an extension word.
        let (bit_op, allowed) = bit_op(op);
        let dst = ea_in(op, allowed)?;
        return Some(Instr::new(
            bit_op,
            bit_size(dst),
            Ea::new(Mode::Immediate, 4),
            dst,
        ));
    }
    let size = size_76(op)?;
    let operation = match kind {
        0 => Op::Or,
        1 => Op::And,
        2 => Op::Sub,
        3 => Op::Add,
        5 => Op::Eor,
        6 => Op::Cmp,
        _ => return None,
    };
    let dst = ea_in(op, class::DATA_ALTERABLE)?;
    Some(Instr::new(operation, size, imm, dst))
}

/// Which bit operation bits 7..6 select, and the modes it allows.
fn bit_op(op: u16) -> (Op, u16) {
    match (op >> 6) & 3 {
        // BTST only reads, so PC-relative operands are fine.
        0 => (Op::Btst, class::DATA & !(1 << Mode::Immediate as u16)),
        1 => (Op::Bchg, class::DATA_ALTERABLE),
        2 => (Op::Bclr, class::DATA_ALTERABLE),
        _ => (Op::Bset, class::DATA_ALTERABLE),
    }
}

/// Bit operations work on all 32 bits of a data register, but on a single
/// byte in memory.
fn bit_size(dst: Ea) -> Size {
    if dst.mode == Mode::DataReg {
        Size::Long
    } else {
        Size::Byte
    }
}

/// Lines 1–3: MOVE and MOVEA. The destination field is encoded with
/// register and mode swapped compared with the source (`rrr mmm`).
fn line_move(op: u16, size: Size) -> Option<Instr> {
    let src = ea_sized(op, size, class::ALL)?;
    let dst_field = ((op >> 9) & 7) | ((op >> 3) & 0x38);
    let dst = Ea::decode(dst_field)?;
    if dst.mode == Mode::AddrReg {
        return (size != Size::Byte).then_some(Instr::new(Op::Movea, size, src, dst));
    }
    (dst.class_bit() & class::DATA_ALTERABLE != 0).then_some(Instr::new(Op::Move, size, src, dst))
}

/// Line 4: the miscellany.
fn line_4(op: u16) -> Option<Instr> {
    // Fixed opcodes first.
    match op {
        0x4AFC => return Some(Instr::bare(Op::Illegal)),
        0x4E70 => return Some(Instr::bare(Op::Reset)),
        0x4E71 => return Some(Instr::bare(Op::Nop)),
        0x4E72 => return Some(Instr::bare(Op::Stop)),
        0x4E73 => return Some(Instr::bare(Op::Rte)),
        0x4E75 => return Some(Instr::bare(Op::Rts)),
        0x4E76 => return Some(Instr::bare(Op::Trapv)),
        0x4E77 => return Some(Instr::bare(Op::Rtr)),
        _ => {}
    }

    if op & 0x0100 != 0 {
        // 0100 rrr1 x1 ea: LEA (111) and CHK.W (110).
        let reg = reg_hi(op);
        return match (op >> 6) & 7 {
            7 => Some(Instr::new(
                Op::Lea,
                Size::Long,
                ea_in(op, class::CONTROL)?,
                Ea::addr(reg),
            )),
            6 => Some(Instr::new(
                Op::Chk,
                Size::Word,
                ea_in(op, class::DATA)?,
                Ea::data(reg),
            )),
            _ => None,
        };
    }

    let reg = Ea::addr(reg_lo(op));
    match (op >> 8) & 0xF {
        0x0 | 0x2 | 0x4 | 0x6 => {
            // NEGX/CLR/NEG/NOT, or (size field = 11) MOVE from/to SR/CCR.
            let unary = match (op >> 9) & 3 {
                0 => Op::Negx,
                1 => Op::Clr,
                2 => Op::Neg,
                _ => Op::Not,
            };
            match size_76(op) {
                Some(size) => Some(Instr::unary(unary, size, ea_in(op, class::DATA_ALTERABLE)?)),
                None => match unary {
                    Op::Negx => Some(Instr::unary(
                        Op::MoveFromSr,
                        Size::Word,
                        ea_in(op, class::DATA_ALTERABLE)?,
                    )),
                    Op::Neg => Some(Instr::new(
                        Op::MoveToCcr,
                        Size::Word,
                        ea_in(op, class::DATA)?,
                        Ea::NONE,
                    )),
                    Op::Not => Some(Instr::new(
                        Op::MoveToSr,
                        Size::Word,
                        ea_in(op, class::DATA)?,
                        Ea::NONE,
                    )),
                    // MOVE from CCR only exists from the 68010 on.
                    _ => None,
                },
            }
        }
        0x8 => match (op >> 6) & 3 {
            0 => Some(Instr::unary(
                Op::Nbcd,
                Size::Byte,
                ea_in(op, class::DATA_ALTERABLE)?,
            )),
            1 if (op >> 3) & 7 == 0 => {
                Some(Instr::unary(Op::Swap, Size::Long, Ea::data(reg_lo(op))))
            }
            1 => Some(Instr::new(
                Op::Pea,
                Size::Long,
                ea_in(op, class::CONTROL)?,
                Ea::NONE,
            )),
            sz if (op >> 3) & 7 == 0 => {
                let size = if sz == 2 { Size::Word } else { Size::Long };
                Some(Instr::unary(Op::Ext, size, Ea::data(reg_lo(op))))
            }
            sz => {
                let size = if sz == 2 { Size::Word } else { Size::Long };
                let allowed = class::CONTROL_ALTERABLE | (1 << Mode::PreDec as u16);
                Some(Instr::new(
                    Op::MovemToMem,
                    size,
                    Ea::NONE,
                    ea_in(op, allowed)?,
                ))
            }
        },
        0xA => match size_76(op) {
            Some(size) => Some(Instr::unary(
                Op::Tst,
                size,
                ea_in(op, class::DATA_ALTERABLE)?,
            )),
            None => Some(Instr::unary(
                Op::Tas,
                Size::Byte,
                ea_in(op, class::DATA_ALTERABLE)?,
            )),
        },
        0xC => {
            if op & 0x80 == 0 {
                return None; // MULx.L/DIVx.L are 68020+
            }
            let size = if op & 0x40 != 0 {
                Size::Long
            } else {
                Size::Word
            };
            let allowed = class::CONTROL | (1 << Mode::PostInc as u16);
            Some(Instr::new(
                Op::MovemToReg,
                size,
                ea_in(op, allowed)?,
                Ea::NONE,
            ))
        }
        0xE => match (op >> 4) & 0xF {
            0x4 => Some(Instr::bare(Op::Trap)),
            0x5 if op & 8 == 0 => Some(Instr::unary(Op::Link, Size::Word, reg)),
            0x5 => Some(Instr::unary(Op::Unlk, Size::Long, reg)),
            0x6 if op & 8 == 0 => Some(Instr::unary(Op::MoveToUsp, Size::Long, reg)),
            0x6 => Some(Instr::unary(Op::MoveFromUsp, Size::Long, reg)),
            0x8..=0xB => Some(Instr::new(
                Op::Jsr,
                Size::Long,
                ea_in(op, class::CONTROL)?,
                Ea::NONE,
            )),
            0xC..=0xF => Some(Instr::new(
                Op::Jmp,
                Size::Long,
                ea_in(op, class::CONTROL)?,
                Ea::NONE,
            )),
            _ => None,
        },
        _ => None,
    }
}

/// Line 5: ADDQ, SUBQ, Scc, DBcc.
fn line_5(op: u16) -> Option<Instr> {
    let Some(size) = size_76(op) else {
        // Scc / DBcc (DBcc is Scc's slot with mode An).
        if (op >> 3) & 7 == 1 {
            return Some(Instr::unary(Op::Dbcc, Size::Word, Ea::data(reg_lo(op))));
        }
        return Some(Instr::unary(
            Op::Scc,
            Size::Byte,
            ea_in(op, class::DATA_ALTERABLE)?,
        ));
    };
    // The quick value 1..8 is encoded with 8 as 0.
    let data = match reg_hi(op) {
        0 => 8,
        n => n,
    };
    let dst = ea_sized(op, size, class::ALTERABLE)?;
    let subtract = op & 0x100 != 0;
    let operation = match (dst.mode, subtract) {
        (Mode::AddrReg, false) => Op::Adda,
        (Mode::AddrReg, true) => Op::Suba,
        (_, false) => Op::Add,
        (_, true) => Op::Sub,
    };
    Some(Instr::new(operation, size, Ea::quick(data), dst))
}

/// Line 6: branches. The condition and displacement are read by the handler.
fn line_6(op: u16) -> Option<Instr> {
    Some(Instr::bare(if (op >> 8) & 0xF == 1 {
        Op::Bsr
    } else {
        Op::Bcc
    }))
}

/// Line 7: MOVEQ (bit 8 must be clear).
fn line_7(op: u16) -> Option<Instr> {
    (op & 0x100 == 0).then_some(Instr::new(
        Op::Moveq,
        Size::Long,
        Ea::quick(op as u8),
        Ea::data(reg_hi(op)),
    ))
}

/// Line 8: OR, DIVU/DIVS, SBCD.
fn line_8(op: u16) -> Option<Instr> {
    let reg = Ea::data(reg_hi(op));
    match (op >> 6) & 7 {
        3 => Some(Instr::new(
            Op::Divu,
            Size::Word,
            ea_in(op, class::DATA)?,
            reg,
        )),
        7 => Some(Instr::new(
            Op::Divs,
            Size::Word,
            ea_in(op, class::DATA)?,
            reg,
        )),
        4 if (op >> 4) & 3 == 0 => Some(bcd_pair(op, Op::Sbcd)),
        _ => logic_pair(op, Op::Or),
    }
}

/// `ABCD`/`SBCD`/`ADDX`/`SUBX` operands: `Dy,Dx` or `-(Ay),-(Ax)` (bit 3).
fn register_pair(op: u16, operation: Op, size: Size) -> Instr {
    let mode = if op & 8 != 0 {
        Mode::PreDec
    } else {
        Mode::DataReg
    };
    Instr::new(
        operation,
        size,
        Ea::new(mode, reg_lo(op)),
        Ea::new(mode, reg_hi(op)),
    )
}

fn bcd_pair(op: u16, operation: Op) -> Instr {
    register_pair(op, operation, Size::Byte)
}

/// AND/OR `<ea>,Dn` (bit 8 clear) and `Dn,<ea>` (bit 8 set).
fn logic_pair(op: u16, operation: Op) -> Option<Instr> {
    let size = size_76(op)?;
    let reg = Ea::data(reg_hi(op));
    if op & 0x100 == 0 {
        Some(Instr::new(operation, size, ea_in(op, class::DATA)?, reg))
    } else {
        Some(Instr::new(
            operation,
            size,
            reg,
            ea_in(op, class::MEMORY_ALTERABLE)?,
        ))
    }
}

/// Lines 9 and D: SUB/ADD, SUBA/ADDA, SUBX/ADDX.
fn line_add_sub(op: u16, plain: Op, address: Op, extended: Op) -> Option<Instr> {
    let reg = reg_hi(op);
    let Some(size) = size_76(op) else {
        // Opmode x11: the address-register form, bit 8 selects the size.
        let size = if op & 0x100 != 0 {
            Size::Long
        } else {
            Size::Word
        };
        return Some(Instr::new(
            address,
            size,
            ea_in(op, class::ALL)?,
            Ea::addr(reg),
        ));
    };
    if op & 0x100 == 0 {
        Some(Instr::new(
            plain,
            size,
            ea_sized(op, size, class::ALL)?,
            Ea::data(reg),
        ))
    } else if (op >> 4) & 3 == 0 {
        // Register modes are useless as a memory destination: ADDX/SUBX.
        Some(register_pair(op, extended, size))
    } else {
        Some(Instr::new(
            plain,
            size,
            Ea::data(reg),
            ea_in(op, class::MEMORY_ALTERABLE)?,
        ))
    }
}

/// Line B: CMP, CMPA, CMPM, EOR.
fn line_b(op: u16) -> Option<Instr> {
    let reg = reg_hi(op);
    let Some(size) = size_76(op) else {
        let size = if op & 0x100 != 0 {
            Size::Long
        } else {
            Size::Word
        };
        return Some(Instr::new(
            Op::Cmpa,
            size,
            ea_in(op, class::ALL)?,
            Ea::addr(reg),
        ));
    };
    if op & 0x100 == 0 {
        Some(Instr::new(
            Op::Cmp,
            size,
            ea_sized(op, size, class::ALL)?,
            Ea::data(reg),
        ))
    } else if (op >> 3) & 7 == 1 {
        let src = Ea::new(Mode::PostInc, reg_lo(op));
        Some(Instr::new(Op::Cmpm, size, src, Ea::new(Mode::PostInc, reg)))
    } else {
        Some(Instr::new(
            Op::Eor,
            size,
            Ea::data(reg),
            ea_in(op, class::DATA_ALTERABLE)?,
        ))
    }
}

/// Line C: AND, MULU/MULS, ABCD, EXG.
fn line_c(op: u16) -> Option<Instr> {
    let reg = Ea::data(reg_hi(op));
    match (op >> 3) & 0x3F {
        // EXG's three forms live in AND Dn,<ea> slots with register modes.
        0x28 | 0x29 | 0x31 => return Some(Instr::bare(Op::Exg)),
        _ => {}
    }
    match (op >> 6) & 7 {
        3 => Some(Instr::new(
            Op::Mulu,
            Size::Word,
            ea_in(op, class::DATA)?,
            reg,
        )),
        7 => Some(Instr::new(
            Op::Muls,
            Size::Word,
            ea_in(op, class::DATA)?,
            reg,
        )),
        4 if (op >> 4) & 3 == 0 => Some(bcd_pair(op, Op::Abcd)),
        _ => logic_pair(op, Op::And),
    }
}

/// Line E: shifts and rotates.
fn line_e(op: u16) -> Option<Instr> {
    let left = op & 0x100 != 0;
    let Some(size) = size_76(op) else {
        // Memory form: one-bit shift of a word, type in bits 10..9.
        if op & 0x0800 != 0 {
            return None; // bit-field instructions are 68020+
        }
        let kind = ShiftKind::new(op >> 9, left);
        return Some(Instr::unary(
            Op::ShiftMem(kind),
            Size::Word,
            ea_in(op, class::MEMORY_ALTERABLE)?,
        ));
    };
    let kind = ShiftKind::new(op >> 3, left);
    // Bit 5: count in a data register, or an immediate 1..8 (0 means 8).
    let count = if op & 0x20 != 0 {
        Ea::data(reg_hi(op))
    } else {
        Ea::quick(match reg_hi(op) {
            0 => 8,
            n => n,
        })
    };
    Some(Instr::new(
        Op::Shift(kind),
        size,
        count,
        Ea::data(reg_lo(op)),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_representative_opcodes() {
        assert_eq!(decode(0x4E71).op, Op::Nop);
        assert_eq!(decode(0x4AFC).op, Op::Illegal);
        let add = decode(0xD041); // ADD.W D1,D0
        assert_eq!(
            (add.op, add.size, add.src, add.dst),
            (Op::Add, Size::Word, Ea::data(1), Ea::data(0))
        );
        assert_eq!(decode(0xD388).op, Op::Addx); // ADDX.L -(A0),-(A1)
        assert_eq!(decode(0xC340).op, Op::Exg); // EXG D1,D0
        assert_eq!(decode(0x2040).op, Op::Movea); // MOVEA.L D0,A0
        assert_eq!(decode(0x1048).op, Op::Illegal); // MOVEA.B does not exist
        assert_eq!(decode(0x0108).op, Op::Movep);
        assert_eq!(decode(0x51C8).op, Op::Dbcc); // DBF D0
        assert_eq!(decode(0xE1C0).op, Op::Illegal); // memory shift of Dn
    }
}
