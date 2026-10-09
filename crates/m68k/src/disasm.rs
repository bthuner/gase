//! Disassembler (Motorola syntax).

/// Disassemble the instruction at `addr`.
pub fn disassemble(addr: u32, mut read_word: impl FnMut(u32) -> u16) -> (String, u32) {
    (format!("dc.w ${:04x}", read_word(addr)), 2)
}
