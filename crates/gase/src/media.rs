//! Minimal writers for PNG screenshots and WAV audio dumps.
//!
//! Both formats are simple enough to write by hand, which keeps the
//! frontend free of image/audio crates. The PNG writer uses uncompressed
//! ("stored") deflate blocks: files are bigger than necessary but valid
//! everywhere.

use std::io::{self, Write};

use gase_core::Frame;

/// CRC-32 (ISO 3309), as used by PNG chunks.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// Adler-32, the zlib stream checksum.
fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for &byte in chunk {
            a += u32::from(byte);
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// Encode an RGB image as PNG. `pixels` are `0x00RRGGBB`.
#[must_use]
pub fn encode_png(pixels: &[u32], width: usize, height: usize, stride: usize) -> Vec<u8> {
    // Raw scanlines: filter type 0 then RGB triples.
    let mut raw = Vec::with_capacity(height * (width * 3 + 1));
    for row in pixels.chunks(stride).take(height) {
        raw.push(0);
        for &p in &row[..width] {
            raw.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
        }
    }

    // zlib stream made of stored deflate blocks (max 65535 bytes each).
    let mut zlib = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65535).collect();
    for (i, block) in blocks.iter().enumerate() {
        zlib.push(u8::from(i + 1 == blocks.len()));
        let len = block.len() as u16;
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(width as u32).to_be_bytes());
    ihdr.extend_from_slice(&(height as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit RGB, no interlace
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &zlib);
    chunk(&mut png, b"IEND", &[]);
    png
}

/// Encode the visible part of a frame as PNG.
#[must_use]
pub fn frame_to_png(frame: &Frame<'_>) -> Vec<u8> {
    encode_png(frame.pixels, frame.width, frame.height, frame.stride)
}

/// Streams 16-bit stereo PCM into a WAV file.
#[derive(Debug)]
pub struct WavWriter<W: Write + io::Seek> {
    out: W,
    samples: u32,
}

impl<W: Write + io::Seek> WavWriter<W> {
    pub fn new(mut out: W, sample_rate: u32) -> io::Result<Self> {
        let channels = 2u16;
        let block_align = channels * 2;
        out.write_all(b"RIFF\0\0\0\0WAVEfmt ")?;
        out.write_all(&16u32.to_le_bytes())?;
        out.write_all(&1u16.to_le_bytes())?; // PCM
        out.write_all(&channels.to_le_bytes())?;
        out.write_all(&sample_rate.to_le_bytes())?;
        out.write_all(&(sample_rate * u32::from(block_align)).to_le_bytes())?;
        out.write_all(&block_align.to_le_bytes())?;
        out.write_all(&16u16.to_le_bytes())?;
        out.write_all(b"data\0\0\0\0")?;
        Ok(Self { out, samples: 0 })
    }

    /// Append interleaved stereo samples.
    pub fn write(&mut self, samples: &[i16]) -> io::Result<()> {
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        self.out.write_all(&bytes)?;
        self.samples += samples.len() as u32;
        Ok(())
    }

    /// Fill in the chunk sizes.
    pub fn finish(mut self) -> io::Result<()> {
        let data_len = self.samples * 2;
        self.out.seek(io::SeekFrom::Start(4))?;
        self.out.write_all(&(36 + data_len).to_le_bytes())?;
        self.out.seek(io::SeekFrom::Start(40))?;
        self.out.write_all(&data_len.to_le_bytes())?;
        self.out.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn png_layout() {
        let png = encode_png(&[0xFF0000, 0x00FF00], 2, 1, 2);
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
    }

    #[test]
    fn wav_header_sizes() {
        let mut cursor = io::Cursor::new(Vec::new());
        let mut wav = WavWriter::new(&mut cursor, 48_000).unwrap();
        wav.write(&[1, 2, 3, 4]).unwrap();
        wav.finish().unwrap();
        let bytes = cursor.into_inner();
        assert_eq!(bytes.len(), 44 + 8);
        assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), 8);
    }
}
