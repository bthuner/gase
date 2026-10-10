//! Everything both frontends share: loading the ROM, game saves and
//! save-state files.
//!
//! Files live next to the ROM: `game.srm` for the game's own saves
//! (battery-backed SRAM or serial EEPROM, whichever the cartridge has),
//! `game.state0` ... `game.state9` for save states and `game-<frame>.png`
//! for screenshots.

use std::fs;
use std::path::{Path, PathBuf};

use gase_core::{Cartridge, Config, Genesis, SaveType};

use crate::cli::Options;
use crate::media;

/// A running game and the files that belong to it.
#[derive(Debug)]
pub struct Session {
    pub genesis: Genesis,
    rom_path: PathBuf,
}

impl Session {
    pub fn open(options: &Options, sample_rate: u32) -> Result<Self, String> {
        let data = fs::read(&options.rom)
            .map_err(|e| format!("cannot read {}: {e}", options.rom.display()))?;
        let cart =
            Cartridge::from_bytes(&data).map_err(|e| format!("{}: {e}", options.rom.display()))?;
        let config = Config {
            region: options.region,
            sample_rate,
            low_pass: options.low_pass,
            address_errors: options.address_errors,
        };
        let mut session = Self {
            genesis: Genesis::new(cart, &config),
            rom_path: options.rom.clone(),
        };
        session.load_save();
        session.genesis.set_trace(options.trace);
        Ok(session)
    }

    /// A one-line description of the game.
    pub fn describe(&self) -> String {
        let cart = self.genesis.cartridge();
        let h = &cart.header;
        let title = self.title();
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
            serial = h.serial,
            region = self.genesis.region(),
            kib = cart.rom().len() / 1024,
        )
    }

    /// Short name used for the window title and file names.
    pub fn title(&self) -> String {
        let h = &self.genesis.cartridge().header;
        let title = if h.overseas_title.is_empty() {
            &h.domestic_title
        } else {
            &h.overseas_title
        };
        if title.is_empty() {
            self.stem()
        } else {
            title.clone()
        }
    }

    fn stem(&self) -> String {
        self.rom_path
            .file_stem()
            .map_or_else(|| "game".into(), |s| s.to_string_lossy().into_owned())
    }

    fn sibling(&self, suffix: &str) -> PathBuf {
        let dir = self.rom_path.parent().unwrap_or_else(|| Path::new("."));
        dir.join(format!("{}{suffix}", self.stem()))
    }

    fn save_path(&self) -> PathBuf {
        self.sibling(".srm")
    }

    #[cfg_attr(not(feature = "sdl"), allow(dead_code))] // used by the window frontend only
    pub fn state_path(&self, slot: u8) -> PathBuf {
        self.sibling(&format!(".state{slot}"))
    }

    fn load_save(&mut self) {
        let path = self.save_path();
        if let (Some(save), Ok(data)) = (
            self.genesis.cartridge_mut().save_data_mut(),
            fs::read(&path),
        ) {
            let n = data.len().min(save.len());
            save[..n].copy_from_slice(&data[..n]);
            eprintln!("Loaded save {}", path.display());
        }
    }

    /// Write the save file (SRAM or EEPROM) if it changed since the last call.
    pub fn flush_save(&mut self) {
        if !self.genesis.cartridge_mut().take_save_dirty() {
            return;
        }
        if let Some(save) = self.genesis.cartridge().save_data() {
            let path = self.save_path();
            if let Err(e) = fs::write(&path, save) {
                eprintln!("Cannot write {}: {e}", path.display());
            }
        }
    }

    #[cfg_attr(not(feature = "sdl"), allow(dead_code))] // used by the window frontend only
    pub fn save_state(&self, slot: u8) -> Result<PathBuf, String> {
        let path = self.state_path(slot);
        fs::write(&path, self.genesis.save_state())
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        Ok(path)
    }

    #[cfg_attr(not(feature = "sdl"), allow(dead_code))] // used by the window frontend only
    pub fn load_state(&mut self, slot: u8) -> Result<PathBuf, String> {
        let path = self.state_path(slot);
        let data = fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        self.genesis
            .load_state(&data)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(path)
    }

    pub fn screenshot(&self, path: Option<&Path>) -> Result<PathBuf, String> {
        let path = path.map_or_else(
            || self.sibling(&format!("-{}.png", self.genesis.frame_count())),
            Path::to_path_buf,
        );
        fs::write(&path, media::frame_to_png(&self.genesis.frame()))
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        Ok(path)
    }

    /// Print trace lines collected during the last frame.
    pub fn print_trace(&mut self) {
        for line in self.genesis.take_trace() {
            println!("{line}");
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.flush_save();
    }
}
