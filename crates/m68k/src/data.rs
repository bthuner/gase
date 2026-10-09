//! Data movement: `MOVE`, `MOVEA`, `MOVEQ`, `MOVEM`, `MOVEP`, `LEA`, `PEA`,
//! `EXG`, `SWAP`, `LINK`, `UNLK`.
//!
//! `MOVE` is by far the most common instruction, and the only one with two
//! full effective addresses. It sets N and Z from the value moved (clearing
//! V and C) *before* writing it, which is visible if the write faults.
//! `MOVEA` moves into an address register and, like all address register
//! writes, leaves the flags alone.
//!
//! `MOVEM` saves or restores a set of registers given by a 16-bit mask, the
//! standard way to preserve registers around a subroutine:
//!
//! ```text
//!     movem.l d0-d3/a0-a2,-(sp)    ; push
//!     movem.l (sp)+,d0-d3/a0-a2    ; pop
//! ```
//!
//! Mask bit 0 is `D0` and bit 15 is `A7`, except with `-(An)` where the
//! order is reversed so that registers still end up in memory in ascending
//! order. Loading words sign-extends them to the whole register, even data
//! registers. Loading also performs one surplus word read past the last
//! register: a famous 68000 quirk, harmless except to hardware registers
//! with read side effects.
//!
//! `MOVEP` transfers a register to every *other* byte of memory, so 8-bit
//! peripherals wired to one half of the 16-bit data bus can be addressed
//! with consecutive register numbers.
//!
//! `LINK`/`UNLK` build and tear down a stack frame for local variables:
//! `LINK A6,#-n` pushes A6, points A6 at it, and reserves n bytes.

use crate::Bus;
use crate::cpu::M68k;
use crate::decode::Instr;
use crate::ea::{Mode, Operand, Size};
use crate::exceptions::Exec;

