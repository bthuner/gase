//! Converting CRAM colours to RGB.
//!
//! A CRAM entry holds 3 bits per channel: `0000BBB0GGG0RRR0`. The VDP's
//! digital-to-analogue converter is not linear, and shadow/highlight mode
//! adds two more intensity sets. The levels below were measured on real
//! hardware and are the ones commonly used by accurate emulators.

/// Output brightness of a pixel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Intensity {
    Shadow = 0,
    Normal = 1,
    Highlight = 2,
}

impl Intensity {
    /// The next brighter intensity (used by highlight operator sprites).
    pub(crate) fn brighter(self) -> Self {
        match self {
            Intensity::Shadow => Intensity::Normal,
            Intensity::Normal | Intensity::Highlight => Intensity::Highlight,
        }
    }
}

const LEVELS: [[u8; 8]; 3] = [
    // Shadow: roughly half brightness.
    [0, 29, 52, 70, 87, 101, 116, 130],
    // Normal.
    [0, 52, 87, 116, 144, 172, 206, 255],
    // Highlight: half brightness plus a half-scale offset.
    [130, 144, 158, 172, 187, 206, 228, 255],
];

/// Pre-converted RGB values for all 64 CRAM entries in all 3 intensities.
///
/// Rebuilt entry by entry whenever the CPU writes CRAM, so rendering is a
/// plain table lookup.
#[derive(Clone, Debug)]
pub(crate) struct Palette {
    rgb: [[u32; 64]; 3],
}

impl Palette {
    pub(crate) fn new() -> Self {
        Self { rgb: [[0; 64]; 3] }
    }

    pub(crate) fn rebuild(&mut self, cram: &[u16; 64]) {
        for (index, &entry) in cram.iter().enumerate() {
            self.update(index, entry);
        }
    }

    pub(crate) fn update(&mut self, index: usize, entry: u16) {
        let r = usize::from((entry >> 1) & 7);
        let g = usize::from((entry >> 5) & 7);
        let b = usize::from((entry >> 9) & 7);
        for (intensity, levels) in LEVELS.iter().enumerate() {
            self.rgb[intensity][index] =
                u32::from(levels[r]) << 16 | u32::from(levels[g]) << 8 | u32::from(levels[b]);
        }
    }

    #[inline]
    pub(crate) fn get(&self, index: u8, intensity: Intensity) -> u32 {
        self.rgb[intensity as usize][usize::from(index & 0x3F)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn white_and_black() {
        let mut p = Palette::new();
        p.update(0, 0x0000);
        p.update(1, 0x0EEE);
        p.update(2, 0x000E);
        assert_eq!(p.get(0, Intensity::Normal), 0x000000);
        assert_eq!(p.get(1, Intensity::Normal), 0xFFFFFF);
        assert_eq!(p.get(2, Intensity::Normal), 0xFF0000);
        assert_eq!(p.get(1, Intensity::Shadow), 0x828282);
    }
}
