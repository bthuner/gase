//! Canonical Huffman codes: building them from code lengths and decoding.
//!
//! # Huffman coding
//!
//! A Huffman code gives frequent symbols short bit strings and rare
//! symbols long ones. No code is a prefix of another (a *prefix code*),
//! so a decoder reading bits one at a time knows exactly when a code ends.
//! Think of a binary tree: each bit picks the left or right branch and
//! the symbols sit on the leaves.
//!
//! # Canonical codes
//!
//! Sending the tree itself would be wasteful. DEFLATE only sends the
//! **length** of each symbol's code, and both sides derive the same codes
//! with a fixed rule (RFC 1951 section 3.2.2):
//!
//! 1. shorter codes come before longer codes (numerically, once padded),
//! 2. codes of the same length are given to symbols in increasing order,
//!    as consecutive binary numbers.
//!
//! For example, lengths `A=2, B=1, C=3, D=3` give:
//!
//! ```text
//! B = 0      (the only 1-bit code)
//! A = 10     (first 2-bit code: (0 + 1) << 1)
//! C = 110    (first 3-bit code: (10 + 1) << 1)
//! D = 111    (next 3-bit code)
//! ```
//!
//! So all a decoder needs is how many codes there are of each length
//! (`Huffman::counts`) and the symbols sorted by (length, symbol)
//! (`Huffman::symbols`).
//!
//! A set of lengths is only usable if it describes a real tree. Each code
//! of length *L* uses up 2^-L of the code space: lengths whose shares add
//! up to more than 1 are **over-subscribed** (two symbols would share a
//! code) and always rejected. Shares that add up to less than 1 leave
//! some bit strings meaning nothing (**incomplete**); zlib only accepts
//! that for the degenerate case of a single code of length 1, and so do we.
//!
//! # Decoding
//!
//! `Huffman::decode_reference` is the textbook decoder (it follows Mark
//! Adler's `puff.c`): read a bit, append it to the code, and check whether
//! the code is one of the codes of the current length. Since canonical
//! codes of one length are consecutive numbers, that check is a single
//! comparison against the first code of that length.
//!
//! `Huffman::decode` is the fast path. It peeks the next
//! `FAST_BITS` bits and looks them up in a table that, for each possible
//! value, gives the symbol whose code starts those bits and the code's
//! length. Codes are read MSB-first but bytes LSB-first, so the table is
//! indexed by the *bit-reversed* code, and a code shorter than
//! `FAST_BITS` fills every entry whose low bits match it. The rare codes
//! longer than `FAST_BITS` (Huffman makes long codes rare by design) fall
//! back to the reference decoder. The tests check the two agree.

use super::InflateError;
use super::bits::BitReader;

/// The longest code DEFLATE allows.
pub(crate) const MAX_BITS: usize = 15;

/// Bits looked up at once by the fast decoder. 10 bits cover almost all
/// literal/length codes in practice while keeping each table at 2 KiB.
pub(crate) const FAST_BITS: u32 = 10;

/// A table entry: `symbol << 4 | length`; length 0 means "not in the
/// table, use the reference decoder".
type Entry = u16;

/// A canonical Huffman code, ready for decoding.
#[derive(Clone)]
pub(crate) struct Huffman {
    /// `counts[len]`: number of codes of each length (`counts[0]` unused).
    counts: [u16; MAX_BITS + 1],
    /// Symbols ordered by code length, then by symbol value.
    symbols: Vec<u16>,
    /// Fast lookup table indexed by the next `FAST_BITS` input bits.
    table: Box<[Entry; 1 << FAST_BITS]>,
}

impl std::fmt::Debug for Huffman {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Huffman")
            .field("counts", &self.counts)
            .field("symbols", &self.symbols.len())
            .finish_non_exhaustive()
    }
}

/// Whether a set of lengths may leave part of the code space unused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Completeness {
    /// The code must be complete (the code-length code).
    Required,
    /// A single 1-bit code is fine too (literal/length and distance codes).
    AllowSingle,
}

impl Huffman {
    /// Build the code from each symbol's code length (0 = symbol unused).
    pub(crate) fn new(lengths: &[u8], completeness: Completeness) -> Result<Self, InflateError> {
        let mut counts = [0u16; MAX_BITS + 1];
        for &len in lengths {
            counts[len as usize] += 1;
        }
        counts[0] = 0;

        // Check that the lengths describe a prefix code. `left` is the
        // number of unused codes of the current length: one empty code of
        // length 0 to start with, each extra bit doubles what is left.
        let mut left: i32 = 1;
        for &count in &counts[1..] {
            left = left * 2 - i32::from(count);
            if left < 0 {
                return Err(InflateError::OversubscribedCode);
            }
        }
        let used: u16 = counts.iter().sum();
        if left > 0 {
            let single = used == 1 && counts[1] == 1;
            let empty = used == 0;
            if completeness == Completeness::Required || !(single || empty) {
                return Err(InflateError::IncompleteCode);
            }
        }

        // Sort symbols by length: `offsets[len]` is where codes of that
        // length start in `symbols` (a counting sort).
        let mut offsets = [0u16; MAX_BITS + 2];
        for len in 1..=MAX_BITS {
            offsets[len + 1] = offsets[len] + counts[len];
        }
        let mut symbols = vec![0u16; used as usize];
        for (symbol, &len) in lengths.iter().enumerate() {
            if len != 0 {
                symbols[offsets[len as usize] as usize] = symbol as u16;
                offsets[len as usize] += 1;
            }
        }

        let mut code = Self {
            counts,
            symbols,
            table: Box::new([0; 1 << FAST_BITS]),
        };
        code.build_table();
        Ok(code)
    }

