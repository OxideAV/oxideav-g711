# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- test: `substituted_codeword_matches_spec_minus_7519` (r380). Pins the
  exact §3.2 decoder-output value the spec names for the all-zero-
  suppression replacement codeword `00000010` — "the value at the decoder
  output is -7519 … decoder output value number 125". The crate's
  16-bit-left-justified decode convention is the spec's 14-bit magnitude
  convention scaled by the segment-7 quantum (4), so the test asserts
  `decode_sample(0x02) == -7519 * 4 == -30076` and the all-zero codeword
  `decode_sample(0x00) == -8031 * 4 == -32124`, and verifies the value-
  number ladder (127 → 125 = a two-step inward move). This closes the
  numeric-value gap the module doc previously flagged: the -7519 figure is
  now pinned exactly rather than only the wire-level codeword contract.

### Changed

- docs: rewrote the `tables::tests::mulaw_endpoints` doc comment from a
  stray stream-of-consciousness derivation (a "wait that's not right"
  scratch note) into a clean ITU-T G.711 §3 statement of the digital-zero
  codewords (`0xFF` / `0x7F` → 0) and the extreme-magnitude derivation
  (`(((0x0F << 3) + 0x84) << 7) - 0x84 = 32124`), and added explicit
  `MULAW_DECODE[0x80] == 32124` / `MULAW_DECODE[0x00] == -32124` endpoint
  assertions (r380). No behavioural change — the decode LUT is unchanged.
- clean-room hygiene: removed an external-implementation reference from the
  `tests/bit_exact_reference.rs` µ-law encode-mirror comment, replacing it
  with a spec-sourced derivation note (the second source of truth is
  written from ITU-T G.711 §3.2 alone) (r380).

### Added

- fuzz: `zero_suppress_invariants` target (r337). A seventh libFuzzer
  target covering the µ-law all-zero character-signal suppression path
  (ITU-T G.711 §3.2, `mulaw::encode_sample_zero_suppress`) — the only
  encode path the existing six targets do not reach (they drive the plain
  `encode_sample` LUT, never the suppression rewrite). Drives
  attacker-chosen i16 sequences and asserts the spec's wire-level
  contract per sample: the all-zero octet is never emitted, every
  non-forbidden codeword is byte-identical to the plain encoder, the one
  forbidden codeword is rewritten to exactly the spec `00000010`
  replacement, and (once per execution) the substituted codeword decodes
  one uniform segment-7 step inward from the suppressed all-zero word with
  the decoder untouched. These are the same invariants
  `tests/mulaw_zero_suppression.rs` pins exhaustively, re-expressed as a
  panic-/UB-freedom regression net against future changes to the
  suppression rewrite or the underlying encode LUT.

- feat: µ-law all-zero character-signal suppression (ITU-T G.711 §3.2, r331).
  New `mulaw::encode_sample_zero_suppress(sample) -> u8` plus the
  `mulaw::MULAW_ZERO_CODEWORD` (`0x00`) / `mulaw::MULAW_ZERO_SUPPRESS_CODEWORD`
  (`0x02`) constants. Identical to `encode_sample` except the one codeword that
  would be transmitted as the all-zero octet `00000000` is rewritten to the
  spec-mandated `00000010`, the substitution G.711 §3.2 requires on links (classic
  T1 spans) where a run of all-zero octets would starve the receiver's bit-clock
  recovery. The decode path is untouched: the substituted `0x02` decodes like any
  other byte. A new `tests/mulaw_zero_suppression.rs` integration suite pins the
  wire-level contract over the full 16-bit input domain — the all-zero octet is
  never emitted, exactly the 1157 most-negative segment-7 inputs (-32768 ..= -31612)
  are rewritten to `0x02` and every other codeword is byte-identical to the plain
  encoder, the rewrite boundary is exact (-31611 already carries a non-zero LSB and
  is left untouched), and the substituted codeword decodes one uniform segment-7
  step inward from the suppressed one (the §3.2 "value number 126 → 125" single
  decision-interval move). The numeric decoder-output value `-7519` §3.2 quotes is
  in the spec's own 14-bit Table-2 magnitude convention, which differs from this
  crate's 16-bit left-justified decode scaling, so that exact figure is documented as a
  spec-table gap rather than asserted (see the final report / test-module note).

