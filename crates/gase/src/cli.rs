//! Command-line parsing (hand-written to stay dependency-free).

use std::path::PathBuf;

use gase_core::Region;

pub const USAGE: &str = "\
gase - Sega Mega Drive / Genesis emulator

USAGE:
    gase [OPTIONS] <ROM>

OPTIONS:
    --region <jp|us|eu>   Force the console region (default: from the ROM header)
    --scale <N>           Initial window scale (default: 3)
    --fullscreen          Start in fullscreen
    --integer-scale       Only scale by whole multiples (sharpest pixels)
    --no-filter           Disable the model 1 audio low-pass filter
    --no-audio            Run without sound (paced by the display instead)
    --headless            Run without a window (see the options below)
    --frames <N>          Headless: number of frames to run (default: 600)
    --screenshot <PATH>   Headless: save the last frame as PNG
    --wav <PATH>          Headless: record the audio as WAV
    --bench               Headless: report emulation speed
    --trace <N>           Print the first N 68000 instructions executed
    -h, --help            Show this help
    -V, --version         Show the version

KEYS (window mode):
    Arrows        D-pad            Enter         Start
    Z / X / C     A / B / C        A / S / D     X / Y / Z
    Q             Mode
    P             Pause            N             Next frame (while paused)
    Tab (hold)    Fast forward     Backspace     Rewind (hold)
    F5 / F8       Save / load state               F6 / F7    Previous / next slot
    F9            Reset            F11           Fullscreen
    F12           Screenshot       M             Mute
    Esc           Quit

Game controllers are supported: the first one is player 1, the second player 2.
";

/// Parsed command line.
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    pub rom: PathBuf,
    pub region: Option<Region>,
    pub scale: u32,
    pub fullscreen: bool,
    pub integer_scale: bool,
    pub low_pass: bool,
    pub audio: bool,
    pub headless: bool,
    pub frames: u64,
    pub screenshot: Option<PathBuf>,
    pub wav: Option<PathBuf>,
    pub bench: bool,
    pub trace: u64,
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

fn parse_number<T: std::str::FromStr>(flag: &str, value: &str) -> Result<T, String> {
    value.parse().map_err(|_| format!("{flag} expects a number, got '{value}'"))
}

/// Parse arguments (without the program name).
pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Command, String> {
    let mut args = args.into_iter();
    let mut rom = None;
    let mut o = Options {
        rom: PathBuf::new(),
        region: None,
        scale: 3,
        fullscreen: false,
        integer_scale: false,
        low_pass: true,
        audio: true,
        headless: false,
        frames: 600,
        screenshot: None,
        wav: None,
        bench: false,
        trace: 0,
    };

    while let Some(arg) = args.next() {
        let mut value = |flag: &str| args.next().ok_or_else(|| format!("{flag} expects a value"));
        match arg.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "-V" | "--version" => return Ok(Command::Version),
            "--region" => o.region = Some(parse_region(&value("--region")?)?),
            "--scale" => o.scale = parse_number::<u32>("--scale", &value("--scale")?)?.clamp(1, 16),
            "--fullscreen" => o.fullscreen = true,
            "--integer-scale" => o.integer_scale = true,
            "--no-filter" => o.low_pass = false,
            "--no-audio" => o.audio = false,
            "--headless" => o.headless = true,
            "--frames" => o.frames = parse_number("--frames", &value("--frames")?)?,
            "--screenshot" => o.screenshot = Some(value("--screenshot")?.into()),
            "--wav" => o.wav = Some(value("--wav")?.into()),
            "--bench" => o.bench = true,
            "--trace" => o.trace = parse_number("--trace", &value("--trace")?)?,
            flag if flag.starts_with('-') && flag.len() > 1 => return Err(format!("unknown option '{flag}'")),
            path => {
                if rom.replace(PathBuf::from(path)).is_some() {
                    return Err("only one ROM can be given".into());
                }
            }
        }
    }
    o.rom = rom.ok_or("no ROM given (try --help)")?;
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
        assert_eq!(o.rom, PathBuf::from("sonic.bin"));
        assert_eq!(o.scale, 3);
        assert!(o.low_pass && o.audio && !o.headless);
    }

    #[test]
    fn options() {
        let o = run(&["--region", "eu", "--headless", "--frames", "10", "game.md", "--bench"]).unwrap();
        assert_eq!(o.region, Some(Region::Europe));
        assert!(o.headless && o.bench);
        assert_eq!(o.frames, 10);
    }

    #[test]
    fn errors() {
        assert!(run(&[]).is_err());
        assert!(run(&["--frames"]).is_err());
        assert!(run(&["--frames", "x", "a.bin"]).is_err());
        assert!(run(&["--wat", "a.bin"]).is_err());
        assert!(run(&["a.bin", "b.bin"]).is_err());
        assert_eq!(parse(["--help".to_string()]), Ok(Command::Help));
    }
}
