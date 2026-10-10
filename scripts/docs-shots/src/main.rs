//! Pictures for the project site (`docs/`), made with the real emulator.
//!
//! ```text
//! gase-docs-shots ROM FRAMES OUT [--lenient] [--press BUTTON@FIRST-LAST]... [--layers] [--tiles]
//! ```
//!
//! Runs `ROM` for `FRAMES` frames (holding the given buttons on pad 1 during
//! the given frame ranges, like `crates/core/tests/test_roms.rs`) and saves
//! the last picture as `OUT.pam` (RGBA). With `--layers` it also saves the
//! VDP's layers one by one, as the renderer draws them:
//! `OUT-plane-b.pam`, `OUT-plane-a.pam`, `OUT-window.pam`, `OUT-sprites.pam`.
//! With `--tiles` it saves all 2048 VRAM tiles (`OUT-tiles.pam`, each tile in
//! the palette the planes or sprites use it with) and the 64 CRAM colours
//! (`OUT-cram.pam`, 16x4 pixels).
//!
//! How a layer is isolated: on a copy of the console, the other layers are
//! made transparent (their name tables point at an empty tile, sprites are
//! moved off-screen, the window is switched off) and the VDP renders the
//! frame again with `Vdp::begin_line`, twice, with two different backdrop
//! colours. Pixels that are the same in both pictures belong to the layer;
//! the others are backdrop and become transparent. The registers are those
//! at the end of the frame, so mid-frame raster effects are not reproduced.
//!
//! `scripts/png.py` turns the PAM files into small PNGs.

use std::path::Path;

use gase_core::{Buttons, Cartridge, Config, Genesis};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Layer {
    PlaneB,
    PlaneA,
    Window,
    Sprites,
}

impl Layer {
    const ALL: [Layer; 4] = [Layer::PlaneB, Layer::PlaneA, Layer::Window, Layer::Sprites];

    fn file_suffix(self) -> &'static str {
        match self {
            Layer::PlaneB => "plane-b",
            Layer::PlaneA => "plane-a",
            Layer::Window => "window",
            Layer::Sprites => "sprites",
        }
    }
}

fn parse_button(name: &str) -> Buttons {
    match name.to_ascii_lowercase().as_str() {
        "a" => Buttons::A,
        "b" => Buttons::B,
        "c" => Buttons::C,
        "start" => Buttons::START,
        "up" => Buttons::UP,
        "down" => Buttons::DOWN,
        "left" => Buttons::LEFT,
        "right" => Buttons::RIGHT,
        _ => panic!("unknown button '{name}'"),
    }
}

/// `start@300-305` -> (300, 305, START): held from frame 300 up to 304.
fn parse_press(spec: &str) -> (u64, u64, Buttons) {
    let (button, range) = spec.split_once('@').expect("--press BUTTON@FIRST-LAST");
    let (first, last) = range.split_once('-').expect("--press BUTTON@FIRST-LAST");
    (
        first.parse().expect("frame number"),
        last.parse().expect("frame number"),
        parse_button(button),
    )
}

fn plane_cells(bits: u8) -> usize {
    match bits & 3 {
        1 => 64,
        3 => 128,
        _ => 32,
    }
}

/// Name table address and size in bytes of plane A or B.
fn plane_table(regs: &[u8; 24], layer: Layer) -> (usize, usize) {
    let size = plane_cells(regs[16]) * plane_cells(regs[16] >> 4) * 2;
    match layer {
        Layer::PlaneA => (usize::from(regs[2] & 0x38) << 10, size),
        Layer::PlaneB => (usize::from(regs[4] & 0x07) << 13, size),
        _ => unreachable!(),
    }
}

/// VRAM ranges the VDP reads as tables rather than tiles: the name tables,
/// the sprite table and the horizontal scroll table.
fn table_ranges(regs: &[u8; 24]) -> Vec<(usize, usize)> {
    let h40 = regs[12] & 0x01 != 0;
    let mut ranges = vec![
        plane_table(regs, Layer::PlaneA),
        plane_table(regs, Layer::PlaneB),
    ];
    let window_mask = if h40 { 0x3C } else { 0x3E };
    ranges.push((usize::from(regs[3] & window_mask) << 10, 64 * 32 * 2));
    let sprite_mask = if h40 { 0x7E } else { 0x7F };
    ranges.push((usize::from(regs[5] & sprite_mask) << 9, 80 * 8));
    ranges.push((usize::from(regs[13] & 0x3F) << 10, 240 * 4));
    ranges
}

