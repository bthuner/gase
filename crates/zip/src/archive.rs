//! The ZIP container: finding and reading the files inside an archive.
//!
//! # Layout
//!
//! A ZIP file is a sequence of files, each preceded by a *local header*,
//! followed by a table of contents, the *central directory*, and a short
//! *end of central directory* record (EOCD) that says where the table is:
//!
//! ```text
//! ┌──────────────┬────────────┬──────────────┬────────────┬─────┬───────────────────┬──────┐
//! │ local header │ file data  │ local header │ file data  │ ... │ central directory │ EOCD │
//! │ "PK\3\4"     │ (deflated) │ "PK\3\4"     │ (stored)   │     │ "PK\1\2" × n      │"PK\5\6"
//! └──────────────┴────────────┴──────────────┴────────────┴─────┴───────────────────┴──────┘
//! ```
//!
//! Putting the table at the end lets a program write an archive in one
//! pass (it only knows the sizes after compressing) and update one by
//! appending. Readers therefore start **at the end**: find the EOCD, jump
//! to the central directory, and only then visit the files. Every record
//! starts with a 4-byte signature, `"PK"` (for Phil Katz, ZIP's author)
//! and two bytes for the record type; all numbers are little-endian.
//!
//! # Finding the end
//!
//! The EOCD is 22 bytes plus an archive comment of up to 65 535 bytes, so
//! it is not at a fixed position. We scan backwards from the last
//! possible position for the signature `PK\5\6`, and accept the first
//! candidate whose comment length fits in the file. (A comment could
//! itself contain the signature; checking the length rules out most such
//! false matches, and it is what other readers do too.)
//!
//! | EOCD offset | size | field |
//! |---|---|---|
//! | 0 | 4 | signature `PK\5\6` |
//! | 4 | 2 | number of this disk (0 unless split) |
//! | 6 | 2 | disk where the central directory starts |
//! | 8 | 2 | central directory entries on this disk |
//! | 10 | 2 | central directory entries in total |
//! | 12 | 4 | size of the central directory |
//! | 16 | 4 | offset of the central directory |
//! | 20 | 2 | comment length, then the comment |
//!
//! # The central directory
//!
//! One 46-byte record per file (signature `PK\1\2`), then its name, an
//! "extra field" (extensions such as timestamps) and a file comment:
//!
//! | offset | size | field |
//! |---|---|---|
//! | 8 | 2 | general-purpose flags (bit 0: encrypted, bit 3: data descriptor, bit 11: UTF-8 name) |
//! | 10 | 2 | compression method (0 = stored, 8 = deflate) |
//! | 16 | 4 | CRC-32 of the uncompressed data |
//! | 20 | 4 | compressed size |
//! | 24 | 4 | uncompressed size |
//! | 28 | 2 | name length |
//! | 30 | 2 | extra field length |
//! | 32 | 2 | file comment length |
//! | 42 | 4 | offset of the local header |
//!
//! # Local headers
//!
//! Each file's data is preceded by a 30-byte local header (signature
//! `PK\3\4`) repeating most of the central directory record. Only its name
//! and extra field lengths (offsets 26 and 28) are needed: they say where
//! the data starts. The sizes and CRC are taken from the central
//! directory, because a program that wrote the archive in a stream could
//! not know them when it wrote the local header: it sets flag bit 3 and
//! stores them in a *data descriptor* after the data instead.
//!
//! # Limits of the format
//!
//! The 32-bit sizes and offsets top out at 4 GiB. ZIP64 extends them by
//! setting fields to `0xFFFFFFFF` and adding 64-bit records; nothing a
//! console ROM needs, so it is detected and refused ([`Error::Zip64`]).

use crate::crc32::crc32;
use crate::inflate::inflate_into;
use crate::{Error, MAX_FILE_SIZE};

const EOCD_SIGNATURE: u32 = 0x0605_4B50; // "PK\5\6"
const ZIP64_LOCATOR_SIGNATURE: u32 = 0x0706_4B50; // "PK\6\7"
const CENTRAL_SIGNATURE: u32 = 0x0201_4B50; // "PK\1\2"
const LOCAL_SIGNATURE: u32 = 0x0403_4B50; // "PK\3\4"
const EOCD_SIZE: usize = 22;
const CENTRAL_SIZE: usize = 46;
const LOCAL_SIZE: usize = 30;

