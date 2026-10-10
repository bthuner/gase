//! Simple first-order filters modelling the console's analogue output stage.
//!
//! These run at the chip's native rate, before the [`Resampler`](crate::Resampler).

use std::f64::consts::PI;

/// First-order (6 dB/octave) low-pass filter, stereo.
///
/// The model-1 Mega Drive's output circuit rolls off the treble quite early
/// (around 3.4 kHz with the usual analysis), which softens the YM2612's
/// aliasing and the PSG's harsh squares. Many players remember the console
/// sounding like that, so it is offered as an option.
///
/// `y[n] = y[n−1] + α (x[n] − y[n−1])` with `α = 1 − e^(−2π·fc/fs)`: the
/// discrete equivalent of an RC filter.
#[derive(Clone, Debug)]
pub struct LowPass {
    alpha: f32,
    left: f32,
    right: f32,
}

impl LowPass {
    /// Create a low-pass with the given −3 dB cut-off, running at
    /// `sample_rate`.
    #[must_use]
    pub fn new(cutoff_hz: f64, sample_rate: f64) -> Self {
        let alpha = 1.0 - (-2.0 * PI * cutoff_hz / sample_rate).exp();
        Self {
            alpha: alpha as f32,
            left: 0.0,
            right: 0.0,
        }
    }

    /// Filter one stereo sample.
    pub fn process(&mut self, l: i32, r: i32) -> (i32, i32) {
        self.left += self.alpha * (l as f32 - self.left);
        self.right += self.alpha * (r as f32 - self.right);
        (self.left.round() as i32, self.right.round() as i32)
    }
}

/// First-order high-pass ("DC blocker"), stereo.
///
/// Real hardware is AC-coupled through capacitors, so a constant offset —
/// like the YM2612 ladder effect's +768 when silent, or a DAC parked at a
/// non-centre value — never reaches the speakers. A cut-off of a few Hz
/// removes it without touching audible bass.
///
/// `y[n] = x[n] − x[n−1] + R·y[n−1]`, `R = e^(−2π·fc/fs)`.
#[derive(Clone, Debug)]
pub struct DcBlocker {
    r: f32,
    last_in: [f32; 2],
    last_out: [f32; 2],
}

impl DcBlocker {
    /// Create a DC blocker with the given cut-off (e.g. 10 Hz).
    #[must_use]
    pub fn new(cutoff_hz: f64, sample_rate: f64) -> Self {
        let r = (-2.0 * PI * cutoff_hz / sample_rate).exp();
        Self {
            r: r as f32,
            last_in: [0.0; 2],
            last_out: [0.0; 2],
        }
    }

    /// Filter one stereo sample.
    pub fn process(&mut self, l: i32, r: i32) -> (i32, i32) {
        let mut out = [0i32; 2];
        for (i, x) in [l as f32, r as f32].into_iter().enumerate() {
            let y = x - self.last_in[i] + self.r * self.last_out[i];
            self.last_in[i] = x;
            self.last_out[i] = y;
            out[i] = y.round() as i32;
        }
        (out[0], out[1])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_pass_passes_dc_and_cuts_treble() {
        let mut lp = LowPass::new(3400.0, 53_267.0);
        let mut out = (0, 0);
        for _ in 0..1000 {
            out = lp.process(10_000, -10_000);
        }
        assert_eq!(out, (10_000, -10_000));
        // A Nyquist-rate square is strongly attenuated.
        let mut lp = LowPass::new(3400.0, 53_267.0);
        let mut peak = 0;
        for k in 0..1000 {
            let x = if k % 2 == 0 { 10_000 } else { -10_000 };
            let (y, _) = lp.process(x, x);
            if k > 100 {
                peak = peak.max(y.abs());
            }
        }
        assert!(peak < 2500, "{peak}");
    }

    #[test]
    fn dc_blocker_removes_offset() {
        let mut dc = DcBlocker::new(10.0, 53_267.0);
        let mut out = (0, 0);
        for _ in 0..53_267 {
            out = dc.process(768, 768);
        }
        assert!(out.0.abs() <= 1 && out.1.abs() <= 1, "{out:?}");
    }
}
