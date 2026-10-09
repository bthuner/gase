//! Everything both frontends share: loading the ROM, battery saves and
//! save-state files.
//!
//! Files live next to the ROM: `game.srm` for battery-backed RAM,
//! `game.state0` ... `game.state9` for save states and `game-<frame>.png`
//! for screenshots.

use std::fs;
use std::path::{Path, PathBuf};

use gase_core::{Cartridge, Config, Genesis};

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
        let data = fs::read(&options.rom).map_err(|e| format!("cannot read {}: {e}", options.rom.display()))?;
        let cart = Cartridge::from_bytes(&data).map_err(|e| format!("{}: {e}", options.rom.display()))?;
        let config = Config { region: options.region, sample_rate, low_pass: options.low_pass };
        let mut session = Self { genesis: Genesis::new(cart, &config), rom_path: options.rom.clone() };
        session.load_sram();
        session.genesis.set_trace(options.trace);
        Ok(session)
    }

    /// A one-line description of the game.
    pub fn describe(&self) -> String {
        let cart = self.genesis.cartridge();
        let h = &cart.header;
        let title = if h.overseas_title.is_empty() { &h.domestic_title } else { &h.overseas_title };
        let checksum = if cart.computed_checksum() == h.checksum { "ok" } else { "mismatch" };
        format!(
            "{title} [{serial}] region {region:?}, {kib} KiB ROM{sram}, checksum {checksum}",
            serial = h.serial,
            region = self.genesis.region(),
            kib = cart.rom().len() / 1024,
            sram = if cart.sram.is_some() { ", battery save" } else { "" },
        )
    }

    /// Short name used for the window title and file names.
    pub fn title(&self) -> String {
        let h = &self.genesis.cartridge().header;
        let title = if h.overseas_title.is_empty() { &h.domestic_title } else { &h.overseas_title };
        if title.is_empty() { self.stem() } else { title.clone() }
    }

    fn stem(&self) -> String {
        self.rom_path.file_stem().map_or_else(|| "game".into(), |s| s.to_string_lossy().into_owned())
    }

    fn sibling(&self, suffix: &str) -> PathBuf {
        let dir = self.rom_path.parent().unwrap_or_else(|| Path::new("."));
        dir.join(format!("{}{suffix}", self.stem()))
    }

    fn sram_path(&self) -> PathBuf {
        self.sibling(".srm")
    }

    pub fn state_path(&self, slot: u8) -> PathBuf {
        self.sibling(&format!(".state{slot}"))
    }

    fn load_sram(&mut self) {
        let path = self.sram_path();
        if let (Some(sram), Ok(data)) = (self.genesis.cartridge_mut().sram.as_mut(), fs::read(&path)) {
            let n = data.len().min(sram.data.len());
            sram.data[..n].copy_from_slice(&data[..n]);
            eprintln!("Loaded battery save {}", path.display());
        }
    }

    /// Write the battery save if it changed since the last call.
    pub fn flush_sram(&mut self) {
        if !self.genesis.cartridge_mut().take_sram_dirty() {
            return;
        }
        if let Some(sram) = &self.genesis.cartridge().sram {
            let path = self.sram_path();
            if let Err(e) = fs::write(&path, &sram.data) {
                eprintln!("Cannot write {}: {e}", path.display());
            }
        }
    }

    pub fn save_state(&self, slot: u8) -> Result<PathBuf, String> {
        let path = self.state_path(slot);
        fs::write(&path, self.genesis.save_state()).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        Ok(path)
    }

    pub fn load_state(&mut self, slot: u8) -> Result<PathBuf, String> {
        let path = self.state_path(slot);
        let data = fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        self.genesis.load_state(&data).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(path)
    }

    pub fn screenshot(&self, path: Option<&Path>) -> Result<PathBuf, String> {
        let path = path.map_or_else(|| self.sibling(&format!("-{}.png", self.genesis.frame_count())), Path::to_path_buf);
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
        self.flush_sram();
    }
}
