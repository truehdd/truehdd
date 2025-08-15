use anyhow::{Result, anyhow, bail};
use log::Level::{Error, Warn};
use log::{trace, warn};

use crate::log_or_err;
use crate::process::MAX_PRESENTATIONS;
use crate::process::decode::DecoderState;
use crate::process::parse::ParserState;
use crate::structs::channel::ChannelLabel;
use crate::structs::extra_data::ExtraData;
use crate::structs::substream::{SubstreamDirectory, SubstreamSegment};
use crate::structs::sync::{MAJOR_SYNC_FBA, MAJOR_SYNC_FBB, MajorSyncInfo, UNIMPLEMENTED_FBB_MSG};
use crate::utils::bitstream_io::BsIoSliceReader;
use crate::utils::errors::AccessUnitError;

/// A parsed access unit containing structured audio data and metadata.
///
/// Access units are the fundamental structural elements of TrueHD bitstreams.
/// Contains timing information, optional major sync data, substream directory,
/// and compressed audio segments.
///
#[derive(Debug, Default)]
pub struct AccessUnit {
    /// Check nibble for access unit validation.
    ///
    /// 4-bit checksum for header validation.
    pub check_nibble: u8,

    /// Length of this access unit in 16-bit words.
    ///
    /// 12-bit field indicating total access unit length.
    pub access_unit_length: u16,

    /// Input timing value for FIFO buffer management.
    ///
    /// 16-bit timing value for buffer management.
    pub input_timing: u16,

    /// Major sync information (present only in major sync access units).
    ///
    /// Contains stream configuration and decoder initialization parameters.
    pub major_sync_info: Option<MajorSyncInfo>,

    /// Substream directory for navigation and CRC control.
    ///
    /// Array of directory entries containing end pointers and control flags.
    pub substream_directory: [SubstreamDirectory; MAX_PRESENTATIONS],

    /// Parsed substream segments containing compressed audio blocks.
    ///
    /// Array of substream segments containing compressed audio data.
    pub substream_segment: [SubstreamSegment; MAX_PRESENTATIONS],

    /// Optional extra data and extensions.
    ///
    /// Contains auxiliary information including object audio metadata.
    pub extra_data: Option<ExtraData>,

    /// Indicates if this access unit is at a valid branch point.
    pub has_valid_branch: bool,
}