### Performance

- perf: byte-pair little-endian decode LUT for the trait-surface decode
  hot loop (r289, depth-mode profiling + one bit-identical optimization).
  `UlawDecoder` / `AlawDecoder::receive_frame` previously loaded the
  `[i16; 256]` decode table, recomputed `i16::to_le_bytes()` per sample,
  and did two scalar stores (the r236 shape). It now indexes a
  pre-serialized `MULAW_DECODE_LE` / `ALAW_DECODE_LE` (`[[u8; 2]; 256]`,
  generated at compile time from the existing `[i16; 256]` decode tables
  — no second source of truth) and stores each sample with one 2-byte
  `copy_from_slice` from `.rodata`. Output is byte-identical: the
  `bit_exact_reference`, `docs_corpus`, and `cross_law_transcode` suites
  all pass unchanged, and a new `tables` unit test pins
  `*_DECODE_LE == *_DECODE.to_le_bytes()` so the serialized view can
  never drift from the i16 table. Measured ~+24% on an isolated
  standalone A/B over a 96 KB buffer (3.56 → 4.68 GiB/s) and ~5-6% on the
  store-bound 8 ch / 48 kHz row of the `profile_g711 decode` driver;
  small mono / stereo rows that fit in L1 are store-insensitive and stay
  within noise. The profiling driver gains two A/B rows
  (`decode-store-recompute` / `decode-store-le-lut`) that isolate the
  store strategy with the per-sample decode pre-selected as a function
  pointer and both LUTs hoisted out of the timed region.

### Benchmarks

- bench: new `benches/cacheladder.rs` (r319, depth-mode benchmarks) —
  the **working-set size sweep** the existing benches never covered.
  Every prior bench file fixes the buffer / frame size and varies the
  input distribution (uniform / voice-Laplacian / segment-locked), law,
  and path; none sweep a single law + path across a geometric ladder of
  buffer sizes, which is the axis that exposes the L1 -> L2 -> L3 -> DRAM
  throughput knee. This file sweeps 1 KiB .. 4 MiB of input codewords
  across five families: direct-LUT decode (mu/A), trait-surface decode
  (mu mono — the r289 store path), and arithmetic-encode formula path
  (mu/A). Per-element `Throughput::Elements` keeps every rung directly
  comparable. Measured on aarch64-darwin (release): the direct-LUT decode
  curve is flat at ~5.7 Gelem/s across the whole ladder (compute-bound —
  the 256-entry LUT stays L1-resident, the streamed input is read once),
  the trait-surface decode runs ~4.05 Gelem/s (paying the per-packet
  output `Vec` alloc + S16 little-endian store the r289 work optimised),
  and the branch-bound arithmetic encode lands ~750-810 Melem/s. The
  load-bearing signal is the *shape* of each curve on a given machine and
  any change in that shape between commits — this is the residency curve
  the r289 store-strategy A/B (measured at a single 96 KB point) needed
  to make its "small buffers are store-insensitive" claim falsifiable.
  Input synthesised in-bench from the same deterministic xorshift32 seed
  as `benches/decode.rs` so a rung lines up with the matching fixed-size
  row there; no fixtures, no external corpus. No behavioural change —
  bench only.
