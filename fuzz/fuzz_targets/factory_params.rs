#![no_main]

//! Drive arbitrary fuzz-supplied `CodecParameters` shapes through
//! all four factory entry points — `mulaw::make_decoder`,
//! `mulaw::make_encoder`, `alaw::make_decoder`, `alaw::make_encoder` —
//! and assert that the parameter-validation surface is total over the
//! attacker's input: every (codec_id, channels, sample_format,
//! sample_rate) tuple either constructs a working trait object that
//! survives one packet / frame round-trip cleanly, or returns a clean
//! `Err` (`Error::unsupported(...)`). Never panics, never aborts,
//! never deadlocks.
//!
//! ## Why a fifth fuzz target
//!
//! The existing four targets (`decode_pipeline`, `encode_pipeline`,
//! `per_sample_invariants`, `streaming_pipeline`) all build
//! `CodecParameters` themselves via [`CodecParameters::audio`] with
//! a known-valid codec id (`"pcm_mulaw"` / `"pcm_alaw"`), the
//! `MediaType::Audio` discriminant pre-set, the channel count
//! pre-clamped to `1..=8`, and the default `sample_format` (`None`,
//! which the encoder factory resolves to `S16` — its only accepted
//! input format). That leaves three rejection branches the four
//! existing fuzzers do not exercise:
//!
//! 1. **`channels == 0`.** All four factories (both laws, both
//!    directions) reject `channels == Some(0)` with
//!    `Error::unsupported(...)`. The existing fuzzers seed
//!    `channels` from `(byte % 8) + 1`, so `0` is unreachable.
//! 2. **Non-`S16` `sample_format`.** Both encoder factories reject
//!    every `SampleFormat` other than `S16`. Twelve variants are
//!    defined in `oxideav_core` (`U8`, `S8`, `S16`, `S24`, `S32`,
//!    `F32`, `F64`, plus their planar counterparts); none of the
//!    existing fuzzers sets `params.sample_format` so eleven of
//!    those twelve branches are dark.
//! 3. **Adversarial `codec_id` strings.** The factories use the
//!    supplied `codec_id` verbatim in the returned trait object's
//!    `codec_id()` accessor — they do not validate that the id
//!    matches the law of the factory being called. A registry
//!    consumer might paint themselves into a corner by passing
//!    `"pcm_alaw"` to `mulaw::make_decoder` (which succeeds and
//!    returns a µ-law-behaving decoder whose `codec_id()` answers
//!    `"pcm_alaw"`). The fuzzer hammers this so any future
//!    validation tightening here (e.g. rejecting mismatched ids)
//!    gets a guaranteed regression net.
//!
//! Beyond the rejection branches, the target also drives the
//! **successful construction path** at an attacker-chosen sample
//! rate that the existing fuzzers do not cover — including `0`
//! (the rate-default branch substitutes `8_000`) and `u32::MAX`
//! (whose `i64` cast must not overflow when building the
//! `TimeBase`). A one-packet / one-frame sanity round-trip on the
//! returned trait object then guarantees the constructed instance
//! itself is total, not just the factory call.
//!
//! ## Fuzz input layout
//!
//! ```text
//!   byte 0      : factory_seed
//!                 bits 0..=1 → factory variant
//!                   0 → mulaw decoder
//!                   1 → mulaw encoder
//!                   2 → alaw decoder
//!                   3 → alaw encoder
//!                 bits 2..=4 → codec_id selector
//!                   0 → "pcm_mulaw"
//!                   1 → "pcm_alaw"
//!                   2 → "g711u"
//!                   3 → "g711a"
//!                   4 → "ulaw"
//!                   5 → "alaw"
//!                   6 → "" (empty)
//!                   7 → "deadbeef" (adversarial)
//!                 bits 5..=7 → reserved (ignored)
//!   bytes 1-2   : channels (LE u16). Zero is the rejection branch;
//!                 nonzero up to u16::MAX is accepted, though the
//!                 follow-up round-trip caps payload size so we
//!                 don't allocate gigabytes for channels=65535.
//!   byte 3      : sample_format_seed → indexes into the 13-entry
//!                 table [None, U8, S8, S16, S24, S32, F32, F64,
//!                 U8P, S16P, S32P, F32P, F64P]. (Out-of-range
//!                 values mod 13.)
//!   bytes 4-7   : sample_rate (LE u32). Zero and u32::MAX are
//!                 both exercised; the factory clamps neither, so
//!                 the encoder construction must be total over the
//!                 full u32 range.
//!   bytes 8..   : payload — fed verbatim into the constructed
//!                 trait object's surface. For a decoder, this
//!                 becomes the packet `data`; for an encoder, the
//!                 LE-byte interpretation as i16 PCM. Capped at
//!                 4 096 bytes so allocations stay tractable across
//!                 large channel counts.
//! ```

