//! Evolution Frame structures
//!
//! This module contains structures for handling Evolution frames,
//! which provide metadata and protection features.

use anyhow::Result;

use crate::utils::bitstream_io::BsIoSliceReader;

/// Configuration for Evolution payload data
#[derive(Debug, Default)]
pub struct EvoPayloadConfig {
    /// actually variable_bits(11)
    pub smploffst: Option<u32>,
    pub duration: Option<u32>,
    pub groupid: Option<u32>,
    pub codedcdata: Option<u8>,
    pub create_duplicate: Option<bool>,
    pub remove_duplicate: Option<bool>,
    pub priority: Option<u8>,
    pub proc_allowed: Option<u8>,
}

impl EvoPayloadConfig {
    pub fn read(reader: &mut BsIoSliceReader) -> Result<Self> {
        let mut config = Self::default();

        if reader.get()? {
            config.smploffst = Some(reader.get_variable_bits_max(11, 2)?);
        }

        if reader.get()? {
            config.duration = Some(reader.get_variable_bits_max(11, 2)?);
        }

        if reader.get()? {
            config.groupid = Some(reader.get_variable_bits_max(2, 16)?);
        }

        if reader.get()? {
            config.codedcdata = Some(reader.get_n(8)?);
        }

        // discard_unknown_payload
        if !reader.get()? {
            let payload_frame_aligned = if config.smploffst.is_none() {
                reader.get()?
            } else {
                false
            };

            if payload_frame_aligned {
                config.create_duplicate = Some(reader.get()?);
                config.remove_duplicate = Some(reader.get()?);
            }

            if config.smploffst.is_some() || payload_frame_aligned {
                config.priority = Some(reader.get_n(5)?);
                config.proc_allowed = Some(reader.get_n(2)?);
            }
        }

        Ok(config)
    }
    pub fn payload_frame_aligned(&self) -> Option<bool> {
        if self.create_duplicate.is_none() && self.remove_duplicate.is_none() {
            return None;
        }

        Some(self.create_duplicate.is_some() || self.remove_duplicate.is_some())
    }

    pub fn discard_unknown_payload(&self) -> bool {
        self.payload_frame_aligned().is_none()
            && self.priority.is_none()
            && self.proc_allowed.is_none()
    }
}

/// Evolution frame payload container
#[derive(Debug, Default)]
pub struct EvoPayload {
    pub evo_payload_id: u32,
    pub evo_payload_config: EvoPayloadConfig,
    pub evo_payload_byte: Vec<u8>,
}

impl EvoPayload {
    pub fn read(reader: &mut BsIoSliceReader) -> Result<Self> {
        let mut evo_payload = Self {
            evo_payload_id: reader.get_n(5)?,
            ..Default::default()
        };

        if evo_payload.evo_payload_id == 0 {
            return Ok(evo_payload);
        }

        evo_payload.evo_payload_config = EvoPayloadConfig::read(reader)?;

        // evo_payload_size
        for _ in 0..reader.get_variable_bits_max(8, 4)? {
            evo_payload.evo_payload_byte.push(reader.get_n(8)?);
        }
        Ok(evo_payload)
    }
}

/// Evolution frame protection and integrity data
///
/// `protection_length_*` selects a truncation width from [`EvoProtection::SIZE`]; the bytes are
/// the leading bytes of an HMAC-SHA-256 digest. `0` means the word is absent.
///
/// The primary digest covers the access unit up to the `extra_data` header, then this frame with
/// the protection words zeroed. The caller supplies the key; see
/// [`ExtraData::verify_evo_protection`](crate::structs::extra_data::ExtraData::verify_evo_protection).
/// The secondary word is absent from every stream observed so far.
#[derive(Debug, Default)]
pub struct EvoProtection {
    pub protection_length_primary: u8,
    pub protection_length_secondary: u8,
    pub protection_bits_primary: [u8; 16],
    pub protection_bits_secondary: [u8; 16],
}

/// Outcome of checking an Evolution frame's primary protection word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvoProtectionStatus {
    /// No Evolution frame, no primary protection word, or an access unit too short to check.
    Absent,
    Match,
    Mismatch {
        expected: [u8; 16],
        actual: [u8; 16],
        length: usize,
    },
}

impl EvoProtectionStatus {
    /// The digest bytes the key produced, truncated to the width the frame selected.
    pub fn expected(&self) -> Option<&[u8]> {
        match self {
            Self::Mismatch {
                expected, length, ..
            } => Some(&expected[..*length]),
            _ => None,
        }
    }

    /// The digest bytes the frame carried, truncated to the width the frame selected.
    pub fn actual(&self) -> Option<&[u8]> {
        match self {
            Self::Mismatch { actual, length, .. } => Some(&actual[..*length]),
            _ => None,
        }
    }
}

impl EvoProtection {
    pub const SIZE: [usize; 4] = [0, 1, 4, 16];

    pub fn read(reader: &mut BsIoSliceReader) -> Result<Self> {
        let mut evo_protection = Self {
            protection_length_primary: reader.get_n(2)?,
            protection_length_secondary: reader.get_n(2)?,
            ..Default::default()
        };

        for i in 0..Self::SIZE[evo_protection.protection_length_primary as usize] {
            evo_protection.protection_bits_primary[i] = reader.get_n(8)?;
        }

        for i in 0..Self::SIZE[evo_protection.protection_length_secondary as usize] {
            evo_protection.protection_bits_secondary[i] = reader.get_n(8)?;
        }

        Ok(evo_protection)
    }
}

/// Complete Evolution frame structure (EMDF without sync)
#[derive(Debug, Default)]
pub struct EvoFrame {
    pub evo_version: u32,
    pub key_id: u32,
    pub evo_payloads: Vec<EvoPayload>,
    pub evo_protection: EvoProtection,

    /// Bit offset of `protection_bits_primary` within this frame.
    pub protection_offset: usize,
}

impl EvoFrame {
    pub fn read(reader: &mut BsIoSliceReader) -> Result<Self> {
        let frame_start = reader.position()?;
        let mut evo_frame = EvoFrame {
            evo_version: reader.get_n(2)?,
            ..Default::default()
        };
        if evo_frame.evo_version == 3 {
            evo_frame.evo_version += reader.get_variable_bits_max(2, 16)?;
        }

        evo_frame.key_id = reader.get_n(3)?;
        if evo_frame.key_id == 7 {
            evo_frame.key_id += reader.get_variable_bits_max(3, 10)?;
        }

        while let Ok(evo_payload) = EvoPayload::read(reader) {
            if evo_payload.evo_payload_id == 0 {
                break;
            }

            evo_frame.evo_payloads.push(evo_payload);
        }

        // 4 bits of lengths precede the primary word
        evo_frame.protection_offset = (reader.position()? - frame_start) as usize + 4;
        evo_frame.evo_protection = EvoProtection::read(reader)?;

        Ok(evo_frame)
    }
}
