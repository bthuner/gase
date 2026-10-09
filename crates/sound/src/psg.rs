//! The PSG: a Texas Instruments SN76489 clone integrated in the Sega VDP.
//!
//! "PSG" stands for *programmable sound generator*. It is about as simple as
//! a sound chip gets, and it is a good place to see the two basic ideas of
//! digital tone generation:
//!
//! # Tone channels: counters and flip-flops
//!
//! Each of the three tone channels has a 10-bit **period** register and a
//! down-counter. The chip's clock (master / 15 ≈ 3.58 MHz) is divided by 16;
//! on each of these ticks ([`Psg::tick`]) every counter is decremented, and
//! when it reaches zero it is reloaded with the period and an output
//! flip-flop toggles. The result is a square wave of frequency
//!
//! ```text
//! f = clock / (32 × period)        e.g. 3 579 545 / (32 × 254) ≈ 440 Hz
//! ```
//!
//! On the Sega variant a period of 0 behaves like 1. Periods 0 and 1 toggle
//! at ~112 kHz, far above hearing: after the console's analogue filtering
//! that is a *constant* level, which games exploit to play samples by
//! writing the volume register rapidly. We model those periods as a constant
//! "high" output, which gives the same audible result without producing an
//! ultrasonic square that would only alias.
//!
//! # Noise channel: a linear-feedback shift register
//!
//! Noise comes from a 16-bit **LFSR**: on each step the register shifts right
//! by one and a new bit, computed from some of its bits, enters at the top.
//! Bit 0 is the output. For *white* noise the new bit is the XOR of bits 0
//! and 3 (Sega's taps; TI's own chips differ), which produces a pseudo-random
//! sequence of 57 337 bits before repeating. For *periodic* noise the new bit
//! is just bit 0: the register rotates, giving a 1-in-16 pulse wave — a
//! buzzy tone at 1/16 of the shift rate.
//!
//! The shift rate is the noise counter's toggle rate divided by two: the
//! counter is reloaded with 16, 32 or 64 ticks, or with tone channel 3's
//! period ("use tone 3", which lets a game *tune* the noise). Writing the
//! noise register resets the LFSR to `0x8000`.
//!
//! # Volume: 2 dB per step
//!
//! Each channel has a 4-bit **attenuation**: 0 is loudest, each step is
//! −2 dB, 15 is off. Logarithmic steps sound evenly spaced to the ear.
//!
//! # Register interface
//!
//! There is a single write-only port. A byte with bit 7 set is a *latch/data*
//! byte: bits 6-5 select the channel, bit 4 selects volume (1) or tone/noise
//! (0), bits 3-0 are data (the low 4 bits of a period). A byte with bit 7
//! clear is a *data* byte for the latched register: the high 6 bits of a
//! period, or the 4 bits of a volume/noise register.
//!
//! # Output scale
//!
//! Each channel outputs ±2048 at full volume (bipolar: the real chip outputs
//! a unipolar level whose DC is removed by the output capacitor, so this only
//! differs by a constant). The four channels together reach ±8192, the
//! amplitude of one full-volume YM2612 channel.

/// Amplitude per attenuation step: `round(2048 × 10^(−2·i/20))`, 15 = off.
const VOLUME: [i32; 16] = [
    2048, 1627, 1292, 1026, 815, 648, 514, 409, 325, 258, 205, 163, 129, 103, 82, 0,
];

/// LFSR value after reset and after any write to the noise register.
const LFSR_RESET: u16 = 0x8000;

/// The SN76489-compatible PSG. See the [module documentation](self).
#[derive(Clone, Debug)]
pub struct Psg {
    /// Tone periods (10 bits).
    tone: [u16; 3],
    /// Noise control: bit 2 = white noise, bits 0-1 = shift rate.
    noise: u8,
    /// Attenuation of tone 1-3 and noise (4 bits, 15 = off).
    attenuation: [u8; 4],
    /// Latched register (0-7): `channel << 1 | is_volume`.
    latched: u8,
    /// Down-counters: three tones and the noise.
    counters: [u16; 4],
    /// Output flip-flops: three tones and the noise clock.
    flip_flops: [bool; 4],
    lfsr: u16,
    /// Channels silenced by the host (bit 0 = tone 1 ... bit 3 = noise); not
    /// part of the chip or the save state.
    muted: u8,
}

gase_savestate::impl_state!(Psg {
    tone,
    noise,
    attenuation,
    latched,
    counters,
    flip_flops,
    lfsr
});

impl Default for Psg {
    fn default() -> Self {
        Self::new()
    }
}

