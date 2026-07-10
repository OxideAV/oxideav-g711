//! A-law (ITU-T G.711 §2) codec — single-sample conversion helpers plus
//! [`AlawDecoder`] / [`AlawEncoder`] implementing the `oxideav_codec`
//! traits. Each encoded byte carries exactly one S16 PCM sample.

use oxideav_core::{
    AudioFrame, CodecId, CodecParameters, Error, Frame, MediaType, Packet, Result, SampleFormat,
    TimeBase,
};
use oxideav_core::{Decoder, Encoder};
use std::collections::VecDeque;

use crate::tables::{alaw_encode_arith, ALAW_DECODE, ALAW_DECODE_LE, ALAW_ENCODE};

/// Decode one A-law byte to a linear S16 sample. Direct LUT lookup
/// against [`ALAW_DECODE`].
#[inline]
pub fn decode_sample(byte: u8) -> i16 {
    ALAW_DECODE[byte as usize]
}

/// Encode one S16 sample as an A-law byte (ITU-T G.711 §2). Direct LUT
/// lookup against [`ALAW_ENCODE`] — every entry in that table was
/// produced at compile time by [`encode_sample_arith`] (the arithmetic
/// implementation of §2), so the table is bit-exact-by-construction
/// relative to the spec formula and the runtime hot path is a single
/// 64 KiB-LUT load instead of segment branch + mantissa shift + XOR.
///
/// If you want the formula instead of the table — e.g. to assert
/// equality with the LUT in a test — call [`encode_sample_arith`].
#[inline]
pub fn encode_sample(sample: i16) -> u8 {
    ALAW_ENCODE[sample as u16 as usize]
}

/// Arithmetic A-law encode (ITU-T G.711 §2). Same result as
/// [`encode_sample`] but computed from the formula every call instead of
/// loaded from [`ALAW_ENCODE`]:
///
/// 1. Extract sign; work with absolute magnitude (clamped to the largest
///    representable A-law level = 32256).
/// 2. Find the segment — the position of the topmost set bit of the
///    magnitude. Segments 1..=7 correspond to the magnitude falling into
///    the ranges `[256..512), [512..1024), ..., [16384..32768)`.
/// 3. Extract the 4-bit mantissa from the bits immediately below the
///    segment bit.
/// 4. Compose S|E|M, then XOR with 0x55 for the on-wire alternate-bit
///    inversion.
///
/// Kept public so callers that legitimately do not want the 64 KiB
/// static LUT linked into their binary, or a test that wants a second
/// source of truth, can reach the spec formula directly.
#[inline]
pub fn encode_sample_arith(sample: i16) -> u8 {
    alaw_encode_arith(sample)
}

// -------------- batch (slice) helpers --------------
//
// Allocation-free bulk conversion over caller-provided buffers. Each
// helper is the exact loop the trait surface runs (the decoder /
// encoder below delegate to `decode_slice_to_le_bytes` /
// `encode_slice_from_le_bytes`), exposed so callers that already own
// their buffers — a jitter buffer, a ring buffer, an FFI boundary —
// can convert without constructing a `Packet` / `Frame` pair or
// paying a per-call `Vec` allocation. Every output value is defined
// per-sample by the corresponding single-sample function; the slice
// forms add no state and no rounding of their own.

/// Decode a slice of A-law bytes into a caller-provided S16 buffer.
/// `output[i]` is exactly [`decode_sample`]`(input[i])` for every `i`.
///
/// # Panics
///
/// Panics if `input.len() != output.len()`.
pub fn decode_slice(input: &[u8], output: &mut [i16]) {
    assert_eq!(
        input.len(),
        output.len(),
        "G.711 A-law decode_slice: input and output lengths must match"
    );
    for (&b, dst) in input.iter().zip(output.iter_mut()) {
        *dst = ALAW_DECODE[b as usize];
    }
}

