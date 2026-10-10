//! Every menu screen, each one a plain function that draws the screen with
//! the [`Ui`] toolkit and returns what the user chose.
//!
//! The functions never change the app themselves (except settings, which
//! they edit in place: a choice *is* the setting). They return an
//! [`Action`] and the app carries it out, which keeps them short and lets
//! the app apply the side effects (saving, loading a game …) in one place.

use gase_core::{Device, Region};

use crate::browser::Browser;
use crate::canvas::{Rect, with_alpha};
use crate::font::{self, CARTRIDGE, FOLDER, text_scaled, text_width};
use crate::game::{self, Game, PREVIEW, SLOTS, SlotInfo};
use crate::input::{ConsoleButton, PlayerBindings};
use crate::platform::{Capabilities, Platform};
use crate::settings::{Aspect, Settings, TouchMode};
use crate::theme;
use crate::ui::Ui;

/// A screen on the menu stack, with the state only it needs.
#[derive(Debug)]
pub(crate) enum Screen {
    Home,
    Browser(Browser),
    Pause,
    States { save: bool, slots: Vec<SlotInfo> },
    Settings,
    Video,
    Audio,
    Emulation,
    Controls,
    Remap(Remap),
}

/// The remapping screen: whose bindings, and the button waiting for a key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Remap {
    pub player: usize,
    /// Gamepad bindings (else keyboard).
    pub pad: bool,
    /// Waiting for a key or pad button for this console button …
    pub waiting: Option<ConsoleButton>,
    /// … since this time (it gives up after a while).
    pub since_ms: u64,
}

/// How long the remapping screen waits for a key.
pub(crate) const REMAP_TIMEOUT_MS: u64 = 6_000;

/// What the user chose on a screen.
#[derive(Debug)]
pub(crate) enum Action {
    None,
    Push(Screen),
    /// Close this screen.
    Pop,
    /// Close every menu and continue the game.
    Resume,
    /// Open this ROM (a path or name for the platform).
    Open(String),
    /// Ask the platform's own file picker.
    Pick,
    CloseGame,
    Reset,
    SaveState(u8),
    LoadState(u8),
    Quit,
}

/// The last kind of device used, to show matching hints.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum InputMode {
    #[default]
    Keyboard,
    Pad,
    Pointer,
}

/// What screens may look at (and the settings, which they edit).
pub(crate) struct Ctx<'a> {
    pub settings: &'a mut Settings,
    pub caps: &'a Capabilities,
    pub game: Option<&'a Game>,
    pub platform: &'a mut dyn Platform,
    pub mode: InputMode,
    /// Connected gamepads: name and player.
    pub pads: Vec<(String, usize)>,
    pub now_ms: u64,
}

/// Where hint lines go: the bottom of the screen.
fn footer_y(ui: &Ui<'_>) -> i32 {
    ui.canvas.height as i32 - theme::MARGIN - 8
}

/// The bottom of a screen's list, above the footer (room for two lines
/// of hints).
fn list_bottom(ui: &Ui<'_>) -> i32 {
    footer_y(ui) - 22
}

fn hint(ui: &mut Ui<'_>, ctx: &Ctx<'_>, keyboard: &str, pad: &str, pointer: &str) {
    let y = footer_y(ui);
    let line = match ctx.mode {
        InputMode::Keyboard => keyboard,
        InputMode::Pad => pad,
        InputMode::Pointer => pointer,
    };
    ui.hint(y, line);
}

/// The usual hint for list screens.
fn back_hint(ui: &mut Ui<'_>, ctx: &Ctx<'_>) {
    hint(
        ui,
        ctx,
        "Enter select · Left/Right change · Esc back",
        "A select · Left/Right change · B back",
        "",
    );
}

