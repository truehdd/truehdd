# truehd

A low-level parser and decoder for Dolby TrueHD audio bitstreams, implemented in Rust.

> ⚠️ **Experimental**: 
> 
> This crate is intended for internal or research use only.  
> It is not designed for production or end-user playback systems.

## Usage

```toml
[dependencies]
truehd = "0.7.2"
```

Requires Rust 1.88.0 or later.

For incremental applications, `process::stream::StreamDecoder` owns extraction,
parsing, decoding, and the accepted-sample timeline as one state machine. Each
emitted frame keeps native integer PCM and typed object metadata together:

```rust
use truehd::process::stream::StreamDecoder;
# fn consume(_: &[[i32; 16]]) {}
# fn example(input: &[u8]) -> Result<(), Box<dyn std::error::Error>> {

let mut decoder = StreamDecoder::for_presentation(3)?;
for chunk in input.chunks(4096) {
    for frame in decoder.push_bytes(chunk)? {
        for presentation in frame.presentations.iter() {
            if presentation.decoded.is_duplicate {
                continue;
            }
            let pcm = &presentation.decoded.pcm_data
                [..presentation.decoded.sample_length];
            let metadata = &presentation.decoded.oamd;
            println!("AU at sample {}, {} OAMD payloads",
                presentation.position.absolute_sample, metadata.len());
            consume(pcm);
        }
    }
}
decoder.finish()?;
# Ok::<(), Box<dyn std::error::Error>>(())
# }
```

`finish` rejects input ending inside an access unit. Extraction, parsing, decoding,
and callback errors are terminal because stream state may already have changed;
call `reset` before a new stream. Configuration changes start a new timeline epoch,
while each effective presentation keeps its own absolute accepted-sample cursor.
Duplicate access units are emitted for observability but do not advance that cursor.
Consumers must skip their PCM and metadata, as in the example above. For branch
events, `branch.position(effective_presentation)` uses the same accepted timeline
as the frame's presentation; `branch.source` preserves the parser's source access-unit
information, whose `sample` counter includes duplicates.

Use `push_bytes_with` to handle each frame directly and avoid collecting a
`Vec<StreamFrame>` for each input chunk. Extraction, parsing, and decoding still
allocate memory internally.

Users can still drive the three stages directly: an `Extractor` finds
frames in a byte stream, a `Parser` turns each frame into an access unit, and a
`Decoder` renders access units to PCM.

On damaged input, `Parser::reset_for_next_major_sync` and
`Decoder::reset_for_next_major_sync` drop stream state so decoding can resume
at the next major sync. Call both at the same point in the frame sequence, or
the two stages will disagree about the stream.

## Development Status


| Category        | Feature                       | Status | Priority | Criticality  | Notes                         |
|-----------------|-------------------------------|--------|----------|--------------|-------------------------------|
| **Parser**      | FBA sync bitstream (Dolby)    | 🟢     | High     | Essential    |                               |
|                 | FBB sync bitstream (Meridian) | 🟢     | Low      | Nice-to-have | DVD-Audio / MLP               |
|                 | Evolution frame               | 🟢     | High     | Essential    |                               |
|                 | CRC and parity validation     | 🟢     | High     | Essential    |                               |
|                 | SMPTE timestamp               | 🟢     | Medium   | Optional     |                               |
|                 | FBA hires output timing       | 🟢     | Medium   | Optional     |                               |
|                 | Object audio metadata         | 🟢     | High     | Essential    | Bed object element skipped    |
|                 | FIFO conformance tests        | 🟢     | Medium   | Optional     |                               |
|                 | FBA bitstream seeking         | 🔴     | Low      | Nice-to-have | Yes, it's possible            |
| **Decoder**     | 31EA / 31EB sync substream    | 🟢     | High     | Essential    |                               |
|                 | 31EC sync substream           | 🟢     | High     | Essential    | 4th / 16ch presentation       |
|                 | Lossless check                | 🟢     | High     | Essential    |                               |
|                 | Optimize DSP performance      | 🔴     | Medium   | Important    |                               |
|                 | Dynamic range control         | 🔴     | Low      | Optional     | State parsed, not applied     |
|                 | Intermediate spatial format   | 🔴     | Low      | Out-of-scope | I have no idea                |
| **Other TODOs** | Documentation                 | 🟢     | High     | Essential    | With kind support from Claude |
|                 | Unit tests                    | 🟢     | High     | Essential    |                               |
|                 | Benchmarking                  | 🔴     | Medium   | Important    |                               |
|                 | Metadata interpolation        | 🔴     | Low      | Nice-to-have |                               |
|                 | Bitstream editing             | 🔴     | Low      | Nice-to-have |                               |
|                 | Encoding                      | 🔴     | Low      | Nice-to-have |                               |
|                 | Object audio rendering        | 🔴     | Low      | Out-of-scope |                               |

**Legend:** 🟢 Completed • 🟡 In Progress • 🔴 Not Started

---

## License

Licensed under the Apache License, Version 2.0. See [LICENSE](../LICENSE) for details.
