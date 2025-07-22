//! Restart header structures and decoder initialization.
//!
//! Restart headers provide decoder initialization and recovery points within TrueHD streams.
//!
//! ## Restart Sync Words
//!
//! - **0x31EA**: Substream 0 and substream 1
//! - **0x31EB**: Substream 1 and substream 2
//! - **0x31EC**: Substream 3 (object-based audio)
//!
//! ## Parameters
//!
//! Contains channel configuration, timing management, dithering parameters,
//! and channel permutation mapping.

use crate::log_or_err;
use crate::process::decode::DecoderState;
use crate::process::parse::ParserState;
use crate::structs::sync::{
    BASE_SAMPLING_RATE_CD, MAJOR_SYNC_FBA, MAJOR_SYNC_FBB, UNIMPLEMENTED_FBB_MSG,
};
use crate::utils::bitstream_io::BsIoSliceReader;
use crate::utils::errors::RestartHeaderError;
use anyhow::{Result, anyhow, bail};
use log::Level::Warn;
use log::{info, trace, warn};

/// Restart synchronization words identifying substream types.
///
/// 16-bit restart sync word at the beginning of restart headers
/// determining substream type and block organization.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u16)]
pub enum RestartSyncWord {
    #[default]
    None,
    A = 0x31EA,
    B,
    C,
}

impl RestartSyncWord {
    pub fn read(reader: &mut BsIoSliceReader) -> Result<Self> {
        let value = reader.get_n::<u16>(14)?;
        Ok(RestartSyncWord::from(value))
    }
}

impl From<u16> for RestartSyncWord {
    fn from(value: u16) -> Self {
        match value {
            0x31EA => RestartSyncWord::A,
            0x31EB => RestartSyncWord::B,
            0x31EC => RestartSyncWord::C,
            _ => panic!("restart_sync_word must be 0x31EA, 0x31EB or 0x31EC. Read {value:#04X}"),
        }
    }
}
impl From<RestartSyncWord> for u16 {
    fn from(value: RestartSyncWord) -> Self {
        match value {
            RestartSyncWord::A => 0x31EA,
            RestartSyncWord::B => 0x31EB,
            RestartSyncWord::C => 0x31EC,
            RestartSyncWord::None => 0,
        }
    }
}

/// Complete restart header for decoder initialization.
///
/// Provides decoder state initialization at sync points.
/// Protected by 8-bit CRC.
#[derive(Clone, Debug, Default)]
pub struct RestartHeader {
    pub restart_sync_word: RestartSyncWord,
    pub output_timing: u16,
    pub min_chan: u8,
    pub max_chan: u8,
    pub max_matrix_chan: u8,
    pub dither_shift: u8,
    pub dither_seed: u32,
    pub max_shift: u8,
    pub max_lsbs: u8,
    pub max_bits: u8,
    pub max_bits_repeat: u8,
    pub error_protect: bool,
    pub lossless_check: u8,

    pub hires_output_timing: bool,
    pub heavy_drc_present: bool,
    pub heavy_drc_gain_update: i16,
    pub heavy_drc_time_update: u8,

    pub ch_assign: [usize; 16],

    pub restart_header_crc: u8,
}