- bench: new `benches/segment.rs` (r298, depth-mode benchmarks) — pins
  the third corner of the input-distribution space. The uniform benches
  spread samples across every segment of the companding curve and the
  r247 `voice` bench concentrates them in segments 0..=2 (segment-search
  fast-exit on ~80% of samples); the new bench confines every sample to
  the **top segment**, so the encode segment search resolves to the same
  high segment on every sample — the branch-history mirror image of the
  voice row. Eight rows (encode arith + LUT × law, decode LUT × law,
  mono 1 s roundtrip × law), input synthesised in-bench from a
  closed-form window over a deterministic xorshift32 stream (no fixtures,
  no external corpus). The informative result is the **A-law arith** row:
  voice ~1.69 GiB/s vs. segment-locked ~1.43 GiB/s (−15%), quantifying
  the A-law §2 segment-0 short-circuit (taken on voice, never on the
  segment-locked input). µ-law arith — which has no equivalent
  short-circuit — is distribution-invariant (~1.47 vs ~1.50 GiB/s), and
  the branchless LUT rows land within noise across both corners as
  expected. New `BENCHMARKS.md` records the cross-distribution baseline
  table + the regression-watch guidance. Measured on aarch64-darwin
  (release, 3 s window). No behavioural change — bench + docs only.

### Other

- tests: new `tests/reference_sequence_g711.rs` integration suite (r323)
  — pins the **normative** §5 reference sequences (Table 5/G.711 for
  A-law, Table 6/G.711 for µ-law). §5 states that applying these
  periodic 8-codeword sequences to a decoder must produce a 1 kHz sine
  at 0 dBm0; at the §2 nominal 8 kHz sampling rate, eight samples per
  period is exactly a 1 kHz fundamental. The crate had exhaustive
  per-codeword decode coverage (`bit_exact_reference`) and the §3.5
  cross-law tables (`cross_law_table34`) but never pinned the §5
  audio-level conformance vectors. Five tests: each law's sequence is
  packed from the spec's bit-1..8 rows via the §4 serial-transmission
  convention (bit 1 = polarity bit = MSB, transmitted first — packed by
  a helper rather than hard-coded hex, so the test documents the bit
  order), then decoded through both the direct LUT and the registry
  trait surface (must agree byte-for-byte). The decoded waveform is
  asserted to have the structure §5 requires of a 1 kHz sine — 8-sample
  period, half-wave antisymmetry (`y[i] == -y[i+4]`), even symmetry
  inside each half (`y[0]==y[3]`, `y[1]==y[2]`), and a monotone
  quarter-sine (`|y[1]| > |y[0]|`, i.e. sin 67.5° > sin 22.5°) — and the
  exact decoder-output values are pinned (A-law
  `±{8960, 20992}`, µ-law `±{8828, 20860}`) so any table edit that
  perturbs the conformance waveform fails here. Further tests: tiling
  three periods proves period-8 continuity across the boundary (the
  §1.2/§3.2 stateless property); the two laws' tones share the same sine
  *shape* and never disagree by more than one A-law top-segment step
  (256 LSB) while being non-bit-identical (the §5 T_max difference,
  3.14 vs 3.17 dBm0); and a §4 sanity check pins that wire byte 0xD5
  decodes to A-law +8 and that the bit-1-first packer produces 0xD5.
  No `src/` change, no public-API change — test-only, CI-gated. Finding:
  our decode tables reproduce the §5 reference sequences exactly; this
  is the first test to assert the audio-level conformance vectors.
- tests: new `tests/cross_law_table34.rs` integration suite (r305) —
  pins the **normative** µ↔A conversion mapping of ITU-T G.711 §3.5,
  Tables 3/G.711 (µ→A) and 4/G.711 (A→µ). The existing
  `cross_law_transcode` suite only covered the §3.6 *equipment option*
  (re-quantising through uniform PCM, `encode_B(decode_A(b))`), which the
  Recommendation explicitly leaves "to the individual equipment
  specification." This suite asserts that our PCM-roundtrip transcode
  reproduces the spec's Table 3 / Table 4 **decoder-output value-number**
  correspondence for all 128 levels of both laws, in both directions and
  on both signs — including the non-trivial high-segment jumps
  (µ value 32→A 25, 33→27, 34→29, 35→31, 36→33) and the deliberately
  modified transparency points (µ-80↔A-80) the Recommendation §3.6 Note 2
  documents. Two further tests pin the Note 2 tandem-transparency claims:
  a µ-A-µ double conversion changes exactly the documented set of µ value
  numbers {0,2,4,6,8,10,12,14} per sign (and is perfectly transparent on
  the other 120 levels), and an A-µ-A double conversion changes exactly
  8 A value numbers per sign. The value-number axis is rebuilt from the
  crate's own decode tables by magnitude rank (the spec's definition of
  "value number"), and a bijectivity test guards that axis. Six tests,
  always CI-gated, no behavioural change — test-only. Finding: our
  long-standing naive cross-law composition was already bit-identical to
  the normative tables; this suite is the first to prove it.
