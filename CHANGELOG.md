# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
