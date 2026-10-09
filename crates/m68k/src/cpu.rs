//! The CPU state, bus access helpers and the fetch/execute loop.

use std::fmt;

use gase_savestate::{Error as StateError, Reader, State, Writer};

use crate::Bus;
use crate::decode::{DecodeTable, Instr, Op};
use crate::ea::Size;
use crate::exceptions::{Access, AddressFault, Exception, Exec};

/// Only 24 address lines leave the chip; the top byte of an address is
/// ignored by the bus (but kept in registers and stack frames).
const ADDRESS_MASK: u32 = 0x00FF_FFFF;

/// Whether the CPU is executing instructions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RunState {
    /// Executing instructions.
    #[default]
    Running,
    /// Waiting for an interrupt after a `STOP` instruction.
    Stopped,
    /// Halted by a double bus fault; only a reset restarts the CPU.
    Halted,
}

/// A Motorola 68000.
///
/// Registers are public so debuggers and tests can inspect and set them.
/// The status register and the two stack pointers go through accessors
/// because changing the S bit swaps which stack pointer `A7` names.
#[derive(Clone)]
pub struct M68k {
    /// Data registers `D0`–`D7`.
    pub d: [u32; 8],
    /// Address registers `A0`–`A7`; `a[7]` is the *active* stack pointer.
    pub a: [u32; 8],
    /// The stack pointer that `A7` does not currently name (the USP in
    /// supervisor mode, the SSP in user mode).
    inactive_sp: u32,

    /// Address of the word held in `irc`. At an instruction boundary this is
    /// 2 bytes past the next opcode; see [`M68k::pc`] for the architectural PC.
    pub(crate) pc: u32,
    /// Prefetch queue: the next opcode...
    pub(crate) ir: u16,
    /// ...and the word after it.
    pub(crate) irc: u16,
    /// Opcode of the instruction being executed (what the hardware calls the
    /// decoded instruction register, IRD). Stacked by address errors.
    pub(crate) ird: u16,

    // Status register, unpacked for speed.
    pub(crate) trace: bool,
    supervisor: bool,
    pub(crate) int_mask: u8,
    pub(crate) x: bool,
    pub(crate) n: bool,
    pub(crate) z: bool,
    pub(crate) v: bool,
    pub(crate) c: bool,

    pub(crate) run_state: RunState,
    /// Current level on the interrupt priority inputs.
    pub(crate) ipl: u8,
    /// Level 7 is edge triggered: it is taken once per rising edge, even
    /// though it cannot be masked.
    pub(crate) nmi_pending: bool,
    /// The last instruction ran with the T bit set: a trace exception is
    /// due before the next one.
    pub(crate) trace_pending: bool,

    // Bookkeeping for the current `step` only (not part of the save state).
    /// Cycles spent so far in this step.
    cycles: u32,
    /// Address of the instruction being executed.
    pub(crate) instruction_pc: u32,
    /// Set while stacking an address error frame, to detect double faults.
    pub(crate) in_group0: bool,
    /// Where the microcode's own PC register stands relative to `pc` during
    /// the current memory operand access; an address error stacks
    /// `pc + fault_pc_bias`. See `ea.rs`.
    pub(crate) fault_pc_bias: i32,
    /// The current operand is a PC-relative (program space) read.
    pub(crate) program_space: bool,

    table: &'static DecodeTable,
}

impl Default for M68k {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for M68k {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("M68k")
            .field("d", &format_args!("{:08X?}", self.d))
            .field("a", &format_args!("{:08X?}", self.a))
            .field("usp", &format_args!("{:08X}", self.usp()))
            .field("ssp", &format_args!("{:08X}", self.ssp()))
            .field("pc", &format_args!("{:08X}", self.pc()))
            .field("sr", &format_args!("{:04X}", self.sr()))
            .field("prefetch", &format_args!("{:04X?}", self.prefetch_queue()))
            .field("run_state", &self.run_state)
            .field("ipl", &self.ipl)
            .finish_non_exhaustive()
    }
}

