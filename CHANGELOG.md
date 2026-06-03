# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Other

- fuzz: new `factory_params` target (r224) — fifth libFuzzer harness
  exercising the parameter-validation surface across all four
  `make_decoder` / `make_encoder` entry points (µ-law + A-law,
  decoder + encoder). Targets the three rejection branches the
  existing four targets do not reach: `channels == Some(0)`, every
  named `SampleFormat` variant on the encoder's non-`S16` rejection
  ladder (`U8`, `S8`, `S24`, `S32`, `F32`, `F64`, plus `U8P`,
  `S16P`, `S32P`, `F32P`, `F64P`), and free-form `codec_id` strings
  (empty, mismatched law, arbitrary token). Also covers the full
  `u32` sample-rate range including `0` and `u32::MAX` — the latter
  must not overflow when the encoder casts it to `i64` while
  constructing its `TimeBase`. Successful constructions are
  exercised with a small attacker payload through one send /
  receive / flush cycle to confirm the produced trait object itself
  stays total — not just the factory call. Cleared
  **18 096 276 iterations / 60 s clean** on aarch64-darwin nightly
  (≈ 297 k exec/s, 399 cov / 628 ft saturation across 160 corpus
  entries), no panics, no aborts. Per the round-selection memory's
  "ONE of fuzz / bench / profile per round" rule for saturated
  codecs (already saturated at four fuzz targets, four criterion
  bench files, and a flat profiling driver), this is the
  parameter-surface depth-mode pick: previous fuzz coverage had
  channels pre-clamped to `1..=8` and `sample_format` always
  defaulted (`None`), leaving the encoder's twelve `SampleFormat`
  rejection branches and the channels-zero branch dark. The new
  target makes the parameter-validation surface a regression-pinned
  property the same way the per-sample math and the framing
  wrapper already are.
- tests: promote every entry in `tests/docs_corpus.rs` from
  `Tier::ReportOnly` to `Tier::BitExact` (r218). The r205 corpus
  integration brief explicitly says fixtures graduate "in the very
  next round" once CI confirms a 100.00% match — r217 confirmed
  every fixture lands at 100.0000% sample-exact (RMS 0.000, max
  |diff| 0, length-match true) on both debug and release builds.
  All 13 entries now hard-assert on (a) byte-length equality between
  decoder output and reference PCM and (b) per-sample bit-exact
  match across every channel. The promotion covers: A-law mono
  8 kHz / 16 kHz / stereo 8 kHz; µ-law mono 8 kHz; silence + DC
  saturation extremes (`silence-alaw`, `silence-mulaw`,
  `dc-positive-alaw`, `dc-negative-alaw`); 1 kHz full-range sine
  sweeps (A-law + µ-law); RIFF WAVE codec_tag dispatch (0x0006 +
  0x0007) and Sun .au encoding dispatch (1 + 27) through their
  respective container parsers; and the containerless raw-vs-WAV
  equivalence pair on both branches — the raw branch was previously
  the only test that logged-without-failing, now it asserts the same
  way the wav branch does. Per the round-selection memory's "ONE of
  fuzz / bench / profile per round" rule for saturated codecs this
  is the natural depth-mode follow-up: the existing fuzz / bench /
  profile harnesses already exercise the trait surface end-to-end,
  what was still ReportOnly was the in-tree corpus's gating tier.
  No code change in `src/` — the same decoder paths that
  successfully passed in ReportOnly mode are now CI-gated against
  regression. `Tier::ReportOnly` is kept in the enum so any future
  fixture added under investigation can ladder up the same way.
