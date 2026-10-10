use super::*;
use gase_savestate::{Reader, State, Writer};

/// Write a register through the bus interface, like a CPU would.
fn reg(ym: &mut Ym2612, part: u8, address: u8, value: u8) {
    ym.write(part * 2, address);
    ym.write(part * 2 + 1, value);
}

/// Configure `ch` (0-5) as algorithm 7 with only S4 audible: a pure sine.
fn sine_patch(ym: &mut Ym2612, ch: u8) {
    let (part, c) = (ch / 3, ch % 3);
    reg(ym, part, 0xB0 + c, 0x07); // algorithm 7, no feedback
    reg(ym, part, 0xB4 + c, 0xC0); // left + right
    for slot in 0..4u8 {
        let o = c + slot * 4;
        reg(ym, part, 0x30 + o, 0x01); // DT 0, MUL 1
        reg(ym, part, 0x40 + o, if slot == 3 { 0 } else { 0x7F }); // only S4 audible
        reg(ym, part, 0x50 + o, 0x1F); // AR 31
        reg(ym, part, 0x60 + o, 0x00); // D1R 0
        reg(ym, part, 0x70 + o, 0x00); // D2R 0
        reg(ym, part, 0x80 + o, 0x0F); // SL 0, RR 15
        reg(ym, part, 0x90 + o, 0x00);
    }
}

fn set_freq(ym: &mut Ym2612, ch: u8, fnum: u16, block: u8) {
    let (part, c) = (ch / 3, ch % 3);
    reg(ym, part, 0xA4 + c, (block << 3) | (fnum >> 8) as u8);
    reg(ym, part, 0xA0 + c, fnum as u8);
}

fn key(ym: &mut Ym2612, ch: u8, ops: u8) {
    let code = if ch < 3 { ch } else { ch + 1 };
    reg(ym, 0, 0x28, (ops << 4) | code);
}

fn run(ym: &mut Ym2612, n: usize) -> Vec<i32> {
    (0..n).map(|_| ym.clock_sample().0).collect()
}

fn rising_zero_crossings(samples: &[i32]) -> usize {
    samples.windows(2).filter(|w| w[0] < 0 && w[1] >= 0).count()
}

#[test]
fn algorithm7_sine_has_the_expected_frequency() {
    let mut ym = Ym2612::new();
    ym.set_ladder_effect(false);
    sine_patch(&mut ym, 0);
    // fnum 1000, block 4: increment = (2000 << 4) >> 2 = 8000 per sample,
    // i.e. a period of 2^20 / 8000 = 131.07 samples (406.4 Hz NTSC).
    set_freq(&mut ym, 0, 1000, 4);
    key(&mut ym, 0, 0xF);
    let samples = run(&mut ym, 131_072);
    let cycles = rising_zero_crossings(&samples);
    assert!((999..=1001).contains(&cycles), "{cycles} cycles");
    // Full volume: the 9-bit DAC peak is 255 → 255 × 32.
    assert_eq!(*samples.iter().max().unwrap(), 255 * 32);
    assert_eq!(*samples.iter().min().unwrap(), -256 * 32);
}

#[test]
fn multiple_and_block_scale_the_frequency() {
    let count = |mul: u8, block: u8| {
        let mut ym = Ym2612::new();
        ym.set_ladder_effect(false);
        sine_patch(&mut ym, 1);
        reg(&mut ym, 0, 0x3D, mul); // S4 of channel 2
        set_freq(&mut ym, 1, 1000, block);
        key(&mut ym, 1, 0xF);
        rising_zero_crossings(&run(&mut ym, 131_072))
    };
    let base = count(1, 3);
    assert!((499..=501).contains(&base), "{base}");
    assert!((249..=251).contains(&count(0, 3))); // MUL 0 = ×½
    assert!((1499..=1501).contains(&count(3, 3)));
    assert!((999..=1001).contains(&count(1, 4)));
}