/// Draw `screen` and handle its input.
pub(crate) fn draw(screen: &mut Screen, ui: &mut Ui<'_>, ctx: &mut Ctx<'_>) -> Action {
    let action = match screen {
        Screen::Home => home(ui, ctx),
        Screen::Browser(browser) => browse(browser, ui, ctx),
        Screen::Pause => pause(ui, ctx),
        Screen::States { save, slots } => states(*save, slots, ui, ctx),
        Screen::Settings => settings_menu(ui, ctx),
        Screen::Video => video(ui, ctx),
        Screen::Audio => audio(ui, ctx),
        Screen::Emulation => emulation(ui, ctx),
        Screen::Controls => controls(ui, ctx),
        Screen::Remap(remap) => remap_screen(remap, ui, ctx),
    };
    if matches!(action, Action::None) && ui.back() {
        return match screen {
            Screen::Home => Action::None,
            Screen::Pause => Action::Resume,
            Screen::Remap(r) if r.waiting.is_some() => Action::None,
            _ => Action::Pop,
        };
    }
    action
}

// --- Home ---------------------------------------------------------------------

/// The Mega Drive's top is a big ring around the cartridge slot; a few
/// faint concentric circles echo it behind the home screen.
fn home_decoration(ui: &mut Ui<'_>) {
    let (w, h) = (ui.canvas.width as i32, ui.canvas.height as i32);
    let r = h * 2 / 3;
    let (cx, cy) = (w - r / 6, h / 2 + r / 4);
    ui.canvas.ring(cx, cy, r, r - 2, with_alpha(0x2A2D35, 0xFF));
    ui.canvas.ring(
        cx,
        cy,
        r * 82 / 100,
        r * 82 / 100 - 1,
        with_alpha(0x1E2027, 0xFF),
    );
    ui.canvas.ring(
        cx,
        cy,
        r * 64 / 100,
        r * 64 / 100 - 1,
        with_alpha(0x1E2027, 0xFF),
    );
}

fn home(ui: &mut Ui<'_>, ctx: &mut Ctx<'_>) -> Action {
    home_decoration(ui);
    let x = ui.column.x;
    let y = ui.y + 4;
    text_scaled(ui.canvas, x, y, "gase", theme::TEXT, 4);
    let tag_x = x + text_width("gase", 4) + 10;
    text_scaled(ui.canvas, tag_x, y + 8, "MEGA DRIVE", theme::GOLD, 1);
    text_scaled(ui.canvas, tag_x, y + 20, "GENESIS", theme::DIM, 1);
    ui.y = y + 32 + 8;
    ui.canvas.fill(Rect::new(x, ui.y, 24, 2), theme::ACCENT);
    ui.canvas
        .fill(Rect::new(x + 24, ui.y, ui.column.w - 24, 1), theme::LINE);
    ui.y += 12;

    let mut action = Action::None;
    ui.begin_list(list_bottom(ui));
    if (ctx.caps.file_browser || ctx.caps.rom_picker)
        && ui.button_with(Some(FOLDER), "Open ROM…", None, theme::TEXT)
    {
        action = if ctx.caps.file_browser {
            open_browser(ctx)
        } else {
            Action::Pick
        };
    }
    if !ctx.settings.recent.is_empty() {
        ui.section("RECENT");
        for rom in &ctx.settings.recent {
            // The folder tells apart ROMs that share a file name.
            let folder = game::file_name(
                rom.strip_suffix(game::file_name(rom))
                    .unwrap_or("")
                    .trim_end_matches(['/', '\\']),
            );
            let folder = font::ellipsize(folder, 16, false);
            let detail = (!folder.is_empty()).then_some(folder.as_str());
            if ui.button_with(Some(CARTRIDGE), game::file_stem(rom), detail, theme::TEXT) {
                action = Action::Open(rom.clone());
            }
        }
        ui.space(6);
    } else {
        ui.space(4);
    }
    if ui.button("Settings") {
        action = Action::Push(Screen::Settings);
    }
    if ctx.caps.can_quit && ui.button("Quit") {
        action = Action::Quit;
    }
    ui.end_list();
    let drop = if ctx.caps.drop_files {
        "Drop a ROM file here to play it"
    } else {
        ""
    };
    hint(ui, ctx, drop, "A select · Up/Down move", "");
    action
}

