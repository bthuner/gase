//! Serial EEPROM saves: a two-wire (I²C) memory chip bit-banged through
//! cartridge addresses.
//!
//! # Why EEPROM?
//!
//! Most Mega Drive games that remember progress use battery-backed SRAM
//! (see [`crate::cartridge`]). A few dozen games, mostly from 1992-1996,
//! used a small **serial EEPROM** instead: no battery to die, a cheap 8-pin
//! chip, but only 128 bytes to 8 KiB of storage, and a slow, awkward
//! interface. Wonder Boy in Monster World, Mega Man: The Wily Wars, the
//! EA sports games of 1992-93, Acclaim's NBA Jam series and the Codemasters
//! Micro Machines games are the best known.
//!
//! # The two wires
//!
//! An I²C EEPROM has just two signal pins:
//!
//! * **SCL**, the clock, always driven by the *master* (here: the 68000);
//! * **SDA**, the data line, driven by whoever is talking. It is
//!   *open-drain*: devices can only pull it low, a resistor pulls it high.
//!   So "sending a 1" really means "letting go of the line".
//!
//! The 68000 has no I²C controller, so the cartridge board connects the
//! two pins to a latch on the data bus. Writing a byte to a magic address
//! sets SCL and/or SDA from certain data bits, and reading a magic address
//! returns SDA on a data bit. The game wiggles the lines in software
//! ("bit-banging"), one bus access per edge. Which address and which bit
//! is board-specific: see [`boards`].
//!
//! # Bits, START and STOP
//!
//! The rule of the bus is: **SDA only changes while SCL is low**, and the
//! receiver samples SDA on the **rising edge** of SCL. Two violations of
//! that rule are used on purpose as framing signals:
//!
//! ```text
//!         START                 one bit           STOP
//! SCL  ‾‾‾‾‾‾‾‾‾\____      ____/‾‾‾‾\____      ____/‾‾‾‾‾‾‾‾
//! SDA  ‾‾‾‾\_________      ===X========X==      _________/‾‾‾
//!          SDA falls          stable while         SDA rises
//!          while SCL high     SCL is high          while SCL high
//! ```
//!
//! A START always (re)starts a transfer, even in the middle of one; a STOP
//! ends it and puts the chip on standby.
//!
//! Bytes travel most significant bit first. After every 8 bits the
//! *receiver* acknowledges during a 9th clock by pulling SDA low (**ACK**);
//! leaving SDA high is a **NACK**. When the EEPROM sends data, the 68000
//! ACKs each byte to ask for the next one and NACKs the last.
//!
//! # Addressing: "mode 1" and "mode 2"
//!
//! The oldest chip, the Xicor **X24C01** (128 bytes), predates the standard
//! I²C addressing scheme. Emulator authors call its protocol *mode 1*: the
//! first byte after START is the 7-bit word address followed by the R/W bit
//! (1 = read):
//!
//! ```text
//! mode 1 write:  START [A6..A0 0] ack [data] ack [data] ack ... STOP
//! mode 1 read:   START [A6..A0 1] ack [data] ACK [data] ACK ... [data] NACK STOP
//! ```
//!
//! Every later chip (24C01, 24C02, ...) speaks standard I²C, *mode 2*. The
//! first byte is a **device address**: the fixed type code `1010`, three
//! select bits and R/W. Then one or two **word address** bytes follow:
//!
//! ```text
//! byte write:    START [1010 sss 0] ack [word addr] ack [data] ack STOP
//! random read:   START [1010 sss 0] ack [word addr] ack
//!                START [1010 sss 1] ack [data] ACK [data] ... NACK STOP
//! current read:  START [1010 sss 1] ack [data] ... NACK STOP
//! ```
//!
//! The random read is a write that is abandoned after the address (the
//! repeated START), followed by a read from the address just set: the chip
//! keeps an internal **address counter** that every byte transferred
//! advances, which is also what makes sequential and current-address reads
//! work.
//!
//! The three `sss` bits mean different things on different chips:
//!
//! | chip | size | word address | page | `sss` |
//! |------|------|--------------|------|-------|
//! | X24C01 | 128 B | 7 bits in the first byte (mode 1) | 4 | — |
//! | 24C01 | 128 B | 1 byte | 8 | chip select |
//! | 24C02 | 256 B | 1 byte | 8 | chip select |
//! | 24C04 | 512 B | 1 byte | 16 | 2 chip select + address bit 8 |
//! | 24C08 | 1 KiB | 1 byte | 16 | 1 chip select + address bits 8-9 |
//! | 24C16 | 2 KiB | 1 byte | 16 | address bits 8-10 |
//! | 24C64 | 8 KiB | 2 bytes | 32 | chip select |
//! | 24C65 | 8 KiB | 2 bytes | 64 | chip select |
//!
//! *Chip select* bits are compared with the chip's A0-A2 pins, which
//! cartridges tie to ground: a device address that does not match gets no
//! ACK and the chip ignores the rest of the transfer. Small chips with only
//! one address byte reuse select bits as extra address bits instead.
//!
//! # Page writes
//!
//! Programming EEPROM cells is slow (milliseconds), so chips latch a whole
//! **page** of bytes and program them together. After each data byte the
//! address counter advances, but only its low bits: writing past the end of
//! a page **wraps around** to the start of the same page and overwrites it.
//! Reads have no such limit and roll over the whole array.
//!
//! # What is simplified
//!
//! * Bytes are stored as soon as they are acknowledged rather than when the
//!   STOP condition starts the programming cycle, and that cycle takes no
//!   time (real chips NACK everything for ~5-10 ms, which games handle by
//!   "ACK polling", which then simply succeeds at once).
//! * Reads return only what the EEPROM drives on SDA. Games always release
//!   SDA (write 1) before reading, so the wired-AND with the 68000's own
//!   output never matters, and some boards return SDA on a separate pin.
//! * Write protection pins are not modelled; no known cartridge uses them.
//!
//! The chip behaviour follows the public datasheets (Xicor X24C01,
//! Microchip/Atmel/ST 24Cxx); the per-game board list was cross-checked
//! against the publicly documented EEPROM database of the Genesis Plus GX
//! emulator. The implementation is our own.

