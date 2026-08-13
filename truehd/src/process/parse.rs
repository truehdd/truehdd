use anyhow::{Result, bail};

use crate::process::extract::Frame;
use crate::process::{MAX_PRESENTATIONS, PresentationMap};
use crate::structs::access_unit::AccessUnit;
use crate::structs::restart_header::Guards;
use crate::utils::bitstream_io::BsIoSliceReader;
use crate::utils::crc::{
    CRC_MAJOR_SYNC_INFO_ALG, CRC_RESTART_BLOCK_HEADER_ALG, CRC_SUBSTREAM_ALG, Crc8, Crc16,
};
use crate::utils::diagnostic::{
    Diagnostic, DiagnosticMode, DiagnosticSink, Location, Rule, bit_position,
};
use crate::utils::errors::ParseError;
use crate::utils::fifo::{ACCUMULATORS, FifoDepthState, FifoPeak, SUBSTREAMS};
// Re-exported so the type is nameable where the method returning it lives
pub use crate::utils::perf::ParserPerfStats;
use crate::utils::timing::HiresOutputTimingState;

/// How many restart gaps [`ParserState::restart_gap`] keeps.
pub const RESTART_GAP_HISTORY: usize = 4;

/// Parses audio frames into structured access units.
///
/// Converts raw frame data into [`AccessUnit`] objects containing
/// parsed metadata, audio blocks, and timing information.
#[derive(Default)]
pub struct Parser {
    state: ParserState,
    resyncing: bool,
}

impl Parser {
    /// Parses an audio frame into a structured access unit.
    ///
    /// Returns an [`AccessUnit`] containing parsed metadata, audio blocks,
    /// and timing information. Handles both major sync frames (with stream
    /// configuration) and continuation frames (audio data only).
    pub fn parse(&mut self, frame: &Frame) -> Result<AccessUnit> {
        self.parse_inner(frame).0
    }

    fn parse_inner(&mut self, frame: &Frame) -> (Result<AccessUnit>, Option<u64>) {
        self.state.perf = ParserPerfStats::default();
        self.state.au_index = frame.index;
        self.state.au_offset = frame.offset;

        let reader = &mut BsIoSliceReader::from_slice(frame.as_ref());
        let access_unit = AccessUnit::read(&mut self.state, reader);

        let bit_offset = match access_unit {
            Ok(_) => None,
            Err(_) => bit_position(reader),
        };

        (access_unit, bit_offset)
    }

    /// Parses a frame, recording what fails instead of returning it.
    ///
    /// For [`DiagnosticMode::Collect`]. A check that ends the access unit is recorded
    /// like any other, the parser is then reset, and parsing resumes at the next frame
    /// carrying a major sync; frames skipped while resynchronising are not parsed and
    /// raise no diagnostics of their own. Returns `None` for an access unit that did not
    /// parse.
    ///
    /// A paired [`Decoder`](crate::process::decode::Decoder) must be reset with
    /// `reset_for_next_major_sync` whenever this returns `None`, or its state will
    /// silently diverge from the parser's.
    pub fn parse_recovering(&mut self, frame: &Frame) -> Option<AccessUnit> {
        if self.resyncing && !frame.is_major_sync() {
            self.state.au_index = frame.index;
            self.state.au_offset = frame.offset;

            return None;
        }

        let recorded = self.state.diagnostics.len();

        match self.parse_inner(frame) {
            (Ok(access_unit), _) => {
                self.resyncing = false;

                Some(access_unit)
            }
            (Err(error), bit_offset) => {
                self.record_parse_failure(recorded, error, bit_offset);
                self.reset_for_next_major_sync();
                self.resyncing = true;

                None
            }
        }
    }

    /// Keeps the error that ended an access unit.
    ///
    /// A check that fails at or below the fail level records itself on the way out, but
    /// leaves [`Diagnostic::source`] empty because the error is still travelling to here.
    /// An error raised outside the check macro has no diagnostic at all yet.
    fn record_parse_failure(
        &mut self,
        recorded: usize,
        error: anyhow::Error,
        bit_offset: Option<u64>,
    ) {
        if !self.state.is_collecting() {
            return;
        }

        if self.state.diagnostics.len() > recorded
            && let Some(diagnostic) = self.state.diagnostics.last_mut()
            && diagnostic.source.is_none()
        {
            diagnostic.source = Some(error);

            return;
        }

        let diagnostic = Diagnostic {
            rule: error.rule_id(),
            severity: log::Level::Error,
            location: self.state.location(bit_offset),
            message: error.to_string(),
            source: Some(error),
        };

        self.state.diagnostics.push(diagnostic);
    }

