//! The CPU interface: control port, data port and DMA.
//!
//! # Control port
//!
//! A word written to the control port is either:
//!
//! * a **register write** `100R RRRR DDDD DDDD` (register R gets value D), or
//! * one half of a two-word **command** that sets the access address and the
//!   "code" (what memory to access, read or write, and whether to DMA):
//!
//! ```text
//! first word:  CD1 CD0 A13 A12 A11 A10 A9 A8 A7 A6 A5 A4 A3 A2 A1 A0
//! second word:  0   0   0   0   0   0   0  0 CD5 CD4 CD3 CD2 0 0 A15 A14
//! ```
//!
//! | CD3-CD0 | access     |
//! |---------|------------|
//! | 0000    | VRAM read  |
//! | 0001    | VRAM write |
//! | 0011    | CRAM write |
//! | 0100    | VSRAM read |
//! | 0101    | VSRAM write|
//! | 1000    | CRAM read  |
//! | 1100    | VRAM 8-bit read (undocumented) |
//!
//! After each data port access the address advances by register 15.
//!
//! # DMA
//!
//! When CD5 is set (and DMA is enabled in register 1) the command starts a
//! DMA. Register 23's top bits choose the kind:
//!
//! * `0x` — **68000 to VDP**: copy words from 68000 memory. The 68000 is
//!   frozen while this happens; the VDP drives its bus.
//! * `10` — **VRAM fill**: the next data port write supplies a byte that is
//!   repeated through VRAM.
//! * `11` — **VRAM copy**: copy bytes within VRAM.
//!
//! Registers 19-20 hold the length in words (0 means 65536) and 21-23 the
//! source address.

use crate::Vdp;

impl Vdp {
    /// Write a word to the control port (`0xC00004`).
    pub fn write_control(&mut self, value: u16) {
        if self.write_pending {
            // Second half of a command.
            self.write_pending = false;
            self.address = (self.address & 0x3FFF) | ((value & 3) << 14);
            self.code = (self.code & 0x03) | ((value >> 2) & 0x3C) as u8;
            if self.code & 0x20 != 0 && self.dma_enabled() {
                self.start_dma();
            }
            return;
        }

        if value & 0xC000 == 0x8000 {
            self.write_register(((value >> 8) & 0x1F) as usize, value as u8);
        } else {
            // First half of a command. Its bits take effect immediately: some
            // games only ever write the first word.
            self.write_pending = true;
            self.address = (self.address & 0xC000) | (value & 0x3FFF);
            self.code = (self.code & 0x3C) | (value >> 14) as u8;
        }
    }

    fn write_register(&mut self, index: usize, value: u8) {
        if index >= self.regs.len() {
            return;
        }
        // While the HV counter latch is being enabled, capture its value.
        if index == 0 && value & 0x02 != 0 && self.regs[0] & 0x02 == 0 {
            self.hv_latch = Some(self.hv_counter(0));
        } else if index == 0 && value & 0x02 == 0 {
            self.hv_latch = None;
        }
        self.regs[index] = value;
    }

    /// Read the status register from the control port.
    ///
    /// `line_cycle` is the number of master clocks elapsed in the current
    /// line, used for the horizontal blanking flag.
    ///
    /// ```text
    /// bit 9: FIFO empty     bit 8: FIFO full    bit 7: vertical interrupt pending
    /// bit 6: sprite overflow bit 5: sprite collision bit 4: odd frame (interlace)
    /// bit 3: vertical blank  bit 2: horizontal blank  bit 1: DMA busy  bit 0: PAL
    /// ```
    pub fn read_status(&mut self, line_cycle: u32) -> u16 {
        // Reading the status register cancels a half-written command.
        self.write_pending = false;

        // The FIFO is not emulated: writes complete instantly, so it always
        // reads as empty. The unused top bits read back whatever was on the
        // bus; 0x3400 matches the most common value.
        let mut status = 0x3400 | 0x0200;
        if self.vint_pending {
            status |= 0x80;
        }
        if self.sprite_overflow {
            status |= 0x40;
        }
        if self.sprite_collision {
            status |= 0x20;
        }
        if self.odd_frame && self.interlaced() {
            status |= 0x10;
        }
        if self.in_vblank || !self.display_enabled() {
            status |= 0x08;
        }
        if Self::in_hblank(line_cycle) {
            status |= 0x04;
        }
        if self.dma_68k_pending {
            status |= 0x02;
        }
        if self.standard == crate::VideoStandard::Pal {
            status |= 0x01;
        }
        // Overflow and collision are cleared by reading them.
        self.sprite_overflow = false;
        self.sprite_collision = false;
        status
    }

