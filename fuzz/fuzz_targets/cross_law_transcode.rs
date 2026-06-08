#![no_main]

//! Drive arbitrary fuzz-supplied bytes through a **cross-law
//! transcoding** pipeline — decode one law via the trait surface,
//! re-encode the decoded PCM as the *other* law via the trait
//! surface, then decode the re-encoded packet back as the second
//! law — and assert the byte / sample sequence matches the
//! per-sample baseline the per-sample helpers `decode_sample` /
//! `encode_sample` define.
//!
//! ## Why a sixth fuzz target
//!
//! The five existing targets all stay within **one law per pipeline**:
//!
//! - `decode_pipeline` / `per_sample_invariants` exercise one law's
//!   decoder in isolation.
//! - `encode_pipeline` builds an encoder + decoder of the *same* law
//!   and roundtrips through them.
//! - `streaming_pipeline` is also a same-law pair, just multi-frame.
//! - `factory_params` exercises adversarial parameter shapes through
//!   each factory in isolation, never crossing the law boundary.
//!
//! Cross-law transcoding is the canonical **PSTN-gateway** use case:
//! a North-American (µ-law) ↔ European (A-law) circuit interconnect
//! must decode incoming bytes under one law and re-encode them under
//! the other on every sample. The trait-surface contract for this
//! pipeline is that the cross-law output equals the per-sample
//! baseline `encode_sample_other(decode_sample_self(b))` applied to
//! every byte of the input, with no framing-level reorder, no
//! padding, and no per-frame state leak between adjacent transcoded
//! bytes. The first byte through the cross-law pipeline must produce
//! the same other-law byte as if it had been transcoded standalone;
//! the last byte through the pipeline likewise. If the framing wrapper
//! ever invents or drops a sample, or if a future per-frame state
//! addition (an internal smoother, a dither generator, a context-
//! dependent quantiser) silently coupled adjacent samples, this
//! target catches it on the first divergent byte.
//!
//! Beyond the forward transcode (law A → law B), the target also
//! drives the **reverse round-trip** (law A → law B → law A) and
//! asserts the result lands within a documented bound of the per-
//! sample double-transcode baseline. G.711 is lossy in each
//! direction, so cross-law transcoding is not bit-exact at the byte
//! level on the return trip — but it must be bit-exact against
//! `encode_sample_self(decode_sample_other(encode_sample_other(decode_sample_self(b))))`
//! applied byte-by-byte, which is the per-sample reference path.
//!
//! ## Fuzz input layout
//!
//! ```text
//!   byte 0      : seed
//!                 bit 0      : forward direction
//!                   0 → µ-law → A-law
//!                   1 → A-law → µ-law
//!                 bit 1      : reverse roundtrip
//!                   0 → forward transcode only (one-shot)
//!                   1 → forward + reverse roundtrip (both directions)
//!                 bits 2..=4 : channels seed → (bits % 8) + 1, in 1..=8
//!                 bits 5..=7 : reserved (ignored)
//!   bytes 1-2   : sample_rate seed → LE u16, clamped to 1..=192_000.
//!                 G.711 is sample-rate agnostic but the encoder builds
//!                 a `TimeBase` from this value; the cross-law pipeline
//!                 inherits both encoders' rate so this lands on the
//!                 same rate in both directions.
//!   bytes 3..   : raw input bytes — fed verbatim to the **input
//!                 decoder** (whose law the bit-0 seed selected). The
//!                 decoder needs no length / format validation beyond
//!                 the wrapper's `len % channels == 0` rejection; the
//!                 per-sample LUT is total over `u8`. Capped at 4 096
//!                 bytes so allocations stay tractable across large
//!                 channel counts.
//! ```
//!
//! ## Per-iteration sample budget
//!
//! 4 096 input bytes → 4 096 i16 PCM samples → 4 096 re-encoded
//! bytes → optional 4 096 PCM samples on the reverse. Under the 8-
//! channel cap this still keeps every per-iteration allocation under
//! ~16 KiB even on the reverse-roundtrip path, well within the
//! libFuzzer per-iter budget.

use libfuzzer_sys::fuzz_target;
use oxideav_core::{
    AudioFrame, CodecId, CodecParameters, Decoder, Encoder, Frame, Packet, SampleFormat, TimeBase,
};
use oxideav_g711::{alaw, mulaw};

const MAX_INPUT_BYTES: usize = 4_096;