fn open_browser(ctx: &mut Ctx<'_>) -> Action {
    let mut browser = Browser::open(ctx.platform, ctx.settings.browse_dir.as_deref());
    if browser.error.is_some() {
        // The remembered folder may be gone: start from the default one.
        browser = Browser::open(ctx.platform, None);
    }
    Action::Push(Screen::Browser(browser))
}

// --- File browser ----------------------------------------------------------------

fn browse(browser: &mut Browser, ui: &mut Ui<'_>, ctx: &mut Ctx<'_>) -> Action {
    let max = (ui.column.w / 8) as usize;
    let path = font::ellipsize(&browser.listing.path, max, true);
    ui.title("Open ROM", Some(&path));
    let mut action = Action::None;
    let mut go_to = None;
    let error_line = browser.error.is_some();
    ui.begin_list(list_bottom(ui) - if error_line { 12 } else { 0 });
    let filter = if browser.show_all {
        "All files"
    } else {
        "ROMs only"
    };
    if ui.choice("Show", filter) != 0 {
        browser.show_all = !browser.show_all;
        browser.refresh();
    }
    let mut first_entry = 1;
    if let Some(parent) = &browser.listing.parent {
        first_entry += 1;
        if ui.button_with(Some(FOLDER), "..", Some("up"), theme::DIM) {
            go_to = Some(parent.clone());
        }
    }
    for entry in &browser.entries {
        let icon = if entry.is_dir {
            Some(FOLDER)
        } else if crate::browser::is_rom(&entry.name) {
            Some(CARTRIDGE)
        } else {
            Some(' ')
        };
        if ui.button_with(icon, &entry.name, None, theme::TEXT) {
            if entry.is_dir {
                go_to = Some(entry.path.clone());
            } else {
                action = Action::Open(entry.path.clone());
            }
        }
    }
    if browser.entries.is_empty() {
        ui.text("No ROMs in this folder", theme::DIM);
    }
    ui.end_list();
    if let Some(error) = &browser.error {
        let y = footer_y(ui) - 14;
        let line = font::ellipsize(error, max, false);
        text_scaled(ui.canvas, ui.column.x, y, &line, theme::ERROR, 1);
    }
    // Type a letter to jump to the next name starting with it.
    if let Some(c) = ui.typed() {
        let current = ui.focused().checked_sub(first_entry);
        if let Some(i) = browser.jump(c, current) {
            ui.set_focus(i + first_entry);
        }
    }
    if let Some(dir) = go_to {
        browser.go(ctx.platform, Some(&dir));
        if browser.error.is_none() {
            ctx.settings.browse_dir = Some(browser.listing.path.clone());
        }
        ui.set_focus(0);
        ui.redraw = true;
    }
    hint(
        ui,
        ctx,
        "Enter open · Esc close · type a letter to jump",
        "A open · B close · LB/RB page",
        "",
    );
    action
}

// --- In-game menus ---------------------------------------------------------------

fn pause(ui: &mut Ui<'_>, ctx: &mut Ctx<'_>) -> Action {
    let title = ctx.game.map_or("", |g| g.title.as_str()).to_string();
    ui.title("Paused", Some(&title));
    let mut action = Action::None;
    ui.begin_list(list_bottom(ui));
    if ui.button("Resume") {
        action = Action::Resume;
    }
    if ui.button("Save state") {
        action = states_screen(true, ctx);
    }
    if ui.button("Load state") {
        action = states_screen(false, ctx);
    }
    if ui.button("Reset") {
        action = Action::Reset;
    }
    if ui.button("Settings") {
        action = Action::Push(Screen::Settings);
    }
    if ui.button("Close game") {
        action = Action::CloseGame;
    }
    ui.end_list();
    hint(
        ui,
        ctx,
        "Esc resume · Tab fast-forward · Backspace rewind",
        "B resume · RT fast-forward · LT rewind",
        "",
    );
    action
}

