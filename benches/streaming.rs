//! Criterion benchmarks for the G.711 **streaming** session pattern —
//! one encoder + decoder pair reused across many frames + packets.
//!
//! Round 206 (depth-mode benchmarks, streaming follow-up to r173):
//! `oxideav-g711` already ships per-call decode / encode / roundtrip
//! benches (`benches/{decode,encode,roundtrip}.rs`) that measure the
//! cost of constructing a fresh encoder / decoder pair plus running it
//! once. Those numbers are useful for an A/B against the inner LUT /
//! segment-search hot paths but they don't reflect what a streaming
//! PSTN caller actually pays: factory cost is amortised across hundreds
//! of frames, what really matters is the **per-frame queue traversal
//! cost** through the encoder's [`std::collections::VecDeque<Packet>`]
//! output queue + the decoder's `pending: Option<Packet>` slot when the
//! same instance is driven for many frames.
//!
//! This file complements the r173 trio by timing exactly that — fresh
//! pair built ONCE outside the timed region, then a configurable
//! frame burst is driven through it inside `b.iter`, mirroring how a
//! real session uses the trait surface.
//!
//! It also mirrors the r201 [`streaming_pipeline`] fuzz target's
//! lifecycle (one pair, many frames, eager vs. deferred drain, pts
//! propagation, end-of-stream flush) so a bench regression and a fuzz
//! escape line up against the same code paths.
//!
//! Each scenario is self-contained — every PCM input is synthesised
//! in-bench from a deterministic xorshift seed. No `docs/` fixtures or
//! external files are read.
//!
//! Scenarios:
//!
//!   - **streaming_mulaw_mono_8k_20ms_x50**: 50 × 20 ms frames of µ-law
//!     mono at 8 kHz (the canonical PSTN 20 ms G.711 packetisation —
//!     ITU-T G.711 §1 / RFC 3551 §4.5.14). Eager drain (one packet out
//!     per `send_frame`); reuses one encoder + decoder pair.
//!   - **streaming_alaw_mono_8k_20ms_x50**: same shape, A-law variant.
//!   - **streaming_mulaw_stereo_8k_20ms_x50**: 50 × 20 ms stereo µ-law
//!     frames — stresses the per-channel modulo check across the burst.
//!   - **streaming_mulaw_mono_8k_20ms_x50_deferred**: same input as the
//!     eager-drain scenario but queue everything first, then drain —
//!     exercises the encoder's `VecDeque` at its deepest depth and the
//!     decoder's `pending` slot at one packet at a time.
//!   - **streaming_alaw_8ch_48k_10ms_x100**: 100 × 10 ms frames of
//!     8-channel A-law at 48 kHz (OTT-grade rate × max channels ×
//!     small frame size) — largest cumulative working set among the
//!     bench scenarios; stresses the queue + interleave together.
//!
//! Run with:
//!     cargo bench -p oxideav-g711 --bench streaming

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use oxideav_core::{
    AudioFrame, CodecId, CodecParameters, Decoder, Encoder, Frame, Packet, SampleFormat, TimeBase,
};
use oxideav_g711::{alaw, mulaw};

/// Deterministic xorshift32 — matches the helper used in the r173
/// decode / encode / roundtrip benches so scenarios share an input
/// distribution and rows are directly comparable.
fn xorshift32(state: &mut u32) -> u32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    *state
}

/// One pre-built frame: little-endian S16 byte payload + samples-per-
/// channel count + pts. Pre-built so the timed region doesn't include
/// PCM synthesis cost (we measure the trait surface, not the seeded
/// RNG).
struct Burst {
    bytes: Vec<u8>,
    samples_per_channel: u32,
    pts: i64,
}