/// General-purpose flag bits.
const FLAG_ENCRYPTED: u16 = 1 << 0;
const FLAG_STRONG_ENCRYPTION: u16 = 1 << 6;
const FLAG_UTF8: u16 = 1 << 11;

/// Little-endian field access within a record, bounds-checked.
#[derive(Clone, Copy)]
struct Record<'a>(&'a [u8]);

impl Record<'_> {
    fn u16(self, offset: usize) -> u16 {
        u16::from_le_bytes([self.0[offset], self.0[offset + 1]])
    }
    fn u32(self, offset: usize) -> u32 {
        u32::from_le_bytes([
            self.0[offset],
            self.0[offset + 1],
            self.0[offset + 2],
            self.0[offset + 3],
        ])
    }
}

/// `len` bytes of `data` at `offset`, or `Truncated(what)`.
fn slice<'a>(
    data: &'a [u8],
    offset: usize,
    len: usize,
    what: &'static str,
) -> Result<&'a [u8], Error> {
    offset
        .checked_add(len)
        .and_then(|end| data.get(offset..end))
        .ok_or(Error::Truncated(what))
}

/// A record of `len` bytes at `offset` starting with `signature`.
fn record<'a>(
    data: &'a [u8],
    offset: usize,
    len: usize,
    signature: u32,
    what: &'static str,
) -> Result<Record<'a>, Error> {
    let r = Record(slice(data, offset, len, what)?);
    if r.u32(0) == signature {
        Ok(r)
    } else {
        Err(Error::BadSignature(what))
    }
}

/// How a file's data is stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// Method 0: not compressed.
    Stored,
    /// Method 8: DEFLATE.
    Deflated,
    /// Anything else (bzip2, LZMA, ...): listed, but cannot be extracted.
    Other(u16),
}

impl From<u16> for Method {
    fn from(m: u16) -> Self {
        match m {
            0 => Method::Stored,
            8 => Method::Deflated,
            m => Method::Other(m),
        }
    }
}

/// A file (or directory) in an archive, from the central directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    name: String,
    method: Method,
    flags: u16,
    crc32: u32,
    compressed_size: u32,
    size: u32,
    /// Where the local header is, in the bytes given to [`Archive::parse`].
    local_header: usize,
}

impl Entry {
    /// The path inside the archive, with `/` separators.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The size of the extracted file.
    #[must_use]
    pub fn size(&self) -> u64 {
        u64::from(self.size)
    }

    /// The size of the data in the archive.
    #[must_use]
    pub fn compressed_size(&self) -> u64 {
        u64::from(self.compressed_size)
    }

    /// The compression method.
    #[must_use]
    pub fn method(&self) -> Method {
        self.method
    }

    /// The CRC-32 the extracted data must have.
    #[must_use]
    pub fn crc32(&self) -> u32 {
        self.crc32
    }

    /// Directories are stored as empty entries whose name ends with `/`.
    #[must_use]
    pub fn is_dir(&self) -> bool {
        self.name.ends_with('/')
    }

    /// Whether the data is encrypted (and so cannot be read).
    #[must_use]
    pub fn is_encrypted(&self) -> bool {
        self.flags & (FLAG_ENCRYPTED | FLAG_STRONG_ENCRYPTION) != 0
    }
}

/// An archive's table of contents, over the bytes of the whole file.
#[derive(Clone, Debug)]
pub struct Archive<'a> {
    data: &'a [u8],
    entries: Vec<Entry>,
    comment: &'a [u8],
}