#[test]
fn key_off_envelope_reaches_silence() {
    let mut ym = Ym2612::new();
    ym.set_ladder_effect(false);
    sine_patch(&mut ym, 3);
    set_freq(&mut ym, 3, 600, 4);
    key(&mut ym, 3, 0xF);
    let on = run(&mut ym, 1000);
    assert!(on.iter().any(|&s| s.abs() > 8000));
    key(&mut ym, 3, 0);
    let off = run(&mut ym, 2000);
    // RR 15 (rate 62+): 1023 steps at 8 per 3 samples ≈ 400 samples.
    assert!(off[..50].iter().any(|&s| s != 0));
    assert!(off[600..].iter().all(|&s| s == 0));
}

#[test]
fn slow_attack_rises_gradually() {
    let mut ym = Ym2612::new();
    ym.set_ladder_effect(false);
    sine_patch(&mut ym, 0);
    reg(&mut ym, 0, 0x5C, 0x0A); // S4 AR 10
    set_freq(&mut ym, 0, 1000, 4);
    key(&mut ym, 0, 0xF);
    let peaks: Vec<i32> = (0..8)
        .map(|_| run(&mut ym, 2000).iter().map(|s| s.abs()).max().unwrap())
        .collect();
    assert!(peaks.windows(2).all(|w| w[1] >= w[0]), "{peaks:?}");
    assert!(peaks[0] < peaks[7] / 2, "{peaks:?}");
}

#[test]
fn modulation_changes_the_waveform() {
    let mut plain = Ym2612::new();
    plain.set_ladder_effect(false);
    sine_patch(&mut plain, 0);
    set_freq(&mut plain, 0, 1000, 4);
    let mut fm = plain.clone();
    reg(&mut fm, 0, 0xB0, 0x00); // algorithm 0: S1→S2→S3→S4
    // Turn the three modulators up (TL registers: +0 = S1, +4 = S3, +8 = S2).
    for address in [0x40, 0x44, 0x48] {
        reg(&mut fm, 0, address, 0x10);
    }
    key(&mut plain, 0, 0xF);
    key(&mut fm, 0, 0xF);
    let a = run(&mut plain, 2000);
    let b = run(&mut fm, 2000);
    assert_ne!(a, b);
    // Same fundamental, but more zero crossings because of the harmonics.
    assert!(rising_zero_crossings(&b) > rising_zero_crossings(&a));
}

#[test]
fn timer_a_overflows_after_1024_minus_na_samples() {
    let mut ym = Ym2612::new();
    let na = 1024 - 10;
    reg(&mut ym, 0, 0x24, (na >> 2) as u8);
    reg(&mut ym, 0, 0x25, (na & 3) as u8);
    reg(&mut ym, 0, 0x27, 0x05); // load A, enable A flag
    for _ in 0..9 {
        ym.clock_sample();
        assert_eq!(ym.read_status() & 1, 0);
    }
    ym.clock_sample();
    assert_eq!(ym.read_status() & 3, 1);
    // Reset the flag; the timer keeps running with the same period.
    reg(&mut ym, 0, 0x27, 0x15);
    assert_eq!(ym.read_status() & 1, 0);
    run(&mut ym, 9);
    assert_eq!(ym.read_status() & 1, 0);
    ym.clock_sample();
    assert_eq!(ym.read_status() & 1, 1);
}

#[test]
fn timer_b_period_is_16_times_256_minus_nb() {
    let mut ym = Ym2612::new();
    reg(&mut ym, 0, 0x26, 254); // 2 × 16 = 32 samples
    reg(&mut ym, 0, 0x27, 0x0A); // load B, enable B flag
    let mut overflows = Vec::new();
    for n in 0..200 {
        ym.clock_sample();
        if ym.read_status() & 2 != 0 {
            overflows.push(n);
            reg(&mut ym, 0, 0x27, 0x2A);
        }
    }
    // The prescaler is free-running, so the first period may be short.
    assert!(overflows[0] < 32);
    assert!(
        overflows.windows(2).all(|w| w[1] - w[0] == 32),
        "{overflows:?}"
    );
}

#[test]
fn timer_flag_needs_enable_bit() {
    let mut ym = Ym2612::new();
    reg(&mut ym, 0, 0x24, 0xFF);
    reg(&mut ym, 0, 0x25, 0x03); // period 1
    reg(&mut ym, 0, 0x27, 0x01); // load only
    run(&mut ym, 10);
    assert_eq!(ym.read_status() & 1, 0);
}

