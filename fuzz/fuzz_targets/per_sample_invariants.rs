#![no_main]

//! Drive arbitrary fuzz-supplied i16 samples and u8 bytes through the
//! per-sample helpers `mulaw::encode_sample` / `mulaw::decode_sample`
//! / `alaw::encode_sample` / `alaw::decode_sample` and assert the
//! three spec-derived invariants:
//!
//! 1. **Projection idempotence (encode ∘ decode ∘ encode = encode),
//!    modulo the documented µ-law −0/+0 canonicalisation.** G.711's
//!    encoder is a quantiser, so re-encoding a previously decoded
//!    sample is expected to land on the same codeword:
//!    `encode(decode(encode(s))) == encode(s)`. µ-law has one
//!    documented exception — the spec carries two codewords for
//!    digital zero (`0x7F` ≡ −0 and `0xFF` ≡ +0); both decode to
//!    linear 0 but the encoder canonicalises 0 → 0xFF, so re-
//!    encoding any sample that quantises to 0 may return 0xFF
//!    instead of the original 0x7F. The fuzz target permits this
//!    one collapse explicitly and asserts the projection on every
//!    other codeword. A-law has no such collapse — its 256 codewords
//!    all carry distinct non-zero magnitudes (smallest ±8), so the
//!    strict identity holds for the entire i16 domain.
//!
//! 2. **Sign symmetry on the byte side.** For A-law, `decode(b)` and
//!    `decode(b ^ 0x80)` are exact negatives of each other across the
//!    entire 256-byte range — the spec's sign bit lives at position 7
//!    of the encoded byte and the magnitude path is identical. µ-law's
//!    table-driven decoder has the same property except at the two
//!    encoded zeros (0x7F → −0, 0xFF → +0) where both decode to 0;
//!    the fuzzer applies the law-specific predicate so this exception
//!    is handled cleanly rather than being suppressed crate-wide.
//!
//! 3. **Quantisation-step bound (the spec's worst-case error).**
//!    Every i16 sample round-trips through `encode → decode` within
//!    the per-segment step the spec defines. We use the same bound
//!    the `quantization_property` test pins: the step at segment
//!    `seg` is `1 << (seg + 4)` for A-law and `1 << (seg + 3)` for
//!    µ-law, plus a 644-LSB saturation slack for µ-law and a 512-LSB
//!    slack for A-law to account for the documented saturation-band
//!    overshoot at the far ends of the i16 range. (These limits are
//!    themselves what `quantization_property` measured exhaustively,
//!    so they double as a regression net.)
//!
//! ## Fuzz input layout
//!
//! Each input is treated as a heterogenous sequence of operations,
//! where every 3-byte chunk encodes:
//!
//! ```text
//!   chunk[0]    : opcode → 0..=3 → law × side
//!                   0 → µ-law, i16 sample side
//!                   1 → µ-law, u8 codeword side
//!                   2 → A-law, i16 sample side
//!                   3 → A-law, u8 codeword side
//!                 (>=4 wraps via `% 4`)
//!   chunk[1..3] : the operand — LE bytes interpreted as i16 (sample
//!                 side) or as `(u8, ignored)` (codeword side).
//! ```
//!
//! An input shorter than 3 bytes is treated as a single op at chunk
//! index 0. Inputs ≥ 3 bytes get multiple ops per iteration so the
//! fuzzer covers many (law, side, value) triples per execution.

use libfuzzer_sys::fuzz_target;
use oxideav_g711::{alaw, mulaw};

/// Per-segment quantisation step the spec guarantees for A-law. The
/// 4-bit mantissa lives in bits `seg+4 .. seg+8` of the magnitude;
/// the step between two adjacent mantissa values is `1 << (seg + 4)`.
/// Half of that is the max round-to-nearest error, but the encoder
/// truncates rather than rounds so we allow the full step.
fn alaw_step_bound(seg: u32) -> i32 {
    1 << (seg + 4)
}

/// Per-segment quantisation step the spec guarantees for µ-law. The
/// 4-bit mantissa lives at bit positions `seg+3 .. seg+7` so the
/// step is `1 << (seg + 3)`.
fn mulaw_step_bound(seg: u32) -> i32 {
    1 << (seg + 3)
}

/// Recover the segment from an encoded codeword.
fn segment_from_byte(byte: u8, alaw_law: bool) -> u32 {
    let unwrapped = if alaw_law {
        byte ^ 0x55
    } else {
        // µ-law inverts every bit on the wire.
        !byte
    };
    ((unwrapped >> 4) & 0x07) as u32
}

