//! Incremental TrueHD decoding.
//!
//! [`StreamDecoder`](crate::process::stream::StreamDecoder) owns the extractor, parser and audio
//! decoder as one state machine. Each call to
//! [`push_bytes`](crate::process::stream::StreamDecoder::push_bytes) accepts an arbitrary fragment
//! and returns every complete access unit made available by that fragment. PCM and typed OAMD stay
//! together in a [`StreamPresentation`](crate::process::stream::StreamPresentation) carrying the
//! accepted-sample position they belong to.
//!
//! ```
//! use truehd::process::EXAMPLE_DATA;
//! use truehd::process::stream::{StreamDecoder, StreamDecoderConfig};
//!
//! let config = StreamDecoderConfig::for_presentation(0)?;
//! let mut decoder = StreamDecoder::new(config);
//!
//! for chunk in EXAMPLE_DATA.chunks(7) {
//!     for frame in decoder.push_bytes(chunk)? {
//!         for presentation in frame.presentations.iter() {
//!             if presentation.decoded.is_duplicate {
//!                 continue;
//!             }
//!             let pcm = &presentation.decoded.pcm_data
//!                 [..presentation.decoded.sample_length];
//!             assert!(!pcm.is_empty());
//!         }
//!     }
//! }
//! decoder.finish()?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use crate::process::MAX_PRESENTATIONS;
use crate::process::decode::{AudioConfiguration, DecodedAccessUnit, Decoder};
use crate::process::extract::Extractor;
use crate::process::parse::{Branch, Parser};
use crate::structs::access_unit::AccessUnit;
use crate::structs::timestamp::Timestamp;
use crate::utils::errors::ExtractError;
use log::Level;

/// Configuration retained when a [`StreamDecoder`] is reset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamDecoderConfig {
    required_presentations: [bool; MAX_PRESENTATIONS],
    fail_level: Level,
    check_fifo: bool,
}

impl Default for StreamDecoderConfig {
    /// Requests the highest TrueHD presentation, matching the command-line decoder, and disables
    /// the authoring-oriented FIFO model for lower overhead in a long-running decoder.
    fn default() -> Self {
        Self {
            required_presentations: [false, false, false, true],
            fail_level: Level::Error,
            check_fifo: false,
        }
    }
}

impl StreamDecoderConfig {
    /// Creates a configuration for one presentation index.
    pub fn for_presentation(presentation: usize) -> Result<Self, StreamError> {
        if presentation >= MAX_PRESENTATIONS {
            return Err(StreamError::InvalidPresentation(presentation));
        }

        let mut required_presentations = [false; MAX_PRESENTATIONS];
        required_presentations[presentation] = true;
        Ok(Self {
            required_presentations,
            ..Self::default()
        })
    }

    /// Creates a configuration for an explicit set of presentations.
    pub fn for_presentations(
        required_presentations: [bool; MAX_PRESENTATIONS],
    ) -> Result<Self, StreamError> {
        if !required_presentations.iter().any(|&required| required) {
            return Err(StreamError::NoPresentations);
        }

        Ok(Self {
            required_presentations,
            ..Self::default()
        })
    }

    /// Makes checks at `level` and above fatal in both parser and decoder.
    pub fn with_fail_level(mut self, level: Level) -> Self {
        self.fail_level = level;
        self
    }

    /// Enables or disables the byte-domain FIFO conformance model.
    ///
    /// It is disabled by default because it answers whether a stream is authorable rather than
    /// changing its decoded samples. Verification tools can enable it explicitly.
    pub fn with_fifo_checking(mut self, enabled: bool) -> Self {
        self.check_fifo = enabled;
        self
    }

    /// Presentations requested from the underlying decoder.
    pub fn required_presentations(&self) -> &[bool; MAX_PRESENTATIONS] {
        &self.required_presentations
    }
}

/// Position of an access unit on the accepted audio timeline.
///
/// Duplicate access units do not advance either sample counter. A configuration change starts a
/// new epoch but does not reset `absolute_sample`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StreamPosition {
    /// Configuration generation, starting at zero.
    pub epoch: u64,
    /// Accepted samples since the start of this effective presentation.
    pub absolute_sample: u64,
    /// Accepted samples since the start of this configuration epoch.
    pub epoch_sample: u64,
}

