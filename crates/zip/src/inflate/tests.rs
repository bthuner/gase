//! Hand-made DEFLATE streams, one feature at a time.

use super::*;

/// Builds a bit stream the way a compressor would.
#[derive(Default)]
struct BitWriter {
    bytes: Vec<u8>,
    bit: usize,
}

impl BitWriter {
    /// A number of `n` bits, least significant bit first (header fields,
    /// extra bits).
    fn bits(&mut self, value: u32, n: u32) -> &mut Self {
        for i in 0..n {
            if self.bit % 8 == 0 {
                self.bytes.push(0);
            }
            let b = ((value >> i) & 1) as u8;
            *self.bytes.last_mut().unwrap() |= b << (self.bit % 8);
            self.bit += 1;
        }
        self
    }

    /// A Huffman code, most significant bit first.
    fn code(&mut self, (code, len): (u32, u32)) -> &mut Self {
        for i in (0..len).rev() {
            self.bits((code >> i) & 1, 1);
        }
        self
    }

    fn align(&mut self) -> &mut Self {
        self.bit = self.bytes.len() * 8;
        self
    }

    fn raw(&mut self, data: &[u8]) -> &mut Self {
        self.align();
        self.bytes.extend_from_slice(data);
        self.bit = self.bytes.len() * 8;
        self
    }
}

/// Canonical codes for a list of lengths, computed exactly as in the RFC
/// (section 3.2.2, steps 1-3); independent of the decoder's code.
fn canonical(lengths: &[u8]) -> Vec<(u32, u32)> {
    let mut bl_count = [0u32; 16];
    for &l in lengths {
        bl_count[l as usize] += 1;
    }
    bl_count[0] = 0;
    let mut next_code = [0u32; 16];
    let mut code = 0;
    for bits in 1..16 {
        code = (code + bl_count[bits - 1]) << 1;
        next_code[bits] = code;
    }
    lengths
        .iter()
        .map(|&l| {
            let c = next_code[l as usize];
            next_code[l as usize] += 1;
            (c, u32::from(l))
        })
        .collect()
}

fn fixed_lengths() -> Vec<u8> {
    let mut l = vec![8u8; 288];
    l[144..256].fill(9);
    l[256..280].fill(7);
    l
}

/// Both decoders must give the same answer for every input.
fn both(data: &[u8], max: usize) -> Result<Vec<u8>, InflateError> {
    let fast = inflate(data, max);
    let reference = inflate_reference(data, max);
    assert_eq!(fast, reference, "fast and reference decoders disagree");
    fast
}

#[test]
fn stored_block() {
    let mut w = BitWriter::default();
    // BFINAL = 1, BTYPE = 00, then LEN = 5, NLEN = !5 on a byte boundary.
    w.bits(1, 1).bits(0, 2).raw(&[5, 0, !5, 0xFF]).raw(b"hello");
    assert_eq!(w.bytes, b"\x01\x05\x00\xfa\xffhello");
    assert_eq!(both(&w.bytes, 100).unwrap(), b"hello");
}

#[test]
fn empty_stored_block() {
    assert_eq!(both(&[0x01, 0, 0, 0xFF, 0xFF], 100).unwrap(), b"");
}

#[test]
fn stored_length_check() {
    assert_eq!(
        both(&[0x01, 5, 0, 0xFA, 0xFE, 1, 2, 3, 4, 5], 100),
        Err(InflateError::StoredLengthMismatch)
    );
}

#[test]
fn fixed_block_with_overlapping_match() {
    let lit = canonical(&fixed_lengths());
    let dist = canonical(&[5; 32]);
    let mut w = BitWriter::default();
    w.bits(1, 1).bits(1, 2);
    for &b in b"ab" {
        w.code(lit[b as usize]);
    }
    // Length 7 is symbol 261 (no extra bits); distance 2 is symbol 1.
    w.code(lit[261]).code(dist[1]);
    // Length 12 is symbol 265 (base 11) + 1 extra bit = 1; distance 9 is
    // symbol 6 (base 9, 2 extra bits = 0).
    w.code(lit[265]).bits(1, 1).code(dist[6]).bits(0, 2);
    w.code(lit[256]);
    assert_eq!(
        both(&w.bytes, 100).unwrap(),
        [&b"ababababa"[..], b"ababababa", b"aba"].concat()
    );
}

#[test]
fn fixed_codes_cover_all_literals() {
    let lit = canonical(&fixed_lengths());
    let mut w = BitWriter::default();
    w.bits(1, 1).bits(1, 2);
    for b in 0..=255u8 {
        w.code(lit[b as usize]);
    }
    w.code(lit[256]);
    let expected: Vec<u8> = (0..=255).collect();
    assert_eq!(both(&w.bytes, 1000).unwrap(), expected);
}

