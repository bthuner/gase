//! The scanline renderer.
//!
//! Each visible line is produced in three steps:
//!
//! 1. Render plane B, then plane A (or the window where it is active), into
//!    line buffers of *layer pixels*.
//! 2. Evaluate the sprite list for this line and draw the sprites into a
//!    third buffer.
//! 3. Composite the three buffers by priority, apply shadow/highlight and
//!    convert to RGB.
//!
//! A layer pixel is one byte: `P0PPCCCC` — bit 7 is the priority bit, bits
//! 4-5 the palette and bits 0-3 the colour index (0 is transparent). The
//! priority bit is kept even for transparent pixels because shadow/highlight
//! mode looks at it.

use crate::color::{Intensity, Palette};
use crate::{MAX_WIDTH, Vdp};

const PRIORITY: u8 = 0x80;

/// Line buffers reused from line to line to avoid allocations.
#[derive(Clone, Debug)]
pub(crate) struct Scratch {
    plane_a: [u8; MAX_WIDTH],
    plane_b: [u8; MAX_WIDTH],
    sprites: [u8; MAX_WIDTH],
}

impl Default for Scratch {
    fn default() -> Self {
        Self {
            plane_a: [0; MAX_WIDTH],
            plane_b: [0; MAX_WIDTH],
            sprites: [0; MAX_WIDTH],
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Plane {
    A,
    B,
}

/// Geometry shared by all layers on one line.
#[derive(Clone, Copy)]
struct LineInfo {
    /// Line in "pattern space": doubled (plus field) in interlace mode 2.
    y: u32,
    width: usize,
    /// Tile height in pixels (8, or 16 in interlace mode 2).
    tile_height: u32,
    /// log2 of the tile size in bytes (32 or 64).
    tile_shift: u32,
}

#[inline]
fn opaque(pixel: u8) -> bool {
    pixel & 0x0F != 0
}

#[inline]
fn high(pixel: u8) -> bool {
    pixel & PRIORITY != 0
}

impl Vdp {
    pub(crate) fn render_line(&mut self, line: u16) {
        let width = if self.h40() { 320 } else { 256 };
        let double = self.interlace_double();
        let field = u32::from(double && self.odd_frame);
        let info = LineInfo {
            y: if double {
                u32::from(line) * 2 + field
            } else {
                u32::from(line)
            },
            width,
            tile_height: if double { 16 } else { 8 },
            tile_shift: if double { 6 } else { 5 },
        };
        let row = if double {
            usize::from(line) * 2 + field as usize
        } else {
            usize::from(line)
        };
        self.frame_width = width;

        let mut scratch = std::mem::take(&mut self.scratch);
        let backdrop = self.regs[7] & 0x3F;
        let backdrop_rgb = self.palette.get(backdrop, Intensity::Normal);

        if !self.display_enabled() || self.regs[1] & 0x04 == 0 {
            // Display disabled (or the unsupported Master System mode 4):
            // only the backdrop colour is shown.
            self.frame[row * MAX_WIDTH..row * MAX_WIDTH + width].fill(backdrop_rgb);
            self.scratch = scratch;
            return;
        }

        self.render_plane(&mut scratch.plane_b, Plane::B, info);
        self.render_plane(&mut scratch.plane_a, Plane::A, info);
        self.render_window(&mut scratch.plane_a, line, info);
        self.render_sprites(&mut scratch.sprites, info);

        let shadow_highlight = self.regs[12] & 0x08 != 0;
        let palette = &self.palette;
        let out = &mut self.frame[row * MAX_WIDTH..row * MAX_WIDTH + width];
        for (x, pixel) in out.iter_mut().enumerate() {
            let (a, b, s) = (scratch.plane_a[x], scratch.plane_b[x], scratch.sprites[x]);
            *pixel = if shadow_highlight {
                Self::composite_shadow_highlight(palette, a, b, s, backdrop)
            } else {
                palette.get(Self::pick(a, b, s, backdrop), Intensity::Normal)
            };
        }

        // Register 0 bit 5 blanks the leftmost 8 pixels (hides the column
        // being updated when scrolling horizontally).
        if self.regs[0] & 0x20 != 0 {
            out[..8].fill(backdrop_rgb);
        }
        self.scratch = scratch;
    }

    /// Standard priority: high sprite, high A, high B, low sprite, low A,
    /// low B, backdrop.
    #[inline]
    fn pick(a: u8, b: u8, s: u8, backdrop: u8) -> u8 {
        let layers = [s, a, b];
        for &pixel in &layers {
            if opaque(pixel) && high(pixel) {
                return pixel & 0x3F;
            }
        }
        for &pixel in &layers {
            if opaque(pixel) {
                return pixel & 0x3F;
            }
        }
        backdrop
    }

    /// Shadow/highlight mode.
    ///
    /// * A pixel is shadowed unless plane A or plane B has its priority bit
    ///   set (whether or not that plane pixel is transparent).
    /// * High-priority sprites are always drawn at normal intensity.
    /// * Sprite colours 14 and 15 of palette 3 are not drawn: they are
    ///   *operators* that highlight (14) or shadow (15) what is beneath them.
    fn composite_shadow_highlight(palette: &Palette, a: u8, b: u8, s: u8, backdrop: u8) -> u32 {
        let mut intensity = if high(a) || high(b) {
            Intensity::Normal
        } else {
            Intensity::Shadow
        };
        let planes_top = Self::pick(a, b, 0, backdrop);
        let plane_high = (opaque(a) && high(a)) || (opaque(b) && high(b));
        let sprite_on_top = opaque(s) && (high(s) || !plane_high);

        let index = if sprite_on_top {
            match s & 0x3F {
                0x3E => {
                    intensity = intensity.brighter();
                    planes_top
                }
                0x3F => {
                    intensity = Intensity::Shadow;
                    planes_top
                }
                colour => {
                    if high(s) || colour & 0x0F == 0x0E {
                        intensity = Intensity::Normal;
                    }
                    colour
                }
            }
        } else {
            planes_top
        };
        palette.get(index, intensity)
    }

    fn plane_size(&self) -> (u32, u32) {
        let cells = |bits: u8| match bits & 3 {
            0 => 32,
            1 => 64,
            3 => 128,
            // Invalid setting; behaves like 32 cells for practical purposes.
            _ => 32,
        };
        (cells(self.regs[16]), cells(self.regs[16] >> 4))
    }

    fn render_plane(&self, out: &mut [u8; MAX_WIDTH], plane: Plane, info: LineInfo) {
        let (name_table, scroll_index) = match plane {
            Plane::A => (u32::from(self.regs[2] & 0x38) << 10, 0),
            Plane::B => (u32::from(self.regs[4] & 0x07) << 13, 1),
        };
        let (width_cells, height_cells) = self.plane_size();
        let width_mask = width_cells * 8 - 1;
        let height_mask = height_cells * info.tile_height - 1;

        // Horizontal scroll: one value for the whole screen, per 8-line
        // strip, or per line, read from the scroll table in VRAM.
        let line = u32::from(self.line);
        let hscroll_base = u32::from(self.regs[13] & 0x3F) << 10;
        let hscroll_offset = match self.regs[11] & 3 {
            0 => 0,
            1 => (line & 7) * 4,
            2 => (line & !7) * 4,
            _ => line * 4,
        };
        let hscroll = u32::from(
            self.vram_word(((hscroll_base + hscroll_offset + scroll_index * 2) & 0xFFFF) as u16)
                & 0x3FF,
        );

        // Vertical scroll: one value, or one per 16-pixel column.
        let per_column = self.regs[11] & 0x04 != 0;
        let vscroll_mask = if info.tile_height == 16 { 0x7FF } else { 0x3FF };
        let vscroll_for = |column: usize| -> u32 {
            let index = if per_column {
                (column * 2 + scroll_index as usize).min(39)
            } else {
                scroll_index as usize
            };
            u32::from(self.vsram[index]) & vscroll_mask
        };

        let mut cached_key = u32::MAX;
        let mut pattern: u32 = 0;
        let mut attributes: u16 = 0;
        for (x, pixel) in out.iter_mut().take(info.width).enumerate() {
            let column = x / 16;
            let px = (x as u32).wrapping_sub(hscroll) & width_mask;
            let py = (info.y + vscroll_for(column)) & height_mask;
            let cell_x = px >> 3;
            let key = (py << 8) | cell_x;
            if key != cached_key {
                cached_key = key;
                let cell_y = py / info.tile_height;
                let entry_addr = name_table + (cell_y * width_cells + cell_x) * 2;
                attributes = self.vram_word((entry_addr & 0xFFFF) as u16);
                pattern = self.tile_row(attributes, py % info.tile_height, info);
            }
            *pixel = Self::tile_pixel(attributes, pattern, px & 7);
        }
    }

    /// Fetch one row of a tile as 8 packed 4-bit pixels (leftmost in the top
    /// nibble), honouring vertical flip.
    #[inline]
    fn tile_row(&self, attributes: u16, row: u32, info: LineInfo) -> u32 {
        let tile = u32::from(attributes & if info.tile_height == 16 { 0x3FF } else { 0x7FF });
        let row = if attributes & 0x1000 != 0 {
            info.tile_height - 1 - row
        } else {
            row
        };
        let addr = ((tile << info.tile_shift) + row * 4) as usize & 0xFFFC;
        u32::from_be_bytes([
            self.vram[addr],
            self.vram[addr + 1],
            self.vram[addr + 2],
            self.vram[addr + 3],
        ])
    }

    /// Extract pixel `x` (0-7) of a tile row as a layer pixel.
    #[inline]
    fn tile_pixel(attributes: u16, pattern: u32, x: u32) -> u8 {
        let x = if attributes & 0x0800 != 0 { 7 - x } else { x };
        let colour = ((pattern >> (28 - x * 4)) & 0xF) as u8;
        let palette = ((attributes >> 9) & 0x30) as u8;
        let priority = ((attributes >> 8) & 0x80) as u8;
        priority | palette | colour
    }

    /// Overwrite plane A with the window where the window is active.
    fn render_window(&self, out: &mut [u8; MAX_WIDTH], line: u16, info: LineInfo) {
        // Vertical extent: register 18 = D00P PPPP, window is the area above
        // (D=0) or below (D=1) line P*8.
        let v_pos = u16::from(self.regs[18] & 0x1F) * 8;
        let below = self.regs[18] & 0x80 != 0;
        let whole_line = if below { line >= v_pos } else { line < v_pos };

        // Horizontal extent: register 17 = R00P PPPP, left (R=0) or right
        // (R=1) of column P*16.
        let h_pos = usize::from(self.regs[17] & 0x1F) * 16;
        let right = self.regs[17] & 0x80 != 0;
        let (start, end) = if whole_line {
            (0, info.width)
        } else if right {
            (h_pos.min(info.width), info.width)
        } else {
            (0, h_pos.min(info.width))
        };
        if start >= end {
            return;
        }

        // The window never scrolls; its name table is 64 cells wide in H40
        // and 32 in H32.
        let (base_mask, width_cells) = if self.h40() { (0x3C, 64) } else { (0x3E, 32) };
        let name_table = u32::from(self.regs[3] & base_mask) << 10;
        let y = info.y;
        let cell_y = y / info.tile_height;
        for (cell_x, chunk) in out[start..end].chunks_mut(8).enumerate() {
            let cell_x = (start / 8 + cell_x) as u32;
            let entry_addr = name_table + (cell_y * width_cells + cell_x) * 2;
            let attributes = self.vram_word((entry_addr & 0xFFFF) as u16);
            let pattern = self.tile_row(attributes, y % info.tile_height, info);
            for (x, pixel) in chunk.iter_mut().enumerate() {
                *pixel = Self::tile_pixel(attributes, pattern, x as u32);
            }
        }
    }

    /// Find the sprites on this line and draw them.
    ///
    /// The sprite table is a linked list starting at sprite 0. Each 8-byte
    /// entry is:
    ///
    /// ```text
    /// word 0: ------YY YYYYYYYY   vertical position + 128
    /// word 1: ----HHVV -LLLLLLL   size (cells - 1) and link to next sprite
    /// word 2: PCCVHNNN NNNNNNNN   priority, palette, flips, first tile
    /// word 3: -------X XXXXXXXX   horizontal position + 128
    /// ```
    ///
    /// Multi-cell sprites use consecutive tiles in column-major order. The
    /// hardware limits how much it can draw: 20 sprites and 320 pixels per
    /// line in H40 (16 and 256 in H32); beyond that the "overflow" status
    /// flag is set and further sprites are dropped.
    fn render_sprites(&mut self, out: &mut [u8; MAX_WIDTH], info: LineInfo) {
        out[..info.width].fill(0);
        let h40 = self.h40();
        let (max_sprites, max_per_line, max_pixels) =
            if h40 { (80, 20, 320) } else { (64, 16, 256) };
        let double = info.tile_height == 16;
        let y_offset = if double { 256 } else { 128 };
        let line = info.y as i32;
        let table = u32::from(self.sprite_table_base());

        // Step 1: walk the linked list and collect up to max_per_line sprites
        // that intersect this line. Y, size and link come from the cache.
        let mut on_line = [0usize; 20];
        let mut count = 0;
        let mut index = 0usize;
        for _ in 0..max_sprites {
            let cache = &self.sat_cache[index * 4..index * 4 + 4];
            let y = i32::from(
                u16::from_be_bytes([cache[0], cache[1]]) & if double { 0x3FF } else { 0x1FF },
            ) - y_offset;
            let height = (i32::from(cache[2] & 3) + 1) * info.tile_height as i32;
            if (y..y + height).contains(&line) {
                if count == max_per_line {
                    self.sprite_overflow = true;
                    break;
                }
                on_line[count] = index;
                count += 1;
            }
            let link = usize::from(cache[3] & 0x7F);
            if link == 0 || link >= max_sprites {
                break;
            }
            index = link;
        }

        // Step 2: draw them. Earlier sprites in the list are in front.
        let mut pixels_left = max_pixels;
        let mut seen_nonzero_x = false;
        for &index in &on_line[..count] {
            let cache = &self.sat_cache[index * 4..index * 4 + 4];
            let y = i32::from(
                u16::from_be_bytes([cache[0], cache[1]]) & if double { 0x3FF } else { 0x1FF },
            ) - y_offset;
            let width_cells = u32::from((cache[2] >> 2) & 3) + 1;
            let height_cells = u32::from(cache[2] & 3) + 1;

            let entry = (table + index as u32 * 8) & 0xFFFF;
            let attributes = self.vram_word((entry + 4) as u16);
            let raw_x = u32::from(self.vram_word((entry + 6) as u16) & 0x1FF);

            // A sprite at X = 0 (raw) masks every later sprite on this line,
            // but only once a sprite at another X has been seen on the line.
            if raw_x == 0 {
                if seen_nonzero_x {
                    break;
                }
            } else {
                seen_nonzero_x = true;
            }

            let mut row = (line - y) as u32;
            if attributes & 0x1000 != 0 {
                row = height_cells * info.tile_height - 1 - row;
            }
            let base_tile = u32::from(attributes & 0x7FF);
            let x0 = raw_x as i32 - 128;
            for cell in 0..width_cells {
                if pixels_left == 0 {
                    self.sprite_overflow = true;
                    return;
                }
                pixels_left -= 8;
                let column = if attributes & 0x0800 != 0 {
                    width_cells - 1 - cell
                } else {
                    cell
                };
                let tile = base_tile + column * height_cells + row / info.tile_height;
                // Build a pseudo name-table entry so tile_row/tile_pixel can be
                // shared with the planes (flip handled by row above).
                let fake = (attributes & 0xF800 & !0x1000) | (tile as u16 & 0x7FF);
                let pattern = self.tile_row(fake, row % info.tile_height, info);
                for x in 0..8u32 {
                    let sx = x0 + (cell * 8 + x) as i32;
                    if sx < 0 || sx as usize >= info.width {
                        continue;
                    }
                    let pixel = Self::tile_pixel(fake, pattern, x);
                    if !opaque(pixel) {
                        continue;
                    }
                    let slot = &mut out[sx as usize];
                    if opaque(*slot) {
                        self.sprite_collision = true;
                    } else {
                        *slot = pixel;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{MAX_WIDTH, Vdp, VideoStandard};

    /// A VDP in H40 with display on, plane A at 0xC000, plane B at 0xE000,
    /// sprites at 0xF000 and hscroll at 0xFC00.
    fn setup() -> Vdp {
        let mut vdp = Vdp::new(VideoStandard::Ntsc);
        for reg in [
            0x8004, 0x8144, 0x8230, 0x8407, 0x8578, 0x8C81, 0x8D3F, 0x8F02, 0x9001,
        ] {
            vdp.write_control(reg);
        }
        // Palette 0: colour 1 red, colour 2 green; palette 1 colour 1 blue.
        for (index, colour) in [(1, 0x000E), (2, 0x00E0), (17, 0x0E00)] {
            vdp.cram[index] = colour;
        }
        vdp.palette.rebuild(&vdp.cram);
        vdp
    }

    fn fill_tile(vdp: &mut Vdp, tile: usize, colour: u8) {
        let byte = colour << 4 | colour;
        vdp.vram[tile * 32..tile * 32 + 32].fill(byte);
    }

    fn set_word(vdp: &mut Vdp, addr: usize, value: u16) {
        vdp.vram[addr..addr + 2].copy_from_slice(&value.to_be_bytes());
    }

    fn pixel(vdp: &Vdp, x: usize, y: usize) -> u32 {
        vdp.frame()[y * MAX_WIDTH + x]
    }

    #[test]
    fn backdrop_when_display_disabled() {
        let mut vdp = setup();
        vdp.write_control(0x8104);
        vdp.write_control(0x8701); // backdrop = palette 0 colour 1
        vdp.begin_line(0);
        assert_eq!(pixel(&vdp, 0, 0), 0xFF0000);
    }

    #[test]
    fn plane_a_over_plane_b() {
        let mut vdp = setup();
        fill_tile(&mut vdp, 1, 1);
        fill_tile(&mut vdp, 2, 2);
        set_word(&mut vdp, 0xE000, 0x0001); // plane B cell (0,0): tile 1
        set_word(&mut vdp, 0xC000, 0x0002); // plane A cell (0,0): tile 2
        vdp.begin_line(0);
        assert_eq!(pixel(&vdp, 0, 0), 0x00FF00);
        // Cell (1,0) of plane A is empty, so plane B... is also empty: backdrop.
        assert_eq!(pixel(&vdp, 8, 0), 0x000000);
    }

    #[test]
    fn priority_plane_b_over_low_plane_a() {
        let mut vdp = setup();
        fill_tile(&mut vdp, 1, 1);
        fill_tile(&mut vdp, 2, 2);
        set_word(&mut vdp, 0xE000, 0x8001);
        set_word(&mut vdp, 0xC000, 0x0002);
        vdp.begin_line(0);
        assert_eq!(pixel(&vdp, 0, 0), 0xFF0000);
    }

    #[test]
    fn horizontal_scroll() {
        let mut vdp = setup();
        fill_tile(&mut vdp, 1, 1);
        set_word(&mut vdp, 0xC000, 0x0001);
        set_word(&mut vdp, 0xFC00, 4); // plane A scrolled right by 4
        vdp.begin_line(0);
        assert_eq!(pixel(&vdp, 3, 0), 0x000000);
        assert_eq!(pixel(&vdp, 4, 0), 0xFF0000);
        assert_eq!(pixel(&vdp, 11, 0), 0xFF0000);
        assert_eq!(pixel(&vdp, 12, 0), 0x000000);
    }

    #[test]
    fn sprite_drawn_and_ordered() {
        let mut vdp = setup();
        fill_tile(&mut vdp, 1, 1);
        fill_tile(&mut vdp, 2, 1);
        // Sprite 0 at (10, 0), 1x1, palette 1, links to sprite 1.
        for (i, word) in [128u16, 0x0001, 0x2001, 138].into_iter().enumerate() {
            vdp.write_control(0x4000 | (0xF000 + i as u16 * 2) & 0x3FFF);
            vdp.write_control(0x0003);
            vdp.write_data(word);
        }
        // Sprite 1 at (12, 0), palette 0, behind sprite 0.
        for (i, word) in [128u16, 0x0000, 0x0002, 140].into_iter().enumerate() {
            vdp.write_control(0x4000 | (0xF008 + i as u16 * 2) & 0x3FFF);
            vdp.write_control(0x0003);
            vdp.write_data(word);
        }
        vdp.begin_line(0);
        assert_eq!(pixel(&vdp, 9, 0), 0x000000);
        assert_eq!(pixel(&vdp, 10, 0), 0x0000FF);
        assert_eq!(pixel(&vdp, 17, 0), 0x0000FF);
        assert_eq!(pixel(&vdp, 18, 0), 0xFF0000);
        assert!(vdp.sprite_collision);
    }

    #[test]
    fn window_replaces_plane_a() {
        let mut vdp = setup();
        fill_tile(&mut vdp, 1, 1);
        vdp.write_control(0x8334); // window name table at 0xD000
        vdp.write_control(0x9101); // window left of column 16
        set_word(&mut vdp, 0xD000, 0x0001);
        set_word(&mut vdp, 0xD000 + 2, 0x0001);
        vdp.begin_line(0);
        assert_eq!(pixel(&vdp, 0, 0), 0xFF0000);
        assert_eq!(pixel(&vdp, 15, 0), 0xFF0000);
        assert_eq!(pixel(&vdp, 16, 0), 0x000000);
    }

    #[test]
    fn shadow_mode_darkens_low_priority() {
        let mut vdp = setup();
        fill_tile(&mut vdp, 1, 1);
        set_word(&mut vdp, 0xC000, 0x0001);
        set_word(&mut vdp, 0xC002, 0x8001);
        vdp.write_control(0x8C89); // H40 + shadow/highlight
        vdp.begin_line(0);
        assert_eq!(pixel(&vdp, 0, 0), 0x820000);
        assert_eq!(pixel(&vdp, 8, 0), 0xFF0000);
    }
}
