//! Bit-exact verification against the canonical ITU-T G.711 reference
//! formulas. G.711 is a deterministic table-lookup codec: every valid
//! byte has exactly one decoded amplitude, and every S16 sample has
//! exactly one encoded byte. This test computes both tables using the
//! reference formulas from the ITU-T G.711 specification and compares
//! them to our implementation for every possible input.

use oxideav_g711::{alaw, mulaw};

// --- Reference µ-law decode (ITU-T G.711 §3.2.2) -----------------------
//
// Wire byte `b` is first complemented (`inv = !b`). Then:
//   sign = inv & 0x80
//   exp  = (inv >> 4) & 0x07
//   mant = inv & 0x0F
//   mag  = (((mant << 3) + 0x84) << exp) - 0x84
//   out  = sign != 0 ? -mag : mag
fn mulaw_ref_decode(b: u8) -> i16 {
    let inv = !b;
    let sign = inv & 0x80;
    let exp = ((inv >> 4) & 0x07) as i32;
    let mant = (inv & 0x0F) as i32;
    let mag = (((mant << 3) + 0x84) << exp) - 0x84;
    let v = if sign != 0 { -mag } else { mag };
    v as i16
}

// --- Reference µ-law encode (mirror of decode, arithmetic form) --------
//
// This follows the canonical Sun Microsystems reference (public-domain)
// used throughout the telecom industry and cited by every interoperable
// G.711 implementation.
fn mulaw_ref_encode(sample: i16) -> u8 {
    const BIAS: i32 = 0x84;
    const CLIP: i32 = 32635;

    let mut pcm = sample as i32;
    let sign = if pcm < 0 {
        pcm = -pcm;
        0x80u8
    } else {
        0
    };
    if pcm > CLIP {
        pcm = CLIP;
    }
    pcm += BIAS;

    // Find the top set bit in bits 7..14. Segment = (top_bit - 7).
    let mut seg: u8 = 0;
    let mut test: i32 = 0x4000;
    for s in (0..=7u8).rev() {
        if pcm & test != 0 {
            seg = s;
            break;
        }
        test >>= 1;
    }
    let mant = ((pcm >> (seg + 3)) & 0x0F) as u8;
    !(sign | (seg << 4) | mant)
}

// --- Reference A-law decode (ITU-T G.711 §2) ---------------------------
//
// Wire byte `b` has alternate bits toggled (XOR 0x55) before the fields
// are extracted. Sign bit is "1 = positive" for A-law (inverse of µ-law
// convention). Magnitude formula differs between segment 0 and segments
// 1..=7:
//   seg == 0: mag = (mant << 4) + 8
//   seg >  0: mag = ((mant << 4) + 0x108) << (seg - 1)
fn alaw_ref_decode(b: u8) -> i16 {
    let inv = b ^ 0x55;
    let sign = inv & 0x80;
    let exp = ((inv >> 4) & 0x07) as i32;
    let mant = (inv & 0x0F) as i32;
    let mag = if exp == 0 {
        (mant << 4) + 8
    } else {
        ((mant << 4) + 0x108) << (exp - 1)
    };
    let v = if sign != 0 { mag } else { -mag };
    v as i16
}

// --- Reference A-law encode --------------------------------------------
fn alaw_ref_encode(sample: i16) -> u8 {
    const CLIP: i32 = 32256;
    let mut pcm = sample as i32;
    let sign: u8 = if pcm < 0 {
        pcm = -pcm;
        0
    } else {
        0x80
    };
    if pcm > CLIP {
        pcm = CLIP;
    }

    let (seg, mant): (u8, u8) = if pcm < 256 {
        (0, ((pcm >> 4) & 0x0F) as u8)
    } else {
        let mut s: u8 = 1;
        let mut threshold: i32 = 512;
        while s < 7 && pcm >= threshold {
            s += 1;
            threshold <<= 1;
        }
        let shift = (s + 3) as u32;
        (s, ((pcm >> shift) & 0x0F) as u8)
    };
    (sign | (seg << 4) | mant) ^ 0x55
}

