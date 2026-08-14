# Changelog

All notable changes to the truehd library crate will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.7.1] - 2026-08-15

### Added
- `Parser::take_branches` returns the branch points recorded so far and leaves the list empty, mirroring `take_diagnostics`. The list grows for the life of the parser, one entry per point the stream's timing restarts at. A pass over a file reads it at the end and its size is bounded by the file, but a consumer that runs for as long as something is playing has no end to read it at, and a stream that restarts its timing often enough grows it without bound. Take it between access units rather than during one: a jump is reported once per substream that reads its restart header, and the records are merged through the last entry in the list, so emptying it part way through an access unit would record that access unit's branch twice and lose the merge
- `Parser::set_check_fifo` turns the byte-domain FIFO depth model off, alongside the `set_allow_seamless_branch` switch already there. The model reports whether a stream is legal to author, which a conformance pass asks and a decoder does not, and the decoded samples are the same either way. The default is unchanged, and like the other parser settings it survives a major sync reset

### Changed
- `log_or_err!` builds a check's error only where it is used: returned, collected, or logged at a level a logger is listening to. It bound the error into a local before deciding, so every check that fired built one, and building one allocates because the call sites box. A check evaluated per access unit rather than per stream paid this for the length of the stream while reporting nothing: a stream declaring a peak data rate over the ceiling exceeds it on every access unit, so a consumer at the default fail level with no `Warn` logger allocated and dropped an error every access unit. Callers of the exported macro whose error expression has a side effect will now see it run only when the error is used; every call site in this crate is a plain construction of an error value, so nothing else changes

## [0.7.0] - 2026-08-11

