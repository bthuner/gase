//! Reading a DEFLATE stream bit by bit.
//!
//! DEFLATE packs its fields into bytes starting from the **least
//! significant bit**: the first field of a stream occupies the low bits of
//! the first byte, and a field that does not fit continues in the low bits
//! of the next byte. Multi-bit numbers (block type, lengths, extra bits)
//! are stored least significant bit first, so reading them is just
//! "take the next *n* bits as a little-endian number".
//!
//! Huffman codes are the exception: RFC 1951 stores them **most
//! significant bit first**, so a code is read one bit at a time and each
//! new bit is appended on the right (see [`super::huffman`]).
//!
//! The reader keeps up to 64 bits in a buffer so most fields need no
//! memory access at all. Past the end of the input the buffer is filled
//! with zero bits that can be *peeked* (a table lookup may look ahead
//! further than the code it finds) but not *consumed*: consuming a bit
//! that is not really there is an [`InflateError::UnexpectedEof`].

use super::InflateError;

#[derive(Debug, Clone)]
pub(crate) struct BitReader<'a> {
    data: &'a [u8],
    /// Next byte of `data` to move into the buffer.
    pos: usize,
    /// Buffered bits; the next bit to read is bit 0. Bits above `count`
    /// are always zero.
    buf: u64,
    /// Number of real (not padding) bits in `buf`.
    count: u32,
}

impl<'a> BitReader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            buf: 0,
            count: 0,
        }
    }

    /// Make sure at least 32 bits are buffered, if the input has them:
    /// enough for any Huffman code (15 bits) plus its extra bits (13).
    #[inline]
    pub(crate) fn ensure_32(&mut self) {
        if self.count < 32 {
            self.refill();
        }
    }

    /// Top the buffer up to at least 56 bits, if the input has them.
    #[inline]
    pub(crate) fn refill(&mut self) {
        if let Some(bytes) = self.data.get(self.pos..self.pos + 8) {
            // Fast path: load 8 bytes at once and keep as many whole bytes
            // as fit above the bits already buffered.
            let word = u64::from_le_bytes(bytes.try_into().expect("8 bytes"));
            self.buf |= word << self.count;
            let taken = (63 - self.count) >> 3;
            self.pos += taken as usize;
            self.count += taken * 8;
            // Bits of the 8-byte word beyond `count` were shifted in but not
            // taken: clear them so the "zero above count" invariant holds.
            self.buf &= low_bits(self.count);
        } else {
            // Near the end of the input: one byte at a time.
            while self.count <= 56 && self.pos < self.data.len() {
                self.buf |= u64::from(self.data[self.pos]) << self.count;
                self.pos += 1;
                self.count += 8;
            }
        }
    }

    /// The next `n` bits (n <= 32) without consuming them. Call
    /// [`Self::ensure_32`] first; bits past the end of the input read as
    /// zero.
    #[inline]
    pub(crate) fn peek(&self, n: u32) -> u32 {
        (self.buf & low_bits(n)) as u32
    }

    /// Drop `n` bits that were peeked.
    #[inline]
    pub(crate) fn consume(&mut self, n: u32) -> Result<(), InflateError> {
        if n > self.count {
            return Err(InflateError::UnexpectedEof);
        }
        self.buf >>= n;
        self.count -= n;
        Ok(())
    }

    /// Read an `n`-bit number (n <= 32), least significant bit first.
    #[inline]
    pub(crate) fn bits(&mut self, n: u32) -> Result<u32, InflateError> {
        if self.count < n {
            self.refill();
        }
        let value = self.peek(n);
        self.consume(n)?;
        Ok(value)
    }

    /// Skip to the next byte boundary and hand back the buffered whole
    /// bytes, so that raw bytes can be read with [`Self::take_bytes`].
    pub(crate) fn align_to_byte(&mut self) {
        let partial = self.count % 8;
        self.buf >>= partial;
        self.count -= partial;
        self.pos -= (self.count / 8) as usize;
        self.buf = 0;
        self.count = 0;
    }

    /// Take `n` raw bytes (after [`Self::align_to_byte`]).
    pub(crate) fn take_bytes(&mut self, n: usize) -> Result<&'a [u8], InflateError> {
        debug_assert_eq!(self.count, 0, "take_bytes needs an aligned reader");
        let bytes = self
            .data
            .get(self.pos..self.pos + n)
            .ok_or(InflateError::UnexpectedEof)?;
        self.pos += n;
        Ok(bytes)
    }
}

/// The low `n` bits set (n < 64).
#[inline]
fn low_bits(n: u32) -> u64 {
    debug_assert!(n < 64);
    (1u64 << n) - 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_are_read_lsb_first() {
        // 0b1011_0110, 0b0000_0001
        let mut r = BitReader::new(&[0xB6, 0x01]);
        assert_eq!(r.bits(1), Ok(0));
        assert_eq!(r.bits(2), Ok(0b11));
        assert_eq!(r.bits(3), Ok(0b110));
        // Crosses into the second byte: bits 6-7 of byte 0, bit 0 of byte 1.
        assert_eq!(r.bits(3), Ok(0b110));
        assert_eq!(r.bits(7), Ok(0));
        assert_eq!(r.bits(1), Err(InflateError::UnexpectedEof));
    }

    #[test]
    fn fast_and_slow_refill_agree() {
        let data: Vec<u8> = (0..40u8).map(|i| i.wrapping_mul(37) ^ 0x5A).collect();
        // Read with widths 1..=13 cycling, compare against a bit-at-a-time
        // reading of the same bytes.
        let mut r = BitReader::new(&data);
        let mut bit = 0usize;
        let mut width = 1;
        while bit + width <= data.len() * 8 {
            let mut expected = 0u32;
            for i in 0..width {
                let b = bit + i;
                expected |= u32::from((data[b / 8] >> (b % 8)) & 1) << i;
            }
            assert_eq!(r.bits(width as u32), Ok(expected), "at bit {bit}");
            bit += width;
            width = width % 13 + 1;
        }
    }

    #[test]
    fn align_returns_buffered_bytes() {
        let mut r = BitReader::new(&[0xFF, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        assert_eq!(r.bits(3), Ok(7));
        r.align_to_byte();
        assert_eq!(r.take_bytes(3), Ok(&[1u8, 2, 3][..]));
        assert_eq!(r.bits(8), Ok(4));
        r.align_to_byte();
        assert_eq!(r.take_bytes(6), Ok(&[5u8, 6, 7, 8, 9, 10][..]));
        assert_eq!(r.take_bytes(1), Err(InflateError::UnexpectedEof));
    }
}
