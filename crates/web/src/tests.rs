//! The shell driven natively, with a fake page.

use std::collections::HashMap;

use super::*;
use crate::video::{HAS_GAME, HAS_OVERLAY, OVERLAY_CHANGED, PACE_AUDIO, PACE_TIMER};

/// A page without a browser: storage in a map, a fake audio queue.
#[derive(Debug, Default)]
struct FakePage {
    now: f64,
    files: HashMap<String, Vec<u8>>,
    downloads: Vec<String>,
    log: Vec<String>,
    /// `None`: sound not allowed yet.
    audio: Option<usize>,
    pushed: usize,
    requests: Vec<Request>,
}

impl Host for FakePage {
    fn now_ms(&self) -> f64 {
        self.now
    }
    fn log(&mut self, message: &str) {
        self.log.push(message.to_string());
    }
    fn read(&mut self, key: &str) -> Option<Vec<u8>> {
        self.files.get(key).cloned()
    }
    fn write(&mut self, key: &str, data: &[u8]) -> Written {
        if key.starts_with(storage::DOWNLOAD_PREFIX) {
            self.downloads.push(key.to_string());
            return Written::Downloaded;
        }
        self.files.insert(key.to_string(), data.to_vec());
        Written::Stored
    }
    fn audio_queued(&self) -> Option<usize> {
        self.audio
    }
    fn audio_push(&mut self, samples: &[i16]) {
        assert_eq!(samples.len() % 2, 0, "stereo frames");
        self.pushed += samples.len() / 2;
    }
    fn request(&mut self, request: &Request) {
        self.requests.push(request.clone());
    }
}

/// The smallest ROM the core accepts: a header and an endless loop.
fn rom() -> Vec<u8> {
    let mut rom = vec![0u8; 0x1000];
    rom[0..4].copy_from_slice(&0x00FF_FE00u32.to_be_bytes());
    rom[4..8].copy_from_slice(&0x0000_0200u32.to_be_bytes());
    rom[0x100..0x110].copy_from_slice(b"SEGA MEGA DRIVE ");
    rom[0x150..0x159].copy_from_slice(b"WEB  TEST");
    rom[0x1F0] = b'U';
    // bra.s * (loop forever)
    rom[0x200..0x202].copy_from_slice(&0x60FEu16.to_be_bytes());
    rom
}

/// What the page does: write bytes at the inbox address. Natively the
/// "address" is meaningless, so tests fill the inbox through a helper.
fn send(shell: &mut Shell<FakePage>, bytes: &[u8]) {
    shell.inbox(bytes.len());
    shell.inbox.copy_from_slice(bytes);
}

fn open_rom(shell: &mut Shell<FakePage>, name: &str, data: &[u8]) {
    // The page stores the ROM first, then hands it over.
    shell
        .platform
        .host
        .files
        .insert(storage::rom_key(name), data.to_vec());
    let mut bytes = name.as_bytes().to_vec();
    bytes.extend_from_slice(data);
    send(shell, &bytes);
    shell.rom(name.len());
}

fn shell() -> Shell<FakePage> {
    let mut shell = Shell::new(FakePage::default(), 48_000, CAP_KEYBOARD);
    shell.resize(1280, 720, 1.0);
    shell
}

#[test]
fn starts_on_the_home_screen_with_an_overlay() {
    let mut s = shell();
    assert_eq!(s.update(), PACE_TIMER);
    s.video();
    let i = *s.info();
    assert_eq!(s.app.screen_name(), "home");
    assert_eq!(i[info::FLAGS] & HAS_GAME, 0);
    assert_ne!(i[info::FLAGS] & HAS_OVERLAY, 0);
    assert_ne!(i[info::FLAGS] & OVERLAY_CHANGED, 0);
    let (w, h, scale) = (
        i[info::OVERLAY_W],
        i[info::OVERLAY_H],
        i[info::OVERLAY_SCALE],
    );
    assert!(w * scale >= 1280 && h * scale >= 720, "{w}x{h} x{scale}");
    assert_eq!(i[info::PACE_VALUE], 60_000);
}

#[test]
fn settings_come_from_storage_and_fullscreen_is_never_restored() {
    let mut page = FakePage::default();
    page.files.insert(
        "settings.cfg".into(),
        b"audio.volume = 40\nvideo.fullscreen = true\n".to_vec(),
    );
    let s = Shell::new(page, 44_100, 0);
    assert_eq!(s.app.settings().audio.volume, 40);
    assert!(!s.app.settings().video.fullscreen);
    assert_eq!(s.app.capabilities().sample_rate, 44_100);
    assert!(s.app.capabilities().rom_picker);
    assert!(!s.app.capabilities().can_quit);
    assert!(!s.app.capabilities().file_browser);
}

#[test]
fn keys_by_code_through_the_inbox() {
    let mut s = shell();
    send(&mut s, b"Escape");
    assert!(s.key(6, true, false));
    send(&mut s, b"MetaLeft");
    assert!(!s.key(8, true, false));
    // A length past the inbox is clipped, not a panic.
    send(&mut s, b"KeyZ");
    assert!(s.key(400, true, false));
}