fn states_screen(save: bool, ctx: &mut Ctx<'_>) -> Action {
    let slots = ctx.game.map(|g| g.slots(ctx.platform)).unwrap_or_default();
    Action::Push(Screen::States { save, slots })
}

fn states(save: bool, slots: &[SlotInfo], ui: &mut Ui<'_>, ctx: &mut Ctx<'_>) -> Action {
    let (title, caption) = if save {
        ("Save state", "Choose a slot to save to")
    } else {
        ("Load state", "Choose a slot to continue from")
    };
    ui.title(title, Some(caption));
    let current = ctx.game.map_or(0, |g| g.slot);
    let (cw, ch, gap) = (PREVIEW.0 as i32 + 6, PREVIEW.1 as i32 + 6 + 12, 6);
    let width = ui.column.w + 2 * theme::MARGIN - 8;
    let cols = ((width + gap) / (cw + gap)).clamp(1, 5);
    let x0 = (ui.canvas.width as i32 - (cols * cw + (cols - 1) * gap)) / 2;
    let mut action = Action::None;
    ui.begin_list(list_bottom(ui));
    let top = ui.y;
    for slot in 0..SLOTS {
        let (col, row) = (i32::from(slot) % cols, i32::from(slot) / cols);
        let rect = Rect::new(x0 + col * (cw + gap), top + row * (ch + gap), cw, ch);
        let info = slots.get(usize::from(slot));
        let preview = info.and_then(|s| s.preview.as_ref()).map(|c| c.image());
        let empty = !info.is_some_and(|s| s.exists);
        let caption = if empty && !save {
            format!("{slot}")
        } else {
            format!("Slot {slot}")
        };
        if ui.slot(rect, preview, &caption, slot == current) {
            action = if save {
                Action::SaveState(slot)
            } else {
                Action::LoadState(slot)
            };
        }
    }
    let rows = (i32::from(SLOTS) + cols - 1) / cols;
    ui.y = top + rows * (ch + gap);
    ui.end_list();
    hint(
        ui,
        ctx,
        "Arrows choose · Enter select · Esc back · F5/F8 use the gold slot",
        "D-pad choose · A select · B back",
        "",
    );
    action
}

// --- Settings ----------------------------------------------------------------------

fn settings_menu(ui: &mut Ui<'_>, ctx: &mut Ctx<'_>) -> Action {
    ui.title("Settings", None);
    let mut action = Action::None;
    ui.begin_list(list_bottom(ui));
    let s = &*ctx.settings;
    let audio = if s.audio.mute {
        "muted".to_string()
    } else {
        format!("{}%", s.audio.volume)
    };
    /// A settings page: its name, a summary, and how to make it.
    type Page = (&'static str, String, fn() -> Screen);
    let pages: [Page; 4] = [
        ("Video", aspect_label(s.video.aspect).to_string(), || {
            Screen::Video
        }),
        ("Audio", audio, || Screen::Audio),
        (
            "Emulation",
            region_label(s.emulation.region).to_string(),
            || Screen::Emulation,
        ),
        ("Controls", String::new(), || Screen::Controls),
    ];
    for (label, detail, screen) in pages {
        if ui.button_with(None, label, Some(&detail), theme::TEXT) {
            action = Action::Push(screen());
        }
    }
    ui.end_list();
    back_hint(ui, ctx);
    action
}

/// Step through `options` by `delta` (wrapping).
fn cycle<T: PartialEq + Copy>(options: &[T], current: T, delta: i32) -> T {
    let i = options.iter().position(|&o| o == current).unwrap_or(0) as i32;
    let n = options.len() as i32;
    options[(i + delta).rem_euclid(n) as usize]
}

fn aspect_label(a: Aspect) -> &'static str {
    match a {
        Aspect::Console => "Console 10:7",
        Aspect::Tv => "TV 4:3",
        Aspect::Stretch => "Stretch",
    }
}

fn region_label(r: Option<Region>) -> &'static str {
    match r {
        None => "Auto",
        Some(Region::Japan) => "Japan",
        Some(Region::Americas) => "USA",
        Some(Region::Europe) => "Europe",
    }
}

