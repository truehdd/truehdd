use std::fmt::Display;

/// Frame extraction from audio bitstreams.
///
/// Provides the [`Extractor`](extract::Extractor) for finding sync patterns and
/// extracting individual [`Frame`](extract::Frame) objects from continuous bitstream data.
pub mod extract;

/// Frame parsing into structured access units.
///
/// Provides the [`Parser`](parse::Parser) for converting raw frames into
/// [`AccessUnit`](crate::structs::access_unit::AccessUnit) objects with parsed metadata.
pub mod parse;

/// Audio decoding to PCM samples.
///
/// Provides the [`Decoder`](decode::Decoder) for converting access units into
/// [`DecodedAccessUnit`](decode::DecodedAccessUnit) objects containing PCM audio data.
pub mod decode;

pub const EXAMPLE_DATA: &[u8] = &[
    0x01, 0x10, 0x00, 0x01, 0x00, 0x23, 0x00, 0x45, 0x00, 0x16, 0x00, 0x19, 0x00, 0x11, 0x80, 0x00,
    0xF0, 0x2A, 0xFF, 0xAC, 0xF8, 0x72, 0x6F, 0xBA, 0x00, 0x00, 0x80, 0x01, 0xB7, 0x52, 0x00, 0x00,
    0x00, 0x00, 0x80, 0x80, 0x10, 0x14, 0x03, 0x80, 0x3F, 0x1F, 0xE3, 0x07, 0xE3, 0x00, 0x52, 0x98,
    0xB0, 0x18, 0x03, 0xF0, 0xF1, 0xEA, 0x00, 0x00, 0x01, 0x10, 0x00, 0x00, 0x02, 0x09, 0x52, 0x80,
    0x00, 0x00, 0x00, 0x02, 0xB4, 0x44, 0x01, 0xE8, 0xC4, 0x40, 0x88, 0xD1, 0xFE, 0x91, 0x00, 0x63,
    0x03, 0xE9, 0x18, 0x33, 0x86, 0x20, 0x68, 0xFF, 0xCB, 0x6E, 0xDB, 0x6D, 0xB6, 0xDB, 0x6D, 0xB7,
    0x80, 0x00, 0x64, 0xF9, 0x50, 0x0A, 0x00, 0x00, 0x70, 0x07, 0x91, 0x40, 0x48, 0x00, 0x11, 0x3D,
    0xDB, 0xEF, 0xF3, 0xDE, 0xD0, 0x00, 0xD5, 0x04,
];

pub const MAX_PRESENTATIONS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentationMap {
    pub masks: [u8; MAX_PRESENTATIONS],
}

impl PresentationMap {
    pub fn with_substream_info(substream_info: u8, extended_substream_info: u8) -> Self {
        Self {
            masks: [
                1,
                (substream_info >> 2) & 3,
                (substream_info >> 4) & 7,
                ((substream_info >> 4) & 8) | (7 ^ (7 >> (extended_substream_info & 3))),
            ],
        }
    }

    pub fn presentation_type_by_index(&self, index: usize) -> PresentationType {
        if index >= self.masks.len() {
            return PresentationType::Invalid;
        }
        let this_mask = self.masks[index];

        if this_mask >> index != 0 {
            if let Some(down_i) = (index + 1..self.masks.len())
                .find(|&i| self.masks[i] >> i != 0 && (self.masks[i] >> index) & 1 != 0)
            {
                return PresentationType::DownmixOf(down_i);
            }
            return PresentationType::Independent;
        }

        if let Some(copy_i) = (0..index).rev().find(|&i| this_mask >> i != 0) {
            return PresentationType::CopyOf(copy_i);
        }

        PresentationType::Invalid
    }

    pub fn max_independent_presentation(&self) -> Option<usize> {
        self.masks
            .iter()
            .enumerate()
            .rev()
            .find(|&(i, &mask)| mask >> i != 0)
            .map(|(i, _)| i)
    }

    pub fn substream_mask_by_required_presentations(
        &self,
        required_presentations: &[bool; MAX_PRESENTATIONS],
    ) -> u8 {
        required_presentations
            .iter()
            .enumerate()
            .fold(
                0,
                |mask, (i, &required)| if required { mask | self.masks[i] } else { mask },
            )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationType {
    Invalid,
    CopyOf(usize),
    DownmixOf(usize),
    Independent,
}

impl Display for PresentationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PresentationType::Invalid => write!(f, "Invalid"),
            PresentationType::CopyOf(i) => write!(f, "Copy of presentation {i}"),
            PresentationType::DownmixOf(i) => write!(f, "Downmix of presentation {i}"),
            PresentationType::Independent => write!(f, "Independent"),
        }
    }
}

#[test]
fn test_presentation_map() {
    let map = PresentationMap::with_substream_info(0b11001100, 0b00000001);
    assert_eq!(map.max_independent_presentation().unwrap(), 3);
    assert_eq!(map.masks, [1, 3, 4, 12]);

    assert_eq!(
        map.presentation_type_by_index(0),
        PresentationType::DownmixOf(1)
    );
    assert_eq!(
        map.presentation_type_by_index(1),
        PresentationType::Independent
    );
    assert_eq!(
        map.presentation_type_by_index(2),
        PresentationType::DownmixOf(3)
    );

    let map = PresentationMap::with_substream_info(0b01011000, 0b00000000);
    assert_eq!(map.max_independent_presentation().unwrap(), 2);
    assert_eq!(map.masks, [1, 2, 5, 0]);

    assert_eq!(
        map.presentation_type_by_index(0),
        PresentationType::DownmixOf(2)
    );
    assert_eq!(
        map.presentation_type_by_index(1),
        PresentationType::Independent
    );
}
