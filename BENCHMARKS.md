# oxideav-g711 — benchmark baselines

Criterion harnesses live under [`benches/`](benches/). Every input is
synthesised in-bench from a deterministic xorshift seed — no fixture
files, no external corpus — so the numbers below are reproducible on
any host with the same toolchain. All rows here were measured on
**aarch64-darwin, release profile**, with a 1 s warm-up + 3 s
measurement window (`--warm-up-time 1 --measurement-time 3`). Treat the
absolute throughputs as host-specific; the **ratios between
distributions** are the regression-meaningful signal.

```sh
cargo bench -p oxideav-g711 --bench decode
cargo bench -p oxideav-g711 --bench encode
cargo bench -p oxideav-g711 --bench roundtrip
cargo bench -p oxideav-g711 --bench streaming
cargo bench -p oxideav-g711 --bench voice
cargo bench -p oxideav-g711 --bench segment
cargo bench -p oxideav-g711 --bench cacheladder
cargo bench -p oxideav-g711 --bench batch
```

The first six harnesses share the **distribution** axis below (fixed
size, varying input distribution / law / path). `cacheladder` (r319) is
orthogonal — it fixes the distribution (uniform) and sweeps the
**working-set size** instead; see *Cache-residency size sweep* at the
end.

## Input-distribution corners

G.711 has no signal-processing state, so per-sample wall time is set
by two things: the per-sample math (a LUT load or an arithmetic
segment search) and the input distribution's interaction with the
branch predictor + cache. Three bench files pin the three corners of
the input-distribution space so a regression that is sensitive to one
corner but not the others is isolated immediately:

| corner | bench file | segment profile | what it stresses |
| --- | --- | --- | --- |
| **uniform** | `decode` / `encode` / `roundtrip` / `streaming` | every segment equally | full-LUT cache coverage; unlearnable segment-search branch |
| **voice** (r247) | `voice` | ~80% in segments 0..=2 | segment-0 fast-exit branch parked; small-magnitude LUT quadrant |
| **segment-locked** (r298) | `segment` | every sample in the top segment | long-search branch parked (mirror of voice); high LUT quadrant |

## r298 baseline — `segment` vs. `voice` (the two arith corners)

The headline result of the r298 segment-locked bench is the
**A-law arithmetic** row. The A-law arith path (§2) has an explicit
segment-0 short-circuit; the voice distribution takes it on ~80% of
samples, the segment-locked distribution **never** takes it (every
sample resolves to the top segment, the full search runs every time).
The measured gap quantifies the value of that short-circuit directly.
The µ-law arith path (§3) has no equivalent short-circuit, so it is
nearly distribution-invariant — a useful negative control. The LUT
rows have no data-dependent branch and land within noise of each
other across both corners, confirming the LUT is cache-line dense
enough that high-vs-low magnitude locality does not dominate.

| row | voice (GiB/s) | segment-locked (GiB/s) | note |
| --- | --- | --- | --- |
| encode µ-law **arith** | ~1.47 | ~1.50 | within noise — µ-law has no seg-0 short-circuit |
| encode A-law **arith** | ~1.69 | **~1.43** | **−15%**: seg-0 short-circuit taken on voice, never on segment-locked |
| encode µ-law LUT | ~10.2 | ~10.8 | within noise (control) |
| encode A-law LUT | ~9.0 | ~10.9 | within noise / cache-warm (control) |
| decode µ-law LUT | ~5.3 | ~5.4 | within noise (control) |
| decode A-law LUT | ~4.2 | ~5.4 | within noise / cache-warm (control) |
| roundtrip µ-law mono | ~3.29 | ~3.21 | within noise |
| roundtrip A-law mono | ~3.29 | ~2.91 | tracks the A-law arith gap through the trait surface |

**Regression watch.** The A-law arith voice↔segment gap is the most
informative single number in the suite: if a future change to the
A-law segment search removes or reshapes the segment-0 short-circuit,
this gap moves (shrinks if the short-circuit is weakened, or inverts
if a new path is faster on the high segment). If a future change
splits a hot cache line (e.g. a SIMD gather pulling non-contiguous
LUT entries), the LUT control rows — currently within noise — develop
a voice↔segment spread. Re-run both benches after any change to the
LUT generators, the segment search, or the encoder/decoder framing.

## Steady-state per-sample hot path (uniform distribution)

These are the canonical worst-case (every-segment) numbers carried
forward from prior rounds; see the crate README for the full r230 /
r236 / r289 optimisation history.

| path | throughput |
| --- | --- |
| decode per-sample LUT (µ-law / A-law) | ~5.5 GiB/s |
| encode per-sample LUT (µ-law / A-law, r230 64 KiB table) | ~11 GiB/s |
| encode per-sample arith (formula, pre-r230 baseline) | ~1.5 GiB/s |
| decode trait-surface (r236 pre-sized slice store) | ~3.8 GiB/s |
| encode trait-surface (r236 pre-sized slice store) | ~5.6 GiB/s |
| roundtrip mono 8 kHz | ~3.1–3.3 GiB/s |
| streaming (50 × 20 ms PSTN burst, one enc+dec pair) | ~1.7–3.3 GiB/s |

