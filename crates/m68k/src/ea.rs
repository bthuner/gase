//! Operand sizes and effective addressing.
//!
//! Most 68000 instructions name their operands with a 6-bit *effective
//! address* field: a 3-bit mode and a 3-bit register number.
//!
//! ```text
//!  mode reg   syntax          extension words   address computation
//!  000  n     Dn              -                 (register)
//!  001  n     An              -                 (register)
//!  010  n     (An)            -                 An
//!  011  n     (An)+           -                 An, then An += size
//!  100  n     -(An)           -                 An -= size, then An
//!  101  n     (d16,An)        1                 An + sign_extend(d16)
//!  110  n     (d8,An,Xn)      1                 An + sign_extend(d8) + Xn
//!  111  000   (xxx).W         1                 sign_extend(word)
//!  111  001   (xxx).L         2                 long
//!  111  010   (d16,PC)        1                 PC + sign_extend(d16)
//!  111  011   (d8,PC,Xn)      1                 PC + sign_extend(d8) + Xn
//!  111  100   #imm            1 or 2            (the data itself)
//! ```
//!
//! Two details are worth knowing:
//!
//! * `(A7)+` and `-(A7)` on a byte move the stack pointer by **2**, keeping
//!   it word aligned (the stack is only ever accessed by words).
//! * The index extension word is `D/A rrr W/L 000 dddddddd`: which register
//!   is the index, whether only its low word (sign-extended) is used, and the
//!   8-bit displacement. Adding the index costs 2 extra internal cycles,
//!   just like the pre-decrement of `-(An)` does.
//!
//! PC-relative modes use the address of the extension word as their base,
//! which in this core is simply the internal `pc` at the time the word is
//! consumed (see [`M68k::prefetch`]).

use crate::cpu::M68k;
use crate::exceptions::Exec;
use crate::Bus;

/// The size of an operation (the `.B`, `.W`, `.L` suffix).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Size {
    Byte,
    Word,
    Long,
}

impl Size {
    /// Number of bytes the operand occupies in memory.
    #[inline]
    pub(crate) const fn bytes(self) -> u32 {
        match self {
            Size::Byte => 1,
            Size::Word => 2,
            Size::Long => 4,
        }
    }

    /// Number of bits.
    #[inline]
    pub(crate) const fn bits(self) -> u32 {
        self.bytes() * 8
    }

    /// Mask selecting the bits of a value that belong to this size.
    #[inline]
    pub(crate) const fn mask(self) -> u32 {
        match self {
            Size::Byte => 0xFF,
            Size::Word => 0xFFFF,
            Size::Long => 0xFFFF_FFFF,
        }
    }

    /// The sign bit for this size.
    #[inline]
    pub(crate) const fn msb(self) -> u32 {
        match self {
            Size::Byte => 0x80,
            Size::Word => 0x8000,
            Size::Long => 0x8000_0000,
        }
    }

    /// Sign-extend the low `self` bits of `value` to 32 bits.
    #[inline]
    pub(crate) const fn sign_extend(self, value: u32) -> u32 {
        match self {
            Size::Byte => value as u8 as i8 as u32,
            Size::Word => value as u16 as i16 as u32,
            Size::Long => value,
        }
    }

    /// Motorola suffix, for the disassembler.
    pub(crate) const fn suffix(self) -> &'static str {
        match self {
            Size::Byte => ".b",
            Size::Word => ".w",
            Size::Long => ".l",
        }
    }
}

/// The twelve addressing modes, plus one internal pseudo-mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    DataReg,
    AddrReg,
    Indirect,
    PostInc,
    PreDec,
    Disp,
    Index,
    AbsShort,
    AbsLong,
    PcDisp,
    PcIndex,
    Immediate,
    /// Not a real 68000 mode: a small constant embedded in the opcode itself
    /// (`ADDQ`/`SUBQ` data, shift counts). Treating it as an operand lets
    /// `ADDQ` share the `ADD` implementation. The value lives in `reg`.
    Quick,
}

/// A decoded effective address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Ea {
    pub mode: Mode,
    pub reg: u8,
}

impl Ea {
    pub(crate) const fn new(mode: Mode, reg: u8) -> Self {
        Self { mode, reg }
    }
    pub(crate) const fn data(reg: u8) -> Self {
        Self::new(Mode::DataReg, reg)
    }
    pub(crate) const fn addr(reg: u8) -> Self {
        Self::new(Mode::AddrReg, reg)
    }
    pub(crate) const fn quick(value: u8) -> Self {
        Self::new(Mode::Quick, value)
    }
    pub(crate) const NONE: Ea = Ea::data(0);