impl M68k {
    /// A CPU in its power-on state. Call [`M68k::reset`] before running it.
    #[must_use]
    pub fn new() -> Self {
        Self {
            d: [0; 8],
            a: [0; 8],
            inactive_sp: 0,
            pc: 0,
            ir: 0,
            irc: 0,
            ird: 0,
            trace: false,
            supervisor: true,
            int_mask: 7,
            x: false,
            n: false,
            z: false,
            v: false,
            c: false,
            run_state: RunState::Running,
            ipl: 0,
            nmi_pending: false,
            trace_pending: false,
            cycles: 0,
            instruction_pc: 0,
            in_group0: false,
            fault_pc_bias: 0,
            program_space: false,
            table: DecodeTable::get(),
        }
    }

    /// Reset: enter supervisor mode with all interrupts masked, load the
    /// initial SSP from address 0 and the initial PC from address 4.
    pub fn reset<B: Bus>(&mut self, bus: &mut B) {
        self.trace = false;
        self.set_supervisor(true);
        self.int_mask = 7;
        self.run_state = RunState::Running;
        self.nmi_pending = false;
        self.trace_pending = false;
        self.in_group0 = false;
        let read_long =
            |bus: &mut B, addr| u32::from(bus.read_word(addr)) << 16 | u32::from(bus.read_word(addr + 2));
        self.a[7] = read_long(bus, 0);
        let pc = read_long(bus, 4);
        self.set_pc(bus, pc);
    }

    // ----------------------------------------------------------------------
    // Registers
    // ----------------------------------------------------------------------

    /// The architectural program counter: the address of the next
    /// instruction to execute.
    #[must_use]
    pub fn pc(&self) -> u32 {
        self.pc.wrapping_sub(2)
    }

    /// Jump to `pc`, refilling the prefetch queue from `bus` (no cycles are
    /// charged). For debuggers and test setups.
    pub fn set_pc<B: Bus>(&mut self, bus: &mut B, pc: u32) {
        let word = |bus: &mut B, addr: u32| bus.read_word(addr & ADDRESS_MASK & !1);
        self.ir = word(bus, pc);
        self.irc = word(bus, pc.wrapping_add(2));
        self.pc = pc.wrapping_add(2);
    }

    /// The prefetch queue: the opcode at [`M68k::pc`] and the word after it.
    #[must_use]
    pub fn prefetch_queue(&self) -> [u16; 2] {
        [self.ir, self.irc]
    }

    /// Set the PC and the prefetch queue contents directly, e.g. to restore
    /// a state captured elsewhere. The queue need not match memory (it does
    /// not on real hardware after self-modifying code).
    pub fn set_pc_and_prefetch(&mut self, pc: u32, prefetch: [u16; 2]) {
        self.pc = pc.wrapping_add(2);
        [self.ir, self.irc] = prefetch;
    }

    /// The status register.
    #[must_use]
    pub fn sr(&self) -> u16 {
        u16::from(self.trace) << 15
            | u16::from(self.supervisor) << 13
            | u16::from(self.int_mask) << 8
            | u16::from(self.ccr())
    }

    /// Set the status register (switching stacks if the S bit changes).
    /// Unimplemented bits read back as zero.
    pub fn set_sr(&mut self, sr: u16) {
        self.trace = sr & 0x8000 != 0;
        self.set_supervisor(sr & 0x2000 != 0);
        self.int_mask = ((sr >> 8) & 7) as u8;
        self.set_ccr(sr as u8);
    }

    /// The condition code register (low byte of SR): `---XNZVC`.
    #[must_use]
    pub fn ccr(&self) -> u8 {
        u8::from(self.x) << 4
            | u8::from(self.n) << 3
            | u8::from(self.z) << 2
            | u8::from(self.v) << 1
            | u8::from(self.c)
    }

