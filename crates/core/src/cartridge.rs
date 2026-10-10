//! Cartridges: ROM loading, the header, battery-backed SRAM and mappers.
//!
//! # The header
//!
//! Every Mega Drive ROM carries a 256-byte header at `0x100`:
//!
//! | offset | contents |
//! |--------|----------|
//! | `0x100` | system type, `"SEGA MEGA DRIVE "` or `"SEGA GENESIS    "` |
//! | `0x120` | domestic (Japanese) title |
//! | `0x150` | overseas title |
//! | `0x180` | serial number, e.g. `"GM 00001009-00"` |
//! | `0x18E` | checksum |
//! | `0x1B0` | `"RA"` if the cartridge has extra RAM, followed by its type and range |
//! | `0x1F0` | region codes: `J`, `U`, `E` (or a hex digit bitmask in later games) |
//!
//! # Save RAM
//!
//! Battery-backed SRAM is usually 8-bit, wired to the odd (or even) bytes
//! of `0x200000-0x20FFFF`. Cartridges bigger than 2 MiB overlap that range
//! with ROM, so they switch between ROM and SRAM by writing `0xA130F1`.
//!
//! # Serial EEPROM
//!
//! A few dozen games save to a small I²C EEPROM instead, bit-banged
//! through one or two cartridge addresses; see [`crate::eeprom`]. To a
//! frontend both kinds are just "save data" ([`Cartridge::save_data`]),
//! stored in the same `.srm` file.
//!
//! # Mappers
//!
//! The 68000 sees at most 4 MiB of cartridge. Super Street Fighter II (5 MiB)
//! introduced the "Sega mapper": registers `0xA130F3..=0xA130FF` map any
//! 512 KiB page of ROM into each of the eight 512 KiB slots.

use std::fmt;
use std::sync::Arc;

use crate::eeprom::{self, ChipType, SerialEeprom};

/// Which consoles a cartridge says it supports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Regions {
    pub japan: bool,
    pub americas: bool,
    pub europe: bool,
}

/// Information parsed from the ROM header.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Header {
    pub system: String,
    pub domestic_title: String,
    pub overseas_title: String,
    pub serial: String,
    pub checksum: u16,
    pub regions: Regions,
}

/// Errors when loading a ROM image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadError {
    /// The file is too small to be a Mega Drive ROM.
    TooSmall(usize),
    /// The file is larger than any known cartridge.
    TooLarge(usize),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::TooSmall(n) => write!(f, "ROM is too small ({n} bytes)"),
            LoadError::TooLarge(n) => write!(f, "ROM is too large ({n} bytes)"),
        }
    }
}

impl std::error::Error for LoadError {}

/// Battery-backed save RAM.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sram {
    pub data: Vec<u8>,
    /// First 68000 address of the SRAM window.
    pub start: u32,
    /// Last 68000 address of the SRAM window.
    pub end: u32,
    /// Which bytes of each word are connected (8-bit SRAM).
    pub lanes: SramLanes,
    /// The contents changed since the last [`Cartridge::take_sram_dirty`].
    pub dirty: bool,
}

/// How an 8-bit SRAM chip is wired onto the 16-bit bus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SramLanes {
    Odd,
    Even,
    Both,
}

impl Sram {
    fn offset(&self, addr: u32) -> Option<usize> {
        if addr < self.start || addr > self.end {
            return None;
        }
        let rel = addr - self.start;
        let index = match self.lanes {
            SramLanes::Both => rel as usize,
            SramLanes::Odd if addr & 1 == 1 => (rel >> 1) as usize,
            SramLanes::Even if addr & 1 == 0 => (rel >> 1) as usize,
            _ => return None,
        };
        (index < self.data.len()).then_some(index)
    }
}

/// What kind of save memory a cartridge has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveType {
    /// No save memory.
    None,
    /// Battery-backed SRAM of this many bytes.
    Sram(usize),
    /// A serial EEPROM.
    Eeprom(ChipType),
}