    /// Write a word to the data port (`0xC00000`).
    pub fn write_data(&mut self, value: u16) {
        self.write_pending = false;
        self.write_memory(value);
        if self.dma_fill_pending {
            self.dma_fill_pending = false;
            self.dma_fill(value);
        }
    }

    /// Read a word from the data port.
    pub fn read_data(&mut self) -> u16 {
        self.write_pending = false;
        let addr = self.address;
        let value = match self.code & 0x0F {
            0x0 => self.vram_word(addr & !1),
            0x4 => {
                let index = usize::from(addr >> 1) % self.vsram.len();
                self.vsram[index] & 0x07FF
            }
            0x8 => self.cram[usize::from(addr >> 1) & 0x3F] & 0x0EEE,
            0xC => {
                // 8-bit VRAM read: the other byte comes from the read buffer.
                let byte = self.vram[usize::from(addr ^ 1)];
                (self.read_buffer & 0xFF00) | u16::from(byte)
            }
            _ => self.read_buffer,
        };
        self.read_buffer = value;
        self.address = self.address.wrapping_add(self.auto_increment());
        value
    }

    /// Perform one data-port write to whatever memory the code selects.
    fn write_memory(&mut self, value: u16) {
        let addr = self.address;
        match self.code & 0x0F {
            0x1 => {
                // A write to an odd address stores the bytes swapped.
                let value = if addr & 1 != 0 {
                    value.swap_bytes()
                } else {
                    value
                };
                self.write_vram_byte(addr & !1, (value >> 8) as u8);
                self.write_vram_byte(addr | 1, value as u8);
            }
            0x3 => {
                let index = usize::from(addr >> 1) & 0x3F;
                self.cram[index] = value & 0x0EEE;
                self.palette.update(index, self.cram[index]);
            }
            0x5 => {
                let index = usize::from(addr >> 1);
                if index < self.vsram.len() {
                    self.vsram[index] = value & 0x07FF;
                }
            }
            // Writes with a read code are ignored.
            _ => {}
        }
        self.address = self.address.wrapping_add(self.auto_increment());
    }

    pub(crate) fn vram_word(&self, addr: u16) -> u16 {
        let a = usize::from(addr);
        u16::from_be_bytes([self.vram[a], self.vram[(a + 1) & 0xFFFF]])
    }

    /// Every VRAM write goes through here so the sprite cache stays coherent.
    fn write_vram_byte(&mut self, addr: u16, value: u8) {
        self.vram[usize::from(addr)] = value;

        let base = self.sprite_table_base();
        let offset = addr.wrapping_sub(base);
        let table_len = if self.h40() { 80 * 8 } else { 64 * 8 };
        // Only the first 4 bytes of each 8-byte entry are cached.
        if offset < table_len && offset & 4 == 0 {
            let index = usize::from(offset >> 3) * 4 + usize::from(offset & 3);
            self.sat_cache[index] = value;
        }
    }

    pub(crate) fn sprite_table_base(&self) -> u16 {
        let mask = if self.h40() { 0x7E } else { 0x7F };
        u16::from(self.regs[5] & mask) << 9
    }

    fn dma_length(&self) -> u32 {
        match u32::from(self.regs[19]) | u32::from(self.regs[20]) << 8 {
            0 => 0x1_0000,
            n => n,
        }
    }

