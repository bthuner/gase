//! Serial EEPROM saves seen from the 68000: board detection from the
//! header, the I²C protocol driven through the cartridge latches (both by a
//! hand-assembled program and by direct bus accesses), and save states.

use gase_core::bus::Hardware;
use gase_core::eeprom::{ChipType, Wiring, boards};
use gase_core::{Cartridge, Config, Genesis, SaveType};
use gase_m68k::Bus;

/// A ROM image with vectors, a header and room for code at `0x200`.
fn rom(size: usize, serial: &str, checksum: u16) -> Vec<u8> {
    let mut bytes = vec![0u8; size];
    bytes[0..4].copy_from_slice(&0x00FF_FE00u32.to_be_bytes()); // initial SSP
    bytes[4..8].copy_from_slice(&0x0000_0200u32.to_be_bytes()); // initial PC
    bytes[0x100..0x110].copy_from_slice(b"SEGA MEGA DRIVE ");
    bytes[0x180..0x18E].fill(b' ');
    bytes[0x180..0x180 + serial.len()].copy_from_slice(serial.as_bytes());
    bytes[0x18E..0x190].copy_from_slice(&checksum.to_be_bytes());
    bytes[0x1F0] = b'U';
    bytes[0x200..0x202].copy_from_slice(&0x60FEu16.to_be_bytes()); // bra.s *
    bytes
}

fn console(bytes: &[u8]) -> Genesis {
    Genesis::new(Cartridge::from_bytes(bytes).unwrap(), &Config::default())
}

/// Something that can drive SCL/SDA and sample SDA.
trait Lines {
    fn set(&mut self, scl: bool, sda: bool);
    fn sample(&mut self) -> bool;
}

// The I²C master side, exactly as a game bit-bangs it.

fn start(l: &mut impl Lines, sda: bool) {
    l.set(false, sda);
    l.set(false, true);
    l.set(true, true);
    l.set(true, false); // SDA falls while SCL is high
    l.set(false, false);
}

fn stop(l: &mut impl Lines) {
    l.set(false, false);
    l.set(true, false);
    l.set(true, true); // SDA rises while SCL is high
}

fn clock(l: &mut impl Lines, sda: bool) -> bool {
    l.set(false, sda);
    l.set(true, sda);
    let line = l.sample();
    l.set(false, sda);
    line
}

/// Send a byte, MSB first; true if acknowledged.
fn send(l: &mut impl Lines, byte: u8) -> bool {
    for i in (0..8).rev() {
        clock(l, byte >> i & 1 != 0);
    }
    !clock(l, true)
}

fn recv(l: &mut impl Lines, more: bool) -> u8 {
    let mut byte = 0;
    for _ in 0..8 {
        byte = (byte << 1) | u8::from(clock(l, true));
    }
    clock(l, !more);
    byte
}

/// Drives the latches with direct 68000 bus accesses.
struct BusMaster<'a> {
    hw: &'a mut Hardware,
    wiring: Wiring,
    scl: bool,
    sda: bool,
    /// Use one word write when SCL and SDA share a word.
    words: bool,
}

impl<'a> BusMaster<'a> {
    fn new(console: &'a mut Genesis, words: bool) -> Self {
        let wiring = console.cartridge().eeprom.as_ref().unwrap().wiring;
        Self {
            hw: &mut console.hw,
            wiring,
            scl: true,
            sda: true,
            words,
        }
    }

    /// The byte the latch at `addr` should receive for the current lines.
    fn latch_value(&self, addr: u32) -> u8 {
        let w = &self.wiring;
        let mut value = 0;
        if w.scl.addr == addr {
            value |= u8::from(self.scl) << w.scl.bit;
        }
        if w.sda_in.addr == addr {
            value |= u8::from(self.sda) << w.sda_in.bit;
        }
        value
    }
}

