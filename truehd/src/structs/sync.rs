//! Sync patterns and format information structures.
//!
//! ## Sync Patterns
//!
//! **Major Sync** (0xF8726FBA): Stream configuration and decoder initialization.
//! **Minor Sync**: Access unit timing and length information.
//!
//! ## Format Types
//!
//! - **FBA Format** (0xF8726FBA): Dolby TrueHD format
//! - **FBB Format** (0xF8726FBB): Meridian format (not implemented)
//!
//! ## Sample Counts
//!
//! Access units contain 40-160 samples based on sampling frequency.

use anyhow::{Result, anyhow, bail};
use log::Level::{Error, Warn};
use log::{debug, trace, warn};

use crate::log_or_err;
use crate::process::PresentationMap;
use crate::process::decode::DecoderState;
use crate::process::parse::ParserState;
use crate::structs::channel::ChannelMeaning;
use crate::utils::bitstream_io::BsIoSliceReader;
use crate::utils::errors::SyncError;

/// Major sync pattern for FBA (Dolby) format streams.
///
/// 32-bit sync word (0xF8726FBA) identifying Dolby TrueHD streams.
pub const MAJOR_SYNC_FBA: u32 = 0xF8_72_6F_BA;

/// Major sync pattern for FBB (Meridian) format streams.
///
/// 32-bit sync word (0xF8726FBB) identifying Meridian MLP streams.
pub const MAJOR_SYNC_FBB: u32 = 0xF8_72_6F_BB;

pub const UNIMPLEMENTED_FBB_MSG: &str = "FBB format is not implemented yet";

/// Base sampling rate for CD-family rates (44.1kHz, 88.2kHz, 176.4kHz).
pub const BASE_SAMPLING_RATE_CD: u32 = 44100;

/// Base sampling rate for DVD-family rates (48kHz, 96kHz, 192kHz).
pub const BASE_SAMPLING_RATE_DVD: u32 = 48000;

/// Base number of samples per access unit at 48kHz.
pub const BASE_SAMPLES_PER_AU: usize = 40;

/// Format information from major sync frames.
///
/// Stream configuration parsed from 32-bit format_info field containing
/// sampling frequency and channel configuration parameters.
#[derive(Debug, Clone, Default)]
pub struct FormatInfo {
    pub _quantization_word_length_1: u8,
    pub _quantization_word_length_2: u8,
    pub audio_sampling_frequency_1: u8,
    pub _audio_sampling_frequency_2: u8,
    pub multi_channel_type: u8,
    pub fbb_channel_assignment: u8,

    pub sixch_multi_channel_type: bool,
    pub eightch_multi_channel_type: bool,
    pub twoch_decoder_channel_modifier: u8,
    pub sixch_decoder_channel_modifier: u8,
    pub sixch_decoder_channel_assignment: u8,
    pub eightch_decoder_channel_modifier: u8,
    pub eightch_decoder_channel_assignment: u16,
}

impl FormatInfo {
    fn read(state: &mut ParserState, reader: &mut BsIoSliceReader) -> Result<Self> {
        let fi = match state.format_sync {
            MAJOR_SYNC_FBA => Self::read_fba(reader)?,
            MAJOR_SYNC_FBB => Self::read_fbb(reader)?,
            sync => bail!(SyncError::InvalidFormatSync(sync)),
        };

        state.is_major_sync = true;

        state.audio_sampling_frequency_1 = fi.sampling_frequency_1()?;
        state.samples_per_au = fi.samples_per_au()?;

        Ok(fi)
    }

    // pub fn quantization_word_length_1(&self) -> Result<u8> {
    //     Self::map_quantization(self.quantization_word_length_1, 1)
    // }

    pub fn sampling_frequency_1(&self) -> Result<u32> {
        Self::map_sampling_freq(self.audio_sampling_frequency_1, 1)
    }

    pub fn samples_per_au(&self) -> Result<usize> {
        let freq = self.sampling_frequency_1()?;
        Ok((freq / BASE_SAMPLING_RATE_CD) as usize * BASE_SAMPLES_PER_AU)
    }

