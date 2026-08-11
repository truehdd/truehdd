# truehdd
[![CI](https://github.com/truehdd/truehdd/workflows/CI/badge.svg)](https://github.com/truehdd/truehdd/actions/workflows/ci.yml)
[![Artifacts](https://github.com/truehdd/truehdd/workflows/Artifacts/badge.svg)](https://github.com/truehdd/truehdd/actions/workflows/release.yml)
[![Github all releases](https://img.shields.io/github/downloads/truehdd/truehdd/total.svg)](https://GitHub.com/truehdd/truehdd/releases/)

A command-line tool for decoding Dolby TrueHD audio streams.

**Language:** English | [简体中文](README.zh-CN.md) | [日本語](README.ja.md)

> ⚠️ **Experimental** 
> 
> This tool is designed for research and development purposes.  
> It is not intended for production environments or consumer playback systems.
> 
> 💡 **Got a new idea?**  
> 
> If you have ideas for useful features, please let us know by opening an issue or starting a discussion.


## Overview

`truehdd` is a command-line interface for the [truehd](truehd/) library, enabling decoding of Dolby TrueHD audio streams.
It also reads the Meridian Lossless Packing streams DVD-Audio carries, and checks either against the conformance rules with `verify`.

## Installation

### From Source

Requires Rust 1.95.0 or later:

```bash
git clone https://github.com/truehdd/truehdd
cd truehdd
cargo build --release
```

The compiled executable will be located at `target/release/truehdd`.

## Usage

```
truehdd [OPTIONS] <COMMAND>

Commands:
  decode    Decode the specified TrueHD stream into PCM audio
  info      Print stream information
  verify    Check the specified TrueHD stream against the conformance rules
  help      Print this message or the help of the given subcommand(s)

Options:
      --loglevel <LOGLEVEL>         Set the log level [default: info]
                                    [possible values: off, error, warn, info, debug, trace]
      --strict                      Treat warnings as fatal errors (fail on first warning)
      --log-format <LOG_FORMAT>     Log output format [default: plain]
                                    [possible values: plain, json]
      --progress                    Show progress bars during operations
  -h, --help                        Print help (see more with '--help')
  -V, --version                     Print version
```

## Commands

### `info` - Stream Analysis

Analyzes TrueHD streams and displays detailed information about their structure and properties without performing decoding.

**Usage:** `truehdd info [OPTIONS] <INPUT>`

```
Arguments:
  <INPUT>  Input TrueHD bitstream

Options:
      --loglevel <LOGLEVEL>      Set the log level [default: info]
                                 [possible values: off, error, warn, info, debug, trace]
      --strict                   Treat warnings as fatal errors (fail on first warning)
      --log-format <LOG_FORMAT>  Log output format [default: plain] [possible values: plain, json]
      --progress                 Show progress bars during operations
  -h, --help                     Print help (see more with '--help')
```

**Examples:**
```bash
# Analyze a TrueHD file
truehdd info movie.thd
```

### `verify` - Conformance Checking

Parses a whole stream and reports every conformance check that fires, rather than stopping
at the first. Each diagnostic prints its rule ID, severity, access unit and byte.bit
position, followed by a summary: a per-rule tally, the stream's format and FIFO peaks
against their caps, the branch points a splice leaves behind, which disc formats the stream
is legal for, and a verdict.

A stream can be a conformant bitstream and still be inadmissible on every disc format, which
is reported as `CONFORMANT, NOT DISC-AUTHORABLE` and exits 0: the disc rules and the codec
rules are different rules.

**Usage:** `truehdd verify [OPTIONS] <INPUT>`

```
Arguments:
  <INPUT>  Input TrueHD bitstream (use "-" for stdin)

Options:
      --fail-on <SEVERITY>       Worst severity that still exits 0 [default: error]
                                 [possible values: info, warning, error, fatal]
      --max-per-rule <N>         Stop printing after this many diagnostics of the same
                                 rule; 0 prints them all [default: 20]
      --json                     Print one JSON object per line instead of the report
      --summary-only             Print the summary alone
      --loglevel <LOGLEVEL>      Set the log level [default: info]
                                 [possible values: off, error, warn, info, debug, trace]
      --strict                   Treat warnings as fatal errors (fail on first warning)
      --log-format <LOG_FORMAT>  Log output format [default: plain] [possible values: plain, json]
      --progress                 Show progress bars during operations
  -h, --help                     Print help (see more with '--help')
```

**Examples:**
```bash
# Check a stream against the conformance rules
truehdd verify movie.thd

# Treat anything at or above a warning as a failure
truehdd verify --fail-on warning movie.thd

# One JSON object per diagnostic, for tooling
truehdd verify --json movie.thd
```

### `decode` - Audio Decoding

Decodes TrueHD streams into PCM audio.

**Usage:** `truehdd decode [OPTIONS] <INPUT>`

```
Arguments:
  <INPUT>  Input TrueHD bitstream (use "-" for stdin)

Options:
      --output-path <PATH>       Output path for audio and metadata files
      --format <FORMAT>          Audio format for output (presentation 3 always uses CAF)
                                 [default: caf] [possible values: caf, pcm, w64]
      --presentation <SELECTION> Presentations to decode: an index (0-3), a list (0,1,3), "all",
                                 or "max" for the highest available presentation [default: max]
      --no-estimate-progress     Disable progress estimation
      --bed-conform              Enable bed conformance for Atmos content
      --metadata-only            Write only object audio metadata, skipping PCM output
      --json                     Print a machine-readable result summary on stdout
      --warp-mode <WARP_MODE>    Specify warp mode when not present in metadata
                                 [possible values: normal, warping, prologiciix, loro]
      --probe-range <PROBE_RANGE>
                                 Access units to probe for Atmos metadata with --bed-conform
                                 [default: 12000]
      --evo-key <KEY>            Verify Evolution frame protection with this HMAC-SHA-256 key,
                                 given as hex or as @FILE holding hex
      --loglevel <LOGLEVEL>      Set the log level [default: info]
      --strict                   Treat warnings as fatal errors (fail on first warning)
      --log-format <LOG_FORMAT>  Log output format [default: plain]
      --progress                 Show progress bars during operations
  -h, --help                     Print help (see more with '--help')
```

**Output Files:**

By default, the maximum available presentation index is chosen for decoding.
When `--output-path` is specified, the tool generates appropriate output files:

- **Channel presentation:** One of the following files, with presentation index 0, 1, or 2
  - `output.caf` - PCM data in Core Audio Format
  - `output.pcm` - Raw PCM (if `--format pcm`)
  - `output.wav` - Wave64 format (if `--format w64`)


- **Object presentation:** Dolby Atmos master file set, with presentation index 3 (if available)
  1. `output.atmos` - Essential information about the presentation
  2. `output.atmos.audio` - Audio for all bed signals and objects in Core Audio format
  3. `output.atmos.metadata` - 3D positional coordinates for static and dynamic signals

  **Note:** Presentation 3 always uses CAF format regardless of `--format` option. Use `--bed-conform` to convert bed channels to 7.1.2 layout.


- **Multiple presentations:** selecting a list, `all`, or several presentations suffixes every output with its presentation index, for example `output_p1.caf` and `output_p3.atmos`, even when only one presentation turns out to exist. Selected presentations are decoded in a single pass, sharing the work their substreams have in common.

**Metadata Only:**

`--metadata-only` writes the `.atmos` header and `.atmos.metadata` for an object presentation and skips the audio file, which is useful for inspecting or collecting metadata without producing gigabytes of PCM. The metadata is identical to a full decode. The `.atmos` header still names the audio file it did not write, so the result is a metadata set rather than a loadable master. A presentation without object audio metadata writes nothing.

**Evolution Frame Protection:**

Evolution frames carry protection words holding a truncated HMAC-SHA-256 over the access unit and the frame itself. Checking them needs the key the encoder used, which is not built in, so `--evo-key` takes one as hex or as `@FILE` holding hex. Without it nothing is verified. A mismatch is a warning by default and is counted in the summary; under `--strict` the first one aborts the decode. Streams with no Evolution frame report zero frames checked rather than a failure.

**Machine-Readable Output:**

`--json` prints a single result object on stdout when decoding finishes, so a
calling program does not have to guess which files were written:

```json
{
  "version": "0.5.0",
  "input": "movie.thd",
  "frames": 225526,
  "skippedFrames": 0,
  "branches": 0,
  "invalidBranches": 0,
  "evoChecked": 0,
  "evoFailed": 0,
  "samples": 9021040,
  "sampleRate": 48000,
  "presentations": [
    {"index": 3, "format": "damf", "channels": 12,
     "files": ["out.atmos", "out.atmos.audio", "out.atmos.metadata"]}
  ]
}
```

`channels` is `null` until the channel count is known. `skippedFrames` counts
frames the extractor could not use and resynchronised past. `branches` counts seamless branch points that satisfy the decoder buffer
model and `invalidBranches` those that do not; the latter is a conformance
finding and does not change the decoded samples. Logs stay on stderr, so
stdout carries only this object. Use `--log-format json` for machine-readable
logs as well.

The exit code identifies which stage failed:

| Code | Meaning |
| --- | --- |
| 0 | Success |
| 1 | Unspecified failure |
| 2 | Invalid command line |
| 3 | Input could not be read |
| 4 | Bitstream could not be parsed |
| 5 | Audio could not be decoded |
| 6 | Output could not be written |
| 7 | Stream is non-conformant (`verify` only) |

With `--strict`, skipped frames are treated as a failure as well.

**Damaged Streams:**

A frame that fails to parse or decode is reported and skipped, and decoding resumes at the next major sync instead of aborting. Pass `--strict` to fail on the first problem instead.

**Warp Mode Options:**

The `--warp-mode` option controls how Dolby Atmos content handles downmix rendering when the metadata doesn't specify a warp mode:

- `normal` - Direct render
- `warping` - Direct render with room balance  
- `prologiciix` - Dolby Pro Logic IIx
- `loro` - Standard (Lo/Ro)

This option only applies when the original OAMD metadata lacks warp mode information. If warp mode is already present in the metadata, this option is ignored.

**Examples:**
```bash
# Decode a TrueHD file with progress
truehdd decode --progress audio.thd --output-path decoded_audio

# Decode with specific warp mode for content missing this metadata
truehdd decode --warp-mode prologiciix audio.thd --output-path decoded_audio

# Decode every available presentation in one pass
truehdd decode --presentation all audio.thd --output-path decoded_audio

# Decode from ffmpeg pipe
ffmpeg -i movie.mkv -c copy -f truehd - | truehdd decode - --output-path audio
```

## License

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE) for details.