impl RestartHeader {
    pub fn read(state: &mut ParserState, reader: &mut BsIoSliceReader) -> Result<Self> {
        let start_pos = reader.position()?;

        let mut rh = Self {
            restart_sync_word: RestartSyncWord::read(reader)?,
            output_timing: reader.get_n(16)?,
            min_chan: reader.get_n(4)?,
            max_chan: reader.get_n(4)?,
            max_matrix_chan: reader.get_n(4)?,
            dither_shift: reader.get_n(4)?,
            dither_seed: reader.get_n(23)?,
            max_shift: reader.get_n(4)?,
            max_lsbs: reader.get_n(5)?,
            max_bits: reader.get_n(5)?,
            max_bits_repeat: reader.get_n(5)?,
            error_protect: reader.get()?,
            lossless_check: reader.get_n(8)?,
            ..Default::default()
        };

        'check_output_timing: {
            if state.has_parsed_substream
                && state.output_timing & 0xFFFF != rh.output_timing as usize
            {
                log_or_err!(
                    state,
                    Warn,
                    anyhow!(RestartHeaderError::OutputTimingMismatch {
                        read: rh.output_timing,
                        substream: state.substream_index,
                        expected: state.output_timing
                    })
                );
            }

            // TODO: check all substreams
            if state.has_parsed_substream {
                break 'check_output_timing;
            }

            state.output_timing = rh.output_timing as usize;

            if !state.has_parsed_au {
                state.first_output_timing = state.output_timing;

                if state.output_timing < state.input_timing {
                    state.output_timing += 0x10000;
                }

                trace!(
                    "AU {}: first output_timing adjusted to {}",
                    state.au_counter, state.output_timing
                );
            } else {
                let history_index = state.substream_state()?.history_index.wrapping_sub(1) & 0x7F;

                let samples_per_au = state.samples_per_au;

                state.advance = state
                    .output_timing
                    .wrapping_sub(samples_per_au)
                    .wrapping_sub(state.input_timing)
                    & 0xFFFF;

                let expected_output_timing = state.output_timing_deviation
                    + samples_per_au
                    + state.substream_state()?.output_timing_history[history_index];

                if expected_output_timing & 0xFFFF == state.output_timing {
                    break 'check_output_timing;
                }

                if state.allow_seamless_branch {
                    if state.has_jump {
                        log_or_err!(
                            state,
                            Warn,
                            anyhow!(RestartHeaderError::OutputTimingAfterJump {
                                read: state.output_timing,
                                expected: expected_output_timing
                            })
                        );
                    }
                    state.output_timing_jump = true;
                    trace!(
                        "Output timing jump: read={}, expected={}",
                        state.output_timing, expected_output_timing
                    );
                } else {
                    log_or_err!(
                        state,
                        Warn,
                        anyhow!(RestartHeaderError::InvalidOutputTiming {
                            read: state.output_timing,
                            expected: expected_output_timing
                        })
                    );
                }

                if state.has_branch || state.input_timing_jump || state.output_timing_jump {
                    let samples_per_au = state.samples_per_au;
                    let prev_advance = state.prev_advance;
                    let advance = state.advance;
                    let prev_access_unit_length = state.prev_access_unit_length;
                    let prev_fifo_duration = state.prev_fifo_duration;

                    let input_timing_interval = samples_per_au
                        .wrapping_add(prev_advance)
                        .wrapping_sub(advance)
                        & 0xFFFF;

                    let data_rate = (state.audio_sampling_frequency_1 as usize
                        * (prev_access_unit_length << 4))
                        .div_ceil(input_timing_interval);

                    if data_rate > state.max_data_rate {
                        state.max_data_rate = data_rate;
                        state.max_data_rate_au_index = state.au_counter - 1;
                    }

                    let samples_per_au_3q4 = 3 * (samples_per_au >> 2);
                    let samples_per_75ms =
                        (state.audio_sampling_frequency_1 as usize * 3).div_ceil(40);

                    let c1 = advance <= prev_advance + samples_per_au_3q4;
                    let c2 = advance <= prev_advance + samples_per_au - prev_fifo_duration;
                    let c3 = advance <= samples_per_75ms - samples_per_au;
                    let c4 = prev_access_unit_length << 8 <= state.prev_peak_data_rate;

                    if c1 && c2 && c3 && c4 {
                        state.has_jump = true;
                        state.reset_for_branch();

                        state.output_timing_deviation = state
                            .output_timing
                            .wrapping_sub(state.first_output_timing)
                            .wrapping_sub(state.au_counter * samples_per_au)
                            & 0xFFFF;

                        info!(
                            "Valid seamless branch. Latency in access unit before branch is {} samples, \
                        latency at branch is {} samples",
                            state.substream_state()?.prev_latency,
                            state.output_timing.wrapping_sub(state.input_timing) & 0xFFFF,
                        );

                        break 'check_output_timing;
                    }

                    if c1 {
                        warn!(
                            "advance[n]>advance[n-1]+3*samples_per_au/4, \
                            ({advance} > {prev_advance} + {})",
                            3 * (samples_per_au >> 2)
                        );
                    }

                    if c2 {
                        warn!(
                            "advance[n]>advance[n-1]+samples_per_au-duration[n-1], \
                            ({advance} > {prev_advance} + {samples_per_au} - {prev_fifo_duration})"
                        );
                    }

                    if c3 {
                        warn!(
                            "advance[n]>samples_per_75ms-samples_per_au, \
                            ({advance} > {samples_per_75ms} - {samples_per_au})"
                        );
                    }

                    if c4 {
                        warn!("data_rate exceeds peak_data_rate after adjusting timing for jump");
                    }

                    log_or_err!(
                        state,
                        Warn,
                        anyhow!(RestartHeaderError::InvalidSeamlessBranch)
                    );
                }
            }
        }

