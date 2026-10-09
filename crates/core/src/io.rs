//! Controller ports and the version register (`0xA10000-0xA1001F`).
//!
//! Each controller port has a **data** register and a **control** register.
//! Bits set in the control register make the matching data bits outputs
//! driven by the console; the others are inputs read from the controller.
//! Games set bit 6 (the "TH" line) as an output and toggle it to read the
//! buttons in two halves, because a pad only has 6 data lines:
//!
//! ```text
//!            bit:  7  6  5  4  3  2  1  0
//! TH = 1 (high):   ?  1  C  B  R  L  D  U
//! TH = 0 (low):    ?  0  St A  0  0  D  U
//! ```
//!
//! Buttons are **active low** (0 = pressed).
//!
//! The 6-button pad adds a counter: after the third falling edge of TH it
//! answers with the extra buttons (X, Y, Z, Mode) instead. The counter
//! resets if TH is left alone for about 1.5 ms.

use gase_savestate::impl_state;

/// Buttons of a Mega Drive controller, as a bit set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Buttons(pub u16);

impl Buttons {
    pub const UP: Buttons = Buttons(1 << 0);
    pub const DOWN: Buttons = Buttons(1 << 1);
    pub const LEFT: Buttons = Buttons(1 << 2);
    pub const RIGHT: Buttons = Buttons(1 << 3);
    pub const B: Buttons = Buttons(1 << 4);
    pub const C: Buttons = Buttons(1 << 5);
    pub const A: Buttons = Buttons(1 << 6);
    pub const START: Buttons = Buttons(1 << 7);
    pub const Z: Buttons = Buttons(1 << 8);
    pub const Y: Buttons = Buttons(1 << 9);
    pub const X: Buttons = Buttons(1 << 10);
    pub const MODE: Buttons = Buttons(1 << 11);

    /// Is every button of `other` pressed?
    #[must_use]
    pub fn contains(self, other: Buttons) -> bool {
        self.0 & other.0 == other.0
    }

    /// Press or release the buttons of `other`.
    pub fn set(&mut self, other: Buttons, pressed: bool) {
        if pressed {
            self.0 |= other.0;
        } else {
            self.0 &= !other.0;
        }
    }

    /// Bits 0-5 of the active-low "TH high" answer (`CBRLDU`).
    fn th_high(self) -> u8 {
        !(self.0 & 0x3F) as u8 & 0x3F
    }
}

impl std::ops::BitOr for Buttons {
    type Output = Buttons;
    fn bitor(self, rhs: Buttons) -> Buttons {
        Buttons(self.0 | rhs.0)
    }
}

/// What is plugged into a controller port.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Device {
    /// Nothing: all inputs float high.
    None,
    /// The original 3-button pad.
    ThreeButton,
    /// The 6-button "Fighting Pad".
    #[default]
    SixButton,
}

/// Master clocks after which an idle 6-button pad resets its counter
/// (about 1.5 ms).
const SIX_BUTTON_TIMEOUT: u64 = 80_000;

/// One controller port.
#[derive(Clone, Debug, Default)]
pub struct Port {
    pub device: Device,
    pub buttons: Buttons,
    data: u8,
    control: u8,
    /// Falling edges of TH seen by the 6-button pad.
    th_count: u8,
    /// Master clock of the last TH change.
    last_th_change: u64,
}

impl_state!(Port {
    data,
    control,
    th_count,
    last_th_change
});

impl Port {
    fn th(&self) -> bool {
        // An input TH line is pulled high.
        self.control & 0x40 == 0 || self.data & 0x40 != 0
    }

    fn read(&mut self, now: u64) -> u8 {
        if now.saturating_sub(self.last_th_change) > SIX_BUTTON_TIMEOUT {
            self.th_count = 0;
        }
        let th = self.th();
        let b = self.buttons;
        let pins = match self.device {
            Device::None => 0x7F,
            Device::ThreeButton => Self::three_button(b, th),
            Device::SixButton => match (th, self.th_count) {
                (false, 3) => {
                    // Start, A, and all directions low: identifies a 6-button pad.
                    let mut v = 0x00;
                    if !b.contains(Buttons::A) {
                        v |= 0x10;
                    }
                    if !b.contains(Buttons::START) {
                        v |= 0x20;
                    }
                    v
                }
                (true, 3) => {
                    // C B Mode X Y Z
                    let mut v = 0x40 | (b.th_high() & 0x30);
                    for (bit, button) in [
                        (0, Buttons::Z),
                        (1, Buttons::Y),
                        (2, Buttons::X),
                        (3, Buttons::MODE),
                    ] {
                        if !b.contains(button) {
                            v |= 1 << bit;
                        }
                    }
                    v
                }
                (false, 4) => {
                    let mut v = 0x0F;
                    if !b.contains(Buttons::A) {
                        v |= 0x10;
                    }
                    if !b.contains(Buttons::START) {
                        v |= 0x20;
                    }
                    v
                }
                _ => Self::three_button(b, th),
            },
        };
        // Output bits read back what the console wrote; inputs come from the
        // device. Bit 7 is unconnected and reads as the written value.
        (self.data & (self.control | 0x80)) | (pins & !self.control & 0x7F)
    }

    fn three_button(b: Buttons, th: bool) -> u8 {
        if th {
            0x40 | b.th_high()
        } else {
            // St A 0 0 D U
            let mut v = b.th_high() & 0x03;
            if !b.contains(Buttons::A) {
                v |= 0x10;
            }
            if !b.contains(Buttons::START) {
                v |= 0x20;
            }
            v
        }
    }

    fn write_data(&mut self, value: u8, now: u64) {
        let old_th = self.th();
        self.data = value;
        self.th_changed(old_th, now);
    }

