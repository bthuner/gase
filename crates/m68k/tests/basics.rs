//! Fast tests of behaviour the single-instruction vectors cannot cover:
//! reset, interrupts, STOP, trace, exceptions across instructions, halting
//! and save states. Each test assembles a few words into RAM and steps.

use gase_m68k::{Bus, M68k};
use gase_savestate::{Reader, State, Writer};

/// 16 MiB of flat RAM that records interrupt acknowledges and resets.
struct Ram {
    mem: Vec<u8>,
    acknowledged: Vec<u8>,
    resets: u32,
    vector: Option<u8>,
}

impl Ram {
    fn new() -> Self {
        Self {
            mem: vec![0; 1 << 24],
            acknowledged: Vec::new(),
            resets: 0,
            vector: None,
        }
    }
    fn poke_words(&mut self, addr: u32, words: &[u16]) {
        for (i, w) in words.iter().enumerate() {
            self.write_word(addr + 2 * i as u32, *w);
        }
    }
    fn poke_long(&mut self, addr: u32, value: u32) {
        self.poke_words(addr, &[(value >> 16) as u16, value as u16]);
    }
    fn peek_long(&mut self, addr: u32) -> u32 {
        u32::from(self.read_word(addr)) << 16 | u32::from(self.read_word(addr + 2))
    }
}

impl Bus for Ram {
    fn read_byte(&mut self, addr: u32) -> u8 {
        self.mem[addr as usize]
    }
    fn read_word(&mut self, addr: u32) -> u16 {
        u16::from_be_bytes([self.mem[addr as usize], self.mem[addr as usize + 1]])
    }
    fn write_byte(&mut self, addr: u32, value: u8) {
        self.mem[addr as usize] = value;
    }
    fn write_word(&mut self, addr: u32, value: u16) {
        self.mem[addr as usize..addr as usize + 2].copy_from_slice(&value.to_be_bytes());
    }
    fn interrupt_acknowledge(&mut self, level: u8) -> Option<u8> {
        self.acknowledged.push(level);
        self.vector
    }
    fn reset_devices(&mut self) {
        self.resets += 1;
    }
}

const STACK: u32 = 0x8000;
const CODE: u32 = 0x1000;

/// A machine with the reset vectors pointing at `code` and every exception
/// vector pointing at a distinct `RTE`-free handler address (0x2000 + 4n),
/// filled with NOPs.
fn machine(code: &[u16]) -> (M68k, Ram) {
    let mut ram = Ram::new();
    ram.poke_long(0, STACK);
    ram.poke_long(4, CODE);
    for vector in 2..64 {
        ram.poke_long(vector * 4, 0x2000 + vector * 0x10);
    }
    for addr in (0x2000..0x3000).step_by(2) {
        ram.write_word(addr, 0x4E71);
    }
    ram.poke_words(CODE, code);
    let mut cpu = M68k::new();
    cpu.reset(&mut ram);
    (cpu, ram)
}

#[test]
fn reset_loads_stack_and_pc() {
    let (cpu, _) = machine(&[0x4E71]);
    assert_eq!(cpu.pc(), CODE);
    assert_eq!(cpu.ssp(), STACK);
    assert_eq!(cpu.sr(), 0x2700);
    assert_eq!(cpu.prefetch_queue(), [0x4E71, 0]);
}

#[test]
fn basic_instructions_and_timing() {
    // moveq #-2,d0 ; addq.l #3,d0 ; nop ; move.l d0,(a0) ; lsl.w #8,d1
    let (mut cpu, mut ram) = machine(&[0x70FE, 0x5680, 0x4E71, 0x2080, 0xE149]);
    cpu.a[0] = 0x4000;
    cpu.d[1] = 0x1234_00FF;
    assert_eq!(cpu.step(&mut ram), 4);
    assert_eq!(cpu.d[0], 0xFFFF_FFFE);
    assert_eq!(cpu.sr() & 0x1F, 0x08, "N set");
    assert_eq!(cpu.step(&mut ram), 8);
    assert_eq!(cpu.d[0], 1);
    assert_eq!(cpu.sr() & 0x1F, 0x11, "X and C from the carry");
    assert_eq!(cpu.step(&mut ram), 4);
    assert_eq!(cpu.step(&mut ram), 12);
    assert_eq!(ram.peek_long(0x4000), 1);
    assert_eq!(cpu.step(&mut ram), 6 + 2 * 8);
    assert_eq!(cpu.d[1], 0x1234_FF00);
}

