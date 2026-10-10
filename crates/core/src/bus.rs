//! The memory maps: what each CPU sees at each address.
//!
//! # 68000 address space (24-bit)
//!
//! | range | contents |
//! |-------|----------|
//! | `000000-3FFFFF` | cartridge ROM (and SRAM, or EEPROM latches) |
//! | `A00000-A0FFFF` | Z80 address space (RAM, YM2612), when the Z80 bus is requested |
//! | `A10000-A1001F` | I/O: version register, controller ports |
//! | `A11100`        | Z80 bus request |
//! | `A11200`        | Z80 reset |
//! | `A130F1-A130FF` | cartridge registers (SRAM enable, mapper) |
//! | `C00000-C0001F` | VDP ports (data, control, HV counter) and the PSG |
//! | `E00000-FFFFFF` | 64 KiB work RAM, mirrored |
//!
//! # Z80 address space (16-bit)
//!
//! | range | contents |
//! |-------|----------|
//! | `0000-1FFF` | 8 KiB sound RAM (mirrored at `2000-3FFF`) |
//! | `4000-5FFF` | YM2612 |
//! | `6000`      | bank register: selects which 32 KiB of 68000 space appears at `8000` |
//! | `7F11`      | PSG |
//! | `8000-FFFF` | banked window into the 68000 address space |
//!
//! Everything that is not a CPU lives in [`Hardware`]. The CPUs are kept
//! outside it so that a CPU can borrow the hardware mutably while it runs.

use gase_sound::{Psg, Ym2612};
use gase_vdp::Vdp;

use crate::audio::AudioClock;
use crate::cartridge::Cartridge;
use crate::io::Io;

/// Every component except the two CPUs.
#[derive(Clone, Debug)]
pub struct Hardware {
    pub cart: Cartridge,
    /// 64 KiB 68000 work RAM.
    pub ram: Vec<u8>,
    /// 8 KiB Z80 sound RAM.
    pub zram: Vec<u8>,
    pub vdp: Vdp,
    pub ym: Ym2612,
    pub psg: Psg,
    pub io: Io,
    pub audio: AudioClock,

    /// The 68000 holds the Z80 bus (register `A11100`).
    pub z80_busreq: bool,
    /// The Z80 is held in reset (register `A11200`).
    pub z80_reset: bool,
    /// The Z80's 9-bit bank register: bits 15-23 of the 68000 address seen at
    /// Z80 `8000-FFFF`.
    pub z80_bank: u16,
    /// Pending Z80 interrupt request (from the VDP's vertical interrupt).
    pub z80_irq: bool,
    /// The 68000 must be told to reset external devices (RESET instruction).
    pub reset_requested: bool,

    /// Current master clock, as seen by the CPU making an access.
    pub now: u64,
    /// Master clock at which the current scanline started.
    pub line_start: u64,
    /// Extra 68000 cycles to charge for the last access (e.g. Z80 bus waits).
    pub m68k_wait: u32,
}

impl Hardware {
    pub(crate) fn line_cycle(&self) -> u32 {
        self.now
            .saturating_sub(self.line_start)
            .min(u64::from(gase_vdp::MASTER_CYCLES_PER_LINE - 1)) as u32
    }

    /// The Z80 runs when it is neither held in reset nor off the bus.
    pub(crate) fn z80_running(&self) -> bool {
        !self.z80_reset && !self.z80_busreq
    }

    // --- VDP ----------------------------------------------------------------

    fn vdp_read_word(&mut self, addr: u32) -> u16 {
        match addr & 0x1F {
            0x00..=0x03 => self.vdp.read_data(),
            0x04..=0x07 => self.vdp.read_status(self.line_cycle()),
            0x08..=0x0F => self.vdp.hv_counter(self.line_cycle()),
            _ => 0xFFFF,
        }
    }

    fn vdp_write_word(&mut self, addr: u32, value: u16) {
        match addr & 0x1F {
            0x00..=0x03 => self.vdp.write_data(value),
            0x04..=0x07 => self.vdp.write_control(value),
            0x10..=0x17 => self.psg_write(value as u8),
            _ => {}
        }
    }

    fn psg_write(&mut self, value: u8) {
        self.sync_audio();
        self.psg.write(value);
    }

    // --- YM2612 ---------------------------------------------------------------

    fn ym_write(&mut self, port: u8, value: u8) {
        // Bring the chip up to date first so the write lands at the right
        // moment. This matters for sampled sound played through the DAC.
        self.sync_audio();
        self.ym.write(port & 3, value);
    }

