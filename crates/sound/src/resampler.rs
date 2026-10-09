//! Converting the chip's native sample rate to the host's.
//!
//! # Why resample at all?
//!
//! The YM2612 produces samples at `clock / 144` ≈ 53 267 Hz (NTSC) or
//! 52 781 Hz (PAL). Sound cards want 44 100 or 48 000 Hz. We could run the
//! chip at the host rate instead, but then its envelopes, LFO and timers —
//! all defined per *chip* sample — would be wrong, and FM's high harmonics
//! would alias differently. So we emulate at the native rate and convert.
//!
//! # Why not just interpolate linearly?
//!
//! Changing the sample rate means reconstructing the continuous signal and
//! sampling it again. The ideal reconstruction filter is a `sinc`: it keeps
//! everything below the output's Nyquist frequency and removes everything
//! above it. Anything above the new Nyquist that is *not* removed folds back
//! ("aliases") as inharmonic noise, and FM sound has plenty of energy up
//! there. Linear interpolation is a very poor low-pass (its first sidelobe is
//! only −26 dB down), so a bright FM patch at 48 kHz would sound gritty.
//!
//! # What we do: windowed-sinc, polyphase
//!
//! Each output sample is a weighted sum of `TAPS` = 32 input samples. The
//! weights are a `sinc` low-pass truncated by a Kaiser window (β = 7, about
//! 70 dB of stop-band attenuation), with its cut-off just below the smaller
//! of the two Nyquist frequencies. The weights depend on where the output
//! sample falls *between* two input samples; we tabulate them for 256
//! fractional positions ("phases") and interpolate linearly between adjacent
//! phases, which makes any ratio — even one that changes every frame — work
//! with the same table.
//!
//! Cost: 32 multiply-adds per channel per output sample, ~3 M/s for stereo
//! 48 kHz — negligible next to the emulation itself.
//!
//! # Dynamic rate control
//!
//! To keep audio and video in sync without crackles, the frontend nudges the
//! *input* rate by a fraction of a percent ([`Resampler::set_input_rate`]).
//! Only the step between output samples changes; the history buffer and the
//! fractional position are kept, so there is no discontinuity. The filter is
//! only rebuilt if the cut-off would move by more than 2 %, which small
//! corrections never do.

use std::fmt;

/// Filter length in input samples. The output is delayed by `TAPS / 2`.
const TAPS: usize = 32;
/// Number of tabulated fractional positions.
const PHASES: usize = 256;
/// Kaiser window shape parameter (higher: less ripple, wider transition).
const BETA: f64 = 7.0;
/// Cut-off as a fraction of the lower of the two sample rates (0.5 would be
/// exactly Nyquist; we leave room for the filter's transition band).
const CUTOFF_FRACTION: f64 = 0.42;

/// Stereo windowed-sinc resampler. See the [module documentation](self).
#[derive(Clone)]
pub struct Resampler {
    input_rate: f64,
    output_rate: f64,
    /// Input samples per output sample.
    step: f64,
    /// Cut-off the kernel was built for, in cycles per input sample.
    cutoff: f64,
    /// `(PHASES + 1) × TAPS` coefficients; row `p` is for fractional
    /// position `p / PHASES`.
    kernel: Vec<f32>,
    left: Vec<f32>,
    right: Vec<f32>,
    /// Position of the next output sample, in input samples from `left[0]`.
    pos: f64,
    gain: f32,
}

impl fmt::Debug for Resampler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Resampler")
            .field("input_rate", &self.input_rate)
            .field("output_rate", &self.output_rate)
            .field("buffered", &self.left.len())
            .field("pos", &self.pos)
            .field("gain", &self.gain)
            .finish_non_exhaustive()
    }
}

impl Resampler {
    /// Create a resampler from `input_rate` to `output_rate` (both in Hz).
    #[must_use]
    pub fn new(input_rate: f64, output_rate: f64) -> Self {
        assert!(
            input_rate > 0.0 && output_rate > 0.0,
            "sample rates must be positive"
        );
        let cutoff = cutoff_for(input_rate, output_rate);
        let half = TAPS / 2;
        Self {
            input_rate,
            output_rate,
            step: input_rate / output_rate,
            cutoff,
            kernel: build_kernel(cutoff),
            // Start with silence as history, so the first output sample is
            // centred on the first input sample.
            left: vec![0.0; half - 1],
            right: vec![0.0; half - 1],
            pos: (half - 1) as f64,
            gain: 1.0,
        }
    }