#[test]
fn autovectored_interrupt() {
    // move #$2000,sr (unmask) ; nop ; nop
    let (mut cpu, mut ram) = machine(&[0x46FC, 0x2000, 0x4E71, 0x4E71]);
    cpu.step(&mut ram);
    cpu.set_interrupt_level(4);
    let cycles = cpu.step(&mut ram);
    assert_eq!(cycles, 44);
    assert_eq!(ram.acknowledged, [4]);
    assert_eq!(
        cpu.pc(),
        0x2000 + 28 * 0x10,
        "level 4 autovector is vector 28"
    );
    assert_eq!(cpu.sr(), 0x2400, "mask raised to the interrupt level");
    let sp = cpu.ssp();
    assert_eq!(sp, STACK - 6);
    assert_eq!(ram.read_word(sp), 0x2000, "old SR stacked");
    assert_eq!(
        ram.peek_long(sp + 2),
        CODE + 4,
        "return to the next instruction"
    );
    // Same level again is masked now.
    assert_eq!(cpu.step(&mut ram), 4);
    assert_eq!(ram.acknowledged.len(), 1);
}

#[test]
fn vectored_interrupt_and_masking() {
    let (mut cpu, mut ram) = machine(&[0x4E71, 0x4E71]);
    ram.vector = Some(0x40);
    ram.poke_long(0x40 * 4, 0x2800);
    cpu.set_interrupt_level(6);
    // SR mask is 7 after reset: no interrupt.
    cpu.step(&mut ram);
    assert!(ram.acknowledged.is_empty());
    // Level 7 cannot be masked, and uses the vector the device supplies.
    cpu.set_interrupt_level(7);
    cpu.step(&mut ram);
    assert_eq!(ram.acknowledged, [7]);
    assert_eq!(cpu.pc(), 0x2800);
    // Level 7 is edge triggered: holding it does not interrupt again.
    cpu.step(&mut ram);
    assert_eq!(ram.acknowledged.len(), 1);
}

#[test]
fn stop_waits_for_interrupt() {
    // stop #$2000 ; nop
    let (mut cpu, mut ram) = machine(&[0x4E72, 0x2000, 0x4E71]);
    cpu.step(&mut ram);
    assert!(cpu.is_stopped());
    assert_eq!(cpu.pc(), CODE + 4);
    assert_eq!(cpu.step(&mut ram), 4, "idling");
    assert!(cpu.is_stopped());
    cpu.set_interrupt_level(2);
    cpu.step(&mut ram);
    assert!(!cpu.is_stopped());
    assert_eq!(
        ram.peek_long(cpu.ssp() + 2),
        CODE + 4,
        "returns after the STOP"
    );
}

#[test]
fn trace_runs_after_each_instruction() {
    // move #$A700,sr (trace on) ; nop ; nop
    let (mut cpu, mut ram) = machine(&[0x46FC, 0xA700, 0x4E71, 0x4E71]);
    cpu.step(&mut ram);
    assert_eq!(cpu.step(&mut ram), 4, "the nop");
    assert_eq!(cpu.step(&mut ram), 34, "then the trace exception");
    assert_eq!(cpu.pc(), 0x2000 + 9 * 0x10);
    assert_eq!(cpu.sr() & 0x8000, 0, "trace off in the handler");
    assert_eq!(ram.peek_long(cpu.ssp() + 2), CODE + 6);
}

