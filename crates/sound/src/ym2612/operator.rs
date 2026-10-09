//! One FM operator: a phase generator, an envelope generator and the
//! sine/attenuation lookup that combines them.

use super::tables::{
    EG_INC, EG_ROW, EG_SHIFT, QUIET_ATTENUATION, Roms, operator_output, phase_increment,
};

/// Envelope phases. Stored as `u8` so save states stay trivial.
pub(crate) const ATTACK: u8 = 0;
pub(crate) const DECAY: u8 = 1;
pub(crate) const SUSTAIN: u8 = 2;
pub(crate) const RELEASE: u8 = 3;

/// Maximum attenuation (≈ 96 dB: silence).
pub(crate) const MAX_ATTENUATION: u16 = 0x3FF;

/// SSG-EG register bits.
const SSG_ENABLE: u8 = 0x08;
const SSG_ATTACK: u8 = 0x04;
const SSG_ALTERNATE: u8 = 0x02;
const SSG_HOLD: u8 = 0x01;

/// An operator: an oscillator with its own envelope.
#[derive(Clone, Debug)]
pub(crate) struct Operator {
    // --- registers ---------------------------------------------------------
    /// Detune (3 bits, bit 2 = negative).
    pub dt: u8,
    /// Frequency multiple (0 = ×½, 1-15 = ×n).
    pub mul: u8,
    /// Total level: 7-bit attenuation in 0.75 dB steps.
    pub tl: u8,
    /// Key scaling (0-3): how much higher notes speed up the envelope.
    pub ks: u8,
    /// Attack rate (5 bits).
    pub ar: u8,
    /// LFO amplitude modulation enabled for this operator.
    pub am: bool,
    /// First decay rate (5 bits).
    pub d1r: u8,
    /// Second decay ("sustain") rate (5 bits).
    pub d2r: u8,
    /// Sustain level (4 bits, 3 dB steps; 15 means 93 dB).
    pub sl: u8,
    /// Release rate (4 bits; doubled + 1 to make a 5-bit rate).
    pub rr: u8,
    /// SSG-EG mode (4 bits).
    pub ssg: u8,

    // --- phase generator -----------------------------------------------------
    /// 20-bit phase accumulator; the top 10 bits address the sine.
    pub phase: u32,
    /// Phase increment per sample (cached; recomputed when the frequency,
    /// DT, MUL or the LFO PM position changes).
    pub inc: u32,
    /// Key code of the note being played (for key scaling and detune).
    pub kcode: u8,

    // --- envelope generator --------------------------------------------------
    /// One of [`ATTACK`], [`DECAY`], [`SUSTAIN`], [`RELEASE`].
    pub eg_phase: u8,
    /// 10-bit attenuation (0 = loudest).
    pub level: u16,
    /// Key state requested by register 0x28.
    pub key_reg: bool,
    /// Effective key state (register key OR CSM key).
    pub key: bool,
    /// SSG-EG output inversion flip-flop.
    pub ssg_inv: bool,
}

gase_savestate::impl_state!(Operator {
    dt,
    mul,
    tl,
    ks,
    ar,
    am,
    d1r,
    d2r,
    sl,
    rr,
    ssg,
    phase,
    inc,
    kcode,
    eg_phase,
    level,
    key_reg,
    key,
    ssg_inv,
});

impl Default for Operator {
    fn default() -> Self {
        Self {
            dt: 0,
            mul: 0,
            tl: 0,
            ks: 0,
            ar: 0,
            am: false,
            d1r: 0,
            d2r: 0,
            sl: 0,
            rr: 0,
            ssg: 0,
            phase: 0,
            inc: 0,
            kcode: 0,
            eg_phase: RELEASE,
            level: MAX_ATTENUATION,
            key_reg: false,
            key: false,
            ssg_inv: false,
        }
    }
}

impl Operator {
    /// Recompute the phase increment from a (doubled, PM-modulated) F-number.
    #[inline]
    pub fn set_frequency(&mut self, fnum12: u32, block: u8, kcode: u8) {
        self.kcode = kcode;
        self.inc = phase_increment(fnum12, block, kcode, self.dt, self.mul);
    }

    /// Effective 6-bit envelope rate for a 5-bit rate register.
    ///
    /// `rate = 2 × R + KSR`, where the key-scaling term `KSR = keycode >>
    /// (3 − KS)` makes high notes decay faster (like a real piano string).
    /// A rate register of 0 means "infinitely slow" and is never key scaled.
    #[inline]
    fn rate(&self, r: u8) -> usize {
        if r == 0 {
            return 0;
        }
        let ksr = self.kcode >> (3 - self.ks);
        usize::from((2 * r + ksr).min(63))
    }

    /// Sustain level as a 10-bit attenuation (SL 15 is special: 93 dB).
    #[inline]
    fn sustain_level(&self) -> u16 {
        if self.sl == 15 {
            0x3E0
        } else {
            u16::from(self.sl) << 5
        }
    }

    #[inline]
    fn ssg_enabled(&self) -> bool {
        self.ssg & SSG_ENABLE != 0
    }

    /// Is the SSG-EG output currently inverted? (`attack` bit XOR flip-flop.)
    #[inline]
    fn ssg_inverted(&self) -> bool {
        self.ssg_enabled() && (self.ssg_inv != (self.ssg & SSG_ATTACK != 0))
    }

    /// Phase the envelope moves to once the attack has reached 0 dB.
    #[inline]
    fn after_attack(&self) -> u8 {
        if self.sl == 0 { SUSTAIN } else { DECAY }
    }

