# Changelog

All notable changes to the truehd library crate will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- AccessUnit struct now includes `has_valid_branch` field to indicate valid branch points

### Fixed
- Lossless check failures are now allowed at valid branch points to prevent false positive warnings

### Changed
- **BREAKING**: Renamed seamless branch related struct fields for clarity
  - `ParserState::has_branch` → `peak_data_rate_jump`
  - `ParserState::has_jump` → `has_valid_branch`
  - Updated field usage throughout parser and decoder states for consistent naming
  - Enhanced branch validation logic in restart header processing

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