impl AccessUnit {
    pub fn read(state: &mut ParserState, reader: &mut BsIoSliceReader) -> Result<Self> {
        state.is_major_sync = false;

        if !state.has_valid_branch {
            state.prev_access_unit_length = state.access_unit_length;
            state.prev_advance = state.advance;
            state.prev_fifo_duration = state.fifo_duration;
            state.prev_input_timing = state.input_timing;
            state.prev_unwrapped_input_timing = state.unwrapped_input_timing;
            state.prev_peak_data_rate = state.peak_data_rate;
        }

        state.input_timing_jump = false;
        state.output_timing_jump = false;
        state.peak_data_rate_jump = false;
        state.has_substream_info_changed = false;

        let mut au = Self {
            check_nibble: reader.get_n(4)?,
            access_unit_length: reader.get_n(12)?,
            input_timing: reader.get_n(16)?,
            ..Default::default()
        };

        state.input_timing = au.input_timing as usize;

        if !state.has_parsed_au {
            state.first_input_timing = au.input_timing as usize;
        }

        {
            let mut unwrapped_input_timing =
                au.input_timing
                    .wrapping_sub(state.output_timing_deviation as u16) as usize;

            while state.prev_unwrapped_input_timing > unwrapped_input_timing {
                unwrapped_input_timing += 0x10000;
            }

            trace!(
                "AU {}: unwrapped_input_timing = {}",
                state.au_counter, unwrapped_input_timing
            );

            state.unwrapped_input_timing = unwrapped_input_timing;

            if !state.has_parsed_au {
                state.first_unwrapped_input_timing = state.unwrapped_input_timing;
            }
        }

        let mut parity = reader.parity_check_nibble_for_last_n_bits(32)?;

        // TODO:
        // stream access_unit_length must be >= %d. Read %d. 2000
        // FBB stream access_unit_length must be <= %d. Read %d. 768

        state.access_unit_length = au.access_unit_length as usize;
        state.au_end_pos_bit += state.access_unit_length << 4;

        let test_bytes: u32 = reader.get_n(32)?;
        reader.seek(-32)?;

        if test_bytes == MAJOR_SYNC_FBA {
            au.major_sync_info = Some(MajorSyncInfo::read(state, reader)?);

            let suffix = if state.last_major_sync_index > 0 {
                format!(
                    "after {} AU",
                    state.au_counter - state.last_major_sync_index
                )
            } else {
                String::new()
            };

            trace!("AU {}: Major sync found {}", state.au_counter, suffix);

            state.last_major_sync_index = state.au_counter;
        } else if test_bytes == MAJOR_SYNC_FBB {
            // TODO: Implement FBB
            unimplemented!("{}", UNIMPLEMENTED_FBB_MSG)
        } else {
            // no major sync, update gap check

            if !state.has_parsed_au {
                bail!(AccessUnitError::MissingInitialSync)
            }
        }

        let major_sync_interval = state.au_counter - state.last_major_sync_index;

        // TODO: 32 for FBB
        if state.format_sync == MAJOR_SYNC_FBA && major_sync_interval > 128 {
            log_or_err!(state, Warn, anyhow!(AccessUnitError::FbaSyncTooFar));
        }

        // TODO: restart gap check

        Self::check_fifo(state)?;

        let minor_start_pos = reader.position()?;

        let Some(substreams) = state.substreams else {
            bail!(AccessUnitError::NoSubstream)
        };

        for i in 0..substreams {
            state.substream_index = i;
            au.substream_directory[i] = SubstreamDirectory::read(state, reader)?;
        }

        state.has_valid_branch = false;

        if reader.position()? & 7 != 0 {
            bail!(AccessUnitError::MisalignedSync)
        }

        let minor_end_pos = reader.position()?;

        parity ^= reader.parity_check_nibble_for_last_n_bits(minor_end_pos - minor_start_pos)?;

        if parity != 0xF {
            bail!(AccessUnitError::NibbleParity(parity));
        }

        state.substream_segment_start_pos = reader.position()?;
        state.has_parsed_substream = false;

        for i in 0..substreams {
            state.substream_index = i;

            if state.substream_mask >> i & 1 == 0 {
                let offset = state.substream_segment_start_pos
                    + ((state.substream_state()?.substream_end_ptr as u64) << 4)
                    - reader.position()?;
                reader.skip_n(offset as u32)?;

                trace!("Skipping substream {i} segment of length {offset}");
                continue;
            }
            au.substream_segment[i] = SubstreamSegment::read(state, reader)?;
            state.has_parsed_substream = true;
        }

        if state.expected_au_end_pos() > reader.position()? as usize + 16 {
            let extra_data = ExtraData::read(state, reader)?;
            au.extra_data = Some(extra_data);
        }

        state.has_parsed_au = true;

        if reader.position()? <= state.expected_au_end_pos() as u64 {
            state.total_access_unit_length += au.access_unit_length as usize;
        } else {
            log_or_err!(
                state,
                Error,
                anyhow!(AccessUnitError::AccessUnitTooLong(
                    reader.position()? as usize,
                    state.expected_au_end_pos()
                ))
            );
        }

        state.au_counter += 1; // TODO: migrate to gap check, should reset on sync check

        au.has_valid_branch = state.has_valid_branch || state.has_substream_info_changed;

        Ok(au)
    }

    pub fn get_channel_labels(&self, presentation_index: usize) -> Option<Vec<ChannelLabel>> {
        let major_sync_info = self.major_sync_info.as_ref()?;

        match presentation_index {
            0 => {
                if self
                    .substream_segment
                    .as_ref()
                    .first()?
                    .block
                    .first()?
                    .restart_header
                    .as_ref()?
                    .max_matrix_chan
                    == 0
                {
                    Some(vec![ChannelLabel::C])
                } else {
                    Some(vec![ChannelLabel::L, ChannelLabel::R])
                }
            }
            1 => ChannelLabel::from_sixch_channel(
                major_sync_info.format_info.sixch_decoder_channel_assignment,
            )
            .ok(),
            2 => ChannelLabel::from_eightch_channel(
                major_sync_info
                    .format_info
                    .eightch_decoder_channel_assignment,
                major_sync_info.flags,
            )
            .ok(),
            3 => {
                let ext_meaning = major_sync_info
                    .channel_meaning
                    .extra_channel_meaning
                    .as_ref()?;

                if ext_meaning.dyn_object_only && ext_meaning.lfe_present || ext_meaning.lfe_only {
                    Some(vec![ChannelLabel::LFE])
                } else {
                    ChannelLabel::from_sixteenth_channel(ext_meaning.sixteench_channel_assignment)
                        .ok()
                }
            }
            _ => None,
        }
    }

