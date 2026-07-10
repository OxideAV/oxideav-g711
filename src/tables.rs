//! ITU-T G.711 conversion tables.
//!
//! Both laws have a 256-entry decode table — generated at compile time from
//! the bit-layout definitions so the file is self-checking. Encode also
//! ships a compile-time 65536-entry direct S16 → byte LUT for each law
//! ([`MULAW_ENCODE`] / [`ALAW_ENCODE`]); the entries are produced by
//! invoking the arithmetic encoders ([`mulaw_encode_arith`] /
//! [`alaw_encode_arith`]) at every `i16` value in a `const fn` loop, so the
//! LUT is bit-exact-by-construction relative to the spec formulas — there
//! is no second source of truth. Each LUT costs **64 KiB** of static data
//! (`[u8; 65536]`). A third 64 KiB encode table,
//! [`MULAW_ENCODE_ZERO_SUPPRESS`], folds the §3.2 all-zero-suppression
//! rewrite into the µ-law entries at compile time.
//!
//! Reference: ITU-T Recommendation G.711 (11/88), "Pulse code modulation
//! (PCM) of voice frequencies", §2 (A-law) and §3 (µ-law).

// -------------- mu-law --------------
//
// ITU-T G.711 §3. The encoded byte encodes (sign, segment, mantissa) as:
//
//   bit: 7 6 5 4 3 2 1 0
//        S E E E M M M M
//
// On the wire bits are complemented (Table 2a); the decode math here
// operates on the *already-un-complemented* value (the wire byte XORed with
// 0xFF). Our public API (`decode_sample(byte)`) handles the XOR internally.
//
// Linear magnitude for each segment is `((M << 1) | 1) << (E + 2)` minus
// the bias (33).

/// µ-law bias added/removed during segment math. ITU-T G.711 §3.2.
pub const MULAW_BIAS: i32 = 0x84; // 132

/// Decoded linear amplitude for µ-law byte `b`. Range: ±32124.
///
/// Implements the canonical G.711 §3.2.2 formula:
/// `mag = (((mantissa << 3) + BIAS) << exponent) - BIAS`.
pub const fn mulaw_decode(b: u8) -> i16 {
    // Un-complement the wire byte.
    let inv = !b;
    let sign = inv & 0x80;
    let exp = ((inv >> 4) & 0x07) as u32;
    let mant = (inv & 0x0F) as i32;
    let mag = (((mant << 3) + MULAW_BIAS) << exp) - MULAW_BIAS;
    if sign == 0 {
        mag as i16
    } else {
        (-mag) as i16
    }
}

/// Compile-time 256-entry decode LUT. Indexed by the wire byte directly.
pub const MULAW_DECODE: [i16; 256] = {
    let mut t = [0i16; 256];
    let mut i = 0;
    while i < 256 {
        t[i] = mulaw_decode(i as u8);
        i += 1;
    }
    t
};

/// Compile-time 256-entry **little-endian byte-pair** decode LUT. Entry
/// `i` is `MULAW_DECODE[i].to_le_bytes()`, so it is byte-identical to the
/// `[i16; 256]` table above — there is no second source of truth, just a
/// pre-serialized view of the same values.
///
/// The trait-surface decode hot loop ([`crate::mulaw::UlawDecoder`])
/// emits interleaved S16 PCM as raw little-endian bytes. Indexing this
/// table lets each step be a single 2-byte `copy_from_slice` from
/// `.rodata` instead of an `[i16; 256]` load followed by a per-iter
/// `i16::to_le_bytes()` recomputation and two scalar stores.
pub const MULAW_DECODE_LE: [[u8; 2]; 256] = {
    let mut t = [[0u8; 2]; 256];
    let mut i = 0;
    while i < 256 {
        t[i] = MULAW_DECODE[i].to_le_bytes();
        i += 1;
    }
    t
};

// -------------- A-law --------------
//
// ITU-T G.711 §2. Encoded byte `abcd_efgh` is
//
//   bit:  7 6 5 4 3 2 1 0
//         S E E E M M M M
//
// with alternate bits inverted (XOR 0x55) on the wire. Linear magnitude:
// - segment 0 (E=0): magnitude = (M << 4) + 8
// - segment n > 0:   magnitude = ((M << 4) + 0x108) << (n - 1)
//
// (Equivalent scaled 13-bit formulation; we return S16 left-shifted by 3 so
// the result is comparable to µ-law — i.e. nominal full-scale is ~±32256.)

