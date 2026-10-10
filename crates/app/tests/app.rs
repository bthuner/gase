//! The whole app driven through its public API by a fake platform, the way
//! a shell drives it: events in, `update`, then look at the result.

use std::collections::BTreeMap;

use gase_app::canvas::opaque;
use gase_app::theme;
use gase_app::{
    App, Capabilities, DirEntry, Event, FileKey, Key, Listing, Pacing, PadButton, Platform,
    PointerKind, PointerPhase, Request,
};
use gase_core::Buttons;

/// A platform that keeps everything in memory.
#[derive(Default)]
struct Fake {
    now: u64,
    files: BTreeMap<String, Vec<u8>>,
    /// Folders: path → entries (name, is_dir).
    dirs: BTreeMap<String, Vec<(String, bool)>>,
    requests: Vec<Request>,
    audio: Option<Vec<i16>>,
}

fn key_name(file: FileKey<'_>) -> String {
    match file {
        FileKey::Settings => "settings".into(),
        FileKey::Save { rom } => format!("{rom}.srm"),
        FileKey::State { rom, slot } => format!("{rom}.state{slot}"),
        FileKey::StatePreview { rom, slot } => format!("{rom}.state{slot}.png"),
        FileKey::Screenshot { rom, frame } => format!("{rom}-{frame}.png"),
    }
}

impl Platform for Fake {
    fn now_ms(&self) -> u64 {
        self.now
    }
    fn load(&mut self, file: FileKey<'_>) -> Option<Vec<u8>> {
        self.files.get(&key_name(file)).cloned()
    }
    fn store(&mut self, file: FileKey<'_>, data: &[u8]) -> Result<String, String> {
        let name = key_name(file);
        self.files.insert(name.clone(), data.to_vec());
        Ok(name)
    }
    fn read_rom(&mut self, path: &str) -> Result<Vec<u8>, String> {
        self.files
            .get(path)
            .cloned()
            .ok_or_else(|| format!("no {path}"))
    }
    fn list_dir(&mut self, dir: Option<&str>) -> Result<Listing, String> {
        let dir = dir.unwrap_or("/roms");
        let entries = self.dirs.get(dir).ok_or("no such folder")?;
        Ok(Listing {
            path: dir.to_string(),
            parent: dir
                .rsplit_once('/')
                .map(|(p, _)| if p.is_empty() { "/" } else { p }.to_string()),
            entries: entries
                .iter()
                .map(|(name, is_dir)| DirEntry {
                    name: name.clone(),
                    path: format!("{}/{name}", dir.trim_end_matches('/')),
                    is_dir: *is_dir,
                })
                .collect(),
        })
    }
    fn audio_queued(&self) -> Option<usize> {
        self.audio.as_ref().map(|_| 0)
    }
    fn queue_audio(&mut self, samples: &[i16]) {
        if let Some(a) = &mut self.audio {
            a.extend_from_slice(samples);
        }
    }
    fn request(&mut self, request: Request) {
        self.requests.push(request);
    }
}

/// A tiny program: count in D0 forever and store it in RAM, so every frame
/// changes the console's state.
fn rom() -> Vec<u8> {
    let mut rom = vec![0u8; 0x1000];
    rom[0..4].copy_from_slice(&0x00FF_FE00u32.to_be_bytes());
    rom[4..8].copy_from_slice(&0x0000_0200u32.to_be_bytes());
    rom[0x100..0x110].copy_from_slice(b"SEGA MEGA DRIVE ");
    rom[0x150..0x159].copy_from_slice(b"TEST GAME");
    rom[0x1F0] = b'U';
    // addq.w #1,d0 / move.w d0,($FF0000).l / bra.s *-8
    let code: [u16; 5] = [0x5240, 0x33C0, 0x00FF, 0x0000, 0x60F6];
    for (i, w) in code.iter().enumerate() {
        rom[0x200 + 2 * i..0x202 + 2 * i].copy_from_slice(&w.to_be_bytes());
    }
    rom
}