    /// Fill the fast lookup table: walk the codes in canonical order,
    /// computing each code value as the rule above describes.
    fn build_table(&mut self) {
        let mut code: u32 = 0;
        let mut index = 0;
        for len in 1..=FAST_BITS {
            for _ in 0..self.counts[len as usize] {
                let symbol = self.symbols[index];
                index += 1;
                // The stream delivers the code's first (most significant)
                // bit first, i.e. in bit 0 of our peeked value.
                let reversed = code.reverse_bits() >> (32 - len);
                let entry = (symbol << 4) | len as u16;
                // Every FAST_BITS-bit value that starts with this code.
                let mut fill = reversed as usize;
                while fill < self.table.len() {
                    self.table[fill] = entry;
                    fill += 1 << len;
                }
                code += 1;
            }
            code <<= 1;
        }
    }

    /// The reference decoder: one bit at a time, as in RFC 1951.
    pub(crate) fn decode_reference(&self, input: &mut BitReader<'_>) -> Result<u16, InflateError> {
        // `code` is the bits read so far; `first` the first code of the
        // current length; `index` where that length's symbols start.
        let mut code: i32 = 0;
        let mut first: i32 = 0;
        let mut index: i32 = 0;
        for len in 1..=MAX_BITS {
            code |= input.bits(1)? as i32;
            let count = i32::from(self.counts[len]);
            if code - first < count {
                return Ok(self.symbols[(index + code - first) as usize]);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        // Only possible with an incomplete code: these bits mean nothing.
        Err(InflateError::InvalidCode)
    }

    /// The fast decoder: one table lookup for codes of up to `FAST_BITS`
    /// bits, the reference decoder for longer ones.
    #[inline]
    pub(crate) fn decode(&self, input: &mut BitReader<'_>) -> Result<u16, InflateError> {
        input.ensure_32();
        let entry = self.table[input.peek(FAST_BITS) as usize];
        let len = u32::from(entry & 0xF);
        if len == 0 {
            return self.decode_reference(input);
        }
        input.consume(len)?;
        Ok(entry >> 4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write codes MSB-first into an LSB-first bit stream.
    fn pack(codes: &[(u32, u32)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut bit = 0;
        for &(code, len) in codes {
            for i in (0..len).rev() {
                if bit % 8 == 0 {
                    out.push(0);
                }
                *out.last_mut().unwrap() |= (((code >> i) & 1) as u8) << (bit % 8);
                bit += 1;
            }
        }
        out
    }

    #[test]
    fn rfc_example() {
        // RFC 1951 3.2.2: lengths (3,3,3,3,3,2,4,4) for A..H give
        // F=00, A=010 .. E=110, G=1110, H=1111.
        let h = Huffman::new(&[3, 3, 3, 3, 3, 2, 4, 4], Completeness::Required).unwrap();
        let stream = pack(&[(0b00, 2), (0b010, 3), (0b110, 3), (0b1110, 4), (0b1111, 4)]);
        for fast in [false, true] {
            let mut r = BitReader::new(&stream);
            let mut got = Vec::new();
            for _ in 0..5 {
                got.push(if fast {
                    h.decode(&mut r).unwrap()
                } else {
                    h.decode_reference(&mut r).unwrap()
                });
            }
            assert_eq!(got, [5, 0, 4, 6, 7]);
        }
    }

    #[test]
    fn bad_lengths_are_rejected() {
        // Three 1-bit codes cannot exist.
        assert_eq!(
            Huffman::new(&[1, 1, 1], Completeness::AllowSingle).unwrap_err(),
            InflateError::OversubscribedCode
        );
        // 1 + 2 bits leaves "11" unused.
        assert_eq!(
            Huffman::new(&[1, 2], Completeness::AllowSingle).unwrap_err(),
            InflateError::IncompleteCode
        );
        // A lone 1-bit code is fine for distances, not for the code-length code.
        assert!(Huffman::new(&[0, 1], Completeness::AllowSingle).is_ok());
        assert!(Huffman::new(&[0, 1], Completeness::Required).is_err());
    }

    #[test]
    fn incomplete_code_rejects_unused_bits() {
        let h = Huffman::new(&[1], Completeness::AllowSingle).unwrap();
        let mut r = BitReader::new(&[0b10, 0, 0]);
        assert_eq!(h.decode(&mut r), Ok(0));
        assert_eq!(h.decode(&mut r), Err(InflateError::InvalidCode));
    }

    #[test]
    fn fast_decoder_matches_reference_on_long_codes() {
        // A skewed code with lengths up to 15 bits: symbol i has length
        // i + 1, the last two share the longest length.
        let lengths: Vec<u8> = (1..=15).chain([15]).collect();
        let h = Huffman::new(&lengths, Completeness::Required).unwrap();
        let mut seed = 99u32;
        let data: Vec<u8> = (0..4096)
            .map(|_| {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                (seed >> 16) as u8
            })
            .collect();
        let mut fast = BitReader::new(&data);
        let mut slow = BitReader::new(&data);
        loop {
            let a = h.decode(&mut fast);
            let b = h.decode_reference(&mut slow);
            assert_eq!(a, b);
            if a.is_err() {
                break;
            }
        }
    }
}
