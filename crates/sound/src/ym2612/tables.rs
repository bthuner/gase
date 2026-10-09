//! The YM2612's ROMs and constant tables.
//!
//! Almost nothing in the chip is computed with multipliers: it is all table
//! lookups, shifts and adds. That is why the chip is cheap, and also why it
//! sounds the way it does — every quantisation step below is audible in some
//! game. The tables here are either *derived from their defining formula*
//! (the two ROMs, the detune table) or are the small hand-made tables found on
//! the die and confirmed by hardware tests (envelope increments, LFO).

use std::f64::consts::FRAC_PI_2;
use std::sync::LazyLock;

/// The two ROMs of the operator: a quarter-wave **log-sine** table and an
/// **exponential** table.
///
/// # Why logarithms?
///
/// An operator must compute `sin(phase) × envelope_gain`. A multiplier was
/// expensive silicon in 1988, but in the log domain a multiplication is an
/// addition: `log(sin × gain) = log(sin) + log(gain)`. The envelope generator
/// already works in *attenuation* (−dB, i.e. a logarithm), so the chip looks
/// up `−log2(sin)`, adds the attenuation, and converts back with an `2^−x`
/// table. Two small ROMs and one adder replace a multiplier.
///
/// * `log_sin[i] = round(−log2(sin((i + ½)·π/2 / 256)) · 256)` — 256 entries
///   covering a quarter of the sine wave (the other three quarters are
///   obtained by mirroring and negating), in units of 1/256 of an octave
///   (≈ 0.0235 dB). The `+ ½` avoids `log(0)` and centres each step.
/// * `exp[i] = round((2^(i/256) − 1) · 1024)` — the fractional part of the
///   inverse. The integer part of the attenuation is applied with a shift.
///
/// These formulas reproduce the values read off the die (e.g. `log_sin[0] =
/// 0x859`, `exp[255] = 0x3FA`); the tests check a few of them.
#[derive(Debug)]
pub(crate) struct Roms {
    pub log_sin: [u16; 256],
    pub exp: [u16; 256],
}

/// Built once, on first use (Rust's `f64::sin`/`log2` are not `const`).
pub(crate) static ROMS: LazyLock<Roms> = LazyLock::new(|| {
    let mut log_sin = [0u16; 256];
    let mut exp = [0u16; 256];
    for (i, (ls, ex)) in log_sin.iter_mut().zip(exp.iter_mut()).enumerate() {
        let i = i as f64;
        let angle = (i + 0.5) * FRAC_PI_2 / 256.0;
        *ls = (-angle.sin().log2() * 256.0).round() as u16;
        *ex = (((i / 256.0).exp2() - 1.0) * 1024.0).round() as u16;
    }
    Roms { log_sin, exp }
});

/// Compute one operator output sample.
///
/// * `phase` — 10-bit phase (1024 steps = one sine period), already including
///   the modulation from other operators.
/// * `attenuation` — 10-bit total attenuation from the envelope generator
///   (0 = loudest, 1023 ≈ −96 dB, one step = 0.09375 dB).
///
/// Returns a 14-bit signed value (±8168 at most).
#[inline]
pub(crate) fn operator_output(roms: &Roms, phase: u32, attenuation: u32) -> i32 {
    // Bits 0-7 index the quarter wave; bit 8 says "second or fourth quarter",
    // where the table is read backwards; bit 9 is the sign (second half).
    let index = if phase & 0x100 != 0 {
        !phase & 0xFF
    } else {
        phase & 0xFF
    };
    // Log domain: attenuation (10 bits, 0.09375 dB) is shifted left by 2 to
    // match the log-sin units (0.0234 dB): 4 log-sin steps = 1 EG step.
    let level = (u32::from(roms.log_sin[index as usize]) + (attenuation << 2)).min(0x1FFF);
    // Back to linear: the low 8 bits pick the mantissa from the exp ROM (read
    // inverted, because the ROM holds 2^+x and we need 2^−x), the implicit
    // leading 1 (0x400) is added, and the high bits are a right shift.
    let mantissa = (u32::from(roms.exp[((level & 0xFF) ^ 0xFF) as usize]) | 0x400) << 2;
    let magnitude = (mantissa >> (level >> 8)) as i32;
    if phase & 0x200 != 0 {
        -magnitude
    } else {
        magnitude
    }
}

/// Above this attenuation an operator's output is always 0 (the final shift
/// is ≥ 14), so we can skip the table lookups. MAME calls this `ENV_QUIET`.
pub(crate) const QUIET_ATTENUATION: u32 = 0x380;

// ---------------------------------------------------------------------------
// Phase generator
// ---------------------------------------------------------------------------

