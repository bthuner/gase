//! Render a short tune with an FM patch and a PSG square wave into
//! `gase-tone.wav` (current directory), so you can listen to the chips.
//!
//! ```text
//! cargo run -p gase-sound --example tone
//! ```

use gase_sound::{
    DcBlocker, MASTER_CLOCK_NTSC, MASTER_CLOCKS_PER_PSG_TICK, MASTER_CLOCKS_PER_YM_SAMPLE, Psg,
    Resampler, Ym2612, ym_sample_rate,
};
use std::fs::File;
use std::io::{self, BufWriter, Write};

const OUTPUT_RATE: u32 = 44_100;

/// Write a YM2612 register (part 0 or 1).
fn reg(ym: &mut Ym2612, part: u8, address: u8, value: u8) {
    ym.write(part * 2, address);
    ym.write(part * 2 + 1, value);
}

/// A bright electric-piano-like patch on channel 1: algorithm 4 (two
/// modulator→carrier pairs), S1 with some feedback.
fn load_patch(ym: &mut Ym2612) {
    reg(ym, 0, 0xB0, (5 << 3) | 4); // feedback 5, algorithm 4
    reg(ym, 0, 0xB4, 0xC0); // centre
    // Per operator, in register order S1, S3, S2, S4:
    //               DT/MUL TL    KS/AR D1R  D2R  SL/RR
    let ops: [[u8; 6]; 4] = [
        [0x71, 0x23, 0x5F, 0x0A, 0x04, 0x37], // S1: modulator, ×1
        [0x0E, 0x2D, 0x5F, 0x0C, 0x04, 0x37], // S3: modulator, ×14 (the "tine")
        [0x31, 0x00, 0x1F, 0x08, 0x03, 0x27], // S2: carrier, ×1
        [0x02, 0x08, 0x1F, 0x09, 0x03, 0x27], // S4: carrier, ×2
    ];
    for (slot, op) in ops.iter().enumerate() {
        let o = slot as u8 * 4;
        for (i, &value) in op.iter().enumerate() {
            reg(ym, 0, 0x30 + 0x10 * i as u8 + o, value);
        }
    }
}

/// F-number and block for a frequency (MUL = 1), at the NTSC sample rate.
fn fm_pitch(freq: f64) -> (u16, u8) {
    let fs = ym_sample_rate(MASTER_CLOCK_NTSC);
    let mut block = 0u8;
    loop {
        let fnum = freq * f64::from(1u32 << 21) / (fs * f64::from(1u32 << block));
        if fnum < 2048.0 || block == 7 {
            return (fnum.round() as u16, block);
        }
        block += 1;
    }
}

fn note(semitones_from_a4: i32) -> f64 {
    440.0 * 2f64.powf(f64::from(semitones_from_a4) / 12.0)
}

fn main() -> io::Result<()> {
    let native_rate = ym_sample_rate(MASTER_CLOCK_NTSC);
    let mut ym = Ym2612::new();
    let mut psg = Psg::new();
    let mut resampler = Resampler::new(native_rate, f64::from(OUTPUT_RATE));
    resampler.set_gain(1.0);
    let mut dc = DcBlocker::new(10.0, native_rate);
    load_patch(&mut ym);

    // A little arpeggio: FM plays the melody, the PSG an octave-up echo.
    let melody = [-9, -5, -2, 3, 7, 3, -2, -5, -9, -4, 0, 3, 8, 3, 0, -4];
    let note_samples = (native_rate * 0.25) as usize;
    let mut pcm = Vec::new();
    let (mut master, mut psg_next) = (0u64, 0u64);

    for (i, &n) in melody.iter().enumerate() {
        // FM: key off, set pitch, key on (all four operators).
        reg(&mut ym, 0, 0x28, 0x00);
        let (fnum, block) = fm_pitch(note(n));
        reg(&mut ym, 0, 0xA4, (block << 3) | (fnum >> 8) as u8);
        reg(&mut ym, 0, 0xA0, fnum as u8);
        reg(&mut ym, 0, 0x28, 0xF0);

        // PSG tone 1, every other note, at attenuation 4 (−8 dB).
        let period = (f64::from(MASTER_CLOCK_NTSC) / 15.0 / (32.0 * note(n + 12))).round() as u16;
        psg.write(0x80 | (period & 0x0F) as u8);
        psg.write((period >> 4) as u8 & 0x3F);
        psg.write(if i % 2 == 0 { 0x94 } else { 0x9F });

        for k in 0..note_samples {
            if k == note_samples * 3 / 4 {
                reg(&mut ym, 0, 0x28, 0x00); // release before the next note
            }
            // Average the PSG over the ticks that fall in this YM sample.
            master += u64::from(MASTER_CLOCKS_PER_YM_SAMPLE);
            let (mut sum, mut ticks) = (0i32, 0i32);
            while psg_next < master {
                psg.tick();
                sum += psg.output();
                ticks += 1;
                psg_next += u64::from(MASTER_CLOCKS_PER_PSG_TICK);
            }
            let psg_level = if ticks > 0 { sum / ticks } else { psg.output() };
            let (l, r) = ym.clock_sample();
            let (l, r) = dc.process(l + psg_level, r + psg_level);
            resampler.push(l, r);
        }
        resampler.drain(&mut pcm);
    }
    // Let the last release ring out.
    for _ in 0..(native_rate * 0.5) as usize {
        let (l, r) = ym.clock_sample();
        let (l, r) = dc.process(l, r);
        resampler.push(l, r);
    }
    resampler.drain(&mut pcm);

    let path = "gase-tone.wav";
    write_wav(path, OUTPUT_RATE, &pcm)?;
    println!(
        "wrote {path}: {:.2} s at {OUTPUT_RATE} Hz",
        pcm.len() as f64 / 2.0 / f64::from(OUTPUT_RATE)
    );
    Ok(())
}

/// Minimal RIFF/WAVE writer: 16-bit PCM, stereo, interleaved samples.
fn write_wav(path: &str, rate: u32, samples: &[i16]) -> io::Result<()> {
    let mut f = BufWriter::new(File::create(path)?);
    let channels: u16 = 2;
    let bits: u16 = 16;
    let block_align = channels * bits / 8;
    let data_len = (samples.len() * 2) as u32;
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + data_len).to_le_bytes())?;
    f.write_all(b"WAVE")?;
    // "fmt " chunk: PCM format description.
    f.write_all(b"fmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?; // PCM
    f.write_all(&channels.to_le_bytes())?;
    f.write_all(&rate.to_le_bytes())?;
    f.write_all(&(rate * u32::from(block_align)).to_le_bytes())?; // byte rate
    f.write_all(&block_align.to_le_bytes())?;
    f.write_all(&bits.to_le_bytes())?;
    // "data" chunk.
    f.write_all(b"data")?;
    f.write_all(&data_len.to_le_bytes())?;
    for s in samples {
        f.write_all(&s.to_le_bytes())?;
    }
    f.flush()
}
