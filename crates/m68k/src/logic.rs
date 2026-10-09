//! Bitwise logic: `AND`, `OR`, `EOR`, `NOT`, and the immediate forms that
//! target the condition codes or the whole status register.
//!
//! Logical operations set N and Z from the result and always clear V and C;
//! X is left alone (it belongs to arithmetic).
//!
//! `ANDI #,SR`, `ORI #,SR` and `EORI #,SR` are the classic way to change the
//! interrupt mask (`ORI #$0700,SR` masks every maskable interrupt). They are
//! privileged because they can clear the S bit or change the mask. After any
//! write to SR the 68000 discards and refetches its prefetch queue, since the
//! change may affect how the next instruction is fetched (trace, privilege).

use crate::Bus;
use crate::cpu::M68k;
use crate::decode::{Instr, Op};
use crate::exceptions::Exec;

impl M68k {
    pub(crate) fn op_and<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        self.binary_op(bus, i, |cpu, d, s| {
            cpu.set_logic_flags(d & s, i.size);
            d & s
        })
    }

    pub(crate) fn op_or<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        self.binary_op(bus, i, |cpu, d, s| {
            cpu.set_logic_flags(d | s, i.size);
            d | s
        })
    }

    pub(crate) fn op_eor<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        self.binary_op(bus, i, |cpu, d, s| {
            cpu.set_logic_flags(d ^ s, i.size);
            d ^ s
        })
    }

    pub(crate) fn op_not<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        self.unary_op(bus, i, |cpu, d| {
            cpu.set_logic_flags(!d, i.size);
            !d
        })
    }

    /// Apply the logical operation of an `xxxI to CCR/SR` instruction.
    fn apply_logic(op: Op, value: u16, imm: u16) -> u16 {
        match op {
            Op::AndiToCcr | Op::AndiToSr => value & imm,
            Op::OriToCcr | Op::OriToSr => value | imm,
            _ => value ^ imm,
        }
    }

    /// `ANDI/ORI/EORI #,CCR` (20 cycles).
    pub(crate) fn op_logic_to_ccr<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        let imm = self.read_ext(bus);
        let ccr = Self::apply_logic(i.op, u16::from(self.ccr()), imm);
        self.set_ccr(ccr as u8);
        self.idle(8);
        self.refetch(bus)
    }

    /// `ANDI/ORI/EORI #,SR` (20 cycles, privileged).
    pub(crate) fn op_logic_to_sr<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        self.require_supervisor()?;
        let imm = self.read_ext(bus);
        let sr = Self::apply_logic(i.op, self.sr(), imm);
        self.set_sr(sr);
        self.idle(8);
        self.refetch(bus)
    }

    /// Throw away the prefetch queue and refill it from the next
    /// instruction, as the 68000 does after changing SR.
    pub(crate) fn refetch<B: Bus>(&mut self, bus: &mut B) -> Exec {
        self.jump(bus, self.pc)
    }
}