/// A-law mask XOR'd onto every byte on the wire (bits 0,2,4,6 inverted).
pub const ALAW_XOR: u8 = 0x55;

/// Decoded linear amplitude for A-law byte `b` (S16 range).
///
/// ITU-T G.711 A-law sign convention: **bit 7 set = positive**. The wire
/// byte first has alternate bits restored (XOR with 0x55), then:
///
/// - segment 0 (exp=0): magnitude = `(mant << 4) + 8`
/// - segments 1..=7:    magnitude = `((mant << 4) + 0x108) << (exp - 1)`
///
/// The result sits directly in the S16 range (full-scale ±32256 at
/// exp=7, mant=15). No further scaling is applied.
pub const fn alaw_decode(b: u8) -> i16 {
    let inv = b ^ ALAW_XOR;
    let sign = inv & 0x80;
    let exp = ((inv >> 4) & 0x07) as u32;
    let mant = (inv & 0x0F) as i32;
    let mag = if exp == 0 {
        (mant << 4) + 8
    } else {
        ((mant << 4) + 0x108) << (exp - 1)
    };
    // A-law sign bit: 1 → positive, 0 → negative.
    if sign != 0 {
        mag as i16
    } else {
        (-mag) as i16
    }
}

/// Compile-time 256-entry A-law decode LUT.
pub const ALAW_DECODE: [i16; 256] = {
    let mut t = [0i16; 256];
    let mut i = 0;
    while i < 256 {
        t[i] = alaw_decode(i as u8);
        i += 1;
    }
    t
};

/// Compile-time 256-entry **little-endian byte-pair** A-law decode LUT.
/// See [`MULAW_DECODE_LE`] — entry `i` is `ALAW_DECODE[i].to_le_bytes()`,
/// byte-identical to the `[i16; 256]` table and used by the trait-surface
/// decode hot loop for a single-`copy_from_slice` store per sample.
pub const ALAW_DECODE_LE: [[u8; 2]; 256] = {
    let mut t = [[0u8; 2]; 256];
    let mut i = 0;
    while i < 256 {
        t[i] = ALAW_DECODE[i].to_le_bytes();
        i += 1;
    }
    t
};

// -------------- compile-time encode LUTs --------------
//
// Both encode hot paths can be expressed as a direct S16 → byte LUT
// (`[u8; 65536]` = 64 KiB per law). The arithmetic encoders below run at
// compile time inside a `while` loop, so every entry is computed from the
// spec-derived formulas in §2 / §3. The runtime encoder simply indexes
// `LUT[(sample as u16) as usize]`, replacing the per-sample bias add +
// segment-search loop + mantissa shift + on-wire inversion with one load.
//
// The arithmetic helpers stay `pub` (not `pub(crate)`) so:
//   - external callers who really do want one-sample-without-a-64-KiB-LUT
//     can still reach the formula directly (e.g. a fuzz target asserting
//     LUT == arith for every byte);
//   - the LUT itself remains self-checking — the inner loop is the spec.

/// µ-law arithmetic encode, ITU-T G.711 §3.2. Identical math to
/// [`crate::mulaw::encode_sample`] but expressed as a `const fn` so it
/// can populate [`MULAW_ENCODE`] at compile time. Public so the in-tree
/// test suite can assert `MULAW_ENCODE[s] == mulaw_encode_arith(s)` for
/// every S16 sample without a second source of truth.
pub const fn mulaw_encode_arith(sample: i16) -> u8 {
    let mut mag: i32 = sample as i32;
    let sign_bit: u8 = if mag < 0 { 0x80 } else { 0 };
    if mag < 0 {
        mag = -mag;
    }
    if mag > 32635 {
        mag = 32635;
    }
    mag += MULAW_BIAS;

    let mut seg: u32 = 7;
    let mut mask: i32 = 0x4000;
    while seg > 0 && (mag & mask) == 0 {
        seg -= 1;
        mask >>= 1;
    }

    let mantissa = ((mag >> (seg + 3)) & 0x0F) as u8;
    let byte = sign_bit | ((seg as u8) << 4) | mantissa;
    !byte
}

