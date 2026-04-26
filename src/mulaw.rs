//! µ-law (ITU-T G.711 §3) codec — single-sample conversion helpers plus
//! [`UlawDecoder`] / [`UlawEncoder`] implementing the `oxideav_codec`
//! traits. Each encoded byte carries exactly one S16 PCM sample.

use oxideav_core::{
    AudioFrame, CodecId, CodecParameters, Error, Frame, MediaType, Packet, Result, SampleFormat,
    TimeBase,
};
use oxideav_core::{Decoder, Encoder};
use std::collections::VecDeque;

use crate::tables::{MULAW_BIAS, MULAW_DECODE};

/// Decode one µ-law byte to a linear S16 sample. Direct LUT lookup.
#[inline]
pub fn decode_sample(byte: u8) -> i16 {
    MULAW_DECODE[byte as usize]
}

/// Encode one S16 sample as a µ-law byte (ITU-T G.711 §3, arithmetic form).
///
/// Algorithm:
/// 1. Extract sign; work with absolute magnitude clamped to 0..=32635 (the
///    largest µ-law-representable amplitude after bias).
/// 2. Add the bias (132).
/// 3. Find the segment (0..=7) as `exp = position_of_highest_set_bit - 7`.
/// 4. The 4-bit mantissa is the next four bits below the segment bit.
/// 5. Compose S|E|M, then complement every bit for the on-wire encoding.
#[inline]
pub fn encode_sample(sample: i16) -> u8 {
    // Clip to µ-law range to avoid overflow of (abs + bias).
    let mut mag: i32 = sample as i32;
    let sign_bit: u8 = if mag < 0 { 0x80 } else { 0 };
    if mag < 0 {
        mag = -mag;
    }
    // Clamp. Values at or above 32635 all collapse into the topmost code.
    if mag > 32635 {
        mag = 32635;
    }
    mag += MULAW_BIAS;

    // Find segment: position of the topmost set bit minus 7.
    // Since (mag + bias) fits in 15 bits with max ~32767, the topmost bit
    // is at most 14.
    let mut seg: u32 = 7;
    let mut mask: i32 = 0x4000;
    while seg > 0 && (mag & mask) == 0 {
        seg -= 1;
        mask >>= 1;
    }

    // Mantissa: the four bits immediately below the segment's top bit.
    let mantissa = ((mag >> (seg + 3)) & 0x0F) as u8;
    let byte = sign_bit | ((seg as u8) << 4) | mantissa;
    // Invert on-wire.
    !byte
}

// -------------- decoder --------------

pub(crate) fn make_decoder(params: &CodecParameters) -> Result<Box<dyn Decoder>> {
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
        let mut out = Vec::with_capacity(pkt.data.len() * 2);
        for &b in &pkt.data {
            let s = decode_sample(b);
            out.extend_from_slice(&s.to_le_bytes());
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

pub(crate) fn make_encoder(params: &CodecParameters) -> Result<Box<dyn Encoder>> {
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
        let mut out = Vec::with_capacity(bytes.len() / 2);
        for chunk in bytes.chunks_exact(2) {
            let s = i16::from_le_bytes([chunk[0], chunk[1]]);
            out.push(encode_sample(s));
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
