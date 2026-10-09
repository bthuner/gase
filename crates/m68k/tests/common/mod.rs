//! Shared runner for the single-instruction test suites.
//!
//! Each test gives an initial CPU + memory state, the expected final state
//! and the number of cycles the instruction takes. Front-ends (`tomharte.rs`,
//! `mame.rs`) parse their file format into [`Test`]s; this module runs them
//! against a flat 16 MiB RAM and reports per-file results.
//!
//! Environment variables understood by both suites:
//! * `GASE_M68K_FILTER`: only run files whose name contains this;
//! * `GASE_M68K_VERBOSE=n`: print the first n failures per file, with the
//!   bus activity of our run next to the expected transactions.

#![allow(dead_code)]

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use gase_m68k::{Bus, M68k};

/// CPU and memory state, normalised across suites.
#[derive(Debug, Default, Clone)]
pub struct CpuState {
    /// D0–D7 then A0–A6.
    pub regs: [u32; 15],
    pub usp: u32,
    pub ssp: u32,
    pub sr: u16,
    /// Architectural PC: address of the opcode in `prefetch[0]`.
    pub pc: u32,
    pub prefetch: [u16; 2],
    pub ram: Vec<(u32, u8)>,
}

#[derive(Debug, Default)]
pub struct Test {
    pub name: String,
    pub initial: CpuState,
    pub expected: CpuState,
    pub cycles: u32,
    /// Expected bus activity, for diagnostics only.
    pub transactions: Vec<String>,
}

/// 16 MiB of RAM, cleared between tests by remembering what was written.
pub struct TestBus {
    ram: Vec<u8>,
    touched: Vec<u32>,
    log: Vec<String>,
    logging: bool,
}

impl TestBus {
    pub fn new() -> Self {
        Self {
            ram: vec![0; 1 << 24],
            touched: Vec::new(),
            log: Vec::new(),
            logging: false,
        }
    }
    fn poke(&mut self, addr: u32, value: u8) {
        self.ram[addr as usize] = value;
        self.touched.push(addr);
    }
    fn clear(&mut self) {
        for &addr in &self.touched {
            self.ram[addr as usize] = 0;
        }
        self.touched.clear();
        self.log.clear();
    }
}

impl Bus for TestBus {
    fn read_byte(&mut self, addr: u32) -> u8 {
        let v = self.ram[addr as usize];
        if self.logging {
            self.log.push(format!("r.b {addr} = {v}"));
        }
        v
    }
    fn read_word(&mut self, addr: u32) -> u16 {
        let v = u16::from_be_bytes([
            self.ram[addr as usize],
            self.ram[(addr as usize + 1) & 0xFF_FFFF],
        ]);
        if self.logging {
            self.log.push(format!("r.w {addr} = {v}"));
        }
        v
    }
    fn write_byte(&mut self, addr: u32, value: u8) {
        if self.logging {
            self.log.push(format!("w.b {addr} = {value}"));
        }
        self.poke(addr, value);
    }
    fn write_word(&mut self, addr: u32, value: u16) {
        if self.logging {
            self.log.push(format!("w.w {addr} = {value}"));
        }
        let [hi, lo] = value.to_be_bytes();
        self.poke(addr, hi);
        self.poke((addr + 1) & 0xFF_FFFF, lo);
    }
}

fn setup(cpu: &mut M68k, bus: &mut TestBus, state: &CpuState) {
    *cpu = M68k::new();
    cpu.set_sr(state.sr);
    cpu.set_usp(state.usp);
    cpu.set_ssp(state.ssp);
    cpu.d.copy_from_slice(&state.regs[..8]);
    cpu.a[..7].copy_from_slice(&state.regs[8..]);
    cpu.set_pc_and_prefetch(state.pc, state.prefetch);
    for &(addr, value) in &state.ram {
        bus.poke(addr & 0xFF_FFFF, value);
    }
}

const REG_NAMES: [&str; 15] = [
    "d0", "d1", "d2", "d3", "d4", "d5", "d6", "d7", "a0", "a1", "a2", "a3", "a4", "a5", "a6",
];