impl Lines for BusMaster<'_> {
    fn set(&mut self, scl: bool, sda: bool) {
        let (scl_changed, sda_changed) = (scl != self.scl, sda != self.sda);
        self.scl = scl;
        self.sda = sda;
        let w = self.wiring;
        if self.words && w.scl.addr & !1 == w.sda_in.addr & !1 {
            let base = w.scl.addr & !1;
            let value = u16::from_be_bytes([self.latch_value(base), self.latch_value(base | 1)]);
            self.hw.write_word(base, value);
            return;
        }
        if scl_changed {
            self.hw.write_byte(w.scl.addr, self.latch_value(w.scl.addr));
        }
        if sda_changed && !(scl_changed && w.sda_in.addr == w.scl.addr) {
            self.hw
                .write_byte(w.sda_in.addr, self.latch_value(w.sda_in.addr));
        }
    }

    fn sample(&mut self) -> bool {
        let pin = self.wiring.sda_out;
        let value = if self.words {
            let word = self.hw.read_word(pin.addr & !1);
            if pin.addr & 1 == 1 {
                word as u8
            } else {
                (word >> 8) as u8
            }
        } else {
            self.hw.read_byte(pin.addr)
        };
        value >> pin.bit & 1 != 0
    }
}

/// Byte write, page write and random/sequential read through the bus.
fn exercise(console: &mut Genesis, words: bool) {
    let chip = console.cartridge().eeprom.as_ref().unwrap().chip.chip();
    let mut m = BusMaster::new(console, words);
    let send_address = |m: &mut BusMaster<'_>, addr: u16| match chip {
        ChipType::X24C01 => unreachable!(),
        ChipType::C24C64 | ChipType::C24C65 => {
            assert!(send(m, 0xA0));
            assert!(send(m, (addr >> 8) as u8));
            assert!(send(m, addr as u8));
        }
        _ => {
            assert!(send(m, 0xA0 | ((addr >> 8) as u8 & 7) << 1));
            assert!(send(m, addr as u8));
        }
    };
    let addr = (chip.size() - 3) as u16;
    if chip == ChipType::X24C01 {
        start(&mut m, true);
        assert!(send(&mut m, (addr as u8) << 1));
    } else {
        start(&mut m, true);
        send_address(&mut m, addr);
    }
    for byte in [0x11, 0x22] {
        assert!(send(&mut m, byte));
    }
    stop(&mut m);

    start(&mut m, true);
    if chip == ChipType::X24C01 {
        assert!(send(&mut m, (addr as u8) << 1 | 1));
    } else {
        send_address(&mut m, addr);
        start(&mut m, false);
        assert!(send(&mut m, 0xA1));
    }
    let got = [recv(&mut m, true), recv(&mut m, false)];
    stop(&mut m);
    assert_eq!(got, [0x11, 0x22], "{chip:?}");

    if chip != ChipType::X24C01 {
        start(&mut m, true);
        assert!(
            !send(&mut m, 0x50),
            "{chip:?}: wrong device address is NACKed"
        );
        stop(&mut m);
    }
    let cart = console.cartridge_mut();
    let data = cart.save_data().unwrap();
    assert_eq!(&data[usize::from(addr)..][..2], &[0x11, 0x22]);
    assert!(cart.take_save_dirty());
    assert!(!cart.take_save_dirty());
}

#[test]
fn acclaim_board_with_word_writes() {
    // NFL Quarterback Club: 24C02, SCL on the even byte, SDA on the odd one.
    let mut c = console(&rom(0x10_0000, "GM T-081276 -00", 0));
    assert_eq!(
        c.cartridge().save_type(),
        SaveType::Eeprom(ChipType::C24C02)
    );
    assert_eq!(
        c.cartridge().eeprom.as_ref().unwrap().wiring,
        boards::ACCLAIM_TYPE2
    );
    exercise(&mut c, true);
}

#[test]
fn acclaim_3mib_rom_still_reads_rom_around_the_latches() {
    // NBA Jam TE: 24C04 on a 3 MiB ROM that overlaps the latch addresses.
    let mut bytes = rom(0x30_0000, "GM T-81406 -00", 0);
    bytes[0x20_0000..0x20_0004].copy_from_slice(&[0xAB, 0xCD, 0xEF, 0x12]);
    let mut c = console(&bytes);
    assert_eq!(
        c.cartridge().save_type(),
        SaveType::Eeprom(ChipType::C24C04)
    );
    // 0x200001 is SDA (released: bit 0 set, other bits clear); 0x200002 is ROM.
    assert_eq!(c.hw.read_word(0x20_0002), 0xEF12);
    assert_eq!(c.hw.read_byte(0x20_0001), 0x01);
    assert_eq!(c.hw.read_byte(0x20_0000), 0xAB);
    exercise(&mut c, false);
}

