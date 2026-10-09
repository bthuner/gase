//! One FM channel: four operators wired together by an algorithm.

use super::operator::Operator;
use super::tables::{AM_SHIFT, Roms, key_code, modulated_fnum};

/// Operator slots in *register order*. The register map lists the operators
/// as S1, S3, S2, S4 (offsets +0, +4, +8, +C), and so do we.
pub(crate) const S1: usize = 0;
pub(crate) const S3: usize = 1;
pub(crate) const S2: usize = 2;
pub(crate) const S4: usize = 3;

/// Frequency of one operator in channel-3 special mode: (F-number, block).
pub(crate) type SpecialFreqs = [(u16, u8); 3];

/// For channel 3 special mode: which entry of the special frequency array
/// (registers A8, A9, AA) drives slots S1, S3, S2. S4 uses the normal A2.
const SPECIAL_SOURCE: [usize; 3] = [1, 0, 2];

#[derive(Clone, Debug)]
pub(crate) struct Channel {
    pub ops: [Operator; 4],
    /// 11-bit F-number (pitch within an octave).
    pub fnum: u16,
    /// 3-bit block (octave).
    pub block: u8,
    pub algorithm: u8,
    /// Self-feedback level of S1 (0-7).
    pub feedback: u8,
    pub pan_left: bool,
    pub pan_right: bool,
    /// Amplitude modulation sensitivity (0-3).
    pub ams: u8,
    /// Phase modulation sensitivity (0-7).
    pub pms: u8,
    /// Last two outputs of S1: `[newest, older]`.
    pub op1_out: [i16; 2],
    /// Previous sample's output of S2 (consumed one sample late, see `calc`).
    pub s2_out: i16,
    /// Frequency registers changed: phase increments must be recomputed.
    pub dirty: bool,
    /// LFO PM position the cached increments were computed for.
    pub last_pm: u8,
}

gase_savestate::impl_state!(Channel {
    ops,
    fnum,
    block,
    algorithm,
    feedback,
    pan_left,
    pan_right,
    ams,
    pms,
    op1_out,
    s2_out,
    dirty,
    last_pm,
});

impl Default for Channel {
    fn default() -> Self {
        Self {
            ops: Default::default(),
            fnum: 0,
            block: 0,
            algorithm: 0,
            feedback: 0,
            pan_left: true,
            pan_right: true,
            ams: 0,
            pms: 0,
            op1_out: [0; 2],
            s2_out: 0,
            dirty: true,
            last_pm: 0,
        }
    }
}

impl Channel {
    /// Recompute the operators' phase increments if anything they depend on
    /// changed. Caching matters: this is the only place with a multiply, and
    /// most channels do not use vibrato.
    #[inline]
    pub fn refresh_frequency(&mut self, special: Option<&SpecialFreqs>, lfo_pm: u8) {
        let pm = if self.pms == 0 { 0 } else { lfo_pm };
        if !self.dirty && pm == self.last_pm {
            return;
        }
        self.dirty = false;
        self.last_pm = pm;
        for (slot, op) in self.ops.iter_mut().enumerate() {
            let (fnum, block) = match special {
                Some(freqs) if slot != S4 => freqs[SPECIAL_SOURCE[slot]],
                _ => (self.fnum, self.block),
            };
            let kcode = key_code(fnum, block);
            op.set_frequency(modulated_fnum(fnum, self.pms, pm), block, kcode);
        }
    }

    /// Compute the channel's output for this sample: a 9-bit signed value
    /// (−256..=255), i.e. what the DAC will see.
    ///
    /// The operators are evaluated in the chip's internal order S1, S3, S2,
    /// S4. Because the chip is pipelined, an input that comes from an operator
    /// computed *later* in that order (or not yet finished) is the value from
    /// the **previous sample**. The resulting delays, as established from the
    /// die (Nuked-OPN2):
    ///
    /// | connection          | used in algorithms | value used        |
    /// |---------------------|--------------------|-------------------|
    /// | S1 → S1 (feedback)  | all                | previous two      |
    /// | S1 → S2             | 0, 3, 4, 5, 6      | current sample    |
    /// | S1 → S3             | 1, 5               | previous sample   |
    /// | S2 → S3             | 0, 1, 2            | previous sample   |
    /// | S3 → S4             | 0, 1, 2, 3, 4      | current sample    |
    /// | S1 → S4             | 2, 5               | current sample    |
    /// | S2 → S4             | 3                  | previous sample   |
    #[inline]
    pub fn calc(&mut self, roms: &Roms, lfo_am: u16) -> i32 {
        let am = lfo_am >> AM_SHIFT[usize::from(self.ams)];
        let s1_prev = i32::from(self.op1_out[0]);
        let s2_prev = i32::from(self.s2_out);
        let algorithm = self.algorithm;

        // Feedback: S1 modulates itself with the sum (i.e. twice the average)
        // of its last two outputs. Averaging two samples tames the tendency
        // of strong feedback to oscillate at Nyquist; FB = 7 gives ±4π.
        let fb = if self.feedback == 0 {
            0
        } else {
            (i32::from(self.op1_out[0]) + i32::from(self.op1_out[1])) >> (10 - self.feedback)
        };
        let s1 = self.ops[S1].run(roms, fb, am);

        // Normal modulation: a 14-bit output shifted right by 1 is added to
        // the 10-bit phase, so a full-scale modulator swings ±4096 phase
        // steps = ±4 periods = ±8π radians (a "modulation index" of ~25).
        let mod3 = match algorithm {
            0 | 2 => s2_prev,
            1 => s1_prev + s2_prev,
            5 => s1_prev,
            _ => 0,
        };
        let s3 = self.ops[S3].run(roms, mod3 >> 1, am);

        let mod2 = match algorithm {
            0 | 3..=6 => s1,
            _ => 0,
        };
        let s2 = self.ops[S2].run(roms, mod2 >> 1, am);

        let mod4 = match algorithm {
            0 | 1 | 4 => s3,
            2 => s1 + s3,
            3 => s3 + s2_prev,
            5 => s1,
            _ => 0,
        };
        let s4 = self.ops[S4].run(roms, mod4 >> 1, am);

        self.op1_out = [s1 as i16, self.op1_out[0]];
        self.s2_out = s2 as i16;

        // Carriers are summed by a 9-bit accumulator: each output is first
        // truncated to its top 9 bits, and the sum saturates.
        let sum = match algorithm {
            0..=3 => s4 >> 5,
            4 => (s2 >> 5) + (s4 >> 5),
            5 | 6 => (s2 >> 5) + (s3 >> 5) + (s4 >> 5),
            _ => (s1 >> 5) + (s2 >> 5) + (s3 >> 5) + (s4 >> 5),
        };
        sum.clamp(-256, 255)
    }
}