/// The "note" part of the key code, indexed by F-number bits 10..7.
///
/// The key code (KC) is a 5-bit coarse pitch, `block × 4 + note`, used for key
/// scaling and detune. `note` is a 2-bit summary of the F-number: bit 1 is
/// F-num bit 10, bit 0 is `bit10 ? (b9|b8|b7) : (b9&b8&b7)`. This roughly
/// splits each octave into four regions.
pub(crate) const FN_NOTE: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 3, 3, 3, 3, 3, 3];

/// Key code from an 11-bit F-number and a 3-bit block.
#[inline]
pub(crate) fn key_code(fnum: u16, block: u8) -> u8 {
    (block << 2) | FN_NOTE[(fnum >> 7) as usize & 0xF]
}

/// Base values from which the detune table is derived (found on the die).
const DETUNE_BASE: [u8; 8] = [16, 17, 19, 20, 22, 24, 27, 29];

/// Detune amount in phase-increment units, indexed by `[DT & 3][key code]`.
///
/// Detune (DT) adds a small, pitch-dependent offset to the phase increment so
/// that operators tuned to the same note beat slightly against each other —
/// the classic "chorus" thickening. The offset grows exponentially with the
/// key code, so its *musical* size (in cents) stays similar across octaves.
/// DT bit 2 selects subtraction instead of addition.
///
/// On the die it is computed as `base[note, octave parity] >> shift`; we
/// evaluate that formula at compile time. The result equals the 4×32 table
/// printed in the Sega/Yamaha documentation (checked in the tests).
pub(crate) const DETUNE: [[u8; 32]; 4] = build_detune();

const fn build_detune() -> [[u8; 32]; 4] {
    let mut table = [[0u8; 32]; 4];
    let mut dt = 1;
    while dt < 4 {
        let mut kc = 0;
        while kc < 32 {
            // Key codes above 28 behave like 28.
            let k = if kc > 28 { 28 } else { kc };
            let block = k >> 2;
            let note = k & 3;
            let sum = block
                + 9
                + if dt == 1 {
                    0
                } else if dt == 2 {
                    2
                } else {
                    3
                };
            table[dt][kc] = DETUNE_BASE[((sum & 1) << 2) | note] >> (9 - (sum >> 1));
            kc += 1;
        }
        dt += 1;
    }
    table
}

// ---------------------------------------------------------------------------
// Envelope generator
// ---------------------------------------------------------------------------

/// Envelope increment patterns. Each row is 8 steps of a repeating pattern;
/// the step is chosen by the global envelope counter.
///
/// The envelope only has 64 rates but must cover a range from "1023 steps in
/// a few milliseconds" to "1023 steps in minutes". It does so in two ways:
/// slow rates update *less often* (see [`EG_SHIFT`]), fast rates add *bigger
/// increments*. Within a group of four rates, the patterns below interpolate
/// between powers of two: 4 rates per octave of speed.
pub(crate) const EG_INC: [[u8; 8]; 19] = [
    [0, 1, 0, 1, 0, 1, 0, 1],         // 0: rates 0..47, rate & 3 == 0
    [0, 1, 0, 1, 1, 1, 0, 1],         // 1: rate & 3 == 1
    [0, 1, 1, 1, 0, 1, 1, 1],         // 2: rate & 3 == 2
    [0, 1, 1, 1, 1, 1, 1, 1],         // 3: rate & 3 == 3
    [1, 1, 1, 1, 1, 1, 1, 1],         // 4: rate 48
    [1, 1, 1, 2, 1, 1, 1, 2],         // 5: rate 49
    [1, 2, 1, 2, 1, 2, 1, 2],         // 6: rate 50
    [1, 2, 2, 2, 1, 2, 2, 2],         // 7: rate 51
    [2, 2, 2, 2, 2, 2, 2, 2],         // 8: rate 52
    [2, 2, 2, 4, 2, 2, 2, 4],         // 9: rate 53
    [2, 4, 2, 4, 2, 4, 2, 4],         // 10: rate 54
    [2, 4, 4, 4, 2, 4, 4, 4],         // 11: rate 55
    [4, 4, 4, 4, 4, 4, 4, 4],         // 12: rate 56
    [4, 4, 4, 8, 4, 4, 4, 8],         // 13: rate 57
    [4, 8, 4, 8, 4, 8, 4, 8],         // 14: rate 58
    [4, 8, 8, 8, 4, 8, 8, 8],         // 15: rate 59
    [8, 8, 8, 8, 8, 8, 8, 8],         // 16: rates 60..63
    [16, 16, 16, 16, 16, 16, 16, 16], // 17: unused by the OPN2, kept for completeness
    [0, 0, 0, 0, 0, 0, 0, 0],         // 18: rates 0 and 1: the envelope never moves
];