use gase_savestate::{Error, Reader, State, Writer};

pub mod boards;

pub use boards::{Board, Pin, Wiring};

/// How the chip expects the first bytes of a transfer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Addressing {
    /// X24C01 only: the first byte is the 7-bit word address and R/W.
    Mode1,
    /// Standard I²C: a device address byte, then `word_bytes` (1 or 2) word
    /// address bytes. The low `block_bits` select bits of the device
    /// address are the top bits of the word address.
    Mode2 { word_bytes: u8, block_bits: u8 },
}

/// The EEPROM chips found in Mega Drive cartridges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChipType {
    /// Xicor X24C01, 128 bytes, "mode 1" addressing.
    X24C01,
    /// 24C01, 128 bytes.
    C24C01,
    /// 24C02, 256 bytes.
    C24C02,
    /// 24C04, 512 bytes.
    C24C04,
    /// 24C08, 1 KiB.
    C24C08,
    /// 24C16, 2 KiB.
    C24C16,
    /// 24C64, 8 KiB with 32-byte pages.
    C24C64,
    /// 24C65, 8 KiB with 64-byte pages.
    C24C65,
}

impl ChipType {
    /// Capacity in bytes.
    #[must_use]
    pub const fn size(self) -> usize {
        match self {
            ChipType::X24C01 | ChipType::C24C01 => 128,
            ChipType::C24C02 => 256,
            ChipType::C24C04 => 512,
            ChipType::C24C08 => 1024,
            ChipType::C24C16 => 2048,
            ChipType::C24C64 | ChipType::C24C65 => 8192,
        }
    }

    /// Bytes per page write.
    #[must_use]
    pub const fn page_size(self) -> usize {
        match self {
            ChipType::X24C01 => 4,
            ChipType::C24C01 | ChipType::C24C02 => 8,
            ChipType::C24C04 | ChipType::C24C08 | ChipType::C24C16 => 16,
            ChipType::C24C64 => 32,
            ChipType::C24C65 => 64,
        }
    }

    /// How transfers address the chip.
    #[must_use]
    pub const fn addressing(self) -> Addressing {
        match self {
            ChipType::X24C01 => Addressing::Mode1,
            ChipType::C24C01 | ChipType::C24C02 => Addressing::Mode2 {
                word_bytes: 1,
                block_bits: 0,
            },
            ChipType::C24C04 => Addressing::Mode2 {
                word_bytes: 1,
                block_bits: 1,
            },
            ChipType::C24C08 => Addressing::Mode2 {
                word_bytes: 1,
                block_bits: 2,
            },
            ChipType::C24C16 => Addressing::Mode2 {
                word_bytes: 1,
                block_bits: 3,
            },
            ChipType::C24C64 | ChipType::C24C65 => Addressing::Mode2 {
                word_bytes: 2,
                block_bits: 0,
            },
        }
    }