impl<'a> Archive<'a> {
    /// Read the table of contents of the ZIP file `data`. Nothing is
    /// decompressed yet; see [`Archive::read`].
    pub fn parse(data: &'a [u8]) -> Result<Self, Error> {
        let eocd_pos = find_eocd(data)?;
        let eocd = Record(&data[eocd_pos..]);
        let comment_len = eocd.u16(20) as usize;
        let comment = slice(data, eocd_pos + EOCD_SIZE, comment_len, "comment")?;

        let this_disk = eocd.u16(4);
        let cd_disk = eocd.u16(6);
        let entries_here = eocd.u16(8);
        let total = eocd.u16(10);
        let cd_size = eocd.u32(12);
        let cd_offset = eocd.u32(16);

        // ZIP64 archives put a locator just before the EOCD and set the
        // fields that overflowed to all ones.
        let locator = eocd_pos
            .checked_sub(20)
            .is_some_and(|p| Record(&data[p..]).u32(0) == ZIP64_LOCATOR_SIGNATURE);
        if locator || total == 0xFFFF || cd_size == u32::MAX || cd_offset == u32::MAX {
            return Err(Error::Zip64);
        }
        if this_disk != 0 || cd_disk != 0 || entries_here != total {
            return Err(Error::MultiDisk);
        }

        // The central directory ends where the EOCD starts. If the EOCD
        // says it starts earlier than that, something was prepended to the
        // archive (a self-extractor stub, a ROM header...) and every offset
        // in the archive is short by that much.
        let cd_size = cd_size as usize;
        let cd_start = eocd_pos
            .checked_sub(cd_size)
            .ok_or(Error::Truncated("central directory"))?;
        let shift = cd_start
            .checked_sub(cd_offset as usize)
            .ok_or(Error::Truncated("central directory"))?;
        let directory = &data[cd_start..eocd_pos];

        let mut entries = Vec::with_capacity((total as usize).min(cd_size / CENTRAL_SIZE));
        let mut pos = 0;
        for _ in 0..total {
            let r = record(
                directory,
                pos,
                CENTRAL_SIZE,
                CENTRAL_SIGNATURE,
                "central directory",
            )?;
            let name_len = r.u16(28) as usize;
            let extra_len = r.u16(30) as usize;
            let comment_len = r.u16(32) as usize;
            let name = slice(directory, pos + CENTRAL_SIZE, name_len, "file name")?;
            let flags = r.u16(8);
            let compressed_size = r.u32(20);
            let size = r.u32(24);
            let local = r.u32(42);
            if compressed_size == u32::MAX || size == u32::MAX || local == u32::MAX {
                return Err(Error::Zip64);
            }
            entries.push(Entry {
                name: decode_name(name, flags & FLAG_UTF8 != 0),
                method: Method::from(r.u16(10)),
                flags,
                crc32: r.u32(16),
                compressed_size,
                size,
                local_header: (local as usize).saturating_add(shift),
            });
            pos += CENTRAL_SIZE + name_len + extra_len + comment_len;
        }
        Ok(Self {
            data,
            entries,
            comment,
        })
    }

    /// Every file and directory, in archive order.
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The archive comment (often empty).
    #[must_use]
    pub fn comment(&self) -> &'a [u8] {
        self.comment
    }

    /// The bytes of an entry as stored in the archive (compressed, for a
    /// deflated entry), found through its local header.
    pub fn compressed_data(&self, entry: &Entry) -> Result<&'a [u8], Error> {
        let local = record(
            self.data,
            entry.local_header,
            LOCAL_SIZE,
            LOCAL_SIGNATURE,
            "local header",
        )?;
        let start =
            entry.local_header + LOCAL_SIZE + local.u16(26) as usize + local.u16(28) as usize;
        slice(
            self.data,
            start,
            entry.compressed_size as usize,
            "file data",
        )
    }

    /// Extract an entry, checking its size and CRC-32.
    pub fn read(&self, entry: &Entry) -> Result<Vec<u8>, Error> {
        if entry.is_encrypted() {
            return Err(Error::Encrypted);
        }
        if entry.size() > MAX_FILE_SIZE {
            return Err(Error::TooLarge(entry.size()));
        }
        let raw = self.compressed_data(entry)?;
        let size = entry.size as usize;
        let data = match entry.method {
            Method::Stored => {
                if raw.len() != size {
                    return Err(Error::WrongSize {
                        expected: entry.size(),
                    });
                }
                raw.to_vec()
            }
            Method::Deflated => {
                // DEFLATE expands at most 1032-fold (a 258-byte match coded
                // in 2 bits), so some declared sizes cannot be true; refuse
                // them before allocating.
                if size > raw.len().saturating_mul(1032) {
                    return Err(Error::WrongSize {
                        expected: entry.size(),
                    });
                }
                // The declared size is both the allocation and the limit:
                // a stream that produces more is damaged (or malicious).
                let mut out = Vec::with_capacity(size);
                match inflate_into::<true>(raw, &mut out, size) {
                    Err(crate::InflateError::OutputTooLarge) => {
                        return Err(Error::WrongSize {
                            expected: entry.size(),
                        });
                    }
                    result => result?,
                }
                if out.len() != size {
                    return Err(Error::WrongSize {
                        expected: entry.size(),
                    });
                }
                out
            }
            Method::Other(m) => return Err(Error::UnsupportedMethod(m)),
        };
        let actual = crc32(&data);
        if actual != entry.crc32 {
            return Err(Error::CrcMismatch {
                expected: entry.crc32,
                actual,
            });
        }
        Ok(data)
    }
}