/// Row of [`EG_INC`] used by each of the 64 rates.
///
/// Rates 0-1 never move. Rates 2-7 do not follow the regular pattern; their
/// rows were measured on hardware by Nemesis (rates 6-7 use row 2).
pub(crate) const EG_ROW: [u8; 64] = build_eg_row();

const fn build_eg_row() -> [u8; 64] {
    let mut rows = [0u8; 64];
    let mut rate = 0;
    while rate < 64 {
        rows[rate] = match rate {
            0 | 1 => 18,
            2..=5 => 0,
            6 | 7 => 2,
            8..=47 => (rate & 3) as u8,
            48..=59 => (rate - 44) as u8,
            _ => 16,
        };
        rate += 1;
    }
    rows
}

/// How many low bits of the global envelope counter must be zero for a rate to
/// update: rate `r < 48` updates every `2^(11 − r/4)` envelope ticks. Rates
/// 48 and up update on every tick.
pub(crate) const EG_SHIFT: [u8; 64] = build_eg_shift();

const fn build_eg_shift() -> [u8; 64] {
    let mut shifts = [0u8; 64];
    let mut rate = 0;
    while rate < 48 {
        shifts[rate] = 11 - (rate / 4) as u8;
        rate += 1;
    }
    shifts
}

// ---------------------------------------------------------------------------
// LFO
// ---------------------------------------------------------------------------

/// Samples per LFO step for each of the 8 LFO frequencies (register 0x22).
///
/// The LFO is a 7-bit counter (128 steps per period); it advances once every
/// `LFO_PERIOD[freq]` samples. At 53 267 Hz this gives 3.82, 5.33, 5.78, 6.12,
/// 6.61, 9.25, 46.2 and 69.4 Hz (the datasheet's 3.98…72.2 Hz figures assume
/// an 8 MHz clock).
pub(crate) const LFO_PERIOD: [u8; 8] = [109, 78, 72, 68, 63, 45, 9, 6];

/// Amplitude modulation depth: the LFO's 0..126 triangle is shifted right by
/// this amount for AMS = 0..3, giving 0, 1.4, 5.9 and 11.8 dB of tremolo.
pub(crate) const AM_SHIFT: [u8; 4] = [7, 3, 1, 0];

/// Phase modulation (vibrato) is computed from the top 7 bits of the F-number
/// with two shifted copies: `(fnum_hi >> PM_SHIFT1) + (fnum_hi >> PM_SHIFT2)`,
/// indexed by `[PMS][LFO step within a quarter wave]`. A shift of 7 means
/// "contributes nothing". Because the deviation is proportional to the
/// F-number, vibrato depth is constant in cents: 0, 3.4, 6.7, 10, 14, 20, 40
/// and 80 cents for PMS 0..7 (PMS 6 and 7 additionally shift left).
pub(crate) const PM_SHIFT1: [[u8; 8]; 8] = [
    [7, 7, 7, 7, 7, 7, 7, 7],
    [7, 7, 7, 7, 7, 7, 7, 7],
    [7, 7, 7, 7, 7, 7, 1, 1],
    [7, 7, 7, 7, 1, 1, 1, 1],
    [7, 7, 7, 1, 1, 1, 1, 0],
    [7, 7, 1, 1, 0, 0, 0, 0],
    [7, 7, 1, 1, 0, 0, 0, 0],
    [7, 7, 1, 1, 0, 0, 0, 0],
];

/// Second shift table for phase modulation, see [`PM_SHIFT1`].
pub(crate) const PM_SHIFT2: [[u8; 8]; 8] = [
    [7, 7, 7, 7, 7, 7, 7, 7],
    [7, 7, 7, 7, 2, 2, 2, 2],
    [7, 7, 7, 2, 2, 2, 7, 7],
    [7, 7, 2, 2, 7, 7, 2, 2],
    [7, 7, 2, 7, 7, 7, 2, 7],
    [7, 7, 7, 2, 7, 7, 2, 1],
    [7, 7, 7, 2, 7, 7, 2, 1],
    [7, 7, 7, 2, 7, 7, 2, 1],
];

