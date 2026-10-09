//! Exception processing: traps, errors, interrupts and trace.
//!
//! Every exception follows the same recipe:
//!
//! 1. copy SR, then switch to supervisor mode and clear the trace bit (so the
//!    handler runs privileged and untraced; interrupts also raise the mask);
//! 2. push a *stack frame* on the supervisor stack;
//! 3. read the handler address from the vector table (`vector * 4`);
//! 4. refill the prefetch queue from there.
//!
//! The ordinary ("group 1/2") frame is 6 bytes: the old SR, then the PC to
//! return to. Which PC is pushed depends on the exception: the *faulting*
//! instruction for illegal opcodes and privilege violations (so a handler can
//! emulate it), the *next* instruction for traps.
//!
//! Address errors ("group 0") abort an instruction half way through, so they
//! push a 14-byte frame with extra diagnostics:
//!
//! ```text
//!  SP+0   special status word: ...........R I FFF
//!         R = 1 for a read, I = 1 if not executing an instruction
//!         (e.g. fetching from a jump target), FFF = function code
//!  SP+2   access address (32 bits)
//!  SP+6   instruction register (the opcode being executed)
//!  SP+8   SR
//!  SP+10  PC (32 bits)
//! ```
//!
//! The upper bits of the status word are undocumented; the real chip leaves
//! bits of the opcode there, which is what we reproduce. The PC pushed is not
//! the instruction address but wherever the prefetch logic had got to, which
//! is why this core models the prefetch queue.
//!
//! An address error while pushing a group 0 frame is a *double bus fault*:
//! the 68000 gives up and halts until reset.

use crate::Bus;
use crate::cpu::{M68k, RunState};

/// Exception vector numbers (the table lives at `vector * 4`).
pub(crate) mod vector {
    pub(crate) const ADDRESS_ERROR: u8 = 3;
    pub(crate) const ILLEGAL: u8 = 4;
    pub(crate) const ZERO_DIVIDE: u8 = 5;
    pub(crate) const CHK: u8 = 6;
    pub(crate) const TRAPV: u8 = 7;
    pub(crate) const PRIVILEGE: u8 = 8;
    pub(crate) const TRACE: u8 = 9;
    pub(crate) const LINE_A: u8 = 10;
    pub(crate) const LINE_F: u8 = 11;
    /// Level `n` autovector is `AUTOVECTOR + n`.
    pub(crate) const AUTOVECTOR: u8 = 24;
    /// `TRAP #n` uses vector `TRAP + n`.
    pub(crate) const TRAP: u8 = 32;
}

/// The kind of bus access that hit an odd address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Access {
    Read,
    Write,
    /// An instruction fetch after a change of flow (jump, branch, return).
    Fetch,
}

/// Details of a word or long access to an odd address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AddressFault {
    pub address: u32,
    pub access: Access,
    /// The PC value that will be stacked.
    pub pc: u32,
}

/// Why an instruction stopped before completing.
///
/// Instruction handlers return `Result<_, Exception>`, so `?` unwinds out of
/// the middle of an instruction exactly like the hardware aborts it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Exception {
    AddressError(AddressFault),
    /// Illegal opcode, line A, line F or privilege violation: the instruction
    /// is not executed at all; the vector number says which.
    Rejected(u8),
}

/// Result type of everything that touches the bus during an instruction.
pub(crate) type Exec<T = ()> = Result<T, Exception>;

impl M68k {
    /// Steps 1 of every exception: returns the SR to stack.
    fn enter_supervisor(&mut self) -> u16 {
        let old = self.sr();
        self.set_supervisor(true);
        self.trace = false;
        old
    }

    /// Read the handler address for `vector` and start executing there.
    fn jump_to_vector<B: Bus>(&mut self, bus: &mut B, vector: u8) -> Exec {
        let handler = self.read_long(bus, u32::from(vector) * 4)?;
        self.jump_with_gap(bus, handler)
    }

    /// Take a group 1 or group 2 exception, stacking `return_pc`.
    ///
    /// Costs 34 cycles: 4 internal, 3 stack writes, 2 vector reads, 2 fetches
    /// with 2 internal cycles between them.
    pub(crate) fn exception<B: Bus>(&mut self, bus: &mut B, vector: u8, return_pc: u32) -> Exec {
        let sr = self.enter_supervisor();
        self.idle(4);
        self.push_frame(bus, return_pc, sr)?;
        self.jump_to_vector(bus, vector)
    }

