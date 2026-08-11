//! Diagnostic records for conformance reporting.
//!
//! Every conformance check in the crate goes through [`log_or_err!`](crate::log_or_err).
//! By default a check that fails at or below the configured fail level ends the parse,
//! which lets a caller see one violation per run. A [`DiagnosticSink`] in
//! [`DiagnosticMode::Collect`] instead keeps a [`Diagnostic`] for every check that fires,
//! so a whole stream can be enumerated.
//!
//! The identity of a check is its [`RuleId`], one variant per error variant. It is derived
//! from the error enums in [`crate::utils::errors`], so it cannot drift from them: adding
//! an error variant fails to compile until the rule is named.

use std::fmt;
use std::io::{Read, Seek};

use crate::utils::bitstream_io::BitstreamIoReader;
use crate::utils::errors::*;

/// The stable identity of the check a value reports.
pub trait Rule {
    fn rule_id(&self) -> RuleId;
}

macro_rules! rule_ids {
    ($(
        $error:ident => $domain_variant:ident / $rule_enum:ident / $domain:literal {
            $( $variant:ident = $name:literal ),* $(,)?
        }
    )*) => {
        $(
            #[doc = concat!("Checks reported as [`", stringify!($error), "`].")]
            #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
            pub enum $rule_enum {
                $( $variant, )*
            }

            impl $rule_enum {
                pub const fn as_str(&self) -> &'static str {
                    match self {
                        $( Self::$variant => $name, )*
                    }
                }
            }

            impl Rule for $error {
                fn rule_id(&self) -> RuleId {
                    match self {
                        $( Self::$variant { .. } => RuleId::$domain_variant($rule_enum::$variant), )*
                    }
                }
            }
        )*

        /// The check a diagnostic reports, one variant per error variant.
        ///
        /// Rendered as `domain.rule`, for example `sync.invalid_substream_info`.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum RuleId {
            $( $domain_variant($rule_enum), )*
            /// An error that carries no rule of its own: an I/O failure, or an error type
            /// outside [`crate::utils::errors`].
            Unclassified,
        }

        impl RuleId {
            /// The group the check belongs to, the error enum's name without `Error`.
            pub const fn domain(&self) -> &'static str {
                match self {
                    $( Self::$domain_variant(_) => $domain, )*
                    Self::Unclassified => "other",
                }
            }

            /// The check itself, unique within its domain.
            pub const fn rule(&self) -> &'static str {
                match self {
                    $( Self::$domain_variant(rule) => rule.as_str(), )*
                    Self::Unclassified => "unclassified",
                }
            }
        }

        impl Rule for anyhow::Error {
            fn rule_id(&self) -> RuleId {
                $(
                    if let Some(error) = self.downcast_ref::<$error>() {
                        return error.rule_id();
                    }
                )*

                RuleId::Unclassified
            }
        }
    };
}

