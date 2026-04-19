# oxideav-g711

Pure-Rust **ITU-T G.711** codec — both µ-law and A-law variants,
decoder + encoder. Spec-exact lookup tables, deterministic per-sample
companding, bit-exact against the reference formulas in G.711 §2 / §3.
Zero C dependencies, no FFI, no `*-sys` crates.

Part of the [oxideav](https://github.com/OxideAV/oxideav-workspace)
framework but usable standalone.

## Installation

```toml
[dependencies]
oxideav-core = "0.1"
oxideav-codec = "0.1"
oxideav-g711 = "0.0"
```

## Quick use

G.711 is 1 byte per input sample, stateless. Every encoded byte
decodes to exactly one S16 PCM sample and vice versa. The spec defines
it at 8 kHz mono (PSTN) but the implementation is rate- and
channel-independent — interleaved S16 input with any channel count
round-trips through the same byte-per-sample mapping.

```rust
use oxideav_codec::CodecRegistry;
use oxideav_core::{CodecId, CodecParameters, Frame, Packet, TimeBase};

let mut reg = CodecRegistry::new();
oxideav_g711::register(&mut reg);

let mut params = CodecParameters::audio(CodecId::new("pcm_mulaw"));
params.sample_rate = Some(8_000);
params.channels = Some(1);

let mut dec = reg.make_decoder(&params)?;
dec.send_packet(&Packet::new(0, TimeBase::new(1, 8_000), mulaw_bytes))?;
let Frame::Audio(a) = dec.receive_frame()? else { unreachable!() };
// `a.data[0]` is interleaved S16 PCM.
# Ok::<(), oxideav_core::Error>(())
```

Encoder is symmetric — build with `reg.make_encoder(&params)`, feed
`Frame::Audio` with S16 PCM, get companded `Packet`s back. One output
byte per input S16 sample, preserving interleave order across channels.

### Codec IDs

- **µ-law**: `"pcm_mulaw"` (aliases: `"g711u"`, `"ulaw"`)
- **A-law**: `"pcm_alaw"` (aliases: `"g711a"`, `"alaw"`)

### Channels and sample rate

Both variants accept any channel count ≥ 1 and any sample rate. The
packet-to-frame mapping assumes interleaved bytes in the same order as
the S16 PCM: `ch0 ch1 … chN ch0 ch1 …`. Decoder packets whose length
is not a multiple of the channel count are rejected.

### Going deeper — bypassing the registry

If you just want the byte-to-sample mapping, skip the trait surface
entirely:

```rust
use oxideav_g711::{mulaw, alaw};

let pcm: i16 = mulaw::decode_sample(0x7F);
let byte: u8  = alaw::encode_sample(12345);
```

The `UlawDecoder` / `UlawEncoder` / `AlawDecoder` / `AlawEncoder`
structs are also publicly constructible via `mulaw::make_decoder` /
`alaw::make_decoder` / etc. for cases where you want full control over
construction without the registry lookup.

### Properties verified by the test suite

- Decode tables match the ITU-T G.711 §2 / §3 reference formulas for
  all 256 bytes of both laws.
- Encode is bit-exact against the reference Sun/ANSI formulas for all
  65 536 S16 inputs of both laws.
- Encode / decode / encode is idempotent for every S16 sample (the
  quantiser is a projection).
- Decode / encode is a round-trip identity for every byte, except µ-law
  bytes 0x7F (negative zero) which the encoder canonicalises to 0xFF
  (positive zero) — both decode to linear 0 per the spec.
- Sign symmetry: byte `b` and byte `b ^ 0x80` decode to exact negatives.
- Monotonicity: the companded transfer function is monotonic across
  the whole S16 range.
- Multichannel round-trip (1, 2, 6, 8 channels) through the trait
  surface returns the same per-sample quantisation as direct calls.

## License

MIT — see [LICENSE](LICENSE).
