//! Cross-law (µ-law ↔ A-law) transcoding contract through the
//! trait surface.
//!
//! This is the canonical **PSTN-gateway** use case: a North-American
//! (µ-law) ↔ European (A-law) circuit interconnect must decode incoming
//! bytes under one law and re-encode them under the *other* law on every
//! sample (ITU-T G.711 §2 defines A-law, §3 defines µ-law; an
//! interconnect that bridges the two networks transcodes at the
//! boundary). The five existing fuzz targets and every other integration
//! test in this crate stay within **one law per pipeline** — the only
//! existing coverage that crosses the µ ↔ A boundary is the
//! `cross_law_transcode` libFuzzer target, which runs **only** under
//! `cargo +nightly fuzz run` and never on the standard CI test job.
//!
//! These tests pin the same composition contract as a deterministic,
//! always-CI-gated regression: the trait-surface cross-law output must
//! equal the per-sample baseline `encode_B(decode_A(b))` applied
//! byte-for-byte, with no framing-level reorder, no padding, and no
//! per-frame state leaking between adjacent transcoded samples. If a
//! future change to either decoder's `receive_frame` ever invents or
//! drops a sample, or if a per-frame state addition (an internal
//! smoother, a dither generator, a context-dependent quantiser) ever
//! coupled adjacent samples through some non-existent state, these
//! tests catch it on the first divergent byte — on every CI run, not
//! just when the nightly fuzzer happens to be invoked.
//!
//! The per-sample helpers `decode_sample` / `encode_sample` are the
//! oracle: they are themselves pinned bit-exact against the ITU-T
//! G.711 §2 / §3 reference formulas by the exhaustive
//! `bit_exact_reference` sweeps, so anchoring the trait-surface
//! transcode to them transitively anchors it to the spec.

use oxideav_core::{
    AudioFrame, CodecId, CodecParameters, Decoder, Encoder, Frame, Packet, SampleFormat, TimeBase,
};
use oxideav_g711::{alaw, mulaw};

/// Build S16 audio params for a given codec id / channel count.
fn params(codec: &str, channels: u16, sample_rate: u32) -> CodecParameters {
    let mut p = CodecParameters::audio(CodecId::new(codec));
    p.sample_rate = Some(sample_rate);
    p.channels = Some(channels);
    p.sample_format = Some(SampleFormat::S16);
    p
}

/// Decode `bytes` under the input law via the trait surface, returning
/// the interleaved S16 PCM the decoder emits (as LE bytes).
fn decode_trait(input_id: &str, channels: u16, sample_rate: u32, bytes: &[u8]) -> Vec<u8> {
    let p = params(input_id, channels, sample_rate);
    let mut dec: Box<dyn Decoder> = match input_id {
        "pcm_mulaw" => mulaw::make_decoder(&p),
        "pcm_alaw" => alaw::make_decoder(&p),
        other => panic!("unknown input law {other}"),
    }
    .expect("make_decoder");
    let pkt = Packet::new(0, TimeBase::new(1, sample_rate as i64), bytes.to_vec());
    dec.send_packet(&pkt).expect("send_packet");
    let Frame::Audio(af) = dec.receive_frame().expect("receive_frame") else {
        panic!("expected audio frame");
    };
    af.data.first().cloned().unwrap_or_default()
}

/// Encode interleaved S16 PCM (`pcm_le`) under the output law via the
/// trait surface, returning the companded bytes.
fn encode_trait(
    output_id: &str,
    channels: u16,
    sample_rate: u32,
    samples_per_channel: u32,
    pcm_le: Vec<u8>,
) -> Vec<u8> {
    let p = params(output_id, channels, sample_rate);
    let mut enc: Box<dyn Encoder> = match output_id {
        "pcm_mulaw" => mulaw::make_encoder(&p),
        "pcm_alaw" => alaw::make_encoder(&p),
        other => panic!("unknown output law {other}"),
    }
    .expect("make_encoder");
    let frame = Frame::Audio(AudioFrame {
        samples: samples_per_channel,
        pts: Some(0),
        data: vec![pcm_le],
    });
    enc.send_frame(&frame).expect("send_frame");
    enc.receive_packet().expect("receive_packet").data
}

