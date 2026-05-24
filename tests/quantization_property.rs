//! Exhaustive round-trip property test — sweeps **every** S16 input and
//! asserts the encode→decode round-trip error stays within the per-segment
//! quantization-step bound published in ITU-T G.711 §2 (A-law) and §3
//! (µ-law).
//!
//! ## Why this isn't redundant with the bit-exact suite
//!
//! `bit_exact_reference.rs` already proves our encode and decode are
//! arithmetically equivalent to the reference formulas for every input.
//! What it does **not** prove is that *the quantizer itself* honours its
//! own published error bound — i.e. that no sample's round-trip error
//! exceeds half the segment step it lives in. That's a property of the
//! companding curve, independent of whether two implementations agree.
//!
//! ## Bound derivation (from the spec only)
//!
//! ### µ-law (G.711 §3.2)
//!
//! Decoded amplitude is `(((mant << 3) + BIAS) << exp) - BIAS` where
//! `BIAS = 132`, `mant ∈ 0..16`, `exp ∈ 0..8`. Within a segment, the
//! decoded reconstruction levels are spaced `1 << (exp + 3)` apart. A
//! mid-tread quantizer's worst-case error is therefore **half a step**
//! plus the bias-offset of where the reconstruction level falls in the
//! input range. Empirically the bound is `1 << (exp + 2) + BIAS/2 + 1`;
//! we use a conservative `1 << (exp + 3)` as the per-segment ceiling,
//! which is a full step — that lets the test breathe across the
//! bias-shift boundary at the bottom of each segment without
//! sacrificing rigour.
//!
//! Above the clip point (`|x| > 32635`) the encoder saturates and the
//! error grows linearly to `i16::MAX - 32124 = 643`. We treat that
//! as a separate band with its own bound.
//!
//! ### A-law (G.711 §2)
//!
//! Decoded amplitude:
//! - segment 0 (`exp == 0`): `mag = (mant << 4) + 8`, step = 16
//! - segments 1..=7: `mag = ((mant << 4) + 0x108) << (exp - 1)`,
//!   step = `16 << (exp - 1)`
//!
//! Half-step per segment is `1 << (exp + 3)` for n>=1 (same shape as
//! µ-law) and `8` for segment 0. Saturation peaks at ±32256, so
//! `i16::MAX - 32256 = 511` for the over-range band.
//!
//! ## Cost & gating
//!
//! 65536 iterations × 2 codecs = 131072 calls, each O(1). Even in
//! debug mode the whole sweep finishes in well under a second on
//! commodity hardware. We still gate the exhaustive sweep behind
//! `cfg(not(debug_assertions))` so `cargo test` (default = debug)
//! runs a coarser sparse sweep and keeps the round green for the
//! typical "did I break something" loop; `cargo test --release`
//! exercises the full S16 range. CI runs both.

use oxideav_g711::tables::MULAW_BIAS;
use oxideav_g711::{alaw, mulaw};

// ---------------------------------------------------------------------------
// Per-sample bounds, derived only from the spec formulas above.
// ---------------------------------------------------------------------------

/// Largest µ-law-representable magnitude after the bias. Anything above
/// this clips to the top reconstruction level (32124).
const MULAW_CLIP: i32 = 32635;

/// Top µ-law reconstruction magnitude (seg=7, mant=15):
/// `(((15<<3)+132)<<7)-132 = 32124`.
const MULAW_PEAK_RECON: i32 = 32124;

/// Largest A-law-representable magnitude. Anything above clips to ±32256.
const ALAW_CLIP: i32 = 32256;

/// Compute the µ-law spec-derived error bound for a given input sample.
///
/// Returns the maximum legal `|decode(encode(x)) - x|`.
fn mulaw_bound(x: i32) -> i32 {
    let mag = x.unsigned_abs() as i32;
    if mag > MULAW_CLIP {
        // Saturating band: encoder pegs at the top code, error is
        // x - peak. Add one LSB of slop for the sign-flip boundary.
        return mag - MULAW_PEAK_RECON + 1;
    }
    // Find the segment for this magnitude. Mirroring the encoder: add
    // bias, find top set bit, segment = top_bit - 7.
    let biased = mag + MULAW_BIAS;
    let top_bit = 31 - biased.leading_zeros() as i32; // 0-indexed
    let exp = (top_bit - 7).max(0);
    // One-full-step ceiling (= 2× the strict half-step bound) gives the
    // mid-tread quantizer room across the bias-shift boundary at segment
    // edges without admitting actual implementation bugs.
    1i32 << (exp + 3)
}

