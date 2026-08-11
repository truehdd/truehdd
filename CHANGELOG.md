# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- `truehdd verify <input>` parses a whole stream and reports every conformance check that fires, rather than stopping at the first like `decode --strict` does. Each diagnostic prints its rule ID, severity, access unit and byte.bit position, followed by a summary with a per-rule tally, the stream's format and FIFO peaks against their caps, and a verdict
- `verify --fail-on <fatal|error|warning|info>` sets the worst severity that still exits 0, default `error`. A stream that parses but violates a rule at or above it exits with the new code 7; a stream that cannot be parsed to its end exits 4
- `verify --max-per-rule <N>` stops printing after N diagnostics of one rule, default 20, `0` for no limit. Every diagnostic is counted whatever the limit, so the tally, the summary and the exit code do not change with it. Each limited rule prints one line naming how many were shown and suppressed and the first and last access unit
- `verify --json` writes JSON Lines: one object per diagnostic, one per rate-limited rule, and a final summary object
- `verify --summary-only` prints the summary alone
- The global `--strict` acts as `--fail-on warning` under `verify`. It does not lower the parser's fail level as it does under `decode` and `info`, which would end the access unit at the first warning and hide every check after it
- `verify` silences the library's own logging, which would otherwise print every check a second time and bury the report. `--loglevel debug` or higher restores it
- `verify` reports the maximum data rate and the access unit it peaked at, the maximum FIFO latency, access-unit size, and a per-substream table of depth against the depth allowed for that substream. A substream whose channel span is empty carries no channel but still carries headers, and is allowed a flat 5000 bytes for them. The FIFO totals are broken into the stream's own bytes plus overhead, and labelled by the decoder each one belongs to, each summing only the substreams that decoder's presentation is made of. A decoder the stream does not carry reports no depth: a DVD-Audio stream whose only substream is six-channel has no two-channel decoder, and says so
- `verify` lists every branch point, where the stream's timing restarts as a splice leaves it: the access unit, its byte offset, its place in the running time, the advance it asks for, and whether it is seamless. A branch that is not names which of the buffer-model conditions it broke: advance step, FIFO duration, the 75 ms limit or the peak data rate. A stream with no splice in it reports no section rather than a zero
- `verify` reports which disc formats the stream is legal for: DVD-Audio, HD DVD-Video and BluRay. Substream 0 must restart on 0x31EA and every presentation must number its channels without a gap or an overlap, or the stream is legal for none of them. At 176.4 and 192 kHz the channel counts are what decide it, BluRay allowing six and HD DVD-Video two, measured both from the declared channel assignment and from the substreams themselves; BluRay does not admit 44.1, 88.2 or 176.4 kHz at all, and takes no DVD-Audio stream. A DVD-Audio stream's own verdict also rests on its 6-channel downmix not clipping, which no parse can settle, so the row states that condition rather than guessing. A stream no format admits is reported as `CONFORMANT, NOT DISC-AUTHORABLE` and still exits 0: the disc rules are not the codec rules, and such a stream decodes correctly and is legal in a file, so it is a state of its own rather than the error an encoder raises when asked to author it
- `decode --evo-key` verifies Evolution frame protection against a supplied HMAC-SHA-256 key, given as hex or as `@FILE`. Mismatches warn and are counted as `evoChecked` and `evoFailed`, or abort under `--strict`