    /// The part number, e.g. `"24C02"`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            ChipType::X24C01 => "X24C01",
            ChipType::C24C01 => "24C01",
            ChipType::C24C02 => "24C02",
            ChipType::C24C04 => "24C04",
            ChipType::C24C08 => "24C08",
            ChipType::C24C16 => "24C16",
            ChipType::C24C64 => "24C64",
            ChipType::C24C65 => "24C65",
        }
    }
}

/// Where the chip is in a transfer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Idle (after STOP, a NACK, or power-on): only a START wakes it up.
    Standby,
    /// Receiving the mode 1 address/R/W byte.
    Mode1Address,
    /// Receiving the mode 2 device address byte.
    DeviceAddress,
    /// Receiving the high word address byte (two-byte chips).
    AddressHigh,
    /// Receiving the (low) word address byte.
    AddressLow,
    /// Receiving data bytes to store.
    Write,
    /// Sending data bytes.
    Read,
}

impl Phase {
    const ALL: [Phase; 7] = [
        Phase::Standby,
        Phase::Mode1Address,
        Phase::DeviceAddress,
        Phase::AddressHigh,
        Phase::AddressLow,
        Phase::Write,
        Phase::Read,
    ];
}

impl State for Phase {
    fn save(&self, w: &mut Writer) {
        (*self as u8).save(w);
    }
    fn load(&mut self, r: &mut Reader<'_>) -> Result<(), Error> {
        let mut index = 0u8;
        index.load(r)?;
        *self = *Phase::ALL
            .get(usize::from(index))
            .ok_or(Error::Invalid("EEPROM phase"))?;
        Ok(())
    }
}

/// An I²C serial EEPROM, seen from its two pins.
///
/// Drive it with [`Eeprom::set_lines`] and read SDA back with
/// [`Eeprom::sda_out`]. The clock count within a byte is tracked in `bit`:
/// rising edges 1-8 move data bits, rising edge 9 is the acknowledge slot,
/// and the falling edge after it moves on to the next byte.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Eeprom {
    chip: ChipType,
    data: Vec<u8>,
    dirty: bool,
    /// SCL as last driven by the 68000.
    scl: bool,
    /// SDA as last driven by the 68000 (true = released, reads high).
    sda: bool,
    /// SDA as driven by the EEPROM (true = released).
    out: bool,
    phase: Phase,
    /// Phase to enter once the current acknowledge slot ends.
    next: Phase,
    /// Rising SCL edges seen in the current byte (0-9).
    bit: u8,
    /// Byte being shifted in or out.
    shift: u8,
    /// Receiving: the chip acknowledges this byte. Sending: the 68000
    /// acknowledged it and wants another.
    ack: bool,
    /// The internal address counter.
    address: u16,
    /// Address bits collected before the last address byte arrives
    /// (block bits from the device address, or the high address byte).
    high: u16,
}

impl Eeprom {
    /// A blank (all `0xFF`, as erased cells read) chip, on standby.
    #[must_use]
    pub fn new(chip: ChipType) -> Self {
        Self {
            chip,
            data: vec![0xFF; chip.size()],
            dirty: false,
            scl: true,
            sda: true,
            out: true,
            phase: Phase::Standby,
            next: Phase::Standby,
            bit: 0,
            shift: 0,
            ack: false,
            address: 0,
            high: 0,
        }
    }

    /// Which chip this is.
    #[must_use]
    pub fn chip(&self) -> ChipType {
        self.chip
    }

