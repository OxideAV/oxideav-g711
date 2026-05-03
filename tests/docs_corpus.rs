//! Integration tests against the `docs/audio/g711/fixtures/` corpus.
//!
//! Each fixture under `../../docs/audio/g711/fixtures/<name>/` ships:
//! * `input.{wav,au,raw}` — the encoded G.711 bitstream (one byte per
//!   sample-per-channel) wrapped in WAV / Sun .au / raw containers.
//! * `expected.wav` (occasionally `expected_from_raw.wav`) — the
//!   reference 16-bit signed PCM that an FFmpeg decode produces.
//! * `expected.sha256` / inlined SHAs in `notes.md` — bit-exact hash of
//!   the reference output (informational; not consumed here — we
//!   compare PCM samples directly).
//! * `trace.txt` + `notes.md` — implementor notes (not consumed by
//!   this driver).
//!
//! For every fixture we:
//! 1. Decode `input.*` to a (channels, sample_rate, codec_alias,
//!    payload-bytes) tuple by parsing the container header.
//! 2. Feed the payload bytes through the in-tree G.711 decoder under
//!    the codec alias the container advertises.
//! 3. Parse `expected.wav` to get the reference S16 PCM data chunk.
//! 4. Report per-channel RMS error and overall match percentage.
//!
//! Tiering:
//! * `Tier::BitExact` — must match the reference exactly. CI fails on
//!   any divergence. G.711 is a per-byte LUT so once the container
//!   parsing is right, every fixture should land here.
//! * `Tier::ReportOnly` — log the deltas without failing CI. New or
//!   under-investigation fixtures start here and graduate to
//!   `BitExact` once they're shown to be clean.
//! * `Tier::Ignored` — skipped (e.g. if the docs corpus isn't present
//!   in the CI checkout).
//!
//! All fixtures start `ReportOnly`. They graduate to `BitExact` once
//! they've been confirmed to round-trip across a CI run.
//!
//! Note: `oxideav-g711` is its own repository and `docs/` is checked
//! into the workspace umbrella, NOT into this crate's checkout. When
//! the fixtures are missing the test logs `skip <name>: missing ...`
//! and returns success, so the CI gate stays clean for both layouts.

use std::fs;
use std::path::PathBuf;

use oxideav_core::{CodecId, CodecParameters, Decoder, Frame, Packet, SampleFormat, TimeBase};
use oxideav_g711::{alaw, mulaw};

// ---------------------------------------------------------------------------
// Fixture path resolution
// ---------------------------------------------------------------------------

/// Locate `docs/audio/g711/fixtures/<name>/`. When the test runs as
/// part of the umbrella workspace, CWD is the crate root and the docs
/// live two levels up at `../../docs/`. When the standalone
/// oxideav-g711 repo is checked out alone (CI), `../../docs/` is
/// absent and every fixture access skips gracefully.
fn fixture_dir(name: &str) -> PathBuf {
    PathBuf::from("../../docs/audio/g711/fixtures").join(name)
}

// ---------------------------------------------------------------------------
// Container parsers — tiny in-test parsers, clean-room from spec/notes.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct G711Stream {
    /// Codec alias to feed `oxideav-g711` (`pcm_alaw` or `pcm_mulaw`).
    codec_alias: &'static str,
    sample_rate: u32,
    channels: u16,
    /// Raw, container-stripped G.711 codeword bytes (one per
    /// sample-per-channel; channels are byte-interleaved).
    payload: Vec<u8>,
}

#[derive(Debug, Clone)]
struct PcmS16 {
    sample_rate: u32,
    channels: u16,
    /// Interleaved S16LE bytes from the WAV `data` chunk.
    bytes: Vec<u8>,
}