    /// What happens to a failed conformance check.
    pub fn set_diagnostic_mode(&mut self, mode: DiagnosticMode) {
        self.state.diagnostic_mode = mode;
    }

    pub fn diagnostic_mode(&self) -> DiagnosticMode {
        self.state.diagnostic_mode
    }

    /// Checks that have fired so far, oldest first.
    ///
    /// Always empty in [`DiagnosticMode::FailFast`], which records nothing.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.state.diagnostics
    }

    /// Takes the collected diagnostics, leaving none behind.
    pub fn take_diagnostics(&mut self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.state.diagnostics)
    }

    pub fn set_required_presentations(
        &mut self,
        required_presentations: &[bool; MAX_PRESENTATIONS],
    ) {
        self.state.required_presentations = *required_presentations;

        if let Some(presentation_map) = &self.state.presentation_map {
            self.state.substream_mask =
                presentation_map.substream_mask_by_required_presentations(required_presentations);
        }
    }

    pub fn hires_output_timing(&self) -> Option<usize> {
        self.state.hires_output_timing
    }

    /// Where parse time went for the most recent access unit.
    ///
    /// Every duration is zero unless the `perf` feature is enabled.
    pub fn last_parse_stats(&self) -> ParserPerfStats {
        self.state.perf
    }

    /// Number of seamless branch points that failed the buffer-model checks.
    ///
    /// These are conformance failures: the decoded samples are unaffected,
    /// but the stream is not a conformant splice.
    /// Every point where the stream's timing restarted, as a splice does.
    pub fn branches(&self) -> &[Branch] {
        &self.state.branches
    }

    /// Takes the branch points recorded so far, leaving the list empty.
    ///
    /// The list grows for the life of the parser, one entry per point the stream's
    /// timing restarts at. A pass over a file reads it at the end, and its size is
    /// bounded by the file; a decoder that runs for as long as something is playing has
    /// no end to read it at, and a stream that restarts its timing often enough grows it
    /// without bound. Such a consumer takes the list periodically — or takes it only to
    /// drop it, which is the point.
    pub fn take_branches(&mut self) -> Vec<Branch> {
        std::mem::take(&mut self.state.branches)
    }

    pub fn invalid_branches(&self) -> usize {
        self.state.branches.iter().filter(|b| !b.is_valid()).count()
    }

    /// Read-only view of the parser state for substream `i`.
    ///
    /// Gives realtime consumers access to per-substream DRC state
    /// (`drc_*`, `heavy_drc_*`) without exposing the parser internals
    /// mutably. Returns `None` for an out-of-range index.
    pub fn substream_state(&self, i: usize) -> Option<&ParserSubstreamState> {
        self.state.substream_i_state(i).ok()
    }

    /// Sets the failure level for validation errors.
    ///
    /// - `log::Level::Error`: Only fail on Error level messages (default)
    /// - `log::Level::Warn`: Fail on Warning level and above (strict mode)
    pub fn set_fail_level(&mut self, level: log::Level) {
        self.state.fail_level = level;
    }

    /// Whether a major sync may excuse the timing and output-timing checks.
    ///
    /// `true` (the default) treats a discontinuity at a major sync as a splice and skips
    /// the checks that a splice legitimately breaks. `false` evaluates them everywhere,
    /// which is what a conformance pass over a stream that is not spliced wants.
    pub fn set_allow_seamless_branch(&mut self, allow: bool) {
        self.state.allow_seamless_branch = allow;
    }

    /// Whether the byte-domain FIFO depth model runs.
    ///
    /// `true` (the default) accounts every access unit against the buffer model and
    /// reports an accumulator that exceeds its cap. The model answers whether a stream
    /// is *authorable*, which a conformance pass wants and a decoder does not: the
    /// samples are the same either way. A consumer that only decodes can turn it off.
    pub fn set_check_fifo(&mut self, check: bool) {
        self.state.check_fifo = check;
    }

    /// Deepest each byte-domain FIFO accumulator has been, in bytes.
    ///
    /// Indexed by [`Accumulator`](crate::utils::fifo::Accumulator): substream 0, the
    /// substream sums the 6-, 8- and 16-channel decoders read, then the whole stream.
    /// All zero unless FIFO checks are enabled.
    pub fn fifo_depth_peaks(&self) -> [usize; ACCUMULATORS] {
        self.state.fifo_depth.peaks()
    }

    /// Each accumulator's deepest point, split into the stream's own substream-segment
    /// bytes and the container overhead standing with them at that moment.
    ///
    /// The two parts always add back up to [`FifoPeak::total`], which is the figure
    /// [`fifo_depth_peaks`](Self::fifo_depth_peaks) reports and the cap applies to.
    pub fn fifo_depth_records(&self) -> [FifoPeak; ACCUMULATORS] {
        self.state.fifo_depth.peak_records()
    }

    /// Deepest each substream's own payload bytes have been, with no overhead priced in.
    ///
    /// A substream's own allowance is 15000 bytes per channel it carries, which
    /// [`ParserRestartState::min_chan`] and `max_chan` give.
    pub fn fifo_substream_peaks(&self) -> [usize; SUBSTREAMS] {
        self.state.fifo_depth.substream_peaks()
    }

    /// Highest instantaneous data rate the stream reached, in bits per second.
    ///
    /// Measured over the input-timing interval an access unit was delivered in, and
    /// taken only across intervals the timing checks did not flag as a jump.
    pub fn max_data_rate(&self) -> usize {
        self.state.max_data_rate
    }

    /// Access unit the maximum data rate was measured at.
    pub fn max_data_rate_au(&self) -> usize {
        self.state.max_data_rate_au_index
    }

    /// Deepest FIFO latency seen, in samples: how far an access unit's playback trails
    /// its delivery.
    pub fn max_fifo_latency(&self) -> usize {
        self.state.max_latency
    }

    /// Largest access unit seen, in bytes.
    pub fn max_access_unit_size(&self) -> usize {
        self.state.max_access_unit_size
    }

    /// Bytes of every access unit that parsed, for an average over their count.
    pub fn total_access_unit_bytes(&self) -> usize {
        self.state.total_access_unit_length << 1
    }

    /// Resets stream state after a fatal parse failure.
    ///
    /// After [`parse`](Self::parse) returns an error, internal state may be
    /// partially updated and is not safe to continue from. Calling this drops
    /// all stream state while preserving configuration (fail level, seamless
    /// branch tolerance, FIFO checks, required presentations), so parsing can
    /// resume at the next frame carrying a major sync.
    ///
    /// Any paired [`Decoder`](crate::process::decode::Decoder) must be reset
    /// with its own `reset_for_next_major_sync` at the same point in the frame
    /// sequence, or its state will silently diverge from the parser's.
    pub fn reset_for_next_major_sync(&mut self) {
        self.state.reset_for_next_major_sync();
    }
}

