//! The SingleStepTests/m68000 vectors: Tom Harte-style single-instruction
//! tests generated from MAME's microcode-level 68000 emulation.
//!
//! These follow the real microcode where the original Tom Harte vectors
//! (`tomharte.rs`) disagree with it, so they serve as the tie-breaker. Fetch
//! them with `scripts/fetch-m68k-tests.sh`, then:
//!
//! ```text
//! cargo test -p gase-m68k --profile fast-test -- --ignored
//! ```
//!
//! `GASE_M68K_MAME_TESTS` overrides the directory (default
//! `target/test-vectors/m68000-mame`); see `common/mod.rs` for the others.
//!
//! The files use a compact little-endian binary format (described by
//! `decode.py` upstream): a header, then per test a name, the initial and
//! final states and the bus transactions, each block tagged with a magic
//! number. Two conventions differ from Tom Harte's: the PC recorded is the
//! *next fetch address*, 4 bytes past the opcode in the prefetch queue; and
//! RAM is listed as 16-bit words.

mod common;

use std::path::Path;

use common::{CpuState, Test};

/// A cursor over the binary file.
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn u8(&mut self) -> u8 {
        self.pos += 1;
        self.bytes[self.pos - 1]
    }
    fn u16(&mut self) -> u16 {
        self.pos += 2;
        u16::from_le_bytes([self.bytes[self.pos - 2], self.bytes[self.pos - 1]])
    }
    fn u32(&mut self) -> u32 {
        self.pos += 4;
        u32::from_le_bytes(self.bytes[self.pos - 4..self.pos].try_into().unwrap())
    }
    /// A block header: its byte length (unused) and magic number.
    fn block(&mut self, magic: u32) {
        let _length = self.u32();
        assert_eq!(self.u32(), magic, "corrupt test file at byte {}", self.pos);
    }

    fn state(&mut self) -> CpuState {
        self.block(0x0123_4567);
        let mut regs = [0; 15];
        for reg in &mut regs {
            *reg = self.u32();
        }
        let usp = self.u32();
        let ssp = self.u32();
        let sr = self.u32() as u16;
        let next_fetch = self.u32();
        let prefetch = [self.u32() as u16, self.u32() as u16];
        let words = self.u32();
        let mut ram = Vec::with_capacity(words as usize * 2);
        for _ in 0..words {
            let addr = self.u32();
            let [high, low] = self.u16().to_be_bytes();
            ram.push((addr, high));
            ram.push((addr | 1, low));
        }
        CpuState {
            regs,
            usp,
            ssp,
            sr,
            pc: next_fetch.wrapping_sub(4),
            prefetch,
            ram,
        }
    }

    fn test(&mut self) -> Test {
        self.block(0xABC1_2367);
        self.block(0x89AB_CDEF);
        let len = self.u32() as usize;
        let name = String::from_utf8_lossy(&self.bytes[self.pos..self.pos + len]).into_owned();
        self.pos += len;
        let initial = self.state();
        let expected = self.state();
        self.block(0x4567_89AB);
        let cycles = self.u32();
        let count = self.u32();
        let mut transactions = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let kind = self.u8();
            let length = self.u32();
            if kind == 0 {
                transactions.push(format!("n{length}"));
                continue;
            }
            let (_fc, addr, data, uds, lds) =
                (self.u32(), self.u32(), self.u32(), self.u32(), self.u32());
            let kind = ["", "w", "r", "t", "r-fault", "w-fault"][kind as usize];
            let size = if uds + lds == 2 { ".w" } else { ".b" };
            transactions.push(format!("{kind}{size} {addr} = {data}"));
        }
        Test {
            name,
            initial,
            expected,
            cycles,
            transactions,
        }
    }
}

fn load(path: &Path) -> Vec<Test> {
    let bytes = std::fs::read(path).expect("read test file");
    let mut r = Reader {
        bytes: &bytes,
        pos: 0,
    };
    assert_eq!(r.u32(), 0x1A3F_5D71, "not a SingleStepTests file");
    let count = r.u32();
    (0..count).map(|_| r.test()).collect()
}

/// Vectors in this suite that are known to be wrong.
const KNOWN_BAD_NOTES: &[(&str, &str)] = &[];

fn known_bad(file: &str, test: &Test) -> Option<&'static str> {
    let _ = (file, test);
    None
}

#[test]
#[ignore = "needs downloaded vectors: scripts/fetch-m68k-tests.sh"]
fn mame_68000() {
    let dir = common::vector_dir("GASE_M68K_MAME_TESTS", "m68000-mame");
    common::run_suite(
        "MAME 68000",
        &dir,
        ".json.bin",
        load,
        known_bad,
        KNOWN_BAD_NOTES,
    );
}
