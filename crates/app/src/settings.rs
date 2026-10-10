//! Settings, and the little text format they are saved in.
//!
//! # The format
//!
//! One `key = value` per line; blank lines and lines starting with `#` are
//! ignored. Keys are dotted paths, values are plain words or numbers:
//!
//! ```text
//! # gase settings
//! video.integer_scale = false
//! audio.volume = 80
//! emulation.region = auto
//! input.p1.key.a = Z
//! input.p1.pad.a = West
//! recent = /home/me/roms/sonic.bin
//! recent = /home/me/roms/gunstar.md
//! ```
//!
//! Why not JSON or TOML? This format needs about a hundred lines to read
//! and write, no dependency, is pleasant to edit by hand, and its parser
//! can be **forgiving**: a typo on one line is reported with its line
//! number and that line is skipped, while every other setting still
//! loads. Unknown keys (from a newer version) are reported and skipped the
//! same way. A missing key keeps its default, so old files keep working.
//!
//! Lists (the recent ROMs) are simply the same key repeated, in order.
//!
//! Where the text is stored is the platform's business
//! ([`FileKey::Settings`](crate::FileKey::Settings)): a file in the user's
//! configuration directory on desktop, browser storage on the web.

use std::fmt::{self, Write as _};

use gase_core::{Device, Region};

use crate::input::{ConsoleButton, Key, PadButton, PlayerBindings};

/// How the game picture is fitted into the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Aspect {
    /// The whole picture in the 320 × 224 (10:7) shape most emulators and
    /// most games' artwork assume; 256-pixel-wide modes are stretched to
    /// the same width, as a television does.
    #[default]
    Console,
    /// Stretched to 4:3, the shape of a CRT television.
    Tv,
    /// Fill the window, whatever its shape.
    Stretch,
}

/// When to show the on-screen touch controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TouchMode {
    /// When the platform says it has a touch screen (or one was touched),
    /// except while a gamepad is being used.
    #[default]
    Auto,
    On,
    Off,
}

/// Picture settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Video {
    /// Initial window size as a multiple of 320 × 224 (desktop).
    pub scale: u32,
    /// Only scale by whole multiples: every console pixel gets the same
    /// number of screen pixels (sharpest, may leave borders).
    pub integer_scale: bool,
    pub aspect: Aspect,
    pub fullscreen: bool,
}

/// Sound settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Audio {
    /// 0-100 %.
    pub volume: u8,
    pub mute: bool,
    /// Emulate the model 1 console's analogue low-pass filter.
    pub low_pass: bool,
}

/// How the console behaves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Emulation {
    /// `None`: the first region the cartridge supports.
    pub region: Option<Region>,
    /// Ignore 68000 address errors, as lenient emulators do (for buggy
    /// homebrew; real hardware crashes).
    pub lenient_address_errors: bool,
    /// Keep recent history for rewinding.
    pub rewind: bool,
    /// Frames emulated per displayed frame while fast-forwarding (2-8).
    pub fast_forward: u32,
}

/// One player's controller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerInput {
    /// Three- or six-button pad (a few old games misbehave with six).
    pub device: Device,
    pub bindings: PlayerBindings,
}

/// Everything the user can change, plus a little remembered state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub video: Video,
    pub audio: Audio,
    pub emulation: Emulation,
    pub players: [PlayerInput; 2],
    pub touch: TouchMode,
    /// Recently opened ROMs, most recent first (platform paths or names).
    pub recent: Vec<String>,
    /// The folder the file browser showed last.
    pub browse_dir: Option<String>,
}

/// How many recent ROMs are remembered.
pub const MAX_RECENT: usize = 8;

impl Default for Settings {
    fn default() -> Self {
        Self {
            video: Video {
                scale: 3,
                integer_scale: false,
                aspect: Aspect::Console,
                fullscreen: false,
            },
            audio: Audio {
                volume: 100,
                mute: false,
                low_pass: true,
            },
            emulation: Emulation {
                region: None,
                lenient_address_errors: false,
                rewind: true,
                fast_forward: 4,
            },
            players: [0, 1].map(|p| PlayerInput {
                device: Device::SixButton,
                bindings: PlayerBindings::default_for(p),
            }),
            touch: TouchMode::Auto,
            recent: Vec::new(),
            browse_dir: None,
        }
    }
}

