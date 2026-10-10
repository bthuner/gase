//! Command-line parsing (hand-written to stay dependency-free).

use std::path::PathBuf;

use gase_core::Region;

pub const USAGE: &str = "\
gase - Sega Mega Drive / Genesis emulator

USAGE:
    gase [OPTIONS] [ROM]

Without a ROM, gase opens its home screen: open a game from there, from the
recent list, or by dropping a ROM file onto the window. Esc (or the Guide
button of a gamepad) opens the menu during a game: save states, settings,
controls. Settings are kept in the user's configuration folder
($GASE_CONFIG_DIR overrides it); saves and states next to the ROM.

OPTIONS (these override the settings for this run):
    --region <jp|us|eu>   Force the console region (default: from the ROM header)
    --scale <N>           Initial window size, N x 320x224 (default: 3)
    --fullscreen          Start in fullscreen
    --integer-scale       Only scale by whole multiples (sharpest pixels)
    --no-filter           Disable the model 1 audio low-pass filter
    --no-audio            Run without sound (paced by the display instead)
    --no-address-errors   Ignore odd-address word accesses like lenient emulators
                          (for buggy homebrew; real hardware would crash)
    --touch               Show the on-screen touch controls
    --mobile              Run the phone app's shell in a window, as on Android
                          and iOS: touch first, files in the app's data folder
                          (~/.local/share/gase on Linux), no debugger; the
                          window is --ui-size big (e.g. --ui-size 540x1170);
                          with --dump-ui: the phone's menus

HEADLESS (no window: tests, CI, benchmarks, recordings):
    --headless            Run without a window
    --frames <N>          Number of frames to run (default: 600)
    --screenshot <PATH>   Save the last frame as PNG
    --wav <PATH>          Record the audio as WAV
    --bench               Report emulation speed
    --dump-ui <PATH>      Save a picture of the user interface as PNG, after
                          the frames (works without a ROM: the home screen)
    --ui-screen <LIST>    Screens to open for --dump-ui, comma-separated:
                          home, game, pause, save, load, settings, video, audio,
                          emulation, controls, remap, remap-waiting, browser,
                          or key:<NAME> to press a key first (e.g. key:F5)
                          (default: pause with a ROM, home without)
    --ui-size <WxH>       Window size for --dump-ui and --mobile (default: 960x672)
    --ui-density <F>      Pixels per point for --dump-ui (default: 1; phones 2-3)

DEBUGGING:
    --trace <N>           Print the first N 68000 instructions executed
    --debug               Window: open the debugger at start, paused
    --break <ADDR>        Stop at this 68000 address (hex, e.g. 200 or $200;
                          repeatable). Headless: print the registers and
                          disassembly, then stop running
    --dump-vram <PATH>    Headless: save all 2048 VRAM tiles (palette 0) as PNG
    --dump-cram <PATH>    Headless: save the four palettes (CRAM) as PNG
    --dump-debugger <PATH> Headless: save a picture of the debugger window
    -h, --help            Show this help
    -V, --version         Show the version

