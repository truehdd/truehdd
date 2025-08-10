# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed
- DAMF header files are now created immediately when Atmos is detected rather than at the end of processing

## [0.1.3] - 2025-08-03

### Added
- CAF wrapping functionality for post-processing PCM files into proper CAF containers
- Duplicate samples at seamless branch points caused by binary concatenation are now discarded

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

### Documentation
- Added CI badges to README files
- Fixed example usage in documentation

## [0.1.0] - 2025-07-21

### Added
- Initial release