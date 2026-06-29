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
//! There are **two distinct, both-legal** G.711 law conversions, and this
//! file pins the relationship between them:
//!
//! 1. The **normative table-driven conversion** of §3.5 — Tables 3/4
//!    directly map one law's value number to the other's. This conversion
//!    embeds a deliberate transparency modification described in §3.6
//!    Note 2 ("µ-80 is converted to A-81 instead of A-80, and A-80 is
//!    converted to µ-79 instead of µ-80"), which makes the double
//!    conversions µ-A-µ and A-µ-A transparent to PCM bits 1-7.
//! 2. The **equipment-option conversion** of §3.6 — re-quantise through
//!    uniform PCM, `encode_B(decode_A(b))`. §3.6 explicitly leaves this
//!    "to the individual equipment specification." This crate implements
//!    this path.
//!
//! The two agree on the bulk of the value-number axis (the value numbers
//! in Tables 1/2 are magnitude-ordered and the §3.6 round-trip lands on
//! the nearest level), but they **deliberately differ** at a fixed set of
//! value numbers near the segment boundaries and the {µ-80 ↔ A-81}
//! transparency tweak. That divergence set is enumerated below
//! ([`MU_TO_A_EQUIP_DIVERGENCE`] / [`A_TO_MU_EQUIP_DIVERGENCE`]) and is
//! itself pinned: each divergence is a single value-number step, the
//! signature of a different rounding choice at a quantizer boundary, not
//! an arbitrary jump.
//!
//! The tables are reproduced here as the literal `(input value number,
//! output value number)` pairs printed in the Recommendation, transcribed
//! directly from `docs/audio/g711/T-REC-G.711-198811-I.pdf`. They are the
//! single source of truth for the normative leg; the value-number ↔
//! codeword bijection is rebuilt from the crate's own decode tables by
//! sorting each law's positive codewords in magnitude order (the
//! definition of "decoder output value number" — Tables 1a/2a column 8).
//! The corrected tables are self-validated by
//! [`normative_tables_reproduce_section_3_6_note_2`], which round-trips
//! them and asserts the change sets match the §3.6 Note 2 prose exactly.
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
    // µ 0..=13
    1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, // µ 14..=27
    8, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, // µ 28..=41
    21, 22, 23, 24, 25, 27, 29, 31, 33, 34, 35, 36, 37, 38, // µ 42..=55
    39, 40, 41, 42, 43, 44, 46, 48, 49, 50, 51, 52, 53, 54, // µ 56..=69
    55, 56, 57, 58, 59, 60, 61, 62, 64, 65, 66, 67, 68, 69, // µ 70..=83
    70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 81, 82, 83, 84, // µ 84..=97
    85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95, 96, 97, 98, // µ 98..=111
    99, 100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112, // µ 112..=125
    113, 114, 115, 116, 117, 118, 119, 120, 121, 122, 123, 124, 125, 126, // µ 126..=127
    127, 128,
];

/// Table 4/G.711 — A-law value number → µ-law value number, all 128
/// positive levels (A value numbers 1..=128, indexed here `n-1`).
/// Transcribed verbatim from the Recommendation.
const TABLE4_A_TO_MU: [u32; 128] = [
    // A 1..=14
    1, 3, 5, 7, 9, 11, 13, 15, 16, 17, 18, 19, 20, 21, // A 15..=28
    22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 32, 33, 33, // A 29..=42
    34, 34, 35, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, // A 43..=56
    46, 47, 48, 48, 49, 49, 50, 51, 52, 53, 54, 55, 56, 57, // A 57..=70
    58, 59, 60, 61, 62, 63, 64, 64, 65, 66, 67, 68, 69, 70, // A 71..=84
    71, 72, 73, 74, 75, 76, 77, 78, 79, 79, 80, 81, 82, 83, // A 85..=98
    84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95, 96, 97, // A 99..=112
    98, 99, 100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, // A 113..=126
    112, 113, 114, 115, 116, 117, 118, 119, 120, 121, 122, 123, 124, 125, // A 127..=128
    126, 127,
];

/// µ-law value numbers (0-based) where the crate's §3.6 equipment-option
/// PCM-roundtrip transcode lands on a different A-law value number than
/// the normative Table 3 mapping. These are the quantizer-boundary and
/// transparency-tweak positions; everywhere else the two conversions
/// agree. At each, the equipment option is one value number *below* the
/// normative table.
const MU_TO_A_EQUIP_DIVERGENCE: &[usize] = &[
    48, 49, 64, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95,
];