fn caps() -> Capabilities {
    Capabilities {
        file_browser: true,
        drop_files: true,
        can_quit: true,
        ..Capabilities::default()
    }
}

/// An app on a 960 × 672 screen (UI scale 2: a 480 × 336 overlay).
fn setup(caps: Capabilities) -> (Fake, App) {
    let mut fake = Fake::default();
    fake.files.insert("/roms/game.bin".into(), rom());
    fake.dirs.insert(
        "/roms".into(),
        vec![
            ("notes.txt".into(), false),
            ("game.bin".into(), false),
            ("Zelda".into(), true),
            ("alpha.md".into(), false),
            (".hidden".into(), true),
        ],
    );
    fake.dirs
        .insert("/roms/Zelda".into(), vec![("inner.gen".into(), false)]);
    let mut app = App::new(&mut fake, caps);
    app.handle(
        &mut fake,
        Event::Resized {
            width: 960,
            height: 672,
            pixels_per_point: 1.0,
        },
    );
    app.update(&mut fake);
    (fake, app)
}

fn press(app: &mut App, fake: &mut Fake, key: Key) {
    for pressed in [true, false] {
        app.handle(
            fake,
            Event::Key {
                key,
                pressed,
                repeat: false,
            },
        );
    }
    app.update(fake);
}

fn pad(app: &mut App, fake: &mut Fake, button: PadButton) {
    pad_n(app, fake, 7, button);
}

fn pad_n(app: &mut App, fake: &mut Fake, pad: u32, button: PadButton) {
    for pressed in [true, false] {
        app.handle(
            fake,
            Event::PadButton {
                pad,
                button,
                pressed,
            },
        );
    }
    app.update(fake);
}

/// Tap the centre of item `index` of the current screen.
fn tap(app: &mut App, fake: &mut Fake, index: usize, kind: PointerKind) {
    let r = app.item_rect(index).expect("item drawn");
    let s = app.ui_scale() as f32;
    let (x, y) = ((r.x + r.w / 2) as f32 * s, (r.y + r.h / 2) as f32 * s);
    for phase in [PointerPhase::Down, PointerPhase::Up] {
        app.handle(
            fake,
            Event::Pointer {
                id: 1,
                kind,
                phase,
                x,
                y,
            },
        );
    }
    app.update(fake);
}

fn open_game(app: &mut App, fake: &mut Fake) {
    app.handle(
        fake,
        Event::OpenRom {
            path: "/roms/game.bin".into(),
        },
    );
    assert_eq!(app.screen_name(), "game");
}

#[test]
fn keyboard_paths_through_the_menus() {
    let (mut fake, mut app) = setup(caps());
    assert_eq!(app.screen_name(), "home");
    // Home: Open ROM…, Settings, Quit (no recent games yet).
    press(&mut app, &mut fake, Key::Down);
    press(&mut app, &mut fake, Key::Enter);
    assert_eq!(app.screen_name(), "settings");
    // Settings → Audio, lower the volume, back twice: the settings are saved.
    press(&mut app, &mut fake, Key::Down);
    press(&mut app, &mut fake, Key::Enter);
    assert_eq!(app.screen_name(), "audio");
    press(&mut app, &mut fake, Key::Left);
    assert_eq!(app.settings().audio.volume, 90);
    assert!(
        !fake.files.contains_key("settings"),
        "saved on leaving, not on every change"
    );
    press(&mut app, &mut fake, Key::Escape);
    press(&mut app, &mut fake, Key::Escape);
    assert_eq!(app.screen_name(), "home");
    let saved = String::from_utf8(fake.files["settings"].clone()).unwrap();
    assert!(saved.contains("audio.volume = 90"), "{saved}");
    // Escape on the home screen does nothing; Quit asks the platform.
    press(&mut app, &mut fake, Key::Escape);
    assert_eq!(app.screen_name(), "home");
    // The home screen kept its focus (on Settings); End goes to Quit.
    assert_eq!(app.focus_index(), Some(1));
    press(&mut app, &mut fake, Key::End);
    press(&mut app, &mut fake, Key::Enter);
    assert!(app.wants_quit());
    assert!(fake.requests.contains(&Request::Quit));
}