    /// The memory contents.
    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Mutable memory contents (to load a save file).
    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }

    /// Return true once after the contents changed.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// The levels the 68000 currently drives on `(SCL, SDA)`.
    #[must_use]
    pub fn lines(&self) -> (bool, bool) {
        (self.scl, self.sda)
    }

    /// The level the EEPROM drives on SDA (true = released/high).
    #[inline]
    #[must_use]
    pub fn sda_out(&self) -> bool {
        self.out
    }

    /// The 68000 sets new levels on SCL and SDA.
    ///
    /// Both lines may change in one call (a word write that hits both
    /// latches); a rising clock then samples the new SDA.
    pub fn set_lines(&mut self, scl: bool, sda: bool) {
        let (old_scl, old_sda) = (self.scl, self.sda);
        self.scl = scl;
        self.sda = sda;
        match (old_scl, scl) {
            // SDA moving while the clock is high is a framing signal.
            (true, true) if old_sda && !sda => self.start(),
            (true, true) if !old_sda && sda => self.stop(),
            (false, true) => self.clock_rise(),
            (true, false) => self.clock_fall(),
            _ => {}
        }
    }

    fn start(&mut self) {
        // The address counter survives: that is what makes the "random
        // read" (address write, repeated START, read) work.
        self.phase = match self.chip.addressing() {
            Addressing::Mode1 => Phase::Mode1Address,
            Addressing::Mode2 { .. } => Phase::DeviceAddress,
        };
        self.bit = 0;
        self.shift = 0;
        self.out = true;
    }

    fn stop(&mut self) {
        self.phase = Phase::Standby;
        self.bit = 0;
        self.out = true;
    }

    /// Rising SCL: the receiving side samples SDA.
    fn clock_rise(&mut self) {
        match self.phase {
            Phase::Standby => {}
            Phase::Read => {
                self.bit += 1;
                if self.bit == 9 {
                    // The 68000 pulls SDA low to ask for another byte.
                    self.ack = !self.sda;
                }
            }
            _ => {
                self.bit += 1;
                if self.bit <= 8 {
                    self.shift = (self.shift << 1) | u8::from(self.sda);
                }
                if self.bit == 8 {
                    self.receive_byte();
                }
            }
        }
    }

    /// Falling SCL: the sending side may change SDA.
    fn clock_fall(&mut self) {
        match self.phase {
            Phase::Standby => {}
            Phase::Read => match self.bit {
                // Present the next bit, MSB first (bit 7 went out when the
                // byte was loaded).
                1..=7 => self.out = self.shift & (0x80 >> self.bit) != 0,
                // Let go of SDA so the 68000 can acknowledge.
                8 => self.out = true,
                9 => {
                    self.bit = 0;
                    if self.ack {
                        self.load_read_byte();
                    } else {
                        // NACK: the master is done; wait for STOP.
                        self.phase = Phase::Standby;
                    }
                }
                _ => {}
            },
            _ => match self.bit {
                // Acknowledge (pull low) during the 9th clock, or not.
                8 => self.out = !self.ack,
                9 => {
                    self.out = true;
                    self.bit = 0;
                    self.shift = 0;
                    self.phase = self.next;
                    if self.phase == Phase::Read {
                        self.load_read_byte();
                    }
                }
                _ => {}
            },
        }
    }

    /// Put the byte at the address counter on the wire and advance.
    fn load_read_byte(&mut self) {
        let size = self.data.len();
        self.shift = self.data[usize::from(self.address) % size];
        // Reads roll over the whole array, unlike page writes.
        self.address = ((usize::from(self.address) + 1) % size) as u16;
        self.out = self.shift & 0x80 != 0;
    }

    /// A complete byte arrived: decide whether to ACK it and what comes next.
    fn receive_byte(&mut self) {
        let byte = self.shift;
        let mask = (self.data.len() - 1) as u16;
        let (ack, next) = match self.phase {
            Phase::Mode1Address => {
                self.address = u16::from(byte >> 1) & mask;
                (
                    true,
                    if byte & 1 != 0 {
                        Phase::Read
                    } else {
                        Phase::Write
                    },
                )
            }
            Phase::DeviceAddress => self.device_address(byte),
            Phase::AddressHigh => {
                self.high = u16::from(byte);
                (true, Phase::AddressLow)
            }
            Phase::AddressLow => {
                self.address = ((self.high << 8) | u16::from(byte)) & mask;
                (true, Phase::Write)
            }
            Phase::Write => {
                let i = usize::from(self.address);
                if self.data[i] != byte {
                    self.data[i] = byte;
                    self.dirty = true;
                }
                // Only the low bits advance: page writes wrap in the page.
                let page = (self.chip.page_size() - 1) as u16;
                self.address = (self.address & !page) | (self.address.wrapping_add(1) & page);
                (true, Phase::Write)
            }
            Phase::Standby | Phase::Read => (false, Phase::Standby),
        };
        self.ack = ack;
        self.next = next;
    }

    fn device_address(&mut self, byte: u8) -> (bool, Phase) {
        let Addressing::Mode2 {
            word_bytes,
            block_bits,
        } = self.chip.addressing()
        else {
            return (false, Phase::Standby);
        };
        let select = (byte >> 1) & 7;
        // Select bits not used for addressing must match the A0-A2 pins,
        // which cartridges tie to ground.
        if byte >> 4 != 0b1010 || select >> block_bits != 0 {
            return (false, Phase::Standby);
        }
        if byte & 1 != 0 {
            // Current-address read: continue from the address counter.
            return (true, Phase::Read);
        }
        self.high = u16::from(select);
        let next = if word_bytes == 2 {
            Phase::AddressHigh
        } else {
            Phase::AddressLow
        };
        (true, next)
    }
}

