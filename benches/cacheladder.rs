//! Criterion benchmark for the G.711 hot-path **working-set size
//! sweep** — the cache-residency curve.
//!
//! Round 319 (depth-mode benchmarks, size-sweep follow-up to r173 /
//! r206 / r247 / r298): every existing bench file fixes the buffer /
//! frame size to a specific shape (1 s @ 8 kHz, 250 ms @ 8 ch / 48 kHz,
//! 20 ms PSTN packets, …) and varies the *distribution* (uniform /
//! voice-Laplacian / segment-locked), the *law*, and the *path* (direct
//! LUT vs. trait surface). None of them sweep a single law + path across
//! a **geometric ladder of buffer sizes**, which is the axis that
//! exposes the L1 → L2 → L3 → DRAM throughput knee of the decode LUT and
//! the encode-arith inner loop.
//!
//! That knee is the missing measurement the r289 store-strategy work
//! wanted: r289 measured the `decode-store-recompute` vs.
//! `decode-store-le-lut` A/B at a *single* 96 KB / 8 ch / 48 kHz point
//! and reported "small mono / stereo rows that fit in L1 are
//! store-insensitive and stay within noise". This file makes that claim
//! falsifiable across the whole residency curve: by sweeping the output
//! working set from 1 KiB (comfortably L1-resident on every target) up
//! through 4 MiB (DRAM-bound on a typical L2/L3), a future
//! store-strategy or SIMD change can see exactly where its win turns on
//! and whether it regresses the small-buffer case.
//!
//! The ladder is reported per **element** (one input byte → one output
//! sample), so `Throughput::Elements` makes every rung directly
//! comparable regardless of absolute size: a flat curve means the path
//! is compute-bound (the 256-entry LUT / arith state stays resident and
//! the only working set that grows is the streamed input/output, which
//! is read/written once and never revisited); a curve that rolls off at
//! large sizes means the path has become memory-bandwidth-bound on the
//! input + output streams.
//!
//! Rungs: 1 KiB, 4 KiB, 16 KiB, 64 KiB, 256 KiB, 1 MiB, 4 MiB of input
//! codewords (output S16 is 2×). The small end sits inside L1d on every
//! supported target; the large end is DRAM-bound on a typical
//! 256 KiB-L2 / few-MiB-L3 machine. Cross-target absolute numbers are
//! not portable (cache geometry differs) — the load-bearing signal is
//! the *shape* of the curve on a given machine and any change in that
//! shape between commits.
//!
//! Five families, all driven from the same uniform xorshift32 stream as
//! `benches/decode.rs` so a rung here lines up with the fixed-size rows
//! there at the matching size:
//!
//!   - **decode_lut_sweep/mulaw** — direct `mulaw::decode_sample` LUT,
//!     one sample at a time, accumulated. The compute-bound lower bound.
//!   - **decode_lut_sweep/alaw** — same, A-law.
//!   - **decode_decoder_sweep/mulaw** — the same input through the
//!     trait-surface `mulaw::make_decoder` → `send_packet` /
//!     `receive_frame`, so the rung also pays the S16 little-endian
//!     store into a freshly-allocated `Vec` (this is the path r289
//!     optimised, and the only one whose curve should *roll off* as the
//!     output `Vec` outgrows cache).
//!   - **encode_arith_sweep/mulaw** — the arithmetic encoder
//!     (`mulaw::encode_sample_arith`, the formula path, *not* the 64 KiB
//!     LUT) over an i16 stream. Segment search is branch-bound, not
//!     table-bound, so this curve isolates input-stream bandwidth from
//!     LUT residency.
//!   - **encode_arith_sweep/alaw** — same, A-law.
//!
//! Every input is synthesised in-bench from a deterministic xorshift32
//! seed. No `docs/` fixtures or external files are read.
//!
//! Run with:
//!     cargo bench -p oxideav-g711 --bench cacheladder

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use oxideav_core::{CodecId, CodecParameters, Packet, TimeBase};
use oxideav_g711::{alaw, mulaw};

/// Deterministic xorshift32 — identical to the helper in
/// `benches/decode.rs` so the two files share an input distribution and
/// a rung here is directly comparable to the matching fixed-size row
/// there.
fn xorshift32(state: &mut u32) -> u32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    *state
}

/// `n` pseudo-random bytes covering all 256 codeword values roughly
/// uniformly — the same construction as `benches/decode.rs::build_bytes`.
fn build_bytes(n: usize, seed: u32) -> Vec<u8> {
    let mut state = seed;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push((xorshift32(&mut state) & 0xFF) as u8);
    }
    out
}

/// `n` pseudo-random i16 samples for the encode sweeps.
fn build_samples(n: usize, seed: u32) -> Vec<i16> {
    let mut state = seed;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push((xorshift32(&mut state) & 0xFFFF) as u16 as i16);
    }
    out
}