#[test]
fn gamepad_and_pointer_paths() {
    let (mut fake, mut app) = setup(caps());
    app.handle(
        &mut fake,
        Event::PadConnected {
            pad: 7,
            name: "Pad".into(),
        },
    );
    pad(&mut app, &mut fake, PadButton::DPadDown);
    pad(&mut app, &mut fake, PadButton::South);
    assert_eq!(app.screen_name(), "settings");
    pad(&mut app, &mut fake, PadButton::East);
    assert_eq!(app.screen_name(), "home");
    // A mouse click on "Settings" (item 1), then on "Video" (item 0).
    tap(&mut app, &mut fake, 1, PointerKind::Mouse);
    assert_eq!(app.screen_name(), "settings");
    tap(&mut app, &mut fake, 0, PointerKind::Touch);
    assert_eq!(app.screen_name(), "video");
    // Tapping a choice changes it.
    let before = app.settings().video.aspect;
    tap(&mut app, &mut fake, 0, PointerKind::Touch);
    assert_ne!(app.settings().video.aspect, before);
}

#[test]
fn file_browser_lists_sorts_filters_and_opens() {
    let (mut fake, mut app) = setup(caps());
    press(&mut app, &mut fake, Key::Enter); // Open ROM…
    assert_eq!(app.screen_name(), "browser");
    // Items: Show, "..", Zelda/, alpha.md, game.bin (no notes.txt, no .hidden).
    assert_eq!(app.item_rect(5), None, "five items");
    assert!(app.item_rect(4).is_some());
    // Type "g" to jump to game.bin, open it.
    press(&mut app, &mut fake, Key::G);
    assert_eq!(app.focus_index(), Some(4));
    press(&mut app, &mut fake, Key::Enter);
    assert_eq!(app.screen_name(), "game");
    assert_eq!(app.game().unwrap().title, "TEST GAME");
    assert_eq!(app.settings().recent, ["/roms/game.bin"]);

    // Back on the home screen the game is in the recent list.
    press(&mut app, &mut fake, Key::Escape);
    let close = 5; // Resume, Save, Load, Reset, Settings, Close game
    tap(&mut app, &mut fake, close, PointerKind::Mouse);
    assert_eq!(app.screen_name(), "home");
    assert!(app.game().is_none());

    // Into a folder and back up; "show all" reveals the other files.
    press(&mut app, &mut fake, Key::Enter);
    press(&mut app, &mut fake, Key::Down); // ..
    press(&mut app, &mut fake, Key::Down); // Zelda
    press(&mut app, &mut fake, Key::Enter);
    assert_eq!(app.settings().browse_dir.as_deref(), Some("/roms/Zelda"));
    assert!(
        app.item_rect(2).is_some() && app.item_rect(3).is_none(),
        "Show, .., inner.gen"
    );
    press(&mut app, &mut fake, Key::Down);
    press(&mut app, &mut fake, Key::Enter); // ..
    press(&mut app, &mut fake, Key::Home);
    press(&mut app, &mut fake, Key::Right); // Show: all files
    assert!(app.item_rect(6).is_some(), "notes.txt and .hidden appear");
}