impl StreamPosition {
    fn offset(self, samples: u64) -> Result<Self, StreamError> {
        Ok(Self {
            epoch: self.epoch,
            absolute_sample: self
                .absolute_sample
                .checked_add(samples)
                .ok_or(StreamError::TimelineOverflow)?,
            epoch_sample: self
                .epoch_sample
                .checked_add(samples)
                .ok_or(StreamError::TimelineOverflow)?,
        })
    }
}

/// Borrowed decoded presentation and the timeline position shared by its PCM and OAMD.
#[derive(Debug, Clone, Copy)]
pub struct StreamPresentation<'a> {
    /// Effective presentation emitted by the decoder. It can differ from the requested index when
    /// a stream does not carry that presentation and resolves it to the highest available one.
    pub presentation: usize,
    /// Start of this access unit on the effective presentation's accepted-audio timeline.
    pub position: StreamPosition,
    /// Native integer PCM, channel identity, flags, and typed OAMD from the decoder.
    pub decoded: &'a DecodedAccessUnit,
}

/// Decoded presentations from one access unit, retained in the allocation produced by the audio
/// decoder. Iteration yields views that cannot separate a result from its timeline position.
#[derive(Debug, Clone)]
pub struct StreamPresentations {
    positions: [Option<StreamPosition>; MAX_PRESENTATIONS],
    decoded: Box<[Option<DecodedAccessUnit>; MAX_PRESENTATIONS]>,
}

impl StreamPresentations {
    /// Returns the effective presentation at `presentation`, when decoded for this access unit.
    pub fn get(&self, presentation: usize) -> Option<StreamPresentation<'_>> {
        let position = self.positions.get(presentation).copied().flatten()?;
        let decoded = self.decoded.get(presentation)?.as_ref()?;
        Some(StreamPresentation {
            presentation,
            position,
            decoded,
        })
    }

    /// Iterates decoded effective presentations in index order.
    pub fn iter(&self) -> impl Iterator<Item = StreamPresentation<'_>> {
        (0..MAX_PRESENTATIONS).filter_map(|presentation| self.get(presentation))
    }

    /// Number of effective presentations decoded for this access unit.
    pub fn len(&self) -> usize {
        self.decoded.iter().flatten().count()
    }

    /// Whether the access unit produced no presentation.
    pub fn is_empty(&self) -> bool {
        self.decoded.iter().all(Option::is_none)
    }
}

/// A source branch point with its position on each decoded presentation's accepted timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamBranch {
    /// Parser information in the source access-unit domain.
    ///
    /// In particular, `source.sample` counts source access units using the current access-unit
    /// sample length. Use [`position`](Self::position) for an accepted-sample position that
    /// accounts for duplicates, trimmed samples, and configuration epochs.
    pub source: Branch,
    positions: [Option<StreamPosition>; MAX_PRESENTATIONS],
}

impl StreamBranch {
    /// Returns the accepted-sample cursor immediately before this branch's access unit.
    ///
    /// The index names an effective presentation, as in [`StreamPresentations::get`]. The
    /// position equals that presentation's position in the containing [`StreamFrame`], including
    /// its configuration epoch. Returns `None` for a presentation not decoded in this frame or
    /// an index outside `0..MAX_PRESENTATIONS`.
    pub fn position(&self, presentation: usize) -> Option<StreamPosition> {
        self.positions.get(presentation).copied().flatten()
    }
}

/// Everything decoded from one source access unit.
#[derive(Debug, Clone)]
pub struct StreamFrame {
    /// Zero-based source access-unit index.
    pub frame_index: u64,
    /// Byte offset of the access unit from the start of the pushed stream.
    pub byte_offset: u64,
    /// SMPTE timestamp preceding the access unit, when present.
    pub timestamp: Option<Timestamp>,
    /// Whether this frame starts a new configuration epoch because the substream map or a decoded
    /// presentation's sample rate, channel count, labels, or effective presence changed.
    pub configuration_changed: bool,
    /// Branch points, retaining their source information and accepted positions per presentation.
    /// Taking them here keeps parser memory bounded for a decoder that runs indefinitely.
    pub branches: Vec<StreamBranch>,
    /// Effective presentations, their native PCM/OAMD, and their accepted-sample positions.
    pub presentations: StreamPresentations,
}