/// A tile whose 32 bytes are all zero (every pixel transparent) and that
/// no table overlaps, so hiding layers cannot change it.
fn empty_tile(vram: &[u8], regs: &[u8; 24]) -> u16 {
    let tables = table_ranges(regs);
    (0..2048)
        .find(|&t| {
            let (start, end) = (t * 32, t * 32 + 32);
            vram[start..end].iter().all(|&b| b == 0)
                && tables
                    .iter()
                    .all(|&(base, size)| end <= base || start >= base + size)
        })
        .expect("VRAM has no empty tile to hide planes with") as u16
}

/// Make `layer` invisible on `console`.
fn hide(console: &mut Genesis, layer: Layer) {
    let vdp = &mut console.hw.vdp;
    match layer {
        Layer::PlaneA | Layer::PlaneB => {
            let (base, size) = plane_table(&vdp.regs, layer);
            let entry = empty_tile(&vdp.vram, &vdp.regs).to_be_bytes();
            for i in (0..size).step_by(2) {
                vdp.vram[(base + i) & 0xFFFF] = entry[0];
                vdp.vram[(base + i + 1) & 0xFFFF] = entry[1];
            }
        }
        // Window position 0 on both axes (left of column 0, above row 0):
        // no window anywhere.
        Layer::Window => {
            vdp.regs[17] = 0;
            vdp.regs[18] = 0;
        }
        // X = 1 puts every sprite (at most 32 pixels wide) left of the
        // screen without triggering sprite masking (which X = 0 would).
        Layer::Sprites => {
            let mask = if vdp.h40() { 0x7E } else { 0x7F };
            let table = usize::from(vdp.regs[5] & mask) << 9;
            for sprite in 0..80 {
                let x = (table + sprite * 8 + 6) & 0xFFFF;
                vdp.vram[x] = 0;
                vdp.vram[x + 1] = 1;
            }
        }
    }
}

/// Render the visible frame again from the VDP's current state.
fn render(console: &mut Genesis) -> (Vec<u32>, usize, usize) {
    let vdp = &mut console.hw.vdp;
    let lines = if vdp.v30() { 240 } else { 224 };
    for line in 0..lines {
        vdp.begin_line(line);
    }
    let frame = console.frame();
    let mut pixels = Vec::with_capacity(frame.width * frame.height);
    for row in frame.pixels.chunks(frame.stride).take(frame.height) {
        pixels.extend_from_slice(&row[..frame.width]);
    }
    (pixels, frame.width, frame.height)
}

/// `pixels` with `None` for transparent ones.
type Picture = (Vec<Option<u32>>, usize, usize);

/// Only `keep`, with the backdrop transparent.
fn layer_picture(console: &Genesis, keep: Layer) -> Picture {
    let mut copy = console.clone();
    for layer in Layer::ALL {
        if layer != keep {
            hide(&mut copy, layer);
        }
    }
    // Two backdrops of different colours.
    let first = usize::from(copy.hw.vdp.regs[7] & 0x3F);
    let first_rgb = copy.hw.vdp.cram_rgb(first);
    let second = (0..64)
        .find(|&i| copy.hw.vdp.cram_rgb(i) != first_rgb)
        .unwrap_or((first + 1) % 64);
    let (one, width, height) = render(&mut copy);
    copy.hw.vdp.regs[7] = second as u8;
    let (two, _, _) = render(&mut copy);
    let pixels = one
        .iter()
        .zip(&two)
        .map(|(&a, &b)| (a == b).then_some(a))
        .collect();
    (pixels, width, height)
}