fn video(ui: &mut Ui<'_>, ctx: &mut Ctx<'_>) -> Action {
    ui.title("Video", None);
    ui.begin_list(list_bottom(ui));
    let v = &mut ctx.settings.video;
    let d = ui.choice("Aspect ratio", aspect_label(v.aspect));
    if d != 0 {
        v.aspect = cycle(&[Aspect::Console, Aspect::Tv, Aspect::Stretch], v.aspect, d);
    }
    if ui.toggle("Integer scaling", v.integer_scale) {
        v.integer_scale = !v.integer_scale;
    }
    if ctx.caps.fullscreen {
        if ui.toggle("Fullscreen", v.fullscreen) {
            v.fullscreen = !v.fullscreen;
        }
        let d = ui.choice("Window size", &format!("{}x", v.scale));
        v.scale = (v.scale as i32 + d).clamp(1, 8) as u32;
    }
    ui.space(4);
    ui.text("Integer scaling gives every pixel the", theme::DIM);
    ui.text("same size: sharpest, with borders.", theme::DIM);
    ui.end_list();
    back_hint(ui, ctx);
    Action::None
}

fn audio(ui: &mut Ui<'_>, ctx: &mut Ctx<'_>) -> Action {
    ui.title("Audio", None);
    ui.begin_list(list_bottom(ui));
    let a = &mut ctx.settings.audio;
    let d = ui.choice("Volume", &format!("{}%", a.volume));
    a.volume = (i32::from(a.volume) + 10 * d).clamp(0, 100) as u8;
    if ui.toggle("Mute", a.mute) {
        a.mute = !a.mute;
    }
    if ui.toggle("Low-pass filter", a.low_pass) {
        a.low_pass = !a.low_pass;
    }
    ui.space(4);
    ui.text("The filter softens the sound like the", theme::DIM);
    ui.text("first Mega Drive. From the next start.", theme::DIM);
    ui.end_list();
    back_hint(ui, ctx);
    Action::None
}

fn emulation(ui: &mut Ui<'_>, ctx: &mut Ctx<'_>) -> Action {
    ui.title("Emulation", None);
    ui.begin_list(list_bottom(ui));
    let e = &mut ctx.settings.emulation;
    let regions = [
        None,
        Some(Region::Japan),
        Some(Region::Americas),
        Some(Region::Europe),
    ];
    let d = ui.choice("Region", region_label(e.region));
    if d != 0 {
        e.region = cycle(&regions, e.region, d);
    }
    let errors = if e.lenient_address_errors {
        "Lenient"
    } else {
        "Strict"
    };
    if ui.choice("Address errors", errors) != 0 {
        e.lenient_address_errors = !e.lenient_address_errors;
    }
    if ui.toggle("Rewind", e.rewind) {
        e.rewind = !e.rewind;
    }
    let d = ui.choice("Fast forward", &format!("{}x", e.fast_forward));
    e.fast_forward = (e.fast_forward as i32 + d).clamp(2, 8) as u32;
    ui.space(4);
    ui.text("Region and address errors apply from", theme::DIM);
    ui.text("the next game start. Lenient address", theme::DIM);
    ui.text("errors are for buggy homebrew only.", theme::DIM);
    ui.end_list();
    back_hint(ui, ctx);
    Action::None
}

fn device_label(d: Device) -> &'static str {
    match d {
        Device::ThreeButton => "3 buttons",
        Device::SixButton => "6 buttons",
        Device::None => "None",
    }
}

