//! CRC-32, the checksum ZIP stores for every file.
//!
//! # What a CRC is
//!
//! A cyclic redundancy check treats the data as one enormous binary
//! polynomial and keeps the remainder of dividing it by a fixed
//! *generator* polynomial. Division over GF(2) has no carries: "subtract"
//! is XOR. The remainder changes completely when any bit of the input
//! changes, and some error patterns are guaranteed to be caught: every
//! burst of up to 32 flipped bits, every odd number of flipped bits.
//! It is not a cryptographic hash, just a very good typo detector.
//!
//! ZIP uses the same CRC-32 as Ethernet, gzip and PNG:
//!
//! * generator `0x04C11DB7` (x³² + x²⁶ + x²³ + … + x + 1),
//! * bits are processed least significant first ("reflected"), so the
//!   generator appears bit-reversed in the code: `0xEDB88320`,
//! * the register starts as `0xFFFFFFFF` and the result is inverted, so
//!   that leading and trailing zero bytes still change the checksum.
//!
//! # Three ways to compute it
//!
//! [`crc32_bitwise`] is the definition: shift one bit at a time and XOR in
//! the generator whenever a 1 falls off the end. Eight steps per byte.
//!
//! Those eight steps depend only on the low byte of the register, so they
//! can be done once for each of the 256 possible bytes and stored in a
//! table: `crc = TABLE[(crc ^ byte) & 0xFF] ^ (crc >> 8)`. That is the
//! classic table-driven CRC ([`crc32_bytewise`]), one lookup per byte.
//!
//! The table-driven loop is still slow-ish because each step needs the
//! result of the previous one. **Slicing-by-8** ([`crc32`], used for real
//! work) processes 8 bytes per step with 8 tables: table *k* gives the
//! effect of a byte followed by *k* zero bytes. Because CRCs are linear
//! (the CRC of `a XOR b` is the XOR of the CRCs), the eight contributions
//! can be computed independently and XORed together, which lets the CPU
//! do the eight lookups in parallel. All three functions give identical
//! results; the tests check this on random data.

/// The reflected CRC-32 generator polynomial.
pub const POLYNOMIAL: u32 = 0xEDB8_8320;

/// The definition: one bit at a time. Slow, but obviously correct.
#[must_use]
pub fn crc32_bitwise(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            // If the bit about to fall off is set, "subtract" the generator.
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ POLYNOMIAL
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// `TABLES[0][b]` is the effect of the eight bit steps on a register whose
/// low byte is `b`; `TABLES[k][b]` the effect of byte `b` followed by `k`
/// zero bytes. Built at compile time.
static TABLES: [[u32; 256]; 8] = build_tables();

const fn build_tables() -> [[u32; 256]; 8] {
    let mut tables = [[0u32; 256]; 8];
    let mut b = 0;
    while b < 256 {
        let mut crc = b as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ POLYNOMIAL
            } else {
                crc >> 1
            };
            bit += 1;
        }
        tables[0][b] = crc;
        b += 1;
    }
    // A byte followed by k zeros: push the previous table's value through
    // one more (zero) byte.
    let mut k = 1;
    while k < 8 {
        let mut b = 0;
        while b < 256 {
            let prev = tables[k - 1][b];
            tables[k][b] = tables[0][(prev & 0xFF) as usize] ^ (prev >> 8);
            b += 1;
        }
        k += 1;
    }
    tables
}

/// The classic table-driven CRC: one table lookup per byte.
#[must_use]
pub fn crc32_bytewise(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc = TABLES[0][((crc ^ u32::from(byte)) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

/// CRC-32 of `data` (slicing-by-8; same result as [`crc32_bitwise`]).
#[must_use]
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    let mut chunks = data.chunks_exact(8);
    for chunk in &mut chunks {
        // The first four bytes are mixed with the register, the other four
        // are new data; each byte is looked up in the table that accounts
        // for the number of bytes still following it in this chunk.
        let lo = crc ^ u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        let hi = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
        crc = TABLES[7][(lo & 0xFF) as usize]
            ^ TABLES[6][((lo >> 8) & 0xFF) as usize]
            ^ TABLES[5][((lo >> 16) & 0xFF) as usize]
            ^ TABLES[4][(lo >> 24) as usize]
            ^ TABLES[3][(hi & 0xFF) as usize]
            ^ TABLES[2][((hi >> 8) & 0xFF) as usize]
            ^ TABLES[1][((hi >> 16) & 0xFF) as usize]
            ^ TABLES[0][(hi >> 24) as usize];
    }
    for &byte in chunks.remainder() {
        crc = TABLES[0][((crc ^ u32::from(byte)) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_values() {
        // The standard check value for CRC-32/ISO-HDLC.
        assert_eq!(crc32_bitwise(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
        assert_eq!(
            crc32(b"The quick brown fox jumps over the lazy dog"),
            0x414F_A339
        );
    }

    #[test]
    fn fast_paths_match_the_definition() {
        let mut seed = 0x1234_5678u32;
        let mut data = Vec::new();
        for len in 0..600 {
            data.clear();
            for _ in 0..len {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                data.push((seed >> 24) as u8);
            }
            let expected = crc32_bitwise(&data);
            assert_eq!(crc32_bytewise(&data), expected, "bytewise, len {len}");
            assert_eq!(crc32(&data), expected, "slicing-by-8, len {len}");
        }
    }
}