/// Position of the end of central directory record.
fn find_eocd(data: &[u8]) -> Result<usize, Error> {
    let last = data.len().checked_sub(EOCD_SIZE).ok_or(Error::NotAZip)?;
    let first = last.saturating_sub(u16::MAX as usize);
    (first..=last)
        .rev()
        .find(|&pos| {
            let r = Record(&data[pos..]);
            r.u32(0) == EOCD_SIGNATURE && pos + EOCD_SIZE + r.u16(20) as usize <= data.len()
        })
        .ok_or(if data.starts_with(&LOCAL_SIGNATURE.to_le_bytes()) {
            // It starts like a zip file: most likely an interrupted download.
            Error::Truncated("no end of central directory")
        } else {
            Error::NotAZip
        })
}

/// File names are UTF-8 when flag bit 11 is set, and otherwise in the
/// original IBM PC character set, code page 437 (as MS-DOS wrote them).
fn decode_name(bytes: &[u8], utf8: bool) -> String {
    if utf8 || bytes.is_ascii() {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    bytes
        .iter()
        .map(|&b| {
            if b < 0x80 {
                b as char
            } else {
                CP437_HIGH.chars().nth(b as usize - 0x80).unwrap_or('?')
            }
        })
        .collect()
}

/// Code page 437, bytes 0x80-0xFF.
const CP437_HIGH: &str = "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜ¢£¥₧ƒáíóúñÑªº¿⌐¬½¼¡«»\
░▒▓│┤╡╢╖╕╣║╗╝╜╛┐└┴┬├─┼╞╟╚╔╩╦╠═╬╧╨╤╥╙╘╒╓╫╪┘┌█▄▌▐▀\
αßΓπΣσµτΦΘΩδ∞φε∩≡±≥≤⌠⌡÷≈°∙·√ⁿ²■\u{a0}";

/// File extensions used for Mega Drive / Genesis ROMs.
pub const ROM_EXTENSIONS: [&str; 6] = ["md", "bin", "gen", "smd", "68k", "sgd"];

/// Pick the Mega Drive ROM in an archive: the largest file with a ROM
/// extension ([`ROM_EXTENSIONS`], any case). Directories and the metadata
/// macOS adds to archives (`__MACOSX/` folders, `._` files) are ignored.
#[must_use]
pub fn find_rom<'e>(archive: &'e Archive<'_>) -> Option<&'e Entry> {
    archive
        .entries()
        .iter()
        .filter(|e| is_rom_name(e.name()) && !e.is_dir())
        .fold(None, |best: Option<&Entry>, e| match best {
            // Keep the first of equally large files.
            Some(b) if b.size() >= e.size() => Some(b),
            _ => Some(e),
        })
}

fn is_rom_name(path: &str) -> bool {
    let file = path.rsplit('/').next().unwrap_or(path);
    if path.split('/').any(|p| p == "__MACOSX") || file.starts_with("._") {
        return false;
    }
    file.rsplit_once('.').is_some_and(|(stem, ext)| {
        !stem.is_empty() && ROM_EXTENSIONS.iter().any(|r| r.eq_ignore_ascii_case(ext))
    })
}

#[cfg(test)]
mod tests;