/// Parse a RIFF WAVE header. Walks chunks until it locates `fmt ` and
/// `data`. Supports WAVEFORMAT / WAVEFORMATEX / WAVEFORMATEXTENSIBLE
/// to the extent G.711 / S16 PCM fixtures need (we only consume the
/// first 16 bytes of `fmt `; trailing extension bytes are skipped).
fn parse_wav(bytes: &[u8]) -> Result<(u16 /*format_tag*/, u16 /*channels*/, u32 /*sample_rate*/, u16 /*bits_per_sample*/, Vec<u8> /*data*/), String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".to_string());
    }
    let mut off = 12usize;
    let mut fmt: Option<(u16, u16, u32, u16)> = None;
    let mut data: Option<Vec<u8>> = None;
    while off + 8 <= bytes.len() {
        let id = &bytes[off..off + 4];
        let size =
            u32::from_le_bytes([bytes[off + 4], bytes[off + 5], bytes[off + 6], bytes[off + 7]])
                as usize;
        let body_off = off + 8;
        if body_off + size > bytes.len() {
            return Err(format!(
                "WAV chunk {:?} body truncated (size {} from off {} > {})",
                std::str::from_utf8(id).unwrap_or("???"),
                size,
                body_off,
                bytes.len()
            ));
        }
        match id {
            b"fmt " => {
                if size < 16 {
                    return Err(format!("fmt chunk too small: {size}"));
                }
                let body = &bytes[body_off..body_off + size];
                let format_tag = u16::from_le_bytes([body[0], body[1]]);
                let channels = u16::from_le_bytes([body[2], body[3]]);
                let sample_rate =
                    u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
                let bits_per_sample = u16::from_le_bytes([body[14], body[15]]);
                fmt = Some((format_tag, channels, sample_rate, bits_per_sample));
            }
            b"data" => {
                data = Some(bytes[body_off..body_off + size].to_vec());
            }
            _ => {}
        }
        // RIFF chunks are word-aligned: pad odd sizes by one byte.
        off = body_off + size + (size & 1);
    }
    let (tag, ch, sr, bps) = fmt.ok_or_else(|| "WAV missing fmt chunk".to_string())?;
    let data = data.ok_or_else(|| "WAV missing data chunk".to_string())?;
    Ok((tag, ch, sr, bps, data))
}

/// Parse a Sun/NeXT `.au` (`.snd`) header. Big-endian throughout.
/// Encoding 1 = 8-bit µ-law, 27 = 8-bit A-law (per the historical
/// SUN_AUDIOFILE_ENCODING_* table). header_size points at the start
/// of the audio payload.
fn parse_au(bytes: &[u8]) -> Result<(&'static str /*alias*/, u16 /*ch*/, u32 /*sr*/, Vec<u8>), String> {
    if bytes.len() < 24 || &bytes[0..4] != b".snd" {
        return Err("not a Sun .au file (.snd magic missing)".to_string());
    }
    let header_size = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
    let _data_size = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    let encoding = u32::from_be_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
    let sr = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let ch = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]) as u16;
    let alias: &'static str = match encoding {
        1 => "pcm_mulaw",
        27 => "pcm_alaw",
        other => return Err(format!(".au encoding {other} not supported by this driver")),
    };
    if header_size > bytes.len() {
        return Err(format!(
            ".au header_size {header_size} > file length {}",
            bytes.len()
        ));
    }
    let payload = bytes[header_size..].to_vec();
    Ok((alias, ch, sr, payload))
}

/// Parse a fixture `input.*` into a G711Stream. The on-disk container
/// (.wav / .au / .raw) is auto-detected; raw inputs need explicit
/// metadata which the caller must supply.
fn load_input(dir: &PathBuf, raw_meta: Option<(&'static str, u16, u32)>) -> Option<G711Stream> {
    // Search order: wav > au > raw. Notes.md fixtures reference these
    // by name so we accept whichever is present.
    let try_path = |name: &str| -> Option<Vec<u8>> {
        let p = dir.join(name);
        match fs::read(&p) {
            Ok(b) => Some(b),
            Err(_) => None,
        }
    };
    if let Some(bytes) = try_path("input.wav") {
        let (tag, ch, sr, bps, data) = match parse_wav(&bytes) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("  load_input: parse_wav failed: {e}");
                return None;
            }
        };
        if bps != 8 {
            eprintln!(
                "  load_input: input.wav bits_per_sample={bps}, expected 8 for G.711"
            );
            return None;
        }
        let alias: &'static str = match tag {
            0x0006 => "pcm_alaw",
            0x0007 => "pcm_mulaw",
            other => {
                eprintln!(
                    "  load_input: input.wav format_tag=0x{other:04x}, expected 0x0006/0x0007"
                );
                return None;
            }
        };
        return Some(G711Stream {
            codec_alias: alias,
            sample_rate: sr,
            channels: ch,
            payload: data,
        });
    }
    if let Some(bytes) = try_path("input.au") {
        let (alias, ch, sr, payload) = match parse_au(&bytes) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("  load_input: parse_au failed: {e}");
                return None;
            }
        };
        return Some(G711Stream {
            codec_alias: alias,
            sample_rate: sr,
            channels: ch,
            payload,
        });
    }
    if let Some(bytes) = try_path("input.raw") {
        let (alias, ch, sr) = raw_meta.expect(
            "raw fixture requires explicit (alias, channels, sample_rate) — \
             no container to read it from",
        );
        return Some(G711Stream {
            codec_alias: alias,
            sample_rate: sr,
            channels: ch,
            payload: bytes,
        });
    }
    None
}