impl Psg {
    /// A PSG in its power-on state: all channels silent.
    #[must_use]
    pub fn new() -> Self {
        Self {
            tone: [0; 3],
            noise: 0,
            attenuation: [15; 4],
            latched: 0,
            counters: [0; 4],
            flip_flops: [false; 4],
            lfsr: LFSR_RESET,
            muted: 0,
        }
    }

    /// Reset to the power-on state (the mute setting is kept).
    pub fn reset(&mut self) {
        let muted = self.muted;
        *self = Self::new();
        self.muted = muted;
    }

    /// Silence channels in [`Psg::output`]: bit 0 = tone 1, bit 1 = tone 2,
    /// bit 2 = tone 3, bit 3 = noise. A host setting for learning and
    /// debugging; the channels keep running.
    pub fn set_muted_channels(&mut self, mask: u8) {
        self.muted = mask & 0x0F;
    }

    /// The mask set with [`Psg::set_muted_channels`].
    #[must_use]
    pub fn muted_channels(&self) -> u8 {
        self.muted
    }

    /// Write a byte to the PSG port.
    pub fn write(&mut self, value: u8) {
        if value & 0x80 != 0 {
            self.latched = (value >> 4) & 7;
            let data = value & 0x0F;
            match self.latched {
                r @ (0 | 2 | 4) => {
                    let t = &mut self.tone[usize::from(r >> 1)];
                    *t = (*t & 0x3F0) | u16::from(data);
                }
                6 => self.write_noise(data),
                r => self.attenuation[usize::from(r >> 1)] = data,
            }
        } else {
            match self.latched {
                r @ (0 | 2 | 4) => {
                    let t = &mut self.tone[usize::from(r >> 1)];
                    *t = (*t & 0x00F) | (u16::from(value & 0x3F) << 4);
                }
                6 => self.write_noise(value & 0x0F),
                r => self.attenuation[usize::from(r >> 1)] = value & 0x0F,
            }
        }
    }

    fn write_noise(&mut self, data: u8) {
        self.noise = data & 7;
        self.lfsr = LFSR_RESET;
    }

    /// Advance 16 PSG clocks (one counter tick).
    pub fn tick(&mut self) {
        for i in 0..3 {
            let counter = &mut self.counters[i];
            *counter = counter.saturating_sub(1);
            if *counter == 0 {
                *counter = self.tone[i].max(1);
                self.flip_flops[i] = !self.flip_flops[i];
            }
        }
        let counter = &mut self.counters[3];
        *counter = counter.saturating_sub(1);
        if *counter == 0 {
            *counter = match self.noise & 3 {
                3 => self.tone[2].max(1),
                rate => 0x10 << rate,
            };
            self.flip_flops[3] = !self.flip_flops[3];
            // The LFSR shifts on the rising edge of the noise clock.
            if self.flip_flops[3] {
                let feedback = if self.noise & 4 != 0 {
                    (self.lfsr ^ (self.lfsr >> 3)) & 1
                } else {
                    self.lfsr & 1
                };
                self.lfsr = (self.lfsr >> 1) | (feedback << 15);
            }
        }
    }