        match rh.restart_sync_word {
            RestartSyncWord::A => {
                if state.substream_index == 1 && state.substream_info & 8 == 0 {
                    bail!(RestartHeaderError::InvalidSyncBForSubstream1)
                }
            }
            RestartSyncWord::B => {
                if state.substream_index == 0 {
                    bail!(RestartHeaderError::InvalidSyncBForSubstream0)
                }
            }
            rsw @ RestartSyncWord::C => {
                if state.substream_index != 3 {
                    bail!(RestartHeaderError::InvalidSyncC(rsw as u16))
                }
            }
            _ => {}
        }

        if rh.max_bits != rh.max_bits_repeat {
            bail!(RestartHeaderError::MaxBitsMismatch {
                first: rh.max_bits,
                second: rh.max_bits_repeat
            })
        }

        rh.hires_output_timing = reader.get()?;

        if !state.has_parsed_substream {
            trace!(
                "AU {}: high-resolution output timing field = {}",
                state.au_counter, rh.hires_output_timing
            );
            let mut hires_output_timing_state = state.substream_state()?.hires_output_timing_state;
            hires_output_timing_state.update(state, rh.hires_output_timing)?;
            state.substream_state_mut()?.hires_output_timing_state = hires_output_timing_state;
        }

        reader.skip_n(2)?;

        if state.flags & 0x2000 != 0 {
            //TODO: implement
            rh.heavy_drc_present = reader.get()?;

            if state.format_sync == MAJOR_SYNC_FBA {
                let ss_state = state.substream_state_mut()?;
                ss_state.heavy_drc_count += 1;

                if ss_state.heavy_drc_active
                    && (1 << ss_state.heavy_drc_time_update) < ss_state.heavy_drc_count
                {
                    warn!(
                        "heavy_drc_time_update={}, but heavy_drc_count={}",
                        ss_state.heavy_drc_time_update, ss_state.heavy_drc_count
                    )
                }
            }
        } else {
            reader.skip_n(1)?;
        }

        // prev
        if state.substream_state_mut()?.heavy_drc_present {
            if state.format_sync == MAJOR_SYNC_FBB {
                unimplemented!("{}", UNIMPLEMENTED_FBB_MSG)
            } else {
                let ss_state = state.substream_state_mut()?;
                ss_state.heavy_drc_active = true;
                ss_state.heavy_drc_count = 0;

                rh.heavy_drc_gain_update = reader.get_s(9)?;
                rh.heavy_drc_time_update = reader.get_n(3)?;
            }
        } else {
            reader.skip_n(12)?;
        }

        // TODO: as context?
        let mut permutation: u16 = 0;