/// Terminal processing errors for the incremental pipeline.
///
/// After a processing error the decoder is failed and rejects more input until
/// [`StreamDecoder::reset`] is called. [`ExtractError::InsufficientData`] is not an error here; it
/// only means another input fragment is needed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StreamError {
    #[error("presentation index {0} is outside 0..{MAX_PRESENTATIONS}")]
    InvalidPresentation(usize),

    #[error("at least one presentation must be requested")]
    NoPresentations,

    #[error("the stream has already been finished")]
    AlreadyFinished,

    #[error("the stream is failed; call reset before decoding another stream")]
    Failed,

    #[error("the input byte counter overflowed")]
    InputTooLarge,

    #[error("frame extraction failed")]
    Extract(#[source] ExtractError),

    #[error("the extractor skipped {count} corrupt frame candidate(s)")]
    CorruptFrameCandidates { count: usize },

    #[error("failed to parse access unit {frame_index} at byte {byte_offset}")]
    Parse {
        frame_index: u64,
        byte_offset: u64,
        #[source]
        source: anyhow::Error,
    },

    #[error("failed to decode access unit {frame_index} at byte {byte_offset}")]
    Decode {
        frame_index: u64,
        byte_offset: u64,
        #[source]
        source: anyhow::Error,
    },

    #[error("access unit {frame_index} produced no decoded presentation")]
    NoDecodedPresentation { frame_index: u64 },

    #[error("the accepted sample timeline overflowed")]
    TimelineOverflow,

    #[error("incomplete input: consumed {consumed} of {received} bytes")]
    IncompleteInput { consumed: u64, received: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lifecycle {
    Active,
    Failed,
    Finished,
}

/// Stateful, single-threaded incremental TrueHD decoder.
pub struct StreamDecoder {
    config: StreamDecoderConfig,
    extractor: Extractor,
    parser: Parser,
    decoder: Decoder,
    lifecycle: Lifecycle,
    bytes_received: u64,
    next_samples: [u64; MAX_PRESENTATIONS],
    epoch: u64,
    epoch_start_samples: [u64; MAX_PRESENTATIONS],
    current_substream_configuration: Option<(u8, u8)>,
    output_configurations: [Option<AudioConfiguration>; MAX_PRESENTATIONS],
    output_configurations_initialized: bool,
}

impl Default for StreamDecoder {
    fn default() -> Self {
        Self::new(StreamDecoderConfig::default())
    }
}

impl StreamDecoder {
    /// Creates a fresh decoder with explicit presentation and validation policy.
    pub fn new(config: StreamDecoderConfig) -> Self {
        let mut parser = Parser::default();
        parser.set_required_presentations(&config.required_presentations);
        parser.set_fail_level(config.fail_level);
        parser.set_check_fifo(config.check_fifo);

        let mut decoder = Decoder::default();
        decoder.set_fail_level(config.fail_level);

        Self {
            config,
            extractor: Extractor::default(),
            parser,
            decoder,
            lifecycle: Lifecycle::Active,
            bytes_received: 0,
            next_samples: [0; MAX_PRESENTATIONS],
            epoch: 0,
            epoch_start_samples: [0; MAX_PRESENTATIONS],
            current_substream_configuration: None,
            output_configurations: core::array::from_fn(|_| None),
            output_configurations_initialized: false,
        }
    }

    /// Convenience constructor for one requested presentation.
    pub fn for_presentation(presentation: usize) -> Result<Self, StreamError> {
        Ok(Self::new(StreamDecoderConfig::for_presentation(
            presentation,
        )?))
    }

    /// Total bytes accepted through [`push_bytes`](Self::push_bytes) and
    /// [`push_bytes_with`](Self::push_bytes_with).
    pub fn bytes_received(&self) -> u64 {
        self.bytes_received
    }

    /// Bytes assigned to complete access units or skipped while finding the first sync.
    pub fn bytes_consumed(&self) -> u64 {
        self.extractor.stream_position()
    }

    fn next_sample_position(&self, presentation: usize) -> Option<StreamPosition> {
        (presentation < MAX_PRESENTATIONS).then(|| StreamPosition {
            epoch: self.epoch,
            absolute_sample: self.next_samples[presentation],
            epoch_sample: self.next_samples[presentation] - self.epoch_start_samples[presentation],
        })
    }

    /// Accepts one arbitrary fragment and returns all newly completed access units.
    ///
    /// If a later access unit in this fragment fails, the collected prefix is discarded.
    /// Use the callback variant when successful prefix frames must be delivered.
    /// For a hot path that wants to avoid allocating the returned `Vec`, use
    /// [`push_bytes_with`](Self::push_bytes_with).
    pub fn push_bytes(&mut self, data: &[u8]) -> Result<Vec<StreamFrame>, StreamError> {
        let mut frames = Vec::new();
        self.push_bytes_with(data, |frame| {
            frames.push(frame);
            Ok::<_, StreamError>(())
        })?;
        Ok(frames)
    }

    /// Accepts one arbitrary fragment and emits complete access units directly to `on_frame`.
    ///
    /// No PCM or OAMD is copied beyond the owned [`DecodedAccessUnit`] produced by the decoder.
    /// A callback error is returned unchanged and stops before decoding the next access unit.
    /// Both callback and decoder errors make the session terminal until [`reset`](Self::reset).
    /// Already delivered frames stay delivered; any remaining input is discarded on reset.
    /// Use `StreamError` for an infallible consumer or an error type such as `anyhow::Error`
    /// that can represent both processing and consumer failures.
    pub fn push_bytes_with<E: From<StreamError>>(
        &mut self,
        data: &[u8],
        mut on_frame: impl FnMut(StreamFrame) -> Result<(), E>,
    ) -> Result<(), E> {
        match self.lifecycle {
            Lifecycle::Finished => return Err(StreamError::AlreadyFinished.into()),
            Lifecycle::Failed => return Err(StreamError::Failed.into()),
            Lifecycle::Active => {}
        }
        if data.is_empty() {
            return Ok(());
        }

        let Some(bytes_received) = self.bytes_received.checked_add(data.len() as u64) else {
            self.lifecycle = Lifecycle::Failed;
            return Err(StreamError::InputTooLarge.into());
        };
        self.bytes_received = bytes_received;
        self.extractor.push_bytes(data);

        let result = self.drain(&mut on_frame);
        if result.is_err() {
            self.lifecycle = Lifecycle::Failed;
        }
        result
    }

    /// Declares EOF and verifies that no partial access unit remains buffered.
    ///
    /// Every complete access unit is emitted synchronously by `push_bytes`, so `finish` never
    /// discards output. Calling it more than once is harmless.
    pub fn finish(&mut self) -> Result<(), StreamError> {
        match self.lifecycle {
            Lifecycle::Finished => return Ok(()),
            Lifecycle::Failed => return Err(StreamError::Failed),
            Lifecycle::Active => {}
        }

        let consumed = self.extractor.stream_position();
        if consumed != self.bytes_received {
            self.lifecycle = Lifecycle::Failed;
            return Err(StreamError::IncompleteInput {
                consumed,
                received: self.bytes_received,
            });
        }

        self.lifecycle = Lifecycle::Finished;
        Ok(())
    }

    /// Clears all stream, timeline and failure state while retaining the configuration.
    pub fn reset(&mut self) {
        *self = Self::new(self.config);
    }

    fn drain<E: From<StreamError>>(
        &mut self,
        on_frame: &mut impl FnMut(StreamFrame) -> Result<(), E>,
    ) -> Result<(), E> {
        loop {
            let errors_before = self.extractor.error_count();
            let extracted = match self.extractor.next() {
                Some(Ok(frame)) => Some(frame),
                Some(Err(ExtractError::InsufficientData)) | None => None,
                Some(Err(error)) => return Err(StreamError::Extract(error).into()),
            };
            let errors_after = self.extractor.error_count();
            if errors_after != errors_before {
                return Err(StreamError::CorruptFrameCandidates {
                    count: errors_after - errors_before,
                }
                .into());
            }

            let Some(frame) = extracted else {
                return Ok(());
            };
            let frame_index = frame.index;
            let byte_offset = frame.offset;

            let access_unit = self
                .parser
                .parse(&frame)
                .map_err(|source| StreamError::Parse {
                    frame_index,
                    byte_offset,
                    source,
                })?;
            let timestamp = frame.timestamp;
            let substream_configuration_changed =
                self.observe_substream_configuration(&access_unit);
            let branches = self.parser.take_branches();
            let mut decoded = self
                .decoder
                .decode_presentations(&access_unit, &self.config.required_presentations)
                .map_err(|source| StreamError::Decode {
                    frame_index,
                    byte_offset,
                    source,
                })?;

            if decoded.iter().all(Option::is_none) {
                return Err(StreamError::NoDecodedPresentation { frame_index }.into());
            }
            let output_configuration_changed = self.observe_output_configurations(&decoded);
            let configuration_changed =
                substream_configuration_changed || output_configuration_changed;
            if configuration_changed {
                self.begin_configuration_epoch()?;
            }
            if substream_configuration_changed {
                for presentation in decoded.iter_mut().flatten() {
                    presentation.substream_info_changed = true;
                }
            }

            let mut next_samples = self.next_samples;
            let mut positions = [None; MAX_PRESENTATIONS];
            for presentation in 0..MAX_PRESENTATIONS {
                let Some(presentation_decoded) = decoded[presentation].as_ref() else {
                    continue;
                };
                let position = self
                    .next_sample_position(presentation)
                    .expect("presentation index is bounded by MAX_PRESENTATIONS");
                for payload in &presentation_decoded.oamd {
                    position.offset(payload.evo_sample_offset)?;
                }
                if !presentation_decoded.is_duplicate {
                    next_samples[presentation] = next_samples[presentation]
                        .checked_add(presentation_decoded.sample_length as u64)
                        .ok_or(StreamError::TimelineOverflow)?;
                }
                positions[presentation] = Some(position);
            }
            let frame = StreamFrame {
                frame_index,
                byte_offset,
                timestamp,
                configuration_changed,
                branches: branches
                    .into_iter()
                    .map(|source| StreamBranch { source, positions })
                    .collect(),
                presentations: StreamPresentations { positions, decoded },
            };
            self.next_samples = next_samples;
            on_frame(frame)?;
        }
    }

    fn observe_substream_configuration(&mut self, access_unit: &AccessUnit) -> bool {
        let Some(sync) = &access_unit.major_sync_info else {
            return false;
        };
        let next = (sync.substream_info, sync.extended_substream_info);
        let changed = self
            .current_substream_configuration
            .is_some_and(|current| current != next);
        self.current_substream_configuration = Some(next);
        changed
    }

    fn observe_output_configurations(
        &mut self,
        decoded: &[Option<DecodedAccessUnit>; MAX_PRESENTATIONS],
    ) -> bool {
        let changed = self.output_configurations_initialized
            && self
                .output_configurations
                .iter()
                .zip(decoded)
                .any(|(current, decoded)| match (current, decoded) {
                    (Some(current), Some(decoded)) => !current.matches(decoded),
                    (None, None) => false,
                    _ => true,
                });

        if !self.output_configurations_initialized || changed {
            self.output_configurations = core::array::from_fn(|presentation| {
                decoded[presentation].as_ref().map(AudioConfiguration::from)
            });
            self.output_configurations_initialized = true;
        }
        changed
    }

    fn begin_configuration_epoch(&mut self) -> Result<(), StreamError> {
        self.epoch = self
            .epoch
            .checked_add(1)
            .ok_or(StreamError::TimelineOverflow)?;
        self.epoch_start_samples = self.next_samples;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::EXAMPLE_DATA;
    use crate::structs::channel::ChannelLabel;

    fn assert_initial_error_is_terminal(data: &[u8], expected: impl Fn(&StreamError) -> bool) {
        for suffix in [false, true] {
            let mut input = data.to_vec();
            if suffix {
                input.extend_from_slice(&EXAMPLE_DATA[16..]);
            }
            for chunk in [1, 7, input.len()] {
                let mut decoder = StreamDecoder::default();
                let mut error = None;
                for part in input.chunks(chunk) {
                    match decoder.push_bytes(part) {
                        Ok(frames) => assert!(frames.is_empty()),
                        Err(failure) => {
                            error = Some(failure);
                            break;
                        }
                    }
                }
                let error = error.expect("corruption must be reported by push_bytes");
                assert!(expected(&error), "unexpected error: {error:?}");
                assert!(matches!(decoder.finish(), Err(StreamError::Failed)));
                assert!(matches!(
                    decoder.push_bytes(EXAMPLE_DATA),
                    Err(StreamError::Failed)
                ));
                assert!(matches!(decoder.push_bytes(&[]), Err(StreamError::Failed)));
                decoder.reset();
                assert_eq!(decoder.push_bytes(EXAMPLE_DATA).unwrap().len(), 2);
                decoder.finish().unwrap();
            }
        }
    }

    #[test]
    fn initial_crc_failure_is_terminal_even_when_all_bytes_were_consumed() {
        let mut corrupt = EXAMPLE_DATA[16..100].to_vec();
        corrupt[30] ^= 1;
        assert_initial_error_is_terminal(&corrupt, |error| {
            matches!(error, StreamError::Extract(ExtractError::ParityCheckFailed))
        });
    }

    #[test]
    fn impossible_initial_length_is_terminal_without_waiting_for_more_input() {
        // Enough header to determine the length, but not a complete access unit.
        let mut corrupt = EXAMPLE_DATA[16..47].to_vec();
        corrupt[0] &= 0xf0;
        corrupt[1] = 0;
        assert_initial_error_is_terminal(&corrupt, |error| {
            matches!(
                error,
                StreamError::Extract(ExtractError::InvalidAccessUnitLength { actual: 0, .. })
            )
        });
    }

    #[test]
    fn one_byte_fragments_decode_on_one_timeline() {
        let mut decoder = StreamDecoder::new(
            StreamDecoderConfig::for_presentations([true; MAX_PRESENTATIONS]).unwrap(),
        );
        let mut frames = Vec::new();
        for byte in EXAMPLE_DATA {
            frames.extend(decoder.push_bytes(std::slice::from_ref(byte)).unwrap());
        }
        decoder.finish().unwrap();

        assert_eq!(frames.len(), 2);
        let first = frames[0].presentations.iter().next().unwrap();
        let presentation = first.presentation;
        let second = frames[1].presentations.get(presentation).unwrap();
        assert_eq!(first.position, StreamPosition::default());
        assert_eq!(
            second.position.absolute_sample,
            first.decoded.sample_length as u64
        );
        assert_eq!(decoder.bytes_received(), EXAMPLE_DATA.len() as u64);
        assert_eq!(decoder.bytes_consumed(), EXAMPLE_DATA.len() as u64);
        assert_eq!(
            decoder
                .next_sample_position(presentation)
                .unwrap()
                .absolute_sample,
            second.position.absolute_sample + second.decoded.sample_length as u64
        );
        assert!(decoder.next_sample_position(MAX_PRESENTATIONS).is_none());
        assert!(matches!(decoder.lifecycle, Lifecycle::Finished));
    }

    #[test]
    fn callback_path_avoids_collecting_frames() {
        let mut decoder = StreamDecoder::for_presentation(0).unwrap();
        let mut frames = 0;
        decoder
            .push_bytes_with(EXAMPLE_DATA, |_| {
                frames += 1;
                Ok::<_, StreamError>(())
            })
            .unwrap();
        decoder.finish().unwrap();
        assert_eq!(frames, 2);
    }

    #[test]
    fn consumer_failure_stops_immediately_and_requires_reset() {
        let mut decoder = StreamDecoder::default();
        let mut delivered = 0;
        let error = decoder
            .push_bytes_with(EXAMPLE_DATA, |_| -> anyhow::Result<()> {
                delivered += 1;
                Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "receiver closed").into())
            })
            .unwrap_err();
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::BrokenPipe
        );
        assert_eq!(delivered, 1);
        assert!(decoder.bytes_consumed() < EXAMPLE_DATA.len() as u64);
        assert!(matches!(decoder.finish(), Err(StreamError::Failed)));
        assert!(matches!(decoder.push_bytes(&[]), Err(StreamError::Failed)));
        decoder.reset();
        assert_eq!(decoder.push_bytes(EXAMPLE_DATA).unwrap().len(), 2);
        decoder.finish().unwrap();
    }

    #[test]
    fn finish_rejects_a_partial_access_unit_and_requires_reset() {
        let mut decoder = StreamDecoder::default();
        decoder
            .push_bytes(&EXAMPLE_DATA[..EXAMPLE_DATA.len() - 1])
            .unwrap();
        assert!(matches!(
            decoder.finish(),
            Err(StreamError::IncompleteInput { .. })
        ));
        assert!(matches!(decoder.push_bytes(&[]), Err(StreamError::Failed)));

        decoder.reset();
        assert_eq!(decoder.bytes_received(), 0);
        assert!(decoder.push_bytes(EXAMPLE_DATA).is_ok());
        assert!(decoder.finish().is_ok());
    }

    #[test]
    fn finish_is_idempotent_but_more_input_is_rejected() {
        let mut decoder = StreamDecoder::default();
        decoder.push_bytes(EXAMPLE_DATA).unwrap();
        decoder.finish().unwrap();
        decoder.finish().unwrap();
        assert!(matches!(
            decoder.push_bytes(&[]),
            Err(StreamError::AlreadyFinished)
        ));
    }

    #[test]
    fn default_configuration_is_explicit_and_realtime_oriented() {
        let config = StreamDecoderConfig::default();
        assert_eq!(
            config.required_presentations(),
            &[false, false, false, true]
        );
        assert_eq!(config.fail_level, Level::Error);
        assert!(!config.check_fifo);
        assert!(matches!(
            StreamDecoderConfig::for_presentation(MAX_PRESENTATIONS),
            Err(StreamError::InvalidPresentation(_))
        ));
        assert!(matches!(
            StreamDecoderConfig::for_presentations([false; MAX_PRESENTATIONS]),
            Err(StreamError::NoPresentations)
        ));
    }

    #[test]
    fn decoded_signal_changes_are_configuration_changes() {
        fn decoded(
            sampling_frequency: u32,
            channel_labels: Vec<ChannelLabel>,
        ) -> DecodedAccessUnit {
            DecodedAccessUnit {
                sampling_frequency,
                sample_length: 40,
                channel_count: channel_labels.len(),
                pcm_data: [[0; 16]; 160],
                channel_labels,
                oamd: Vec::new(),
                is_duplicate: false,
                substream_info_changed: false,
            }
        }

        let mut decoder = StreamDecoder::for_presentation(0).unwrap();
        let mut presentations = core::array::from_fn(|_| None);
        presentations[0] = Some(decoded(48_000, vec![ChannelLabel::L, ChannelLabel::R]));
        assert!(!decoder.observe_output_configurations(&presentations));
        assert!(!decoder.observe_output_configurations(&presentations));

        presentations[0] = Some(decoded(96_000, vec![ChannelLabel::L, ChannelLabel::R]));
        assert!(decoder.observe_output_configurations(&presentations));
        decoder.begin_configuration_epoch().unwrap();
        assert_eq!(decoder.next_sample_position(0).unwrap().epoch, 1);

        presentations[0] = Some(decoded(96_000, vec![ChannelLabel::C]));
        assert!(decoder.observe_output_configurations(&presentations));
        presentations[0] = None;
        assert!(decoder.observe_output_configurations(&presentations));
    }

    #[test]
    fn metadata_timeline_overflow_is_reported() {
        let position = StreamPosition {
            absolute_sample: u64::MAX,
            ..StreamPosition::default()
        };
        assert!(matches!(
            position.offset(1),
            Err(StreamError::TimelineOverflow)
        ));
        let position = StreamPosition {
            epoch_sample: u64::MAX,
            ..StreamPosition::default()
        };
        assert!(matches!(
            position.offset(1),
            Err(StreamError::TimelineOverflow)
        ));
    }
}
