//! Tests of the debugger support (`gase_core::debug`): stepping, breakpoints
//! and the guarantee that running through the debugger changes nothing.

use gase_core::{Cartridge, Config, Debugger, Genesis, Stop};

const VDP_CTRL: u32 = 0xC0_0004;

/// A tiny hand-assembled ROM: vectors, a minimal header and code at 0x200.
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

    fn at(&mut self, pc: usize) -> &mut Self {
        self.pc = pc;
        self
    }

    /// `move.w #imm, (addr).l`
    fn move_w(&mut self, imm: u16, addr: u32) -> &mut Self {
        self.words(&[0x33FC, imm, (addr >> 16) as u16, addr as u16])
    }

    /// `move.l #imm, (addr).l`
    fn move_l(&mut self, imm: u32, addr: u32) -> &mut Self {
        self.words(&[
            0x23FC,
            (imm >> 16) as u16,
            imm as u16,
            (addr >> 16) as u16,
            addr as u16,
        ])
    }

    /// `move.b #imm, (addr).l`
    fn move_b(&mut self, imm: u8, addr: u32) -> &mut Self {
        self.words(&[0x13FC, u16::from(imm), (addr >> 16) as u16, addr as u16])
    }

    fn console(&self) -> Genesis {
        Genesis::new(
            Cartridge::from_bytes(&self.bytes).unwrap(),
            &Config::default(),
        )
    }
}

/// A program that keeps every part of the scheduler busy: a DMA, a running
/// Z80, a vertical interrupt handler and a main loop writing to RAM.
fn busy_rom() -> Rom {
    let mut rom = Rom::new();
    rom.move_w(0x8174, VDP_CTRL) // display, VINT and DMA on, mode 5
        .move_w(0x8F02, VDP_CTRL) // auto-increment 2
        .move_w(0x9310, VDP_CTRL) // DMA length: 16 words
        .move_w(0x9400, VDP_CTRL)
        .move_w(0x9500, VDP_CTRL) // DMA source: 0
        .move_w(0x9600, VDP_CTRL)
        .move_w(0x9700, VDP_CTRL)
        .move_l(0x4000_0080, VDP_CTRL) // VRAM write at 0, with DMA
        .move_w(0x0100, 0xA1_1100) // Z80 bus request
        .move_w(0x0100, 0xA1_1200);
    // Z80: inc a / ld ($1000),a / jr -6
    for (i, &byte) in [0x3C, 0x32, 0x00, 0x10, 0x18, 0xFA].iter().enumerate() {
        rom.move_b(byte, 0xA0_0000 + i as u32);
    }
    rom.move_w(0x0000, 0xA1_1200)
        .move_w(0x0000, 0xA1_1100)
        .move_w(0x0100, 0xA1_1200)
        .words(&[0x46FC, 0x2000]); // move #$2000, sr: unmask interrupts
    let main_loop = rom.pc as u16;
    rom.words(&[0x5279, 0x00FF, 0x0002]); // addq.w #1, ($FF0002).l
    let displacement = main_loop.wrapping_sub(rom.pc as u16 + 2);
    rom.words(&[0x6000, displacement]); // bra.w main_loop
    // Level 6 handler: addq.w #1, ($FF0000).l ; rte
    rom.at(0x400).words(&[0x5279, 0x00FF, 0x0000, 0x4E73]);
    rom.bytes[30 * 4..30 * 4 + 4].copy_from_slice(&0x400u32.to_be_bytes());
    rom
}

fn assert_same(a: &Genesis, b: &Genesis) {
    assert_eq!(a.frame_count(), b.frame_count());
    assert!(a.save_state() == b.save_state(), "save states differ");
    assert!(a.frame().pixels == b.frame().pixels, "frames differ");
}

#[test]
fn frames_run_through_the_debugger_are_identical() {
    let rom = busy_rom();
    let (mut a, mut b) = (rom.console(), rom.console());
    let mut debugger = Debugger::new();
    for _ in 0..5 {
        a.run_frame();
        assert_eq!(debugger.run_frame(&mut b), Stop::FrameEnd);
        assert!(!debugger.in_frame());
    }
    assert_same(&a, &b);
    // The program really did all those things.
    assert!(b.hw.ram[1] >= 4, "VBlank handler ran {} times", b.hw.ram[1]);
    assert_ne!(b.hw.zram[0x1000], 0, "Z80 ran");
    assert_eq!(&b.hw.vdp.vram[..8], &b.cartridge().rom()[..8], "DMA ran");
}

