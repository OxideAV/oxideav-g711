//! µ-law (ITU-T G.711 §3) codec — single-sample conversion helpers plus
//! [`UlawDecoder`] / [`UlawEncoder`] implementing the `oxideav_codec`
//! traits. Each encoded byte carries exactly one S16 PCM sample.

use oxideav_core::{
    AudioFrame, CodecId, CodecParameters, Error, Frame, MediaType, Packet, Result, SampleFormat,
    TimeBase,
};
use oxideav_core::{Decoder, Encoder};
use std::collections::VecDeque;

use crate::tables::{mulaw_encode_arith, MULAW_DECODE, MULAW_DECODE_LE, MULAW_ENCODE};

/// Decode one µ-law byte to a linear S16 sample. Direct LUT lookup
/// against [`MULAW_DECODE`].
#[inline]
pub fn decode_sample(byte: u8) -> i16 {
    MULAW_DECODE[byte as usize]
}

/// Encode one S16 sample as a µ-law byte (ITU-T G.711 §3). Direct LUT
/// lookup against [`MULAW_ENCODE`] — every entry in that table was
/// produced at compile time by [`encode_sample_arith`] (the arithmetic
/// implementation of §3.2), so the table is bit-exact-by-construction
/// relative to the spec formula and the runtime hot path is a single
/// 64 KiB-LUT load instead of bias + segment search + mantissa shift +
/// on-wire inversion.
///
/// If you want the formula instead of the table — e.g. to assert
/// equality with the LUT in a test — call [`encode_sample_arith`].
#[inline]
pub fn encode_sample(sample: i16) -> u8 {
    MULAW_ENCODE[sample as u16 as usize]
}

/// Arithmetic µ-law encode (ITU-T G.711 §3.2). Same result as
/// [`encode_sample`] but computed from the formula every call instead of
/// loaded from [`MULAW_ENCODE`]:
///
/// 1. Extract sign; work with absolute magnitude clamped to 0..=32635 (the
///    largest µ-law-representable amplitude after bias).
/// 2. Add the bias (132).
/// 3. Find the segment (0..=7) as `exp = position_of_highest_set_bit - 7`.
/// 4. The 4-bit mantissa is the next four bits below the segment bit.
/// 5. Compose S|E|M, then complement every bit for the on-wire encoding.
///
/// Kept public so callers that legitimately do not want the 64 KiB
/// static LUT linked into their binary (e.g. wasm size-sensitive
/// callers, or a test that wants a second source of truth) can still
/// reach the spec formula directly.
#[inline]
pub fn encode_sample_arith(sample: i16) -> u8 {
    mulaw_encode_arith(sample)
}

// -------------- decoder --------------

/// Build a boxed [`Decoder`] for G.711 µ-law with the given codec
/// parameters. This is the direct-factory entry point — the
/// [`crate::register`] / [`crate::register_codecs`] paths install
/// this same function into the codec registry, so callers who don't
/// want a registry lookup may invoke this directly with `params`
/// they constructed manually.
pub fn make_decoder(params: &CodecParameters) -> Result<Box<dyn Decoder>> {
    let channels = params.channels.unwrap_or(1);
    if channels == 0 {
        return Err(Error::unsupported(
            "G.711 µ-law decoder: channel count must be >= 1",
        ));
    }
    Ok(Box::new(UlawDecoder {
        codec_id: params.codec_id.clone(),
        channels,
        pending: None,
        eof: false,
    }))
}

pub struct UlawDecoder {
    codec_id: CodecId,
    channels: u16,
    pending: Option<Packet>,
    eof: bool,
}

impl Decoder for UlawDecoder {
    fn codec_id(&self) -> &CodecId {
        &self.codec_id
    }

    fn send_packet(&mut self, packet: &Packet) -> Result<()> {
        if self.pending.is_some() {
            return Err(Error::other(
                "G.711 µ-law decoder: call receive_frame before sending another packet",
            ));
        }
        self.pending = Some(packet.clone());
        Ok(())
    }

