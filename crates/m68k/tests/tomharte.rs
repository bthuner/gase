//! Tom Harte's 68000 single-instruction test vectors.
//!
//! Each JSON file holds ~8000 randomised tests for one instruction family:
//! an initial CPU + memory state, the expected final state, and the number of
//! cycles taken. Fetch them first with `scripts/fetch-m68k-tests.sh`, then:
//!
//! ```text
//! cargo test -p gase-m68k --profile fast-test -- --ignored
//! ```
//!
//! Environment variables:
//! * `GASE_M68K_TESTS`: vector directory (default `target/test-vectors/m68000`);
//! * `GASE_M68K_FILTER`: only run files whose name contains this;
//! * `GASE_M68K_VERBOSE=n`: print details of the first n failures per file.
//!
//! Some upstream vectors are known to disagree with real hardware; they are
//! listed in [`KNOWN_BAD`] and reported separately.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::PathBuf;

use gase_m68k::{Bus, M68k};

// ---------------------------------------------------------------------------
// A minimal JSON parser (the test vectors only use objects, arrays, strings
// and integers).
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum Json {
    Number(i64),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
    Bool(bool),
    Null,
}

impl Json {
    fn get(&self, key: &str) -> &Json {
        match self {
            Json::Object(fields) => {
                &fields.iter().find(|(k, _)| k == key).unwrap_or_else(|| panic!("missing key {key}")).1
            }
            _ => panic!("not an object"),
        }
    }
    fn num(&self) -> i64 {
        match self {
            Json::Number(n) => *n,
            _ => panic!("not a number: {self:?}"),
        }
    }
    fn u32(&self) -> u32 {
        self.num() as u32
    }
    fn array(&self) -> &[Json] {
        match self {
            Json::Array(items) => items,
            _ => panic!("not an array"),
        }
    }
    fn str(&self) -> &str {
        match self {
            Json::String(s) => s,
            _ => panic!("not a string"),
        }
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn expect(&mut self, byte: u8) {
        self.skip_ws();
        assert_eq!(self.bytes[self.pos], byte, "JSON syntax error at byte {}", self.pos);
        self.pos += 1;
    }

    fn value(&mut self) -> Json {
        self.skip_ws();
        match self.bytes[self.pos] {
            b'{' => {
                self.pos += 1;
                let mut fields = Vec::new();
                self.skip_ws();
                if self.bytes[self.pos] == b'}' {
                    self.pos += 1;
                    return Json::Object(fields);
                }
                loop {
                    self.skip_ws();
                    let key = self.string();
                    self.expect(b':');
                    fields.push((key, self.value()));
                    self.skip_ws();
                    self.pos += 1;
                    if self.bytes[self.pos - 1] == b'}' {
                        return Json::Object(fields);
                    }
                }
            }
            b'[' => {
                self.pos += 1;
                let mut items = Vec::new();
                self.skip_ws();
                if self.bytes[self.pos] == b']' {
                    self.pos += 1;
                    return Json::Array(items);
                }
                loop {
                    items.push(self.value());
                    self.skip_ws();
                    self.pos += 1;
                    if self.bytes[self.pos - 1] == b']' {
                        return Json::Array(items);
                    }
                }
            }
            b'"' => Json::String(self.string()),
            b't' => {
                self.pos += 4;
                Json::Bool(true)
            }
            b'f' => {
                self.pos += 5;
                Json::Bool(false)
            }
            b'n' => {
                self.pos += 4;
                Json::Null
            }
            _ => {
                let negative = self.bytes[self.pos] == b'-';
                if negative {
                    self.pos += 1;
                }
                let mut n: i64 = 0;
                while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
                    n = n * 10 + i64::from(self.bytes[self.pos] - b'0');
                    self.pos += 1;
                }
                Json::Number(if negative { -n } else { n })
            }
        }
    }

    fn string(&mut self) -> String {
        assert_eq!(self.bytes[self.pos], b'"');
        self.pos += 1;
        let start = self.pos;
        while self.bytes[self.pos] != b'"' {
            // The vectors contain no escapes beyond what we can skip.
            if self.bytes[self.pos] == b'\\' {
                self.pos += 1;
            }
            self.pos += 1;
        }
        let s = String::from_utf8_lossy(&self.bytes[start..self.pos]).into_owned();
        self.pos += 1;
        s
    }
}

fn parse_json(bytes: &[u8]) -> Json {
    Parser { bytes, pos: 0 }.value()
}

// ---------------------------------------------------------------------------
// Test bus: 16 MiB of RAM, reset between tests by remembering what changed.
// ---------------------------------------------------------------------------

struct TestBus {
    ram: Vec<u8>,
    touched: Vec<u32>,
    log: Vec<String>,
    logging: bool,
}

