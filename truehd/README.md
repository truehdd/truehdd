# truehd

A low-level parser and decoder for Dolby TrueHD audio bitstreams, implemented in Rust.

> ⚠️ **Experimental**: 
> 
> This crate is intended for internal or research use only.  
> It is not designed for production or end-user playback systems.

## Usage

```toml
[dependencies]
truehd = "0.6.2"
```

Requires Rust 1.88.0 or later.

Decoding runs in three stages: an `Extractor` finds frames in a byte stream, a
`Parser` turns each frame into an access unit, and a `Decoder` renders access
units to PCM. See the [crate documentation](https://docs.rs/truehd) for a
worked example.

On damaged input, `Parser::reset_for_next_major_sync` and
`Decoder::reset_for_next_major_sync` drop stream state so decoding can resume
at the next major sync. Call both at the same point in the frame sequence, or
the two stages will disagree about the stream.

## Development Status


| Category        | Feature                       | Status | Priority | Criticality  | Notes                         |
|-----------------|-------------------------------|--------|----------|--------------|-------------------------------|
| **Parser**      | FBA sync bitstream (Dolby)    | 🟢     | High     | Essential    |                               |
|                 | FBB sync bitstream (Meridian) | 🔴     | Low      | Nice-to-have | Do you really need it?        |
|                 | Evolution frame               | 🟢     | High     | Essential    |                               |
|                 | CRC and parity validation     | 🟢     | High     | Essential    |                               |
|                 | SMPTE timestamp               | 🟢     | Medium   | Optional     |                               |
|                 | FBA hires output timing       | 🟢     | Medium   | Optional     |                               |
|                 | Object audio metadata         | 🟡     | High     | Essential    | Mostly done                   |
|                 | FIFO conformance tests        | 🟡     | Medium   | Optional     | Partially done                |
|                 | FBA bitstream seeking         | 🔴     | Low      | Nice-to-have | Yes, it's possible            |
| **Decoder**     | 31EA / 31EB sync substream    | 🟢     | High     | Essential    |                               |
|                 | 31EC sync substream           | 🟢     | High     | Essential    | 4th / 16ch presentation       |
|                 | Lossless check                | 🟢     | High     | Essential    |                               |
|                 | Optimize DSP performance      | 🔴     | Medium   | Important    |                               |
|                 | Dynamic range control         | 🔴     | Low      | Optional     | State parsed, not applied     |
|                 | Intermediate spatial format   | 🔴     | Low      | Out-of-scope | I have no idea                |
| **Other TODOs** | Documentation                 | 🟡     | High     | Essential    | With kind support from Claude |
|                 | Unit tests                    | 🟡     | High     | Essential    | Partially done                |
|                 | Benchmarking                  | 🔴     | Medium   | Important    |                               |
|                 | Metadata interpolation        | 🔴     | Low      | Nice-to-have |                               |
|                 | Bitstream editing             | 🔴     | Low      | Nice-to-have |                               |
|                 | Encoding                      | 🔴     | Low      | Nice-to-have |                               |
|                 | Object audio rendering        | 🔴     | Low      | Out-of-scope |                               |

**Legend:** 🟢 Completed • 🟡 In Progress • 🔴 Not Started

---

## License

Licensed under the Apache License, Version 2.0. See [LICENSE](../LICENSE) for details.