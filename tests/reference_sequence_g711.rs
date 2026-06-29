//! ITU-T G.711 §5 reference-sequence conformance.
//!
//! G.711 §5 ("Relationship between the encoding laws and the audio
//! level") pins a normative 8-codeword periodic sequence for each law —
//! Table 5/G.711 (A-law) and Table 6/G.711 (µ-law) — and states:
//!
//! > A sine-wave signal of 1 kHz at a nominal level of 0 dBm0 should be
//! > present at any voice frequency output of the PCM multiplex when the
//! > periodic sequence of character signals of Table 5/G.711 for the
//! > A-law and of Table 6/G.711 for the µ-law is applied to the decoder
//! > input.
//!
//! Eight samples per period at the §2 nominal 8 kHz sampling rate is
//! exactly a 1 kHz fundamental. This test fixes the two reference
//! sequences as their on-wire codewords, decodes them through both the
//! direct LUT and the trait surface, and asserts the decoded PCM has the
//! structure the spec requires of a 1 kHz sine: half-wave antisymmetry,
//! even symmetry within each half-cycle, and the monotone segment
//! ordering of a quarter-sine. The exact decoder-output values are pinned
//! so any future table edit that perturbs the conformance waveform fails
//! here.
//!
//! Source read: `docs/audio/g711/T-REC-G.711-198811-I.pdf` §4
//! (serial transmission: "bit No. 1 (polarity bit) is transmitted first
//! and No. 8 (the least significant bit) last") and §5 + Tables 5/6.
//! Nothing else was consulted.

use oxideav_core::{
    CodecId, CodecParameters, Frame, Packet, RuntimeContext, SampleFormat, TimeBase,
};

/// Pack a Table 5/6 row — listed in the spec as bits 1..8, bit 1 the
/// polarity (most-significant, transmitted first per §4), bit 8 the LSB —
/// into the wire byte. This mirrors the §4 serial transmission order
/// rather than hard-coding a hex literal, so the test also documents the
/// bit-1-is-MSB convention.
fn row_to_byte(bits: [u8; 8]) -> u8 {
    let mut v = 0u8;
    for &b in &bits {
        assert!(b <= 1, "spec rows are binary digits");
        v = (v << 1) | b;
    }
    v
}

/// Table 5/G.711 — A-law reference sequence (bits 1..8 per row).
const ALAW_TABLE5: [[u8; 8]; 8] = [
    [0, 0, 1, 1, 0, 1, 0, 0],
    [0, 0, 1, 0, 0, 0, 0, 1],
    [0, 0, 1, 0, 0, 0, 0, 1],
    [0, 0, 1, 1, 0, 1, 0, 0],
    [1, 0, 1, 1, 0, 1, 0, 0],
    [1, 0, 1, 0, 0, 0, 0, 1],
    [1, 0, 1, 0, 0, 0, 0, 1],
    [1, 0, 1, 1, 0, 1, 0, 0],
];

/// Table 6/G.711 — µ-law reference sequence (bits 1..8 per row).
const MULAW_TABLE6: [[u8; 8]; 8] = [
    [0, 0, 0, 1, 1, 1, 1, 0],
    [0, 0, 0, 0, 1, 0, 1, 1],
    [0, 0, 0, 0, 1, 0, 1, 1],
    [0, 0, 0, 1, 1, 1, 1, 0],
    [1, 0, 0, 1, 1, 1, 1, 0],
    [1, 0, 0, 0, 1, 0, 1, 1],
    [1, 0, 0, 0, 1, 0, 1, 1],
    [1, 0, 0, 1, 1, 1, 1, 0],
];

fn pack(rows: &[[u8; 8]; 8]) -> [u8; 8] {
    let mut out = [0u8; 8];
    for (i, r) in rows.iter().enumerate() {
        out[i] = row_to_byte(*r);
    }
    out
}

/// Assert `decoded` is the PCM shape §5 demands of a 1 kHz sine sampled
/// at 8 kHz: 8 samples per period, half-wave antisymmetric, even within
/// each half, with the quarter-sine being monotone (the second sample of
/// a half is larger in magnitude than the first — sin 67.5° > sin 22.5°).
fn assert_one_khz_sine_shape(decoded: &[i16]) {
    assert_eq!(decoded.len(), 8, "one period is 8 samples at 8 kHz");

    // Half-wave antisymmetry: y[i] == -y[i+4] for a pure sinusoid.
    for i in 0..4 {
        assert_eq!(
            decoded[i],
            -decoded[i + 4],
            "half-wave antisymmetry at index {i}: {} vs {}",
            decoded[i],
            decoded[i + 4]
        );
    }

    // Even symmetry inside each half cycle about its centre: the four
    // negative-going samples are {a, b, b, a}.
    assert_eq!(decoded[0], decoded[3], "even symmetry within first half");
    assert_eq!(decoded[1], decoded[2], "even symmetry within first half");

    // Monotone quarter-sine: |y[1]| (≈sin 67.5°) > |y[0]| (≈sin 22.5°).
    assert!(
        decoded[1].unsigned_abs() > decoded[0].unsigned_abs(),
        "quarter-sine must be monotone: |{}| !> |{}|",
        decoded[1],
        decoded[0]
    );

    // The negative-going half really is negative (sign convention check).
    assert!(decoded[0] < 0 && decoded[1] < 0, "first half is negative");
    assert!(decoded[4] > 0 && decoded[5] > 0, "second half is positive");
}

