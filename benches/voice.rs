//! Criterion benchmarks for the G.711 hot paths under a
//! **voice-realistic** input distribution.
//!
//! Round 247 (depth-mode benchmarks, voice-distribution follow-up to
//! r173 / r206): the existing four bench files (`decode`, `encode`,
//! `roundtrip`, `streaming`) all synthesise their input from a
//! deterministic xorshift32 RNG, which spreads samples roughly
//! uniformly across the i16 range. That's a good worst-case stress
//! for the encode segment-search loop and for full-LUT cache
//! coverage, but it is **not** what a real PSTN session sees on the
//! wire. ITU-T G.711 is specified for 8 kHz / 8-bit voice
//! transmission; voice content is well-known to be concentrated
//! near zero amplitude with occasional larger excursions — the kind
//! of signed-amplitude distribution that produces a roughly
//! exponential / Laplacian magnitude profile (loudly-asserted
//! speech is mostly silence and small consonant energy with vowel
//! peaks). G.711's logarithmic companding curve is in fact
//! optimised for exactly this property — small-magnitude segments
//! have the finest quantisation step (1 LSB for A-law seg-0,
//! 2 LSB for µ-law seg-0) while loud segments share larger steps.
//!
//! The two distributions therefore stress complementary parts of
//! the codec:
//!
//! - **uniform**: every segment of the curve sees equal traffic,
//!   the encode LUT's 64 KiB working set is fully exercised every
//!   pass, and the decode LUT's 256-entry working set is touched
//!   uniformly. This is what the r173 / r206 benches measure and
//!   it is the canonical worst-case for cache-pressure regressions.
//! - **voice (Laplacian-near-zero)**: ~80% of samples land in
//!   segments 0..=2 (magnitudes ≤ 1024), so the encode LUT touches
//!   primarily the **first 4 KiB of each half** of the table
//!   (positive + negative halves, indexed via the `s as u16 as
//!   usize` cast — small positive sample indices land at the start,
//!   small negative samples at the very end via the two's-
//!   complement wrap). The decode LUT touches mostly the same
//!   16-byte block. A regression that splits a hot cache line in
//!   half — e.g. a future SIMD gather that pulls non-contiguous
//!   entries — would show up disproportionately here.
//!
//! Both distributions hit the per-sample math at the same arithmetic
//! cost (the LUT load and the segment-search loop are both O(1) per
//! sample), so the *ratio* between uniform and voice rows isolates
//! the **memory-system contribution** to the per-sample wall time.
//! On most hosts the rows land within a few percent of each other;
//! a future change that introduces a meaningful spread is worth
//! investigating.
//!
//! ## Voice-distribution synthesis
//!
//! We approximate a discrete Laplacian distribution centred at
//! zero with a small closed-form generator that does not depend on
//! any external numeric library:
//!
//! 1. Draw a uniform `u` ∈ (0, 1) from a xorshift32 stream.
//! 2. Compute the signed log-transform `s = sign · floor(scale ·
//!    ln(1 - |2u - 1|))` clipped to the i16 range.
//!
//! The `scale` parameter governs how concentrated the distribution
//! is. We pick `scale = 800` so the resulting magnitude
//! distribution lands ~80% of samples in |s| ≤ 1024 (segments
//! 0..=2 in both laws), ~95% in |s| ≤ 8192 (segments 0..=5),
//! matching the rough shape that the spec-recommended `s` formula
//! for a generic voice signal exhibits.
//!
//! ## Scenarios
//!
//! - **voice_decode_mulaw_lut_8k_1s** — 1 s of µ-law bytes (encoded
//!   from a voice-distributed i16 stream so the decode LUT sees
//!   the concentrated codeword distribution voice content actually
//!   produces). Indexes [`oxideav_g711::mulaw::decode_sample`] per
//!   sample.
//! - **voice_decode_alaw_lut_8k_1s** — same shape, A-law.
//! - **voice_encode_mulaw_lut_8k_1s** — 1 s of voice-distributed
//!   i16 PCM through [`oxideav_g711::mulaw::encode_sample`] (the
//!   r230 64 KiB compile-time LUT). Stresses only the low-magnitude
//!   half of the table on most samples.
//! - **voice_encode_alaw_lut_8k_1s** — same shape, A-law.
//! - **voice_encode_mulaw_arith_8k_1s** — the formula path
//!   ([`oxideav_g711::mulaw::encode_sample_arith`]) on the same
//!   voice-distributed input. The arith loop's segment search hits
//!   the segment-0 fast exit on ~80% of samples, so this row
//!   measures the segment-0-heavy branch profile (vs. the
//!   uniform-random row's even-segment-distribution profile).
//! - **voice_encode_alaw_arith_8k_1s** — same shape, A-law (the
//!   A-law arith path has its own segment-0 short-circuit which the
//!   voice distribution exercises heavily).
//! - **voice_roundtrip_mulaw_mono_8k_1s** — full encoder + decoder
//!   pair through the trait surface on a voice-distributed
//!   1 s mono input. Mirrors `roundtrip_mulaw_mono_8k_1s` from the
//!   r173 bench file, swapping the input distribution. Useful for
//!   the headline "what does a real PSTN session pay" number.
//! - **voice_roundtrip_alaw_mono_8k_1s** — same shape, A-law.
//!
//! ## Provenance
//!
//! Every input is synthesised in-bench from deterministic seeds.
//! No `docs/` fixtures or external files are read; no audio
//! corpora; no probability-distribution crates. The Laplacian
//! generator is a closed-form transform of a uniform RNG, written
//! from scratch in this file.
//!
//! Run with:
//!     cargo bench -p oxideav-g711 --bench voice

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use oxideav_core::{AudioFrame, CodecId, CodecParameters, Frame, SampleFormat};
use oxideav_g711::{alaw, mulaw};

