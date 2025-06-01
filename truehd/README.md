# truehd

A low-level parser and decoder for Dolby TrueHD audio bitstreams, implemented in Rust.

> ⚠️ **Experimental**: 
> 
> This crate is intended for internal or research use only.  
> It is not designed for production or end-user playback systems.

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
|                 | Dynamic range control         | 🔴     | Low      | Optional     |                               |
|                 | Intermediate spatial format   | 🔴     | Low      | Out-of-scope | I have no idea                |
| **Other TODOs** | Documentation                 | 🟡     | High     | Essential    | With kind support from Claude |
|                 | Unit tests                    | 🔴     | High     | Essential    |                               |
|                 | Benchmarking                  | 🔴     | Medium   | Important    |                               |
|                 | Metadata interpolation        | 🔴     | Low      | Nice-to-have |                               |
|                 | Bitstream editing             | 🔴     | Low      | Nice-to-have |                               |
|                 | Encoding                      | 🔴     | Low      | Nice-to-have |                               |
|                 | Object audio rendering        | 🔴     | Low      | Out-of-scope |                               |

**Legend:** 🟢 Completed • 🟡 In Progress • 🔴 Not Started

---

## License

Licensed under the Apache License, Version 2.0. See [LICENSE](../LICENSE) for details.