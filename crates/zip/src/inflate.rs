//! DEFLATE decompression (RFC 1951), the method almost every ZIP file uses.
//!
//! DEFLATE combines two old ideas: **LZ77**, which replaces repeated text
//! by references to an earlier copy, and **Huffman coding**, which writes
//! common symbols with fewer bits. Decompressing ("inflating") undoes them
//! in the opposite order: decode Huffman symbols, then follow them.
//!
//! # LZ77: back-references
//!
//! The decompressor's output is also its dictionary. Each step either
//! emits a **literal** byte or a **match**: "go back *distance* bytes in
//! what you have written so far and copy *length* bytes from there". The
//! distance can be at most 32 768 (the *window*), the length 3 to 258.
//!
//! ```text
//! input symbols:  'a' 'b' 'c' <length 6, distance 3>
//! output:         a b c a b c a b c
//!                       ^^^^^^^^^^^ copied, starting 3 bytes back
//! ```
//!
//! The copy may overlap what it is writing: with distance 3 and length 6
//! the copy reads bytes it has just produced, which turns a short match
//! into a repetition (distance 1 repeats one byte, like run-length
//! encoding). So the copy must go byte by byte, front to back, whenever
//! `distance < length`.
//!
//! # Blocks
//!
//! A stream is a sequence of blocks. Each starts with a 3-bit header: one
//! bit `BFINAL` (this is the last block) and two bits `BTYPE`:
//!
//! | `BTYPE` | block |
//! |---|---|
//! | 0 | **stored**: raw bytes, for data that does not compress |
//! | 1 | **fixed Huffman**: compressed with codes defined by the RFC |
//! | 2 | **dynamic Huffman**: compressed with codes sent at the block start |
//! | 3 | reserved (an error) |
//!
//! A stored block skips to the next byte boundary, then has `LEN` and its
//! one's complement `NLEN` (16 bits each, a cheap sanity check) and `LEN`
//! raw bytes.
//!
//! # One alphabet for literals and lengths
//!
//! Compressed blocks use two Huffman codes. The **literal/length** code
//! has 286 symbols: 0-255 are literal bytes, 256 ends the block and
//! 257-285 start a match and give its length. The **distance** code (30
//! symbols) follows each length symbol and gives the distance.
//!
//! Lengths and distances have too many values to give each its own symbol,
//! so a symbol stands for a range, and **extra bits** following it (read as
//! a plain number, not Huffman-coded) select the value inside the range.
//! Small, common values get narrow ranges and few extra bits:
//!
//! | length symbols | extra bits | lengths |
//! |---|---|---|
//! | 257-264 | 0 | 3-10 |
//! | 265-268 | 1 | 11-18 (in steps of 2) |
//! | 269-272 | 2 | 19-34 |
//! | ... | ... | ... |
//! | 281-284 | 5 | 131-257 |
//! | 285 | 0 | 258 |
//!
//! Distances work the same way with 0 to 13 extra bits (see
//! [`LENGTH_BASE`], [`LENGTH_EXTRA`], [`DIST_BASE`], [`DIST_EXTRA`]).
//!
//! # Fixed codes
//!
//! Short blocks are not worth sending a code for, so `BTYPE` 1 uses a
//! code from the RFC: literal/length symbols 0-143 have 8-bit codes,
//! 144-255 9 bits, 256-279 7 bits and 280-287 8 bits; all distance codes
//! are 5 bits.
//!
//! # Dynamic codes, and a code for the code
//!
//! `BTYPE` 2 sends its own codes as lists of code lengths (a canonical
//! Huffman code is fully described by its lengths, see [`huffman`]). Those
//! lists are long (up to 316 numbers) and repetitive, so they are
//! themselves compressed: with a third Huffman code, the **code-length
//! code**, over a 19-symbol alphabet:
//!
//! | symbol | meaning |
//! |---|---|
//! | 0-15 | a code length |
//! | 16 | repeat the previous length 3-6 times (2 extra bits) |
//! | 17 | 3-10 zero lengths (3 extra bits) |
//! | 18 | 11-138 zero lengths (7 extra bits) |
//!
//! The code-length code is sent as up to 19 lengths of 3 bits each, in
//! this peculiar order:
//!
//! ```text
//! 16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15
//! ```
//!
//! The header says how many are sent (`HCLEN` + 4); the rest are zero.
//! The order puts the symbols most likely to be used first (the repeat
//! codes and middle lengths around 8) and the unlikely ones (very short
//! and very long lengths) last, so that trailing unused entries can simply
//! be left out. A dynamic block header is therefore:
//!
//! ```text
//! HLIT  (5 bits)  number of literal/length lengths - 257
//! HDIST (5 bits)  number of distance lengths - 1
//! HCLEN (4 bits)  number of code-length-code lengths - 4
//! (HCLEN + 4) x 3 bits     code-length code, in the order above
//! HLIT + 257 + HDIST + 1   code lengths, coded with the code-length code
//!                          (one list: a repeat may run from the
//!                           literal/length lengths into the distance ones)
//! ```
//!
//! # Robustness
//!
//! Input may be damaged or hostile, so every rule is checked and reported
//! as an [`InflateError`] instead of a panic: invalid block types, bad
//! `NLEN`, over-subscribed or incomplete codes, repeats with nothing to
//! repeat, distances reaching before the start of the output, truncated
//! input. The output size is capped by the caller (`max_output`), so a
//! tiny "zip bomb" cannot exhaust memory: DEFLATE can expand data about
//! 1000-fold.
//!
//! # Speed
//!
//! Following the rule of the rest of gase, a readable reference and a
//! tested fast path:
//!
//! * [`inflate_reference`] decodes every Huffman code bit by bit, as the
//!   RFC describes it, and copies every match byte by byte;
//! * [`inflate`] decodes most codes with one table lookup (see
//!   [`huffman`]) and copies matches that do not overlap their source as
//!   one slice.
//!
//! Both share everything else, and the tests check they give identical
//! results, errors included, on hand-made, zlib-made and random streams.
//! Two more details matter for speed: the bit reader loads 8 input bytes
//! at a time and only when fewer than 32 bits are left, and the fixed
//! codes are built once. On a 2.1 GHz Xeon the fast path inflates a 4 MiB
//! ROM in about 19 ms (200 MiB/s, on par with zlib), the reference in
//! about 55 ms.

