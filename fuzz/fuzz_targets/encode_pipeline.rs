#![no_main]

//! Drive arbitrary fuzz-supplied i16 PCM through both encoders and
//! then back through the matching decoder, asserting the trait
//! surface is a faithful wrapper around the per-sample math the
//! `bit_exact_reference` integration test already pins.
//!
//! ## Why a roundtrip-equivalence target (not a bit-exact roundtrip)
//!
//! G.711 is **lossy** — the encoder projects each i16 sample onto one
//! of ~256 representable levels, so generic `decode(encode(s)) == s`
//! does not hold. What does hold, and what this target asserts, is
//! that the trait surface preserves the per-sample mapping:
//!
//! ```text
//!   trait_decode(trait_encode([s0, s1, …, sN]))
//!       == [decode_sample(encode_sample(s0)),
//!           decode_sample(encode_sample(s1)),
//!           …,
//!           decode_sample(encode_sample(sN))]
//! ```
//!
//! Any deviation — wrong endianness in the encoder's byte assembly, a
//! framing-level reorder under multichannel interleave, a silent
//! padding sample — surfaces as a mismatch. The bit-exact
//! `decode_sample(encode_sample(s))` baseline is the per-sample LUT
//! result `bit_exact_reference` already validates against the
//! spec-derived reference formulas, so this target is effectively
//! "the framing wrapper must not invent samples or shuffle order."
//!
//! ## Fuzz input layout
//!
//! ```text
//!   byte 0      : channels seed → (b0 % 8) + 1, in 1..=8
//!   bytes 1-4   : sample_rate seed → LE u32, masked to a sensible
//!                 range so the encoder doesn't reject it (any rate
//!                 ≥ 1 and ≤ 192_000 is accepted; G.711 is sample-rate
//!                 agnostic per the spec).
//!   byte 5      : codec seed → 0 → µ-law, 1 → A-law
//!   bytes 6..   : interleaved i16 PCM little-endian. Trailing odd
//!                 byte is dropped (the encoder rejects odd payloads
//!                 explicitly so we keep the input even-length here).
//! ```
//!
//! ## Sample budget
//!
//! Capped at 4096 samples / channel so the fuzzer's per-iteration
//! budget lands on the framing wrapper rather than on multi-MiB
//! allocations. The encoder's per-sample work is `O(1)` so a tighter
//! cap would still reach the same coverage.

use libfuzzer_sys::fuzz_target;
use oxideav_core::{AudioFrame, CodecId, CodecParameters, Decoder, Encoder, Frame, SampleFormat};
use oxideav_g711::{alaw, mulaw};

const MAX_SAMPLES_PER_CHANNEL: usize = 4096;

fuzz_target!(|data: &[u8]| {
    if data.len() < 8 {
        return;
    }
    let channels = ((data[0] as u16) % 8) + 1;
    let raw_rate = u32::from_le_bytes([data[1], data[2], data[3], data[4]]);
    // Clamp to 1..=192_000 — the encoder accepts any non-zero rate but
    // a uselessly huge value just consumes RuntimeContext storage in
    // CodecParameters and crowds the fuzzer's iteration budget.
    let sample_rate = (raw_rate % 192_000).max(1);
    let use_alaw = (data[5] & 1) != 0;

    // i16 payload: interleaved LE bytes from byte 6 onward, capped.
    let nch = channels as usize;
    let max_slots = MAX_SAMPLES_PER_CHANNEL * nch;
    let raw_payload = &data[6..];
    let slot_bytes = (raw_payload.len() / 2) * 2; // round down to even
    let target_slots = slot_bytes / 2;
    let capped_slots = target_slots.min(max_slots);
    let slots = (capped_slots / nch) * nch; // multiple of nch
    if slots == 0 {
        return;
    }
    let pcm_bytes: Vec<u8> = raw_payload[..slots * 2].to_vec();

    // Per-sample bit-exact baseline.
    let mut expected_decoded: Vec<i16> = Vec::with_capacity(slots);
    for chunk in pcm_bytes.chunks_exact(2) {
        let s = i16::from_le_bytes([chunk[0], chunk[1]]);
        let b = if use_alaw {
            alaw::encode_sample(s)
        } else {
            mulaw::encode_sample(s)
        };
        let d = if use_alaw {
            alaw::decode_sample(b)
        } else {
            mulaw::decode_sample(b)
        };
        expected_decoded.push(d);
    }

    // Build encoder + decoder.
    let codec_id = if use_alaw { "pcm_alaw" } else { "pcm_mulaw" };
    let mut params = CodecParameters::audio(CodecId::new(codec_id));
    params.channels = Some(channels);
    params.sample_rate = Some(sample_rate);
    params.sample_format = Some(SampleFormat::S16);

    let make_enc = if use_alaw {
        alaw::make_encoder
    } else {
        mulaw::make_encoder
    };
    let make_dec = if use_alaw {
        alaw::make_decoder
    } else {
        mulaw::make_decoder
    };

    let mut enc: Box<dyn Encoder> = match make_enc(&params) {
        Ok(e) => e,
        Err(_) => return,
    };
    let mut dec: Box<dyn Decoder> = match make_dec(&params) {
        Ok(d) => d,
        Err(_) => return,
    };

    // Encode.
    let frame = Frame::Audio(AudioFrame {
        samples: (slots / nch) as u32,
        pts: Some(0),
        data: vec![pcm_bytes.clone()],
    });
    if enc.send_frame(&frame).is_err() {
        return;
    }
    let pkt = match enc.receive_packet() {
        Ok(p) => p,
        Err(_) => return,
    };

    // Sanity: one encoded byte per input i16 sample.
    assert_eq!(
        pkt.data.len(),
        slots,
        "encoder produced {} bytes for {} input samples (codec={codec_id} ch={channels})",
        pkt.data.len(),
        slots
    );

    // Decode.
    if dec.send_packet(&pkt).is_err() {
        return;
    }
    let af = match dec.receive_frame() {
        Ok(Frame::Audio(a)) => a,
        _ => return,
    };

    // Cross-check sample counts.
    assert_eq!(
        af.samples as usize * nch,
        slots,
        "decoder produced {} interleaved samples × {nch} ch = {} (expected {slots})",
        af.samples,
        af.samples as usize * nch
    );
    let decoded_bytes = af.data.first().cloned().unwrap_or_default();
    assert_eq!(
        decoded_bytes.len(),
        slots * 2,
        "decoder output byte length {} (expected {})",
        decoded_bytes.len(),
        slots * 2
    );

    // Trait-surface decoded samples must match the per-sample baseline.
    let mut decoded: Vec<i16> = Vec::with_capacity(slots);
    for chunk in decoded_bytes.chunks_exact(2) {
        decoded.push(i16::from_le_bytes([chunk[0], chunk[1]]));
    }
    assert_eq!(
        decoded, expected_decoded,
        "trait-surface roundtrip diverges from per-sample baseline \
         (codec={codec_id} ch={channels} rate={sample_rate} slots={slots})"
    );
});