    fn start_dma(&mut self) {
        match self.regs[23] >> 6 {
            0 | 1 => self.dma_68k_pending = true,
            2 => self.dma_fill_pending = true,
            _ => self.dma_copy(),
        }
    }

    /// Is a 68000-to-VDP DMA waiting to be run by the bus?
    #[must_use]
    pub fn dma_68k_pending(&self) -> bool {
        self.dma_68k_pending
    }

    /// Run a pending 68000-to-VDP DMA, reading source words through `read`.
    ///
    /// Returns the number of 68000 cycles the CPU is frozen for. The transfer
    /// itself happens instantly; the stall approximates how many words the
    /// VDP can move per line (few during active display, many during blanking).
    pub fn run_dma_68k(&mut self, mut read: impl FnMut(u32) -> u16) -> u32 {
        if !self.dma_68k_pending {
            return 0;
        }
        self.dma_68k_pending = false;

        let length = self.dma_length();
        // The source is a word address; it wraps within a 128 KiB window
        // because the top register (23) is never incremented.
        let mut source = u32::from(self.regs[21]) | u32::from(self.regs[22]) << 8;
        let high = u32::from(self.regs[23] & 0x7F) << 17;
        for _ in 0..length {
            let word = read(high | (source << 1));
            self.write_memory(word);
            source = (source + 1) & 0xFFFF;
        }
        self.regs[21] = source as u8;
        self.regs[22] = (source >> 8) as u8;
        self.regs[19] = 0;
        self.regs[20] = 0;

        // Words transferred per scanline (Sega documentation): 488 68000
        // cycles per line divided by the slot count.
        let blanking = self.in_vblank || !self.display_enabled();
        let words_per_line = match (self.h40(), blanking) {
            (true, false) => 18,
            (false, false) => 16,
            (true, true) => 205,
            (false, true) => 167,
        };
        length * 488 / words_per_line
    }

    fn dma_fill(&mut self, value: u16) {
        let length = self.dma_length();
        let fill_byte = (value >> 8) as u8;
        for _ in 0..length {
            match self.code & 0x0F {
                // VRAM fill writes the high byte next to the current address.
                0x1 => self.write_vram_byte(self.address ^ 1, fill_byte),
                // CRAM and VSRAM fills write the whole word.
                0x3 | 0x5 => {
                    self.write_memory(value);
                    continue;
                }
                _ => {}
            }
            self.address = self.address.wrapping_add(self.auto_increment());
        }
        self.regs[19] = 0;
        self.regs[20] = 0;
    }

    fn dma_copy(&mut self) {
        let length = self.dma_length();
        let mut source = u16::from(self.regs[21]) | u16::from(self.regs[22]) << 8;
        for _ in 0..length {
            let byte = self.vram[usize::from(source)];
            self.write_vram_byte(self.address, byte);
            source = source.wrapping_add(1);
            self.address = self.address.wrapping_add(self.auto_increment());
        }
        self.regs[21] = source as u8;
        self.regs[22] = (source >> 8) as u8;
        self.regs[19] = 0;
        self.regs[20] = 0;
    }
}

#[cfg(test)]
mod tests {
    use crate::{Vdp, VideoStandard};

    fn command(vdp: &mut Vdp, code: u8, addr: u16) {
        let code = u16::from(code);
        vdp.write_control(((code & 3) << 14) | (addr & 0x3FFF));
        vdp.write_control(((code & 0x3C) << 2) | (addr >> 14));
    }

    fn vdp() -> Vdp {
        let mut vdp = Vdp::new(VideoStandard::Ntsc);
        vdp.write_control(0x8F02); // auto-increment 2
        vdp
    }

    #[test]
    fn register_write() {
        let mut vdp = vdp();
        vdp.write_control(0x8174);
        assert_eq!(vdp.regs[1], 0x74);
    }