/// Per-substream parser state that outlives a restart header.
///
/// Everything a restart header re-establishes lives in [`ParserRestartState`] under
/// [`restart`](Self::restart) instead, so a field placed here keeps its value across
/// restarts by construction rather than by appearing in a list. Which of the two
/// structs a new field goes in is the whole decision; the reset never changes.
#[derive(Clone, Copy, Debug)]
pub struct ParserSubstreamState {
    pub crc_present: bool,
    pub substream_end_ptr: u16,

    pub drc_active: bool,
    pub drc_gain_update: i16,
    pub drc_time_update: u8,
    pub drc_count: usize,

    pub heavy_drc_active: bool,
    pub heavy_drc_present: bool,
    pub heavy_drc_gain_update: i16,
    pub heavy_drc_time_update: u8,
    pub heavy_drc_count: usize,

    /// Discarded wholesale by every restart header.
    pub restart: ParserRestartState,

    /// The bit this substream's last restart header carried.
    pub hires_output_timing: bool,
    pub hires_output_timing_state: HiresOutputTimingState,

    pub latency: usize,
    pub prev_latency: usize,

    pub output_timing_history: [usize; 128],
    pub substream_size_history: [usize; 128],
    pub history_index: usize,
}

/// Per-substream parser state a restart header re-establishes.
///
/// [`ParserState::reset_parser_substream_state`] replaces this with
/// [`Default`](Default::default) and touches nothing else, so a field added here is
/// reset at the next restart header with no other edit, and a field that must survive
/// one belongs in [`ParserSubstreamState`].
#[derive(Clone, Copy, Debug)]
pub struct ParserRestartState {
    pub block_index: usize,