#[test]
fn stepping_through_a_frame_changes_nothing() {
    let rom = busy_rom();
    let (mut a, mut b) = (rom.console(), rom.console());
    let mut debugger = Debugger::new();
    a.run_frame();
    a.run_frame();
    debugger.run_frame(&mut b);
    // Single-step a few thousand instructions, then finish the frame.
    for _ in 0..5000 {
        assert_eq!(debugger.step_instruction(&mut b), Stop::Stepped);
    }
    assert!(debugger.in_frame());
    assert_eq!(debugger.run_frame(&mut b), Stop::FrameEnd);
    assert_same(&a, &b);
}

#[test]
fn step_executes_one_instruction() {
    let mut rom = Rom::new();
    rom.words(&[0x7001, 0x7202, 0x7403, 0x60FE]); // moveq #1,d0 / moveq #2,d1 / moveq #3,d2 / bra *
    let mut g = rom.console();
    let mut debugger = Debugger::new();
    assert_eq!(g.m68k.pc(), 0x200);
    debugger.step_instruction(&mut g);
    assert_eq!((g.m68k.pc(), g.m68k.d[0], g.m68k.d[1]), (0x202, 1, 0));
    debugger.step_instruction(&mut g);
    assert_eq!((g.m68k.pc(), g.m68k.d[1], g.m68k.d[2]), (0x204, 2, 0));
    assert_eq!(debugger.line(), Some(0));
    assert_eq!(g.disassemble(0x204), ("moveq #$3,d2".to_string(), 2));
}

#[test]
fn breakpoints_stop_before_the_instruction() {
    let mut rom = Rom::new();
    rom.words(&[0x7001, 0x5281, 0x60FC]); // moveq #1,d0 / loop: addq.l #1,d1 / bra loop
    let mut g = rom.console();
    let mut debugger = Debugger::new();
    assert!(debugger.toggle_breakpoint(0x202));
    assert_eq!(debugger.run_frame(&mut g), Stop::Breakpoint(0x202));
    assert_eq!((g.m68k.pc(), g.m68k.d[0], g.m68k.d[1]), (0x202, 1, 0));
    // Continuing executes the instruction under the breakpoint, then stops
    // there again on the next loop iteration.
    assert_eq!(debugger.run_frame(&mut g), Stop::Breakpoint(0x202));
    assert_eq!(g.m68k.d[1], 1);
    // Without breakpoints the frame runs to its end.
    assert!(!debugger.toggle_breakpoint(0x202));
    assert!(debugger.breakpoints().is_empty());
    assert_eq!(debugger.run_frame(&mut g), Stop::FrameEnd);
    assert_eq!(g.frame_count(), 1);
}

#[test]
fn run_to_vblank_then_step_into_the_handler() {
    let mut g = busy_rom().console();
    let mut debugger = Debugger::new();
    // The first vertical interrupt arrives during frame 0, at line 224.
    assert_eq!(debugger.run_to_vblank(&mut g), Stop::VBlank);
    assert_eq!(debugger.line(), Some(224));
    assert_eq!(g.frame_count(), 0);
    debugger.step_instruction(&mut g); // the interrupt exception
    assert_eq!(g.m68k.pc(), 0x400);
    // The next one is a frame later.
    assert_eq!(debugger.run_to_vblank(&mut g), Stop::VBlank);
    assert_eq!(g.frame_count(), 1);
}

#[test]
fn peeking_has_no_side_effects() {
    let rom = busy_rom();
    let mut g = rom.console();
    g.run_frame();
    let before = g.save_state();
    assert_eq!(g.peek_word(0x200), 0x33FC);
    assert_eq!(g.peek_byte(0x201), 0xFC);
    assert_eq!(g.peek_word(0xC0_0004), 0xFFFF); // VDP control port: not read
    assert_eq!(
        g.peek_word(0xFF_0000),
        u16::from_be_bytes([g.hw.ram[0], g.hw.ram[1]])
    );
    assert_eq!(g.peek_z80(0), 0x3C);
    assert_eq!(g.disassemble_z80(0), ("INC A".to_string(), 1));
    assert!(g.save_state() == before);
}

#[test]
fn channel_mutes_round_trip() {
    let mut g = busy_rom().console();
    g.set_channel_mutes(0b10_0001, 0b1000);
    assert_eq!(g.channel_mutes(), (0b10_0001, 0b1000));
}