/// A-law value numbers (1-based) where the crate's §3.6 equipment-option
/// PCM-roundtrip transcode lands on a different µ-law value number than
/// the normative Table 4 mapping. At each, the equipment option is one
/// value number *above* the normative table (the {A-80→µ-79} tweak and
/// the run that follows it).
const A_TO_MU_EQUIP_DIVERGENCE: &[usize] = &[
    80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95, 96,
];

/// The crate's §3.6 equipment-option µ→A PCM-roundtrip transcode
/// (decode µ, re-encode A) reproduces the normative Table 3/G.711
/// value-number correspondence for every positive µ-law level **except**
/// the documented [`MU_TO_A_EQUIP_DIVERGENCE`] set, where the
/// equipment-option rounding differs from the table-driven conversion by
/// exactly one value number (the table-driven path embeds the §3.6 Note 2
/// transparency tweak; the equipment option, also legal, does not).
#[test]
fn mulaw_to_alaw_matches_table3_positive_modulo_equipment_option() {
    let mu = mulaw_positive_codewords_by_value_number();
    let a = alaw_positive_codewords_by_value_number();
    // A-law codeword → value number (1..=128).
    let mut a_value_of_codeword = [0u32; 256];
    for (i, &cw) in a.iter().enumerate() {
        a_value_of_codeword[cw as usize] = (i + 1) as u32;
    }

    let mut observed_divergences = Vec::new();
    for (mu_value, &table_a_value) in TABLE3_MU_TO_A.iter().enumerate() {
        let mu_cw = mu[mu_value];
        // §3.6 equipment option: decode µ to PCM, re-encode as A.
        let a_cw = alaw::encode_sample(mulaw::decode_sample(mu_cw));
        let got_a_value = a_value_of_codeword[a_cw as usize];
        if got_a_value != table_a_value {
            observed_divergences.push(mu_value);
            // Each divergence is a single value-number step below the table.
            assert_eq!(
                got_a_value + 1,
                table_a_value,
                "µ value number {mu_value} (codeword 0x{mu_cw:02X}): equipment \
                 option gives A {got_a_value}, table gives A {table_a_value} — \
                 expected a single-step divergence"
            );
        }
    }
    assert_eq!(
        observed_divergences, MU_TO_A_EQUIP_DIVERGENCE,
        "the µ→A equipment-option divergence set drifted from the documented set"
    );
}

/// The crate's §3.6 equipment-option A→µ PCM-roundtrip transcode
/// reproduces the normative Table 4/G.711 value-number correspondence for
/// every positive A-law level **except** the documented
/// [`A_TO_MU_EQUIP_DIVERGENCE`] set, where it differs by exactly one
/// value number.
#[test]
fn alaw_to_mulaw_matches_table4_positive_modulo_equipment_option() {
    let mu = mulaw_positive_codewords_by_value_number();
    let a = alaw_positive_codewords_by_value_number();
    // µ-law codeword → value number (0..=127).
    let mut mu_value_of_codeword = [0u32; 256];
    for (i, &cw) in mu.iter().enumerate() {
        mu_value_of_codeword[cw as usize] = i as u32;
    }

    let mut observed_divergences = Vec::new();
    for (idx, &table_mu_value) in TABLE4_A_TO_MU.iter().enumerate() {
        // A value numbers run 1..=128; `idx` is `value - 1`.
        let a_cw = a[idx];
        let mu_cw = mulaw::encode_sample(alaw::decode_sample(a_cw));
        let got_mu_value = mu_value_of_codeword[mu_cw as usize];
        if got_mu_value != table_mu_value {
            observed_divergences.push(idx + 1);
            assert_eq!(
                got_mu_value,
                table_mu_value + 1,
                "A value number {} (codeword 0x{a_cw:02X}): equipment option \
                 gives µ {got_mu_value}, table gives µ {table_mu_value} — \
                 expected a single-step divergence",
                idx + 1
            );
        }
    }
    assert_eq!(
        observed_divergences, A_TO_MU_EQUIP_DIVERGENCE,
        "the A→µ equipment-option divergence set drifted from the documented set"
    );
}

