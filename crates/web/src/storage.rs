//! Where the app's files go in the browser: keys of one IndexedDB store.
//!
//! The desktop keeps saves next to the ROM; a web page has no folders, only
//! storage keyed by strings. Every [`FileKey`] becomes one key in the
//! page's IndexedDB object store (see `web/storage.js`), and ROMs the user
//! picked are kept there too, so the recent list still works tomorrow:
//!
//! | What | Key |
//! |---|---|
//! | settings | `settings.cfg` |
//! | the game's save memory | `save/<rom>` |
//! | save state 3 and its picture | `state/<rom>/3`, `preview/<rom>/3` |
//! | a screenshot | `screenshot/<stem>-<frame>.png`: downloaded, not kept |
//! | a picked ROM | `rom/<name>` (the page keeps the 8 most recently used) |
//!
//! # Synchronous calls over an asynchronous store
//!
//! IndexedDB only has asynchronous calls, but [`Platform::load`] must
//! answer at once (the app reads a save while opening a game). The page
//! solves this the way many ports of native programs do: at startup it
//! reads **every** stored file into a JavaScript `Map` (a few hundred
//! kilobytes, plus the recent ROMs), then serves reads from the map
//! synchronously and writes **behind**: a write updates the map at once
//! and is copied to IndexedDB a moment later, in one transaction for
//! everything written in that frame.
//!
//! [`Platform::load`]: gase_app::Platform::load

use gase_app::FileKey;
use gase_app::game::file_stem;

/// The prefix of picked ROMs' keys.
pub const ROM_PREFIX: &str = "rom/";
/// The prefix of keys the page downloads instead of storing.
pub const DOWNLOAD_PREFIX: &str = "screenshot/";

/// The storage key of a file.
#[must_use]
pub fn key(file: FileKey<'_>) -> String {
    match file {
        FileKey::Settings => "settings.cfg".to_string(),
        FileKey::Save { rom } => format!("save/{rom}"),
        FileKey::State { rom, slot } => format!("state/{rom}/{slot}"),
        FileKey::StatePreview { rom, slot } => format!("preview/{rom}/{slot}"),
        FileKey::Screenshot { rom, frame } => {
            format!("{DOWNLOAD_PREFIX}{}-{frame}.png", file_stem(rom))
        }
    }
}

/// The storage key of a picked ROM.
#[must_use]
pub fn rom_key(name: &str) -> String {
    format!("{ROM_PREFIX}{name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys() {
        let rom = "Sonic (W).md";
        assert_eq!(key(FileKey::Settings), "settings.cfg");
        assert_eq!(key(FileKey::Save { rom }), "save/Sonic (W).md");
        assert_eq!(key(FileKey::State { rom, slot: 3 }), "state/Sonic (W).md/3");
        assert_eq!(
            key(FileKey::StatePreview { rom, slot: 0 }),
            "preview/Sonic (W).md/0"
        );
        assert_eq!(
            key(FileKey::Screenshot { rom, frame: 42 }),
            "screenshot/Sonic (W)-42.png"
        );
        assert_eq!(
            key(FileKey::Screenshot {
                rom: ".hidden",
                frame: 1
            }),
            "screenshot/.hidden-1.png"
        );
        assert_eq!(rom_key(rom), "rom/Sonic (W).md");
    }
}