/// Read a reference PCM fixture (S16LE, interleaved). The corpus
/// stores these as RIFF WAVE files; we re-use parse_wav.
fn load_expected_pcm(path: &PathBuf) -> Option<PcmS16> {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(_) => return None,
    };
    let (tag, ch, sr, bps, data) = match parse_wav(&bytes) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("  load_expected_pcm: parse_wav failed: {e}");
            return None;
        }
    };
    if tag != 0x0001 {
        eprintln!(
            "  load_expected_pcm: expected.wav format_tag=0x{tag:04x}, expected 0x0001 (PCM)"
        );
        return None;
    }
    if bps != 16 {
        eprintln!(
            "  load_expected_pcm: expected.wav bits_per_sample={bps}, expected 16"
        );
        return None;
    }
    Some(PcmS16 {
        sample_rate: sr,
        channels: ch,
        bytes: data,
    })
}

// ---------------------------------------------------------------------------
// Decode + score
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
#[allow(dead_code)]
enum Tier {
    /// Must decode bit-exactly — any divergence fails CI. Promote
    /// fixtures here once CI confirms 100.00% match in `ReportOnly`
    /// mode.
    BitExact,
    /// Currently divergent or under investigation; logged but does not
    /// gate CI. All fixtures start here per the brief.
    ReportOnly,
}

struct PerChannel {
    /// Number of samples in this channel.
    n: usize,
    /// Number of samples that matched bit-exactly.
    exact: usize,
    /// Maximum absolute amplitude difference observed in this channel.
    max_abs_diff: i32,
    /// Sum of squared differences, used to compute RMS.
    sse: f64,
}

impl PerChannel {
    fn new() -> Self {
        Self {
            n: 0,
            exact: 0,
            max_abs_diff: 0,
            sse: 0.0,
        }
    }

    fn rms(&self) -> f64 {
        if self.n == 0 {
            0.0
        } else {
            (self.sse / self.n as f64).sqrt()
        }
    }

    fn pct(&self) -> f64 {
        if self.n == 0 {
            100.0
        } else {
            self.exact as f64 / self.n as f64 * 100.0
        }
    }
}

/// Run the in-tree decoder over `stream.payload` and produce
/// interleaved S16LE bytes — same shape as `parse_wav` returns for a
/// 16-bit PCM `data` chunk.
fn decode_stream(stream: &G711Stream) -> Result<Vec<u8>, String> {
    let mut params = CodecParameters::audio(CodecId::new(stream.codec_alias));
    params.sample_rate = Some(stream.sample_rate);
    params.channels = Some(stream.channels);
    params.sample_format = Some(SampleFormat::S16);
    let mut dec: Box<dyn Decoder> = match stream.codec_alias {
        "pcm_alaw" => alaw::make_decoder(&params).map_err(|e| format!("alaw ctor: {e:?}"))?,
        "pcm_mulaw" => mulaw::make_decoder(&params).map_err(|e| format!("mulaw ctor: {e:?}"))?,
        other => return Err(format!("unknown alias {other}")),
    };
    let pkt = Packet::new(0, TimeBase::new(1, stream.sample_rate as i64), stream.payload.clone());
    dec.send_packet(&pkt)
        .map_err(|e| format!("send_packet: {e:?}"))?;
    let frame = dec
        .receive_frame()
        .map_err(|e| format!("receive_frame: {e:?}"))?;
    let Frame::Audio(af) = frame else {
        return Err("decoder returned non-audio frame".to_string());
    };
    af.data
        .into_iter()
        .next()
        .ok_or_else(|| "decoder produced empty audio frame".to_string())
}