impl fmt::Display for SaveType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn size(bytes: usize) -> String {
            if bytes >= 1024 && bytes % 1024 == 0 {
                format!("{} KiB", bytes / 1024)
            } else {
                format!("{bytes} bytes")
            }
        }
        match self {
            SaveType::None => f.write_str("no save memory"),
            SaveType::Sram(bytes) => write!(f, "battery save ({} SRAM)", size(*bytes)),
            SaveType::Eeprom(chip) => {
                write!(f, "EEPROM save ({} {})", size(chip.size()), chip.name())
            }
        }
    }
}

/// A loaded cartridge.
#[derive(Clone)]
pub struct Cartridge {
    /// ROM contents, padded to a multiple of 2 bytes. Shared, so cloning a
    /// console (for rewind snapshots) does not copy megabytes of ROM.
    rom: Arc<[u8]>,
    pub header: Header,
    pub sram: Option<Sram>,
    /// SRAM is mapped instead of ROM (register `0xA130F1` bit 0).
    pub sram_enabled: bool,
    /// The Sega mapper is present (ROMs above 4 MiB).
    has_mapper: bool,
    /// Current 512 KiB page for each of the eight slots.
    pub banks: [u8; 8],
    /// Serial EEPROM, for the games that have one (instead of SRAM).
    pub eeprom: Option<SerialEeprom>,
    /// Byte address the EEPROM's SDA is read from, or `u32::MAX`. Cached
    /// outside the `Option` so the hot ROM read path pays one compare.
    eeprom_read: u32,
}

impl fmt::Debug for Cartridge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Cartridge")
            .field("rom_len", &self.rom.len())
            .field("header", &self.header)
            .field(
                "sram",
                &self.sram.as_ref().map(|s| (s.start, s.end, s.data.len())),
            )
            .field(
                "eeprom",
                &self.eeprom.as_ref().map(|e| (e.chip.chip(), e.wiring.name)),
            )
            .finish_non_exhaustive()
    }
}

