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

## Installation

### From Source

Requires Rust 1.87.0 or later:

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
  help      Print this message or the help of the given subcommand(s)

Options:
      --loglevel <LOGLEVEL>         Set the log level [default: info]
                                    [possible values: off, error, warn, info, debug, trace]
      --strict                      Treat warnings as fatal errors (fail on first warning)
      --log-format <LOG_FORMAT>     Log output format [default: plain]
                                    [possible values: plain, json]
      --progress                    Show progress bars during operations
  -h, --help                        Print help (see a summary with '-h')
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
...
```

**Examples:**
```bash
# Analyze a TrueHD file
truehdd info movie.thd
```

### `decode` - Audio Decoding

Decodes TrueHD streams into PCM audio.

**Usage:** `truehdd decode [OPTIONS] <INPUT>`

```
Arguments:
  <INPUT>  Input TrueHD bitstream (use "-" for stdin)

Options:
      --output-path <PATH>       Output path for audio and metadata files
      --format <FORMAT>          Audio format for output [default: caf] [possible values: caf, pcm]
      --presentation <INDEX>     Presentation index (0-3) [default: 3]
      --no-estimate-progress     Disable progress estimation
...
```

**Output Files:**

When `--output-path` is specified, the tool generates appropriate output files:

*Regular TrueHD streams:*
- `output.caf` - Core Audio Format
- `output.pcm` - Raw 24-bit PCM (if `--format pcm`)

*Dolby Atmos streams:*
- `output.atmos` - DAMF header file
- `output.atmos.audio` - Audio data in CAF format
- `output.atmos.metadata` - DAMF metadata file

**Examples:**
```bash
# Decode a TrueHD file with progress, without output
truehdd decode --progress audio.thd --output-path decoded_audio

# Decode from ffmpeg pipe
ffmpeg -i movie.mkv -c copy -f truehd - | truehdd decode - --output-path audio
```

## License

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE) for details.