    fn write_control(&mut self, value: u8, now: u64) {
        let old_th = self.th();
        self.control = value;
        self.th_changed(old_th, now);
    }

    fn th_changed(&mut self, old_th: bool, now: u64) {
        let th = self.th();
        if th == old_th {
            return;
        }
        if now.saturating_sub(self.last_th_change) > SIX_BUTTON_TIMEOUT {
            self.th_count = 0;
        }
        self.last_th_change = now;
        if old_th && !th {
            self.th_count = (self.th_count + 1).min(4);
        }
    }
}

/// The console's region, as reported by the version register.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Region {
    /// Japanese Mega Drive (NTSC).
    Japan,
    /// Genesis (NTSC).
    #[default]
    Americas,
    /// European Mega Drive (PAL).
    Europe,
}

impl Region {
    /// Does this region use a PAL (50 Hz) VDP?
    #[must_use]
    pub fn is_pal(self) -> bool {
        self == Region::Europe
    }
}

/// The I/O chip: version register and controller ports.
#[derive(Clone, Debug, Default)]
pub struct Io {
    pub region: Region,
    pub ports: [Port; 2],
    /// The expansion port (nothing connected).
    ext: Port,
}

impl_state!(Io { ports, ext });

impl Io {
    #[must_use]
    pub fn new(region: Region) -> Self {
        let mut io = Self {
            region,
            ..Self::default()
        };
        io.ext.device = Device::None;
        io
    }

    /// Read a byte; `addr` is the offset in `0xA10000-0xA1001F`.
    pub fn read(&mut self, addr: u32, now: u64) -> u8 {
        match (addr >> 1) & 0xF {
            0 => {
                // Version register: bit 7 overseas, bit 6 PAL, bit 5 no
                // expansion unit (Mega CD) attached, bits 0-3 hardware
                // version (0 = no TMSS).
                let mut version = 0x20;
                if self.region != Region::Japan {
                    version |= 0x80;
                }
                if self.region.is_pal() {
                    version |= 0x40;
                }
                version
            }
            1 => self.ports[0].read(now),
            2 => self.ports[1].read(now),
            3 => self.ext.read(now),
            4 => self.ports[0].control,
            5 => self.ports[1].control,
            6 => self.ext.control,
            // Serial port registers: report "idle".
            _ => 0x00,
        }
    }

    /// Write a byte; `addr` is the offset in `0xA10000-0xA1001F`.
    pub fn write(&mut self, addr: u32, value: u8, now: u64) {
        match (addr >> 1) & 0xF {
            1 => self.ports[0].write_data(value, now),
            2 => self.ports[1].write_data(value, now),
            3 => self.ext.write_data(value, now),
            4 => self.ports[0].write_control(value, now),
            5 => self.ports[1].write_control(value, now),
            6 => self.ext.write_control(value, now),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn io_with(device: Device, buttons: Buttons) -> Io {
        let mut io = Io::new(Region::Americas);
        io.ports[0].device = device;
        io.ports[0].buttons = buttons;
        // Drive TH high first, then make it an output, as games do: this
        // avoids a spurious falling edge.
        io.write(0x03, 0x40, 0);
        io.write(0x09, 0x40, 0);
        io
    }

    #[test]
    fn three_button_read_both_halves() {
        let mut io = io_with(
            Device::ThreeButton,
            Buttons::UP | Buttons::C | Buttons::START,
        );
        io.write(0x03, 0x40, 0);
        // TH high: C B R L D U, active low -> C (bit 5) and U (bit 0) low.
        assert_eq!(io.read(0x03, 0) & 0x3F, 0b01_1110);
        io.write(0x03, 0x00, 0);
        // TH low: St A 0 0 D U -> Start (bit 5) and U low.
        assert_eq!(io.read(0x03, 0) & 0x3F, 0b01_0010);
    }

    #[test]
    fn six_button_sequence() {
        let mut io = io_with(Device::SixButton, Buttons::X | Buttons::MODE);
        let mut answers = Vec::new();
        for _ in 0..4 {
            io.write(0x03, 0x40, 0);
            answers.push(io.read(0x03, 0) & 0x3F);
            io.write(0x03, 0x00, 0);
            answers.push(io.read(0x03, 0) & 0x3F);
        }
        // Third TH low: directions all low (6-button identification).
        assert_eq!(answers[5] & 0x0F, 0x00);
        // Following TH high: Z Y X Mode in bits 0-3, X and Mode pressed.
        assert_eq!(answers[6] & 0x0F, 0b0011);
        // Fourth TH low: bits 0-3 high.
        assert_eq!(answers[7] & 0x0F, 0x0F);
    }

    #[test]
    fn six_button_counter_times_out() {
        let mut io = io_with(Device::SixButton, Buttons::default());
        for t in 0..3 {
            io.write(0x03, 0x40, t);
            io.write(0x03, 0x00, t);
        }
        let late = SIX_BUTTON_TIMEOUT * 2;
        assert_eq!(io.read(0x03, late) & 0x0C, 0x00); // TH low, normal answer bits 2,3 = 0
        io.write(0x03, 0x40, late);
        // Counter reset: TH high gives the normal CBRLDU answer.
        assert_eq!(io.read(0x03, late) & 0x3F, 0x3F);
    }

    #[test]
    fn version_register() {
        assert_eq!(Io::new(Region::Japan).read(0x01, 0), 0x20);
        assert_eq!(Io::new(Region::Americas).read(0x01, 0), 0xA0);
        assert_eq!(Io::new(Region::Europe).read(0x01, 0), 0xE0);
    }
}