    pub restart_sync_word: u16,
    pub min_chan: usize,
    pub max_chan: usize,
    pub max_matrix_chan: usize,
    pub max_shift: i8,
    pub max_lsbs: u32,
    pub error_protect: bool,

    pub guards: Guards,
    pub block_size: usize,

    pub primitive_matrices: usize,
    pub matrix_ch: [u8; 16],
    pub frac_bits: [u8; 16],
    pub lsb_bypass_used: [bool; 16],

    pub cf_mask: [u16; 16],
    pub delta_bits: [u8; 16],
    pub lsb_bypass_bit_count: [u8; 16],

    pub huff_offset: [i32; 16],
    pub huff_type: [usize; 16],
    pub huff_lsbs: [u32; 16],

    pub output_shift: [i8; 16],
    pub quantiser_step_size: [u32; 16],
}

impl Default for ParserSubstreamState {
    fn default() -> Self {
        Self {
            crc_present: false,
            substream_end_ptr: 0,

            drc_active: false,
            drc_gain_update: 0,
            drc_time_update: 0,
            drc_count: 0,

            hires_output_timing: false,

            heavy_drc_active: false,
            heavy_drc_present: false,
            heavy_drc_gain_update: 0,
            heavy_drc_time_update: 0,
            heavy_drc_count: 0,

            restart: ParserRestartState::default(),

            hires_output_timing_state: HiresOutputTimingState::default(),

            latency: 0,
            prev_latency: 0,

            output_timing_history: [0; 128],
            substream_size_history: [0; 128],
            history_index: 0,
        }
    }
}

impl Default for ParserRestartState {
    fn default() -> Self {
        Self {
            block_index: 0,
            restart_sync_word: 0,
            min_chan: 0,
            max_chan: 0,
            max_matrix_chan: 0,
            max_shift: 0,
            max_lsbs: 0,
            error_protect: false,

            guards: Guards::default(),
            block_size: 8,

            primitive_matrices: 0,
            matrix_ch: [0; 16],
            frac_bits: [0; 16],
            lsb_bypass_used: [false; 16],

            cf_mask: [0; 16],
            delta_bits: [0; 16],
            lsb_bypass_bit_count: [0; 16],

            huff_offset: [0; 16],
            huff_type: [0; 16],
            huff_lsbs: [24; 16],

            output_shift: [0; 16],
            quantiser_step_size: [0; 16],
        }
    }
}

/// The four buffer-model conditions a branch must meet to be seamless. A decoder that
/// starts at the branch has to be able to play across it without its input running dry or
/// overflowing, and each condition bounds one way that can fail.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BranchConditions {
    /// The advance grew by no more than three quarters of an access unit.
    pub advance_step: bool,
    /// The advance stays inside what the previous access unit had buffered.
    pub fifo_duration: bool,
    /// The advance stays inside the 75 ms the buffer model allows.
    pub within_75ms: bool,
    /// The access unit before the branch fits the peak data rate over the interval.
    pub data_rate: bool,
}

impl BranchConditions {
    pub const fn is_valid(&self) -> bool {
        self.advance_step && self.fifo_duration && self.within_75ms && self.data_rate
    }

    /// The conditions that failed, named as the report names them.
    pub fn failed(&self) -> Vec<&'static str> {
        [
            (self.advance_step, "advance step"),
            (self.fifo_duration, "FIFO duration"),
            (self.within_75ms, "75 ms limit"),
            (self.data_rate, "peak data rate"),
        ]
        .into_iter()
        .filter_map(|(met, name)| (!met).then_some(name))
        .collect()
    }
}

/// A point where the stream's timing restarts, which is what a splice leaves behind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Branch {
    /// Access unit the branch was found at.
    pub au_index: usize,
    /// Byte offset of that access unit from the start of the stream.
    pub byte_offset: u64,
    /// Samples played before it.
    pub sample: u64,
    /// Samples the decoder is running ahead of playback at the branch.
    pub advance: usize,
    pub conditions: BranchConditions,
}

