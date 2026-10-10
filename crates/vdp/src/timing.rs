//! Scanline timing, interrupts and the HV counter.
//!
//! A scanline lasts 3420 master clocks on both NTSC and PAL machines
//! (488.57 68000 cycles, 228 Z80 cycles). An NTSC frame has 262 lines, a PAL
//! frame 313. The first 224 (or 240 in V30 mode) are the active display; the
//! rest is vertical blanking, when games update VRAM freely.
//!
//! Two interrupts come from the VDP:
//!
//! * **Vertical interrupt** (68000 level 6) at the start of vertical
//!   blanking. It also pulses the Z80's interrupt line.
//! * **Horizontal interrupt** (level 4): a counter loaded from register 10 is
//!   decremented every active line; when it underflows the interrupt fires
//!   and the counter reloads. Used for raster effects (water lines, etc.).
//!
//! Line numbers used by this crate start at 0 for the first active line.

use crate::Vdp;

/// Master clocks per scanline.
pub const MASTER_CYCLES_PER_LINE: u32 = 3420;

/// Master clock at which horizontal blanking (and the horizontal interrupt)
/// begins, counted from the start of the line.
pub const HBLANK_START_CYCLE: u32 = 2650;

/// Master clock, within the first blanking line, at which the vertical
/// interrupt fires.
pub const VINT_CYCLE: u32 = 770;

/// The television standard the console is built for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum VideoStandard {
    /// 60 Hz, 262 lines (Japan, Americas).
    #[default]
    Ntsc,
    /// 50 Hz, 313 lines (Europe).
    Pal,
}

impl VideoStandard {
    /// Master clock frequency in Hz.
    #[must_use]
    pub fn master_clock(self) -> u32 {
        match self {
            VideoStandard::Ntsc => 53_693_175,
            VideoStandard::Pal => 53_203_424,
        }
    }

    /// Scanlines per frame.
    #[must_use]
    pub fn lines_per_frame(self) -> u16 {
        match self {
            VideoStandard::Ntsc => 262,
            VideoStandard::Pal => 313,
        }
    }

    /// Frames per second.
    #[must_use]
    pub fn frame_rate(self) -> f64 {
        f64::from(self.master_clock())
            / f64::from(MASTER_CYCLES_PER_LINE)
            / f64::from(self.lines_per_frame())
    }
}

impl Vdp {
    /// Scanlines per frame.
    #[must_use]
    pub fn total_lines(&self) -> u16 {
        self.standard.lines_per_frame()
    }

    /// Number of visible lines (224, or 240 in V30 mode).
    #[must_use]
    pub fn active_lines(&self) -> u16 {
        if self.v30() { 240 } else { 224 }
    }

    /// The line currently being processed.
    #[must_use]
    pub fn line(&self) -> u16 {
        self.line
    }

    /// Start a new scanline: update blanking flags and render it if visible.
    ///
    /// Any access slots left in the previous line are used first, with the
    /// previous line's slot pattern.
    pub fn begin_line(&mut self, line: u16) {
        self.next_line_slots();
        self.line = line;
        let active = self.active_lines();
        if line == 0 {
            self.in_vblank = false;
            if self.interlaced() {
                self.odd_frame = !self.odd_frame;
            } else {
                self.odd_frame = false;
            }
            let rows = usize::from(active);
            self.frame_height = if self.interlace_double() {
                rows * 2
            } else {
                rows
            };
        }
        if line == active {
            self.in_vblank = true;
        }
        if line < active {
            self.render_line(line);
        }
    }

    /// Horizontal blanking has started on the current line: clock the
    /// horizontal interrupt counter.
    pub fn hblank(&mut self) {
        // The counter runs on every active line plus the first blanking line;
        // during the rest of vertical blanking it is continuously reloaded.
        if self.line <= self.active_lines() {
            if self.hint_counter == 0 {
                self.hint_counter = self.regs[10];
                self.hint_pending = true;
            } else {
                self.hint_counter -= 1;
            }
        } else {
            self.hint_counter = self.regs[10];
        }
    }

    /// The vertical interrupt point has been reached (first blanking line).
    pub fn trigger_vint(&mut self) {
        self.vint_pending = true;
    }

    /// The interrupt level the VDP presents to the 68000 (0, 4 or 6).
    #[must_use]
    pub fn interrupt_level(&self) -> u8 {
        if self.vint_pending && self.regs[1] & 0x20 != 0 {
            6
        } else if self.hint_pending && self.regs[0] & 0x10 != 0 {
            4
        } else {
            0
        }
    }

