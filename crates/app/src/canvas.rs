//! A plain pixel buffer and the handful of drawing operations the whole
//! user interface is built from.
//!
//! # Pixels
//!
//! A pixel is a `u32` laid out as `0xAARRGGBB`, the same layout as the
//! console's frames ([`gase_core::Frame`]) and as the streaming textures of
//! SDL (`ARGB8888`), HTML canvases (after a byte swap) and most GPUs. The
//! alpha byte only matters for the [overlay](crate::App::video): the
//! debugger draws opaque pictures and ignores it.
//!
//! Alpha is *straight* (not premultiplied): `0x80FF0000` is red at half
//! opacity. [`Canvas::blend`] composites a colour **over** what is already
//! there with the usual "source over" rule
//!
//! ```text
//! out.a   = src.a + dst.a × (1 − src.a)
//! out.rgb = (src.rgb × src.a + dst.rgb × dst.a × (1 − src.a)) / out.a
//! ```
//!
//! which works both on an opaque picture (then `dst.a` = 1 and it reduces
//! to the familiar `src × a + dst × (1 − a)`) and on a transparent overlay
//! that a GPU will later blend over the game.
//!
//! # Coordinates and clipping
//!
//! Positions are `i32` so that a widget may sit partly off the canvas (a
//! scrolled list, a touch button near the edge): every operation clips to
//! the canvas and to an optional [clip rectangle](Canvas::set_clip). The
//! `usize` helpers ([`Canvas::fill_rect`], [`Canvas::set`]) are kept for
//! the debugger, whose layout never leaves the canvas.

use crate::png;

/// A rectangle in pixels. `w` and `h` may be zero (an empty rectangle).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    #[must_use]
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    /// One past the right edge.
    #[must_use]
    pub const fn right(self) -> i32 {
        self.x + self.w
    }

    /// One past the bottom edge.
    #[must_use]
    pub const fn bottom(self) -> i32 {
        self.y + self.h
    }

    /// The centre, rounded down.
    #[must_use]
    pub const fn center(self) -> (i32, i32) {
        (self.x + self.w / 2, self.y + self.h / 2)
    }

    /// Does the rectangle contain the point?
    #[must_use]
    pub fn contains(self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    /// The part both rectangles cover (possibly empty).
    #[must_use]
    pub fn intersect(self, other: Rect) -> Rect {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let r = self.right().min(other.right());
        let b = self.bottom().min(other.bottom());
        Rect::new(x, y, (r - x).max(0), (b - y).max(0))
    }

    /// Shrink by `n` pixels on every side.
    #[must_use]
    pub fn inset(self, n: i32) -> Rect {
        Rect::new(
            self.x + n,
            self.y + n,
            (self.w - 2 * n).max(0),
            (self.h - 2 * n).max(0),
        )
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.w <= 0 || self.h <= 0
    }
}

/// Opaque version of a colour (alpha `0xFF`).
#[must_use]
pub const fn opaque(rgb: u32) -> u32 {
    rgb | 0xFF00_0000
}

/// `rgb` with the given alpha (0 = transparent, 255 = opaque).
#[must_use]
pub const fn with_alpha(rgb: u32, alpha: u8) -> u32 {
    (rgb & 0x00FF_FFFF) | (alpha as u32) << 24
}

/// Composite `src` over `dst` (both straight-alpha `0xAARRGGBB`).
#[must_use]
#[inline]
pub fn over(src: u32, dst: u32) -> u32 {
    let sa = src >> 24;
    if sa == 0xFF {
        return src;
    }
    if sa == 0 {
        return dst;
    }
    let da = dst >> 24;
    // Everything in 0..=255 fixed point.
    let da_part = (da * (255 - sa) + 127) / 255;
    let out_a = sa + da_part;
    let channel = |shift: u32| {
        let s = (src >> shift) & 0xFF;
        let d = (dst >> shift) & 0xFF;
        (s * sa + d * da_part + out_a / 2) / out_a
    };
    out_a << 24 | channel(16) << 16 | channel(8) << 8 | channel(0)
}

/// A picture. All drawing is clipped to its bounds and to [`Canvas::clip`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Canvas {
    pub width: usize,
    pub height: usize,
    /// Row-major `0xAARRGGBB` pixels.
    pub pixels: Vec<u32>,
    /// Drawing outside this rectangle is discarded.
    clip: Rect,
}

