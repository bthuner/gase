//! The flag register `F` and lookup tables used to compute it.
//!
//! ```text
//!   bit   7   6   5   4   3   2    1   0
//!       +---+---+---+---+---+----+---+---+
//!   F = | S | Z | Y | H | X | PV | N | C |
//!       +---+---+---+---+---+----+---+---+
//! ```
//!
//! Bits 5 and 3 are not documented by Zilog, but they are real latches whose
//! value is fully deterministic: for most instructions they are copies of
//! bits 5 and 3 of the result (that is why they are often called `F5`/`F3`).
//! Software that relies on them is rare, but test suites (ZEXALL, the
//! SingleStepTests vectors) check them, and getting them right is a good sign
//! that the rest of the ALU is right too.

/// Sign: bit 7 of the result.
pub const S: u8 = 0x80;
/// Zero: the result is zero.
pub const Z: u8 = 0x40;
/// Undocumented, usually a copy of bit 5 of the result (sometimes called F5 or YF).
pub const Y: u8 = 0x20;
/// Half carry: carry out of (or borrow into) bit 3. Only `DAA` consumes it.
pub const H: u8 = 0x10;
/// Undocumented, usually a copy of bit 3 of the result (sometimes called F3 or XF).
pub const X: u8 = 0x08;
/// Parity (logic ops: even number of set bits) or overflow (arithmetic: signed result
/// did not fit). The same bit serves both purposes depending on the instruction.
pub const PV: u8 = 0x04;
/// Subtract: the last arithmetic operation was a subtraction. Only `DAA` consumes it.
pub const N: u8 = 0x02;
/// Carry: carry out of (or borrow into) bit 7 (bit 15 for 16-bit operations).
pub const C: u8 = 0x01;

/// Both undocumented bits.
pub const XY: u8 = X | Y;

/// `S`, `Z`, `Y` and `X` for every possible 8-bit result.
///
/// These four flags depend on the result alone, so a 256-entry table replaces
/// a handful of shifts and compares in every ALU operation.
pub(crate) static SZXY: [u8; 256] = build_table(false);

/// [`SZXY`] plus the parity flag, for logic operations, rotates and `IN`.
pub(crate) static SZXYP: [u8; 256] = build_table(true);

const fn build_table(with_parity: bool) -> [u8; 256] {
    let mut table = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        let v = i as u8;
        let mut f = v & (S | XY);
        if v == 0 {
            f |= Z;
        }
        if with_parity && v.count_ones() % 2 == 0 {
            f |= PV;
        }
        table[i] = f;
        i += 1;
    }
    table
}

/// `PV` if `v` has even parity, else 0.
#[inline]
pub(crate) fn parity(v: u8) -> u8 {
    SZXYP[v as usize] & PV
}
