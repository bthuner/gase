//! The desktop half of the platform contract: files and folders with
//! `std::fs`. The SDL shell adds sound and windows on top
//! ([`crate::sdl`]); the headless runner uses it alone.
//!
//! Where things go:
//!
//! | What ([`FileKey`]) | Where |
//! |---|---|
//! | settings | `$GASE_CONFIG_DIR`, else the system's configuration folder: `~/.config/gase/settings.cfg` (Linux), `~/Library/Application Support/gase/` (macOS), `%APPDATA%\gase\` (Windows) |
//! | the game's save memory | `game.srm` next to the ROM |
//! | save states | `game.state0` … `game.state9` next to the ROM, with `game.state0.png` previews |
//! | screenshots | `game-<frame>.png` next to the ROM |
//!
//! Keeping saves next to the ROM is what gase always did, and what users of
//! other emulators expect; it also means a folder of ROMs carries its
//! saves along when copied.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use gase_app::{DirEntry, FileKey, Listing, Platform};

/// File storage and folder listing for desktop systems.
#[derive(Debug)]
pub struct Desktop {
    start: Instant,
    /// Where the settings file lives (`None`: settings are not stored).
    config_dir: Option<PathBuf>,
}

/// The folder for gase's settings on this system.
fn default_config_dir() -> Option<PathBuf> {
    let env = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if let Some(dir) = env("GASE_CONFIG_DIR") {
        return Some(dir);
    }
    if cfg!(windows) {
        return env("APPDATA").map(|d| d.join("gase"));
    }
    if cfg!(target_os = "macos") {
        return env("HOME").map(|d| d.join("Library/Application Support/gase"));
    }
    env("XDG_CONFIG_HOME")
        .or_else(|| env("HOME").map(|h| h.join(".config")))
        .map(|d| d.join("gase"))
}

/// `game.bin` + `.srm` → `game.srm`, in the ROM's folder.
fn sibling(rom: &str, suffix: &str) -> PathBuf {
    let rom = Path::new(rom);
    let stem = rom
        .file_stem()
        .map_or_else(|| "game".into(), |s| s.to_string_lossy().into_owned());
    let dir = rom.parent().unwrap_or_else(|| Path::new(""));
    dir.join(format!("{stem}{suffix}"))
}

impl Desktop {
    #[must_use]
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
            config_dir: default_config_dir(),
        }
    }

    /// The file a [`FileKey`] is stored in.
    #[must_use]
    pub fn path(&self, file: FileKey<'_>) -> Option<PathBuf> {
        Some(match file {
            FileKey::Settings => self.config_dir.as_ref()?.join("settings.cfg"),
            FileKey::Save { rom } => sibling(rom, ".srm"),
            FileKey::State { rom, slot } => sibling(rom, &format!(".state{slot}")),
            FileKey::StatePreview { rom, slot } => sibling(rom, &format!(".state{slot}.png")),
            FileKey::Screenshot { rom, frame } => sibling(rom, &format!("-{frame}.png")),
        })
    }
}

impl Default for Desktop {
    fn default() -> Self {
        Self::new()
    }
}

impl Platform for Desktop {
    fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    fn log(&mut self, message: &str) {
        eprintln!("{message}");
    }

    fn load(&mut self, file: FileKey<'_>) -> Option<Vec<u8>> {
        fs::read(self.path(file)?).ok()
    }

    fn store(&mut self, file: FileKey<'_>, data: &[u8]) -> Result<String, String> {
        let path = self.path(file).ok_or("no folder to store settings in")?;
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
        fs::write(&path, data).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        Ok(path.display().to_string())
    }

    fn read_rom(&mut self, path: &str) -> Result<Vec<u8>, String> {
        fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))
    }

    fn list_dir(&mut self, dir: Option<&str>) -> Result<Listing, String> {
        let dir = match dir {
            Some(d) => PathBuf::from(d),
            None => std::env::current_dir().map_err(|e| e.to_string())?,
        };
        // An absolute, tidy path makes a better title and a reliable parent.
        let dir = fs::canonicalize(&dir).unwrap_or(dir);
        let read = fs::read_dir(&dir).map_err(|e| format!("cannot open {}: {e}", dir.display()))?;
        let entries = read
            .filter_map(Result::ok)
            .map(|entry| {
                let path = entry.path();
                DirEntry {
                    name: entry.file_name().to_string_lossy().into_owned(),
                    // `metadata` follows symbolic links to folders.
                    is_dir: fs::metadata(&path).is_ok_and(|m| m.is_dir()),
                    path: path.to_string_lossy().into_owned(),
                }
            })
            .collect();
        Ok(Listing {
            path: dir.to_string_lossy().into_owned(),
            parent: dir.parent().map(|p| p.to_string_lossy().into_owned()),
            entries,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_go_next_to_the_rom() {
        let d = Desktop::new();
        let rom = "/roms/sonic.bin";
        assert_eq!(
            d.path(FileKey::Save { rom }),
            Some(PathBuf::from("/roms/sonic.srm"))
        );
        assert_eq!(
            d.path(FileKey::State { rom, slot: 3 }),
            Some(PathBuf::from("/roms/sonic.state3"))
        );
        assert_eq!(
            d.path(FileKey::StatePreview { rom, slot: 3 }),
            Some(PathBuf::from("/roms/sonic.state3.png"))
        );
        assert_eq!(
            d.path(FileKey::Screenshot { rom, frame: 77 }),
            Some(PathBuf::from("/roms/sonic-77.png"))
        );
        // A ROM known only by name (dropped bytes) saves in the current folder.
        assert_eq!(
            d.path(FileKey::Save { rom: "x.md" }),
            Some(PathBuf::from("x.srm"))
        );
    }

    #[test]
    fn lists_a_real_folder() {
        let dir = std::env::temp_dir().join(format!("gase-desktop-test-{}", std::process::id()));
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("game.bin"), [0u8; 4]).unwrap();
        let mut d = Desktop::new();
        let listing = d.list_dir(Some(dir.to_str().unwrap())).unwrap();
        assert!(listing.parent.is_some());
        let mut names: Vec<(String, bool)> = listing
            .entries
            .iter()
            .map(|e| (e.name.clone(), e.is_dir))
            .collect();
        names.sort();
        assert_eq!(
            names,
            [("game.bin".to_string(), false), ("sub".to_string(), true)]
        );
        assert!(
            d.read_rom(&listing.entries.iter().find(|e| !e.is_dir).unwrap().path)
                .is_ok()
        );
        assert!(d.list_dir(Some("/definitely/not/here")).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
}