/// Decode a slice of A-law bytes into a caller-provided buffer of
/// **little-endian S16 byte pairs** — `output[2*i..2*i+2]` is exactly
/// [`decode_sample`]`(input[i]).to_le_bytes()`. This is the loop the
/// trait-surface decoder ([`AlawDecoder`]) runs; it indexes the
/// pre-serialized [`ALAW_DECODE_LE`] table so each sample is a single
/// fixed-width 2-byte copy from `.rodata`.
///
/// # Panics
///
/// Panics if `output.len() != input.len() * 2`.
pub fn decode_slice_to_le_bytes(input: &[u8], output: &mut [u8]) {
    assert_eq!(
        input.len() * 2,
        output.len(),
        "G.711 A-law decode_slice_to_le_bytes: output must be exactly 2 bytes per input byte"
    );
    for (&b, dst) in input.iter().zip(output.chunks_exact_mut(2)) {
        dst.copy_from_slice(&ALAW_DECODE_LE[b as usize]);
    }
}

/// Encode a slice of S16 samples into a caller-provided A-law byte
/// buffer. `output[i]` is exactly [`encode_sample`]`(input[i])` for
/// every `i`.
///
/// # Panics
///
/// Panics if `input.len() != output.len()`.
pub fn encode_slice(input: &[i16], output: &mut [u8]) {
    assert_eq!(
        input.len(),
        output.len(),
        "G.711 A-law encode_slice: input and output lengths must match"
    );
    for (&s, dst) in input.iter().zip(output.iter_mut()) {
        *dst = ALAW_ENCODE[s as u16 as usize];
    }
}

/// Encode a slice of **little-endian S16 byte pairs** into a
/// caller-provided A-law byte buffer — `output[i]` is exactly
/// [`encode_sample`] applied to `i16::from_le_bytes(input[2*i..2*i+2])`.
/// This is the loop the trait-surface encoder ([`AlawEncoder`]) runs on
/// each frame's raw byte plane.
///
/// # Panics
///
/// Panics if `input.len() != output.len() * 2` (which also enforces
/// that `input.len()` is even).
pub fn encode_slice_from_le_bytes(input: &[u8], output: &mut [u8]) {
    assert_eq!(
        input.len(),
        output.len() * 2,
        "G.711 A-law encode_slice_from_le_bytes: input must be exactly 2 bytes per output byte"
    );
    for (src, dst) in input.chunks_exact(2).zip(output.iter_mut()) {
        let s = i16::from_le_bytes([src[0], src[1]]);
        *dst = ALAW_ENCODE[s as u16 as usize];
    }
}

// -------------- decoder --------------

/// Build a boxed [`Decoder`] for G.711 A-law with the given codec
/// parameters. This is the direct-factory entry point — the
/// [`crate::register`] / [`crate::register_codecs`] paths install
/// this same function into the codec registry, so callers who don't
/// want a registry lookup may invoke this directly with `params`
/// they constructed manually.
pub fn make_decoder(params: &CodecParameters) -> Result<Box<dyn Decoder>> {
    let channels = params.channels.unwrap_or(1);
    if channels == 0 {
        return Err(Error::unsupported(
            "G.711 A-law decoder: channel count must be >= 1",
        ));
    }
    Ok(Box::new(AlawDecoder {
        codec_id: params.codec_id.clone(),
        channels,
        pending: None,
        eof: false,
    }))
}

pub struct AlawDecoder {
    codec_id: CodecId,
    channels: u16,
    pending: Option<Packet>,
    eof: bool,
}

impl Decoder for AlawDecoder {
    fn codec_id(&self) -> &CodecId {
        &self.codec_id
    }

