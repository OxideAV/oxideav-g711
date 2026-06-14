//! Criterion benchmarks for the G.711 hot paths under a
//! **single-segment-locked** input distribution.
//!
//! Round 298 (depth-mode benchmarks): the existing bench files span
//! two ends of the input-distribution spectrum. `decode` / `encode` /
//! `roundtrip` / `streaming` feed a uniform-random xorshift32 stream
//! (every segment of the companding curve sees equal traffic — the
//! canonical worst-case for cache-pressure and for the encode
//! segment-search branch profile), and `voice` (r247) feeds a
//! Laplacian-near-zero stream (~80% of samples in segments 0..=2, so
//! the segment search resolves to the segment-0 fast exit on most
//! samples). This file adds the **third corner**: every sample is
//! confined to a *single high segment*, so the encode segment search
//! resolves to the **same** segment index on every sample — the
//! opposite of both uniform (segment varies sample-to-sample) and
//! voice (segment is almost always 0).
//!
//! ## Why a constant-segment row is informative
//!
//! The arithmetic encode path's per-sample cost is dominated by the
//! segment search — for µ-law (ITU-T G.711 §3) the position of the
//! highest set bit of the biased magnitude, for A-law (§2) the
//! position of the highest set bit of the magnitude with an explicit
//! segment-0 short-circuit. The three distributions exercise that
//! search with three distinct branch-history profiles:
//!
//! - **uniform**: the resolved segment is effectively random
//!   sample-to-sample, so a branch predictor cannot learn it — the
//!   worst case for any data-dependent branch in the search.
//! - **voice**: the resolved segment is almost always 0, so the
//!   predictor parks on the fast-exit branch and the few large
//!   excursions are the only mispredicts.
//! - **segment (this file)**: the resolved segment is a *fixed*
//!   high value (segment 7 = the top band, magnitudes in
//!   `[16384..32768)`), so the predictor parks on the **opposite**
//!   branch from voice — the long-search path taken every time. If a
//!   future change makes the segment search's cost depend on which
//!   segment is resolved (e.g. an unrolled per-segment ladder, or a
//!   `leading_zeros`-based path whose latency varies with the input),
//!   the gap between the voice row and this row quantifies that
//!   data-dependence directly.
//!
//! The LUT rows (`encode_sample` / `decode_sample`) are a useful
//! control: a single 64 KiB-table load (encode) or 256-entry load
//! (decode) has no data-dependent branch, so the segment-locked LUT
//! rows should land *very close* to the voice and uniform LUT rows.
//! A meaningful spread on the LUT rows would instead point at a
//! cache-locality effect — the segment-7 band touches only the high
//! quadrants of `MULAW_ENCODE` / `ALAW_ENCODE` (large positive
//! indices near 0x4000..0x8000 and the two's-complement-wrapped large
//! negative indices near 0x8000..0xC000), a contiguous-but-distinct
//! 16 KiB region from the small-magnitude block the voice rows touch.
//!
//! ## Segment-locked synthesis
//!
//! We pin every sample into the **top segment** of the curve — the
//! band the spec assigns its coarsest quantisation step, where
//! companding error is largest and where a loud constant tone (a
//! steady DTMF-style or saturating-line signal) actually lives. The
//! A-law top segment (segment 7) covers magnitudes `[16384..32768)`;
//! the same band is well inside the µ-law top segment after bias, so
//! a single magnitude window serves both laws. We spread the magnitude
//! across the whole `[16384..32767]` window (so the 4-bit mantissa
//! extraction sees all 16 mantissa values, not a single codeword) and
//! alternate the sign from a deterministic xorshift32 bit, so the
//! sign-extraction branch is *also* exercised — only the resolved
//! segment is held constant.
//!
//! ## Scenarios
//!
//! - **segment_encode_mulaw_arith_8k_1s** — 1 s of segment-locked
//!   i16 PCM through [`oxideav_g711::mulaw::encode_sample_arith`]
//!   (the formula path). The segment search resolves to the same
//!   high segment every sample.
//! - **segment_encode_alaw_arith_8k_1s** — same shape, A-law (whose
//!   segment-0 short-circuit is *never* taken on this input, the
//!   mirror image of the voice row where it is almost always taken).
//! - **segment_encode_mulaw_lut_8k_1s** — same input through the
//!   r230 64 KiB compile-time LUT ([`oxideav_g711::mulaw::encode_sample`]),
//!   touching only the high quadrants of `MULAW_ENCODE`.
//! - **segment_encode_alaw_lut_8k_1s** — same shape, A-law LUT.
//! - **segment_decode_mulaw_lut_8k_1s** — 1 s of µ-law bytes encoded
//!   from the segment-locked stream (so the decode LUT sees the
//!   high-segment codeword distribution) through
//!   [`oxideav_g711::mulaw::decode_sample`].
//! - **segment_decode_alaw_lut_8k_1s** — same shape, A-law.
//! - **segment_roundtrip_mulaw_mono_8k_1s** — full encoder + decoder
//!   pair through the trait surface on a 1 s mono segment-locked
//!   input. Mirrors `roundtrip_mulaw_mono_8k_1s` / the voice file's
//!   roundtrip row, swapping only the input distribution.
//! - **segment_roundtrip_alaw_mono_8k_1s** — same shape, A-law.
//!
//! ## Provenance
//!
//! Every input is synthesised in-bench from deterministic seeds. No
//! `docs/` fixtures or external files are read; no audio corpora; no
//! probability-distribution crates. The segment-locked generator is a
//! closed-form window over a uniform xorshift32 stream, written from
//! scratch in this file.
//!
//! Run with:
//!     cargo bench -p oxideav-g711 --bench segment

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use oxideav_core::{AudioFrame, CodecId, CodecParameters, Frame, SampleFormat};
use oxideav_g711::{alaw, mulaw};

