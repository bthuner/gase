//! A tiny 68000 "assembler" for whole-system tests: builds ROM images
//! from hand-encoded instructions, so no ROM files are needed.

// Each test binary uses a different subset of these helpers.
#![allow(dead_code)]

use gase_core::{Cartridge, Config, Genesis};

pub const VDP_CTRL: u32 = 0xC0_0004;
pub const VDP_DATA: u32 = 0xC0_0000;

/// Builds a ROM image: vectors, a minimal header, and code at 0x200.
pub struct Rom {
    pub bytes: Vec<u8>,
    pub pc: usize,
}

impl Default for Rom {
    fn default() -> Self {
        Self::new()
    }
}

impl Rom {
    pub fn new() -> Self {
        let mut bytes = vec![0u8; 0x1000];
        bytes[0..4].copy_from_slice(&0x00FF_FE00u32.to_be_bytes()); // initial SSP
        bytes[4..8].copy_from_slice(&0x0000_0200u32.to_be_bytes()); // initial PC
        bytes[0x100..0x110].copy_from_slice(b"SEGA MEGA DRIVE ");
        bytes[0x1F0] = b'U';
        Self { bytes, pc: 0x200 }
    }

    pub fn words(&mut self, words: &[u16]) -> &mut Self {
        for w in words {
            self.bytes[self.pc..self.pc + 2].copy_from_slice(&w.to_be_bytes());
            self.pc += 2;
        }
        self
    }

    pub fn vector(&mut self, number: usize, target: u32) -> &mut Self {
        self.bytes[number * 4..number * 4 + 4].copy_from_slice(&target.to_be_bytes());
        self
    }

    pub fn at(&mut self, pc: usize) -> &mut Self {
        self.pc = pc;
        self
    }

    // A few 68000 instructions, encoded by hand.

    /// `move.w #imm, (addr).l`
    pub fn move_w(&mut self, imm: u16, addr: u32) -> &mut Self {
        self.words(&[0x33FC, imm, (addr >> 16) as u16, addr as u16])
    }

    /// `move.l #imm, (addr).l`
    pub fn move_l(&mut self, imm: u32, addr: u32) -> &mut Self {
        self.words(&[
            0x23FC,
            (imm >> 16) as u16,
            imm as u16,
            (addr >> 16) as u16,
            addr as u16,
        ])
    }

    /// `move.b #imm, (addr).l`
    pub fn move_b(&mut self, imm: u8, addr: u32) -> &mut Self {
        self.words(&[0x13FC, u16::from(imm), (addr >> 16) as u16, addr as u16])
    }

    /// `bra.s *` (spin forever)
    pub fn spin(&mut self) -> &mut Self {
        self.words(&[0x60FE])
    }

    /// `Bcc.s target`: `condition` is the high byte of the opcode (`0x60`
    /// bra, `0x66` bne, `0x67` beq).
    pub fn branch(&mut self, condition: u16, target: usize) -> &mut Self {
        let displacement = target as i64 - (self.pc as i64 + 2);
        assert!(
            (-128..128).contains(&displacement) && displacement != 0,
            "branch out of range"
        );
        self.words(&[(condition << 8) | (displacement as u8 as u16)])
    }

    /// `lea (addr).l, An`
    pub fn lea(&mut self, addr: u32, an: u16) -> &mut Self {
        self.words(&[0x41F9 | (an << 9), (addr >> 16) as u16, addr as u16])
    }

    /// `move.w (An), Dn`
    pub fn read_w(&mut self, an: u16, dn: u16) -> &mut Self {
        self.words(&[0x3010 | (dn << 9) | an])
    }

    /// `move.w #imm, (An)`
    pub fn write_w(&mut self, imm: u16, an: u16) -> &mut Self {
        self.words(&[0x30BC | (an << 9), imm])
    }

    /// `move.l #imm, (An)`
    pub fn write_l(&mut self, imm: u32, an: u16) -> &mut Self {
        self.words(&[0x20BC | (an << 9), (imm >> 16) as u16, imm as u16])
    }

    /// `move.w Dn, (addr).l`
    pub fn store_w(&mut self, dn: u16, addr: u32) -> &mut Self {
        self.words(&[0x33C0 | dn, (addr >> 16) as u16, addr as u16])
    }

    /// `btst #bit, Dn`
    pub fn btst(&mut self, bit: u16, dn: u16) -> &mut Self {
        self.words(&[0x0800 | dn, bit])
    }

    /// `moveq #0, Dn`
    pub fn clear(&mut self, dn: u16) -> &mut Self {
        self.words(&[0x7000 | (dn << 9)])
    }

    /// `addq.w #1, Dn`
    pub fn inc(&mut self, dn: u16) -> &mut Self {
        self.words(&[0x5240 | dn])
    }

    pub fn console(&self) -> Genesis {
        Genesis::new(
            Cartridge::from_bytes(&self.bytes).unwrap(),
            &Config::default(),
        )
    }
}