- examples: extend `profile_g711.rs` with a `streaming` mode (r213)
  mirroring the r206 `benches/streaming.rs` Criterion scenarios
  byte-for-byte (50 × 20 ms µ-law mono / A-law mono / µ-law stereo
  at 8 kHz; 50 × 20 ms µ-law mono deferred-drain; 100 × 10 ms
  8-channel A-law at 48 kHz — same frame counts, same channel
  layouts, same xorshift32 seeds as the benches). The existing four
  profile modes (`decode`, `encode`, `roundtrip`, `all`) build a
  fresh encoder + decoder pair per iter, which is the right shape
  for the r173 per-call benches but a poor profile target for
  steady-state PSTN sessions where one pair is reused across
  hundreds of frames and the cost that dominates is queue
  traversal (encoder `VecDeque<Packet>` + decoder
  `pending: Option<Packet>` slot) plus per-frame interleave + LE
  byte unpack. The new mode builds the pair once outside the timed
  region — same shape as `iter_batched(BatchSize::SmallInput)` in
  the r206 bench — and drives a configurable burst inside, so a
  samply / flamegraph capture of the streaming mode lines up 1:1
  with the matching `streaming_*` bench rows and with the r201
  `streaming_pipeline` fuzz target's code paths (eager vs. deferred
  drain, pts propagation through the encoder queue, decoder
  `pending` slot under repeated send/receive cycles). The `all`
  mode now runs streaming after roundtrip. No new dependencies, no
  fixture reads, no public-API changes; pure depth-mode addition
  per the round-selection memory's "ONE of fuzz / bench / profile
  per round" rule for saturated codecs. Measured on aarch64-darwin
  with default iter counts: streaming/mulaw/mono/8k/20ms/x50/eager
  ≈ 691 MiB/s, streaming/alaw/mono/8k/20ms/x50/eager ≈ 677 MiB/s,
  streaming/mulaw/stereo/8k/20ms/x50/eager ≈ 805 MiB/s,
  streaming/mulaw/mono/8k/20ms/x50/deferred ≈ 715 MiB/s,
  streaming/alaw/8ch/48k/10ms/x100/eager ≈ 980 MiB/s — in the same
  ballpark as the r206 bench rows on the same host. Run with
  `cargo run --example profile_g711 --release -- streaming [iters]`.
- benches: add `streaming` Criterion harness (r206) timing the
  one-encoder-and-decoder-pair-reused-across-many-frames pattern that
  real PSTN sessions actually exercise. The existing r173 `roundtrip`
  bench constructs a fresh encoder + decoder pair per `b.iter` and
  drives one frame through, which charges factory cost in every
  sample; the new streaming harness builds the pair ONCE outside the
  timed region (via `iter_batched`) and drives a configurable burst
  inside, so the row reflects steady-state per-frame queue traversal
  cost rather than amortised factory cost. Five scenarios mirror the
  r201 `streaming_pipeline` fuzz target's shapes byte-for-byte: 50 ×
  20 ms µ-law mono / A-law mono / µ-law stereo at 8 kHz (canonical
  PSTN packetisation — ITU-T G.711 §1 / RFC 3551 §4.5.14); 50 × 20 ms
  µ-law mono with deferred drain (queue all 50 packets first, drain
  after, exercising the encoder `VecDeque<Packet>` at depth 50); and
  100 × 10 ms 8-channel A-law at 48 kHz (largest cumulative working
  set, stressing queue + per-channel interleave together). All
  inputs are synthesised in-bench from xorshift32 seeds that match
  the r173 helpers — no `docs/` fixtures, no external corpus.
  Measured on aarch64-darwin: µ-law mono 8 kHz x50 ≈ 760 MiB/s,
  A-law mono 8 kHz x50 ≈ 749 MiB/s, µ-law stereo 8 kHz x50 ≈ 853
  MiB/s, µ-law mono deferred ≈ 745 MiB/s, A-law 8ch 48 kHz x100 ≈
  985 MiB/s. The deferred-vs-eager comparison gives future
  optimisation rounds (e.g. flattening the encoder queue to a single-
  slot Option or vectorising the per-frame interleave) a stable
  baseline to A/B against. Run with `cargo bench -p oxideav-g711
  --bench streaming`.
- fuzz: add `streaming_pipeline` libFuzzer target (r201) covering
  multi-frame / multi-packet sessions through a single encoder +
  decoder pair. Complements the r180 trio (`decode_pipeline`,
  `encode_pipeline`, `per_sample_invariants`) by exercising the
  cross-frame state the trait surface accumulates — encoder
  `VecDeque<Packet>` FIFO ordering (eager vs. deferred drain), pts
  propagation across many frames, decoder `pending` slot under
  repeated send/receive cycles, mid-stream `flush()` semantics, and
  the post-stream `NeedMore` vs. `Eof` contract on the decoder. The
  per-sample baseline (`decode_sample(encode_sample(s))` applied
  verbatim, the same oracle `encode_pipeline` uses) is asserted
  per-frame so any framing-level skew across the multi-frame
  lifecycle surfaces immediately. Attacker-controlled inputs: codec
  selector (µ-law / A-law), channels (1..=8), sample rate
  (1..=192 000), frame count (1..=8), per-frame samples/channel
  (0..=256), drain order (eager / deferred), mid-stream-flush flag.
  Runs clean against 7 066 742 iterations / 60 s on aarch64-darwin
  (≈115 k exec/s, 433 cov / 1563 ft saturation). Run with
  `cargo +nightly fuzz run streaming_pipeline`.