rule_ids! {
    DecodeError => Decode / DecodeRule / "decode" {
        RecorrelatorPositiveSaturation = "recorrelator_positive_saturation",
        RecorrelatorNegativeSaturation = "recorrelator_negative_saturation",
        FilterBInputTooWide32 = "filter_b_input_too_wide32",
        FilterBInputTooWide24 = "filter_b_input_too_wide24",
        InvalidPresentation = "invalid_presentation",
        OutputsExceedMaxBits = "outputs_exceed_max_bits",
    }
    ExtractError => Extract / ExtractRule / "extract" {
        SubstreamMismatch = "substream_mismatch",
        ParityCheckFailed = "parity_check_failed",
        InsufficientData = "insufficient_data",
    }
    ParseError => Parse / ParseRule / "parse" {
        NoSubstream = "no_substream",
        InvalidSubstreamIndex = "invalid_substream_index",
    }
    AccessUnitError => AccessUnit / AccessUnitRule / "access_unit" {
        MissingInitialSync = "missing_initial_sync",
        FbaSyncTooFar = "fba_sync_too_far",
        FbbSyncTooFar = "fbb_sync_too_far",
        RestartGapInvalid = "restart_gap_invalid",
        NoSubstream = "no_substream",
        MisalignedSync = "misaligned_sync",
        NibbleParity = "nibble_parity",
        TimingTooShort = "timing_too_short",
        TimingTooShortAfterJump = "timing_too_short_after_jump",
        TimingShorterThanPrevious = "timing_shorter_than_previous",
        TimingShorterThanPreviousAfterJump = "timing_shorter_than_previous_after_jump",
        PeakDataRateTooHigh = "peak_data_rate_too_high",
        DataRateExceeded = "data_rate_exceeded",
        DataRateExceededAfterJump = "data_rate_exceeded_after_jump",
        TimingTooLong = "timing_too_long",
        TimingTooLongAfterJump = "timing_too_long_after_jump",
        AccessUnitTooLong = "access_unit_too_long",
        FixedRateMismatch = "fixed_rate_mismatch",
    }
    BlockError => Block / BlockRule / "block" {
        InvalidBlockSizeRange = "invalid_block_size_range",
        BlockSizeExceedsAU = "block_size_exceeds_au",
        OutputShiftTooLarge = "output_shift_too_large",
        BlockDataBitsTooLarge = "block_data_bits_too_large",
        LatencyInconsistent = "latency_inconsistent",
        DurationExceedsLatency = "duration_exceeds_latency",
        LatencyTooHigh = "latency_too_high",
        LatencyTooLow = "latency_too_low",
        SubstreamSizeUnderflow = "substream_size_underflow",
        HuffLsbsTooLarge = "huff_lsbs_too_large",
        QuantiserStepTooLarge = "quantiser_step_too_large",
        HuffmanNinthBitMissing = "huffman_ninth_bit_missing",
        HuffmanPositiveSaturation = "huffman_positive_saturation",
        HuffmanNegativeSaturation = "huffman_negative_saturation",
        HuffmanSampleTooLong = "huffman_sample_too_long",
        BlockDataBitCountMismatch = "block_data_bit_count_mismatch",
    }
    ChannelError => Channel / ChannelRule / "channel" {
        FilterOrderTooHigh = "filter_order_too_high",
        CoeffQMismatch = "coeff_q_mismatch",
        HuffLsbsTooLarge = "huff_lsbs_too_large",
        DrcStartUpGainTooLarge = "drc_start_up_gain_too_large",
        HeavyDrcStartUpGainTooLarge = "heavy_drc_start_up_gain_too_large",
    }
    ExtraDataError => ExtraData / ExtraDataRule / "extra_data" {
        MisalignedExtraDataStart = "misaligned_extra_data_start",
        PaddingNotZero = "padding_not_zero",
        LengthParityFailed = "length_parity_failed",
        ExtraDataTooLong = "extra_data_too_long",
        EvoFrameTooLong = "evo_frame_too_long",
        EvoFrameNoRoom = "evo_frame_no_room",
        EvoFrameMisaligned = "evo_frame_misaligned",
        EvoFramePaddingNotZero = "evo_frame_padding_not_zero",
        ExtraDataParityMismatch = "extra_data_parity_mismatch",
    }
    FilterError => Filter / FilterRule / "filter" {
        FilterAOrderTooHigh = "filter_a_order_too_high",
        FilterBOrderTooHigh = "filter_b_order_too_high",
        InvalidCoeffQ = "invalid_coeff_q",
        InvalidCoeffBits = "invalid_coeff_bits",
        InvalidCoeffShift = "invalid_coeff_shift",
        TotalCoeffBitsTooLarge = "total_coeff_bits_too_large",
        InvalidCoeffValue = "invalid_coeff_value",
        FilterANewStatesNotAllowed = "filter_a_new_states_not_allowed",
        FilterStateOutOfRange = "filter_state_out_of_range",
    }
    MatrixError => Matrix / MatrixRule / "matrix" {
        MatrixChannelTooHigh = "matrix_channel_too_high",
        FracBitsTooHigh = "frac_bits_too_high",
        InvalidLsbBypass = "invalid_lsb_bypass",
    }
    RestartHeaderError => RestartHeader / RestartHeaderRule / "restart_header" {
        HeavyDrcPresentInFbb = "heavy_drc_present_in_fbb",
        HeavyDrcTimeUpdateExceeded = "heavy_drc_time_update_exceeded",
        InvalidRestartSyncWord = "invalid_restart_sync_word",
        OutputTimingMismatch = "output_timing_mismatch",
        OutputTimingAfterJump = "output_timing_after_jump",
        InvalidOutputTiming = "invalid_output_timing",
        ZeroInputTimingInterval = "zero_input_timing_interval",
        InvalidSeamlessBranch = "invalid_seamless_branch",
        BranchAdvanceTooLarge = "branch_advance_too_large",
        BranchAdvanceExceedsBuffer = "branch_advance_exceeds_buffer",
        BranchAdvanceExceeds75ms = "branch_advance_exceeds_75ms",
        BranchDataRateExceeded = "branch_data_rate_exceeded",
        InvalidSyncBForSubstream1 = "invalid_sync_b_for_substream1",
        InvalidSyncBForSubstream0 = "invalid_sync_b_for_substream0",
        InvalidSyncC = "invalid_sync_c",
        MaxBitsMismatch = "max_bits_mismatch",
        ChannelAssignTooHigh = "channel_assign_too_high",
        ChannelAssignMisordered = "channel_assign_misordered",
        ChannelAssignDuplicate = "channel_assign_duplicate",
        RestartHeaderCrcMismatch = "restart_header_crc_mismatch",
        InvalidStream = "invalid_stream",
        LosslessCheckMismatch = "lossless_check_mismatch",
    }
    SubstreamError => Substream / SubstreamRule / "substream" {
        InvalidExtraSubstreamWordFbb = "invalid_extra_substream_word_fbb",
        InvalidRestartNonexistent = "invalid_restart_nonexistent",
        TooManyBlocks = "too_many_blocks",
        SampleCountMismatch = "sample_count_mismatch",
        DrcTimeUpdateExceeded = "drc_time_update_exceeded",
        InvalidTerminationWord = "invalid_termination_word",
        InvalidTerminatorB = "invalid_terminator_b",
        TooManyZeroSamples = "too_many_zero_samples",
        UnalignedSegmentEnd = "unaligned_segment_end",
        SubstreamEndMismatch = "substream_end_mismatch",
        SubstreamSizeUnderflow = "substream_size_underflow",
        SubstreamLengthUnderflow = "substream_length_underflow",
        ParityMismatch = "parity_mismatch",
        CrcMismatch = "crc_mismatch",
    }
    SyncError => Sync / SyncRule / "sync" {
        InvalidFormatSync = "invalid_format_sync",
        InvalidAudioSamplingFreq = "invalid_audio_sampling_freq",
        InvalidMajorSyncSignature = "invalid_major_sync_signature",
        ReservedFlagsNonZero = "reserved_flags_non_zero",
        InvalidFlagsSyntaxMarker = "invalid_flags_syntax_marker",
        FlagsMismatch = "flags_mismatch",
        PeakDataRateMismatch = "peak_data_rate_mismatch",
        SubstreamCountMismatch = "substream_count_mismatch",
        MajorSyncCrcMismatch = "major_sync_crc_mismatch",
        ReservedSubstreamInfo = "reserved_substream_info",
        InvalidSubstreamInfo = "invalid_substream_info",
        SubstreamInfoMismatch = "substream_info_mismatch",
        ReservedExtendedSubstreamInfo = "reserved_extended_substream_info",
        ReservedBeforeSubstreamInfo = "reserved_before_substream_info",
        ReservedChannelMeaningNonZero = "reserved_channel_meaning_non_zero",
        ExtendedSubstreamInfoMismatch = "extended_substream_info_mismatch",
        SixchAndEightchChannelAssignmentMismatch = "sixch_and_eightch_channel_assignment_mismatch",
        SixchAndEightchChannelModifierMismatch = "sixch_and_eightch_channel_modifier_mismatch",
        SubstreamInfoInCompatible = "substream_info_in_compatible",
        SubstreamCountInsufficient = "substream_count_insufficient",
        SubstreamCountInfoInconsistent = "substream_count_info_inconsistent",
    }
    TimestampError => Timestamp / TimestampRule / "timestamp" {
        InvalidSyncBytes = "invalid_sync_bytes",
        InvalidBcdDigit = "invalid_bcd_digit",
    }
    FifoError => Fifo / FifoRule / "fifo" {
        Substream0DepthExceeded = "substream0_depth_exceeded",
        GroupDepthExceeded = "group_depth_exceeded",
        WholeStreamDepthExceeded = "whole_stream_depth_exceeded",
        Underrun = "underrun",
    }
}

