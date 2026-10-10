//! Yamaha YM2612 (OPN2): six-channel, four-operator FM synthesis.
//!
//! This is a from-scratch implementation following what the hardware was
//! reverse-engineered to do: Nemesis' measurements on real consoles, and the
//! die-shot-derived Nuked-OPN2 and Genesis Plus GX as references of knowledge.
//! It is sample-accurate (not cycle-accurate): one call to
//! [`Ym2612::clock_sample`] computes one output sample, exactly as the chip
//! does every 144 of its clocks.
//!
//! # FM synthesis in five minutes
//!
//! An **operator** is a sine oscillator with a volume envelope:
//! `out(t) = env(t) · sin(phase(t))`. Alone it makes a pure, dull tone. The
//! trick of frequency modulation (John Chowning, 1973; licensed to Yamaha) is
//! to add one operator's output to another one's *phase*:
//!
//! ```text
//! out = sin(ωc·t + I · sin(ωm·t))
//! ```
//!
//! The *modulator* (frequency ωm) bends the *carrier* (ωc) back and forth.
//! This creates sidebands at `ωc ± k·ωm`: when ωm/ωc is a simple ratio
//! (1, 2, 3…) the sidebands are harmonics and the result is a rich,
//! pitched timbre; odd ratios give bells and metallic sounds. The modulation
//! index `I` (the modulator's volume) controls brightness, and since the
//! modulator has its own envelope, brightness can evolve over a note — a
//! plucked bass that starts bright and mellows, for instance. Two sines can
//! do what would otherwise need dozens of oscillators or a resonant filter.
//!
//! ## Algorithms
//!
//! Each channel has four operators, S1-S4, and register `B0` selects one of
//! eight **algorithms**: how the operators modulate each other. Operators
//! whose output is heard are *carriers*; the others are *modulators*.
//!
//! ```text
//!  0: S1→S2→S3→S4→        4: S1→S2→ ┐       6: S1→S2 ┐
//!                            S3→S4→ ┴→          S3    ┼→
//!  1: S1┐                                       S4    ┘
//!     S2┴→S3→S4→          5:    ┌→S2┐
//!                            S1─┼→S3┼→       7: S1 S2 S3 S4 (all carriers,
//!  2:    S1──┐                  └→S4┘           additive "organ")
//!     S2→S3→┴→S4→
//!
//!  3: S1→S2─┐
//!        S3─┴→S4→
//! ```
//!
//! S1 can also modulate *itself* (**feedback**, register `B0` bits 3-5): with
//! increasing feedback its sine morphs towards a sawtooth and finally noise.
//!
//! # How the OPN2 computes an operator
//!
//! ## Phase generator
//!
//! A 20-bit accumulator advances every sample by an increment computed from
//! the channel's **F-number** (11 bits, pitch within an octave) and **block**
//! (3 bits, octave), the operator's **MUL** (frequency ratio ×½, ×1…×15 — the
//! knob that sets the harmonic ratio above) and **DT** (detune, a small
//! pitch-dependent offset for chorus). The output frequency is
//!
//! ```text
//! f = fnum · 2^block · MUL / 2^21 · (clock / 144)      (MUL = 0 counts as ½)
//! ```
//!
//! The top 10 bits of the accumulator form the phase: 1024 steps per period.
//! See `phase_increment` in `tables.rs`.
//!
//! ## Sine without multiplying: log-sin and exp ROMs
//!
//! The chip has no multiplier. It stores `−log2(sin)` for a quarter wave (256
//! entries), *adds* the envelope's attenuation (already logarithmic: dB), and
//! converts back to linear with a 256-entry `2^x` table and a shift. The
//! output is a 14-bit signed number. Modulation is added to the phase before
//! the lookup. Details in `tables.rs`.
//!
//! ## Envelope generator (ADSR)
//!
//! Each operator's volume follows an envelope, kept as a 10-bit
//! **attenuation** (0 = full volume, 1023 ≈ −96 dB; 0.09375 dB per step):
//!
//! ```text
//!  level ▲ 0 dB        key on                          key off
//!        │  ╱╲___                                         │
//!        │ ╱  D1R ╲____ SL                                │
//!        │╱AR          ╲‾‾‾‾‾‾‾‾‾‾‾‾‾ D2R ‾‾‾‾‾‾‾‾‾‾‾‾╲    │
//!        │                                            ╲RR │
//!        └──────────────────────────────────────────────╲─► time
//! ```
//!
//! *Attack* (AR) rises exponentially to 0 dB, *first decay* (D1R) falls
//! linearly in dB to the *sustain level* (SL), *second decay* (D2R) keeps
//! falling while the key is held, *release* (RR) falls after key off. On top
//! of this, **TL** (total level, 0.75 dB steps) is a fixed offset — on a
//! modulator it sets the modulation index, on a carrier the loudness.
//!
//! Rates are 5-bit registers turned into 6-bit rates `2·R + KSR`, where key
//! scaling (KS) adds part of the note's key code so that high notes are
//! shorter. The envelope is clocked every 3 samples; a global 12-bit counter
//! decides which rates step on this tick and by how much (see
//! `EG_INC` in `tables.rs`): 64 rates spanning from minutes to under a millisecond.
//!
//! ## SSG-EG
//!
//! A mode borrowed from the AY-3-8910 ("SSG") family's envelope shapes: when
//! enabled (register `90`), the decay runs 4× faster and, on reaching −48 dB,
//! the envelope repeats (sawtooth), alternates direction (triangle) or holds,
//! optionally inverted. Rarely used in music, but some games rely on it.
//!
//! ## LFO
//!
//! One global low-frequency oscillator (≈ 3.8-69 Hz, register `22`) can add
//! *tremolo* (amplitude modulation: up to 11.8 dB, per-channel depth AMS,
//! per-operator enable) and *vibrato* (phase modulation: up to ±80 cents,
//! per-channel depth PMS) by nudging the F-number.
//!
//! # Around the operators
//!
//! * **Channel 3 special mode** (register `27` bits 6-7): channel 3's four
//!   operators get independent frequencies (`A8`-`AE`), handy for chords or
//!   sound effects.
//! * **Timers** A (10-bit, period `1024 − NA` samples) and B (8-bit, period
//!   `16 × (256 − NB)` samples) set flags in the status register; drivers poll
//!   them to keep tempo.
//! * **CSM** (composite sine mode): timer A overflow keys channel 3 on for one
//!   sample — originally for speech synthesis.
//! * **DAC**: with register `2B` bit 7 set, channel 6's output is replaced by
//!   the 8-bit value written to `2A`. The Z80 streams drum samples through it.
//!
//! # The output stage and the "ladder effect"
//!
//! Each channel produces a 9-bit value that goes to a single, time-
//! multiplexed 9-bit DAC. In the discrete YM2612 (model-1 consoles) this DAC
//! has a defect: in the slots where a channel is *not* outputting its value,
//! the DAC still outputs a small level that depends on the sign of that
//! value. Averaged, each channel contributes `v + 4` when `v ≥ 0` and `v − 3`
//! when `v < 0`, and even a channel panned *off* a side contributes `±4` to
//! it. This crossover distortion makes quiet notes noticeably louder and
//! grittier — part of the "Mega Drive sound" — so it is emulated by default
//! ([`Ym2612::set_ladder_effect`]). The later integrated YM3438 does not have
//! it.
//!
//! # Output scale
//!
//! [`Ym2612::clock_sample`] returns the sum over the six channels of the
//! 9-bit DAC values (ladder offsets included) multiplied by 32, i.e. back on
//! the 14-bit operator scale: one channel at full volume is ±8160 (+128 for
//! the ladder offset), six are about ±50 000. With the ladder effect on, a
//! silent chip outputs a constant +768 (6 × 4 × 32) on both sides: real
//! hardware has the same DC offset, removed by the output capacitor; use a
//! [`DcBlocker`](crate::DcBlocker) if it matters.

