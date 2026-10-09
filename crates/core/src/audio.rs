//! Driving the sound chips from the master clock and mixing them.
//!
//! The YM2612 produces one stereo sample every 1008 master clocks
//! (≈ 53 267 Hz on NTSC). The PSG's counters tick every 240 master clocks
//! (≈ 223 722 Hz). We run both "lazily": nothing happens until someone
//! needs the chips to be up to date (a CPU writes a sound register, or the
//! frame ends), and then [`AudioClock::run_until`] catches them up.
//!
//! Between two YM2612 samples the PSG ticks about 4.2 times; we average its
//! output over those ticks, which is a cheap low-pass filter, and add it to
//! the FM output. The resulting stream is at the YM2612's native rate; the
//! system resamples it to the host's rate afterwards.

use gase_savestate::impl_state;
use gase_sound::{Psg, Ym2612};

/// Master clocks per YM2612 output sample (7 × 144).
pub const YM_SAMPLE_PERIOD: u64 = 1008;
/// Master clocks per PSG counter tick (15 × 16).
pub const PSG_TICK_PERIOD: u64 = 240;

/// PSG level relative to the FM output, as a fraction (numerator / 256).
/// At 1:1, the four PSG channels together are as loud as one FM channel,
/// which matches the balance of a real console.
const PSG_GAIN: i64 = 256;

/// Native sample rate of the mixed output for a master clock frequency.
#[must_use]
pub fn native_rate(master_clock: u32) -> f64 {
    f64::from(master_clock) / YM_SAMPLE_PERIOD as f64
}

/// Keeps the sound chips in step with the master clock.
#[derive(Clone, Debug, Default)]
pub struct AudioClock {
    /// Master clock at which the next YM2612 sample is due.
    ym_next: u64,
    /// Master clock at which the next PSG tick is due.
    psg_next: u64,
    psg_sum: i64,
    psg_ticks: i64,
    /// Mixed stereo samples at the native rate, waiting to be resampled.
    pub samples: Vec<(i32, i32)>,
    /// Silence the PSG (to listen to the FM chip alone).
    pub psg_muted: bool,
}

impl_state!(AudioClock { ym_next, psg_next, psg_sum, psg_ticks });

impl AudioClock {
    /// Run the chips until the master clock reaches `now`.
    pub fn run_until(&mut self, now: u64, ym: &mut Ym2612, psg: &mut Psg) {
        loop {
            while self.psg_next <= now && self.psg_next <= self.ym_next {
                psg.tick();
                self.psg_sum += i64::from(psg.output());
                self.psg_ticks += 1;
                self.psg_next += PSG_TICK_PERIOD;
            }
            if self.ym_next > now {
                break;
            }
            let (left, right) = ym.clock_sample();
            let psg_level = if self.psg_ticks == 0 || self.psg_muted {
                0
            } else {
                (self.psg_sum / self.psg_ticks * PSG_GAIN / 256) as i32
            };
            self.psg_sum = 0;
            self.psg_ticks = 0;
            self.samples.push((left + psg_level, right + psg_level));
            self.ym_next += YM_SAMPLE_PERIOD;
        }
    }

    /// Re-anchor the clocks, e.g. after the master clock was reset.
    pub fn rebase(&mut self, now: u64) {
        self.ym_next = now;
        self.psg_next = now;
    }
}