impl TestBus {
    fn new() -> Self {
        Self { ram: vec![0; 1 << 24], touched: Vec::new(), log: Vec::new(), logging: false }
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
        let v = u16::from_be_bytes([self.ram[addr as usize], self.ram[(addr as usize + 1) & 0xFF_FFFF]]);
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

// ---------------------------------------------------------------------------
// Running the tests
// ---------------------------------------------------------------------------

/// Vectors that disagree with real hardware or with the documented
/// behaviour this core implements: (file, reason). Failures in these files
/// are reported but do not fail the run.
const KNOWN_BAD: &[(&str, &str)] = &[];

const REGS: [&str; 15] = ["d0", "d1", "d2", "d3", "d4", "d5", "d6", "d7", "a0", "a1", "a2", "a3", "a4", "a5", "a6"];

fn setup(cpu: &mut M68k, bus: &mut TestBus, state: &Json) {
    cpu.set_sr(state.get("sr").u32() as u16);
    cpu.set_usp(state.get("usp").u32());
    cpu.set_ssp(state.get("ssp").u32());
    for (i, name) in REGS.iter().enumerate() {
        let value = state.get(name).u32();
        if i < 8 {
            cpu.d[i] = value;
        } else {
            cpu.a[i - 8] = value;
        }
    }
    let prefetch = state.get("prefetch").array();
    cpu.set_pc_and_prefetch(state.get("pc").u32(), [prefetch[0].u32() as u16, prefetch[1].u32() as u16]);
    for pair in state.get("ram").array() {
        let pair = pair.array();
        bus.poke(pair[0].u32() & 0xFF_FFFF, pair[1].u32() as u8);
    }
}

/// Compare the CPU and memory to the expected state; returns a description
/// of every mismatch.
fn compare(cpu: &M68k, bus: &TestBus, state: &Json) -> String {
    let mut errors = String::new();
    for (i, name) in REGS.iter().enumerate() {
        let actual = if i < 8 { cpu.d[i] } else { cpu.a[i - 8] };
        let expected = state.get(name).u32();
        if actual != expected {
            let _ = write!(errors, " {name}={actual:08X} (want {expected:08X})");
        }
    }
    for (name, actual) in [("usp", cpu.usp()), ("ssp", cpu.ssp()), ("pc", cpu.pc()), ("sr", u32::from(cpu.sr()))] {
        let expected = state.get(name).u32();
        if actual != expected {
            let _ = write!(errors, " {name}={actual:08X} (want {expected:08X})");
        }
    }
    for pair in state.get("ram").array() {
        let pair = pair.array();
        let addr = pair[0].u32() & 0xFF_FFFF;
        let expected = pair[1].u32() as u8;
        let actual = bus.ram[addr as usize];
        if actual != expected {
            let _ = write!(errors, " [{addr:06X}]={actual:02X} (want {expected:02X})");
        }
    }
    errors
}

#[derive(Default)]
struct FileResult {
    total: usize,
    state_failures: usize,
    cycle_failures: usize,
}

fn run_file(path: &PathBuf, verbose: usize, cpu: &mut M68k, bus: &mut TestBus) -> FileResult {
    let bytes = std::fs::read(path).expect("read test file");
    let tests = parse_json(&bytes);
    let mut result = FileResult::default();
    let mut shown = 0;
    for test in tests.array() {
        result.total += 1;
        bus.clear();
        *cpu = M68k::new();
        setup(cpu, bus, test.get("initial"));
        bus.logging = shown < verbose;
        let cycles = cpu.step(bus);
        let errors = compare(cpu, bus, test.get("final"));
        let expected_cycles = test.get("length").u32();
        let cycles_ok = cycles == expected_cycles;
        if !errors.is_empty() {
            result.state_failures += 1;
        } else if !cycles_ok {
            result.cycle_failures += 1;
        }
        if (!errors.is_empty() || !cycles_ok) && shown < verbose {
            shown += 1;
            println!("  FAIL {}: cycles {cycles} (want {expected_cycles}){errors}", test.get("name").str());
            println!("    ours:   {}", bus.log.join(", "));
            let theirs: Vec<String> = test
                .get("transactions")
                .array()
                .iter()
                .map(|t| {
                    let t = t.array();
                    match t[0].str() {
                        "n" => format!("n{}", t[1].num()),
                        kind => format!("{kind}{} {} = {}", t[4].str(), t[3].num(), t[5].num()),
                    }
                })
                .collect();
            println!("    theirs: {}", theirs.join(", "));
        }
    }
    result
}

#[test]
#[ignore = "needs downloaded vectors: scripts/fetch-m68k-tests.sh"]
fn tom_harte_68000() {
    let dir = std::env::var_os("GASE_M68K_TESTS").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-vectors/m68000"),
        PathBuf::from,
    );
    let filter = std::env::var("GASE_M68K_FILTER").unwrap_or_default();
    let verbose: usize = std::env::var("GASE_M68K_VERBOSE").ok().and_then(|v| v.parse().ok()).unwrap_or(0);

    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|_| panic!("no test vectors in {}; run scripts/fetch-m68k-tests.sh", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter(|p| p.file_name().unwrap().to_string_lossy().contains(&filter))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no *.json files in {}", dir.display());

    let mut cpu = M68k::new();
    let mut bus = TestBus::new();
    let mut summary: HashMap<&str, usize> = HashMap::new();
    let mut unexpected = 0;
    for path in &files {
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let r = run_file(path, verbose, &mut cpu, &mut bus);
        let failed = r.state_failures + r.cycle_failures;
        let known = KNOWN_BAD.iter().find(|(file, _)| *file == name);
        println!(
            "{name:<14} {:>5}/{:<5} passed  ({} state, {} cycle-only failures){}",
            r.total - failed,
            r.total,
            r.state_failures,
            r.cycle_failures,
            known.map_or(String::new(), |(_, why)| format!("  [known bad upstream: {why}]")),
        );
        *summary.entry("total").or_default() += r.total;
        *summary.entry("state").or_default() += r.state_failures;
        *summary.entry("cycles").or_default() += r.cycle_failures;
        if known.is_none() {
            unexpected += failed;
        }
    }
    println!(
        "TOTAL: {} tests, {} state failures, {} cycle-only failures",
        summary["total"], summary["state"], summary["cycles"]
    );
    assert_eq!(unexpected, 0, "{unexpected} unexpected failures");
}