## [0.0.7](https://github.com/OxideAV/oxideav-g711/compare/v0.0.6...v0.0.7) - 2026-05-29

### Other

- flat profiling driver mirroring bench scenarios (r189)
- libFuzzer harnesses for framing + per-sample invariants (r180)
- criterion harnesses for decode / encode / roundtrip (r173)
- spec-derived property sweep + PSNR floor regressions (r121)
- re-document the canonical codec-id strings

### Other

- add a standalone profiling driver as `examples/profile_g711.rs` for
  `samply` / `cargo flamegraph` / `perf record`. The driver mirrors
  the three Criterion bench harness scenarios byte-for-byte (same
  xorshift32 seeds, same five mulaw / alaw × mono / stereo / 8ch
  shapes) but runs a flat fixed-iteration loop with a single
  `Instant::now()` / `elapsed()` pair around the whole pass — no
  Criterion warm-up / sampling / estimator layers in the profile,
  so the codec hot paths (256-entry decode LUT load, encode
  sign-extract + bias + segment-search top-bit loop, on-wire
  inversion) and the trait-surface framing overhead (LE byte
  packing, packet validation, channel-count modulo, output queue
  management) are what shows up in the flamegraph. Each scenario
  prints its own throughput row so the binary also doubles as a
  quick A/B harness for the inner tweak-remeasure loop when
  Criterion's per-run overhead is too coarse. No new dependencies
  (`std::time::Instant` + the crate's existing public per-sample
  helpers / trait-surface factories). Run with
  `cargo run --example profile_g711 --release -- [mode] [iters]`
  where `mode` is one of `decode` / `encode` / `roundtrip` / `all`.
- add a libFuzzer-driven `fuzz/` package with three targets covering
  the framing wrapper and per-sample invariants that the existing
  exhaustive bit-exact tests do not directly exercise as
  panic-/UB-freedom contracts. **`decode_pipeline`** drives arbitrary
  bytes through `mulaw::UlawDecoder` and `alaw::AlawDecoder` with
  attacker-chosen channel counts (1..=8), hammering the
  `pkt.data.len() % channels` rejection path, the empty-packet early
  return, the double-`send_packet` rejection, and the post-flush
  `Eof` path. **`encode_pipeline`** drives arbitrary i16 PCM through
  both encoders at attacker-chosen channels (1..=8) and sample rate
  (1..=192 000), then back through the matching decoder, and asserts
  the trait-surface output equals the per-sample baseline
  `decode_sample(encode_sample(s))` applied verbatim — catching any
  framing-level skew (endianness, padding, channel shuffle).
  **`per_sample_invariants`** drives `encode_sample` / `decode_sample`
  directly and asserts (a) projection idempotence
  `encode(decode(encode(s))) == encode(s)` modulo the documented µ-law
  −0/+0 canonicalisation (`0x7F → 0xFF` when the encoder collapses
  the two zero codewords), (b) sign symmetry on the codeword side
  (`decode(b) == −decode(b ^ 0x80)` except at the µ-law zero
  codepoint), and (c) the per-segment quantisation-step bound with
  the spec-derived saturation slack (644 LSB for µ-law above ±32 635,
  512 LSB for A-law above ±32 256) — the same bounds the
  `quantization_property` test pins on the exhaustive sweep. Runs
  clean against 20 000–80 000 iterations per target on
  aarch64-darwin; corpus and artifacts live under `fuzz/` and are
  `.gitignore`d. Run with
  `cargo +nightly fuzz run <target>` (cargo-fuzz required).
