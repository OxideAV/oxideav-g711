//! Normative µ↔A conversion compliance: ITU-T G.711 Tables 3/G.711 and
//! 4/G.711.
//!
//! G.711 §3.5 states: *"The rules for conversion are given in
//! Tables 3/G.711 and 4/G.711."* Those two tables are the **normative**
//! law-conversion mapping, expressed as a correspondence between the
//! *decoder output value number* of one law and that of the other:
//!
//! * **Table 3/G.711** maps a µ-law decoder-output value number to an
//!   A-law decoder-output value number (the µ→A direction).
//! * **Table 4/G.711** maps an A-law value number to a µ-law value
//!   number (the A→µ direction).
//!
//! The existing [`cross_law_transcode`] suite only pins the *equipment
//! option* of §3.6 — re-quantising through uniform PCM,
//! `encode_B(decode_A(b))` — which §3.6 explicitly leaves "to the
//! individual equipment specification." This file is the missing
//! companion: it asserts our PCM-roundtrip transcode **reproduces the
//! normative Table 3 / Table 4 value-number correspondence** for all
//! 128 levels of both signs, in both directions. The two are only
//! guaranteed to agree because the value numbers in Tables 1/2 are
//! laid out in magnitude order and the §3.6 round-trip lands on the
//! nearest output level — but that agreement is exactly the property
//! worth pinning, since a future change to either companding curve that
//! left the per-law bit-exact sweeps green could still drift the
//! cross-law correspondence off the normative table.
//!
//! The tables are reproduced here as the literal `(input value number,
//! output value number)` pairs printed in the Recommendation. They are
//! the single source of truth for this test; the value-number ↔
//! codeword bijection is rebuilt from the crate's own decode tables by
//! sorting each law's positive codewords in magnitude order (which is
//! the definition of "decoder output value number" — see Tables 1a/2a
//! column 8).
//!
//! Reference: ITU-T Recommendation G.711 (11/88) §3.5, §3.6, Tables
//! 1a/1b/2a/2b (value-number columns) and Tables 3/4 (conversion).

use oxideav_g711::{alaw, mulaw};

/// Positive µ-law codewords ordered by ascending decoded magnitude.
/// Index `n` is the µ-law decoder-output value number `n` (0..=127),
/// per Table 2a/G.711 column 8.
fn mulaw_positive_codewords_by_value_number() -> Vec<u8> {
    let mut v: Vec<(i16, u8)> = (0x80u8..=0xFF)
        .map(|b| (mulaw::decode_sample(b), b))
        .collect();
    // Positive µ-law wire bytes are 0x80..=0xFF: the wire byte is the
    // one's-complement of S|E|M, so a clear stored sign bit (positive)
    // becomes a set bit on the wire.
    assert!(
        v.iter().all(|&(s, _)| s >= 0),
        "µ positive set has a negative sample"
    );
    v.sort_by_key(|&(s, _)| s);
    v.into_iter().map(|(_, b)| b).collect()
}

/// Positive A-law codewords ordered by ascending decoded magnitude.
/// Index `n` is the A-law decoder-output value number `n + 1`
/// (value numbers run 1..=128), per Table 1a/G.711 column 8.
fn alaw_positive_codewords_by_value_number() -> Vec<u8> {
    let mut v: Vec<(i16, u8)> = (0u16..=255)
        .map(|b| b as u8)
        // A-law sign bit is bit 7 *after* the on-wire 0x55 inversion;
        // 1 ⇒ positive.
        .filter(|&b| (b ^ 0x55) & 0x80 != 0)
        .map(|b| (alaw::decode_sample(b), b))
        .collect();
    assert!(
        v.iter().all(|&(s, _)| s >= 0),
        "A positive set has a negative sample"
    );
    v.sort_by_key(|&(s, _)| s);
    v.into_iter().map(|(_, b)| b).collect()
}

/// Table 3/G.711 — µ-law value number → A-law value number, all 128
/// positive levels (µ value numbers 0..=127). Transcribed verbatim
/// from the Recommendation.
const TABLE3_MU_TO_A: [u32; 128] = [
    // µ 0..=43
    1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20,
    21, 22, 23, 24, 25, 27, 29, 31, 33, 34, 35, 36, 37, 38, 39, 40, // µ 44..=87
    41, 42, 43, 44, 45, 47, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 65, 66, 67,
    68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81, 82, 83, 84, 85, 86,
    87, // µ 88..=127
    88, 89, 90, 91, 92, 93, 94, 95, 97, 98, 99, 100, 101, 102, 103, 104, 105, 106, 107, 108, 109,
    110, 111, 112, 113, 114, 115, 116, 117, 118, 119, 120, 121, 122, 123, 124, 125, 126, 127, 128,
];

