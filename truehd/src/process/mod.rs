use anyhow::{Result, bail};
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

/// One access unit of an FBB (Meridian / DVD-Audio) stream, six-channel independent with a
/// two-channel downmix. Regression fixture for reading FBB at all, which bailed with
/// `unimplemented!` until 0.6.4.
pub const EXAMPLE_DATA_FBB: &[u8] = &[
    0xF0, 0xF1, 0xF1, 0xFD, 0xF8, 0x72, 0x6F, 0xBB, 0x22, 0x00, 0x00, 0x11, 0xB7, 0x52, 0x40, 0x00,
    0x00, 0x00, 0x82, 0xA1, 0x20, 0x0D, 0x56, 0x3F, 0x00, 0x00, 0x80, 0x80, 0x00, 0x00, 0xA7, 0x63,
    0x30, 0x45, 0x20, 0xDF, 0xF1, 0xEA, 0x00, 0x00, 0x01, 0x10, 0x00, 0x00, 0x02, 0x12, 0xB5, 0x80,
    0x00, 0x00, 0x00, 0x02, 0x16, 0x48, 0x79, 0xD3, 0xC4, 0xDE, 0x6F, 0x81, 0xCE, 0x17, 0xE4, 0x11,
    0x44, 0x21, 0x84, 0x80, 0x09, 0x25, 0x80, 0x00, 0x1F, 0xFF, 0x7A, 0xB9, 0xAE, 0xDB, 0xB2, 0xCB,
    0x9D, 0xE5, 0xBB, 0xDA, 0x0B, 0xA2, 0x24, 0xDA, 0x45, 0xD4, 0xE8, 0xDF, 0x1A, 0x1D, 0x3E, 0xF2,
    0x5F, 0xCC, 0x4C, 0xE4, 0xC5, 0x18, 0x30, 0xCA, 0x2A, 0x81, 0x7D, 0x89, 0x10, 0x0C, 0xF6, 0x01,
    0x7F, 0xC6, 0x80, 0x50, 0x7F, 0xED, 0x1D, 0x3F, 0x85, 0x30, 0x5F, 0xFE, 0xA1, 0x73, 0xD8, 0x05,
    0xFE, 0xDA, 0x01, 0xB1, 0xFF, 0x80, 0x13, 0x73, 0x7E, 0xAD, 0xDE, 0x21, 0xD2, 0x87, 0x64, 0x65,
    0xB4, 0xAA, 0x55, 0xA2, 0x14, 0xA4, 0xF5, 0x31, 0x60, 0xD2, 0x68, 0x33, 0x96, 0x09, 0x07, 0x52,
    0xB9, 0x90, 0xAE, 0x95, 0x31, 0x1D, 0xD4, 0x48, 0x56, 0x8A, 0xD6, 0x00, 0xE6, 0xC4, 0xF1, 0xEA,
    0x00, 0x00, 0x25, 0x50, 0x00, 0x00, 0x04, 0x14, 0xB5, 0x80, 0x00, 0x00, 0x00, 0x02, 0x10, 0x62,
    0x0A, 0xCE, 0x58, 0xD9, 0x10, 0x88, 0xFF, 0xB8, 0x10, 0x38, 0x08, 0xAA, 0x92, 0x87, 0xF9, 0x38,
    0x04, 0x67, 0xBA, 0x03, 0x04, 0x8D, 0xF0, 0x20, 0xE1, 0x33, 0xF7, 0xE7, 0xE0, 0x77, 0x7E, 0xD0,
    0x30, 0x2D, 0xC0, 0x07, 0xED, 0xE1, 0x00, 0x47, 0xFF, 0x03, 0x89, 0x61, 0xFC, 0x00, 0x02, 0x00,
    0x84, 0x01, 0x02, 0x7F, 0xCF, 0x94, 0x82, 0x70, 0xC2, 0x18, 0xC0, 0x03, 0x0C, 0x00, 0x00, 0xC0,
    0x00, 0x18, 0x00, 0x00, 0xE3, 0x93, 0x8E, 0x16, 0xEA, 0xAC, 0x88, 0x55, 0x59, 0xE3, 0x1C, 0x9D,
    0xB0, 0xB9, 0x39, 0x65, 0x51, 0x2A, 0xEE, 0x8A, 0xAD, 0x3E, 0xA4, 0x6A, 0x60, 0x6A, 0x8C, 0x02,
    0xD4, 0x23, 0xA9, 0x6A, 0x18, 0x1C, 0x2D, 0x31, 0xE5, 0x6D, 0xA0, 0x72, 0xA8, 0xE6, 0x7C, 0x28,
    0x71, 0xFA, 0xC6, 0xD7, 0xCC, 0x56, 0xE9, 0x84, 0x9A, 0x40, 0x4A, 0x3A, 0x44, 0x4B, 0xBE, 0x3B,
    0x73, 0xE2, 0xBB, 0x34, 0x31, 0xA1, 0x41, 0x72, 0xD6, 0xA4, 0x40, 0x33, 0xD8, 0x05, 0xFF, 0x0A,
    0x01, 0x11, 0xFF, 0xF4, 0x74, 0xFE, 0x15, 0x01, 0x82, 0x33, 0xD8, 0x05, 0xFF, 0x4A, 0x01, 0x11,
    0xFF, 0xB0, 0x87, 0x3C, 0x78, 0x5F, 0xE9, 0x40, 0x34, 0x7F, 0xF9, 0x00, 0x2E, 0x13, 0x3C, 0x78,
    0x5F, 0xE1, 0x40, 0x4C, 0x7F, 0xEA, 0x38, 0x6F, 0x15, 0x81, 0x7F, 0xF9, 0x01, 0x1C, 0x0D, 0x3E,
    0x1A, 0xFF, 0x1B, 0x5A, 0x0A, 0x7C, 0x5C, 0xCE, 0x5B, 0x6C, 0x2E, 0xCA, 0x0B, 0xD7, 0x59, 0x4B,
    0x8D, 0xA8, 0x2B, 0x06, 0x67, 0x8A, 0x9E, 0x86, 0xCA, 0x5B, 0xA5, 0xB9, 0xF6, 0x85, 0x69, 0x5B,
    0x94, 0x99, 0x4C, 0x64, 0x38, 0xBD, 0xC3, 0xB8, 0x4B, 0x43, 0x48, 0x2D, 0xC2, 0xE7, 0x82, 0x92,
    0x97, 0x5E, 0x72, 0x26, 0xC2, 0xB2, 0x06, 0xAC, 0x81, 0x96, 0x13, 0x91, 0x75, 0xD3, 0x91, 0x85,
    0x69, 0x81, 0x35, 0x31, 0xD8, 0x8A, 0x49, 0xB0, 0x82, 0x29, 0xD0, 0x8A, 0x08, 0x72, 0x22, 0x70,
    0x2C, 0x10, 0x33, 0x26, 0x13, 0x2D, 0x0E, 0x84, 0xC9, 0x82, 0x91, 0x62, 0x00, 0xE4, 0x62, 0x00,
    0xA4, 0x88,
];