    /// Set the condition codes.
    pub fn set_ccr(&mut self, ccr: u8) {
        self.x = ccr & 0x10 != 0;
        self.n = ccr & 0x08 != 0;
        self.z = ccr & 0x04 != 0;
        self.v = ccr & 0x02 != 0;
        self.c = ccr & 0x01 != 0;
    }

    /// Is the CPU in supervisor mode?
    #[must_use]
    pub fn is_supervisor(&self) -> bool {
        self.supervisor
    }

    /// Change privilege mode, swapping the stack pointers.
    pub(crate) fn set_supervisor(&mut self, supervisor: bool) {
        if supervisor != self.supervisor {
            std::mem::swap(&mut self.a[7], &mut self.inactive_sp);
            self.supervisor = supervisor;
        }
    }

    /// The user stack pointer.
    #[must_use]
    pub fn usp(&self) -> u32 {
        if self.supervisor { self.inactive_sp } else { self.a[7] }
    }

    /// The supervisor stack pointer.
    #[must_use]
    pub fn ssp(&self) -> u32 {
        if self.supervisor { self.a[7] } else { self.inactive_sp }
    }

    /// Set the user stack pointer.
    pub fn set_usp(&mut self, value: u32) {
        if self.supervisor {
            self.inactive_sp = value;
        } else {
            self.a[7] = value;
        }
    }

    /// Set the supervisor stack pointer.
    pub fn set_ssp(&mut self, value: u32) {
        if self.supervisor {
            self.a[7] = value;
        } else {
            self.inactive_sp = value;
        }
    }

    /// Was the CPU stopped by a `STOP` instruction (and not yet woken)?
    #[must_use]
    pub fn is_stopped(&self) -> bool {
        self.run_state == RunState::Stopped
    }

    /// Has the CPU halted after a double bus fault?
    #[must_use]
    pub fn is_halted(&self) -> bool {
        self.run_state == RunState::Halted
    }

    /// Current run state.
    #[must_use]
    pub fn run_state(&self) -> RunState {
        self.run_state
    }

    /// Drive the interrupt priority inputs (0 = no interrupt, 1..=7).
    ///
    /// The level is sampled before each instruction. Levels 1–6 are level
    /// sensitive: the device must hold the line until it is acknowledged
    /// (and should drop it then, or the interrupt is taken again as soon as
    /// the handler lowers the mask). Level 7 is edge triggered and ignores
    /// the mask.
    pub fn set_interrupt_level(&mut self, level: u8) {
        let level = level.min(7);
        if level == 7 && self.ipl != 7 {
            self.nmi_pending = true;
        }
        self.ipl = level;
    }

    /// Write the low `size` bits of a data register, keeping the rest.
    #[inline]
    pub(crate) fn set_d(&mut self, reg: u8, size: Size, value: u32) {
        let r = &mut self.d[reg as usize];
        *r = (*r & !size.mask()) | (value & size.mask());
    }

    // ----------------------------------------------------------------------
    // Condition codes
    // ----------------------------------------------------------------------

    /// Set N and Z from a result.
    #[inline]
    pub(crate) fn set_nz(&mut self, value: u32, size: Size) {
        self.n = value & size.msb() != 0;
        self.z = value & size.mask() == 0;
    }

    /// The flags of logical operations and moves: N and Z from the result,
    /// V and C cleared, X untouched.
    #[inline]
    pub(crate) fn set_logic_flags(&mut self, value: u32, size: Size) {
        self.set_nz(value, size);
        self.v = false;
        self.c = false;
    }