#[test]
fn busy_flag_after_data_write() {
    let mut ym = Ym2612::new();
    ym.write(0, 0x30);
    assert_eq!(ym.read_status() & 0x80, 0);
    ym.write(1, 0x01);
    assert_eq!(ym.read_status() & 0x80, 0x80);
    ym.clock_sample();
    assert_eq!(ym.read_status() & 0x80, 0x80);
    ym.clock_sample();
    assert_eq!(ym.read_status() & 0x80, 0);
}

#[test]
fn dac_replaces_channel_6() {
    let mut ym = Ym2612::new();
    ym.set_ladder_effect(false);
    reg(&mut ym, 1, 0xB6, 0x80); // channel 6 left only
    reg(&mut ym, 0, 0x2B, 0x80);
    reg(&mut ym, 0, 0x2A, 0xFF);
    assert_eq!(ym.clock_sample(), (127 << 6, 0));
    reg(&mut ym, 0, 0x2A, 0x00);
    assert_eq!(ym.clock_sample(), (-128 << 6, 0));
    reg(&mut ym, 0, 0x2A, 0x80);
    assert_eq!(ym.clock_sample(), (0, 0));
    reg(&mut ym, 0, 0x2B, 0x00);
    reg(&mut ym, 0, 0x2A, 0xFF);
    assert_eq!(ym.clock_sample(), (0, 0));
}

#[test]
fn ladder_effect_offsets() {
    let mut ym = Ym2612::new();
    // Silent chip: every channel outputs 0 → +4 each, on both sides.
    assert_eq!(ym.clock_sample(), (6 * 4 * 32, 6 * 4 * 32));
    // DAC at −1 (9-bit −2) on channel 6, panned left only.
    reg(&mut ym, 1, 0xB6, 0x80);
    reg(&mut ym, 0, 0x2B, 0x80);
    reg(&mut ym, 0, 0x2A, 0x7F);
    let (l, r) = ym.clock_sample();
    assert_eq!(l, (5 * 4 + (-2 - 3)) * 32);
    assert_eq!(r, (5 * 4 - 4) * 32);
    ym.set_ladder_effect(false);
    assert_eq!(ym.clock_sample(), (-2 * 32, 0));
}

#[test]
fn channel3_special_mode_uses_per_operator_frequencies() {
    let mut ym = Ym2612::new();
    ym.set_ladder_effect(false);
    sine_patch(&mut ym, 2);
    // Make S1 the only audible operator instead of S4.
    reg(&mut ym, 0, 0x42, 0x00);
    reg(&mut ym, 0, 0x4E, 0x7F);
    set_freq(&mut ym, 2, 1000, 4); // normal frequency (used by S4)
    // S1 ← A9/AD: fnum 1000 (0x3E8), block 3 = half the normal frequency.
    reg(&mut ym, 0, 0xAD, (3 << 3) | 3);
    reg(&mut ym, 0, 0xA9, 0xE8);
    key(&mut ym, 2, 0xF);
    let mut normal = ym.clone();
    let n = rising_zero_crossings(&run(&mut normal, 131_072));
    assert!((999..=1001).contains(&n), "{n}");
    reg(&mut ym, 0, 0x27, 0x40); // special mode
    let s = rising_zero_crossings(&run(&mut ym, 131_072));
    assert!((499..=501).contains(&s), "{s}");
}

#[test]
fn csm_mode_keys_channel_3_on_timer_a() {
    let mut ym = Ym2612::new();
    ym.set_ladder_effect(false);
    sine_patch(&mut ym, 2);
    reg(&mut ym, 0, 0x8E, 0x0F); // S4 RR 15
    set_freq(&mut ym, 2, 1000, 4);
    reg(&mut ym, 0, 0x24, 0x00);
    reg(&mut ym, 0, 0x25, 0x00); // period 1024
    // Nothing keyed: silence.
    assert!(run(&mut ym, 100).iter().all(|&s| s == 0));
    reg(&mut ym, 0, 0x27, 0x81); // CSM + load A
    let out = run(&mut ym, 1100);
    assert!(out[..1023].iter().all(|&s| s == 0));
    // Keyed on for one sample, then released: a short blip.
    assert!(out[1023..1100].iter().any(|&s| s != 0));
    assert!(ym.channels[2].ops.iter().all(|op| !op.key));
}