    fn check_fifo(state: &mut ParserState) -> Result<()> {
        if !state.check_fifo {
            return Ok(());
        }

        // peak data rate check
        let peak_data_rate = state.peak_data_rate;

        state.fifo_duration = if peak_data_rate != 0 {
            let fifo_duration = (state.access_unit_length << 8).div_ceil(peak_data_rate);

            trace!(
                "AU {}: length={}, peak_rate={}, fifo_duration={}",
                state.au_counter, state.access_unit_length, peak_data_rate, fifo_duration
            );

            fifo_duration
        } else {
            0
        };

        let max_data_rate = if state.format_sync == MAJOR_SYNC_FBA {
            288000000
        } else {
            153600000
        };

        if state.peak_data_rate * state.audio_sampling_frequency_1 as usize > max_data_rate {
            warn!("Peak data rate exceeds maximum allowed");
        }

        if !state.has_parsed_au {
            return Ok(());
        }

        let input_timing_interval = if state.has_valid_branch {
            state
                .unwrapped_input_timing
                .wrapping_sub(state.prev_unwrapped_input_timing)
        } else {
            state.input_timing.wrapping_sub(state.prev_input_timing) & 0xFFFF
        };

        trace!(
            "AU {}: input_timing {}, prev_input_timing {}, input_timing_interval {}",
            state.au_counter, state.input_timing, state.prev_input_timing, input_timing_interval
        );

        let samples_per_75ms = (state.audio_sampling_frequency_1 * 3).div_ceil(40);

        if input_timing_interval < state.samples_per_au >> 2 {
            if !state.allow_seamless_branch || !state.is_major_sync {
                log_or_err!(
                    state,
                    Warn,
                    anyhow!(AccessUnitError::TimingTooShort(
                        state.input_timing,
                        state.prev_input_timing,
                        state.samples_per_au >> 2
                    ))
                );
            }

            if state.has_valid_branch {
                log_or_err!(
                    state,
                    Warn,
                    anyhow!(AccessUnitError::TimingTooShortAfterJump)
                );
            }

            trace!("input_timing jump: input_timing[n]-input_timing[n-1]<samples_per_au/4");
            state.input_timing_jump = true;
        }

        if input_timing_interval < state.prev_fifo_duration {
            if !state.allow_seamless_branch || !state.is_major_sync {
                log_or_err!(
                    state,
                    Warn,
                    anyhow!(AccessUnitError::TimingShorterThanPrevious)
                );
            }

            if state.has_valid_branch {
                log_or_err!(
                    state,
                    Warn,
                    anyhow!(AccessUnitError::TimingShorterThanPreviousAfterJump)
                );
            }

            trace!("input_timing jump: input_timing[n]-input_timing[n-1]<duration[n-1]");
            state.input_timing_jump = true;
        }

        if state.variable_rate
            && (state.prev_access_unit_length << 8 > input_timing_interval * state.peak_data_rate)
        {
            if !state.allow_seamless_branch || !state.is_major_sync {
                log_or_err!(state, Warn, anyhow!(AccessUnitError::DataRateExceeded));
            }

            if state.has_valid_branch {
                log_or_err!(
                    state,
                    Warn,
                    anyhow!(AccessUnitError::DataRateExceededAfterJump)
                );
            }

            trace!("input_timing jump: apparent data_rate exceeds peak_data_rate");
            state.input_timing_jump = true;
        }

        if state.has_parsed_au && input_timing_interval > samples_per_75ms as usize {
            if !state.allow_seamless_branch || !state.is_major_sync {
                log_or_err!(state, Warn, anyhow!(AccessUnitError::TimingTooLong));
            }

            if state.has_valid_branch {
                log_or_err!(
                    state,
                    Warn,
                    anyhow!(AccessUnitError::TimingTooLongAfterJump)
                );
            }

            trace!("input_timing jump: input_timing[n]-input_timing[n-1] > samples_per_75ms");
            state.input_timing_jump = true;
        }

        if !state.input_timing_jump {
            let data_rate = (state.audio_sampling_frequency_1 as usize
                * (state.prev_access_unit_length << 4))
                .div_ceil(input_timing_interval);

            if data_rate > state.max_data_rate {
                state.max_data_rate = data_rate;
                state.max_data_rate_au_index = state.au_counter - 1;
            }
        }

        if !state.variable_rate {
            let data_rate_16x =
                (state.unwrapped_input_timing - state.first_input_timing) * state.peak_data_rate;
            let total_length_16x = state.total_access_unit_length << 8;
            if data_rate_16x.abs_diff(total_length_16x) >= 0x100 {
                log_or_err!(
                    state,
                    Warn,
                    anyhow!(AccessUnitError::FixedRateMismatch(
                        data_rate_16x,
                        total_length_16x
                    ))
                );
            }
        }

        Ok(())
    }

    pub fn update_decoder_state(&self, state: &mut DecoderState) -> Result<()> {
        state.has_valid_branch = self.has_valid_branch;
        if let Some(major_sync_info) = &self.major_sync_info {
            major_sync_info.update_decoder_state(state)?;
        } else if !state.valid {
            return Ok(());
        }

        Ok(())
    }
}