impl Branch {
    pub const fn is_valid(&self) -> bool {
        self.conditions.is_valid()
    }
}

#[derive(Debug)]
#[repr(C)]
pub struct ParserState {
    // hyper
    pub fail_level: log::Level,
    pub allow_seamless_branch: bool,
    pub check_fifo: bool,
    pub diagnostic_mode: DiagnosticMode,

    /// Checks that fired, in the order they fired. Only filled in
    /// [`DiagnosticMode::Collect`].
    pub diagnostics: Vec<Diagnostic>,

    /// Location of the access unit being parsed, taken from its [`Frame`].
    pub au_index: u64,
    pub au_offset: u64,

    /// Access units between each of the last few major syncs, most recent first.
    pub restart_gap: [usize; RESTART_GAP_HISTORY],
    pub last_major_sync_index: usize,
    pub au_counter: usize,
    pub is_major_sync: bool,
    pub fifo_depth: FifoDepthState,
    /// Synthetic output clock for the FIFO model: the unwrapped output timing of the
    /// first access unit, advanced one access unit per access unit, never re-synchronised.
    pub fifo_output_clock: Option<usize>,
    pub has_parsed_au: bool,
    pub segment_start: bool,

    pub au_start_pos: usize,

    pub access_unit_length: usize,
    pub prev_access_unit_length: usize,
    pub total_access_unit_length: usize,

    pub au_end_pos_bit: usize,

    pub max_data_rate: usize,
    pub max_data_rate_au_index: usize,
    /// Deepest FIFO latency seen, in samples.
    pub max_latency: usize,
    /// Largest access unit seen, in bytes.
    pub max_access_unit_size: usize,

    pub advance: usize,
    pub prev_advance: usize,

    pub fifo_duration: usize,
    pub prev_fifo_duration: usize,

    pub input_timing: usize,
    pub first_input_timing: usize,
    pub prev_input_timing: usize,
    pub wrapped_input_timing: usize,

    pub output_timing: usize,
    /// First `output_timing` of the access unit being parsed and the substream that
    /// carried it, which every other substream in the access unit must agree with.
    pub au_output_timing: Option<(usize, u16)>,
    pub first_output_timing: usize,
    /// 1456
    pub output_timing_deviation: usize,
    pub hires_output_timing: Option<usize>,

    pub unwrapped_input_timing: usize,
    pub prev_unwrapped_input_timing: usize,
    pub first_unwrapped_input_timing: usize,

    pub input_timing_jump: bool,
    pub output_timing_jump: bool,
    /// 1452
    pub peak_data_rate_jump: bool,
    pub has_valid_branch: bool,
    pub has_substream_info_changed: bool,
    pub branches: Vec<Branch>,

    /// Parse timing for the current access unit; see the `perf` feature.
    pub perf: ParserPerfStats,

    pub variable_rate: bool,
    pub peak_data_rate: usize,
    pub prev_peak_data_rate: usize,

    // pub quantization_word_length_1: u8,
    // pub quantization_word_length_2: u8,
    pub audio_sampling_frequency_1: u32,
    // pub audio_sampling_frequency_2: u32,
    pub samples_per_au: usize,
    pub format_sync: u32,
    pub flags: u16,

    pub presentation_map: Option<PresentationMap>,
    pub required_presentations: [bool; MAX_PRESENTATIONS],

    pub substreams: Option<usize>,
    pub extended_substream_info: u8,
    pub substream_info: u8,

    pub has_parsed_substream: bool,

    pub substream_segment_start_pos: u64,
    pub substream_index: usize,
    pub substream_mask: u8,
    pub substream_state: [ParserSubstreamState; MAX_PRESENTATIONS],

    pub crc_restart_block_header: Crc8,
    pub crc_substream: Crc8,
    pub crc_major_sync_info: Crc16,

    pub bypassed_lsb: [[i32; 16]; 160],
    pub sample_buffer: [[i32; 16]; 160],
}