### Added
- `truehd/tests/assets/fba_atmos_dimtrim.mlp`, a stream whose object metadata carries the dimensional trim and extended object elements, which most encodes leave out. The two were only exercised by crafted payloads before
- `HeadphoneElement` and `ObjectDescriptionElement`, the OAMD elements with `oa_element_id` 3 and 4, which were skipped with a warning. The first carries a headphone rendering mode and a head-tracking flag per object and block, stated once for the payload in three of its four modes and coded per block in the fourth, where a run of ISF objects shares one intent. The second carries `object_dialog_indication`, in two coded forms and a reserved form that states nothing. Both are read from `ObjectAudioMetadataPayload`. An element the payload does not carry is `None`, and `oa_element_id` 6 is still skipped
- `RestartHeaderError::InvalidHiresOutputTiming`: a malformed high-resolution output timing field was detected and only logged, so a stream carrying one passed with no diagnostic and no effect on a verdict. It is now reported, naming the access unit the field started in rather than the one the fault surfaced at. The field is decoded per substream, each substream serialising its own over its own restart headers, so a malformed one is reported once per substream and a substream disagreeing with the first is no longer unread
- `RestartHeaderError::InvalidHiresOutputTimingSequence`: a field that decodes but does not continue the one before it was logged and nothing more, so a stream whose high-resolution timing runs backwards or skips passed with no diagnostic. It is now reported like the malformed-field checks beside it
- `AccessUnitError::HiresOutputTimingMismatch`: every substream of an access unit carrying a major sync must state the same `hires_output_timing` bit, and nothing compared them. Each disagreeing pair is now reported. FBA only, and only at a major sync, which is what makes the comparison sound: a substream that restarts less often is otherwise still holding the bit from its last restart header
- `Parser::branches` returns every point the stream's timing restarts at, each carrying its access unit, byte offset, sample position, the advance it asks for, and which of the four buffer-model conditions it met. `Parser::invalid_branches` is now derived from that list, and counts a branch once however many substreams saw the jump
- `Parser::max_data_rate`, `max_data_rate_au_index`, `max_fifo_latency` and access-unit size maxima, and `fifo_depth_detail` returning each accumulator's own bytes separately from the header and extra-data overhead that rides with it. The per-substream depth these expose matches the figures an encoder reports
- `Parser::set_allow_seamless_branch` turns off the tolerance that lets a major sync excuse the timing and output-timing checks. Five checks were unreachable without it, since the tolerance defaulted on and had no setter. The default is unchanged
- The restart gap, the number of access units between one major sync and the next, is now tracked in `ParserState::restart_gap` (a four-deep history, most recent first) and checked: a gap must be 1 or at least 8. The field was declared and never read. The rules over *runs* of short gaps, and their relaxation for a spliced stream, are not implemented: only their wording is known, not the conditions that raise them
- `ExtraData::verify_evo_protection` checks an Evolution frame's primary protection word against a supplied key, returning `EvoProtectionStatus`. Behind the optional `evo-protection` feature, off by default
- `ExtraData::evo_hmac_message` returns the bytes the protection digest covers, with `ExtraData::extra_data_offset` and `EvoFrame::protection_offset`
- FBB (Meridian / DVD-Audio) stream support. Parsing bailed with `unimplemented!` at three sites, so an FBB stream could not be read at all. The major sync now takes the same path as FBA, the channel-assignment bound applies to both formats while the identity-permutation rule stays FBA-only, and `AccessUnitError::FbbSyncTooFar` covers FBB's tighter 32-access-unit repetition limit. `EXAMPLE_DATA_FBB` and `EXAMPLE_DATA_FBB_UNEXTRACTABLE` are the fixtures
- Byte-domain FIFO depth model in `utils::fifo`, checked per access unit and read with `Parser::fifo_depth_peaks`. Five accumulators, one for each presentation the stream can declare plus the whole stream, each carry a byte cap, reported as `FifoError` when exceeded. Every accumulator sums the substreams its own presentation is made of, the same mask the decode path resolves, so a presentation that one independent substream carries whole is not charged for the substreams below it. This is where the model departs from the figures an encoder reports for its 6-channel sum, which counts substream 0 in whatever the layout
- `utils::diagnostic`: a `Diagnostic` record for every conformance check that fires, carrying its `RuleId`, severity, `Location` and message, plus the error itself. A `Location` names the access unit and its byte offset, and the bit the check fired at wherever the check holds the bit reader. `RuleId` names one rule per error variant, rendered as `domain.rule`, and is derived from the error enums so it cannot drift from them
- `Parser::set_diagnostic_mode`. In `DiagnosticMode::Collect` a check that fires is recorded rather than only logged, read with `Parser::diagnostics` or `Parser::take_diagnostics`. `DiagnosticMode::FailFast` is the default and is the previous behaviour exactly
- `Parser::parse_recovering` parses an access unit without returning its failure: the check is recorded, the parser resets, and parsing resumes at the next major sync, so one bad access unit no longer ends the stream
- `Frame::index` and `Frame::offset` carry each access unit's position in the extracted sequence and its byte offset from the first byte pushed, with `Extractor::stream_position` for the current read point
- `DecodeError::OutputsExceedMaxBits`: a restart header states in `max_bits` how many bits the substream's outputs use, sign apart, and the decoded samples must fit it. The field was read and its two occurrences compared with each other, but never with anything it describes. Checked once per substream per access unit, where the outputs exist, and reported as a warning: the samples are unaffected
- `SubstreamError::SampleCountMismatch`: the blocks of a substream segment must decode exactly one access unit of samples between them, whatever sizes they are split into. The count was never compared, so a segment could decode a short access unit and only be caught, indirectly, by its own parity and CRC
- `truehd/tests/decode_verify.rs`: decode-correctness tests over embedded stream slices. The PCM digests are derived from the encoder's source WAVs wherever a lossless source exists, so they pin decoding as the encoder's inverse rather than as the decoder's own output; the corruption tests prove the `lossless_check` comparison fires