    /// Restart the envelope as on key-on (also used by looping SSG-EG).
    fn restart_attack(&mut self) {
        if self.rate(self.ar) >= 62 {
            // The fastest attack rates jump straight to full volume.
            self.level = 0;
        }
        self.eg_phase = if self.level == 0 {
            self.after_attack()
        } else {
            ATTACK
        };
    }

    /// Change the effective key state; acts only on edges.
    pub fn set_key(&mut self, on: bool) {
        if on == self.key {
            return;
        }
        self.key = on;
        if on {
            // Key-on: the oscillator restarts from phase 0, the envelope
            // restarts its attack *from the current level* (so retriggering a
            // sounding note does not click down to silence first).
            self.phase = 0;
            self.ssg_inv = false;
            self.restart_attack();
        } else if self.eg_phase != RELEASE {
            if self.ssg_enabled() {
                // The release continues from what was being *heard*, so an
                // inverted SSG level is converted to a normal one.
                if self.ssg_inverted() {
                    self.level = 0x200u16.wrapping_sub(self.level) & MAX_ATTENUATION;
                }
                if self.level >= 0x200 {
                    self.level = MAX_ATTENUATION;
                }
            }
            self.eg_phase = RELEASE;
        }
    }

    /// One envelope generator tick (every 3 samples).
    ///
    /// `counter` is the global 12-bit envelope counter. A rate updates the
    /// level only when the low `EG_SHIFT[rate]` bits of the counter are zero,
    /// and the increment is taken from an 8-step pattern selected by the next
    /// three bits.
    pub fn eg_clock(&mut self, counter: u16) {
        if self.eg_phase == DECAY && self.level >= self.sustain_level() {
            self.eg_phase = SUSTAIN;
        }
        let r = match self.eg_phase {
            ATTACK => self.ar,
            DECAY => self.d1r,
            SUSTAIN => self.d2r,
            _ => self.rr * 2 + 1,
        };
        let rate = self.rate(r);
        let shift = EG_SHIFT[rate];
        if counter & ((1 << shift) - 1) != 0 {
            return;
        }
        let inc = EG_INC[usize::from(EG_ROW[rate])][usize::from((counter >> shift) & 7)];
        if inc == 0 {
            return;
        }
        if self.eg_phase == ATTACK {
            // The attack is exponential: each step removes a fraction
            // (inc/16) of the *remaining* attenuation, `~level` being
            // −(level + 1). That is why FM attacks have their characteristic
            // fast-then-slow (in dB) shape.
            let level = i32::from(self.level);
            let next = level + ((!level * i32::from(inc)) >> 4);
            if next <= 0 {
                self.level = 0;
                self.eg_phase = self.after_attack();
            } else {
                self.level = next as u16;
            }
        } else if self.ssg_enabled() {
            // SSG-EG runs the decay four times faster and stops at 0x200;
            // `ssg_update` then loops/holds/inverts.
            if self.level < 0x200 {
                self.level += 4 * u16::from(inc);
            }
            if self.eg_phase == RELEASE && self.level >= 0x200 {
                self.level = MAX_ATTENUATION;
            }
        } else {
            self.level = (self.level + u16::from(inc)).min(MAX_ATTENUATION);
        }
    }

    /// SSG-EG state machine, run once per sample before the output.
    ///
    /// When the level crosses 0x200 (−48 dB) during attack/decay/sustain, the
    /// envelope either **loops** (restart the attack, optionally flipping the
    /// output inversion — "alternate" — or else resetting the oscillator
    /// phase) or **holds** (stay at the end level, which is silence or full
    /// volume depending on the inversion).
    pub fn ssg_update(&mut self) {
        if !self.ssg_enabled() || self.level < 0x200 || self.eg_phase == RELEASE {
            return;
        }
        if self.ssg & SSG_HOLD != 0 {
            if self.ssg & SSG_ALTERNATE != 0 {
                self.ssg_inv = true;
            }
            if self.eg_phase != ATTACK && !self.ssg_inverted() {
                self.level = MAX_ATTENUATION;
            }
        } else {
            if self.ssg & SSG_ALTERNATE != 0 {
                self.ssg_inv = !self.ssg_inv;
            } else {
                self.phase = 0;
            }
            if self.eg_phase != ATTACK {
                self.restart_attack();
            }
        }
    }

    /// Total attenuation (10 bits) heard at the output: envelope (possibly
    /// SSG-inverted) + total level + LFO tremolo.
    #[inline]
    pub fn attenuation(&self, am: u16) -> u32 {
        let mut level = self.level;
        if self.eg_phase != RELEASE && self.ssg_inverted() {
            level = 0x200u16.wrapping_sub(level) & MAX_ATTENUATION;
        }
        let am = if self.am { am } else { 0 };
        (u32::from(level) + (u32::from(self.tl) << 3) + u32::from(am)).min(0x3FF)
    }

    /// Produce this sample's output with the given phase modulation (in
    /// 10-bit phase units), then advance the oscillator.
    #[inline]
    pub fn run(&mut self, roms: &Roms, modulation: i32, am: u16) -> i32 {
        let att = self.attenuation(am);
        let out = if att >= QUIET_ATTENUATION {
            0
        } else {
            let phase = ((self.phase >> 10) as i32 + modulation) as u32 & 0x3FF;
            operator_output(roms, phase, att)
        };
        self.phase = (self.phase + self.inc) & 0xF_FFFF;
        out
    }
}