impl Default for ParserState {
    fn default() -> Self {
        Self {
            fail_level: log::Level::Error,
            allow_seamless_branch: true,
            check_fifo: true,
            diagnostic_mode: DiagnosticMode::default(),
            diagnostics: Vec::new(),
            au_index: 0,
            au_offset: 0,
            restart_gap: [0, 8, 8, 8],

            last_major_sync_index: 0,
            au_counter: 0,
            is_major_sync: false,
            fifo_depth: FifoDepthState::default(),
            fifo_output_clock: None,
            segment_start: false,
            has_parsed_au: false,

            au_start_pos: 0,

            access_unit_length: 0,
            prev_access_unit_length: 0,
            total_access_unit_length: 0,

            au_end_pos_bit: 0,

            max_data_rate: 0,
            max_data_rate_au_index: 0,
            max_latency: 0,
            max_access_unit_size: 0,

            advance: 0,
            prev_advance: 0,

            fifo_duration: 0,
            prev_fifo_duration: 0,

            input_timing: 0,
            first_input_timing: 0,
            prev_input_timing: 0,
            wrapped_input_timing: 0,

            output_timing: 0,
            au_output_timing: None,
            first_output_timing: 0,
            output_timing_deviation: 0,
            hires_output_timing: None,

            // quantization_word_length_1: 0,
            // quantization_word_length_2: 0,
            unwrapped_input_timing: 0,
            prev_unwrapped_input_timing: 0,
            first_unwrapped_input_timing: 0,

            input_timing_jump: false,
            output_timing_jump: false,
            peak_data_rate_jump: false,
            has_valid_branch: false,
            has_substream_info_changed: false,
            branches: Vec::new(),
            perf: ParserPerfStats::default(),

            variable_rate: false,
            peak_data_rate: 0,
            prev_peak_data_rate: 0,

            audio_sampling_frequency_1: 0,
            // audio_sampling_frequency_2: 0,
            samples_per_au: 0,
            format_sync: 0,
            flags: 0,

            presentation_map: None,
            required_presentations: [true; MAX_PRESENTATIONS],

            substreams: None,
            extended_substream_info: 0,
            substream_info: 0,

            has_parsed_substream: false,

            substream_segment_start_pos: 0,
            substream_index: 0,
            substream_mask: 0,
            substream_state: [ParserSubstreamState::default(); MAX_PRESENTATIONS],

            crc_restart_block_header: Crc8::new(&CRC_RESTART_BLOCK_HEADER_ALG),
            crc_substream: Crc8::new(&CRC_SUBSTREAM_ALG),
            crc_major_sync_info: Crc16::new(&CRC_MAJOR_SYNC_INFO_ALG),

            bypassed_lsb: [[0; 16]; 160],
            sample_buffer: [[0; 16]; 160],
        }
    }
}

impl DiagnosticSink for ParserState {
    fn fail_level(&self) -> log::Level {
        self.fail_level
    }

    fn diagnostic_mode(&self) -> DiagnosticMode {
        self.diagnostic_mode
    }

    fn location(&self, bit_offset: Option<u64>) -> Location {
        Location {
            au_index: self.au_index,
            au_offset: self.au_offset,
            bit_offset,
        }
    }

    fn push_diagnostic(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }
}

impl ParserState {
    /// Records a branch at the current access unit. A jump is found once per substream
    /// that reads its restart header, so the record is merged rather than repeated, and a
    /// condition that fails for any substream fails for the branch.
    pub fn record_branch(&mut self, advance: usize, conditions: BranchConditions) {
        if let Some(branch) = self.branches.last_mut()
            && branch.au_index == self.au_counter
        {
            let met = branch.conditions;
            branch.conditions = BranchConditions {
                advance_step: met.advance_step && conditions.advance_step,
                fifo_duration: met.fifo_duration && conditions.fifo_duration,
                within_75ms: met.within_75ms && conditions.within_75ms,
                data_rate: met.data_rate && conditions.data_rate,
            };

            return;
        }

        self.branches.push(Branch {
            au_index: self.au_counter,
            byte_offset: self.au_offset,
            sample: self.au_counter as u64 * self.samples_per_au as u64,
            advance,
            conditions,
        });
    }