/// Deterministic xorshift32 — matches the generator the other bench
/// files use so a voice-distribution row and a uniform-distribution
/// row at the same seed share an underlying RNG stream.
fn xorshift32(state: &mut u32) -> u32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    *state
}

/// Voice-distribution scale parameter — see the file header docs
/// for the rationale. 800 was picked so the resulting magnitude
/// distribution lands ~80% of samples in `|s| <= 1024` (segments
/// 0..=2 in both laws), which matches the rough envelope of
/// continuous-voice content on the PSTN.
const VOICE_SCALE: f64 = 800.0;

/// Draw one voice-distributed i16 sample from the supplied
/// xorshift32 state. Implements the inverse-CDF of a discrete
/// Laplacian centred at zero:
///
/// 1. Map the 32-bit xorshift output to a uniform `u` ∈ (0, 1) by
///    dividing by 2^32 (with a tiny offset so we never hit the
///    degenerate `2u - 1 == ±1` endpoint that would feed `ln(0)`).
/// 2. Apply the inverse-CDF `sign(2u - 1) · scale · ln(1 - |2u -
///    1|)`.
/// 3. Clip to the i16 range (the only samples that would otherwise
///    fall outside i16 are the deep tail; clipping at ±32767 is
///    representative of a real ADC saturating).
fn voice_sample(state: &mut u32) -> i16 {
    // Two xorshift draws give us a 64-bit-precision uniform, which
    // helps the tail of the Laplacian look smooth at sub-i16
    // granularity. (One 32-bit draw also works but the LSB stairs
    // are visible in a histogram.)
    let hi = xorshift32(state) as u64;
    let lo = xorshift32(state) as u64;
    let mantissa = (hi << 32) | lo;
    // Map to (eps, 1 - eps). The 53-bit mantissa is what f64 can
    // hold without rounding; we mask down to that. The `+ 1` keeps
    // the bottom away from exact zero so `ln(0)` cannot fire.
    let bits = (mantissa >> 11) & ((1u64 << 53) - 1);
    let u = (bits as f64 + 1.0) / ((1u64 << 53) as f64 + 2.0);

    let centred = 2.0 * u - 1.0;
    let abs_c = centred.abs();
    // 1 - |2u - 1| ∈ (0, 1), never exactly 0 thanks to the `+ 1` /
    // `+ 2` shift above, so `ln` is always finite.
    let magnitude = -VOICE_SCALE * (1.0 - abs_c).ln();
    let signed = if centred < 0.0 { -magnitude } else { magnitude };

    // Clamp to i16 — the deep Laplacian tail wants to overflow.
    if signed >= i16::MAX as f64 {
        i16::MAX
    } else if signed <= i16::MIN as f64 {
        i16::MIN
    } else {
        signed as i16
    }
}

/// Build `n` voice-distributed S16 PCM samples seeded by `seed`.
fn build_voice_pcm(n: usize, seed: u32) -> Vec<i16> {
    let mut state = seed.max(1);
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push(voice_sample(&mut state));
    }
    out
}

