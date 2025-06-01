//! SMPTE timestamp structures
//!
//! This module contains structures for handling SMPTE timestamps embedded
//! in streams for timecode and synchronization purposes.

use std::fmt::{Display, Formatter};

use crate::utils::errors::TimestampError;
use anyhow::{Result, bail, ensure};
use log::trace;

/// SMPTE timestamp with frame and sample precision
#[derive(Debug, Clone)]
pub struct Timestamp {
    pub hours: u16,
    pub minutes: u16,
    pub seconds: u16,
    pub frames: u16,
    pub samples: u16,
    pub _reserved1: u16,
    pub framerate: Framerate,
    pub _reserved2: bool,
    pub dropframe: bool,
}

impl Display for Timestamp {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:0width$}:{:02}:{:02}:{:02}{} @ {} fps{}",
            self.hours,
            self.minutes,
            self.seconds,
            self.frames,
            if self.samples > 0 {
                format!(" +{}", self.samples)
            } else {
                String::new()
            },
            self.framerate,
            if self.dropframe { " DF" } else { "" },
            width = if self.hours >= 100 { 0 } else { 2 }
        )
    }
}

impl Timestamp {
    pub fn from_bytes(buffer: &[u8]) -> Result<Self> {
        ensure!(
            buffer.len() >= 16,
            "Insufficient data for parsing Timestamp"
        );

        if buffer[0] != 0x01 || buffer[1] != 0x10 || buffer[14] != 0x80 || buffer[15] != 0 {
            bail!(TimestampError::InvalidSyncBytes);
        }

        let word7 = u16::from_be_bytes([buffer[12], buffer[13]]);

        let timestamp = Timestamp {
            hours: Self::parse_bcd16(u16::from_be_bytes([buffer[2], buffer[3]]))?,
            minutes: Self::parse_bcd16(u16::from_be_bytes([buffer[4], buffer[5]]))?,
            seconds: Self::parse_bcd16(u16::from_be_bytes([buffer[6], buffer[7]]))?,
            frames: Self::parse_bcd16(u16::from_be_bytes([buffer[8], buffer[9]]))?,
            samples: u16::from_be_bytes([buffer[10], buffer[11]]),
            _reserved1: word7 >> 6,
            framerate: ((((word7) >> 2) & 0xF) as u8).into(),
            _reserved2: (word7 & 2) != 0,
            dropframe: (word7 & 1) != 0,
        };

        let samples_part = if timestamp.samples > 0 {
            format!(" +{}", timestamp.samples)
        } else {
            String::new()
        };
        let dropframe_part = if timestamp.dropframe { " DF" } else { "" };

        trace!(
            "SMPTE timestamp: {:02}:{:02}:{:02}:{:02}{} @ {}fps{}",
            timestamp.hours,
            timestamp.minutes,
            timestamp.seconds,
            timestamp.frames,
            samples_part,
            timestamp.framerate,
            dropframe_part
        );

        Ok(timestamp)
    }

    pub fn parse_bcd16(value: u16) -> Result<u16> {
        let a = value >> 12;
        let b = (value >> 8) & 0xF;
        let c = (value >> 4) & 0xF;
        let d = value & 0xF;

        if a > 9 || b > 9 || c > 9 || d > 9 {
            bail!(TimestampError::InvalidBcdDigit);
        }

        Ok(1000 * a + 100 * b + 10 * c + d)
    }
}

/// SMPTE framerate enumeration
#[derive(Debug, Clone)]
#[repr(u8)]
pub enum Framerate {
    R23_976 = 0,
    R24,
    R25,
    R29_97,
    R30,
    R50,
    R59_94,
    R60,
    Invalid(u8) = 0xFF,
}

impl From<u8> for Framerate {
    fn from(value: u8) -> Self {
        match value {
            1 => Self::R23_976,
            2 => Self::R24,
            3 => Self::R25,
            4 => Self::R29_97,
            5 => Self::R30,
            6 => Self::R50,
            7 => Self::R59_94,
            8 => Self::R60,
            _ => Self::Invalid(value),
        }
    }
}

impl Display for Framerate {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let fps = match &self {
            Framerate::R23_976 => "23.976",
            Framerate::R24 => "24",
            Framerate::R25 => "25",
            Framerate::R29_97 => "29.97",
            Framerate::R30 => "30",
            Framerate::R50 => "50",
            Framerate::R59_94 => "59.94",
            Framerate::R60 => "60",
            Framerate::Invalid(v) => &format!("Invalid({v:02X})"),
        };

        f.write_str(fps)
    }
}

#[test]
fn print_timestamp() {
    let timestamp = Timestamp {
        hours: 2,
        minutes: 34,
        seconds: 56,
        frames: 23,
        samples: 0,
        _reserved1: 0,
        framerate: Framerate::R23_976,
        _reserved2: false,
        dropframe: false,
    };
    assert_eq!(format!("{timestamp}"), "02:34:56:23 @ 23.976 fps");
}