    #[test]
    fn vram_write_and_read_back() {
        let mut vdp = vdp();
        command(&mut vdp, 0x01, 0x1234);
        vdp.write_data(0xABCD);
        vdp.write_data(0x1122);
        assert_eq!(vdp.vram_word(0x1234), 0xABCD);
        assert_eq!(vdp.vram_word(0x1236), 0x1122);

        command(&mut vdp, 0x00, 0x1234);
        assert_eq!(vdp.read_data(), 0xABCD);
        assert_eq!(vdp.read_data(), 0x1122);
    }

    #[test]
    fn odd_address_vram_write_swaps_bytes() {
        let mut vdp = vdp();
        command(&mut vdp, 0x01, 0x0101);
        vdp.write_data(0xABCD);
        assert_eq!(vdp.vram_word(0x0100), 0xCDAB);
    }

    #[test]
    fn cram_write_masks_unused_bits() {
        let mut vdp = vdp();
        command(&mut vdp, 0x03, 0x0002);
        vdp.write_data(0xFFFF);
        assert_eq!(vdp.cram[1], 0x0EEE);
    }

    #[test]
    fn dma_fill() {
        let mut vdp = vdp();
        vdp.write_control(0x8114); // DMA enable, mode 5
        vdp.write_control(0x8F01); // increment 1
        vdp.write_control(0x9304); // length 4
        vdp.write_control(0x9400);
        vdp.write_control(0x9780); // fill
        command(&mut vdp, 0x21, 0x2000);
        vdp.write_data(0x5500);
        // The initial write stores 0x55 0x00 at 0x2000 and moves to 0x2001;
        // the fill then writes 0x55 at address ^ 1: 0x2000, 0x2003, 0x2002,
        // 0x2005.
        assert_eq!(
            &vdp.vram[0x2000..0x2006],
            &[0x55, 0x00, 0x55, 0x55, 0x00, 0x55]
        );
    }

    #[test]
    fn dma_copy() {
        let mut vdp = vdp();
        vdp.vram[0x100..0x104].copy_from_slice(&[1, 2, 3, 4]);
        vdp.write_control(0x8114);
        vdp.write_control(0x8F01);
        vdp.write_control(0x9304);
        vdp.write_control(0x9400);
        vdp.write_control(0x9500); // source 0x0100
        vdp.write_control(0x9601);
        vdp.write_control(0x97C0); // copy
        command(&mut vdp, 0x20, 0x0200);
        assert_eq!(&vdp.vram[0x200..0x204], &[1, 2, 3, 4]);
    }

    #[test]
    fn dma_68k_to_cram() {
        let mut vdp = vdp();
        vdp.write_control(0x8114);
        vdp.write_control(0x9302); // 2 words
        vdp.write_control(0x9400);
        vdp.write_control(0x9500 | 0x80); // source word address 0x000080 -> byte 0x100
        vdp.write_control(0x9600);
        vdp.write_control(0x9700);
        command(&mut vdp, 0x23, 0x0000);
        assert!(vdp.dma_68k_pending());
        let mut reads = Vec::new();
        vdp.run_dma_68k(|addr| {
            reads.push(addr);
            0x0E0E
        });
        assert_eq!(reads, vec![0x100, 0x102]);
        assert_eq!(vdp.cram[0], 0x0E0E);
        assert_eq!(vdp.cram[1], 0x0E0E);
        assert!(!vdp.dma_68k_pending());
    }

    #[test]
    fn sprite_cache_follows_writes() {
        let mut vdp = vdp();
        vdp.write_control(0x8C81); // H40
        vdp.write_control(0x8578); // sprite table at 0xF000
        command(&mut vdp, 0x01, 0xF008);
        vdp.write_data(0x0123);
        vdp.write_data(0x0F02);
        vdp.write_data(0xFFFF); // attribute word: not cached
        assert_eq!(&vdp.sat_cache[4..8], &[0x01, 0x23, 0x0F, 0x02]);
        assert_eq!(vdp.sat_cache[8], 0);
    }
}