/// Decode `payload` through the registry trait surface and return the S16
/// samples.
fn decode_trait(codec_id: &str, payload: &[u8]) -> Vec<i16> {
    let mut ctx = RuntimeContext::new();
    oxideav_g711::register(&mut ctx);
    let mut params = CodecParameters::audio(CodecId::new(codec_id));
    params.sample_rate = Some(8_000);
    params.channels = Some(1);
    params.sample_format = Some(SampleFormat::S16);

    let mut dec = ctx.codecs.first_decoder(&params).expect("make_decoder");
    dec.send_packet(&Packet::new(0, TimeBase::new(1, 8_000), payload.to_vec()))
        .unwrap();
    let Frame::Audio(af) = dec.receive_frame().unwrap() else {
        panic!("expected audio frame");
    };
    af.data[0]
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect()
}

#[test]
fn alaw_table5_reference_sequence_decodes_to_one_khz_sine() {
    let codewords = pack(&ALAW_TABLE5);

    // Direct-LUT decode of each codeword.
    let direct: Vec<i16> = codewords
        .iter()
        .map(|&b| oxideav_g711::alaw::decode_sample(b))
        .collect();

    assert_one_khz_sine_shape(&direct);

    // Normative decoder-output values for the A-law conformance sequence.
    // These are column-8 "value at decoder output" levels of Table 1a/1b
    // read out by the Table 5 codeword pattern.
    assert_eq!(
        direct,
        [-8960, -20992, -20992, -8960, 8960, 20992, 20992, 8960]
    );

    // Trait surface must agree byte-for-byte with the direct path.
    let via_trait = decode_trait("pcm_alaw", &codewords);
    assert_eq!(via_trait, direct, "trait surface vs direct LUT");
}

#[test]
fn mulaw_table6_reference_sequence_decodes_to_one_khz_sine() {
    let codewords = pack(&MULAW_TABLE6);

    let direct: Vec<i16> = codewords
        .iter()
        .map(|&b| oxideav_g711::mulaw::decode_sample(b))
        .collect();

    assert_one_khz_sine_shape(&direct);

    // Normative decoder-output values for the µ-law conformance sequence.
    assert_eq!(
        direct,
        [-8828, -20860, -20860, -8828, 8828, 20860, 20860, 8828]
    );

    let via_trait = decode_trait("pcm_mulaw", &codewords);
    assert_eq!(via_trait, direct, "trait surface vs direct LUT");
}

/// §5: the reference sequence is *periodic* — repeating it must produce a
/// steady 1 kHz tone with no discontinuity at the period boundary. Decode
/// three concatenated periods and confirm the waveform is exactly
/// period-8 (sample n equals sample n-8 throughout), proving the codec is
/// stateless across the period boundary as §1.2/§3.2 require.
#[test]
fn reference_sequences_tile_into_a_steady_periodic_tone() {
    for (codec_id, rows) in [("pcm_alaw", &ALAW_TABLE5), ("pcm_mulaw", &MULAW_TABLE6)] {
        let one_period = pack(rows);
        let three: Vec<u8> = one_period
            .iter()
            .chain(one_period.iter())
            .chain(one_period.iter())
            .copied()
            .collect();
        let decoded = decode_trait(codec_id, &three);
        assert_eq!(decoded.len(), 24);
        for n in 8..decoded.len() {
            assert_eq!(
                decoded[n],
                decoded[n - 8],
                "{codec_id}: period-8 continuity broken at sample {n}"
            );
        }
    }
}

/// The §3.4 conversion direction ("countries adopting the µ-law do any
/// necessary conversion") means a gateway commonly decodes a µ-law tone
/// and re-encodes it as A-law. The two reference tones are the same 1 kHz
/// 0 dBm0 stimulus expressed in each law, so their decoded waveforms must
/// agree in *shape* even though the quantised amplitudes differ slightly
/// (A-law T_max = 3.14 dBm0, µ-law T_max = 3.17 dBm0 per §5). Pin that the
/// two tones are sample-by-sample close (within one A-law top-segment
/// step) while not bit-identical.
#[test]
fn alaw_and_mulaw_reference_tones_are_the_same_signal() {
    let alaw: Vec<i16> = pack(&ALAW_TABLE5)
        .iter()
        .map(|&b| oxideav_g711::alaw::decode_sample(b))
        .collect();
    let mulaw: Vec<i16> = pack(&MULAW_TABLE6)
        .iter()
        .map(|&b| oxideav_g711::mulaw::decode_sample(b))
        .collect();

    // Same shape contract for both.
    assert_one_khz_sine_shape(&alaw);
    assert_one_khz_sine_shape(&mulaw);

    let mut any_difference = false;
    for (a, u) in alaw.iter().zip(mulaw.iter()) {
        let diff = (i32::from(*a) - i32::from(*u)).unsigned_abs();
        // 256 = the A-law top-segment (segment 7) step size. The two
        // representations of the same 0 dBm0 tone never disagree by more
        // than one such step.
        assert!(
            diff <= 256,
            "A-law {a} vs µ-law {u} differ by {diff} (> one top-segment step)"
        );
        if diff != 0 {
            any_difference = true;
        }
    }
    assert!(
        any_difference,
        "the two laws quantise the tone differently; they must not be bit-identical"
    );
}