mod channel;
mod operator;
mod tables;
#[cfg(test)]
mod tests;

use channel::{Channel, S1, S2, S3, S4, SpecialFreqs};
use tables::{LFO_PERIOD, ROMS};

/// How long the busy flag stays set after a data write, in YM2612 clocks.
///
/// The chip needs 32 of its internal cycles (each 6 clocks: the 24 operator
/// slots of a sample take 144 clocks) to latch a register write. We only
/// advance time in whole samples, so the flag is cleared after the 2nd sample
/// following the write (192 clocks → 2 × 144), which is close enough for the
/// drivers that poll it; nothing ever depends on its exact length.
const BUSY_CLOCKS: u16 = 32 * 6;

/// Register 0x27 bits.
const LOAD_A: u8 = 0x01;
const LOAD_B: u8 = 0x02;
const ENABLE_A: u8 = 0x04;
const ENABLE_B: u8 = 0x08;

/// Channel 3 mode (register 0x27 bits 6-7) value for CSM.
const CH3_MODE_CSM: u8 = 2;

/// The YM2612 FM synthesiser. See the [module documentation](self).
#[derive(Clone, Debug)]
pub struct Ym2612 {
    channels: [Channel; 6],

    // --- bus interface -------------------------------------------------------
    /// Latched register address.
    address: u8,
    /// Latched register part (0 = I, channels 1-3; 1 = II, channels 4-6).
    part: u8,
    busy_clocks: u16,

