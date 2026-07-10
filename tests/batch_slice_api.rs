//! Exhaustive equivalence suite for the r406 batch (slice) helpers.
//!
//! The batch API's whole contract is *per-sample definitional
//! equality*: every helper is specified as "output element `i` is
//! exactly the corresponding single-sample function applied to input
//! element `i`". Both G.711 domains are small enough to enumerate
//! completely — 256 wire codewords on the decode side, 65 536 S16
//! samples on the encode side — so this suite pins the contract over
//! the **entire** domain of every helper rather than a sample of it:
//!
//! - `decode_slice` / `decode_slice_to_le_bytes` over all 256 bytes,
//!   both laws, as one slice and re-chunked at adversarial lengths
//!   (1, 3, 7, 251) so no chunk boundary can hide positional state;
//! - `encode_slice` / `encode_slice_from_le_bytes` over all 65 536
//!   samples, both laws, same re-chunking;
//! - `encode_slice_zero_suppress` (µ-law §3.2) over all 65 536
//!   samples — never emits the all-zero octet, rewrites exactly the
//!   codewords the plain encoder maps to it, leaves every other
//!   position byte-identical;
//! - trait-surface delegation: the packet a `make_encoder` encoder
//!   emits and the frame a `make_decoder` decoder emits are
//!   byte-identical to the corresponding batch helper's output over
//!   full-domain inputs (the decoder / encoder hot loops *are* the
//!   helpers since r406 — this pins the delegation against future
//!   drift);
//! - empty slices are a no-op; mismatched lengths panic.
//!
//! No fixtures, no `docs/` reads — the single-sample functions
//! (themselves pinned bit-exact to the §2 / §3 formulas by
//! `bit_exact_reference.rs`) are the oracle.

use oxideav_core::{CodecId, CodecParameters, Frame, Packet, SampleFormat, TimeBase};
use oxideav_g711::{alaw, mulaw};

/// All 256 wire codewords in order.
fn all_bytes() -> Vec<u8> {
    (0u8..=255).collect()
}

/// All 65 536 S16 samples in `i16` iteration order (`i16::MIN..=MAX`).
fn all_samples() -> Vec<i16> {
    (i16::MIN..=i16::MAX).collect()
}

/// Chunk lengths used to re-run every slice helper over the same
/// domain split at awkward boundaries. Primes (and 1) so chunk edges
/// never align with segment boundaries of the companding curve.
const CHUNK_LENGTHS: &[usize] = &[1, 3, 7, 251];

// ---------------------------------------------------------------
// decode_slice — full 256-codeword domain, both laws
// ---------------------------------------------------------------

#[test]
fn mulaw_decode_slice_matches_per_sample_over_full_domain() {
    let input = all_bytes();
    let mut out = vec![0i16; input.len()];
    mulaw::decode_slice(&input, &mut out);
    for (i, (&b, &got)) in input.iter().zip(out.iter()).enumerate() {
        assert_eq!(
            got,
            mulaw::decode_sample(b),
            "µ-law decode_slice diverged from decode_sample at index {i} (byte {b:#04x})"
        );
    }
    // Re-chunked: same domain, adversarial slice boundaries.
    for &len in CHUNK_LENGTHS {
        for chunk in input.chunks(len) {
            let mut part = vec![0i16; chunk.len()];
            mulaw::decode_slice(chunk, &mut part);
            for (&b, &got) in chunk.iter().zip(part.iter()) {
                assert_eq!(got, mulaw::decode_sample(b));
            }
        }
    }
}

#[test]
fn alaw_decode_slice_matches_per_sample_over_full_domain() {
    let input = all_bytes();
    let mut out = vec![0i16; input.len()];
    alaw::decode_slice(&input, &mut out);
    for (i, (&b, &got)) in input.iter().zip(out.iter()).enumerate() {
        assert_eq!(
            got,
            alaw::decode_sample(b),
            "A-law decode_slice diverged from decode_sample at index {i} (byte {b:#04x})"
        );
    }
    for &len in CHUNK_LENGTHS {
        for chunk in input.chunks(len) {
            let mut part = vec![0i16; chunk.len()];
            alaw::decode_slice(chunk, &mut part);
            for (&b, &got) in chunk.iter().zip(part.iter()) {
                assert_eq!(got, alaw::decode_sample(b));
            }
        }
    }
}