/// Every tile in VRAM, 64 per row, each drawn in the palette that the
/// planes or sprites use it with (palette 0 for tiles nothing points at).
fn tile_sheet(console: &Genesis) -> Picture {
    let vdp = &console.hw.vdp;
    let vram = &vdp.vram;
    let word = |addr: usize| u16::from_be_bytes([vram[addr & 0xFFFF], vram[(addr + 1) & 0xFFFF]]);
    let mut palette = [None::<usize>; 2048];
    for layer in [Layer::PlaneB, Layer::PlaneA] {
        let (base, size) = plane_table(&vdp.regs, layer);
        for i in (0..size).step_by(2) {
            let entry = word(base + i);
            palette[usize::from(entry & 0x7FF)].get_or_insert(usize::from(entry >> 13) & 3);
        }
    }
    let sprite_mask = if vdp.h40() { 0x7E } else { 0x7F };
    let table = usize::from(vdp.regs[5] & sprite_mask) << 9;
    for sprite in 0..80 {
        let size = vram[(table + sprite * 8 + 2) & 0xFFFF];
        let cells = ((usize::from(size >> 2) & 3) + 1) * ((usize::from(size) & 3) + 1);
        let entry = word(table + sprite * 8 + 4);
        for cell in 0..cells {
            palette[(usize::from(entry & 0x7FF) + cell) & 0x7FF]
                .get_or_insert(usize::from(entry >> 13) & 3);
        }
    }
    let (width, height) = (64 * 8, 32 * 8);
    let mut pixels = vec![None; width * height];
    for tile in 0..2048 {
        let line = palette[tile].unwrap_or(0) * 16;
        for y in 0..8 {
            for x in 0..8 {
                let byte = vram[tile * 32 + y * 4 + x / 2];
                let index = if x % 2 == 0 { byte >> 4 } else { byte & 0x0F };
                let rgb = vdp.cram_rgb(line + usize::from(index));
                pixels[((tile / 64) * 8 + y) * width + (tile % 64) * 8 + x] =
                    (index != 0).then_some(rgb);
            }
        }
    }
    (pixels, width, height)
}

/// The 64 CRAM colours: four rows (palettes) of sixteen 1-pixel swatches.
fn cram(console: &Genesis) -> Picture {
    let pixels = (0..64).map(|i| Some(console.hw.vdp.cram_rgb(i))).collect();
    (pixels, 16, 4)
}

/// Portable arbitrary map (PAM), RGBA: trivial to write, read by png.py.
fn write_pam(path: &Path, (pixels, width, height): &Picture) {
    let mut out = format!(
        "P7\nWIDTH {width}\nHEIGHT {height}\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n"
    )
    .into_bytes();
    for pixel in pixels {
        match pixel {
            Some(p) => out.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8, 255]),
            None => out.extend_from_slice(&[0, 0, 0, 0]),
        }
    }
    std::fs::write(path, out).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = "usage: gase-docs-shots ROM FRAMES OUT [--lenient] [--press BUTTON@FIRST-LAST]... [--layers] [--tiles]";
    assert!(args.len() >= 3, "{usage}");
    let rom = std::fs::read(&args[0]).unwrap_or_else(|e| panic!("{}: {e}", args[0]));
    let frames: u64 = args[1].parse().expect(usage);
    let out = &args[2];
    let mut lenient = false;
    let mut layers = false;
    let mut tiles = false;
    let mut presses = Vec::new();
    let mut rest = args[3..].iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--lenient" => lenient = true,
            "--layers" => layers = true,
            "--tiles" => tiles = true,
            "--press" => presses.push(parse_press(rest.next().expect(usage))),
            _ => panic!("{usage}"),
        }
    }

    let config = Config {
        address_errors: !lenient,
        ..Config::default()
    };
    let mut console = Genesis::new(Cartridge::from_bytes(&rom).expect("ROM"), &config);
    for frame in 0..frames {
        let mut buttons = Buttons::default();
        for &(first, last, b) in &presses {
            if (first..last).contains(&frame) {
                buttons = buttons | b;
            }
        }
        console.set_buttons(0, buttons);
        console.run_frame();
    }

    let frame = console.frame();
    let mut pixels = Vec::new();
    for row in frame.pixels.chunks(frame.stride).take(frame.height) {
        pixels.extend(row[..frame.width].iter().map(|&p| Some(p)));
    }
    let picture = (pixels, frame.width, frame.height);
    write_pam(Path::new(&format!("{out}.pam")), &picture);

    if tiles {
        write_pam(
            Path::new(&format!("{out}-tiles.pam")),
            &tile_sheet(&console),
        );
        write_pam(Path::new(&format!("{out}-cram.pam")), &cram(&console));
    }
    if layers {
        for layer in Layer::ALL {
            let picture = layer_picture(&console, layer);
            let opaque = picture.0.iter().filter(|p| p.is_some()).count();
            eprintln!("{out}: {} has {opaque} opaque pixels", layer.file_suffix());
            write_pam(
                Path::new(&format!("{out}-{}.pam", layer.file_suffix())),
                &picture,
            );
        }
    }
}
