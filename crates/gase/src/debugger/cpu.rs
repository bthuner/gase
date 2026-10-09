//! Text views of the two CPUs: registers and disassembly.

use gase_core::{Debugger, Genesis};

/// The 68000's registers, decoded.
#[must_use]
pub fn m68k_lines(genesis: &Genesis) -> Vec<String> {
    let cpu = &genesis.m68k;
    let sr = cpu.sr();
    let bit = |n: u16| u8::from(sr & (1 << n) != 0);
    let state = if cpu.is_halted() {
        "HALTED (double bus fault)"
    } else if cpu.is_stopped() {
        "stopped (STOP: waiting for an interrupt)"
    } else {
        "running"
    };
    let row = |name: char, regs: &[u32], first: usize| {
        regs.iter()
            .enumerate()
            .map(|(i, v)| format!("{name}{} {v:08X}", first + i))
            .collect::<Vec<_>>()
            .join("  ")
    };
    vec![
        format!("PC {:06X}  {state}", cpu.pc() & 0xFF_FFFF),
        format!(
            "SR {sr:04X}  T={} S={} I={}  X={} N={} Z={} V={} C={}",
            bit(15),
            bit(13),
            (sr >> 8) & 7,
            bit(4),
            bit(3),
            bit(2),
            bit(1),
            bit(0)
        ),
        row('D', &cpu.d[..4], 0),
        row('D', &cpu.d[4..], 4),
        row('A', &cpu.a[..4], 0),
        row('A', &cpu.a[4..], 4),
        format!(
            "USP {:08X}  SSP {:08X}  ({} mode)",
            cpu.usp(),
            cpu.ssp(),
            if cpu.is_supervisor() {
                "supervisor"
            } else {
                "user"
            }
        ),
    ]
}

/// One disassembled instruction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub addr: u32,
    pub text: String,
}

/// `count` 68000 instructions starting at `start`.
#[must_use]
pub fn disassembly(genesis: &Genesis, start: u32, count: usize) -> Vec<Line> {
    let mut addr = start & 0xFF_FFFE;
    (0..count)
        .map(|_| {
            let (text, len) = genesis.disassemble(addr);
            let line = Line { addr, text };
            addr = (addr + len.max(2)) & 0xFF_FFFF;
            line
        })
        .collect()
}

/// The Z80's registers and state.
#[must_use]
pub fn z80_lines(genesis: &Genesis) -> Vec<String> {
    let z = &genesis.z80;
    let hw = &genesis.hw;
    let state = if hw.z80_reset {
        "held in reset"
    } else if hw.z80_busreq {
        "paused: 68000 holds its bus"
    } else if z.is_halted() {
        "HALT"
    } else {
        "running"
    };
    let f = z.f;
    let flag = |bit: u8, c: char| if f & (1 << bit) != 0 { c } else { '-' };
    vec![
        format!("PC {:04X}  SP {:04X}  {state}", z.pc, z.sp),
        format!(
            "AF {:04X}  BC {:04X}  DE {:04X}  HL {:04X}  IX {:04X}  IY {:04X}",
            z.af(),
            z.bc(),
            z.de(),
            z.hl(),
            z.ix,
            z.iy
        ),
        format!(
            "F {}{}{}{}{}{}  IM {}  IFF1 {}  bank ${:06X}",
            flag(7, 'S'),
            flag(6, 'Z'),
            flag(4, 'H'),
            flag(2, 'P'),
            flag(1, 'N'),
            flag(0, 'C'),
            z.im,
            u8::from(z.iff1),
            u32::from(hw.z80_bank) << 15
        ),
    ]
}

/// `count` Z80 instructions starting at `start`.
#[must_use]
pub fn z80_disassembly(genesis: &Genesis, start: u16, count: usize) -> Vec<(u16, String)> {
    let mut addr = start;
    (0..count)
        .map(|_| {
            let (text, len) = genesis.disassemble_z80(addr);
            let line = (addr, text);
            addr = addr.wrapping_add(len.max(1));
            line
        })
        .collect()
}

/// A plain-text report of both CPUs, for printing in a terminal.
#[must_use]
pub fn report(genesis: &Genesis, debugger: &Debugger) -> String {
    let mut out = String::from("68000:\n");
    for line in m68k_lines(genesis) {
        out += &format!("  {line}\n");
    }
    let pc = genesis.m68k.pc() & 0xFF_FFFF;
    for line in disassembly(genesis, pc, 8) {
        let mark = if line.addr == pc { '>' } else { ' ' };
        let bp = if debugger.breakpoints().contains(&line.addr) {
            '*'
        } else {
            ' '
        };
        out += &format!("  {mark}{bp} {:06X}  {}\n", line.addr, line.text);
    }
    out += "Z80:\n";
    for line in z80_lines(genesis) {
        out += &format!("  {line}\n");
    }
    out
}