    // --- frequency latches ---------------------------------------------------
    /// High F-number/block byte written to A4-A6, applied on the A0-A2 write.
    /// One latch shared by all channels (writing A4 then A1 affects ch. 2!).
    fnum_latch: u8,
    /// Same for channel-3 special frequencies (AC-AE → A8-AA).
    ch3_latch: u8,
    /// Channel-3 special F-numbers for registers A8, A9, AA.
    ch3_fnum: [u16; 3],
    /// Channel-3 special blocks for registers AC, AD, AE.
    ch3_block: [u8; 3],
    /// Register 0x27 bits 6-7: 0 normal, 1 special, 2 CSM (+special), 3 special.
    ch3_mode: u8,
    /// CSM key-on currently asserted.
    csm_key: bool,

    // --- timers --------------------------------------------------------------
    timer_a: u16,
    timer_a_count: u16,
    timer_b: u8,
    timer_b_count: u16,
    /// Free-running divide-by-16 prescaler of timer B.
    timer_b_prescaler: u8,
    /// Register 0x27 bits 0-3 (load A/B, enable A/B flags).
    timer_control: u8,
    /// Status bits 0-1 (timer overflow flags).
    status: u8,

    // --- LFO -----------------------------------------------------------------
    lfo_enabled: bool,
    lfo_freq: u8,
    /// 7-bit LFO position.
    lfo_counter: u8,
    lfo_divider: u8,

    // --- envelope clock ------------------------------------------------------
    /// Counts samples 0..3; the envelope generator ticks every 3 samples.
    eg_divider: u8,
    /// Global 12-bit envelope counter (1..=4095).
    eg_counter: u16,

    // --- DAC -----------------------------------------------------------------
    dac_enabled: bool,
    dac_data: u8,

    // --- host settings (not saved) -------------------------------------------
    ladder: bool,
    /// Channels silenced by the host (bit 0 = channel 1), for listening to
    /// channels one at a time. The channels still run; only the mix skips
    /// them.
    muted: u8,
}

gase_savestate::impl_state!(Ym2612 {
    channels,
    address,
    part,
    busy_clocks,
    fnum_latch,
    ch3_latch,
    ch3_fnum,
    ch3_block,
    ch3_mode,
    csm_key,
    timer_a,
    timer_a_count,
    timer_b,
    timer_b_count,
    timer_b_prescaler,
    timer_control,
    status,
    lfo_enabled,
    lfo_freq,
    lfo_counter,
    lfo_divider,
    eg_divider,
    eg_counter,
    dac_enabled,
    dac_data,
});

impl Default for Ym2612 {
    fn default() -> Self {
        Self::new()
    }
}

