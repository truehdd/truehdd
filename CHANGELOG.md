# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.1] - 2026-08-01

### Fixed
- `cargo install truehdd` failed to compile because the version string required git metadata that a published crate does not carry; builds outside a checkout now report the package version. This affected every published version

## [0.5.0] - 2026-08-01

### Added
- `--presentation` accepts a list (`0,1,3`), `all`, or `max` in addition to a single index; multiple presentations decode in a single pass with shared extraction and parsing, writing one output per presentation with `_p{index}` filename suffixes
- Extraction, parsing and decoding run on separate threads
- Decode now recovers from mid-stream corruption: after a parse or decode failure, both stages reset in lockstep and resume at the next major sync instead of continuing on damaged state
- `--metadata-only` writes the Dolby Atmos master header and metadata without the audio file (#10)
- `--json` prints a result summary on stdout listing the files written per presentation, along with frame, sample, skipped-frame and seamless-branch counts, including branch points that fail the buffer-model checks
- Exit codes now identify the failing stage: 3 input, 4 parse, 5 decode, 6 write
- `--probe-range` sets how many access units are probed for Atmos metadata when `--bed-conform` is used (default 12000)

### Changed
- Minimum supported Rust version for building the CLI is now 1.95.0
- Updated dependencies (clap 4.6, vergen-gitcl 10, darling 0.24 with syn 3 in truehdd-macros)
- Decode pipeline errors now travel in-band with the data: failures report the originating stage (input/parse/decode/write) instead of always "Write error", and output file headers are finalized even when decoding fails, keeping partial output playable
- The default `--presentation` is now spelled `max`; it requests the same presentation as the previous default of `3`, including the fallback used when a stream has no presentation 3
- **BREAKING**: `--format` now applies to whichever presentation is written, except presentation 3 which always uses CAF; it was previously ignored whenever presentation 3 was requested, so a stream without presentation 3 produced CAF regardless of the option
- **BREAKING**: `--strict` treats frames the extractor had to skip as a failure, so it now exits non-zero on input it previously accepted

### Fixed
- The presentation list parser failed to build on Windows, where an extra `FromIterator` implementation made the element type ambiguous
- Errors are reported on stderr even when logging is turned off
- DAMF YAML no longer corrupts file references containing double spaces, `- ` or single quotes, and keeps quoting for names that need it so the output stays valid YAML (#17, #18)
- Fields in `--log-format json` output are escaped, so paths and messages containing quotes no longer break the JSON
- Removed a shutdown race that could report success after an unreported pipeline error

The DAMF YAML fix above builds on @nekno's report and first fix in #18.

## [0.4.0] - 2025-08-15

### Added
- Handle substream info changes that cause channel count changes by creating segmented output files with `_{AU_index}` suffix

## [0.3.0] - 2025-08-12

### Added
- `--warp-mode` option to specify warp mode when not present in metadata

## [0.2.0] - 2025-08-12

### Added
- Wave64 (w64) format support for audio output with `.wav` extension
- `--bed-conform` flag for Dolby Atmos content to conform bed channels to 7.1.2 layout

### Changed
- **BREAKING**: `--format` option is now ignored for presentation 3, which always uses CAF format
- DAMF header files are now created immediately when Atmos is detected rather than at the end of processing
- Build timestamps now respect SOURCE_DATE_EPOCH for reproducible builds (thanks @al3xtjames)

### Fixed
- Corrected bed channel assignments for 7.1.2 configuration in Atmos content

## [0.1.3] - 2025-08-03

### Fixed
- Atmos output files now get correct extensions when OAMD is detected after initial file creation
- PCM format files are properly wrapped with CAF headers when Atmos content is discovered
- Resolved format corruption where PCM files contained CAF data due to late Atmos detection

## [0.1.2] - 2025-07-22

### Changed
- Connect `--strict` mode to level-based error handling
- Add GNU Linux targets to CI for better performance

## [0.1.1] - 2025-07-21

### Fixed
- Fixed incorrect field mapping for `front_back_balance_listener` in DAMF output
- Fixed example usage in documentation

## [0.1.0] - 2025-07-21

### Added
- Initial release