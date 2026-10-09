//! The Mega Drive's sound hardware: a Yamaha **YM2612** FM synthesiser, the
//! **SN76489**-compatible PSG built into the VDP, and the host-side
//! post-processing that turns their output into something a sound card can
//! play.
//!
//! # The signal chain
//!
//! ```text
//!   Z80 / 68000 writes
//!        │                     ┌──────────────────────────────┐
//!        ├──► Ym2612::write ──►│ YM2612  (master/7 = 7.67 MHz) │── one stereo sample every
//!        │                     │ 6 FM channels, DAC on ch. 6   │   144 YM clocks ≈ 53 267 Hz
//!        │                     └──────────────────────────────┘          │
//!        │                     ┌──────────────────────────────┐          ▼
//!        └──► Psg::write ─────►│ PSG (master/15 = 3.58 MHz)    │──► mix (system) ──► [LowPass] ──► [DcBlocker]
//!                              │ 3 squares + noise, tick /16   │   at 53 267 Hz                       │
//!                              └──────────────────────────────┘                                      ▼
//!                                                                                Resampler ──► host (e.g. 48 000 Hz)
//! ```
//!
//! * [`Ym2612`] — the FM chip. Its module documentation is a small course on
//!   FM synthesis as the OPN2 actually implements it: operators, algorithms,
//!   the log-sin/exp tables, the envelope generator, SSG-EG, the LFO, timers,
//!   the DAC and the infamous "ladder effect".
//! * [`Psg`] — three square-wave tone generators and a noise generator.
//! * [`Resampler`] — a windowed-sinc polyphase resampler from the chip's
//!   native rate to the host rate, with glitch-free rate changes for dynamic
//!   rate control.
//! * [`LowPass`] and [`DcBlocker`] — optional "analogue" stages: the model-1
//!   console's output low-pass, and the coupling capacitor that removes DC.
//!
//! # Output scale (read this before mixing)
//!
//! Everything in this crate speaks one integer scale, the YM2612's internal
//! **14-bit** operator scale:
//!
//! * one FM channel at full volume swings about **±8192**
//!   ([`FM_CHANNEL_FULL_SCALE`]); [`Ym2612::clock_sample`] returns the sum of
//!   the six channels, so it ranges over roughly ±50 000 per side;
//! * one PSG channel at full volume swings **±2048**, so all four PSG channels
//!   at full volume together equal one FM channel ([`Psg::output`]).
//!
//! The recommended mix is **FM + PSG at 1 : 1** (just add `psg.output()` to
//! both FM sides). A full-volume PSG square then has about half the loudness
//! (RMS) of a full-volume FM sine, which is roughly what a model-1 console
//! sounds like, and matches the default balance of Genesis Plus GX. Before
//! converting to `i16`, scale the sum down (the [`Resampler`] has a gain for
//! exactly that); `0.5` is a good default since games rarely drive all six FM
//! channels at full volume at once.
//!
//! # Why the chips are clocked per sample
//!
//! The YM2612 really does compute one sample every 144 clocks: internally it
//! steps through 24 operator "slots" of 6 clocks each, and the DAC multiplexes
//! the channels in time. Nothing observable by software happens at a finer
//! granularity except the busy flag (see [`Ym2612::read_status`]), so stepping
//! one whole sample at a time is both exact enough and fast.

pub mod filter;
pub mod psg;
pub mod resampler;
pub mod ym2612;

pub use filter::{DcBlocker, LowPass};
pub use psg::Psg;
pub use resampler::Resampler;
pub use ym2612::Ym2612;

/// NTSC master clock in Hz.
pub const MASTER_CLOCK_NTSC: u32 = 53_693_175;
/// PAL master clock in Hz.
pub const MASTER_CLOCK_PAL: u32 = 53_203_424;
/// The YM2612 runs at master clock / 7.
pub const YM_CLOCK_DIVIDER: u32 = 7;
/// YM2612 clocks per output sample.
pub const YM_CLOCKS_PER_SAMPLE: u32 = 144;
/// Master clocks per YM2612 output sample (7 × 144).
pub const MASTER_CLOCKS_PER_YM_SAMPLE: u32 = YM_CLOCK_DIVIDER * YM_CLOCKS_PER_SAMPLE;
/// The PSG runs at master clock / 15.
pub const PSG_CLOCK_DIVIDER: u32 = 15;
/// PSG clocks per counter tick ([`Psg::tick`]).
pub const PSG_CLOCKS_PER_TICK: u32 = 16;
/// Master clocks per PSG counter tick (15 × 16).
pub const MASTER_CLOCKS_PER_PSG_TICK: u32 = PSG_CLOCK_DIVIDER * PSG_CLOCKS_PER_TICK;
/// Approximate peak amplitude of one FM channel at full volume (14-bit scale).
pub const FM_CHANNEL_FULL_SCALE: i32 = 8192;

/// Native YM2612 sample rate for a master clock frequency (≈ 53 267 Hz NTSC).
#[must_use]
pub fn ym_sample_rate(master_clock: u32) -> f64 {
    f64::from(master_clock) / f64::from(MASTER_CLOCKS_PER_YM_SAMPLE)
}