#[test]
fn save_state_from_the_menu_then_load_it_is_deterministic() {
    let (mut fake, mut app) = setup(caps());
    open_game(&mut app, &mut fake);
    for _ in 0..30 {
        app.update(&mut fake);
    }
    // Esc → pause menu → Save state → slot 0.
    press(&mut app, &mut fake, Key::Escape);
    assert_eq!(app.screen_name(), "pause");
    press(&mut app, &mut fake, Key::Down);
    press(&mut app, &mut fake, Key::Enter);
    assert_eq!(app.screen_name(), "save");
    let frame_saved = app.game().unwrap().genesis.frame_count();
    press(&mut app, &mut fake, Key::Enter);
    assert!(fake.files.contains_key("/roms/game.bin.state0"));
    assert!(
        fake.files.contains_key("/roms/game.bin.state0.png"),
        "with a preview"
    );
    assert_eq!(
        app.game().unwrap().genesis.frame_count(),
        frame_saved,
        "menus stop the game"
    );

    // Play on and remember where 40 frames from the save lead.
    press(&mut app, &mut fake, Key::Escape);
    press(&mut app, &mut fake, Key::Escape);
    assert_eq!(app.screen_name(), "game");
    let run = |app: &mut App, fake: &mut Fake| {
        while app.game().unwrap().genesis.frame_count() < frame_saved + 40 {
            app.update(fake);
        }
        app.game().unwrap().genesis.save_state()
    };
    let first = run(&mut app, &mut fake);
    assert_ne!(
        app.game().unwrap().genesis.hw.ram[..2],
        [0, 0],
        "the program ran"
    );

    // Load slot 0 from the menu: the game resumes from the save …
    press(&mut app, &mut fake, Key::Escape);
    press(&mut app, &mut fake, Key::Down);
    press(&mut app, &mut fake, Key::Down);
    press(&mut app, &mut fake, Key::Enter);
    assert_eq!(app.screen_name(), "load");
    press(&mut app, &mut fake, Key::Enter);
    assert_eq!(app.screen_name(), "game");
    assert_eq!(app.game().unwrap().genesis.frame_count(), frame_saved);
    // … and the same 40 frames give exactly the same console.
    assert_eq!(run(&mut app, &mut fake), first);

    // Loading an empty slot is an error, not a crash.
    press(&mut app, &mut fake, Key::F7); // slot 1
    press(&mut app, &mut fake, Key::F8);
    assert_eq!(
        app.game().unwrap().genesis.frame_count(),
        frame_saved + 40 + 2
    );
}

#[test]
fn keyboard_and_pads_drive_both_players() {
    let (mut fake, mut app) = setup(caps());
    open_game(&mut app, &mut fake);
    let held = |app: &App| {
        let g = &app.game().unwrap().genesis;
        (g.hw.io.ports[0].buttons, g.hw.io.ports[1].buttons)
    };
    app.handle(
        &mut fake,
        Event::Key {
            key: Key::Z,
            pressed: true,
            repeat: false,
        },
    );
    app.handle(
        &mut fake,
        Event::PadConnected {
            pad: 1,
            name: "One".into(),
        },
    );
    app.handle(
        &mut fake,
        Event::PadConnected {
            pad: 2,
            name: "Two".into(),
        },
    );
    app.handle(
        &mut fake,
        Event::PadButton {
            pad: 2,
            button: PadButton::East,
            pressed: true,
        },
    );
    app.handle(
        &mut fake,
        Event::PadAxis {
            pad: 1,
            axis: gase_app::PadAxis::LeftX,
            value: -0.9,
        },
    );
    app.update(&mut fake);
    assert_eq!(held(&app), (Buttons::A | Buttons::LEFT, Buttons::C));
    // A key repeat must not press anything new; key up releases.
    app.handle(
        &mut fake,
        Event::Key {
            key: Key::Z,
            pressed: false,
            repeat: false,
        },
    );
    app.handle(
        &mut fake,
        Event::Key {
            key: Key::Enter,
            pressed: true,
            repeat: true,
        },
    );
    app.update(&mut fake);
    assert_eq!(held(&app).0, Buttons::LEFT);
    // Guide opens the menu and releases everything.
    pad_n(&mut app, &mut fake, 1, PadButton::Guide);
    assert_eq!(app.screen_name(), "pause");
    pad_n(&mut app, &mut fake, 2, PadButton::East);
    assert_eq!(app.screen_name(), "game");
}