    pub fn update_decoder_state(&self, state: &mut DecoderState) -> Result<()> {
        state.sampling_frequency = self.sampling_frequency_1()?;
        state.samples_per_au = self.samples_per_au()?;

        Ok(())
    }

    fn read_fba(reader: &mut BsIoSliceReader) -> Result<Self> {
        let mut fi = Self {
            _quantization_word_length_1: 2,
            audio_sampling_frequency_1: reader.get_n(4)?,
            sixch_multi_channel_type: reader.get()?,
            eightch_multi_channel_type: reader.get()?,
            ..Default::default()
        };

        reader.skip_n(2)?;
        fi.twoch_decoder_channel_modifier = reader.get_n(2)?;
        fi.sixch_decoder_channel_modifier = reader.get_n(2)?;
        fi.sixch_decoder_channel_assignment = reader.get_n(5)?;
        fi.eightch_decoder_channel_modifier = reader.get_n(2)?;
        fi.eightch_decoder_channel_assignment = reader.get_n(13)?;

        Ok(fi)
    }

    fn read_fbb(reader: &mut BsIoSliceReader) -> Result<Self> {
        let mut fi = Self {
            _quantization_word_length_1: reader.get_n(4)?,
            _quantization_word_length_2: reader.get_n(4)?,
            audio_sampling_frequency_1: reader.get_n(4)?,
            _audio_sampling_frequency_2: reader.get_n(4)?,
            ..Default::default()
        };

        reader.skip_n(4)?;
        fi.multi_channel_type = reader.get_n(4)?;
        reader.skip_n(3)?;
        fi.fbb_channel_assignment = reader.get_n(5)?;

        // state.quantization_word_length_2 =
        //     Self::map_quantization(fi.quantization_word_length_2, 2)?;
        // state.audio_sampling_frequency_2 =
        //     Self::map_sampling_freq(fi.audio_sampling_frequency_2, 2)?;

        Ok(fi)
    }

    // fn map_quantization(value: u8, index: u8) -> Result<u8> {
    //     match value {
    //         0..=2 => Ok(16 + (value << 2)),
    //         _ => bail!(
    //             "Invalid format_info: quantization_word_length_{}. Read {:#01X}",
    //             index,
    //             value
    //         ),
    //     }
    // }

    fn map_sampling_freq(value: u8, index: u8) -> Result<u32> {
        match value {
            0..=2 => Ok(BASE_SAMPLING_RATE_DVD << value),
            8..=10 => Ok(BASE_SAMPLING_RATE_CD << (value - 8)),
            _ => bail!(SyncError::InvalidAudioSamplingFreq { index, value }),
        }
    }
}

/// Complete major sync information structure.
///
/// Contains stream configuration and decoder initialization parameters.
/// Protected by 16-bit CRC.
#[derive(Debug, Clone, Default)]
pub struct MajorSyncInfo {
    pub format_sync: u32,
    pub format_info: FormatInfo,
    pub signature: u16,
    pub flags: u16,
    pub reserved: u16,
    pub variable_rate: bool,
    pub peak_data_rate: u16,
    pub substreams: usize,
    pub extended_substream_info: u8,
    pub substream_info: u8,
    pub channel_meaning: ChannelMeaning,
    pub major_sync_info_crc: u16,
}