        for i in 0..=rh.max_matrix_chan as usize {
            let ch_assign = reader.get_n::<u8>(6)?;

            if state.format_sync == MAJOR_SYNC_FBA {
                if ch_assign > rh.max_matrix_chan {
                    bail!(RestartHeaderError::ChannelAssignTooHigh {
                        index: i,
                        value: ch_assign,
                        max: rh.max_matrix_chan
                    })
                } else if state.substream_index == 0
                    && i != ch_assign as usize
                    && state.audio_sampling_frequency_1 >= BASE_SAMPLING_RATE_CD << 2
                {
                    bail!(RestartHeaderError::ChannelAssignMisordered {
                        index: i,
                        value: ch_assign,
                    })
                }
            } else {
                unimplemented!("{}", UNIMPLEMENTED_FBB_MSG)
            }

            let permutation_bit = 1 << ch_assign;

            if permutation_bit & permutation != 0 {
                bail!(RestartHeaderError::ChannelAssignDuplicate(
                    rh.max_matrix_chan
                ))
            }

            permutation |= permutation_bit;

            rh.ch_assign[i] = ch_assign as usize;
        }

        let len = reader.position()? - start_pos;

        rh.restart_header_crc = reader.get_n(8)?;

        let crc = reader.crc8_check(&state.crc_restart_block_header, start_pos, len)?;

        if crc != rh.restart_header_crc {
            bail!(RestartHeaderError::RestartHeaderCrcMismatch {
                calculated: crc,
                read: rh.restart_header_crc
            });
        }

        state.reset_parser_substream_state();
        let ss_state = state.substream_state_mut()?;

        ss_state.restart_sync_word = rh.restart_sync_word as u16;
        ss_state.min_chan = rh.min_chan as usize;
        ss_state.max_chan = rh.max_chan as usize;
        ss_state.max_matrix_chan = rh.max_matrix_chan as usize;
        ss_state.max_shift = rh.max_shift as i8;
        ss_state.max_lsbs = rh.max_lsbs as u32;
        ss_state.error_protect = rh.error_protect;
        ss_state.heavy_drc_present = rh.heavy_drc_present;

        Ok(rh)
    }

    pub fn update_decoder_state(&self, state: &mut DecoderState) -> Result<()> {
        if state.valid && state.substream_index == state.presentation {
            let substream_info = state.substream_info;
            if match state.substream_index {
                0 => true,
                1 => substream_info & 8 != 0 || substream_info & 0x60 == 0x20,
                2 => substream_info & 0x40 != 0,
                3 => substream_info >> 7 != 0,
                _ => bail!(RestartHeaderError::InvalidStream),
            } {
                let mut lossless_check_i32 = state.substream_state()?.lossless_check_i32;
                lossless_check_i32 ^= lossless_check_i32 >> 16;
                lossless_check_i32 ^= lossless_check_i32 >> 8;
                lossless_check_i32 &= 0xFF;

                if lossless_check_i32 != self.lossless_check as i32 {
                    log_or_err!(
                        state,
                        Warn,
                        anyhow!(RestartHeaderError::LosslessCheckMismatch {
                            substream: state.substream_index,
                            calculated: lossless_check_i32,
                            read: self.lossless_check
                        })
                    )
                }
            }
        }

        state.reset_decoder_substream_state();

        let ss_state = state.substream_state_mut()?;

        ss_state.restart_sync_word = self.restart_sync_word as u16;
        ss_state.min_chan = self.min_chan as usize;
        ss_state.max_chan = self.max_chan as usize;
        ss_state.max_matrix_chan = self.max_matrix_chan as usize;
        ss_state.dither_shift = self.dither_shift as u32;
        ss_state.dither_seed = self.dither_seed;
        ss_state.ch_assign = self.ch_assign;

        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Guards(u8);

impl Default for Guards {
    fn default() -> Self {
        Self(0xFF)
    }
}

impl Guards {
    pub fn read(reader: &mut BsIoSliceReader) -> Result<Self> {
        let guards = reader.get_n(8)?;
        Ok(Self(guards))
    }
}

#[repr(u8)]
pub enum GuardsField {
    Guards,
    HuffOffset,
    CoeffsB,
    CoeffsA,
    QuantiserStepSize,
    OutputShift,
    Matrixing,
    BlockSize,
}

impl Guards {
    pub fn need_change(&self, field: GuardsField) -> bool {
        self.0 & (1 << field as u8) != 0
    }
}