    pub fn reset_for_next_major_sync(&mut self) {
        let diagnostics = std::mem::take(&mut self.diagnostics);

        *self = Self {
            fail_level: self.fail_level,
            allow_seamless_branch: self.allow_seamless_branch,
            check_fifo: self.check_fifo,
            required_presentations: self.required_presentations,
            branches: self.branches.clone(),
            diagnostic_mode: self.diagnostic_mode,
            diagnostics,
            au_index: self.au_index,
            au_offset: self.au_offset,
            ..Default::default()
        };
    }

    pub fn expected_au_end_pos(&self) -> usize {
        self.au_start_pos + (self.access_unit_length << 4)
    }

    pub fn substream_state_mut(&mut self) -> Result<&mut ParserSubstreamState> {
        self.substream_i_state_mut(self.substream_index)
    }

    pub fn substream_state(&self) -> Result<&ParserSubstreamState> {
        self.substream_i_state(self.substream_index)
    }

    pub fn substream_i_state_mut(&mut self, i: usize) -> Result<&mut ParserSubstreamState> {
        self.check_substream(i)?;
        Ok(&mut self.substream_state[i])
    }

    pub fn substream_i_state(&self, i: usize) -> Result<&ParserSubstreamState> {
        self.check_substream(i)?;
        Ok(&self.substream_state[i])
    }

    pub fn has_jump(&self) -> bool {
        self.peak_data_rate_jump || self.input_timing_jump || self.output_timing_jump
    }

    // TODO: provide iterator for sss here

    pub fn reset_parser_substream_state(&mut self) {
        self.substream_state[self.substream_index].restart = ParserRestartState::default();
    }

    /// Restarts the timing and FIFO model as if the stream began at this access unit.
    pub fn restart_stream_for_branch(&mut self, output_timing: usize) {
        self.output_timing_deviation = 0;
        self.unwrapped_input_timing = self.input_timing;
        self.prev_unwrapped_input_timing = 0;
        self.first_input_timing = self.input_timing;
        self.first_unwrapped_input_timing = self.unwrapped_input_timing;

        let mut output_timing = output_timing;
        if output_timing < self.input_timing {
            output_timing += 0x10000;
        }
        self.output_timing = output_timing;
        self.first_output_timing = output_timing;
        self.fifo_output_clock = None;

        for ss_state in &mut self.substream_state {
            ss_state.history_index = 0;
        }

        self.fifo_depth.restart();
        self.segment_start = true;
    }

    pub fn reset_for_branch(&mut self) {
        for ss_state in &mut self.substream_state {
            ss_state.hires_output_timing_state.reset_for_branch()
        }
    }

