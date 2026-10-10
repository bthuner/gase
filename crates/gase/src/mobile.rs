//! The phone half of the platform contract: files in the app's sandbox.
//!
//! # Where a phone app may write
//!
//! A desktop program can read and write wherever its user can. A phone app
//! is *sandboxed*: it gets a private folder of its own, and everything else
//! (the user's downloads, other apps' files) is only reachable through the
//! system's document picker, one file at a time, with the user's consent.
//!
//! So gase keeps all it writes in that private folder, which SDL finds for
//! us with `SDL_GetPrefPath` (`sdl2::filesystem::pref_path` in the shell):
//!
//! | System | The private folder | Picked ROMs are copied to |
//! |---|---|---|
//! | Android | `/data/data/io.github.bthuner.gase/files/` (`Context.getFilesDir()`) | `files/roms/` (by `GaseActivity.java`) |
//! | iOS | `<container>/Library/Application Support/gase/` | `<container>/Documents/roms/` (by `GaseAppDelegate.m`), visible in the Files app |
//!
//! Inside it:
//!
//! | What ([`FileKey`]) | Where |
//! |---|---|
//! | settings | `settings.cfg` |
//! | the game's save memory | `saves/<game>.srm` |
//! | save states | `states/<game>.state0` … `.state9`, with `.png` previews |
//! | screenshots | `screenshots/<game>-<frame>.png` |
//!
//! `<game>` is the ROM's file name without its extension: a ROM opened
//! again later, from another copy or after the app was reinstalled, finds
//! its saves.
//!
//! The system deletes this folder with the app, and backs it up with the
//! phone (Android Auto Backup, iCloud/iTunes backups) unless told
//! otherwise.
//!
//! # ROMs are copies
//!
//! The native glue (Java on Android, Objective-C on iOS) copies every ROM
//! the user picks into the sandbox before handing its path to Rust, so a
//! ROM can be opened again from the recent list without asking the user.
//! On iOS the container's path changes when the app is updated, so a
//! recent entry that no longer exists is looked up by its file name in the
//! ROM folders ([`MobileFiles::read_rom`]).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use gase_app::{Capabilities, FileKey, Platform};

/// What the app may offer on a phone or tablet.
#[must_use]
pub fn phone_capabilities(sample_rate: u32) -> Capabilities {
    Capabilities {
        sample_rate,
        touch_screen: true,
        // A phone's file system is not the user's to browse: ROMs come
        // through the system's document picker.
        file_browser: false,
        rom_picker: true,
        drop_files: false,
        // Android and iOS apps do not quit; the system closes them.
        can_quit: false,
        fullscreen: false,
        debugger: false,
        keyboard: false,
    }
}

/// File storage for phones and tablets: see the [module documentation](self).
#[derive(Debug)]
pub struct MobileFiles {
    start: Instant,
    /// The app's private folder.
    data: PathBuf,
    /// Where copies of picked ROMs are kept, searched by file name when a
    /// remembered path no longer exists.
    rom_dirs: Vec<PathBuf>,
}

/// `/x/Sonic The Hedgehog (USA).md` → `Sonic The Hedgehog (USA)`.
fn game_name(rom: &str) -> String {
    // Both separators: a name may come from another system.
    let file = rom.rsplit(['/', '\\']).next().unwrap_or(rom);
    if matches!(file, "" | "." | "..") {
        return "game".into();
    }
    match file.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem.into(),
        _ => file.into(),
    }
}

impl MobileFiles {
    /// Storage in `data` (the app's private folder), looking for ROMs in
    /// `rom_dirs` when a remembered path is gone.
    #[must_use]
    pub fn new(data: PathBuf, rom_dirs: Vec<PathBuf>) -> Self {
        Self {
            start: Instant::now(),
            data,
            rom_dirs,
        }
    }

    /// The storage of this system, given its private folder: on iOS the
    /// ROM copies are in `Documents` (shared with the Files app), on
    /// Android in `files/roms`.
    #[must_use]
    pub fn for_this_system(data: PathBuf) -> Self {
        let mut rom_dirs = vec![data.join("roms")];
        if cfg!(target_os = "ios") {
            // iOS sets HOME to the app's container.
            if let Some(home) = std::env::var_os("HOME") {
                let documents = Path::new(&home).join("Documents");
                // `Inbox` is where iOS puts files opened with "Open in…".
                rom_dirs.extend([documents.join("roms"), documents.join("Inbox"), documents]);
            }
        }
        Self::new(data, rom_dirs)
    }

    /// The app's private folder.
    #[must_use]
    pub fn data_dir(&self) -> &Path {
        &self.data
    }

    /// The file a [`FileKey`] is stored in.
    #[must_use]
    pub fn path(&self, file: FileKey<'_>) -> PathBuf {
        let d = &self.data;
        match file {
            FileKey::Settings => d.join("settings.cfg"),
            FileKey::Save { rom } => d.join("saves").join(format!("{}.srm", game_name(rom))),
            FileKey::State { rom, slot } => d
                .join("states")
                .join(format!("{}.state{slot}", game_name(rom))),
            FileKey::StatePreview { rom, slot } => d
                .join("states")
                .join(format!("{}.state{slot}.png", game_name(rom))),
            FileKey::Screenshot { rom, frame } => d
                .join("screenshots")
                .join(format!("{}-{frame}.png", game_name(rom))),
        }
    }
}