    /// Evaluate one of the 16 conditions used by `Bcc`, `DBcc` and `Scc`.
    #[inline]
    pub(crate) fn condition(&self, cc: u16) -> bool {
        match cc & 0xF {
            0x0 => true,                            // T  (BRA)
            0x1 => false,                           // F  (BSR in Bcc's slot)
            0x2 => !self.c && !self.z,              // HI higher (unsigned)
            0x3 => self.c || self.z,                // LS lower or same
            0x4 => !self.c,                         // CC carry clear (HS)
            0x5 => self.c,                          // CS carry set (LO)
            0x6 => !self.z,                         // NE
            0x7 => self.z,                          // EQ
            0x8 => !self.v,                         // VC
            0x9 => self.v,                          // VS
            0xA => !self.n,                         // PL
            0xB => self.n,                          // MI
            0xC => self.n == self.v,                // GE (signed)
            0xD => self.n != self.v,                // LT
            0xE => !self.z && self.n == self.v,     // GT
            _ => self.z || self.n != self.v,        // LE
        }
    }

    // ----------------------------------------------------------------------
    // Bus access. Every access costs 4 clock cycles.
    // ----------------------------------------------------------------------

    /// Spend `cycles` clock cycles on internal work.
    #[inline]
    pub(crate) fn idle(&mut self, cycles: u32) {
        self.cycles += cycles;
    }

    /// An address error for a data access at `address`.
    #[cold]
    fn odd_access(&self, address: u32, access: Access) -> Exception {
        let access = if access == Access::Read && self.program_space { Access::ProgramRead } else { access };
        Exception::AddressError(AddressFault {
            address,
            access,
            pc: self.pc.wrapping_add_signed(self.fault_pc_bias),
        })
    }

    #[inline]
    pub(crate) fn read_byte<B: Bus>(&mut self, bus: &mut B, addr: u32) -> u8 {
        self.cycles += 4;
        bus.read_byte(addr & ADDRESS_MASK)
    }

    #[inline]
    pub(crate) fn read_word<B: Bus>(&mut self, bus: &mut B, addr: u32) -> Exec<u16> {
        if addr & 1 != 0 {
            return Err(self.odd_access(addr, Access::Read));
        }
        self.cycles += 4;
        Ok(bus.read_word(addr & ADDRESS_MASK))
    }

    /// Long accesses are two word accesses, high word first.
    #[inline]
    pub(crate) fn read_long<B: Bus>(&mut self, bus: &mut B, addr: u32) -> Exec<u32> {
        let high = self.read_word(bus, addr)?;
        let low = self.read_word(bus, addr.wrapping_add(2))?;
        Ok(u32::from(high) << 16 | u32::from(low))
    }

    #[inline]
    pub(crate) fn write_byte<B: Bus>(&mut self, bus: &mut B, addr: u32, value: u8) {
        self.cycles += 4;
        bus.write_byte(addr & ADDRESS_MASK, value);
    }

    #[inline]
    pub(crate) fn write_word<B: Bus>(&mut self, bus: &mut B, addr: u32, value: u16) -> Exec {
        if addr & 1 != 0 {
            return Err(self.odd_access(addr, Access::Write));
        }
        self.cycles += 4;
        bus.write_word(addr & ADDRESS_MASK, value);
        Ok(())
    }

    #[inline]
    pub(crate) fn write_long<B: Bus>(&mut self, bus: &mut B, addr: u32, value: u32) -> Exec {
        self.write_word(bus, addr, (value >> 16) as u16)?;
        self.write_word(bus, addr.wrapping_add(2), value as u16)
    }

    #[inline]
    pub(crate) fn read_sized<B: Bus>(&mut self, bus: &mut B, addr: u32, size: Size) -> Exec<u32> {
        Ok(match size {
            Size::Byte => u32::from(self.read_byte(bus, addr)),
            Size::Word => u32::from(self.read_word(bus, addr)?),
            Size::Long => self.read_long(bus, addr)?,
        })
    }

    #[inline]
    pub(crate) fn write_sized<B: Bus>(
        &mut self,
        bus: &mut B,
        addr: u32,
        size: Size,
        value: u32,
    ) -> Exec {
        match size {
            Size::Byte => {
                self.write_byte(bus, addr, value as u8);
                Ok(())
            }
            Size::Word => self.write_word(bus, addr, value as u16),
            Size::Long => self.write_long(bus, addr, value),
        }
    }