#[test]
fn lfo_tremolo_modulates_amplitude() {
    let mut ym = Ym2612::new();
    ym.set_ladder_effect(false);
    sine_patch(&mut ym, 0);
    reg(&mut ym, 0, 0x6C, 0x80); // S4: AM on
    reg(&mut ym, 0, 0xB4, 0xF0); // AMS 3
    reg(&mut ym, 0, 0x22, 0x0F); // LFO on, fastest (≈ 69 Hz)
    set_freq(&mut ym, 0, 1000, 6);
    key(&mut ym, 0, 0xF);
    let out = run(&mut ym, 4000);
    let peaks: Vec<i32> = out
        .chunks(100)
        .map(|c| c.iter().map(|s| s.abs()).max().unwrap())
        .collect();
    let (lo, hi) = (*peaks.iter().min().unwrap(), *peaks.iter().max().unwrap());
    // 11.8 dB of tremolo: a ratio of about 3.9.
    assert!(hi > 3 * lo, "{lo} {hi}");
}

#[test]
fn lfo_vibrato_bends_pitch() {
    let mut a = Ym2612::new();
    a.set_ladder_effect(false);
    sine_patch(&mut a, 0);
    set_freq(&mut a, 0, 1000, 4);
    let mut b = a.clone();
    reg(&mut b, 0, 0xB4, 0xC7); // PMS 7
    reg(&mut b, 0, 0x22, 0x08);
    key(&mut a, 0, 0xF);
    key(&mut b, 0, 0xF);
    assert_ne!(run(&mut a, 20_000), run(&mut b, 20_000));
}

#[test]
fn ssg_eg_repeats_the_decay() {
    let mut ym = Ym2612::new();
    ym.set_ladder_effect(false);
    sine_patch(&mut ym, 0);
    reg(&mut ym, 0, 0x6C, 0x12); // S4 D1R 18: ~3000 samples per sweep
    reg(&mut ym, 0, 0x8C, 0xFF); // SL 15
    reg(&mut ym, 0, 0x9C, 0x08); // SSG-EG: repeated sawtooth
    set_freq(&mut ym, 0, 1000, 6);
    key(&mut ym, 0, 0xF);
    let peaks: Vec<i32> = (0..80)
        .map(|_| run(&mut ym, 200).iter().map(|s| s.abs()).max().unwrap())
        .collect();
    // The envelope keeps coming back: loud segments appear repeatedly.
    let loud = peaks.iter().filter(|&&p| p > 4000).count();
    let quiet = peaks.iter().filter(|&&p| p < 1000).count();
    assert!(loud >= 3 && quiet >= 3, "{peaks:?}");
}

#[test]
fn feedback_adds_harmonics() {
    let mut a = Ym2612::new();
    a.set_ladder_effect(false);
    sine_patch(&mut a, 0);
    reg(&mut a, 0, 0x40, 0x00); // S1 audible
    reg(&mut a, 0, 0x4C, 0x7F); // S4 silent
    set_freq(&mut a, 0, 1000, 4);
    let mut b = a.clone();
    reg(&mut b, 0, 0xB0, 0x07 | (7 << 3)); // feedback 7
    key(&mut a, 0, 0xF);
    key(&mut b, 0, 0xF);
    let (sa, sb) = (run(&mut a, 5000), run(&mut b, 5000));
    assert_ne!(sa, sb);
}

#[test]
fn savestate_round_trip() {
    let mut ym = Ym2612::new();
    sine_patch(&mut ym, 0);
    sine_patch(&mut ym, 4);
    reg(&mut ym, 0, 0x22, 0x0B);
    reg(&mut ym, 0, 0xB4, 0xF5);
    reg(&mut ym, 0, 0x6C, 0x85);
    reg(&mut ym, 0, 0x24, 0x80);
    reg(&mut ym, 0, 0x27, 0x0F);
    set_freq(&mut ym, 0, 700, 4);
    set_freq(&mut ym, 4, 900, 3);
    key(&mut ym, 0, 0xF);
    key(&mut ym, 4, 0xF);
    run(&mut ym, 777);

    let mut w = Writer::new();
    ym.save(&mut w);
    let bytes = w.into_bytes();
    let mut copy = Ym2612::new();
    copy.load(&mut Reader::new(&bytes)).unwrap();
    for _ in 0..5000 {
        assert_eq!(ym.clock_sample(), copy.clock_sample());
        assert_eq!(ym.read_status(), copy.read_status());
    }
}

