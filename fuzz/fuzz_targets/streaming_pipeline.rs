#![no_main]

//! Drive arbitrary fuzz-supplied i16 PCM through both encoders and
//! the matching decoders as a **streaming, multi-packet** session —
//! the lifecycle a real PSTN-style caller exercises (one encoder /
//! decoder instance, many small frames / packets, an end-of-stream
//! flush).
//!
//! ## Why a separate streaming target
//!
//! The sibling [`encode_pipeline`] target drives one frame in / one
//! packet out / one frame out through fresh encoder + decoder pairs.
//! That covers the per-frame byte-assembly contract but not the
//! cross-frame state the trait surface accumulates:
//!
//! - The encoder's [`std::collections::VecDeque<Packet>`] output queue
//!   — multiple `send_frame` calls must enqueue distinct packets that
//!   `receive_packet` drains in FIFO order without losing or
//!   re-emitting any.
//! - The decoder's `pending: Option<Packet>` slot — alternating
//!   `send_packet` / `receive_frame` over many cycles must always
//!   advance the state machine cleanly (no leaked state, no spurious
//!   `Error::other` from the double-send guard between cycles).
//! - The encoder's per-frame `pts` / `dts` / `duration` propagation
//!   across a multi-frame sequence — the decoder must observe the same
//!   `pts` stream the encoder emitted.
//! - Encoder `flush()` followed by post-flush behaviour: G.711 has no
//!   delay line so the encoder produces no extra packets at flush, but
//!   the call must succeed without disturbing already-queued packets.
//! - Decoder `flush()` mid-stream — `receive_frame()` on the next
//!   `send_packet` after a flush still produces output, then on the
//!   subsequent empty drain returns `Eof`.
//!
//! This target combines all of the above into a single attacker-driven
//! sequence and re-uses the per-sample baseline already pinned by the
//! `encode_pipeline` target as the sample-equality oracle.
//!
//! ## Fuzz input layout
//!
//! ```text
//!   byte 0       : codec seed → bit 0: 0 = µ-law, 1 = A-law
//!   byte 1       : channels seed → (b1 % 8) + 1, in 1..=8
//!   bytes 2..4   : sample_rate seed → LE u16, clamped to 1..=192_000
//!   byte 4       : burst layout seed → number of frames in 1..=8
//!                  (each frame is sized from a follow-up payload byte)
//!   byte 5       : drain order seed → 0 → drain after each send,
//!                  1 → enqueue all then drain.
//!   byte 6       : mid-stream-flush seed → 0 → no decoder flush mid-
//!                  stream, 1 → flush decoder after every other packet
//!                  (asserts `Eof` is only returned after the drain).
//!   bytes 7..    : per-frame headers + interleaved i16 LE payloads:
//!                    - first byte = samples-per-channel for this
//!                      frame, masked to 0..=255 (so each frame is at
//!                      most 256 samples/channel × nch channels × 2
//!                      bytes = up to 4 KiB of PCM).
//!                    - then `samples * nch * 2` raw bytes (truncated
//!                      to whatever the fuzz input has left).
//! ```
//!
//! Per-iteration cap: 8 frames × 256 samples/channel × 8 channels = 16 K
//! samples per session — well within the libFuzzer per-iteration budget
//! while still exercising the queue under realistic load.

use libfuzzer_sys::fuzz_target;
use oxideav_core::{
    AudioFrame, CodecId, CodecParameters, Decoder, Encoder, Error, Frame, SampleFormat,
};
use oxideav_g711::{alaw, mulaw};

const MAX_SAMPLES_PER_CHANNEL_PER_FRAME: usize = 256;
const MAX_FRAMES: usize = 8;

/// One queued frame ready to encode — payload bytes + samples/channel.
struct PendingFrame {
    pcm_bytes: Vec<u8>,
    samples_per_channel: u32,
    pts: i64,
}