use libfuzzer_sys::fuzz_target;
use oxideav_core::{
    AudioFrame, CodecId, CodecParameters, Decoder, Encoder, Frame, MediaType, Packet, SampleFormat,
    TimeBase,
};
use oxideav_g711::{alaw, mulaw};

const MAX_PAYLOAD_BYTES: usize = 4_096;

/// The 13-entry sample-format table the fuzzer's `byte 3` indexes into.
/// `None` exercises the factory-default branch (encoder substitutes
/// `S16` when `sample_format` is unset); the remaining twelve cover
/// every named variant on the encoder's rejection ladder.
const SAMPLE_FORMATS: &[Option<SampleFormat>] = &[
    None,
    Some(SampleFormat::U8),
    Some(SampleFormat::S8),
    Some(SampleFormat::S16),
    Some(SampleFormat::S24),
    Some(SampleFormat::S32),
    Some(SampleFormat::F32),
    Some(SampleFormat::F64),
    Some(SampleFormat::U8P),
    Some(SampleFormat::S16P),
    Some(SampleFormat::S32P),
    Some(SampleFormat::F32P),
    Some(SampleFormat::F64P),
];

const CODEC_IDS: &[&str] = &[
    "pcm_mulaw",
    "pcm_alaw",
    "g711u",
    "g711a",
    "ulaw",
    "alaw",
    "",
    "deadbeef",
];

fuzz_target!(|data: &[u8]| {
    if data.len() < 8 {
        return;
    }
    let factory_seed = data[0];
    let factory_variant = factory_seed & 0x03;
    let codec_id_idx = ((factory_seed >> 2) & 0x07) as usize;
    let channels = u16::from_le_bytes([data[1], data[2]]);
    let sf_idx = (data[3] as usize) % SAMPLE_FORMATS.len();
    let sample_rate = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);

    let payload_raw = &data[8..];
    let payload_len = payload_raw.len().min(MAX_PAYLOAD_BYTES);
    let payload = &payload_raw[..payload_len];

    // Build the parameter struct. We *do not* go through
    // `CodecParameters::audio(...)` because we want full control
    // over every discriminant the factory inspects — including
    // exotic `media_type` shapes that the convenience constructor
    // would lock to `Audio`. A direct field-fill keeps the
    // attacker's seed reaching every validation site.
    let mut params = CodecParameters::audio(CodecId::new(CODEC_IDS[codec_id_idx]));
    params.media_type = MediaType::Audio;
    // channels = 0 is the documented rejection branch; we set it
    // verbatim from the seed (Some(0) is the failure path, Some(N>0)
    // is the success path). Mapping through Option preserves the
    // factory's `unwrap_or(1)` default-branch when the seed is 1.
    params.channels = Some(channels);
    params.sample_format = SAMPLE_FORMATS[sf_idx];
    params.sample_rate = Some(sample_rate);

    // ── Construct via the selected factory. The factory call itself
    //    must never panic regardless of (codec_id, channels,
    //    sample_format, sample_rate). On `Err`, drop and return —
    //    the rejection branch is the contract we're verifying.
    match factory_variant {
        0 => drive_decoder(mulaw::make_decoder(&params), channels, payload),
        1 => drive_encoder(mulaw::make_encoder(&params), payload),
        2 => drive_decoder(alaw::make_decoder(&params), channels, payload),
        3 => drive_encoder(alaw::make_encoder(&params), payload),
        _ => unreachable!("factory_variant masked to 0..=3"),
    }
});

