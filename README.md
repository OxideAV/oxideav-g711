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

### Properties verified by the test suite

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
```

Per-sample LUT decode tops out around 5.5 GiB/s (µ-law and A-law
roughly tied); per-sample arithmetic encode is ~1.5 GiB/s for µ-law
and ~1.4 GiB/s for A-law (segment-search loop, not LUT-bound). The
full trait-surface encode→decode round-trip lands around 1 GiB/s
on aarch64-darwin including factory construction. The r206
**streaming** bench amortises construction across a 50-frame PSTN
20 ms burst and tops out around 760–985 MiB/s end-to-end depending
on channel count + rate (the per-frame trait-surface overhead is
what shows up once factory cost is amortised). Run them again
after any change to the encode segment search, the inner LUT
load, or the encoder/decoder queue management to spot regressions.

## Fuzzing

A libFuzzer-driven [`fuzz/`](fuzz/) package ships three targets that
exercise the framing wrapper and per-sample invariants as panic- /
UB-freedom contracts. The exhaustive bit-exact reference test
already pins every encode and decode codepoint individually; the
fuzzer's job is to drive **the trait-surface wrapper** at attacker-
chosen channel counts, packet shapes, and sample rates, plus the
per-sample helpers across (i16 sample, u8 codeword, law) triples
the unit tests do not directly hammer.

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

```sh
cargo +nightly fuzz run decode_pipeline
cargo +nightly fuzz run encode_pipeline
cargo +nightly fuzz run per_sample_invariants
cargo +nightly fuzz run streaming_pipeline
```

Requires the [`cargo-fuzz`](https://github.com/rust-fuzz/cargo-fuzz)
sub-command and a nightly toolchain (libFuzzer needs `-Zsanitizer`).
Initial runs cleared 20 000–80 000 iterations per target on
aarch64-darwin for the first three targets; the r201 streaming
target cleared **7 066 742 iterations / 60 s clean** on the same
host (≈ 115 k exec/s, 433 cov / 1563 ft saturation). Corpus and
crash artifacts live under `fuzz/` and are `.gitignore`d.

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
