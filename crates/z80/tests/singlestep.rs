//! Runs the SingleStepTests Z80 vectors (<https://github.com/SingleStepTests/z80>).
//!
//! Each JSON file holds 1000 randomised executions of one opcode: the initial
//! CPU and RAM state, the expected final state, the bus cycles and the port
//! accesses. We compare every register (including WZ, Q, R, the IFFs and the
//! EI/LD-A,I latches), RAM, port traffic and the number of T-states.
//!
//! The vectors are large and not committed. Fetch them with
//! `scripts/fetch-z80-tests.sh`, then run:
//!
//! ```text
//! cargo test -p gase-z80 --profile fast-test -- --ignored singlestep
//! ```
//!
//! Environment variables: `GASE_Z80_TESTS` overrides the vector directory;
//! `GASE_Z80_FILTER` only runs files whose name starts with the given prefix
//! (e.g. `"ed b"`).

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use gase_z80::{Bus, Z80};

// ---------------------------------------------------------------------------
// A minimal JSON parser (the workspace has no dependencies, not even for tests).
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum Json {
    Null,
    Bool(bool),
    Num(i64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    fn num(&self) -> i64 {
        match self {
            Json::Num(n) => *n,
            Json::Bool(b) => i64::from(*b),
            other => panic!("expected a number, got {other:?}"),
        }
    }
    fn arr(&self) -> &[Json] {
        match self {
            Json::Arr(a) => a,
            other => panic!("expected an array, got {other:?}"),
        }
    }
    fn str(&self) -> &str {
        match self {
            Json::Str(s) => s,
            other => panic!("expected a string, got {other:?}"),
        }
    }
}

struct Parser<'a> {
    s: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn parse(s: &[u8]) -> Json {
        let mut p = Parser { s, pos: 0 };
        let v = p.value();
        p.ws();
        assert_eq!(p.pos, p.s.len(), "trailing data in JSON");
        v
    }

    fn ws(&mut self) {
        while self.pos < self.s.len() && self.s[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn peek(&mut self) -> u8 {
        self.ws();
        self.s[self.pos]
    }

    fn expect(&mut self, c: u8) {
        assert_eq!(self.peek(), c, "JSON syntax error at byte {}", self.pos);
        self.pos += 1;
    }

    fn literal(&mut self, word: &str, v: Json) -> Json {
        assert!(
            self.s[self.pos..].starts_with(word.as_bytes()),
            "bad literal at {}",
            self.pos
        );
        self.pos += word.len();
        v
    }

    fn value(&mut self) -> Json {
        match self.peek() {
            b'{' => {
                self.pos += 1;
                let mut fields = Vec::new();
                if self.peek() == b'}' {
                    self.pos += 1;
                    return Json::Obj(fields);
                }
                loop {
                    let key = self.string();
                    self.expect(b':');
                    fields.push((key, self.value()));
                    match self.peek() {
                        b',' => self.pos += 1,
                        _ => {
                            self.expect(b'}');
                            return Json::Obj(fields);
                        }
                    }
                }
            }
            b'[' => {
                self.pos += 1;
                let mut items = Vec::new();
                if self.peek() == b']' {
                    self.pos += 1;
                    return Json::Arr(items);
                }
                loop {
                    items.push(self.value());
                    match self.peek() {
                        b',' => self.pos += 1,
                        _ => {
                            self.expect(b']');
                            return Json::Arr(items);
                        }
                    }
                }
            }
            b'"' => Json::Str(self.string()),
            b'n' => self.literal("null", Json::Null),
            b't' => self.literal("true", Json::Bool(true)),
            b'f' => self.literal("false", Json::Bool(false)),
            _ => self.number(),
        }
    }

    fn number(&mut self) -> Json {
        let start = self.pos;
        while self.pos < self.s.len() && matches!(self.s[self.pos], b'-' | b'0'..=b'9') {
            self.pos += 1;
        }
        let text = std::str::from_utf8(&self.s[start..self.pos]).unwrap();
        Json::Num(
            text.parse()
                .unwrap_or_else(|_| panic!("bad number {text:?} at {start}")),
        )
    }

    fn string(&mut self) -> String {
        self.expect(b'"');
        let mut out = String::new();
        loop {
            let c = self.s[self.pos];
            self.pos += 1;
            match c {
                b'"' => return out,
                b'\\' => {
                    let e = self.s[self.pos];
                    self.pos += 1;
                    out.push(match e {
                        b'n' => '\n',
                        b't' => '\t',
                        b'u' => {
                            let hex = std::str::from_utf8(&self.s[self.pos..self.pos + 4]).unwrap();
                            self.pos += 4;
                            char::from_u32(u32::from_str_radix(hex, 16).unwrap()).unwrap_or('?')
                        }
                        other => other as char,
                    });
                }
                _ => out.push(c as char),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The test bus: flat 64 KiB RAM plus scripted port reads.
// ---------------------------------------------------------------------------

struct TestBus {
    mem: Vec<u8>,
    /// Values the test says port reads return, in order.
    port_reads: Vec<u8>,
    next_read: usize,
    /// Port traffic observed: (port, value, is_write).
    ports: Vec<(u16, u8, bool)>,
}

impl Bus for TestBus {
    fn read(&mut self, addr: u16) -> u8 {
        self.mem[addr as usize]
    }
    fn write(&mut self, addr: u16, value: u8) {
        self.mem[addr as usize] = value;
    }
    fn port_in(&mut self, port: u16) -> u8 {
        let v = self.port_reads.get(self.next_read).copied().unwrap_or(0xFF);
        self.next_read += 1;
        self.ports.push((port, v, false));
        v
    }
    fn port_out(&mut self, port: u16, value: u8) {
        self.ports.push((port, value, true));
    }
}

/// Every register field of the vectors' state objects.
const FIELDS: [&str; 25] = [
    "pc", "sp", "a", "b", "c", "d", "e", "f", "h", "l", "i", "r", "ei", "wz", "ix", "iy", "af_",
    "bc_", "de_", "hl_", "im", "p", "q", "iff1", "iff2",
];

fn get_field(cpu: &Z80, name: &str) -> Option<i64> {
    Some(match name {
        "pc" => cpu.pc.into(),
        "sp" => cpu.sp.into(),
        "a" => cpu.a.into(),
        "b" => cpu.b.into(),
        "c" => cpu.c.into(),
        "d" => cpu.d.into(),
        "e" => cpu.e.into(),
        "f" => cpu.f.into(),
        "h" => cpu.h.into(),
        "l" => cpu.l.into(),
        "i" => cpu.i.into(),
        "r" => cpu.r.into(),
        "ei" => cpu.ei_delay.into(),
        "wz" => cpu.wz.into(),
        "ix" => cpu.ix.into(),
        "iy" => cpu.iy.into(),
        "af_" => cpu.af_alt.into(),
        "bc_" => cpu.bc_alt.into(),
        "de_" => cpu.de_alt.into(),
        "hl_" => cpu.hl_alt.into(),
        "im" => cpu.im.into(),
        "p" => cpu.ld_a_ir.into(),
        "q" => cpu.q.into(),
        "iff1" => cpu.iff1.into(),
        "iff2" => cpu.iff2.into(),
        _ => return None,
    })
}

fn set_field(cpu: &mut Z80, name: &str, v: i64) {
    match name {
        "pc" => cpu.pc = v as u16,
        "sp" => cpu.sp = v as u16,
        "a" => cpu.a = v as u8,
        "b" => cpu.b = v as u8,
        "c" => cpu.c = v as u8,
        "d" => cpu.d = v as u8,
        "e" => cpu.e = v as u8,
        "f" => cpu.f = v as u8,
        "h" => cpu.h = v as u8,
        "l" => cpu.l = v as u8,
        "i" => cpu.i = v as u8,
        "r" => cpu.r = v as u8,
        "ei" => cpu.ei_delay = v != 0,
        "wz" => cpu.wz = v as u16,
        "ix" => cpu.ix = v as u16,
        "iy" => cpu.iy = v as u16,
        "af_" => cpu.af_alt = v as u16,
        "bc_" => cpu.bc_alt = v as u16,
        "de_" => cpu.de_alt = v as u16,
        "hl_" => cpu.hl_alt = v as u16,
        "im" => cpu.im = v as u8,
        "p" => cpu.ld_a_ir = v != 0,
        "q" => cpu.q = v as u8,
        "iff1" => cpu.iff1 = v != 0,
        "iff2" => cpu.iff2 = v != 0,
        "ram" => {}
        other => panic!("unknown state field {other:?}"),
    }
}

/// Run one test case; `Err` describes every mismatch.
fn run_case(case: &Json, bus: &mut TestBus) -> Result<(), String> {
    let initial = case.get("initial").expect("initial");
    let expected = case.get("final").expect("final");

    let mut cpu = Z80::new();
    if let Json::Obj(fields) = initial {
        for (k, v) in fields {
            if k != "ram" {
                set_field(&mut cpu, k, v.num());
            }
        }
    }
    let ram_pairs = |state: &Json| -> Vec<(u16, u8)> {
        state
            .get("ram")
            .expect("ram")
            .arr()
            .iter()
            .map(|p| (p.arr()[0].num() as u16, p.arr()[1].num() as u8))
            .collect()
    };
    let initial_ram = ram_pairs(initial);
    let final_ram = ram_pairs(expected);
    for &(a, v) in &initial_ram {
        bus.mem[a as usize] = v;
    }
    bus.ports.clear();
    bus.port_reads.clear();
    bus.next_read = 0;
    let expected_ports: Vec<(u16, u8, bool)> = case.get("ports").map_or(Vec::new(), |p| {
        p.arr()
            .iter()
            .map(|e| {
                let e = e.arr();
                (e[0].num() as u16, e[1].num() as u8, e[2].str() == "w")
            })
            .collect()
    });
    bus.port_reads = expected_ports
        .iter()
        .filter(|p| !p.2)
        .map(|p| p.1)
        .collect();

    let cycles = cpu.step(bus);

    let mut errors = Vec::new();
    for name in FIELDS {
        if let (Some(want), Some(got)) = (expected.get(name), get_field(&cpu, name)) {
            if want.num() != got {
                errors.push(format!("{name}: got {got:#x}, want {:#x}", want.num()));
            }
        }
    }
    for &(a, v) in &final_ram {
        let got = bus.mem[a as usize];
        if got != v {
            errors.push(format!("ram[{a:#06x}]: got {got:#04x}, want {v:#04x}"));
        }
    }
    let want_cycles = case.get("cycles").expect("cycles").arr().len() as u32;
    if cycles != want_cycles {
        errors.push(format!("cycles: got {cycles}, want {want_cycles}"));
    }
    if bus.ports != expected_ports {
        errors.push(format!(
            "ports: got {:x?}, want {:x?}",
            bus.ports, expected_ports
        ));
    }

    // Clean RAM for the next case.
    for &(a, _) in initial_ram.iter().chain(&final_ram) {
        bus.mem[a as usize] = 0;
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

fn vector_dir() -> PathBuf {
    std::env::var_os("GASE_Z80_TESTS").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-vectors/z80"),
        PathBuf::from,
    )
}

#[test]
#[ignore = "needs downloaded vectors: scripts/fetch-z80-tests.sh"]
fn singlestep() {
    let dir = vector_dir();
    let filter = std::env::var("GASE_Z80_FILTER").unwrap_or_default();
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| {
            panic!(
                "cannot read {}: {e}; run scripts/fetch-z80-tests.sh",
                dir.display()
            )
        })
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.extension().is_some_and(|e| e == "json")
                && p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(&filter)
        })
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no vectors in {}", dir.display());

    let next = AtomicUsize::new(0);
    let total = AtomicUsize::new(0);
    let failed = AtomicUsize::new(0);
    let reports = Mutex::new(Vec::new());
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());

    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                let mut bus = TestBus {
                    mem: vec![0; 0x10000],
                    port_reads: Vec::new(),
                    next_read: 0,
                    ports: Vec::new(),
                };
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = files.get(i) else { break };
                    let data = std::fs::read(path).unwrap();
                    let cases = Parser::parse(&data);
                    let mut file_failed = 0;
                    let mut first = None;
                    for case in cases.arr() {
                        if let Err(e) = run_case(case, &mut bus) {
                            file_failed += 1;
                            first.get_or_insert_with(|| {
                                format!("{}: {e}", case.get("name").map_or("?", Json::str))
                            });
                        }
                    }
                    total.fetch_add(cases.arr().len(), Ordering::Relaxed);
                    failed.fetch_add(file_failed, Ordering::Relaxed);
                    if let Some(first) = first {
                        let name = path.file_name().unwrap().to_string_lossy().into_owned();
                        reports.lock().unwrap().push(format!(
                            "{name}: {file_failed}/{} failed, first: {first}",
                            cases.arr().len()
                        ));
                    }
                }
            });
        }
    });

    let mut reports = reports.into_inner().unwrap();
    reports.sort();
    for r in &reports {
        eprintln!("{r}");
    }
    let (total, failed) = (total.into_inner(), failed.into_inner());
    eprintln!(
        "{} files, {total} cases, {failed} failed ({} files with failures)",
        files.len(),
        reports.len()
    );
    assert_eq!(
        failed, 0,
        "{failed} of {total} SingleStepTests cases failed"
    );
}