### Fixed
- `info` and `verify` disagreed about how to print the same three things. The format sync is now named and then given in hex in both, `FBA (F8726FBA)`, where `info` printed the bare word; every byte offset is upper-case hex in both the tables and the diagnostic lines, where the two differed in case; and a branch offset is no longer padded to eight digits, which invented leading zeros under 4 GB and stopped being uniform over it. Offsets have always been 64-bit, so a stream past 4 GB reports the right value and its column simply widens
- The CAF `chan` chunk was written without its channel-description count, four bytes every reader expects between the layout tag and the descriptions. The chunk was therefore malformed and the layout ignored altogether: Core Audio reported "no channel layout" for every file this ever wrote. The chunk is now written per the specification, so the layout is read, and the unused bitmap field is zero instead of a stray bit. Only the chunk changes; the PCM payload of every output is byte-identical
- `decode --format caf` picked the channel layout tag from the channel count alone, so it described a layout the samples were not in. A DVD-Audio (FBB) 5.1 stream decodes as `L R Ls Rs C LFE` and was tagged `MPEG_5_1_A` (`L R C LFE Ls Rs`), which routes centre to the surrounds on a player that honours the tag; it is now `MPEG_5_1_B`. An 8-channel TrueHD presentation decodes as `L R C LFE Ls Rs Lb Rb` and was tagged `MPEG_7_1_A`, whose last pair is a front centre pair, putting the rear surrounds in front of the listener; it is now `MPEG_7_1_C`. The tag now comes from the channel labels the decoder reports, an order no standard tag names is written as channel descriptions, and an order the stream does not state leaves the file without a layout rather than with a guess. Samples are never reordered to fit a tag
- `info` printed no channel order at all for a DVD-Audio (FBB) presentation, where a TrueHD one prints `Channel assignment`. It now prints the same line, from the same channel labels the audio writer describes the file with, so the two cannot disagree. A presentation whose order is not known prints nothing
- `decode` with the default `--presentation max` wrote a one-channel file of wrong PCM for a DVD-Audio (FBB) stream that declares a single decodable substream. The presentation map was derived by the FBA rules, which invented presentations the stream does not have, and the decode resolved to one of them. The default now decodes the presentation the stream declares
- `info` reported the FBA `channel_meaning` fields for a DVD-Audio stream, which put an entirely different structure in those 64 bits, so dialogue and mix levels came out of bits that mean nothing of the sort. It now lists only the presentations FBB declares, without the FBA per-presentation metadata that syntax does not carry. The FBB block those bits really hold is parsed but not printed: outside `hdcd_process` its fields have no reading this crate can demonstrate, and raw values under their bitstream names told a reader nothing
- `info` reported nothing at all about trimming when a stream carries no high-resolution timing field, which reads the same as a stream that signals no trim. It now says the field is not signalled, so "starts at zero" and "does not say" are distinguishable
- Building the Dolby Atmos metadata panicked on three OAMD shapes it does not yet handle: multiple object info blocks, multiple bed instances, and intermediate spatial format objects. Each now reports what is unsupported, and the decode exits 6 with output files finalized instead of aborting. Streams that decoded before are unaffected. This affected every version

## [0.5.3] - 2026-08-04

### Fixed
- Decoding a stream that does not carry the requested presentation wrote no output at all and still reported success. The default `--presentation max` asks for index 3, so any stream with fewer than four substreams — a plain stereo or 5.1 TrueHD stream, for example — logged `Presentation 3 is not available, using presentation 0`, then finished with `0 frames, 0 samples` and exit code 0 without creating a file. An explicit `--presentation` naming an absent index behaved the same way. Output is byte-identical to 0.4.0 again. This affected 0.5.0 through 0.5.2

## [0.5.2] - 2026-08-04

### Fixed
- Decoding Atmos without `--bed-conform` wrote an audio file header that understated the channel count by the number of bed channels, while the PCM payload still carried every channel. An LFE-only bed with 11 objects produced a CAF declaring 11 channels for 12-channel interleaved data, so every frame boundary but the first fell in the wrong place and Dolby Atmos tooling rejected the master (#29). The count now comes from the decoded count, so a bed assignment naming more channels than the presentation carries cannot skew it either
- An output path whose name contains a dot lost everything after the last dot when the audio and metadata extensions were added, so `--output-path Movie.2024.1080p` wrote `Movie.2024.atmos.audio` while the Dolby Atmos master header referenced `Movie.2024.1080p.atmos.audio`, leaving the master incomplete. Non-Atmos output was misnamed the same way (`Movie.2024.caf`). This affected 0.5.0 and 0.5.1

The channel count fix is @sven-pke's, from #29.

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