impl State for Eeprom {
    fn save(&self, w: &mut Writer) {
        self.data.save(w);
        self.scl.save(w);
        self.sda.save(w);
        self.out.save(w);
        self.phase.save(w);
        self.next.save(w);
        self.bit.save(w);
        self.shift.save(w);
        self.ack.save(w);
        self.address.save(w);
        self.high.save(w);
    }

    fn load(&mut self, r: &mut Reader<'_>) -> Result<(), Error> {
        self.data.load(r)?;
        self.scl.load(r)?;
        self.sda.load(r)?;
        self.out.load(r)?;
        self.phase.load(r)?;
        self.next.load(r)?;
        self.bit.load(r)?;
        self.shift.load(r)?;
        self.ack.load(r)?;
        self.address.load(r)?;
        self.high.load(r)?;
        if self.bit > 9 {
            return Err(Error::Invalid("EEPROM bit counter"));
        }
        if usize::from(self.address) >= self.data.len() {
            return Err(Error::Invalid("EEPROM address"));
        }
        Ok(())
    }
}

/// An EEPROM together with the cartridge wiring that connects it to the
/// 68000 bus.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SerialEeprom {
    pub chip: Eeprom,
    pub wiring: Wiring,
}

impl SerialEeprom {
    /// A blank chip of type `chip` connected through `wiring`.
    #[must_use]
    pub fn new(chip: ChipType, wiring: Wiring) -> Self {
        Self {
            chip: Eeprom::new(chip),
            wiring,
        }
    }

    /// The byte read from the SDA output address (`wiring.sda_out.addr`):
    /// SDA on its data bit, other bits zero.
    #[inline]
    #[must_use]
    pub fn read_sda(&self) -> u8 {
        u8::from(self.chip.sda_out()) << self.wiring.sda_out.bit
    }

    /// Update `scl`/`sda` from a byte written at `addr`; true if `addr` is
    /// one of the latches.
    fn latch(&self, addr: u32, value: u8, scl: &mut bool, sda: &mut bool) -> bool {
        let w = &self.wiring;
        let mut hit = false;
        if addr == w.scl.addr {
            *scl = value >> w.scl.bit & 1 != 0;
            hit = true;
        }
        if addr == w.sda_in.addr {
            *sda = value >> w.sda_in.bit & 1 != 0;
            hit = true;
        }
        hit
    }

    /// A byte write to cartridge space.
    pub fn write_byte(&mut self, addr: u32, value: u8) {
        let (mut scl, mut sda) = self.chip.lines();
        if self.latch(addr, value, &mut scl, &mut sda) {
            self.chip.set_lines(scl, sda);
        }
    }

