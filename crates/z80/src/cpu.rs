//! The register file, the public API and the bus-cycle helpers that every
//! instruction is built from.

use crate::Bus;

/// A Zilog Z80 CPU.
///
/// All architectural registers are public fields so that debuggers, tests and
/// save-state code can inspect and poke them directly. The 8-bit registers
/// are stored individually; use the pair accessors ([`Z80::bc`],
/// [`Z80::set_hl`], ...) for 16-bit views.
///
/// See the [crate documentation](crate) for an overview of what each register
/// is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Z80 {
    /// Accumulator.
    pub a: u8,
    /// Flags; see [`crate::flags`].
    pub f: u8,
    /// General purpose register; also the loop counter of `DJNZ` and block I/O.
    pub b: u8,
    /// General purpose register; also the port low byte of `IN r,(C)`.
    pub c: u8,
    /// General purpose register.
    pub d: u8,
    /// General purpose register.
    pub e: u8,
    /// High byte of `HL`, the main 16-bit pointer register.
    pub h: u8,
    /// Low byte of `HL`.
    pub l: u8,

    /// Shadow `AF'`, swapped in by `EX AF,AF'`.
    pub af_alt: u16,
    /// Shadow `BC'`, swapped in by `EXX`.
    pub bc_alt: u16,
    /// Shadow `DE'`, swapped in by `EXX`.
    pub de_alt: u16,
    /// Shadow `HL'`, swapped in by `EXX`.
    pub hl_alt: u16,

    /// Index register (selected by the `DD` prefix).
    pub ix: u16,
    /// Index register (selected by the `FD` prefix).
    pub iy: u16,
    /// Stack pointer. The stack grows downwards.
    pub sp: u16,
    /// Program counter.
    pub pc: u16,

    /// Interrupt vector base: high byte of the vector table address in IM 2.
    pub i: u8,
    /// Memory refresh counter. The low 7 bits count M1 (opcode fetch) cycles;
    /// bit 7 only changes through `LD R,A`.
    pub r: u8,

    /// Interrupt enable flip-flop: maskable interrupts are accepted when set.
    pub iff1: bool,
    /// Copy of `IFF1` saved while an NMI is serviced; restored by `RETN`,
    /// readable through the P/V flag of `LD A,I` / `LD A,R`.
    pub iff2: bool,
    /// Interrupt mode (0, 1 or 2) selected by `IM n`.
    pub im: u8,

    /// The hidden internal register `WZ`, also known as MEMPTR. It latches
    /// addresses during instruction execution and leaks into the X/Y flags of
    /// `BIT n,(HL)`.
    pub wz: u16,
    /// The hidden `Q` latch: the value of `F` if the previous instruction
    /// changed the flags, 0 otherwise. `SCF` and `CCF` use it to compute X/Y.
    pub q: u8,
    /// Set by `EI`: the instruction right after `EI` is always executed before
    /// a maskable interrupt can be accepted (so `EI; RET` is atomic).
    pub ei_delay: bool,
    /// Set by `LD A,I` / `LD A,R`. On NMOS Z80s an interrupt accepted right
    /// after one of those clears the P/V flag the instruction just set.
    pub ld_a_ir: bool,
    /// Set by `HALT`: the CPU executes internal NOPs until an interrupt.
    pub halted: bool,

    /// Current level of the (active low) `/INT` input, as last set by
    /// [`Z80::set_irq`].
    pub irq_line: bool,
    /// An edge was seen on `/NMI` and has not been serviced yet.
    pub nmi_pending: bool,

    /// T-states consumed by the step in progress.
    pub(crate) t: u32,
    /// The instruction in progress has written `F` (drives `Q`).
    pub(crate) flags_written: bool,
}

impl Default for Z80 {
    fn default() -> Self {
        Self::new()
    }
}

impl Z80 {
    /// A CPU in its power-on state.
    ///
    /// Registers that the hardware leaves undefined at power on are set to
    /// `0xFF`, which is what real chips most often show.
    #[must_use]
    pub fn new() -> Self {
        let mut cpu = Self {
            a: 0xFF,
            f: 0xFF,
            b: 0xFF,
            c: 0xFF,
            d: 0xFF,
            e: 0xFF,
            h: 0xFF,
            l: 0xFF,
            af_alt: 0xFFFF,
            bc_alt: 0xFFFF,
            de_alt: 0xFFFF,
            hl_alt: 0xFFFF,
            ix: 0xFFFF,
            iy: 0xFFFF,
            sp: 0xFFFF,
            pc: 0,
            i: 0,
            r: 0,
            iff1: false,
            iff2: false,
            im: 0,
            wz: 0,
            q: 0,
            ei_delay: false,
            ld_a_ir: false,
            halted: false,
            irq_line: false,
            nmi_pending: false,
            t: 0,
            flags_written: false,
        };
        cpu.reset();
        cpu
    }

