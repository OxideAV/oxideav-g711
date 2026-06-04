//! Criterion benchmarks for the G.711 encoder hot paths.
//!
//! Round 173 (depth-mode benchmarks): companion to `benches/decode.rs`.
//! Round 230 added a compile-time 64 KiB direct S16 → byte LUT for
//! both laws ([`oxideav_g711::mulaw::encode_sample`] now indexes
//! `MULAW_ENCODE`; [`encode_sample_arith`] still ships the
//! formula). The `arith` rows here keep the segment-search-loop
//! baseline so future optimisation rounds (e.g. SIMD-batched LUT
//! gather, or a 2× 32 KiB sign-folded LUT, or a runtime-feature-gated
//! `leading_zeros` arith path) can A/B against both the formula and
//! the table without removing either side.
//!
//! Each scenario is self-contained: every S16 input is synthesised
//! in-bench from a deterministic xorshift seed so we exercise every
//! segment of the companding curve roughly uniformly. No `docs/`
//! fixtures or external files are read.
//!
//! Scenarios:
//!
//!   - **encode_mulaw_arith_8k_1s**: 1 second of synthesised S16 PCM
//!     at 8 kHz mono, encoded one sample at a time via the per-sample
//!     [`oxideav_g711::mulaw::encode_sample_arith`] formula path —
//!     the pre-r230 baseline; kept so optimisation rounds can A/B
//!     against the table.
//!   - **encode_alaw_arith_8k_1s**: same shape, A-law variant. A-law
//!     has a slightly different segment search shape (no bias add,
//!     explicit segment 0 short-circuit) so its cost can drift
//!     independently of µ-law.
//!   - **encode_mulaw_lut_8k_1s** (r230): same input as the µ-law
//!     `arith` row but encoded via the 64 KiB compile-time
//!     `MULAW_ENCODE` LUT — the current per-sample hot path.
//!   - **encode_alaw_lut_8k_1s** (r230): same input as the A-law
//!     `arith` row but via `ALAW_ENCODE` — the LUT win is larger on
//!     A-law because the arith path has an extra branch
//!     (segment-0 fast-path), so the table closes more of that gap.
//!   - **encode_mulaw_encoder_mono_8k_1s**: 1 second mono through the
//!     trait-surface [`oxideav_core::Encoder`] — adds packet framing,
//!     LE-byte unpacking, output queue management on top of the
//!     per-sample LUT load (post-r230).
//!   - **encode_alaw_encoder_stereo_8k_1s**: 1 second stereo A-law
//!     through the trait surface — same overhead breakdown but on
//!     twice the byte count.
//!   - **encode_mulaw_encoder_8ch_48k_250ms**: 0.25 s of 8-channel
//!     µ-law at 48 kHz (typical OTT-grade rate, 96 000 i16 samples)
//!     — exercises the larger interleave with a realistic packet size.
//!
//! Run with:
//!     cargo bench -p oxideav-g711 --bench encode

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use oxideav_core::{AudioFrame, CodecId, CodecParameters, Frame, SampleFormat};
use oxideav_g711::{alaw, mulaw};

/// Deterministic xorshift32 — mirrors the helper in `decode.rs` so
/// both encoders see comparable input distributions.
fn xorshift32(state: &mut u32) -> u32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    *state
}

/// Build `n` pseudo-random i16 samples. The xorshift output covers
/// the full S16 range roughly uniformly so the segment search hits
/// every branch of the encode table.
fn build_pcm(n: usize, seed: u32) -> Vec<i16> {
    let mut state = seed;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push((xorshift32(&mut state) & 0xFFFF) as i16);
    }
    out
}

