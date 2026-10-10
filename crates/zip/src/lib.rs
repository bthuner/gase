//! Reading ZIP archives, with a DEFLATE decoder written to be read.
//!
//! ROMs are often distributed zipped, so gase opens `.zip` files directly.
//! This crate does that without dependencies, and doubles as a guided tour
//! of two formats everyone uses and few people have looked inside:
//!
//! * [`archive`]: the ZIP container, a list of files with a table of
//!   contents at the *end*;
//! * [`inflate`](mod@inflate): DEFLATE (RFC 1951), the compression inside almost every
//!   ZIP, gzip and PNG file: LZ77 back-references plus Huffman coding;
//! * [`crc32`]: the checksum that proves the extracted file is intact.
//!
//! ```
//! # fn main() -> Result<(), gase_zip::Error> {
//! # let bytes = include_bytes!("../tests/fixtures/several.zip");
//! use gase_zip::Archive;
//!
//! let archive = Archive::parse(bytes)?;
//! for entry in archive.entries() {
//!     println!("{:>8} {}", entry.size(), entry.name());
//! }
//! let rom = gase_zip::find_rom(&archive).expect("a ROM in the archive");
//! let data = archive.read(rom)?;
//! assert_eq!(data.len() as u64, rom.size());
//! # Ok(())
//! # }
//! ```
//!
//! # Scope
//!
//! Enough of ZIP for ROM archives as people actually make them: methods 0
//! (stored) and 8 (deflate), archive comments, data descriptors, data
//! prepended to the archive (self-extractors). Not supported, with a clear
//! [`Error`]: ZIP64 (archives or files over 4 GiB), encryption, archives
//! split over several files, and other compression methods (bzip2, LZMA,
//! ...). Files are limited to [`MAX_FILE_SIZE`] (64 MiB) so a malicious
//! archive cannot make the emulator allocate gigabytes.

pub mod archive;
pub mod crc32;
pub mod inflate;

#[cfg(test)]
mod test_util;

use std::fmt;

pub use archive::{Archive, Entry, Method, find_rom};
pub use inflate::{InflateError, inflate, inflate_reference};

/// The largest file [`Archive::read`] extracts: 64 MiB.
pub const MAX_FILE_SIZE: u64 = inflate::DEFAULT_MAX_OUTPUT as u64;

/// Why an archive could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// No "end of central directory" record: not a ZIP file.
    NotAZip,
    /// A structure extends past the end of the file.
    Truncated(&'static str),
    /// A structure does not start with its signature (wrong offset or
    /// damaged file).
    BadSignature(&'static str),
    /// The archive uses ZIP64 extensions (files or archives over 4 GiB).
    Zip64,
    /// The archive is split over several files ("disks").
    MultiDisk,
    /// The file is encrypted.
    Encrypted,
    /// The file uses a compression method other than stored or deflate.
    UnsupportedMethod(u16),
    /// The file is larger than [`MAX_FILE_SIZE`].
    TooLarge(u64),
    /// The extracted data is not the size the archive says it is.
    WrongSize { expected: u64 },
    /// The compressed data is invalid.
    Inflate(InflateError),
    /// The extracted data does not have the CRC-32 the archive says it has.
    CrcMismatch { expected: u32, actual: u32 },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotAZip => f.write_str("not a zip archive (no end of central directory)"),
            Error::Truncated(what) => write!(f, "archive is truncated ({what})"),
            Error::BadSignature(what) => write!(f, "archive is damaged (bad {what} signature)"),
            Error::Zip64 => f.write_str("ZIP64 archives are not supported"),
            Error::MultiDisk => f.write_str("archives split over several files are not supported"),
            Error::Encrypted => f.write_str("file is encrypted"),
            Error::UnsupportedMethod(m) => {
                write!(
                    f,
                    "unsupported compression method {m} (only stored and deflate)"
                )
            }
            Error::TooLarge(n) => write!(f, "file is too large ({n} bytes)"),
            Error::WrongSize { expected } => {
                write!(f, "extracted data is not the declared {expected} bytes")
            }
            Error::Inflate(e) => write!(f, "compressed data is damaged: {e}"),
            Error::CrcMismatch { expected, actual } => write!(
                f,
                "checksum mismatch (CRC-32 {actual:08x}, expected {expected:08x})"
            ),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Inflate(e) => Some(e),
            _ => None,
        }
    }
}

impl From<InflateError> for Error {
    fn from(e: InflateError) -> Self {
        Error::Inflate(e)
    }
}