impl Ym2612 {
    /// A chip in its power-on state (all channels silent, panned centre).
    #[must_use]
    pub fn new() -> Self {
        // Force the ROMs to be built now rather than during the first sample.
        let _ = &*ROMS;
        Self {
            channels: Default::default(),
            address: 0,
            part: 0,
            busy_clocks: 0,
            fnum_latch: 0,
            ch3_latch: 0,
            ch3_fnum: [0; 3],
            ch3_block: [0; 3],
            ch3_mode: 0,
            csm_key: false,
            timer_a: 0,
            timer_a_count: 1024,
            timer_b: 0,
            timer_b_count: 256,
            timer_b_prescaler: 0,
            timer_control: 0,
            status: 0,
            lfo_enabled: false,
            lfo_freq: 0,
            lfo_counter: 0,
            lfo_divider: 0,
            eg_divider: 0,
            eg_counter: 0,
            dac_enabled: false,
            dac_data: 0x80,
            ladder: true,
            muted: 0,
        }
    }

    /// Reset the chip (the `/IC` pin). The ladder-effect and mute settings
    /// are kept.
    pub fn reset(&mut self) {
        let (ladder, muted) = (self.ladder, self.muted);
        *self = Self::new();
        self.ladder = ladder;
        self.muted = muted;
    }

    /// Silence channels in the output mix: bit 0 mutes channel 1 ... bit 5
    /// channel 6 (including the DAC). A host setting for learning and
    /// debugging, not a chip feature: muted channels keep running and come
    /// back exactly in step.
    pub fn set_muted_channels(&mut self, mask: u8) {
        self.muted = mask & 0x3F;
    }

    /// The mask set with [`Ym2612::set_muted_channels`].
    #[must_use]
    pub fn muted_channels(&self) -> u8 {
        self.muted
    }

    /// Enable or disable emulation of the discrete YM2612's DAC distortion
    /// (on by default). When disabled, the output is the clean 9-bit DAC
    /// value, as on the YM3438 found in later consoles.
    pub fn set_ladder_effect(&mut self, enabled: bool) {
        self.ladder = enabled;
    }

    /// Bus write. `port` is A1:A0: 0 = address (part I), 1 = data (part I),
    /// 2 = address (part II), 3 = data (part II).
    ///
    /// As on the real chip, the part is taken from the last *address* write;
    /// the data port only sets the busy flag and delivers the value.
    pub fn write(&mut self, port: u8, value: u8) {
        match port & 3 {
            0 => {
                self.address = value;
                self.part = 0;
            }
            2 => {
                self.address = value;
                self.part = 1;
            }
            _ => {
                self.busy_clocks = BUSY_CLOCKS;
                self.write_register(self.part, self.address, value);
            }
        }
    }

    /// Status register: bit 7 busy, bit 1 timer B overflow, bit 0 timer A
    /// overflow. (On the discrete YM2612 every port reads the status.)
    pub fn read_status(&mut self) -> u8 {
        self.status | if self.busy_clocks > 0 { 0x80 } else { 0 }
    }

