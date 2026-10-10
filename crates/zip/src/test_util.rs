//! Tiny DEFLATE and ZIP *writers* for the tests: just enough to build
//! valid inputs, and broken variants of them, by hand.

use crate::crc32::crc32;

/// A DEFLATE stream of stored blocks.
pub(crate) fn stored(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut blocks = data.chunks(0xFFFF).peekable();
    if blocks.peek().is_none() {
        return vec![1, 0, 0, 0xFF, 0xFF];
    }
    while let Some(block) = blocks.next() {
        out.push(u8::from(blocks.peek().is_none()));
        let len = block.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(block);
    }
    out
}

/// A single fixed-Huffman block of literals only.
pub(crate) fn fixed_literals(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut bit = 0usize;
    let mut put = |value: u32, len: u32, msb_first: bool| {
        for i in 0..len {
            let b = if msb_first {
                (value >> (len - 1 - i)) & 1
            } else {
                (value >> i) & 1
            };
            if bit % 8 == 0 {
                out.push(0);
            }
            *out.last_mut().unwrap() |= (b as u8) << (bit % 8);
            bit += 1;
        }
    };
    put(1, 1, false);
    put(1, 2, false);
    for &byte in data {
        if byte < 144 {
            put(0x30 + u32::from(byte), 8, true);
        } else {
            put(0x190 + u32::from(byte) - 144, 9, true);
        }
    }
    put(0, 7, true); // end of block: symbol 256, code 0000000
    out
}

/// One file for [`zip`].
pub(crate) struct File<'a> {
    pub name: &'a str,
    pub data: &'a [u8],
    /// 0 = stored, 8 = deflate (with stored blocks).
    pub method: u16,
    pub flags: u16,
}

impl<'a> File<'a> {
    pub(crate) fn new(name: &'a str, data: &'a [u8], method: u16) -> Self {
        Self {
            name,
            data,
            method,
            flags: 0,
        }
    }
}

/// Build a ZIP archive.
pub(crate) fn zip(files: &[File<'_>], comment: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for f in files {
        let packed = match f.method {
            8 => stored(f.data),
            _ => f.data.to_vec(),
        };
        let offset = out.len() as u32;
        let crc = crc32(f.data);
        // Fields shared by the local header (from offset 4) and the central
        // directory record (from offset 6).
        let mut common = Vec::new();
        common.extend_from_slice(&20u16.to_le_bytes()); // version needed
        common.extend_from_slice(&f.flags.to_le_bytes());
        common.extend_from_slice(&f.method.to_le_bytes());
        common.extend_from_slice(&[0, 0, 0x21, 0]); // time, date: 1980-01-01
        common.extend_from_slice(&crc.to_le_bytes());
        common.extend_from_slice(&(packed.len() as u32).to_le_bytes());
        common.extend_from_slice(&(f.data.len() as u32).to_le_bytes());
        common.extend_from_slice(&(f.name.len() as u16).to_le_bytes());
        common.extend_from_slice(&0u16.to_le_bytes()); // extra length

        out.extend_from_slice(b"PK\x03\x04");
        out.extend_from_slice(&common);
        out.extend_from_slice(f.name.as_bytes());
        out.extend_from_slice(&packed);

        central.extend_from_slice(b"PK\x01\x02");
        central.extend_from_slice(&20u16.to_le_bytes()); // version made by
        central.extend_from_slice(&common);
        central.extend_from_slice(&[0; 10]); // comment len, disk, attributes
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(f.name.as_bytes());
    }
    let cd_offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(b"PK\x05\x06");
    out.extend_from_slice(&[0; 4]); // disks
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&(comment.len() as u16).to_le_bytes());
    out.extend_from_slice(comment);
    out
}