impl Canvas {
    /// A `width` × `height` canvas filled with `color`.
    #[must_use]
    pub fn new(width: usize, height: usize, color: u32) -> Self {
        Self {
            width,
            height,
            pixels: vec![color; width * height],
            clip: Rect::new(0, 0, width as i32, height as i32),
        }
    }

    /// The whole canvas as a rectangle.
    #[must_use]
    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.width as i32, self.height as i32)
    }

    /// Change the size, keeping the allocation when possible. The contents
    /// are undefined afterwards (callers redraw everything).
    pub fn resize(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height;
        self.pixels.resize(width * height, 0);
        self.clip = self.bounds();
    }

    /// Fill the whole canvas, ignoring the clip rectangle.
    pub fn clear(&mut self, color: u32) {
        self.pixels.fill(color);
    }

    /// Restrict drawing to `rect` (`None`: the whole canvas).
    pub fn set_clip(&mut self, rect: Option<Rect>) {
        let bounds = self.bounds();
        self.clip = rect.map_or(bounds, |r| r.intersect(bounds));
    }

    /// The current clip rectangle.
    #[must_use]
    pub fn clip(&self) -> Rect {
        self.clip
    }

    /// The pixel at (`x`, `y`); 0 outside the canvas.
    #[must_use]
    pub fn get(&self, x: usize, y: usize) -> u32 {
        if x < self.width && y < self.height {
            self.pixels[y * self.width + x]
        } else {
            0
        }
    }

    /// Set one pixel (ignored outside the clip rectangle).
    #[inline]
    pub fn set(&mut self, x: usize, y: usize, color: u32) {
        if self.clip.contains(x as i32, y as i32) {
            self.pixels[y * self.width + x] = color;
        }
    }

    /// Set one pixel at signed coordinates (ignored outside the clip).
    #[inline]
    pub fn put(&mut self, x: i32, y: i32, color: u32) {
        if self.clip.contains(x, y) {
            self.pixels[y as usize * self.width + x as usize] = color;
        }
    }

    /// Fill `rect` with `color`, replacing what was there.
    pub fn fill(&mut self, rect: Rect, color: u32) {
        let r = rect.intersect(self.clip);
        if r.is_empty() {
            return;
        }
        for y in r.y..r.bottom() {
            let row = y as usize * self.width;
            self.pixels[row + r.x as usize..row + r.right() as usize].fill(color);
        }
    }

    /// Composite `color` (with its alpha) over `rect`.
    pub fn blend(&mut self, rect: Rect, color: u32) {
        if color >> 24 == 0xFF {
            return self.fill(rect, color);
        }
        let r = rect.intersect(self.clip);
        if r.is_empty() {
            return;
        }
        for y in r.y..r.bottom() {
            let row = y as usize * self.width;
            for p in &mut self.pixels[row + r.x as usize..row + r.right() as usize] {
                *p = over(color, *p);
            }
        }
    }

    /// Fill a rectangle given with `usize` coordinates (debugger helper).
    pub fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: u32) {
        self.fill(Rect::new(x as i32, y as i32, w as i32, h as i32), color);
    }

    /// Draw a one-pixel rectangle outline (debugger helper; opaque).
    pub fn outline(&mut self, x: usize, y: usize, w: usize, h: usize, color: u32) {
        let r = Rect::new(x as i32, y as i32, w as i32, h as i32);
        if r.is_empty() {
            return;
        }
        self.fill(Rect::new(r.x, r.y, r.w, 1), color);
        self.fill(Rect::new(r.x, r.bottom() - 1, r.w, 1), color);
        self.fill(Rect::new(r.x, r.y, 1, r.h), color);
        self.fill(Rect::new(r.right() - 1, r.y, 1, r.h), color);
    }

    /// Draw a rectangle outline `thickness` pixels wide, inside `rect`,
    /// composited.
    pub fn frame(&mut self, rect: Rect, thickness: i32, color: u32) {
        if rect.is_empty() {
            return;
        }
        let t = thickness.min(rect.w).min(rect.h);
        let Rect { x, y, w, h } = rect;
        self.blend(Rect::new(x, y, w, t), color);
        self.blend(Rect::new(x, y + h - t, w, t), color);
        self.blend(Rect::new(x, y + t, t, h - 2 * t), color);
        self.blend(Rect::new(x + w - t, y + t, t, h - 2 * t), color);
    }

    /// Fill a disc of radius `r` centred on (`cx`, `cy`), composited.
    ///
    /// Each row is one horizontal span whose half-width comes from
    /// Pythagoras (`dx² + dy² ≤ r²`), so no pixel is blended twice.
    pub fn disc(&mut self, cx: i32, cy: i32, r: i32, color: u32) {
        self.ring(cx, cy, r, 0, color);
    }

    /// Fill the ring between radii `inner` (exclusive) and `outer`.
    pub fn ring(&mut self, cx: i32, cy: i32, outer: i32, inner: i32, color: u32) {
        for dy in -outer..=outer {
            let half = isqrt(outer * outer - dy * dy);
            let y = cy + dy;
            if inner > 0 && dy.abs() < inner {
                let hole = isqrt(inner * inner - dy * dy);
                self.blend(Rect::new(cx - half, y, half - hole, 1), color);
                self.blend(Rect::new(cx + hole + 1, y, half - hole, 1), color);
            } else {
                self.blend(Rect::new(cx - half, y, 2 * half + 1, 1), color);
            }
        }
    }

    /// Fill a rectangle with rounded corners of radius `r`, composited.
    pub fn rounded(&mut self, rect: Rect, r: i32, color: u32) {
        let r = r.min(rect.w / 2).min(rect.h / 2).max(0);
        for dy in 0..rect.h {
            // Distance into the corner zone, from the top or bottom.
            let edge = dy.min(rect.h - 1 - dy);
            let inset = if edge < r {
                let d = r - edge;
                r - isqrt(r * r - d * d)
            } else {
                0
            };
            self.blend(
                Rect::new(rect.x + inset, rect.y + dy, rect.w - 2 * inset, 1),
                color,
            );
        }
    }

    /// Copy `src` with its top-left corner at (`x`, `y`), keeping only every
    /// `step`-th pixel in each direction (`step` = 1 copies everything; 2
    /// shrinks the image to half size, and so on).
    pub fn blit(&mut self, x: usize, y: usize, src: &Canvas, step: usize) {
        let step = step.max(1);
        for (dy, sy) in (0..src.height).step_by(step).enumerate() {
            for (dx, sx) in (0..src.width).step_by(step).enumerate() {
                self.set(x + dx, y + dy, src.pixels[sy * src.width + sx]);
            }
        }
    }

    /// Draw `src` stretched to fill `dest` with nearest-neighbour sampling.
    /// With `blend` the source alpha is honoured; without, pixels are
    /// copied as opaque.
    pub fn draw_scaled(&mut self, dest: Rect, src: Image<'_>, blend: bool) {
        if dest.is_empty() || src.width == 0 || src.height == 0 {
            return;
        }
        let visible = dest.intersect(self.clip);
        if visible.is_empty() {
            return;
        }
        for y in visible.y..visible.bottom() {
            // Each destination pixel takes the source pixel under its centre.
            let sy = ((2 * (y - dest.y) + 1) as usize * src.height) / (2 * dest.h as usize);
            let src_row = &src.pixels[sy * src.stride..sy * src.stride + src.width];
            let row = y as usize * self.width;
            for x in visible.x..visible.right() {
                let sx = ((2 * (x - dest.x) + 1) as usize * src.width) / (2 * dest.w as usize);
                let p = &mut self.pixels[row + x as usize];
                *p = if blend {
                    over(src_row[sx], *p)
                } else {
                    opaque(src_row[sx])
                };
            }
        }
    }

    /// Borrow as an [`Image`].
    #[must_use]
    pub fn image(&self) -> Image<'_> {
        Image {
            pixels: &self.pixels,
            width: self.width,
            height: self.height,
            stride: self.width,
        }
    }

    /// Encode as PNG (alpha is dropped).
    #[must_use]
    pub fn to_png(&self) -> Vec<u8> {
        png::encode(&self.pixels, self.width, self.height, self.width)
    }
}

