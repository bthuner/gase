//! The whole console: two CPUs, the hardware, and the scheduler that keeps
//! them in step.
//!
//! # Scheduling
//!
//! All chips are derived from one master clock (53.69 MHz on NTSC). The
//! 68000 runs at master/7, the Z80 at master/15, the VDP outputs a scanline
//! every 3420 master clocks. We keep one master-clock timestamp per CPU and
//! run the frame line by line:
//!
//! 1. At the start of a line the VDP renders it.
//! 2. The 68000 executes instructions until the next event in the line
//!    (vertical interrupt, horizontal blanking, end of line). After every
//!    68000 instruction the Z80 runs until it has caught up to the same
//!    master clock, so the two CPUs never drift more than one instruction
//!    apart.
//! 3. The sound chips are synchronised lazily: whenever a CPU touches them,
//!    and at the end of each frame.
//! 4. So is the VDP's memory side: queued writes and DMA steps happen in the
//!    VDP's access slots, which are caught up (`Vdp::advance`) whenever a
//!    CPU touches a VDP port and at the end of each line. A data-port access
//!    that must wait for a slot adds a stall to the 68000's clock.
//!
//! While a 68000-to-VDP DMA runs the 68000 is frozen: instead of executing
//! instructions, the loop lets the VDP progress up to the next event and
//! moves the 68000's clock along, until the DMA ends. Line events (and so
//! rendering) still happen on time in the middle of a long transfer.
//!
//! This "catch-up" design is simple, fast, and accurate enough for virtually
//! all software, because the CPUs only interact through shared memory and
//! interrupts at well-defined points.

use gase_m68k::M68k;
use gase_savestate::{Reader, State, Writer};
use gase_sound::{DcBlocker, LowPass, Psg, Resampler, Ym2612};
use gase_vdp::{HBLANK_START_CYCLE, MASTER_CYCLES_PER_LINE, VINT_CYCLE, Vdp, VideoStandard};
use gase_z80::Z80;

use crate::audio::{AudioClock, native_rate};
use crate::bus::{Hardware, Z80Bus};
use crate::cartridge::Cartridge;
use crate::io::{Buttons, Device, Io, Region};

/// Options chosen when the console is created.
#[derive(Clone, Debug)]
pub struct Config {
    /// Console region; `None` picks one the cartridge supports.
    pub region: Option<Region>,
    /// Host audio sample rate in Hz.
    pub sample_rate: u32,
    /// Emulate the model 1 console's analogue low-pass filter.
    pub low_pass: bool,
    /// Raise 68000 address errors on odd word accesses, as the hardware does.
    /// Turning this off mimics lenient emulators, which a few buggy homebrew
    /// programs depend on (see [`gase_m68k::M68k::set_address_errors`]).
    pub address_errors: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            region: None,
            sample_rate: 48_000,
            low_pass: true,
            address_errors: true,
        }
    }
}

/// Errors when loading a save state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateError {
    /// The data is not a gase save state.
    NotASaveState,
    /// The save state was made by an incompatible version of gase.
    WrongVersion(u32),
    /// The save state belongs to a different game.
    WrongGame,
    /// The save state is damaged.
    Corrupt(gase_savestate::Error),
}

impl std::fmt::Display for StateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StateError::NotASaveState => f.write_str("not a gase save state"),
            StateError::WrongVersion(v) => {
                write!(f, "save state format version {v} is not supported")
            }
            StateError::WrongGame => f.write_str("save state belongs to a different game"),
            StateError::Corrupt(e) => write!(f, "save state is corrupt: {e}"),
        }
    }
}

impl std::error::Error for StateError {}

impl From<gase_savestate::Error> for StateError {
    fn from(e: gase_savestate::Error) -> Self {
        StateError::Corrupt(e)
    }
}

const STATE_MAGIC: &[u8; 4] = b"GASE";
/// Bump whenever the layout changes. 2: VDP write FIFO and DMA progress.
const STATE_VERSION: u32 = 2;

/// A borrowed view of the last rendered frame.
#[derive(Clone, Copy, Debug)]
pub struct Frame<'a> {
    /// `0x00RRGGBB` pixels.
    pub pixels: &'a [u32],
    pub width: usize,
    pub height: usize,
    /// Distance in pixels between the starts of two rows.
    pub stride: usize,
}

