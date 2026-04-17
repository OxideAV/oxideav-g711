# oxideav-g711

Pure-Rust **ITU-T G.711** codec — both μ-law and A-law variants, decoder
+ encoder.

Spec-exact lookup tables. Zero C dependencies, no FFI, no `*-sys` crates.

Originally part of the [oxideav](https://github.com/KarpelesLab/oxideav)
framework; extracted to its own crate for independent publication.

## Usage

```toml
[dependencies]
oxideav-g711 = "0.0.3"
```

Plugs into [`oxideav-codec`](https://crates.io/crates/oxideav-codec):

```rust
let mut reg = oxideav_codec::CodecRegistry::new();
oxideav_g711::register(&mut reg);
```

Decoder ids: `"pcm_mulaw"` and `"pcm_alaw"` (with common aliases) — input
packets are 1 byte/sample at 8 kHz mono, output is 16-bit linear PCM.
Encoder inverts the mapping.

## License

MIT — see [LICENSE](LICENSE).
