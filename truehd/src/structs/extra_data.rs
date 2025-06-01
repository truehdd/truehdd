//! Extra data structures
//!
//! This module contains structures for handling extra data sections,
//! which may contain Evolution frames and other auxiliary information.

use anyhow::{Result, bail};
use log::trace;

use crate::process::parse::ParserState;
use crate::structs::evolution::EvoFrame;
use crate::utils::bitstream_io::BsIoSliceReader;
use crate::utils::errors::ExtraDataError;

/// Extra data container for auxiliary information
#[derive(Debug, Default)]
pub struct ExtraData {
    pub header_check_nibble: u8,
    pub extra_data_length: u16,
    pub evo_frame_reserved: u8,
    pub evo_frame_byte_length: u16,
    pub evo_frame: Option<EvoFrame>,
    pub ectra_data_padding: usize,
    pub extra_data_parity: u8,
}

impl ExtraData {
    pub fn read(state: &mut ParserState, reader: &mut BsIoSliceReader) -> Result<Self> {
        if reader.position()? & 0x7 != 0 {
            bail!(ExtraDataError::MisalignedExtraDataStart);
        }

        let mut extra_data = Self {
            header_check_nibble: reader.get_n(4)?,
            extra_data_length: reader.get_n(12)?,
            ..Default::default()
        };

        // Padding only
        if extra_data.header_check_nibble == 0 && extra_data.extra_data_length == 0 {
            while reader.position()? < state.expected_au_end_pos() as u64 {
                if reader.get_n::<u16>(16)? != 0 {
                    bail!(ExtraDataError::PaddingNotZero);
                }

                extra_data.ectra_data_padding += 16;
            }

            trace!(
                "Extra data contains only padding: {} bits",
                extra_data.ectra_data_padding
            );

            return Ok(extra_data);
        }

        let parity = reader.parity_check_nibble_for_last_n_bits(16)?;

        if parity != 0xF {
            bail!(ExtraDataError::LengthParityFailed(parity));
        }

        // Does not contain first 16 bits
        let extra_data_bits = (extra_data.extra_data_length as usize) << 4;
        let start_pos = reader.position()?;
        let expected_remaining_bits = state.expected_au_end_pos() - start_pos as usize;

        if extra_data_bits > expected_remaining_bits {
            bail!(ExtraDataError::ExtraDataTooLong {
                length: extra_data.extra_data_length,
                remaining: expected_remaining_bits
            });
        }

        extra_data.evo_frame = if state.flags & 0x1000 != 0 {
            extra_data.evo_frame_reserved = reader.get_n(4)?;
            extra_data.evo_frame_byte_length = reader.get_n(12)?;

            if ((extra_data.evo_frame_byte_length as usize) << 3) + 24 > extra_data_bits {
                bail!(ExtraDataError::EvoFrameTooLong {
                    evo_len: extra_data.evo_frame_byte_length,
                    extra_len: extra_data.extra_data_length
                });
            }

            if reader.position()? & 0x7 != 0 {
                bail!(ExtraDataError::EvoFrameMisaligned);
            }

            let start_pos = reader.position()?;
            let evo_frame = EvoFrame::read(reader)?;
            let actual_evo_frame_bits = (reader.position()? - start_pos) as usize;

            for _ in 0..(extra_data_bits - 24 - actual_evo_frame_bits) {
                if reader.get()? {
                    bail!(ExtraDataError::EvoFramePaddingNotZero);
                }
            }

            Some(evo_frame)
        } else {
            None
        };

        let parity = reader.parity_check_for_last_n_bits(extra_data_bits as u64 - 8)? ^ 0xA9;
        extra_data.extra_data_parity = reader.get_n(8)?;

        if parity != extra_data.extra_data_parity {
            bail!(ExtraDataError::ExtraDataParityMismatch {
                expected: parity,
                actual: extra_data.extra_data_parity
            });
        }

        Ok(extra_data)
    }
}
