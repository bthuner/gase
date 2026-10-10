//! Minimal writers for PNG screenshots and WAV audio dumps.
//!
//! Both formats are simple enough to write by hand, which keeps the
//! frontend free of image/audio crates. The PNG writer lives in `gase-app`
//! ([`gase_app::png`]), which also uses it for save-state previews.

use std::io::{self, Write};

use gase_core::Frame;

/// Encode the visible part of a frame as PNG.
#[must_use]
pub fn frame_to_png(frame: &Frame<'_>) -> Vec<u8> {
    gase_app::png::encode(frame.pixels, frame.width, frame.height, frame.stride)
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
