//! G.711 is stateless per-sample, so arbitrary interleaved channel
//! counts round-trip cleanly. These tests drive the `Decoder` / `Encoder`
//! trait surface through the registry to prove it end-to-end for both
//! laws and a range of channel counts.

use oxideav_core::CodecRegistry;
use oxideav_core::{AudioFrame, CodecId, CodecParameters, Frame, Packet, SampleFormat, TimeBase};

fn params(codec: &str, channels: u16, sample_rate: u32) -> CodecParameters {
    let mut p = CodecParameters::audio(CodecId::new(codec));
    p.sample_rate = Some(sample_rate);
    p.channels = Some(channels);
    p.sample_format = Some(SampleFormat::S16);
    p
}

fn make_frame(samples_per_channel: usize, channels: u16, sample_rate: u32) -> (Frame, Vec<i16>) {
    // Deterministic signal: linear ramp per channel with a channel-dependent
    // offset so every interleaved slot is distinct.
    let total = samples_per_channel * channels as usize;
    let mut interleaved = Vec::with_capacity(total);
    for i in 0..samples_per_channel {
        for ch in 0..channels as usize {
            let v = ((i as i32 * 7) - 8000 + (ch as i32 * 123)) as i16;
            interleaved.push(v);
        }
    }
    let mut bytes = Vec::with_capacity(total * 2);
    for s in &interleaved {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    let frame = Frame::Audio(AudioFrame {
        format: SampleFormat::S16,
        channels,
        sample_rate,
        samples: samples_per_channel as u32,
        pts: Some(0),
        time_base: TimeBase::new(1, sample_rate as i64),
        data: vec![bytes],
    });
    (frame, interleaved)
}

fn roundtrip(codec: &str, channels: u16) {
    let mut reg = CodecRegistry::new();
    oxideav_g711::register(&mut reg);
    let p = params(codec, channels, 8_000);

    let mut enc = reg.make_encoder(&p).expect("make_encoder");
    let mut dec = reg.make_decoder(&p).expect("make_decoder");

    let samples_per_channel = 160; // 20 ms @ 8 kHz
    let (frame, input) = make_frame(samples_per_channel, channels, 8_000);

    enc.send_frame(&frame).unwrap();
    let pkt = enc.receive_packet().unwrap();
    // One G.711 byte per input S16 sample, interleaved the same way.
    assert_eq!(pkt.data.len(), samples_per_channel * channels as usize);

    dec.send_packet(&pkt).unwrap();
    let Frame::Audio(af) = dec.receive_frame().unwrap() else {
        panic!("expected audio frame");
    };
    assert_eq!(af.channels, channels);
    assert_eq!(af.samples as usize, samples_per_channel);
    assert_eq!(af.data.len(), 1);
    assert_eq!(
        af.data[0].len(),
        samples_per_channel * channels as usize * 2
    );

    // Every decoded sample must be the deterministic quantisation of
    // the corresponding input sample: `decode(encode(x))`. Channels
    // simply interleave this operation, they do not interact.
    let decoded: Vec<i16> = af.data[0]
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect();
    assert_eq!(decoded.len(), input.len());
    for (i, (&x, &y)) in input.iter().zip(decoded.iter()).enumerate() {
        let re_quant = match codec {
            "pcm_mulaw" => {
                oxideav_g711::mulaw::decode_sample(oxideav_g711::mulaw::encode_sample(x))
            }
            "pcm_alaw" => oxideav_g711::alaw::decode_sample(oxideav_g711::alaw::encode_sample(x)),
            _ => unreachable!(),
        };
        assert_eq!(
            y, re_quant,
            "codec {codec} ch={channels} sample_index={i} input={x} expected={re_quant} got={y}"
        );
    }
}

#[test]
fn mulaw_stereo_roundtrip() {
    roundtrip("pcm_mulaw", 2);
}

#[test]
fn mulaw_5_1_roundtrip() {
    roundtrip("pcm_mulaw", 6);
}

#[test]
fn mulaw_7_1_roundtrip() {
    roundtrip("pcm_mulaw", 8);
}

#[test]
fn alaw_stereo_roundtrip() {
    roundtrip("pcm_alaw", 2);
}

#[test]
fn alaw_5_1_roundtrip() {
    roundtrip("pcm_alaw", 6);
}

#[test]
fn alaw_7_1_roundtrip() {
    roundtrip("pcm_alaw", 8);
}

#[test]
fn mulaw_mono_still_works_through_registry() {
    roundtrip("pcm_mulaw", 1);
}

#[test]
fn alaw_mono_still_works_through_registry() {
    roundtrip("pcm_alaw", 1);
}

/// Odd sample rates are fine — the companding math is rate-independent.
#[test]
fn mulaw_custom_sample_rate_roundtrip() {
    let mut reg = CodecRegistry::new();
    oxideav_g711::register(&mut reg);
    let p = params("pcm_mulaw", 2, 16_000);
    let mut enc = reg.make_encoder(&p).unwrap();
    let mut dec = reg.make_decoder(&p).unwrap();

    let (frame, _) = make_frame(320, 2, 16_000);
    enc.send_frame(&frame).unwrap();
    let pkt = enc.receive_packet().unwrap();
    dec.send_packet(&pkt).unwrap();
    let Frame::Audio(af) = dec.receive_frame().unwrap() else {
        unreachable!()
    };
    assert_eq!(af.sample_rate, 16_000);
    assert_eq!(af.channels, 2);
    assert_eq!(af.samples, 320);
}

#[test]
fn decoder_rejects_partial_channel_packet() {
    let mut reg = CodecRegistry::new();
    oxideav_g711::register(&mut reg);
    let p = params("pcm_mulaw", 2, 8_000);
    let mut dec = reg.make_decoder(&p).unwrap();
    // 5 bytes with 2 channels → not divisible, must error.
    let pkt = Packet::new(0, TimeBase::new(1, 8_000), vec![0xFFu8; 5]);
    dec.send_packet(&pkt).unwrap();
    let err = dec.receive_frame().unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("multiple of channel count"),
        "unexpected error: {msg}"
    );
}
