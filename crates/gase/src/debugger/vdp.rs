//! Pictures and descriptions of the VDP's state: decoded registers, the
//! palette, every tile in VRAM, the planes and the sprite table.
//!
//! These functions read VRAM, CRAM and the registers directly, the way the
//! VDP itself does, so they double as a worked example of the formats
//! documented in `gase-vdp`:
//!
//! * a **tile** is 8×8 pixels of 4 bits, 32 bytes, two pixels per byte with
//!   the left pixel in the high nibble; tile `n` lives at VRAM `n × 32`;
//! * a **name table entry** (planes and window) is the word
//!   `PCCVHNNN NNNNNNNN`: priority, palette, vertical/horizontal flip,
//!   tile number;
//! * a **sprite table entry** is 8 bytes; see [`sprites`].

use gase_core::Genesis;

use super::canvas::Canvas;

/// Number of tiles that fit in the 64 KiB of VRAM.
pub const TILE_COUNT: usize = 2048;
/// Tiles per row in [`tile_sheet`].
pub const SHEET_COLUMNS: usize = 64;

/// The 64 CRAM colours as `0x00RRGGBB`.
#[must_use]
pub fn palette_rgb(genesis: &Genesis) -> [u32; 64] {
    std::array::from_fn(|i| genesis.hw.vdp.cram_rgb(i))
}

/// Colour index (0-15) of pixel (`x`, `y`) of tile `tile`.
#[must_use]
pub fn tile_pixel(vram: &[u8], tile: usize, x: usize, y: usize) -> u8 {
    let byte = vram[(tile * 32 + y * 4 + x / 2) & 0xFFFF];
    if x % 2 == 0 { byte >> 4 } else { byte & 0x0F }
}

/// Draw tile `tile` at (`x`, `y`) using a 16-colour palette, optionally
/// flipped. Colour 0 is drawn too (as palette entry 0).
#[allow(clippy::too_many_arguments)]
pub fn draw_tile(
    canvas: &mut Canvas,
    x: usize,
    y: usize,
    vram: &[u8],
    tile: usize,
    colors: &[u32],
    hflip: bool,
    vflip: bool,
) {
    for row in 0..8 {
        let src_row = if vflip { 7 - row } else { row };
        for col in 0..8 {
            let src_col = if hflip { 7 - col } else { col };
            let index = tile_pixel(vram, tile, src_col, src_row);
            canvas.set(x + col, y + row, colors[usize::from(index)]);
        }
    }
}

/// All 2048 VRAM tiles, 64 per row (512×256 pixels), drawn with one
/// 16-colour palette.
#[must_use]
pub fn tile_sheet(vram: &[u8], colors: &[u32]) -> Canvas {
    let rows = TILE_COUNT / SHEET_COLUMNS;
    let mut canvas = Canvas::new(SHEET_COLUMNS * 8, rows * 8, 0);
    for tile in 0..TILE_COUNT {
        let (x, y) = ((tile % SHEET_COLUMNS) * 8, (tile / SHEET_COLUMNS) * 8);
        draw_tile(&mut canvas, x, y, vram, tile, colors, false, false);
    }
    canvas
}

/// The four 16-colour palettes as rows of `size`×`size` squares.
#[must_use]
pub fn cram_swatches(colors: &[u32; 64], size: usize) -> Canvas {
    let mut canvas = Canvas::new(16 * size, 4 * size, 0);
    for (i, &color) in colors.iter().enumerate() {
        canvas.fill_rect((i % 16) * size, (i / 16) * size, size, size, color);
    }
    canvas
}

/// A tile map the VDP can show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    PlaneA,
    PlaneB,
    Window,
}

impl Layer {
    pub fn name(self) -> &'static str {
        match self {
            Layer::PlaneA => "plane A",
            Layer::PlaneB => "plane B",
            Layer::Window => "window",
        }
    }
}

fn cells(bits: u8) -> usize {
    match bits & 3 {
        1 => 64,
        3 => 128,
        _ => 32, // 0, and the invalid setting 2
    }
}

/// Size of plane A/B in cells (register 16).
#[must_use]
pub fn plane_size(regs: &[u8; 24]) -> (usize, usize) {
    (cells(regs[16]), cells(regs[16] >> 4))
}

fn h40(regs: &[u8; 24]) -> bool {
    regs[12] & 0x01 != 0
}

