//! Which games have an EEPROM, which chip, and how it is wired.
//!
//! Nothing in a cartridge tells the console "I have an EEPROM on these
//! pins": the game simply knows its own board. An emulator therefore needs
//! a list of games, recognised by the serial number in the header (and the
//! header checksum where several games share a serial, as unlicensed
//! Codemasters releases do).
//!
//! # The board families
//!
//! Each publisher designed its own glue logic, so the same chip appears at
//! different addresses and data bits. All of them sit in the otherwise
//! unused space above 2 MiB, decoded only at one or two addresses:
//!
//! | wiring | SCL | SDA in (write) | SDA out (read) |
//! |--------|-----|----------------|----------------|
//! | Sega | `200001` bit 1 | `200001` bit 0 | `200001` bit 0 |
//! | Acclaim type 1 (NBA Jam) | `200000` bit 1 | `200000` bit 0 | `200000` bit 1 |
//! | Acclaim type 2 | `200000` bit 0 | `200001` bit 0 | `200001` bit 0 |
//! | Electronic Arts | `200001` bit 6 | `200001` bit 7 | `200001` bit 7 |
//! | Codemasters | `300000` bit 1 | `300000` bit 0 | `380001` bit 7 |
//!
//! Acclaim's later boards put SCL on the *even* byte and SDA on the *odd*
//! byte, so a single `move.w` can change both at once; their bigger (3 MiB)
//! ROMs also overlap `200000`, which is why only the exact latch addresses
//! are taken over and the rest of that area still reads as ROM.
//! Codemasters games read SDA from a different address than they write it.
//!
//! Games whose header declares backup RAM of type `0xE840` (`"RA"`,
//! `0xE8`, `0x40` at `0x1B0`), Sega's code for "serial EEPROM", but that
//! are not in the list get the most common configuration: an X24C01 on the
//! Sega wiring.
//!
//! The list was compiled from public emulator documentation, chiefly the
//! EEPROM database of Genesis Plus GX, and the chip types from cartridge
//! scans. Additions are welcome: one line per game.

use super::ChipType;

/// A single data bit at one 68000 byte address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pin {
    pub addr: u32,
    pub bit: u8,
}

/// How a board connects the EEPROM's pins to the 68000 bus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wiring {
    /// Name of the board family, for display.
    pub name: &'static str,
    /// Clock, written by the 68000.
    pub scl: Pin,
    /// Data from the 68000 to the EEPROM.
    pub sda_in: Pin,
    /// Data from the EEPROM to the 68000.
    pub sda_out: Pin,
}

const fn pin(addr: u32, bit: u8) -> Pin {
    Pin { addr, bit }
}

/// Sega's own boards (Wonder Boy in Monster World, The Wily Wars, ...).
pub const SEGA: Wiring = Wiring {
    name: "Sega",
    scl: pin(0x20_0001, 1),
    sda_in: pin(0x20_0001, 0),
    sda_out: pin(0x20_0001, 0),
};

/// Acclaim's first board (NBA Jam).
pub const ACCLAIM_TYPE1: Wiring = Wiring {
    name: "Acclaim type 1",
    scl: pin(0x20_0000, 1),
    sda_in: pin(0x20_0000, 0),
    sda_out: pin(0x20_0000, 1),
};

/// Acclaim's later boards (NFL Quarterback Club, NBA Jam TE, ...).
pub const ACCLAIM_TYPE2: Wiring = Wiring {
    name: "Acclaim type 2",
    scl: pin(0x20_0000, 0),
    sda_in: pin(0x20_0001, 0),
    sda_out: pin(0x20_0001, 0),
};

/// Electronic Arts (NHLPA Hockey '93, Rings of Power, ...).
pub const EA: Wiring = Wiring {
    name: "Electronic Arts",
    scl: pin(0x20_0001, 6),
    sda_in: pin(0x20_0001, 7),
    sda_out: pin(0x20_0001, 7),
};

/// Codemasters (Micro Machines 2, Brian Lara Cricket, ...).
pub const CODEMASTERS: Wiring = Wiring {
    name: "Codemasters",
    scl: pin(0x30_0000, 1),
    sda_in: pin(0x30_0000, 0),
    sda_out: pin(0x38_0001, 7),
};

/// One known EEPROM game.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Board {
    /// Title, for documentation and display.
    pub game: &'static str,
    /// Text that must appear in the header serial field (`0x180`).
    pub serial: &'static str,
    /// Header checksum (`0x18E`) that must match too, if any.
    pub checksum: Option<u16>,
    pub chip: ChipType,
    pub wiring: Wiring,
}

const fn board(
    game: &'static str,
    serial: &'static str,
    checksum: Option<u16>,
    chip: ChipType,
    wiring: Wiring,
) -> Board {
    Board {
        game,
        serial,
        checksum,
        chip,
        wiring,
    }
}

use ChipType::{C24C02, C24C04, C24C08, C24C16, C24C65, X24C01};

