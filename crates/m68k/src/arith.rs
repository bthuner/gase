//! Integer arithmetic: `ADD`, `SUB`, `CMP` and their `A`/`I`/`Q`/`X`/`M`
//! variants, `NEG`, `NEGX`, `CLR`, `TST`, `EXT`.
//!
//! ## Carry and overflow
//!
//! C is the unsigned carry (or borrow) out of the top bit; V is signed
//! overflow: the result has the wrong sign for the operands. Both fall out
//! of looking at the top bits of the two inputs and the result:
//!
//! * addition overflows when both inputs have the same sign and the result's
//!   sign differs: `(s ^ r) & (d ^ r)`;
//! * subtraction `d - s` overflows when the inputs differ in sign and the
//!   result's sign differs from `d`: `(s ^ d) & (r ^ d)`.
//!
//! X copies C for arithmetic, so a chain of `ADDX` can propagate a carry
//! across words of a big number. `ADDX`/`SUBX`/`NEGX` also only *clear* Z
//! (never set it), so after a chain Z tells whether the whole number is zero.
//!
//! `CMP` is a subtraction that only sets flags (and leaves X alone).
//!
//! ## The A variants
//!
//! `ADDA`/`SUBA`/`CMPA` treat the source as a signed address offset: word
//! sources are sign-extended and the whole 32-bit register is used. Address
//! arithmetic never changes the flags (except `CMPA`, whose job it is).
//!
//! ## Timing
//!
//! Byte and word operations into a data register take no time beyond their
//! bus accesses; the 16-bit ALU needs two passes for a long, costing 2 extra
//! cycles, or 4 when the source is a register or immediate (nothing to
//! overlap the second pass with).

use crate::Bus;
use crate::cpu::M68k;
use crate::decode::Instr;
use crate::ea::{Ea, Mode, Operand, Size};
use crate::exceptions::Exec;

/// Result and flags of `d + s + carry_in`.
#[derive(Clone, Copy)]
pub(crate) struct Sum {
    pub result: u32,
    pub carry: bool,
    pub overflow: bool,
}

/// Add with carry-in, at the given size.
#[inline]
pub(crate) fn add(d: u32, s: u32, carry_in: bool, size: Size) -> Sum {
    let result = d.wrapping_add(s).wrapping_add(u32::from(carry_in)) & size.mask();
    let msb = size.msb();
    let carry = ((s & d) | ((s | d) & !result)) & msb != 0;
    let overflow = (s ^ result) & (d ^ result) & msb != 0;
    Sum { result, carry, overflow }
}

/// Subtract `d - s - borrow_in`, at the given size.
#[inline]
pub(crate) fn sub(d: u32, s: u32, borrow_in: bool, size: Size) -> Sum {
    let result = d.wrapping_sub(s).wrapping_sub(u32::from(borrow_in)) & size.mask();
    let msb = size.msb();
    let carry = ((s & !d) | (result & !d) | (s & result)) & msb != 0;
    let overflow = (s ^ d) & (result ^ d) & msb != 0;
    Sum { result, carry, overflow }
}

impl M68k {
    fn set_arith_flags(&mut self, sum: Sum, size: Size) {
        self.set_nz(sum.result, size);
        self.v = sum.overflow;
        self.c = sum.carry;
        self.x = sum.carry;
    }

    /// Internal time of a long ALU operation into a data register.
    #[inline]
    pub(crate) fn long_alu_delay(&mut self, src: Ea) {
        self.idle(if src.is_register_or_immediate() { 4 } else { 2 });
    }

    /// Shared shape of ADD, SUB, AND, OR, EOR: `dst = dst op src`.
    ///
    /// When the destination is a data register, the source is any operand;
    /// otherwise the source is a register or immediate and the destination
    /// is read, modified and written back.
    #[inline]
    pub(crate) fn binary_op<B: Bus>(
        &mut self,
        bus: &mut B,
        i: Instr,
        op: impl FnOnce(&mut Self, u32, u32) -> u32,
    ) -> Exec {
        let s = self.read_ea(bus, i.src, i.size)?;
        if let Mode::DataReg = i.dst.mode {
            let d = self.d[i.dst.reg as usize] & i.size.mask();
            let r = op(self, d, s);
            self.prefetch(bus);
            if i.size == Size::Long {
                self.long_alu_delay(i.src);
            }
            self.set_d(i.dst.reg, i.size, r);
        } else {
            let dst = self.resolve(bus, i.dst, i.size);
            let d = self.read_operand(bus, dst, i.size)?;
            let r = op(self, d, s);
            self.prefetch(bus);
            self.write_operand(bus, dst, i.size, r)?;
        }
        Ok(())
    }