#[test]
fn a_rom_from_the_page_runs_and_paces_by_audio() {
    let mut s = shell();
    open_rom(&mut s, "web test.md", &rom());
    assert!(s.app.game().is_some(), "{:?}", s.platform.host.log);
    assert_eq!(s.app.settings().recent[0], "web test.md");
    // Without sound the clock paces, at the console's rate.
    assert_eq!(s.update(), PACE_TIMER);
    assert!(s.info()[info::PACE_VALUE] > 59_000);
    // With sound: the audio queue.
    s.platform.host.audio = Some(0);
    assert_eq!(s.update(), PACE_AUDIO);
    assert_eq!(s.info()[info::PACE_VALUE], 2_400);
    assert!(s.platform.host.pushed > 700, "about 800 frames per frame");
    s.video();
    let i = *s.info();
    assert_ne!(i[info::FLAGS] & HAS_GAME, 0);
    // The VDP starts in its 256-pixel mode until a game picks one.
    assert_eq!((i[info::GAME_W], i[info::GAME_H]), (256, 224));
    assert_eq!(s.game_rgba().len(), 256 * 224);
    assert!(s.game_rgba().iter().all(|p| p >> 24 == 0xFF), "opaque");
    // The game fills the height of a 16:9 window, centred.
    assert_eq!(i[info::RECT_H], 720);
    assert!(i[info::RECT_X] > 0);
    assert_eq!(i[info::FRAME], 2);
    // Settings were stored, with the ROM in the recent list.
    let settings = &s.platform.host.files["settings.cfg"];
    assert!(String::from_utf8_lossy(settings).contains("recent = web test.md"));
}

#[test]
fn the_recent_list_reopens_stored_roms() {
    let mut s = shell();
    open_rom(&mut s, "a.md", &rom());
    s.app.close_game(&mut s.platform);
    assert!(s.app.game().is_none());
    assert!(s.app.open_rom(&mut s.platform, "a.md").is_ok());
    // A ROM the page no longer has: a message, no game.
    s.app.close_game(&mut s.platform);
    assert!(s.app.open_rom(&mut s.platform, "gone.md").is_err());
    assert!(
        s.platform
            .host
            .log
            .iter()
            .any(|l| l.contains("open it again"))
    );
}

#[test]
fn save_states_and_screenshots() {
    let mut s = shell();
    open_rom(&mut s, "game.bin", &rom());
    s.update();
    let game = s.app.game_mut().unwrap();
    game.save_state(2, &mut s.platform).unwrap();
    assert!(s.platform.host.files.contains_key("state/game.bin/2"));
    assert!(s.platform.host.files.contains_key("preview/game.bin/2"));
    // F12 takes a screenshot: downloaded, not stored.
    send(&mut s, b"F12");
    assert!(s.key(3, true, false));
    assert_eq!(s.platform.host.downloads.len(), 1);
    assert!(s.platform.host.downloads[0].starts_with("screenshot/game-"));
}

#[test]
fn hiding_the_page_pauses_into_the_menu() {
    let mut s = shell();
    open_rom(&mut s, "game.bin", &rom());
    s.update();
    assert!(!s.app.menu_open());
    s.suspend();
    assert!(s.app.menu_open());
    s.update();
    s.video();
    assert_ne!(s.info()[info::FLAGS] & HAS_OVERLAY, 0);
}

#[test]
fn pads_and_pointers_reach_the_app() {
    let mut s = shell();
    send(&mut s, b"Xbox 360 Controller (XInput STANDARD GAMEPAD)");
    s.pad_connected(0, 46);
    // A pad's Start confirms the focused home-screen item; nothing to
    // assert there without a ROM, but unknown indices must be harmless.
    s.pad_button(0, 6, true);
    s.pad_button(0, 99, true);
    s.pad_axis(0, 9, 1.0);
    s.pointer(1, 1, 0, 10.0, 10.0);
    s.pointer(1, 1, 9, 10.0, 10.0);
    s.wheel(120.0, 0);
    s.update();
    s.pad_disconnected(0);
    s.focus_lost();
}

#[test]
fn requests_for_the_page() {
    assert_eq!(
        encode_request(&Request::PickRom),
        Some((REQUEST_PICK_ROM, 0, ""))
    );
    assert_eq!(
        encode_request(&Request::SetFullscreen(true)),
        Some((REQUEST_FULLSCREEN, 1, ""))
    );
    assert_eq!(
        encode_request(&Request::SetTitle("gase - X".into())),
        Some((REQUEST_TITLE, 0, "gase - X"))
    );
    assert_eq!(encode_request(&Request::Quit), None);
    assert_eq!(encode_request(&Request::ToggleDebugger), None);
}

#[test]
fn bench_runs_frames() {
    let mut s = shell();
    assert_eq!(s.bench(5), 0, "no game");
    open_rom(&mut s, "game.bin", &rom());
    assert_eq!(s.bench(5), 5);
}
