//! Whole-system tests using tiny hand-assembled programs, so no ROM files
//! are needed.

mod common;

use common::{Rom, VDP_CTRL, VDP_DATA};
use gase_core::{Cartridge, Config, Genesis};

#[test]
fn backdrop_colour_reaches_the_frame() {
    let mut rom = Rom::new();
    rom.move_w(0x8144, VDP_CTRL) // display on, mode 5
        .move_w(0x8C81, VDP_CTRL) // H40
        .move_w(0x8700, VDP_CTRL) // backdrop = palette 0 colour 0
        .move_l(0xC000_0000, VDP_CTRL) // CRAM write, address 0
        .move_w(0x000E, VDP_DATA) // red
        .spin();
    let mut console = rom.console();
    console.run_frame();
    console.run_frame();
    let frame = console.frame();
    assert_eq!((frame.width, frame.height), (320, 224));
    assert_eq!(frame.pixels[100 * frame.stride + 100], 0xFF0000);
}

#[test]
fn vertical_interrupt_runs_once_per_frame() {
    let mut rom = Rom::new();
    rom.vector(30, 0x400) // level 6 autovector
        .move_w(0x8164, VDP_CTRL) // display on, VINT enabled
        .words(&[0x46FC, 0x2000]) // move #$2000, sr: unmask interrupts
        .spin();
    rom.at(0x400).words(&[0x5279, 0x00FF, 0x0000, 0x4E73]); // addq.w #1, ($FF0000).l; rte
    let mut console = rom.console();
    for _ in 0..10 {
        console.run_frame();
    }
    let count = u16::from_be_bytes([console.hw.ram[0], console.hw.ram[1]]);
    assert!(
        (9..=10).contains(&count),
        "VINT ran {count} times in 10 frames"
    );
}

#[test]
fn z80_program_loaded_by_68000_runs() {
    // Z80: ld a, $42 / ld ($1000), a / jr $
    let z80_code = [0x3E, 0x42, 0x32, 0x00, 0x10, 0x18, 0xFE];
    let mut rom = Rom::new();
    rom.move_w(0x0100, 0xA1_1100) // request the Z80 bus
        .move_w(0x0100, 0xA1_1200); // release reset
    for (i, &byte) in z80_code.iter().enumerate() {
        rom.move_b(byte, 0xA0_0000 + i as u32);
    }
    rom.move_w(0x0000, 0xA1_1200) // reset the Z80
        .move_w(0x0000, 0xA1_1100) // give the bus back
        .move_w(0x0100, 0xA1_1200) // and let it run
        .spin();
    let mut console = rom.console();
    console.run_frame();
    assert_eq!(console.hw.zram[0x1000], 0x42);
}

#[test]
fn save_state_round_trip_is_deterministic() {
    let mut rom = Rom::new();
    rom.vector(30, 0x400)
        .move_w(0x8164, VDP_CTRL)
        .words(&[0x46FC, 0x2000])
        .spin();
    rom.at(0x400).words(&[0x5279, 0x00FF, 0x0000, 0x4E73]);
    let mut a = rom.console();
    for _ in 0..5 {
        a.run_frame();
    }
    let state = a.save_state();
    let mut b = rom.console();
    b.load_state(&state).unwrap();
    for _ in 0..5 {
        a.run_frame();
        b.run_frame();
    }
    assert_eq!(a.save_state(), b.save_state());
}

#[test]
fn rejects_foreign_save_state() {
    let mut console = Rom::new().spin().console();
    assert!(console.load_state(b"not a state").is_err());
}

#[test]
fn audio_is_produced_at_the_host_rate() {
    let mut console = Rom::new().spin().console();
    let mut audio = Vec::new();
    for _ in 0..60 {
        console.run_frame();
        console.drain_audio(&mut audio);
    }
    // 60 NTSC frames ≈ 1.0013 s of stereo audio at 48 kHz.
    let frames = audio.len() / 2;
    assert!(
        (47_000..49_200).contains(&frames),
        "got {frames} sample frames"
    );
}

#[test]
fn lenient_address_errors_option() {
    // move.l #$11223344,d0 ; move.w d0,($FF0001).l ; spin
    let mut rom = Rom::new();
    rom.vector(3, 0x400) // address error -> handler that marks RAM
        .words(&[0x203C, 0x1122, 0x3344, 0x33C0, 0x00FF, 0x0001])
        .spin();
    rom.at(0x400).move_w(0xDEAD, 0xFF_0010).spin();

    let mut strict = rom.console();
    strict.run_frame();
    assert_eq!(
        &strict.hw.ram[0x10..0x12],
        &[0xDE, 0xAD],
        "hardware: address error"
    );

    let config = Config {
        address_errors: false,
        ..Config::default()
    };
    let mut lenient = Genesis::new(Cartridge::from_bytes(&rom.bytes).unwrap(), &config);
    lenient.run_frame();
    assert_eq!(
        &lenient.hw.ram[0..2],
        &[0x33, 0x44],
        "lenient: written at $FF0000"
    );
    assert_eq!(&lenient.hw.ram[0x10..0x12], &[0, 0]);
}