    pub(crate) fn op_add<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        self.binary_op(bus, i, |cpu, d, s| {
            let sum = add(d, s, false, i.size);
            cpu.set_arith_flags(sum, i.size);
            sum.result
        })
    }

    pub(crate) fn op_sub<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        self.binary_op(bus, i, |cpu, d, s| {
            let diff = sub(d, s, false, i.size);
            cpu.set_arith_flags(diff, i.size);
            diff.result
        })
    }

    /// `CMP <ea>,Dn` and `CMPI #,<ea>`: subtract for the flags only.
    pub(crate) fn op_cmp<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        let s = self.read_ea(bus, i.src, i.size)?;
        let d = self.read_ea(bus, i.dst, i.size)?;
        self.compare(d, s, i.size);
        self.prefetch(bus);
        if i.size == Size::Long && i.dst.mode == Mode::DataReg {
            self.idle(2);
        }
        Ok(())
    }

    fn compare(&mut self, d: u32, s: u32, size: Size) {
        let diff = sub(d, s, false, size);
        self.set_nz(diff.result, size);
        self.v = diff.overflow;
        self.c = diff.carry;
    }

    /// Source operand of an address-register operation: sign-extended.
    fn address_source<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec<u32> {
        Ok(i.size.sign_extend(self.read_ea(bus, i.src, i.size)?))
    }

    /// Internal time of ADDA/SUBA: the 32-bit add always needs two ALU
    /// passes.
    fn address_alu_delay(&mut self, i: Instr) {
        if i.size == Size::Word {
            self.idle(4);
        } else {
            self.long_alu_delay(i.src);
        }
    }

    pub(crate) fn op_adda<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        let s = self.address_source(bus, i)?;
        let r = i.dst.reg as usize;
        self.a[r] = self.a[r].wrapping_add(s);
        self.prefetch(bus);
        self.address_alu_delay(i);
        Ok(())
    }

    pub(crate) fn op_suba<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        let s = self.address_source(bus, i)?;
        let r = i.dst.reg as usize;
        self.a[r] = self.a[r].wrapping_sub(s);
        self.prefetch(bus);
        self.address_alu_delay(i);
        Ok(())
    }

    pub(crate) fn op_cmpa<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        let s = self.address_source(bus, i)?;
        self.compare(self.a[i.dst.reg as usize], s, Size::Long);
        self.prefetch(bus);
        self.idle(2);
        Ok(())
    }

    /// `CMPM (Ay)+,(Ax)+`: compare two memory blocks element by element.
    pub(crate) fn op_cmpm<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        self.fault_pc_bias = 2;
        let s = self.read_cmpm_source(bus, i.src.reg, i.size)?;
        // The destination register is only advanced once its read succeeds.
        let d = self.read_sized(bus, self.a[i.dst.reg as usize], i.size)?;
        self.post_increment(i.dst.reg, i.size);
        self.compare(d, s, i.size);
        self.prefetch(bus);
        Ok(())
    }

    /// CMPM's source `(An)+` read advances `An` a word at a time, so a long
    /// that faults on its first word still moves `An` by 2.
    fn read_cmpm_source<B: Bus>(&mut self, bus: &mut B, reg: u8, size: Size) -> Exec<u32> {
        let r = reg as usize;
        let addr = self.a[r];
        if size == Size::Long {
            self.a[r] = addr.wrapping_add(2);
            let high = self.read_word(bus, addr)?;
            self.a[r] = addr.wrapping_add(4);
            let low = self.read_word(bus, addr.wrapping_add(2))?;
            Ok(u32::from(high) << 16 | u32::from(low))
        } else {
            self.post_increment(reg, size);
            self.read_sized(bus, addr, size)
        }
    }

    /// ADDX/SUBX: `Dy,Dx` or `-(Ay),-(Ax)`, with X as carry-in.
    fn extended_op<B: Bus>(
        &mut self,
        bus: &mut B,
        i: Instr,
        op: fn(u32, u32, bool, Size) -> Sum,
    ) -> Exec {
        if i.src.mode == Mode::DataReg {
            let s = self.d[i.src.reg as usize] & i.size.mask();
            let d = self.d[i.dst.reg as usize] & i.size.mask();
            let sum = op(d, s, self.x, i.size);
            self.set_extended_flags(sum, i.size);
            self.prefetch(bus);
            if i.size == Size::Long {
                self.idle(4);
            }
            self.set_d(i.dst.reg, i.size, sum.result);
        } else {
            // Both operands are -(An): the decrement penalty is paid once.
            self.idle(2);
            self.fault_pc_bias = 2;
            let s = self.read_predecrement(bus, i.src.reg, i.size)?;
            let d = self.read_predecrement(bus, i.dst.reg, i.size)?;
            let dst_addr = self.a[i.dst.reg as usize];
            let sum = op(d, s, self.x, i.size);
            self.set_extended_flags(sum, i.size);
            self.prefetch(bus);
            self.write_sized(bus, dst_addr, i.size, sum.result)?;
        }
        Ok(())
    }

    /// Read a `-(An)` operand the way `ADDX`/`SUBX` do: a long is read low
    /// word first, so an odd `An` faults at `An - 2` (leaving `An` as it
    /// was).
    fn read_predecrement<B: Bus>(&mut self, bus: &mut B, reg: u8, size: Size) -> Exec<u32> {
        if size == Size::Long {
            let high_addr = self.a[reg as usize].wrapping_sub(4);
            let low = self.read_word(bus, high_addr.wrapping_add(2))?;
            let high = self.read_word(bus, high_addr)?;
            self.a[reg as usize] = high_addr;
            Ok(u32::from(high) << 16 | u32::from(low))
        } else {
            let addr = self.predecrement(reg, size);
            self.read_sized(bus, addr, size)
        }
    }

    /// Flags of ADDX/SUBX/NEGX: Z is only ever cleared.
    fn set_extended_flags(&mut self, sum: Sum, size: Size) {
        let z = self.z;
        self.set_arith_flags(sum, size);
        self.z = z && sum.result == 0;
    }

    pub(crate) fn op_addx<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        self.extended_op(bus, i, add)
    }

    pub(crate) fn op_subx<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        self.extended_op(bus, i, sub)
    }

    /// Shared shape of NEG, NEGX, NOT, CLR: read, modify, write one operand.
    ///
    /// Note that `CLR` really does read its operand first: the 68000 reuses
    /// the read-modify-write microcode.
    pub(crate) fn unary_op<B: Bus>(
        &mut self,
        bus: &mut B,
        i: Instr,
        op: impl FnOnce(&mut Self, u32) -> u32,
    ) -> Exec {
        let operand = self.resolve(bus, i.dst, i.size);
        let value = self.read_operand(bus, operand, i.size)?;
        let result = op(self, value);
        self.prefetch(bus);
        if let Operand::Data(_) = operand {
            if i.size == Size::Long {
                self.idle(2);
            }
        }
        self.write_operand(bus, operand, i.size, result)
    }

    pub(crate) fn op_neg<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        self.unary_op(bus, i, |cpu, d| {
            let diff = sub(0, d, false, i.size);
            cpu.set_arith_flags(diff, i.size);
            diff.result
        })
    }

    pub(crate) fn op_negx<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        self.unary_op(bus, i, |cpu, d| {
            let diff = sub(0, d, cpu.x, i.size);
            cpu.set_extended_flags(diff, i.size);
            diff.result
        })
    }

    pub(crate) fn op_clr<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        self.unary_op(bus, i, |cpu, _| {
            cpu.set_logic_flags(0, i.size);
            0
        })
    }

    pub(crate) fn op_tst<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        let value = self.read_ea(bus, i.dst, i.size)?;
        self.set_logic_flags(value, i.size);
        self.prefetch(bus);
        Ok(())
    }

    /// `EXT.W` sign-extends a byte to a word, `EXT.L` a word to a long.
    pub(crate) fn op_ext<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        let r = i.dst.reg;
        let value = self.d[r as usize];
        let extended = match i.size {
            Size::Word => Size::Byte.sign_extend(value),
            _ => Size::Word.sign_extend(value),
        };
        self.set_logic_flags(extended, i.size);
        self.set_d(r, i.size, extended);
        self.prefetch(bus);
        Ok(())
    }
}