/// The header of the dynamic block described in the comments below, with
/// a choice of code-length-code symbols to make it valid or broken.
fn dynamic_header(cl_symbols: &[(u16, u32)]) -> BitWriter {
    // Code-length code: symbols 18 and 3 get 2 bits; 0, 1, 2, 16 get 3.
    let mut cl_lengths = [0u8; 19];
    for (s, l) in [(18, 2), (3, 2), (0, 3), (1, 3), (2, 3), (16, 3)] {
        cl_lengths[s] = l;
    }
    let cl = canonical(&cl_lengths);

    let mut w = BitWriter::default();
    w.bits(1, 1).bits(2, 2); // final, dynamic
    w.bits(259 - 257, 5); // HLIT: literal/length symbols 0..=258
    w.bits(2 - 1, 5); // HDIST: distance symbols 0..=1
    w.bits(18 - 4, 4); // HCLEN: send 18 lengths (up to symbol 1)
    for &s in &CODE_LENGTH_ORDER[..18] {
        w.bits(u32::from(cl_lengths[s]), 3);
    }
    for &(symbol, extra) in cl_symbols {
        w.code(cl[symbol as usize]);
        match symbol {
            16 => w.bits(extra, 2),
            17 => w.bits(extra, 3),
            18 => w.bits(extra, 7),
            _ => &mut w,
        };
    }
    w
}

fn dynamic_stream(cl_symbols: &[(u16, u32)]) -> Vec<u8> {
    dynamic_header(cl_symbols).bytes
}

/// The code lengths of the example: 'a'-'d' 3 bits, end-of-block and
/// symbol 258 (length 4) 2 bits; two 1-bit distance codes.
const DYNAMIC_LENGTHS: &[(u16, u32)] = &[
    (18, 97 - 11), // 97 zeros: symbols 0..=96
    (3, 0),        // 'a'
    (16, 0),       // 'b' 'c' 'd': repeat 3 times
    (18, 138 - 11),
    (18, 17 - 11), // 155 zeros: 101..=255
    (2, 0),        // 256: end of block
    (0, 0),        // 257
    (2, 0),        // 258: length 4
    (1, 0),        // distance symbol 0: distance 1
    (1, 0),        // distance symbol 1: distance 2
];

#[test]
fn dynamic_block() {
    let mut lit_lengths = vec![0u8; 259];
    lit_lengths[97..101].fill(3);
    lit_lengths[256] = 2;
    lit_lengths[258] = 2;
    let lit = canonical(&lit_lengths);
    let dist = canonical(&[1, 1]);

    let mut w = dynamic_header(DYNAMIC_LENGTHS);
    for &b in b"abcd" {
        w.code(lit[b as usize]);
    }
    w.code(lit[258]).code(dist[1]); // "cdcd"
    w.code(lit[258]).code(dist[0]); // "dddd"
    w.code(lit[256]);
    assert_eq!(both(&w.bytes, 100).unwrap(), b"abcdcdcddddd");
}

#[test]
fn dynamic_header_errors() {
    // A repeat as the very first length.
    let mut lengths = DYNAMIC_LENGTHS.to_vec();
    lengths.insert(0, (16, 0));
    assert_eq!(
        both(&dynamic_stream(&lengths), 100),
        Err(InflateError::RepeatWithoutPrevious)
    );
    // A zero run past the end of the 261 lengths.
    let mut lengths = DYNAMIC_LENGTHS.to_vec();
    lengths.insert(7, (18, 127));
    assert_eq!(
        both(&dynamic_stream(&lengths), 100),
        Err(InflateError::TooManyCodeLengths)
    );
    // No end-of-block code: symbol 256 gets length 0.
    let mut lengths = DYNAMIC_LENGTHS.to_vec();
    lengths[5] = (0, 0);
    assert_eq!(
        both(&dynamic_stream(&lengths), 100),
        Err(InflateError::MissingEndOfBlock)
    );
    // Lengths 2 for symbol 257 too: five codes of 2-3 bits do not fit.
    let mut lengths = DYNAMIC_LENGTHS.to_vec();
    lengths[6] = (2, 0);
    assert_eq!(
        both(&dynamic_stream(&lengths), 100),
        Err(InflateError::OversubscribedCode)
    );
    // Drop 'd': the literal/length code is incomplete (7/8 of the space).
    let lengths = [
        (18, 97 - 11),
        (3, 0),
        (3, 0),
        (3, 0),
        (18, 138 - 11),
        (18, 18 - 11),
        (2, 0),
        (0, 0),
        (2, 0),
        (1, 0),
        (1, 0),
    ];
    assert_eq!(
        both(&dynamic_stream(&lengths), 100),
        Err(InflateError::IncompleteCode)
    );
}

#[test]
fn bad_code_length_code() {
    // HCLEN = 4: lengths for 16, 17, 18, 0 only, all 1 bit: over-subscribed.
    let mut w = BitWriter::default();
    w.bits(1, 1).bits(2, 2).bits(0, 5).bits(0, 5).bits(0, 4);
    for _ in 0..4 {
        w.bits(1, 3);
    }
    assert_eq!(both(&w.bytes, 100), Err(InflateError::OversubscribedCode));
    // All code-length-code lengths zero: nothing can be decoded.
    let mut w = BitWriter::default();
    w.bits(1, 1)
        .bits(2, 2)
        .bits(0, 5)
        .bits(0, 5)
        .bits(0, 4)
        .bits(0, 12);
    assert_eq!(both(&w.bytes, 100), Err(InflateError::IncompleteCode));
}

