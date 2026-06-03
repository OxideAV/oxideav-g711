//! Standalone profiling driver for the G.711 µ-law and A-law codecs.
//!
//! Round 189 (depth-mode profiling): the three Criterion harnesses
//! (`benches/{decode,encode,roundtrip}.rs`, round 173) measure
//! steady-state throughput in a sampling framework, but they are a
//! poor target for `samply` / `perf record` / `cargo flamegraph`
//! because Criterion's warm-up + sampling layers + estimator math
//! show up in the profile and bury the real codec hot paths (LUT
//! load, bias + segment search, on-wire inversion). This example is
//! a flat measure-this-thing driver: it synthesises deterministic
//! PCM / byte buffers once, then runs a fixed iteration count of the
//! requested path with a single `Instant::now()` / `elapsed()` pair
//! around the whole loop. Throughput is printed at the end so the
//! same binary doubles as a quick A/B harness for the inner
//! tweak-remeasure loop when Criterion's per-run overhead is too
//! coarse.
//!
//! Usage:
//!
//!     cargo run --example profile_g711 --release -- <mode> [<iters>]
//!
//! Modes:
//!
//!     decode      — synthesise µ-law and A-law byte buffers once,
//!                   then run N decode passes (LUT + trait surface)
//!     encode      — synthesise S16 PCM once, then run N encode
//!                   passes (per-sample arithmetic + trait surface)
//!     roundtrip   — synthesise S16 PCM once, then run N
//!                   encode→decode passes through the trait surface
//!     streaming   — build one encoder + decoder pair, then drive a
//!                   multi-frame burst through it (eager and deferred
//!                   drain shapes) — mirrors the r206 `streaming`
//!                   Criterion bench and the r201 `streaming_pipeline`
//!                   fuzz target so a profile capture corresponds 1:1
//!                   with the streaming bench rows
//!     all         — run every mode (default)
//!
//! With `samply` (the recommended path on macOS / Linux — no root,
//! produces a Firefox Profiler URL):
//!
//!     cargo build --example profile_g711 --release
//!     samply record -- ./target/release/examples/profile_g711 encode 5000
//!     samply record -- ./target/release/examples/profile_g711 decode 5000
//!
//! With `cargo flamegraph` (needs `cargo install flamegraph` and
//! perf / dtrace privileges):
//!
//!     cargo flamegraph --example profile_g711 -- encode 5000
//!
//! No external files are read — every input is synthesised in-driver
//! from a deterministic xorshift32 seed, byte-for-byte matching the
//! Criterion bench harnesses so profile output and bench numbers
//! correspond. The scenarios mirror the bench rows:
//!
//!   - mulaw / mono / 8 kHz / 1 s — canonical PSTN load, lower bound
//!   - alaw  / mono / 8 kHz / 1 s — A-law variant of the canonical
//!   - mulaw / stereo / 8 kHz / 1 s — per-channel interleave check
//!   - alaw  / stereo / 8 kHz / 1 s — per-channel interleave on A-law
//!   - mulaw / 8ch / 48 kHz / 250 ms — OTT-grade rate × max channels,
//!     biggest working set (96 000 bytes / 192 000 PCM bytes per iter)
//!
//! The driver intentionally walks the LUT / arithmetic per-sample
//! paths *and* the trait-surface `Decoder` / `Encoder` paths
//! separately, so a profile capture can show how much of any future
//! regression lives in the codec inner loop vs. framing overhead
//! (packet construction, LE byte unpack, output queue management).
//!
//! Round 213 (depth-mode profiling, streaming follow-up to r189):
//! the `streaming` mode adds five extra rows that mirror the r206
//! `benches/streaming.rs` Criterion scenarios byte-for-byte (same
//! frame counts, same channel layouts, same xorshift32 seeds,
//! eager-drain and deferred-drain variants). The four r189 modes
//! above build a fresh encoder + decoder pair PER iter, charging
//! factory cost in every sample; that is the right shape for the
//! r173 per-call benches but a poor profile target for steady-state
//! PSTN sessions where one pair is reused across hundreds of frames
//! and the cost that dominates is queue traversal (encoder
//! `VecDeque<Packet>` + decoder `pending: Option<Packet>` slot) plus
//! per-frame interleave + LE byte unpack. The streaming mode builds
//! the pair ONCE outside the timed region (mirroring `iter_batched`
//! in the r206 bench) and drives a configurable burst inside, so a
//! samply / flamegraph capture of the streaming mode lines up with
//! the matching `streaming_*` bench rows directly.