fn compare_per_channel(our_s16: &[u8], ref_s16: &[u8], channels: u16) -> Vec<PerChannel> {
    let mut out: Vec<PerChannel> = (0..channels).map(|_| PerChannel::new()).collect();
    let pair_size = channels as usize * 2;
    if pair_size == 0 {
        return out;
    }
    let n = our_s16.len().min(ref_s16.len()) / pair_size;
    for i in 0..n {
        for c in 0..channels as usize {
            let off = i * pair_size + c * 2;
            let our_v = i16::from_le_bytes([our_s16[off], our_s16[off + 1]]) as i32;
            let ref_v = i16::from_le_bytes([ref_s16[off], ref_s16[off + 1]]) as i32;
            let diff = our_v - ref_v;
            let abs = diff.abs();
            let entry = &mut out[c];
            entry.n += 1;
            if abs == 0 {
                entry.exact += 1;
            }
            if abs > entry.max_abs_diff {
                entry.max_abs_diff = abs;
            }
            entry.sse += (diff as f64) * (diff as f64);
        }
    }
    out
}

struct CorpusCase {
    /// Fixture directory name under `docs/audio/g711/fixtures/`.
    name: &'static str,
    /// Tier — controls whether divergence fails the test.
    tier: Tier,
    /// Where to look for the expected PCM. Most fixtures use
    /// `expected.wav`; the containerless pair also has
    /// `expected_from_raw.wav` for the raw-input branch.
    expected_filename: &'static str,
    /// For raw inputs only (no container header to derive metadata
    /// from): (codec_alias, channels, sample_rate).
    raw_meta: Option<(&'static str, u16, u32)>,
}

fn evaluate(case: &CorpusCase) {
    let dir = fixture_dir(case.name);
    let stream = match load_input(&dir, case.raw_meta) {
        Some(s) => s,
        None => {
            eprintln!(
                "skip {}: no input.{{wav,au,raw}} found in {}",
                case.name,
                dir.display()
            );
            return;
        }
    };
    let expected_path = dir.join(case.expected_filename);
    let expected = match load_expected_pcm(&expected_path) {
        Some(e) => e,
        None => {
            eprintln!(
                "skip {}: missing or unreadable {}",
                case.name,
                expected_path.display()
            );
            return;
        }
    };

    if expected.sample_rate != stream.sample_rate || expected.channels != stream.channels {
        eprintln!(
            "[{:?}] {}: container/expected metadata mismatch (input {} ch @ {} Hz, \
             expected {} ch @ {} Hz)",
            case.tier,
            case.name,
            stream.channels,
            stream.sample_rate,
            expected.channels,
            expected.sample_rate
        );
    }

    let our_s16 = match decode_stream(&stream) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[{:?}] {}: decoder error: {e}", case.tier, case.name);
            match case.tier {
                Tier::BitExact => panic!("{}: decoder error: {e}", case.name),
                Tier::ReportOnly => return,
            }
        }
    };

    let stats = compare_per_channel(&our_s16, &expected.bytes, stream.channels);

    let mut total_n = 0usize;
    let mut total_exact = 0usize;
    let mut overall_max_abs = 0i32;
    for (c, st) in stats.iter().enumerate() {
        eprintln!(
            "  ch{}: {} samples, {} exact ({:.4}%), RMS={:.3}, max |diff|={}",
            c,
            st.n,
            st.exact,
            st.pct(),
            st.rms(),
            st.max_abs_diff
        );
        total_n += st.n;
        total_exact += st.exact;
        overall_max_abs = overall_max_abs.max(st.max_abs_diff);
    }
    let overall_pct = if total_n == 0 {
        100.0
    } else {
        total_exact as f64 / total_n as f64 * 100.0
    };
    let length_match = our_s16.len() == expected.bytes.len();
    eprintln!(
        "[{:?}] {}: {}/{} samples exact ({:.4}%), max |diff|={}, length-match={}",
        case.tier,
        case.name,
        total_exact,
        total_n,
        overall_pct,
        overall_max_abs,
        length_match
    );

    match case.tier {
        Tier::BitExact => {
            assert!(
                length_match,
                "{}: length mismatch (our {} bytes, expected {} bytes)",
                case.name,
                our_s16.len(),
                expected.bytes.len()
            );
            assert_eq!(
                total_exact, total_n,
                "{}: not bit-exact (max |diff|={}, {:.4}% match)",
                case.name, overall_max_abs, overall_pct
            );
        }
        Tier::ReportOnly => {
            // Don't fail. Report-only fixtures graduate to BitExact
            // once we've confirmed they round-trip cleanly.
            let _ = overall_pct;
        }
    }
}