#[test]
fn acclaim_type1_nba_jam() {
    let mut c = console(&rom(0x10_0000, "GM T-081326 -00", 0));
    assert_eq!(
        c.cartridge().eeprom.as_ref().unwrap().wiring,
        boards::ACCLAIM_TYPE1
    );
    exercise(&mut c, false);
}

#[test]
fn codemasters_board_reads_sda_elsewhere() {
    // Micro Machines Turbo Tournament 96: shared serial, told apart by checksum.
    let mut c = console(&rom(0x10_0000, "GM 00000000-00", 0x165E));
    assert_eq!(
        c.cartridge().save_type(),
        SaveType::Eeprom(ChipType::C24C65)
    );
    exercise(&mut c, false);
    // A different checksum is a different game, with no EEPROM.
    let other = console(&rom(0x10_0000, "GM 00000000-00", 0x1234));
    assert_eq!(other.cartridge().save_type(), SaveType::None);
}

#[test]
fn codemasters_24c08_block_bits() {
    let mut c = console(&rom(0x10_0000, "GM T-120096 -00", 0));
    assert_eq!(
        c.cartridge().save_type(),
        SaveType::Eeprom(ChipType::C24C08)
    );
    exercise(&mut c, false);
}

#[test]
fn ea_board_uses_bits_6_and_7() {
    let mut c = console(&rom(0x10_0000, "GM T-50396 -00", 0));
    assert_eq!(
        c.cartridge().save_type(),
        SaveType::Eeprom(ChipType::X24C01)
    );
    assert_eq!(c.hw.read_byte(0x20_0001), 0x80);
    exercise(&mut c, false);
}

/// `RA` header with type `0xE840`: an unknown EEPROM game.
fn ra_eeprom_rom() -> Vec<u8> {
    let mut bytes = rom(0x8000, "GM 99999999-00", 0);
    bytes[0x1B0..0x1BC].copy_from_slice(&[b'R', b'A', 0xE8, 0x40, 0, 0x20, 0, 1, 0, 0x20, 0, 1]);
    bytes
}

/// Emits a straight-line 68000 program that bit-bangs the lines, storing
/// every sampled SDA byte in consecutive work RAM bytes.
struct Program {
    bytes: Vec<u8>,
    pc: usize,
    wiring: Wiring,
    scl: bool,
    sda: bool,
    samples: u32,
}

impl Program {
    fn words(&mut self, words: &[u16]) {
        for w in words {
            self.bytes[self.pc..self.pc + 2].copy_from_slice(&w.to_be_bytes());
            self.pc += 2;
        }
    }
}

impl Lines for Program {
    fn set(&mut self, scl: bool, sda: bool) {
        if (scl, sda) == (self.scl, self.sda) {
            return;
        }
        self.scl = scl;
        self.sda = sda;
        // Sega wiring: both lines in one byte, so one write per change.
        assert_eq!(self.wiring.scl.addr, self.wiring.sda_in.addr);
        let addr = self.wiring.scl.addr;
        let value =
            u16::from(scl) << self.wiring.scl.bit | u16::from(sda) << self.wiring.sda_in.bit;
        // move.b #value, (addr).l
        self.words(&[0x13FC, value, (addr >> 16) as u16, addr as u16]);
    }

    fn sample(&mut self) -> bool {
        let src = self.wiring.sda_out.addr;
        let dst = 0xFF_0000 + self.samples;
        self.samples += 1;
        // move.b (src).l, (dst).l
        self.words(&[
            0x13F9,
            (src >> 16) as u16,
            src as u16,
            (dst >> 16) as u16,
            dst as u16,
        ]);
        true // the real value is only known once the program has run
    }
}