- tests: new `tests/cross_law_transcode.rs` integration suite (r270) —
  pins the µ-law ↔ A-law (PSTN-gateway) transcode contract through the
  trait surface as a deterministic, **always-CI-gated** regression. The
  r262 `cross_law_transcode` libFuzzer target was previously the *only*
  coverage that crossed the µ ↔ A boundary, and it runs solely under
  `cargo +nightly fuzz run` — never on the standard CI test job. The six
  new tests assert the same composition contract on every CI run: the
  trait-surface forward transcode (decode law A → re-encode law B) equals
  the per-sample baseline `encode_B(decode_A(b))` byte-for-byte across all
  256 codewords in both directions and across 1 / 2 / 6 / 8 interleaved
  channels (interleave independence); the full A → B → A reverse roundtrip
  matches the per-sample double-transcode baseline
  `encode_A(decode_B(encode_B(decode_A(b))))` byte-for-byte; and a tandem
  double-hop (µ → A → µ → A → µ) is byte-idempotent from the second hop on
  (PSTN tandem connections must not accumulate per-hop drift past the first
  round-trip). The oracle is the per-sample `decode_sample` /
  `encode_sample` helpers, themselves pinned bit-exact against the ITU-T
  G.711 §2 (A-law) / §3 (µ-law) reference formulas by the exhaustive
  `bit_exact_reference` sweeps, so anchoring the trait-surface transcode to
  them transitively anchors it to the spec. If a future change to either
  decoder's `receive_frame` ever invents / drops a sample, or a per-frame
  state addition (an internal smoother, dither generator, or
  context-dependent quantiser) ever coupled adjacent samples through some
  non-existent state, these tests fail on the first divergent byte without
  waiting for a nightly fuzz invocation. Per the round-selection memory's
  "ONE of fuzz / bench / profile per round" rule for saturated codecs, this
  is the **property-test** lane (r262 was fuzz, r247 bench, r236 profile).
  No `src/` change, no public-API change; 72 tests total (66 → 72), clippy
  + rustfmt clean.
- fuzz: new `cross_law_transcode` target (r262) — sixth libFuzzer
  harness in `fuzz/fuzz_targets/`, the first that crosses the
  µ-law ↔ A-law boundary inside a single pipeline. Decodes the
  attacker's input bytes under one law via the trait-surface
  decoder, re-encodes the recovered PCM as the *other* law via
  the trait-surface encoder, and (optionally, behind a seed bit)
  drives the full law-A → law-B → law-A reverse roundtrip. The
  forward output is asserted byte-for-byte against the
  per-sample baseline `encode_other(decode_self(b))` applied to
  every input byte, and the reverse roundtrip is asserted
  byte-for-byte against the analogous double-transcode per-sample
  reference path. This is the canonical PSTN-gateway transcoding
  contract (North-American µ-law ↔ European A-law circuit
  interconnects must transcode every sample at the boundary);
  the five existing fuzz targets all stay within one law per
  pipeline so the trait-surface composition of two different-law
  factory objects was previously unfuzzed. Per the
  round-selection memory's "ONE of fuzz / bench / profile per
  round" rule for saturated codecs, this is the **fuzz** lane
  (last fuzz round was r224's `factory_params`; r247 was bench,
  r236 was profile). Measured on aarch64-darwin (release, 30 s
  window): **2 594 496 iterations / 31 s clean** (≈ 83.7 k
  exec/s, 452 cov / 1107 ft saturation across 52 corpus entries,
  zero crashes / panics / divergences). No public-API change.
  All 66 existing tests stay green; clippy + rustfmt clean on
  both the crate and the fuzz package. The new target hardens
  the framing-wrapper composition contract: if a future change
  to either decoder's `receive_frame` ever invents / drops a
  sample, or if a future per-frame state addition (an internal
  smoother, a dither generator, a context-dependent quantiser)
  ever coupled adjacent samples through some non-existent state,
  the very first divergent byte fails the assert.