impl Settings {
    /// Put `rom` at the top of the recent list.
    pub fn add_recent(&mut self, rom: &str) {
        self.recent.retain(|r| r != rom);
        self.recent.insert(0, rom.to_string());
        self.recent.truncate(MAX_RECENT);
    }
}

/// A line of the settings text that could not be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    /// 1-based line number.
    pub line: usize,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ParseError {}

// --- Value conversions ---------------------------------------------------

fn parse_bool(v: &str) -> Result<bool, String> {
    match v.to_ascii_lowercase().as_str() {
        "true" | "on" | "yes" | "1" => Ok(true),
        "false" | "off" | "no" | "0" => Ok(false),
        _ => Err(format!("expected true or false, got '{v}'")),
    }
}

fn parse_int(v: &str, min: u32, max: u32) -> Result<u32, String> {
    match v.parse::<u32>() {
        Ok(n) if (min..=max).contains(&n) => Ok(n),
        _ => Err(format!("expected a number from {min} to {max}, got '{v}'")),
    }
}

fn region_name(r: Option<Region>) -> &'static str {
    match r {
        None => "auto",
        Some(Region::Japan) => "jp",
        Some(Region::Americas) => "us",
        Some(Region::Europe) => "eu",
    }
}

fn parse_region(v: &str) -> Result<Option<Region>, String> {
    Ok(match v.to_ascii_lowercase().as_str() {
        "auto" => None,
        "jp" => Some(Region::Japan),
        "us" => Some(Region::Americas),
        "eu" => Some(Region::Europe),
        _ => return Err(format!("expected auto, jp, us or eu, got '{v}'")),
    })
}

fn aspect_name(a: Aspect) -> &'static str {
    match a {
        Aspect::Console => "console",
        Aspect::Tv => "tv",
        Aspect::Stretch => "stretch",
    }
}

fn parse_aspect(v: &str) -> Result<Aspect, String> {
    Ok(match v.to_ascii_lowercase().as_str() {
        "console" => Aspect::Console,
        "tv" => Aspect::Tv,
        "stretch" => Aspect::Stretch,
        _ => return Err(format!("expected console, tv or stretch, got '{v}'")),
    })
}

fn touch_name(t: TouchMode) -> &'static str {
    match t {
        TouchMode::Auto => "auto",
        TouchMode::On => "on",
        TouchMode::Off => "off",
    }
}

fn parse_touch(v: &str) -> Result<TouchMode, String> {
    Ok(match v.to_ascii_lowercase().as_str() {
        "auto" => TouchMode::Auto,
        "on" => TouchMode::On,
        "off" => TouchMode::Off,
        _ => return Err(format!("expected auto, on or off, got '{v}'")),
    })
}

fn device_name(d: Device) -> &'static str {
    match d {
        Device::ThreeButton => "3button",
        Device::SixButton => "6button",
        Device::None => "none",
    }
}

fn parse_device(v: &str) -> Result<Device, String> {
    Ok(match v.to_ascii_lowercase().as_str() {
        "3button" => Device::ThreeButton,
        "6button" => Device::SixButton,
        "none" => Device::None,
        _ => return Err(format!("expected 3button, 6button or none, got '{v}'")),
    })
}

fn parse_binding<T>(v: &str, from_name: fn(&str) -> Option<T>) -> Result<Option<T>, String> {
    if v.eq_ignore_ascii_case("none") {
        return Ok(None);
    }
    from_name(v)
        .map(Some)
        .ok_or_else(|| format!("unknown key or button '{v}'"))
}

// --- Writing ---------------------------------------------------------------

