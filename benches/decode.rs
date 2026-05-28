//! Criterion benchmarks for the G.711 decoder hot paths.
//!
//! Round 173 (depth-mode benchmarks): `oxideav-g711` hit saturation
//! long ago (workspace README row: ✅ 100% decoder / ✅ 100% encoder)
//! and r121 already added an exhaustive S16-domain property sweep plus
//! a PSNR-floor regression. Per the workspace
//! "saturated -> fuzz/bench/profile" memo this round wires `criterion`
//! benches so future optimisation rounds (e.g. attempts at a 64 KiB
//! direct encode LUT, SIMD batch decode, no-bounds-check inner loops)
//! can A/B-test their changes against a stable, deterministic baseline.
//!
//! This file covers the **decoder**; sibling files cover `encode`
//! (sign-extract + bias + segment search) and `roundtrip` (back-to-
//! back encode + decode through the trait surface).
//!
//! Each scenario is self-contained — every byte input is synthesised
//! in-bench from a deterministic xorshift seed and fed through either
//! the per-sample [`oxideav_g711::mulaw::decode_sample`] /
//! [`oxideav_g711::alaw::decode_sample`] helpers or the trait-surface
//! [`oxideav_g711::mulaw::make_decoder`] /
//! [`oxideav_g711::alaw::make_decoder`] factories. No `docs/`
//! fixtures or external files are read.
//!
//! Scenarios:
//!
//!   - **decode_mulaw_lut_8k_1s**: 1 second of µ-law bytes at the spec's
//!     8 kHz PSTN rate, decoded one sample at a time via the direct
//!     256-entry LUT. The fastest possible path; serves as the lower
//!     bound for what the trait surface can hope to achieve.
//!   - **decode_alaw_lut_8k_1s**: same shape, A-law variant.
//!   - **decode_mulaw_decoder_mono_8k_1s**: 1 second mono through the
//!     trait-surface [`oxideav_core::Decoder`] — adds packet framing,
//!     channel-count validation, S16 little-endian byte packing on top
//!     of the LUT.
//!   - **decode_alaw_decoder_stereo_8k_1s**: 1 second stereo A-law
//!     through the trait surface — exercises the per-channel modulo
//!     check + interleaved-byte fast path.
//!   - **decode_mulaw_decoder_8ch_48k_250ms**: 0.25 s of 8-channel
//!     µ-law at 48 kHz (typical OTT-grade rate) — stresses the wider
//!     interleave with a realistic packet size (~96 KiB output).
//!
//! Run with:
//!     cargo bench -p oxideav-g711 --bench decode

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use oxideav_core::{CodecId, CodecParameters, Packet, TimeBase};
use oxideav_g711::{alaw, mulaw};

/// Deterministic xorshift32 — synthesises evenly-distributed byte
/// inputs so segment-search costs and LUT cache pressure look
/// representative. A pure-zero buffer would hide the cost of the
/// arithmetic encode segment search in the encode benches and lock
/// the decode LUT into a single cache line.
fn xorshift32(state: &mut u32) -> u32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    *state
}

/// Build `n` pseudo-random bytes suitable for feeding either A-law or
/// µ-law decode. The xorshift output covers all 256 byte values
/// roughly uniformly so the LUT load pattern hits every segment of
/// the companding curve.
fn build_bytes(n: usize, seed: u32) -> Vec<u8> {
    let mut state = seed;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push((xorshift32(&mut state) & 0xFF) as u8);
    }
    out
}

