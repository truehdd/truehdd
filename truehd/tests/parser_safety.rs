use std::sync::Arc;

use truehd::process::decode::DecoderState;
use truehd::process::extract::{Extractor, Frame};
use truehd::process::parse::{Parser, ParserState};
use truehd::structs::block::Block;
use truehd::structs::restart_header::{RestartHeader, RestartSyncWord};
use truehd::structs::substream::SubstreamSegment;
use truehd::structs::sync::{MAJOR_SYNC_FBA, MAJOR_SYNC_FBB};
use truehd::utils::bitstream_io::BsIoSliceReader;
use truehd::utils::crc::{CRC_MAJOR_SYNC_INFO_ALG, Crc16};
use truehd::utils::errors::ExtractError;

const FBA_2CH: &[u8] = include_bytes!("assets/fba_2ch.mlp");

fn zero_bit_state(format_sync: u32, block_size: usize) -> ParserState {
    let mut state = ParserState {
        format_sync,
        substreams: Some(1),
        samples_per_au: 40,
        has_parsed_substream: true,
        ..Default::default()
    };
    let restart = &mut state.substream_state[0].restart;
    restart.block_size = block_size;
    restart.huff_type = [0; 16];
    restart.huff_lsbs = [0; 16];
    restart.quantiser_step_size = [0; 16];
    state
}

#[test]
fn declared_substream_counts_cannot_exceed_physical_state() {
    let mut extractor = Extractor::default();
    extractor.push_bytes(FBA_2CH);
    let original = extractor.next().unwrap().unwrap();
    for count in [0, 5, 15] {
        let mut data = original.data.to_vec();
        data[20] = (count << 4) | (data[20] & 15);
        let crc = Crc16::new(&CRC_MAJOR_SYNC_INFO_ALG).update(0, &data[4..30]);
        data[30..32].copy_from_slice(&crc.to_be_bytes());
        let frame = Frame {
            data: Arc::from(data),
            ..original.clone()
        };
        assert!(Parser::default().parse(&frame).is_err(), "count {count}");
    }
    assert!(Parser::default().parse(&original).is_ok());
}

#[test]
fn state_accessors_check_the_array_even_with_an_invalid_declared_count() {
    let mut state = ParserState {
        substreams: Some(15),
        ..Default::default()
    };
    assert!(state.substream_i_state(4).is_err());
    assert!(state.substream_i_state_mut(4).is_err());
}

#[test]
fn empty_channel_spans_do_not_panic() {
    for (min_chan, max_chan) in [(1, 0), (6, 5), (15, 0)] {
        let mut state = zero_bit_state(MAJOR_SYNC_FBA, 8);
        state.substream_state[0].restart.min_chan = min_chan;
        state.substream_state[0].restart.max_chan = max_chan;
        assert!(Block::read(&mut state, &mut BsIoSliceReader::from_slice(&[0])).is_ok());
    }
}

#[test]
fn type_a_restart_reserves_space_for_both_dither_channels() {
    for max_matrix_chan in [14, 15] {
        let header = RestartHeader {
            restart_sync_word: RestartSyncWord::A,
            max_matrix_chan,
            ..Default::default()
        };
        let mut state = DecoderState {
            substreams: 1,
            ..Default::default()
        };
        assert!(header.update_decoder_state(&mut state).is_err());
    }
    for (sync, maximum) in [
        (RestartSyncWord::A, 13),
        (RestartSyncWord::B, 15),
        (RestartSyncWord::C, 15),
    ] {
        let header = RestartHeader {
            restart_sync_word: sync,
            max_matrix_chan: maximum,
            ..Default::default()
        };
        let mut state = DecoderState {
            substreams: 1,
            ..Default::default()
        };
        assert!(header.update_decoder_state(&mut state).is_ok());
    }
}

#[test]
fn excess_blocks_stop_before_the_next_allocation() {
    for (format, maximum) in [(MAJOR_SYNC_FBA, 3), (MAJOR_SYNC_FBB, 5)] {
        let mut state = zero_bit_state(format, 8);
        state.samples_per_au = 160;
        let bytes = [0; 32];
        assert!(
            SubstreamSegment::read(&mut state, &mut BsIoSliceReader::from_slice(&bytes)).is_err()
        );
        assert_eq!(state.substream_state[0].restart.block_index, maximum);
    }
}

#[test]
fn aggregate_sample_overflow_is_an_error_even_below_the_block_limit() {
    let mut state = zero_bit_state(MAJOR_SYNC_FBA, 32);
    state.substream_state[0].substream_end_ptr = 1;
    let bytes = [0x10, 0]; // Two headerless blocks; the second is last.
    assert!(SubstreamSegment::read(&mut state, &mut BsIoSliceReader::from_slice(&bytes)).is_err());
}

#[test]
fn locked_failures_are_reported_once_and_recover_in_fragmented_input() {
    let mut original = Extractor::default();
    original.push_bytes(FBA_2CH);
    let major = original.next().unwrap().unwrap();
    let minor = original.next().unwrap().unwrap();
    for mismatch in [false, true] {
        let mut bad = if mismatch {
            major.data.to_vec()
        } else {
            minor.data.to_vec()
        };
        if mismatch {
            bad[20] = 0x20 | (bad[20] & 15);
        } else {
            bad[0] ^= 0x10;
        }
        let bytes = [major.as_ref(), bad.as_slice(), major.as_ref()].concat();
        for chunk in [1, 7, bytes.len()] {
            let mut extractor = Extractor::default();
            let mut errors = 0;
            let mut frames = Vec::new();
            for fragment in bytes.chunks(chunk) {
                extractor.push_bytes(fragment);
                for result in extractor.by_ref() {
                    match result {
                        Ok(frame) => frames.push(frame),
                        Err(ExtractError::InsufficientData) => break,
                        Err(ExtractError::ParityCheckFailed) if !mismatch => errors += 1,
                        Err(ExtractError::SubstreamMismatch { .. }) if mismatch => errors += 1,
                        Err(error) => panic!("unexpected extraction error: {error}"),
                    }
                }
            }
            assert_eq!(errors, 1);
            assert_eq!(extractor.error_count(), 1);
            assert_eq!(frames.len(), 2);
            assert_eq!(frames[1].offset, (major.data.len() + bad.len()) as u64);
            assert_eq!(frames[1].as_ref(), major.as_ref());
        }
    }
}