// ---------------------------------------------------------------
// decode_slice_to_le_bytes — full domain, byte-pair layout
// ---------------------------------------------------------------

#[test]
fn mulaw_decode_slice_to_le_bytes_matches_per_sample_over_full_domain() {
    let input = all_bytes();
    let mut out = vec![0u8; input.len() * 2];
    mulaw::decode_slice_to_le_bytes(&input, &mut out);
    for (i, (&b, pair)) in input.iter().zip(out.chunks_exact(2)).enumerate() {
        assert_eq!(
            pair,
            mulaw::decode_sample(b).to_le_bytes(),
            "µ-law decode_slice_to_le_bytes diverged at index {i} (byte {b:#04x})"
        );
    }
}

#[test]
fn alaw_decode_slice_to_le_bytes_matches_per_sample_over_full_domain() {
    let input = all_bytes();
    let mut out = vec![0u8; input.len() * 2];
    alaw::decode_slice_to_le_bytes(&input, &mut out);
    for (i, (&b, pair)) in input.iter().zip(out.chunks_exact(2)).enumerate() {
        assert_eq!(
            pair,
            alaw::decode_sample(b).to_le_bytes(),
            "A-law decode_slice_to_le_bytes diverged at index {i} (byte {b:#04x})"
        );
    }
}

// ---------------------------------------------------------------
// encode_slice — full 65 536-sample domain, both laws
// ---------------------------------------------------------------

#[test]
fn mulaw_encode_slice_matches_per_sample_over_full_domain() {
    let input = all_samples();
    let mut out = vec![0u8; input.len()];
    mulaw::encode_slice(&input, &mut out);
    for (i, (&s, &got)) in input.iter().zip(out.iter()).enumerate() {
        assert_eq!(
            got,
            mulaw::encode_sample(s),
            "µ-law encode_slice diverged from encode_sample at index {i} (sample {s})"
        );
    }
    for &len in CHUNK_LENGTHS {
        for chunk in input.chunks(len) {
            let mut part = vec![0u8; chunk.len()];
            mulaw::encode_slice(chunk, &mut part);
            for (&s, &got) in chunk.iter().zip(part.iter()) {
                assert_eq!(got, mulaw::encode_sample(s));
            }
        }
    }
}

#[test]
fn alaw_encode_slice_matches_per_sample_over_full_domain() {
    let input = all_samples();
    let mut out = vec![0u8; input.len()];
    alaw::encode_slice(&input, &mut out);
    for (i, (&s, &got)) in input.iter().zip(out.iter()).enumerate() {
        assert_eq!(
            got,
            alaw::encode_sample(s),
            "A-law encode_slice diverged from encode_sample at index {i} (sample {s})"
        );
    }
    for &len in CHUNK_LENGTHS {
        for chunk in input.chunks(len) {
            let mut part = vec![0u8; chunk.len()];
            alaw::encode_slice(chunk, &mut part);
            for (&s, &got) in chunk.iter().zip(part.iter()) {
                assert_eq!(got, alaw::encode_sample(s));
            }
        }
    }
}

// ---------------------------------------------------------------
// encode_slice_from_le_bytes — full domain through the LE layout
// ---------------------------------------------------------------

fn all_samples_le() -> Vec<u8> {
    let mut v = Vec::with_capacity(65_536 * 2);
    for s in i16::MIN..=i16::MAX {
        v.extend_from_slice(&s.to_le_bytes());
    }
    v
}