    /// Current mono output (sum of the four channels), on the same scale as
    /// one YM2612 channel: ±2048 per channel at full volume.
    #[must_use]
    pub fn output(&self) -> i32 {
        let mut sum = 0;
        for i in 0..3 {
            if self.muted & (1 << i) != 0 {
                continue;
            }
            let volume = VOLUME[usize::from(self.attenuation[i])];
            // Periods 0/1 are ultrasonic: treat as a constant high level.
            let high = self.tone[i] <= 1 || self.flip_flops[i];
            sum += if high { volume } else { -volume };
        }
        if self.muted & 0x08 != 0 {
            return sum;
        }
        let volume = VOLUME[usize::from(self.attenuation[3])];
        sum + if self.lfsr & 1 != 0 { volume } else { -volume }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_tone(psg: &mut Psg, channel: u8, period: u16) {
        psg.write(0x80 | (channel << 5) | (period & 0xF) as u8);
        psg.write((period >> 4) as u8 & 0x3F);
    }

    #[test]
    fn volume_table_is_2db_per_step() {
        for (i, &v) in VOLUME.iter().enumerate().take(15) {
            let expected = 2048.0 * 10f64.powf(-2.0 * i as f64 / 20.0);
            assert!((f64::from(v) - expected).abs() <= 0.5, "step {i}");
        }
    }

    #[test]
    fn tone_period_and_frequency() {
        let mut psg = Psg::new();
        set_tone(&mut psg, 0, 254);
        psg.write(0x90); // channel 0 volume: full
        assert_eq!(psg.tone[0], 254);
        // Count output toggles over many ticks: one every `period` ticks.
        let mut last = psg.output();
        let mut toggles = 0;
        let ticks = 254 * 200;
        for _ in 0..ticks {
            psg.tick();
            let out = psg.output();
            if out != last {
                toggles += 1;
            }
            last = out;
        }
        assert_eq!(toggles, 200);
        // 254 → 3 579 545 / (32 × 254) ≈ 440.4 Hz.
        let freq = 53_693_175.0 / 15.0 / (32.0 * 254.0);
        assert!((freq - 440.4_f64).abs() < 0.1);
    }

    #[test]
    fn period_zero_acts_as_one_and_is_constant() {
        let mut psg = Psg::new();
        set_tone(&mut psg, 1, 0);
        psg.write(0xB0);
        for _ in 0..10 {
            psg.tick();
            assert_eq!(psg.counters[1], 1);
            assert_eq!(psg.output(), 2048); // only channel 1 is audible, held high
        }
    }

    #[test]
    fn white_noise_lfsr_period() {
        let mut psg = Psg::new();
        psg.write(0xE4); // white noise, rate N/512
        assert_eq!(psg.lfsr, 0x8000);
        let mut lfsr = psg.lfsr;
        let mut period = 0u32;
        loop {
            let fb = (lfsr ^ (lfsr >> 3)) & 1;
            lfsr = (lfsr >> 1) | (fb << 15);
            period += 1;
            if lfsr == 0x8000 {
                break;
            }
        }
        assert_eq!(period, 57_337);
    }

    #[test]
    fn noise_sequence_and_reset() {
        let mut psg = Psg::new();
        psg.write(0xE4); // white, reload 16
        // Shift rate: one shift per 2 × 16 ticks.
        let mut shifts = 0;
        let mut last = psg.lfsr;
        for _ in 0..32 * 10 {
            psg.tick();
            if psg.lfsr != last {
                shifts += 1;
                last = psg.lfsr;
            }
        }
        assert_eq!(shifts, 10);
        // First shifts from 0x8000 with taps 0 and 3.
        let mut expected = 0x8000u16;
        for _ in 0..10 {
            expected = (expected >> 1) | (((expected ^ (expected >> 3)) & 1) << 15);
        }
        assert_eq!(psg.lfsr, expected);
        // Periodic noise rotates a single bit: period 16.
        psg.write(0xE0);
        assert_eq!(psg.lfsr, 0x8000);
        for _ in 0..16 * 32 {
            psg.tick();
        }
        assert_eq!(psg.lfsr, 0x8000);
        // A data byte to the latched noise register also resets the LFSR.
        psg.tick();
        psg.write(0x03);
        assert_eq!((psg.noise, psg.lfsr), (3, 0x8000));
    }

    #[test]
    fn noise_uses_tone3_period() {
        let mut psg = Psg::new();
        set_tone(&mut psg, 2, 100);
        psg.write(0xE7);
        let mut shifts = 0;
        let mut last = psg.lfsr;
        for _ in 0..200 * 5 {
            psg.tick();
            if psg.lfsr != last {
                shifts += 1;
                last = psg.lfsr;
            }
        }
        assert_eq!(shifts, 5);
    }

    #[test]
    fn savestate_round_trip() {
        use gase_savestate::{Reader, State, Writer};
        let mut psg = Psg::new();
        set_tone(&mut psg, 0, 300);
        psg.write(0x92);
        psg.write(0xE5);
        psg.write(0xF0);
        for _ in 0..1234 {
            psg.tick();
        }
        let mut w = Writer::new();
        psg.save(&mut w);
        let bytes = w.into_bytes();
        let mut copy = Psg::new();
        copy.load(&mut Reader::new(&bytes)).unwrap();
        for _ in 0..5000 {
            psg.tick();
            copy.tick();
            assert_eq!(psg.output(), copy.output());
        }
    }

    #[test]
    fn muted_channels_are_silent() {
        let mut psg = Psg::new();
        psg.write(0x90); // tone 1 full volume
        let loud = psg.output();
        psg.set_muted_channels(0x01);
        // Tone 1 no longer contributes; the other (silent) channels remain.
        assert_eq!(psg.output(), loud - 2048);
        psg.set_muted_channels(0x0F);
        assert_eq!(psg.output(), 0);
        psg.reset();
        assert_eq!(psg.muted_channels(), 0x0F);
    }
}