#[test]
fn remapping_a_key() {
    let (mut fake, mut app) = setup(caps());
    open_game(&mut app, &mut fake);
    assert!(app.show_screen(&mut fake, "remap"));
    app.update(&mut fake);
    press(&mut app, &mut fake, Key::Enter); // "Up"
    assert_eq!(app.screen_name(), "remap-waiting");
    press(&mut app, &mut fake, Key::W);
    assert_eq!(app.screen_name(), "remap");
    assert_eq!(app.settings().players[0].bindings.keys[0], Some(Key::W));
    // Escape cancels waiting without changing anything.
    press(&mut app, &mut fake, Key::Enter);
    press(&mut app, &mut fake, Key::Escape);
    assert_eq!(app.screen_name(), "remap");
    assert_eq!(app.settings().players[0].bindings.keys[0], Some(Key::W));
    // Waiting gives up after a while.
    press(&mut app, &mut fake, Key::Enter);
    fake.now += 10_000;
    app.update(&mut fake);
    assert_eq!(app.screen_name(), "remap");
    // In the game, W is now Up.
    press(&mut app, &mut fake, Key::Escape); // back to the game
    app.handle(
        &mut fake,
        Event::Key {
            key: Key::W,
            pressed: true,
            repeat: false,
        },
    );
    app.update(&mut fake);
    assert_eq!(
        app.game().unwrap().genesis.hw.io.ports[0].buttons,
        Buttons::UP
    );
}

#[test]
fn touch_controls_in_portrait() {
    let caps = Capabilities {
        touch_screen: true,
        keyboard: false,
        ..caps()
    };
    let (mut fake, mut app) = setup(caps);
    app.handle(
        &mut fake,
        Event::Resized {
            width: 1080,
            height: 2340,
            pixels_per_point: 3.0,
        },
    );
    open_game(&mut app, &mut fake);
    app.update(&mut fake);
    // The picture sits at the top, the controls below it.
    let video = app.video();
    assert_eq!(video.game_rect.y, 0);
    assert!(video.overlay.is_some(), "controls are drawn");
    // Thumb on the lower left of the screen: the d-pad, pressing down-left.
    let s = app.ui_scale() as f32;
    let (w, h) = (1080.0 / s, 2340.0 / s);
    let at = |x: f32, y: f32| (x * s, y * s);
    let find = |app: &mut App, fake: &mut Fake, buttons: Buttons| {
        // Scan the lower half for a spot pressing exactly `buttons`.
        for yi in 0..80 {
            for xi in 0..40 {
                let (x, y) = at(w * xi as f32 / 40.0, h / 2.0 + h / 2.0 * yi as f32 / 80.0);
                app.handle(
                    fake,
                    Event::Pointer {
                        id: 9,
                        kind: PointerKind::Touch,
                        phase: PointerPhase::Down,
                        x,
                        y,
                    },
                );
                app.update(fake);
                let got = app.game().unwrap().genesis.hw.io.ports[0].buttons;
                app.handle(
                    fake,
                    Event::Pointer {
                        id: 9,
                        kind: PointerKind::Touch,
                        phase: PointerPhase::Up,
                        x,
                        y,
                    },
                );
                app.update(fake);
                if got == buttons {
                    return Some((x, y));
                }
            }
        }
        None
    };
    let diagonal =
        find(&mut app, &mut fake, Buttons::DOWN | Buttons::LEFT).expect("a diagonal on the d-pad");
    let b = find(&mut app, &mut fake, Buttons::B).expect("button B");
    // Both at once: multi-touch.
    for (id, (x, y)) in [(1, diagonal), (2, b)] {
        app.handle(
            &mut fake,
            Event::Pointer {
                id,
                kind: PointerKind::Touch,
                phase: PointerPhase::Down,
                x,
                y,
            },
        );
    }
    app.update(&mut fake);
    assert_eq!(
        app.game().unwrap().genesis.hw.io.ports[0].buttons,
        Buttons::DOWN | Buttons::LEFT | Buttons::B
    );
}