impl fmt::Display for RuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.domain(), self.rule())
    }
}

/// Where in the stream a diagnostic was raised.
///
/// `au_offset` and `au_index` come from the [`Frame`](crate::process::extract::Frame) the
/// access unit was parsed from. `bit_offset` is present only where the reporting check had
/// the bit reader at hand; the access-unit-level checks have no position finer than the
/// access unit itself.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Location {
    /// Position of the access unit in the extracted sequence.
    pub au_index: u64,

    /// Byte offset of the access unit from the first byte pushed to the extractor.
    pub au_offset: u64,

    /// Bits from the start of the access unit, when the check knows it.
    pub bit_offset: Option<u64>,
}

impl Location {
    /// Byte offset of the reported position, or of the access unit when no bit offset is
    /// known.
    pub const fn byte_offset(&self) -> u64 {
        match self.bit_offset {
            Some(bits) => self.au_offset + bits / 8,
            None => self.au_offset,
        }
    }

    /// Bit within [`byte_offset`](Self::byte_offset), when known.
    pub const fn bit_in_byte(&self) -> Option<u8> {
        match self.bit_offset {
            Some(bits) => Some((bits % 8) as u8),
            None => None,
        }
    }
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "au {} @{:#x}", self.au_index, self.byte_offset())?;

        match self.bit_in_byte() {
            Some(bit) => write!(f, "+{bit}"),
            None => Ok(()),
        }
    }
}

