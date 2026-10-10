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
//! * `GASE_M68K_FILTER`, `GASE_M68K_VERBOSE`: see `common/mod.rs`.
//!
//! Some upstream vectors are known to disagree with real hardware (checked
//! against the MAME-derived suite in `mame.rs`); [`known_bad`] identifies
//! them and they are reported separately.

mod common;

use std::path::Path;

use common::{CpuState, Test};

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
    Bool,
    Null,
}

impl Json {
    fn get(&self, key: &str) -> &Json {
        match self {
            Json::Object(fields) => {
                &fields
                    .iter()
                    .find(|(k, _)| k == key)
                    .unwrap_or_else(|| panic!("missing key {key}"))
                    .1
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
        assert_eq!(
            self.bytes[self.pos], byte,
            "JSON syntax error at byte {}",
            self.pos
        );
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
                Json::Bool
            }
            b'f' => {
                self.pos += 5;
                Json::Bool
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
// Conversion to the common format
// ---------------------------------------------------------------------------

fn state(json: &Json) -> CpuState {
    let mut regs = [0; 15];
    for (i, reg) in regs.iter_mut().enumerate() {
        let name = if i < 8 {
            format!("d{i}")
        } else {
            format!("a{}", i - 8)
        };
        *reg = json.get(&name).u32();
    }
    let prefetch = json.get("prefetch").array();
    CpuState {
        regs,
        usp: json.get("usp").u32(),
        ssp: json.get("ssp").u32(),
        sr: json.get("sr").u32() as u16,
        pc: json.get("pc").u32(),
        prefetch: [prefetch[0].u32() as u16, prefetch[1].u32() as u16],
        ram: json
            .get("ram")
            .array()
            .iter()
            .map(|pair| {
                let pair = pair.array();
                (pair[0].u32(), pair[1].u32() as u8)
            })
            .collect(),
    }
}

fn load(path: &Path) -> Vec<Test> {
    let bytes = std::fs::read(path).expect("read test file");
    parse_json(&bytes)
        .array()
        .iter()
        .map(|test| Test {
            name: test.get("name").str().to_owned(),
            initial: state(test.get("initial")),
            expected: state(test.get("final")),
            cycles: test.get("length").u32(),
            transactions: test
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
                .collect(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Known-bad vectors
// ---------------------------------------------------------------------------

/// Upstream vectors whose expected results contradict the real 68000.
///
/// The arbiter is the SingleStepTests/m68000 suite (`mame.rs`), generated
/// from MAME's emulation of the 68000's actual microcode, which this core
/// passes in full; it also agrees with Motorola's documentation wherever the
/// two sets differ. Each entry is narrow: only tests matching the described
/// situation are excused, and only if they fail.
const KNOWN_BAD_NOTES: &[(&str, &str)] = &[
    (
        "address errors (all files)",
        "these vectors stack a PC that is 0-4 bytes off (sometimes not even the documented \
         'instruction + 2..10'), set the I/N bit for jump-target faults, update An/flags/IR \
         at different points and charge 50 instead of 58 cycles; the microcode-derived \
         vectors define all of these",
    ),
    (
        "ASR",
        "count > operand size on a negative value: C and X are the sign bit, not 0",
    ),
    (
        "ADD.l/SUB.l",
        "ADDQ.L/SUBQ.L #,An take 8 cycles (as documented), not 6",
    ),
    (
        "CHK",
        "N always reflects the sign of Dn, and the trap timing depends on bound - Dn",
    ),
    (
        "DIVU/DIVS",
        "overflow sets N and clears Z; DIVS signed overflow takes the full-length path",
    ),
    (
        "ASL.b",
        "two corrupt vectors where a byte shift rewrites the upper 24 bits of Dn",
    ),
    (
        "LINK",
        "LINK A7 pushes the value A7 had before the instruction, not the decremented one",
    ),
    (
        "DIVU",
        "the only divide-by-zero vector stacks the DIVU's own address; group 2 traps stack \
         the address of the next instruction",
    ),
];

/// The opcode under test.
fn opcode(test: &Test) -> u16 {
    test.initial.prefetch[0]
}

fn known_bad(file: &str, test: &Test) -> Option<&'static str> {
    // Any test whose expected bus activity reads the address error vector.
    if test.transactions.iter().any(|t| t.starts_with("r.w 12 = ")) {
        return Some("address error");
    }
    let op = opcode(test);
    let regs = &test.initial.regs;
    match file {
        "ASR.b" | "ASR.w" | "ASR.l" if op & 0x20 != 0 && (op >> 6) & 3 != 3 => {
            let bits = 8 << ((op >> 6) & 3);
            let count = regs[usize::from((op >> 9) & 7)] % 64;
            let value = regs[usize::from(op & 7)];
            (count > bits && (value >> (bits - 1)) & 1 != 0).then_some("ASR carry")
        }
        "ADD.l" | "SUB.l" if op & 0xF0F8 == 0x5088 => Some("ADDQ/SUBQ.L An timing"),
        "CHK" => {
            // Excused: N not matching the sign of Dn (these vectors leave N
            // alone when not trapping), and traps on a negative Dn (whose
            // timing these vectors get wrong half the time).
            let dn = regs[usize::from((op >> 9) & 7)] as u16 as i16;
            let n_disagrees = (test.expected.sr & 8 != 0) != (dn < 0);
            (n_disagrees || dn < 0).then_some("CHK flags/timing")
        }
        "LINK" if op == 0x4E57 => Some("LINK A7"),
        "DIVU" | "DIVS" => {
            let dn = usize::from((op >> 9) & 7);
            let overflowed = test.expected.sr & 2 != 0 && test.expected.regs[dn] == regs[dn];
            let by_zero = test.transactions.iter().any(|t| t.starts_with("r.w 20 = "));
            (overflowed || by_zero).then_some("division overflow / by zero")
        }
        "ASL.b" => {
            let dn = usize::from(op & 7);
            ((test.expected.regs[dn] ^ regs[dn]) & !0xFF != 0).then_some("corrupt vector")
        }
        _ => None,
    }
}

#[test]
#[ignore = "needs downloaded vectors: scripts/fetch-m68k-tests.sh"]
fn tom_harte_68000() {
    let dir = common::vector_dir("GASE_M68K_TESTS", "m68000");
    common::run_suite(
        "Tom Harte 68000",
        &dir,
        ".json",
        load,
        known_bad,
        KNOWN_BAD_NOTES,
    );
}
