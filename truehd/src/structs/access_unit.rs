use anyhow::{Result, anyhow, bail};
use log::Level::{Error, Warn};
use log::trace;

use crate::log_or_err;
use crate::process::decode::DecoderState;
use crate::process::parse::ParserState;
use crate::process::{MAX_PRESENTATIONS, PresentationMap};
use crate::structs::channel::ChannelLabel;
use crate::structs::extra_data::ExtraData;
use crate::structs::restart_header::RestartHeader;
use crate::structs::substream::{SubstreamDirectory, SubstreamSegment};
use crate::structs::sync::{MAJOR_SYNC_FBA, MAJOR_SYNC_FBB, MajorSyncInfo};
use crate::utils::bitstream_io::BsIoSliceReader;
use crate::utils::errors::{AccessUnitError, FifoError};
use crate::utils::fifo::{ACCUMULATORS, Accumulator, FifoContribution, SUBSTREAMS};
use crate::utils::perf::Timer;

/// The one FBB `fbb_channel_assignment` whose decoded channel order has been measured.
const MEASURED_ARRANGEMENT_SURROUND_FIRST: u8 = 20;

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

            if state.has_parsed_au {
                Self::check_restart_gap(state, reader)?;
            }

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
        state.au_output_timing = None;

        let segments = Timer::start();
        for i in 0..substreams {
            state.substream_index = i;

            if state.substream_mask >> i & 1 == 0 {
                RestartHeader::peek_output_timing(state, reader)?;

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

        Self::check_hires_output_timing_matches(state, substreams, reader)?;

        // One whole 16-bit word is enough. `extra_data` is entered whenever the
        // access unit has a word left, so a lone trailing word is a block like any other:
        // its header nibble is checked, and a non-zero header that declares a length it has
        // no room for is an error. Gating on *more* than a word left skipped it entirely.
        if state.expected_au_end_pos() >= reader.position()? as usize + 16 {
            let timer = Timer::start();
            let extra_data = ExtraData::read(state, reader)?;
            timer.record(&mut state.perf.extra_data);
            au.extra_data = Some(extra_data);
        }

        state.has_parsed_au = true;
        state.segment_start = false;
        access_unit.record(&mut state.perf.access_unit_total);

        let au_end_pos = reader.position()?;

        if au_end_pos <= state.expected_au_end_pos() as u64 {
            state.total_access_unit_length += au.access_unit_length as usize;
            state.max_access_unit_size = state
                .max_access_unit_size
                .max((au.access_unit_length as usize) << 1);
        } else {
            log_or_err!(
                state,
                Error,
                anyhow!(AccessUnitError::AccessUnitTooLong(
                    au_end_pos as usize,
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

    /// Every substream must carry the same `hires_output_timing` bit.
    ///
    /// FBA only, and only at a major sync: elsewhere a substream may still hold the bit
    /// from its last restart header. A substream the presentation mask skipped is left out
    /// for the same reason, since nothing read a bit for it here.
    fn check_hires_output_timing_matches(
        state: &mut ParserState,
        substreams: usize,
        reader: &mut BsIoSliceReader,
    ) -> Result<()> {
        if state.format_sync != MAJOR_SYNC_FBA || !state.is_major_sync {
            return Ok(());
        }

        let mut read = [(0usize, false); MAX_PRESENTATIONS];
        let mut count = 0;
        for i in 0..substreams {
            if state.substream_mask >> i & 1 == 1 {
                read[count] = (i, state.substream_i_state(i)?.hires_output_timing);
                count += 1;
            }
        }

        for first in 0..count.saturating_sub(1) {
            for second in first + 1..count {
                if read[first].1 != read[second].1 {
                    let (first, second) = (read[first].0, read[second].0);
                    log_or_err!(
                        state,
                        Warn,
                        anyhow!(AccessUnitError::HiresOutputTimingMismatch { first, second }),
                        reader
                    );
                }
            }
        }

        Ok(())
    }

    /// Channels of a presentation, in the order the decoder outputs them.
    ///
    /// `None` where the syntax does not say, so that a caller describing the audio
    /// leaves the order unstated rather than assuming one.
    pub fn get_channel_labels(&self, presentation_index: usize) -> Option<Vec<ChannelLabel>> {
        let major_sync_info = self.major_sync_info.as_ref()?;

        if major_sync_info.format_sync == MAJOR_SYNC_FBB {
            return self.fbb_channel_labels(major_sync_info, presentation_index);
        }

        match presentation_index {
            0 => {
                if self.presentation_channel_count(0)? == 1 {
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
                let ext_meaning = major_sync_info.channel_meaning.extra_channel_meaning()?;

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

    /// Channels of an FBB presentation, where this decoder's output has been measured.
    ///
    /// FBB names its whole channel arrangement with one `fbb_channel_assignment` index
    /// instead of describing the channels, and what an index stands for is not
    /// recoverable from the bitstream. Only what has been measured against this
    /// decoder's own output is reported, so that an unmeasured arrangement leaves the
    /// order unstated rather than assumed.
    fn fbb_channel_labels(
        &self,
        major_sync_info: &MajorSyncInfo,
        presentation_index: usize,
    ) -> Option<Vec<ChannelLabel>> {
        use ChannelLabel::*;

        let channels = self.presentation_channel_count(presentation_index)?;

        // Measured channel for channel against this decoder's own output for a
        // six-channel DVD-Audio stream carrying arrangement 20: its surround pair comes
        // out before the centre and the LFE, not in the conventional 5.1 order. No
        // other arrangement has been measured, including the other six-channel ones,
        // which this deliberately does not cover.
        if major_sync_info.format_info.fbb_channel_assignment == MEASURED_ARRANGEMENT_SURROUND_FIRST
            && channels == 6
        {
            return Some(vec![L, R, Ls, Rs, C, LFE]);
        }

        // Whatever the arrangement, its first substream is a plain mono or stereo
        // presentation when that is all it decodes.
        match (presentation_index, channels) {
            (0, 1) => Some(vec![C]),
            (0, 2) => Some(vec![L, R]),
            _ => None,
        }
    }

    /// Channels a presentation decodes, as its restart header states.
    fn presentation_channel_count(&self, presentation_index: usize) -> Option<usize> {
        Some(
            self.substream_segment
                .as_ref()
                .get(presentation_index)?
                .block
                .first()?
                .restart_header
                .as_ref()?
                .max_matrix_chan as usize
                + 1,
        )
    }

    /// Access units between this major sync and the previous one.
    ///
    /// Only the bound on a single gap is checked. There are also rules over
    /// runs of short gaps, and a relaxation of them for a spliced stream, but their
    /// triggering conditions are not established, so they are not implemented here; the
    /// history is kept so that they can be.
    fn check_restart_gap(state: &mut ParserState, reader: &mut BsIoSliceReader) -> Result<()> {
        let gap = state.au_counter - state.last_major_sync_index;

        state.restart_gap.rotate_right(1);
        state.restart_gap[0] = gap;

        trace!(
            "AU {}: restart_gap {gap}, after {:?}",
            state.au_counter,
            &state.restart_gap[1..]
        );

        if gap != 1 && gap < 8 {
            log_or_err!(
                state,
                Warn,
                anyhow!(AccessUnitError::RestartGapInvalid(gap)),
                reader
            );
        }

        Ok(())
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

        let rate = state.peak_data_rate * state.audio_sampling_frequency_1 as usize;

        if rate > max_data_rate {
            log_or_err!(
                state,
                Warn,
                anyhow!(AccessUnitError::PeakDataRateTooHigh {
                    rate,
                    max: max_data_rate
                })
            );
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
                    224 + match ms.channel_meaning.extra_channel_meaning() {
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
        // plus its payload, priced as the difference of cumulative end pointers. The
        // payload alone is the stream's own bytes; the directory word is overhead.
        let mut region = [0u64; MAX_PRESENTATIONS];
        let mut payload = [0u64; MAX_PRESENTATIONS];
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
            payload[i] = 16 * end.wrapping_sub(previous_end);
            region[i] = words + payload[i];
            previous_end = end;
        }

        let info = state.substream_info;
        let map = PresentationMap::for_format_sync(
            state.format_sync,
            info,
            state.extended_substream_info,
        );
        let mut bits = [0u64; ACCUMULATORS];
        let mut stream_bits = [0u64; ACCUMULATORS];

        // A decoder buffers the substreams its own presentation is made of, which is the
        // mask the decode path resolves. Deriving every sum from the mask keeps the
        // 6-channel one in step with the others: summing substreams 0 and 1 for it however
        // the stream is laid out overstates a presentation that one independent substream
        // carries whole.
        for (k, mask) in (0..MAX_PRESENTATIONS)
            .map(|k| (k, map.substream_mask_by_index(k)))
            .filter(|&(_, mask)| mask != 0)
        {
            // FBB has no 8- or 16-channel decoder, and a 16-channel presentation needs a
            // fourth substream to live in.
            if (!is_fba && k >= 2) || (k == 3 && substreams < 4) {
                continue;
            }

            for r in (0..MAX_PRESENTATIONS).filter(|r| mask >> r & 1 != 0) {
                bits[k] += region[r];
                stream_bits[k] += payload[r];
            }

            bits[k] += base + extra_bits;
        }

        let mut contribution = FifoContribution::default();

        for k in 0..ACCUMULATORS {
            contribution.total[k] = (bits[k] / 8) as usize;
            contribution.stream[k] = (stream_bits[k] / 8) as usize;
        }

        // The whole-stream row is priced from the access unit length rather than summed,
        // so its overhead is whatever the length holds beyond the substream segments.
        contribution.total[4] = state.access_unit_length << 1;
        contribution.stream[4] = (previous_end << 1) as usize;

        for (own, bits) in contribution
            .substream
            .iter_mut()
            .zip(payload)
            .take(substreams.min(SUBSTREAMS))
        {
            *own = (bits / 8) as usize;
        }

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
