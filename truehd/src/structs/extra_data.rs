//! Extra data structures
//!
//! This module contains structures for handling extra data sections,
//! which may contain Evolution frames and other auxiliary information.

use anyhow::{Result, anyhow};
use log::trace;

use crate::log_or_err;
use crate::process::parse::ParserState;
#[cfg(feature = "evo-protection")]
use crate::structs::evolution::EvoProtectionStatus;
use crate::structs::evolution::{EvoFrame, EvoProtection};
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

    /// Byte offset of `extra_data` within the access unit.
    pub extra_data_offset: usize,
}

impl ExtraData {
    pub fn read(state: &mut ParserState, reader: &mut BsIoSliceReader) -> Result<Self> {
        if reader.position()? & 0x7 != 0 {
            log_or_err!(
                state,
                log::Level::Warn,
                anyhow!(ExtraDataError::MisalignedExtraDataStart)
            );
        }

        let extra_data_offset = (reader.position()? >> 3) as usize;

        let mut extra_data = Self {
            header_check_nibble: reader.get_n(4)?,
            extra_data_length: reader.get_n(12)?,
            extra_data_offset,
            ..Default::default()
        };

        // Padding only
        if extra_data.header_check_nibble == 0 && extra_data.extra_data_length == 0 {
            while reader.position()? < state.expected_au_end_pos() as u64 {
                if reader.get_n::<u16>(16)? != 0 {
                    log_or_err!(
                        state,
                        log::Level::Warn,
                        anyhow!(ExtraDataError::PaddingNotZero)
                    );
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
            log_or_err!(
                state,
                log::Level::Warn,
                anyhow!(ExtraDataError::LengthParityFailed(parity))
            );
        }

        // Does not contain first 16 bits
        let extra_data_bits = (extra_data.extra_data_length as usize) << 4;
        let start_pos = reader.position()?;
        let expected_remaining_bits = state.expected_au_end_pos() - start_pos as usize;

        if extra_data_bits > expected_remaining_bits {
            log_or_err!(
                state,
                log::Level::Warn,
                anyhow!(ExtraDataError::ExtraDataTooLong {
                    length: extra_data.extra_data_length,
                    remaining: expected_remaining_bits
                })
            );
        }

        extra_data.evo_frame = if state.flags & 0x1000 != 0 {
            extra_data.evo_frame_reserved = reader.get_n(4)?;
            extra_data.evo_frame_byte_length = reader.get_n(12)?;

            if ((extra_data.evo_frame_byte_length as usize) << 3) + 24 > extra_data_bits {
                log_or_err!(
                    state,
                    log::Level::Warn,
                    anyhow!(ExtraDataError::EvoFrameTooLong {
                        evo_len: extra_data.evo_frame_byte_length,
                        extra_len: extra_data.extra_data_length
                    })
                );
            }

            if reader.position()? & 0x7 != 0 {
                log_or_err!(
                    state,
                    log::Level::Warn,
                    anyhow!(ExtraDataError::EvoFrameMisaligned)
                );
            }

            let start_pos = reader.position()?;
            let evo_frame = EvoFrame::read(reader)?;
            let actual_evo_frame_bits = (reader.position()? - start_pos) as usize;

            for _ in 0..(extra_data_bits - 24 - actual_evo_frame_bits) {
                if reader.get()? {
                    log_or_err!(
                        state,
                        log::Level::Warn,
                        anyhow!(ExtraDataError::EvoFramePaddingNotZero)
                    );
                }
            }

            Some(evo_frame)
        } else {
            None
        };

        let parity = reader.parity_check_for_last_n_bits(extra_data_bits as u64 - 8)? ^ 0xA9;
        extra_data.extra_data_parity = reader.get_n(8)?;

        if parity != extra_data.extra_data_parity {
            log_or_err!(
                state,
                log::Level::Warn,
                anyhow!(ExtraDataError::ExtraDataParityMismatch {
                    expected: parity,
                    actual: extra_data.extra_data_parity
                })
            );
        }

        Ok(extra_data)
    }

    /// Bytes covered by the Evolution frame protection digest, given the access unit they were
    /// parsed from.
    ///
    /// The message is the access unit up to the `extra_data` header, followed by the Evolution
    /// frame with its protection words zeroed. The four bytes in between, the `extra_data`
    /// header and the Evolution frame length, are not covered.
    ///
    /// Returns `None` when the access unit carries no Evolution frame, or when it is too short
    /// to be the one this was parsed from.
    pub fn evo_hmac_message(&self, access_unit: &[u8]) -> Option<Vec<u8>> {
        let evo = self.evo_frame_zeroed(access_unit)?;

        let mut message = Vec::with_capacity(self.extra_data_offset + evo.len());
        message.extend_from_slice(&access_unit[..self.extra_data_offset]);
        message.extend_from_slice(&evo);

        Some(message)
    }

    /// Checks the Evolution frame's primary protection word against `key`.
    ///
    /// The word holds the leading bytes of `HMAC-SHA-256(key, `[`evo_hmac_message`]`)`, truncated
    /// to the width the frame selected. `access_unit` is the bytes this was parsed from.
    ///
    /// Secondary protection words are not checked. No observed TrueHD stream carries one, so the
    /// check would be untested.
    ///
    /// [`evo_hmac_message`]: Self::evo_hmac_message
    #[cfg(feature = "evo-protection")]
    pub fn verify_evo_protection(&self, access_unit: &[u8], key: &[u8]) -> EvoProtectionStatus {
        use hmac::{Hmac, Mac};

        let Some(evo_frame) = self.evo_frame.as_ref() else {
            return EvoProtectionStatus::Absent;
        };

        let protection = &evo_frame.evo_protection;
        let length = EvoProtection::SIZE[protection.protection_length_primary as usize];

        if length == 0 {
            return EvoProtectionStatus::Absent;
        }

        let Some(evo) = self.evo_frame_zeroed(access_unit) else {
            return EvoProtectionStatus::Absent;
        };

        let mut mac =
            <Hmac<sha2::Sha256>>::new_from_slice(key).expect("HMAC accepts keys of any length");
        mac.update(&access_unit[..self.extra_data_offset]);
        mac.update(&evo);
        let digest = mac.finalize().into_bytes();

        if digest[..length] == protection.protection_bits_primary[..length] {
            return EvoProtectionStatus::Match;
        }

        let mut expected = [0u8; 16];
        expected[..length].copy_from_slice(&digest[..length]);

        EvoProtectionStatus::Mismatch {
            expected,
            actual: protection.protection_bits_primary,
            length,
        }
    }

    /// The Evolution frame as it appears in `access_unit`, with both protection words zeroed.
    fn evo_frame_zeroed(&self, access_unit: &[u8]) -> Option<Vec<u8>> {
        let evo_frame = self.evo_frame.as_ref()?;
        let evo_start = self.extra_data_offset + 4;
        let evo_end = evo_start + self.evo_frame_byte_length as usize;

        if evo_end > access_unit.len() {
            return None;
        }

        let mut evo = access_unit[evo_start..evo_end].to_vec();

        let protection = &evo_frame.evo_protection;
        let bits = (EvoProtection::SIZE[protection.protection_length_primary as usize]
            + EvoProtection::SIZE[protection.protection_length_secondary as usize])
            << 3;

        for bit in evo_frame.protection_offset..evo_frame.protection_offset + bits {
            let byte = bit >> 3;
            if byte >= evo.len() {
                return None;
            }
            evo[byte] &= !(0x80 >> (bit & 7));
        }

        Some(evo)
    }
}