#[test]
fn mulaw_encode_slice_from_le_bytes_matches_per_sample_over_full_domain() {
    let input = all_samples_le();
    let mut out = vec![0u8; input.len() / 2];
    mulaw::encode_slice_from_le_bytes(&input, &mut out);
    for (i, (s, &got)) in (i16::MIN..=i16::MAX).zip(out.iter()).enumerate() {
        assert_eq!(
            got,
            mulaw::encode_sample(s),
            "µ-law encode_slice_from_le_bytes diverged at index {i} (sample {s})"
        );
    }
}

#[test]
fn alaw_encode_slice_from_le_bytes_matches_per_sample_over_full_domain() {
    let input = all_samples_le();
    let mut out = vec![0u8; input.len() / 2];
    alaw::encode_slice_from_le_bytes(&input, &mut out);
    for (i, (s, &got)) in (i16::MIN..=i16::MAX).zip(out.iter()).enumerate() {
        assert_eq!(
            got,
            alaw::encode_sample(s),
            "A-law encode_slice_from_le_bytes diverged at index {i} (sample {s})"
        );
    }
}

// ---------------------------------------------------------------
// encode_slice_zero_suppress — §3.2 wire contract, full domain
// ---------------------------------------------------------------

#[test]
fn mulaw_encode_slice_zero_suppress_full_domain_contract() {
    let input = all_samples();
    let mut plain = vec![0u8; input.len()];
    let mut suppressed = vec![0u8; input.len()];
    mulaw::encode_slice(&input, &mut plain);
    mulaw::encode_slice_zero_suppress(&input, &mut suppressed);
    let mut rewrites = 0usize;
    for (i, (&p, &z)) in plain.iter().zip(suppressed.iter()).enumerate() {
        // The all-zero octet never appears on the suppressed wire.
        assert_ne!(
            z,
            mulaw::MULAW_ZERO_CODEWORD,
            "all-zero octet leaked through zero-suppress at index {i}"
        );
        if p == mulaw::MULAW_ZERO_CODEWORD {
            // Exactly the forbidden codeword is rewritten, to exactly
            // the spec replacement `00000010`.
            assert_eq!(
                z,
                mulaw::MULAW_ZERO_SUPPRESS_CODEWORD,
                "wrong replacement codeword at index {i}"
            );
            rewrites += 1;
        } else {
            // Every other position is byte-identical to the plain law.
            assert_eq!(
                z, p,
                "zero-suppress mutated a non-forbidden codeword at index {i}"
            );
        }
        // Definitional equality with the single-sample form.
        assert_eq!(z, mulaw::encode_sample_zero_suppress(input[i]));
    }
    // The plain encoder does hit the forbidden codeword somewhere in
    // the S16 domain (the most-negative decision interval), so the
    // rewrite branch was genuinely exercised.
    assert!(
        rewrites > 0,
        "no S16 sample quantised to the all-zero codeword — rewrite branch untested"
    );
}

// ---------------------------------------------------------------
// Trait-surface delegation — the codec paths equal the batch helpers
// ---------------------------------------------------------------

fn params(id: &str) -> CodecParameters {
    let mut p = CodecParameters::audio(CodecId::new(id));
    p.sample_rate = Some(8_000);
    p.channels = Some(1);
    p.sample_format = Some(SampleFormat::S16);
    p
}

type DecoderFactory = fn(&CodecParameters) -> oxideav_core::Result<Box<dyn oxideav_core::Decoder>>;
type EncoderFactory = fn(&CodecParameters) -> oxideav_core::Result<Box<dyn oxideav_core::Encoder>>;
type BatchFn = fn(&[u8], &mut [u8]);