/// One check that fired, with its identity, severity and location.
#[derive(Debug)]
pub struct Diagnostic {
    /// Which check fired.
    pub rule: RuleId,

    /// The level the check reports at. A check at or below the sink's fail level ends the
    /// access unit; anything above it is advisory.
    pub severity: log::Level,

    /// Where in the stream the check fired.
    pub location: Location,

    /// The error's own message.
    pub message: String,

    /// The error itself, for callers that want its fields rather than its message. Absent
    /// for a diagnostic whose error was propagated to the caller instead.
    pub source: Option<anyhow::Error>,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}: {}", self.location, self.rule, self.message)
    }
}

/// What a [`DiagnosticSink`] does with a check that fires.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DiagnosticMode {
    /// Log the check and, at or below the fail level, propagate it. The default, and the
    /// only behaviour before diagnostics existed.
    #[default]
    FailFast,

    /// Keep a [`Diagnostic`] for every check that fires, on top of the fail-fast
    /// behaviour. A check above the fail level is recorded and parsing continues; a check
    /// at or below it is recorded and still ends the access unit, since the parser's
    /// invariants are what those levels protect.
    Collect,
}

/// A parser component that checks report to.
pub trait DiagnosticSink {
    /// Checks at or below this level end the access unit.
    fn fail_level(&self) -> log::Level;

    fn diagnostic_mode(&self) -> DiagnosticMode {
        DiagnosticMode::FailFast
    }

    fn is_collecting(&self) -> bool {
        self.diagnostic_mode() == DiagnosticMode::Collect
    }

    /// The position a check that fires right now would report.
    fn location(&self, _bit_offset: Option<u64>) -> Location {
        Location::default()
    }

    fn push_diagnostic(&mut self, _diagnostic: Diagnostic) {}
}

/// Records a check whose error is propagated to the caller.
///
/// [`Diagnostic::source`] is left empty; the recovery layer that catches the error owns it
/// and can fill it in.
pub fn record<S, E>(sink: &mut S, severity: log::Level, error: &E, bit_offset: Option<u64>)
where
    S: DiagnosticSink + ?Sized,
    E: Rule + fmt::Display,
{
    let diagnostic = Diagnostic {
        rule: error.rule_id(),
        severity,
        location: sink.location(bit_offset),
        message: error.to_string(),
        source: None,
    };

    sink.push_diagnostic(diagnostic);
}

/// Records a check whose error is not propagated, keeping the error itself.
pub fn record_owned<S, E>(sink: &mut S, severity: log::Level, error: E, bit_offset: Option<u64>)
where
    S: DiagnosticSink + ?Sized,
    E: Rule + fmt::Display + Into<anyhow::Error>,
{
    let diagnostic = Diagnostic {
        rule: error.rule_id(),
        severity,
        location: sink.location(bit_offset),
        message: error.to_string(),
        source: Some(error.into()),
    };

    sink.push_diagnostic(diagnostic);
}

/// Bit position of a reader, for the checks that hold one.
pub fn bit_position<R: Read + Seek>(reader: &mut BitstreamIoReader<R>) -> Option<u64> {
    reader.position().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_ids_render_as_domain_and_rule() {
        assert_eq!(
            SyncError::InvalidSubstreamInfo(0).rule_id().to_string(),
            "sync.invalid_substream_info"
        );
        assert_eq!(
            FifoError::Underrun { index: 0 }.rule_id(),
            RuleId::Fifo(FifoRule::Underrun)
        );
    }

    /// The two `NoSubstream` variants live in different domains and must stay distinct.
    #[test]
    fn rule_ids_are_unique_across_domains() {
        assert_ne!(
            ParseError::NoSubstream.rule_id(),
            AccessUnitError::NoSubstream.rule_id()
        );
    }

    #[test]
    fn anyhow_errors_recover_their_rule() {
        let error = anyhow::anyhow!(AccessUnitError::FbbSyncTooFar);
        assert_eq!(
            error.rule_id(),
            RuleId::AccessUnit(AccessUnitRule::FbbSyncTooFar)
        );

        let error = anyhow::anyhow!("something else");
        assert_eq!(error.rule_id(), RuleId::Unclassified);
    }

    #[test]
    fn a_location_splits_into_byte_and_bit() {
        let location = Location {
            au_index: 7,
            au_offset: 0x100,
            bit_offset: Some(19),
        };

        assert_eq!(location.byte_offset(), 0x102);
        assert_eq!(location.bit_in_byte(), Some(3));
        assert_eq!(location.to_string(), "au 7 @0x102+3");

        let location = Location {
            bit_offset: None,
            ..location
        };

        assert_eq!(location.byte_offset(), 0x100);
        assert_eq!(location.to_string(), "au 7 @0x100");
    }
}
