# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Other

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
