//! The built-in debugger: a window that shows what the console is doing,
//! for learning how a Mega Drive game works.
//!
//! Everything is drawn into a plain pixel buffer ([`canvas::Canvas`]) with a
//! tiny built-in font ([`font`]), so the views need no GUI library, can be
//! unit-tested, and the headless runner can save them as PNG files. The SDL
//! frontend only copies the buffer into a second window and forwards keys
//! ([`Panel::key`]).
//!
//! Execution control (pause, step, breakpoints) lives in the core:
//! [`gase_core::Debugger`].

// Without the window, only the headless dumps and reports use this module.
#![cfg_attr(not(feature = "sdl"), allow(dead_code))]

pub mod canvas;
pub mod cpu;
pub mod font;
pub mod vdp;

use gase_core::{Debugger, Genesis};

use canvas::Canvas;
use font::draw_text;
use vdp::Layer;

/// Size of the debugger picture in pixels.
pub const WIDTH: usize = 1280;
pub const HEIGHT: usize = 800;

const BACKGROUND: u32 = 0x10_1418;
const TEXT: u32 = 0xD8_D8D8;
const DIM: u32 = 0x80_8890;
const HEADER: u32 = 0xFF_C850;
const PC_BAR: u32 = 0x1F_3F6F;
const CURSOR_BAR: u32 = 0x3A_3A3A;
const BREAKPOINT: u32 = 0xFF_5050;
const GOOD: u32 = 0x70_E070;

/// Line height of text in pixels.
const LINE: usize = 10;
/// Number of 68000 instructions listed.
const LISTING: usize = 24;
/// Number of sprites listed at once.
const SPRITE_ROWS: usize = 24;

/// A key press, independent of the windowing library.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// A printable key (letters lower-case).
    Char(char),
    Up,
    Down,
    PageDown,
    Home,
    Enter,
    Backspace,
    Escape,
}

/// Something the frontend must do in response to a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    TogglePause,
    /// Execute one 68000 instruction.
    Step,
    /// Run to the end of the frame.
    StepFrame,
    /// Run until the next vertical interrupt.
    RunToVBlank,
    /// Hide the debugger.
    Close,
}

/// What the big picture area shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum View {
    Tiles,
    Map(Layer),
}

impl View {
    fn next(self) -> Self {
        match self {
            View::Tiles => View::Map(Layer::PlaneA),
            View::Map(Layer::PlaneA) => View::Map(Layer::PlaneB),
            View::Map(Layer::PlaneB) => View::Map(Layer::Window),
            View::Map(Layer::Window) => View::Tiles,
        }
    }
}

/// The debugger window's state: what is selected and scrolled where.
#[derive(Debug)]
pub struct Panel {
    canvas: Canvas,
    /// Palette used by the tile viewer (0-3).
    palette: usize,
    view: View,
    /// First address of the listing; `None` follows the PC.
    listing_start: Option<u32>,
    /// Selected line of the listing.
    cursor: usize,
    sprite_scroll: usize,
    /// Hex digits typed after `G`.
    input: Option<String>,
    /// A short message shown at the bottom.
    status: String,
}

impl Default for Panel {
    fn default() -> Self {
        Self::new()
    }
}

impl Panel {
    #[must_use]
    pub fn new() -> Self {
        Self {
            canvas: Canvas::new(WIDTH, HEIGHT, BACKGROUND),
            palette: 0,
            view: View::Tiles,
            listing_start: None,
            cursor: 0,
            sprite_scroll: 0,
            input: None,
            status: String::new(),
        }
    }

    /// Show `text` in the status line.
    pub fn set_status(&mut self, text: impl Into<String>) {
        self.status = text.into();
    }

    /// Make the listing follow the PC again (after execution moved it).
    pub fn follow_pc(&mut self) {
        self.listing_start = None;
        self.cursor = 0;
    }

    fn listing(&self, genesis: &Genesis) -> Vec<cpu::Line> {
        let start = self
            .listing_start
            .unwrap_or_else(|| genesis.m68k.pc() & 0xFF_FFFF);
        cpu::disassembly(genesis, start, LISTING)
    }