    /// Push a long onto the active stack.
    #[inline]
    pub(crate) fn push_long<B: Bus>(&mut self, bus: &mut B, value: u32) -> Exec {
        let sp = self.a[7].wrapping_sub(4);
        self.a[7] = sp;
        self.write_long(bus, sp, value)
    }

    /// Pop a long from the active stack.
    #[inline]
    pub(crate) fn pop_long<B: Bus>(&mut self, bus: &mut B) -> Exec<u32> {
        let value = self.read_long(bus, self.a[7])?;
        self.a[7] = self.a[7].wrapping_add(4);
        Ok(value)
    }

    // ----------------------------------------------------------------------
    // The prefetch queue
    // ----------------------------------------------------------------------

    /// Fetch a program word. `pc` is always even here: every change of flow
    /// goes through [`M68k::jump`], which checks alignment.
    #[inline]
    pub(crate) fn fetch<B: Bus>(&mut self, bus: &mut B, addr: u32) -> u16 {
        self.cycles += 4;
        bus.read_word(addr & ADDRESS_MASK)
    }

    /// Consume the extension word waiting in IRC and refill the queue.
    ///
    /// The refill is what makes extension words "cost" 4 cycles: the word
    /// itself was already fetched while the previous one was being used.
    #[inline]
    pub(crate) fn read_ext<B: Bus>(&mut self, bus: &mut B) -> u16 {
        let word = self.irc;
        self.pc = self.pc.wrapping_add(2);
        self.irc = self.fetch(bus, self.pc);
        word
    }

    /// Two extension words forming a long, high word first.
    #[inline]
    pub(crate) fn read_ext_long<B: Bus>(&mut self, bus: &mut B) -> u32 {
        let high = self.read_ext(bus);
        let low = self.read_ext(bus);
        u32::from(high) << 16 | u32::from(low)
    }

    /// Consume the word in IRC *without* refilling the queue. Instructions
    /// that are about to jump anyway (`JMP`, `BRA.W`, ...) save the fetch.
    #[inline]
    pub(crate) fn take_ext(&mut self) -> u16 {
        self.pc = self.pc.wrapping_add(2);
        self.irc
    }

    /// The fetch that ends almost every instruction: the word in IRC becomes
    /// the next opcode and the word after it is fetched into IRC.
    #[inline]
    pub(crate) fn prefetch<B: Bus>(&mut self, bus: &mut B) {
        self.ir = self.irc;
        self.pc = self.pc.wrapping_add(2);
        self.irc = self.fetch(bus, self.pc);
    }

    /// Change the flow of control: refill the whole queue from `target`.
    ///
    /// An odd target raises an address error on the first fetch. The new PC
    /// is not committed until that fetch succeeds, so the PC stacked is the
    /// jumping instruction's own.
    #[inline]
    pub(crate) fn jump<B: Bus>(&mut self, bus: &mut B, target: u32) -> Exec {
        self.check_target(target)?;
        self.ir = self.fetch(bus, target);
        self.pc = target.wrapping_add(2);
        self.irc = self.fetch(bus, self.pc);
        Ok(())
    }

    /// Like [`M68k::jump`] but with 2 internal cycles between the two
    /// fetches, as at the end of exception processing.
    pub(crate) fn jump_with_gap<B: Bus>(&mut self, bus: &mut B, target: u32) -> Exec {
        self.check_target(target)?;
        self.ir = self.fetch(bus, target);
        self.idle(2);
        self.pc = target.wrapping_add(2);
        self.irc = self.fetch(bus, self.pc);
        Ok(())
    }

    /// Raise an address error if a jump target is odd.
    #[inline]
    pub(crate) fn check_target(&self, target: u32) -> Exec {
        if target & 1 != 0 {
            return Err(Exception::AddressError(AddressFault {
                address: target,
                access: Access::Fetch,
                pc: self.pc,
            }));
        }
        Ok(())
    }