/// Per-sample forward-transcode oracle: `encode_B(decode_A(b))` for a
/// single input byte `b`, where A is `forward_mulaw_to_alaw`'s input law.
fn baseline_forward_byte(b: u8, forward_mulaw_to_alaw: bool) -> u8 {
    if forward_mulaw_to_alaw {
        alaw::encode_sample(mulaw::decode_sample(b))
    } else {
        mulaw::encode_sample(alaw::decode_sample(b))
    }
}

/// Per-sample double-transcode oracle for the reverse roundtrip:
/// `encode_A(decode_B(encode_B(decode_A(b))))`.
fn baseline_reverse_byte(b: u8, forward_mulaw_to_alaw: bool) -> u8 {
    let mid = baseline_forward_byte(b, forward_mulaw_to_alaw);
    if forward_mulaw_to_alaw {
        // mid is A-law; decode A, re-encode µ.
        mulaw::encode_sample(alaw::decode_sample(mid))
    } else {
        // mid is µ-law; decode µ, re-encode A.
        alaw::encode_sample(mulaw::decode_sample(mid))
    }
}

/// Drive one forward cross-law transcode through the trait surface and
/// assert it matches the per-sample baseline byte-for-byte.
fn assert_forward_transcode(forward_mulaw_to_alaw: bool, channels: u16, payload: &[u8]) {
    let (input_id, output_id) = if forward_mulaw_to_alaw {
        ("pcm_mulaw", "pcm_alaw")
    } else {
        ("pcm_alaw", "pcm_mulaw")
    };
    let sample_rate = 8_000;
    assert_eq!(
        payload.len() % channels as usize,
        0,
        "test payload must be a whole number of frames"
    );
    let samples_per_channel = (payload.len() / channels as usize) as u32;

    // Step 1: decode under the input law.
    let pcm_le = decode_trait(input_id, channels, sample_rate, payload);
    assert_eq!(
        pcm_le.len(),
        payload.len() * 2,
        "input decoder must emit one S16 per input byte"
    );

    // Step 2: re-encode under the output law.
    let xcoded = encode_trait(
        output_id,
        channels,
        sample_rate,
        samples_per_channel,
        pcm_le,
    );
    assert_eq!(
        xcoded.len(),
        payload.len(),
        "cross-law transcode must preserve the byte count (1 byte/sample both laws)"
    );

    // The trait-surface cross-law output must equal the per-sample
    // baseline applied byte-by-byte.
    let expected: Vec<u8> = payload
        .iter()
        .map(|&b| baseline_forward_byte(b, forward_mulaw_to_alaw))
        .collect();
    assert_eq!(
        xcoded, expected,
        "cross-law transcode {input_id} → {output_id} (ch={channels}) \
         diverged from per-sample baseline encode_B(decode_A(b))"
    );
}

/// Drive the full reverse roundtrip (A → B → A) through the trait
/// surface and assert it matches the per-sample double-transcode
/// baseline byte-for-byte. (G.711 is lossy in each direction, so the
/// output is *not* the original payload — equality holds only against
/// the per-sample reference path.)
fn assert_reverse_roundtrip(forward_mulaw_to_alaw: bool, channels: u16, payload: &[u8]) {
    let (input_id, output_id) = if forward_mulaw_to_alaw {
        ("pcm_mulaw", "pcm_alaw")
    } else {
        ("pcm_alaw", "pcm_mulaw")
    };
    let sample_rate = 8_000;
    let samples_per_channel = (payload.len() / channels as usize) as u32;

    // Forward: A → B.
    let pcm_a = decode_trait(input_id, channels, sample_rate, payload);
    let xcoded = encode_trait(output_id, channels, sample_rate, samples_per_channel, pcm_a);

    // Reverse: B → A through fresh trait objects.
    let pcm_b = decode_trait(output_id, channels, sample_rate, &xcoded);
    let final_bytes = encode_trait(input_id, channels, sample_rate, samples_per_channel, pcm_b);

    assert_eq!(
        final_bytes.len(),
        payload.len(),
        "reverse roundtrip must preserve the byte count"
    );

    let expected: Vec<u8> = payload
        .iter()
        .map(|&b| baseline_reverse_byte(b, forward_mulaw_to_alaw))
        .collect();
    assert_eq!(
        final_bytes, expected,
        "cross-law reverse roundtrip {input_id} → {output_id} → {input_id} \
         (ch={channels}) diverged from per-sample double-transcode baseline"
    );
}