    /// Push the 6-byte group 1/2 frame. The hardware writes the low word of
    /// the PC first, then SR, then the high word of the PC.
    fn push_frame<B: Bus>(&mut self, bus: &mut B, pc: u32, sr: u16) -> Exec {
        let sp = self.a[7].wrapping_sub(6);
        self.a[7] = sp;
        self.write_word(bus, sp.wrapping_add(4), pc as u16)?;
        self.write_word(bus, sp, sr)?;
        self.write_word(bus, sp.wrapping_add(2), (pc >> 16) as u16)
    }

    /// Handle an exception that escaped from an instruction.
    pub(crate) fn abort_instruction<B: Bus>(&mut self, bus: &mut B, exception: Exception) {
        let result = match exception {
            Exception::AddressError(fault) => {
                self.address_error(bus, fault);
                return;
            }
            Exception::Rejected(vector) => {
                // The stacked PC is the rejected instruction itself.
                let pc = self.instruction_pc;
                self.exception(bus, vector, pc)
            }
        };
        if let Err(Exception::AddressError(fault)) = result {
            self.address_error(bus, fault);
        }
    }

    /// Group 0: build the 14-byte address error frame (50 cycles).
    pub(crate) fn address_error<B: Bus>(&mut self, bus: &mut B, fault: AddressFault) {
        if self.in_group0 {
            // A second address error while stacking the first one: the
            // 68000 gives up ("double bus fault") and halts.
            self.run_state = RunState::Halted;
            return;
        }
        self.in_group0 = true;
        let sr = self.enter_supervisor();
        let function_code = if sr & 0x2000 != 0 { 4 } else { 0 }
            | if fault.access == Access::Fetch { 2 } else { 1 };
        let status = (self.ird & !0x1F)
            | if fault.access == Access::Write { 0 } else { 0x10 }
            | if fault.access == Access::Fetch { 0x08 } else { 0 }
            | function_code;
        // The aborted bus cycle still takes its 4 cycles, then 8 more pass
        // before the frame is written.
        self.idle(12);
        let result = (|| -> Exec {
            let sp = self.a[7].wrapping_sub(14);
            self.a[7] = sp;
            let at = |offset: u32| sp.wrapping_add(offset);
            self.write_word(bus, at(12), fault.pc as u16)?;
            self.write_word(bus, at(8), sr)?;
            self.write_word(bus, at(10), (fault.pc >> 16) as u16)?;
            self.write_word(bus, at(6), self.ird)?;
            self.write_word(bus, at(4), fault.address as u16)?;
            self.write_word(bus, at(0), status)?;
            self.write_word(bus, at(2), (fault.address >> 16) as u16)?;
            self.jump_to_vector(bus, vector::ADDRESS_ERROR)
        })();
        self.in_group0 = false;
        if result.is_err() {
            self.run_state = RunState::Halted;
        }
    }

    /// Is an interrupt waiting that the current mask lets through?
    #[inline]
    pub(crate) fn interrupt_pending(&self) -> bool {
        self.nmi_pending || self.ipl > self.int_mask
    }

    /// Accept the pending interrupt (44 cycles with an autovector).
    pub(crate) fn interrupt<B: Bus>(&mut self, bus: &mut B) {
        let level = if self.nmi_pending { 7 } else { self.ipl };
        self.nmi_pending = false;
        self.run_state = RunState::Running;
        let sr = self.enter_supervisor();
        self.int_mask = level;
        let pc = self.pc();
        let result = (|| -> Exec {
            self.idle(6);
            let sp = self.a[7].wrapping_sub(6);
            self.a[7] = sp;
            self.write_word(bus, sp.wrapping_add(4), pc as u16)?;
            // Interrupt acknowledge: the device either supplies a vector
            // number or asserts VPA to request an autovector. Autovectored
            // cycles are synchronised to the slow 6800-style E clock; we
            // charge the average.
            self.idle(4);
            let vector = match bus.interrupt_acknowledge(level) {
                Some(vector) => vector,
                None => {
                    self.idle(4);
                    vector::AUTOVECTOR + level
                }
            };
            self.write_word(bus, sp, sr)?;
            self.write_word(bus, sp.wrapping_add(2), (pc >> 16) as u16)?;
            self.jump_to_vector(bus, vector)
        })();
        if let Err(Exception::AddressError(fault)) = result {
            self.address_error(bus, fault);
        }
    }

    /// The trace exception, taken after an instruction that started with
    /// the T bit set (34 cycles).
    pub(crate) fn trace_exception<B: Bus>(&mut self, bus: &mut B) {
        let pc = self.pc();
        if let Err(Exception::AddressError(fault)) = self.exception(bus, vector::TRACE, pc) {
            self.address_error(bus, fault);
        }
    }
}