    /// Handle a key press while the debugger window has the focus.
    pub fn key(&mut self, key: Key, genesis: &mut Genesis, debugger: &mut Debugger) -> Action {
        if let Some(input) = &mut self.input {
            match key {
                Key::Char(c) if c.is_ascii_hexdigit() && input.len() < 6 => input.push(c),
                Key::Backspace => {
                    input.pop();
                }
                Key::Enter => {
                    if let Ok(addr) = u32::from_str_radix(input, 16) {
                        self.listing_start = Some(addr & 0xFF_FFFE);
                        self.cursor = 0;
                    }
                    self.input = None;
                }
                Key::Escape => self.input = None,
                _ => {}
            }
            return Action::None;
        }

        match key {
            Key::Escape => return Action::Close,
            Key::Char(' ' | 'p') => return Action::TogglePause,
            Key::Char('s') => return Action::Step,
            Key::Char('f' | 'n') => return Action::StepFrame,
            Key::Char('v') => return Action::RunToVBlank,
            Key::Up => self.cursor = self.cursor.saturating_sub(1),
            Key::Down => {
                if self.cursor + 1 < LISTING {
                    self.cursor += 1;
                } else {
                    self.listing_start = Some(self.listing(genesis)[1].addr);
                }
            }
            Key::PageDown => {
                self.listing_start = Some(self.listing(genesis)[LISTING - 1].addr);
            }
            Key::Home => self.follow_pc(),
            Key::Char('g') => self.input = Some(String::new()),
            Key::Char('b') => {
                let addr = self.listing(genesis)[self.cursor].addr;
                let set = debugger.toggle_breakpoint(addr);
                self.status = format!(
                    "Breakpoint {} ${addr:06X}",
                    if set { "set at" } else { "removed from" }
                );
            }
            Key::Char('c') => {
                debugger.clear_breakpoints();
                self.status = "All breakpoints removed".into();
            }
            Key::Char('[') => self.palette = (self.palette + 3) % 4,
            Key::Char(']') => self.palette = (self.palette + 1) % 4,
            Key::Char('t') => self.view = self.view.next(),
            Key::Char(',') => self.sprite_scroll = self.sprite_scroll.saturating_sub(SPRITE_ROWS),
            Key::Char('.') => self.sprite_scroll += SPRITE_ROWS,
            Key::Char(c @ '1'..='9' | c @ '0') => {
                // 1-6: FM channels, 7-9: PSG tones, 0: PSG noise.
                let n = if c == '0' { 9 } else { c as u8 - b'1' };
                let (mut fm, mut psg) = genesis.channel_mutes();
                if n < 6 {
                    fm ^= 1 << n;
                } else {
                    psg ^= 1 << (n - 6);
                }
                genesis.set_channel_mutes(fm, psg);
            }
            _ => {}
        }
        Action::None
    }

    /// Draw every view into the panel's picture and return it.
    pub fn draw(&mut self, genesis: &Genesis, debugger: &Debugger, paused: bool) -> &Canvas {
        let mut c = std::mem::replace(&mut self.canvas, Canvas::new(0, 0, 0));
        c.pixels.fill(BACKGROUND);
        self.draw_cpus(&mut c, genesis, debugger, paused);
        self.draw_vdp(&mut c, genesis);
        self.canvas = c;
        &self.canvas
    }