pub mod bits;
pub mod huffman;

use std::fmt;
use std::sync::OnceLock;

use bits::BitReader;
use huffman::{Completeness, Huffman};

/// Default cap on the decompressed size: 64 MiB, far above any Mega Drive
/// ROM (the largest are 5 MiB) and small enough to be harmless.
pub const DEFAULT_MAX_OUTPUT: usize = 64 << 20;

/// The largest distance a match may have.
pub const WINDOW_SIZE: usize = 32 * 1024;

/// Base length for literal/length symbols 257..=285.
pub const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
/// Extra bits after literal/length symbols 257..=285.
pub const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
/// Base distance for distance symbols 0..=29.
pub const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
/// Extra bits after distance symbols 0..=29.
pub const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

/// The order in which the code-length code's lengths are sent.
pub const CODE_LENGTH_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// Why a DEFLATE stream could not be decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InflateError {
    /// The input ended in the middle of the stream.
    UnexpectedEof,
    /// Block type 3, which is reserved.
    InvalidBlockType,
    /// A stored block's `NLEN` is not the complement of its `LEN`.
    StoredLengthMismatch,
    /// A dynamic block declares more than 286 literal/length or 30
    /// distance codes.
    TooManyCodes,
    /// Code lengths that would give two symbols the same code.
    OversubscribedCode,
    /// Code lengths that leave bit patterns without meaning.
    IncompleteCode,
    /// A "repeat previous length" with no previous length.
    RepeatWithoutPrevious,
    /// Repeated code lengths run past the end of the list.
    TooManyCodeLengths,
    /// The literal/length code has no end-of-block symbol.
    MissingEndOfBlock,
    /// Bits that are not a code (possible only with an incomplete code).
    InvalidCode,
    /// Literal/length symbol 286 or 287, or distance symbol 30 or 31.
    InvalidSymbol,
    /// A match reaches back before the start of the output.
    DistanceTooFar,
    /// The output would exceed the allowed size.
    OutputTooLarge,
}

