# oxideav-g711

Pure-Rust **ITU-T G.711** codec — both μ-law and A-law variants,
decoder + encoder. Spec-exact lookup tables. Zero C dependencies, no
FFI, no `*-sys` crates.

Part of the [oxideav](https://github.com/OxideAV/oxideav-workspace)
framework but usable entirely standalone.

## Installation

```toml
[dependencies]
oxideav-core = "0.0"
oxideav-codec = "0.0"
oxideav-g711 = "0.0"
```

## Quick use

G.711 is 8 kHz mono · 1 byte per sample · stateless. Every byte of
input decodes to exactly one S16 PCM sample.

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
// `a.data[0]` is interleaved S16 PCM at 8 kHz mono.
# Ok::<(), oxideav_core::Error>(())
```

Encoder is symmetric — build with `reg.make_encoder(&params)`, feed
`Frame::Audio` with S16 PCM, get µ-law `Packet`s back.

### Codec IDs

- **µ-law**: `"pcm_mulaw"` (aliases: `"g711u"`, `"ulaw"`)
- **A-law**: `"pcm_alaw"` (aliases: `"g711a"`, `"alaw"`)

### Going deeper — skipping the registry

If you just want the byte-to-sample mapping, bypass the trait surface
entirely:

```rust
use oxideav_g711::{mulaw, alaw};

let pcm: i16 = mulaw::decode_sample(0x7F);
let byte: u8  = alaw::encode_sample(12345);
```

The `UlawDecoder` / `UlawEncoder` / `AlawDecoder` / `AlawEncoder`
structs implementing the trait are also publicly constructible for
cases where you want to skip the registry.

## License

MIT — see [LICENSE](LICENSE).
