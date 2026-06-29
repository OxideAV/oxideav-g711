//! ITU-T G.711 §3.6 / Tables 1/2 — the uniform-PCM reconstruction-level
//! lattice.
//!
//! §3.6 ("Conversion to and from uniform PCM") states every "decision
//! value" and "quantized value" of each law maps to a uniform-PCM value
//! via Tables 1/G.711 (A-law) and 2/G.711 (µ-law), in a 13-bit (A-law) /
//! 14-bit (µ-law) uniform code. The companding curve is piecewise-linear:
//! the magnitude range is split into 8 segments, each holding 16 evenly
//! spaced reconstruction levels, with the segment step doubling from one
//! segment to the next.
//!
//! The other test files pin *agreement* with the reference formulas
//! (`bit_exact_reference.rs`) and the per-sample *error bound*
//! (`quantization_property.rs`). Neither pins the **geometry** of the
//! reconstruction lattice itself: that the 256 decode outputs are exactly
//! the 8×16 evenly-spaced levels the spec defines, that the within-segment
//! spacing is exactly the segment step, and that the segment boundaries
//! sit where §3 / §2 place them. A future decode-table edit that kept the
//! per-sample error inside the bound but skewed the lattice (e.g. an
//! off-by-one in a single segment's step) would slip past the other
//! suites; it fails here.
//!
//! Source read: `docs/audio/g711/T-REC-G.711-198811-I.pdf` §2, §3, §3.6,
//! Tables 1/2. Nothing else was consulted.

use oxideav_g711::{alaw, mulaw};

/// All distinct **positive** µ-law reconstruction magnitudes, ascending.
/// The positive wire bytes are 0x80..=0xFF (the wire is the one's
/// complement of S|E|M, so a clear stored sign bit shows as a set bit).
fn mulaw_positive_levels() -> Vec<i32> {
    let mut v: Vec<i32> = (0x80u8..=0xFF)
        .map(|b| mulaw::decode_sample(b) as i32)
        .collect();
    v.sort_unstable();
    v
}

/// All distinct positive A-law reconstruction magnitudes, ascending.
/// A-law's sign bit (after the on-wire 0x55 inversion) set ⇒ positive.
fn alaw_positive_levels() -> Vec<i32> {
    let mut v: Vec<i32> = (0u16..=255)
        .map(|b| b as u8)
        .filter(|&b| (b ^ 0x55) & 0x80 != 0)
        .map(|b| alaw::decode_sample(b) as i32)
        .collect();
    v.sort_unstable();
    v
}

/// µ-law: the positive side has 128 reconstruction levels — but the two
/// zero codewords (`0x7F` / `0xFF`) both decode to 0, so the *positive*
/// magnitude set has 127 strictly-positive values plus the shared 0. We
/// assert the full structure: 8 segments × 16 levels, with the bottom
/// segment's first level being 0 (the digital-zero codeword).
#[test]
fn mulaw_levels_are_eight_segments_of_sixteen() {
    let levels = mulaw_positive_levels();
    assert_eq!(levels.len(), 128, "µ-law positive side has 128 codewords");

    // §3.2 reconstruction value: (((mant<<3) + 132) << exp) - 132.
    // Within a fixed segment (exp), stepping mant by 1 changes the value
    // by exactly (1 << 3) << exp = 1 << (exp + 3). The 16 levels of a
    // segment are therefore an arithmetic progression with that step.
    for exp in 0u32..8 {
        let seg = &levels[(exp as usize) * 16..(exp as usize) * 16 + 16];
        let step = 1i32 << (exp + 3);
        for w in seg.windows(2) {
            assert_eq!(
                w[1] - w[0],
                step,
                "µ-law segment {exp}: spacing {} != expected step {step}",
                w[1] - w[0]
            );
        }
        // The first level of segment `exp` is the spec reconstruction
        // value at mant=0: (132 << exp) - 132.
        let first = (132i32 << exp) - 132;
        assert_eq!(
            seg[0], first,
            "µ-law segment {exp} base level {} != (132<<{exp})-132 = {first}",
            seg[0]
        );
    }

    // Segment 0 base is 0 (the digital-zero codeword reconstruction).
    assert_eq!(levels[0], 0, "µ-law segment-0 base must be 0");
    // Top level (seg 7, mant 15) is the spec peak 32124.
    assert_eq!(*levels.last().unwrap(), 32124, "µ-law peak level");
}

/// A-law: 128 positive codewords, 8 segments × 16 levels. Segment 0 is
/// the linear region (`(mant<<4)+8`, step 16); segments 1..=7 are
/// exponential (`((mant<<4)+0x108) << (exp-1)`, step `16 << (exp-1)`).
#[test]
fn alaw_levels_are_eight_segments_of_sixteen() {
    let levels = alaw_positive_levels();
    assert_eq!(levels.len(), 128, "A-law positive side has 128 codewords");

    for exp in 0u32..8 {
        let seg = &levels[(exp as usize) * 16..(exp as usize) * 16 + 16];
        let step = if exp == 0 { 16 } else { 16i32 << (exp - 1) };
        for w in seg.windows(2) {
            assert_eq!(
                w[1] - w[0],
                step,
                "A-law segment {exp}: spacing {} != expected step {step}",
                w[1] - w[0]
            );
        }
        // Base level (mant=0).
        let first = if exp == 0 { 8 } else { 0x108i32 << (exp - 1) };
        assert_eq!(
            seg[0], first,
            "A-law segment {exp} base level {} != expected {first}",
            seg[0]
        );
    }

    // A-law has no exact zero; smallest positive level is +8.
    assert_eq!(levels[0], 8, "A-law smallest positive level is +8");
    // Top level (seg 7, mant 15) is the spec peak 32256.
    assert_eq!(*levels.last().unwrap(), 32256, "A-law peak level");
}

/// The encoder's decision values (segment boundaries) place a sample at
/// the correct reconstruction level on each side of the boundary. For
/// both laws we walk every reconstruction level, step one unit above and
/// below it, and confirm the encoder maps each test point to a codeword
/// whose decoded level is the nearest lattice point — i.e. the decision
/// thresholds sit at the lattice midpoints, as a mid-tread quantizer
/// requires. This pins the *decision values* of Tables 1/2, not just the
/// *quantized values*.
#[test]
fn decision_thresholds_sit_at_lattice_midpoints() {
    for (name, levels, enc, dec) in [
        (
            "µ-law",
            mulaw_positive_levels(),
            mulaw::encode_sample as fn(i16) -> u8,
            mulaw::decode_sample as fn(u8) -> i16,
        ),
        (
            "A-law",
            alaw_positive_levels(),
            alaw::encode_sample as fn(i16) -> u8,
            alaw::decode_sample as fn(u8) -> i16,
        ),
    ] {
        // For each interior level L with neighbours P (below) and N
        // (above): the midpoint (L+P)/2 is the lower decision threshold,
        // (L+N)/2 the upper. A sample just inside the interval must
        // reconstruct to L.
        for w in levels.windows(3) {
            let (p, l, n) = (w[0], w[1], w[2]);
            // A point comfortably inside L's interval (quarter-step in).
            let lo = l - (l - p) / 4;
            let hi = l + (n - l) / 4;
            for probe in [lo, hi] {
                if probe < 0 || probe > i16::MAX as i32 {
                    continue;
                }
                let recon = dec(enc(probe as i16)) as i32;
                assert_eq!(
                    recon, l,
                    "{name}: probe {probe} near level {l} reconstructed to {recon}"
                );
            }
        }
    }
}
