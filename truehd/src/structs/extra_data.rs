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
///
/// `extra_data` is a container with no type field of its own. Which of its three shapes an
/// access unit carries is decided outside it, by the header word and by `flags` bit 12:
///
/// | header word | `flags & 0x1000` | shape |
/// |---|---|---|
/// | zero | either | padding: every remaining word of the access unit must be zero |
/// | non-zero | set | an Evolution frame, in [`evo_frame`](Self::evo_frame) |
/// | non-zero | clear | an opaque payload, in [`payload`](Self::payload) |
///
/// The opaque shape carries no parity byte and no zero requirement: it reads
/// `extra_data_length` words and hands them on without interpreting them.
#[derive(Debug, Default)]
pub struct ExtraData {
    pub header_check_nibble: u8,
    pub extra_data_length: u16,
    pub evo_frame_reserved: u8,
    pub evo_frame_byte_length: u16,
    pub evo_frame: Option<EvoFrame>,

    /// The payload of a block that carries no Evolution frame, `extra_data_length` words of
    /// it, uninterpreted. `None` for the Evolution and padding shapes.
    pub payload: Option<Vec<u8>>,

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
                anyhow!(ExtraDataError::MisalignedExtraDataStart),
                reader
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
                        anyhow!(ExtraDataError::PaddingNotZero),
                        reader
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
                anyhow!(ExtraDataError::LengthParityFailed(parity)),
                reader
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
                }),
                reader
            );

            // The block does not fit, so there is nothing to read: abandon
            // the access unit here rather than reading past its end.
            return Ok(extra_data);
        }

        // Without the Evolution flag the payload is opaque: no parity byte, no zero
        // requirement, handed on unexamined.
        if state.flags & 0x1000 == 0 {
            let mut payload = Vec::with_capacity(extra_data_bits >> 3);

            for _ in 0..(extra_data_bits >> 3) {
                payload.push(reader.get_n(8)?);
            }

            trace!("Extra data carries {} opaque bytes", payload.len());
            extra_data.payload = Some(payload);

            return Ok(extra_data);
        }

        // An Evolution block declares at least its own 16-bit header; below that the
        // read runs past the access unit and it is rejected whatever was found.
        if extra_data_bits < 16 {
            log_or_err!(
                state,
                log::Level::Warn,
                anyhow!(ExtraDataError::EvoFrameNoRoom {
                    extra_len: extra_data.extra_data_length
                }),
                reader
            );

            return Ok(extra_data);
        }

        extra_data.evo_frame = {
            extra_data.evo_frame_reserved = reader.get_n(4)?;
            extra_data.evo_frame_byte_length = reader.get_n(12)?;

            if ((extra_data.evo_frame_byte_length as usize) << 3) + 24 > extra_data_bits {
                log_or_err!(
                    state,
                    log::Level::Warn,
                    anyhow!(ExtraDataError::EvoFrameTooLong {
                        evo_len: extra_data.evo_frame_byte_length,
                        extra_len: extra_data.extra_data_length
                    }),
                    reader
                );

                // The declared frame does not fit, so there is no frame to read and no
                // parity byte to compare, so the access unit is abandoned here.
                return Ok(extra_data);
            }

            if reader.position()? & 0x7 != 0 {
                log_or_err!(
                    state,
                    log::Level::Warn,
                    anyhow!(ExtraDataError::EvoFrameMisaligned),
                    reader
                );
            }

            // A zero length is how a block that carries no Evolution frame this access unit
            // says so, and the whole payload is then padding. There is no
            // frame from it: it skips its digest check and writes nothing to its Evolution
            // sink. Reading one anyway would invent a frame out of the padding.
            let evo_frame = if extra_data.evo_frame_byte_length == 0 {
                None
            } else {
                let start_pos = reader.position()?;
                let evo_frame = EvoFrame::read(reader)?;
                Some((evo_frame, (reader.position()? - start_pos) as usize))
            };

            let actual_evo_frame_bits = evo_frame.as_ref().map_or(0, |(_, bits)| *bits);

            for _ in 0..(extra_data_bits - 24 - actual_evo_frame_bits) {
                if reader.get()? {
                    log_or_err!(
                        state,
                        log::Level::Warn,
                        anyhow!(ExtraDataError::EvoFramePaddingNotZero),
                        reader
                    );
                }
            }

            evo_frame.map(|(frame, _)| frame)
        };

        // The fold covers the evolution header, the frame and its padding. Both
        // implementations seed it with `evo_frame_byte_length ^ evo_frame_reserved ^ 0xA9`
        // and fold the high byte down at the end, which puts the four bits of
        // `evo_frame_reserved` at positions 0..3 where the bitstream has them at 4..7. The
        // difference is the correction below; it vanishes for the zero the encoder writes.
        let reserved = extra_data.evo_frame_reserved;
        let parity = reader.parity_check_for_last_n_bits(extra_data_bits as u64 - 8)?
            ^ 0xA9
            ^ (reserved << 4)
            ^ reserved;
        extra_data.extra_data_parity = reader.get_n(8)?;

        if parity != extra_data.extra_data_parity {
            log_or_err!(
                state,
                log::Level::Warn,
                anyhow!(ExtraDataError::ExtraDataParityMismatch {
                    expected: parity,
                    actual: extra_data.extra_data_parity
                }),
                reader
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