KEYS DURING A GAME (change the console buttons in Menu > Settings > Controls):
    Arrows        D-pad            Enter         Start
    Z / X / C     A / B / C        A / S / D     X / Y / Z
    Q             Mode             Esc           Menu
    P             Pause            N             Next frame (while paused)
    Tab (hold)    Fast forward     Backspace     Rewind (hold)
    F5 / F8       Save / load state               F6 / F7    Previous / next slot
    F9            Reset            F11           Fullscreen
    F12           Screenshot       M             Mute
    F1 or `       Debugger window (see README)

IN MENUS:
    Arrows move, Enter selects, Left/Right change a setting, Esc goes back.
    The mouse and touch screens work too.

GAMEPADS (any controller SDL knows; first = player 1, second = player 2):
    D-pad / left stick   D-pad      X A B (left, bottom, right)   A B C
    LB Y RB              X Y Z      Start / Back                 Start / Mode
    Guide, or Back+Start Menu       LT / RT (hold)               Rewind / fast forward
";

/// Command-line options override the settings for this run.
pub fn apply_overrides(app: &mut gase_app::App, options: &Options) {
    let s = app.settings_mut();
    if let Some(scale) = options.scale {
        s.video.scale = scale;
    }
    s.video.fullscreen |= options.fullscreen;
    s.video.integer_scale |= options.integer_scale;
    s.audio.low_pass &= options.low_pass;
    if options.region.is_some() {
        s.emulation.region = options.region;
    }
    s.emulation.lenient_address_errors |= !options.address_errors;
    if options.touch {
        s.touch = gase_app::TouchMode::On;
    }
}

/// Parsed command line.
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    pub rom: Option<PathBuf>,
    pub region: Option<Region>,
    pub scale: Option<u32>,
    pub fullscreen: bool,
    pub integer_scale: bool,
    pub low_pass: bool,
    pub audio: bool,
    pub address_errors: bool,
    pub headless: bool,
    pub frames: u64,
    pub screenshot: Option<PathBuf>,
    pub wav: Option<PathBuf>,
    pub bench: bool,
    pub trace: u64,
    pub debug: bool,
    pub breakpoints: Vec<u32>,
    pub dump_vram: Option<PathBuf>,
    pub dump_cram: Option<PathBuf>,
    pub dump_debugger: Option<PathBuf>,
    pub touch: bool,
    pub dump_ui: Option<PathBuf>,
    pub ui_screens: Option<String>,
    pub ui_size: (u32, u32),
    pub ui_density: f32,
    pub mobile: bool,
}

/// What the command line asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Run(Box<Options>),
    Help,
    Version,
}

fn parse_region(value: &str) -> Result<Region, String> {
    match value.to_ascii_lowercase().as_str() {
        "jp" | "j" | "japan" => Ok(Region::Japan),
        "us" | "u" | "usa" | "americas" => Ok(Region::Americas),
        "eu" | "e" | "europe" | "pal" => Ok(Region::Europe),
        _ => Err(format!("unknown region '{value}' (expected jp, us or eu)")),
    }
}

/// Parse a 68000 address in hex, with an optional `$` or `0x` prefix.
fn parse_address(value: &str) -> Result<u32, String> {
    let digits = value
        .strip_prefix('$')
        .or_else(|| value.strip_prefix("0x"))
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    match u32::from_str_radix(digits, 16) {
        Ok(addr) if addr <= 0xFF_FFFF => Ok(addr),
        _ => Err(format!(
            "--break expects a hex address up to FFFFFF, got '{value}'"
        )),
    }
}

/// `960x672` → (960, 672).
fn parse_size(value: &str) -> Result<(u32, u32), String> {
    let error = || format!("--ui-size expects WIDTHxHEIGHT, got '{value}'");
    let (w, h) = value.split_once(['x', 'X']).ok_or_else(error)?;
    let (w, h) = (
        w.parse::<u32>().map_err(|_| error())?,
        h.parse::<u32>().map_err(|_| error())?,
    );
    if !(64..=8192).contains(&w) || !(64..=8192).contains(&h) {
        return Err(error());
    }
    Ok((w, h))
}

fn parse_number<T: std::str::FromStr>(flag: &str, value: &str) -> Result<T, String> {
    value
        .parse()
        .map_err(|_| format!("{flag} expects a number, got '{value}'"))
}

/// Parse arguments (without the program name).
pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Command, String> {
    let mut args = args.into_iter();
    let mut o = Options {
        rom: None,
        region: None,
        scale: None,
        fullscreen: false,
        integer_scale: false,
        low_pass: true,
        audio: true,
        address_errors: true,
        headless: false,
        frames: 600,
        screenshot: None,
        wav: None,
        bench: false,
        trace: 0,
        debug: false,
        breakpoints: Vec::new(),
        dump_vram: None,
        dump_cram: None,
        dump_debugger: None,
        touch: false,
        dump_ui: None,
        ui_screens: None,
        ui_size: (960, 672),
        ui_density: 1.0,
        mobile: false,
    };

    while let Some(arg) = args.next() {
        let mut value = |flag: &str| args.next().ok_or_else(|| format!("{flag} expects a value"));
        match arg.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "-V" | "--version" => return Ok(Command::Version),
            "--region" => o.region = Some(parse_region(&value("--region")?)?),
            "--scale" => {
                o.scale = Some(parse_number::<u32>("--scale", &value("--scale")?)?.clamp(1, 16))
            }
            "--fullscreen" => o.fullscreen = true,
            "--integer-scale" => o.integer_scale = true,
            "--no-filter" => o.low_pass = false,
            "--no-audio" => o.audio = false,
            "--no-address-errors" => o.address_errors = false,
            "--headless" => o.headless = true,
            "--frames" => o.frames = parse_number("--frames", &value("--frames")?)?,
            "--screenshot" => o.screenshot = Some(value("--screenshot")?.into()),
            "--wav" => o.wav = Some(value("--wav")?.into()),
            "--bench" => o.bench = true,
            "--trace" => o.trace = parse_number("--trace", &value("--trace")?)?,
            "--debug" => o.debug = true,
            "--break" => o.breakpoints.push(parse_address(&value("--break")?)?),
            "--dump-vram" => o.dump_vram = Some(value("--dump-vram")?.into()),
            "--dump-cram" => o.dump_cram = Some(value("--dump-cram")?.into()),
            "--dump-debugger" => o.dump_debugger = Some(value("--dump-debugger")?.into()),
            "--touch" => o.touch = true,
            "--mobile" => o.mobile = true,
            "--dump-ui" => o.dump_ui = Some(value("--dump-ui")?.into()),
            "--ui-screen" => o.ui_screens = Some(value("--ui-screen")?),
            "--ui-size" => o.ui_size = parse_size(&value("--ui-size")?)?,
            "--ui-density" => {
                o.ui_density =
                    parse_number::<f32>("--ui-density", &value("--ui-density")?)?.clamp(0.5, 8.0);
            }
            flag if flag.starts_with('-') && flag.len() > 1 => {
                return Err(format!("unknown option '{flag}'"));
            }
            path => {
                if o.rom.replace(PathBuf::from(path)).is_some() {
                    return Err("only one ROM can be given".into());
                }
            }
        }
    }
    if o.headless && o.rom.is_none() && o.dump_ui.is_none() {
        return Err("--headless needs a ROM (try --help)".into());
    }
    Ok(Command::Run(Box::new(o)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> Result<Options, String> {
        match parse(args.iter().map(|s| (*s).to_string()))? {
            Command::Run(o) => Ok(*o),
            other => Err(format!("{other:?}")),
        }
    }

    #[test]
    fn defaults() {
        let o = run(&["sonic.bin"]).unwrap();
        assert_eq!(o.rom, Some(PathBuf::from("sonic.bin")));
        assert_eq!(o.scale, None);
        assert!(o.low_pass && o.audio && o.address_errors && !o.headless);
    }

    #[test]
    fn options() {
        let o = run(&[
            "--region",
            "eu",
            "--headless",
            "--frames",
            "10",
            "game.md",
            "--bench",
        ])
        .unwrap();
        assert_eq!(o.region, Some(Region::Europe));
        assert!(o.headless && o.bench);
        assert_eq!(o.frames, 10);
    }

    #[test]
    fn errors() {
        assert!(run(&["--headless"]).is_err(), "headless needs a ROM");
        assert!(run(&["--ui-size", "12", "--headless", "a.bin"]).is_err());
        assert!(run(&["--frames"]).is_err());
        assert!(run(&["--frames", "x", "a.bin"]).is_err());
        assert!(run(&["--wat", "a.bin"]).is_err());
        assert!(run(&["a.bin", "b.bin"]).is_err());
        assert_eq!(parse(["--help".to_string()]), Ok(Command::Help));
    }

    #[test]
    fn debugger_options() {
        let o = run(&[
            "--break",
            "$200",
            "--break",
            "0x1234",
            "--break",
            "ff00a",
            "--dump-vram",
            "vram.png",
            "--dump-cram",
            "cram.png",
            "--debug",
            "--dump-debugger",
            "debugger.png",
            "game.bin",
        ])
        .unwrap();
        assert_eq!(o.breakpoints, [0x200, 0x1234, 0xF_F00A]);
        assert_eq!(o.dump_vram, Some(PathBuf::from("vram.png")));
        assert_eq!(o.dump_cram, Some(PathBuf::from("cram.png")));
        assert!(o.debug);
        assert_eq!(o.dump_debugger, Some(PathBuf::from("debugger.png")));
        let o = run(&["game.bin"]).unwrap();
        assert!(o.breakpoints.is_empty() && o.dump_vram.is_none() && !o.debug);
        assert!(run(&["--break", "xyz", "a.bin"]).is_err());
        assert!(run(&["--break", "1000000", "a.bin"]).is_err());
        assert!(run(&["--break"]).is_err());
        assert!(run(&["--dump-vram"]).is_err());
    }

    #[test]
    fn gui_options() {
        // No ROM: the home screen.
        let o = run(&[]).unwrap();
        assert_eq!(o.rom, None);
        let o = run(&[
            "--headless",
            "--dump-ui",
            "ui.png",
            "--ui-screen",
            "pause,save",
            "--ui-size",
            "1080x2340",
            "--ui-density",
            "3",
            "--touch",
            "--mobile",
            "--scale",
            "2",
        ])
        .unwrap();
        assert_eq!(o.dump_ui, Some(PathBuf::from("ui.png")));
        assert_eq!(o.ui_screens.as_deref(), Some("pause,save"));
        assert_eq!(o.ui_size, (1080, 2340));
        assert!((o.ui_density - 3.0).abs() < 1e-6);
        assert!(o.touch);
        assert!(o.mobile);
        assert_eq!(o.scale, Some(2));
    }
}