/// Name table address and size in cells of a layer.
#[must_use]
pub fn layer_geometry(regs: &[u8; 24], layer: Layer) -> (usize, usize, usize) {
    let (w, h) = plane_size(regs);
    match layer {
        Layer::PlaneA => (usize::from(regs[2] & 0x38) << 10, w, h),
        Layer::PlaneB => (usize::from(regs[4] & 0x07) << 13, w, h),
        Layer::Window => {
            let (mask, width) = if h40(regs) { (0x3C, 64) } else { (0x3E, 32) };
            (usize::from(regs[3] & mask) << 10, width, 32)
        }
    }
}

/// A whole tile map, unscrolled, with each entry's own palette and flips.
#[must_use]
pub fn layer_map(vram: &[u8], colors: &[u32; 64], regs: &[u8; 24], layer: Layer) -> Canvas {
    let (base, w, h) = layer_geometry(regs, layer);
    let mut canvas = Canvas::new(w * 8, h * 8, 0);
    for cy in 0..h {
        for cx in 0..w {
            let addr = (base + (cy * w + cx) * 2) & 0xFFFF;
            let entry = u16::from_be_bytes([vram[addr], vram[(addr + 1) & 0xFFFF]]);
            let palette = usize::from((entry >> 13) & 3);
            draw_tile(
                &mut canvas,
                cx * 8,
                cy * 8,
                vram,
                usize::from(entry & 0x7FF),
                &colors[palette * 16..palette * 16 + 16],
                entry & 0x0800 != 0,
                entry & 0x1000 != 0,
            );
        }
    }
    canvas
}

/// One decoded sprite table entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sprite {
    pub index: usize,
    /// Screen position (the table stores it plus 128).
    pub x: i32,
    pub y: i32,
    /// Size in cells (1-4 each way).
    pub width: u8,
    pub height: u8,
    pub link: u8,
    pub tile: u16,
    pub palette: u8,
    pub priority: bool,
    pub hflip: bool,
    pub vflip: bool,
}

/// Address of the sprite attribute table (register 5).
#[must_use]
pub fn sprite_table(regs: &[u8; 24]) -> usize {
    let mask = if h40(regs) { 0x7E } else { 0x7F };
    usize::from(regs[5] & mask) << 9
}

/// Walk the sprite list the way the VDP does: from sprite 0, following the
/// link fields until a link of 0 (or a loop, or the table's end).
///
/// ```text
/// word 0: ------YY YYYYYYYY   vertical position + 128
/// word 1: ----HHVV -LLLLLLL   size (cells - 1) and link to next sprite
/// word 2: PCCVHNNN NNNNNNNN   priority, palette, flips, first tile
/// word 3: -------X XXXXXXXX   horizontal position + 128
/// ```
///
/// (The VDP actually takes Y, size and link from an internal cache; this
/// reads them from VRAM, which is what the cache mirrors in normal use.)
#[must_use]
pub fn sprites(vram: &[u8], regs: &[u8; 24]) -> Vec<Sprite> {
    let base = sprite_table(regs);
    let max = if h40(regs) { 80 } else { 64 };
    let word = |addr: usize| u16::from_be_bytes([vram[addr & 0xFFFF], vram[(addr + 1) & 0xFFFF]]);
    let mut list = Vec::new();
    let mut seen = [false; 80];
    let mut index = 0;
    while index < max && !seen[index] {
        seen[index] = true;
        let entry = base + index * 8;
        let (w0, w1, w2, w3) = (
            word(entry),
            word(entry + 2),
            word(entry + 4),
            word(entry + 6),
        );
        let link = (w1 & 0x7F) as u8;
        list.push(Sprite {
            index,
            x: i32::from(w3 & 0x1FF) - 128,
            y: i32::from(w0 & 0x3FF) - 128,
            width: ((w1 >> 10) & 3) as u8 + 1,
            height: ((w1 >> 8) & 3) as u8 + 1,
            link,
            tile: w2 & 0x7FF,
            palette: ((w2 >> 13) & 3) as u8,
            priority: w2 & 0x8000 != 0,
            hflip: w2 & 0x0800 != 0,
            vflip: w2 & 0x1000 != 0,
        });
        if link == 0 {
            break;
        }
        index = usize::from(link);
    }
    list
}

impl Sprite {
    /// One line of text describing the sprite.
    #[must_use]
    pub fn describe(&self) -> String {
        format!(
            "{:02} x{:>4} y{:>4} {}x{} link {:02} tile {:03X} pal {} {}{}{}",
            self.index,
            self.x,
            self.y,
            self.width,
            self.height,
            self.link,
            self.tile,
            self.palette,
            if self.priority { "pri" } else { "   " },
            if self.hflip { " H" } else { "  " },
            if self.vflip { " V" } else { "  " },
        )
    }
}