use std::env;
use std::io::Write;
use std::time::Instant;

use oxideav_core::{AudioFrame, CodecId, CodecParameters, Frame, Packet, SampleFormat, TimeBase};
use oxideav_g711::{alaw, mulaw};

/// Deterministic xorshift32 — same constant used by the bench
/// harnesses so the profile and bench inputs are byte-identical.
fn xorshift32(state: &mut u32) -> u32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    *state
}

/// Synthesise `n` evenly-distributed bytes for the decode paths.
/// The xorshift output covers all 256 codepoints roughly uniformly
/// so the LUT load hits every segment of the companding curve.
fn build_bytes(n: usize, seed: u32) -> Vec<u8> {
    let mut state = seed;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push((xorshift32(&mut state) & 0xFF) as u8);
    }
    out
}

/// Synthesise `n` evenly-distributed i16 PCM samples for the encode
/// paths. Covers the full S16 range roughly uniformly so the
/// segment-search loop hits every branch of the encode table.
fn build_pcm(n: usize, seed: u32) -> Vec<i16> {
    let mut state = seed;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push((xorshift32(&mut state) & 0xFFFF) as i16);
    }
    out
}

/// Convert an i16 buffer to LE bytes for the trait-surface Encoder
/// (each sample becomes a 2-byte LE pair inside `AudioFrame::data[0]`).
fn pcm_le_bytes(pcm: &[i16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(pcm.len() * 2);
    for &s in pcm {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// One row in the profile table. Mirrors the Criterion bench groups
/// so the profile capture and bench numbers line up by scenario.
struct Scenario {
    /// Display name, used in the throughput print and search-from-
    /// flamegraph cross-reference.
    name: &'static str,
    /// G.711 variant for this row.
    law: Law,
    /// Channel count fed to the trait-surface factory.
    channels: u16,
    /// Sample rate fed to the trait-surface factory.
    sample_rate: u32,
    /// Number of total bytes / total i16 samples for the row.
    /// For decode this is the byte count; for encode and roundtrip
    /// it is the S16 sample count (so byte count = 2 × this).
    n: usize,
    /// xorshift seed for the synthesised input.
    seed: u32,
    /// Default iteration count if the user does not override on the
    /// command line. Picked so each row spends roughly 0.5–1 s of
    /// wall-clock on a modern aarch64-darwin laptop in release mode.
    iters_default: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Law {
    Mulaw,
    Alaw,
}

fn scenarios() -> &'static [Scenario] {
    &[
        Scenario {
            name: "mulaw/mono/8k/1s",
            law: Law::Mulaw,
            channels: 1,
            sample_rate: 8_000,
            n: 8_000,
            seed: 0xCAFE_F00D,
            iters_default: 2_000,
        },
        Scenario {
            name: "alaw/mono/8k/1s",
            law: Law::Alaw,
            channels: 1,
            sample_rate: 8_000,
            n: 8_000,
            seed: 0xBADC_0FFE,
            iters_default: 2_000,
        },
        Scenario {
            name: "mulaw/stereo/8k/1s",
            law: Law::Mulaw,
            channels: 2,
            sample_rate: 8_000,
            n: 16_000,
            seed: 0xDEAD_BEEF,
            iters_default: 1_000,
        },
        Scenario {
            name: "alaw/stereo/8k/1s",
            law: Law::Alaw,
            channels: 2,
            sample_rate: 8_000,
            n: 16_000,
            seed: 0xFEED_FACE,
            iters_default: 1_000,
        },
        Scenario {
            name: "mulaw/8ch/48k/250ms",
            law: Law::Mulaw,
            channels: 8,
            sample_rate: 48_000,
            n: 96_000,
            seed: 0x1234_5678,
            iters_default: 200,
        },
    ]
}

fn params(scen: &Scenario) -> CodecParameters {
    let id = match scen.law {
        Law::Mulaw => "pcm_mulaw",
        Law::Alaw => "pcm_alaw",
    };
    let mut p = CodecParameters::audio(CodecId::new(id));
    p.sample_rate = Some(scen.sample_rate);
    p.channels = Some(scen.channels);
    p.sample_format = Some(SampleFormat::S16);
    p
}

fn print_throughput_line(label: &str, name: &str, iters: u32, bytes_per_iter: usize, elapsed: f64) {
    let per_iter_us = elapsed * 1_000_000.0 / iters.max(1) as f64;
    let total = bytes_per_iter as f64 * iters as f64;
    let mib_per_s = total / elapsed / (1024.0 * 1024.0);
    println!(
        "  {label:9} {name:24} iters={iters:>6} {per_iter_us:9.3} us/iter  {mib_per_s:8.2} MiB/s",
    );
}

/// Decode mode — the per-sample LUT path *and* the trait-surface
/// Decoder path, one row per scenario. The LUT path is the lower
/// bound for what the trait surface can hope to achieve; the gap
/// between the two rows on the same scenario is the framing cost
/// (packet validation, channel-count modulo, S16 LE byte packing).
fn profile_decode(iters_override: Option<u32>) {
    println!("== decode ==");
    for scen in scenarios() {
        let iters = iters_override.unwrap_or(scen.iters_default);
        let bytes = build_bytes(scen.n, scen.seed);

        // Per-sample LUT — warm-up + timed loop.
        let acc_warm: i32 = bytes
            .iter()
            .map(|&b| match scen.law {
                Law::Mulaw => mulaw::decode_sample(b) as i32,
                Law::Alaw => alaw::decode_sample(b) as i32,
            })
            .sum();
        std::hint::black_box(acc_warm);

        let t = Instant::now();
        let mut sink: i64 = 0;
        for _ in 0..iters {
            let src = std::hint::black_box(&bytes);
            let mut acc: i32 = 0;
            for &b in src {
                acc = acc.wrapping_add(match scen.law {
                    Law::Mulaw => mulaw::decode_sample(b) as i32,
                    Law::Alaw => alaw::decode_sample(b) as i32,
                });
            }
            sink ^= acc as i64;
        }
        std::hint::black_box(sink);
        let elapsed = t.elapsed().as_secs_f64();
        print_throughput_line("decode-lut", scen.name, iters, scen.n, elapsed);

        // Trait-surface Decoder — fresh factory per iter (matches the
        // bench harness; the factory is light so it does not skew the
        // measured cost). Decoder owns one pending packet at a time,
        // so we drain the frame in the same iteration.
        let p = params(scen);
        let _warm = {
            let mut dec = match scen.law {
                Law::Mulaw => mulaw::make_decoder(&p).expect("make_decoder"),
                Law::Alaw => alaw::make_decoder(&p).expect("make_decoder"),
            };
            let pkt = Packet::new(0, TimeBase::new(1, scen.sample_rate as i64), bytes.clone());
            dec.send_packet(&pkt).expect("send_packet");
            dec.receive_frame().expect("receive_frame")
        };
        std::hint::black_box(_warm);

        let t = Instant::now();
        for _ in 0..iters {
            let mut dec = match scen.law {
                Law::Mulaw => mulaw::make_decoder(&p).expect("make_decoder"),
                Law::Alaw => alaw::make_decoder(&p).expect("make_decoder"),
            };
            let pkt = Packet::new(0, TimeBase::new(1, scen.sample_rate as i64), bytes.clone());
            dec.send_packet(&pkt).expect("send_packet");
            let frame = dec.receive_frame().expect("receive_frame");
            std::hint::black_box(frame);
        }
        let elapsed = t.elapsed().as_secs_f64();
        print_throughput_line("decode-trait", scen.name, iters, scen.n, elapsed);

        std::io::stdout().flush().ok();
    }
}

/// Encode mode — per-sample arithmetic path + trait-surface Encoder
/// path. The arithmetic path is the headline cost (sign extract,
/// bias add, segment search via top-bit loop, mantissa shift,
/// on-wire inversion); the trait-surface row adds LE byte unpack
/// and output queue management.
fn profile_encode(iters_override: Option<u32>) {
    println!("== encode ==");
    for scen in scenarios() {
        let iters = iters_override.unwrap_or(scen.iters_default);
        let pcm = build_pcm(scen.n, scen.seed);
        let bytes = pcm_le_bytes(&pcm);
        // Encode input is the S16 PCM byte stream — 2 × sample count.
        let bytes_per_iter = scen.n * 2;

        // Per-sample arithmetic — warm-up + timed loop.
        let acc_warm: u32 = pcm
            .iter()
            .map(|&s| match scen.law {
                Law::Mulaw => mulaw::encode_sample(s) as u32,
                Law::Alaw => alaw::encode_sample(s) as u32,
            })
            .sum();
        std::hint::black_box(acc_warm);

        let t = Instant::now();
        let mut sink: u64 = 0;
        for _ in 0..iters {
            let src = std::hint::black_box(&pcm);
            let mut acc: u32 = 0;
            for &s in src {
                acc = acc.wrapping_add(match scen.law {
                    Law::Mulaw => mulaw::encode_sample(s) as u32,
                    Law::Alaw => alaw::encode_sample(s) as u32,
                });
            }
            sink ^= acc as u64;
        }
        std::hint::black_box(sink);
        let elapsed = t.elapsed().as_secs_f64();
        print_throughput_line("encode-arith", scen.name, iters, bytes_per_iter, elapsed);

        // Trait-surface Encoder.
        let p = params(scen);
        let samples_per_channel = (scen.n / scen.channels as usize) as u32;
        {
            // Warm-up.
            let mut enc = match scen.law {
                Law::Mulaw => mulaw::make_encoder(&p).expect("make_encoder"),
                Law::Alaw => alaw::make_encoder(&p).expect("make_encoder"),
            };
            let frame = Frame::Audio(AudioFrame {
                samples: samples_per_channel,
                pts: Some(0),
                data: vec![bytes.clone()],
            });
            enc.send_frame(&frame).expect("send_frame");
            let pkt = enc.receive_packet().expect("receive_packet");
            std::hint::black_box(pkt);
        }

        let t = Instant::now();
        for _ in 0..iters {
            let mut enc = match scen.law {
                Law::Mulaw => mulaw::make_encoder(&p).expect("make_encoder"),
                Law::Alaw => alaw::make_encoder(&p).expect("make_encoder"),
            };
            let frame = Frame::Audio(AudioFrame {
                samples: samples_per_channel,
                pts: Some(0),
                data: vec![bytes.clone()],
            });
            enc.send_frame(&frame).expect("send_frame");
            let pkt = enc.receive_packet().expect("receive_packet");
            std::hint::black_box(pkt);
        }
        let elapsed = t.elapsed().as_secs_f64();
        print_throughput_line("encode-trait", scen.name, iters, bytes_per_iter, elapsed);

        std::io::stdout().flush().ok();
    }
}

/// Roundtrip mode — encode → decode through the trait surface, in
/// the order a transcoder would actually pay. Useful for catching
/// regressions where one half gets faster but the other half
/// regresses by more.
fn profile_roundtrip(iters_override: Option<u32>) {
    println!("== roundtrip ==");
    for scen in scenarios() {
        let iters = iters_override.unwrap_or(scen.iters_default);
        let pcm = build_pcm(scen.n, scen.seed);
        let bytes = pcm_le_bytes(&pcm);
        let bytes_per_iter = scen.n * 2;
        let p = params(scen);
        let samples_per_channel = (scen.n / scen.channels as usize) as u32;

        // Warm-up.
        {
            let mut enc = match scen.law {
                Law::Mulaw => mulaw::make_encoder(&p).expect("make_encoder"),
                Law::Alaw => alaw::make_encoder(&p).expect("make_encoder"),
            };
            let mut dec = match scen.law {
                Law::Mulaw => mulaw::make_decoder(&p).expect("make_decoder"),
                Law::Alaw => alaw::make_decoder(&p).expect("make_decoder"),
            };
            let in_frame = Frame::Audio(AudioFrame {
                samples: samples_per_channel,
                pts: Some(0),
                data: vec![bytes.clone()],
            });
            enc.send_frame(&in_frame).expect("send_frame");
            let pkt = enc.receive_packet().expect("receive_packet");
            dec.send_packet(&pkt).expect("send_packet");
            let out = dec.receive_frame().expect("receive_frame");
            std::hint::black_box(out);
        }

        let t = Instant::now();
        for _ in 0..iters {
            let mut enc = match scen.law {
                Law::Mulaw => mulaw::make_encoder(&p).expect("make_encoder"),
                Law::Alaw => alaw::make_encoder(&p).expect("make_encoder"),
            };
            let mut dec = match scen.law {
                Law::Mulaw => mulaw::make_decoder(&p).expect("make_decoder"),
                Law::Alaw => alaw::make_decoder(&p).expect("make_decoder"),
            };
            let in_frame = Frame::Audio(AudioFrame {
                samples: samples_per_channel,
                pts: Some(0),
                data: vec![bytes.clone()],
            });
            enc.send_frame(&in_frame).expect("send_frame");
            let pkt = enc.receive_packet().expect("receive_packet");
            dec.send_packet(&pkt).expect("send_packet");
            let out = dec.receive_frame().expect("receive_frame");
            std::hint::black_box(out);
        }
        let elapsed = t.elapsed().as_secs_f64();
        print_throughput_line("roundtrip", scen.name, iters, bytes_per_iter, elapsed);

        std::io::stdout().flush().ok();
    }
}

/// One row in the streaming profile table. Mirrors the r206
/// Criterion bench groups in `benches/streaming.rs` byte-for-byte
/// (same frame counts, same channel layouts, same xorshift32 seeds)
/// so a samply / flamegraph capture and the matching `streaming_*`
/// bench row correspond directly.
struct StreamingScenario {
    /// Display name. Matches the Criterion group prefix so a
    /// flamegraph search lines up with the bench output.
    name: &'static str,
    /// G.711 variant for this row.
    law: Law,
    /// Channel count fed to the trait-surface factory.
    channels: u16,
    /// Sample rate fed to the trait-surface factory.
    sample_rate: u32,
    /// Number of PCM frames in one burst (the inner `b.iter` loop
    /// body in the r206 bench).
    frame_count: usize,
    /// Samples per channel per frame. e.g. 160 = 20 ms at 8 kHz,
    /// 480 = 10 ms at 48 kHz.
    samples_per_channel: u32,
    /// xorshift seed for the synthesised PCM. Matches the r206 bench
    /// seeds so the inputs are byte-identical.
    seed: u32,
    /// `true` = eager drain (one packet out per `send_frame`).
    /// `false` = deferred drain (queue every input, then drain every
    /// output).
    eager_drain: bool,
    /// Default iteration count if the user does not override on the
    /// command line. Picked so each row spends roughly 0.5–1 s of
    /// wall-clock on a modern aarch64-darwin laptop in release mode
    /// (the streaming inner loop drives `frame_count` frames per
    /// iter, so iter counts are about an order of magnitude lower
    /// than the per-call modes).
    iters_default: u32,
}

fn streaming_scenarios() -> &'static [StreamingScenario] {
    &[
        // 50 × 20 ms µ-law mono at 8 kHz — canonical PSTN
        // packetisation (ITU-T G.711 §1 / RFC 3551 §4.5.14).
        // Matches `streaming_mulaw_mono_8k_20ms_x50` in
        // `benches/streaming.rs`.
        StreamingScenario {
            name: "streaming/mulaw/mono/8k/20ms/x50/eager",
            law: Law::Mulaw,
            channels: 1,
            sample_rate: 8_000,
            frame_count: 50,
            samples_per_channel: 160,
            seed: 0xCAFE_F00D,
            eager_drain: true,
            iters_default: 500,
        },
        // 50 × 20 ms A-law mono at 8 kHz — A-law variant of the
        // canonical PSTN scenario. Matches
        // `streaming_alaw_mono_8k_20ms_x50`.
        StreamingScenario {
            name: "streaming/alaw/mono/8k/20ms/x50/eager",
            law: Law::Alaw,
            channels: 1,
            sample_rate: 8_000,
            frame_count: 50,
            samples_per_channel: 160,
            seed: 0xBADC_0FFE,
            eager_drain: true,
            iters_default: 500,
        },
        // 50 × 20 ms µ-law stereo at 8 kHz — exercises the
        // per-channel modulo check + interleave across the burst.
        // Matches `streaming_mulaw_stereo_8k_20ms_x50`.
        StreamingScenario {
            name: "streaming/mulaw/stereo/8k/20ms/x50/eager",
            law: Law::Mulaw,
            channels: 2,
            sample_rate: 8_000,
            frame_count: 50,
            samples_per_channel: 160,
            seed: 0xFEED_FACE,
            eager_drain: true,
            iters_default: 250,
        },
        // 50 × 20 ms µ-law mono with deferred drain — queue all 50
        // packets first, drain after. Stresses the encoder
        // `VecDeque<Packet>` at depth 50 and the decoder `pending`
        // slot at one packet at a time. Matches
        // `streaming_mulaw_mono_8k_20ms_x50_deferred`.
        StreamingScenario {
            name: "streaming/mulaw/mono/8k/20ms/x50/deferred",
            law: Law::Mulaw,
            channels: 1,
            sample_rate: 8_000,
            frame_count: 50,
            samples_per_channel: 160,
            seed: 0xDEAD_BEEF,
            eager_drain: false,
            iters_default: 500,
        },
        // 100 × 10 ms A-law 8 ch at 48 kHz — OTT-grade rate × max
        // channels, largest cumulative working set. Matches
        // `streaming_alaw_8ch_48k_10ms_x100`.
        StreamingScenario {
            name: "streaming/alaw/8ch/48k/10ms/x100/eager",
            law: Law::Alaw,
            channels: 8,
            sample_rate: 48_000,
            frame_count: 100,
            samples_per_channel: 480,
            seed: 0x1234_5678,
            eager_drain: true,
            iters_default: 25,
        },
    ]
}

/// One pre-built frame: LE S16 byte payload + samples-per-channel
/// count + pts. Pre-built so the timed region inside the inner loop
/// does not include PCM synthesis cost (we measure the trait
/// surface, not the seeded RNG). Mirrors the `Burst` struct in the
/// r206 streaming bench.
struct ProfileBurst {
    bytes: Vec<u8>,
    samples_per_channel: u32,
    pts: i64,
}

/// Build `frame_count` PCM frames each of `samples_per_channel`
/// samples across `channels` interleaved channels, monotonically
/// incrementing pts so the profile mode also exercises
/// pts-propagation through the encoder queue. Byte-identical to the
/// `build_burst` helper in `benches/streaming.rs`.
fn build_profile_burst(
    frame_count: usize,
    samples_per_channel: u32,
    channels: u16,
    seed: u32,
) -> Vec<ProfileBurst> {
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
        out.push(ProfileBurst {
            bytes,
            samples_per_channel,
            pts,
        });
        pts = pts.saturating_add(samples_per_channel as i64);
    }
    out
}

fn streaming_params(scen: &StreamingScenario) -> CodecParameters {
    let id = match scen.law {
        Law::Mulaw => "pcm_mulaw",
        Law::Alaw => "pcm_alaw",
    };
    let mut p = CodecParameters::audio(CodecId::new(id));
    p.sample_rate = Some(scen.sample_rate);
    p.channels = Some(scen.channels);
    p.sample_format = Some(SampleFormat::S16);
    p
}

/// Streaming mode — one encoder + decoder pair reused across a burst
/// of frames. Mirrors the r206 `benches/streaming.rs` Criterion
/// scenarios and the r201 `streaming_pipeline` fuzz target. Two
/// shapes per scenario:
///
///   - eager drain: `send_frame` → `receive_packet` → `send_packet`
///     → `receive_frame` per frame (the canonical streaming session
///     pattern a real PSTN caller pays).
///   - deferred drain: queue every input, then drain every output
///     (stresses the encoder `VecDeque<Packet>` at depth =
///     `frame_count`).
///
/// The pair is built ONCE before the timed loop and reused across
/// iters — same shape as `iter_batched(BatchSize::SmallInput)` in
/// the r206 bench — so the profile capture reflects steady-state
/// per-frame queue traversal cost rather than amortised factory
/// cost.
fn profile_streaming(iters_override: Option<u32>) {
    println!("== streaming ==");
    for scen in streaming_scenarios() {
        let iters = iters_override.unwrap_or(scen.iters_default);
        let burst = build_profile_burst(
            scen.frame_count,
            scen.samples_per_channel,
            scen.channels,
            scen.seed,
        );
        // Per-iter byte count = sum of PCM input bytes across all
        // frames in the burst. Same throughput convention as the
        // r206 bench.
        let bytes_per_iter: usize = burst.iter().map(|b| b.bytes.len()).sum();
        let p = streaming_params(scen);

        // Warm-up: one full burst through a fresh pair so the
        // allocator and LUT cache lines are hot before the timed
        // loop starts.
        {
            let mut enc = match scen.law {
                Law::Mulaw => mulaw::make_encoder(&p).expect("make_encoder"),
                Law::Alaw => alaw::make_encoder(&p).expect("make_encoder"),
            };
            let mut dec = match scen.law {
                Law::Mulaw => mulaw::make_decoder(&p).expect("make_decoder"),
                Law::Alaw => alaw::make_decoder(&p).expect("make_decoder"),
            };
            if scen.eager_drain {
                let warm = drive_burst_eager(&mut enc, &mut dec, &burst, scen.sample_rate);
                std::hint::black_box(warm);
            } else {
                let warm = drive_burst_deferred(&mut enc, &mut dec, &burst, scen.sample_rate);
                std::hint::black_box(warm);
            }
        }

        let t = Instant::now();
        for _ in 0..iters {
            // Pair is rebuilt per outer iter so each iter is a fresh
            // session lifecycle — matches the r206 bench's
            // `iter_batched` shape. The factory cost is small
            // (single allocation + parameter copy) so it does not
            // dominate; the burst loop inside is what we measure.
            let mut enc = match scen.law {
                Law::Mulaw => mulaw::make_encoder(&p).expect("make_encoder"),
                Law::Alaw => alaw::make_encoder(&p).expect("make_encoder"),
            };
            let mut dec = match scen.law {
                Law::Mulaw => mulaw::make_decoder(&p).expect("make_decoder"),
                Law::Alaw => alaw::make_decoder(&p).expect("make_decoder"),
            };
            let n = if scen.eager_drain {
                drive_burst_eager(&mut enc, &mut dec, &burst, scen.sample_rate)
            } else {
                drive_burst_deferred(&mut enc, &mut dec, &burst, scen.sample_rate)
            };
            std::hint::black_box(n);
        }
        let elapsed = t.elapsed().as_secs_f64();
        print_throughput_line("streaming", scen.name, iters, bytes_per_iter, elapsed);

        std::io::stdout().flush().ok();
    }
}

/// Drive the burst eagerly: `send_frame` → `receive_packet` →
/// `send_packet` → `receive_frame` per frame, returning the byte
/// count actually decoded so `black_box` can stop the optimiser
/// collapsing the loop. Byte-identical to `drive_session_eager` in
/// `benches/streaming.rs`.
fn drive_burst_eager(
    enc: &mut Box<dyn oxideav_core::Encoder>,
    dec: &mut Box<dyn oxideav_core::Decoder>,
    burst: &[ProfileBurst],
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

/// Deferred-drain variant: queue every input, then drain every
/// output. Byte-identical to `drive_session_deferred` in
/// `benches/streaming.rs`.
fn drive_burst_deferred(
    enc: &mut Box<dyn oxideav_core::Encoder>,
    dec: &mut Box<dyn oxideav_core::Decoder>,
    burst: &[ProfileBurst],
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

fn main() {
    let mut args = env::args().skip(1);
    let mode = args.next().unwrap_or_else(|| "all".to_string());
    let iters_override: Option<u32> = args.next().and_then(|s| s.parse().ok());

    println!(
        "=== oxideav-g711 profile (mode={mode}, iters={}) ===",
        iters_override
            .map(|n| n.to_string())
            .unwrap_or_else(|| "default".to_string()),
    );
    println!();

    match mode.as_str() {
        "decode" => profile_decode(iters_override),
        "encode" => profile_encode(iters_override),
        "roundtrip" => profile_roundtrip(iters_override),
        "streaming" => profile_streaming(iters_override),
        "all" => {
            profile_decode(iters_override);
            println!();
            profile_encode(iters_override);
            println!();
            profile_roundtrip(iters_override);
            println!();
            profile_streaming(iters_override);
        }
        other => {
            eprintln!("unknown mode: {other:?}");
            eprintln!("usage: profile_g711 [decode|encode|roundtrip|streaming|all] [<iters>]",);
            std::process::exit(2);
        }
    }
}
