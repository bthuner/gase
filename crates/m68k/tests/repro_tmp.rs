use gase_m68k::{Bus, M68k};
struct R(Vec<u8>);
impl Bus for R {
    fn read_byte(&mut self, a: u32) -> u8 { self.0[a as usize] }
    fn read_word(&mut self, a: u32) -> u16 { u16::from_be_bytes([self.0[a as usize], self.0[a as usize + 1]]) }
    fn write_byte(&mut self, a: u32, v: u8) { self.0[a as usize] = v }
    fn write_word(&mut self, a: u32, v: u16) { self.0[a as usize..a as usize + 2].copy_from_slice(&v.to_be_bytes()) }
}
#[test]
fn repro() {
    let mut bus = R(vec![0; 1 << 24]);
    let code: [u16; 8] = [0x2C40, 0x203C, 0x0000, 0x0100, 0x2046, 0x3080, 0x4E71, 0x4E71];
    for (i, w) in code.iter().enumerate() { bus.write_word(0x1000 + 2 * i as u32, *w); }
    let mut cpu = M68k::new();
    cpu.set_pc(&mut bus, 0x1000);
    cpu.d[0] = 0x00A1_1100;
    for _ in 0..3 { cpu.step(&mut bus); println!("{:08X?} {:08X?}", cpu.a, cpu.d); }
}