impl MajorSyncInfo {
    pub fn read(state: &mut ParserState, reader: &mut BsIoSliceReader) -> Result<Self> {
        let start_pos = reader.position()?;

        let mut ms = Self {
            format_sync: reader.get_n(32)?,
            ..Default::default()
        };

        state.format_sync = ms.format_sync;

        {
            // TODO: restart_gap
        }

        ms.format_info = FormatInfo::read(state, reader)?;
        ms.signature = reader.get_n(16)?;

        if ms.signature != 0xB752 {
            log_or_err!(
                state,
                Warn,
                anyhow!(SyncError::InvalidMajorSyncSignature(ms.signature))
            )
        }

        ms.flags = reader.get_n(16)?;

        // check with bit-14
        if ms.flags & 0x67FF != 0 {
            log_or_err!(
                state,
                Warn,
                anyhow!(SyncError::ReservedFlagsNonZero(ms.flags))
            )
        }

        if state.has_parsed_au && state.flags != ms.flags {
            log_or_err!(
                state,
                Warn,
                anyhow!(SyncError::FlagsMismatch {
                    read: ms.flags,
                    expected: state.flags
                })
            );
        }

        state.flags = ms.flags;

        ms.reserved = reader.get_n(16)?;

        ms.variable_rate = reader.get()?;
        ms.peak_data_rate = reader.get_n(15)?;
        ms.substreams = reader.get_n::<u8>(4)? as usize;

        // peak data rate check
        if state.check_fifo
            && state.has_parsed_au
            && state.peak_data_rate != ms.peak_data_rate as usize
        {
            if state.allow_seamless_branch {
                trace!(
                    "Peak data rate change allowed at branch: {} -> {}",
                    state.peak_data_rate, ms.peak_data_rate
                );
                state.has_branch = true;
            } else {
                log_or_err!(
                    state,
                    Warn,
                    anyhow!(SyncError::PeakDataRateMismatch {
                        read: ms.peak_data_rate,
                        expected: state.peak_data_rate,
                    })
                )
            }
        }

        state.variable_rate = ms.variable_rate;
        state.peak_data_rate = ms.peak_data_rate as usize;

        if let Some(substreams) = state.substreams {
            if substreams != ms.substreams {
                log_or_err!(
                    state,
                    Warn,
                    anyhow!(SyncError::SubstreamCountMismatch {
                        read: ms.substreams,
                        expected: substreams,
                    })
                )
            }
        } else {
            state.substreams = Some(ms.substreams);
        }

        // reserved(2) field is part of extended_substream_info
        ms.extended_substream_info = reader.get_n(4)?;
        ms.substream_info = reader.get_n(8)?;

        'check_substream_info: {
            if ms.extended_substream_info >> 2 != 0 {
                log_or_err!(
                    state,
                    log::Level::Debug,
                    anyhow!(SyncError::ReservedExtendedSubstreamInfo(
                        ms.extended_substream_info >> 2
                    ))
                );
            }

            if ms.substream_info & 3 != 0 {
                log_or_err!(
                    state,
                    log::Level::Debug,
                    anyhow!(SyncError::ReservedSubstreamInfo(ms.substream_info))
                );
            }

            if state.has_parsed_au {
                if ms.substream_info != state.substream_info {
                    log_or_err!(
                        state,
                        Error,
                        anyhow!(SyncError::SubstreamInfoMismatch {
                            read: ms.substream_info,
                            expected: state.substream_info
                        })
                    )
                }

                if ms.extended_substream_info != state.extended_substream_info {
                    log_or_err!(
                        state,
                        Error,
                        anyhow!(SyncError::ExtendedSubstreamInfoMismatch {
                            read: ms.extended_substream_info,
                            expected: state.extended_substream_info
                        })
                    )
                }

                break 'check_substream_info;
            }

            let extended_substream_info = ms.extended_substream_info & 3;
            let substream_info = ms.substream_info & 0x7C;

            if substream_info <= 76
                && (76562297473007889u64 >> substream_info.wrapping_sub(20)) & 1 != 0
                || (68987981841u64 >> substream_info.wrapping_sub(88)) & 1 != 0
            {
                debug!("substream_info={substream_info:#02X}")
            } else {
                log_or_err!(
                    state,
                    Error,
                    anyhow!(SyncError::InvalidSubstreamInfo(substream_info))
                )
            }

            debug!("extended_substream_info={extended_substream_info:#X}");

            if substream_info >> 7 != 0 && extended_substream_info == 3 && substream_info != 0x7C
                || extended_substream_info == 2 && substream_info != 0x68
                || extended_substream_info == 1 && substream_info & 0x78 != 0x48
            {
                log_or_err!(
                    state,
                    Warn,
                    anyhow!(SyncError::SubstreamInfoInCompatible {
                        substream_info,
                        extended_substream_info
                    })
                )
            }

            let substream_info = ms.substream_info & 0xFC;

            if substream_info >> 7 == 0 && extended_substream_info != 0 {
                log_or_err!(
                    state,
                    log::Level::Debug,
                    anyhow!(SyncError::ReservedExtendedSubstreamInfo(
                        ms.extended_substream_info
                    ))
                );
            };

            if (substream_info >> 4) & 7 == (substream_info >> 2) & 0xC {
                let sixch_assign = ms.format_info.sixch_decoder_channel_assignment;
                let eightch_assign = ms.format_info.eightch_decoder_channel_assignment;

                if sixch_assign as u16 != eightch_assign {
                    log_or_err!(
                        state,
                        log::Level::Debug,
                        anyhow!(SyncError::SixchAndEightchChannelAssignmentMismatch {
                            sixch: sixch_assign,
                            eightch: eightch_assign
                        })
                    );
                }

                if sixch_assign == 1 || eightch_assign == 1 {
                    let sixch_modifier = ms.format_info.sixch_decoder_channel_modifier;
                    let eightch_modifier = ms.format_info.eightch_decoder_channel_modifier;

                    if sixch_modifier != eightch_modifier {
                        log_or_err!(
                            state,
                            Warn,
                            anyhow!(SyncError::SixchAndEightchChannelModifierMismatch {
                                sixch: sixch_modifier,
                                eightch: eightch_modifier,
                            })
                        )
                    }
                }
            }

            for (min, bit) in [(2, 3), (2, 5), (3, 6), (4, 7)] {
                if substream_info & (1 << bit) != 0 && ms.substreams < min {
                    log_or_err!(
                        state,
                        Warn,
                        anyhow!(SyncError::SubstreamCountInsufficient { min, bit })
                    );
                }
            }

            if if substream_info >> 7 != 0 {
                4
            } else if substream_info & 0x48 != 0 || substream_info & 0x60 == 0x20 {
                (substream_info as usize >> 6 & 1) + 2
            } else {
                1
            } != ms.substreams
            {
                log_or_err!(
                    state,
                    log::Level::Debug,
                    anyhow!(SyncError::SubstreamCountInfoInconsistent)
                );
            };
        }