fuzz_target!(|data: &[u8]| {
    if data.len() < 3 {
        return;
    }
    let seed = data[0];
    let forward_mulaw_to_alaw = (seed & 0x01) == 0;
    let do_reverse_roundtrip = (seed & 0x02) != 0;
    let channels = (((seed >> 2) & 0x07) as u16) + 1;

    let raw_rate = u16::from_le_bytes([data[1], data[2]]) as u32;
    let sample_rate = (raw_rate % 192_000).max(1);

    let payload_raw = &data[3..];
    let payload_len = payload_raw.len().min(MAX_INPUT_BYTES);
    // Round the payload length down to a multiple of the channel
    // count so the input decoder's `len % channels == 0` wrapper
    // check passes and we reach the inner LUT-decode loop. (The
    // rejection branch is already covered by `decode_pipeline`; this
    // target wants the happy-path transcode flow.)
    let nch = channels as usize;
    let trimmed = (payload_len / nch) * nch;
    if trimmed == 0 {
        return;
    }
    let payload = &payload_raw[..trimmed];

    // ── Build the parameter shapes. Both directions share the same
    //    channel count and sample rate; only the codec id changes.
    let input_id = if forward_mulaw_to_alaw {
        "pcm_mulaw"
    } else {
        "pcm_alaw"
    };
    let output_id = if forward_mulaw_to_alaw {
        "pcm_alaw"
    } else {
        "pcm_mulaw"
    };

    let mut input_params = CodecParameters::audio(CodecId::new(input_id));
    input_params.channels = Some(channels);
    input_params.sample_rate = Some(sample_rate);
    input_params.sample_format = Some(SampleFormat::S16);

    let mut output_params = CodecParameters::audio(CodecId::new(output_id));
    output_params.channels = Some(channels);
    output_params.sample_rate = Some(sample_rate);
    output_params.sample_format = Some(SampleFormat::S16);

    let make_input_dec = if forward_mulaw_to_alaw {
        mulaw::make_decoder
    } else {
        alaw::make_decoder
    };
    let make_output_enc = if forward_mulaw_to_alaw {
        alaw::make_encoder
    } else {
        mulaw::make_encoder
    };

    let mut input_dec: Box<dyn Decoder> = match make_input_dec(&input_params) {
        Ok(d) => d,
        Err(_) => return,
    };
    let mut output_enc: Box<dyn Encoder> = match make_output_enc(&output_params) {
        Ok(e) => e,
        Err(_) => return,
    };

    // ── Compute the per-sample baseline the cross-law pipeline must
    //    reproduce byte-for-byte. This is the canonical PSTN-gateway
    //    transcoding contract: for every input byte `b` under law A,
    //    the cross-law output is `encode_B(decode_A(b))` applied byte-
    //    by-byte. Anything else would mean the framing wrapper
    //    invented samples, lost samples, or coupled adjacent samples
    //    through some non-existent state.
    let expected_xcoded: Vec<u8> = payload
        .iter()
        .map(|&b| {
            let s = if forward_mulaw_to_alaw {
                mulaw::decode_sample(b)
            } else {
                alaw::decode_sample(b)
            };
            if forward_mulaw_to_alaw {
                alaw::encode_sample(s)
            } else {
                mulaw::encode_sample(s)
            }
        })
        .collect();

    // ── Drive the forward transcode through the trait surface.
    //    Step 1: send the input payload to the input-law decoder.
    let input_pkt = Packet::new(0, TimeBase::new(1, sample_rate as i64), payload.to_vec());
    if input_dec.send_packet(&input_pkt).is_err() {
        return;
    }
    let decoded_af = match input_dec.receive_frame() {
        Ok(Frame::Audio(a)) => a,
        _ => return,
    };

    // Sanity: the decoder must emit exactly one S16 per input byte
    // (length in bytes = 2 × payload_len).
    let decoded_bytes = decoded_af.data.first().cloned().unwrap_or_default();
    assert_eq!(
        decoded_bytes.len(),
        payload.len() * 2,
        "input decoder produced {} PCM bytes for {} input bytes \
         (input_law={input_id} channels={channels})",
        decoded_bytes.len(),
        payload.len(),
    );

    //    Step 2: encode the decoded PCM as the opposite law.
    let frame = Frame::Audio(AudioFrame {
        samples: decoded_af.samples,
        pts: Some(0),
        data: vec![decoded_bytes.clone()],
    });
    if output_enc.send_frame(&frame).is_err() {
        return;
    }
    let xcoded_pkt = match output_enc.receive_packet() {
        Ok(p) => p,
        Err(_) => return,
    };

    // Sanity: one re-encoded byte per input byte (the cross-law
    // transcode preserves the byte count exactly — both laws are
    // 1 byte per sample).
    assert_eq!(
        xcoded_pkt.data.len(),
        payload.len(),
        "cross-law transcode byte count {} ≠ input byte count {} \
         (input_law={input_id} output_law={output_id} channels={channels})",
        xcoded_pkt.data.len(),
        payload.len(),
    );

    // ── Forward equality: the cross-law output must match the per-
    //    sample baseline byte-for-byte. Any divergence is a framing
    //    bug, a state-coupling bug, or a per-sample-helper drift.
    assert_eq!(
        xcoded_pkt.data,
        expected_xcoded,
        "cross-law transcode diverges from per-sample baseline \
         (input_law={input_id} output_law={output_id} \
         channels={channels} input_bytes={})",
        payload.len(),
    );

    if !do_reverse_roundtrip {
        return;
    }

    // ── Reverse roundtrip — feed the re-encoded packet back through
    //    a fresh decoder of the *output* law, then re-encode under
    //    the *input* law, and assert byte-for-byte equality with the
    //    per-sample double-transcode baseline. The output is generally
    //    not equal to the original input payload (G.711 is lossy in
    //    each direction); equality is only guaranteed against the
    //    per-sample reference path.
    let make_output_dec = if forward_mulaw_to_alaw {
        alaw::make_decoder
    } else {
        mulaw::make_decoder
    };
    let make_input_enc = if forward_mulaw_to_alaw {
        mulaw::make_encoder
    } else {
        alaw::make_encoder
    };

    let mut output_dec: Box<dyn Decoder> = match make_output_dec(&output_params) {
        Ok(d) => d,
        Err(_) => return,
    };
    let mut input_enc: Box<dyn Encoder> = match make_input_enc(&input_params) {
        Ok(e) => e,
        Err(_) => return,
    };

    let expected_reverse: Vec<u8> = payload
        .iter()
        .map(|&b| {
            // Apply law-A → law-B → law-A entirely through the per-
            // sample helpers as the oracle for the reverse trip.
            let s1 = if forward_mulaw_to_alaw {
                mulaw::decode_sample(b)
            } else {
                alaw::decode_sample(b)
            };
            let b_out = if forward_mulaw_to_alaw {
                alaw::encode_sample(s1)
            } else {
                mulaw::encode_sample(s1)
            };
            let s2 = if forward_mulaw_to_alaw {
                alaw::decode_sample(b_out)
            } else {
                mulaw::decode_sample(b_out)
            };
            if forward_mulaw_to_alaw {
                mulaw::encode_sample(s2)
            } else {
                alaw::encode_sample(s2)
            }
        })
        .collect();

    if output_dec.send_packet(&xcoded_pkt).is_err() {
        return;
    }
    let reverse_af = match output_dec.receive_frame() {
        Ok(Frame::Audio(a)) => a,
        _ => return,
    };
    let reverse_pcm = reverse_af.data.first().cloned().unwrap_or_default();
    assert_eq!(
        reverse_pcm.len(),
        payload.len() * 2,
        "reverse decoder produced {} PCM bytes for {} input bytes \
         (output_law={output_id} channels={channels})",
        reverse_pcm.len(),
        payload.len(),
    );

    let reverse_frame = Frame::Audio(AudioFrame {
        samples: reverse_af.samples,
        pts: Some(0),
        data: vec![reverse_pcm],
    });
    if input_enc.send_frame(&reverse_frame).is_err() {
        return;
    }
    let final_pkt = match input_enc.receive_packet() {
        Ok(p) => p,
        Err(_) => return,
    };

    assert_eq!(
        final_pkt.data.len(),
        payload.len(),
        "reverse-roundtrip byte count {} ≠ input byte count {} \
         (input_law={input_id} output_law={output_id} channels={channels})",
        final_pkt.data.len(),
        payload.len(),
    );

    assert_eq!(
        final_pkt.data,
        expected_reverse,
        "cross-law reverse roundtrip diverges from per-sample double-\
         transcode baseline (input_law={input_id} output_law={output_id} \
         channels={channels} input_bytes={})",
        payload.len(),
    );
});