    fn ym_read(&mut self) -> u8 {
        self.sync_audio();
        self.ym.read_status()
    }

    /// Advance the sound chips to the current master clock.
    pub(crate) fn sync_audio(&mut self) {
        self.audio.run_until(self.now, &mut self.ym, &mut self.psg);
    }

    // --- Z80 address space (shared by both CPUs) --------------------------------

    fn z80_space_read(&mut self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x3FFF => self.zram[usize::from(addr & 0x1FFF)],
            0x4000..=0x5FFF => self.ym_read(),
            0x6000..=0x7EFF => 0xFF,
            0x7F00..=0x7F1F => {
                // The VDP as seen by the Z80 (mostly used for the HV counter).
                let word = self.vdp_read_word(u32::from(addr));
                if addr & 1 == 0 {
                    (word >> 8) as u8
                } else {
                    word as u8
                }
            }
            0x7F20..=0x7FFF => 0xFF,
            0x8000..=0xFFFF => {
                let addr68 = (u32::from(self.z80_bank) << 15) | u32::from(addr & 0x7FFF);
                if (0xA0_0000..0xA1_0000).contains(&addr68) {
                    // The Z80 reading its own bus through the 68000 bus
                    // locks up real hardware; return open bus instead.
                    0xFF
                } else {
                    self.read_byte_68k(addr68)
                }
            }
        }
    }

    fn z80_space_write(&mut self, addr: u16, value: u8) {
        match addr {
            0x0000..=0x3FFF => self.zram[usize::from(addr & 0x1FFF)] = value,
            0x4000..=0x5FFF => self.ym_write(addr as u8, value),
            // Bank register: a 1-bit serial shift register. Each write shifts
            // bit 0 of the value in from the top.
            0x6000..=0x60FF => self.z80_bank = (self.z80_bank >> 1) | (u16::from(value & 1) << 8),
            0x7F00..=0x7F1F => {
                if (0x10..=0x17).contains(&(addr & 0x1F)) {
                    self.psg_write(value);
                }
            }
            0x8000..=0xFFFF => {
                let addr68 = (u32::from(self.z80_bank) << 15) | u32::from(addr & 0x7FFF);
                if !(0xA0_0000..0xA1_0000).contains(&addr68) {
                    self.write_byte_68k(addr68, value);
                }
            }
            _ => {}
        }
    }

    // --- 68000 address space -----------------------------------------------------

    pub(crate) fn read_byte_68k(&mut self, addr: u32) -> u8 {
        let addr = addr & 0xFF_FFFF;
        match addr {
            0x00_0000..=0x3F_FFFF => self.cart.read_byte(addr),
            0xA0_0000..=0xA0_FFFF => {
                // Accessing the Z80 bus costs the 68000 extra cycles.
                self.m68k_wait += 3;
                self.z80_space_read(addr as u16)
            }
            0xA1_0000..=0xA1_001F => self.io.read(addr & 0x1F, self.now),
            0xA1_1100..=0xA1_1101 => self.busreq_status(addr),
            0xC0_0000..=0xDF_FFFF => {
                let word = self.vdp_read_word(addr);
                if addr & 1 == 0 {
                    (word >> 8) as u8
                } else {
                    word as u8
                }
            }
            0xE0_0000..=0xFF_FFFF => self.ram[(addr & 0xFFFF) as usize],
            _ => 0xFF,
        }
    }

    fn busreq_status(&self, addr: u32) -> u8 {
        // Bit 0 reads 0 once the 68000 owns the Z80 bus. The other bits are
        // open bus.
        if addr & 1 == 0 {
            u8::from(!(self.z80_busreq && !self.z80_reset))
        } else {
            0
        }
    }

    pub(crate) fn read_word_68k(&mut self, addr: u32) -> u16 {
        let addr = addr & 0xFF_FFFE;
        match addr {
            0x00_0000..=0x3F_FFFF => self.cart.read_word(addr),
            0xC0_0000..=0xDF_FFFF => self.vdp_read_word(addr),
            0xE0_0000..=0xFF_FFFF => {
                let i = (addr & 0xFFFF) as usize;
                u16::from_be_bytes([self.ram[i], self.ram[i + 1]])
            }
            // 8-bit devices: the byte appears on both halves of the bus.
            0xA0_0000..=0xA0_FFFF => {
                let byte = self.read_byte_68k(addr);
                u16::from_be_bytes([byte, byte])
            }
            _ => u16::from_be_bytes([self.read_byte_68k(addr), self.read_byte_68k(addr | 1)]),
        }
    }

    pub(crate) fn write_byte_68k(&mut self, addr: u32, value: u8) {
        let addr = addr & 0xFF_FFFF;
        match addr {
            0x00_0000..=0x3F_FFFF => self.cart.write_byte(addr, value),
            0xA0_0000..=0xA0_FFFF => {
                self.m68k_wait += 3;
                self.z80_space_write(addr as u16, value);
            }
            0xA1_0000..=0xA1_001F => self.io.write(addr & 0x1F, value, self.now),
            0xA1_1100 => self.z80_busreq = value & 1 != 0,
            0xA1_1200 => self.set_z80_reset(value & 1 == 0),
            0xA1_30F0..=0xA1_30FF => self.cart.write_register(addr, value),
            // Byte writes to the VDP put the byte on both halves of the bus.
            0xC0_0000..=0xDF_FFFF => {
                if (0x10..=0x17).contains(&(addr & 0x1F)) {
                    self.psg_write(value);
                } else {
                    self.vdp_write_word(addr, u16::from_be_bytes([value, value]));
                }
            }
            0xE0_0000..=0xFF_FFFF => self.ram[(addr & 0xFFFF) as usize] = value,
            _ => {}
        }
    }

    pub(crate) fn write_word_68k(&mut self, addr: u32, value: u16) {
        let addr = addr & 0xFF_FFFE;
        match addr {
            // Cartridge: a word write may change two EEPROM lines at once.
            0x00_0000..=0x3F_FFFF => self.cart.write_word(addr, value),
            0xC0_0000..=0xDF_FFFF => self.vdp_write_word(addr, value),
            0xE0_0000..=0xFF_FFFF => {
                let i = (addr & 0xFFFF) as usize;
                self.ram[i..i + 2].copy_from_slice(&value.to_be_bytes());
            }
            // Word writes to the Z80 area only transfer the high byte.
            0xA0_0000..=0xA0_FFFF => self.write_byte_68k(addr, (value >> 8) as u8),
            // The Z80 control registers look at the high byte of a word write.
            0xA1_1100 | 0xA1_1200 => self.write_byte_68k(addr, (value >> 8) as u8),
            _ => {
                let [high, low] = value.to_be_bytes();
                self.write_byte_68k(addr, high);
                self.write_byte_68k(addr | 1, low);
            }
        }
    }

    fn set_z80_reset(&mut self, asserted: bool) {
        if asserted && !self.z80_reset {
            // The reset line also resets the YM2612.
            self.sync_audio();
            self.ym.reset();
        }
        self.z80_reset = asserted;
    }
}