impl Platform for MobileFiles {
    fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    fn log(&mut self, message: &str) {
        eprintln!("{message}");
    }

    fn load(&mut self, file: FileKey<'_>) -> Option<Vec<u8>> {
        fs::read(self.path(file)).ok()
    }

    fn store(&mut self, file: FileKey<'_>, data: &[u8]) -> Result<String, String> {
        let path = self.path(file);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
        // Write a new file and rename it over the old one: if the system
        // kills the app halfway (it may, at any time, in the background),
        // the old save survives instead of a half-written one.
        let partial = path.with_extension("partial");
        fs::write(&partial, data)
            .and_then(|()| fs::rename(&partial, &path))
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        // The user cannot see these paths; name the place instead.
        let shown = path.strip_prefix(&self.data).unwrap_or(&path);
        Ok(shown.display().to_string())
    }

    fn read_rom(&mut self, path: &str) -> Result<Vec<u8>, String> {
        let error = match fs::read(path) {
            Ok(data) => return Ok(data),
            Err(e) => e,
        };
        // A copy we made earlier, under another container path?
        let name = Path::new(path).file_name();
        name.into_iter()
            .flat_map(|name| self.rom_dirs.iter().map(move |dir| dir.join(name)))
            .find_map(|candidate| fs::read(candidate).ok())
            .ok_or_else(|| format!("cannot read {path}: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gase-mobile-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn game_names_come_from_the_file_name() {
        assert_eq!(game_name("/a/b/Sonic (USA).md"), "Sonic (USA)");
        assert_eq!(game_name("C:\\roms\\x.bin"), "x");
        assert_eq!(game_name("noext"), "noext");
        assert_eq!(game_name(".hidden"), ".hidden");
        assert_eq!(game_name("a.b.gen"), "a.b");
        assert_eq!(game_name(""), "game");
        assert_eq!(game_name("/roms/.."), "game");
    }

    #[test]
    fn everything_goes_into_the_private_folder() {
        let m = MobileFiles::new(PathBuf::from("/data/files"), Vec::new());
        let rom = "/data/files/roms/Sonic.md";
        assert_eq!(
            m.path(FileKey::Settings),
            PathBuf::from("/data/files/settings.cfg")
        );
        assert_eq!(
            m.path(FileKey::Save { rom }),
            PathBuf::from("/data/files/saves/Sonic.srm")
        );
        assert_eq!(
            m.path(FileKey::State { rom, slot: 2 }),
            PathBuf::from("/data/files/states/Sonic.state2")
        );
        assert_eq!(
            m.path(FileKey::StatePreview { rom, slot: 2 }),
            PathBuf::from("/data/files/states/Sonic.state2.png")
        );
        assert_eq!(
            m.path(FileKey::Screenshot { rom, frame: 9 }),
            PathBuf::from("/data/files/screenshots/Sonic-9.png")
        );
        // A ROM that came as bytes, known by name only, saves the same way.
        assert_eq!(
            m.path(FileKey::Save { rom: "Sonic.md" }),
            m.path(FileKey::Save { rom })
        );
    }

    #[test]
    fn stores_atomically_and_reads_back() {
        let dir = temp("store");
        let mut m = MobileFiles::new(dir.clone(), Vec::new());
        let rom = "x/Game.bin";
        assert_eq!(m.load(FileKey::Save { rom }), None);
        let shown = m.store(FileKey::Save { rom }, b"one").unwrap();
        assert_eq!(Path::new(&shown), Path::new("saves/Game.srm"));
        m.store(FileKey::Save { rom }, b"two").unwrap();
        assert_eq!(m.load(FileKey::Save { rom }).as_deref(), Some(&b"two"[..]));
        // No partial file is left behind.
        let names: Vec<_> = fs::read_dir(dir.join("saves"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["Game.srm"]);
        assert!(m.list_dir(None).is_err(), "no file browser on phones");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_moved_rom_is_found_by_its_name() {
        let dir = temp("roms");
        let roms = dir.join("roms");
        fs::create_dir_all(&roms).unwrap();
        fs::write(roms.join("Game.md"), [1, 2, 3]).unwrap();
        let mut m = MobileFiles::new(dir.clone(), vec![roms.clone()]);
        // The path it was opened with, under the container's old name.
        let old = "/var/mobile/Containers/Data/Application/OLD/Documents/roms/Game.md";
        assert_eq!(m.read_rom(old).unwrap(), [1, 2, 3]);
        let direct = roms.join("Game.md");
        assert_eq!(m.read_rom(direct.to_str().unwrap()).unwrap(), [1, 2, 3]);
        let err = m.read_rom("/nowhere/Other.md").unwrap_err();
        assert!(err.contains("/nowhere/Other.md"), "{err}");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn this_system_keeps_rom_copies_in_roms() {
        let m = MobileFiles::for_this_system(PathBuf::from("/p"));
        assert_eq!(m.rom_dirs[0], PathBuf::from("/p/roms"));
        assert_eq!(m.data_dir(), Path::new("/p"));
    }
}