    /// Write a register directly (`part` 0 or 1), bypassing the address latch
    /// and the busy flag. Handy for music players (VGM) and tests.
    pub fn write_register(&mut self, part: u8, address: u8, value: u8) {
        let part = usize::from(part & 1);
        if address < 0x30 {
            // Global registers exist only in part I.
            if part == 0 {
                self.write_global(address, value);
            }
            return;
        }
        let ch_in_part = usize::from(address & 3);
        if ch_in_part == 3 || address >= 0xB8 {
            return;
        }
        let ch = part * 3 + ch_in_part;
        let channel = &mut self.channels[ch];
        if address < 0xA0 {
            // Operator registers: bits 2-3 of the address select the slot,
            // in register order S1, S3, S2, S4.
            let op = &mut channel.ops[usize::from((address >> 2) & 3)];
            match address & 0xF0 {
                0x30 => {
                    op.dt = (value >> 4) & 7;
                    op.mul = value & 0x0F;
                    channel.dirty = true;
                }
                0x40 => op.tl = value & 0x7F,
                0x50 => {
                    op.ks = value >> 6;
                    op.ar = value & 0x1F;
                }
                0x60 => {
                    op.am = value & 0x80 != 0;
                    op.d1r = value & 0x1F;
                }
                0x70 => op.d2r = value & 0x1F,
                0x80 => {
                    op.sl = value >> 4;
                    op.rr = value & 0x0F;
                }
                _ => op.ssg = value & 0x0F,
            }
            return;
        }
        match address & 0xFC {
            0xA0 => {
                channel.fnum = (u16::from(self.fnum_latch & 7) << 8) | u16::from(value);
                channel.block = (self.fnum_latch >> 3) & 7;
                channel.dirty = true;
            }
            0xA4 => self.fnum_latch = value & 0x3F,
            0xA8 if part == 0 => {
                self.ch3_fnum[ch_in_part] = (u16::from(self.ch3_latch & 7) << 8) | u16::from(value);
                self.ch3_block[ch_in_part] = (self.ch3_latch >> 3) & 7;
                self.channels[2].dirty = true;
            }
            0xAC if part == 0 => self.ch3_latch = value & 0x3F,
            0xB0 => {
                channel.feedback = (value >> 3) & 7;
                channel.algorithm = value & 7;
            }
            0xB4 => {
                channel.pan_left = value & 0x80 != 0;
                channel.pan_right = value & 0x40 != 0;
                channel.ams = (value >> 4) & 3;
                channel.pms = value & 7;
                channel.dirty = true;
            }
            _ => {}
        }
    }

    fn write_global(&mut self, address: u8, value: u8) {
        match address {
            0x22 => {
                self.lfo_enabled = value & 0x08 != 0;
                self.lfo_freq = value & 7;
                if !self.lfo_enabled {
                    // A disabled LFO is held at position 0 (which, note, means
                    // *maximum* tremolo attenuation for AM-enabled operators).
                    self.lfo_counter = 0;
                    self.lfo_divider = 0;
                }
            }
            0x24 => self.timer_a = (self.timer_a & 3) | (u16::from(value) << 2),
            0x25 => self.timer_a = (self.timer_a & 0x3FC) | u16::from(value & 3),
            0x26 => self.timer_b = value,
            0x27 => {
                let old = self.timer_control;
                // A 0→1 transition of a load bit (re)starts the timer.
                if value & LOAD_A != 0 && old & LOAD_A == 0 {
                    self.timer_a_count = 1024 - self.timer_a;
                }
                if value & LOAD_B != 0 && old & LOAD_B == 0 {
                    self.timer_b_count = 256 - u16::from(self.timer_b);
                }
                if value & 0x10 != 0 {
                    self.status &= !1;
                }
                if value & 0x20 != 0 {
                    self.status &= !2;
                }
                self.timer_control = value & 0x0F;
                let mode = value >> 6;
                if mode != self.ch3_mode {
                    self.ch3_mode = mode;
                    self.channels[2].dirty = true;
                }
            }
            0x28 => {
                let ch = match value & 7 {
                    c @ 0..=2 => usize::from(c),
                    c @ 4..=6 => usize::from(c) - 1,
                    _ => return,
                };
                let csm = ch == 2 && self.csm_key;
                let ops = &mut self.channels[ch].ops;
                for (bit, slot) in [(0x10, S1), (0x20, S2), (0x40, S3), (0x80, S4)] {
                    let op = &mut ops[slot];
                    op.key_reg = value & bit != 0;
                    op.set_key(op.key_reg || csm);
                }
            }
            0x2A => self.dac_data = value,
            0x2B => self.dac_enabled = value & 0x80 != 0,
            // 0x21 and 0x2C are LSI test registers; nothing uses them.
            _ => {}
        }
    }