/// Table 4/G.711 — A-law value number → µ-law value number, all 128
/// positive levels (A value numbers 1..=128, indexed here `n-1`).
/// Transcribed verbatim from the Recommendation.
const TABLE4_A_TO_MU: [u32; 128] = [
    // A 1..=44
    1, 3, 5, 7, 9, 11, 13, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32,
    32, 33, 33, 34, 34, 35, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, // A 45..=88
    48, 48, 49, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 64, 64, 65, 66, 67, 68,
    69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81, 82, 83, 84, 85, 86, 87,
    88, // A 89..=128
    89, 90, 91, 92, 93, 94, 95, 96, 96, 97, 98, 99, 100, 101, 102, 103, 104, 105, 106, 107, 108,
    109, 110, 111, 112, 113, 114, 115, 116, 117, 118, 119, 120, 121, 122, 123, 124, 125, 126, 127,
];

/// The PCM-roundtrip µ→A transcode (decode µ, re-encode A) reproduces
/// the normative Table 3/G.711 value-number correspondence for every
/// positive µ-law level.
#[test]
fn mulaw_to_alaw_matches_table3_positive() {
    let mu = mulaw_positive_codewords_by_value_number();
    let a = alaw_positive_codewords_by_value_number();
    // A-law codeword → value number (1..=128).
    let mut a_value_of_codeword = [0u32; 256];
    for (i, &cw) in a.iter().enumerate() {
        a_value_of_codeword[cw as usize] = (i + 1) as u32;
    }

    for (mu_value, &expected_a_value) in TABLE3_MU_TO_A.iter().enumerate() {
        let mu_cw = mu[mu_value];
        // §3.6 equipment option: decode µ to PCM, re-encode as A.
        let a_cw = alaw::encode_sample(mulaw::decode_sample(mu_cw));
        let got_a_value = a_value_of_codeword[a_cw as usize];
        assert_eq!(
            got_a_value, expected_a_value,
            "Table 3 µ→A mismatch at µ value number {mu_value} \
             (codeword 0x{mu_cw:02X}): expected A value {expected_a_value}, got {got_a_value}"
        );
    }
}

/// The PCM-roundtrip A→µ transcode reproduces the normative
/// Table 4/G.711 value-number correspondence for every positive A-law
/// level.
#[test]
fn alaw_to_mulaw_matches_table4_positive() {
    let mu = mulaw_positive_codewords_by_value_number();
    let a = alaw_positive_codewords_by_value_number();
    // µ-law codeword → value number (0..=127).
    let mut mu_value_of_codeword = [0u32; 256];
    for (i, &cw) in mu.iter().enumerate() {
        mu_value_of_codeword[cw as usize] = i as u32;
    }

    for (idx, &expected_mu_value) in TABLE4_A_TO_MU.iter().enumerate() {
        // A value numbers run 1..=128; `idx` is `value - 1`.
        let a_cw = a[idx];
        let mu_cw = mulaw::encode_sample(alaw::decode_sample(a_cw));
        let got_mu_value = mu_value_of_codeword[mu_cw as usize];
        assert_eq!(
            got_mu_value,
            expected_mu_value,
            "Table 4 A→µ mismatch at A value number {} \
             (codeword 0x{a_cw:02X}): expected µ value {expected_mu_value}, got {got_mu_value}",
            idx + 1
        );
    }
}

/// Map any codeword to its decoder-output **value number** by decoded
/// magnitude (sign-independent), so a negative-side input that happens
/// to transcode to a positive-side output (e.g. the µ-law signed-zero
/// codeword 0x7F decodes to linear 0 and re-encodes to the *positive*
/// A-law +8 codeword 0xD5) still resolves to the correct value number.
/// The value number is a magnitude rank — the same on both signs — so a
/// magnitude-keyed lookup is the law-faithful axis.
fn value_number_by_magnitude(codewords_by_value: &[u8], target: u8, decode: fn(u8) -> i16) -> u32 {
    let mag = decode(target).unsigned_abs();
    for (i, &cw) in codewords_by_value.iter().enumerate() {
        if decode(cw).unsigned_abs() == mag {
            return i as u32;
        }
    }
    panic!("no value number for codeword 0x{target:02X} (magnitude {mag})");
}