    /// Decode the 6-bit mode/register field from bits 5..0 of `field`.
    /// Returns `None` for the unused encodings `111 101`..`111 111`.
    pub(crate) const fn decode(field: u16) -> Option<Ea> {
        let reg = (field & 7) as u8;
        let mode = match (field >> 3) & 7 {
            0 => Mode::DataReg,
            1 => Mode::AddrReg,
            2 => Mode::Indirect,
            3 => Mode::PostInc,
            4 => Mode::PreDec,
            5 => Mode::Disp,
            6 => Mode::Index,
            _ => match reg {
                0 => Mode::AbsShort,
                1 => Mode::AbsLong,
                2 => Mode::PcDisp,
                3 => Mode::PcIndex,
                4 => Mode::Immediate,
                _ => return None,
            },
        };
        Some(Ea { mode, reg })
    }

    /// Is this a register (data or address) operand?
    #[inline]
    pub(crate) const fn is_register(self) -> bool {
        matches!(self.mode, Mode::DataReg | Mode::AddrReg)
    }

    /// Is this a register, an immediate or a quick constant? Long ALU
    /// operations with such a source take 2 more internal cycles than with a
    /// memory source, because the ALU is not overlapped with a memory read.
    #[inline]
    pub(crate) const fn is_register_or_immediate(self) -> bool {
        matches!(
            self.mode,
            Mode::DataReg | Mode::AddrReg | Mode::Immediate | Mode::Quick
        )
    }

    /// Does this operand live in memory?
    #[inline]
    pub(crate) const fn is_memory(self) -> bool {
        !matches!(
            self.mode,
            Mode::DataReg | Mode::AddrReg | Mode::Immediate | Mode::Quick
        )
    }

    /// Bit for this mode in an [`EaSet`].
    pub(crate) const fn class_bit(self) -> u16 {
        1 << self.mode as u16
    }
}

/// Sets of addressing modes allowed by an instruction, as bit masks over
/// [`Mode`] (Motorola's manual names these categories).
pub(crate) mod class {
    use super::Mode;
    const fn bit(m: Mode) -> u16 {
        1 << m as u16
    }
    pub(crate) const ALL: u16 = (1 << 12) - 1;
    /// Everything except `An`.
    pub(crate) const DATA: u16 = ALL & !bit(Mode::AddrReg);
    /// Everything that lives in memory.
    pub(crate) const MEMORY: u16 = DATA & !bit(Mode::DataReg);
    /// Modes that can be written: no PC-relative, no immediate.
    pub(crate) const ALTERABLE: u16 =
        ALL & !bit(Mode::PcDisp) & !bit(Mode::PcIndex) & !bit(Mode::Immediate);
    pub(crate) const DATA_ALTERABLE: u16 = DATA & ALTERABLE;
    pub(crate) const MEMORY_ALTERABLE: u16 = MEMORY & ALTERABLE;
    /// Memory modes that denote an address without size: what `LEA`, `JMP`,
    /// `PEA`... accept.
    pub(crate) const CONTROL: u16 = MEMORY
        & !bit(Mode::PostInc)
        & !bit(Mode::PreDec)
        & !bit(Mode::Immediate);
    pub(crate) const CONTROL_ALTERABLE: u16 = CONTROL & ALTERABLE;
}

/// Where an operand lives once its effective address has been computed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Operand {
    Data(u8),
    Addr(u8),
    Mem(u32),
    /// `(An)+`: memory at the address, with `An` incremented only once the
    /// first access has succeeded (an address error leaves `An` untouched).
    /// Reading performs the increment; a read-modify-write's second access
    /// does not repeat it.
    PostInc(u8, u32),
    Imm(u32),
}

impl M68k {
    /// How far `(An)+` / `-(An)` move `An` for an operand of `size`.
    #[inline]
    fn step_size(reg: u8, size: Size) -> u32 {
        if reg == 7 && size == Size::Byte { 2 } else { size.bytes() }
    }

    /// Increment `An` after a `(An)+` access.
    #[inline]
    pub(crate) fn post_increment(&mut self, reg: u8, size: Size) {
        let r = reg as usize;
        self.a[r] = self.a[r].wrapping_add(Self::step_size(reg, size));
    }

    /// Decrement `An` for a `-(An)` access and return the new address
    /// (without the 2 internal cycles that most instructions spend on it).
    #[inline]
    pub(crate) fn predecrement(&mut self, reg: u8, size: Size) -> u32 {
        let r = reg as usize;
        self.a[r] = self.a[r].wrapping_sub(Self::step_size(reg, size));
        self.a[r]
    }