    /// Pulse the `/RESET` line.
    ///
    /// A reset clears `PC`, `I`, `R`, both interrupt flip-flops and the
    /// interrupt mode; it sets `AF` and `SP` to `0xFFFF`. Other registers keep
    /// their values, as on the real chip. The level of `/INT` is an external
    /// signal and is not affected.
    pub fn reset(&mut self) {
        self.pc = 0;
        self.i = 0;
        self.r = 0;
        self.iff1 = false;
        self.iff2 = false;
        self.im = 0;
        self.set_af(0xFFFF);
        self.sp = 0xFFFF;
        self.wz = 0;
        self.q = 0;
        self.ei_delay = false;
        self.ld_a_ir = false;
        self.halted = false;
        self.nmi_pending = false;
    }

    /// Execute one instruction, or accept a pending interrupt, or idle for one
    /// NOP while halted. Returns the number of T-states (clock cycles) used.
    ///
    /// Prefixes are not separate steps: `DD CB 05 06` is executed in one call,
    /// as the real CPU never accepts an interrupt between a prefix and the
    /// opcode it modifies.
    pub fn step<B: Bus>(&mut self, bus: &mut B) -> u32 {
        self.t = 0;

        // Interrupts are sampled at the end of the previous instruction, i.e.
        // before the next one starts. NMI wins over INT.
        if self.nmi_pending {
            self.accept_nmi(bus);
            return self.t;
        }
        if self.irq_line && self.iff1 && !self.ei_delay {
            self.accept_int(bus);
            return self.t;
        }

        self.ei_delay = false;
        self.ld_a_ir = false;
        self.flags_written = false;

        if self.halted {
            // HALT keeps fetching (and discarding) opcodes so that DRAM refresh
            // continues; PC already points past the HALT instruction.
            self.inc_r();
            self.t = 4;
        } else {
            let op = self.fetch_opcode(bus);
            self.execute(bus, op);
        }

        self.q = if self.flags_written { self.f } else { 0 };
        self.t
    }

    /// The program counter.
    #[must_use]
    pub fn pc(&self) -> u16 {
        self.pc
    }

    /// Whether the CPU is halted (waiting for an interrupt).
    #[must_use]
    pub fn is_halted(&self) -> bool {
        self.halted
    }

    // ---------------------------------------------------------------------
    // 16-bit register pairs
    // ---------------------------------------------------------------------

    /// `AF` as a 16-bit value.
    #[must_use]
    pub fn af(&self) -> u16 {
        u16::from_be_bytes([self.a, self.f])
    }
    /// `BC` as a 16-bit value.
    #[must_use]
    pub fn bc(&self) -> u16 {
        u16::from_be_bytes([self.b, self.c])
    }
    /// `DE` as a 16-bit value.
    #[must_use]
    pub fn de(&self) -> u16 {
        u16::from_be_bytes([self.d, self.e])
    }
    /// `HL` as a 16-bit value.
    #[must_use]
    pub fn hl(&self) -> u16 {
        u16::from_be_bytes([self.h, self.l])
    }
    /// Set `AF`.
    pub fn set_af(&mut self, v: u16) {
        [self.a, self.f] = v.to_be_bytes();
    }
    /// Set `BC`.
    pub fn set_bc(&mut self, v: u16) {
        [self.b, self.c] = v.to_be_bytes();
    }
    /// Set `DE`.
    pub fn set_de(&mut self, v: u16) {
        [self.d, self.e] = v.to_be_bytes();
    }
    /// Set `HL`.
    pub fn set_hl(&mut self, v: u16) {
        [self.h, self.l] = v.to_be_bytes();
    }