// ---------------------------------------------------------------------------
// Per-fixture tests — every entry maps 1:1 to a directory under
// docs/audio/g711/fixtures/. All start `Tier::ReportOnly` per the
// brief — the driver logs per-channel RMS / match-pct for every
// fixture and a follow-up round promotes the clean ones to
// `Tier::BitExact` once CI confirms a 100.00% match. G.711 is a
// deterministic per-byte LUT so we expect every fixture to graduate
// in the very next round; any that don't are real bugs in either the
// decoder or the in-test container parser. The driver still skips
// gracefully when the fixture corpus isn't present (standalone CI
// checkout).
// ---------------------------------------------------------------------------

#[test]
fn corpus_alaw_mono_16000_0_5s() {
    evaluate(&CorpusCase {
        name: "alaw-mono-16000-0.5s",
        tier: Tier::ReportOnly,
        expected_filename: "expected.wav",
        raw_meta: None,
    });
}

#[test]
fn corpus_alaw_mono_8000_1s() {
    evaluate(&CorpusCase {
        name: "alaw-mono-8000-1s",
        tier: Tier::ReportOnly,
        expected_filename: "expected.wav",
        raw_meta: None,
    });
}

#[test]
fn corpus_alaw_stereo_8000_0_5s() {
    evaluate(&CorpusCase {
        name: "alaw-stereo-8000-0.5s",
        tier: Tier::ReportOnly,
        expected_filename: "expected.wav",
        raw_meta: None,
    });
}

#[test]
fn corpus_mulaw_mono_8000_1s() {
    evaluate(&CorpusCase {
        name: "mulaw-mono-8000-1s",
        tier: Tier::ReportOnly,
        expected_filename: "expected.wav",
        raw_meta: None,
    });
}

#[test]
fn corpus_silence_alaw() {
    evaluate(&CorpusCase {
        name: "silence-alaw",
        tier: Tier::ReportOnly,
        expected_filename: "expected.wav",
        raw_meta: None,
    });
}

#[test]
fn corpus_silence_mulaw() {
    evaluate(&CorpusCase {
        name: "silence-mulaw",
        tier: Tier::ReportOnly,
        expected_filename: "expected.wav",
        raw_meta: None,
    });
}

#[test]
fn corpus_dc_positive_alaw() {
    evaluate(&CorpusCase {
        name: "dc-positive-alaw",
        tier: Tier::ReportOnly,
        expected_filename: "expected.wav",
        raw_meta: None,
    });
}

#[test]
fn corpus_dc_negative_alaw() {
    evaluate(&CorpusCase {
        name: "dc-negative-alaw",
        tier: Tier::ReportOnly,
        expected_filename: "expected.wav",
        raw_meta: None,
    });
}

#[test]
fn corpus_sine_1khz_alaw_0_5s() {
    // Bonus fixture present in docs/audio/g711/fixtures/ — 1 kHz sine
    // exercises a wider segment / quant range than the canonical
    // 440 Hz tone in alaw-mono-8000-1s.
    evaluate(&CorpusCase {
        name: "sine-1khz-alaw-0.5s",
        tier: Tier::ReportOnly,
        expected_filename: "expected.wav",
        raw_meta: None,
    });
}

