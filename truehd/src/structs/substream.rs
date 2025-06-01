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

use anyhow::{Result, bail};
use log::{trace, warn};

use crate::process::parse::ParserState;
use crate::structs::block::Block;
use crate::structs::sync::{MAJOR_SYNC_FBA, MAJOR_SYNC_FBB};
use crate::utils::bitstream_io::BsIoSliceReader;
use crate::utils::errors::SubstreamError;

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

            if ss_state.drc_active && 1 << ss_state.drc_time_update < ss_state.drc_count {
                warn!(
                    "drc_time_update={}, but drc_count={}",
                    ss_state.drc_time_update, ss_state.drc_count
                )
            }
        }

        if sd.extra_substream_word {
            if state.format_sync == MAJOR_SYNC_FBB {
                bail!(SubstreamError::InvalidExtraSubstreamWordFbb);
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
            bail!(SubstreamError::InvalidRestartNonexistent {
                expected: !sd.restart_nonexistent,
                suffix: if state.is_major_sync {
                    "".into()
                } else {
                    "out".into()
                }
            });
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
        let start_pos = reader.position()?;
        if start_pos & 0xF != 0 {
            bail!(
                "Substream {} segment not byte-aligned at start",
                state.substream_index
            );
        }

        let mut ss = Self::default();
        let mut last_block_in_segment = false;
        state.substream_state_mut()?.block_index = 0;

        while !last_block_in_segment {
            if ss.block.len() > 4 || ss.block.len() >= 3 && state.format_sync == MAJOR_SYNC_FBA {
                bail!(SubstreamError::TooManyBlocks(ss.block.len()));
            }
            ss.block.push(Block::read(state, reader)?);
            last_block_in_segment = reader.get()?;
            state.substream_state_mut()?.block_index += 1;
        }

        reader.align_16bit()?;

        let crc_present = state.substream_state()?.crc_present;

        let expected_end_pos = state.substream_segment_start_pos
            + ((state.substream_state()?.substream_end_ptr as u64) << 4);

        let mut test_size = 32;

        if crc_present {
            test_size += 16;
        }

        if expected_end_pos - reader.position()? >= test_size {
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
                        warn!(
                            "Too many zero samples to complete access unit for substream {}, Read {}",
                            state.substream_index, tm.zero_samples
                        )
                    }
                } else {
                    tm.terminator_b = reader.get_n(13)?;

                    if tm.terminator_b != 0x1234 {
                        warn!(
                            "Invalid terminator B: expected 0x1234, found {:#04X} (substream {})",
                            tm.terminator_b, state.substream_index
                        )
                    } else {
                        trace!(
                            "Termination word {:#08X} found for substream {}",
                            0xD234D234u32, state.substream_index
                        )
                    }
                }
            } else {
                warn!("Invalid termination word: expected 0x348D3, found {terminator_a:#X}",);

                reader.seek(-18)?;
            }

            // TODO: check new matrixing and filter coeffs (for each channel) happens no more than once for each substream
            // TODO: check if decoded more than it should be
        }

        let len = reader.position()? - start_pos;

        if crc_present {
            let parity = reader.parity_check_for_last_n_bits(len)? ^ 0xa9;

            ss.substream_parity = reader.get_n(8)?;
            ss.substream_crc = reader.get_n(8)?;

            if parity != ss.substream_parity {
                bail!(SubstreamError::ParityMismatch {
                    substream: state.substream_index,
                    calculated: parity,
                    read: ss.substream_parity
                });
            }

            let crc = reader.crc8_check(&state.crc_substream, start_pos, len)?;

            if crc != ss.substream_crc {
                bail!(SubstreamError::CrcMismatch {
                    substream: state.substream_index,
                    calculated: crc,
                    read: ss.substream_crc
                });
            }
        }

        let end_pos = reader.position()?;

        if end_pos & 0xF != 0 {
            bail!(SubstreamError::UnalignedSegmentEnd(state.substream_index));
        } else if expected_end_pos != end_pos {
            bail!(SubstreamError::SubstreamEndMismatch {
                substream: state.substream_index,
                read: reader.position()?,
                expected: expected_end_pos
            });
        }

        Ok(ss)
    }
}