### Fixed
- Three object metadata fields were decoded wrongly, none of them reachable from a conformant stream. A position is a code clamped to its own range, and it is the clamped code the next block's differential applies to, so a stream whose positions walk past an edge diverged on every block after it. `zone_constraints_idx` 7 is not a zone constraint and reads as 0. `object_div_code` 0 is a legal way to say no divergence, and warned instead
- `extra_data` was read as an Evolution frame whatever it held. It is a container with no type of its own: a zero header word is padding, and with `flags` bit 12 clear a non-zero one selects an opaque payload of `extra_data_length` words carrying no parity byte and no zero requirement. A stream of either other shape was reported as malformed, three of the findings fatally, and its parity mismatched on every block before the read ran on into the next access unit, while a truncated or overlong Evolution block went unreported. The opaque payload is now kept in `ExtraData::payload`, and the parity byte is only looked for where one exists
- `extra_data` was skipped whenever exactly one 16-bit word was left in the access unit, so a lone trailing block was never checked at all. `extra_data` is entered whenever a whole word remains: a block whose header nibble fails, or which declares a length it has no room for, is an error there like anywhere else
- A block declaring `evo_frame_byte_length` of zero had a frame read out of its padding. Zero is how a block flagged for Evolution says it carries no frame this access unit; its digest is not checked and it contributes nothing to an Evolution sink. `evo_frame` is now `None`
- `ExtraDataError::ExtraDataTooLong` and `EvoFrameTooLong` were reported and then parsed through, reading a frame and a parity byte out of whatever followed the access unit. Both now abandon the block. `ExtraDataError::EvoFrameNoRoom` covers the case that can only be rejected by reading an evolution header from beyond the access unit first
- A branch the buffer-model conditions accept left the window pricing every later arrival against a playhead a whole wrap ahead of it, so a legally spliced stream underran on every access unit after the seam. The access unit carrying the branch is unwrapped before the branch is judged, against the alignment the run before it left behind; it is now re-anchored once the deviation that aligns the two clocks is known. A stream that is legally spliced now reports nothing
- The byte-domain FIFO model was carried across a branch it had rejected, so a spliced stream underran on every access unit after the first join and its depths came out a third of what a decoder actually holds. A branch the buffer-model conditions reject restarts the model: the timing and the window begin again at that access unit as if it opened the stream, keeping the peaks already reached. A branch they accept keeps the window, as it must. Depths, per-substream peaks, the maximum data rate and its access unit, and the maximum latency are now right for a spliced stream
- The FBB substream-0 FIFO cap table was mistranscribed: two entries were transposed and the two-substream entry was dropped to zero, so every conformant stream of the shape most DVD-Audio content uses was reported as exceeding a cap of zero on each access unit. A cap belongs to a decoder rather than a substream, being 15000 bytes per channel of the presentation it reconstructs
- `AccessUnit::get_channel_labels` read the FBA channel-assignment fields whatever the syntax, so an FBB (DVD-Audio) presentation was described with fields `read_fbb` never populates: a six-channel presentation came back with no labels at all, and a caller had nothing to describe the audio with. FBB is now handled separately, and reports the order for what has been measured against this decoder's own output. A six-channel presentation of a stream carrying `fbb_channel_assignment` 20 decodes as `L R Ls Rs C LFE`, surrounds before centre and LFE, and a first substream may be mono or stereo. Every other arrangement is left unstated rather than assumed, so a caller writes no channel order rather than a wrong one
- `flags` was judged by the FBA rules whatever the syntax. Bit 14 says which syntax a stream is and must be clear in FBA and set in FBB, so it looked like a reserved bit set on every major sync of every conformant DVD-Audio stream. The reserved masks are now per syntax, FBA leaving bits 0-10 and 13 clear and FBB bits 0-13, and bit 14 is checked as the marker it is, reported as `SyncError::InvalidFlagsSyntaxMarker`
- `flags` constancy compared all sixteen bits. Only the meaningful ones have to be constant, bits 11, 12, 14 and 15 in FBA and 14 and 15 in FBB. A reserved bit that changes between major syncs is a reserved-bit violation, not a change of configuration
- The FBA `channel_meaning` layout was read for FBB streams too. FBB puts a different 64-bit structure there, `fs`, `wordwidth`, `channel_occupancy`, `mlp_multi_channel_type`, `speaker_layout`, `copy_protection`, `level_control`, `hdcd_process`, `source_format` and `summary_info`, so every `channel_meaning` field reported for a DVD-Audio stream was meaningless. Both blocks are 64 bits, which is why the CRC and everything after it still came out right. The seven bits from `hdcd_process` on are expected to be zero and are reported as `SyncError::ReservedChannelMeaningNonZero`
- FBB has no `extended_substream_info`: the four bits before `substream_info` are wholly reserved, and reading them as one made a conformant stream look incompatible with itself. They are now reported as `SyncError::ReservedBeforeSubstreamInfo`
- FBB `substream_info` was judged by the FBA rules: its low two bits were read as reserved, its upper bits as a presentation derivation, and the whole byte as a FIFO cap index. In FBB only the low nibble is defined, only 4, 5, 7 and 13 are legal, and bit 3 alone says anything about presentations: it means substream 1 is also to be decoded. So `0x05` legitimately declares exactly one decodable substream, and used to be reported as a reserved-bits violation on every major sync
- The presentations an FBB stream declares came from the FBA derivation, which invented a second presentation as a copy of the first and a third the stream does not have. A decode asking for the highest presentation resolved to one of those, decoding a substream that is not there
- The 8-channel and 16-channel `flags` bits (11 and 12) are FBA features; they are reserved in FBB and are now reported as such
- `output_timing` was only compared between the substreams a decode happened to parse. The presentation mask skips whole segments, so a request for a lower presentation left the higher substreams' restart headers unread and the stream free to disagree with itself. Every substream present in the access unit is now compared against the first, a skipped one by reading no further into its segment than the field, and `RestartHeaderError::OutputTimingMismatch` names both substreams and both values
- Reading an FBB stream whose previous restart header set `heavy_drc_present` panicked with `unimplemented!`. The field is FBA-only and must be false in FBB, so it is now reported as `RestartHeaderError::HeavyDrcPresentInFbb` and parsing continues; nothing follows the field in FBB. This was reachable from the moment FBB streams became readable, and was the last `unimplemented!` in the crate
- `heavy_drc_present` was taken from the previous restart header rather than the one being read, and was left standing where `flags` bit 13 says the stream carries no heavy DRC at all. What follows the field belongs to the header that carries it: twelve bits of gain and time update where it is set, twelve reserved bits where it is not, and where bit 13 is clear the field itself is reserved and false. In FBA both readings consume the same twelve bits, so no stream lost its place, but the DRC state a decoder tracks was taken from reserved bits or missed altogether; in FBB, where nothing follows the field, the reader could be left twelve bits out of step
- The high-resolution output timing sequence check subtracted in native width, so a field that ran backwards, the very thing the check exists to catch, panicked under debug overflow checks instead of reporting. The field is the high half of a 32-bit sample position, so the comparison and the stream-start calculation are now 32-bit and wrapping
- High-resolution output timing findings report at info rather than warning. A stream carrying no such field is legal, and these are informational outside an encoder
- Nine conformance checks were bare log calls, so no caller could see them under `--strict`, count them by rule or place them in the stream: the peak data rate ceiling, the four seamless-branch cause checks, the termination word and the two checks inside it, and DRC time update against DRC count. Each now reports a typed error at the bit it fired at, still at the level it logged at before
- The heavy DRC time update check was a bare log call, so no caller could see it under `--strict`, count it by rule or place it in the stream. It is now `RestartHeaderError::HeavyDrcTimeUpdateExceeded`, reported at the bit it fired at and still at the level it logged at, like its non-heavy twin
- The two DRC start-up gain checks were bare log calls, so no caller could see them under `--strict`, count them by rule or place them in the stream. A `channel_meaning` restated at a major sync may not give a start-up gain louder than the DRC gain a substream is already running at, or a decoder that joins the stream there plays louder than one that tracked it from the beginning. They are now `ChannelError::DrcStartUpGainTooLarge` and `ChannelError::HeavyDrcStartUpGainTooLarge`, reported at the bit they fired at and still at the level they logged at
- `BlockError::HuffmanNinthBitMissing` was declared and never raised. The huffman tables reach their deepest leaf through a nine-bit code whose ninth bit they ignore, and only the code ending in 1 is legal; that is now checked
- The extractor found no frames at all in FBB streams whose `channel_meaning` sets the bit FBA defines as `extra_channel_meaning_present`: it computed an extended major sync info length whose CRC could never match, and the parser misread the same extension. Both are FBA-only; an FBB major sync info is always the fixed 26 bytes. `fbb_unextractable_stream_extracts` is no longer ignored
- The `substream_info` whitelist rejected valid FBB values and accepted FBB's layered `0x0C` only through release-mode shift wrapping that panics under debug overflow checks. It now applies to FBA alone, and its shifts are range-guarded
- The `lossless_check` byte was compared only for the highest presentation of a decode, so a multi-presentation run wrote corrupted PCM in a lower presentation without warning. It now runs for every effective presentation
- `DecodedAccessUnit::channel_labels` was always empty: the labels were assigned when decoding started and erased by the substream reset every restart header performs. They now survive restarts
- `ObjectAudioMetadataPayload::read` panicked on any `oamd_version` other than 0, and again on `intermediate_spatial_format_idx` 6 or 7 while indexing a six-entry table. Both are reachable from arbitrary stream bytes. An unknown version is now an error; the two reserved ISF indices carry no objects and parse as such. This affected every version
- A trim element with `b_default_trim` set was dropped instead of stored, so a renderer-derived trim was indistinguishable from an absent one