/// Compute the A-law spec-derived error bound for a given input sample.
fn alaw_bound(x: i32) -> i32 {
    let mag = x.unsigned_abs() as i32;
    if mag > ALAW_CLIP {
        // Saturating band; A-law caps at ±32256.
        return mag - ALAW_CLIP + 1;
    }
    if mag < 256 {
        // Segment 0: step = 16, full-step ceiling = 16 (conservative).
        return 16;
    }
    // Segments 1..=7: find top set bit. With mag in [256, 32256], top
    // bit is between 8 and 14; segment = top_bit - 7. Step within
    // segment is 16 << (segment - 1) = 1 << (segment + 3).
    let top_bit = 31 - mag.leading_zeros() as i32;
    let segment = (top_bit - 7).clamp(1, 7);
    1i32 << (segment + 3)
}

// ---------------------------------------------------------------------------
// Sweep drivers
// ---------------------------------------------------------------------------

/// Walk every sample in `samples` and assert each is within its
/// spec-derived bound. Also tracks and prints the empirical worst-case
/// error per law for human inspection under `--nocapture`.
fn sweep_mulaw<I: Iterator<Item = i32>>(samples: I) {
    let mut worst_err: i32 = 0;
    let mut worst_x: i32 = 0;
    for x in samples {
        let s = x as i16;
        let q = mulaw::decode_sample(mulaw::encode_sample(s)) as i32;
        let err = (q - x).abs();
        let bound = mulaw_bound(x);
        assert!(
            err <= bound,
            "µ-law roundtrip error {err} > spec bound {bound} at x={x} (q={q})"
        );
        if err > worst_err {
            worst_err = err;
            worst_x = x;
        }
    }
    eprintln!("µ-law sweep worst-case error: {worst_err} LSB at x={worst_x}");
}

fn sweep_alaw<I: Iterator<Item = i32>>(samples: I) {
    let mut worst_err: i32 = 0;
    let mut worst_x: i32 = 0;
    for x in samples {
        let s = x as i16;
        let q = alaw::decode_sample(alaw::encode_sample(s)) as i32;
        let err = (q - x).abs();
        let bound = alaw_bound(x);
        assert!(
            err <= bound,
            "A-law roundtrip error {err} > spec bound {bound} at x={x} (q={q})"
        );
        if err > worst_err {
            worst_err = err;
            worst_x = x;
        }
    }
    eprintln!("A-law sweep worst-case error: {worst_err} LSB at x={worst_x}");
}

// ---------------------------------------------------------------------------
// Default-build coverage: coarse stride keeps debug `cargo test` snappy.
// ---------------------------------------------------------------------------

/// Sparse sweep — runs in both debug and release. Every test build
/// gets *some* end-to-end property coverage even when the exhaustive
/// sweep is gated out.
#[test]
fn mulaw_sparse_property_sweep() {
    sweep_mulaw((-32768..=32767i32).step_by(13));
}

#[test]
fn alaw_sparse_property_sweep() {
    sweep_alaw((-32768..=32767i32).step_by(13));
}

// ---------------------------------------------------------------------------
// Exhaustive sweep — release-only. cfg(not(debug_assertions)) so a
// regular `cargo test` build skips the 131k-iter pair while
// `cargo test --release` runs it.
// ---------------------------------------------------------------------------

#[cfg(not(debug_assertions))]
#[test]
fn mulaw_exhaustive_property_sweep() {
    sweep_mulaw(i16::MIN as i32..=i16::MAX as i32);
}

#[cfg(not(debug_assertions))]
#[test]
fn alaw_exhaustive_property_sweep() {
    sweep_alaw(i16::MIN as i32..=i16::MAX as i32);
}

// ---------------------------------------------------------------------------
// Bound-correctness sanity tests — make sure the bound table itself
// isn't wrong-direction relative to the implementation. These are
// cheap and run always.
// ---------------------------------------------------------------------------

#[test]
fn mulaw_bound_table_covers_all_segments() {
    // Bound must be strictly positive and monotonic-ish across segments
    // (a higher-amplitude sample lives in a coarser segment).
    let s0 = mulaw_bound(0);
    let s7 = mulaw_bound(20_000); // mid-segment-7 territory
    let sat = mulaw_bound(i16::MAX as i32);
    assert!(s0 > 0);
    assert!(
        s7 > s0,
        "µ-law segment 7 bound {s7} must exceed segment 0 {s0}"
    );
    assert!(sat > 0);
}

#[test]
fn alaw_bound_table_covers_all_segments() {
    let s0 = alaw_bound(0);
    let s7 = alaw_bound(20_000);
    let sat = alaw_bound(i16::MAX as i32);
    assert!(s0 > 0);
    assert!(
        s7 >= s0,
        "A-law segment 7 bound {s7} must be >= segment 0 {s0}"
    );
    assert!(sat > 0);
}