fn bench_decode_mulaw_lut_8k_1s(c: &mut Criterion) {
    let n = 8_000; // 1 s @ 8 kHz mono
    let bytes = build_bytes(n, 0xCAFE_F00D);
    let mut g = c.benchmark_group("decode_mulaw_lut_8k_1s");
    g.throughput(Throughput::Bytes(n as u64));
    g.bench_function(BenchmarkId::from_parameter("mulaw/lut/8k/1s"), |b| {
        b.iter(|| {
            // Drive the per-sample LUT directly — the fastest available
            // decode path. `black_box` on input + the accumulator keeps
            // the optimiser from collapsing the loop into a constant.
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

fn bench_decode_alaw_lut_8k_1s(c: &mut Criterion) {
    let n = 8_000;
    let bytes = build_bytes(n, 0xBADC_0FFE);
    let mut g = c.benchmark_group("decode_alaw_lut_8k_1s");
    g.throughput(Throughput::Bytes(n as u64));
    g.bench_function(BenchmarkId::from_parameter("alaw/lut/8k/1s"), |b| {
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
    p
}

fn bench_decode_mulaw_decoder_mono_8k_1s(c: &mut Criterion) {
    let n = 8_000;
    let bytes = build_bytes(n, 0xDEAD_BEEF);
    let mut g = c.benchmark_group("decode_mulaw_decoder_mono_8k_1s");
    g.throughput(Throughput::Bytes(n as u64));
    g.bench_function(
        BenchmarkId::from_parameter("mulaw/decoder/mono/8k/1s"),
        |b| {
            b.iter(|| {
                // Construction cost is part of the iter — a real caller
                // typically reuses a decoder across packets, but the
                // factory is so light (one tiny struct) that including it
                // keeps the bench fair to the trait surface as a whole.
                let p = params("pcm_mulaw", 1, 8_000);
                let mut dec = mulaw::make_decoder(&p).expect("make_decoder");
                let pkt = Packet::new(0, TimeBase::new(1, 8_000), bytes.clone());
                dec.send_packet(&pkt).expect("send_packet");
                let frame = dec.receive_frame().expect("receive_frame");
                criterion::black_box(frame);
            });
        },
    );
    g.finish();
}

fn bench_decode_alaw_decoder_stereo_8k_1s(c: &mut Criterion) {
    let n = 16_000; // 1 s stereo @ 8 kHz = 16k interleaved bytes
    let bytes = build_bytes(n, 0xFEED_FACE);
    let mut g = c.benchmark_group("decode_alaw_decoder_stereo_8k_1s");
    g.throughput(Throughput::Bytes(n as u64));
    g.bench_function(
        BenchmarkId::from_parameter("alaw/decoder/stereo/8k/1s"),
        |b| {
            b.iter(|| {
                let p = params("pcm_alaw", 2, 8_000);
                let mut dec = alaw::make_decoder(&p).expect("make_decoder");
                let pkt = Packet::new(0, TimeBase::new(1, 8_000), bytes.clone());
                dec.send_packet(&pkt).expect("send_packet");
                let frame = dec.receive_frame().expect("receive_frame");
                criterion::black_box(frame);
            });
        },
    );
    g.finish();
}

fn bench_decode_mulaw_decoder_8ch_48k_250ms(c: &mut Criterion) {
    // 0.25 s of 8-channel µ-law at 48 kHz = 12 000 samples/channel ×
    // 8 channels = 96 000 bytes. Stresses the per-channel modulo
    // check plus the larger working set so cache effects start to
    // matter.
    let n = 96_000;
    let bytes = build_bytes(n, 0x1234_5678);
    let mut g = c.benchmark_group("decode_mulaw_decoder_8ch_48k_250ms");
    g.throughput(Throughput::Bytes(n as u64));
    g.bench_function(
        BenchmarkId::from_parameter("mulaw/decoder/8ch/48k/250ms"),
        |b| {
            b.iter(|| {
                let p = params("pcm_mulaw", 8, 48_000);
                let mut dec = mulaw::make_decoder(&p).expect("make_decoder");
                let pkt = Packet::new(0, TimeBase::new(1, 48_000), bytes.clone());
                dec.send_packet(&pkt).expect("send_packet");
                let frame = dec.receive_frame().expect("receive_frame");
                criterion::black_box(frame);
            });
        },
    );
    g.finish();
}

criterion_group!(
    benches,
    bench_decode_mulaw_lut_8k_1s,
    bench_decode_alaw_lut_8k_1s,
    bench_decode_mulaw_decoder_mono_8k_1s,
    bench_decode_alaw_decoder_stereo_8k_1s,
    bench_decode_mulaw_decoder_8ch_48k_250ms,
);
criterion_main!(benches);
