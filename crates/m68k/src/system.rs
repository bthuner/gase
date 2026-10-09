//! System control: status register moves, `MOVE USP`, `TRAP`, `TRAPV`,
//! `CHK`, `RESET`, `STOP`.
//!
//! The 68000 has two privilege levels. User mode cannot touch the system
//! byte of SR (`MOVE to SR`, `ANDI/ORI/EORI to SR`, `RTE`), the user stack
//! pointer (`MOVE USP`), or the machine (`RESET`, `STOP`): trying raises a
//! privilege violation. `MOVE from SR` is *not* privileged on the 68000 (it
//! is on the 68010 and later, a famous compatibility break).
//!
//! `TRAP #n` is the system call instruction (vectors 32–47). `TRAPV` traps
//! if V is set; `CHK` traps if a register is out of bounds (array index
//! checking).
//!
//! `STOP #sr` loads SR and halts instruction execution until an interrupt
//! (or trace, or reset) arrives: the way to idle the CPU until the next frame.

use crate::Bus;
use crate::cpu::{M68k, RunState};
use crate::decode::Instr;
use crate::ea::{Mode, Size};
use crate::exceptions::{Exec, vector};

impl M68k {
    /// `MOVE SR,<ea>`.
    pub(crate) fn op_move_from_sr<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        let sr = u32::from(self.sr());
        if i.dst.mode == Mode::DataReg {
            self.set_d(i.dst.reg, Size::Word, sr);
            self.prefetch(bus);
            self.idle(2);
            return Ok(());
        }
        let operand = self.resolve(bus, i.dst, Size::Word);
        self.read_operand(bus, operand, Size::Word)?;
        self.prefetch(bus);
        self.write_operand(bus, operand, Size::Word, sr)
    }

    /// `MOVE <ea>,CCR`: a word source, of which only the low byte is used.
    pub(crate) fn op_move_to_ccr<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        let value = self.read_ea(bus, i.src, Size::Word)?;
        self.set_ccr(value as u8);
        self.idle(4);
        self.refetch(bus)
    }

    /// `MOVE <ea>,SR` (privileged).
    pub(crate) fn op_move_to_sr<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        self.require_supervisor()?;
        let value = self.read_ea(bus, i.src, Size::Word)?;
        self.set_sr(value as u16);
        self.idle(4);
        self.refetch(bus)
    }

    /// `MOVE An,USP` (privileged).
    pub(crate) fn op_move_to_usp<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        self.require_supervisor()?;
        self.set_usp(self.a[i.dst.reg as usize]);
        self.prefetch(bus);
        Ok(())
    }

    /// `MOVE USP,An` (privileged).
    pub(crate) fn op_move_from_usp<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        self.require_supervisor()?;
        self.a[i.dst.reg as usize] = self.usp();
        self.prefetch(bus);
        Ok(())
    }

    /// `CHK <ea>,Dn`: trap unless 0 ≤ Dn.W ≤ bound.
    pub(crate) fn op_chk<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        let bound = self.read_ea(bus, i.src, Size::Word)? as u16 as i16;
        let value = self.d[i.dst.reg as usize] as u16 as i16;
        self.n = value < 0;
        self.z = value == 0;
        self.v = false;
        self.c = false;
        if (0..=bound).contains(&value) {
            self.prefetch(bus);
            self.idle(6);
            return Ok(());
        }
        // The microcode first computes `bound - value` and branches on its
        // sign and overflow; only a negative value whose difference fits in
        // 16 bits goes through the 2-cycle slower path.
        let (difference, overflow) = bound.overflowing_sub(value);
        // (Plus the 4 internal cycles that start every exception.)
        self.idle(if value < 0 && difference >= 0 && !overflow { 6 } else { 4 });
        let pc = self.pc;
        self.exception(bus, vector::CHK, pc)
    }

    /// `TRAP #n`: stacks the address of the next instruction.
    pub(crate) fn op_trap<B: Bus>(&mut self, bus: &mut B) -> Exec {
        let pc = self.pc;
        self.exception(bus, vector::TRAP + (self.ird & 0xF) as u8, pc)
    }

    pub(crate) fn op_trapv<B: Bus>(&mut self, bus: &mut B) -> Exec {
        if self.v {
            let pc = self.pc;
            self.exception(bus, vector::TRAPV, pc)
        } else {
            self.prefetch(bus);
            Ok(())
        }
    }

    /// `RESET`: assert the reset line for 124 clocks so peripherals restart.
    /// The CPU itself is not reset.
    pub(crate) fn op_reset<B: Bus>(&mut self, bus: &mut B) -> Exec {
        self.require_supervisor()?;
        bus.reset_devices();
        self.idle(128);
        self.prefetch(bus);
        Ok(())
    }

    /// `STOP #sr` (privileged).
    pub(crate) fn op_stop<B: Bus>(&mut self, bus: &mut B) -> Exec {
        self.require_supervisor()?;
        let _ = bus;
        // The new SR comes straight from IRC; no refill, since nothing
        // executes until an exception reloads the queue.
        let sr = self.take_ext();
        // Leave `pc` as it is at an instruction boundary, so the PC stacked
        // by the waking interrupt is the instruction after STOP.
        self.pc = self.pc.wrapping_add(2);
        self.set_sr(sr);
        self.run_state = RunState::Stopped;
        Ok(())
    }
}
