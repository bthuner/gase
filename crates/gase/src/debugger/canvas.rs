//! A plain `0x00RRGGBB` pixel buffer that the debugger draws into.
//!
//! Drawing into memory rather than through SDL keeps the debugger views
//! testable and lets the headless runner save them as PNG files.

use crate::media;

/// An RGB image. All drawing is clipped to its bounds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Canvas {
    pub width: usize,
    pub height: usize,
    /// Row-major `0x00RRGGBB` pixels.
    pub pixels: Vec<u32>,
}

impl Canvas {
    /// A `width` × `height` canvas filled with `color`.
    #[must_use]
    pub fn new(width: usize, height: usize, color: u32) -> Self {
        Self {
            width,
            height,
            pixels: vec![color; width * height],
        }
    }

    /// The pixel at (`x`, `y`); 0 outside the canvas.
    #[cfg(test)]
    #[must_use]
    pub fn get(&self, x: usize, y: usize) -> u32 {
        if x < self.width && y < self.height {
            self.pixels[y * self.width + x]
        } else {
            0
        }
    }

    /// Set one pixel (ignored outside the canvas).
    #[inline]
    pub fn set(&mut self, x: usize, y: usize, color: u32) {
        if x < self.width && y < self.height {
            self.pixels[y * self.width + x] = color;
        }
    }

    /// Fill a rectangle.
    pub fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: u32) {
        let (x_end, y_end) = ((x + w).min(self.width), (y + h).min(self.height));
        for row in y.min(y_end)..y_end {
            self.pixels[row * self.width + x.min(x_end)..row * self.width + x_end].fill(color);
        }
    }

    /// Draw a one-pixel rectangle outline.
    pub fn outline(&mut self, x: usize, y: usize, w: usize, h: usize, color: u32) {
        if w == 0 || h == 0 {
            return;
        }
        self.fill_rect(x, y, w, 1, color);
        self.fill_rect(x, y + h - 1, w, 1, color);
        self.fill_rect(x, y, 1, h, color);
        self.fill_rect(x + w - 1, y, 1, h, color);
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

    /// Encode as PNG.
    #[must_use]
    pub fn to_png(&self) -> Vec<u8> {
        media::encode_png(&self.pixels, self.width, self.height, self.width)
    }
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
        let mut c = Canvas::new(3, 3, 0);
        c.outline(0, 0, 3, 3, 1);
        assert_eq!(c.pixels, [1, 1, 1, 1, 0, 1, 1, 1, 1]);
    }

    #[test]
    fn blit_with_step_shrinks() {
        let mut src = Canvas::new(4, 2, 0);
        src.pixels = vec![1, 2, 3, 4, 5, 6, 7, 8];
        let mut dst = Canvas::new(3, 1, 0);
        dst.blit(1, 0, &src, 2);
        assert_eq!(dst.pixels, [0, 1, 3]);
    }
}