    /// Advance one output sample (144 YM clocks) and return the stereo output
    /// `(left, right)`, mixed over the six channels with panning. See the
    /// module documentation for the scale.
    pub fn clock_sample(&mut self) -> (i32, i32) {
        let roms = &*ROMS;

        // LFO outputs for this sample: a 0..126 triangle for tremolo, and
        // the top 5 bits of the counter for vibrato.
        let lfo_am = u16::from(if self.lfo_counter & 0x40 != 0 {
            self.lfo_counter & 0x3F
        } else {
            self.lfo_counter ^ 0x3F
        }) << 1;
        let lfo_pm = self.lfo_counter >> 2;

        let special: Option<SpecialFreqs> = (self.ch3_mode != 0).then(|| {
            let (f, b) = (self.ch3_fnum, self.ch3_block);
            [(f[0], b[0]), (f[1], b[1]), (f[2], b[2])]
        });
        let (mut left, mut right) = (0i32, 0i32);
        let mut mix = [(0i32, 0i32); 6];
        for (i, ch) in self.channels.iter_mut().enumerate() {
            ch.refresh_frequency(if i == 2 { special.as_ref() } else { None }, lfo_pm);
            for op in &mut ch.ops {
                op.ssg_update();
            }
            let mut out = ch.calc(roms, lfo_am);
            if i == 5 && self.dac_enabled {
                // 8-bit unsigned DAC sample → 9-bit signed (LSB = 0).
                out = (i32::from(self.dac_data) - 128) << 1;
            }
            let (l, r) = if self.ladder {
                // Ladder effect: see the module documentation.
                let (on, off) = if out >= 0 {
                    (out + 4, 4)
                } else {
                    (out - 3, -4)
                };
                (
                    if ch.pan_left { on } else { off },
                    if ch.pan_right { on } else { off },
                )
            } else {
                (
                    if ch.pan_left { out } else { 0 },
                    if ch.pan_right { out } else { 0 },
                )
            };
            left += l;
            right += r;
            mix[i] = (l, r);
        }
        if self.muted != 0 {
            // Take the muted channels back out of the mix (rarely used, so
            // this costs nothing in the normal case).
            for (i, (l, r)) in mix.iter().enumerate() {
                if self.muted & (1 << i) != 0 {
                    left -= l;
                    right -= r;
                }
            }
        }

        self.clock_lfo();
        self.clock_envelopes();
        self.clock_timers();
        self.busy_clocks = self.busy_clocks.saturating_sub(144);

        // 9-bit DAC scale → 14-bit operator scale.
        (left << 5, right << 5)
    }

    fn clock_lfo(&mut self) {
        if self.lfo_enabled {
            self.lfo_divider += 1;
            if self.lfo_divider >= LFO_PERIOD[usize::from(self.lfo_freq)] {
                self.lfo_divider = 0;
                self.lfo_counter = (self.lfo_counter + 1) & 0x7F;
            }
        }
    }

    fn clock_envelopes(&mut self) {
        self.eg_divider += 1;
        if self.eg_divider < 3 {
            return;
        }
        self.eg_divider = 0;
        // The counter skips 0 when it wraps, so that the slowest rates do not
        // all fire together right after power-on.
        self.eg_counter = if self.eg_counter >= 4095 {
            1
        } else {
            self.eg_counter + 1
        };
        let counter = self.eg_counter;
        for ch in &mut self.channels {
            for op in &mut ch.ops {
                op.eg_clock(counter);
            }
        }
    }

    fn clock_timers(&mut self) {
        let mut csm_fire = false;
        if self.timer_control & LOAD_A != 0 {
            self.timer_a_count = self.timer_a_count.saturating_sub(1);
            if self.timer_a_count == 0 {
                self.timer_a_count = 1024 - self.timer_a;
                if self.timer_control & ENABLE_A != 0 {
                    self.status |= 1;
                }
                csm_fire = self.ch3_mode == CH3_MODE_CSM;
            }
        }
        // CSM: the key-on lasts exactly one sample unless timer A fires again.
        if csm_fire || self.csm_key {
            self.csm_key = csm_fire;
            for op in &mut self.channels[2].ops {
                op.set_key(op.key_reg || csm_fire);
            }
        }

        self.timer_b_prescaler = (self.timer_b_prescaler + 1) & 15;
        if self.timer_b_prescaler == 0 && self.timer_control & LOAD_B != 0 {
            self.timer_b_count = self.timer_b_count.saturating_sub(1);
            if self.timer_b_count == 0 {
                self.timer_b_count = 256 - u16::from(self.timer_b);
                if self.timer_control & ENABLE_B != 0 {
                    self.status |= 2;
                }
            }
        }
    }
}