#[test]
fn too_many_codes() {
    let mut w = BitWriter::default();
    w.bits(1, 1).bits(2, 2).bits(30, 5).bits(0, 5).bits(0, 4);
    assert_eq!(both(&w.bytes, 100), Err(InflateError::TooManyCodes));
}

#[test]
fn reserved_block_type() {
    assert_eq!(both(&[0x07], 100), Err(InflateError::InvalidBlockType));
}

#[test]
fn distance_before_start() {
    let lit = canonical(&fixed_lengths());
    let dist = canonical(&[5; 32]);
    let mut w = BitWriter::default();
    w.bits(1, 1).bits(1, 2);
    w.code(lit[b'x' as usize]).code(lit[257]).code(dist[1]); // distance 2 > 1 byte
    w.code(lit[256]);
    assert_eq!(both(&w.bytes, 100), Err(InflateError::DistanceTooFar));
}

#[test]
fn invalid_symbols() {
    let lit = canonical(&fixed_lengths());
    let dist = canonical(&[5; 32]);
    let mut w = BitWriter::default();
    w.bits(1, 1).bits(1, 2).code(lit[286]);
    assert_eq!(both(&w.bytes, 100), Err(InflateError::InvalidSymbol));
    let mut w = BitWriter::default();
    w.bits(1, 1).bits(1, 2);
    w.code(lit[b'x' as usize]).code(lit[257]).code(dist[30]);
    assert_eq!(both(&w.bytes, 100), Err(InflateError::InvalidSymbol));
}

#[test]
fn several_blocks() {
    let lit = canonical(&fixed_lengths());
    let mut w = BitWriter::default();
    w.bits(0, 1).bits(0, 2).raw(&[2, 0, 0xFD, 0xFF]).raw(b"ab");
    w.bits(0, 1)
        .bits(1, 2)
        .code(lit[b'c' as usize])
        .code(lit[256]);
    w.bits(1, 1).bits(0, 2).raw(&[1, 0, 0xFE, 0xFF]).raw(b"d");
    assert_eq!(both(&w.bytes, 100).unwrap(), b"abcd");
}

#[test]
fn truncated_streams_are_errors() {
    let data = b"hello hello hello hello, said the zip file to the emulator";
    for stream in [
        crate::test_util::stored(data),
        crate::test_util::fixed_literals(data),
    ] {
        assert_eq!(both(&stream, 1000).unwrap(), data);
        for cut in 0..stream.len() {
            assert!(both(&stream[..cut], 1000).is_err(), "cut at {cut}");
        }
    }
    assert_eq!(both(&[], 100), Err(InflateError::UnexpectedEof));
}

#[test]
fn output_is_limited() {
    let data = [7u8; 300];
    let stream = crate::test_util::stored(&data);
    assert_eq!(both(&stream, 300).unwrap().len(), 300);
    assert_eq!(both(&stream, 299), Err(InflateError::OutputTooLarge));
    // A match that would cross the limit: "a" then <258, distance 1>.
    let lit = canonical(&fixed_lengths());
    let dist = canonical(&[5; 32]);
    let mut w = BitWriter::default();
    w.bits(1, 1).bits(1, 2).code(lit[b'a' as usize]);
    w.code(lit[285]).code(dist[0]).code(lit[256]);
    assert_eq!(both(&w.bytes, 259).unwrap(), [b'a'; 259]);
    assert_eq!(both(&w.bytes, 258), Err(InflateError::OutputTooLarge));
    assert_eq!(both(&w.bytes, 0), Err(InflateError::OutputTooLarge));
}

#[test]
fn random_input_never_panics() {
    // Random bytes, and valid streams with random damage: whatever comes
    // out, both decoders agree and nothing panics.
    let mut seed = 0xDEAD_BEEFu32;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    for _ in 0..20_000 {
        let len = (next() % 64) as usize;
        let mut data: Vec<u8> = (0..len).map(|_| next() as u8).collect();
        // Make dynamic blocks (the hardest case) more likely.
        if let Some(b) = data.first_mut() {
            if next() % 2 == 0 {
                *b = (*b & !6) | 4;
            }
        }
        let _ = both(&data, 1 << 16);
    }
    let valid = dynamic_stream(DYNAMIC_LENGTHS);
    for _ in 0..20_000 {
        let mut data = valid.clone();
        for _ in 0..1 + next() % 3 {
            let i = next() as usize % data.len();
            data[i] ^= 1 << (next() % 8);
        }
        data.extend((0..next() % 16).map(|_| next() as u8));
        let _ = both(&data, 1 << 16);
    }
}