## Cache-residency size sweep (r319 — `cacheladder`)

Orthogonal to the distribution corners: this harness fixes the input
distribution (uniform) and sweeps the **working-set size** across a
geometric ladder (1 KiB → 4 KiB → 16 KiB → 64 KiB → 256 KiB → 1 MiB →
4 MiB of input codewords) for three paths. Throughput is reported
per-element so every rung is directly comparable; the load-bearing
signal is the **shape** of each curve and any change in it between
commits. Absolute numbers are host-specific (cache geometry differs).

| family | path | aarch64-darwin curve shape |
| --- | --- | --- |
| `decode_lut_sweep` (µ/A) | direct `decode_sample` LUT | **flat ~5.7 Gelem/s** — compute-bound, 256-entry LUT stays L1-resident, input read once |
| `decode_decoder_sweep` (µ mono) | trait surface (`make_decoder` → `send_packet`/`receive_frame`) | **~4.05 Gelem/s** — pays the per-packet output `Vec` alloc + S16 LE store (the r289-optimised path) |
| `encode_arith_sweep` (µ/A) | arithmetic formula (`encode_sample_arith`) | **~750–810 Melem/s** — branch-bound segment search, no table residency |

This is the residency curve the r289 store-strategy A/B needed: r289
measured `decode-store-recompute` vs. `decode-store-le-lut` at a single
96 KB / 8 ch / 48 kHz point and asserted "small buffers are
store-insensitive". The sweep makes that claim falsifiable across the
whole L1 → DRAM range on a given machine, so a future store-strategy or
SIMD change can see exactly where its win turns on and whether it
regresses the small-buffer case.

## Call-surface decomposition (r406 — `batch`)

The r406 batch (slice) API gives every direction × law a third call
surface between the per-sample helpers and the trait objects, and the
trait-surface hot loops now delegate to the slice helpers — so within
one Criterion group the **`trait` − `slice_le` spread is exactly the
cost of packet/frame framing + the per-call output `Vec` allocation**
(the inner loop is shared). All rows: 96 000 uniform-random elements
(the 8 ch / 48 kHz / 250 ms shape), throughput per input byte (decode)
or per input sample (encode). r406 baseline, measured under parallel
build load — treat within-group ratios as the signal:

| group | per_sample | slice | slice_le | trait |
| --- | --- | --- | --- | --- |
| decode µ-law | ~1.36 GiB/s | ~5.01 GiB/s | ~4.99 GiB/s | ~3.71 GiB/s |
| decode A-law | ~1.29 GiB/s | ~4.77 GiB/s | ~4.83 GiB/s | ~3.62 GiB/s |
| encode µ-law | ~1.25 GiB/s | ~4.73 GiB/s | ~3.48 GiB/s | ~3.00 GiB/s |
| encode A-law | ~1.29 GiB/s | ~4.56 GiB/s | ~3.44 GiB/s | ~2.96 GiB/s |
| encode µ-law zero-suppress | ~4.89 GiB/s | ~4.43 GiB/s | — | — |

Reading the decomposition:

- **`slice` vs `trait`**: the framing + allocation premium is ~26%
  on decode and ~14% on encode at this size — that is the entire
  remaining gap, since the loops are shared. Callers with reusable
  buffers get it back by calling the slice helpers directly.
- **`per_sample` is not "the LUT is slow"**: that row consumes each
  sample into a serial accumulator (the historical bench shape), so
  it measures a loop-carried dependency chain, not the table. The
  independent-store slice rows are the honest bulk-throughput
  numbers.
- **`slice_le` encode < `slice` encode**: the LE form pays the
  byte-pair deserialisation on load; decode's LE form pays nothing
  because the store is a fixed 2-byte copy either way.
- **zero-suppress ≈ plain encode**: the first r406 measurement had
  the branch-per-store form at ~3.59 GiB/s on the slice row, ~24%
  behind plain `encode_slice` — the §3.2 rewrite cost a compare +
  select per store on top of the LUT load. Folding the rewrite into
  a dedicated compile-time table (`MULAW_ENCODE_ZERO_SUPPRESS`,
  r406) closed the gap: **3.59 → 4.43 GiB/s (+23.5%)** on the slice
  row, bringing the suppressed wire within ~6% of the plain law
  (both are now a single 64 KiB-LUT load per sample).

## Profiling driver

A flat single-`Instant` driver ships at
[`examples/profile_g711.rs`](examples/profile_g711.rs) for
`samply` / `cargo flamegraph` / `perf record` capture, mirroring the
Criterion scenarios byte-for-byte (same seeds). See the README
"Profiling" section.