impl fmt::Display for InflateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            InflateError::UnexpectedEof => "compressed data is truncated",
            InflateError::InvalidBlockType => "invalid block type",
            InflateError::StoredLengthMismatch => "stored block length check failed",
            InflateError::TooManyCodes => "too many length or distance codes",
            InflateError::OversubscribedCode => "over-subscribed Huffman code",
            InflateError::IncompleteCode => "incomplete Huffman code",
            InflateError::RepeatWithoutPrevious => "code length repeat with no previous length",
            InflateError::TooManyCodeLengths => "code length repeat runs past the end",
            InflateError::MissingEndOfBlock => "Huffman code has no end-of-block symbol",
            InflateError::InvalidCode => "invalid Huffman code",
            InflateError::InvalidSymbol => "invalid length or distance symbol",
            InflateError::DistanceTooFar => "match distance reaches before the start",
            InflateError::OutputTooLarge => "decompressed data is too large",
        })
    }
}

impl std::error::Error for InflateError {}

/// Decompress a raw DEFLATE stream (no zlib or gzip wrapper) of at most
/// `max_output` bytes.
///
/// ```
/// // "hello hello hello": literals, then <length 12, distance 6>.
/// let packed = [0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x57, 0xc8, 0x40, 0x90, 0x00];
/// let text = gase_zip::inflate(&packed, 100).unwrap();
/// assert_eq!(text, b"hello hello hello");
/// ```
pub fn inflate(data: &[u8], max_output: usize) -> Result<Vec<u8>, InflateError> {
    let mut out = Vec::new();
    inflate_into::<true>(data, &mut out, max_output)?;
    Ok(out)
}

/// [`inflate`] with the bit-at-a-time reference Huffman decoder: slower,
/// and the definition the fast path is tested against.
pub fn inflate_reference(data: &[u8], max_output: usize) -> Result<Vec<u8>, InflateError> {
    let mut out = Vec::new();
    inflate_into::<false>(data, &mut out, max_output)?;
    Ok(out)
}

/// Decompress `data`, appending to `out` (which should be empty, or hold
/// data the stream may refer back to). `out` never grows beyond
/// `max_output`; reserve capacity beforehand when the size is known.
pub(crate) fn inflate_into<const FAST: bool>(
    data: &[u8],
    out: &mut Vec<u8>,
    max_output: usize,
) -> Result<(), InflateError> {
    let mut input = BitReader::new(data);
    loop {
        let last = input.bits(1)? == 1;
        match input.bits(2)? {
            0 => stored_block(&mut input, out, max_output)?,
            1 => {
                let (lit, dist) = fixed_codes();
                codes::<FAST>(&mut input, out, max_output, lit, dist)?;
            }
            2 => {
                let (lit, dist) = dynamic_codes::<FAST>(&mut input)?;
                codes::<FAST>(&mut input, out, max_output, &lit, &dist)?;
            }
            _ => return Err(InflateError::InvalidBlockType),
        }
        if last {
            return Ok(());
        }
    }
}

fn stored_block(
    input: &mut BitReader<'_>,
    out: &mut Vec<u8>,
    max_output: usize,
) -> Result<(), InflateError> {
    input.align_to_byte();
    let header = input.take_bytes(4)?;
    let len = u16::from_le_bytes([header[0], header[1]]);
    let nlen = u16::from_le_bytes([header[2], header[3]]);
    if len != !nlen {
        return Err(InflateError::StoredLengthMismatch);
    }
    let bytes = input.take_bytes(len as usize)?;
    if out.len() + bytes.len() > max_output {
        return Err(InflateError::OutputTooLarge);
    }
    out.extend_from_slice(bytes);
    Ok(())
}

/// The fixed codes of `BTYPE` 1 (RFC 1951 section 3.2.6), built once.
fn fixed_codes() -> &'static (Huffman, Huffman) {
    static CODES: OnceLock<(Huffman, Huffman)> = OnceLock::new();
    CODES.get_or_init(build_fixed_codes)
}

fn build_fixed_codes() -> (Huffman, Huffman) {
    let mut lengths = [0u8; 288];
    lengths[..144].fill(8);
    lengths[144..256].fill(9);
    lengths[256..280].fill(7);
    lengths[280..].fill(8);
    // These are valid by construction; `expect` documents that.
    let lit = Huffman::new(&lengths, Completeness::Required).expect("fixed literal code");
    // 32 distance codes, of which 30 and 31 are never valid.
    let dist = Huffman::new(&[5; 32], Completeness::Required).expect("fixed distance code");
    (lit, dist)
}