#[test]
fn header_declared_eeprom_driven_by_a_68000_program() {
    let bytes = ra_eeprom_rom();
    let cart = Cartridge::from_bytes(&bytes).unwrap();
    assert_eq!(cart.save_type(), SaveType::Eeprom(ChipType::X24C01));
    assert!(cart.sram.is_none(), "the RA header describes the EEPROM");
    let wiring = cart.eeprom.as_ref().unwrap().wiring;
    assert_eq!(wiring, boards::SEGA);

    let mut p = Program {
        bytes,
        pc: 0x200,
        wiring,
        scl: true,
        sda: true,
        samples: 0,
    };
    // Mode 1 page write of two bytes at 0x41, then read three from 0x40.
    start(&mut p, true);
    send(&mut p, 0x41 << 1);
    send(&mut p, 0xC5);
    send(&mut p, 0x3A);
    stop(&mut p);
    start(&mut p, true);
    send(&mut p, 0x40 << 1 | 1);
    for _ in 0..3 {
        recv(&mut p, true);
    }
    stop(&mut p);
    p.words(&[0x60FE]); // bra.s *
    let samples = p.samples as usize;

    let mut c = console(&p.bytes);
    c.cartridge_mut().save_data_mut().unwrap()[0x40] = 0x99;
    c.run_frame();
    let ram = &c.hw.ram[..samples];
    let bits: Vec<u8> = ram.iter().map(|b| b >> wiring.sda_out.bit & 1).collect();
    // Write: three bytes, each acknowledged on its 9th clock.
    for byte in 0..3 {
        assert_eq!(bits[byte * 9 + 8], 0, "write byte {byte} ACK");
    }
    let read = &bits[27..];
    assert_eq!(read[8], 0, "read address ACK");
    let byte = |n: usize| {
        read[9 + n * 9..][..8]
            .iter()
            .fold(0u8, |acc, &b| (acc << 1) | b)
    };
    assert_eq!([byte(0), byte(1), byte(2)], [0x99, 0xC5, 0x3A]);
    assert_eq!(
        &c.cartridge().save_data().unwrap()[0x40..0x43],
        &[0x99, 0xC5, 0x3A]
    );
}

#[test]
fn save_state_carries_the_eeprom_mid_transfer() {
    let bytes = rom(0x10_0000, "GM T-081276 -00", 0);
    let mut a = console(&bytes);
    {
        let mut m = BusMaster::new(&mut a, false);
        start(&mut m, true);
        assert!(send(&mut m, 0xA0));
        assert!(send(&mut m, 0x10));
        assert!(send(&mut m, 0x42));
        stop(&mut m);
        // Leave a random read half done: address set, read not started.
        start(&mut m, true);
        assert!(send(&mut m, 0xA0));
        assert!(send(&mut m, 0x10));
    }
    let state = a.save_state();

    let mut b = console(&bytes);
    b.load_state(&state).unwrap();
    assert_eq!(b.cartridge().save_data().unwrap()[0x10], 0x42);
    assert_eq!(b.save_state(), state);
    for console in [&mut a, &mut b] {
        let mut m = BusMaster::new(console, false);
        m.scl = false; // where the transfer above left the lines
        m.sda = true;
        start(&mut m, true);
        assert!(send(&mut m, 0xA1));
        assert_eq!(recv(&mut m, false), 0x42);
        stop(&mut m);
    }
    assert_eq!(a.save_state(), b.save_state());

    // A state from a console without that EEPROM content restores it.
    let mut fresh = console(&bytes);
    fresh.load_state(&state).unwrap();
    assert_eq!(fresh.cartridge().save_data().unwrap()[0x10], 0x42);
}

#[test]
fn sram_games_keep_working_through_the_generic_api() {
    let mut bytes = rom(0x10_0000, "GM 00001009-00", 0);
    bytes[0x1B0..0x1BC]
        .copy_from_slice(&[b'R', b'A', 0xF8, 0x20, 0, 0x20, 0, 1, 0, 0x20, 0x3F, 0xFF]);
    let mut c = console(&bytes);
    assert_eq!(c.cartridge().save_type(), SaveType::Sram(0x2000));
    assert!(c.cartridge().eeprom.is_none());
    c.hw.write_word(0x20_0000, 0x1234); // only the odd byte is connected
    assert_eq!(c.cartridge().save_data().unwrap()[0], 0x34);
    assert!(c.cartridge_mut().take_save_dirty());
    c.cartridge_mut().save_data_mut().unwrap()[1] = 0x56;
    assert_eq!(c.hw.read_byte(0x20_0003), 0x56);
}
