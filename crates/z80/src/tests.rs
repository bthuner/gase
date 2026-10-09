//! Fast unit tests that need no downloaded vectors. The exhaustive checks
//! live in `tests/singlestep.rs` and `tests/zex.rs`.

use crate::disasm::disassemble;
use crate::flags::{C, PV, Z};
use crate::{Bus, Z80};
use gase_savestate::{Reader, State, Writer};

struct Machine {
    mem: Vec<u8>,
    vector: u8,
    out: Vec<(u16, u8)>,
}

impl Machine {
    fn new(program: &[u8]) -> Self {
        let mut mem = vec![0; 0x10000];
        mem[..program.len()].copy_from_slice(program);
        Self {
            mem,
            vector: 0xFF,
            out: Vec::new(),
        }
    }
}

impl Bus for Machine {
    fn read(&mut self, addr: u16) -> u8 {
        self.mem[addr as usize]
    }
    fn write(&mut self, addr: u16, value: u8) {
        self.mem[addr as usize] = value;
    }
    fn port_in(&mut self, port: u16) -> u8 {
        port as u8 ^ 0x5A
    }
    fn port_out(&mut self, port: u16, value: u8) {
        self.out.push((port, value));
    }
    fn interrupt_data(&mut self) -> u8 {
        self.vector
    }
}

fn cpu_at(pc: u16, sp: u16) -> Z80 {
    let mut cpu = Z80::new();
    cpu.pc = pc;
    cpu.sp = sp;
    cpu
}

#[test]
fn power_on_and_reset_state() {
    let mut cpu = Z80::new();
    assert_eq!(
        (cpu.pc(), cpu.af(), cpu.sp, cpu.iff1, cpu.im),
        (0, 0xFFFF, 0xFFFF, false, 0)
    );
    cpu.pc = 0x1234;
    cpu.b = 7;
    cpu.iff1 = true;
    cpu.reset();
    assert_eq!((cpu.pc(), cpu.b, cpu.iff1), (0, 7, false));
}

#[test]
fn small_program_and_cycle_counts() {
    // LD B,3; loop: ADD A,B; DJNZ loop; OUT (0x10),A; HALT
    let mut m = Machine::new(&[0x06, 0x03, 0x80, 0x10, 0xFD, 0xD3, 0x10, 0x76]);
    let mut cpu = cpu_at(0, 0);
    cpu.a = 0;
    let mut t = 0;
    while !cpu.is_halted() {
        t += cpu.step(&mut m);
    }
    assert_eq!(cpu.a, 6);
    assert_eq!(m.out, vec![(0x0610, 6)]);
    // 7 + 3*4 (ADD) + 2*13 + 8 (DJNZ) + 11 + 4
    assert_eq!(t, 7 + 12 + 26 + 8 + 11 + 4);
    // Halted: PC stays after HALT, each step is a 4 T-state NOP.
    assert_eq!((cpu.pc(), cpu.step(&mut m), cpu.pc()), (8, 4, 8));
}

#[test]
fn r_counts_m1_cycles() {
    // NOP; LD IX,0 (DD 21); BIT 0,(IX+0) (DD CB 00 46); RLC B (CB 00)
    let mut m = Machine::new(&[0x00, 0xDD, 0x21, 0, 0, 0xDD, 0xCB, 0x00, 0x46, 0xCB, 0x00]);
    let mut cpu = cpu_at(0, 0);
    cpu.r = 0x80 | 0x7F;
    for _ in 0..4 {
        cpu.step(&mut m);
    }
    // 1 + 2 + 2 + 2 M1 cycles; the low 7 bits wrap, bit 7 is preserved.
    assert_eq!(cpu.r, 0x80 | 6);
}

#[test]
fn ei_delays_interrupts_by_one_instruction() {
    // IM 1; EI; NOP; NOP
    let mut m = Machine::new(&[0xED, 0x56, 0xFB, 0x00, 0x00]);
    let mut cpu = cpu_at(0, 0x8000);
    cpu.set_irq(true);
    cpu.step(&mut m); // IM 1
    cpu.step(&mut m); // EI
    assert_eq!(cpu.step(&mut m), 4, "the instruction after EI runs first");
    assert_eq!(cpu.pc(), 4);
    assert_eq!(cpu.step(&mut m), 13, "IM 1 acknowledge");
    assert_eq!((cpu.pc(), cpu.iff1, cpu.iff2), (0x38, false, false));
    assert_eq!(m.mem[0x7FFE..0x8000], [4, 0]);
}

#[test]
fn im2_vectors_through_table() {
    let mut m = Machine::new(&[0x00]);
    m.mem[0x4080] = 0x34;
    m.mem[0x4081] = 0x12;
    m.vector = 0x80;
    let mut cpu = cpu_at(0, 0x8000);
    (cpu.im, cpu.i, cpu.iff1) = (2, 0x40, true);
    cpu.set_irq(true);
    assert_eq!(cpu.step(&mut m), 19);
    assert_eq!((cpu.pc(), cpu.wz), (0x1234, 0x1234));
}

#[test]
fn im0_executes_bus_opcode() {
    let mut m = Machine::new(&[0x00]);
    m.vector = 0xD7; // RST 10h
    let mut cpu = cpu_at(0, 0x8000);
    cpu.iff1 = true;
    cpu.set_irq(true);
    assert_eq!(cpu.step(&mut m), 13);
    assert_eq!(cpu.pc(), 0x10);
}