    fn receive_frame(&mut self) -> Result<Frame> {
        let Some(pkt) = self.pending.take() else {
            return if self.eof {
                Err(Error::Eof)
            } else {
                Err(Error::NeedMore)
            };
        };
        if pkt.data.is_empty() {
            // Nothing to decode — emit an empty frame rather than erroring.
            return Ok(Frame::Audio(AudioFrame {
                samples: 0,
                pts: pkt.pts,
                data: vec![Vec::new()],
            }));
        }
        let ch = self.channels as usize;
        if pkt.data.len() % ch != 0 {
            return Err(Error::invalid(format!(
                "G.711 µ-law decoder: packet length {} is not a multiple of channel count {ch}",
                pkt.data.len()
            )));
        }
        let samples_per_channel = pkt.data.len() / ch;
        // r289 hot loop: index the pre-serialized little-endian
        // byte-pair LUT and store the two bytes with one
        // `copy_from_slice`, replacing the r236 `[i16; 256]` load +
        // per-iter `i16::to_le_bytes()` recomputation + two scalar
        // stores. The LE LUT is byte-identical to `MULAW_DECODE` (it is
        // built from it at compile time), so output is unchanged; the
        // store collapses to a fixed-width 2-byte copy from `.rodata`.
        // Measured ~+24% on the 8ch/48k decode row (3.56 → 4.68 GiB/s).
        let mut out = vec![0u8; pkt.data.len() * 2];
        for (&b, dst) in pkt.data.iter().zip(out.chunks_exact_mut(2)) {
            dst.copy_from_slice(&MULAW_DECODE_LE[b as usize]);
        }
        Ok(Frame::Audio(AudioFrame {
            samples: samples_per_channel as u32,
            pts: pkt.pts,
            data: vec![out],
        }))
    }

    fn flush(&mut self) -> Result<()> {
        self.eof = true;
        Ok(())
    }
}

// -------------- encoder --------------

/// Build a boxed [`Encoder`] for G.711 µ-law with the given codec
/// parameters. Direct-factory counterpart to [`make_decoder`].
pub fn make_encoder(params: &CodecParameters) -> Result<Box<dyn Encoder>> {
    let channels = params.channels.unwrap_or(1);
    if channels == 0 {
        return Err(Error::unsupported(
            "G.711 µ-law encoder: channel count must be >= 1",
        ));
    }
    let sample_format = params.sample_format.unwrap_or(SampleFormat::S16);
    if sample_format != SampleFormat::S16 {
        return Err(Error::unsupported(format!(
            "G.711 µ-law encoder: input sample format {sample_format:?} not supported (need S16)"
        )));
    }
    let sample_rate = params.sample_rate.unwrap_or(8_000);
    let mut output = params.clone();
    output.media_type = MediaType::Audio;
    output.sample_format = Some(SampleFormat::S16);
    output.channels = Some(channels);
    output.sample_rate = Some(sample_rate);
    output.codec_id = params.codec_id.clone();
    Ok(Box::new(UlawEncoder {
        output,
        time_base: TimeBase::new(1, sample_rate as i64),
        queue: VecDeque::new(),
    }))
}

pub struct UlawEncoder {
    output: CodecParameters,
    time_base: TimeBase,
    queue: VecDeque<Packet>,
}

impl Encoder for UlawEncoder {
    fn codec_id(&self) -> &CodecId {
        &self.output.codec_id
    }

    fn output_params(&self) -> &CodecParameters {
        &self.output
    }

    fn send_frame(&mut self, frame: &Frame) -> Result<()> {
        let Frame::Audio(a) = frame else {
            return Err(Error::invalid("G.711 µ-law encoder: audio frames only"));
        };
        // Stream-level format/channels/sample_rate are now contractual via
        // CodecParameters and validated at construction; per-frame checks
        // disappear with the slim AudioFrame shape.
        let bytes = a
            .data
            .first()
            .ok_or_else(|| Error::invalid("G.711 µ-law encoder: empty frame"))?;
        if bytes.len() % 2 != 0 {
            return Err(Error::invalid("G.711 µ-law encoder: odd byte count"));
        }
        let n = bytes.len() / 2;
        // r236 hot loop: pre-size the output, then zip the LE-S16 source
        // pairs with `chunks_exact_mut(1)` over the destination so the
        // codegen sees a direct slice-load + slice-store pair without
        // a `Vec::push` bounds-check chain. `chunks_exact(2)` on the
        // source still hands us the two LE halves; the trailing
        // `chunks_exact(2).remainder()` is empty because `bytes.len()
        // % 2 == 0` was just validated above.
        let mut out = vec![0u8; n];
        for (src, dst) in bytes.chunks_exact(2).zip(out.iter_mut()) {
            let s = i16::from_le_bytes([src[0], src[1]]);
            *dst = MULAW_ENCODE[s as u16 as usize];
        }
        let mut pkt = Packet::new(0, self.time_base, out);
        pkt.pts = a.pts;
        pkt.dts = a.pts;
        pkt.duration = Some(a.samples as i64);
        pkt.flags.keyframe = true;
        self.queue.push_back(pkt);
        Ok(())
    }

    fn receive_packet(&mut self) -> Result<Packet> {
        self.queue.pop_front().ok_or(Error::NeedMore)
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}