#[test]
fn corpus_sine_1khz_mulaw_0_5s() {
    // Bonus fixture, µ-law twin of the 1 kHz A-law sine above.
    evaluate(&CorpusCase {
        name: "sine-1khz-mulaw-0.5s",
        tier: Tier::ReportOnly,
        expected_filename: "expected.wav",
        raw_meta: None,
    });
}

#[test]
fn corpus_au_container_mulaw() {
    // Sun .au container with µ-law payload — proves the codeword
    // layout in `.au` data chunks is identical to the WAV variant
    // (the .au header carries the same (codec, channels, rate) triple
    // and nothing decode-relevant beyond it).
    evaluate(&CorpusCase {
        name: "au-container-mulaw",
        tier: Tier::ReportOnly,
        expected_filename: "expected.wav",
        raw_meta: None,
    });
}

// containerless-raw-alaw-vs-wav-pair ships TWO inputs that should
// produce identical PCM:
//   * input.wav (RIFF/WAVE-wrapped)  -> expected.wav
//   * input.raw (no container)      -> expected_from_raw.wav
// We split into two tests. `load_input` prefers wav over raw, so the
// raw branch lives in its own subdirectory-less path: we delegate
// the raw load explicitly via `raw_meta` and a custom evaluator.

#[test]
fn corpus_containerless_raw_alaw_vs_wav_pair_wav_branch() {
    evaluate(&CorpusCase {
        name: "containerless-raw-alaw-vs-wav-pair",
        tier: Tier::ReportOnly,
        expected_filename: "expected.wav",
        raw_meta: None,
    });
}

#[test]
fn corpus_containerless_raw_alaw_vs_wav_pair_raw_branch() {
    // Raw branch — bypass `load_input`'s wav-first preference by
    // assembling the stream directly from input.raw. Per the fixture
    // notes: A-law, mono, 8000 Hz.
    let dir = fixture_dir("containerless-raw-alaw-vs-wav-pair");
    let raw_path = dir.join("input.raw");
    let raw_bytes = match fs::read(&raw_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!(
                "skip containerless-raw-alaw-vs-wav-pair raw branch: missing {} ({e})",
                raw_path.display()
            );
            return;
        }
    };
    let stream = G711Stream {
        codec_alias: "pcm_alaw",
        sample_rate: 8_000,
        channels: 1,
        payload: raw_bytes,
    };
    let expected_path = dir.join("expected_from_raw.wav");
    let expected = match load_expected_pcm(&expected_path) {
        Some(e) => e,
        None => {
            eprintln!(
                "skip containerless-raw-alaw-vs-wav-pair raw branch: missing {}",
                expected_path.display()
            );
            return;
        }
    };
    let our_s16 = decode_stream(&stream).expect("alaw decoder");
    let stats = compare_per_channel(&our_s16, &expected.bytes, stream.channels);
    let mut total_n = 0usize;
    let mut total_exact = 0usize;
    let mut overall_max_abs = 0i32;
    for (c, st) in stats.iter().enumerate() {
        eprintln!(
            "  ch{}: {} samples, {} exact ({:.4}%), RMS={:.3}, max |diff|={}",
            c,
            st.n,
            st.exact,
            st.pct(),
            st.rms(),
            st.max_abs_diff
        );
        total_n += st.n;
        total_exact += st.exact;
        overall_max_abs = overall_max_abs.max(st.max_abs_diff);
    }
    let overall_pct = if total_n == 0 {
        100.0
    } else {
        total_exact as f64 / total_n as f64 * 100.0
    };
    let length_match = our_s16.len() == expected.bytes.len();
    eprintln!(
        "[ReportOnly] containerless-raw-alaw-vs-wav-pair (raw branch): {}/{} samples exact \
         ({:.4}%), max |diff|={}, length-match={}",
        total_exact, total_n, overall_pct, overall_max_abs, length_match
    );
    // ReportOnly: don't fail. Promote to hard asserts once CI confirms
    // the raw branch round-trips cleanly.
    let _ = length_match;
}
