//! A loaded game: the console, its save files, rewind history and the
//! debugger's execution state.

use gase_core::{Cartridge, Config, Debugger, Genesis, Rewind, SaveType, Stop};

use crate::canvas::{Canvas, Image};
use crate::platform::{FileKey, Platform};
use crate::png;
use crate::settings::Settings;

/// Size of the save-state pictures: a fifth of 320 × 224.
pub const PREVIEW: (usize, usize) = (64, 44);
/// Number of save-state slots.
pub const SLOTS: u8 = 10;
/// How often the game's save memory is written out if it changed.
pub const SAVE_FLUSH_MS: u64 = 5_000;

/// The file name in a path (either separator), for messages and titles.
#[must_use]
pub fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// The file name without its extension.
#[must_use]
pub fn file_stem(path: &str) -> &str {
    let name = file_name(path);
    match name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => name,
    }
}

/// What the debugger should run (see [`gase_core::Debugger`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DebugRun {
    /// One 68000 instruction.
    Instruction,
    /// To the end of the frame.
    Frame,
    /// To the next vertical interrupt.
    VBlank,
}

/// A save-state slot as shown in the menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotInfo {
    pub exists: bool,
    pub preview: Option<Canvas>,
}

/// A running game.
#[derive(Debug)]
pub struct Game {
    pub genesis: Genesis,
    /// The path or name the ROM was opened with (the platform's key for it
    /// and for its save files).
    pub rom: String,
    /// The title from the ROM header, or the file name.
    pub title: String,
    /// Recent history, if rewinding is enabled.
    pub rewind: Option<Rewind>,
    /// Breakpoints and the position inside a frame.
    pub debugger: Debugger,
    /// Paused without a menu (P key, or the debugger).
    pub paused: bool,
    /// Run one frame although paused.
    pub step_frame: bool,
    /// The slot used by the save/load hotkeys.
    pub slot: u8,
    last_flush_ms: u64,
}

impl Game {
    /// Insert the cartridge `data` (opened as `rom`) and power on, with the
    /// save memory restored from the platform.
    ///
    /// # Errors
    ///
    /// A message for the user if `data` is not a usable ROM.
    pub fn open(
        rom: &str,
        data: &[u8],
        settings: &Settings,
        sample_rate: u32,
        platform: &mut dyn Platform,
    ) -> Result<Self, String> {
        let cart = Cartridge::from_bytes(data).map_err(|e| format!("{}: {e}", file_name(rom)))?;
        let config = Config {
            region: settings.emulation.region,
            sample_rate,
            low_pass: settings.audio.low_pass,
            address_errors: !settings.emulation.lenient_address_errors,
        };
        let mut genesis = Genesis::new(cart, &config);
        for (port, player) in settings.players.iter().enumerate() {
            genesis.set_device(port, player.device);
        }
        let h = &genesis.cartridge().header;
        let title = if h.overseas_title.is_empty() {
            h.domestic_title.clone()
        } else {
            h.overseas_title.clone()
        };
        let title = if title.is_empty() {
            file_stem(rom).to_string()
        } else {
            title
        };
        let rewind = settings
            .emulation
            .rewind
            .then(|| Rewind::new(20, 4, genesis.frame_rate()));
        let mut game = Self {
            genesis,
            rom: rom.to_string(),
            title,
            rewind,
            debugger: Debugger::new(),
            paused: false,
            step_frame: false,
            slot: 0,
            last_flush_ms: platform.now_ms(),
        };
        if let Some(data) = platform.load(FileKey::Save { rom }) {
            if let Some(save) = game.genesis.cartridge_mut().save_data_mut() {
                let n = data.len().min(save.len());
                save[..n].copy_from_slice(&data[..n]);
                platform.log(&format!("Loaded the game's save ({n} bytes)"));
            }
        }
        Ok(game)
    }

    /// A one-line description for logs.
    #[must_use]
    pub fn describe(&self) -> String {
        let cart = self.genesis.cartridge();
        let h = &cart.header;
        let checksum = if cart.computed_checksum() == h.checksum {
            "ok"
        } else {
            "mismatch"
        };
        let save = match cart.save_type() {
            SaveType::None => String::new(),
            kind => format!(", {kind}"),
        };
        format!(
            "{title} [{serial}] region {region:?}, {kib} KiB ROM{save}, checksum {checksum}",
            title = self.title,
            serial = h.serial,
            region = self.genesis.region(),
            kib = cart.rom().len() / 1024,
        )
    }

    /// Write the game's save memory if it changed since the last write.
    pub fn flush_save(&mut self, platform: &mut dyn Platform) {
        self.last_flush_ms = platform.now_ms();
        if !self.genesis.cartridge_mut().take_save_dirty() {
            return;
        }
        if let Some(save) = self.genesis.cartridge().save_data() {
            if let Err(e) = platform.store(FileKey::Save { rom: &self.rom }, save) {
                platform.log(&format!("Cannot write the save: {e}"));
            }
        }
    }

    /// Flush the save memory if the last write was a while ago. Games
    /// write their saves a byte at a time; waiting a few seconds turns
    /// hundreds of small writes into one.
    pub fn flush_save_periodically(&mut self, platform: &mut dyn Platform) {
        if platform.now_ms().saturating_sub(self.last_flush_ms) >= SAVE_FLUSH_MS {
            self.flush_save(platform);
        }
    }

