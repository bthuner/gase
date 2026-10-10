//! Access slots: *when* the CPU side may touch VDP memory.
//!
//! # Why the VDP cannot accept writes at any time
//!
//! While it draws a line the VDP is busy reading VRAM almost continuously:
//! name table entries for planes A and B, the tile patterns they point to,
//! the horizontal scroll value, sprite attributes and sprite patterns. VRAM
//! has a single port, so the chip divides each line into a fixed sequence of
//! memory *slots* and gives most of them to the renderer. Only a few,
//! called **external access slots**, are left for the outside world: the
//! 68000 (through the write FIFO, see `fifo`) and the DMA engine (`dma`).
//! A handful more are *refresh* slots, used by nobody but the DRAM refresh.
//!
//! | mode | display active | blanking or display off |
//! |------|----------------|-------------------------|
//! | H40 (320 px) | 18 slots per line | 205 slots per line |
//! | H32 (256 px) | 16 slots per line | 167 slots per line |
//!
//! During active display the external slots come one every 16 pixels, except
//! that every fourth such position is a refresh slot, plus a few in
//! horizontal blanking. When the VDP is not drawing (vertical blanking, or
//! the whole frame when register 1's display bit is clear) it does not fetch
//! anything, so every slot except refresh becomes external: roughly one slot
//! every 2 pixels.
//!
//! This is the single most important fact about Mega Drive video bandwidth.
//! A word written to VRAM needs *two* slots (VRAM is accessed a byte at a
//! time on this path), so during active display in H40 the VDP absorbs only
//! about 9 VRAM words per line, against about 102 during blanking. That is
//! why games do their big transfers in vertical blanking, and why they turn
//! the display off while loading a level: with the display off the whole
//! frame runs at blanking speed.
//!
//! # Model
//!
//! Positions are expressed in master clocks from the start of the line, the
//! same unit the scheduler uses (3420 per line). The exact slot positions of
//! the real chip depend on its pixel clock, which in H40 is not even
//! constant across a line; the tables below follow the documented
//! *structure* (one slot every 16 pixels with every fourth missing, the rest
//! in horizontal blanking; evenly spread slots during blanking) and get the
//! documented *counts* exactly, which is what determines throughput.

use crate::timing::MASTER_CYCLES_PER_LINE as LINE;

/// External slots of an active H40 line. The display area (master clocks
/// 40-2650 here) has a slot every 128 clocks (16 pixels at 8 clocks per
/// pixel) with every fourth replaced by refresh: 15 slots. Three more fall
/// in horizontal blanking.
const H40_ACTIVE: [u32; 18] = [
    104, 232, 360, 616, 744, 872, 1128, 1256, 1384, 1640, 1768, 1896, 2152, 2280, 2408, 2690, 2950,
    3210,
];

/// External slots of an active H32 line: every 160 clocks (16 pixels at 10
/// clocks per pixel) with every fourth missing, 12 slots, plus four in
/// horizontal blanking.
const H32_ACTIVE: [u32; 16] = [
    120, 280, 440, 760, 920, 1080, 1400, 1560, 1720, 2040, 2200, 2360, 2690, 2870, 3050, 3230,
];

/// External slots per line while the VDP is not drawing: every slot but the
/// refresh ones.
const H40_BLANK: u32 = 205;
const H32_BLANK: u32 = 167;

/// Which slot pattern the current line follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SlotMode {
    H40Active,
    H32Active,
    H40Blank,
    H32Blank,
}

impl SlotMode {
    pub(crate) fn new(h40: bool, blanking: bool) -> Self {
        match (h40, blanking) {
            (true, false) => SlotMode::H40Active,
            (false, false) => SlotMode::H32Active,
            (true, true) => SlotMode::H40Blank,
            (false, true) => SlotMode::H32Blank,
        }
    }

    /// Number of external slots in one line.
    pub(crate) fn per_line(self) -> u32 {
        match self {
            SlotMode::H40Active => H40_ACTIVE.len() as u32,
            SlotMode::H32Active => H32_ACTIVE.len() as u32,
            SlotMode::H40Blank => H40_BLANK,
            SlotMode::H32Blank => H32_BLANK,
        }
    }

    /// The first slot at or after `pos` (master clocks from the start of the
    /// current line). `pos` may lie beyond the end of the line: lines repeat
    /// with the same pattern, which is how a CPU stall that crosses into the
    /// next line is handled.
    #[inline]
    pub(crate) fn next_slot(self, pos: u32) -> u32 {
        let base = pos - pos % LINE;
        let p = pos - base;
        let in_line = match self {
            SlotMode::H40Active => table_next(&H40_ACTIVE, p),
            SlotMode::H32Active => table_next(&H32_ACTIVE, p),
            SlotMode::H40Blank => even_next(H40_BLANK, p),
            SlotMode::H32Blank => even_next(H32_BLANK, p),
        };
        match in_line {
            Some(slot) => base + slot,
            // Past the last slot: the first slot of the next line.
            None => base + LINE + self.next_slot(0),
        }
    }
}

#[inline]
fn table_next(table: &[u32], p: u32) -> Option<u32> {
    table.iter().copied().find(|&slot| slot >= p)
}

/// `n` slots spread evenly over the line: slot `k` is at `k * LINE / n`.
/// The first one at or after `p` is slot `ceil(p * n / LINE)`.
#[inline]
fn even_next(n: u32, p: u32) -> Option<u32> {
    let k = (p * n).div_ceil(LINE);
    (k < n).then(|| k * LINE / n)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Count slots by walking `next_slot` through one line.
    fn count(mode: SlotMode) -> u32 {
        let mut pos = 0;
        let mut n = 0;
        loop {
            let slot = mode.next_slot(pos);
            if slot >= LINE {
                return n;
            }
            n += 1;
            pos = slot + 1;
        }
    }

    #[test]
    fn documented_slot_counts() {
        assert_eq!(count(SlotMode::H40Active), 18);
        assert_eq!(count(SlotMode::H32Active), 16);
        assert_eq!(count(SlotMode::H40Blank), 205);
        assert_eq!(count(SlotMode::H32Blank), 167);
        for mode in [
            SlotMode::H40Active,
            SlotMode::H32Active,
            SlotMode::H40Blank,
            SlotMode::H32Blank,
        ] {
            assert_eq!(count(mode), mode.per_line());
        }
    }

    #[test]
    fn tables_are_sorted_and_inside_the_line() {
        for table in [&H40_ACTIVE[..], &H32_ACTIVE[..]] {
            assert!(table.windows(2).all(|w| w[0] < w[1]));
            assert!(*table.last().unwrap() < LINE);
        }
    }

    #[test]
    fn next_slot_wraps_into_the_next_line() {
        let mode = SlotMode::H40Active;
        assert_eq!(mode.next_slot(0), 104);
        assert_eq!(mode.next_slot(104), 104);
        assert_eq!(mode.next_slot(105), 232);
        assert_eq!(mode.next_slot(3211), LINE + 104);
        assert_eq!(mode.next_slot(2 * LINE + 300), 2 * LINE + 360);
        let blank = SlotMode::H32Blank;
        assert_eq!(blank.next_slot(0), 0);
        assert_eq!(blank.next_slot(LINE - 1), LINE);
    }

    #[test]
    fn active_display_has_slots_spread_across_the_line() {
        // No gap between two H40 active slots is longer than 256 clocks
        // (two 16-pixel blocks) inside the display area: a stalled CPU is
        // never stuck for long.
        let gaps: Vec<u32> = H40_ACTIVE.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(gaps.iter().all(|&g| g <= 300), "{gaps:?}");
    }
}