/// One access unit of another FBB stream, six-channel copy-of two-channel. It sets
/// extra_channel_meaning_present and carries substream_info 0x05, both of which used to be
/// judged by FBA-only rules, so no frame came out. Regression fixture for FBB-specific
/// major sync info.
pub const EXAMPLE_DATA_FBB_UNEXTRACTABLE: &[u8] = &[
    0xA0, 0x4B, 0xF3, 0x48, 0xF8, 0x72, 0x6F, 0xBB, 0x2F, 0x0F, 0x00, 0x01, 0xB7, 0x52, 0x40, 0x00,
    0x00, 0x00, 0x80, 0xC2, 0x10, 0x05, 0x56, 0x03, 0x00, 0x00, 0x80, 0x80, 0x00, 0x1B, 0x65, 0x38,
    0x30, 0x3A, 0xF1, 0xEA, 0x00, 0x00, 0x01, 0x10, 0x00, 0x00, 0x02, 0x12, 0xB5, 0x80, 0x00, 0x00,
    0x00, 0x02, 0x16, 0x44, 0x51, 0x2F, 0x80, 0x43, 0x09, 0x00, 0x14, 0x4B, 0x00, 0x00, 0x3F, 0xFE,
    0xDA, 0xA1, 0x31, 0xB6, 0xC6, 0xEA, 0x0C, 0xCB, 0x6D, 0xD7, 0xF8, 0x1B, 0x49, 0x3E, 0x1C, 0xF9,
    0xC9, 0xB9, 0x48, 0xA8, 0xB1, 0xA4, 0xA7, 0x0A, 0xF5, 0x73, 0x09, 0xBA, 0xC9, 0x8F, 0x12, 0x01,
    0xE9, 0x52, 0x20, 0x19, 0xE3, 0xC2, 0xFF, 0x7A, 0x01, 0xA3, 0xFF, 0x02, 0x0C, 0xF6, 0x01, 0x7F,
    0xB6, 0x80, 0x6C, 0x7F, 0xE0, 0x04, 0x72, 0xB6, 0xF3, 0x22, 0x7D, 0xBF, 0xA7, 0x0E, 0xFB, 0xBB,
    0xEB, 0xA5, 0x37, 0x51, 0xD3, 0x7C, 0xDE, 0x3B, 0xF4, 0xCF, 0x99, 0xA5, 0x4C, 0x7D, 0x71, 0x44,
    0x67, 0x0B, 0x84, 0x00, 0xE3, 0x66,
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

    pub fn substream_mask_by_index(&self, index: usize) -> u8 {
        if index >= MAX_PRESENTATIONS {
            0
        } else {
            self.masks[index]
        }
    }

    pub fn substream_mask_by_required_presentations(
        &self,
        required_presentations: &[bool; MAX_PRESENTATIONS],
    ) -> u8 {
        required_presentations
            .iter()
            .enumerate()
            .fold(0, |mask, (i, &required)| {
                if required {
                    // A presentation the stream does not carry resolves to the highest
                    // one it does, so those substreams are still needed
                    let index = match self.presentation_type_by_index(i) {
                        PresentationType::Invalid => self.max_independent_presentation(),
                        _ => Some(i),
                    };
                    mask | index.map_or(0, |i| self.substream_mask_by_index(i))
                } else {
                    mask
                }
            })
    }

    pub fn effective_presentations(
        &self,
        required_presentations: &[bool; MAX_PRESENTATIONS],
    ) -> Result<[bool; MAX_PRESENTATIONS]> {
        let mut presentations = [false; MAX_PRESENTATIONS];

        for (i, _) in required_presentations
            .iter()
            .enumerate()
            .filter(|&(_, &required)| required)
        {
            match self.presentation_type_by_index(i) {
                // A stream need not carry four presentations. Asking for one it does
                // not have decodes the highest available rather than nothing
                PresentationType::Invalid => {
                    let Some(max_independent) = self.max_independent_presentation() else {
                        bail!("No presentation is available");
                    };
                    presentations[max_independent] = true;
                }
                PresentationType::CopyOf(copy_i) => presentations[copy_i] = true,
                _ => presentations[i] = true,
            }
        }

        Ok(presentations)
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

/// The two derived presentation kinds point in opposite directions: a presentation can only
/// be a copy of one with fewer channels, so a `CopyOf` names a LOWER index and a `DownmixOf`
/// a HIGHER one.
#[test]
fn copy_of_points_down_and_downmix_of_points_up() {
    // EXAMPLE_DATA: substream_info 0b00010100, one substream shared by three presentations
    let fba = PresentationMap::with_substream_info(0b00010100, 0);
    assert_eq!(fba.masks, [1, 1, 1, 0]);
    assert_eq!(fba.max_independent_presentation().unwrap(), 0);
    assert_eq!(
        fba.presentation_type_by_index(0),
        PresentationType::Independent
    );
    for i in [1, 2] {
        match fba.presentation_type_by_index(i) {
            PresentationType::CopyOf(j) => {
                assert!(
                    j < i,
                    "a copy must name a lower presentation, got {j} for {i}"
                )
            }
            other => panic!("presentation {i} should be a copy, got {other:?}"),
        }
    }
    assert_eq!(fba.presentation_type_by_index(3), PresentationType::Invalid);

    // EXAMPLE_DATA_FBB: substream_info 0b00001101, a two-channel downmix of the six-channel
    let fbb = PresentationMap::with_substream_info(0b00001101, 0);
    assert_eq!(fbb.masks, [1, 3, 0, 0]);
    assert_eq!(fbb.max_independent_presentation().unwrap(), 1);
    match fbb.presentation_type_by_index(0) {
        PresentationType::DownmixOf(j) => {
            assert!(j > 0, "a downmix must name a higher presentation, got {j}")
        }
        other => panic!("presentation 0 should be a downmix, got {other:?}"),
    }
    assert_eq!(
        fbb.presentation_type_by_index(1),
        PresentationType::Independent
    );
}

/// Presentation masks are NOT nested. It is tempting to assume presentation *n* reads
/// substreams 0..=n, but `substream_info = 0b11001100` with extended bit 0 gives masks
/// `[1, 3, 4, 12]`, where `3 & 4 == 0`: presentation 2 uses substream 2 alone and shares
/// nothing with presentation 1.
#[test]
fn masks_need_not_be_nested() {
    let m = PresentationMap::with_substream_info(0b11001100, 0b00000001);
    assert_eq!(m.masks, [1, 3, 4, 12]);
    assert_eq!(m.masks[1] & m.masks[2], 0, "disjoint, not nested");

    // where they do overlap it is genuine containment, so neither rule holds universally
    assert_eq!(m.masks[0] & m.masks[1], m.masks[0]);
}

/// A stream need not carry four presentations. Asking for one it does not have
/// resolved to nothing between 0.5.0 and 0.6.2, so the caller received no audio
/// while the log claimed a fallback had been applied.
#[test]
fn absent_presentation_resolves_to_the_highest_available() {
    // masks [1, 2, 5, 0]: presentation 3 is absent, 2 is the highest available.
    let map = PresentationMap::with_substream_info(0b01011000, 0b00000000);
    assert_eq!(map.presentation_type_by_index(3), PresentationType::Invalid);

    let required = [false, false, false, true];
    assert_eq!(
        map.effective_presentations(&required).unwrap(),
        [false, false, true, false]
    );
    assert_eq!(
        map.substream_mask_by_required_presentations(&required),
        map.substream_mask_by_index(2)
    );
}

#[test]
fn every_presentation_of_the_example_stream_decodes() {
    use decode::Decoder;
    use extract::Extractor;
    use parse::Parser;

    for presentation in 0..MAX_PRESENTATIONS {
        let mut extractor = Extractor::default();
        extractor.push_bytes(EXAMPLE_DATA);

        let mut parser = Parser::default();
        let mut decoder = Decoder::default();
        let mut frames = 0;

        for frame in extractor.flatten() {
            let access_unit = parser.parse(&frame).expect("example data must parse");
            let decoded = decoder
                .decode_presentation(&access_unit, presentation)
                .unwrap_or_else(|e| panic!("presentation {presentation}: {e}"));
            assert!(decoded.sample_length > 0, "presentation {presentation}");
            frames += 1;
        }

        assert_eq!(frames, 2, "presentation {presentation}");
    }
}

/// Each accumulator is gated on its own `substream_info` bit. Here bit 3 is clear, so the
/// 6-channel accumulator counts nothing at all rather than falling back to substream 0,
/// while bit 4 gates the substream-0 region into the 8-channel sum.
#[test]
fn fifo_depth_gates_each_accumulator_on_substream_info() {
    use extract::Extractor;
    use parse::Parser;

    let mut extractor = Extractor::default();
    extractor.push_bytes(EXAMPLE_DATA);

    let mut parser = Parser::default();
    for frame in extractor.flatten() {
        parser.parse(&frame).expect("example data must parse");
    }

    assert_eq!(parser.fifo_depth_peaks(), [104, 0, 104, 0, 104]);
}

/// FBB never computes the 8-channel accumulator. The 6-channel one still gates on bit 3 of
/// `substream_info` (0b00001101 here, so it counts substreams 0 and 1), and the 16-channel
/// one needs a fourth substream FBB cannot have.
#[test]
fn fbb_skips_the_eightch_accumulator_rather_than_capping_it() {
    use extract::Extractor;
    use parse::Parser;

    let mut extractor = Extractor::default();
    extractor.push_bytes(EXAMPLE_DATA_FBB);

    let mut parser = Parser::default();
    for frame in extractor.flatten() {
        parser.parse(&frame).expect("FBB example data must parse");
    }

    let peaks = parser.fifo_depth_peaks();
    assert_eq!(peaks, [172, 482, 0, 0, 482]);
    assert_eq!(peaks[2], 0, "FBB must never accumulate an 8-channel depth");
}