- add Criterion bench harnesses (`decode`, `encode`, `roundtrip`)
  covering the per-sample LUT decode + arithmetic encode hot paths
  and the trait-surface Decoder/Encoder framing overhead. Each
  scenario is self-contained (deterministic xorshift PCM, no
  fixtures) so future optimisation rounds can A/B-test their
  changes against a stable baseline. Initial measurements on
  aarch64-darwin: per-sample LUT decode ~5.5 GiB/s; per-sample
  arithmetic encode ~1.5 GiB/s (µ-law) / ~1.4 GiB/s (A-law);
  full encode+decode round-trip ~1.0 GiB/s through the trait
  surface. Run with `cargo bench -p oxideav-g711 --bench <name>`.
- promote `mulaw::make_decoder` / `mulaw::make_encoder` /
  `alaw::make_decoder` / `alaw::make_encoder` from `pub(crate)` to
  `pub`, matching the dual-API convention (registry path **and**
  direct `make_` factory endpoints) the README already documents.
  No behavioural change; the trait-surface objects returned were
  already reachable via the registry, the direct path is now also
  callable without a `CodecRegistry` round-trip.
- add exhaustive S16-domain property sweep gated on
  `cfg(not(debug_assertions))` — every encode→decode round-trip is
  checked against the per-segment quantization-step bound derived from
  ITU-T G.711 §2 (A-law) and §3 (µ-law). Sparse stride (`step_by(13)`)
  runs in debug for fast feedback; the exhaustive 65 536-sample sweep
  runs under `cargo test --release`. Empirical worst-case error:
  µ-law 644 LSB (at i16::MIN, in the spec-permitted saturation band),
  A-law 512 LSB (same band).
- add PSNR floor regressions: 1-second sines at 400 Hz / 1 kHz / 2 kHz
  at -3 dBFS, encoded+decoded, assert PSNR ≥ 35 dB (well above the
  ~38 dB SQNR design point the G.711 staged Recommendation cites for
  voice-band tones). Measured: µ-law 46.69 / 40.96 / 44.10 dB; A-law
  49.06 / 39.25 / 49.39 dB at 400 Hz / 1 kHz / 2 kHz respectively.
  Includes a cross-law within-5-dB-at-1-kHz check.

## [0.0.6](https://github.com/OxideAV/oxideav-g711/compare/v0.0.5...v0.0.6) - 2026-05-06

### Other

- drop dead `linkme` dep
- registry calls: rename make_decoder/make_encoder → first_decoder/first_encoder
- auto-register via oxideav_core::register! macro (linkme distributed slice)
- unify entry point on register(&mut RuntimeContext) ([#502](https://github.com/OxideAV/oxideav-g711/pull/502))

### Changed

- **`register` entry point unified on `RuntimeContext`** (task #502).
  The legacy `pub fn register(reg: &mut CodecRegistry)` is renamed to
  `register_codecs` and a new `pub fn register(ctx: &mut
  oxideav_core::RuntimeContext)` calls it internally. Breaking change
  for direct callers passing a `CodecRegistry`; switch to either the
  new `RuntimeContext` entry or the explicit `register_codecs` name.

## [0.0.5](https://github.com/OxideAV/oxideav-g711/compare/v0.0.4...v0.0.5) - 2026-05-03

### Other

- clear clippy lints in docs_corpus driver
- route docs_corpus through public CodecRegistry + rustfmt pass
- wire docs/audio/g711/fixtures/ corpus into integration suite
- replace never-match regex with semver_check = false
- migrate to centralized OxideAV/.github reusable workflows
- adopt slim VideoFrame/AudioFrame shape
- pin release-plz to patch-only bumps

## [0.0.4](https://github.com/OxideAV/oxideav-g711/compare/v0.0.3...v0.0.4) - 2026-04-25

### Other

- drop oxideav-codec/oxideav-container shims, import from oxideav-core
- drop Cargo.lock — this crate is a library
- bump oxideav-core / oxideav-codec dep examples to "0.1"
- bump to oxideav-core 0.1.1 + codec 0.1.1
- migrate register() to CodecInfo builder
- bump oxideav-core + oxideav-codec deps to "0.1"
- claim WAVEFORMATEX tags via oxideav-codec CodecTag registry
- add bit-exact reference + multichannel coverage
- support arbitrary interleaved channel counts (not just mono)
- add 'Quick use' example for standalone decode/encode
- loosen oxideav-* pins to '0.0' (accept any 0.0.x)