/// Apply LFO phase modulation to an 11-bit F-number.
///
/// Returns a 12-bit "doubled" F-number (`fnum × 2 ± deviation`): the chip
/// keeps one extra fractional bit so that small deviations are not lost.
/// `lfo_pm` is the 5-bit LFO PM position: bit 4 is the sign (second half of
/// the wave), bits 0-3 walk up and back down a quarter wave.
#[inline]
pub(crate) fn modulated_fnum(fnum: u16, pms: u8, lfo_pm: u8) -> u32 {
    let doubled = u32::from(fnum) << 1;
    if pms == 0 {
        return doubled;
    }
    let mut step = lfo_pm & 0x0F;
    if step & 0x08 != 0 {
        step ^= 0x0F; // mirror: 0..7 then 7..0
    }
    let fnum_hi = u32::from(fnum >> 4);
    let (pms, step) = (pms as usize, step as usize);
    let mut deviation = (fnum_hi >> PM_SHIFT1[pms][step]) + (fnum_hi >> PM_SHIFT2[pms][step]);
    if pms > 5 {
        deviation <<= pms - 5;
    }
    deviation >>= 2;
    let f = if lfo_pm & 0x10 != 0 {
        doubled.wrapping_sub(deviation)
    } else {
        doubled + deviation
    };
    f & 0xFFF
}

/// Phase increment per sample (20-bit phase accumulator) for one operator.
///
/// * `fnum12` — the doubled, PM-modulated F-number from [`modulated_fnum`].
/// * `block` — octave (0-7): a left shift, i.e. ×2 per octave.
/// * `dt` — 3-bit detune register, `mul` — 4-bit multiple register.
///
/// The frequency in Hz is `increment × sample_rate / 2^20`.
#[inline]
pub(crate) fn phase_increment(fnum12: u32, block: u8, kcode: u8, dt: u8, mul: u8) -> u32 {
    let mut base = (fnum12 << block) >> 2;
    let detune = u32::from(DETUNE[usize::from(dt & 3)][usize::from(kcode & 31)]);
    if dt & 4 != 0 {
        // Can underflow for very low notes: the real chip wraps around too,
        // producing a very high pitch (a known YM2612 quirk).
        base = base.wrapping_sub(detune);
    } else {
        base += detune;
    }
    base &= 0x1_FFFF;
    // MUL = 0 means ×½: the chip multiplies by (MUL × 2, or 1) then halves.
    let multiple = if mul == 0 { 1 } else { u32::from(mul) * 2 };
    ((base * multiple) >> 1) & 0xF_FFFF
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roms_match_die_values() {
        let roms = &*ROMS;
        assert_eq!(roms.log_sin[0], 0x859);
        assert_eq!(roms.log_sin[1], 0x6C3);
        assert_eq!(roms.log_sin[255], 0);
        assert_eq!(&roms.exp[..6], &[0, 3, 6, 8, 11, 14]);
        assert_eq!(roms.exp[255], 0x3FA);
    }

    #[test]
    fn detune_matches_documented_table() {
        let fd1: [u8; 32] = [
            0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 3, 3, 3, 4, 4, 4, 5, 5, 6, 6, 7, 8,
            8, 8, 8,
        ];
        let fd2: [u8; 32] = [
            1, 1, 1, 1, 2, 2, 2, 2, 2, 3, 3, 3, 4, 4, 4, 5, 5, 6, 6, 7, 8, 8, 9, 10, 11, 12, 13,
            14, 16, 16, 16, 16,
        ];
        let fd3: [u8; 32] = [
            2, 2, 2, 2, 2, 3, 3, 3, 4, 4, 4, 5, 5, 6, 6, 7, 8, 8, 9, 10, 11, 12, 13, 14, 16, 17,
            19, 20, 22, 22, 22, 22,
        ];
        assert_eq!(DETUNE[0], [0; 32]);
        assert_eq!(DETUNE[1], fd1);
        assert_eq!(DETUNE[2], fd2);
        assert_eq!(DETUNE[3], fd3);
    }

    #[test]
    fn operator_output_extremes() {
        let roms = &*ROMS;
        // Peak of the sine (phase 256 = 90°) at full volume.
        assert_eq!(operator_output(roms, 0xFF, 0), 8168);
        assert_eq!(operator_output(roms, 0x2FF, 0), -8168);
        assert_eq!(operator_output(roms, 0xFF, QUIET_ATTENUATION), 0);
        // 6 dB = 64 EG steps halves the amplitude.
        let half = operator_output(roms, 0xFF, 64);
        assert!((4080..=4088).contains(&half), "{half}");
    }

    #[test]
    fn vibrato_depth() {
        // PMS 7 at the positive peak: fnum 0x400 → +0x60 on the doubled value.
        assert_eq!(modulated_fnum(0x400, 7, 7), 0x800 + 0x60);
        assert_eq!(modulated_fnum(0x400, 7, 0x17), 0x800 - 0x60);
        assert_eq!(modulated_fnum(0x400, 0, 7), 0x800);
    }
}