/// Convert an i16 buffer into the little-endian byte form the
/// trait-surface Encoder expects (each sample becomes a 2-byte LE
/// pair inside `AudioFrame::data[0]`).
fn pcm_le_bytes(pcm: &[i16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(pcm.len() * 2);
    for &s in pcm {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

fn bench_encode_mulaw_arith_8k_1s(c: &mut Criterion) {
    let n = 8_000;
    let pcm = build_pcm(n, 0xC0DE_BABE);
    let mut g = c.benchmark_group("encode_mulaw_arith_8k_1s");
    g.throughput(Throughput::Bytes((n * 2) as u64));
    g.bench_function(BenchmarkId::from_parameter("mulaw/arith/8k/1s"), |b| {
        b.iter(|| {
            let src = criterion::black_box(&pcm);
            let mut acc: u32 = 0;
            for &s in src {
                acc = acc.wrapping_add(mulaw::encode_sample_arith(s) as u32);
            }
            criterion::black_box(acc)
        });
    });
    g.finish();
}

fn bench_encode_alaw_arith_8k_1s(c: &mut Criterion) {
    let n = 8_000;
    let pcm = build_pcm(n, 0xC001_D00D);
    let mut g = c.benchmark_group("encode_alaw_arith_8k_1s");
    g.throughput(Throughput::Bytes((n * 2) as u64));
    g.bench_function(BenchmarkId::from_parameter("alaw/arith/8k/1s"), |b| {
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

/// r230 — per-sample LUT path (the steady-state hot path post-r230).
/// Same input distribution as the matching `arith` row so a direct
/// throughput ratio measures the win from replacing segment search
/// with a single 64 KiB LUT load.
fn bench_encode_mulaw_lut_8k_1s(c: &mut Criterion) {
    let n = 8_000;
    let pcm = build_pcm(n, 0xC0DE_BABE);
    let mut g = c.benchmark_group("encode_mulaw_lut_8k_1s");
    g.throughput(Throughput::Bytes((n * 2) as u64));
    g.bench_function(BenchmarkId::from_parameter("mulaw/lut/8k/1s"), |b| {
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

fn bench_encode_alaw_lut_8k_1s(c: &mut Criterion) {
    let n = 8_000;
    let pcm = build_pcm(n, 0xC001_D00D);
    let mut g = c.benchmark_group("encode_alaw_lut_8k_1s");
    g.throughput(Throughput::Bytes((n * 2) as u64));
    g.bench_function(BenchmarkId::from_parameter("alaw/lut/8k/1s"), |b| {
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

fn params(id: &str, channels: u16, sample_rate: u32) -> CodecParameters {
    let mut p = CodecParameters::audio(CodecId::new(id));
    p.sample_rate = Some(sample_rate);
    p.channels = Some(channels);
    p.sample_format = Some(SampleFormat::S16);
    p
}

fn bench_encode_mulaw_encoder_mono_8k_1s(c: &mut Criterion) {
    let n = 8_000;
    let pcm = build_pcm(n, 0xFACE_F00D);
    let bytes = pcm_le_bytes(&pcm);
    let mut g = c.benchmark_group("encode_mulaw_encoder_mono_8k_1s");
    g.throughput(Throughput::Bytes((n * 2) as u64));
    g.bench_function(
        BenchmarkId::from_parameter("mulaw/encoder/mono/8k/1s"),
        |b| {
            b.iter(|| {
                let p = params("pcm_mulaw", 1, 8_000);
                let mut enc = mulaw::make_encoder(&p).expect("make_encoder");
                let frame = Frame::Audio(AudioFrame {
                    samples: n as u32,
                    pts: Some(0),
                    data: vec![bytes.clone()],
                });
                enc.send_frame(&frame).expect("send_frame");
                let pkt = enc.receive_packet().expect("receive_packet");
                criterion::black_box(pkt);
            });
        },
    );
    g.finish();
}

fn bench_encode_alaw_encoder_stereo_8k_1s(c: &mut Criterion) {
    let n = 16_000; // 1 s stereo @ 8 kHz = 16k samples total
    let pcm = build_pcm(n, 0xABAD_1DEA);
    let bytes = pcm_le_bytes(&pcm);
    let mut g = c.benchmark_group("encode_alaw_encoder_stereo_8k_1s");
    g.throughput(Throughput::Bytes((n * 2) as u64));
    g.bench_function(
        BenchmarkId::from_parameter("alaw/encoder/stereo/8k/1s"),
        |b| {
            b.iter(|| {
                let p = params("pcm_alaw", 2, 8_000);
                let mut enc = alaw::make_encoder(&p).expect("make_encoder");
                let frame = Frame::Audio(AudioFrame {
                    samples: (n / 2) as u32,
                    pts: Some(0),
                    data: vec![bytes.clone()],
                });
                enc.send_frame(&frame).expect("send_frame");
                let pkt = enc.receive_packet().expect("receive_packet");
                criterion::black_box(pkt);
            });
        },
    );
    g.finish();
}

fn bench_encode_mulaw_encoder_8ch_48k_250ms(c: &mut Criterion) {
    // 0.25 s × 48 kHz × 8 channels = 96 000 S16 samples = 192 000
    // input bytes → 96 000 µ-law output bytes.
    let n = 96_000;
    let pcm = build_pcm(n, 0xB16B_00B5);
    let bytes = pcm_le_bytes(&pcm);
    let mut g = c.benchmark_group("encode_mulaw_encoder_8ch_48k_250ms");
    g.throughput(Throughput::Bytes((n * 2) as u64));
    g.bench_function(
        BenchmarkId::from_parameter("mulaw/encoder/8ch/48k/250ms"),
        |b| {
            b.iter(|| {
                let p = params("pcm_mulaw", 8, 48_000);
                let mut enc = mulaw::make_encoder(&p).expect("make_encoder");
                let frame = Frame::Audio(AudioFrame {
                    samples: (n / 8) as u32,
                    pts: Some(0),
                    data: vec![bytes.clone()],
                });
                enc.send_frame(&frame).expect("send_frame");
                let pkt = enc.receive_packet().expect("receive_packet");
                criterion::black_box(pkt);
            });
        },
    );
    g.finish();
}

criterion_group!(
    benches,
    bench_encode_mulaw_arith_8k_1s,
    bench_encode_alaw_arith_8k_1s,
    bench_encode_mulaw_lut_8k_1s,
    bench_encode_alaw_lut_8k_1s,
    bench_encode_mulaw_encoder_mono_8k_1s,
    bench_encode_alaw_encoder_stereo_8k_1s,
    bench_encode_mulaw_encoder_8ch_48k_250ms,
);
criterion_main!(benches);
