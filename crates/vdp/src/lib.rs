//! The Sega 315-5313 Video Display Processor (VDP).
//!
//! # What the VDP is
//!
//! The VDP is the Mega Drive's graphics chip. It owns its own memories, which
//! the CPUs can only reach through two I/O ports:
//!
//! * **VRAM** (64 KiB): tile patterns, name tables, the sprite table and the
//!   horizontal scroll table all live here, at programmable addresses.
//! * **CRAM** (64 × 9-bit words): four palettes of 16 colours, each colour
//!   `0000BBB0GGG0RRR0` (3 bits per channel, 512 possible colours).
//! * **VSRAM** (40 × 11-bit words): vertical scroll values.
//! * 24 8-bit **registers** configuring everything else.
//!
//! # What it draws
//!
//! The picture is built from 8×8-pixel, 4-bit-per-pixel *tiles* (32 bytes
//! each). Three layers are composited every line:
//!
//! * **Plane B** and **Plane A**: scrollable maps of tiles (32 to 128 cells
//!   wide/high). Each map entry picks a tile, a palette, flips and a priority.
//! * The **window**, which replaces plane A in a fixed rectangle (used for
//!   status bars that must not scroll).
//! * **Sprites**: up to 80 objects of 1×1 to 4×4 tiles, kept in a linked list.
//!
//! Each pixel has a *priority* bit. The final pixel is the first opaque one
//! in this order: high-priority sprite, high-priority A, high-priority B,
//! low-priority sprite, low-priority A, low-priority B, backdrop colour.
//! "Shadow/highlight" mode additionally darkens or brightens pixels.
//!
//! # How this implementation is organised
//!
//! * [`ports`](crate::Vdp::write_control): the CPU interface — control port
//!   commands, the data port and DMA transfers.
//! * `timing`: scanline bookkeeping, interrupts and the HV counter.
//! * `render`: a scanline renderer producing 32-bit pixels.
//! * `color`: the 9-bit colour to RGB conversion, including the non-linear
//!   output DAC and shadow/highlight intensities.
//!
//! The renderer works one scanline at a time: the system calls
//! [`Vdp::begin_line`] at the start of every line. Register writes made by the
//! CPUs during horizontal blanking (raster effects, typically from the
//! horizontal interrupt) therefore show up on the next line, which is what
//! virtually all games rely on.

mod color;
mod ports;
mod render;
mod timing;

use gase_savestate::{Error, Reader, State, Writer};

pub use timing::{HBLANK_START_CYCLE, MASTER_CYCLES_PER_LINE, VINT_CYCLE, VideoStandard};

/// Width of the frame buffer in pixels (the widest mode, H40).
pub const MAX_WIDTH: usize = 320;
/// Height of the frame buffer in pixels (240 lines, doubled in interlace mode 2).
pub const MAX_HEIGHT: usize = 480;

const VRAM_SIZE: usize = 0x1_0000;

/// Number of 4-byte entries in the sprite attribute cache (80 sprites).
const SAT_CACHE_SIZE: usize = 80 * 4;

/// The Video Display Processor.
#[derive(Clone)]
pub struct Vdp {
    /// Video RAM, stored as the CPU sees it: big-endian words.
    pub vram: Vec<u8>,
    /// Colour RAM: 64 entries of `0000BBB0GGG0RRR0`.
    pub cram: [u16; 64],
    /// Vertical scroll RAM: 40 entries.
    pub vsram: [u16; 40],
    /// The 24 VDP registers.
    pub regs: [u8; 24],

    // --- Control port state -------------------------------------------------
    /// The first half of a two-word command has been written.
    write_pending: bool,
    /// The "code" register: CD0-CD3 select the target memory and direction,
    /// CD5 requests a DMA.
    code: u8,
    /// The current VRAM/CRAM/VSRAM address.
    address: u16,
    /// A DMA fill has been set up and waits for the data port write that
    /// provides its value.
    dma_fill_pending: bool,
    /// A 68000-to-VDP DMA has been requested; the system bus must run it with
    /// [`Vdp::run_dma_68k`] because only the bus can read 68000 memory.
    dma_68k_pending: bool,
    /// Buffered word for data port reads.
    read_buffer: u16,

    // --- Status and interrupts ----------------------------------------------
    vint_pending: bool,
    hint_pending: bool,
    sprite_overflow: bool,
    sprite_collision: bool,
    odd_frame: bool,
    in_vblank: bool,
    hint_counter: u8,
    hv_latch: Option<u16>,
    standard: VideoStandard,

    // --- Timing --------------------------------------------------------------
    line: u16,

    /// Copy of the first 4 bytes (Y, size, link) of each sprite table entry.
    ///
    /// The real VDP keeps this internal cache, refreshed only when the CPU
    /// writes to VRAM inside the sprite table. If a game later moves the
    /// sprite table base, the cache keeps the *old* Y/size/link values. A few
    /// games (e.g. Castlevania: Bloodlines) depend on this.
    sat_cache: [u8; SAT_CACHE_SIZE],

    // --- Output (not part of the save state) ---------------------------------
    frame: Vec<u32>,
    frame_width: usize,
    frame_height: usize,
    palette: color::Palette,
    scratch: render::Scratch,
}