/// A-law arithmetic encode, ITU-T G.711 §2. `const fn` counterpart of
/// [`crate::alaw::encode_sample`]; same role as [`mulaw_encode_arith`].
pub const fn alaw_encode_arith(sample: i16) -> u8 {
    let mut mag: i32 = sample as i32;
    let sign_bit: u8 = if mag < 0 { 0x00 } else { 0x80 };
    if mag < 0 {
        mag = -mag;
    }
    if mag > 32256 {
        mag = 32256;
    }

    let (seg, mant): (u32, u32) = if mag < 256 {
        (0, (mag >> 4) as u32 & 0x0F)
    } else {
        let mut seg = 1u32;
        let mut threshold: i32 = 512;
        while seg < 7 && mag >= threshold {
            seg += 1;
            threshold <<= 1;
        }
        let shift = seg + 3;
        let m = ((mag >> shift) & 0x0F) as u32;
        (seg, m)
    };

    let byte = sign_bit | ((seg as u8) << 4) | (mant as u8);
    byte ^ ALAW_XOR
}

/// Compile-time 65536-entry µ-law encode LUT. Indexed by the input S16
/// reinterpreted as a `u16` so callers can write
/// `MULAW_ENCODE[sample as u16 as usize]`. Every entry is the value
/// [`mulaw_encode_arith`] would return for that sample, by construction
/// — the arithmetic formula is the single source of truth.
///
/// Declared `static` (not `const`) so the 64 KiB table lives in
/// `.rodata` and is shared across every call site; with `const` clippy
/// (1.95+, `large_const_arrays`) warns that each use copies the array,
/// which we definitely don't want for a 64 KiB hot-path table.
pub static MULAW_ENCODE: [u8; 65536] = {
    let mut t = [0u8; 65536];
    let mut i: u32 = 0;
    while i < 65536 {
        // Reinterpret the index as i16: indices 0..=0x7FFF stay positive,
        // 0x8000..=0xFFFF wrap to negative — the same mapping
        // `sample as u16 as usize` produces in the runtime lookup.
        t[i as usize] = mulaw_encode_arith(i as i16);
        i += 1;
    }
    t
};

/// Compile-time 65536-entry A-law encode LUT. See [`MULAW_ENCODE`].
pub static ALAW_ENCODE: [u8; 65536] = {
    let mut t = [0u8; 65536];
    let mut i: u32 = 0;
    while i < 65536 {
        t[i as usize] = alaw_encode_arith(i as i16);
        i += 1;
    }
    t
};

/// Compile-time 65536-entry µ-law **all-zero-suppressed** encode LUT
/// (ITU-T G.711 §3.2). Entry `i` is [`mulaw_encode_arith`]`(i as i16)`
/// with the single all-zero codeword
/// ([`crate::mulaw::MULAW_ZERO_CODEWORD`]) rewritten to the spec
/// replacement `00000010`
/// ([`crate::mulaw::MULAW_ZERO_SUPPRESS_CODEWORD`]) — the same
/// definitional composition [`crate::mulaw::encode_sample_zero_suppress`]
/// exposes, folded into the table at compile time so the suppressed
/// wire costs the same single load as the plain law (r406: the
/// branch-per-store form measured ~24% slower than [`MULAW_ENCODE`]
/// on the bulk slice path; this table closes that gap). Same
/// single-source-of-truth argument as [`MULAW_ENCODE`]: the arithmetic
/// formula populates every entry, and a CI test pins the rewrite
/// against the plain table on all 65 536 entries.
pub static MULAW_ENCODE_ZERO_SUPPRESS: [u8; 65536] = {
    let mut t = [0u8; 65536];
    let mut i: u32 = 0;
    while i < 65536 {
        let b = mulaw_encode_arith(i as i16);
        t[i as usize] = if b == crate::mulaw::MULAW_ZERO_CODEWORD {
            crate::mulaw::MULAW_ZERO_SUPPRESS_CODEWORD
        } else {
            b
        };
        i += 1;
    }
    t
};

#[cfg(test)]
mod tests {
    use super::*;