    /// Change the input rate (dynamic rate control). Glitch-free: the
    /// buffered history and the current position are kept.
    pub fn set_input_rate(&mut self, input_rate: f64) {
        assert!(input_rate > 0.0, "sample rates must be positive");
        self.input_rate = input_rate;
        self.step = input_rate / self.output_rate;
        let cutoff = cutoff_for(input_rate, self.output_rate);
        if (cutoff / self.cutoff - 1.0).abs() > 0.02 {
            self.cutoff = cutoff;
            self.kernel = build_kernel(cutoff);
        }
    }

    /// Current input rate.
    #[must_use]
    pub fn input_rate(&self) -> f64 {
        self.input_rate
    }

    /// Output rate.
    #[must_use]
    pub fn output_rate(&self) -> f64 {
        self.output_rate
    }

    /// Linear gain applied before converting to `i16` (default 1.0).
    pub fn set_gain(&mut self, gain: f32) {
        self.gain = gain;
    }

    /// Number of input samples buffered but not yet fully consumed.
    #[must_use]
    pub fn buffered(&self) -> usize {
        self.left.len()
    }

    /// Feed one stereo sample at the input rate.
    pub fn push(&mut self, left: i32, right: i32) {
        self.left.push(left as f32);
        self.right.push(right as f32);
    }

    /// Append every output sample that can be computed from the input so far
    /// to `out`, interleaved `L, R`, clamped to `i16`.
    pub fn drain(&mut self, out: &mut Vec<i16>) {
        let half = TAPS / 2;
        let mut taps = [0f32; TAPS];
        loop {
            let i = self.pos as usize; // floor: pos is always positive
            if i + half >= self.left.len() {
                break;
            }
            let frac = self.pos - i as f64;
            let phase_f = frac * PHASES as f64;
            let p = (phase_f as usize).min(PHASES - 1);
            let t = (phase_f - p as f64) as f32;
            let a = &self.kernel[p * TAPS..(p + 1) * TAPS];
            let b = &self.kernel[(p + 1) * TAPS..(p + 2) * TAPS];
            for ((k, &x), &y) in taps.iter_mut().zip(a).zip(b) {
                *k = x + (y - x) * t;
            }
            let start = i + 1 - half;
            let l = dot(&taps, &self.left[start..start + TAPS]);
            let r = dot(&taps, &self.right[start..start + TAPS]);
            out.push(to_i16(l * self.gain));
            out.push(to_i16(r * self.gain));
            self.pos += self.step;
        }
        // Forget input that no future output sample can reach.
        let consumed = (self.pos as usize + 1)
            .saturating_sub(half)
            .min(self.left.len());
        if consumed > 0 {
            self.left.drain(..consumed);
            self.right.drain(..consumed);
            self.pos -= consumed as f64;
        }
    }
}

#[inline]
fn dot(a: &[f32; TAPS], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[inline]
fn to_i16(x: f32) -> i16 {
    x.round().clamp(-32768.0, 32767.0) as i16
}

/// Cut-off in cycles per input sample.
fn cutoff_for(input_rate: f64, output_rate: f64) -> f64 {
    CUTOFF_FRACTION * input_rate.min(output_rate) / input_rate
}

/// Modified Bessel function of the first kind, order 0 (for the Kaiser
/// window), by its power series.
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let mut k = 1.0;
    while term > 1e-12 * sum {
        term *= (x / (2.0 * k)) * (x / (2.0 * k));
        sum += term;
        k += 1.0;
    }
    sum
}

/// Tabulate the windowed-sinc kernel for `PHASES + 1` fractional positions.
fn build_kernel(cutoff: f64) -> Vec<f32> {
    use std::f64::consts::PI;
    let half = TAPS as f64 / 2.0;
    let i0_beta = bessel_i0(BETA);
    let mut kernel = vec![0f32; (PHASES + 1) * TAPS];
    for (p, row) in kernel.chunks_exact_mut(TAPS).enumerate() {
        let frac = p as f64 / PHASES as f64;
        let mut taps = [0f64; TAPS];
        for (j, tap) in taps.iter_mut().enumerate() {
            // Distance (in input samples) between the output instant and
            // input sample `start + j`.
            let tau = frac + half - 1.0 - j as f64;
            let x = tau / half;
            let window = if x.abs() >= 1.0 {
                0.0
            } else {
                bessel_i0(BETA * (1.0 - x * x).sqrt()) / i0_beta
            };
            let sinc = if tau == 0.0 {
                2.0 * cutoff
            } else {
                (2.0 * PI * cutoff * tau).sin() / (PI * tau)
            };
            *tap = sinc * window;
        }
        // Normalise every phase to unity DC gain, so a constant input gives
        // exactly the same constant output whatever the position.
        let sum: f64 = taps.iter().sum();
        for (out, tap) in row.iter_mut().zip(taps) {
            *out = (tap / sum) as f32;
        }
    }
    kernel
}

