//! Substream structures and multi-presentation organization.
//!
//! TrueHD bitstreams support up to 4 substreams carrying different audio presentations.
//!
//! ## Substream Organization
//!
//! - **Substream 0**: Always present, carries 2-channel presentation
//! - **Substream 1-3**: Optional, carry additional channel presentations
//!
//! ## Directory Structure
//!
//! Contains end pointers, restart flags, CRC flags, and dynamic range control parameters.
//!
//! ## Error Protection
//!
//! Optional 8-bit parity check and CRC protection.

use anyhow::{Result, anyhow};
use log::trace;

use crate::log_or_err;
use crate::process::parse::ParserState;
use crate::structs::block::Block;
use crate::structs::sync::{MAJOR_SYNC_FBA, MAJOR_SYNC_FBB};
use crate::utils::bitstream_io::BsIoSliceReader;
use crate::utils::errors::SubstreamError;
use crate::utils::perf::Timer;

/// Directory entry for substream navigation and control.
///
/// Provides navigation information and control flags for one substream.
/// Contains end pointers, restart flags, and optional dynamic range control data.
#[derive(Debug, Default)]
pub struct SubstreamDirectory {
    pub extra_substream_word: bool,
    pub restart_nonexistent: bool,
    pub crc_present: bool,
    pub reserved: bool,
    pub substream_end_ptr: u16,
    pub drc_gain_update: i16,
    pub drc_time_update: u8,
}
impl SubstreamDirectory {
    pub fn read(state: &mut ParserState, reader: &mut BsIoSliceReader) -> Result<Self> {
        let mut sd = Self {
            extra_substream_word: reader.get()?,
            restart_nonexistent: reader.get()?,
            crc_present: reader.get()?,
            ..Default::default()
        };

        sd.reserved = reader.get()?;
        sd.substream_end_ptr = reader.get_n(12)?;

        if state.format_sync == MAJOR_SYNC_FBA {
            let ss_state = state.substream_state_mut()?;

            ss_state.drc_count += 1;

            let (drc_active, drc_time_update, drc_count) = (
                ss_state.drc_active,
                ss_state.drc_time_update,
                ss_state.drc_count,
            );

            if drc_active && 1 << drc_time_update < drc_count {
                log_or_err!(
                    state,
                    log::Level::Warn,
                    anyhow!(SubstreamError::DrcTimeUpdateExceeded {
                        drc_time_update,
                        drc_count
                    }),
                    reader
                );
            }
        }

        if sd.extra_substream_word {
            if state.format_sync == MAJOR_SYNC_FBB {
                log_or_err!(
                    state,
                    log::Level::Error,
                    anyhow!(SubstreamError::InvalidExtraSubstreamWordFbb),
                    reader
                );
            }

            sd.drc_gain_update = reader.get_s(9)?;
            sd.drc_time_update = reader.get_n(3)?;

            reader.skip_n(4)?;

            let ss_state = state.substream_state_mut()?;

            ss_state.drc_active = true;
            ss_state.drc_gain_update = sd.drc_gain_update;
            ss_state.drc_time_update = sd.drc_time_update;
            ss_state.drc_count = 0;
        }

        if !(state.is_major_sync ^ sd.restart_nonexistent) {
            log_or_err!(
                state,
                log::Level::Warn,
                anyhow!(SubstreamError::InvalidRestartNonexistent {
                    expected: !sd.restart_nonexistent,
                    suffix: if state.is_major_sync {
                        "".into()
                    } else {
                        "out".into()
                    }
                }),
                reader
            );
        }

        let ss_state = state.substream_state_mut()?;

        ss_state.crc_present = sd.crc_present;
        ss_state.substream_end_ptr = sd.substream_end_ptr;

        Ok(sd)
    }
}

/// Stream termination information for final access unit.
///
/// Contains termination markers indicating stream completion.
#[derive(Debug, Default)]
pub struct Terminator {
    pub terminator_a: u32,
    pub zero_samples_indicated: bool,
    pub zero_samples: u16,
    pub terminator_b: u16,
}

/// Complete substream segment with compressed audio blocks.
///
/// Contains compressed audio data for one substream with optional error protection.
#[derive(Debug, Default)]
pub struct SubstreamSegment {
    pub block: Vec<Block>,
    pub substream_parity: u8,
    pub substream_crc: u8,
    pub terminator: Option<Terminator>,
}