#[test]
fn decoder_trait_surface_equals_decode_slice_to_le_bytes() {
    let laws: [(&str, DecoderFactory, BatchFn); 2] = [
        (
            "pcm_mulaw",
            mulaw::make_decoder,
            mulaw::decode_slice_to_le_bytes,
        ),
        (
            "pcm_alaw",
            alaw::make_decoder,
            alaw::decode_slice_to_le_bytes,
        ),
    ];
    for (id, dec_factory, batch) in laws {
        let input = all_bytes();
        let mut expected = vec![0u8; input.len() * 2];
        batch(&input, &mut expected);

        let p = params(id);
        let mut dec = dec_factory(&p).expect("make_decoder");
        let pkt = Packet::new(0, TimeBase::new(1, 8_000), input.clone());
        dec.send_packet(&pkt).expect("send_packet");
        let Frame::Audio(af) = dec.receive_frame().expect("receive_frame") else {
            panic!("expected audio frame");
        };
        assert_eq!(
            af.data[0], expected,
            "{id}: decoder trait surface diverged from decode_slice_to_le_bytes"
        );
    }
}

#[test]
fn encoder_trait_surface_equals_encode_slice_from_le_bytes() {
    let laws: [(&str, EncoderFactory, BatchFn); 2] = [
        (
            "pcm_mulaw",
            mulaw::make_encoder,
            mulaw::encode_slice_from_le_bytes,
        ),
        (
            "pcm_alaw",
            alaw::make_encoder,
            alaw::encode_slice_from_le_bytes,
        ),
    ];
    for (id, enc_factory, batch) in laws {
        let pcm = all_samples_le();
        let mut expected = vec![0u8; pcm.len() / 2];
        batch(&pcm, &mut expected);

        let p = params(id);
        let mut enc = enc_factory(&p).expect("make_encoder");
        let frame = Frame::Audio(oxideav_core::AudioFrame {
            samples: (pcm.len() / 2) as u32,
            pts: Some(0),
            data: vec![pcm.clone()],
        });
        enc.send_frame(&frame).expect("send_frame");
        let pkt = enc.receive_packet().expect("receive_packet");
        assert_eq!(
            pkt.data, expected,
            "{id}: encoder trait surface diverged from encode_slice_from_le_bytes"
        );
    }
}

// ---------------------------------------------------------------
// Degenerate shapes: empty slices, mismatched lengths
// ---------------------------------------------------------------

#[test]
fn empty_slices_are_a_no_op() {
    mulaw::decode_slice(&[], &mut []);
    mulaw::decode_slice_to_le_bytes(&[], &mut []);
    mulaw::encode_slice(&[], &mut []);
    mulaw::encode_slice_from_le_bytes(&[], &mut []);
    mulaw::encode_slice_zero_suppress(&[], &mut []);
    alaw::decode_slice(&[], &mut []);
    alaw::decode_slice_to_le_bytes(&[], &mut []);
    alaw::encode_slice(&[], &mut []);
    alaw::encode_slice_from_le_bytes(&[], &mut []);
}

#[test]
#[should_panic(expected = "lengths must match")]
fn mulaw_decode_slice_length_mismatch_panics() {
    let mut out = vec![0i16; 3];
    mulaw::decode_slice(&[0u8; 4], &mut out);
}

#[test]
#[should_panic(expected = "2 bytes per input byte")]
fn alaw_decode_slice_to_le_bytes_length_mismatch_panics() {
    let mut out = vec![0u8; 7]; // needs 8
    alaw::decode_slice_to_le_bytes(&[0u8; 4], &mut out);
}

#[test]
#[should_panic(expected = "lengths must match")]
fn alaw_encode_slice_length_mismatch_panics() {
    let mut out = vec![0u8; 5];
    alaw::encode_slice(&[0i16; 4], &mut out);
}

#[test]
#[should_panic(expected = "2 bytes per output byte")]
fn mulaw_encode_slice_from_le_bytes_odd_input_panics() {
    let mut out = vec![0u8; 2];
    // 5 input bytes can never be 2 × any output length.
    mulaw::encode_slice_from_le_bytes(&[0u8; 5], &mut out);
}

#[test]
#[should_panic(expected = "lengths must match")]
fn mulaw_encode_slice_zero_suppress_length_mismatch_panics() {
    let mut out = vec![0u8; 1];
    mulaw::encode_slice_zero_suppress(&[0i16; 2], &mut out);
}