    /// A word write to cartridge space (`addr` even). Both bytes reach the
    /// latches at the same instant, so the chip sees a single change.
    pub fn write_word(&mut self, addr: u32, value: u16) {
        let [high, low] = value.to_be_bytes();
        let (mut scl, mut sda) = self.chip.lines();
        let hit_high = self.latch(addr, high, &mut scl, &mut sda);
        let hit_low = self.latch(addr | 1, low, &mut scl, &mut sda);
        if hit_high || hit_low {
            self.chip.set_lines(scl, sda);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test master that bit-bangs the protocol the way a game does.
    struct Master {
        rom: Eeprom,
    }

    impl Master {
        fn new(chip: ChipType) -> Self {
            Self {
                rom: Eeprom::new(chip),
            }
        }

        fn lines(&mut self, scl: bool, sda: bool) {
            self.rom.set_lines(scl, sda);
        }

        fn start(&mut self) {
            // From any state: release SDA with SCL low, raise SCL, then
            // pull SDA low while SCL is high.
            self.lines(false, self.rom.sda);
            self.lines(false, true);
            self.lines(true, true);
            self.lines(true, false);
            self.lines(false, false);
        }

        fn stop(&mut self) {
            self.lines(false, false);
            self.lines(true, false);
            self.lines(true, true);
        }

        /// Clock one bit out and return what SDA read while SCL was high.
        fn clock(&mut self, sda: bool) -> bool {
            self.lines(false, sda);
            self.lines(true, sda);
            let line = self.rom.sda_out() && sda;
            self.lines(false, sda);
            line
        }

        /// Send a byte; returns true if the EEPROM acknowledged it.
        fn send(&mut self, byte: u8) -> bool {
            for i in (0..8).rev() {
                self.clock(byte >> i & 1 != 0);
            }
            !self.clock(true)
        }

        /// Receive a byte, then ACK (`more`) or NACK it.
        fn recv(&mut self, more: bool) -> u8 {
            let mut byte = 0;
            for _ in 0..8 {
                byte = (byte << 1) | u8::from(self.clock(true));
            }
            self.clock(!more);
            byte
        }

        /// Mode 2 address phase (device address + word address bytes).
        fn address(&mut self, addr: u16) {
            let Addressing::Mode2 {
                word_bytes,
                block_bits,
            } = self.rom.chip.addressing()
            else {
                panic!("mode 2 only");
            };
            let block = if word_bytes == 1 {
                (addr >> 8) as u8 & ((1 << block_bits) - 1)
            } else {
                0
            };
            self.start();
            assert!(self.send(0xA0 | block << 1), "device address");
            if word_bytes == 2 {
                assert!(self.send((addr >> 8) as u8), "high address");
            }
            assert!(self.send(addr as u8), "low address");
        }

        fn write(&mut self, addr: u16, bytes: &[u8]) {
            if self.rom.chip == ChipType::X24C01 {
                self.start();
                assert!(self.send((addr as u8) << 1));
            } else {
                self.address(addr);
            }
            for &b in bytes {
                assert!(self.send(b), "data byte");
            }
            self.stop();
        }

        fn read(&mut self, addr: u16, n: usize) -> Vec<u8> {
            if self.rom.chip == ChipType::X24C01 {
                self.start();
                assert!(self.send((addr as u8) << 1 | 1));
            } else {
                self.address(addr);
                self.start(); // repeated START turns the write into a read
                assert!(self.send(0xA1));
            }
            let out = (0..n).map(|i| self.recv(i + 1 < n)).collect();
            self.stop();
            out
        }

        fn read_current(&mut self, n: usize) -> Vec<u8> {
            self.start();
            assert!(self.send(0xA1));
            let out = (0..n).map(|i| self.recv(i + 1 < n)).collect();
            self.stop();
            out
        }
    }

    const ALL: [ChipType; 8] = [
        ChipType::X24C01,
        ChipType::C24C01,
        ChipType::C24C02,
        ChipType::C24C04,
        ChipType::C24C08,
        ChipType::C24C16,
        ChipType::C24C64,
        ChipType::C24C65,
    ];

    #[test]
    fn byte_write_then_random_read() {
        for chip in ALL {
            let mut m = Master::new(chip);
            let top = (chip.size() - 1) as u16;
            m.write(5, &[0x5A]);
            m.write(top, &[0xC3]);
            assert_eq!(m.rom.data()[5], 0x5A, "{chip:?}");
            assert_eq!(m.rom.data()[usize::from(top)], 0xC3, "{chip:?}");
            assert!(m.rom.take_dirty());
            assert_eq!(m.read(5, 1), [0x5A], "{chip:?}");
            assert_eq!(m.read(top, 1), [0xC3], "{chip:?}");
        }
    }

    #[test]
    fn page_write_wraps_within_the_page() {
        for chip in ALL {
            let mut m = Master::new(chip);
            let page = chip.page_size();
            // Start two bytes before the end of the second page and write
            // four bytes: the last two wrap to the start of that page.
            let base = page * 2 - 2;
            m.write(base as u16, &[1, 2, 3, 4]);
            let d = m.rom.data();
            assert_eq!((d[base], d[base + 1]), (1, 2), "{chip:?}");
            assert_eq!((d[page], d[page + 1]), (3, 4), "{chip:?}");
            assert_eq!(d[page * 2], 0xFF, "{chip:?}: no spill into next page");
        }
    }

    #[test]
    fn sequential_read_rolls_over_the_array() {
        for chip in ALL {
            let mut m = Master::new(chip);
            let size = chip.size();
            for (i, byte) in m.rom.data_mut().iter_mut().enumerate() {
                *byte = i as u8 ^ 0x55;
            }
            let start = size - 2;
            let got = m.read(start as u16, 4);
            let want: Vec<u8> = [start, start + 1, 0, 1]
                .iter()
                .map(|&i| i as u8 ^ 0x55)
                .collect();
            assert_eq!(got, want, "{chip:?}");
        }
    }

    #[test]
    fn current_address_read_continues_after_last_access() {
        for chip in ALL.into_iter().filter(|&c| c != ChipType::X24C01) {
            let mut m = Master::new(chip);
            m.write(0x20, &[0xAA, 0xBB, 0xCC]);
            assert_eq!(m.read(0x20, 1), [0xAA]);
            assert_eq!(m.read_current(2), [0xBB, 0xCC], "{chip:?}");
            // After a write the counter points just past the last byte.
            m.write(0x40, &[0x11]);
            m.write(0x41, &[0x22]);
            m.write(0x40, &[0x33]);
            assert_eq!(m.read_current(1), [0x22], "{chip:?}");
        }
    }

    #[test]
    fn wrong_device_address_is_not_acknowledged() {
        let mut m = Master::new(ChipType::C24C02);
        m.write(3, &[0x77]);
        m.start();
        // Wrong type code.
        assert!(!m.send(0xB0));
        // The chip ignores everything until the next START.
        assert!(!m.send(0x03));
        assert!(!m.send(0x99));
        m.stop();
        // Chip select bits 1 (pins are grounded).
        m.start();
        assert!(!m.send(0xA2));
        m.stop();
        assert_eq!(m.rom.data()[3], 0x77);
        // 24C16: all three select bits are address bits, so any is accepted.
        let mut m = Master::new(ChipType::C24C16);
        m.start();
        assert!(m.send(0xAE));
        m.stop();
        // 24C64: none are.
        let mut m = Master::new(ChipType::C24C64);
        m.start();
        assert!(!m.send(0xA2));
        m.stop();
    }

    #[test]
    fn x24c01_mode1_uses_the_first_byte_as_address() {
        let mut m = Master::new(ChipType::X24C01);
        m.write(0x7F, &[0x12]);
        m.write(0x00, &[0x34, 0x56]);
        assert_eq!(&m.rom.data()[..2], &[0x34, 0x56]);
        assert_eq!(m.rom.data()[0x7F], 0x12);
        assert_eq!(m.read(0x7F, 3), [0x12, 0x34, 0x56]);
    }

    #[test]
    fn stop_mid_byte_aborts_without_writing() {
        let mut m = Master::new(ChipType::C24C02);
        m.address(0x10);
        for _ in 0..4 {
            m.clock(false);
        }
        m.stop();
        assert_eq!(m.rom.data()[0x10], 0xFF);
        assert!(!m.rom.take_dirty());
    }

    #[test]
    fn state_round_trip_mid_transfer() {
        let mut a = Master::new(ChipType::C24C08);
        a.write(0x123, &[9, 8, 7]);
        a.address(0x123);
        a.start();
        assert!(a.send(0xA1));
        // The dirty flag belongs to the frontend, not to the save state.
        assert!(a.rom.take_dirty());
        let mut w = Writer::new();
        a.rom.save(&mut w);
        let bytes = w.into_bytes();
        let mut b = Master::new(ChipType::C24C08);
        b.rom.load(&mut Reader::new(&bytes)).unwrap();
        assert_eq!(a.rom, b.rom);
        assert_eq!(b.recv(true), 9);
        assert_eq!(b.recv(false), 8);
    }

    #[test]
    fn word_write_changes_both_lines_at_once() {
        // Acclaim style: SCL on 0x200000 bit 0, SDA on 0x200001 bit 0.
        let mut e = SerialEeprom::new(ChipType::C24C02, boards::ACCLAIM_TYPE2);
        e.write_word(0x20_0000, 0x0101);
        assert_eq!(e.chip.lines(), (true, true));
        // SDA falls while SCL stays high: START.
        e.write_word(0x20_0000, 0x0100);
        assert_eq!(e.chip.phase, Phase::DeviceAddress);
        assert_eq!(e.read_sda(), 1);
    }
}