impl Settings {
    /// The settings as text, every key written out (so the file documents
    /// what can be set).
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut s = String::from("# gase settings. Lines are `key = value`; edit freely.\n");
        let kv = |s: &mut String, k: &str, v: &dyn fmt::Display| {
            let _ = writeln!(s, "{k} = {v}");
        };
        s.push_str("\n# Picture. aspect: console, tv or stretch\n");
        kv(&mut s, "video.scale", &self.video.scale);
        kv(&mut s, "video.integer_scale", &self.video.integer_scale);
        kv(&mut s, "video.aspect", &aspect_name(self.video.aspect));
        kv(&mut s, "video.fullscreen", &self.video.fullscreen);
        s.push_str("\n# Sound. volume: 0-100\n");
        kv(&mut s, "audio.volume", &self.audio.volume);
        kv(&mut s, "audio.mute", &self.audio.mute);
        kv(&mut s, "audio.low_pass", &self.audio.low_pass);
        s.push_str("\n# Console. region: auto, jp, us or eu\n");
        kv(
            &mut s,
            "emulation.region",
            &region_name(self.emulation.region),
        );
        kv(
            &mut s,
            "emulation.lenient_address_errors",
            &self.emulation.lenient_address_errors,
        );
        kv(&mut s, "emulation.rewind", &self.emulation.rewind);
        kv(
            &mut s,
            "emulation.fast_forward",
            &self.emulation.fast_forward,
        );
        for (p, player) in self.players.iter().enumerate() {
            let n = p + 1;
            let _ = writeln!(
                s,
                "\n# Player {n}. device: 3button or 6button; keys and pad buttons or `none`"
            );
            kv(
                &mut s,
                &format!("input.p{n}.device"),
                &device_name(player.device),
            );
            for b in ConsoleButton::ALL {
                let key = player.bindings.keys[b.index()].map_or("none", Key::name);
                kv(&mut s, &format!("input.p{n}.key.{}", b.name()), &key);
            }
            for b in ConsoleButton::ALL {
                let pad = player.bindings.pad[b.index()].map_or("none", PadButton::name);
                kv(&mut s, &format!("input.p{n}.pad.{}", b.name()), &pad);
            }
        }
        s.push_str("\n# On-screen controls: auto (on touch screens), on or off\n");
        kv(&mut s, "touch.controls", &touch_name(self.touch));
        s.push_str("\n# Remembered\n");
        if let Some(dir) = &self.browse_dir {
            kv(&mut s, "browse_dir", dir);
        }
        for rom in &self.recent {
            kv(&mut s, "recent", rom);
        }
        s
    }

    // --- Reading -----------------------------------------------------------

    /// Read settings from text. Lines that cannot be used are skipped and
    /// reported; everything else is applied on top of the defaults.
    #[must_use]
    pub fn parse(text: &str) -> (Settings, Vec<ParseError>) {
        let mut settings = Settings::default();
        let mut errors = Vec::new();
        for (i, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let result = match line.split_once('=') {
                Some((key, value)) => settings.set(key.trim(), value.trim()),
                None => Err(format!("expected `key = value`, got '{line}'")),
            };
            if let Err(message) = result {
                errors.push(ParseError {
                    line: i + 1,
                    message,
                });
            }
        }
        (settings, errors)
    }

    /// Apply one `key = value` pair.
    fn set(&mut self, key: &str, v: &str) -> Result<(), String> {
        match key {
            "video.scale" => self.video.scale = parse_int(v, 1, 16)?,
            "video.integer_scale" => self.video.integer_scale = parse_bool(v)?,
            "video.aspect" => self.video.aspect = parse_aspect(v)?,
            "video.fullscreen" => self.video.fullscreen = parse_bool(v)?,
            "audio.volume" => self.audio.volume = parse_int(v, 0, 100)? as u8,
            "audio.mute" => self.audio.mute = parse_bool(v)?,
            "audio.low_pass" => self.audio.low_pass = parse_bool(v)?,
            "emulation.region" => self.emulation.region = parse_region(v)?,
            "emulation.lenient_address_errors" => {
                self.emulation.lenient_address_errors = parse_bool(v)?;
            }
            "emulation.rewind" => self.emulation.rewind = parse_bool(v)?,
            "emulation.fast_forward" => self.emulation.fast_forward = parse_int(v, 2, 8)?,
            "touch.controls" => self.touch = parse_touch(v)?,
            "browse_dir" => self.browse_dir = (!v.is_empty()).then(|| v.to_string()),
            "recent" => {
                if !v.is_empty()
                    && self.recent.len() < MAX_RECENT
                    && !self.recent.iter().any(|r| r == v)
                {
                    self.recent.push(v.to_string());
                }
            }
            _ => return self.set_input(key, v),
        }
        Ok(())
    }

    /// `input.p<N>.device`, `input.p<N>.key.<button>`, `input.p<N>.pad.<button>`.
    fn set_input(&mut self, key: &str, v: &str) -> Result<(), String> {
        let unknown = || format!("unknown setting '{key}'");
        let rest = key.strip_prefix("input.p").ok_or_else(unknown)?;
        let (n, rest) = rest.split_once('.').ok_or_else(unknown)?;
        let player = match n {
            "1" => &mut self.players[0],
            "2" => &mut self.players[1],
            _ => return Err(unknown()),
        };
        if rest == "device" {
            player.device = parse_device(v)?;
            return Ok(());
        }
        let (kind, button) = rest.split_once('.').ok_or_else(unknown)?;
        let button = ConsoleButton::from_name(button).ok_or_else(unknown)?;
        // Assign directly (no swapping): the file is the source of truth.
        match kind {
            "key" => player.bindings.keys[button.index()] = parse_binding(v, Key::from_name)?,
            "pad" => {
                player.bindings.pad[button.index()] = parse_binding(v, PadButton::from_name)?;
            }
            _ => return Err(unknown()),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn customised() -> Settings {
        let mut s = Settings::default();
        s.video.scale = 5;
        s.video.integer_scale = true;
        s.video.aspect = Aspect::Tv;
        s.video.fullscreen = true;
        s.audio.volume = 35;
        s.audio.mute = true;
        s.audio.low_pass = false;
        s.emulation.region = Some(Region::Europe);
        s.emulation.lenient_address_errors = true;
        s.emulation.rewind = false;
        s.emulation.fast_forward = 8;
        s.players[1].device = Device::ThreeButton;
        s.players[1]
            .bindings
            .bind_key(ConsoleButton::A, Some(Key::Kp1));
        s.players[0].bindings.bind_pad(ConsoleButton::Mode, None);
        s.touch = TouchMode::Off;
        s.browse_dir = Some("/roms/with = equals".into());
        s.add_recent("C:\\games\\b.md");
        s.add_recent("/roms/a #1.bin");
        s
    }

    #[test]
    fn round_trip() {
        for s in [Settings::default(), customised()] {
            let text = s.to_text();
            let (back, errors) = Settings::parse(&text);
            assert!(errors.is_empty(), "{errors:?}\n{text}");
            assert_eq!(back, s);
            assert_eq!(back.to_text(), text, "writing is deterministic");
        }
    }

    #[test]
    fn empty_text_gives_defaults() {
        assert_eq!(Settings::parse(""), (Settings::default(), vec![]));
        let (s, errors) = Settings::parse("# only a comment\n\n   \n");
        assert_eq!(s, Settings::default());
        assert!(errors.is_empty());
    }

    #[test]
    fn bad_lines_are_reported_and_skipped() {
        let text = "audio.volume = 50\n\
                    video.scale = huge\n\
                    this line has no equals sign\n\
                    audio.volume = 101\n\
                    future.setting = 1\n\
                    input.p3.key.a = Z\n\
                    input.p1.key.a = NoSuchKey\n\
                    input.p1.key.b = none\n\
                    emulation.region = mars\n\
                    audio.mute=yes";
        let (s, errors) = Settings::parse(text);
        // The good lines were applied …
        assert_eq!(s.audio.volume, 50);
        assert!(s.audio.mute);
        assert_eq!(s.players[0].bindings.keys[ConsoleButton::B.index()], None);
        // … the bad ones left their defaults.
        assert_eq!(s.video.scale, 3);
        assert_eq!(s.emulation.region, None);
        assert_eq!(s.players[0].bindings.keys[0], Some(Key::Up));
        let lines: Vec<usize> = errors.iter().map(|e| e.line).collect();
        assert_eq!(lines, [2, 3, 4, 5, 6, 7, 9]);
        assert!(
            errors[0]
                .to_string()
                .starts_with("line 2: expected a number")
        );
        assert!(
            errors[3]
                .message
                .contains("unknown setting 'future.setting'")
        );
    }

    #[test]
    fn recent_list_order_and_limits() {
        let mut s = Settings::default();
        for i in 0..12 {
            s.add_recent(&format!("rom{i}"));
        }
        s.add_recent("rom5");
        assert_eq!(s.recent.len(), MAX_RECENT);
        assert_eq!(s.recent[0], "rom5");
        assert_eq!(s.recent[1], "rom11");
        assert_eq!(s.recent.iter().filter(|r| *r == "rom5").count(), 1);
        let (back, _) = Settings::parse(&s.to_text());
        assert_eq!(back.recent, s.recent);
    }
}