impl Vdp {
    /// Create a VDP in its power-on state.
    #[must_use]
    pub fn new(standard: VideoStandard) -> Self {
        let mut vdp = Self {
            vram: vec![0; VRAM_SIZE],
            cram: [0; 64],
            vsram: [0; 40],
            regs: [0; 24],
            write_pending: false,
            code: 0,
            address: 0,
            dma_fill_pending: false,
            dma_68k_pending: false,
            read_buffer: 0,
            vint_pending: false,
            hint_pending: false,
            sprite_overflow: false,
            sprite_collision: false,
            odd_frame: false,
            in_vblank: false,
            hint_counter: 0,
            hv_latch: None,
            standard,
            line: 0,
            sat_cache: [0; SAT_CACHE_SIZE],
            frame: vec![0; MAX_WIDTH * MAX_HEIGHT],
            frame_width: 320,
            frame_height: 224,
            palette: color::Palette::new(),
            scratch: render::Scratch::default(),
        };
        vdp.palette.rebuild(&vdp.cram);
        vdp
    }

    /// Reset the VDP. Memories keep their contents, as on hardware.
    pub fn reset(&mut self) {
        self.regs = [0; 24];
        self.write_pending = false;
        self.code = 0;
        self.address = 0;
        self.dma_fill_pending = false;
        self.dma_68k_pending = false;
        self.vint_pending = false;
        self.hint_pending = false;
        self.hv_latch = None;
    }

    /// The video standard (NTSC 60 Hz or PAL 50 Hz) the VDP runs at.
    #[must_use]
    pub fn standard(&self) -> VideoStandard {
        self.standard
    }

    /// Change the video standard (e.g. after loading a ROM from another region).
    pub fn set_standard(&mut self, standard: VideoStandard) {
        self.standard = standard;
    }

    /// The last completed frame as `0x00RRGGBB` pixels, `width` pixels per row
    /// with a row stride of [`MAX_WIDTH`].
    #[must_use]
    pub fn frame(&self) -> &[u32] {
        &self.frame
    }

    /// Size in pixels of the visible picture in [`Vdp::frame`].
    #[must_use]
    pub fn frame_size(&self) -> (usize, usize) {
        (self.frame_width, self.frame_height)
    }

    /// Is the 40-cell (320 pixel) horizontal mode selected? Otherwise 32 cells.
    #[must_use]
    pub fn h40(&self) -> bool {
        self.regs[12] & 0x01 != 0
    }

    /// Is the 30-cell (240 line) vertical mode selected? Only usable on PAL.
    #[must_use]
    pub fn v30(&self) -> bool {
        self.regs[1] & 0x08 != 0
    }

    fn display_enabled(&self) -> bool {
        self.regs[1] & 0x40 != 0
    }

    fn dma_enabled(&self) -> bool {
        self.regs[1] & 0x10 != 0
    }

    /// Interlace mode 2: double vertical resolution with 8×16 tiles (used by
    /// Sonic 2's split-screen mode).
    fn interlace_double(&self) -> bool {
        self.regs[12] & 0x06 == 0x06
    }

    fn interlaced(&self) -> bool {
        self.regs[12] & 0x02 != 0
    }

    fn auto_increment(&self) -> u16 {
        u16::from(self.regs[15])
    }
}

impl std::fmt::Debug for Vdp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vdp")
            .field("regs", &self.regs)
            .field("code", &self.code)
            .field("address", &self.address)
            .field("line", &self.line)
            .field("standard", &self.standard)
            .finish_non_exhaustive()
    }
}

impl State for Vdp {
    fn save(&self, w: &mut Writer) {
        self.vram.save(w);
        self.cram.save(w);
        self.vsram.save(w);
        self.regs.save(w);
        self.write_pending.save(w);
        self.code.save(w);
        self.address.save(w);
        self.dma_fill_pending.save(w);
        self.dma_68k_pending.save(w);
        self.read_buffer.save(w);
        self.vint_pending.save(w);
        self.hint_pending.save(w);
        self.sprite_overflow.save(w);
        self.sprite_collision.save(w);
        self.odd_frame.save(w);
        self.in_vblank.save(w);
        self.hint_counter.save(w);
        self.hv_latch.is_some().save(w);
        self.hv_latch.unwrap_or(0).save(w);
        u8::from(self.standard == VideoStandard::Pal).save(w);
        self.line.save(w);
        self.sat_cache.save(w);
    }

    fn load(&mut self, r: &mut Reader<'_>) -> Result<(), Error> {
        self.vram.load(r)?;
        self.cram.load(r)?;
        self.vsram.load(r)?;
        self.regs.load(r)?;
        self.write_pending.load(r)?;
        self.code.load(r)?;
        self.address.load(r)?;
        self.dma_fill_pending.load(r)?;
        self.dma_68k_pending.load(r)?;
        self.read_buffer.load(r)?;
        self.vint_pending.load(r)?;
        self.hint_pending.load(r)?;
        self.sprite_overflow.load(r)?;
        self.sprite_collision.load(r)?;
        self.odd_frame.load(r)?;
        self.in_vblank.load(r)?;
        self.hint_counter.load(r)?;
        let (mut latched, mut latch) = (false, 0u16);
        latched.load(r)?;
        latch.load(r)?;
        self.hv_latch = latched.then_some(latch);
        let mut pal = 0u8;
        pal.load(r)?;
        self.standard = if pal != 0 {
            VideoStandard::Pal
        } else {
            VideoStandard::Ntsc
        };
        self.line.load(r)?;
        self.sat_cache.load(r)?;
        self.palette.rebuild(&self.cram);
        Ok(())
    }
}
