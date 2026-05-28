//! Criterion benchmarks for the G.711 encode → decode round-trip.
//!
//! Round 173 (depth-mode benchmarks): companion to `benches/decode.rs`
//! and `benches/encode.rs`. These scenarios time the full encoder-
//! plus-decoder pair through the public trait surface, in the order
//! a transcoder would actually pay (i16 PCM in, encoded bytes, i16
//! PCM out). Useful for catching regressions where one half gets
//! faster but the other half regresses by more — the round-trip
//! number bounds what a streaming pipeline actually observes.
//!
//! All inputs are synthesised in-bench from a deterministic xorshift
//! seed; no `docs/` fixtures or external files are read.
//!
//! Scenarios:
//!
//!   - **roundtrip_mulaw_mono_8k_1s**: 1 s mono µ-law at 8 kHz — the
//!     canonical PSTN scenario; smallest packet that still hits every
//!     segment of the curve.
//!   - **roundtrip_alaw_mono_8k_1s**: same shape, A-law variant.
//!   - **roundtrip_mulaw_stereo_8k_1s**: 1 s stereo µ-law at 8 kHz —
//!     stresses the per-channel interleave check on both ends.
//!   - **roundtrip_alaw_8ch_48k_250ms**: 0.25 s of 8-channel A-law
//!     at 48 kHz — typical OTT-grade rate × max channel count;
//!     largest working set among the bench scenarios.
//!
//! Run with:
//!     cargo bench -p oxideav-g711 --bench roundtrip

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use oxideav_core::{AudioFrame, CodecId, CodecParameters, Frame, SampleFormat};
use oxideav_g711::{alaw, mulaw};

/// Deterministic xorshift32 — matches the helpers in the sibling
/// bench files so scenarios share an input distribution.
fn xorshift32(state: &mut u32) -> u32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    *state
}

fn build_pcm_bytes(n_samples: usize, seed: u32) -> Vec<u8> {
    let mut state = seed;
    let mut out = Vec::with_capacity(n_samples * 2);
    for _ in 0..n_samples {
        let s = (xorshift32(&mut state) & 0xFFFF) as i16;
        out.extend_from_slice(&s.to_le_bytes());
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

/// Encode → decode through the public trait surface; returns the
/// decoded PCM frame so `black_box` can prevent dead-code elimination.
fn mulaw_roundtrip(pcm_bytes: &[u8], samples_per_channel: u32, channels: u16, sr: u32) -> Frame {
    let p = params("pcm_mulaw", channels, sr);
    let mut enc = mulaw::make_encoder(&p).expect("make_encoder");
    let mut dec = mulaw::make_decoder(&p).expect("make_decoder");
    let in_frame = Frame::Audio(AudioFrame {
        samples: samples_per_channel,
        pts: Some(0),
        data: vec![pcm_bytes.to_vec()],
    });
    enc.send_frame(&in_frame).expect("send_frame");
    let pkt = enc.receive_packet().expect("receive_packet");
    dec.send_packet(&pkt).expect("send_packet");
    dec.receive_frame().expect("receive_frame")
}

fn alaw_roundtrip(pcm_bytes: &[u8], samples_per_channel: u32, channels: u16, sr: u32) -> Frame {
    let p = params("pcm_alaw", channels, sr);
    let mut enc = alaw::make_encoder(&p).expect("make_encoder");
    let mut dec = alaw::make_decoder(&p).expect("make_decoder");
    let in_frame = Frame::Audio(AudioFrame {
        samples: samples_per_channel,
        pts: Some(0),
        data: vec![pcm_bytes.to_vec()],
    });
    enc.send_frame(&in_frame).expect("send_frame");
    let pkt = enc.receive_packet().expect("receive_packet");
    dec.send_packet(&pkt).expect("send_packet");
    dec.receive_frame().expect("receive_frame")
}

fn bench_roundtrip_mulaw_mono_8k_1s(c: &mut Criterion) {
    let n = 8_000;
    let pcm = build_pcm_bytes(n, 0xCAFE_1234);
    let mut g = c.benchmark_group("roundtrip_mulaw_mono_8k_1s");
    g.throughput(Throughput::Bytes((n * 2) as u64));
    g.bench_function(BenchmarkId::from_parameter("mulaw/mono/8k/1s"), |b| {
        b.iter(|| {
            let out = mulaw_roundtrip(criterion::black_box(&pcm), n as u32, 1, 8_000);
            criterion::black_box(out);
        });
    });
    g.finish();
}

fn bench_roundtrip_alaw_mono_8k_1s(c: &mut Criterion) {
    let n = 8_000;
    let pcm = build_pcm_bytes(n, 0xBEEF_5678);
    let mut g = c.benchmark_group("roundtrip_alaw_mono_8k_1s");
    g.throughput(Throughput::Bytes((n * 2) as u64));
    g.bench_function(BenchmarkId::from_parameter("alaw/mono/8k/1s"), |b| {
        b.iter(|| {
            let out = alaw_roundtrip(criterion::black_box(&pcm), n as u32, 1, 8_000);
            criterion::black_box(out);
        });
    });
    g.finish();
}

fn bench_roundtrip_mulaw_stereo_8k_1s(c: &mut Criterion) {
    let n = 16_000; // 1 s stereo @ 8 kHz = 16k samples total
    let pcm = build_pcm_bytes(n, 0xF00D_9ABC);
    let mut g = c.benchmark_group("roundtrip_mulaw_stereo_8k_1s");
    g.throughput(Throughput::Bytes((n * 2) as u64));
    g.bench_function(BenchmarkId::from_parameter("mulaw/stereo/8k/1s"), |b| {
        b.iter(|| {
            let out = mulaw_roundtrip(criterion::black_box(&pcm), (n / 2) as u32, 2, 8_000);
            criterion::black_box(out);
        });
    });
    g.finish();
}

fn bench_roundtrip_alaw_8ch_48k_250ms(c: &mut Criterion) {
    // 0.25 s × 48 kHz × 8 channels = 96 000 i16 samples.
    let n = 96_000;
    let pcm = build_pcm_bytes(n, 0xDECA_FBAD);
    let mut g = c.benchmark_group("roundtrip_alaw_8ch_48k_250ms");
    g.throughput(Throughput::Bytes((n * 2) as u64));
    g.bench_function(BenchmarkId::from_parameter("alaw/8ch/48k/250ms"), |b| {
        b.iter(|| {
            let out = alaw_roundtrip(criterion::black_box(&pcm), (n / 8) as u32, 8, 48_000);
            criterion::black_box(out);
        });
    });
    g.finish();
}

criterion_group!(
    benches,
    bench_roundtrip_mulaw_mono_8k_1s,
    bench_roundtrip_alaw_mono_8k_1s,
    bench_roundtrip_mulaw_stereo_8k_1s,
    bench_roundtrip_alaw_8ch_48k_250ms,
);
criterion_main!(benches);