/// Build `n` µ-law bytes from voice-distributed S16 input (so the
/// decode bench sees the codeword distribution voice actually
/// produces, not a uniform one over 0..=255).
fn build_voice_mulaw_bytes(n: usize, seed: u32) -> Vec<u8> {
    build_voice_pcm(n, seed)
        .into_iter()
        .map(mulaw::encode_sample)
        .collect()
}

/// Build `n` A-law bytes from voice-distributed S16 input.
fn build_voice_alaw_bytes(n: usize, seed: u32) -> Vec<u8> {
    build_voice_pcm(n, seed)
        .into_iter()
        .map(alaw::encode_sample)
        .collect()
}

/// Convert an i16 buffer into the little-endian byte form the
/// trait-surface Encoder expects.
fn pcm_le_bytes(pcm: &[i16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(pcm.len() * 2);
    for &s in pcm {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

fn bench_voice_decode_mulaw_lut_8k_1s(c: &mut Criterion) {
    let n = 8_000;
    let bytes = build_voice_mulaw_bytes(n, 0xCAFE_F00D);
    let mut g = c.benchmark_group("voice_decode_mulaw_lut_8k_1s");
    g.throughput(Throughput::Bytes(n as u64));
    g.bench_function(BenchmarkId::from_parameter("mulaw/voice/lut/8k/1s"), |b| {
        b.iter(|| {
            let src = criterion::black_box(&bytes);
            let mut acc: i32 = 0;
            for &byte in src {
                acc = acc.wrapping_add(mulaw::decode_sample(byte) as i32);
            }
            criterion::black_box(acc)
        });
    });
    g.finish();
}

fn bench_voice_decode_alaw_lut_8k_1s(c: &mut Criterion) {
    let n = 8_000;
    let bytes = build_voice_alaw_bytes(n, 0xBADC_0FFE);
    let mut g = c.benchmark_group("voice_decode_alaw_lut_8k_1s");
    g.throughput(Throughput::Bytes(n as u64));
    g.bench_function(BenchmarkId::from_parameter("alaw/voice/lut/8k/1s"), |b| {
        b.iter(|| {
            let src = criterion::black_box(&bytes);
            let mut acc: i32 = 0;
            for &byte in src {
                acc = acc.wrapping_add(alaw::decode_sample(byte) as i32);
            }
            criterion::black_box(acc)
        });
    });
    g.finish();
}

fn bench_voice_encode_mulaw_lut_8k_1s(c: &mut Criterion) {
    let n = 8_000;
    let pcm = build_voice_pcm(n, 0xC0DE_BABE);
    let mut g = c.benchmark_group("voice_encode_mulaw_lut_8k_1s");
    g.throughput(Throughput::Bytes((n * 2) as u64));
    g.bench_function(BenchmarkId::from_parameter("mulaw/voice/lut/8k/1s"), |b| {
        b.iter(|| {
            let src = criterion::black_box(&pcm);
            let mut acc: u32 = 0;
            for &s in src {
                acc = acc.wrapping_add(mulaw::encode_sample(s) as u32);
            }
            criterion::black_box(acc)
        });
    });
    g.finish();
}

fn bench_voice_encode_alaw_lut_8k_1s(c: &mut Criterion) {
    let n = 8_000;
    let pcm = build_voice_pcm(n, 0xC001_D00D);
    let mut g = c.benchmark_group("voice_encode_alaw_lut_8k_1s");
    g.throughput(Throughput::Bytes((n * 2) as u64));
    g.bench_function(BenchmarkId::from_parameter("alaw/voice/lut/8k/1s"), |b| {
        b.iter(|| {
            let src = criterion::black_box(&pcm);
            let mut acc: u32 = 0;
            for &s in src {
                acc = acc.wrapping_add(alaw::encode_sample(s) as u32);
            }
            criterion::black_box(acc)
        });
    });
    g.finish();
}

fn bench_voice_encode_mulaw_arith_8k_1s(c: &mut Criterion) {
    let n = 8_000;
    let pcm = build_voice_pcm(n, 0xC0DE_BABE);
    let mut g = c.benchmark_group("voice_encode_mulaw_arith_8k_1s");
    g.throughput(Throughput::Bytes((n * 2) as u64));
    g.bench_function(
        BenchmarkId::from_parameter("mulaw/voice/arith/8k/1s"),
        |b| {
            b.iter(|| {
                let src = criterion::black_box(&pcm);
                let mut acc: u32 = 0;
                for &s in src {
                    acc = acc.wrapping_add(mulaw::encode_sample_arith(s) as u32);
                }
                criterion::black_box(acc)
            });
        },
    );
    g.finish();
}

fn bench_voice_encode_alaw_arith_8k_1s(c: &mut Criterion) {
    let n = 8_000;
    let pcm = build_voice_pcm(n, 0xC001_D00D);
    let mut g = c.benchmark_group("voice_encode_alaw_arith_8k_1s");
    g.throughput(Throughput::Bytes((n * 2) as u64));
    g.bench_function(BenchmarkId::from_parameter("alaw/voice/arith/8k/1s"), |b| {
        b.iter(|| {
            let src = criterion::black_box(&pcm);
            let mut acc: u32 = 0;
            for &s in src {
                acc = acc.wrapping_add(alaw::encode_sample_arith(s) as u32);
            }
            criterion::black_box(acc)
        });
    });
    g.finish();
}

fn params(id: &str, channels: u16, sample_rate: u32) -> CodecParameters {
    let mut p = CodecParameters::audio(CodecId::new(id));
    p.sample_rate = Some(sample_rate);
    p.channels = Some(channels);
    p.sample_format = Some(SampleFormat::S16);
    p
}

fn bench_voice_roundtrip_mulaw_mono_8k_1s(c: &mut Criterion) {
    let n = 8_000;
    let pcm = build_voice_pcm(n, 0xFACE_F00D);
    let bytes = pcm_le_bytes(&pcm);
    let mut g = c.benchmark_group("voice_roundtrip_mulaw_mono_8k_1s");
    g.throughput(Throughput::Bytes((n * 2) as u64));
    g.bench_function(
        BenchmarkId::from_parameter("mulaw/voice/roundtrip/mono/8k/1s"),
        |b| {
            b.iter(|| {
                let p = params("pcm_mulaw", 1, 8_000);
                let mut enc = mulaw::make_encoder(&p).expect("make_encoder");
                let mut dec = mulaw::make_decoder(&p).expect("make_decoder");
                let frame = Frame::Audio(AudioFrame {
                    samples: n as u32,
                    pts: Some(0),
                    data: vec![bytes.clone()],
                });
                enc.send_frame(&frame).expect("send_frame");
                let pkt = enc.receive_packet().expect("receive_packet");
                dec.send_packet(&pkt).expect("send_packet");
                let out = dec.receive_frame().expect("receive_frame");
                criterion::black_box(out);
            });
        },
    );
    g.finish();
}

fn bench_voice_roundtrip_alaw_mono_8k_1s(c: &mut Criterion) {
    let n = 8_000;
    let pcm = build_voice_pcm(n, 0xABAD_1DEA);
    let bytes = pcm_le_bytes(&pcm);
    let mut g = c.benchmark_group("voice_roundtrip_alaw_mono_8k_1s");
    g.throughput(Throughput::Bytes((n * 2) as u64));
    g.bench_function(
        BenchmarkId::from_parameter("alaw/voice/roundtrip/mono/8k/1s"),
        |b| {
            b.iter(|| {
                let p = params("pcm_alaw", 1, 8_000);
                let mut enc = alaw::make_encoder(&p).expect("make_encoder");
                let mut dec = alaw::make_decoder(&p).expect("make_decoder");
                let frame = Frame::Audio(AudioFrame {
                    samples: n as u32,
                    pts: Some(0),
                    data: vec![bytes.clone()],
                });
                enc.send_frame(&frame).expect("send_frame");
                let pkt = enc.receive_packet().expect("receive_packet");
                dec.send_packet(&pkt).expect("send_packet");
                let out = dec.receive_frame().expect("receive_frame");
                criterion::black_box(out);
            });
        },
    );
    g.finish();
}

criterion_group!(
    benches,
    bench_voice_decode_mulaw_lut_8k_1s,
    bench_voice_decode_alaw_lut_8k_1s,
    bench_voice_encode_mulaw_lut_8k_1s,
    bench_voice_encode_alaw_lut_8k_1s,
    bench_voice_encode_mulaw_arith_8k_1s,
    bench_voice_encode_alaw_arith_8k_1s,
    bench_voice_roundtrip_mulaw_mono_8k_1s,
    bench_voice_roundtrip_alaw_mono_8k_1s,
);
criterion_main!(benches);
