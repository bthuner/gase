//! The external-CPU mode (`gase_core::system::external`), checked against
//! the normal scheduler.
//!
//! The simplest possible external CPU is the interpreter itself, stepped
//! through the [`External`] view of the console. Driving it that way must
//! give the same results as `Genesis::run_frame` driving it directly: the
//! same RAM, the same picture, the same registers. That pins down the
//! inverted scheduler that statically recompiled code relies on.

mod common;

use common::{Rom, VDP_CTRL, VDP_DATA};
use gase_core::Genesis;
use gase_core::system::external::{self, External, ExternalCpu};
use gase_m68k::M68k;

/// The interpreter, as an external CPU.
#[derive(Clone, Debug)]
struct Stepper;

impl ExternalCpu for Stepper {
    fn run_frame(&mut self, cpu: &mut M68k, console: &mut External<'_>) {
        let mut cycles = console.cycles();
        while !console.frame_done() {
            console.set_clock(cycles);
            cpu.set_interrupt_level(console.interrupt_level());
            cycles += u64::from(cpu.step(console));
            cycles = console.stalled_clock(cycles);
            console.catch_up(cycles);
        }
    }

    fn clone_box(&self) -> Box<dyn ExternalCpu> {
        Box::new(self.clone())
    }
}

/// Run `frames` frames with and without the external CPU, compare.
fn same_results(rom: &Rom, frames: u32) -> (Genesis, Genesis) {
    let mut normal = rom.console();
    let mut external = rom.console();
    external.set_external_cpu(Some(Box::new(Stepper)));
    for _ in 0..frames {
        normal.run_frame();
        external.run_frame();
    }
    assert_eq!(normal.hw.ram, external.hw.ram, "work RAM");
    assert_eq!(normal.hw.zram, external.hw.zram, "Z80 RAM");
    assert_eq!(normal.frame().pixels, external.frame().pixels, "picture");
    assert_eq!(normal.m68k.d, external.m68k.d, "data registers");
    assert_eq!(normal.m68k.a, external.m68k.a, "address registers");
    assert_eq!(normal.frame_count(), external.frame_count());
    (normal, external)
}

#[test]
fn interrupts_and_display() {
    let mut rom = Rom::new();
    rom.vector(30, 0x400) // VBlank handler counts frames
        .move_w(0x8164, VDP_CTRL) // display on, VINT enabled
        .move_w(0x8C81, VDP_CTRL) // H40
        .move_l(0xC000_0000, VDP_CTRL) // CRAM write, address 0
        .move_w(0x000E, VDP_DATA) // red backdrop
        .words(&[0x46FC, 0x2000]) // unmask interrupts
        .spin();
    rom.at(0x400).words(&[0x5279, 0x00FF, 0x0000, 0x4E73]); // addq.w #1,($FF0000).l; rte
    let (_, external) = same_results(&rom, 30);
    let count = u16::from_be_bytes([external.hw.ram[0], external.hw.ram[1]]);
    assert!(
        (29..=30).contains(&count),
        "VINT ran {count} times in 30 frames"
    );
}

#[test]
fn z80_runs_alongside() {
    // Z80: ld a,$42 / ld ($1000),a / jr $
    let z80_code = [0x3E, 0x42, 0x32, 0x00, 0x10, 0x18, 0xFE];
    let mut rom = Rom::new();
    rom.move_w(0x0100, 0xA1_1100).move_w(0x0100, 0xA1_1200);
    for (i, &byte) in z80_code.iter().enumerate() {
        rom.move_b(byte, 0xA0_0000 + i as u32);
    }
    rom.move_w(0x0000, 0xA1_1200)
        .move_w(0x0000, 0xA1_1100)
        .move_w(0x0100, 0xA1_1200)
        .spin();
    let (_, external) = same_results(&rom, 3);
    assert_eq!(external.hw.zram[0x1000], 0x42);
}

#[test]
fn dma_freezes_the_cpu_the_same_way() {
    // DMA 0x200 words from ROM $000000 to VRAM $0000, then count how far a
    // loop gets in the rest of the frame: the freeze must cost the same.
    let mut rom = Rom::new();
    rom.move_w(0x8154, VDP_CTRL) // display off, DMA enabled
        .move_w(0x8F02, VDP_CTRL) // auto-increment 2
        .move_w(0x9300, VDP_CTRL) // length low
        .move_w(0x9402, VDP_CTRL) // length high: $200 words
        .move_w(0x9500, VDP_CTRL) // source = 0
        .move_w(0x9600, VDP_CTRL)
        .move_w(0x9700, VDP_CTRL)
        .move_l(0x4000_0080, VDP_CTRL) // VRAM write with DMA
        .words(&[0x5279, 0x00FF, 0x0010]) // loop: addq.w #1,($FF0010).l
        .words(&[0x60F8]); // bra.s loop
    let (normal, _) = same_results(&rom, 2);
    let loops = u16::from_be_bytes([normal.hw.ram[0x10], normal.hw.ram[0x11]]);
    assert!(loops > 1000, "the loop ran after the DMA ({loops} times)");
}

#[test]
fn save_states_and_rewind_see_the_cpu() {
    let mut rom = Rom::new();
    rom.vector(30, 0x400)
        .move_w(0x8164, VDP_CTRL)
        .words(&[0x46FC, 0x2000])
        .spin();
    rom.at(0x400).words(&[0x5279, 0x00FF, 0x0000, 0x4E73]);
    let mut a = rom.console();
    a.set_external_cpu(Some(Box::new(Stepper)));
    for _ in 0..5 {
        a.run_frame();
    }
    let state = a.save_state();
    let mut b = rom.console();
    b.set_external_cpu(Some(Box::new(Stepper)));
    b.load_state(&state).unwrap();
    for _ in 0..5 {
        a.run_frame();
        b.run_frame();
    }
    assert_eq!(a.save_state(), b.save_state());
}

#[test]
fn registered_cpus_are_picked_by_rom_checksum() {
    let mut rom = Rom::new();
    rom.words(&[0x7000 | 0x2A]).spin(); // a ROM no other test uses
    let crc = gase_zip::crc32::crc32(rom.console().cartridge().rom());
    assert!(rom.console().external_cpu().is_none());
    external::register(crc, || Box::new(Stepper));
    assert!(rom.console().external_cpu().is_some());
    // Another ROM is not affected.
    let mut other = Rom::new();
    other.words(&[0x7000 | 0x2B]).spin();
    assert!(other.console().external_cpu().is_none());
}
