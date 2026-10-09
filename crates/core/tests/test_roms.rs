//! Regression tests on real (freely licensed) Mega Drive software.
//!
//! The ROMs are not part of the repository; fetch them with
//! `scripts/fetch-test-roms.sh` (or point `$GASE_TEST_ROMS` at a directory
//! with the same layout), then run:
//!
//! ```sh
//! cargo test -p gase-core --profile fast-test --test test_roms -- --ignored
//! ```
//!
//! Each case runs a ROM for a fixed number of frames, with scripted button
//! presses, and compares a hash of the final picture with a known-good value.
//! The known-good frames were checked by eye against real hardware
//! behaviour. When a change intentionally alters the output (e.g. a colour
//! conversion fix), re-check the screenshots and update the hashes with
//! `GASE_BLESS=1`, which prints the new values instead of failing.

use std::path::PathBuf;

use gase_core::{Buttons, Cartridge, Config, Genesis};

struct Case {
    name: &'static str,
    path: &'static str,
    frames: u32,
    /// Hold these buttons on pad 1 during the given frame range.
    presses: &'static [(u32, u32, Buttons)],
    /// FNV-1a hash of the visible frame after `frames` frames.
    expected: u64,
}

const CASES: &[Case] = &[
    Case {
        name: "240p test suite main menu",
        path: "240p-test-suite/240pSuite-1.23.bin",
        frames: 300,
        presses: &[],
        expected: 0xead09c2ed0bcdcb4,
    },
    Case {
        name: "240p test suite PLUGE",
        path: "240p-test-suite/240pSuite-1.23.bin",
        frames: 400,
        // Test Patterns -> first entry. The second press waits for the
        // pattern menu, which takes ~19 frames to load with the display on.
        presses: &[(300, 305, Buttons::A), (345, 350, Buttons::A)],
        expected: 0x92f08ddbca1d550a,
    },
    Case {
        name: "genmd-imgrom test pattern",
        path: "genmd-imgrom-testpattern/testpattern.bin",
        frames: 120,
        presses: &[],
        expected: 0x1b56fe5fc7f7fa4a,
    },
    // Airstriker's compiler-generated code later writes a word to an odd
    // address, which faults on a real 68000 (lenient emulators ignore it),
    // so only its boot sequence is checked.
    Case {
        name: "Airstriker SEGA logo",
        path: "airstriker/Airstriker.md",
        frames: 60,
        presses: &[],
        expected: 0x4d6a175bde9be241,
    },
    Case {
        name: "Right 2 Repair title",
        path: "right2repair-ggj2020/rom.bin",
        frames: 600,
        presses: &[],
        expected: 0x45482c258a415507,
    },
    Case {
        name: "The Spiral demo",
        path: "resistance-the-spiral/rom.bin",
        frames: 1200,
        presses: &[],
        expected: 0xb268223d07378971,
    },
];

fn rom_dir() -> PathBuf {
    std::env::var_os("GASE_TEST_ROMS").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-roms"),
        PathBuf::from,
    )
}

fn frame_hash(console: &Genesis) -> u64 {
    let frame = console.frame();
    let mut hash: u64 = 0xCBF2_9CE4_8422_2325;
    let mut feed = |v: u32| {
        for b in v.to_le_bytes() {
            hash = (hash ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01B3);
        }
    };
    feed(frame.width as u32);
    feed(frame.height as u32);
    for row in frame.pixels.chunks(frame.stride).take(frame.height) {
        row[..frame.width].iter().for_each(|&p| feed(p));
    }
    hash
}

/// Save the frame as a PPM so blessed hashes can be checked by eye.
fn dump_ppm(console: &Genesis, path: &std::path::Path) {
    let frame = console.frame();
    let mut out = format!("P6 {} {} 255\n", frame.width, frame.height).into_bytes();
    for row in frame.pixels.chunks(frame.stride).take(frame.height) {
        for &p in &row[..frame.width] {
            out.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
        }
    }
    std::fs::write(path, out).unwrap();
}

#[test]
#[ignore = "needs ROMs from scripts/fetch-test-roms.sh"]
fn test_roms() {
    let dir = rom_dir();
    let bless = std::env::var_os("GASE_BLESS").is_some();
    let mut failures = Vec::new();
    for case in CASES {
        let path = dir.join(case.path);
        let data = std::fs::read(&path)
            .unwrap_or_else(|e| panic!("{}: {e} (run scripts/fetch-test-roms.sh)", path.display()));
        let mut console = Genesis::new(Cartridge::from_bytes(&data).unwrap(), &Config::default());
        for frame in 0..case.frames {
            let mut buttons = Buttons::default();
            for &(start, end, b) in case.presses {
                if (start..end).contains(&frame) {
                    buttons = buttons | b;
                }
            }
            console.set_buttons(0, buttons);
            console.run_frame();
        }
        let hash = frame_hash(&console);
        if bless {
            println!("{:<32} expected: {hash:#018x},", case.name);
            dump_ppm(
                &console,
                &dir.join(format!("{}.ppm", case.name.replace(' ', "_"))),
            );
        } else if hash != case.expected {
            failures.push(format!(
                "{}: got {hash:#018x}, expected {:#018x}",
                case.name, case.expected
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "frame mismatches:\n{}",
        failures.join("\n")
    );
}