    fn draw_cpus(&mut self, c: &mut Canvas, genesis: &Genesis, debugger: &Debugger, paused: bool) {
        let x = 8;
        let mut y = 6;
        let state = if paused { "PAUSED" } else { "running" };
        let mut title = format!("68000   frame {}", genesis.frame_count());
        if let Some(line) = debugger.line() {
            title += &format!(", stopped in line {line}");
        }
        draw_text(c, x, y, &title, HEADER);
        draw_text(
            c,
            x + 50 * 8,
            y,
            state,
            if paused { BREAKPOINT } else { GOOD },
        );
        y += LINE + 2;
        for line in cpu::m68k_lines(genesis) {
            draw_text(c, x, y, &line, TEXT);
            y += LINE;
        }

        y += 4;
        let pc = genesis.m68k.pc() & 0xFF_FFFF;
        let listing = self.listing(genesis);
        self.cursor = self.cursor.min(LISTING - 1);
        for (i, line) in listing.iter().enumerate() {
            if line.addr == pc {
                c.fill_rect(x - 4, y - 1, 520, LINE, PC_BAR);
            }
            if i == self.cursor {
                c.outline(x - 4, y - 1, 520, LINE, CURSOR_BAR | 0x50_5050);
            }
            if debugger.breakpoints().contains(&line.addr) {
                c.fill_rect(x, y, 7, 7, BREAKPOINT);
            }
            let mark = if line.addr == pc { ">" } else { " " };
            draw_text(c, x + 10, y, mark, HEADER);
            let text = format!("{:06X}  {}", line.addr, line.text);
            draw_text(c, x + 20, y, &text, TEXT);
            y += LINE;
        }

        y += 8;
        draw_text(c, x, y, "Z80", HEADER);
        y += LINE + 2;
        for line in cpu::z80_lines(genesis) {
            draw_text(c, x, y, &line, TEXT);
            y += LINE;
        }
        let zpc = genesis.z80.pc;
        for (addr, text) in cpu::z80_disassembly(genesis, zpc, 8) {
            if addr == zpc {
                c.fill_rect(x - 4, y - 1, 520, LINE, PC_BAR);
            }
            draw_text(c, x + 20, y, &format!("{addr:04X}  {text}"), TEXT);
            y += LINE;
        }

        y += 8;
        let bps: Vec<String> = debugger
            .breakpoints()
            .iter()
            .map(|a| format!("{a:06X}"))
            .collect();
        draw_text(c, x, y, "Breakpoints:", HEADER);
        let list = if bps.is_empty() {
            "none".to_string()
        } else {
            bps.join(" ")
        };
        draw_text(c, x + 13 * 8, y, &list, BREAKPOINT);
        y += LINE;

        let (fm, psg) = genesis.channel_mutes();
        draw_text(c, x, y, "Sound:", HEADER);
        let mut sx = x + 7 * 8;
        let names = [
            "FM1", "FM2", "FM3", "FM4", "FM5", "FM6", "SQ1", "SQ2", "SQ3", "NOI",
        ];
        for (i, name) in names.iter().enumerate() {
            let muted = if i < 6 {
                fm & (1 << i) != 0
            } else {
                psg & (1 << (i - 6)) != 0
            };
            sx = draw_text(c, sx, y, name, if muted { DIM } else { GOOD }) + 8;
        }
        y += LINE + 6;

        let help = [
            "Space/P pause   S step   F frame   V to VBlank",
            "Up/Down/PgDn/Home move   B breakpoint   C clear all",
            "G goto address (hex, Enter)   T tiles/planes",
            "[ ] palette   , . sprites   1-6 FM   7-9 0 PSG mute",
            "Esc/F1 close (keys go to the game window when it",
            "has the focus)",
        ];
        for line in help {
            draw_text(c, x, y, line, DIM);
            y += LINE;
        }
        y += 4;
        if let Some(input) = &self.input {
            draw_text(c, x, y, &format!("Go to address: ${input}_"), HEADER);
        } else {
            draw_text(c, x, y, &self.status, TEXT);
        }
    }