fuzz_target!(|data: &[u8]| {
    // Cover at least one op per input so trivial inputs still
    // exercise an invariant.
    let ops = data.chunks(3);
    for chunk in ops {
        let op = chunk.first().copied().unwrap_or(0) % 4;
        let operand_bytes = [
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];

        match op {
            0 => {
                // µ-law, i16 sample side.
                let s = i16::from_le_bytes(operand_bytes);
                let b = mulaw::encode_sample(s);
                let d = mulaw::decode_sample(b);
                // (1) projection idempotence, modulo the documented
                // µ-law −0 → +0 canonicalisation: byte 0x7F decodes
                // to 0 and the encoder canonicalises encode(0) =
                // 0xFF, so any sample whose codeword decodes to 0
                // may legitimately re-encode to 0xFF instead of
                // 0x7F. All other codewords must satisfy the strict
                // identity.
                let b2 = mulaw::encode_sample(d);
                let zero_collapse = d == 0 && b == 0x7F && b2 == 0xFF;
                if !zero_collapse {
                    assert_eq!(
                        b, b2,
                        "µ-law projection-idempotence violation: \
                         encode({s}) = 0x{b:02X}, decode = {d}, re-encode = 0x{b2:02X}"
                    );
                }
                // (3) per-segment quantisation bound, with saturation
                // slack at the far ends of the i16 range.
                let seg = segment_from_byte(b, /*alaw_law=*/ false);
                let bound = mulaw_step_bound(seg);
                let err = (s as i32 - d as i32).unsigned_abs() as i32;
                // The saturation band lives above ±32635 — that is
                // where the encoder clamps before the segment search
                // and the round-trip error grows beyond the in-band
                // step bound (up to the documented 644 LSB measured
                // by `quantization_property`).
                let saturation_slack = if s.unsigned_abs() as i32 > 32635 {
                    644
                } else {
                    0
                };
                assert!(
                    err <= bound + saturation_slack,
                    "µ-law step-bound violation: sample={s} \
                     codeword=0x{b:02X} decoded={d} err={err} seg={seg} \
                     bound={bound} slack={saturation_slack}"
                );
            }
            1 => {
                // µ-law, u8 codeword side.
                let b = operand_bytes[0];
                let d = mulaw::decode_sample(b);
                let d_neg = mulaw::decode_sample(b ^ 0x80);
                // (2) sign symmetry except at the two encoded zeros
                // (0x7F → −0 and 0xFF → +0 both decode to linear 0).
                let canonical_zero = (b & 0x7F) == 0x7F;
                if !canonical_zero {
                    assert_eq!(
                        d_neg as i32,
                        -(d as i32),
                        "µ-law sign-symmetry violation: \
                         decode(0x{b:02X}) = {d}, decode(0x{:02X}) = {d_neg}",
                        b ^ 0x80
                    );
                }
            }
            2 => {
                // A-law, i16 sample side.
                let s = i16::from_le_bytes(operand_bytes);
                let b = alaw::encode_sample(s);
                let d = alaw::decode_sample(b);
                let b2 = alaw::encode_sample(d);
                assert_eq!(
                    b, b2,
                    "A-law projection-idempotence violation: \
                     encode({s}) = 0x{b:02X}, decode = {d}, re-encode = 0x{b2:02X}"
                );
                let seg = segment_from_byte(b, /*alaw_law=*/ true);
                let bound = alaw_step_bound(seg);
                let err = (s as i32 - d as i32).unsigned_abs() as i32;
                let saturation_slack = if s.unsigned_abs() as i32 > 32256 {
                    512
                } else {
                    0
                };
                assert!(
                    err <= bound + saturation_slack,
                    "A-law step-bound violation: sample={s} \
                     codeword=0x{b:02X} decoded={d} err={err} seg={seg} \
                     bound={bound} slack={saturation_slack}"
                );
            }
            _ => {
                // A-law, u8 codeword side. Sign symmetry holds across
                // the entire 256-byte range — A-law has no canonical-
                // zero exception (both 0xD5 and 0x55 decode to 8 and
                // −8 respectively; the zero-magnitude segment exists
                // but is not collapsed by the encoder).
                let b = operand_bytes[0];
                let d = alaw::decode_sample(b);
                let d_neg = alaw::decode_sample(b ^ 0x80);
                assert_eq!(
                    d_neg as i32,
                    -(d as i32),
                    "A-law sign-symmetry violation: \
                     decode(0x{b:02X}) = {d}, decode(0x{:02X}) = {d_neg}",
                    b ^ 0x80
                );
            }
        }
    }
});
