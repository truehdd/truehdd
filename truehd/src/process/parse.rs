use anyhow::{Result, bail};

use crate::process::extract::Frame;
use crate::process::{MAX_PRESENTATIONS, PresentationMap};
use crate::structs::access_unit::AccessUnit;
use crate::structs::restart_header::Guards;
use crate::utils::bitstream_io::BsIoSliceReader;
use crate::utils::crc::{
    CRC_MAJOR_SYNC_INFO_ALG, CRC_RESTART_BLOCK_HEADER_ALG, CRC_SUBSTREAM_ALG, Crc8, Crc16,
};
use crate::utils::errors::ParseError;
use crate::utils::timing::HiresOutputTimingState;

/// Parses audio frames into structured access units.
///
/// Converts raw frame data into [`AccessUnit`] objects containing
/// parsed metadata, audio blocks, and timing information.
#[derive(Default)]
pub struct Parser {
    state: ParserState,
}

impl Parser {
    /// Parses an audio frame into a structured access unit.
    ///
    /// Returns an [`AccessUnit`] containing parsed metadata, audio blocks,
    /// and timing information. Handles both major sync frames (with stream
    /// configuration) and continuation frames (audio data only).
    pub fn parse(&mut self, frame: &Frame) -> Result<AccessUnit> {
        let reader = &mut BsIoSliceReader::from_slice(frame.as_ref());
        AccessUnit::read(&mut self.state, reader)
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

    /// Sets the failure level for validation errors.
    ///
    /// - `log::Level::Error`: Only fail on Error level messages (default)
    /// - `log::Level::Warn`: Fail on Warning level and above (strict mode)
    pub fn set_fail_level(&mut self, level: log::Level) {
        self.state.fail_level = level;
    }
}

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

    pub block_index: usize,

    pub restart_sync_word: u16,
    pub min_chan: usize,
    pub max_chan: usize,
    pub max_matrix_chan: usize,
    pub max_shift: i8,
    pub max_lsbs: u32,
    pub error_protect: bool,

    pub hires_output_timing_state: HiresOutputTimingState,

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

    pub latency: usize,
    pub prev_latency: usize,

    pub output_timing_history: [usize; 128],
    pub substream_size_history: [usize; 128],
    pub history_index: usize,
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

            heavy_drc_active: false,
            heavy_drc_present: false,
            heavy_drc_gain_update: 0,
            heavy_drc_time_update: 0,
            heavy_drc_count: 0,

            block_index: 0,
            restart_sync_word: 0,
            min_chan: 0,
            max_chan: 0,
            max_matrix_chan: 0,
            max_shift: 0,
            max_lsbs: 0,
            error_protect: false,

            hires_output_timing_state: HiresOutputTimingState::default(),

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

            latency: 0,
            prev_latency: 0,

            output_timing_history: [0; 128],
            substream_size_history: [0; 128],
            history_index: 0,
        }
    }
}

#[derive(Debug)]
#[repr(C)]
pub struct ParserState {
    // hyper
    pub fail_level: log::Level,
    pub allow_seamless_branch: bool,
    pub check_fifo: bool,

    pub restart_gap: [usize; MAX_PRESENTATIONS],
    pub last_major_sync_index: usize,
    pub au_counter: usize,
    pub is_major_sync: bool,
    pub has_parsed_au: bool,

    pub au_start_pos: usize,

    pub access_unit_length: usize,
    pub prev_access_unit_length: usize,
    pub total_access_unit_length: usize,

    pub au_end_pos_bit: usize,

    pub max_data_rate: usize,
    pub max_data_rate_au_index: usize,

    pub advance: usize,
    pub prev_advance: usize,

    pub fifo_duration: usize,
    pub prev_fifo_duration: usize,

    pub input_timing: usize,
    pub first_input_timing: usize,
    pub prev_input_timing: usize,
    pub wrapped_input_timing: usize,

    pub output_timing: usize,
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
            restart_gap: [0, 8, 8, 8],

            last_major_sync_index: 0,
            au_counter: 0,
            is_major_sync: false,
            has_parsed_au: false,

            au_start_pos: 0,

            access_unit_length: 0,
            prev_access_unit_length: 0,
            total_access_unit_length: 0,

            au_end_pos_bit: 0,

            max_data_rate: 0,
            max_data_rate_au_index: 0,

            advance: 0,
            prev_advance: 0,

            fifo_duration: 0,
            prev_fifo_duration: 0,

            input_timing: 0,
            first_input_timing: 0,
            prev_input_timing: 0,
            wrapped_input_timing: 0,

            output_timing: 0,
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

impl ParserState {
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
        let ss_state = &mut self.substream_state[self.substream_index];
        *ss_state = ParserSubstreamState {
            crc_present: ss_state.crc_present,
            substream_end_ptr: ss_state.substream_end_ptr,
            drc_active: ss_state.drc_active,
            drc_gain_update: ss_state.drc_gain_update,
            drc_time_update: ss_state.drc_time_update,
            drc_count: ss_state.drc_count,
            hires_output_timing_state: ss_state.hires_output_timing_state,
            latency: ss_state.latency,
            prev_latency: ss_state.prev_latency,
            output_timing_history: ss_state.output_timing_history,
            substream_size_history: ss_state.substream_size_history,
            history_index: ss_state.history_index,
            // state: ss_state.coeff_state,
            ..Default::default()
        }
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
