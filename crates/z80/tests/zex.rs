//! ZEXDOC and ZEXALL, Frank Cringle's Z80 instruction exercisers.
//!
//! They are CP/M programs: each test runs an instruction over many operand
//! combinations, folds the resulting registers, memory and flags into a CRC,
//! and compares it with the CRC recorded on a real Z80. ZEXDOC masks out the
//! undocumented X/Y flags, ZEXALL checks them too.
//!
//! We emulate just enough CP/M to run them: the program is loaded at 0x0100,
//! `CALL 5` is the BDOS entry point (we implement function 2, print a
//! character, and 9, print a `$`-terminated string) and a jump to 0x0000
//! (warm boot) ends the program.
//!
//! Fetch the binaries with `scripts/fetch-z80-tests.sh`, then:
//!
//! ```text
//! cargo test -p gase-z80 --profile fast-test -- --ignored zex --nocapture
//! ```

use std::path::{Path, PathBuf};

use gase_z80::{Bus, Z80};

struct Ram(Vec<u8>);

impl Bus for Ram {
    fn read(&mut self, addr: u16) -> u8 {
        self.0[addr as usize]
    }
    fn write(&mut self, addr: u16, value: u8) {
        self.0[addr as usize] = value;
    }
}

fn program_path(name: &str) -> PathBuf {
    std::env::var_os("GASE_Z80_TESTS")
        .map_or_else(
            || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-vectors/z80"),
            PathBuf::from,
        )
        .join(name)
}

/// Run a CP/M program to completion; returns its console output.
fn run_cpm(name: &str) -> String {
    let path = program_path(name);
    let program = std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read {}: {e}; run scripts/fetch-z80-tests.sh",
            path.display()
        )
    });

    let mut ram = Ram(vec![0; 0x10000]);
    ram.0[0x100..0x100 + program.len()].copy_from_slice(&program);
    // 0x0005: the BDOS entry. We intercept calls there before they execute
    // and let the RET we place there return to the caller. The word at
    // 0x0006 is the top of usable memory, which the exercisers use as their
    // initial stack pointer.
    ram.0[0x0005] = 0xC9; // RET
    ram.0[0x0006..0x0008].copy_from_slice(&0xFE00u16.to_le_bytes());

    let mut cpu = Z80::new();
    cpu.pc = 0x0100;
    cpu.sp = 0xFE00;

    let mut output = String::new();
    let mut cycles: u64 = 0;
    loop {
        match cpu.pc {
            0x0000 => break,
            0x0005 => {
                let text = match cpu.c {
                    2 => (cpu.e as char).to_string(),
                    9 => {
                        let mut s = String::new();
                        let mut addr = cpu.de();
                        while ram.0[addr as usize] != b'$' {
                            s.push(ram.0[addr as usize] as char);
                            addr = addr.wrapping_add(1);
                        }
                        s
                    }
                    _ => String::new(),
                };
                eprint!("{text}");
                output.push_str(&text);
            }
            _ => {}
        }
        cycles += u64::from(cpu.step(&mut ram));
    }
    eprintln!("\n{name}: {cycles} T-states");
    output
}

fn check(name: &str) {
    let output = run_cpm(name);
    assert!(output.contains("Tests complete"), "{name} did not finish");
    let errors = output.matches("ERROR").count();
    assert_eq!(errors, 0, "{name}: {errors} tests failed:\n{output}");
}

#[test]
#[ignore = "needs downloaded zexdoc.com: scripts/fetch-z80-tests.sh"]
fn zexdoc() {
    check("zexdoc.com");
}

#[test]
#[ignore = "needs downloaded zexall.com: scripts/fetch-z80-tests.sh"]
fn zexall() {
    check("zexall.com");
}