- benches: new `voice` harness (r247) — fifth Criterion bench file
  driving the same per-sample LUT + arith + roundtrip hot paths
  the r173 / r206 benches cover, but feeding them from a closed-
  form Laplacian generator concentrated near zero instead of the
  existing uniform-random xorshift32 stream. Eight scenarios:
  decode + encode × LUT + arith × law (six rows), plus a mono 1 s
  roundtrip per law (two rows). The Laplacian-centred distribution
  models PSTN voice content — ~80% of samples land in segments
  0..=2 (|s| ≤ 1024) — so the encode 64 KiB LUT touches primarily
  its low-magnitude quadrants and the arith path hits the
  segment-0 fast exit on the same 80%. Per the round-selection
  memory's "ONE of fuzz / bench / profile per round" rule for
  saturated codecs, this is the **bench** lane — complementing
  r230's bench-driven LUT swap (also bench lane) and r224's fuzz
  lane / r236's profile lane. Measured on aarch64-darwin (release,
  2 s window): the voice-distribution rows land within a few
  percent of their uniform counterparts (decode LUT
  ≈ 5.5 GiB/s both laws; encode LUT µ-law ≈ 9.5 GiB/s / A-law
  ≈ 10.9 GiB/s; encode arith µ-law ≈ 1.49 GiB/s / A-law ≈
  1.72 GiB/s; roundtrip mono 8 kHz ≈ 3.1–3.3 GiB/s) — that's
  itself a useful finding (the LUTs are cache-line dense enough
  that input-distribution locality does not dominate per-sample
  wall time on this host), and it gives future tweaks a permanent
  A/B baseline so any change that introduces a meaningful spread
  between the two distributions is caught immediately. No public-
  API change. The bench file is self-contained — every input is
  synthesised in-bench from a deterministic seed; no `docs/`
  fixtures, no audio corpora, no probability-distribution crates.
  All 66 existing tests stay green; clippy + rustfmt clean.
