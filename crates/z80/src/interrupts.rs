//! Interrupt inputs and their acceptance sequences.
//!
//! The Z80 has two interrupt inputs:
//!
//! * `/NMI`, non-maskable and **edge**-triggered: a falling edge latches a
//!   request that is serviced at the end of the current instruction, always.
//!   The CPU saves `IFF1` in `IFF2`, clears `IFF1` and calls `0x0066`.
//!   `RETN` restores `IFF1` from `IFF2`.
//! * `/INT`, maskable and **level**-sensitive: while the line is held low and
//!   `IFF1` is set, the CPU acknowledges it at the end of each instruction.
//!   The device must release the line once acknowledged (on the Mega Drive
//!   the VDP holds it for about one scanline). Acceptance clears both IFFs,
//!   so the handler is not re-entered until it executes `EI`.
//!
//! What happens on an `/INT` acknowledge depends on the interrupt mode:
//!
//! * **IM 0** (8080 compatible): the device puts an instruction on the data
//!   bus and the CPU executes it. Almost always an `RST n`. A floating bus
//!   reads `0xFF` = `RST 38h`, which makes IM 0 behave like IM 1.
//! * **IM 1**: always `RST 38h`; the data bus is ignored. This is what the
//!   Mega Drive's sound drivers use.
//! * **IM 2**: the device supplies the low byte of a pointer whose high byte
//!   is `I`; the CPU calls the address stored there (a vectored interrupt).
//!
//! If the CPU was halted, accepting an interrupt resumes execution after the
//! `HALT` instruction.

use crate::flags::PV;
use crate::{Bus, Z80};

impl Z80 {
    /// Set the level of the maskable interrupt input. `true` means the
    /// (active low) line is asserted. The request stays pending for as long as
    /// the line is asserted.
    pub fn set_irq(&mut self, asserted: bool) {
        self.irq_line = asserted;
    }

    /// Signal a falling edge on the non-maskable interrupt input. It will be
    /// serviced before the next instruction.
    pub fn nmi(&mut self) {
        self.nmi_pending = true;
    }

    /// Bookkeeping shared by NMI and INT acceptance.
    fn begin_interrupt(&mut self) {
        self.halted = false;
        // NMOS quirk: if the instruction just executed was `LD A,I` or
        // `LD A,R`, the IFF2 copy it put in P/V is lost: the interrupt
        // acknowledge resets IFF2 while the flag is still being latched.
        if self.ld_a_ir {
            self.f &= !PV;
        }
        self.ei_delay = false;
        self.ld_a_ir = false;
        self.q = 0;
        // The acknowledge cycle is an M1 cycle, so R advances.
        self.inc_r();
    }

    /// Service a pending NMI: 5 T-states for the (ignored) opcode fetch, then
    /// a push of PC and a jump to 0x0066. 11 T-states in total.
    pub(crate) fn accept_nmi<B: Bus>(&mut self, bus: &mut B) {
        self.nmi_pending = false;
        self.begin_interrupt();
        self.iff1 = false;
        self.internal(5);
        self.push(bus, self.pc);
        self.pc = 0x0066;
        self.wz = self.pc;
    }

    /// Acknowledge the maskable interrupt. The acknowledge M1 cycle lasts 6
    /// T-states (4 plus two automatic wait states).
    pub(crate) fn accept_int<B: Bus>(&mut self, bus: &mut B) {
        self.begin_interrupt();
        self.iff1 = false;
        self.iff2 = false;
        self.internal(6);
        match self.im {
            0 => {
                // Execute whatever the device supplied as if it had been
                // fetched; an `RST n` totals 13 T-states. Multi-byte
                // instructions would take their operands from the bus; here
                // they are read from memory at PC, which no Mega Drive
                // software relies on.
                let op = bus.interrupt_data();
                self.flags_written = false;
                self.execute(bus, op);
                if self.flags_written {
                    self.q = self.f;
                }
                self.flags_written = false;
            }
            1 => {
                // RST 38h: 13 T-states.
                self.internal(1);
                self.push(bus, self.pc);
                self.pc = 0x0038;
                self.wz = self.pc;
            }
            _ => {
                // Vectored: 19 T-states.
                let vector = u16::from_be_bytes([self.i, bus.interrupt_data()]);
                self.internal(1);
                self.push(bus, self.pc);
                self.pc = self.read_word(bus, vector);
                self.wz = self.pc;
            }
        }
    }
}