/// Compute the per-sample-baseline decoded i16 stream the trait
/// surface must reproduce. Mirrors the oracle the `encode_pipeline`
/// target uses, just split across multiple frames.
fn expected_decode(pcm_bytes: &[u8], use_alaw: bool) -> Vec<i16> {
    let mut out = Vec::with_capacity(pcm_bytes.len() / 2);
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
        out.push(d);
    }
    out
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 8 {
        return;
    }
    let use_alaw = (data[0] & 1) != 0;
    let channels = ((data[1] as u16) % 8) + 1;
    let raw_rate = u16::from_le_bytes([data[2], data[3]]) as u32;
    let sample_rate = (raw_rate % 192_000).max(1);
    let frame_count = ((data[4] as usize) % MAX_FRAMES) + 1;
    let drain_eager = (data[5] & 1) != 0;
    let mid_stream_flush = (data[6] & 1) != 0;

    let nch = channels as usize;

    // Carve up the remaining bytes into frame_count frames. Each frame
    // begins with a one-byte length marker (samples/channel) and is
    // followed by `samples * nch * 2` PCM bytes.
    let mut cursor = 7;
    let mut frames: Vec<PendingFrame> = Vec::with_capacity(frame_count);
    let mut next_pts: i64 = 0;
    for _ in 0..frame_count {
        if cursor >= data.len() {
            break;
        }
        let samples_per_channel = (data[cursor] as usize) % (MAX_SAMPLES_PER_CHANNEL_PER_FRAME + 1);
        cursor += 1;
        if samples_per_channel == 0 {
            // Empty frame — the encoder rejects empty `data[0]`. Skip
            // rather than feed an empty frame; the empty-packet path
            // is already covered by `decode_pipeline`.
            continue;
        }
        let bytes_needed = samples_per_channel.saturating_mul(nch).saturating_mul(2);
        let available = data.len().saturating_sub(cursor);
        let take = bytes_needed.min(available);
        // Round down to a multiple of (nch * 2) so each frame stays
        // a whole number of interleaved samples — otherwise the
        // encoder would reject an odd byte count.
        let unit = nch.saturating_mul(2).max(2);
        let take = (take / unit) * unit;
        if take == 0 {
            // Not enough bytes left for a whole sample. Stop.
            break;
        }
        let pcm_bytes = data[cursor..cursor + take].to_vec();
        cursor += take;
        let actual_samples = (take / unit) as u32;
        frames.push(PendingFrame {
            pcm_bytes,
            samples_per_channel: actual_samples,
            pts: next_pts,
        });
        next_pts = next_pts.saturating_add(actual_samples as i64);
    }
    if frames.is_empty() {
        return;
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

    // ── Phase 1: feed every frame to the encoder.
    // Depending on the drain_eager seed, we either drain each packet
    // immediately (exercising the queue at depth 1) or queue them all
    // first (exercising the queue at depth = frame_count).
    let mut produced: Vec<(i64, Vec<u8>, Vec<u8>)> = Vec::with_capacity(frames.len());
    // (input_pts, encoded_bytes, original_pcm_bytes) per frame so the
    // decode phase can verify pts propagation + per-sample equivalence.

    for frame in &frames {
        let f = Frame::Audio(AudioFrame {
            samples: frame.samples_per_channel,
            pts: Some(frame.pts),
            data: vec![frame.pcm_bytes.clone()],
        });
        if enc.send_frame(&f).is_err() {
            return;
        }
        if drain_eager {
            let pkt = match enc.receive_packet() {
                Ok(p) => p,
                Err(_) => return,
            };
            assert_eq!(
                pkt.data.len(),
                frame.pcm_bytes.len() / 2,
                "eager drain: encoded packet length {} ≠ samples count {} (codec={codec_id})",
                pkt.data.len(),
                frame.pcm_bytes.len() / 2,
            );
            assert_eq!(
                pkt.pts,
                Some(frame.pts),
                "eager drain: pts propagation drift (sent {:?}, got {:?})",
                Some(frame.pts),
                pkt.pts,
            );
            produced.push((frame.pts, pkt.data, frame.pcm_bytes.clone()));
        }
    }

    // ── Encoder flush. G.711 carries no delay line so flush emits no
    // additional packets but must succeed and must not disturb the
    // queue.
    let _ = enc.flush();

    if !drain_eager {
        // Drain the full queue now.
        for frame in &frames {
            let pkt = match enc.receive_packet() {
                Ok(p) => p,
                Err(_) => return,
            };
            assert_eq!(
                pkt.data.len(),
                frame.pcm_bytes.len() / 2,
                "deferred drain: encoded packet length {} ≠ samples count {} (codec={codec_id})",
                pkt.data.len(),
                frame.pcm_bytes.len() / 2,
            );
            assert_eq!(
                pkt.pts,
                Some(frame.pts),
                "deferred drain: FIFO order broken (sent {:?}, got {:?})",
                Some(frame.pts),
                pkt.pts,
            );
            produced.push((frame.pts, pkt.data, frame.pcm_bytes.clone()));
        }
        // Queue must now be empty: subsequent receive_packet returns
        // NeedMore.
        match enc.receive_packet() {
            Err(Error::NeedMore) => {}
            Err(_) => return,
            Ok(_) => panic!(
                "encoder queue not drained: extra packet emerged after \
                 {} expected frames (codec={codec_id})",
                frames.len()
            ),
        }
    }

    // ── Phase 2: decode every packet, checking per-sample equivalence
    // against the in-target baseline + pts propagation.
    for (idx, (pts, encoded, pcm_bytes)) in produced.iter().enumerate() {
        use oxideav_core::TimeBase;
        let mut pkt =
            oxideav_core::Packet::new(0, TimeBase::new(1, sample_rate as i64), encoded.clone());
        pkt.pts = Some(*pts);
        if dec.send_packet(&pkt).is_err() {
            return;
        }
        let af = match dec.receive_frame() {
            Ok(Frame::Audio(a)) => a,
            _ => return,
        };
        assert_eq!(
            af.pts,
            Some(*pts),
            "decoder pts drift on frame {idx}: sent {:?}, got {:?}",
            Some(*pts),
            af.pts,
        );
        let got = af.data.first().cloned().unwrap_or_default();
        assert_eq!(
            got.len(),
            pcm_bytes.len(),
            "decoded PCM length {} ≠ original input length {} on frame {idx} (codec={codec_id})",
            got.len(),
            pcm_bytes.len(),
        );
        let mut decoded_samples: Vec<i16> = Vec::with_capacity(got.len() / 2);
        for chunk in got.chunks_exact(2) {
            decoded_samples.push(i16::from_le_bytes([chunk[0], chunk[1]]));
        }
        let expected = expected_decode(pcm_bytes, use_alaw);
        assert_eq!(
            decoded_samples, expected,
            "streaming roundtrip diverges from per-sample baseline on \
             frame {idx} (codec={codec_id} ch={channels} rate={sample_rate})"
        );

        // Mid-stream-flush: after every other packet, flush the
        // decoder. The decoder treats flush as a state-machine marker
        // — subsequent `send_packet` must still succeed and produce
        // a correct frame.
        if mid_stream_flush && (idx % 2 == 1) {
            let _ = dec.flush();
            // After flush + drained pending, the decoder returns Eof
            // on receive_frame (no packet queued). But we already
            // drained the just-sent packet above, so a follow-up
            // receive_frame with no pending packet returns Eof.
            match dec.receive_frame() {
                Err(Error::Eof) => {}
                Err(_) => return,
                Ok(_) => panic!(
                    "decoder emitted spurious frame after flush + drain \
                     on frame {idx} (codec={codec_id})"
                ),
            }
            // Rebuild a fresh decoder so subsequent frames in the
            // session continue to validate. (The spec-level guarantee
            // is that flush is terminal; we test the terminal
            // behaviour here and then continue the session on a fresh
            // instance, which is the real-world recovery pattern.)
            dec = match make_dec(&params) {
                Ok(d) => d,
                Err(_) => return,
            };
        }
    }

    // ── Phase 3: post-loop EOF semantics. With no pending packet and
    // no flush, receive_frame must report NeedMore.
    match dec.receive_frame() {
        Err(Error::NeedMore) => {}
        Err(Error::Eof) => {
            // Permissible only if the loop ended on a flushed instance
            // — which happens when mid_stream_flush flipped on the last
            // odd index and we rebuilt above; in that case a fresh
            // decoder reports NeedMore, so reaching Eof here would be
            // a bug. But since we always rebuild on mid_stream_flush,
            // Eof here is unexpected; treat as a fuzzer-detectable
            // contract slip.
            panic!("fresh decoder reported Eof without flush (codec={codec_id})");
        }
        Err(_) => return,
        Ok(_) => panic!(
            "decoder produced spurious frame after stream drain \
             (codec={codec_id})"
        ),
    }
    // After flush(), receive_frame must report Eof.
    let _ = dec.flush();
    match dec.receive_frame() {
        Err(Error::Eof) => {}
        Err(_) => return,
        Ok(_) => panic!(
            "decoder produced spurious frame after flush + drain \
             (codec={codec_id})"
        ),
    }
});
