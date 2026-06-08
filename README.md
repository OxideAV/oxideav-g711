# oxideav-g711

Pure-Rust **ITU-T G.711** codec — both µ-law and A-law variants,
decoder + encoder. Spec-exact lookup tables, deterministic per-sample
companding, bit-exact against the reference formulas in G.711 §2 / §3.
Zero C dependencies, no FFI, no `*-sys` crates.

Part of the [oxideav](https://github.com/OxideAV/oxideav-workspace)
framework but usable standalone.

## Installation

```toml
[dependencies]
oxideav-core = "0.1"
oxideav-codec = "0.1"
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

Encoder is symmetric — build with `reg.make_encoder(&params)`, feed
`Frame::Audio` with S16 PCM, get companded `Packet`s back. One output
byte per input S16 sample, preserving interleave order across channels.

### Codec IDs

- **µ-law**: `"pcm_mulaw"` (aliases: `"g711u"`, `"ulaw"`)
- **A-law**: `"pcm_alaw"` (aliases: `"g711a"`, `"alaw"`)

### Channels and sample rate

Both variants accept any channel count ≥ 1 and any sample rate. The
packet-to-frame mapping assumes interleaved bytes in the same order as
the S16 PCM: `ch0 ch1 … chN ch0 ch1 …`. Decoder packets whose length
is not a multiple of the channel count are rejected.

### Going deeper — bypassing the registry

If you just want the byte-to-sample mapping, skip the trait surface
entirely:

```rust
use oxideav_g711::{mulaw, alaw};

let pcm: i16 = mulaw::decode_sample(0x7F);
let byte: u8  = alaw::encode_sample(12345);
```

The `UlawDecoder` / `UlawEncoder` / `AlawDecoder` / `AlawEncoder`
structs are also publicly constructible via `mulaw::make_decoder` /
`alaw::make_decoder` / etc. for cases where you want full control over
construction without the registry lookup.

### Encode hot path — compile-time S16 → byte LUTs

`mulaw::encode_sample` and `alaw::encode_sample` index 64 KiB
compile-time tables (`tables::MULAW_ENCODE`, `tables::ALAW_ENCODE`,
both `[u8; 65536]`). The entries are produced by running the
arithmetic encoders inside a `const fn` loop, so each LUT is
**bit-exact-by-construction** relative to the spec formulas in §2 /
§3 — a regression test (`mulaw_lut_matches_arith_for_every_sample`
/ `alaw_lut_matches_arith_for_every_sample`) pins this equality on
every one of the 65536 entries on every CI run. Callers that need
the formula path without linking the table in can call
`mulaw::encode_sample_arith` / `alaw::encode_sample_arith` instead;
they delegate to the same `const fn` that populates the LUT, so
the result is the same byte.

### Properties verified by the test suite

- All 13 fixtures in [`docs/audio/g711/fixtures/`](../../docs/audio/g711/fixtures/)
  are CI-gated at `Tier::BitExact` (r218 promotion). Every fixture
  round-trips at 100.0000% sample-exact (RMS 0.000, max |diff| 0,
  length-match true) on both debug and release. Covers A-law /
  µ-law × mono / stereo / 8 ch wideband, 8 kHz / 16 kHz sample rates,
  silence + DC saturation extremes, full-range sine sweeps, RIFF WAVE +
  Sun .au container dispatch, and the containerless raw vs. WAV
  equivalence pair (both branches). A future divergence in either the
  per-byte LUT, the trait-surface framing, or the in-test container
  parsers fails CI immediately.
- Decode tables match the ITU-T G.711 §2 / §3 reference formulas for
  all 256 bytes of both laws.
- Encode is bit-exact against the reference Sun/ANSI formulas for all
  65 536 S16 inputs of both laws.
- Encode / decode / encode is idempotent for every S16 sample (the
  quantiser is a projection).
- Decode / encode is a round-trip identity for every byte, except µ-law
  bytes 0x7F (negative zero) which the encoder canonicalises to 0xFF
  (positive zero) — both decode to linear 0 per the spec.
- Sign symmetry: byte `b` and byte `b ^ 0x80` decode to exact negatives.
- Monotonicity: the companded transfer function is monotonic across
  the whole S16 range.
- Multichannel round-trip (1, 2, 6, 8 channels) through the trait
  surface returns the same per-sample quantisation as direct calls.
- **Per-sample quantization bound**: every S16 input round-trips
  within the spec-derived per-segment step bound (full sweep gated on
  `cfg(not(debug_assertions))` so `cargo test --release` exercises all
  65 536 samples; sparse stride runs in debug). Worst-case observed
  error for both laws lives in the saturation band above ±32256
  (A-law) / ±32635 (µ-law), as predicted by the spec.
- **PSNR floor**: 1-second sinusoids at 400 Hz / 1 kHz / 2 kHz at
  -3 dBFS round-trip with PSNR ≥ 35 dB (measured: µ-law ~41–47 dB,
  A-law ~39–49 dB). Comfortably above the ~38 dB SQNR design point
  the G.711 staged Recommendation cites for voice-band tones.

## Benchmarks

Criterion bench harnesses ship under `benches/` for the per-sample
LUT decode + arithmetic encode hot paths and for the trait-surface
Decoder/Encoder framing overhead. Every input is synthesised
in-bench from a deterministic xorshift seed, so the scenarios are
self-contained — no fixture files, no external corpus.

```sh
cargo bench -p oxideav-g711 --bench decode
cargo bench -p oxideav-g711 --bench encode
cargo bench -p oxideav-g711 --bench roundtrip
cargo bench -p oxideav-g711 --bench streaming
cargo bench -p oxideav-g711 --bench voice
```

The first four use a uniform-random xorshift32 input distribution
(every segment of the curve sees equal traffic — the canonical
worst-case for cache-pressure regressions). **r247** added a fifth
bench, `voice`, that drives the same per-sample + trait-surface
hot paths from a closed-form Laplacian generator concentrated near
zero — ~80% of samples land in segments 0..=2 (|s| ≤ 1024), so the
encode 64 KiB LUT touches primarily its low-magnitude quadrants
and the segment-search arith path hits the segment-0 fast exit on
the same 80%. Measured on aarch64-darwin (release, 2 s window):
the voice-distribution rows land within a few percent of their
uniform counterparts (decode LUT ≈ 5.5 GiB/s both laws; encode
LUT µ-law ≈ 9.5 GiB/s / A-law ≈ 10.9 GiB/s; encode arith µ-law ≈
1.49 GiB/s / A-law ≈ 1.72 GiB/s; roundtrip mono 8 kHz ≈
3.1–3.3 GiB/s), confirming the LUT is cache-line dense enough that
input-distribution locality does not dominate per-sample wall
time. A future regression that splits a hot cache line — e.g. a
SIMD gather that pulls non-contiguous entries — would show up
disproportionately on the voice rows.

Per-sample LUT decode tops out around 5.5 GiB/s (µ-law and A-law
roughly tied). Encode runs through the **r230** compile-time
S16 → byte LUTs (`MULAW_ENCODE` / `ALAW_ENCODE`, 64 KiB each) — the
per-sample inner loop hits **≈ 11.1 GiB/s** for both laws (vs. the
~1.5 GiB/s the pre-r230 arithmetic segment-search loop measured on
the same host). The matching `encode_sample_arith` helpers still
ship the formula path for callers that want the spec mechanics
without the table linked in.

**r236** reshaped the four trait-surface inner loops to pre-size
the output `Vec<u8>` and zip the source against an `iter_mut()` /
`chunks_exact_mut(2)` destination, replacing the pre-r236
`Vec::push` (encode) and `Vec::extend_from_slice(&s.to_le_bytes())`
(decode) per-iter patterns. That gives the codegen a single
slice-load + slice-store pair per sample with no bounds-check chain
or 2-byte temporary, so LLVM lifts the loop into wider stores on
aarch64. Measured on aarch64-darwin (release, 3 s Criterion window):

| scenario | pre-r236 | post-r236 |
| --- | --- | --- |
| decode trait mono 8 kHz | 2.6 GiB/s | **3.76 GiB/s** (+44%) |
| decode trait stereo 8 kHz | 2.7 GiB/s | **3.88 GiB/s** (+44%) |
| decode trait 8 ch / 48 kHz | 2.5 GiB/s | **3.76 GiB/s** (+51%) |
| encode trait mono 8 kHz | 3.85 GiB/s | **5.64 GiB/s** (+47%) |
| encode trait stereo 8 kHz | 3.65 GiB/s | **5.29 GiB/s** (+45%) |
| encode trait 8 ch / 48 kHz | 3.79 GiB/s | **5.86 GiB/s** (+55%) |
| roundtrip mulaw mono 8 kHz | 2.1 GiB/s | **3.13 GiB/s** (+49%) |
| roundtrip alaw 8 ch / 48 kHz | 2.2 GiB/s | **3.35 GiB/s** (+52%) |
| streaming alaw 8 ch / 48 kHz | 2.22 GiB/s | **3.25 GiB/s** (+46%) |

The per-sample LUT rows are unchanged (~5.5 GiB/s decode /
~11.1 GiB/s encode) — they were already a single slice-load + push
pair; the r236 win comes entirely from the framing-wrapper code
moving from `push` / `extend_from_slice` to indexed writes against
a pre-sized buffer. The r206 **streaming** bench (one encoder +
decoder pair across a 50-frame PSTN 20 ms burst) now lands at
**1.71–3.25 GiB/s** end-to-end across the five scenarios (was
1.05–2.10 GiB/s pre-r236, +35–46%). Run the benches again after
any change to the LUT generators, the inner slice load, or the
encoder/decoder queue management to spot regressions.

## Fuzzing

A libFuzzer-driven [`fuzz/`](fuzz/) package ships six targets that
exercise the framing wrapper, the parameter-validation surface,
per-sample invariants, and the cross-law transcoding lifecycle as
panic- / UB-freedom contracts. The exhaustive bit-exact reference
test already pins every encode and decode codepoint individually;
the fuzzer's job is to drive **the trait-surface wrapper** at
attacker-chosen channel counts, packet shapes, and sample rates,
plus the per-sample helpers across (i16 sample, u8 codeword, law)
triples the unit tests do not directly hammer.

- `decode_pipeline` — arbitrary bytes through both decoders at
  attacker channel counts (1..=8); empty / odd-length / repeated-
  send / post-flush paths.
- `encode_pipeline` — arbitrary i16 PCM through both encoders, then
  back through the matching decoder; the trait-surface result must
  equal the per-sample `decode_sample(encode_sample(s))` baseline.
- `per_sample_invariants` — projection idempotence (with the
  documented µ-law −0/+0 collapse), sign symmetry, and the
  spec-derived per-segment quantisation-step bound applied to every
  attacker-chosen sample.
- `streaming_pipeline` — multi-frame / multi-packet session through
  one encoder + decoder pair: drives 1..=8 frames per session at
  attacker-chosen channels + sample rate + drain order (eager vs.
  deferred), with mid-stream decoder `flush()` interleaved. Asserts
  the encoder queue is FIFO + fully drained, pts propagates intact
  across the encode → decode boundary, the decoder's `pending` slot
  always advances cleanly across repeat cycles, and post-stream the
  decoder returns `NeedMore` (pre-flush) / `Eof` (post-flush) as
  the trait surface contracts demand.
- `factory_params` — adversarial `CodecParameters` shapes through
  all four `make_decoder` / `make_encoder` entry points (µ-law +
  A-law, decoder + encoder). Drives the rejection branches the
  other four targets do not reach: `channels == 0`, every named
  `SampleFormat` variant on the encoder's non-`S16` rejection
  ladder (`U8`, `S8`, `S24`, `S32`, `F32`, `F64` plus four planar
  shapes), free-form `codec_id` strings (empty, mismatched law,
  arbitrary), and the full `u32` sample-rate range including `0`
  and `u32::MAX` (the latter must not overflow when the encoder
  casts it to `i64` for its `TimeBase`). Successful constructions
  are exercised with a small attacker payload to confirm the
  produced trait object survives one send / receive / flush cycle
  cleanly.
- `cross_law_transcode` (r262) — cross-law PSTN-gateway
  transcoding lifecycle: decode incoming bytes under one law,
  re-encode the recovered PCM as the *other* law, then optionally
  decode + re-encode in reverse for the full A↔µ↔A roundtrip. The
  first five targets all stay within one law per pipeline; this
  target is the first to assert that **two different-law trait
  objects compose correctly through the trait surface** at the byte
  level. The cross-law output must equal
  `encode_B(decode_A(b))` applied byte-by-byte to every input byte
  (and the reverse must equal the analogous double-transcode
  per-sample baseline) — any framing-level reorder, padded sample,
  or future per-frame state addition that coupled adjacent samples
  surfaces as a divergent byte on the very first transcoded
  position.

```sh
cargo +nightly fuzz run decode_pipeline
cargo +nightly fuzz run encode_pipeline
cargo +nightly fuzz run per_sample_invariants
cargo +nightly fuzz run streaming_pipeline
cargo +nightly fuzz run factory_params
cargo +nightly fuzz run cross_law_transcode
```

Requires the [`cargo-fuzz`](https://github.com/rust-fuzz/cargo-fuzz)
sub-command and a nightly toolchain (libFuzzer needs `-Zsanitizer`).
Initial runs cleared 20 000–80 000 iterations per target on
aarch64-darwin for the first three targets; the r201 streaming
target cleared **7 066 742 iterations / 60 s clean** on the same
host (≈ 115 k exec/s, 433 cov / 1563 ft saturation); the r224
`factory_params` target cleared **18 096 276 iterations / 60 s
clean** on the same host (≈ 297 k exec/s, 399 cov / 628 ft
saturation across 160 corpus entries); the r262
`cross_law_transcode` target cleared **2 594 496 iterations / 31 s
clean** on the same host (≈ 83.7 k exec/s, 452 cov / 1107 ft
saturation across 52 corpus entries). Corpus and crash artifacts
live under `fuzz/` and are `.gitignore`d.

## Profiling

A flat profiling driver ships at `examples/profile_g711.rs` for
`samply` / `cargo flamegraph` / `perf record` capture. Criterion's
warm-up + sampling + estimator layers show up in those tools and
bury the real codec hot paths (LUT load, segment-search loop,
on-wire inversion); this driver instead runs a fixed iteration
count with a single `Instant::now()` / `elapsed()` pair around the
whole pass. The five scenarios mirror the Criterion benches byte-
for-byte — same xorshift32 seeds — so a profile capture and a
bench row correspond directly. Each pass prints its own throughput
line so the binary also doubles as a quick A/B harness for the
inner tweak-remeasure loop when Criterion's per-run overhead is
too coarse.

```sh
cargo build --example profile_g711 --release
samply record -- ./target/release/examples/profile_g711 encode 5000
samply record -- ./target/release/examples/profile_g711 decode 5000

# or with cargo flamegraph (needs `cargo install flamegraph`):
cargo flamegraph --example profile_g711 -- roundtrip 5000
```

Modes are `decode` / `encode` / `roundtrip` / `streaming` / `all`
(default). Decode rows walk both the per-sample LUT path and the
trait-surface Decoder path so the gap between them isolates the
framing cost from the inner-loop cost; encode rows do the same for
the arithmetic encode path vs. the trait-surface Encoder.

The `streaming` mode (added in r213) reuses one encoder + decoder
pair across a multi-frame burst — the canonical PSTN session shape
the per-call `decode` / `encode` / `roundtrip` modes can't capture,
because those rebuild the pair per iter and so charge factory cost
in every sample. It mirrors the r206 `benches/streaming.rs`
Criterion scenarios byte-for-byte (50 × 20 ms µ-law mono / A-law
mono / µ-law stereo at 8 kHz; 50 × 20 ms µ-law mono with deferred
drain — queue all 50 packets first, drain after — exercising the
encoder `VecDeque<Packet>` at depth 50; 100 × 10 ms 8-channel A-law
at 48 kHz) and the r201 `streaming_pipeline` fuzz target's
lifecycle, so a samply / flamegraph capture of the streaming mode
lines up with the matching `streaming_*` bench row and the matching
fuzz-target code path directly. Measured on aarch64-darwin: ≈
680–980 MiB/s across the five rows, headlining the OTT-grade A-law
8 ch / 48 kHz scenario at ≈ 980 MiB/s.

```sh
samply record -- ./target/release/examples/profile_g711 streaming 500
```

## License

MIT — see [LICENSE](LICENSE).