/// Deterministic xorshift32 — matches the generator the other bench
/// files use so a segment-locked row and a uniform / voice row at the
/// same seed share an underlying RNG stream.
fn xorshift32(state: &mut u32) -> u32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    *state
}

/// Inclusive lower bound of the top segment's magnitude window. The
/// A-law top segment (segment 7) is `[16384..32768)`; the same window
/// is well inside the µ-law top segment after the 132-sample bias, so
/// one window serves both laws.
const SEG_TOP_LO: i32 = 16_384;
/// Exclusive upper bound — i16::MAX + 1 would overflow, so we cap the
/// span at i16::MAX (32767) inclusive below.
const SEG_TOP_HI: i32 = 32_767;

/// Draw one segment-locked i16 sample from the supplied xorshift32
/// state. The magnitude is spread uniformly across the top segment's
/// `[16384..=32767]` window (so all 16 mantissa values are exercised),
/// and the sign is drawn from a deterministic bit of the same stream
/// (so the sign-extraction branch is exercised too). Only the resolved
/// *segment* is held constant across every sample.
fn segment_sample(state: &mut u32) -> i16 {
    let r = xorshift32(state);
    // Low 31 bits choose the magnitude window position; the top bit
    // chooses the sign. Keeping them from one draw keeps the generator
    // a single call per sample like the voice / uniform generators.
    let span = (SEG_TOP_HI - SEG_TOP_LO + 1) as u32; // 16384
    let magnitude = SEG_TOP_LO + ((r & 0x7FFF_FFFF) % span) as i32;
    if r & 0x8000_0000 != 0 {
        // Negative side: -16384..=-32767 all live in the top segment
        // too (i16::MIN = -32768 maps to the same segment but we keep
        // the symmetric window for clarity).
        (-magnitude) as i16
    } else {
        magnitude as i16
    }
}

/// Build `n` segment-locked S16 PCM samples seeded by `seed`.
fn build_segment_pcm(n: usize, seed: u32) -> Vec<i16> {
    let mut state = seed.max(1);
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push(segment_sample(&mut state));
    }
    out
}

/// Build `n` µ-law bytes from segment-locked S16 input (so the decode
/// bench sees the high-segment codeword distribution this input
/// produces, not a uniform one over 0..=255).
fn build_segment_mulaw_bytes(n: usize, seed: u32) -> Vec<u8> {
    build_segment_pcm(n, seed)
        .into_iter()
        .map(mulaw::encode_sample)
        .collect()
}