#[inline]
fn decode<const FAST: bool>(
    code: &Huffman,
    input: &mut BitReader<'_>,
) -> Result<u16, InflateError> {
    if FAST {
        code.decode(input)
    } else {
        code.decode_reference(input)
    }
}

/// Read a dynamic block's header and build its two codes.
fn dynamic_codes<const FAST: bool>(
    input: &mut BitReader<'_>,
) -> Result<(Huffman, Huffman), InflateError> {
    let nlen = input.bits(5)? as usize + 257;
    let ndist = input.bits(5)? as usize + 1;
    let ncode = input.bits(4)? as usize + 4;
    if nlen > 286 || ndist > 30 {
        return Err(InflateError::TooManyCodes);
    }

    // The code-length code, sent in CODE_LENGTH_ORDER.
    let mut cl_lengths = [0u8; 19];
    for &symbol in &CODE_LENGTH_ORDER[..ncode] {
        cl_lengths[symbol] = input.bits(3)? as u8;
    }
    let cl_code = Huffman::new(&cl_lengths, Completeness::Required)?;

    // The literal/length and distance lengths, as one list.
    let mut lengths = [0u8; 286 + 30];
    let total = nlen + ndist;
    let mut i = 0;
    while i < total {
        let symbol = decode::<FAST>(&cl_code, input)?;
        let (value, repeat) = match symbol {
            0..=15 => (symbol as u8, 1),
            16 => {
                let previous = *i
                    .checked_sub(1)
                    .and_then(|p| lengths.get(p))
                    .ok_or(InflateError::RepeatWithoutPrevious)?;
                (previous, 3 + input.bits(2)? as usize)
            }
            17 => (0, 3 + input.bits(3)? as usize),
            _ => (0, 11 + input.bits(7)? as usize), // 18
        };
        if i + repeat > total {
            return Err(InflateError::TooManyCodeLengths);
        }
        lengths[i..i + repeat].fill(value);
        i += repeat;
    }

    if lengths[256] == 0 {
        return Err(InflateError::MissingEndOfBlock);
    }
    let lit = Huffman::new(&lengths[..nlen], Completeness::AllowSingle)?;
    let dist = Huffman::new(&lengths[nlen..total], Completeness::AllowSingle)?;
    Ok((lit, dist))
}

/// Decode the compressed data of a block until its end-of-block symbol.
fn codes<const FAST: bool>(
    input: &mut BitReader<'_>,
    out: &mut Vec<u8>,
    max_output: usize,
    lit: &Huffman,
    dist: &Huffman,
) -> Result<(), InflateError> {
    loop {
        let symbol = decode::<FAST>(lit, input)?;
        if symbol < 256 {
            if out.len() >= max_output {
                return Err(InflateError::OutputTooLarge);
            }
            out.push(symbol as u8);
            continue;
        }
        if symbol == 256 {
            return Ok(());
        }

        // A match: length symbol + extra bits, distance symbol + extra bits.
        let index = usize::from(symbol - 257);
        let (Some(&base), Some(&extra)) = (LENGTH_BASE.get(index), LENGTH_EXTRA.get(index)) else {
            return Err(InflateError::InvalidSymbol);
        };
        let length = usize::from(base) + input.bits(u32::from(extra))? as usize;

        let index = usize::from(decode::<FAST>(dist, input)?);
        let (Some(&base), Some(&extra)) = (DIST_BASE.get(index), DIST_EXTRA.get(index)) else {
            return Err(InflateError::InvalidSymbol);
        };
        let distance = usize::from(base) + input.bits(u32::from(extra))? as usize;

        if distance > out.len() {
            return Err(InflateError::DistanceTooFar);
        }
        if out.len() + length > max_output {
            return Err(InflateError::OutputTooLarge);
        }
        copy_match::<FAST>(out, distance, length);
    }
}

/// Append `length` bytes copied from `distance` bytes back.
#[inline]
fn copy_match<const FAST: bool>(out: &mut Vec<u8>, distance: usize, length: usize) {
    let start = out.len() - distance;
    if FAST && distance >= length {
        // Source and destination do not overlap: one slice copy.
        out.extend_from_within(start..start + length);
    } else {
        // The reference: byte by byte, so an overlapping copy re-reads the
        // bytes it has just written.
        for i in start..start + length {
            let byte = out[i];
            out.push(byte);
        }
    }
}

#[cfg(test)]
mod tests;