#[test]
fn audio_is_paced_by_the_sound_device() {
    let (mut fake, mut app) = setup(caps());
    fake.audio = Some(Vec::new());
    open_game(&mut app, &mut fake);
    let pacing = app.update(&mut fake);
    assert!(
        matches!(pacing, Pacing::Audio { target: 2400 }),
        "{pacing:?}"
    );
    let queued = fake.audio.as_ref().unwrap().len();
    assert!(
        (1500..2000).contains(&queued),
        "about 800 stereo frames per frame: {queued}"
    );
    // In a menu nothing is queued and the timer paces.
    press(&mut app, &mut fake, Key::Escape);
    assert!(matches!(app.update(&mut fake), Pacing::Timer { .. }));
    assert_eq!(fake.audio.as_ref().unwrap().len(), queued);
    // Fast-forward runs several frames without pacing.
    press(&mut app, &mut fake, Key::Escape);
    let before = app.game().unwrap().genesis.frame_count();
    app.handle(
        &mut fake,
        Event::Key {
            key: Key::Tab,
            pressed: true,
            repeat: false,
        },
    );
    assert_eq!(app.update(&mut fake), Pacing::Unthrottled);
    assert_eq!(app.game().unwrap().genesis.frame_count(), before + 4);
}

/// FNV-1a over the pixels, for comparing pictures.
fn hash(pixels: &[u32]) -> u64 {
    pixels.iter().fold(0xcbf2_9ce4_8422_2325, |h, p| {
        (h ^ u64::from(*p)).wrapping_mul(0x100_0000_01b3)
    })
}

#[test]
fn screenshots_of_the_home_screen_and_pause_menu() {
    let (mut fake, mut app) = setup(caps());
    let home = app.compose();
    assert_eq!((home.width, home.height), (960, 672));
    // The background, the red focus bar on "Open ROM…", and the title.
    assert_eq!(home.get(4, 4), theme::BACKGROUND);
    let r = app.item_rect(0).unwrap();
    let s = app.ui_scale() as usize;
    assert_eq!(
        home.get(r.x as usize * s, (r.y + r.h / 2) as usize * s),
        theme::ACCENT
    );
    assert_eq!(
        home.get((r.x + 4) as usize * s, (r.y + 1) as usize * s),
        theme::FOCUS
    );
    let title_area = (0..60 * s).flat_map(|y| (0..home.width).map(move |x| (x, y)));
    assert!(
        title_area
            .clone()
            .any(|(x, y)| home.get(x, y) == theme::TEXT)
    );
    assert!(
        title_area
            .into_iter()
            .any(|(x, y)| home.get(x, y) == theme::GOLD)
    );
    // Drawing is deterministic: the same state gives the same picture.
    app.update(&mut fake);
    assert_eq!(hash(&app.compose().pixels), hash(&home.pixels));

    // The pause menu over a game: the game shows around the menu sheet.
    open_game(&mut app, &mut fake);
    press(&mut app, &mut fake, Key::Escape);
    let pause = app.compose();
    let video = app.video();
    assert!(video.game.is_some() && video.overlay.is_some());
    let r = app.item_rect(0).unwrap();
    assert_eq!(
        pause.get(r.x as usize * s, (r.y + r.h / 2) as usize * s),
        theme::ACCENT
    );
    assert_ne!(hash(&pause.pixels), hash(&home.pixels));
    // Resume: the overlay disappears entirely (menus cost nothing in game).
    press(&mut app, &mut fake, Key::Escape);
    assert!(app.video().overlay.is_none());
    assert_eq!(app.compose().get(4, 4) | 0xFF00_0000, opaque(0));
}