/// Build `n` A-law bytes from segment-locked S16 input.
fn build_segment_alaw_bytes(n: usize, seed: u32) -> Vec<u8> {
    build_segment_pcm(n, seed)
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

fn bench_segment_encode_mulaw_arith_8k_1s(c: &mut Criterion) {
    let n = 8_000;
    let pcm = build_segment_pcm(n, 0xC0DE_BABE);
    let mut g = c.benchmark_group("segment_encode_mulaw_arith_8k_1s");
    g.throughput(Throughput::Bytes((n * 2) as u64));
    g.bench_function(
        BenchmarkId::from_parameter("mulaw/segment/arith/8k/1s"),
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

fn bench_segment_encode_alaw_arith_8k_1s(c: &mut Criterion) {
    let n = 8_000;
    let pcm = build_segment_pcm(n, 0xC001_D00D);
    let mut g = c.benchmark_group("segment_encode_alaw_arith_8k_1s");
    g.throughput(Throughput::Bytes((n * 2) as u64));
    g.bench_function(
        BenchmarkId::from_parameter("alaw/segment/arith/8k/1s"),
        |b| {
            b.iter(|| {
                let src = criterion::black_box(&pcm);
                let mut acc: u32 = 0;
                for &s in src {
                    acc = acc.wrapping_add(alaw::encode_sample_arith(s) as u32);
                }
                criterion::black_box(acc)
            });
        },
    );
    g.finish();
}

fn bench_segment_encode_mulaw_lut_8k_1s(c: &mut Criterion) {
    let n = 8_000;
    let pcm = build_segment_pcm(n, 0xC0DE_BABE);
    let mut g = c.benchmark_group("segment_encode_mulaw_lut_8k_1s");
    g.throughput(Throughput::Bytes((n * 2) as u64));
    g.bench_function(
        BenchmarkId::from_parameter("mulaw/segment/lut/8k/1s"),
        |b| {
            b.iter(|| {
                let src = criterion::black_box(&pcm);
                let mut acc: u32 = 0;
                for &s in src {
                    acc = acc.wrapping_add(mulaw::encode_sample(s) as u32);
                }
                criterion::black_box(acc)
            });
        },
    );
    g.finish();
}

fn bench_segment_encode_alaw_lut_8k_1s(c: &mut Criterion) {
    let n = 8_000;
    let pcm = build_segment_pcm(n, 0xC001_D00D);
    let mut g = c.benchmark_group("segment_encode_alaw_lut_8k_1s");
    g.throughput(Throughput::Bytes((n * 2) as u64));
    g.bench_function(BenchmarkId::from_parameter("alaw/segment/lut/8k/1s"), |b| {
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

fn bench_segment_decode_mulaw_lut_8k_1s(c: &mut Criterion) {
    let n = 8_000;
    let bytes = build_segment_mulaw_bytes(n, 0xCAFE_F00D);
    let mut g = c.benchmark_group("segment_decode_mulaw_lut_8k_1s");
    g.throughput(Throughput::Bytes(n as u64));
    g.bench_function(
        BenchmarkId::from_parameter("mulaw/segment/lut/8k/1s"),
        |b| {
            b.iter(|| {
                let src = criterion::black_box(&bytes);
                let mut acc: i32 = 0;
                for &byte in src {
                    acc = acc.wrapping_add(mulaw::decode_sample(byte) as i32);
                }
                criterion::black_box(acc)
            });
        },
    );
    g.finish();
}

fn bench_segment_decode_alaw_lut_8k_1s(c: &mut Criterion) {
    let n = 8_000;
    let bytes = build_segment_alaw_bytes(n, 0xBADC_0FFE);
    let mut g = c.benchmark_group("segment_decode_alaw_lut_8k_1s");
    g.throughput(Throughput::Bytes(n as u64));
    g.bench_function(BenchmarkId::from_parameter("alaw/segment/lut/8k/1s"), |b| {
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

fn params(id: &str, channels: u16, sample_rate: u32) -> CodecParameters {
    let mut p = CodecParameters::audio(CodecId::new(id));
    p.sample_rate = Some(sample_rate);
    p.channels = Some(channels);
    p.sample_format = Some(SampleFormat::S16);
    p
}

fn bench_segment_roundtrip_mulaw_mono_8k_1s(c: &mut Criterion) {
    let n = 8_000;
    let pcm = build_segment_pcm(n, 0xFACE_F00D);
    let bytes = pcm_le_bytes(&pcm);
    let mut g = c.benchmark_group("segment_roundtrip_mulaw_mono_8k_1s");
    g.throughput(Throughput::Bytes((n * 2) as u64));
    g.bench_function(
        BenchmarkId::from_parameter("mulaw/segment/roundtrip/mono/8k/1s"),
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

fn bench_segment_roundtrip_alaw_mono_8k_1s(c: &mut Criterion) {
    let n = 8_000;
    let pcm = build_segment_pcm(n, 0xABAD_1DEA);
    let bytes = pcm_le_bytes(&pcm);
    let mut g = c.benchmark_group("segment_roundtrip_alaw_mono_8k_1s");
    g.throughput(Throughput::Bytes((n * 2) as u64));
    g.bench_function(
        BenchmarkId::from_parameter("alaw/segment/roundtrip/mono/8k/1s"),
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
    bench_segment_encode_mulaw_arith_8k_1s,
    bench_segment_encode_alaw_arith_8k_1s,
    bench_segment_encode_mulaw_lut_8k_1s,
    bench_segment_encode_alaw_lut_8k_1s,
    bench_segment_decode_mulaw_lut_8k_1s,
    bench_segment_decode_alaw_lut_8k_1s,
    bench_segment_roundtrip_mulaw_mono_8k_1s,
    bench_segment_roundtrip_alaw_mono_8k_1s,
);
criterion_main!(benches);