impl SubstreamSegment {
    pub fn read(state: &mut ParserState, reader: &mut BsIoSliceReader) -> Result<Self> {
        // No start-alignment check: everything between the access unit start and the
        // first segment is a whole number of 16-bit words (the 32-bit header, a major
        // sync info that ends 16-bit aligned, 16- or 32-bit directory entries), and a
        // segment that ends misaligned ends the access unit before the next one starts.
        let start_pos = reader.position()?;

        let mut ss = Self::default();
        let mut last_block_in_segment = false;
        state.substream_state_mut()?.restart.block_index = 0;

        let blocks = Timer::start();

        while !last_block_in_segment {
            if ss.block.len() > 4 || ss.block.len() >= 3 && state.format_sync == MAJOR_SYNC_FBA {
                log_or_err!(
                    state,
                    log::Level::Warn,
                    anyhow!(SubstreamError::TooManyBlocks(ss.block.len())),
                    reader
                );
            }
            ss.block.push(Block::read(state, reader)?);
            last_block_in_segment = reader.get()?;
            state.substream_state_mut()?.restart.block_index += 1;
        }
        blocks.record(&mut state.perf.substream_segment_blocks);

        let tail = Timer::start();

        reader.align_16bit()?;

        let crc_present = state.substream_state()?.crc_present;

        let expected_end_pos = state.substream_segment_start_pos
            + ((state.substream_state()?.substream_end_ptr as u64) << 4);

        let mut test_size = 32;

        if crc_present {
            test_size += 16;
        }

        let current_pos = reader.position()?;
        let remaining_bits = expected_end_pos.checked_sub(current_pos).ok_or_else(|| {
            anyhow!(SubstreamError::SubstreamSizeUnderflow {
                substream: state.substream_index,
                end_pos: expected_end_pos,
                current_pos,
            })
        })?;

        if remaining_bits >= test_size {
            let terminator_a = reader.get_n(18)?;

            if terminator_a == 0x348D3 {
                let mut tm = Terminator {
                    terminator_a,
                    ..Default::default()
                };

                tm.zero_samples_indicated = reader.get()?;

                if tm.zero_samples_indicated {
                    tm.zero_samples = reader.get_n(13)?;

                    trace!(
                        "Termination word {:#08X} found for substream {}",
                        (((tm.zero_samples_indicated as u32) << 13) + tm.zero_samples as u32)
                            .wrapping_sub(0x2DCB4000),
                        state.substream_index
                    );

                    if (tm.zero_samples as usize) < state.samples_per_au {
                        trace!(
                            "{} sample period(s) added to complete access unit for substream {}",
                            tm.zero_samples, state.substream_index
                        )
                    } else {
                        log_or_err!(
                            state,
                            log::Level::Warn,
                            anyhow!(SubstreamError::TooManyZeroSamples {
                                substream: state.substream_index,
                                zero_samples: tm.zero_samples
                            }),
                            reader
                        );
                    }
                } else {
                    tm.terminator_b = reader.get_n(13)?;

                    if tm.terminator_b != 0x1234 {
                        log_or_err!(
                            state,
                            log::Level::Warn,
                            anyhow!(SubstreamError::InvalidTerminatorB {
                                substream: state.substream_index,
                                read: tm.terminator_b
                            }),
                            reader
                        );
                    } else {
                        trace!(
                            "Termination word {:#08X} found for substream {}",
                            0xD234D234u32, state.substream_index
                        )
                    }
                }
            } else {
                reader.seek(-18)?;

                log_or_err!(
                    state,
                    log::Level::Warn,
                    anyhow!(SubstreamError::InvalidTerminationWord(terminator_a)),
                    reader
                );
            }

            // TODO: check new matrixing and filter coeffs (for each channel) happens no more than once for each substream
            // TODO: check if decoded more than it should be
        }

        let current_pos = reader.position()?;
        let len = current_pos.checked_sub(start_pos).ok_or_else(|| {
            anyhow!(SubstreamError::SubstreamLengthUnderflow {
                substream: state.substream_index,
                current_pos,
                start_pos,
            })
        })?;

        if crc_present {
            let parity = reader.parity_check_for_last_n_bits(len)? ^ 0xa9;

            ss.substream_parity = reader.get_n(8)?;
            ss.substream_crc = reader.get_n(8)?;

            if parity != ss.substream_parity {
                log_or_err!(
                    state,
                    log::Level::Error,
                    anyhow!(SubstreamError::ParityMismatch {
                        substream: state.substream_index,
                        calculated: parity,
                        read: ss.substream_parity
                    }),
                    reader
                );
            }

            let crc = reader.crc8_check(&state.crc_substream, start_pos, len)?;

            if crc != ss.substream_crc {
                log_or_err!(
                    state,
                    log::Level::Error,
                    anyhow!(SubstreamError::CrcMismatch {
                        substream: state.substream_index,
                        calculated: crc,
                        read: ss.substream_crc
                    }),
                    reader
                );
            }
        }

        let end_pos = reader.position()?;

        if end_pos & 0xF != 0 {
            log_or_err!(
                state,
                log::Level::Error,
                anyhow!(SubstreamError::UnalignedSegmentEnd(state.substream_index)),
                reader
            );
        } else if expected_end_pos != end_pos {
            log_or_err!(
                state,
                log::Level::Error,
                anyhow!(SubstreamError::SubstreamEndMismatch {
                    substream: state.substream_index,
                    read: reader.position()?,
                    expected: expected_end_pos
                }),
                reader
            );
        }

        tail.record(&mut state.perf.substream_segment_tail);

        Ok(ss)
    }
}