/// A borrowed picture: what [`Canvas::draw_scaled`] reads from. Console
/// frames have rows wider than their visible width (`stride`), so this is
/// more general than a [`Canvas`].
#[derive(Clone, Copy, Debug)]
pub struct Image<'a> {
    pub pixels: &'a [u32],
    pub width: usize,
    pub height: usize,
    pub stride: usize,
}

impl<'a> From<gase_core::Frame<'a>> for Image<'a> {
    fn from(f: gase_core::Frame<'a>) -> Self {
        Image {
            pixels: f.pixels,
            width: f.width,
            height: f.height,
            stride: f.stride,
        }
    }
}

/// Integer square root (rounded down) of a non-negative number.
#[must_use]
pub fn isqrt(n: i32) -> i32 {
    if n <= 0 {
        return 0;
    }
    let mut x = f64::from(n).sqrt() as i32;
    // Correct any floating-point rounding at the edges.
    while x * x > n {
        x -= 1;
    }
    while (x + 1) * (x + 1) <= n {
        x += 1;
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangles_are_clipped() {
        let mut c = Canvas::new(4, 3, 0);
        c.fill_rect(2, 1, 10, 10, 7);
        assert_eq!(c.pixels, [0, 0, 0, 0, 0, 0, 7, 7, 0, 0, 7, 7]);
        c.fill_rect(9, 9, 2, 2, 5); // entirely outside: no panic
        c.fill(Rect::new(9, 0, 2, 3), 5); // beside it, overlapping its rows
        c.blend(Rect::new(9, 0, 2, 3), 0x8000_0000);
        c.fill(Rect::new(-5, -5, 6, 6), 9); // partly outside
        assert_eq!(c.pixels[0], 9);
        let mut c = Canvas::new(3, 3, 0);
        c.outline(0, 0, 3, 3, 1);
        assert_eq!(c.pixels, [1, 1, 1, 1, 0, 1, 1, 1, 1]);
    }

    #[test]
    fn clip_rectangle_limits_drawing() {
        let mut c = Canvas::new(4, 4, 0);
        c.set_clip(Some(Rect::new(1, 1, 2, 2)));
        c.fill(c.bounds(), 1);
        assert_eq!(c.pixels.iter().filter(|&&p| p == 1).count(), 4);
        assert_eq!((c.get(0, 0), c.get(1, 1)), (0, 1));
        c.set_clip(None);
        c.put(0, 0, 2);
        assert_eq!(c.get(0, 0), 2);
    }

    #[test]
    fn blit_with_step_shrinks() {
        let mut src = Canvas::new(4, 2, 0);
        src.pixels = vec![1, 2, 3, 4, 5, 6, 7, 8];
        let mut dst = Canvas::new(3, 1, 0);
        dst.blit(1, 0, &src, 2);
        assert_eq!(dst.pixels, [0, 1, 3]);
    }

    #[test]
    fn alpha_compositing() {
        // Half-transparent white over opaque black: mid grey, opaque.
        assert_eq!(over(0x80FF_FFFF, 0xFF00_0000), 0xFF80_8080);
        // Opaque source wins; transparent source changes nothing.
        assert_eq!(over(0xFF12_3456, 0xFF00_0000), 0xFF12_3456);
        assert_eq!(over(0x0012_3456, 0xFFAB_CDEF), 0xFFAB_CDEF);
        // Over a transparent pixel the colour keeps its own value.
        assert_eq!(over(0x80FF_0000, 0x0000_0000), 0x80FF_0000);
        // Two half layers make a three-quarter layer.
        assert_eq!(over(0x8000_0000, 0x8000_0000) >> 24, 0xC0);
    }

    #[test]
    fn discs_and_rings() {
        let mut c = Canvas::new(21, 21, 0);
        c.disc(10, 10, 5, opaque(1));
        // Close to π r² (78.5); the pixel version is a bit more.
        let n = c.pixels.iter().filter(|&&p| p != 0).count();
        assert!((75..=97).contains(&n), "{n}");
        assert_ne!(c.get(10, 5), 0);
        assert_eq!(c.get(10, 4), 0);
        let mut c = Canvas::new(21, 21, 0);
        c.ring(10, 10, 6, 3, opaque(1));
        assert_eq!(c.get(10, 10), 0, "the hole stays empty");
        assert_ne!(c.get(10, 4), 0);
        assert_eq!(isqrt(24), 4);
        assert_eq!(isqrt(25), 5);
    }

    #[test]
    fn scaled_drawing_uses_nearest_pixels() {
        let src = [1u32, 2, 3, 4];
        let image = Image {
            pixels: &src,
            width: 2,
            height: 2,
            stride: 2,
        };
        let mut c = Canvas::new(4, 4, 0);
        c.draw_scaled(Rect::new(0, 0, 4, 4), image, false);
        assert_eq!(c.get(0, 0), opaque(1));
        assert_eq!(c.get(3, 0), opaque(2));
        assert_eq!(c.get(1, 3), opaque(3));
        assert_eq!(c.get(3, 3), opaque(4));
    }
}
