//! Where things go on the screen: the size of a UI pixel, and the
//! rectangle the game picture is drawn in.
//!
//! # Two layers, two resolutions
//!
//! The shell draws two pictures on top of each other:
//!
//! 1. **The game**, a 320 × 224 (or 256 × 224, or interlaced 448-line)
//!    frame, stretched by the GPU into [`game_rect`].
//! 2. **The overlay** — menus, on-screen controls, messages — drawn by the
//!    app into a small canvas and enlarged by a whole number, the
//!    [UI scale](ui_scale), with nearest-neighbour filtering. Every font
//!    pixel becomes a crisp `scale × scale` block, which suits an 8×8
//!    bitmap font and the console's look, and the app only ever draws a
//!    few hundred thousand pixels however big the screen is.
//!
//! The UI scale has to satisfy two people: the one with a 4K monitor (the
//! menu should not be a stamp in the middle) and the one with a phone
//! (a button must be big enough for a thumb). So it is the larger of
//! "fit about 400 × 260 UI pixels on the screen" and "a UI pixel is at
//! least 1.5 points" (a point being the platform's density-independent
//! unit, see [`Event::Resized`](crate::Event::Resized)).

use crate::canvas::Rect;
use crate::settings::Aspect;

/// The smallest overlay we aim for, in UI pixels.
pub const MIN_UI: (u32, u32) = (400, 260);

/// How many screen pixels one UI pixel covers.
#[must_use]
pub fn ui_scale(width: u32, height: u32, pixels_per_point: f32) -> i32 {
    let fit = (width / MIN_UI.0).min(height / MIN_UI.1) as i32;
    let readable = (1.5 * pixels_per_point.max(0.5)).round() as i32;
    fit.max(readable).max(1)
}

/// Where to draw a game picture of `frame` size (width, height in console
/// pixels) inside `area` (screen pixels).
///
/// The console's pixels are not square: whatever the mode, the picture
/// covers the same area of a television. With [`Aspect::Console`] it fills
/// a 320 × 224 shape (10:7; 320 × 240 for 240-line PAL modes), so 256-pixel
/// modes are widened, as on a TV. Interlaced frames have twice the lines
/// but the same shape.
///
/// With `integer`, the scale is rounded down to a whole number of screen
/// pixels per line, so every line is equally tall (the sharpest result).
/// `top` places the picture at the top of the area instead of centring it
/// (portrait phones, where the controls go below).
#[must_use]
pub fn game_rect(
    area: Rect,
    frame: (usize, usize),
    aspect: Aspect,
    integer: bool,
    top: bool,
) -> Rect {
    let lines = if frame.1 > 240 { frame.1 / 2 } else { frame.1 } as f64;
    let (aw, ah) = (f64::from(area.w.max(1)), f64::from(area.h.max(1)));
    let base_w = match aspect {
        Aspect::Console => 320.0,
        Aspect::Tv => lines * 4.0 / 3.0,
        Aspect::Stretch => return area,
    };
    let mut scale = (aw / base_w).min(ah / lines);
    if integer && scale >= 1.0 {
        scale = scale.floor();
    }
    let w = ((base_w * scale).round() as i32).clamp(1, area.w.max(1));
    let h = ((lines * scale).round() as i32).clamp(1, area.h.max(1));
    let y = if top {
        area.y
    } else {
        area.y + (area.h - h) / 2
    };
    Rect::new(area.x + (area.w - w) / 2, y, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letterboxing_and_integer_scaling() {
        let window = Rect::new(0, 0, 1024, 768);
        // A 4:3 window shows the 10:7 picture with bars above and below.
        let r = game_rect(window, (320, 224), Aspect::Console, false, false);
        assert_eq!((r.w, r.x), (1024, 0));
        assert!(r.y > 0);
        let r = game_rect(
            Rect::new(0, 0, 1000, 700),
            (320, 224),
            Aspect::Console,
            true,
            false,
        );
        assert_eq!((r.w, r.h), (960, 672));
        // 256-pixel and interlaced modes get the same shape.
        let a = game_rect(window, (256, 224), Aspect::Console, false, false);
        let b = game_rect(window, (320, 448), Aspect::Console, false, false);
        assert_eq!(a, b);
        // 4:3 and stretch.
        let r = game_rect(window, (320, 224), Aspect::Tv, false, false);
        assert_eq!((r.w, r.h), (1024, 768));
        assert_eq!(
            game_rect(window, (320, 224), Aspect::Stretch, true, false),
            window
        );
        // Top-anchored for portrait screens.
        let r = game_rect(
            Rect::new(0, 0, 1080, 2340),
            (320, 224),
            Aspect::Console,
            false,
            true,
        );
        assert_eq!((r.y, r.w), (0, 1080));
    }

    #[test]
    fn ui_scale_fits_and_stays_readable() {
        assert_eq!(ui_scale(960, 672, 1.0), 2);
        assert_eq!(ui_scale(1920, 1080, 1.0), 4);
        assert_eq!(ui_scale(640, 448, 1.0), 2);
        assert_eq!(ui_scale(300, 200, 1.0), 2);
        // A phone in portrait: the density decides.
        assert_eq!(ui_scale(1080, 2340, 3.0), 5);
    }
}