    // ----------------------------------------------------------------------
    // Execution
    // ----------------------------------------------------------------------

    /// Execute one instruction, or process one pending exception/interrupt,
    /// or idle while stopped/halted.
    ///
    /// Returns the number of CPU clock cycles consumed. Idling costs 4
    /// cycles per call.
    ///
    /// Exceptions that the hardware processes *between* instructions (trace,
    /// interrupts) each take a call of their own, in the hardware's priority
    /// order: an instruction executed with T set is followed by its trace
    /// exception, then any interrupt is accepted, all before the next
    /// instruction.
    pub fn step<B: Bus>(&mut self, bus: &mut B) -> u32 {
        self.cycles = 0;
        if self.run_state == RunState::Halted {
            return 4;
        }
        if self.trace_pending {
            self.trace_pending = false;
            self.trace_exception(bus);
            return self.cycles;
        }
        if self.interrupt_pending() {
            self.interrupt(bus);
            return self.cycles;
        }
        if self.run_state == RunState::Stopped {
            return 4;
        }

        let tracing = self.trace;
        self.fault_pc_bias = 0;
        self.program_space = false;
        let opcode = self.ir;
        self.ird = opcode;
        self.instruction_pc = self.pc();
        let instr = self.table.lookup(opcode);
        match self.execute(bus, instr) {
            // Instructions that trap (TRAP, CHK, ...) are still traced; ones
            // that are rejected (illegal, privileged) or abort are not.
            Ok(()) => self.trace_pending = tracing,
            Err(exception) => self.abort_instruction(bus, exception),
        }
        self.cycles
    }