/// Read a header text field, trimming padding.
fn text(rom: &[u8], range: std::ops::Range<usize>) -> String {
    rom.get(range)
        .unwrap_or(&[])
        .iter()
        .map(|&b| {
            if b.is_ascii_graphic() || b == b' ' {
                b as char
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn be32(rom: &[u8], offset: usize) -> u32 {
    rom.get(offset..offset + 4)
        .map_or(0, |b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// Convert the ".smd" dump format to a plain binary.
///
/// SMD files start with a 512-byte header and store each 16 KiB block with
/// all odd bytes first, then all even bytes.
fn deinterleave_smd(data: &[u8]) -> Vec<u8> {
    let body = &data[512..];
    let mut out = Vec::with_capacity(body.len());
    for block in body.chunks(0x4000) {
        let half = block.len() / 2;
        for i in 0..half {
            out.push(block[half + i]);
            out.push(block[i]);
        }
    }
    out
}

fn looks_like_smd(data: &[u8]) -> bool {
    // The SMD header has 0xAA 0xBB at offset 8 and the file size is
    // 512 + a multiple of 16 KiB.
    data.len() > 512 && data.len() % 0x4000 == 512 && data[8] == 0xAA && data[9] == 0xBB
}

impl Cartridge {
    /// Load a ROM image (plain `.bin`/`.md`/`.gen`, or interleaved `.smd`).
    pub fn from_bytes(data: &[u8]) -> Result<Self, LoadError> {
        let mut rom = if looks_like_smd(data) {
            deinterleave_smd(data)
        } else {
            data.to_vec()
        };
        if rom.len() < 0x200 {
            return Err(LoadError::TooSmall(rom.len()));
        }
        if rom.len() > 0x100_0000 {
            return Err(LoadError::TooLarge(rom.len()));
        }
        if rom.len() % 2 == 1 {
            rom.push(0xFF);
        }

        let region_text = text(&rom, 0x1F0..0x1F3);
        let header = Header {
            system: text(&rom, 0x100..0x110),
            domestic_title: text(&rom, 0x120..0x150),
            overseas_title: text(&rom, 0x150..0x180),
            serial: text(&rom, 0x180..0x18E),
            checksum: u16::from_be_bytes([rom[0x18E], rom[0x18F]]),
            regions: parse_regions(&region_text),
        };

        // An EEPROM game's "RA" header (if any) describes the EEPROM, not SRAM.
        let eeprom = Self::detect_eeprom(&rom, &header);
        let sram = if eeprom.is_some() {
            None
        } else {
            Self::detect_sram(&rom)
        };
        let has_mapper = rom.len() > 0x40_0000 || header.system.starts_with("SEGA SSF");
        Ok(Self {
            eeprom_read: eeprom.as_ref().map_or(u32::MAX, |e| e.wiring.sda_out.addr),
            eeprom,
            // Cartridges smaller than 2 MiB with SRAM keep it permanently
            // mapped; bigger ones start with ROM visible.
            sram_enabled: sram.is_some() && rom.len() <= 0x20_0000,
            rom: rom.into(),
            header,
            sram,
            has_mapper,
            banks: [0, 1, 2, 3, 4, 5, 6, 7],
        })
    }

    fn detect_eeprom(rom: &[u8], header: &Header) -> Option<SerialEeprom> {
        let board = eeprom::boards::find(&header.serial, header.checksum).or_else(|| {
            // Backup RAM type 0xE840 is Sega's code for a serial EEPROM.
            (rom[0x1B0..0x1B4] == *b"RA\xE8\x40").then_some(&eeprom::boards::GENERIC)
        })?;
        Some(SerialEeprom::new(board.chip, board.wiring))
    }

    fn detect_sram(rom: &[u8]) -> Option<Sram> {
        if &rom[0x1B0..0x1B2] == b"RA" {
            let kind = rom[0x1B2];
            let mut start = be32(rom, 0x1B4) & 0xFF_FFFF;
            let mut end = be32(rom, 0x1B8) & 0xFF_FFFF;
            if end < start || end - start > 0x10_0000 {
                // Nonsense values in some headers: fall back to the standard.
                start = 0x20_0001;
                end = 0x20_FFFF;
            }
            let lanes = match kind & 0x18 {
                0x18 => SramLanes::Odd,
                0x10 => SramLanes::Even,
                _ if start & 1 == 1 => SramLanes::Odd,
                _ => SramLanes::Both,
            };
            let size = match lanes {
                SramLanes::Both => end - start + 1,
                _ => (end - start) / 2 + 1,
            } as usize;
            return Some(Sram {
                data: vec![0xFF; size],
                start,
                end,
                lanes,
                dirty: false,
            });
        }
        None
    }

    /// The raw ROM bytes.
    #[must_use]
    pub fn rom(&self) -> &[u8] {
        &self.rom
    }

    /// Checksum computed the way the boot code of many games does: the sum
    /// of all words after the header.
    #[must_use]
    pub fn computed_checksum(&self) -> u16 {
        self.rom[0x200..].chunks_exact(2).fold(0u16, |sum, w| {
            sum.wrapping_add(u16::from_be_bytes([w[0], w[1]]))
        })
    }

    /// Map a 68000 address in `0x000000..0x400000` to a ROM offset.
    #[inline]
    fn rom_offset(&self, addr: u32) -> usize {
        if self.has_mapper {
            let slot = (addr >> 19) as usize & 7;
            (usize::from(self.banks[slot]) << 19) | (addr as usize & 0x7_FFFF)
        } else {
            addr as usize
        }
    }

    #[inline]
    fn sram_offset(&self, addr: u32) -> Option<usize> {
        if self.sram_enabled {
            self.sram.as_ref()?.offset(addr)
        } else {
            None
        }
    }

    /// Read a byte from cartridge space.
    #[inline]
    #[must_use]
    pub fn read_byte(&self, addr: u32) -> u8 {
        if addr == self.eeprom_read {
            return self.eeprom.as_ref().map_or(0xFF, SerialEeprom::read_sda);
        }
        if let Some(i) = self.sram_offset(addr) {
            return self.sram.as_ref().map_or(0xFF, |s| s.data[i]);
        }
        // Reads past the end of small ROMs mirror (address lines not decoded).
        let offset = self.rom_offset(addr);
        self.rom
            .get(offset)
            .copied()
            .unwrap_or_else(|| self.rom[offset % self.rom.len()])
    }

    /// Read a word from cartridge space.
    #[inline]
    #[must_use]
    pub fn read_word(&self, addr: u32) -> u16 {
        if (self.sram_enabled && self.sram.is_some()) || addr & !1 == self.eeprom_read & !1 {
            return u16::from_be_bytes([self.read_byte(addr), self.read_byte(addr | 1)]);
        }
        let offset = self.rom_offset(addr);
        if offset + 1 < self.rom.len() {
            u16::from_be_bytes([self.rom[offset], self.rom[offset + 1]])
        } else {
            u16::from_be_bytes([self.read_byte(addr), self.read_byte(addr | 1)])
        }
    }

    /// Write a byte to cartridge space (only SRAM and the EEPROM latches
    /// are writable).
    pub fn write_byte(&mut self, addr: u32, value: u8) {
        if let Some(e) = &mut self.eeprom {
            e.write_byte(addr, value);
            return;
        }
        if let Some(i) = self.sram_offset(addr) {
            if let Some(sram) = self.sram.as_mut() {
                if sram.data[i] != value {
                    sram.data[i] = value;
                    sram.dirty = true;
                }
            }
        }
    }

    /// Write a word to cartridge space.
    pub fn write_word(&mut self, addr: u32, value: u16) {
        if let Some(e) = &mut self.eeprom {
            // Both halves reach the EEPROM latches at the same instant.
            e.write_word(addr & !1, value);
            return;
        }
        let [high, low] = value.to_be_bytes();
        self.write_byte(addr, high);
        self.write_byte(addr | 1, low);
    }

    /// Write to the cartridge control registers at `0xA130F1..=0xA130FF`.
    pub fn write_register(&mut self, addr: u32, value: u8) {
        match addr & 0xFF {
            0xF1 => {
                if self.sram.is_some() {
                    self.sram_enabled = value & 1 != 0;
                }
            }
            reg @ 0xF3..=0xFF if reg & 1 == 1 && self.has_mapper => {
                let slot = ((reg - 0xF1) / 2) as usize;
                // Slot 0 is fixed to page 0 on the real mapper.
                if slot > 0 {
                    let pages = (self.rom.len() >> 19).max(1);
                    self.banks[slot] = (usize::from(value & 0x3F) % pages) as u8;
                }
            }
            _ => {}
        }
    }

    /// Return true once after the SRAM was modified (for periodic saving).
    pub fn take_sram_dirty(&mut self) -> bool {
        self.sram
            .as_mut()
            .is_some_and(|s| std::mem::take(&mut s.dirty))
    }

    /// What kind of save memory the cartridge has.
    #[must_use]
    pub fn save_type(&self) -> SaveType {
        if let Some(e) = &self.eeprom {
            SaveType::Eeprom(e.chip.chip())
        } else if let Some(s) = &self.sram {
            SaveType::Sram(s.data.len())
        } else {
            SaveType::None
        }
    }

    /// The contents of the save memory (SRAM or EEPROM), as stored in the
    /// `.srm` file.
    #[must_use]
    pub fn save_data(&self) -> Option<&[u8]> {
        if let Some(e) = &self.eeprom {
            Some(e.chip.data())
        } else {
            self.sram.as_ref().map(|s| s.data.as_slice())
        }
    }

    /// Mutable save memory, to load a `.srm` file into.
    pub fn save_data_mut(&mut self) -> Option<&mut [u8]> {
        if let Some(e) = &mut self.eeprom {
            Some(e.chip.data_mut())
        } else {
            self.sram.as_mut().map(|s| s.data.as_mut_slice())
        }
    }

    /// Return true once after the save memory was modified (for periodic
    /// saving), whatever its kind.
    pub fn take_save_dirty(&mut self) -> bool {
        match &mut self.eeprom {
            Some(e) => e.chip.take_dirty(),
            None => self.take_sram_dirty(),
        }
    }
}

/// Parse the region field: either letters (`"JUE"`) or, in later games, a
/// single hex digit bitmask (bit 0 Japan, bit 2 Americas, bit 3 Europe).
fn parse_regions(field: &str) -> Regions {
    let mut regions = Regions::default();
    let letters = field
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>();
    if letters.len() == 1 && letters != "J" && letters != "U" && letters != "E" {
        if let Some(mask) = letters.chars().next().and_then(|c| c.to_digit(16)) {
            regions.japan = mask & 1 != 0;
            regions.americas = mask & 4 != 0;
            regions.europe = mask & 8 != 0;
            return regions;
        }
    }
    regions.japan = letters.contains('J');
    regions.americas = letters.contains('U');
    regions.europe = letters.contains('E');
    if !(regions.japan || regions.americas || regions.europe) {
        regions.americas = true;
    }
    regions
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rom_with_header(size: usize) -> Vec<u8> {
        let mut rom = vec![0u8; size];
        rom[0x100..0x110].copy_from_slice(b"SEGA MEGA DRIVE ");
        rom[0x150..0x15A].copy_from_slice(b"TEST  GAME");
        rom[0x1F0..0x1F3].copy_from_slice(b"JUE");
        rom
    }

    #[test]
    fn parses_header() {
        let cart = Cartridge::from_bytes(&rom_with_header(0x1000)).unwrap();
        assert_eq!(cart.header.system, "SEGA MEGA DRIVE");
        assert_eq!(cart.header.overseas_title, "TEST GAME");
        assert_eq!(
            cart.header.regions,
            Regions {
                japan: true,
                americas: true,
                europe: true
            }
        );
    }

    #[test]
    fn hex_region_mask() {
        assert_eq!(
            parse_regions("4"),
            Regions {
                japan: false,
                americas: true,
                europe: false
            }
        );
        assert_eq!(
            parse_regions("E"),
            Regions {
                japan: false,
                americas: false,
                europe: true
            }
        );
    }

    #[test]
    fn sram_on_odd_bytes() {
        let mut rom = rom_with_header(0x1000);
        rom[0x1B0..0x1BC]
            .copy_from_slice(&[b'R', b'A', 0xF8, 0x20, 0, 0x20, 0, 1, 0, 0x20, 0x3F, 0xFF]);
        let mut cart = Cartridge::from_bytes(&rom).unwrap();
        let sram = cart.sram.as_ref().unwrap();
        assert_eq!(
            (sram.start, sram.end, sram.lanes, sram.data.len()),
            (0x200001, 0x203FFF, SramLanes::Odd, 0x2000)
        );
        cart.write_byte(0x200001, 0x42);
        cart.write_byte(0x200000, 0x99); // even byte: not connected
        assert_eq!(cart.read_byte(0x200001), 0x42);
        assert!(cart.take_sram_dirty());
        assert!(!cart.take_sram_dirty());
    }

    #[test]
    fn smd_deinterleave() {
        let plain = rom_with_header(0x4000);
        let mut smd = vec![0u8; 512];
        smd[8] = 0xAA;
        smd[9] = 0xBB;
        let odd: Vec<u8> = plain.iter().skip(1).step_by(2).copied().collect();
        let even: Vec<u8> = plain.iter().step_by(2).copied().collect();
        smd.extend(odd);
        smd.extend(even);
        let cart = Cartridge::from_bytes(&smd).unwrap();
        assert_eq!(cart.rom(), &plain[..]);
    }

    #[test]
    fn sega_mapper_switches_pages() {
        let mut rom = rom_with_header(0x50_0000);
        rom[0x48_0000] = 0x77; // page 9
        let mut cart = Cartridge::from_bytes(&rom).unwrap();
        assert_eq!(cart.read_byte(0x08_0000), 0);
        cart.write_register(0xA130F3, 9); // slot 1 -> page 9
        assert_eq!(cart.read_byte(0x08_0000), 0x77);
    }
}
