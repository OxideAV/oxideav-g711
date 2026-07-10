#![no_main]

//! Drive arbitrary fuzz-supplied i16 samples through the µ-law all-zero
//! character-signal suppression path
//! ([`mulaw::encode_sample_zero_suppress`], ITU-T G.711 §3.2) and assert
//! the wire-level invariants the spec states literally:
//!
//! 1. **The all-zero octet is never emitted.** §3.2 forbids the one
//!    codeword that would be transmitted as `00000000`
//!    ([`MULAW_ZERO_CODEWORD`]) so a long run of zero octets cannot
//!    starve a T1-style receiver's bit-clock recovery. The suppressed
//!    encoder must therefore return a non-`0x00` byte for *every* i16
//!    input.
//!
//! 2. **Suppression is the identity everywhere except the all-zero
//!    codeword.** For any sample whose plain codeword
//!    ([`mulaw::encode_sample`]) is not `0x00`, the suppressed encoder
//!    must return the *same* byte — the rewrite touches exactly the
//!    forbidden codeword and nothing else. For any sample whose plain
//!    codeword *is* `0x00`, the suppressed encoder must return exactly
//!    the spec replacement `00000010` ([`MULAW_ZERO_SUPPRESS_CODEWORD`]).
//!
//! 3. **The decoder is untouched and the substitution stays in-segment.**
//!    A conformant decoder maps the substituted `0x02` like any other
//!    codeword; the replacement sits one quantisation step inward (toward
//!    smaller negative magnitude) from the suppressed `0x00`, so the two
//!    decode to strictly-ordered negative levels separated by the uniform
//!    segment-7 step. This is the §3.2 "move by one decision interval"
//!    contract, not an arbitrary jump.
//!
//! These properties hold *by construction* in the crate (the rewrite is
//! folded into the compile-time `MULAW_ENCODE_ZERO_SUPPRESS` table, whose
//! entries are the plain-law formula plus a single equality rewrite); the
//! fuzzer drives them across attacker-chosen i16 sequences as a regression
//! net against any future change to the suppression table or the
//! underlying encode LUT.
//!
//! ## Fuzz input layout
//!
//! Each input is consumed as a sequence of 2-byte little-endian i16
//! samples (`chunks(2)`); a trailing odd byte is zero-extended into one
//! final sample. An empty input still exercises sample `0` so trivial
//! inputs cover the invariants.
//!
//! Since r406 the same sample sequence is also replayed through the
//! batch form ([`encode_slice_zero_suppress`], which since r406 indexes
//! the dedicated compile-time `MULAW_ENCODE_ZERO_SUPPRESS` table) and
//! each output byte is asserted equal to the per-sample helper — so a
//! future divergence between the suppress LUT, the per-sample wrapper
//! and the slice loop trips here as well as in CI.

use libfuzzer_sys::fuzz_target;
use oxideav_g711::mulaw::{
    decode_sample, encode_sample, encode_sample_zero_suppress, encode_slice_zero_suppress,
    MULAW_ZERO_CODEWORD, MULAW_ZERO_SUPPRESS_CODEWORD,
};

/// Check the three §3.2 suppression invariants for a single i16 sample.
fn check_sample(s: i16) {
    let plain = encode_sample(s);
    let supp = encode_sample_zero_suppress(s);

    // (1) The all-zero octet must never be emitted under suppression.
    assert_ne!(
        supp, MULAW_ZERO_CODEWORD,
        "all-zero codeword leaked under suppression for input {s}"
    );

    // (2) Suppression rewrites exactly the all-zero codeword and is the
    //     identity on every other codeword.
    if plain == MULAW_ZERO_CODEWORD {
        assert_eq!(
            supp, MULAW_ZERO_SUPPRESS_CODEWORD,
            "input {s} (plain 0x00) was not rewritten to the spec 0x02 replacement"
        );
    } else {
        assert_eq!(
            supp, plain,
            "input {s} changed under suppression but its plain codeword 0x{plain:02X} \
             was not the forbidden all-zero codeword"
        );
    }
}

fuzz_target!(|data: &[u8]| {
    // (3) The decode-side ordering contract is input-independent — pin it
    //     once per execution so every run also re-asserts that the
    //     substituted codeword stays one uniform segment-7 step inward
    //     from the suppressed all-zero codeword (and that the decoder is
    //     untouched by the suppression rewrite).
    let v00 = decode_sample(MULAW_ZERO_CODEWORD) as i32; // the forbidden word
    let v01 = decode_sample(0x01) as i32; // the intervening quant level
    let v02 = decode_sample(MULAW_ZERO_SUPPRESS_CODEWORD) as i32; // the replacement
    assert!(
        v00 < v01 && v01 < v02 && v02 < 0,
        "substituted codeword ordering broken: \
         decode(0x00)={v00}, decode(0x01)={v01}, decode(0x02)={v02}"
    );
    assert_eq!(
        v01 - v00,
        v02 - v01,
        "segment-7 step is non-uniform across the substitution interval"
    );

    if data.is_empty() {
        check_sample(0);
        return;
    }

    let mut samples = Vec::with_capacity(data.len() / 2 + 1);
    for chunk in data.chunks(2) {
        let lo = chunk[0];
        let hi = chunk.get(1).copied().unwrap_or(0);
        let s = i16::from_le_bytes([lo, hi]);
        check_sample(s);
        samples.push(s);
    }

    // r406 batch-surface cross-check: the slice form must agree with
    // the per-sample helper byte-for-byte on the same sequence.
    let mut wire = vec![0u8; samples.len()];
    encode_slice_zero_suppress(&samples, &mut wire);
    for (i, &s) in samples.iter().enumerate() {
        assert_eq!(
            wire[i],
            encode_sample_zero_suppress(s),
            "encode_slice_zero_suppress diverged from the per-sample helper at index {i}"
        );
    }
});