#[test]
fn traps_and_rejected_instructions() {
    // trap #5 at CODE
    let (mut cpu, mut ram) = machine(&[0x4E45]);
    assert_eq!(cpu.step(&mut ram), 34);
    assert_eq!(cpu.pc(), 0x2000 + 37 * 0x10);
    assert_eq!(
        ram.peek_long(cpu.ssp() + 2),
        CODE + 2,
        "TRAP stacks the next instruction"
    );

    // Illegal and line A stack the instruction itself.
    for (opcode, vector) in [(0x4AFC, 4), (0xA123, 10), (0xF000, 11)] {
        let (mut cpu, mut ram) = machine(&[opcode]);
        assert_eq!(cpu.step(&mut ram), 34);
        assert_eq!(cpu.pc(), 0x2000 + vector * 0x10);
        assert_eq!(ram.peek_long(cpu.ssp() + 2), CODE);
    }

    // Privileged instruction in user mode: move #0,sr ; then move #$2700,sr
    let (mut cpu, mut ram) = machine(&[0x46FC, 0x0000, 0x46FC, 0x2700]);
    cpu.step(&mut ram);
    assert!(!cpu.is_supervisor());
    cpu.step(&mut ram);
    assert_eq!(cpu.pc(), 0x2000 + 8 * 0x10);
    assert!(cpu.is_supervisor());
    assert_eq!(ram.peek_long(cpu.ssp() + 2), CODE + 4);
}

#[test]
fn reset_instruction_and_division_by_zero() {
    // reset ; divu d1,d0 (d1 = 0)
    let (mut cpu, mut ram) = machine(&[0x4E70, 0x80C1]);
    assert_eq!(cpu.step(&mut ram), 132);
    assert_eq!(ram.resets, 1);
    assert_eq!(cpu.step(&mut ram), 38);
    assert_eq!(cpu.pc(), 0x2000 + 5 * 0x10);
}

#[test]
fn address_error_and_double_fault() {
    // move.w (a0),d0 with odd a0
    let (mut cpu, mut ram) = machine(&[0x3010]);
    cpu.a[0] = 0x4001;
    cpu.step(&mut ram);
    assert_eq!(cpu.pc(), 0x2000 + 3 * 0x10);
    assert_eq!(cpu.ssp(), STACK - 14);
    assert_eq!(
        ram.peek_long(cpu.ssp() + 2),
        0x4001,
        "faulting address stacked"
    );

    // Same with an odd stack pointer: the frame cannot be written.
    let (mut cpu, mut ram) = machine(&[0x3010]);
    cpu.a[0] = 0x4001;
    cpu.a[7] = 0x7001;
    cpu.step(&mut ram);
    assert!(cpu.is_halted());
    assert_eq!(cpu.step(&mut ram), 4);
    cpu.reset(&mut ram);
    assert!(!cpu.is_halted());
}

#[test]
fn lenient_mode_ignores_bit_zero_of_data_accesses() {
    // move.l d0,(a0) ; move.w (a0),d1 with A0 odd.
    let code = [0x2080, 0x3210];

    // Hardware behaviour: the long write faults (vector 3).
    let (mut cpu, mut ram) = machine(&code);
    cpu.a[0] = 0x4001;
    cpu.d[0] = 0x1122_3344;
    cpu.step(&mut ram); // the fault is taken within the same step
    assert_eq!(cpu.pc(), 0x2030, "address error handler");
    assert_eq!(ram.peek_long(0x4000), 0);

    // Lenient: the accesses go to the even address below instead.
    let (mut cpu, mut ram) = machine(&code);
    cpu.set_address_errors(false);
    assert!(!cpu.address_errors());
    cpu.a[0] = 0x4001;
    cpu.d[0] = 0x1122_3344;
    assert_eq!(cpu.step(&mut ram), 12);
    assert_eq!(ram.peek_long(0x4000), 0x1122_3344);
    cpu.step(&mut ram);
    assert_eq!(cpu.d[1] & 0xFFFF, 0x1122);
    assert_eq!(cpu.pc(), CODE + 4);

    // Odd jump targets still fault in lenient mode: jmp (a0).
    let (mut cpu, mut ram) = machine(&[0x4ED0]);
    cpu.set_address_errors(false);
    cpu.a[0] = 0x4001;
    cpu.step(&mut ram);
    assert_eq!(cpu.pc(), 0x2030);
}