#[test]
fn part_two_global_registers_are_ignored() {
    let mut ym = Ym2612::new();
    reg(&mut ym, 1, 0x2B, 0x80);
    assert!(!ym.dac_enabled);
    reg(&mut ym, 0, 0x2B, 0x80);
    assert!(ym.dac_enabled);
}

#[test]
fn muted_channels_leave_the_mix_but_keep_running() {
    let mut ym = Ym2612::new();
    ym.set_ladder_effect(false);
    reg(&mut ym, 1, 0xB6, 0x80); // channel 6 left only
    reg(&mut ym, 0, 0x2B, 0x80); // DAC on
    reg(&mut ym, 0, 0x2A, 0xFF);
    ym.set_muted_channels(1 << 5);
    assert_eq!(ym.clock_sample(), (0, 0));
    // Reset keeps the host setting.
    ym.reset();
    assert_eq!(ym.muted_channels(), 1 << 5);
    ym.set_ladder_effect(false);
    reg(&mut ym, 1, 0xB6, 0x80);
    reg(&mut ym, 0, 0x2B, 0x80);
    reg(&mut ym, 0, 0x2A, 0xFF);
    ym.set_muted_channels(0);
    assert_eq!(ym.clock_sample(), (127 << 6, 0));
}

/// A tiny deterministic pseudo-random generator for the tests below.
fn next_random(state: &mut u32) -> u32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    *state
}

/// A silent operator (see `Operator::is_silent`) with random settings.
fn random_silent_operator(seed: &mut u32) -> operator::Operator {
    let mut r = || next_random(seed);
    operator::Operator {
        dt: (r() & 7) as u8,
        mul: (r() & 15) as u8,
        tl: (r() & 0x7F) as u8,
        ks: (r() & 3) as u8,
        ar: (r() & 31) as u8,
        am: r() & 1 != 0,
        d1r: (r() & 31) as u8,
        d2r: (r() & 31) as u8,
        sl: (r() & 15) as u8,
        rr: (r() & 15) as u8,
        ssg: (r() & 15) as u8,
        phase: r() & 0xF_FFFF,
        inc: r() & 0x3_FFFF,
        kcode: (r() & 31) as u8,
        eg_phase: operator::RELEASE,
        level: operator::MAX_ATTENUATION,
        key_reg: false,
        key: false,
        ssg_inv: r() & 1 != 0,
    }
}

#[test]
fn envelope_tick_leaves_silent_operators_unchanged() {
    // The reason `eg_clock` may skip silent operators: the full tick
    // (`eg_step`) changes nothing on them, for any settings and counter.
    let mut seed = 0x1234_5678;
    for _ in 0..200 {
        let op = random_silent_operator(&mut seed);
        let mut stepped = op.clone();
        for counter in 1..4096 {
            stepped.eg_step(counter);
            assert_eq!(format!("{stepped:?}"), format!("{op:?}"));
        }
    }
}

#[test]
fn silent_channel_fast_path_matches_full_computation() {
    let roms = &*ROMS;
    let mut seed = 0x9E37_79B9;
    for _ in 0..2000 {
        let mut ch = channel::Channel::default();
        for op in &mut ch.ops {
            *op = random_silent_operator(&mut seed);
        }
        ch.algorithm = (next_random(&mut seed) & 7) as u8;
        ch.feedback = (next_random(&mut seed) & 7) as u8;
        ch.ams = (next_random(&mut seed) & 3) as u8;
        ch.op1_out = [next_random(&mut seed) as i16, next_random(&mut seed) as i16];
        ch.s2_out = next_random(&mut seed) as i16;
        let lfo_am = (next_random(&mut seed) & 0x7E) as u16;
        let mut reference = ch.clone();
        for _ in 0..4 {
            assert_eq!(
                ch.calc(roms, lfo_am),
                reference.calc_operators(roms, lfo_am)
            );
            assert_eq!(format!("{ch:?}"), format!("{reference:?}"));
        }
    }
}