/// §5 level relationship — the two reference tones express the same
/// 1 kHz / 0 dBm0 stimulus, but §5 states different theoretical load
/// capacities: **A-law T_max = +3.14 dBm0, µ-law T_max = +3.17 dBm0**.
/// T_max is the level of a sine that just reaches full scale, so a higher
/// T_max means more headroom above the 0 dBm0 reference. The A-law tone
/// therefore sits *closer* to its full-scale peak than the µ-law tone
/// (3.14 dB of headroom vs 3.17 dB). We pin this directional relationship
/// — derivable from the §5 numbers alone — by comparing each tone's
/// fundamental amplitude to its law's peak reconstruction level.
///
/// The fundamental amplitude is recovered from the second sample of each
/// quarter-period: at 8 kHz the §5 sequence samples the sine at 22.5°,
/// 67.5°, …, so `sample[1] = A0 · sin 67.5°`. We do not assert the exact
/// +3.14 / +3.17 figures (those depend on the dBm0 / digital-milliwatt
/// reference convention, which §5 does not spell out enough to derive
/// cleanly); we assert the *ordering* and that both headrooms are a small
/// positive few-tenths-of-a-dB, consistent with the stated T_max values.
#[test]
fn reference_tone_headroom_matches_section_5_tmax_ordering() {
    let alaw: Vec<i16> = pack(&ALAW_TABLE5)
        .iter()
        .map(|&b| oxideav_g711::alaw::decode_sample(b))
        .collect();
    let mulaw: Vec<i16> = pack(&MULAW_TABLE6)
        .iter()
        .map(|&b| oxideav_g711::mulaw::decode_sample(b))
        .collect();

    let sin_67_5 = 67.5f64.to_radians().sin();
    // sample[1] is the 67.5° sample of the (negative-going first) half;
    // use its magnitude.
    let a_amp = f64::from(alaw[1].unsigned_abs()) / sin_67_5;
    let u_amp = f64::from(mulaw[1].unsigned_abs()) / sin_67_5;

    // Peak reconstruction (full scale) per law.
    const A_PEAK: f64 = 32256.0;
    const U_PEAK: f64 = 32124.0;
    let a_headroom_db = 20.0 * (A_PEAK / a_amp).log10();
    let u_headroom_db = 20.0 * (U_PEAK / u_amp).log10();

    // Both tones sit a small positive headroom below full scale (a few
    // tenths of a dB, in the neighbourhood of the §5 +3.1 dBm0 region).
    assert!(
        (2.5..3.5).contains(&a_headroom_db),
        "A-law headroom {a_headroom_db:.3} dB out of the §5 T_max neighbourhood"
    );
    assert!(
        (2.5..3.5).contains(&u_headroom_db),
        "µ-law headroom {u_headroom_db:.3} dB out of the §5 T_max neighbourhood"
    );
    // §5 ordering: A-law T_max (3.14) < µ-law T_max (3.17) ⇒ the A-law tone
    // has less headroom (sits closer to full scale) than the µ-law tone.
    assert!(
        a_headroom_db < u_headroom_db,
        "§5 T_max ordering violated: A-law headroom {a_headroom_db:.3} dB \
         must be < µ-law {u_headroom_db:.3} dB (A T_max 3.14 < µ T_max 3.17)"
    );
}

/// Sanity on the §4 transmission convention encoded in `row_to_byte`:
/// the A-law silence/idle codeword in Table 1a (positive smallest
/// magnitude, decoder output +8) is the row `1 0 1 0 1 0 1 0` after even-
/// bit inversion on the wire, i.e. 0xD5 — the value `silence-alaw` uses.
/// This pins that bit 1 is the MSB so the Table 5/6 packing above is read
/// in the right order.
#[test]
fn bit_one_is_msb_polarity_bit() {
    // Wire byte 0xD5 decodes to A-law +8 (smallest positive magnitude).
    assert_eq!(oxideav_g711::alaw::decode_sample(0xD5), 8);
    // And our packer produces 0xD5 from the bit-1-first row.
    assert_eq!(row_to_byte([1, 1, 0, 1, 0, 1, 0, 1]), 0xD5);
}