fn controls(ui: &mut Ui<'_>, ctx: &mut Ctx<'_>) -> Action {
    ui.title("Controls", None);
    let mut action = Action::None;
    ui.begin_list(list_bottom(ui));
    for p in 0..2 {
        let player = &mut ctx.settings.players[p];
        let d = ui.choice(
            &format!("Player {} pad", p + 1),
            device_label(player.device),
        );
        if d != 0 {
            player.device = cycle(&[Device::ThreeButton, Device::SixButton], player.device, d);
        }
    }
    let touch = match ctx.settings.touch {
        TouchMode::Auto => "Auto",
        TouchMode::On => "On",
        TouchMode::Off => "Off",
    };
    let d = ui.choice("Touch controls", touch);
    if d != 0 {
        ctx.settings.touch = cycle(
            &[TouchMode::Auto, TouchMode::On, TouchMode::Off],
            ctx.settings.touch,
            d,
        );
    }
    ui.section("REMAP");
    for p in 0..2 {
        for (pad, name) in [(false, "keyboard"), (true, "gamepad")] {
            if ui.button_with(
                None,
                &format!("Player {} {name}", p + 1),
                Some("▶"),
                theme::TEXT,
            ) {
                action = Action::Push(Screen::Remap(Remap {
                    player: p,
                    pad,
                    waiting: None,
                    since_ms: 0,
                }));
            }
        }
    }
    ui.section("GAMEPADS");
    if ctx.pads.is_empty() {
        ui.text("None connected", theme::FAINT);
    }
    for (name, player) in &ctx.pads {
        ui.text(&format!("P{} {name}", player + 1), theme::DIM);
    }
    ui.end_list();
    back_hint(ui, ctx);
    action
}

fn remap_screen(remap: &mut Remap, ui: &mut Ui<'_>, ctx: &mut Ctx<'_>) -> Action {
    let device = if remap.pad { "gamepad" } else { "keyboard" };
    let caption = if remap.pad {
        "Pad buttons by position: A bottom, B right"
    } else {
        "Pick a button, then press its new key"
    };
    ui.title(
        &format!("Player {} {device}", remap.player + 1),
        Some(caption),
    );
    ui.begin_list(list_bottom(ui));
    let bindings = &mut ctx.settings.players[remap.player].bindings;
    for &button in ConsoleButton::ALL {
        let value = if remap.pad {
            bindings.pad[button.index()].map_or("-", |b| b.label())
        } else {
            bindings.keys[button.index()].map_or("-", |k| k.name())
        };
        let value = if remap.waiting == Some(button) {
            "…"
        } else {
            value
        };
        if ui.value_row(button.label(), value) && remap.waiting.is_none() {
            remap.waiting = Some(button);
            remap.since_ms = ctx.now_ms;
        }
    }
    ui.space(4);
    if ui.button("Restore defaults") {
        let defaults = PlayerBindings::default_for(remap.player);
        if remap.pad {
            bindings.pad = defaults.pad;
        } else {
            bindings.keys = defaults.keys;
        }
    }
    ui.end_list();
    back_hint(ui, ctx);

    if let Some(button) = remap.waiting {
        // A modal box: the next key press goes to the binding.
        let (w, h) = (ui.canvas.width as i32, ui.canvas.height as i32);
        ui.canvas
            .blend(ui.canvas.bounds(), with_alpha(0x000000, 0x90));
        let bw = (w - 2 * theme::MARGIN).min(260);
        let r = Rect::new((w - bw) / 2, h / 2 - 34, bw, 68);
        ui.canvas.fill(r, theme::PANEL);
        ui.canvas.frame(r, 1, theme::LINE);
        ui.canvas.fill(Rect::new(r.x, r.y, r.w, 2), theme::ACCENT);
        let what = if remap.pad { "a pad button" } else { "a key" };
        let line1 = format!("Press {what} for");
        let lx = r.x + (r.w - text_width(&line1, 1) - 8 - text_width(button.label(), 2)) / 2;
        let end = text_scaled(ui.canvas, lx, r.y + 18, &line1, theme::TEXT, 1);
        text_scaled(ui.canvas, end + 8, r.y + 14, button.label(), theme::GOLD, 2);
        let line2 = if remap.pad {
            "Esc or wait to cancel"
        } else {
            "Esc cancels · Backspace clears"
        };
        let l2x = r.x + (r.w - text_width(line2, 1)) / 2;
        text_scaled(ui.canvas, l2x, r.y + 44, line2, theme::DIM, 1);
    }
    Action::None
}