    fn check_substream(&self, i: usize) -> Result<()> {
        let Some(substreams) = self.substreams else {
            bail!(ParseError::NoSubstream);
        };

        if substreams <= i {
            bail!(ParseError::InvalidSubstreamIndex(i + 1, substreams));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Branch, Parser};
    use log::Level;

    /// The branch list has no end to be read at in a decoder that runs for as long as
    /// something is playing, so it must be possible to empty it.
    #[test]
    fn taking_the_branches_empties_the_list() {
        let mut parser = Parser::default();
        parser.state.branches.push(Branch::default());
        parser.state.branches.push(Branch::default());

        assert_eq!(parser.take_branches().len(), 2);
        assert!(parser.branches().is_empty(), "the list is left empty");
        assert!(
            parser.take_branches().is_empty(),
            "taking twice is harmless"
        );
    }

    /// The buffer model answers whether a stream is authorable, which a decoder does not
    /// ask. It runs by default and a consumer that only decodes can turn it off.
    #[test]
    fn the_fifo_depth_model_can_be_turned_off() {
        let mut parser = Parser::default();
        assert!(parser.state.check_fifo, "on by default");

        parser.set_check_fifo(false);
        assert!(!parser.state.check_fifo);
    }

    #[test]
    fn reset_preserves_configuration() {
        let mut parser = Parser::default();
        parser.set_fail_level(Level::Warn);
        parser.set_required_presentations(&[true, false, true, false]);

        // Simulate accumulated stream state
        parser.state.has_parsed_au = true;
        parser.state.au_counter = 42;
        parser.state.input_timing = 1234;

        parser.reset_for_next_major_sync();

        assert_eq!(parser.state.fail_level, Level::Warn);
        assert_eq!(
            parser.state.required_presentations,
            [true, false, true, false]
        );
        assert!(!parser.state.has_parsed_au);
        assert_eq!(parser.state.au_counter, 0);
        assert_eq!(parser.state.input_timing, 0);
    }

    /// FBB streams bailed with `unimplemented!` at three sites, so the crate could not read
    /// them at all even though the format info, substream and restart header paths were
    /// already written.
    fn first_au(data: &[u8]) -> Option<crate::structs::access_unit::AccessUnit> {
        let mut extractor = crate::process::extract::Extractor::default();
        extractor.push_bytes(data);
        let frame = extractor.by_ref().next()?.ok()?;
        Parser::default().parse(&frame).ok()
    }

    #[test]
    fn fbb_stream_parses() {
        let au = first_au(crate::process::EXAMPLE_DATA_FBB).expect("an access unit");
        let ms = au.major_sync_info.as_ref().expect("a major sync");
        assert_eq!(ms.format_sync, crate::structs::sync::MAJOR_SYNC_FBB);
        assert_eq!(ms.format_info.sampling_frequency_1().unwrap(), 48000);
    }

    /// The FBB `channel_meaning` is its own 64-bit structure. It is the same width as the
    /// FBA one and in the same place, so the CRC and everything after it come out right
    /// either way; only the field values say which layout was read.
    #[test]
    fn fbb_channel_meaning_is_read_with_the_fbb_layout() {
        let au = first_au(crate::process::EXAMPLE_DATA_FBB).expect("an access unit");
        let ms = au.major_sync_info.as_ref().expect("a major sync");

        assert!(ms.channel_meaning.fba().is_none(), "not the FBA layout");
        let cm = ms.channel_meaning.fbb().expect("the FBB layout");

        assert_eq!(cm.fs, 10);
        assert_eq!(cm.wordwidth, 24);
        assert_eq!(cm.channel_occupancy, 0x3F);
        assert_eq!(cm.mlp_multi_channel_type, 0);
        assert_eq!(cm.speaker_layout, 0);
        assert_eq!(cm.copy_protection, 0);
        assert_eq!(cm.level_control, 0x8080);
        assert!(!cm.hdcd_process);
        assert_eq!(cm.reserved2, 0);
        assert_eq!(cm.source_format, 0);
        assert_eq!(cm.summary_info, 0);

        // The last bit of the block is the last bit of summary_info, not the FBA
        // extra_channel_meaning_present, so nothing follows it but the CRC.
        assert!(ms.channel_meaning.extra_channel_meaning().is_none());
    }

    /// FBA is unaffected by the FBB work.
    #[test]
    fn fba_stream_still_parses() {
        let au = first_au(crate::process::EXAMPLE_DATA).expect("an access unit");
        let ms = au.major_sync_info.as_ref().expect("a major sync");
        assert_eq!(ms.format_sync, crate::structs::sync::MAJOR_SYNC_FBA);
    }

    /// A single-substream FBB access unit carrying `substream_info` 0x05. Judged by the
    /// FBA rules, its low two bits looked like reserved bits set and the last bit of its
    /// channel_meaning looked like `extra_channel_meaning_present`, which read the block
    /// that follows past the major sync info CRC, so no frame came out at all.
    #[test]
    fn fbb_unextractable_stream_extracts() {
        let au = first_au(crate::process::EXAMPLE_DATA_FBB_UNEXTRACTABLE).expect("an access unit");
        let ms = au.major_sync_info.as_ref().expect("a major sync");
        assert_eq!(ms.format_sync, crate::structs::sync::MAJOR_SYNC_FBB);
        assert_eq!(ms.substream_info, 0x05);
        assert_eq!(ms.channel_meaning.fbb().expect("the FBB layout").fs, 10);
    }

    /// The major-sync repetition limit differs by format: 128 access units for FBA, 32 for
    /// FBB.
    #[test]
    fn fbb_sync_interval_limit_is_stricter_than_fba() {
        use crate::utils::errors::AccessUnitError;
        assert_eq!(
            AccessUnitError::FbbSyncTooFar.to_string(),
            "FBB stream major syncs must occur at intervals not exceeding 32 access units"
        );
        assert_eq!(
            AccessUnitError::FbaSyncTooFar.to_string(),
            "FBA stream major syncs must occur at intervals not exceeding 128 access units"
        );
    }
}