    // ---------------------------------------------------------------------
    // Machine cycles
    //
    // Every instruction is a sequence of machine cycles (M-cycles). Building
    // instructions out of these helpers makes the T-state count fall out of
    // the code instead of living in a separate table:
    //
    //   opcode fetch (M1)  4 T  (also refreshes memory: R is incremented)
    //   memory read        3 T
    //   memory write       3 T
    //   I/O read / write   4 T  (one automatic wait state)
    //
    // plus a few "internal" T-states where the CPU is busy with the ALU or
    // the 16-bit incrementer and the bus is idle.
    // ---------------------------------------------------------------------

    /// Increment the 7 low bits of `R`, as every M1 cycle does.
    #[inline]
    pub(crate) fn inc_r(&mut self) {
        self.r = (self.r & 0x80) | (self.r.wrapping_add(1) & 0x7F);
    }

    /// M1 cycle: fetch an opcode (or prefix) byte.
    #[inline]
    pub(crate) fn fetch_opcode<B: Bus>(&mut self, bus: &mut B) -> u8 {
        let op = bus.read(self.pc);
        self.pc = self.pc.wrapping_add(1);
        self.inc_r();
        self.t += 4;
        op
    }

    /// Read an immediate operand byte at `PC`.
    #[inline]
    pub(crate) fn fetch_byte<B: Bus>(&mut self, bus: &mut B) -> u8 {
        let v = self.read(bus, self.pc);
        self.pc = self.pc.wrapping_add(1);
        v
    }

    /// Read a little-endian immediate word at `PC`.
    #[inline]
    pub(crate) fn fetch_word<B: Bus>(&mut self, bus: &mut B) -> u16 {
        let lo = self.fetch_byte(bus);
        let hi = self.fetch_byte(bus);
        u16::from_le_bytes([lo, hi])
    }

    /// Read a signed displacement and apply it to `base`: the `d` of `(IX+d)`
    /// and of relative jumps.
    #[inline]
    pub(crate) fn fetch_displaced<B: Bus>(&mut self, bus: &mut B, base: u16) -> u16 {
        let d = self.fetch_byte(bus) as i8;
        base.wrapping_add_signed(d.into())
    }

    #[inline]
    pub(crate) fn read<B: Bus>(&mut self, bus: &mut B, addr: u16) -> u8 {
        self.t += 3;
        bus.read(addr)
    }

    #[inline]
    pub(crate) fn write<B: Bus>(&mut self, bus: &mut B, addr: u16, v: u8) {
        self.t += 3;
        bus.write(addr, v);
    }

    #[inline]
    pub(crate) fn read_word<B: Bus>(&mut self, bus: &mut B, addr: u16) -> u16 {
        let lo = self.read(bus, addr);
        let hi = self.read(bus, addr.wrapping_add(1));
        u16::from_le_bytes([lo, hi])
    }

    #[inline]
    pub(crate) fn write_word<B: Bus>(&mut self, bus: &mut B, addr: u16, v: u16) {
        let [lo, hi] = v.to_le_bytes();
        self.write(bus, addr, lo);
        self.write(bus, addr.wrapping_add(1), hi);
    }

    #[inline]
    pub(crate) fn port_in<B: Bus>(&mut self, bus: &mut B, port: u16) -> u8 {
        self.t += 4;
        bus.port_in(port)
    }

    #[inline]
    pub(crate) fn port_out<B: Bus>(&mut self, bus: &mut B, port: u16, v: u8) {
        self.t += 4;
        bus.port_out(port, v);
    }

    /// T-states during which the bus is idle.
    #[inline]
    pub(crate) fn internal(&mut self, t: u32) {
        self.t += t;
    }

    /// Push a word: high byte first, at `SP-1`, then the low byte at `SP-2`.
    #[inline]
    pub(crate) fn push<B: Bus>(&mut self, bus: &mut B, v: u16) {
        let [lo, hi] = v.to_le_bytes();
        self.sp = self.sp.wrapping_sub(1);
        self.write(bus, self.sp, hi);
        self.sp = self.sp.wrapping_sub(1);
        self.write(bus, self.sp, lo);
    }

    #[inline]
    pub(crate) fn pop<B: Bus>(&mut self, bus: &mut B) -> u16 {
        let v = self.read_word(bus, self.sp);
        self.sp = self.sp.wrapping_add(2);
        v
    }

    /// Write `F` from an instruction that computes flags (as opposed to one
    /// that merely restores them, like `POP AF`); this is what feeds `Q`.
    #[inline]
    pub(crate) fn set_f(&mut self, f: u8) {
        self.f = f;
        self.flags_written = true;
    }
}
