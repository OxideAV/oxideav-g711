//! PSNR floor regressions for the G.711 µ-law / A-law round-trip.
//!
//! G.711 is a 13/14-bit non-uniform quantizer whose published quality
//! floor for voice-band tones is in the mid-30s dB SQNR / PSNR range —
//! a well-known number repeated across the ITU-T G.711 staged
//! Recommendation and downstream interop guides (the staged spec frames
//! it as "the companding curve provides an SQNR independent of input
//! level over a 40 dB dynamic range, approximately 38 dB", which in
//! peak-to-noise PSNR terms is consistently > 35 dB across the
//! voice-band tones we test here).
//!
//! These tests synthesise a 1-second sinusoid at three standard
//! telephony test frequencies (400 Hz, 1 kHz, 2 kHz) at -3 dBFS,
//! round-trip it through encode→decode, and assert the resulting PSNR
//! sits above a 35 dB floor — a conservative envelope that's
//! comfortably above the spec floor but tight enough to catch any
//! regression that would knock multiple bits off the curve. Failing
//! this floor in a future change indicates the encoder no longer
//! sits on the spec's companding curve.
//!
//! ## Why not SNR / SQNR?
//!
//! PSNR is `20·log10(PEAK / RMS_error)` where PEAK is the peak of the
//! S16 range (32767). It's the right metric for "how close to a
//! reconstructed signal" and it's what the rest of the workspace's
//! lossy codecs report. SQNR (signal / noise) is the equivalent for
//! pure-tone testing and is reported alongside PSNR for the human
//! reader's sake.

use std::f64::consts::PI;

use oxideav_g711::{alaw, mulaw};

/// S16 full-scale (peak amplitude).
const S16_PEAK: f64 = 32767.0;

/// PSNR floor we assert at. The companded SQNR floor cited in the
/// staged G.711 Recommendation §1 introduction is ≈ 38 dB across the
/// codec's design dynamic range for voice-band tones; 35 dB gives 3 dB
/// of regression-detection margin while still requiring the encoder
/// stays on its curve.
const PSNR_FLOOR_DB: f64 = 35.0;

/// Synthesise `n` samples of a sine wave at the given frequency, sample
/// rate, and amplitude (in S16 LSBs). Returns the interleaved S16
/// vector.
fn synth_sine(freq_hz: f64, sample_rate: u32, amplitude: f64, n: usize) -> Vec<i16> {
    let sr = sample_rate as f64;
    (0..n)
        .map(|i| {
            let t = i as f64 / sr;
            let v = amplitude * (2.0 * PI * freq_hz * t).sin();
            v.round().clamp(i16::MIN as f64, i16::MAX as f64) as i16
        })
        .collect()
}

/// Compute PSNR (dB) for a reference / reconstructed pair. Returns
/// `f64::INFINITY` if the two are bit-identical.
fn psnr_db(reference: &[i16], reconstructed: &[i16]) -> f64 {
    assert_eq!(
        reference.len(),
        reconstructed.len(),
        "PSNR inputs must be the same length"
    );
    let n = reference.len() as f64;
    let sse: f64 = reference
        .iter()
        .zip(reconstructed.iter())
        .map(|(&r, &q)| {
            let d = r as f64 - q as f64;
            d * d
        })
        .sum();
    if sse == 0.0 {
        return f64::INFINITY;
    }
    let mse = sse / n;
    20.0 * (S16_PEAK / mse.sqrt()).log10()
}

/// Compute SQNR (dB) — signal-to-noise ratio for the sine input. Same
/// as PSNR but normalised by the signal's own RMS instead of the S16
/// peak. Reported alongside PSNR for human inspection.
fn sqnr_db(signal: &[i16], reconstructed: &[i16]) -> f64 {
    let n = signal.len() as f64;
    let sse: f64 = signal
        .iter()
        .zip(reconstructed.iter())
        .map(|(&s, &q)| {
            let d = s as f64 - q as f64;
            d * d
        })
        .sum();
    let sig_pow: f64 = signal.iter().map(|&s| (s as f64).powi(2)).sum();
    if sse == 0.0 {
        return f64::INFINITY;
    }
    let signal_rms = (sig_pow / n).sqrt();
    let noise_rms = (sse / n).sqrt();
    20.0 * (signal_rms / noise_rms).log10()
}

/// One PSNR measurement for a given law + frequency. Asserts the floor.
fn measure_and_assert(
    label: &str,
    freq_hz: f64,
    encode: fn(i16) -> u8,
    decode: fn(u8) -> i16,
) -> f64 {
    let sample_rate = 8_000u32;
    let n = sample_rate as usize; // 1 second
                                  // -3 dBFS amplitude: 32767 * 10^(-3/20) ≈ 23197.
    let amplitude = S16_PEAK * 10.0_f64.powf(-3.0 / 20.0);
    let signal = synth_sine(freq_hz, sample_rate, amplitude, n);
    let reconstructed: Vec<i16> = signal.iter().map(|&s| decode(encode(s))).collect();
    let p = psnr_db(&signal, &reconstructed);
    let s = sqnr_db(&signal, &reconstructed);
    eprintln!(
        "{label} @ {freq_hz:.0} Hz, -3 dBFS, 1 s @ {sample_rate} Hz: PSNR = {p:.2} dB, SQNR = {s:.2} dB"
    );
    assert!(
        p >= PSNR_FLOOR_DB,
        "{label} @ {freq_hz} Hz fell below PSNR floor: {p:.2} dB < {PSNR_FLOOR_DB} dB"
    );
    p
}

// ---------------------------------------------------------------------------
// µ-law PSNR floors
// ---------------------------------------------------------------------------

#[test]
fn mulaw_psnr_floor_400hz() {
    measure_and_assert("µ-law", 400.0, mulaw::encode_sample, mulaw::decode_sample);
}

#[test]
fn mulaw_psnr_floor_1khz() {
    measure_and_assert("µ-law", 1000.0, mulaw::encode_sample, mulaw::decode_sample);
}

#[test]
fn mulaw_psnr_floor_2khz() {
    measure_and_assert("µ-law", 2000.0, mulaw::encode_sample, mulaw::decode_sample);
}

// ---------------------------------------------------------------------------
// A-law PSNR floors
// ---------------------------------------------------------------------------

#[test]
fn alaw_psnr_floor_400hz() {
    measure_and_assert("A-law", 400.0, alaw::encode_sample, alaw::decode_sample);
}

#[test]
fn alaw_psnr_floor_1khz() {
    measure_and_assert("A-law", 1000.0, alaw::encode_sample, alaw::decode_sample);
}

#[test]
fn alaw_psnr_floor_2khz() {
    measure_and_assert("A-law", 2000.0, alaw::encode_sample, alaw::decode_sample);
}

// ---------------------------------------------------------------------------
// Cross-law floor comparison: both laws should sit within a few dB of
// each other at the same input — they target the same SQNR by design.
// ---------------------------------------------------------------------------

#[test]
fn mulaw_and_alaw_psnr_are_within_5db_at_1khz() {
    let mu = measure_and_assert("µ-law", 1000.0, mulaw::encode_sample, mulaw::decode_sample);
    let a = measure_and_assert("A-law", 1000.0, alaw::encode_sample, alaw::decode_sample);
    let delta = (mu - a).abs();
    assert!(
        delta < 5.0,
        "µ-law and A-law PSNR diverged by {delta:.2} dB at 1 kHz \
         (µ-law {mu:.2}, A-law {a:.2}); expected < 5 dB"
    );
}