/// The 68000's view of the hardware.
impl gase_m68k::Bus for Hardware {
    #[inline]
    fn read_byte(&mut self, addr: u32) -> u8 {
        self.read_byte_68k(addr)
    }

    #[inline]
    fn read_word(&mut self, addr: u32) -> u16 {
        // Fast path for the overwhelmingly common cases: ROM and work RAM.
        if addr < 0x40_0000 {
            return self.cart.read_word(addr);
        }
        self.read_word_68k(addr)
    }

    #[inline]
    fn write_byte(&mut self, addr: u32, value: u8) {
        self.write_byte_68k(addr, value);
    }

    #[inline]
    fn write_word(&mut self, addr: u32, value: u16) {
        self.write_word_68k(addr, value);
    }

    fn interrupt_acknowledge(&mut self, level: u8) -> Option<u8> {
        self.vdp.acknowledge_interrupt(level);
        // The Mega Drive uses autovectors for every interrupt.
        None
    }

    fn reset_devices(&mut self) {
        self.reset_requested = true;
    }
}

/// The Z80's view of the hardware.
#[derive(Debug)]
pub struct Z80Bus<'a>(pub &'a mut Hardware);

impl gase_z80::Bus for Z80Bus<'_> {
    #[inline]
    fn read(&mut self, addr: u16) -> u8 {
        self.0.z80_space_read(addr)
    }

    #[inline]
    fn write(&mut self, addr: u16, value: u8) {
        self.0.z80_space_write(addr, value);
    }

    // The Z80's I/O ports are not connected on the Mega Drive.
    fn port_in(&mut self, _port: u16) -> u8 {
        0xFF
    }
}