    /// Compute `base + displacement + index` from an index extension word.
    #[inline]
    pub(crate) fn indexed(&self, base: u32, ext: u16) -> u32 {
        let reg = ((ext >> 12) & 7) as usize;
        let xn = if ext & 0x8000 != 0 { self.a[reg] } else { self.d[reg] };
        let xn = if ext & 0x0800 != 0 { xn } else { xn as u16 as i16 as u32 };
        base.wrapping_add(ext as u8 as i8 as u32).wrapping_add(xn)
    }

    /// Resolve an effective address into an [`Operand`]: read its extension
    /// words, apply `(An)+` / `-(An)` side effects and charge the internal
    /// cycles of the address calculation.
    #[inline]
    pub(crate) fn resolve<B: Bus>(&mut self, bus: &mut B, ea: Ea, size: Size) -> Operand {
        match ea.mode {
            Mode::DataReg => Operand::Data(ea.reg),
            Mode::AddrReg => Operand::Addr(ea.reg),
            Mode::PreDec => {
                self.idle(2);
                Operand::Mem(self.predecrement(ea.reg, size))
            }
            Mode::PostInc => Operand::PostInc(ea.reg, self.a[ea.reg as usize]),
            Mode::Immediate => Operand::Imm(match size {
                Size::Byte => u32::from(self.read_ext(bus) & 0xFF),
                Size::Word => u32::from(self.read_ext(bus)),
                Size::Long => self.read_ext_long(bus),
            }),
            Mode::Quick => Operand::Imm(u32::from(ea.reg)),
            _ => Operand::Mem(self.control_address(bus, ea)),
        }
    }

    /// The address denoted by a memory mode that has no side effects
    /// (`(An)`, displacements, indexed, absolute, PC-relative).
    #[inline]
    pub(crate) fn control_address<B: Bus>(&mut self, bus: &mut B, ea: Ea) -> u32 {
        let r = ea.reg as usize;
        match ea.mode {
            Mode::Indirect => self.a[r],
            Mode::Disp => {
                let disp = self.read_ext(bus) as i16 as u32;
                self.a[r].wrapping_add(disp)
            }
            Mode::Index => {
                let ext = self.read_ext(bus);
                self.idle(2);
                self.indexed(self.a[r], ext)
            }
            Mode::AbsShort => self.read_ext(bus) as i16 as u32,
            Mode::AbsLong => self.read_ext_long(bus),
            Mode::PcDisp => {
                let base = self.pc;
                base.wrapping_add(self.read_ext(bus) as i16 as u32)
            }
            Mode::PcIndex => {
                let base = self.pc;
                let ext = self.read_ext(bus);
                self.idle(2);
                self.indexed(base, ext)
            }
            // `(An)+`/`-(An)` are not control modes; callers that allow them
            // go through `resolve`.
            _ => unreachable!("not a control addressing mode: {ea:?}"),
        }
    }

    /// Read the value of a resolved operand.
    #[inline]
    pub(crate) fn read_operand<B: Bus>(
        &mut self,
        bus: &mut B,
        operand: Operand,
        size: Size,
    ) -> Exec<u32> {
        Ok(match operand {
            Operand::Data(r) => self.d[r as usize] & size.mask(),
            Operand::Addr(r) => self.a[r as usize] & size.mask(),
            Operand::Mem(addr) => self.read_sized(bus, addr, size)?,
            Operand::PostInc(reg, addr) => {
                let value = self.read_sized(bus, addr, size)?;
                self.post_increment(reg, size);
                value
            }
            Operand::Imm(value) => value,
        })
    }

    /// Write a value to a resolved operand. Data registers keep the bits
    /// above `size`; address registers are always written in full.
    #[inline]
    pub(crate) fn write_operand<B: Bus>(
        &mut self,
        bus: &mut B,
        operand: Operand,
        size: Size,
        value: u32,
    ) -> Exec {
        match operand {
            Operand::Data(r) => self.set_d(r, size, value),
            Operand::Addr(r) => self.a[r as usize] = value,
            Operand::Mem(addr) | Operand::PostInc(_, addr) => self.write_sized(bus, addr, size, value)?,
            Operand::Imm(_) => unreachable!("write to an immediate operand"),
        }
        Ok(())
    }

    /// Resolve and read in one go: the common case of a source operand.
    #[inline]
    pub(crate) fn read_ea<B: Bus>(&mut self, bus: &mut B, ea: Ea, size: Size) -> Exec<u32> {
        let operand = self.resolve(bus, ea, size);
        self.read_operand(bus, operand, size)
    }
}