/// Build `frame_count` PCM frames each of `samples_per_channel` samples
/// across `channels` interleaved channels, monotonically incrementing
/// pts so the bench also pins pts-propagation through the encoder
/// queue.
fn build_burst(
    frame_count: usize,
    samples_per_channel: u32,
    channels: u16,
    seed: u32,
) -> Vec<Burst> {
    let mut state = seed;
    let mut out = Vec::with_capacity(frame_count);
    let mut pts: i64 = 0;
    let total_i16 = samples_per_channel as usize * channels as usize;
    for _ in 0..frame_count {
        let mut bytes = Vec::with_capacity(total_i16 * 2);
        for _ in 0..total_i16 {
            let s = (xorshift32(&mut state) & 0xFFFF) as i16;
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        out.push(Burst {
            bytes,
            samples_per_channel,
            pts,
        });
        pts = pts.saturating_add(samples_per_channel as i64);
    }
    out
}

fn params(id: &str, channels: u16, sample_rate: u32) -> CodecParameters {
    let mut p = CodecParameters::audio(CodecId::new(id));
    p.sample_rate = Some(sample_rate);
    p.channels = Some(channels);
    p.sample_format = Some(SampleFormat::S16);
    p
}

/// Drive the burst through one encoder + decoder pair, eager-drain
/// shape: `send_frame` → `receive_packet` → `send_packet` →
/// `receive_frame` per frame. Returns the byte count actually decoded
/// so `black_box` can stop the optimiser collapsing the loop.
fn drive_session_eager(
    enc: &mut Box<dyn Encoder>,
    dec: &mut Box<dyn Decoder>,
    burst: &[Burst],
    sample_rate: u32,
) -> usize {
    let mut total_bytes = 0usize;
    for b in burst {
        let f = Frame::Audio(AudioFrame {
            samples: b.samples_per_channel,
            pts: Some(b.pts),
            data: vec![b.bytes.clone()],
        });
        enc.send_frame(&f).expect("send_frame");
        let pkt = enc.receive_packet().expect("receive_packet");
        let dec_pkt = Packet::new(0, TimeBase::new(1, sample_rate as i64), pkt.data);
        dec.send_packet(&dec_pkt).expect("send_packet");
        if let Frame::Audio(af) = dec.receive_frame().expect("receive_frame") {
            total_bytes += af.data.first().map(|d| d.len()).unwrap_or(0);
        }
    }
    total_bytes
}

/// Deferred-drain variant: queue every input, then drain every output.
/// Stresses the encoder `VecDeque` at depth = `burst.len()`.
fn drive_session_deferred(
    enc: &mut Box<dyn Encoder>,
    dec: &mut Box<dyn Decoder>,
    burst: &[Burst],
    sample_rate: u32,
) -> usize {
    for b in burst {
        let f = Frame::Audio(AudioFrame {
            samples: b.samples_per_channel,
            pts: Some(b.pts),
            data: vec![b.bytes.clone()],
        });
        enc.send_frame(&f).expect("send_frame");
    }
    let _ = enc.flush();
    let mut total_bytes = 0usize;
    for _ in 0..burst.len() {
        let pkt = enc.receive_packet().expect("receive_packet");
        let dec_pkt = Packet::new(0, TimeBase::new(1, sample_rate as i64), pkt.data);
        dec.send_packet(&dec_pkt).expect("send_packet");
        if let Frame::Audio(af) = dec.receive_frame().expect("receive_frame") {
            total_bytes += af.data.first().map(|d| d.len()).unwrap_or(0);
        }
    }
    total_bytes
}

fn bench_streaming_mulaw_mono_8k_20ms_x50(c: &mut Criterion) {
    // 50 × 20 ms frames @ 8 kHz mono = 50 × 160 samples = 8000 samples
    // total = 1 s of audio in 50 packets. This is the canonical PSTN
    // packetisation shape (ITU-T G.711 §1 / RFC 3551 §4.5.14: 20 ms
    // frames at 8 kHz).
    let burst = build_burst(50, 160, 1, 0xCAFE_F00D);
    let total_bytes: u64 = burst.iter().map(|b| b.bytes.len() as u64).sum();
    let mut g = c.benchmark_group("streaming_mulaw_mono_8k_20ms_x50");
    g.throughput(Throughput::Bytes(total_bytes));
    g.bench_function(
        BenchmarkId::from_parameter("mulaw/mono/8k/20ms/x50/eager"),
        |b| {
            b.iter_batched(
                || {
                    let p = params("pcm_mulaw", 1, 8_000);
                    let enc = mulaw::make_encoder(&p).expect("make_encoder");
                    let dec = mulaw::make_decoder(&p).expect("make_decoder");
                    (enc, dec)
                },
                |(mut enc, mut dec)| {
                    let n = drive_session_eager(&mut enc, &mut dec, &burst, 8_000);
                    criterion::black_box(n);
                },
                criterion::BatchSize::SmallInput,
            );
        },
    );
    g.finish();
}

fn bench_streaming_alaw_mono_8k_20ms_x50(c: &mut Criterion) {
    let burst = build_burst(50, 160, 1, 0xBADC_0FFE);
    let total_bytes: u64 = burst.iter().map(|b| b.bytes.len() as u64).sum();
    let mut g = c.benchmark_group("streaming_alaw_mono_8k_20ms_x50");
    g.throughput(Throughput::Bytes(total_bytes));
    g.bench_function(
        BenchmarkId::from_parameter("alaw/mono/8k/20ms/x50/eager"),
        |b| {
            b.iter_batched(
                || {
                    let p = params("pcm_alaw", 1, 8_000);
                    let enc = alaw::make_encoder(&p).expect("make_encoder");
                    let dec = alaw::make_decoder(&p).expect("make_decoder");
                    (enc, dec)
                },
                |(mut enc, mut dec)| {
                    let n = drive_session_eager(&mut enc, &mut dec, &burst, 8_000);
                    criterion::black_box(n);
                },
                criterion::BatchSize::SmallInput,
            );
        },
    );
    g.finish();
}

fn bench_streaming_mulaw_stereo_8k_20ms_x50(c: &mut Criterion) {
    // 50 × 20 ms × stereo = 50 × 160 samples/channel × 2 channels =
    // 16 000 interleaved S16 samples = 32 KiB total PCM input.
    let burst = build_burst(50, 160, 2, 0xFEED_FACE);
    let total_bytes: u64 = burst.iter().map(|b| b.bytes.len() as u64).sum();
    let mut g = c.benchmark_group("streaming_mulaw_stereo_8k_20ms_x50");
    g.throughput(Throughput::Bytes(total_bytes));
    g.bench_function(
        BenchmarkId::from_parameter("mulaw/stereo/8k/20ms/x50/eager"),
        |b| {
            b.iter_batched(
                || {
                    let p = params("pcm_mulaw", 2, 8_000);
                    let enc = mulaw::make_encoder(&p).expect("make_encoder");
                    let dec = mulaw::make_decoder(&p).expect("make_decoder");
                    (enc, dec)
                },
                |(mut enc, mut dec)| {
                    let n = drive_session_eager(&mut enc, &mut dec, &burst, 8_000);
                    criterion::black_box(n);
                },
                criterion::BatchSize::SmallInput,
            );
        },
    );
    g.finish();
}

fn bench_streaming_mulaw_mono_8k_20ms_x50_deferred(c: &mut Criterion) {
    // Same input shape as the eager-drain scenario but the queue
    // accumulates 50 packets before any drain happens. Maps to the
    // `drain_eager = false` arm of the streaming_pipeline fuzz target.
    let burst = build_burst(50, 160, 1, 0xDEAD_BEEF);
    let total_bytes: u64 = burst.iter().map(|b| b.bytes.len() as u64).sum();
    let mut g = c.benchmark_group("streaming_mulaw_mono_8k_20ms_x50_deferred");
    g.throughput(Throughput::Bytes(total_bytes));
    g.bench_function(
        BenchmarkId::from_parameter("mulaw/mono/8k/20ms/x50/deferred"),
        |b| {
            b.iter_batched(
                || {
                    let p = params("pcm_mulaw", 1, 8_000);
                    let enc = mulaw::make_encoder(&p).expect("make_encoder");
                    let dec = mulaw::make_decoder(&p).expect("make_decoder");
                    (enc, dec)
                },
                |(mut enc, mut dec)| {
                    let n = drive_session_deferred(&mut enc, &mut dec, &burst, 8_000);
                    criterion::black_box(n);
                },
                criterion::BatchSize::SmallInput,
            );
        },
    );
    g.finish();
}

fn bench_streaming_alaw_8ch_48k_10ms_x100(c: &mut Criterion) {
    // 100 × 10 ms frames @ 48 kHz × 8 ch = 100 × 480 samples/channel
    // × 8 channels = 384 000 i16 samples = 768 000 input bytes total —
    // 1 s of OTT-grade 8-channel A-law in 100 packets. Largest
    // cumulative working set among the bench scenarios; stresses the
    // queue + per-channel interleave check + cache pressure together.
    let burst = build_burst(100, 480, 8, 0x1234_5678);
    let total_bytes: u64 = burst.iter().map(|b| b.bytes.len() as u64).sum();
    let mut g = c.benchmark_group("streaming_alaw_8ch_48k_10ms_x100");
    g.throughput(Throughput::Bytes(total_bytes));
    g.bench_function(
        BenchmarkId::from_parameter("alaw/8ch/48k/10ms/x100/eager"),
        |b| {
            b.iter_batched(
                || {
                    let p = params("pcm_alaw", 8, 48_000);
                    let enc = alaw::make_encoder(&p).expect("make_encoder");
                    let dec = alaw::make_decoder(&p).expect("make_decoder");
                    (enc, dec)
                },
                |(mut enc, mut dec)| {
                    let n = drive_session_eager(&mut enc, &mut dec, &burst, 48_000);
                    criterion::black_box(n);
                },
                criterion::BatchSize::SmallInput,
            );
        },
    );
    g.finish();
}

criterion_group!(
    benches,
    bench_streaming_mulaw_mono_8k_20ms_x50,
    bench_streaming_alaw_mono_8k_20ms_x50,
    bench_streaming_mulaw_stereo_8k_20ms_x50,
    bench_streaming_mulaw_mono_8k_20ms_x50_deferred,
    bench_streaming_alaw_8ch_48k_10ms_x100,
);
criterion_main!(benches);