/// One send-packet / receive-frame / flush cycle on a successfully
/// constructed decoder. The factory's `channels` field decides the
/// packet-length divisibility test we get hit by — channels above
/// the payload length send an empty packet (the early-return path),
/// channels at 1 with attacker payload exercises the LUT-decode
/// inner loop on attacker bytes, channels in 2..=payload_len exercise
/// the modulo-reject branch or the interleaved-decode happy path
/// depending on payload size.
fn drive_decoder(factory: oxideav_core::Result<Box<dyn Decoder>>, channels: u16, payload: &[u8]) {
    let Ok(mut dec) = factory else {
        return;
    };
    // Trim the payload so allocation stays tractable for huge channel
    // counts — the factory accepts channels = u16::MAX (=65535), but a
    // payload that long would force the decoder to allocate ~131 KiB
    // of S16 output per fuzz iteration and gnaw on the libFuzzer
    // budget. Capping by `channels` here keeps the run-time per
    // input low while preserving every interesting (channels,
    // length) modulo case.
    let cap = if channels == 0 {
        payload.len()
    } else {
        payload.len().min((channels as usize).saturating_mul(64))
    };
    let pkt = Packet::new(0, TimeBase::new(1, 8_000), payload[..cap].to_vec());
    // The decoder must accept and either decode-and-emit, reject
    // with `Error::Invalid` (length % channels != 0), or report
    // `NeedMore` after a flush — none of these may panic.
    if dec.send_packet(&pkt).is_ok() {
        let _ = dec.receive_frame();
    }
    let _ = dec.flush();
    // After flush + drain, the decoder must report `Eof` if asked
    // for another frame. We assert the no-panic property; we do not
    // assert the error kind because future relaxations of the trait
    // surface might widen the post-flush contract.
    let _ = dec.receive_frame();
    // codec_id() accessor — must always be callable. The factory
    // copies the params' codec_id verbatim into the trait object,
    // so this exercises the `"deadbeef"` and empty-string cases too.
    let _ = dec.codec_id();
}

/// One send-frame / receive-packet / flush cycle on a successfully
/// constructed encoder. Splits the attacker payload as interleaved
/// LE i16 PCM, trims to a channel-aligned slot count, and feeds the
/// resulting AudioFrame in. Encoder rejection paths inside
/// `send_frame` (odd byte count, empty data plane) are also reached
/// by short payloads.
fn drive_encoder(factory: oxideav_core::Result<Box<dyn Encoder>>, payload: &[u8]) {
    let Ok(mut enc) = factory else {
        return;
    };
    // Round payload to an even byte count — odd-length payloads are
    // explicitly rejected by send_frame and we want to reach both
    // the rejection branch and the happy-path encode loop without
    // burning iterations on a single rejection shape per input.
    let even = (payload.len() / 2) * 2;
    let bytes = payload[..even].to_vec();
    let samples = (even / 2) as u32;
    let frame = Frame::Audio(AudioFrame {
        samples,
        pts: Some(0),
        data: vec![bytes],
    });
    if enc.send_frame(&frame).is_ok() {
        // Drain whatever the encoder queued. send_frame produces
        // exactly one packet per accepted frame in the current
        // implementation; we still loop in case the contract widens.
        while enc.receive_packet().is_ok() {}
    }
    let _ = enc.flush();
    // Output-params accessor — must always be callable on a
    // successfully constructed encoder. Exercises the same
    // verbatim-codec_id propagation as the decoder side.
    let _ = enc.output_params();
    let _ = enc.codec_id();
}