    fn send_packet(&mut self, packet: &Packet) -> Result<()> {
        if self.pending.is_some() {
            return Err(Error::other(
                "G.711 A-law decoder: call receive_frame before sending another packet",
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
            return Ok(Frame::Audio(AudioFrame {
                samples: 0,
                pts: pkt.pts,
                data: vec![Vec::new()],
            }));
        }
        let ch = self.channels as usize;
        if pkt.data.len() % ch != 0 {
            return Err(Error::invalid(format!(
                "G.711 A-law decoder: packet length {} is not a multiple of channel count {ch}",
                pkt.data.len()
            )));
        }
        let samples_per_channel = pkt.data.len() / ch;
        // r289 hot loop, extracted to the public batch helper in r406:
        // index the pre-serialized little-endian byte-pair LUT and store
        // the two bytes with one `copy_from_slice`, replacing the r236
        // `[i16; 256]` load + per-iter `i16::to_le_bytes()` recomputation
        // + two scalar stores. The LE LUT is byte-identical to
        // `ALAW_DECODE` (it is built from it at compile time), so output
        // is unchanged; the store collapses to a fixed-width 2-byte copy
        // from `.rodata`. Measured ~+24% on the 8ch/48k decode row. The
        // length invariant the helper asserts holds by construction
        // (`out` is sized right here).
        let mut out = vec![0u8; pkt.data.len() * 2];
        decode_slice_to_le_bytes(&pkt.data, &mut out);
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

/// Build a boxed [`Encoder`] for G.711 A-law with the given codec
/// parameters. Direct-factory counterpart to [`make_decoder`].
pub fn make_encoder(params: &CodecParameters) -> Result<Box<dyn Encoder>> {
    let channels = params.channels.unwrap_or(1);
    if channels == 0 {
        return Err(Error::unsupported(
            "G.711 A-law encoder: channel count must be >= 1",
        ));
    }
    let sample_format = params.sample_format.unwrap_or(SampleFormat::S16);
    if sample_format != SampleFormat::S16 {
        return Err(Error::unsupported(format!(
            "G.711 A-law encoder: input sample format {sample_format:?} not supported (need S16)"
        )));
    }
    let sample_rate = params.sample_rate.unwrap_or(8_000);
    let mut output = params.clone();
    output.media_type = MediaType::Audio;
    output.sample_format = Some(SampleFormat::S16);
    output.channels = Some(channels);
    output.sample_rate = Some(sample_rate);
    output.codec_id = params.codec_id.clone();
    Ok(Box::new(AlawEncoder {
        output,
        time_base: TimeBase::new(1, sample_rate as i64),
        queue: VecDeque::new(),
    }))
}

pub struct AlawEncoder {
    output: CodecParameters,
    time_base: TimeBase,
    queue: VecDeque<Packet>,
}

impl Encoder for AlawEncoder {
    fn codec_id(&self) -> &CodecId {
        &self.output.codec_id
    }

    fn output_params(&self) -> &CodecParameters {
        &self.output
    }

    fn send_frame(&mut self, frame: &Frame) -> Result<()> {
        let Frame::Audio(a) = frame else {
            return Err(Error::invalid("G.711 A-law encoder: audio frames only"));
        };
        // Stream-level format/channels/sample_rate are now contractual via
        // CodecParameters and validated at construction; per-frame checks
        // disappear with the slim AudioFrame shape.
        let bytes = a
            .data
            .first()
            .ok_or_else(|| Error::invalid("G.711 A-law encoder: empty frame"))?;
        if bytes.len() % 2 != 0 {
            return Err(Error::invalid("G.711 A-law encoder: odd byte count"));
        }
        let n = bytes.len() / 2;
        // r236 hot loop, extracted to the public batch helper in r406:
        // pre-size the output, then zip the LE-S16 source pairs against
        // the destination so the codegen sees a single slice-load +
        // slice-store pair with no `Vec::push` bounds-check chain. The
        // helper's length assert holds by construction — `out` is sized
        // `bytes.len() / 2` and `bytes.len() % 2 == 0` was just
        // validated above.
        let mut out = vec![0u8; n];
        encode_slice_from_le_bytes(bytes, &mut out);
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