    /// The 68000 acknowledged an interrupt of `level`.
    pub fn acknowledge_interrupt(&mut self, level: u8) {
        match level {
            6 => self.vint_pending = false,
            4 => self.hint_pending = false,
            _ => {}
        }
    }

    pub(crate) fn in_hblank(line_cycle: u32) -> bool {
        !(40..HBLANK_START_CYCLE).contains(&line_cycle)
    }

    /// The HV counter (`0xC00008`): vertical position in the high byte,
    /// horizontal position in the low byte.
    ///
    /// Neither counter counts linearly: both jump backwards during blanking
    /// so that they fit in 8 bits. For example the NTSC vertical counter runs
    /// `0x00..=0xEA`, then `0xE5..=0xFF`.
    #[must_use]
    pub fn hv_counter(&self, line_cycle: u32) -> u16 {
        if let Some(latched) = self.hv_latch {
            return latched;
        }
        let h = self.h_counter(line_cycle);
        let v = self.v_counter();
        (v << 8) | h
    }

    fn h_counter(&self, line_cycle: u32) -> u16 {
        // H40: the counter takes 211 distinct values per line:
        // 0x00..=0xB6 then 0xE4..=0xFF. H32: 342 pixels, 0x00..=0x93 then
        // 0xE9..=0xFF.
        let (steps, last, restart) = if self.h40() {
            (211, 0xB6, 0xE4)
        } else {
            (171, 0x93, 0xE9)
        };
        let step =
            (line_cycle.min(MASTER_CYCLES_PER_LINE - 1) * steps / MASTER_CYCLES_PER_LINE) as u16;
        if step <= last {
            step
        } else {
            step - last - 1 + restart
        }
    }

    fn v_counter(&self) -> u16 {
        let total = self.total_lines();
        // Last line value before the counter jumps back.
        let last = match (self.standard, self.v30()) {
            (VideoStandard::Ntsc, false) => 0xEA,
            (VideoStandard::Ntsc, true) => 0x1FF,
            (VideoStandard::Pal, false) => 0x102,
            (VideoStandard::Pal, true) => 0x10A,
        };
        let line = self.line;
        let v = if line <= last {
            line
        } else {
            (line + 0x200 - total) & 0x1FF
        };
        if self.interlaced() {
            // In interlace modes bit 0 is replaced by bit 8.
            ((v << 1) | (v >> 8)) & 0xFF
        } else {
            v & 0xFF
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ntsc_v_counter_jumps() {
        let mut vdp = Vdp::new(VideoStandard::Ntsc);
        vdp.line = 0xEA;
        assert_eq!(vdp.hv_counter(0) >> 8, 0xEA);
        vdp.line = 0xEB;
        assert_eq!(vdp.hv_counter(0) >> 8, 0xE5);
        vdp.line = 261;
        assert_eq!(vdp.hv_counter(0) >> 8, 0xFF);
    }

    #[test]
    fn h40_h_counter_jumps() {
        let mut vdp = Vdp::new(VideoStandard::Ntsc);
        vdp.regs[12] = 0x81;
        assert_eq!(vdp.hv_counter(0) & 0xFF, 0);
        assert_eq!(vdp.hv_counter(MASTER_CYCLES_PER_LINE - 1) & 0xFF, 0xFF);
    }

    #[test]
    fn horizontal_interrupt_every_n_lines() {
        let mut vdp = Vdp::new(VideoStandard::Ntsc);
        vdp.regs[0] = 0x10;
        vdp.regs[10] = 2;
        let mut fired = Vec::new();
        for line in 0..262 {
            vdp.begin_line(line);
            vdp.hblank();
            if vdp.interrupt_level() == 4 {
                fired.push(line);
                vdp.acknowledge_interrupt(4);
            }
        }
        // The counter starts at 0, so it fires on line 0 and then every 3rd line.
        assert_eq!(&fired[..4], &[0, 3, 6, 9]);
        assert_eq!(*fired.last().unwrap(), 222);
    }

    #[test]
    fn frame_rates() {
        assert!((VideoStandard::Ntsc.frame_rate() - 59.92).abs() < 0.01);
        assert!((VideoStandard::Pal.frame_rate() - 49.70).abs() < 0.01);
    }
}