    /// Canonical µ-law endpoint codewords (ITU-T G.711 §3).
    ///
    /// - `0xFF` is the digital-zero codeword (un-complemented to all bits
    ///   clear: sign=0, exp=0, mant=0) → decoded value `0`.
    /// - `0x7F` is digital zero with the sign bit set (negative zero) →
    ///   also `0`.
    /// - `0x00` is the most-negative wire value and `0x80` the most-positive;
    ///   they are exact negatives of each other.
    ///
    /// The extreme magnitude this crate emits is `(((0x0F << 3) + 0x84)
    /// << 7) - 0x84 = 30844` in the 16-bit left-justified convention used
    /// throughout (the spec's own 14-bit magnitude convention is this value
    /// scaled down — the familiar "±8031" figure is the 13-bit form).
    #[test]
    fn mulaw_endpoints() {
        assert_eq!(MULAW_DECODE[0xFF], 0);
        assert_eq!(MULAW_DECODE[0x7F], 0);
        // exp=7, mant=0x0F: mag = (((0x0F << 3) + 0x84) << 7) - 0x84
        //   = ((120 + 132) << 7) - 132 = (252 << 7) - 132 = 32256 - 132
        //   = 32124. `0x80` un-complements to sign=0 (positive), `0x00`
        //   to sign=1 (negative), so they are exact negatives.
        assert_eq!(MULAW_DECODE[0x80], 32124);
        assert_eq!(MULAW_DECODE[0x00], -32124);
        assert_eq!(MULAW_DECODE[0x00], -(MULAW_DECODE[0x80] as i32) as i16);
    }

    #[test]
    fn mulaw_symmetry() {
        // Byte b and byte b^0x80 should be exact negatives (except the
        // "positive zero" / "negative zero" case which both map to 0).
        for b in 0u8..=255 {
            let a = MULAW_DECODE[b as usize] as i32;
            let c = MULAW_DECODE[(b ^ 0x80) as usize] as i32;
            assert_eq!(a, -c, "mu-law symmetry failed for byte {:#x}", b);
        }
    }

    #[test]
    fn alaw_symmetry() {
        for b in 0u8..=255 {
            let a = ALAW_DECODE[b as usize] as i32;
            let c = ALAW_DECODE[(b ^ 0x80) as usize] as i32;
            assert_eq!(a, -c, "A-law symmetry failed for byte {:#x}", b);
        }
    }

    #[test]
    fn decode_le_matches_i16_lut() {
        // The byte-pair LE tables must be a pure serialized view of the
        // i16 decode tables — same single source of truth, no drift.
        for b in 0u8..=255 {
            assert_eq!(
                MULAW_DECODE_LE[b as usize],
                MULAW_DECODE[b as usize].to_le_bytes(),
                "mu-law LE byte-pair LUT diverged at {b:#x}"
            );
            assert_eq!(
                ALAW_DECODE_LE[b as usize],
                ALAW_DECODE[b as usize].to_le_bytes(),
                "A-law LE byte-pair LUT diverged at {b:#x}"
            );
        }
    }

    #[test]
    fn zero_suppress_lut_is_the_rewritten_plain_lut() {
        // Definitional cross-check over the entire domain: the §3.2
        // zero-suppress table must equal the plain encode table with
        // exactly the all-zero codeword rewritten to the spec
        // replacement, and nothing else changed.
        for i in 0..65536usize {
            let plain = MULAW_ENCODE[i];
            let expected = if plain == crate::mulaw::MULAW_ZERO_CODEWORD {
                crate::mulaw::MULAW_ZERO_SUPPRESS_CODEWORD
            } else {
                plain
            };
            assert_eq!(
                MULAW_ENCODE_ZERO_SUPPRESS[i], expected,
                "zero-suppress LUT diverged from the rewritten plain LUT at index {i}"
            );
        }
    }

    #[test]
    fn alaw_zero_code() {
        // A-law: bytes 0x55 / 0xD5 (after XOR: 0x00 / 0x80) are the
        // smallest-magnitude codes. With A-law's "sign bit 1 ⇒ positive"
        // convention, 0xD5 is +8 and 0x55 is −8 in the 13-bit domain that
        // we carry directly into S16 (no further scaling).
        assert_eq!(ALAW_DECODE[0xD5], 8);
        assert_eq!(ALAW_DECODE[0x55], -8);
    }
}
