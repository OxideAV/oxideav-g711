//! Criterion benchmarks for the r406 **batch (slice) API** — the third
//! call surface next to the per-sample helpers and the trait surface.
//!
//! Every earlier harness drives either the single-sample functions in
//! a caller-side loop or the full `Decoder` / `Encoder` trait objects.
//! The batch helpers sit between the two: the same LUT loops as the
//! trait surface, but over caller-provided buffers with no `Packet` /
//! `Frame` construction and no per-call output allocation. This file
//! pins all three surfaces against each other **in one Criterion group
//! per direction × law**, so the cost decomposition is directly
//! readable from a single report:
//!
//!   - `per_sample`  — caller-side loop over `decode_sample` /
//!     `encode_sample` with an accumulator (the historical baseline
//!     shape from `benches/decode.rs` / `benches/encode.rs`);
//!   - `slice`       — `decode_slice` (i16 out) / `encode_slice`
//!     (i16 in), output buffer reused across iterations;
//!   - `slice_le`    — `decode_slice_to_le_bytes` /
//!     `encode_slice_from_le_bytes`, the exact loops the trait surface
//!     delegates to, on reused buffers;
//!   - `trait`       — the full `make_decoder` / `make_encoder` path
//!     (packet/frame framing + per-call output `Vec` allocation), so
//!     the trait-surface premium over `slice_le` is the measured cost
//!     of framing + allocation alone (the inner loop is shared since
//!     r406).
//!
//! Plus one µ-law-only group for the §3.2 zero-suppress slice form
//! against its per-sample loop.
//!
//! Input is 96 000 uniform-random elements (the 8 ch / 48 kHz / 250 ms
//! shape used by the largest r173 row) from the same xorshift32
//! generator as the sibling harnesses. Throughput is reported per
//! input byte for decode rows and per input sample (= per output byte)
//! for encode rows, so rows within a group are directly comparable.
//!
//! Run with:
//!     cargo bench -p oxideav-g711 --bench batch

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use oxideav_core::{AudioFrame, CodecId, CodecParameters, Frame, Packet, TimeBase};
use oxideav_g711::{alaw, mulaw};

/// Deterministic xorshift32 — same generator as the sibling harnesses
/// so inputs cover all 256 codewords / every companding segment
/// roughly uniformly.
fn xorshift32(state: &mut u32) -> u32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    *state
}

fn build_bytes(n: usize, seed: u32) -> Vec<u8> {
    let mut state = seed;
    (0..n)
        .map(|_| (xorshift32(&mut state) & 0xFF) as u8)
        .collect()
}

fn build_samples(n: usize, seed: u32) -> Vec<i16> {
    let mut state = seed;
    (0..n)
        .map(|_| (xorshift32(&mut state) & 0xFFFF) as u16 as i16)
        .collect()
}

/// 8 ch × 48 kHz × 250 ms — the largest fixed-size row in the r173
/// harnesses; big enough that the per-call allocation and framing
/// cost of the trait surface is measured against a realistic bulk
/// workload rather than amortised into noise.
const N: usize = 96_000;

fn params(id: &str) -> CodecParameters {
    let mut p = CodecParameters::audio(CodecId::new(id));
    p.sample_rate = Some(48_000);
    p.channels = Some(8);
    p
}

type DecodeSampleFn = fn(u8) -> i16;
type DecodeSliceFn = fn(&[u8], &mut [i16]);
type DecodeSliceLeFn = fn(&[u8], &mut [u8]);
type EncodeSampleFn = fn(i16) -> u8;
type EncodeSliceFn = fn(&[i16], &mut [u8]);
type EncodeSliceLeFn = fn(&[u8], &mut [u8]);
type DecoderFactory = fn(&CodecParameters) -> oxideav_core::Result<Box<dyn oxideav_core::Decoder>>;
type EncoderFactory = fn(&CodecParameters) -> oxideav_core::Result<Box<dyn oxideav_core::Encoder>>;

#[allow(clippy::too_many_arguments)]
fn bench_decode_surfaces(
    c: &mut Criterion,
    group_name: &str,
    codec_id: &str,
    seed: u32,
    per_sample: DecodeSampleFn,
    slice: DecodeSliceFn,
    slice_le: DecodeSliceLeFn,
    factory: DecoderFactory,
) {
    let bytes = build_bytes(N, seed);
    let mut g = c.benchmark_group(group_name);
    g.throughput(Throughput::Bytes(N as u64));

    g.bench_function(BenchmarkId::from_parameter("per_sample"), |b| {
        b.iter(|| {
            let src = criterion::black_box(&bytes);
            let mut acc: i32 = 0;
            for &byte in src {
                acc = acc.wrapping_add(per_sample(byte) as i32);
            }
            criterion::black_box(acc)
        });
    });

    let mut pcm_out = vec![0i16; N];
    g.bench_function(BenchmarkId::from_parameter("slice"), |b| {
        b.iter(|| {
            slice(criterion::black_box(&bytes), &mut pcm_out);
            criterion::black_box(&mut pcm_out);
        });
    });

    let mut le_out = vec![0u8; N * 2];
    g.bench_function(BenchmarkId::from_parameter("slice_le"), |b| {
        b.iter(|| {
            slice_le(criterion::black_box(&bytes), &mut le_out);
            criterion::black_box(&mut le_out);
        });
    });

    let p = params(codec_id);
    g.bench_function(BenchmarkId::from_parameter("trait"), |b| {
        b.iter(|| {
            let mut dec = factory(&p).expect("make_decoder");
            let pkt = Packet::new(0, TimeBase::new(1, 48_000), bytes.clone());
            dec.send_packet(&pkt).expect("send_packet");
            let frame = dec.receive_frame().expect("receive_frame");
            criterion::black_box(frame);
        });
    });

    g.finish();
}

