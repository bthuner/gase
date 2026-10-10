//! Whole-system tests of VDP memory timing: the write FIFO, access slots and
//! DMA, seen the way a program sees them — by polling status flags and
//! reading the HV counter. Expected values are derived from the slot counts
//! (18 per line in active H40 display, 205 in blanking) with generous
//! margins, since the exact numbers depend on instruction timings.

mod common;

use common::{Rom, VDP_CTRL, VDP_DATA};
use gase_core::Genesis;

const VDP_HV: u32 = 0xC0_0008;
// Address registers used by the programs below.
const CTRL: u16 = 0; // a0
const DATA: u16 = 1; // a1
const HV: u16 = 2; // a2
const RESULT: u32 = 0xFF_0000;
const DONE: u32 = 0xFF_0010;

/// Point a0-a2 at the VDP ports and select H40 with the given register 1.
fn setup(rom: &mut Rom, reg1: u8) {
    rom.lea(VDP_CTRL, CTRL)
        .lea(VDP_DATA, DATA)
        .lea(VDP_HV, HV)
        .write_w(0x8C81, CTRL) // H40
        .write_w(0x8F02, CTRL) // auto-increment 2
        .write_w(0x8100 | u16::from(reg1), CTRL);
}

/// Wait for vertical blanking to start and then end: afterwards the program
/// runs at the top of the active display.
fn wait_for_active_display(rom: &mut Rom) {
    let top = rom.pc;
    rom.read_w(CTRL, 1).btst(3, 1).branch(0x67, top); // beq: not in vblank yet
    let top = rom.pc;
    rom.read_w(CTRL, 1).btst(3, 1).branch(0x66, top); // bne: still in vblank
}

/// `d0 = 0; do { d0++; d1 = status } while (d1 bit `bit` == set_means_wait)`,
/// then store d0 at `RESULT + offset`. One iteration is 32 68000 cycles.
fn count_polls(rom: &mut Rom, bit: u16, wait_while_set: bool, offset: u32) {
    rom.clear(0);
    let top = rom.pc;
    rom.inc(0).read_w(CTRL, 1).btst(bit, 1);
    rom.branch(if wait_while_set { 0x66 } else { 0x67 }, top);
    rom.store_w(0, RESULT + offset);
}

fn finish(rom: &mut Rom) -> Genesis {
    rom.move_w(1, DONE).spin();
    let mut console = rom.console();
    for _ in 0..4 {
        console.run_frame();
    }
    assert_eq!(word(&console, DONE), 1, "program did not finish");
    console
}

fn word(console: &Genesis, addr: u32) -> u16 {
    let i = (addr & 0xFFFF) as usize;
    u16::from_be_bytes([console.hw.ram[i], console.hw.ram[i + 1]])
}

/// Lines elapsed between two HV counter reads stored at `RESULT + 2/4`
/// (within active display the V counter equals the line number).
fn lines_between(console: &Genesis) -> u16 {
    (word(console, RESULT + 4) >> 8) - (word(console, RESULT + 2) >> 8)
}

/// Write four VRAM words, then count status polls until the FIFO is empty.
fn fifo_drain_polls(display_on: bool) -> u16 {
    let mut rom = Rom::new();
    setup(&mut rom, if display_on { 0x44 } else { 0x04 });
    if display_on {
        wait_for_active_display(&mut rom);
    }
    rom.write_l(0x4000_0000, CTRL); // VRAM write, address 0
    for _ in 0..4 {
        rom.write_w(0x1234, DATA);
    }
    count_polls(&mut rom, 9, false, 0); // until FIFO empty
    let console = finish(&mut rom);
    assert_eq!(&console.hw.vdp.vram[..8], &[0x12, 0x34].repeat(4)[..]);
    word(&console, RESULT)
}

#[test]
fn fifo_empty_flag_follows_the_access_slots() {
    // Four VRAM words need eight slots: ~1100-1500 master clocks in active
    // display (~5-7 polls), ~130 with the display off (one poll).
    let active = fifo_drain_polls(true);
    let off = fifo_drain_polls(false);
    assert!((3..=10).contains(&active), "active display: {active} polls");
    assert!((1..=2).contains(&off), "display off: {off} polls");
}