#[cfg(test)]
mod tests {
    use super::*;

    const IN_RATE: f64 = 53_693_175.0 / 1008.0;

    fn run_sine(freq: f64, amplitude: f64, out_rate: f64, n: usize) -> Vec<i16> {
        let mut rs = Resampler::new(IN_RATE, out_rate);
        let mut out = Vec::new();
        for k in 0..n {
            let v = (amplitude * (2.0 * std::f64::consts::PI * freq * k as f64 / IN_RATE).sin())
                .round() as i32;
            rs.push(v, -v);
            if k % 100 == 0 {
                rs.drain(&mut out);
            }
        }
        rs.drain(&mut out);
        out
    }

    #[test]
    fn produces_the_right_number_of_samples() {
        let mut rs = Resampler::new(IN_RATE, 48_000.0);
        let mut out = Vec::new();
        for _ in 0..IN_RATE as usize {
            rs.push(0, 0);
        }
        rs.drain(&mut out);
        let frames = out.len() / 2;
        assert!((47_980..=48_000).contains(&frames), "{frames}");
        // Feeding in small chunks gives the same count as one big chunk.
        let mut rs2 = Resampler::new(IN_RATE, 48_000.0);
        let mut out2 = Vec::new();
        for _ in 0..IN_RATE as usize {
            rs2.push(0, 0);
            rs2.drain(&mut out2);
        }
        assert_eq!(out2.len(), out.len());
    }

    #[test]
    fn passes_a_low_sine_accurately() {
        let freq = 1000.0;
        let out = run_sine(freq, 10_000.0, 48_000.0, 20_000);
        let step = IN_RATE / 48_000.0;
        let mut max_err = 0.0f64;
        for (k, frame) in out.chunks_exact(2).enumerate().skip(TAPS) {
            // Output k is centred on input sample k × step.
            let t = k as f64 * step / IN_RATE;
            let expected = 10_000.0 * (2.0 * std::f64::consts::PI * freq * t).sin();
            max_err = max_err.max((f64::from(frame[0]) - expected).abs());
            assert_eq!(frame[1], frame[0].saturating_neg());
        }
        assert!(max_err < 10.0, "max error {max_err}");
    }

    #[test]
    fn rejects_frequencies_above_output_nyquist() {
        // 25 kHz cannot be represented at 48 kHz: it must not alias to 23 kHz.
        let out = run_sine(25_000.0, 10_000.0, 48_000.0, 20_000);
        let rms = (out
            .chunks_exact(2)
            .skip(TAPS)
            .map(|f| f64::from(f[0]).powi(2))
            .sum::<f64>()
            / (out.len() / 2 - TAPS) as f64)
            .sqrt();
        assert!(rms < 30.0, "aliased rms {rms}");
    }

    #[test]
    fn rate_change_is_continuous() {
        let mut rs = Resampler::new(IN_RATE, 48_000.0);
        let mut out = Vec::new();
        for k in 0..20_000 {
            if k == 10_000 {
                rs.set_input_rate(IN_RATE * 1.005);
            }
            let v = (8000.0 * (k as f64 * 0.05).sin()) as i32;
            rs.push(v, v);
            rs.drain(&mut out);
        }
        // A smooth input stays smooth: no jumps bigger than the signal slope.
        let max_jump = out
            .chunks_exact(2)
            .skip(TAPS)
            .zip(out.chunks_exact(2).skip(TAPS + 1))
            .map(|(a, b)| (i32::from(a[0]) - i32::from(b[0])).abs())
            .max()
            .unwrap();
        assert!(max_jump < 600, "{max_jump}");
    }

    #[test]
    fn clamps_and_applies_gain() {
        let mut rs = Resampler::new(48_000.0, 48_000.0);
        rs.set_gain(0.5);
        let mut out = Vec::new();
        for _ in 0..200 {
            rs.push(100_000, 1000);
        }
        rs.drain(&mut out);
        let last = &out[out.len() - 2..];
        assert_eq!(last, &[32767, 500]);
    }
}