/// A Sega Mega Drive / Genesis.
#[derive(Clone, Debug)]
pub struct Genesis {
    pub m68k: M68k,
    pub z80: Z80,
    pub hw: Hardware,
    /// Master clock reached by the 68000.
    m68k_clock: u64,
    /// Master clock reached by the Z80.
    z80_clock: u64,
    /// Master clock at which the next scanline starts.
    next_line: u64,
    frame_count: u64,
    resampler: Resampler,
    low_pass: Option<LowPass>,
    dc_blocker: DcBlocker,
    audio_out: Vec<i16>,
    /// Number of 68000 instructions still to be traced.
    trace_remaining: u64,
    /// Disassembled instructions collected for the frontend to print.
    trace_log: Vec<String>,
}

/// Pick a region the cartridge supports, preferring the Americas, then
/// Japan, then Europe.
fn auto_region(cart: &Cartridge) -> Region {
    let r = cart.header.regions;
    if r.americas {
        Region::Americas
    } else if r.japan {
        Region::Japan
    } else if r.europe {
        Region::Europe
    } else {
        Region::Americas
    }
}

impl Genesis {
    /// Insert `cart` and power the console on.
    #[must_use]
    pub fn new(cart: Cartridge, config: &Config) -> Self {
        let region = config.region.unwrap_or_else(|| auto_region(&cart));
        let standard = if region.is_pal() {
            VideoStandard::Pal
        } else {
            VideoStandard::Ntsc
        };
        let rate = native_rate(standard.master_clock());
        let hw = Hardware {
            cart,
            ram: vec![0; 0x1_0000],
            zram: vec![0; 0x2000],
            vdp: Vdp::new(standard),
            ym: Ym2612::new(),
            psg: Psg::new(),
            io: Io::new(region),
            audio: AudioClock::default(),
            z80_busreq: false,
            z80_reset: true,
            z80_bank: 0,
            z80_irq: false,
            reset_requested: false,
            now: 0,
            line_start: 0,
            m68k_wait: 0,
            vdp_stall: 0,
        };
        let mut genesis = Self {
            m68k: M68k::new(),
            z80: Z80::new(),
            hw,
            m68k_clock: 0,
            z80_clock: 0,
            next_line: 0,
            frame_count: 0,
            resampler: {
                // Six FM channels plus the PSG can exceed the 16-bit range;
                // halving leaves headroom for loud games without clipping.
                let mut r = Resampler::new(rate, f64::from(config.sample_rate));
                r.set_gain(0.5);
                r
            },
            // The YM2612's ladder effect adds a constant offset; real
            // consoles remove it with an output capacitor.
            dc_blocker: DcBlocker::new(10.0, rate),
            low_pass: config.low_pass.then(|| LowPass::new(3390.0, rate)),
            audio_out: Vec::new(),
            trace_remaining: 0,
            trace_log: Vec::new(),
        };
        genesis.hw.io.ports[0].device = Device::SixButton;
        genesis.hw.io.ports[1].device = Device::SixButton;
        genesis.m68k.set_address_errors(config.address_errors);
        genesis.m68k.reset(&mut genesis.hw);
        genesis
    }

    /// Press the console's reset button.
    ///
    /// Only the 68000 (and the devices on its reset line) is reset; memory,
    /// VRAM and the Z80 program survive, which is why some games behave
    /// differently after a soft reset.
    pub fn reset(&mut self) {
        self.hw.z80_reset = true;
        self.hw.z80_busreq = false;
        self.hw.sync_audio();
        self.hw.ym.reset();
        self.hw.vdp.reset();
        self.m68k.reset(&mut self.hw);
    }

    /// The console's region.
    #[must_use]
    pub fn region(&self) -> Region {
        self.hw.io.region
    }

    /// Frames per second for this console (≈59.92 NTSC, ≈49.70 PAL).
    #[must_use]
    pub fn frame_rate(&self) -> f64 {
        self.hw.vdp.standard().frame_rate()
    }

    /// Number of frames emulated since power-on.
    #[must_use]
    pub fn frame_count(&self) -> u64 {
        self.frame_count
    }

    /// The cartridge.
    #[must_use]
    pub fn cartridge(&self) -> &Cartridge {
        &self.hw.cart
    }

    /// Mutable access to the cartridge (to load or save its SRAM).
    pub fn cartridge_mut(&mut self) -> &mut Cartridge {
        &mut self.hw.cart
    }

    /// Set the buttons currently held on controller `port` (0 or 1).
    pub fn set_buttons(&mut self, port: usize, buttons: Buttons) {
        if let Some(p) = self.hw.io.ports.get_mut(port) {
            p.buttons = buttons;
        }
    }

    /// Choose what is plugged into controller `port`.
    pub fn set_device(&mut self, port: usize, device: Device) {
        if let Some(p) = self.hw.io.ports.get_mut(port) {
            p.device = device;
        }
    }