        let presentation_map =
            PresentationMap::with_substream_info(ms.substream_info, ms.extended_substream_info);

        // TODO: check mismatch
        state.presentation_map = Some(presentation_map);
        state.substream_mask = presentation_map
            .substream_mask_by_required_presentations(&state.required_presentations);

        state.substream_info = ms.substream_info;
        state.extended_substream_info = ms.extended_substream_info;

        ms.channel_meaning = ChannelMeaning::read(state, reader)?;

        let len = reader.position()? - start_pos;

        ms.major_sync_info_crc = reader.get_n(16)?;

        let crc = reader.crc16_check(&state.crc_major_sync_info, start_pos, len)?;

        if crc != ms.major_sync_info_crc {
            log_or_err!(
                state,
                Error,
                anyhow!(SyncError::MajorSyncCrcMismatch {
                    calculated: crc,
                    read: ms.major_sync_info_crc
                })
            );
        } else {
            // for gap check
        }

        Ok(ms)
    }

    pub fn update_decoder_state(&self, state: &mut DecoderState) -> Result<()> {
        self.format_info.update_decoder_state(state)?;
        if state.valid && state.substreams != self.substreams {
            state.valid = false;
            warn!(
                "Substream count must be constant: expected {}, found {}",
                state.substreams, self.substreams
            )
        }

        state.substreams = self.substreams;
        state.substream_info = self.substream_info;
        state.extended_substream_info = self.extended_substream_info;

        state.presentation_map = Some(PresentationMap::with_substream_info(
            self.substream_info,
            self.extended_substream_info,
        ));

        Ok(())
    }
}