#[test]
fn bcd_and_division_results() {
    // abcd d1,d0 ; divu d3,d2 ; divs d5,d4
    let (mut cpu, mut ram) = machine(&[0xC101, 0x84C3, 0x89C5]);
    cpu.d[0] = 0x19;
    cpu.d[1] = 0x28;
    cpu.d[2] = 100_000;
    cpu.d[3] = 7;
    cpu.d[4] = (-100i32) as u32;
    cpu.d[5] = 7;
    cpu.step(&mut ram);
    assert_eq!(cpu.d[0], 0x47);
    cpu.step(&mut ram);
    assert_eq!(cpu.d[2], ((100_000 % 7) << 16) | (100_000 / 7));
    cpu.step(&mut ram);
    assert_eq!(cpu.d[4] as u16 as i16, -14);
    assert_eq!(
        (cpu.d[4] >> 16) as u16 as i16,
        -2,
        "remainder takes the dividend's sign"
    );
}

#[test]
fn save_state_round_trip() {
    // moveq #1,d0 ; moveq #2,d1 ; moveq #3,d2
    let (mut cpu, mut ram) = machine(&[0x7001, 0x7202, 0x7403]);
    cpu.step(&mut ram);
    cpu.set_interrupt_level(3);
    let mut w = Writer::new();
    cpu.save(&mut w);
    let bytes = w.into_bytes();

    let mut restored = M68k::new();
    restored.load(&mut Reader::new(&bytes)).unwrap();
    assert_eq!(format!("{restored:?}"), format!("{cpu:?}"));
    for _ in 0..2 {
        assert_eq!(cpu.step(&mut ram), restored.step(&mut ram));
    }
    assert_eq!(cpu.d, restored.d);
    assert_eq!(cpu.pc(), restored.pc());
}

#[test]
fn user_and_supervisor_stacks_are_separate() {
    // move.l #$5000,a0 ; move a0,usp ; move #0,sr ; move.l sp,d0
    let (mut cpu, mut ram) = machine(&[0x207C, 0x0000, 0x5000, 0x4E60, 0x46FC, 0x0000, 0x200F]);
    for _ in 0..4 {
        cpu.step(&mut ram);
    }
    assert_eq!(cpu.d[0], 0x5000);
    assert_eq!(cpu.usp(), 0x5000);
    assert_eq!(cpu.ssp(), STACK);
}

/// Rough throughput check: `cargo test -p gase-m68k --release -- --ignored speed --nocapture`.
#[test]
#[ignore = "benchmark"]
fn speed() {
    // A loop mixing moves, ALU ops, memory accesses, shifts and a branch:
    //   loop: move.l (a0)+,d1 ; add.l d1,d2 ; lsl.w #3,d3 ; move.w d2,-(a1)
    //         addq.w #1,d4 ; cmp.w #$8000,d4 ; dbf d0,loop ; bra start
    let code = [
        0x2218, 0xD481, 0xE74B, 0x3302, 0x5244, 0xB87C, 0x8000, 0x51C8, 0xFFF0, 0x6000, 0xFFEC,
    ];
    let (mut cpu, mut ram) = machine(&[0x41F8, 0x4000, 0x43F8, 0x6000, 0x303C, 0x0FFF]);
    ram.poke_words(CODE + 12, &code);
    let start = std::time::Instant::now();
    let (mut cycles, mut instructions) = (0u64, 0u64);
    while cycles < 500_000_000 {
        cycles += u64::from(cpu.step(&mut ram));
        instructions += 1;
        if instructions % 10_000 == 0 {
            cpu.a[0] = 0x4000;
            cpu.a[1] = 0x6000;
        }
    }
    let secs = start.elapsed().as_secs_f64();
    println!(
        "{:.1} M instructions/s, {:.0} MHz emulated ({:.0}x a 7.67 MHz Mega Drive)",
        instructions as f64 / secs / 1e6,
        cycles as f64 / secs / 1e6,
        cycles as f64 / secs / 7.67e6
    );
}