    /// The most recently completed frame.
    #[must_use]
    pub fn frame(&self) -> Frame<'_> {
        let (width, height) = self.hw.vdp.frame_size();
        Frame {
            pixels: self.hw.vdp.frame(),
            width,
            height,
            stride: gase_vdp::MAX_WIDTH,
        }
    }

    /// Move the audio produced so far (interleaved stereo at the host rate)
    /// to the end of `out`.
    pub fn drain_audio(&mut self, out: &mut Vec<i16>) {
        out.append(&mut self.audio_out);
    }

    /// Fine-tune the audio speed for audio/video synchronisation: a `ratio`
    /// slightly above 1.0 produces slightly more samples per frame.
    pub fn set_audio_speed(&mut self, ratio: f64) {
        let native = native_rate(self.hw.vdp.standard().master_clock());
        self.resampler.set_input_rate(native / ratio);
    }

    /// Emulate one complete video frame.
    pub fn run_frame(&mut self) {
        let total = self.hw.vdp.total_lines();
        let active = self.hw.vdp.active_lines();
        for line in 0..total {
            let start = self.next_line;
            self.hw.line_start = start;
            self.hw.vdp.begin_line(line);

            if line == active {
                self.run_until(start + u64::from(VINT_CYCLE));
                self.hw.vdp.trigger_vint();
                // The Z80 also receives the vertical interrupt, for about one line.
                self.hw.z80_irq = true;
            } else if line == active + 1 {
                self.hw.z80_irq = false;
            }

            self.run_until(start + u64::from(HBLANK_START_CYCLE));
            self.hw.vdp.hblank();
            self.run_until(start + u64::from(MASTER_CYCLES_PER_LINE));
            self.next_line = start + u64::from(MASTER_CYCLES_PER_LINE);
        }

        self.hw.now = self.next_line;
        self.hw.sync_audio();
        self.resample_audio();
        self.frame_count += 1;
    }

    /// Run both CPUs until the 68000 reaches master clock `target`.
    fn run_until(&mut self, target: u64) {
        while self.m68k_clock < target {
            if self.hw.vdp.dma_68k_active() {
                self.run_dma(target);
                continue;
            }
            self.hw.now = self.m68k_clock;
            self.m68k.set_interrupt_level(self.hw.vdp.interrupt_level());
            if self.trace_remaining > 0 {
                self.trace_instruction();
            }
            let mut cycles = self.m68k.step(&mut self.hw);
            cycles += std::mem::take(&mut self.hw.m68k_wait);
            self.hw.reset_requested = false;
            self.m68k_clock +=
                u64::from(cycles) * 7 + u64::from(std::mem::take(&mut self.hw.vdp_stall));
            self.run_z80_until(self.m68k_clock);
        }
    }

    fn run_z80_until(&mut self, target: u64) {
        if !self.hw.z80_running() {
            if self.hw.z80_reset {
                self.z80.reset();
            }
            self.z80_clock = self.z80_clock.max(target);
            return;
        }
        while self.z80_clock < target {
            self.hw.now = self.z80_clock;
            self.z80.set_irq(self.hw.z80_irq);
            let cycles = self.z80.step(&mut Z80Bus(&mut self.hw));
            self.z80_clock += u64::from(cycles) * 15;
        }
    }

    /// The 68000 is frozen by a 68000-to-VDP DMA: let the VDP move data up
    /// to `target` (or until the transfer ends, whichever comes first) and
    /// move the 68000's clock along without running it. The Z80 keeps
    /// running meanwhile.
    fn run_dma(&mut self, target: u64) {
        if self.hw.vdp.dma_68k_pending() {
            self.hw.start_dma_68k();
        }
        let line_start = self.hw.line_start;
        self.hw.vdp.advance((target - line_start) as u32);
        let resume = if self.hw.vdp.dma_68k_active() {
            target
        } else {
            line_start + u64::from(self.hw.vdp.dma_68k_done_at())
        };
        self.m68k_clock = self.m68k_clock.max(resume);
        self.run_z80_until(self.m68k_clock);
    }

    fn resample_audio(&mut self) {
        for (left, right) in self.hw.audio.samples.drain(..) {
            let (left, right) = self.dc_blocker.process(left, right);
            let (left, right) = match &mut self.low_pass {
                Some(filter) => filter.process(left, right),
                None => (left, right),
            };
            self.resampler.push(left, right);
        }
        self.resampler.drain(&mut self.audio_out);
    }

    /// Record the next `count` 68000 instructions (see [`Genesis::take_trace`]).
    pub fn set_trace(&mut self, count: u64) {
        self.trace_remaining = count;
    }

    /// Take the trace lines recorded so far.
    pub fn take_trace(&mut self) -> Vec<String> {
        std::mem::take(&mut self.trace_log)
    }

    fn trace_instruction(&mut self) {
        self.trace_remaining -= 1;
        let pc = self.m68k.pc();
        let hw = &mut self.hw;
        // Only disassemble from ROM and RAM: reading I/O has side effects.
        let (text, _) = gase_m68k::disasm::disassemble(pc, |addr| {
            if !(0x40_0000..0xE0_0000).contains(&addr) {
                hw.read_word_68k(addr)
            } else {
                0
            }
        });
        let d = &self.m68k.d;
        let a = &self.m68k.a;
        self.trace_log.push(format!(
            "{pc:06X}  {text:<28} SR={sr:04X} D0={:08X} D1={:08X} A0={:08X} A7={:08X}",
            d[0],
            d[1],
            a[0],
            a[7],
            sr = self.m68k.sr(),
        ));
    }

    /// Mute or unmute the PSG (for listening to the FM chip alone).
    pub fn set_psg_muted(&mut self, muted: bool) {
        self.hw.audio.psg_muted = muted;
    }

    /// Serialise the complete console state.
    #[must_use]
    pub fn save_state(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.bytes(STATE_MAGIC);
        STATE_VERSION.save(&mut w);
        self.game_id().save(&mut w);

        self.m68k.save(&mut w);
        self.z80.save(&mut w);
        self.hw.vdp.save(&mut w);
        self.hw.ym.save(&mut w);
        self.hw.psg.save(&mut w);
        self.hw.io.save(&mut w);
        self.hw.audio.save(&mut w);
        self.hw.ram.save(&mut w);
        self.hw.zram.save(&mut w);
        self.hw.z80_busreq.save(&mut w);
        self.hw.z80_reset.save(&mut w);
        self.hw.z80_bank.save(&mut w);
        self.hw.z80_irq.save(&mut w);
        self.hw.cart.sram_enabled.save(&mut w);
        self.hw.cart.banks.save(&mut w);
        if let Some(sram) = &self.hw.cart.sram {
            sram.data.save(&mut w);
        }
        self.m68k_clock.save(&mut w);
        self.z80_clock.save(&mut w);
        self.next_line.save(&mut w);
        self.frame_count.save(&mut w);
        w.into_bytes()
    }

    /// Restore a state produced by [`Genesis::save_state`].
    ///
    /// On error the console is left unchanged.
    pub fn load_state(&mut self, data: &[u8]) -> Result<(), StateError> {
        let mut r = Reader::new(data);
        if r.bytes(4).ok() != Some(STATE_MAGIC.as_slice()) {
            return Err(StateError::NotASaveState);
        }
        let mut version = 0u32;
        version.load(&mut r)?;
        if version != STATE_VERSION {
            return Err(StateError::WrongVersion(version));
        }
        let mut game = 0u32;
        game.load(&mut r)?;
        if game != self.game_id() {
            return Err(StateError::WrongGame);
        }

        // Load into a copy so a corrupt state cannot leave us half-updated.
        let mut next = self.clone();
        next.m68k.load(&mut r)?;
        next.z80.load(&mut r)?;
        next.hw.vdp.load(&mut r)?;
        next.hw.ym.load(&mut r)?;
        next.hw.psg.load(&mut r)?;
        next.hw.io.load(&mut r)?;
        next.hw.audio.load(&mut r)?;
        next.hw.ram.load(&mut r)?;
        next.hw.zram.load(&mut r)?;
        next.hw.z80_busreq.load(&mut r)?;
        next.hw.z80_reset.load(&mut r)?;
        next.hw.z80_bank.load(&mut r)?;
        next.hw.z80_irq.load(&mut r)?;
        next.hw.cart.sram_enabled.load(&mut r)?;
        next.hw.cart.banks.load(&mut r)?;
        if let Some(sram) = &mut next.hw.cart.sram {
            sram.data.load(&mut r)?;
        }
        next.m68k_clock.load(&mut r)?;
        next.z80_clock.load(&mut r)?;
        next.next_line.load(&mut r)?;
        next.frame_count.load(&mut r)?;
        next.hw.audio.samples.clear();
        *self = next;
        Ok(())
    }

    /// A cheap fingerprint of the ROM, to refuse states from other games.
    fn game_id(&self) -> u32 {
        // FNV-1a over the header and the ROM length.
        let rom = self.hw.cart.rom();
        let mut hash: u32 = 0x811C_9DC5;
        for &byte in rom[0x100..0x200]
            .iter()
            .chain(&(rom.len() as u32).to_le_bytes())
        {
            hash = (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193);
        }
        hash
    }
}
