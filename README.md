# oxideav-g711

Pure-Rust **ITU-T G.711** codec — both µ-law and A-law variants,
decoder + encoder. Spec-exact lookup tables, deterministic per-sample
companding, bit-exact against the formulas in G.711 §2 / §3. Zero C
dependencies, no FFI, no `*-sys` crates.

Part of the [oxideav](https://github.com/OxideAV/oxideav-workspace)
framework but usable standalone.

## Installation

```toml
[dependencies]
oxideav-core = "0.1"
oxideav-g711 = "0.0"
```

## Quick use

G.711 is 1 byte per input sample, stateless. Every encoded byte
decodes to exactly one S16 PCM sample and vice versa. The spec defines
it at 8 kHz mono (PSTN) but the implementation is rate- and
channel-independent — interleaved S16 input with any channel count
round-trips through the same byte-per-sample mapping.

```rust
use oxideav_core::{CodecId, CodecParameters, Frame, Packet, RuntimeContext, TimeBase};

let mut ctx = RuntimeContext::new();
oxideav_g711::register(&mut ctx);

let mut params = CodecParameters::audio(CodecId::new("pcm_mulaw"));
params.sample_rate = Some(8_000);
params.channels = Some(1);

let mut dec = ctx.codecs.make_decoder(&params)?;
dec.send_packet(&Packet::new(0, TimeBase::new(1, 8_000), mulaw_bytes))?;
let Frame::Audio(a) = dec.receive_frame()? else { unreachable!() };
// `a.data[0]` is interleaved S16 PCM.
# Ok::<(), oxideav_core::Error>(())
```

Encoder is symmetric — build with `ctx.codecs.make_encoder(&params)`,
feed `Frame::Audio` with S16 PCM, get companded `Packet`s back. One
output byte per input S16 sample, preserving interleave order across
channels.

### Codec IDs

- **µ-law**: `"pcm_mulaw"` (aliases: `"g711u"`, `"ulaw"`)
- **A-law**: `"pcm_alaw"` (aliases: `"g711a"`, `"alaw"`)

### Channels and sample rate

Both variants accept any channel count ≥ 1 and any sample rate. The
packet-to-frame mapping assumes interleaved bytes in the same order as
the S16 PCM: `ch0 ch1 … chN ch0 ch1 …`. Decoder packets whose length
is not a multiple of the channel count are rejected.

### Bypassing the registry

If you just want the byte-to-sample mapping, skip the trait surface
entirely:

```rust
use oxideav_g711::{mulaw, alaw};

let pcm: i16 = mulaw::decode_sample(0x7F);
let byte: u8  = alaw::encode_sample(12345);
```

The `UlawDecoder` / `UlawEncoder` / `AlawDecoder` / `AlawEncoder`
structs are also constructible via `mulaw::make_decoder` /
`alaw::make_decoder` / etc. for full control over construction without
the registry lookup.

### Encode hot path — compile-time S16 → byte LUTs

`mulaw::encode_sample` and `alaw::encode_sample` index 64 KiB
compile-time tables (`tables::MULAW_ENCODE`, `tables::ALAW_ENCODE`,
both `[u8; 65536]`). The entries are produced by running the
arithmetic encoders inside a `const fn` loop, so each LUT is
bit-exact-by-construction relative to the spec formulas in §2 / §3 —
a regression test pins this equality on all 65536 entries every CI
run. Callers that need the formula path without linking the table can
call `mulaw::encode_sample_arith` / `alaw::encode_sample_arith`
instead; they delegate to the same `const fn` that populates the LUT.

## Properties verified by the test suite

- All 13 fixtures in [`docs/audio/g711/fixtures/`](../../docs/audio/g711/fixtures/)
  are CI-gated at `Tier::BitExact`: every fixture round-trips at
  100.0000% sample-exact (RMS 0.000, max |diff| 0) on both debug and
  release. Covers A-law / µ-law × mono / stereo / 8 ch, 8 kHz /
  16 kHz, silence + DC saturation extremes, full-range sine sweeps,
  RIFF WAVE + Sun .au container dispatch, and the containerless raw
  vs. WAV equivalence pair.
- Decode tables match the ITU-T G.711 §2 / §3 formulas for all 256
  bytes of both laws; encode is bit-exact against the spec formulas
  for all 65 536 S16 inputs of both laws.
- Encode / decode / encode is idempotent for every S16 sample.
- Decode / encode is a round-trip identity for every byte, except
  µ-law byte 0x7F (negative zero) which the encoder canonicalises to
  0xFF (positive zero) — both decode to linear 0.
- Sign symmetry: byte `b` and `b ^ 0x80` decode to exact negatives;
  the companded transfer function is monotonic across the S16 range.
- Multichannel round-trip (1, 2, 6, 8 channels) through the trait
  surface returns the same per-sample quantisation as direct calls.
- **Cross-law (µ ↔ A) transcode**: the PSTN-gateway pipeline —
  decode under one law, re-encode the recovered PCM as the other law
  — equals the per-sample baseline `encode_B(decode_A(b))`
  byte-for-byte for all 256 codewords in both directions, across
  1 / 2 / 6 / 8 channels, including the full A → B → A reverse and
  tandem double-hop idempotence.
- **Normative µ↔A conversion**: the transcode reproduces the G.711
  §3.5 Table 3 (µ→A) / Table 4 (A→µ) value-number correspondence for
  all 128 levels of both laws, both directions, both signs, including
  the high-segment jumps and the §3.6 Note 2 tandem-transparency
  claims.
- **Per-sample quantization bound**: every S16 input round-trips
  within the spec-derived per-segment step bound (full 65 536-sample
  sweep gated on release builds; sparse stride in debug).
- **PSNR floor**: 1-second sinusoids at 400 Hz / 1 kHz / 2 kHz at
  -3 dBFS round-trip with PSNR ≥ 35 dB (measured: µ-law ~41–47 dB,
  A-law ~39–49 dB).

## Benchmarks

Six Criterion harnesses ship under `benches/` for the per-sample LUT
decode + arithmetic encode hot paths and the trait-surface framing
overhead. Every input is synthesised in-bench from a deterministic
xorshift seed — no fixture files, no external corpus.

```sh
cargo bench -p oxideav-g711 --bench {decode,encode,roundtrip,streaming,voice,segment}
```

`decode` / `encode` / `roundtrip` / `streaming` use a uniform-random
distribution; `voice` uses a Laplacian generator concentrated near
zero (exercising the segment-0 fast exit); `segment` confines every
sample to the top segment (the branch-history mirror of `voice`).
Together they pin all three corners of the input-distribution space.
A consolidated cross-distribution baseline table lives in
[`BENCHMARKS.md`](BENCHMARKS.md).

## Fuzzing

A libFuzzer-driven [`fuzz/`](fuzz/) package ships six targets that
exercise the framing wrapper, parameter-validation surface, per-sample
invariants, and the cross-law transcoding lifecycle as panic-/UB-freedom
contracts: `decode_pipeline`, `encode_pipeline`, `per_sample_invariants`,
`streaming_pipeline`, `factory_params`, `cross_law_transcode`.

```sh
cargo +nightly fuzz run <target>
```

Requires the [`cargo-fuzz`](https://github.com/rust-fuzz/cargo-fuzz)
sub-command and a nightly toolchain. Corpus and crash artifacts live
under `fuzz/` and are `.gitignore`d.

## Profiling

A flat profiling driver ships at `examples/profile_g711.rs` for
`samply` / `cargo flamegraph` / `perf record` capture — a fixed
iteration count with a single `Instant::now()` / `elapsed()` pair
around the whole pass, mirroring the Criterion bench seeds so a
profile capture and a bench row correspond directly.

```sh
cargo build --example profile_g711 --release
samply record -- ./target/release/examples/profile_g711 <mode> 5000
# modes: decode | encode | roundtrip | streaming | all (default)
```

## License

MIT — see [LICENSE](LICENSE).