/// Self-validation of the corrected Tables 3/4: round-tripping the two
/// normative mappings against each other must reproduce the §3.6 Note 2
/// change sets *exactly* — the prose names the value numbers that change
/// under the double conversions, and that is the acceptance test for the
/// table transcription itself (independent of the crate's codec).
///
/// §3.6 Note 2 (Table 3): µ-A-µ changes µ value numbers
/// {0, 2, 4, 6, 8, 10, 12, 14}.
/// §3.6 Note 2 (Table 4): A-µ-A changes A value numbers
/// {26, 28, 30, 32, 45, 47, 63, 80}.
#[test]
fn normative_tables_reproduce_section_3_6_note_2() {
    // A-µ-A using the tables alone: A value n → µ = T4[n-1] → A' = T3[µ].
    let mut amua_changed = Vec::new();
    for n in 1u32..=128 {
        let mu = TABLE4_A_TO_MU[(n - 1) as usize];
        let a2 = TABLE3_MU_TO_A[mu as usize];
        if a2 != n {
            amua_changed.push(n);
        }
    }
    assert_eq!(
        amua_changed,
        vec![26, 28, 30, 32, 45, 47, 63, 80],
        "Table 3/4 A-µ-A round trip must reproduce the §3.6 Note 2 A value-number set"
    );

    // µ-A-µ: µ value m → A = T3[m] → µ' = T4[A-1].
    let mut muam_changed = Vec::new();
    for m in 0u32..=127 {
        let a = TABLE3_MU_TO_A[m as usize];
        let mu2 = TABLE4_A_TO_MU[(a - 1) as usize];
        if mu2 != m {
            muam_changed.push(m);
        }
    }
    assert_eq!(
        muam_changed,
        vec![0, 2, 4, 6, 8, 10, 12, 14],
        "Table 3/4 µ-A-µ round trip must reproduce the §3.6 Note 2 µ value-number set"
    );

    // §3.6 Note 2 also names the deliberate tweak literally: µ-80 → A-81
    // and A-80 → µ-79. Pin both directly.
    assert_eq!(TABLE3_MU_TO_A[80], 81, "deliberate tweak: µ-80 → A-81");
    assert_eq!(TABLE4_A_TO_MU[80 - 1], 79, "deliberate tweak: A-80 → µ-79");
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

/// Both conversions are symmetric under sign negation: the negative half
/// of each law (Tables 1b/2b are exact mirror images of 1a/2a) reproduces
/// the same value-number correspondence as the positive half — including
/// the same equipment-option divergence set from the normative tables. We
/// verify by negating every positive input codeword's sign bit and
/// re-running the value-number check by magnitude, so the negative table
/// is exercised without a second literal transcription. The divergence
/// from the normative Tables 3/4 must be the identical
/// [`MU_TO_A_EQUIP_DIVERGENCE`] / [`A_TO_MU_EQUIP_DIVERGENCE`] sets, each a
/// single value-number step (the equipment option is sign-symmetric).
#[test]
fn conversion_tables_hold_on_negative_half_modulo_equipment_option() {
    let mu_pos = mulaw_positive_codewords_by_value_number();
    let a_pos = alaw_positive_codewords_by_value_number();

    // µ→A on the negative half: A value numbers run 1..=128, so the
    // magnitude-rank index `i` is value number `i + 1`.
    let mut mu_to_a_div = Vec::new();
    for (mu_value, &table_a_value) in TABLE3_MU_TO_A.iter().enumerate() {
        let mu_neg_cw = mu_pos[mu_value] ^ 0x80;
        let a_cw = alaw::encode_sample(mulaw::decode_sample(mu_neg_cw));
        let got_a_value = value_number_by_magnitude(&a_pos, a_cw, alaw::decode_sample) + 1;
        if got_a_value != table_a_value {
            mu_to_a_div.push(mu_value);
            assert_eq!(
                got_a_value + 1,
                table_a_value,
                "negative-half µ→A: µ value {mu_value} diverges by more than one step"
            );
        }
    }
    assert_eq!(
        mu_to_a_div, MU_TO_A_EQUIP_DIVERGENCE,
        "negative-half µ→A divergence set must mirror the positive half"
    );

    // A→µ on the negative half: µ value numbers run 0..=127, so the
    // magnitude-rank index is the value number directly.
    let mut a_to_mu_div = Vec::new();
    for (idx, &table_mu_value) in TABLE4_A_TO_MU.iter().enumerate() {
        let a_neg_cw = a_pos[idx] ^ 0x80;
        let mu_cw = mulaw::encode_sample(alaw::decode_sample(a_neg_cw));
        let got_mu_value = value_number_by_magnitude(&mu_pos, mu_cw, mulaw::decode_sample);
        if got_mu_value != table_mu_value {
            a_to_mu_div.push(idx + 1);
            assert_eq!(
                got_mu_value,
                table_mu_value + 1,
                "negative-half A→µ: A value {} diverges by more than one step",
                idx + 1
            );
        }
    }
    assert_eq!(
        a_to_mu_div, A_TO_MU_EQUIP_DIVERGENCE,
        "negative-half A→µ divergence set must mirror the positive half"
    );
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

/// G.711 §3.6 Note 2 (Table 4): an A-µ conversion followed by a µ-A
/// conversion changes exactly **8 octets per sign**, and the
/// Recommendation names the specific A-law decoder-output value numbers
/// that change under the **normative table-driven** conversion:
/// {26, 28, 30, 32, 45, 47, 63, 80} — pinned exactly against the tables
/// in [`normative_tables_reproduce_section_3_6_note_2`].
///
/// This crate implements the §3.6 **equipment-option** PCM-roundtrip,
/// which is a different (also legal) conversion: it does **not** embed the
/// deliberate {µ-80 ↔ A-81} transparency tweak, so its A-µ-A change set is
/// the segment-boundary set {26, 28, 30, 32, 46, 48, 64, 96} rather than
/// the table-driven {…, 45, 47, 63, 80}. The two agree on the count (8 per
/// sign) and on the lower four value numbers; they differ on the upper
/// four because the equipment option rounds at the nearest level instead
/// of following the table's deliberate modification. We pin the
/// equipment-option set here so a future change to either companding curve
/// that perturbed the transcode would fail.
#[test]
fn alaw_mulaw_alaw_equipment_option_changes_exactly_eight_value_numbers() {
    let a = alaw_positive_codewords_by_value_number();
    // The equipment-option (PCM-roundtrip) A-law value numbers (1-based).
    let equip_changed: [u32; 8] = [26, 28, 30, 32, 46, 48, 64, 96];

    let mut changed_value_numbers = Vec::new();
    for (idx, &a_cw) in a.iter().enumerate() {
        let mu_cw = mulaw::encode_sample(alaw::decode_sample(a_cw));
        let back = alaw::encode_sample(mulaw::decode_sample(mu_cw));
        if back != a_cw {
            changed_value_numbers.push((idx + 1) as u32); // 1-based value number
        }
    }
    assert_eq!(
        changed_value_numbers, equip_changed,
        "A-µ-A equipment-option transparency: changed A value numbers must be \
         exactly {equip_changed:?} (the segment-boundary set; the normative \
         table-driven set {{26,28,30,32,45,47,63,80}} is pinned separately)"
    );
    // Both sets have the §3.6 Note-2 count of 8.
    assert_eq!(changed_value_numbers.len(), 8);
}

/// §3.6 Note 2 also states the *only* PCM bit that changes in either
/// double-conversion direction is bit No. 8 — the least significant bit
/// of the value-number / magnitude axis — so bits Nos. 1-7 are fully
/// transparent. For A-µ-A we verify every changed value number moves by
/// exactly one decoder-output level (one value number), i.e. a single-LSB
/// change on the magnitude rank, never a larger jump.
#[test]
fn alaw_mulaw_alaw_changes_are_single_value_number_steps() {
    let a = alaw_positive_codewords_by_value_number();
    // codeword → A value number (1-based) by magnitude rank.
    let mut a_value_of_codeword = [0u32; 256];
    for (i, &cw) in a.iter().enumerate() {
        a_value_of_codeword[cw as usize] = (i + 1) as u32;
    }

    for (idx, &a_cw) in a.iter().enumerate() {
        let mu_cw = mulaw::encode_sample(alaw::decode_sample(a_cw));
        let back = alaw::encode_sample(mulaw::decode_sample(mu_cw));
        if back != a_cw {
            let before = (idx + 1) as u32;
            let after = a_value_of_codeword[back as usize];
            let step = before.abs_diff(after);
            assert_eq!(
                step, 1,
                "A-µ-A: value number {before} moved to {after} (step {step}); \
                 §3.6 Note 2 allows only a single-LSB (one value-number) change"
            );
        }
    }
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
