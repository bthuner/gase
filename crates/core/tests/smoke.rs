//! Whole-system tests using tiny hand-assembled programs, so no ROM files
//! are needed.

use gase_core::{Cartridge, Config, Genesis};

/// Builds a ROM image: vectors, a minimal header, and code at 0x200.
struct Rom {
    bytes: Vec<u8>,
    pc: usize,
}

impl Rom {
    fn new() -> Self {
        let mut bytes = vec![0u8; 0x1000];
        bytes[0..4].copy_from_slice(&0x00FF_FE00u32.to_be_bytes()); // initial SSP
        bytes[4..8].copy_from_slice(&0x0000_0200u32.to_be_bytes()); // initial PC
        bytes[0x100..0x110].copy_from_slice(b"SEGA MEGA DRIVE ");
        bytes[0x1F0] = b'U';
        Self { bytes, pc: 0x200 }
    }

    fn words(&mut self, words: &[u16]) -> &mut Self {
        for w in words {
            self.bytes[self.pc..self.pc + 2].copy_from_slice(&w.to_be_bytes());
            self.pc += 2;
        }
        self
    }

    fn vector(&mut self, number: usize, target: u32) -> &mut Self {
        self.bytes[number * 4..number * 4 + 4].copy_from_slice(&target.to_be_bytes());
        self
    }

    fn at(&mut self, pc: usize) -> &mut Self {
        self.pc = pc;
        self
    }

    // A few 68000 instructions, encoded by hand.

    /// `move.w #imm, (addr).l`
    fn move_w(&mut self, imm: u16, addr: u32) -> &mut Self {
        self.words(&[0x33FC, imm, (addr >> 16) as u16, addr as u16])
    }

    /// `move.l #imm, (addr).l`
    fn move_l(&mut self, imm: u32, addr: u32) -> &mut Self {
        self.words(&[0x23FC, (imm >> 16) as u16, imm as u16, (addr >> 16) as u16, addr as u16])
    }

    /// `move.b #imm, (addr).l`
    fn move_b(&mut self, imm: u8, addr: u32) -> &mut Self {
        self.words(&[0x13FC, u16::from(imm), (addr >> 16) as u16, addr as u16])
    }

    /// `bra.s *` (spin forever)
    fn spin(&mut self) -> &mut Self {
        self.words(&[0x60FE])
    }

    fn console(&self) -> Genesis {
        Genesis::new(Cartridge::from_bytes(&self.bytes).unwrap(), &Config::default())
    }
}

const VDP_CTRL: u32 = 0xC0_0004;
const VDP_DATA: u32 = 0xC0_0000;

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
    assert!((9..=10).contains(&count), "VINT ran {count} times in 10 frames");
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
    rom.vector(30, 0x400).move_w(0x8164, VDP_CTRL).words(&[0x46FC, 0x2000]).spin();
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
    assert!((47_000..49_200).contains(&frames), "got {frames} sample frames");
}