#[test]
fn nmi_leaves_halt_and_retn_restores_iff1() {
    let mut m = Machine::new(&[0x76]);
    m.mem[0x66..0x68].copy_from_slice(&[0xED, 0x45]); // RETN
    let mut cpu = cpu_at(0, 0x8000);
    (cpu.iff1, cpu.iff2) = (true, true);
    cpu.step(&mut m);
    assert!(cpu.is_halted());
    cpu.nmi();
    assert_eq!(cpu.step(&mut m), 11);
    assert_eq!(
        (cpu.pc(), cpu.is_halted(), cpu.iff1, cpu.iff2),
        (0x66, false, false, true)
    );
    cpu.step(&mut m);
    assert_eq!((cpu.pc(), cpu.iff1), (1, true));
}

#[test]
fn ld_a_i_parity_bug() {
    // LD A,I reports IFF2 in P/V, but an interrupt accepted right after it
    // clears the flag on NMOS parts.
    let mut m = Machine::new(&[0xED, 0x57]);
    let mut cpu = cpu_at(0, 0x8000);
    (cpu.iff1, cpu.iff2, cpu.im) = (true, true, 1);
    cpu.step(&mut m);
    assert_ne!(cpu.f & PV, 0);
    cpu.set_irq(true);
    cpu.step(&mut m);
    assert_eq!(cpu.f & PV, 0);
}

#[test]
fn scf_ccf_use_q() {
    // XOR A sets the flags (Q = F), so SCF copies X/Y from A only.
    let mut m = Machine::new(&[0xAF, 0x37, 0x37]);
    let mut cpu = cpu_at(0, 0);
    cpu.step(&mut m);
    assert_eq!(cpu.q, cpu.f);
    cpu.step(&mut m);
    assert_eq!(cpu.f, Z | PV | C);
    // A second SCF: Q is now F as well, so still nothing from F leaks.
    cpu.f |= 0x28;
    cpu.q = 0;
    cpu.step(&mut m);
    assert_eq!(cpu.f & 0x28, 0x28, "with Q = 0 the old X/Y are kept");
}

#[test]
fn indexed_cb_copies_into_register() {
    // LD IX,0x100; DD CB 05 00 = LD B,RLC (IX+5)
    let mut m = Machine::new(&[0xDD, 0x21, 0x00, 0x01, 0xDD, 0xCB, 0x05, 0x00]);
    m.mem[0x105] = 0x81;
    let mut cpu = cpu_at(0, 0);
    cpu.step(&mut m);
    assert_eq!(cpu.step(&mut m), 23);
    assert_eq!((m.mem[0x105], cpu.b, cpu.f & C), (0x03, 0x03, C));
}

#[test]
fn repeated_prefixes_execute_as_one_step() {
    // DD FD 21 34 12: only the last prefix counts: LD IY,0x1234.
    let mut m = Machine::new(&[0xDD, 0xFD, 0x21, 0x34, 0x12]);
    let mut cpu = cpu_at(0, 0);
    assert_eq!(cpu.step(&mut m), 18);
    assert_eq!((cpu.iy, cpu.ix), (0x1234, 0xFFFF));
}

#[test]
fn ldir_repeats_with_21_t_states() {
    let mut m = Machine::new(&[0xED, 0xB0]);
    m.mem[0x100..0x103].copy_from_slice(&[1, 2, 3]);
    let mut cpu = cpu_at(0, 0);
    cpu.set_hl(0x100);
    cpu.set_de(0x200);
    cpu.set_bc(3);
    let cycles: Vec<u32> = (0..3).map(|_| cpu.step(&mut m)).collect();
    assert_eq!(cycles, [21, 21, 16]);
    assert_eq!(
        (&m.mem[0x200..0x203], cpu.bc(), cpu.pc()),
        (&[1, 2, 3][..], 0, 2)
    );
}

#[test]
fn save_state_round_trip() {
    let mut m = Machine::new(&[0x3E, 0x12, 0xED, 0x47, 0xFB]);
    let mut cpu = cpu_at(0, 0x4321);
    for _ in 0..3 {
        cpu.step(&mut m);
    }
    cpu.nmi();
    let mut w = Writer::new();
    cpu.save(&mut w);
    let bytes = w.into_bytes();
    let mut loaded = Z80::new();
    loaded.load(&mut Reader::new(&bytes)).unwrap();
    assert_eq!(loaded, cpu);
}

#[test]
fn disassembler() {
    let cases: &[(&[u8], &str)] = &[
        (&[0x00], "NOP"),
        (&[0x21, 0x34, 0x12], "LD HL,$1234"),
        (&[0xDD, 0x21, 0x34, 0x12], "LD IX,$1234"),
        (&[0xFD, 0x36, 0x05, 0x99], "LD (IY+$05),$99"),
        (&[0xDD, 0x66, 0xFE], "LD H,(IX-$02)"),
        (&[0xDD, 0x64], "LD IXH,IXH"),
        (&[0xDD, 0xCB, 0x02, 0x46], "BIT 0,(IX+$02)"),
        (&[0xFD, 0xCB, 0x02, 0x10], "RL (IY+$02),B"),
        (&[0xCB, 0x37], "SLL A"),
        (&[0xED, 0xB0], "LDIR"),
        (&[0xED, 0x71], "OUT (C),0"),
        (&[0xED, 0x00], "NOP* (ED 00)"),
        (&[0x18, 0xFE], "JR $0000"),
        (&[0xFF], "RST $38"),
        (&[0xDD, 0xDD, 0x00], "DB $DD"),
    ];
    for &(bytes, text) in cases {
        let (got, len) = disassemble(0, |a| bytes.get(a as usize).copied().unwrap_or(0));
        assert_eq!(got, text);
        let want_len = if bytes[1..].starts_with(&[0xDD]) {
            1
        } else {
            bytes.len() as u16
        };
        assert_eq!(len, want_len, "{text}");
    }
}