#[test]
fn bad_roms_and_settings_are_reported_not_fatal() {
    let mut fake = Fake::default();
    fake.files.insert(
        "settings".into(),
        b"audio.volume = loud\nvideo.aspect = tv\n".to_vec(),
    );
    fake.files.insert("/tiny.bin".into(), vec![0; 16]);
    let mut app = App::new(&mut fake, caps());
    assert_eq!(app.settings().video.aspect, gase_app::Aspect::Tv);
    assert_eq!(app.settings().audio.volume, 100);
    assert!(app.open_rom(&mut fake, "/tiny.bin").is_err());
    assert!(app.open_rom(&mut fake, "/missing.bin").is_err());
    assert_eq!(app.screen_name(), "home");
    // A ROM given as bytes (a web or phone file picker) works too.
    app.handle(
        &mut fake,
        Event::RomData {
            name: "picked.bin".into(),
            data: rom(),
        },
    );
    assert_eq!(app.screen_name(), "game");
    assert_eq!(app.rom_name(), Some("picked.bin"));
}

#[test]
fn a_gamepad_hides_the_touch_controls_until_the_screen_is_touched() {
    let caps = Capabilities {
        touch_screen: true,
        keyboard: false,
        ..caps()
    };
    let (mut fake, mut app) = setup(caps);
    app.handle(
        &mut fake,
        Event::Resized {
            width: 1080,
            height: 2340,
            pixels_per_point: 3.0,
        },
    );
    open_game(&mut app, &mut fake);
    app.update(&mut fake);
    // Portrait with controls: the picture is at the top.
    assert_eq!(app.video().game_rect.y, 0);
    assert!(app.video().overlay.is_some());

    // A pad appears: nothing changes until it is used…
    app.handle(
        &mut fake,
        Event::PadConnected {
            pad: 7,
            name: "Pad".into(),
        },
    );
    app.update(&mut fake);
    assert_eq!(app.video().game_rect.y, 0);
    // …then the controls go and the picture is centred.
    app.handle(
        &mut fake,
        Event::PadButton {
            pad: 7,
            button: PadButton::South,
            pressed: true,
        },
    );
    app.update(&mut fake);
    assert!(app.video().game_rect.y > 0, "picture centred");
    // Only the pad's toast is left on the overlay; once it is gone,
    // nothing is drawn over the game at all.
    fake.now += 60_000;
    app.update(&mut fake);
    assert!(app.video().overlay.is_none(), "no on-screen controls");

    // Touching the screen brings them back.
    app.handle(
        &mut fake,
        Event::Pointer {
            id: 1,
            kind: PointerKind::Touch,
            phase: PointerPhase::Down,
            x: 500.0,
            y: 2000.0,
        },
    );
    app.update(&mut fake);
    assert_eq!(app.video().game_rect.y, 0);
    assert!(app.video().overlay.is_some());

    // Used again, then unplugged: the controls return by themselves.
    app.handle(
        &mut fake,
        Event::PadButton {
            pad: 7,
            button: PadButton::South,
            pressed: true,
        },
    );
    app.update(&mut fake);
    assert!(app.video().game_rect.y > 0);
    app.handle(&mut fake, Event::PadDisconnected { pad: 7 });
    app.update(&mut fake);
    assert_eq!(app.video().game_rect.y, 0);
}

#[test]
fn suspend_writes_the_save_and_settings_and_pauses() {
    let (mut fake, mut app) = setup(caps());
    open_game(&mut app, &mut fake);
    app.update(&mut fake);
    // A hotkey's settings change (mute) is normally written when a menu
    // closes; a phone app may be killed in the background without warning,
    // so suspending writes it at once.
    press(&mut app, &mut fake, Key::M);
    fake.files.remove("settings");
    app.handle(&mut fake, Event::Suspend);
    assert!(fake.files.contains_key("settings"), "settings written");
    assert_eq!(app.screen_name(), "pause", "the game waits in its menu");
}