fn on(flag: bool) -> &'static str {
    if flag { "on" } else { "off" }
}

/// The registers that matter, one line each: raw value, then meaning.
#[must_use]
pub fn register_lines(regs: &[u8; 24]) -> Vec<String> {
    let r = |n: usize| format!("R{n:02X}={:02X}", regs[n]);
    let (pw, ph) = plane_size(regs);
    let hscroll = match regs[11] & 3 {
        0 => "full",
        1 => "invalid",
        2 => "per cell",
        _ => "per line",
    };
    let interlace = match (regs[12] >> 1) & 3 {
        0 | 2 => "off",
        1 => "on",
        _ => "double",
    };
    let dma_len = usize::from(regs[19]) | usize::from(regs[20]) << 8;
    let dma_src = (usize::from(regs[21]) | usize::from(regs[22]) << 8) << 1;
    let dma = match regs[23] >> 6 {
        0 | 1 => format!("68k ${:06X}", dma_src | usize::from(regs[23] & 0x7F) << 17),
        2 => "fill".to_string(),
        _ => format!("copy ${dma_src:04X}"),
    };
    vec![
        format!(
            "{} HINT {}, HV latch {}",
            r(0),
            on(regs[0] & 0x10 != 0),
            on(regs[0] & 0x02 != 0)
        ),
        format!(
            "{} display {}, VINT {}, DMA {}",
            r(1),
            on(regs[1] & 0x40 != 0),
            on(regs[1] & 0x20 != 0),
            on(regs[1] & 0x10 != 0)
        ),
        format!(
            "      {} lines, mode {}",
            if regs[1] & 0x08 != 0 { 240 } else { 224 },
            if regs[1] & 0x04 != 0 { 5 } else { 4 }
        ),
        format!(
            "{} plane A at ${:04X}",
            r(2),
            layer_geometry(regs, Layer::PlaneA).0
        ),
        format!(
            "{} window  at ${:04X}",
            r(3),
            layer_geometry(regs, Layer::Window).0
        ),
        format!(
            "{} plane B at ${:04X}",
            r(4),
            layer_geometry(regs, Layer::PlaneB).0
        ),
        format!("{} sprites at ${:04X}", r(5), sprite_table(regs)),
        format!(
            "{} backdrop: palette {} colour {}",
            r(7),
            (regs[7] >> 4) & 3,
            regs[7] & 15
        ),
        format!("{} HINT every {} lines", r(10), u16::from(regs[10]) + 1),
        format!(
            "{} scroll H {}, V {}",
            r(11),
            hscroll,
            if regs[11] & 0x04 != 0 {
                "per 2 cells"
            } else {
                "full"
            }
        ),
        format!(
            "{} {}, S/H {}, interlace {}",
            r(12),
            if h40(regs) {
                "H40 (320px)"
            } else {
                "H32 (256px)"
            },
            on(regs[12] & 0x08 != 0),
            interlace
        ),
        format!(
            "{} H scroll table at ${:04X}",
            r(13),
            usize::from(regs[13] & 0x3F) << 10
        ),
        format!("{} auto-increment {}", r(15), regs[15]),
        format!("{} planes {pw}x{ph} cells", r(16)),
        format!(
            "{} window x: {} of cell {}",
            r(17),
            if regs[17] & 0x80 != 0 {
                "right"
            } else {
                "left"
            },
            usize::from(regs[17] & 0x1F) * 2
        ),
        format!(
            "{} window y: {} row {}",
            r(18),
            if regs[18] & 0x80 != 0 {
                "below"
            } else {
                "above"
            },
            regs[18] & 0x1F
        ),
        format!("R13/14 DMA length ${dma_len:04X} words"),
        format!("R15-17 DMA source {dma}"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 16 distinct test colours.
    fn colors() -> Vec<u32> {
        (0..16).map(|i| i * 0x10_1010).collect()
    }

    #[test]
    fn tile_sheet_places_tiles_and_nibbles() {
        let mut vram = vec![0u8; 0x1_0000];
        // Tile 1, row 0: pixels 1, 2, 3, ... 8 (left pixel in the high nibble).
        vram[32..36].copy_from_slice(&[0x12, 0x34, 0x56, 0x78]);
        // Tile 65 (second row of the sheet, column 1), row 7, pixel 7: colour 15.
        vram[65 * 32 + 7 * 4 + 3] = 0x0F;
        let sheet = tile_sheet(&vram, &colors());
        assert_eq!((sheet.width, sheet.height), (512, 256));
        for x in 0..8 {
            assert_eq!(sheet.get(8 + x, 0), (x as u32 + 1) * 0x10_1010);
        }
        assert_eq!(sheet.get(8, 1), 0); // row 1 of tile 1 is colour 0
        assert_eq!(sheet.get(8 + 7, 8 + 7), 15 * 0x10_1010);
        assert_eq!(sheet.get(8 + 6, 8 + 7), 0);
    }

    #[test]
    fn flips_mirror_the_tile() {
        let mut vram = vec![0u8; 64];
        vram[32] = 0x10; // tile 1: only pixel (0, 0) is set
        let mut c = Canvas::new(8, 8, 99);
        draw_tile(&mut c, 0, 0, &vram, 1, &colors(), true, true);
        assert_eq!(c.get(7, 7), 0x10_1010);
        assert_eq!(c.get(0, 0), 0);
    }

    #[test]
    fn cram_swatches_layout() {
        let mut cram = [0u32; 64];
        cram[17] = 0xFF0000; // palette 1, colour 1
        cram[63] = 0x00FF00;
        let c = cram_swatches(&cram, 4);
        assert_eq!((c.width, c.height), (64, 16));
        assert_eq!(c.get(4, 4), 0xFF0000);
        assert_eq!(c.get(7, 7), 0xFF0000);
        assert_eq!(c.get(8, 4), 0);
        assert_eq!(c.get(63, 15), 0x00FF00);
    }

    #[test]
    fn layer_map_uses_entry_palette_and_flip() {
        let mut vram = vec![0u8; 0x1_0000];
        let mut regs = [0u8; 24];
        regs[2] = 0x30; // plane A at $C000
        vram[32] = 0x10; // tile 1, pixel (0, 0) = colour 1
        // Entry (1, 0): palette 2, horizontal flip, tile 1.
        vram[0xC002..0xC004].copy_from_slice(&(0x4000u16 | 0x0800 | 1).to_be_bytes());
        let mut cram = [0u32; 64];
        cram[33] = 0x123456;
        let map = layer_map(&vram, &cram, &regs, Layer::PlaneA);
        assert_eq!((map.width, map.height), (256, 256));
        assert_eq!(map.get(8 + 7, 0), 0x123456);
        assert_eq!(map.get(8, 0), 0);
    }

    #[test]
    fn sprite_list_follows_links() {
        let mut vram = vec![0u8; 0x1_0000];
        let mut regs = [0u8; 24];
        regs[5] = 0x7C; // table at $F800
        regs[12] = 0x81; // H40
        let mut put = |index: usize, words: [u16; 4]| {
            for (i, w) in words.iter().enumerate() {
                let a = 0xF800 + index * 8 + i * 2;
                vram[a..a + 2].copy_from_slice(&w.to_be_bytes());
            }
        };
        put(
            0,
            [128 + 10, 0x0F02, 0x8000 | 0x2000 | 0x1000 | 0x123, 128 + 20],
        );
        put(2, [128, 0x0000, 0x0800, 128]);
        let list = sprites(&vram, &regs);
        assert_eq!(list.len(), 2);
        let s = list[0];
        assert_eq!((s.x, s.y, s.width, s.height, s.link), (20, 10, 4, 4, 2));
        assert_eq!(
            (s.tile, s.palette, s.priority, s.vflip, s.hflip),
            (0x123, 1, true, true, false)
        );
        assert_eq!((list[1].index, list[1].hflip), (2, true));
        assert!(
            s.describe()
                .starts_with("00 x  20 y  10 4x4 link 02 tile 123 pal 1 pri")
        );
    }

    #[test]
    fn registers_are_named() {
        let mut regs = [0u8; 24];
        regs[1] = 0x74;
        regs[2] = 0x30;
        regs[12] = 0x81;
        regs[16] = 0x01;
        let lines = register_lines(&regs).join("\n");
        assert!(
            lines.contains("R01=74 display on, VINT on, DMA on"),
            "{lines}"
        );
        assert!(lines.contains("plane A at $C000"));
        assert!(lines.contains("H40 (320px)"));
        assert!(lines.contains("planes 64x32 cells"));
    }
}
