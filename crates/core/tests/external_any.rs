//! `external::register_any`: one CPU for every ROM without its own (a JIT).
//! In a file of its own: the registration is global to the process, and
//! the other tests expect consoles without an external CPU.

mod common;

use common::Rom;
use gase_core::system::external::{self, External, ExternalCpu};
use gase_m68k::M68k;

#[derive(Clone, Debug)]
struct Any;

impl ExternalCpu for Any {
    fn run_frame(&mut self, _cpu: &mut M68k, _console: &mut External<'_>) {}
    fn clone_box(&self) -> Box<dyn ExternalCpu> {
        Box::new(self.clone())
    }
    fn describe(&self) -> String {
        "any".into()
    }
}

#[derive(Clone, Debug)]
struct Own;

impl ExternalCpu for Own {
    fn run_frame(&mut self, _cpu: &mut M68k, _console: &mut External<'_>) {}
    fn clone_box(&self) -> Box<dyn ExternalCpu> {
        Box::new(self.clone())
    }
    fn describe(&self) -> String {
        "own".into()
    }
}

#[test]
fn a_cpu_for_any_rom_and_one_for_a_given_rom() {
    let mut first = Rom::new();
    first.words(&[0x7000 | 0x31]).spin();
    let mut second = Rom::new();
    second.words(&[0x7000 | 0x32]).spin();
    assert!(first.console().external_cpu().is_none());

    external::register_any(Some(|| Box::new(Any)));
    let describe = |rom: &Rom| rom.console().external_cpu().map(|c| c.describe());
    assert_eq!(describe(&first).as_deref(), Some("any"));
    assert_eq!(describe(&second).as_deref(), Some("any"));

    // A ROM's own CPU (a recompiled game) wins.
    let crc = gase_zip::crc32::crc32(second.console().cartridge().rom());
    external::register(crc, || Box::new(Own));
    assert_eq!(describe(&first).as_deref(), Some("any"));
    assert_eq!(describe(&second).as_deref(), Some("own"));

    external::register_any(None);
    assert!(first.console().external_cpu().is_none());
}