impl M68k {
    pub(crate) fn op_move<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        let value = self.read_ea(bus, i.src, i.size)?;
        match i.dst.mode {
            Mode::DataReg => {
                self.set_logic_flags(value, i.size);
                self.set_d(i.dst.reg, i.size, value);
                self.prefetch(bus);
            }
            Mode::PreDec => {
                // No 2-cycle decrement penalty for MOVE: the decrement is
                // overlapped with the prefetch, which therefore comes first.
                self.fault_pc_bias = 0;
                let reg = i.dst.reg;
                if i.size == Size::Long {
                    // The queue is refilled before the writes, but the new
                    // opcode only moves into IR at the very end.
                    let next_opcode = self.irc;
                    self.pc = self.pc.wrapping_add(2);
                    self.irc = self.fetch(bus, self.pc);
                    self.set_move_flags(value, i);
                    // Written low word first, so a fault reports An - 2 (with
                    // An itself not yet updated).
                    let high_addr = self.a[reg as usize].wrapping_sub(4);
                    self.write_word(bus, high_addr.wrapping_add(2), value as u16)?;
                    self.write_word(bus, high_addr, (value >> 16) as u16)?;
                    self.a[reg as usize] = high_addr;
                    self.set_logic_flags(value, Size::Long);
                    self.ir = next_opcode;
                } else {
                    self.set_logic_flags(value, i.size);
                    self.prefetch(bus);
                    let addr = self.predecrement(reg, i.size);
                    self.write_sized(bus, addr, i.size, value)?;
                }
            }
            Mode::AbsLong if i.src.is_memory() => {
                // A quirk of MOVE <memory>,(xxx).L: the low address word is
                // used straight from IRC and only replaced *after* the write.
                let high = self.read_ext(bus);
                let addr = u32::from(high) << 16 | u32::from(self.irc);
                self.fault_pc_bias = 0;
                self.set_move_flags(value, i);
                self.write_sized(bus, addr, i.size, value)?;
                self.set_logic_flags(value, i.size);
                self.read_ext(bus);
                self.prefetch(bus);
            }
            _ => {
                let dst = self.resolve(bus, i.dst, i.size);
                // (The microcode's PC runs ahead differently for a MOVE
                // destination than for a source; see `fault_pc_bias`.)
                self.fault_pc_bias = if let Operand::Mem(_) | Operand::PostInc(..) = dst {
                    if matches!(i.dst.mode, Mode::Indirect | Mode::PostInc) {
                        2
                    } else {
                        0
                    }
                } else {
                    0
                };
                self.set_move_flags(value, i);
                self.write_operand(bus, dst, i.size, value)?;
                self.set_logic_flags(value, i.size);
                if let Operand::PostInc(reg, _) = dst {
                    self.post_increment(reg, i.size);
                }
                self.prefetch(bus);
            }
        }
        Ok(())
    }

    /// Flags as they stand when MOVE starts writing its result, which is
    /// only visible if that write faults. Bytes and words are tested before
    /// the write. For longs it depends on how far the microcode has got,
    /// which varies with both operands (the full flags are set once the
    /// write succeeds):
    ///
    /// * from memory: only the low word has been tested for `(An)`, `(An)+`
    ///   and `(xxx).L` destinations, the whole long otherwise;
    /// * from a register or immediate: nothing yet for `(An)` and `(An)+`,
    ///   N and Z only for `(d16,An)` and `(d8,An,Xn)`, everything otherwise.
    fn set_move_flags(&mut self, value: u32, i: Instr) {
        if i.size != Size::Long {
            self.set_logic_flags(value, i.size);
        } else if i.src.is_memory() {
            if matches!(i.dst.mode, Mode::Indirect | Mode::PostInc | Mode::AbsLong) {
                self.set_logic_flags(value & 0xFFFF, Size::Word);
            } else {
                self.set_logic_flags(value, Size::Long);
            }
        } else {
            match i.dst.mode {
                Mode::Indirect | Mode::PostInc => {}
                Mode::Disp | Mode::Index => self.set_nz(value, Size::Long),
                _ => self.set_logic_flags(value, Size::Long),
            }
        }
    }

    pub(crate) fn op_movea<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        let value = self.read_ea(bus, i.src, i.size)?;
        self.a[i.dst.reg as usize] = i.size.sign_extend(value);
        self.prefetch(bus);
        Ok(())
    }

    /// `MOVEQ #d8,Dn`: an 8-bit constant sign-extended to 32 bits.
    pub(crate) fn op_moveq<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        let value = i.src.reg as i8 as u32;
        self.d[i.dst.reg as usize] = value;
        self.set_logic_flags(value, Size::Long);
        self.prefetch(bus);
        Ok(())
    }

    /// Value of register `n` in MOVEM numbering (0–7 = D0–D7, 8–15 = A0–A7).
    #[inline]
    fn movem_register(&self, n: usize) -> u32 {
        if n < 8 { self.d[n] } else { self.a[n - 8] }
    }

    pub(crate) fn op_movem_to_memory<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        let mask = self.read_ext(bus);
        let size = i.size;
        self.fault_pc_bias = 2;
        if i.dst.mode == Mode::PreDec {
            let r = i.dst.reg as usize;
            let mut addr = self.a[r];
            // Reversed mask: bit 0 is A7, bit 15 is D0. Storing from A7 down
            // to D0 at decreasing addresses leaves them in ascending order.
            for bit in 0..16 {
                if mask & (1 << bit) != 0 {
                    let value = self.movem_register(15 - bit);
                    // Longs go out low word first (descending addresses).
                    if size == Size::Long {
                        addr = addr.wrapping_sub(2);
                        self.write_word(bus, addr, value as u16)?;
                        addr = addr.wrapping_sub(2);
                        self.write_word(bus, addr, (value >> 16) as u16)?;
                    } else {
                        addr = addr.wrapping_sub(2);
                        self.write_word(bus, addr, value as u16)?;
                    }
                }
            }
            self.a[r] = addr;
        } else {
            let mut addr = self.control_address(bus, i.dst);
            self.fault_pc_bias = 2;
            for bit in 0..16 {
                if mask & (1 << bit) != 0 {
                    let value = self.movem_register(bit);
                    self.write_sized(bus, addr, size, value)?;
                    addr = addr.wrapping_add(size.bytes());
                }
            }
        }
        self.prefetch(bus);
        Ok(())
    }

    pub(crate) fn op_movem_to_registers<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        let mask = self.read_ext(bus);
        let size = i.size;
        let post_increment = i.src.mode == Mode::PostInc;
        let mut addr = if post_increment {
            self.a[i.src.reg as usize]
        } else {
            self.control_address(bus, i.src)
        };
        // MOVEM's microcode PC runs one word ahead of the queue.
        self.fault_pc_bias = 2;
        for bit in 0..16 {
            if mask & (1 << bit) != 0 {
                let value = size.sign_extend(self.read_sized(bus, addr, size)?);
                if bit < 8 {
                    self.d[bit] = value;
                } else {
                    self.a[bit - 8] = value;
                }
                addr = addr.wrapping_add(size.bytes());
            }
        }
        // The surplus read.
        self.read_word(bus, addr)?;
        // With `(An)+`, An ends up past the last register loaded (so loading
        // An itself is pointless: the address wins).
        if post_increment {
            self.a[i.src.reg as usize] = addr;
        }
        self.prefetch(bus);
        Ok(())
    }

    /// `MOVEP Dx,(d16,Ay)` / `MOVEP (d16,Ay),Dx`: bytes at addr, addr+2, ...
    /// most significant first.
    pub(crate) fn op_movep<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        let bytes = i.size.bytes();
        if i.src.mode == Mode::DataReg {
            let addr = self.control_address(bus, i.dst);
            let value = self.d[i.src.reg as usize];
            for k in 0..bytes {
                let byte = (value >> (8 * (bytes - 1 - k))) as u8;
                self.write_byte(bus, addr.wrapping_add(2 * k), byte);
            }
        } else {
            let addr = self.control_address(bus, i.src);
            let mut value = 0u32;
            for k in 0..bytes {
                value = value << 8 | u32::from(self.read_byte(bus, addr.wrapping_add(2 * k)));
            }
            self.set_d(i.dst.reg, i.size, value);
        }
        self.prefetch(bus);
        Ok(())
    }

    /// `LEA <ea>,An`: load the address itself, not what it points to.
    pub(crate) fn op_lea<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        let addr = self.control_address(bus, i.src);
        self.a[i.dst.reg as usize] = addr;
        if matches!(i.src.mode, Mode::Index | Mode::PcIndex) {
            self.idle(2);
        }
        self.prefetch(bus);
        Ok(())
    }

    /// `PEA <ea>`: push an address.
    pub(crate) fn op_pea<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        let addr = self.control_address(bus, i.src);
        if matches!(i.src.mode, Mode::Index | Mode::PcIndex) {
            self.idle(2);
        }
        self.push_long(bus, addr)?;
        self.prefetch(bus);
        Ok(())
    }

    pub(crate) fn op_exg<B: Bus>(&mut self, bus: &mut B) -> Exec {
        let x = ((self.ird >> 9) & 7) as usize;
        let y = (self.ird & 7) as usize;
        match (self.ird >> 3) & 0x1F {
            0x08 => self.d.swap(x, y),
            0x09 => self.a.swap(x, y),
            _ => std::mem::swap(&mut self.d[x], &mut self.a[y]),
        }
        self.prefetch(bus);
        self.idle(2);
        Ok(())
    }

    /// `SWAP Dn`: exchange the two halves of a data register.
    pub(crate) fn op_swap<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        let r = i.dst.reg as usize;
        self.d[r] = self.d[r].rotate_left(16);
        self.set_logic_flags(self.d[r], Size::Long);
        self.prefetch(bus);
        Ok(())
    }

    pub(crate) fn op_link<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        let r = i.dst.reg as usize;
        let disp = self.read_ext(bus) as i16 as u32;
        let sp = self.a[7].wrapping_sub(4);
        let value = self.a[r];
        self.write_long(bus, sp, value)?;
        self.a[7] = sp;
        self.a[r] = sp;
        self.a[7] = self.a[7].wrapping_add(disp);
        self.prefetch(bus);
        Ok(())
    }

    pub(crate) fn op_unlk<B: Bus>(&mut self, bus: &mut B, i: Instr) -> Exec {
        let r = i.dst.reg as usize;
        let frame = self.a[r];
        self.fault_pc_bias = 2;
        let value = self.read_long(bus, frame)?;
        self.a[7] = frame.wrapping_add(4);
        self.a[r] = value;
        self.prefetch(bus);
        Ok(())
    }
}
