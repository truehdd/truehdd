# Changelog

All notable changes to the truehd library crate will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.0] - 2026-08-01

### Added
- `Decoder::decode_presentations()` decoding several presentations from one access unit in a single pass, and `PresentationMap` for resolving which substreams a presentation needs
- `Parser::invalid_branches()` counting seamless branch points that fail the buffer-model checks; the decoded samples are unaffected, so callers can treat it as a conformance signal
- `Parser::reset_for_next_major_sync()` and `Decoder::reset_for_next_major_sync()` to recover from fatal parse/decode failures at the next major sync
- `Extractor::error_count()` exposing the number of corrupt-frame events
- `Parser::substream_state()` read-only accessor for per-substream state (DRC gain/time values)
- OAMD object distance parsing: `ObjectRenderInfo::distance_factor` resolved via the new `DISTANCE_FACTORS` table
- `PartialEq`, `Eq`, `Hash` derives on `SpeakerLabels`

### Changed
- **BREAKING**: `DecoderState::decode_access_unit()` is replaced by `decode_access_unit_presentations()`; use `Decoder::decode_presentation()` or `Decoder::decode_presentations()` instead
- Minimum supported Rust version is now 1.88.0 (let chains; was already required, now declared correctly)
- Updated dependency floors (anyhow, bitstream-io 4.10, log, thiserror)
- **BREAKING**: `ObjectRenderInfo` fields `b_object_at_infinity` and `distance_factor_idx` replaced by `distance_factor: Option<f64>`
- `Extractor` internal buffer reworked (Vec + cursor with amortized compaction) removing per-resync allocations

### Fixed
- Correct substream 0 size calculation for substream size history
- Correct `TRIM_LUT[14]` value (-16.0 -> -15.0) (#26, fixed by @yuygfgg in #27)
- No longer panic on corrupt bitstreams: invalid `restart_sync_word`, division by zero in seamless-branch timing, and substream size/length underflows now return typed errors (#15, #16)
- Persist heavy DRC gain/time updates in parser substream state; heavy DRC validation previously compared against zeroed values
- `decode_presentations()` no longer copies OAMD payloads into non-object presentation results

The panic-free parsing, recovery API, DRC state, OAMD distance and extractor buffer changes above are based on work by @harletty in the harletty-bridge fork.

## [0.4.0] - 2025-08-15

### Added
- `substream_info_changed` field to `DecodedAccessUnit` and `DecoderState` to track substream info changes
- `has_substream_info_changed` field to `ParserState` to track substream info changes

### Changed
- `SubstreamInfoMismatch` and `ExtendedSubstreamInfoMismatch` error level from Error to Warn

## [0.3.1] - 2025-08-12

### Fixed
- Corrected position coordinates for SpeakerLabels

## [0.3.0] - 2025-08-03

### Added
- AccessUnit struct now includes `has_valid_branch` field to indicate valid branch points
- Duplicate sample detection at TrueHD stream branch points. DecodedAccessUnit struct now includes `is_duplicate` field. Such access units should be discarded

### Fixed
- Lossless check failures are now allowed at valid branch points to prevent false positive warnings
- Fix iterator borrowing issue in `ParserState::reset_for_branch()` method

### Changed
- **BREAKING**: Renamed seamless branch related struct fields for clarity
  - `ParserState::has_branch` → `peak_data_rate_jump`
  - `ParserState::has_jump` → `has_valid_branch`
  - Updated field usage throughout parser and decoder states for consistent naming
  - Enhanced branch validation logic in restart header processing
- **BREAKING**: `PresentationMap::max_independent_presentation()` now returns `Option<usize>` instead of `usize`
  - Returns `None` when no independent presentations are available
  - Improves error handling for invalid presentation configurations
- Extract jump detection logic into `ParserState::has_jump()` method for better code organization

## [0.2.1] - 2025-07-23

### Fixed
- Seamless branch validation logic in restart header - corrected inverted conditions that caused incorrect validation warnings

## [0.2.0] - 2025-07-22

### Added
- Level-based error handling system with configurable failure thresholds
- `set_fail_level()` methods on `Parser` and `Decoder` structs for configuring error handling behavior
- AU length validation
- Seamless branch validation
- Substream info validation
- Fixed data rate validation

### Fixed
- **BREAKING**: Corrected `coeff_q` for filter A

### Changed
- **BREAKING**: Replaced `fail_on_warning: bool` with `fail_level: log::Level` in `ParserState` and `DecoderState`
- **BREAKING**: `ParserState::default()` now uses `log::Level::Error` instead of `fail_on_warning: false`
- **BREAKING**: `DecoderState::default()` now uses `log::Level::Error` instead of `fail_on_warning: false`