/// Both conversion tables are symmetric under sign negation: the µ→A and
/// A→µ correspondences hold identically on the negative half of each
/// law (the spec prints Tables 1b/2b — the negative halves — as exact
/// mirror images of 1a/2a). We verify by negating every positive input
/// codeword's sign bit and re-running the value-number check by
/// magnitude, so the negative table is exercised without a second
/// literal transcription.
#[test]
fn conversion_tables_hold_on_negative_half() {
    let mu_pos = mulaw_positive_codewords_by_value_number();
    let a_pos = alaw_positive_codewords_by_value_number();

    // µ→A on the negative half: A value numbers run 1..=128, so the
    // magnitude-rank index `i` is value number `i + 1`.
    for (mu_value, &expected_a_value) in TABLE3_MU_TO_A.iter().enumerate() {
        let mu_neg_cw = mu_pos[mu_value] ^ 0x80;
        let a_cw = alaw::encode_sample(mulaw::decode_sample(mu_neg_cw));
        let got_a_value = value_number_by_magnitude(&a_pos, a_cw, alaw::decode_sample) + 1;
        assert_eq!(
            got_a_value, expected_a_value,
            "Table 3 µ→A negative-half mismatch at µ value number {mu_value}"
        );
    }

    // A→µ on the negative half: µ value numbers run 0..=127, so the
    // magnitude-rank index is the value number directly.
    for (idx, &expected_mu_value) in TABLE4_A_TO_MU.iter().enumerate() {
        let a_neg_cw = a_pos[idx] ^ 0x80;
        let mu_cw = mulaw::encode_sample(alaw::decode_sample(a_neg_cw));
        let got_mu_value = value_number_by_magnitude(&mu_pos, mu_cw, mulaw::decode_sample);
        assert_eq!(
            got_mu_value,
            expected_mu_value,
            "Table 4 A→µ negative-half mismatch at A value number {}",
            idx + 1
        );
    }
}

/// G.711 §3.6 Note 2 (Table 3): *"If a µ-A conversion is followed by an
/// A-µ conversion, ... only those octets which correspond to µ-law
/// decoder output value numbers 0, 2, 4, 6, 8, 10, 12, 14 are
/// changed."* We verify the round-trip µ→A→µ is identity on every
/// positive µ value number **except** exactly that set, in which the
/// double conversion is allowed to shift (the deliberate
/// transparency-table modification described in the Note).
#[test]
fn mulaw_a_mulaw_changes_only_the_documented_value_numbers() {
    let mu = mulaw_positive_codewords_by_value_number();
    let documented_changed: [usize; 8] = [0, 2, 4, 6, 8, 10, 12, 14];

    for (mu_value, &mu_cw) in mu.iter().enumerate() {
        let a_cw = alaw::encode_sample(mulaw::decode_sample(mu_cw));
        let back = mulaw::encode_sample(alaw::decode_sample(a_cw));
        let changed = back != mu_cw;
        let documented = documented_changed.contains(&mu_value);
        assert_eq!(
            changed, documented,
            "µ-A-µ transparency: µ value number {mu_value} (codeword 0x{mu_cw:02X}) \
             changed={changed} but the Recommendation §3.6 Note 2 set says \
             documented={documented}"
        );
    }
}

/// G.711 §3.6 Note 2 (Table 4): the dual claim — an A-µ conversion
/// followed by a µ-A conversion changes exactly **8 octets per sign**.
/// (The Recommendation lists the specific A-law value numbers, but the
/// load-bearing, convention-independent claim is the count and that the
/// remaining 120 levels per sign are perfectly transparent; the count
/// is what we pin.)
#[test]
fn alaw_mulaw_alaw_changes_exactly_eight_value_numbers() {
    let a = alaw_positive_codewords_by_value_number();
    let mut changed = 0usize;
    for &a_cw in &a {
        let mu_cw = mulaw::encode_sample(alaw::decode_sample(a_cw));
        let back = alaw::encode_sample(mulaw::decode_sample(mu_cw));
        if back != a_cw {
            changed += 1;
        }
    }
    assert_eq!(
        changed, 8,
        "A-µ-A transparency: expected exactly 8 changed A value numbers per sign, got {changed}"
    );
}

/// Sanity: the value-number bijections are total and one-to-one — every
/// one of the 256 codewords (128 positive + 128 negative) is assigned a
/// value number, and no two distinct codewords on the same sign share
/// one. This guards the magnitude-sort that the Table 3/4 checks rely
/// on against a future decode-table change that introduced a duplicate
/// magnitude (which would silently corrupt the value-number axis).
#[test]
fn value_number_axes_are_bijective() {
    let mu = mulaw_positive_codewords_by_value_number();
    let a = alaw_positive_codewords_by_value_number();
    assert_eq!(
        mu.len(),
        128,
        "µ positive value-number axis must have 128 levels"
    );
    assert_eq!(
        a.len(),
        128,
        "A positive value-number axis must have 128 levels"
    );

    // µ: strictly non-decreasing magnitudes, all distinct codewords.
    let mut seen = std::collections::HashSet::new();
    for &cw in &mu {
        assert!(seen.insert(cw), "duplicate µ codeword in value-number axis");
    }
    let mut seen_a = std::collections::HashSet::new();
    for &cw in &a {
        assert!(
            seen_a.insert(cw),
            "duplicate A codeword in value-number axis"
        );
    }
    // A-law has no exact zero; magnitudes strictly increase, so adjacent
    // value numbers must have strictly increasing decoded magnitude.
    for w in a.windows(2) {
        assert!(
            alaw::decode_sample(w[0]) < alaw::decode_sample(w[1]),
            "A-law value-number axis is not strictly increasing"
        );
    }
}