- perf: indexed-write trait-surface hot loops in `mulaw.rs` /
  `alaw.rs` (r236). The four encoder + decoder framing wrappers
  previously built their output `Vec<u8>` via `Vec::push` (encode)
  / `Vec::extend_from_slice(&s.to_le_bytes())` (decode), one entry
  at a time, inside a `chunks_exact(2)` / `pkt.data.iter()` loop.
  Switched to pre-size the destination (`vec![0u8; n]` /
  `vec![0u8; pkt.data.len() * 2]`) and zip the source against
  `iter_mut()` (encode) / `chunks_exact_mut(2)` (decode) — every
  step becomes a single LUT load + 1 or 2 adjacent indexed stores
  with no bounds-check chain or 2-byte temporary, so LLVM lifts
  the loop into wider stores on aarch64. No public-API change, no
  spec-relevant logic change (the LUTs themselves are unchanged
  from r230, the per-sample helpers are unchanged), no algorithmic
  change — purely a codegen-friendly restructure of how the result
  buffer is filled. Measured on aarch64-darwin (release, 3 s
  Criterion measurement window): the four trait-surface decode rows
  go **2.5–2.7 GiB/s → 3.76–3.88 GiB/s (+44–51%)**, the three
  encode rows go **3.65–3.85 GiB/s → 5.29–5.86 GiB/s (+45–55%)**,
  the four end-to-end r173 roundtrip rows go
  **≈ 2.1 GiB/s → 3.13–3.35 GiB/s (+49–52%)**, and the five r206
  streaming rows go **1.05–2.10 GiB/s → 1.71–3.25 GiB/s
  (+35–46%)**. The per-sample LUT rows are unchanged (~5.5 GiB/s
  decode / ~11.1 GiB/s encode) — they were already a single
  slice-load + push pair; the r236 win comes entirely from the
  framing wrapper closing the gap to the inner loop. All 66 unit /
  integration tests stay green on both `cargo test` and
  `cargo test --release` (including the exhaustive
  `bit_exact_reference` 65 536-input sweeps and the 13 `Tier::
  BitExact` `docs_corpus` fixtures from r218) — bit-exact behaviour
  is preserved because the loop body still computes the same byte
  per sample, only the store pattern changes. Per the
  round-selection memory's "ONE of fuzz / bench / profile per
  round" rule for saturated codecs, this is the **profile-driven
  optimisation** lane (after r230's bench-driven LUT swap and
  r224's fuzz-driven parameter surface); the framing-wrapper gap
  was the largest remaining headroom visible in the r173 bench
  rows (per-sample LUT decode = 5.5 GiB/s vs. trait-surface
  decode = 2.6 GiB/s pre-r236).
- perf: compile-time 64 KiB S16 → byte encode LUT for both laws
  (r230). `mulaw::encode_sample` and `alaw::encode_sample` now
  index `MULAW_ENCODE` / `ALAW_ENCODE` (`[u8; 65536]` each, 64 KiB
  per law, 128 KiB total static data) instead of running the per-call
  bias-add + segment-search + mantissa-shift + on-wire-inversion
  formula. Each LUT entry is produced at compile time by the
  arithmetic encoder (`tables::mulaw_encode_arith` /
  `alaw_encode_arith` — both `const fn`) so the tables are
  bit-exact-by-construction relative to the ITU-T G.711 §2 / §3
  formulas: no second source of truth, no risk of LUT drift, the
  existing `*_encode_is_bit_exact_for_all_65536_samples` exhaustive
  tests stay green. The pre-r230 arithmetic helpers stay public as
  `mulaw::encode_sample_arith` / `alaw::encode_sample_arith` for
  callers that legitimately don't want the 64 KiB table linked into
  their binary (e.g. wasm size-sensitive consumers, or a second
  source of truth in a test). A new pair of exhaustive
  `*_lut_matches_arith_for_every_sample` tests pins
  `LUT[s] == arith(s)` for every one of the 65536 entries on every
  CI run — any future tweak that lets table and formula drift fails
  immediately. The matching r173 `encode` Criterion bench gains two
  new rows (`encode_mulaw_lut_8k_1s`, `encode_alaw_lut_8k_1s`)
  alongside the renamed `arith` rows so the A/B is one bench
  invocation away. Measured on aarch64-darwin (release, 3 s
  Criterion measurement window): per-sample inner loop jumps from
  **µ-law 1.50 GiB/s → 10.85 GiB/s (7.2×)** and **A-law
  1.39 GiB/s → 10.90 GiB/s (7.8×)**. The trait-surface encoder hot
  loops in `mulaw.rs` / `alaw.rs` index the slice directly inside
  `chunks_exact(2)` to let the codegen merge the LE byte unpack +
  table load + push without an inline wall. End-to-end deltas: the
  r173 `roundtrip` bench (factory + encode + decode + one frame)
  now lands at **≈ 2.1 GiB/s** across all four scenarios (was
  ~1 GiB/s pre-r230 — ≈ 2×). The r206 `streaming` bench (one pair
  reused across a 50-frame PSTN burst) lands at
  **1.05–2.10 GiB/s** across the five scenarios (was
  760–985 MiB/s, ≈ 2×). Per the round-selection memory's "ONE of
  fuzz / bench / profile per round" rule for saturated codecs, this
  is the **bench** pick — the LUT swap was already flagged as
  future work by the r173 bench-file header comment ("Future
  optimisation rounds may [...] drop in a 64 KiB direct S16 → byte
  LUT"), and the win is large enough to want a permanent A/B
  baseline rather than a one-shot measurement. No public-API
  break: the `encode_sample` signature is unchanged; only its
  internals moved from formula to table. The arithmetic path is
  still reachable under a new public name.
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