/// Every game known to save to EEPROM.
#[rustfmt::skip]
pub const BOARDS: &[Board] = &[
    // Sega boards.
    board("Wonder Boy in Monster World",          "G-4060",      None,         X24C01, SEGA),
    board("Mega Man: The Wily Wars",              "T-12046",     None,         X24C01, SEGA),
    board("Rockman Mega World (J) [alt]",         "T-12053",     Some(0xEA80), X24C01, SEGA),
    board("Evander Holyfield's Real Deal Boxing", "MK-1215",     None,         X24C01, SEGA),
    board("Greatest Heavyweights (U)",            "MK-1228",     None,         C24C02, SEGA),
    board("Greatest Heavyweights (J)",            "G-5538",      None,         C24C02, SEGA),
    board("Greatest Heavyweights (E)",            "PR-1993",     None,         C24C02, SEGA),
    board("Sports Talk Baseball",                 "00001211-00", None,         X24C01, SEGA),
    board("Honoo no Toukyuuji Dodge Danpei",      "00004076-00", None,         X24C01, SEGA),
    board("Ninja Burai Densetsu",                 "G-4524",      None,         X24C01, SEGA),
    board("Game Toshokan",                        "00054503-00", None,         X24C01, SEGA),
    // Acclaim boards.
    board("NBA Jam (U/E)",                        "T-081326",    None,         C24C02, ACCLAIM_TYPE1),
    board("NBA Jam (J)",                          "T-81033",     None,         C24C02, ACCLAIM_TYPE1),
    board("NFL Quarterback Club",                 "T-081276",    None,         C24C02, ACCLAIM_TYPE2),
    board("NBA Jam Tournament Edition",           "T-81406",     None,         C24C04, ACCLAIM_TYPE2),
    board("NFL Quarterback Club '96",             "T-081586",    None,         C24C16, ACCLAIM_TYPE2),
    board("College Slam",                         "T-81576",     None,         C24C65, ACCLAIM_TYPE2),
    board("Frank Thomas Big Hurt Baseball",       "T-81476",     None,         C24C65, ACCLAIM_TYPE2),
    // Electronic Arts boards (all X24C01).
    board("Rings of Power",                       "T-50176",     None,         X24C01, EA),
    board("NHLPA Hockey '93",                     "T-50396",     None,         X24C01, EA),
    board("John Madden Football '93",             "T-50446",     None,         X24C01, EA),
    board("John Madden Football '93 Champ. Ed.",  "T-50516",     None,         X24C01, EA),
    board("Bill Walsh College Football",          "T-50606",     None,         X24C01, EA),
    // Codemasters boards. Several share the placeholder serial
    // "00000000-00", so the checksum tells them apart.
    board("Brian Lara Cricket",                   "T-120106",    None,         C24C08, CODEMASTERS),
    board("Micro Machines 2: Turbo Tournament",   "T-120096",    None,         C24C08, CODEMASTERS),
    board("Micro Machines Military",              "00000000-00", Some(0x168B), C24C08, CODEMASTERS),
    board("Micro Machines Military [alt]",        "00000000-00", Some(0xCEE0), C24C08, CODEMASTERS),
    board("Brian Lara Cricket 96 / Shane Warne",  "T-120146",    None,         C24C16, CODEMASTERS),
    board("Micro Machines Turbo Tournament 96",   "00000000-00", Some(0x165E), C24C65, CODEMASTERS),
    board("Micro Machines Turbo Tournament 96 [alt]", "00000000-00", Some(0x2C41), C24C65, CODEMASTERS),
];

/// The configuration used for an unknown game whose header announces an
/// EEPROM.
pub const GENERIC: Board = board("unknown", "", None, X24C01, SEGA);

/// Find the board for a game from its header serial field and checksum.
#[must_use]
pub fn find(serial: &str, checksum: u16) -> Option<&'static Board> {
    BOARDS
        .iter()
        .find(|b| serial.contains(b.serial) && b.checksum.is_none_or(|c| c == checksum))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_by_serial_and_checksum() {
        let wbmw = find("GM G-4060 -00", 0x1234).unwrap();
        assert_eq!((wbmw.chip, wbmw.wiring), (X24C01, SEGA));
        let mm = find("GM 00000000-00", 0x165E).unwrap();
        assert_eq!(mm.chip, C24C65);
        assert!(find("GM 00000000-00", 0x0001).is_none());
        assert!(find("GM 00001009-00", 0).is_none());
    }

    #[test]
    fn latches_are_inside_cartridge_space_and_bits_valid() {
        for b in BOARDS {
            for p in [b.wiring.scl, b.wiring.sda_in, b.wiring.sda_out] {
                assert!(p.addr < 0x40_0000 && p.bit < 8, "{}", b.game);
            }
            // SCL and SDA-in on the same bit of the same byte would make
            // a START impossible.
            assert_ne!(b.wiring.scl, b.wiring.sda_in, "{}", b.game);
        }
    }
}
