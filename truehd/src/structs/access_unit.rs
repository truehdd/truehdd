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
use crate::structs::sync::{MAJOR_SYNC_FBA, MAJOR_SYNC_FBB, MajorSyncInfo};
use crate::utils::bitstream_io::BsIoSliceReader;
use crate::utils::errors::{AccessUnitError, FifoError};
use crate::utils::fifo::{ACCUMULATORS, Accumulator};
use crate::utils::perf::Timer;

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
        let access_unit = Timer::start();
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

        if test_bytes == MAJOR_SYNC_FBA || test_bytes == MAJOR_SYNC_FBB {
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
        } else {
            // no major sync, update gap check

            if !state.has_parsed_au {
                bail!(AccessUnitError::MissingInitialSync)
            }
        }

        let major_sync_interval = state.au_counter - state.last_major_sync_index;

        // FBA repeats its major sync at least every 128 access units, FBB every 32.
        let (sync_limit, too_far) = if state.format_sync == MAJOR_SYNC_FBB {
            (32, AccessUnitError::FbbSyncTooFar)
        } else {
            (128, AccessUnitError::FbaSyncTooFar)
        };

        if major_sync_interval > sync_limit {
            log_or_err!(state, Warn, anyhow!(too_far), reader);
        }

        // TODO: restart gap check

        Self::check_fifo(state)?;

        let minor_start_pos = reader.position()?;

        let Some(substreams) = state.substreams else {
            bail!(AccessUnitError::NoSubstream)
        };

        let directories = Timer::start();
        for i in 0..substreams {
            state.substream_index = i;
            au.substream_directory[i] = SubstreamDirectory::read(state, reader)?;
        }
        directories.record(&mut state.perf.substream_directories);

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

        let segments = Timer::start();
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
        segments.record(&mut state.perf.substream_segments);

        if state.expected_au_end_pos() > reader.position()? as usize + 16 {
            let timer = Timer::start();
            let extra_data = ExtraData::read(state, reader)?;
            timer.record(&mut state.perf.extra_data);
            au.extra_data = Some(extra_data);
        }

        state.has_parsed_au = true;
        access_unit.record(&mut state.perf.access_unit_total);

        if reader.position()? <= state.expected_au_end_pos() as u64 {
            state.total_access_unit_length += au.access_unit_length as usize;
        } else {
            log_or_err!(
                state,
                Error,
                anyhow!(AccessUnitError::AccessUnitTooLong(
                    reader.position()? as usize,
                    state.expected_au_end_pos()
                )),
                reader
            );
        }

        Self::check_fifo_depth(state, &au)?;

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

        let samples_per_75ms =
            crate::structs::sync::samples_per_75ms(state.audio_sampling_frequency_1);

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

    /// Byte-domain FIFO depth check. Contributions are priced in bits from the directory
    /// end pointers and fixed header sizes, summed per decoder, and divided by eight
    /// once at the end. See [`crate::utils::fifo`] for the window semantics.
    fn check_fifo_depth(state: &mut ParserState, au: &AccessUnit) -> Result<()> {
        if !state.check_fifo {
            return Ok(());
        }

        let is_fba = state.format_sync == MAJOR_SYNC_FBA;

        if !is_fba && state.format_sync != MAJOR_SYNC_FBB {
            return Ok(());
        }

        let Some(substreams) = state.substreams else {
            return Ok(());
        };

        // The header is priced at fixed sizes rather than measured from the parsed bits:
        // 32 for the access unit header, 224 more for a major sync, plus the extra
        // channel meaning block when an FBA major sync carries one.
        let base: u64 = 32
            + match &au.major_sync_info {
                Some(ms) => {
                    224 + match &ms.channel_meaning.extra_channel_meaning {
                        Some(ecm) if is_fba => 16 * (ecm.extra_channel_meaning_length as u64 + 1),
                        _ => 0,
                    }
                }
                None => 0,
            };

        // EXTRA_DATA costs its 16-bit header word plus its payload words, in every sum.
        let extra_bits: u64 = match &au.extra_data {
            Some(extra) if extra.extra_data_length != 0 => 16 * extra.extra_data_length as u64 + 16,
            _ => 0,
        };

        // A substream region is its directory entry (one word, two with the extra word)
        // plus its payload, priced as the difference of cumulative end pointers.
        let mut region = [0u64; MAX_PRESENTATIONS];
        let mut previous_end = 0u64;

        // Only `substreams` directory entries are populated; the remaining
        // fixed-array slots are default zeros, and pricing them would wrap the
        // cumulative end-pointer difference.
        for (i, directory) in au.substream_directory.iter().enumerate().take(substreams) {
            let end = directory.substream_end_ptr as u64;
            let words: u64 = if directory.extra_substream_word {
                32
            } else {
                16
            };
            region[i] = words + 16 * end.wrapping_sub(previous_end);
            previous_end = end;
        }

        let info = state.substream_info;
        let mut bits = [0u64; ACCUMULATORS];

        bits[0] = base + region[0] + extra_bits;

        // The 6-channel decoder reads substreams 0 and 1, and counts nothing at all
        // unless substream_info says a second substream carries the 6-channel mix.
        if info & 0x08 != 0 {
            bits[1] = base + region[0] + region[1] + extra_bits;
        }

        if is_fba {
            // The 8-channel decoder gates every region on its own substream_info bit.
            // FBB skips this accumulator entirely.
            let mut sum = base;

            for (bit, r) in [(0x10, 0), (0x20, 1), (0x40, 2)] {
                if info & bit != 0 {
                    sum += region[r];
                }
            }

            bits[2] = sum + extra_bits;

            // The 16-channel decoder reads the top region down to the one selected by
            // extended_substream_info.
            if info & 0x80 != 0 && substreams >= 4 {
                let lowest = 3 - (state.extended_substream_info & 3) as usize;
                bits[3] = base + region[lowest..=3].iter().sum::<u64>() + extra_bits;
            }
        }

        let mut contribution = [0usize; ACCUMULATORS];

        for (k, item) in bits.iter().enumerate() {
            contribution[k] = (item / 8) as usize;
        }

        contribution[4] = state.access_unit_length << 1;

        // Time is priced with a synthetic output clock: the unwrapped output timing of
        // the first access unit, advanced one access unit per access unit ever after and
        // never re-synchronised, not even across a seamless branch, which adjusts the
        // input clock instead. A record drains once the input clock passes its output
        // time by strictly more than one access unit.
        let samples_per_au = state.samples_per_au;
        let output_clock = match state.fifo_output_clock {
            Some(clock) => clock + samples_per_au,
            None => state.first_output_timing,
        };
        state.fifo_output_clock = Some(output_clock);

        let report = state.fifo_depth.push(
            state.unwrapped_input_timing,
            output_clock + samples_per_au,
            contribution,
        );

        trace!(
            "AU {}: fifo depth {:?} over {} access units",
            state.au_counter,
            report.depths,
            state.fifo_depth.buffered()
        );

        if let Some(index) = report.underrun {
            log_or_err!(state, Warn, anyhow!(FifoError::Underrun { index }));
        }

        for (k, accumulator) in Accumulator::ALL.iter().enumerate() {
            let cap = if is_fba {
                // The 16-channel cap only applies when the stream has a 16-channel
                // presentation and a fourth substream to carry it
                if *accumulator == Accumulator::Sixteench && (info & 0x80 == 0 || substreams <= 3) {
                    continue;
                }

                accumulator.fba_cap()
            } else {
                match accumulator.fbb_cap(info) {
                    Some(cap) => cap,
                    None => continue,
                }
            };

            let depth = report.depths[k];

            if depth <= cap {
                continue;
            }

            let error = match accumulator {
                Accumulator::Substream0 => FifoError::Substream0DepthExceeded { depth, cap },
                Accumulator::WholeStream => FifoError::WholeStreamDepthExceeded { depth, cap },
                _ => FifoError::GroupDepthExceeded {
                    group: accumulator.group(),
                    depth,
                    cap,
                },
            };

            log_or_err!(state, Warn, anyhow!(error));
        }

        Ok(())
    }
}