    fn draw_vdp(&mut self, c: &mut Canvas, genesis: &Genesis) {
        let vdp = &genesis.hw.vdp;
        let x = 560;
        let mut y = 6;
        draw_text(c, x, y, "VDP registers (Rnn=value, in hex)", HEADER);
        y += LINE + 2;
        let lines = vdp::register_lines(&vdp.regs);
        let half = lines.len().div_ceil(2);
        for (i, line) in lines.iter().enumerate() {
            let (col, row) = (i / half, i % half);
            draw_text(c, x + col * 360, y + row * LINE, line, TEXT);
        }
        y += half * LINE + 8;

        // CRAM: four rows of 16 colours.
        let colors = vdp::palette_rgb(genesis);
        draw_text(c, x, y, "CRAM (palettes 0-3)", HEADER);
        y += LINE + 2;
        let swatch = 18;
        c.blit(x, y, &vdp::cram_swatches(&colors, swatch), 1);
        c.outline(
            x - 1,
            y - 1 + self.palette * swatch,
            16 * swatch + 2,
            swatch + 2,
            0xFF_FFFF,
        );
        for row in 0..4 {
            draw_text(
                c,
                x + 16 * swatch + 8,
                y + row * swatch + 5,
                &row.to_string(),
                DIM,
            );
        }
        y += 4 * swatch + 8;

        // Tiles or a plane, in a 512×256 area.
        let (title, picture) = match self.view {
            View::Tiles => (
                format!("VRAM tiles 0-2047 (palette {}), T: planes", self.palette),
                vdp::tile_sheet(
                    &vdp.vram,
                    &colors[self.palette * 16..self.palette * 16 + 16],
                ),
            ),
            View::Map(layer) => {
                let (base, w, h) = vdp::layer_geometry(&vdp.regs, layer);
                (
                    format!("{} at ${base:04X}, {w}x{h} cells, T: next", layer.name()),
                    vdp::layer_map(&vdp.vram, &colors, &vdp.regs, layer),
                )
            }
        };
        draw_text(c, x, y, &title, HEADER);
        y += LINE + 2;
        let step = (picture.width.div_ceil(512)).max(picture.height.div_ceil(256));
        c.fill_rect(x, y, 512, 256, 0);
        c.blit(x, y, &picture, step);
        if step > 1 {
            draw_text(c, x + 520, y, &format!("1/{step}"), DIM);
        }
        y += 256 + 8;

        // The sprite list.
        let sprites = vdp::sprites(&vdp.vram, &vdp.regs);
        if self.sprite_scroll >= sprites.len() {
            self.sprite_scroll = 0;
        }
        draw_text(
            c,
            x,
            y,
            &format!(
                "Sprites at ${:04X}: {} in list (, . scroll)",
                vdp::sprite_table(&vdp.regs),
                sprites.len()
            ),
            HEADER,
        );
        y += LINE + 2;
        let shown = &sprites[self.sprite_scroll..];
        for (i, sprite) in shown.iter().take(SPRITE_ROWS).enumerate() {
            let (col, row) = (i / (SPRITE_ROWS / 2), i % (SPRITE_ROWS / 2));
            let sx = x + col * 360;
            let sy = y + row * LINE;
            // A little preview of the sprite's first tile in its palette.
            let p = usize::from(sprite.palette) * 16;
            vdp::draw_tile(
                c,
                sx,
                sy,
                &vdp.vram,
                usize::from(sprite.tile),
                &colors[p..p + 16],
                sprite.hflip,
                sprite.vflip,
            );
            draw_text(c, sx + 12, sy, &sprite.describe(), TEXT);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gase_core::{Cartridge, Config};

    fn console() -> Genesis {
        let mut rom = vec![0u8; 0x1000];
        rom[0..4].copy_from_slice(&0x00FF_FE00u32.to_be_bytes());
        rom[4..8].copy_from_slice(&0x0000_0200u32.to_be_bytes());
        rom[0x100..0x110].copy_from_slice(b"SEGA MEGA DRIVE ");
        // moveq #1,d0 / moveq #2,d1 / bra *
        rom[0x200..0x206].copy_from_slice(&[0x70, 0x01, 0x72, 0x02, 0x60, 0xFE]);
        Genesis::new(Cartridge::from_bytes(&rom).unwrap(), &Config::default())
    }

    #[test]
    fn breakpoint_key_uses_the_cursor_line() {
        let mut g = console();
        let mut d = Debugger::new();
        let mut panel = Panel::new();
        assert_eq!(panel.key(Key::Down, &mut g, &mut d), Action::None);
        panel.key(Key::Char('b'), &mut g, &mut d);
        assert_eq!(d.breakpoints(), &[0x202]);
        panel.key(Key::Char('b'), &mut g, &mut d);
        assert!(d.breakpoints().is_empty());
        assert_eq!(panel.key(Key::Char('s'), &mut g, &mut d), Action::Step);
    }

    #[test]
    fn goto_address_then_breakpoint() {
        let mut g = console();
        let mut d = Debugger::new();
        let mut panel = Panel::new();
        for k in [
            Key::Char('g'),
            Key::Char('2'),
            Key::Char('0'),
            Key::Char('4'),
            Key::Enter,
        ] {
            panel.key(k, &mut g, &mut d);
        }
        panel.key(Key::Char('b'), &mut g, &mut d);
        assert_eq!(d.breakpoints(), &[0x204]);
    }

    #[test]
    fn mute_keys_toggle_channels() {
        let mut g = console();
        let mut d = Debugger::new();
        let mut panel = Panel::new();
        panel.key(Key::Char('2'), &mut g, &mut d);
        panel.key(Key::Char('0'), &mut g, &mut d);
        assert_eq!(g.channel_mutes(), (0b10, 0b1000));
    }

    #[test]
    fn draw_shows_the_listing() {
        let g = console();
        let d = Debugger::new();
        let mut panel = Panel::new();
        let canvas = panel.draw(&g, &d, true);
        assert_eq!((canvas.width, canvas.height), (WIDTH, HEIGHT));
        // The PC bar is drawn behind the first listing line.
        assert!(canvas.pixels.contains(&PC_BAR));
    }
}
