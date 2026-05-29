#![no_main]

//! Drive arbitrary fuzz-supplied bytes through the µ-law and A-law
//! decoder trait surface.
//!
//! G.711 decode is stateless per-sample (one byte ⇒ one S16 PCM
//! sample via a 256-entry LUT) so the per-sample math itself has no
//! room for malformed input — it is total over `u8`. The risk shape
//! the fuzzer targets is the **framing wrapper**:
//!
//! - Packet length divisibility: `pkt.data.len() % channels` must
//!   return `Err(Error::Invalid…)`, never panic. The fuzzer chooses
//!   the channel count from the first input byte so any (length,
//!   channels) combination is reached.
//! - Empty packets: `pkt.data.is_empty()` must yield an empty
//!   `AudioFrame` (zero samples), not a divide-by-zero.
//! - Sequencing: `receive_frame` before `send_packet` must report
//!   `NeedMore`; calling `send_packet` twice without an intervening
//!   `receive_frame` must report `Error::other`. Both branches need
//!   to return cleanly across arbitrary `(channels, payload)` shapes.
//! - Post-flush behaviour: after `flush()`, `receive_frame()` must
//!   report `Error::Eof`, not panic. The fuzzer hits this path on
//!   every input by flushing once at the end.
//!
//! Both laws share the same framing wrapper (separate `UlawDecoder` /
//! `AlawDecoder` structs, identical contract) so each input drives
//! both decoders independently and discards their output. The
//! `pkt.data.len() % channels` rejection is a property of the
//! wrapper, not the codec id, but exercising both decoders guards
//! against any future per-law specialisation that might re-introduce
//! a panic site.
//!
//! ## Fuzz input layout
//!
//! ```text
//!   byte 0      : channels seed → (b0 % 8) + 1, in 1..=8
//!   bytes 1..   : raw packet payload — fed verbatim to both
//!                 decoders. Length divisibility against `channels`
//!                 is exactly what the wrapper validates.
//! ```

use libfuzzer_sys::fuzz_target;
use oxideav_core::{CodecId, CodecParameters, Decoder, Packet, TimeBase};
use oxideav_g711::{alaw, mulaw};

fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
        return;
    }
    let channels = ((data[0] as u16) % 8) + 1;
    let payload = data[1..].to_vec();

    // Build CodecParameters for each law. Sample rate is irrelevant
    // to the decoder (no resampling) but the canonical 8 kHz PSTN
    // rate keeps the parameter shape readable in any saved corpus.
    for codec_id in &["pcm_mulaw", "pcm_alaw"] {
        let mut params = CodecParameters::audio(CodecId::new(*codec_id));
        params.channels = Some(channels);
        params.sample_rate = Some(8_000);

        let mut dec: Box<dyn Decoder> = if *codec_id == "pcm_mulaw" {
            match mulaw::make_decoder(&params) {
                Ok(d) => d,
                Err(_) => return,
            }
        } else {
            match alaw::make_decoder(&params) {
                Ok(d) => d,
                Err(_) => return,
            }
        };

        // ── 1. Empty packet first — the `is_empty()` early-return
        //    must not panic and must yield an empty frame.
        let empty = Packet::new(0, TimeBase::new(1, 8_000), Vec::new());
        if dec.send_packet(&empty).is_ok() {
            let _ = dec.receive_frame();
        }

        // ── 2. The attacker payload at the attacker's channel count.
        let pkt = Packet::new(0, TimeBase::new(1, 8_000), payload.clone());
        if dec.send_packet(&pkt).is_ok() {
            let _ = dec.receive_frame();
        }

        // ── 3. Double-send without intervening receive — must
        //    return `Error::other`, not panic. Discard.
        let pkt2 = Packet::new(0, TimeBase::new(1, 8_000), payload.clone());
        let _ = dec.send_packet(&pkt2);
        let _ = dec.send_packet(&pkt2);
        let _ = dec.receive_frame();

        // ── 4. Post-flush: receive_frame must yield Eof, not panic.
        let _ = dec.flush();
        let _ = dec.receive_frame();
    }
});
