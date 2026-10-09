//! Changes of flow: `Bcc`/`BRA`/`BSR`, `DBcc`, `Scc`, `JMP`, `JSR`, `RTS`,
//! `RTR`, `RTE`.
//!
//! Branch displacements are relative to the address of the word after the
//! opcode, and come in two sizes: an 8-bit displacement packed in the opcode
//! itself, or (when that byte is 0) a 16-bit one in an extension word.
//!
//! Any change of flow throws away the prefetch queue and refills it with two
//! fetches from the target. That is why a taken branch (10 cycles) costs more
//! than an untaken short one (8): the refill, plus 2 cycles to compute the
//! target. Jumping to an odd address raises an address error on the first
//! fetch.
//!
//! `DBcc Dn,label` is the 68000's loop instruction: *unless* the condition
//! is true, decrement the low word of `Dn` and branch if it did not wrap to
//! -1. `DBF` (condition false) is therefore a plain counted loop.
//!
//! `Scc` sets a byte to all ones if the condition holds, all zeros otherwise.

use crate::Bus;
use crate::cpu::M68k;
use crate::decode::Instr;
use crate::ea::{Ea, Mode, Size};
use crate::exceptions::Exec;

impl M68k {
    /// The branch target: the 8-bit displacement in the opcode, or (if that
    /// is 0) the 16-bit one waiting in IRC. Taken branches never refill IRC
    /// (they jump instead), and the PC is not advanced past a displacement
    /// word until the jump succeeds, which shows if the target is odd.
    #[inline]
    fn branch_target(&self) -> u32 {
        let disp8 = self.ird as u8;
        let disp = if disp8 == 0 { self.irc as i16 as u32 } else { disp8 as i8 as u32 };
        self.pc.wrapping_add(disp)
    }

    pub(crate) fn op_bcc<B: Bus>(&mut self, bus: &mut B) -> Exec {
        if self.condition(self.ird >> 8) {
            self.idle(2);
            let target = self.branch_target();
            self.jump(bus, target)
        } else {
            self.idle(4);
            if self.ird as u8 == 0 {
                // Skip over the unused displacement word.
                self.read_ext(bus);
            }
            self.prefetch(bus);
            Ok(())
        }
    }

    pub(crate) fn op_bsr<B: Bus>(&mut self, bus: &mut B) -> Exec {
        self.idle(2);
        let target = self.branch_target();
        let return_pc = if self.ird as u8 == 0 { self.pc.wrapping_add(2) } else { self.pc };
        self.push_long(bus, return_pc)?;
        // BSR has already loaded the target into its PC when the fetch
        // faults, so that is what an address error stacks.
        if target & 1 != 0 {
            self.pc = target;
        }
        self.jump(bus, target)
    }

    pub(crate) fn op_dbcc<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        if self.condition(self.ird >> 8) {
            self.idle(4);
            self.read_ext(bus);
            self.prefetch(bus);
            return Ok(());
        }
        let r = i.dst.reg;
        let counter = (self.d[r as usize] as u16).wrapping_sub(1);
        self.idle(2);
        if counter != 0xFFFF {
            let base = self.pc;
            let target = base.wrapping_add(self.take_ext() as i16 as u32);
            // The decremented counter is only written back once the branch
            // target has been fetched successfully.
            self.check_target(target)?;
            self.set_d(r, Size::Word, u32::from(counter));
            self.jump(bus, target)
        } else {
            self.set_d(r, Size::Word, u32::from(counter));
            // Loop finished: the 68000 fetches from the branch target anyway
            // before discovering it must fall through.
            self.idle(4);
            self.read_ext(bus);
            self.prefetch(bus);
            Ok(())
        }
    }

    pub(crate) fn op_scc<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        let condition = self.condition(self.ird >> 8);
        let value = if condition { 0xFF } else { 0 };
        if i.dst.mode == Mode::DataReg {
            self.set_d(i.dst.reg, Size::Byte, value);
            self.prefetch(bus);
            if condition {
                self.idle(2);
            }
            return Ok(());
        }
        // Like CLR, Scc reads the byte before overwriting it.
        let operand = self.resolve(bus, i.dst, Size::Byte);
        self.read_operand(bus, operand, Size::Byte)?;
        self.prefetch(bus);
        self.write_operand(bus, operand, Size::Byte, value)
    }

    /// Target of `JMP`/`JSR`. These take their last extension word straight
    /// from IRC without a refill (the queue is reloaded from the target
    /// anyway), spending that time on internal work instead. As with
    /// branches, `pc` is not advanced past that word yet; the second value
    /// returned is how far it still has to move.
    fn jump_target<B: Bus>(&mut self, bus: &mut B, ea: Ea) -> (u32, u32) {
        let r = ea.reg as usize;
        let target = match ea.mode {
            Mode::Indirect => return (self.a[r], 0),
            Mode::Disp => {
                self.idle(2);
                self.a[r].wrapping_add(self.irc as i16 as u32)
            }
            Mode::Index => {
                self.idle(6);
                let ext = self.irc;
                self.indexed(self.a[r], ext)
            }
            Mode::AbsShort => {
                self.idle(2);
                self.irc as i16 as u32
            }
            Mode::AbsLong => {
                let high = self.read_ext(bus);
                u32::from(high) << 16 | u32::from(self.irc)
            }
            Mode::PcDisp => {
                self.idle(2);
                let base = self.pc;
                base.wrapping_add(self.irc as i16 as u32)
            }
            Mode::PcIndex => {
                self.idle(6);
                let base = self.pc;
                let ext = self.irc;
                self.indexed(base, ext)
            }
            _ => unreachable!("JMP/JSR with non-control mode {ea:?}"),
        };
        (target, 2)
    }

    pub(crate) fn op_jmp<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        let start = self.pc;
        let (target, _) = self.jump_target(bus, i.src);
        // An odd target stacks the PC as it was at the start of the JMP.
        if target & 1 != 0 {
            self.pc = start;
        }
        self.jump(bus, target)
    }

    pub(crate) fn op_jsr<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        let (target, pending) = self.jump_target(bus, i.src);
        let return_pc = self.pc.wrapping_add(pending);
        // An odd target stacks the return address.
        self.pc = return_pc;
        // The first fetch from the target happens before the push, so an odd
        // target faults with nothing pushed.
        self.check_target(target)?;
        self.push_long(bus, return_pc)?;
        self.jump(bus, target)
    }

    pub(crate) fn op_rts<B: Bus>(&mut self, bus: &mut B) -> Exec {
        let target = self.pop_long(bus)?;
        self.jump(bus, target)
    }

    /// Return and restore condition codes (pops CCR as a word, then PC).
    pub(crate) fn op_rtr<B: Bus>(&mut self, bus: &mut B) -> Exec {
        let sp = self.a[7];
        let ccr = self.read_word(bus, sp)?;
        let target = self.read_long(bus, sp.wrapping_add(2))?;
        self.a[7] = sp.wrapping_add(6);
        self.set_ccr(ccr as u8);
        self.jump(bus, target)
    }

    /// Return from exception: pop SR and PC (privileged). Restoring SR may
    /// drop back to user mode, switching stacks.
    pub(crate) fn op_rte<B: Bus>(&mut self, bus: &mut B) -> Exec {
        self.require_supervisor()?;
        let sp = self.a[7];
        let sr = self.read_word(bus, sp)?;
        let target = self.read_long(bus, sp.wrapping_add(2))?;
        self.a[7] = sp.wrapping_add(6);
        self.set_sr(sr);
        self.jump(bus, target)
    }
}