    /// Emulate `count` frames (fast-forward runs several). Returns the
    /// address of a breakpoint if the debugger stopped there.
    pub fn run_frames(&mut self, count: u32) -> Option<u32> {
        for _ in 0..count {
            // The plain frame loop is a little faster; the debugger's is
            // only needed when it has something to do.
            if self.debugger.breakpoints().is_empty() && !self.debugger.in_frame() {
                self.genesis.run_frame();
            } else if let Stop::Breakpoint(pc) = self.debugger.run_frame(&mut self.genesis) {
                self.paused = true;
                return Some(pc);
            }
            if let Some(rewind) = &mut self.rewind {
                rewind.record(&self.genesis);
            }
        }
        None
    }

    /// Go back one step in history and show that frame. False when the
    /// history is exhausted (or rewind is off).
    pub fn rewind_step(&mut self) -> bool {
        let Some(rewind) = &mut self.rewind else {
            return false;
        };
        if rewind.step_back(&mut self.genesis) {
            self.debugger.forget_position();
            self.genesis.run_frame();
            true
        } else {
            false
        }
    }

    /// Run under the debugger's control; the game stays paused.
    pub fn debug_run(&mut self, what: DebugRun) -> Stop {
        let stop = match what {
            DebugRun::Instruction => self.debugger.step_instruction(&mut self.genesis),
            DebugRun::Frame => self.debugger.run_frame(&mut self.genesis),
            DebugRun::VBlank => self.debugger.run_to_vblank(&mut self.genesis),
        };
        if stop == Stop::FrameEnd {
            if let Some(rewind) = &mut self.rewind {
                rewind.record(&self.genesis);
            }
        }
        self.paused = true;
        stop
    }

    /// A small picture of the current frame for the save-state menu.
    ///
    /// Each preview pixel is the **average** of the block of frame pixels
    /// it covers (a box filter). Simply picking every fifth pixel would
    /// make thin lines and text flicker in and out of existence.
    #[must_use]
    pub fn preview(&self) -> Canvas {
        let frame = self.genesis.frame();
        let (pw, ph) = PREVIEW;
        let mut out = Canvas::new(pw, ph, 0);
        for py in 0..ph {
            let (y0, y1) = (
                py * frame.height / ph,
                ((py + 1) * frame.height / ph).max(py * frame.height / ph + 1),
            );
            for px in 0..pw {
                let (x0, x1) = (
                    px * frame.width / pw,
                    ((px + 1) * frame.width / pw).max(px * frame.width / pw + 1),
                );
                let (mut r, mut g, mut b, mut n) = (0, 0, 0, 0);
                for y in y0..y1 {
                    for &p in &frame.pixels[y * frame.stride + x0..y * frame.stride + x1] {
                        r += (p >> 16) & 0xFF;
                        g += (p >> 8) & 0xFF;
                        b += p & 0xFF;
                        n += 1;
                    }
                }
                out.pixels[py * pw + px] = 0xFF00_0000 | (r / n) << 16 | (g / n) << 8 | (b / n);
            }
        }
        out
    }

    /// Save the console's state, and its picture, in `slot`.
    ///
    /// # Errors
    ///
    /// A message for the user if the platform could not store it (or the
    /// debugger stopped in the middle of a frame, which a state cannot
    /// describe).
    pub fn save_state(&mut self, slot: u8, platform: &mut dyn Platform) -> Result<(), String> {
        if self.debugger.in_frame() {
            return Err("Stopped mid-frame: finish the frame first".into());
        }
        let rom = &self.rom;
        platform.store(FileKey::State { rom, slot }, &self.genesis.save_state())?;
        let preview = self.preview();
        // The picture is optional: a failure here does not fail the save.
        let _ = platform.store(FileKey::StatePreview { rom, slot }, &preview.to_png());
        self.slot = slot;
        Ok(())
    }

    /// Load the state in `slot`.
    ///
    /// # Errors
    ///
    /// A message for the user if the slot is empty or the state unusable.
    pub fn load_state(&mut self, slot: u8, platform: &mut dyn Platform) -> Result<(), String> {
        let data = platform
            .load(FileKey::State {
                rom: &self.rom,
                slot,
            })
            .ok_or_else(|| format!("Slot {slot} is empty"))?;
        self.genesis
            .load_state(&data)
            .map_err(|e| format!("Slot {slot}: {e}"))?;
        if let Some(rewind) = &mut self.rewind {
            rewind.clear();
        }
        self.debugger.forget_position();
        self.slot = slot;
        Ok(())
    }

    /// What is in each slot, for the menu.
    #[must_use]
    pub fn slots(&self, platform: &mut dyn Platform) -> Vec<SlotInfo> {
        (0..SLOTS)
            .map(|slot| {
                let rom = &self.rom;
                let exists = platform.load(FileKey::State { rom, slot }).is_some();
                let preview = exists
                    .then(|| platform.load(FileKey::StatePreview { rom, slot }))
                    .flatten()
                    .and_then(|data| png::decode(&data))
                    .map(|(w, h, pixels)| {
                        let mut c = Canvas::new(w, h, 0);
                        c.pixels = pixels;
                        c
                    });
                SlotInfo { exists, preview }
            })
            .collect()
    }

    /// The current frame as an [`Image`].
    #[must_use]
    pub fn frame(&self) -> Image<'_> {
        self.genesis.frame().into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_from_paths() {
        assert_eq!(file_name("/roms/sonic.bin"), "sonic.bin");
        assert_eq!(file_name("C:\\roms\\sonic.md"), "sonic.md");
        assert_eq!(file_name("plain.gen"), "plain.gen");
        assert_eq!(file_stem("/roms/a.b.bin"), "a.b");
        assert_eq!(file_stem(".hidden"), ".hidden");
        assert_eq!(file_stem("noext"), "noext");
    }
}