    /// Dispatch a decoded instruction to its handler.
    fn execute<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        use crate::exceptions::vector;
        match i.op {
            Op::Add => self.op_add(bus, i),
            Op::Sub => self.op_sub(bus, i),
            Op::Cmp => self.op_cmp(bus, i),
            Op::Adda => self.op_adda(bus, i),
            Op::Suba => self.op_suba(bus, i),
            Op::Cmpa => self.op_cmpa(bus, i),
            Op::Addx => self.op_addx(bus, i),
            Op::Subx => self.op_subx(bus, i),
            Op::Cmpm => self.op_cmpm(bus, i),
            Op::Neg => self.op_neg(bus, i),
            Op::Negx => self.op_negx(bus, i),
            Op::Clr => self.op_clr(bus, i),
            Op::Tst => self.op_tst(bus, i),
            Op::Ext => self.op_ext(bus, i),
            Op::And => self.op_and(bus, i),
            Op::Or => self.op_or(bus, i),
            Op::Eor => self.op_eor(bus, i),
            Op::Not => self.op_not(bus, i),
            Op::AndiToCcr | Op::OriToCcr | Op::EoriToCcr => self.op_logic_to_ccr(bus, i),
            Op::AndiToSr | Op::OriToSr | Op::EoriToSr => self.op_logic_to_sr(bus, i),
            Op::Shift(kind) => self.op_shift_register(bus, i, kind),
            Op::ShiftMem(kind) => self.op_shift_memory(bus, i, kind),
            Op::Btst | Op::Bchg | Op::Bclr | Op::Bset => self.op_bit(bus, i),
            Op::Tas => self.op_tas(bus, i),
            Op::Abcd => self.op_abcd(bus, i),
            Op::Sbcd => self.op_sbcd(bus, i),
            Op::Nbcd => self.op_nbcd(bus, i),
            Op::Bcc => self.op_bcc(bus),
            Op::Bsr => self.op_bsr(bus),
            Op::Dbcc => self.op_dbcc(bus, i),
            Op::Scc => self.op_scc(bus, i),
            Op::Jmp => self.op_jmp(bus, i),
            Op::Jsr => self.op_jsr(bus, i),
            Op::Rts => self.op_rts(bus),
            Op::Rtr => self.op_rtr(bus),
            Op::Rte => self.op_rte(bus),
            Op::Mulu => self.op_mulu(bus, i),
            Op::Muls => self.op_muls(bus, i),
            Op::Divu => self.op_divu(bus, i),
            Op::Divs => self.op_divs(bus, i),
            Op::Move => self.op_move(bus, i),
            Op::Movea => self.op_movea(bus, i),
            Op::Moveq => self.op_moveq(bus, i),
            Op::MovemToMem => self.op_movem_to_memory(bus, i),
            Op::MovemToReg => self.op_movem_to_registers(bus, i),
            Op::Movep => self.op_movep(bus, i),
            Op::Lea => self.op_lea(bus, i),
            Op::Pea => self.op_pea(bus, i),
            Op::Exg => self.op_exg(bus),
            Op::Swap => self.op_swap(bus, i),
            Op::Link => self.op_link(bus, i),
            Op::Unlk => self.op_unlk(bus, i),
            Op::MoveFromSr => self.op_move_from_sr(bus, i),
            Op::MoveToCcr => self.op_move_to_ccr(bus, i),
            Op::MoveToSr => self.op_move_to_sr(bus, i),
            Op::MoveToUsp => self.op_move_to_usp(bus, i),
            Op::MoveFromUsp => self.op_move_from_usp(bus, i),
            Op::Chk => self.op_chk(bus, i),
            Op::Trap => self.op_trap(bus),
            Op::Trapv => self.op_trapv(bus),
            Op::Reset => self.op_reset(bus),
            Op::Stop => self.op_stop(bus),
            Op::Nop => {
                self.prefetch(bus);
                Ok(())
            }
            Op::Illegal => Err(Exception::Rejected(vector::ILLEGAL)),
            Op::LineA => Err(Exception::Rejected(vector::LINE_A)),
            Op::LineF => Err(Exception::Rejected(vector::LINE_F)),
        }
    }

    /// Fail with a privilege violation unless in supervisor mode.
    #[inline]
    pub(crate) fn require_supervisor(&self) -> Exec {
        if self.supervisor {
            Ok(())
        } else {
            Err(Exception::Rejected(crate::exceptions::vector::PRIVILEGE))
        }
    }
}

impl State for M68k {
    fn save(&self, w: &mut Writer) {
        self.d.save(w);
        self.a.save(w);
        self.inactive_sp.save(w);
        self.pc.save(w);
        self.ir.save(w);
        self.irc.save(w);
        self.ird.save(w);
        self.sr().save(w);
        let run_state: u8 = match self.run_state {
            RunState::Running => 0,
            RunState::Stopped => 1,
            RunState::Halted => 2,
        };
        run_state.save(w);
        self.ipl.save(w);
        self.nmi_pending.save(w);
        self.trace_pending.save(w);
    }

    fn load(&mut self, r: &mut Reader<'_>) -> Result<(), StateError> {
        self.d.load(r)?;
        self.a.load(r)?;
        self.inactive_sp.load(r)?;
        self.pc.load(r)?;
        self.ir.load(r)?;
        self.irc.load(r)?;
        self.ird.load(r)?;
        let mut sr = 0u16;
        sr.load(r)?;
        // Restore SR without swapping stacks: a[7] and inactive_sp were saved
        // as they were.
        self.supervisor = sr & 0x2000 != 0;
        self.set_sr(sr);
        let mut run_state = 0u8;
        run_state.load(r)?;
        self.run_state = match run_state {
            0 => RunState::Running,
            1 => RunState::Stopped,
            2 => RunState::Halted,
            _ => return Err(StateError::Invalid("68000 run state")),
        };
        self.ipl.load(r)?;
        self.nmi_pending.load(r)?;
        self.trace_pending.load(r)?;
        Ok(())
    }
}