#[allow(clippy::too_many_arguments)]
fn bench_encode_surfaces(
    c: &mut Criterion,
    group_name: &str,
    codec_id: &str,
    seed: u32,
    per_sample: EncodeSampleFn,
    slice: EncodeSliceFn,
    slice_le: EncodeSliceLeFn,
    factory: EncoderFactory,
) {
    let samples = build_samples(N, seed);
    let mut pcm_le = Vec::with_capacity(N * 2);
    for &s in &samples {
        pcm_le.extend_from_slice(&s.to_le_bytes());
    }

    let mut g = c.benchmark_group(group_name);
    // One output byte per input sample — report throughput per sample
    // so encode rows compare like-for-like with the decode groups.
    g.throughput(Throughput::Bytes(N as u64));

    g.bench_function(BenchmarkId::from_parameter("per_sample"), |b| {
        b.iter(|| {
            let src = criterion::black_box(&samples);
            let mut acc: u32 = 0;
            for &s in src {
                acc = acc.wrapping_add(per_sample(s) as u32);
            }
            criterion::black_box(acc)
        });
    });

    let mut wire_out = vec![0u8; N];
    g.bench_function(BenchmarkId::from_parameter("slice"), |b| {
        b.iter(|| {
            slice(criterion::black_box(&samples), &mut wire_out);
            criterion::black_box(&mut wire_out);
        });
    });

    g.bench_function(BenchmarkId::from_parameter("slice_le"), |b| {
        b.iter(|| {
            slice_le(criterion::black_box(&pcm_le), &mut wire_out);
            criterion::black_box(&mut wire_out);
        });
    });

    let p = params(codec_id);
    g.bench_function(BenchmarkId::from_parameter("trait"), |b| {
        b.iter(|| {
            let mut enc = factory(&p).expect("make_encoder");
            let frame = Frame::Audio(AudioFrame {
                samples: (N / 8) as u32,
                pts: Some(0),
                data: vec![pcm_le.clone()],
            });
            enc.send_frame(&frame).expect("send_frame");
            let pkt = enc.receive_packet().expect("receive_packet");
            criterion::black_box(pkt);
        });
    });

    g.finish();
}

fn bench_decode_mulaw(c: &mut Criterion) {
    bench_decode_surfaces(
        c,
        "batch_decode_mulaw_96k",
        "pcm_mulaw",
        0xCAFE_F00D,
        mulaw::decode_sample,
        mulaw::decode_slice,
        mulaw::decode_slice_to_le_bytes,
        mulaw::make_decoder,
    );
}

fn bench_decode_alaw(c: &mut Criterion) {
    bench_decode_surfaces(
        c,
        "batch_decode_alaw_96k",
        "pcm_alaw",
        0xBADC_0FFE,
        alaw::decode_sample,
        alaw::decode_slice,
        alaw::decode_slice_to_le_bytes,
        alaw::make_decoder,
    );
}

fn bench_encode_mulaw(c: &mut Criterion) {
    bench_encode_surfaces(
        c,
        "batch_encode_mulaw_96k",
        "pcm_mulaw",
        0xDEAD_BEEF,
        mulaw::encode_sample,
        mulaw::encode_slice,
        mulaw::encode_slice_from_le_bytes,
        mulaw::make_encoder,
    );
}

fn bench_encode_alaw(c: &mut Criterion) {
    bench_encode_surfaces(
        c,
        "batch_encode_alaw_96k",
        "pcm_alaw",
        0xFEED_FACE,
        alaw::encode_sample,
        alaw::encode_slice,
        alaw::encode_slice_from_le_bytes,
        alaw::make_encoder,
    );
}

/// µ-law §3.2 zero-suppress: slice form vs the per-sample loop. The
/// suppression adds one compare + conditional-move per sample on top
/// of the plain LUT load; this group pins that premium.
fn bench_encode_mulaw_zero_suppress(c: &mut Criterion) {
    let samples = build_samples(N, 0x0BAD_F00D);
    let mut g = c.benchmark_group("batch_encode_mulaw_zero_suppress_96k");
    g.throughput(Throughput::Bytes(N as u64));

    g.bench_function(BenchmarkId::from_parameter("per_sample"), |b| {
        b.iter(|| {
            let src = criterion::black_box(&samples);
            let mut acc: u32 = 0;
            for &s in src {
                acc = acc.wrapping_add(mulaw::encode_sample_zero_suppress(s) as u32);
            }
            criterion::black_box(acc)
        });
    });

    let mut wire_out = vec![0u8; N];
    g.bench_function(BenchmarkId::from_parameter("slice"), |b| {
        b.iter(|| {
            mulaw::encode_slice_zero_suppress(criterion::black_box(&samples), &mut wire_out);
            criterion::black_box(&mut wire_out);
        });
    });

    g.finish();
}

criterion_group!(
    benches,
    bench_decode_mulaw,
    bench_decode_alaw,
    bench_encode_mulaw,
    bench_encode_alaw,
    bench_encode_mulaw_zero_suppress,
);
criterion_main!(benches);
