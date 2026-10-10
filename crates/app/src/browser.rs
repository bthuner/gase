//! The built-in file browser's model: listing, filtering and sorting a
//! folder.
//!
//! SDL2 has no file dialog, and native dialogs differ on every system and
//! need extra libraries. A file browser drawn by the app itself works the
//! same everywhere a [`Platform::list_dir`]
//! exists, and with a gamepad from the sofa, which system dialogs do not.
//! (Platforms with a better native picker, like the web and phones, offer
//! that instead: [`Capabilities::rom_picker`](crate::Capabilities).)
//!
//! This module holds no drawing code: it is the data behind the browser
//! screen, so its rules can be tested with a fake platform.

use crate::platform::{DirEntry, Listing, Platform};

/// File extensions shown when filtering for ROMs.
pub const ROM_EXTENSIONS: &[&str] = &["bin", "md", "gen", "smd", "zip"];

/// Does `name` look like a ROM?
#[must_use]
pub fn is_rom(name: &str) -> bool {
    name.rsplit_once('.')
        .is_some_and(|(_, ext)| ROM_EXTENSIONS.iter().any(|e| e.eq_ignore_ascii_case(ext)))
}

/// Folders first, then files, each in case-insensitive name order (ties
/// broken case-sensitively so the order is always the same).
pub fn sort(entries: &mut [DirEntry]) {
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
}

/// Keep what the browser shows: folders and ROMs, or everything with
/// `show_all`. Hidden entries (starting with `.`) only with `show_all`.
#[must_use]
pub fn visible(entry: &DirEntry, show_all: bool) -> bool {
    if show_all {
        return true;
    }
    !entry.name.starts_with('.') && (entry.is_dir || is_rom(&entry.name))
}

/// A folder being browsed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Browser {
    /// The folder as listed by the platform.
    pub listing: Listing,
    /// What is shown: filtered and sorted.
    pub entries: Vec<DirEntry>,
    /// Why the folder could not be listed.
    pub error: Option<String>,
    pub show_all: bool,
}

impl Browser {
    /// Open `dir` (`None`: where the platform starts).
    #[must_use]
    pub fn open(platform: &mut dyn Platform, dir: Option<&str>) -> Self {
        let mut browser = Browser::default();
        browser.go(platform, dir);
        browser
    }

    /// List another folder. On failure the current listing stays and
    /// [`Browser::error`] says why.
    pub fn go(&mut self, platform: &mut dyn Platform, dir: Option<&str>) {
        match platform.list_dir(dir) {
            Ok(listing) => {
                self.listing = listing;
                self.error = None;
                self.refresh();
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// Re-apply the filter (after changing [`Browser::show_all`]).
    pub fn refresh(&mut self) {
        self.entries = self
            .listing
            .entries
            .iter()
            .filter(|e| visible(e, self.show_all))
            .cloned()
            .collect();
        sort(&mut self.entries);
    }

    /// The next entry after `after` whose name starts with `c` (wrapping
    /// around), for "type a letter to jump".
    #[must_use]
    pub fn jump(&self, c: char, after: Option<usize>) -> Option<usize> {
        let n = self.entries.len();
        let start = after.map_or(0, |i| i + 1);
        (0..n).map(|k| (start + k) % n).find(|&i| {
            self.entries[i]
                .name
                .chars()
                .next()
                .is_some_and(|first| first.eq_ignore_ascii_case(&c))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, is_dir: bool) -> DirEntry {
        DirEntry {
            name: name.into(),
            path: format!("/x/{name}"),
            is_dir,
        }
    }

    #[test]
    fn rom_extensions() {
        assert!(is_rom("Sonic.BIN"));
        assert!(is_rom("game.md") && is_rom("a.b.gen") && is_rom("x.smd") && is_rom("y.zip"));
        assert!(!is_rom("readme.txt") && !is_rom("bin") && !is_rom("save.srm"));
    }

    #[test]
    fn sorting_puts_folders_first() {
        let mut v = vec![
            entry("zeta.bin", false),
            entry("Alpha.md", false),
            entry("roms", true),
            entry("alpha.md", false),
            entry("Backups", true),
        ];
        sort(&mut v);
        let names: Vec<&str> = v.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            ["Backups", "roms", "Alpha.md", "alpha.md", "zeta.bin"]
        );
    }

    #[test]
    fn filtering() {
        assert!(visible(&entry("game.bin", false), false));
        assert!(visible(&entry("folder", true), false));
        assert!(!visible(&entry("notes.txt", false), false));
        assert!(!visible(&entry(".hidden", true), false));
        assert!(visible(&entry(".hidden", true), true));
        assert!(visible(&entry("notes.txt", false), true));
    }

    #[test]
    fn jumping_by_letter_wraps() {
        let b = Browser {
            entries: vec![
                entry("apple.bin", false),
                entry("banana.bin", false),
                entry("avocado.bin", false),
            ],
            ..Browser::default()
        };
        assert_eq!(b.jump('a', None), Some(0));
        assert_eq!(b.jump('a', Some(0)), Some(2));
        assert_eq!(b.jump('A', Some(2)), Some(0));
        assert_eq!(b.jump('z', None), None);
    }
}