/// Describe every difference between the CPU/memory and `state`.
fn compare(cpu: &M68k, bus: &TestBus, state: &CpuState) -> String {
    let mut errors = String::new();
    for (i, name) in REG_NAMES.iter().enumerate() {
        let actual = if i < 8 { cpu.d[i] } else { cpu.a[i - 8] };
        if actual != state.regs[i] {
            let _ = write!(errors, " {name}={actual:08X} (want {:08X})", state.regs[i]);
        }
    }
    let specials = [
        ("usp", cpu.usp(), state.usp),
        ("ssp", cpu.ssp(), state.ssp),
        ("pc", cpu.pc(), state.pc),
        ("sr", u32::from(cpu.sr()), u32::from(state.sr)),
    ];
    for (name, actual, expected) in specials {
        if actual != expected {
            let _ = write!(errors, " {name}={actual:08X} (want {expected:08X})");
        }
    }
    for &(addr, expected) in &state.ram {
        let actual = bus.ram[(addr & 0xFF_FFFF) as usize];
        if actual != expected {
            let _ = write!(errors, " [{addr:06X}]={actual:02X} (want {expected:02X})");
        }
    }
    errors
}

/// Outcome of one file.
#[derive(Default)]
pub struct FileResult {
    pub total: usize,
    pub state_failures: usize,
    pub cycle_failures: usize,
    /// Failures in tests the suite's `known_bad` hook excused.
    pub excused: usize,
}

/// Why a test's expected result is not trusted, if it is not.
pub type KnownBad = fn(file: &str, test: &Test) -> Option<&'static str>;

pub fn run_tests(
    file: &str,
    tests: &[Test],
    known_bad: KnownBad,
    cpu: &mut M68k,
    bus: &mut TestBus,
) -> FileResult {
    let verbose: usize = std::env::var("GASE_M68K_VERBOSE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut result = FileResult::default();
    let mut shown = 0;
    for test in tests {
        result.total += 1;
        bus.clear();
        setup(cpu, bus, &test.initial);
        bus.logging = shown < verbose;
        let cycles = cpu.step(bus);
        let errors = compare(cpu, bus, &test.expected);
        let cycles_ok = cycles == test.cycles;
        if errors.is_empty() && cycles_ok {
            continue;
        }
        if known_bad(file, test).is_some() {
            result.excused += 1;
            continue;
        }
        if errors.is_empty() {
            result.cycle_failures += 1;
        } else {
            result.state_failures += 1;
        }
        if shown < verbose {
            shown += 1;
            println!(
                "  FAIL {}: cycles {cycles} (want {}){errors}",
                test.name, test.cycles
            );
            println!("    ours:   {}", bus.log.join(", "));
            println!("    theirs: {}", test.transactions.join(", "));
        }
    }
    result
}

/// Run every file in `dir` with the given extension through `load`.
pub fn run_suite(
    suite: &str,
    dir: &Path,
    extension: &str,
    load: fn(&Path) -> Vec<Test>,
    known_bad: KnownBad,
    known_bad_notes: &[(&str, &str)],
) {
    let filter = std::env::var("GASE_M68K_FILTER").unwrap_or_default();
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|_| {
            panic!(
                "no test vectors in {}; run scripts/fetch-m68k-tests.sh",
                dir.display()
            )
        })
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.to_string_lossy().ends_with(extension))
        .filter(|p| p.file_name().unwrap().to_string_lossy().contains(&filter))
        .collect();
    files.sort();
    assert!(
        !files.is_empty(),
        "no *{extension} files in {}",
        dir.display()
    );

    let mut cpu = M68k::new();
    let mut bus = TestBus::new();
    let (mut total, mut failed, mut excused) = (0, 0, 0);
    println!("{suite}: {} files", files.len());
    for path in &files {
        let file_name = path.file_name().unwrap().to_string_lossy();
        let name = file_name.trim_end_matches(extension);
        let tests = load(path);
        let r = run_tests(name, &tests, known_bad, &mut cpu, &mut bus);
        let bad = r.state_failures + r.cycle_failures;
        let mut line = format!(
            "{name:<12} {:>5}/{:<5} passed",
            r.total - bad - r.excused,
            r.total,
        );
        if bad > 0 {
            let _ = write!(
                line,
                "  FAILED: {} state, {} cycles only",
                r.state_failures, r.cycle_failures
            );
        }
        if r.excused > 0 {
            let _ = write!(line, "  ({} known-bad vectors excused)", r.excused);
        }
        println!("{line}");
        total += r.total;
        failed += bad;
        excused += r.excused;
    }
    println!(
        "{suite} TOTAL: {total} tests, {} passed, {failed} failed, {excused} known-bad excused",
        total - failed - excused
    );
    if excused > 0 {
        println!("known-bad vectors (expected results that disagree with the real 68000):");
        for (what, why) in known_bad_notes {
            println!("  {what}: {why}");
        }
    }
    assert_eq!(failed, 0, "{failed} unexpected failures");
}

/// Directory from an environment variable, or a default under `target/`.
pub fn vector_dir(var: &str, default: &str) -> PathBuf {
    std::env::var_os(var).map_or_else(
        || {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/test-vectors")
                .join(default)
        },
        PathBuf::from,
    )
}