### Changed
- `MajorSyncInfo::channel_meaning` is a `ChannelMeaning` enum over `FbaChannelMeaning` and `FbbChannelMeaning`, since the two syntaxes put unrelated structures there. The FBA fields keep their names; `ChannelMeaning::fba`, `::fbb` and `::extra_channel_meaning` reach them. `ChannelMeaning` was the FBA struct
- `PresentationMap::with_substream_info` is the FBA derivation alone. `PresentationMap::with_fbb_substream_info` is the FBB one, and `PresentationMap::for_format_sync` picks between them
- The `flags` masks are named constants in `structs::sync`: `FLAGS_SYNTAX_MARKER`, `FBA_FLAGS_RESERVED`, `FBB_FLAGS_RESERVED`, `FBA_FLAGS_CONSTANT` and `FBB_FLAGS_CONSTANT`
- The high-resolution timing decoder takes a `TimingContext` snapshot and returns the stream start timing, replacing the `Timing` trait. The caller no longer has to copy the state out, update the copy and write it back to satisfy the borrow checker
- `ExtractError::InvalidSyncPattern` and `SubstreamError::UnalignedSegmentStart` are gone, along with the rule IDs they carried. Neither could ever be raised: the extractor searches for a sync pattern rather than validating one, and a substream segment always starts 16-bit aligned because everything ahead of it is a whole number of 16-bit words
- `samples_per_75ms` is a function in `structs::sync` rather than the same expression written out at three call sites. Behaviour is unchanged, including the rounding at 44.1 kHz, the one rate where 75 ms is not a whole number of samples
- `ISF_COUNT_LIST` covers all eight values of the 3-bit index rather than six, and `MAX_OBJ_INFO_BLOCKS` is exposed alongside `MAX_OBJECT_COUNT`
- Error messages spell their comparisons in ASCII, `<=` and `>=` rather than the typographic forms, so they survive any terminal or log encoding
- The per-substream parser fields a restart header re-establishes moved into `ParserRestartState`, reached as `ParserSubstreamState::restart`. Which fields survive a restart is now the struct a field is declared in, not a list inside the reset, so a new field can no longer be silently reset. Field names, defaults and reset behaviour are unchanged

## [0.6.3] - 2026-08-04

### Fixed
- Requesting a presentation the stream does not carry resolved to no presentation at all: `effective_presentations` skipped the absent index and `substream_mask_by_required_presentations` selected no substreams for it, while the decoder logged that it was falling back to the highest presentation available. `decode_presentation` therefore failed with `Failed to get presentation N` and `decode_presentations` returned all `None`. Both now resolve an absent presentation to that fallback, and fail only when the stream carries no presentation at all. This affected 0.5.0 through 0.6.2

## [0.6.2] - 2026-08-04

### Fixed
- `object_gain` consumed its 6-bit index twice in `object_basic_info`, taking 12 bits and applying the dB mapping to the second read instead of the matched one. Any payload signalling an explicit object gain desynchronised from that point on

## [0.6.1] - 2026-08-01

### Changed
- `ParserPerfStats` is re-exported from `process::parse`, where the method returning it lives, as well as from `utils::perf`

## [0.6.0] - 2026-08-01

### Added
- `perf` feature exposing per-stage parse timing through `Parser::last_parse_stats()`, attributing time to substream directories, segments, block header setup, LSB bypass, Huffman decoding and conformance checks. The hooks compile out when the feature is off; enabling them costs about a third of parse time

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

