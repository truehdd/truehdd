# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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