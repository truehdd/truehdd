//! Inputs no encoder produces. Each one is well formed as far as its framing and checksums
//! go, and declares something the format has no room for.

use std::sync::Arc;

use truehd::process::decode::Decoder;
use truehd::process::extract::{Extractor, Frame};
use truehd::process::parse::{Parser, ParserState};
use truehd::structs::restart_header::RestartHeader;
use truehd::structs::substream::{SubstreamSegment, Terminator};
use truehd::structs::sync::MAJOR_SYNC_FBA;
use truehd::utils::bitstream_io::BsIoSliceReader;
use truehd::utils::crc::{CRC_MAJOR_SYNC_INFO_ALG, Crc16};
use truehd::utils::errors::{ExtractError, RestartHeaderError, SubstreamError};

const FBA_2CH: &[u8] = include_bytes!("assets/fba_2ch.mlp");

/// Packs `(value, width)` fields most significant bit first, zero padded to `len` bytes.
fn pack(fields: &[(u32, usize)], len: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; len];
    let mut position = 0;

    for &(value, width) in fields {
        for i in (0..width).rev() {
            if (value >> i) & 1 == 1 {
                bytes[position >> 3] |= 0x80 >> (position & 7);
            }
            position += 1;
        }
    }

    bytes
}

/// The count is a four-bit field, and there are four substreams to index.
#[test]
fn a_major_sync_cannot_declare_a_substream_count_outside_one_to_four() {
    let mut extractor = Extractor::default();
    extractor.push_bytes(FBA_2CH);
    let original = extractor.next().unwrap().unwrap();

    for count in [0, 5, 15] {
        let mut data = original.data.to_vec();
        data[20] = (count << 4) | (data[20] & 0xF);
        let crc = Crc16::new(&CRC_MAJOR_SYNC_INFO_ALG);
        let crc = crc.update(crc.init, &data[4..30]);
        data[30..32].copy_from_slice(&crc.to_be_bytes());

        let mut extractor = Extractor::default();
        extractor.push_bytes(&data);
        assert!(matches!(
            extractor.next(),
            Some(Err(ExtractError::InvalidSubstreamCount(read))) if read == count as usize
        ));

        let frame = Frame {
            data: Arc::from(data),
            ..original.clone()
        };
        assert!(Parser::default().parse(&frame).is_err(), "count {count}");
    }
}

/// A channel range that runs backwards, and a 0x31EA substream whose two noise channels
/// would land past the sixteenth.
#[test]
fn a_restart_header_cannot_name_channels_there_is_no_room_for() {
    let read = |min_chan, max_chan, max_matrix_chan| {
        let bytes = pack(
            &[
                (0x31EA, 14),
                (0, 16),
                (min_chan, 4),
                (max_chan, 4),
                (max_matrix_chan, 4),
            ],
            64,
        );
        let mut state = ParserState {
            substreams: Some(1),
            ..Default::default()
        };

        RestartHeader::read(&mut state, &mut BsIoSliceReader::from_slice(&bytes))
            .unwrap_err()
            .downcast::<RestartHeaderError>()
    };

    assert!(matches!(
        read(3, 1, 3),
        Ok(RestartHeaderError::ChannelRangeReversed { min: 3, max: 1 })
    ));
    assert!(matches!(
        read(0, 1, 14),
        Ok(RestartHeaderError::MatrixChannelsLeaveNoNoise(14))
    ));
}

/// Blocks of 32 samples in an access unit of 40, with no last block in sight. Reading has
/// to stop at the second.
#[test]
fn blocks_cannot_hold_more_samples_than_the_access_unit() {
    let mut state = ParserState {
        format_sync: MAJOR_SYNC_FBA,
        substreams: Some(1),
        samples_per_au: 40,
        has_parsed_substream: true,
        ..Default::default()
    };
    let restart = &mut state.substream_state[0].restart;
    restart.block_size = 32;
    restart.huff_lsbs = [0; 16];

    // A block with no header and no coded bits takes two bits, both zero here.
    let error = SubstreamSegment::read(&mut state, &mut BsIoSliceReader::from_slice(&[0; 8]))
        .unwrap_err()
        .downcast::<SubstreamError>();

    assert!(matches!(
        error,
        Ok(SubstreamError::SampleCountMismatch {
            decoded: 64,
            expected: 40,
            ..
        })
    ));
}

/// `zero_samples` is thirteen bits wide, far more than an access unit holds.
#[test]
fn a_terminator_cannot_drop_more_samples_than_the_access_unit_holds() {
    let mut extractor = Extractor::default();
    extractor.push_bytes(FBA_2CH);
    let mut parser = Parser::default();
    let mut decoder = Decoder::default();

    let first = parser.parse(&extractor.next().unwrap().unwrap()).unwrap();
    decoder.decode_presentation(&first, 0).unwrap();

    let mut second = parser.parse(&extractor.next().unwrap().unwrap()).unwrap();
    second.substream_segment[0].terminator = Some(Terminator {
        zero_samples_indicated: true,
        zero_samples: 8191,
        ..Default::default()
    });

    assert_eq!(
        decoder
            .decode_presentation(&second, 0)
            .unwrap()
            .sample_length,
        0
    );
}