/// Write 24 VRAM words back to back and measure the time with the HV counter.
fn burst_lines(display_on: bool) -> u16 {
    let mut rom = Rom::new();
    setup(&mut rom, if display_on { 0x44 } else { 0x04 });
    if display_on {
        wait_for_active_display(&mut rom);
    }
    rom.read_w(HV, 2).write_l(0x4000_0000, CTRL);
    for _ in 0..24 {
        rom.write_w(0x5555, DATA);
    }
    rom.read_w(HV, 3)
        .store_w(2, RESULT + 2)
        .store_w(3, RESULT + 4);
    let console = finish(&mut rom);
    assert!(console.hw.vdp.vram[..48].iter().all(|&b| b == 0x55));
    lines_between(&console)
}

#[test]
fn full_fifo_stalls_the_68000() {
    // The 68000 can only go on once 20 of the 24 words have been written:
    // 40 slots, over two lines in active display, a fraction of one with the
    // display off. Without the FIFO the burst would take ~300 cycles.
    let active = burst_lines(true);
    let off = burst_lines(false);
    assert!((2..=3).contains(&active), "active display: {active} lines");
    assert!(off <= 1, "display off: {off} lines");
}

/// DMA 180 words from ROM to VRAM, timing it with the HV counter.
fn dma_lines(display_on: bool) -> u16 {
    let mut rom = Rom::new();
    for i in 0..360 {
        rom.bytes[0x800 + i] = i as u8;
    }
    setup(&mut rom, if display_on { 0x54 } else { 0x14 });
    if display_on {
        wait_for_active_display(&mut rom);
    }
    rom.write_w(0x93B4, CTRL) // 180 words
        .write_w(0x9400, CTRL)
        .write_w(0x9500, CTRL) // source 0x000800 (word address 0x400)
        .write_w(0x9604, CTRL)
        .write_w(0x9700, CTRL)
        .read_w(HV, 2)
        .write_l(0x4000_0080, CTRL) // VRAM 0, DMA
        .read_w(HV, 3)
        .store_w(2, RESULT + 2)
        .store_w(3, RESULT + 4);
    let console = finish(&mut rom);
    assert_eq!(&console.hw.vdp.vram[..360], &rom.bytes[0x800..0x800 + 360]);
    // The source registers point past the transfer, the length is zero.
    let regs = &console.hw.vdp.regs;
    assert_eq!((regs[19], regs[20]), (0, 0));
    assert_eq!(u16::from(regs[21]) | u16::from(regs[22]) << 8, 0x400 + 180);
    lines_between(&console)
}

#[test]
fn dma_from_68000_freezes_it_for_the_transfer() {
    // 176 words after the first four fill the FIFO, two slots each: ~20
    // lines in active display, under 2 with the display off. This is why
    // games upload graphics in vertical blanking.
    let active = dma_lines(true);
    let off = dma_lines(false);
    assert!(
        (18..=22).contains(&active),
        "active display: {active} lines"
    );
    assert!((1..=2).contains(&off), "display off: {off} lines");
}

#[test]
fn dma_busy_flag_lasts_for_fill_and_copy() {
    let mut rom = Rom::new();
    setup(&mut rom, 0x14); // display off: 205 slots per line
    rom.write_w(0x8F01, CTRL) // auto-increment 1
        // Fill 0x1000 bytes at 0x2000 with 0xAB.
        .write_w(0x9300, CTRL)
        .write_w(0x9410, CTRL)
        .write_w(0x9780, CTRL)
        .write_l(0x6000_0080, CTRL)
        .write_w(0xAB00, DATA);
    count_polls(&mut rom, 1, true, 0); // while DMA busy
    // Copy them to 0x4000.
    rom.write_w(0x9300, CTRL)
        .write_w(0x9410, CTRL)
        .write_w(0x9500, CTRL)
        .write_w(0x9620, CTRL)
        .write_w(0x97C0, CTRL)
        .write_l(0x0000_0081, CTRL);
    count_polls(&mut rom, 1, true, 2);
    let console = finish(&mut rom);

    // A fill moves a byte per slot: 4096 slots, 20 lines, ~9800 cycles or
    // ~300 polls. The 68000 keeps running meanwhile (it is the one polling).
    let fill = word(&console, RESULT);
    assert!((250..=360).contains(&fill), "fill: {fill} polls");
    // A copy needs two slots per byte.
    let copy = word(&console, RESULT + 2);
    let ratio = f64::from(copy) / f64::from(fill);
    assert!((1.8..=2.2).contains(&ratio), "copy {copy} vs fill {fill}");

    let vram = &console.hw.vdp.vram;
    assert!(vram[0x2002..0x3000].iter().all(|&b| b == 0xAB));
    assert_eq!(vram[0x4002..0x5000], vram[0x2002..0x3000]);
}
