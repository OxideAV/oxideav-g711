//! ITU-T G.711 §3.2 — µ-law all-zero character-signal suppression.
//!
//! G.711 §3.2 (third paragraph): *"When using the µ-law in networks where
//! suppression of the all 0 character signal is required, the character
//! signal corresponding to negative input values between decision values
//! numbers 127 and 128 should be `00000010` and the value at the decoder
//! output is -7519. The corresponding decoder output value number is 125."*
//!
//! Long runs of the all-zero octet (`0x00`) on a T1-style span starve the
//! receiver's bit-clock recovery, so the standard forbids the one codeword
//! that would be transmitted as all zeros and substitutes the spec-given wire
//! codeword `00000010` (`0x02`). [`mulaw::encode_sample_zero_suppress`]
//! implements exactly that rewrite.
//!
//! These tests pin the *wire-level* contract the spec states literally
//! (forbidden codeword + replacement codeword), which is convention-
//! independent. The numeric decoder-output value -7519 quoted by §3.2 is in
//! the spec's own 14-bit Table-2 magnitude convention, which differs from the
//! FFmpeg-style decode scaling this crate uses; see the test-module note for
//! the documented gap.

use oxideav_g711::mulaw::{
    decode_sample, encode_sample, encode_sample_zero_suppress, MULAW_ZERO_CODEWORD,
    MULAW_ZERO_SUPPRESS_CODEWORD,
};

/// The literal codewords the spec names: the forbidden all-zero word and its
/// `00000010` replacement.
#[test]
fn spec_codeword_constants_are_literal() {
    assert_eq!(MULAW_ZERO_CODEWORD, 0b0000_0000);
    assert_eq!(MULAW_ZERO_SUPPRESS_CODEWORD, 0b0000_0010);
}

/// The all-zero octet must never be emitted in suppression mode, over the
/// full 16-bit input domain.
#[test]
fn suppression_never_emits_all_zero_octet() {
    for s in i16::MIN..=i16::MAX {
        assert_ne!(
            encode_sample_zero_suppress(s),
            MULAW_ZERO_CODEWORD,
            "all-zero codeword leaked for input {s}"
        );
    }
}

/// Every input that the plain encoder maps to the all-zero codeword must map
/// to exactly the `00000010` replacement in suppression mode, and every other
/// input must be byte-identical to the plain encoder.
#[test]
fn suppression_rewrites_only_the_all_zero_codeword() {
    let mut rewritten = 0u32;
    for s in i16::MIN..=i16::MAX {
        let plain = encode_sample(s);
        let supp = encode_sample_zero_suppress(s);
        if plain == MULAW_ZERO_CODEWORD {
            assert_eq!(
                supp, MULAW_ZERO_SUPPRESS_CODEWORD,
                "input {s} (plain 0x00) was not rewritten to 0x02"
            );
            rewritten += 1;
        } else {
            assert_eq!(
                supp, plain,
                "input {s} changed under suppression but did not map to 0x00"
            );
        }
    }
    // The plain encoder maps the most-negative segment-7 inputs
    // (-32768 ..= -31612, 1157 values) to the all-zero codeword; all of them
    // are rewritten and nothing else is.
    assert_eq!(rewritten, 1157, "unexpected count of rewritten inputs");
}

/// The boundary of the all-zero region: the most-negative codeword above the
/// rewrite threshold (`-31611`) already carries a non-zero LSB (`0x01`) and is
/// left untouched, while the threshold sample (`-31612`) is rewritten.
#[test]
fn suppression_boundary_is_exact() {
    assert_eq!(encode_sample(-31612), MULAW_ZERO_CODEWORD);
    assert_eq!(
        encode_sample_zero_suppress(-31612),
        MULAW_ZERO_SUPPRESS_CODEWORD
    );

    assert_eq!(encode_sample(-31611), 0x01);
    assert_eq!(encode_sample_zero_suppress(-31611), 0x01);
}

/// A conformant decoder is untouched by suppression: the substituted `0x02`
/// decodes like any other codeword, and it sits one quantisation step inward
/// (toward smaller negative magnitude) from the suppressed `0x00`, i.e. the
/// substitution is the §3.2 "value number 126 → 125" move of one decision
/// interval, not an arbitrary jump.
#[test]
fn substituted_codeword_decodes_as_adjacent_negative_level() {
    let v00 = decode_sample(0x00) as i32; // the codeword that was forbidden
    let v01 = decode_sample(0x01) as i32; // the intervening quant level
    let v02 = decode_sample(MULAW_ZERO_SUPPRESS_CODEWORD) as i32; // the replacement

    // All three are negative (segment-7 negative region in this crate's
    // wire convention) and strictly increase toward zero by codeword index.
    assert!(v00 < v01 && v01 < v02 && v02 < 0);
    // Uniform segment-7 step: the levels are evenly spaced.
    assert_eq!(v01 - v00, v02 - v01);
}
