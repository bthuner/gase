//! A minimal PNG writer, and a reader for the PNGs it writes.
//!
//! PNG is simple enough to write by hand, which keeps gase free of image
//! crates: a signature, then *chunks* (`length`, `type`, `data`, `CRC-32`).
//! `IHDR` gives the size and pixel format, `IDAT` holds the pixels as a
//! zlib stream, `IEND` ends the file. Each row of pixels is preceded by a
//! *filter* byte (0 = none).
//!
//! The zlib stream uses uncompressed ("stored") deflate blocks: files are
//! bigger than necessary but valid everywhere, and the code fits on a page.
//!
//! The reader ([`decode`]) understands exactly that subset (8-bit RGB, no
//! interlacing, stored blocks, filter 0). gase uses it for the little
//! previews of save states, which it wrote itself. Any other PNG is
//! rejected with `None` and the menu simply shows no preview.

/// CRC-32 (ISO 3309), as used by PNG chunks.
#[must_use]
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// Adler-32, the zlib stream checksum.
#[must_use]
pub fn adler32(data: &[u8]) -> u32 {
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

const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// Encode an RGB image as PNG. `pixels` are `0x??RRGGBB` (alpha ignored),
/// rows `stride` pixels apart.
#[must_use]
pub fn encode(pixels: &[u32], width: usize, height: usize, stride: usize) -> Vec<u8> {
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
    if blocks.is_empty() {
        zlib.extend_from_slice(&[1, 0, 0, 0xFF, 0xFF]);
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut png = SIGNATURE.to_vec();
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(width as u32).to_be_bytes());
    ihdr.extend_from_slice(&(height as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit RGB, no interlace
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &zlib);
    chunk(&mut png, b"IEND", &[]);
    png
}

/// Decode a PNG written by [`encode`]: returns `(width, height, pixels)`
/// with opaque `0xFFRRGGBB` pixels, or `None` for anything else.
#[must_use]
pub fn decode(data: &[u8]) -> Option<(usize, usize, Vec<u32>)> {
    let mut rest = data.strip_prefix(SIGNATURE)?;
    let mut size = None;
    let mut zlib = Vec::new();
    // Walk the chunks.
    while rest.len() >= 12 {
        let len = u32::from_be_bytes(rest[..4].try_into().ok()?) as usize;
        let kind = &rest[4..8];
        let body = rest.get(8..8 + len)?;
        let crc = u32::from_be_bytes(rest.get(8 + len..12 + len)?.try_into().ok()?);
        if crc32(&rest[4..8 + len]) != crc {
            return None;
        }
        match kind {
            b"IHDR" => {
                let w = u32::from_be_bytes(body.get(..4)?.try_into().ok()?) as usize;
                let h = u32::from_be_bytes(body.get(4..8)?.try_into().ok()?) as usize;
                if body.get(8..13)? != [8, 2, 0, 0, 0] || w > 4096 || h > 4096 {
                    return None;
                }
                size = Some((w, h));
            }
            b"IDAT" => zlib.extend_from_slice(body),
            b"IEND" => break,
            _ => {}
        }
        rest = &rest[12 + len..];
    }
    let (width, height) = size?;

    // The zlib header, then stored blocks: BFINAL/BTYPE byte, LEN, NLEN.
    let mut z = zlib.get(2..)?;
    let mut raw = Vec::with_capacity(height * (width * 3 + 1));
    loop {
        let (&header, after) = z.split_first()?;
        if header & 0b110 != 0 {
            return None; // compressed block: not ours
        }
        let len = usize::from(u16::from_le_bytes(after.get(..2)?.try_into().ok()?));
        raw.extend_from_slice(after.get(4..4 + len)?);
        z = &after[4 + len..];
        if header & 1 != 0 {
            break;
        }
    }
    if raw.len() != height * (width * 3 + 1)
        || adler32(&raw) != u32::from_be_bytes(z.get(..4)?.try_into().ok()?)
    {
        return None;
    }
    let mut pixels = Vec::with_capacity(width * height);
    for row in raw.chunks_exact(width * 3 + 1) {
        if row[0] != 0 {
            return None; // a filtered row: not ours
        }
        pixels.extend(
            row[1..].chunks_exact(3).map(|p| {
                0xFF00_0000 | u32::from(p[0]) << 16 | u32::from(p[1]) << 8 | u32::from(p[2])
            }),
        );
    }
    Some((width, height, pixels))
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
        let png = encode(&[0xFF0000, 0x00FF00], 2, 1, 2);
        assert_eq!(&png[..8], SIGNATURE);
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
    }

    #[test]
    fn round_trip_including_several_deflate_blocks() {
        // 200 × 120 RGB is 72 120 bytes of scanlines: two stored blocks.
        let (w, h) = (200, 120);
        let pixels: Vec<u32> = (0..w * h)
            .map(|i| (i as u32).wrapping_mul(2_654_435_761) & 0xFF_FFFF)
            .collect();
        let png = encode(&pixels, w, h, w);
        let (dw, dh, decoded) = decode(&png).unwrap();
        assert_eq!((dw, dh), (w, h));
        assert!(
            decoded
                .iter()
                .zip(&pixels)
                .all(|(d, p)| *d == p | 0xFF00_0000)
        );
    }

    #[test]
    fn foreign_or_damaged_files_are_rejected() {
        let mut png = encode(&[1, 2, 3, 4], 2, 2, 2);
        assert!(decode(&png).is_some());
        assert!(decode(b"not a png").is_none());
        let last = png.len() - 20;
        png[last] ^= 0xFF; // breaks a CRC
        assert!(decode(&png).is_none());
        assert!(decode(&png[..30]).is_none());
    }
}