// --- Tests -------------------------------------------------------------

#[test]
fn mulaw_decode_table_is_bit_exact_for_all_256_bytes() {
    for b in 0u8..=255 {
        let got = mulaw::decode_sample(b);
        let want = mulaw_ref_decode(b);
        assert_eq!(
            got, want,
            "µ-law decode mismatch at byte {:#04x}: got {got}, want {want}",
            b
        );
    }
}

#[test]
fn alaw_decode_table_is_bit_exact_for_all_256_bytes() {
    for b in 0u8..=255 {
        let got = alaw::decode_sample(b);
        let want = alaw_ref_decode(b);
        assert_eq!(
            got, want,
            "A-law decode mismatch at byte {:#04x}: got {got}, want {want}",
            b
        );
    }
}

#[test]
fn mulaw_encode_is_bit_exact_for_all_65536_samples() {
    for x in i16::MIN..=i16::MAX {
        let got = mulaw::encode_sample(x);
        let want = mulaw_ref_encode(x);
        assert_eq!(
            got, want,
            "µ-law encode mismatch at sample {x}: got {got:#04x}, want {want:#04x}"
        );
    }
}

#[test]
fn alaw_encode_is_bit_exact_for_all_65536_samples() {
    for x in i16::MIN..=i16::MAX {
        let got = alaw::encode_sample(x);
        let want = alaw_ref_encode(x);
        assert_eq!(
            got, want,
            "A-law encode mismatch at sample {x}: got {got:#04x}, want {want:#04x}"
        );
    }
}

/// Encode then decode every possible S16 sample. Re-encoding the
/// decoded sample must yield a byte that decodes to the same linear
/// amplitude. µ-law has two codes for zero (0x7F "negative zero",
/// 0xFF "positive zero"): the encoder canonicalises to 0xFF, so a
/// strict byte-equality check would fail only for samples that land
/// on 0x7F and re-encode to 0xFF. We check equivalence after one more
/// decode, which is the true projection property.
#[test]
fn mulaw_encode_decode_encode_is_idempotent() {
    for x in i16::MIN..=i16::MAX {
        let b1 = mulaw::encode_sample(x);
        let s1 = mulaw::decode_sample(b1);
        let b2 = mulaw::encode_sample(s1);
        let s2 = mulaw::decode_sample(b2);
        assert_eq!(
            s1, s2,
            "µ-law not idempotent at sample {x} (b1={b1:#04x}, s1={s1}, b2={b2:#04x}, s2={s2})"
        );
    }
}

#[test]
fn alaw_encode_decode_encode_is_idempotent() {
    for x in i16::MIN..=i16::MAX {
        let b1 = alaw::encode_sample(x);
        let s1 = alaw::decode_sample(b1);
        let b2 = alaw::encode_sample(s1);
        assert_eq!(
            b1, b2,
            "A-law not idempotent at sample {x} (b1={b1:#04x}, s1={s1}, b2={b2:#04x})"
        );
    }
}

/// For every byte, decode then encode must reproduce the original
/// byte — modulo µ-law's dual zero representation. Bytes 0x7F and 0xFF
/// both decode to linear 0; the encoder canonicalises to 0xFF, so the
/// only permitted difference is a 0x7F input getting re-encoded as 0xFF.
/// Every other byte must round-trip exactly.
#[test]
fn mulaw_decode_encode_is_identity_for_every_byte() {
    for b in 0u8..=255 {
        let s = mulaw::decode_sample(b);
        let b2 = mulaw::encode_sample(s);
        let allowed = b == b2 || (b == 0x7F && b2 == 0xFF);
        assert!(
            allowed,
            "µ-law decode/encode not identity at byte {b:#04x}: got {b2:#04x}"
        );
    }
}

#[test]
fn alaw_decode_encode_is_identity_for_every_byte() {
    for b in 0u8..=255 {
        let s = alaw::decode_sample(b);
        let b2 = alaw::encode_sample(s);
        assert_eq!(
            b, b2,
            "A-law decode/encode not identity at byte {b:#04x}: got {b2:#04x}"
        );
    }
}
