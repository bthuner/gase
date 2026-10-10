//! Pictures for a `<canvas>`: pixel format conversion and the frame
//! description the page reads after each frame.
//!
//! # Pixel formats
//!
//! The app and the core store a pixel as one `u32`, `0xAARRGGBB`. A
//! canvas' `ImageData` wants four *bytes* per pixel in the order R, G, B,
//! A. WebAssembly is little-endian, so a `u32` is stored lowest byte
//! first: `0xAABBGGRR` lands in memory as R, G, B, A. Converting is
//! therefore a swap of the red and blue bytes, done here while copying
//! into a buffer the page wraps without copying:
//!
//! ```text
//!   wasm memory: [ R G B A | R G B A | … ]   ← rgba: Vec<u32>, 0xAABBGGRR
//!                 ▲
//!   JS: new ImageData(new Uint8ClampedArray(memory.buffer, ptr, w*h*4), w, h)
//! ```
//!
//! (`ImageData` copies nothing either: `putImageData` reads straight out of
//! the WebAssembly memory into the canvas.) The console's pictures carry
//! no alpha (the top byte is 0), so the game picture is made opaque; the
//! overlay keeps its alpha, straight (not premultiplied), as canvases
//! expect.
//!
//! # The frame description
//!
//! After [`Shell::update`](crate::Shell::update) the page needs a dozen
//! numbers: where the pictures are in memory, their sizes, where to draw
//! them, how to pace the next frame. Rather than a dozen exported
//! getters, the shell writes them into a small `[u32; INFO_LEN]` and
//! the page reads them all through one `Uint32Array` view, the way C
//! programs share a struct. The indices below are mirrored in
//! `web/gase.js` (`INFO`); keep both in sync.

use gase_app::{Image, Pacing};

/// Indices into the frame description.
pub mod info {
    /// Bit flags: [`HAS_GAME`](super::HAS_GAME), [`HAS_OVERLAY`](super::HAS_OVERLAY),
    /// [`OVERLAY_CHANGED`](super::OVERLAY_CHANGED).
    pub const FLAGS: usize = 0;
    /// Address of the game picture (RGBA bytes) in wasm memory.
    pub const GAME_PTR: usize = 1;
    pub const GAME_W: usize = 2;
    pub const GAME_H: usize = 3;
    /// Where to draw the game picture, in physical screen pixels (`i32`s).
    pub const RECT_X: usize = 4;
    pub const RECT_Y: usize = 5;
    pub const RECT_W: usize = 6;
    pub const RECT_H: usize = 7;
    /// Address and size of the overlay (RGBA bytes with alpha).
    pub const OVERLAY_PTR: usize = 8;
    pub const OVERLAY_W: usize = 9;
    pub const OVERLAY_H: usize = 10;
    /// Draw the overlay enlarged this many times, from the top-left corner.
    pub const OVERLAY_SCALE: usize = 11;
    /// `0xRRGGBB` for the screen around the pictures.
    pub const BACKGROUND: usize = 12;
    /// How to pace: [`PACE_TIMER`](super::PACE_TIMER),
    /// [`PACE_AUDIO`](super::PACE_AUDIO) or
    /// [`PACE_UNTHROTTLED`](super::PACE_UNTHROTTLED).
    pub const PACE: usize = 13;
    /// Timer: frames per second × 1000. Audio: the queue level to keep, in
    /// stereo sample frames.
    pub const PACE_VALUE: usize = 14;
    /// Frames emulated since the game was opened (for the page's fps meter).
    pub const FRAME: usize = 15;
}
/// Number of `u32`s in the frame description.
pub const INFO_LEN: usize = 16;

pub const HAS_GAME: u32 = 1;
pub const HAS_OVERLAY: u32 = 2;
pub const OVERLAY_CHANGED: u32 = 4;

pub const PACE_TIMER: u32 = 0;
pub const PACE_AUDIO: u32 = 1;
pub const PACE_UNTHROTTLED: u32 = 2;

/// Encode a [`Pacing`] as (kind, value) for the frame description.
#[must_use]
pub fn encode_pacing(pacing: Pacing) -> (u32, u32) {
    match pacing {
        Pacing::Timer { fps } => (PACE_TIMER, (fps * 1000.0).round() as u32),
        Pacing::Audio { target } => (PACE_AUDIO, target as u32),
        Pacing::Unthrottled => (PACE_UNTHROTTLED, 0),
    }
}

/// `0xAARRGGBB` → `0xAABBGGRR` (bytes R, G, B, A in little-endian memory).
#[inline]
#[must_use]
pub const fn rgba(argb: u32) -> u32 {
    (argb & 0xFF00_FF00) | ((argb >> 16) & 0xFF) | ((argb & 0xFF) << 16)
}

/// Copy `image` into `out` (resized to fit, rows packed without padding)
/// as canvas RGBA. `opaque` forces alpha to 255.
pub fn to_rgba(image: &Image<'_>, out: &mut Vec<u32>, opaque: bool) {
    let alpha = if opaque { 0xFF00_0000 } else { 0 };
    out.clear();
    out.reserve(image.width * image.height);
    for row in image.pixels.chunks(image.stride).take(image.height) {
        out.extend(row[..image.width].iter().map(|&p| rgba(p) | alpha));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn red_and_blue_swap() {
        assert_eq!(rgba(0x8011_2233), 0x8033_2211);
        // As bytes in memory (little-endian): R G B A.
        assert_eq!(rgba(0xFF11_2233).to_le_bytes(), [0x11, 0x22, 0x33, 0xFF]);
    }

    #[test]
    fn packs_rows_and_sets_alpha() {
        // A 2×2 picture inside rows of 3 (stride), console style (alpha 0).
        let pixels = [
            0x0011_2233,
            0x0044_5566,
            0xDEAD,
            0x0077_8899,
            0x00AA_BBCC,
            0xBEEF,
        ];
        let image = Image {
            pixels: &pixels,
            width: 2,
            height: 2,
            stride: 3,
        };
        let mut out = vec![1, 2, 3, 4, 5, 6, 7];
        to_rgba(&image, &mut out, true);
        assert_eq!(out, [0xFF33_2211, 0xFF66_5544, 0xFF99_8877, 0xFFCC_BBAA]);
        to_rgba(&image, &mut out, false);
        assert_eq!(out[0], 0x0033_2211);
    }

    #[test]
    fn pacing_codes() {
        assert_eq!(
            encode_pacing(Pacing::Timer { fps: 59.922 }),
            (PACE_TIMER, 59922)
        );
        assert_eq!(
            encode_pacing(Pacing::Audio { target: 2400 }),
            (PACE_AUDIO, 2400)
        );
        assert_eq!(encode_pacing(Pacing::Unthrottled), (PACE_UNTHROTTLED, 0));
    }

    #[test]
    fn info_indices_are_distinct_and_in_range() {
        let all = [
            info::FLAGS,
            info::GAME_PTR,
            info::GAME_W,
            info::GAME_H,
            info::RECT_X,
            info::RECT_Y,
            info::RECT_W,
            info::RECT_H,
            info::OVERLAY_PTR,
            info::OVERLAY_W,
            info::OVERLAY_H,
            info::OVERLAY_SCALE,
            info::BACKGROUND,
            info::PACE,
            info::PACE_VALUE,
            info::FRAME,
        ];
        for (i, &a) in all.iter().enumerate() {
            assert!(a < INFO_LEN);
            assert!(all[i + 1..].iter().all(|&b| b != a));
        }
    }
}