/// Every one of the 256 codewords, as a single mono frame, must
/// forward-transcode µ → A through the trait surface exactly as the
/// per-sample baseline does.
#[test]
fn forward_mulaw_to_alaw_all_bytes_mono() {
    let all: Vec<u8> = (0..=255u8).collect();
    assert_forward_transcode(true, 1, &all);
}

/// And A → µ likewise, exhaustively over all 256 input codewords.
#[test]
fn forward_alaw_to_mulaw_all_bytes_mono() {
    let all: Vec<u8> = (0..=255u8).collect();
    assert_forward_transcode(false, 1, &all);
}

/// Multichannel interleave must not couple adjacent samples: the same
/// 256-byte sweep tiled across 1, 2, 6, 8 channels must transcode
/// identically per byte (channels merely interleave the per-sample op).
#[test]
fn forward_transcode_multichannel_interleave_is_independent() {
    for forward in [true, false] {
        for &channels in &[1u16, 2, 6, 8] {
            // 256 frames × `channels` bytes, distinct per slot so an
            // interleave bug surfaces as a divergent byte.
            let mut payload = Vec::with_capacity(256 * channels as usize);
            for i in 0..256u32 {
                for ch in 0..channels as u32 {
                    payload.push(
                        ((i.wrapping_mul(7)).wrapping_add(ch.wrapping_mul(101)) & 0xFF) as u8,
                    );
                }
            }
            assert_forward_transcode(forward, channels, &payload);
        }
    }
}

/// The full A ↔ µ ↔ A reverse roundtrip composes correctly through four
/// trait objects, exhaustively over all 256 codewords, both directions.
#[test]
fn reverse_roundtrip_all_bytes_both_directions() {
    let all: Vec<u8> = (0..=255u8).collect();
    assert_reverse_roundtrip(true, 1, &all);
    assert_reverse_roundtrip(false, 1, &all);
}

/// Reverse roundtrip across multichannel frames stays per-sample
/// independent the same way the forward transcode does.
#[test]
fn reverse_roundtrip_multichannel() {
    for forward in [true, false] {
        for &channels in &[2u16, 6, 8] {
            let mut payload = Vec::with_capacity(256 * channels as usize);
            for i in 0..256u32 {
                for ch in 0..channels as u32 {
                    payload.push(
                        ((i.wrapping_mul(13)).wrapping_add(ch.wrapping_mul(57)) & 0xFF) as u8,
                    );
                }
            }
            assert_reverse_roundtrip(forward, channels, &payload);
        }
    }
}

/// A second forward transcode (µ → A → µ via two *forward* transcodes
/// rather than the reverse helper) is **idempotent** at the byte level
/// from the second hop on: once a sample has been quantised onto the
/// A-law grid and back, transcoding it again lands on the same byte.
/// This pins the projection property across the cross-law boundary —
/// PSTN gateways that double-transcode a tandem connection must not
/// accumulate per-hop drift beyond the first round-trip.
#[test]
fn cross_law_double_transcode_is_idempotent_from_second_hop() {
    let sample_rate = 8_000;
    let all: Vec<u8> = (0..=255u8).collect();

    // Hop 1: µ → A.
    let pcm0 = decode_trait("pcm_mulaw", 1, sample_rate, &all);
    let a1 = encode_trait("pcm_alaw", 1, sample_rate, all.len() as u32, pcm0);
    // Hop 2: A → µ.
    let pcm1 = decode_trait("pcm_alaw", 1, sample_rate, &a1);
    let m2 = encode_trait("pcm_mulaw", 1, sample_rate, a1.len() as u32, pcm1);
    // Hop 3: µ → A again.
    let pcm2 = decode_trait("pcm_mulaw", 1, sample_rate, &m2);
    let a3 = encode_trait("pcm_alaw", 1, sample_rate, m2.len() as u32, pcm2);
    // Hop 4: A → µ again.
    let pcm3 = decode_trait("pcm_alaw", 1, sample_rate, &a3);
    let m4 = encode_trait("pcm_mulaw", 1, sample_rate, a3.len() as u32, pcm3);

    // From the second µ-law observation on, the byte stream is fixed.
    assert_eq!(
        a1, a3,
        "A-law tandem output drifted between the first and second double-hop"
    );
    assert_eq!(
        m2, m4,
        "µ-law tandem output drifted between the first and second double-hop"
    );
}