/// The geometric size ladder, in input-codeword bytes. The small end is
/// L1d-resident on every supported target; the large end is DRAM-bound
/// on a typical 256 KiB-L2 machine.
const LADDER: &[usize] = &[
    1 << 10, // 1 KiB
    1 << 12, // 4 KiB
    1 << 14, // 16 KiB
    1 << 16, // 64 KiB
    1 << 18, // 256 KiB
    1 << 20, // 1 MiB
    1 << 22, // 4 MiB
];

/// Human-readable rung label so the BenchmarkId reads `…/16KiB` etc.
fn rung_label(n: usize) -> String {
    if n >= 1 << 20 {
        format!("{}MiB", n >> 20)
    } else {
        format!("{}KiB", n >> 10)
    }
}

fn bench_decode_lut_sweep(c: &mut Criterion) {
    let mut g = c.benchmark_group("decode_lut_sweep");
    for &n in LADDER {
        let bytes = build_bytes(n, 0xCAFE_F00D ^ n as u32);
        g.throughput(Throughput::Elements(n as u64));
        g.bench_function(BenchmarkId::new("mulaw", rung_label(n)), |b| {
            b.iter(|| {
                let src = criterion::black_box(&bytes);
                let mut acc: i32 = 0;
                for &byte in src {
                    acc = acc.wrapping_add(mulaw::decode_sample(byte) as i32);
                }
                criterion::black_box(acc)
            });
        });
        g.bench_function(BenchmarkId::new("alaw", rung_label(n)), |b| {
            b.iter(|| {
                let src = criterion::black_box(&bytes);
                let mut acc: i32 = 0;
                for &byte in src {
                    acc = acc.wrapping_add(alaw::decode_sample(byte) as i32);
                }
                criterion::black_box(acc)
            });
        });
    }
    g.finish();
}

fn params(id: &str, channels: u16, sample_rate: u32) -> CodecParameters {
    let mut p = CodecParameters::audio(CodecId::new(id));
    p.sample_rate = Some(sample_rate);
    p.channels = Some(channels);
    p
}

fn bench_decode_decoder_sweep(c: &mut Criterion) {
    // The trait surface allocates a fresh output `Vec<u8>` of 2×n bytes
    // per packet and writes the S16 little-endian samples into it — this
    // is the r289 store path and the only family whose curve should roll
    // off as the output `Vec` outgrows cache. Mono so the per-channel
    // modulo check is cheap and the bytes / sample relationship is 1:1.
    let mut g = c.benchmark_group("decode_decoder_sweep");
    for &n in LADDER {
        let bytes = build_bytes(n, 0xDEAD_BEEF ^ n as u32);
        g.throughput(Throughput::Elements(n as u64));
        g.bench_function(BenchmarkId::new("mulaw_mono", rung_label(n)), |b| {
            b.iter(|| {
                // Build the decoder once per iter — the factory is a tiny
                // struct, matching `benches/decode.rs`'s fairness choice.
                let p = params("pcm_mulaw", 1, 8_000);
                let mut dec = mulaw::make_decoder(&p).expect("make_decoder");
                let pkt = Packet::new(0, TimeBase::new(1, 8_000), bytes.clone());
                dec.send_packet(&pkt).expect("send_packet");
                let frame = dec.receive_frame().expect("receive_frame");
                criterion::black_box(frame);
            });
        });
    }
    g.finish();
}

fn bench_encode_arith_sweep(c: &mut Criterion) {
    // The arithmetic encoder's segment search is branch-bound, not
    // table-bound — its only growing working set is the streamed i16
    // input, so this family isolates input-stream bandwidth from any LUT
    // residency effect. Drives the formula path
    // (`encode_sample_arith`), not the 64 KiB LUT, on purpose.
    let mut g = c.benchmark_group("encode_arith_sweep");
    for &n in LADDER {
        let samples = build_samples(n, 0xBADC_0FFE ^ n as u32);
        g.throughput(Throughput::Elements(n as u64));
        g.bench_function(BenchmarkId::new("mulaw", rung_label(n)), |b| {
            b.iter(|| {
                let src = criterion::black_box(&samples);
                let mut acc: u32 = 0;
                for &s in src {
                    acc = acc.wrapping_add(mulaw::encode_sample_arith(s) as u32);
                }
                criterion::black_box(acc)
            });
        });
        g.bench_function(BenchmarkId::new("alaw", rung_label(n)), |b| {
            b.iter(|| {
                let src = criterion::black_box(&samples);
                let mut acc: u32 = 0;
                for &s in src {
                    acc = acc.wrapping_add(alaw::encode_sample_arith(s) as u32);
                }
                criterion::black_box(acc)
            });
        });
    }
    g.finish();
}

criterion_group!(
    benches,
    bench_decode_lut_sweep,
    bench_decode_decoder_sweep,
    bench_encode_arith_sweep,
);
criterion_main!(benches);
