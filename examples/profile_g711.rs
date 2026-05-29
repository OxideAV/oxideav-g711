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
        "all" => {
            profile_decode(iters_override);
            println!();
            profile_encode(iters_override);
            println!();
            profile_roundtrip(iters_override);
        }
        other => {
            eprintln!("unknown mode: {other:?}");
            eprintln!("usage: profile_g711 [decode|encode|roundtrip|all] [<iters>]");
            std::process::exit(2);
        }
    }
}
