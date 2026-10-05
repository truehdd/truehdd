//! End-to-end stream regressions

use truehd::process::MAX_PRESENTATIONS;
use truehd::process::decode::Decoder;
use truehd::process::extract::{Extractor, Frame};
use truehd::process::parse::Parser;
use truehd::process::stream::{StreamDecoder, StreamDecoderConfig, StreamFrame};
use truehd::structs::channel::ChannelLabel;
use truehd::utils::crc::{CRC_MAJOR_SYNC_INFO_ALG, Crc16};
use truehd::utils::errors::ExtractError;

const ATMOS_CBI: &[u8] = include_bytes!("assets/fba_atmos_cbi.mlp");
const ATMOS_OBJ: &[u8] = include_bytes!("assets/fba_atmos_obj.mlp");
const SPLICED: &[u8] = include_bytes!("assets/fba_spliced.mlp");

fn extract(data: &[u8]) -> Vec<Frame> {
    let mut extractor = Extractor::default();
    extractor.push_bytes(data);
    let mut frames = Vec::new();
    for result in extractor {
        match result {
            Ok(frame) => frames.push(frame),
            Err(ExtractError::InsufficientData) => break,
            Err(error) => panic!("fixture extraction failed: {error}"),
        }
    }
    frames
}

fn decode(data: &[u8], chunk: usize, config: StreamDecoderConfig) -> Vec<StreamFrame> {
    let mut stream = StreamDecoder::new(config);
    let mut frames = Vec::new();
    for part in data.chunks(chunk) {
        frames.extend(stream.push_bytes(part).unwrap());
    }
    stream.finish().unwrap();
    assert_eq!(stream.bytes_consumed(), data.len() as u64);
    frames
}

fn repair_major_sync_crc(data: &mut [u8], offset: usize) {
    let length = if data[offset + 29] & 1 == 0 {
        26
    } else {
        28 + ((data[offset + 30] >> 3) & 0x1e) as usize
    };
    let crc = Crc16::new(&CRC_MAJOR_SYNC_INFO_ALG);
    let end = offset + 4 + length;
    let value = crc.update(crc.init, &data[offset + 4..end]);
    data[end..end + 2].copy_from_slice(&value.to_be_bytes());
}

#[test]
fn channel_layout_changes_refresh_labels_and_start_exactly_one_epoch() {
    use ChannelLabel::{C, L, LFE, Ls, R, Rs, Tfl, Tfr};

    let original = ATMOS_CBI.repeat(2);
    let mut changed = original.clone();
    let syncs: Vec<_> = extract(&original)
        .into_iter()
        .filter(|frame| frame.is_major_sync())
        .map(|frame| frame.offset as usize)
        .collect();
    // Change the layout at the splice, so its branch position must carry the new epoch.
    let change_offset = ATMOS_CBI.len();
    assert!(syncs.contains(&change_offset));
    for &offset in syncs.iter().filter(|&&offset| offset >= change_offset) {
        let assignment = u16::from_be_bytes([changed[offset + 10], changed[offset + 11]]);
        assert_eq!(assignment & 0x50, 0x40);
        // Rear speakers become front heights; the PCM and channel count stay identical.
        changed[offset + 10..offset + 12].copy_from_slice(&(assignment ^ 0x50).to_be_bytes());
        repair_major_sync_crc(&mut changed, offset);
    }

    for config in [
        StreamDecoderConfig::for_presentation(2).unwrap(),
        StreamDecoderConfig::for_presentations([true; MAX_PRESENTATIONS]).unwrap(),
    ] {
        let baseline = decode(&original, original.len(), config);
        let boundary = baseline
            .iter()
            .find(|frame| frame.byte_offset == change_offset as u64)
            .unwrap();
        let epoch_start = boundary
            .presentations
            .get(2)
            .unwrap()
            .position
            .absolute_sample;

        for chunk in [1, 7, 4096, changed.len()] {
            let frames = decode(&changed, chunk, config);
            assert_eq!(frames.len(), baseline.len());
            assert_eq!(
                frames
                    .iter()
                    .filter(|frame| frame.configuration_changed)
                    .count(),
                1
            );
            for (frame, original) in frames.iter().zip(&baseline) {
                let after_change = frame.byte_offset >= change_offset as u64;
                assert_eq!(
                    frame.configuration_changed,
                    frame.byte_offset == change_offset as u64
                );
                if frame.configuration_changed {
                    assert!(!frame.branches.is_empty());
                }
                for presentation in frame.presentations.iter() {
                    let previous = original
                        .presentations
                        .get(presentation.presentation)
                        .unwrap();
                    let decoded = presentation.decoded;
                    assert_eq!(decoded.pcm_data, previous.decoded.pcm_data);
                    assert_eq!(decoded.channel_count, previous.decoded.channel_count);
                    assert_eq!(decoded.is_duplicate, previous.decoded.is_duplicate);
                    assert_eq!(
                        presentation.position.absolute_sample,
                        previous.position.absolute_sample
                    );
                    assert_eq!(presentation.position.epoch, u64::from(after_change));
                    for branch in &frame.branches {
                        assert_eq!(
                            branch.position(presentation.presentation),
                            Some(presentation.position),
                        );
                    }
                    assert_eq!(
                        presentation.position.epoch_sample,
                        previous.position.absolute_sample
                            - if after_change { epoch_start } else { 0 }
                    );
                    if after_change && presentation.presentation == 2 {
                        assert_eq!(decoded.channel_labels, vec![L, R, C, LFE, Ls, Rs, Tfl, Tfr]);
                    } else {
                        assert_eq!(decoded.channel_labels, previous.decoded.channel_labels);
                    }
                }
            }
        }
    }
}

#[test]
fn copied_presentations_do_not_skip_other_requested_presentations() {
    let original = ATMOS_CBI.repeat(2);
    let syncs: Vec<_> = extract(&original)
        .into_iter()
        .filter(|frame| frame.is_major_sync())
        .map(|frame| frame.offset as usize)
        .collect();
    assert!(syncs.len() >= 3);
    let baseline = decode(
        &original,
        original.len(),
        StreamDecoderConfig::for_presentations([true; MAX_PRESENTATIONS]).unwrap(),
    );

    for initially_copied in [false, true] {
        let mut changed = original.clone();
        for (index, &offset) in syncs.iter().enumerate() {
            assert_eq!(changed[offset + 21], 0xc8);
            if (index % 2 == 0) == initially_copied {
                changed[offset + 21] = 0x98;
                repair_major_sync_crc(&mut changed, offset);
            }
        }

        for required in [[false, true, true, false], [true, true, true, false]] {
            let config = StreamDecoderConfig::for_presentations(required).unwrap();
            for chunk in [1, 7, 4096, changed.len()] {
                let frames = decode(&changed, chunk, config);
                assert_eq!(frames.len(), baseline.len());
                assert_eq!(
                    frames
                        .iter()
                        .filter(|frame| frame.configuration_changed)
                        .count(),
                    syncs.len() - 1
                );
                for (frame, expected) in frames.iter().zip(&baseline) {
                    let sync =
                        syncs.partition_point(|&offset| offset as u64 <= frame.byte_offset) - 1;
                    let copied = (sync % 2 == 0) == initially_copied;
                    assert_eq!(frame.presentations.get(0).is_some(), required[0] || copied);
                    assert!(frame.presentations.get(1).is_some());
                    assert_eq!(frame.presentations.get(2).is_some(), !copied);
                    assert!(frame.presentations.get(3).is_none());

                    for presentation in frame.presentations.iter() {
                        let previous = expected
                            .presentations
                            .get(presentation.presentation)
                            .unwrap();
                        assert!(
                            presentation.decoded.pcm_data == previous.decoded.pcm_data,
                            "PCM mismatch in presentation {} at access unit {}",
                            presentation.presentation,
                            frame.frame_index
                        );
                        assert_eq!(
                            presentation.decoded.channel_count,
                            previous.decoded.channel_count
                        );
                        assert_eq!(
                            presentation.decoded.is_duplicate,
                            previous.decoded.is_duplicate
                        );
                    }
                    assert_eq!(
                        frame.presentations.get(1).unwrap().position.absolute_sample,
                        expected
                            .presentations
                            .get(1)
                            .unwrap()
                            .position
                            .absolute_sample
                    );
                }
            }
        }
    }
}

#[test]
fn atmos_payloads_remain_attached_to_their_access_unit() {
    let mut parser = Parser::default();
    parser.set_required_presentations(&[false, false, false, true]);
    parser.set_check_fifo(false);
    let mut decoder = Decoder::default();
    let direct: Vec<_> = extract(ATMOS_OBJ)
        .into_iter()
        .map(|frame| {
            let access_unit = parser.parse(&frame).unwrap();
            decoder.decode_presentation(&access_unit, 3).unwrap()
        })
        .collect();
    assert!(direct.iter().any(|decoded| !decoded.oamd.is_empty()));

    for chunk in [1, 7, 4096, ATMOS_OBJ.len()] {
        let frames = decode(
            ATMOS_OBJ,
            chunk,
            StreamDecoderConfig::for_presentation(3).unwrap(),
        );
        assert_eq!(frames.len(), direct.len());
        let mut samples = 0;
        for (frame, expected) in frames.iter().zip(&direct) {
            let presentation = frame.presentations.get(3).unwrap();
            assert_eq!(presentation.position.absolute_sample, samples);
            assert_eq!(presentation.decoded.pcm_data, expected.pcm_data);
            // The payload types do not implement PartialEq. Compare the full typed
            // representation, including Evolution offsets and nested object updates.
            assert_eq!(
                format!("{:?}", presentation.decoded.oamd),
                format!("{:?}", expected.oamd)
            );
            for payload in &presentation.decoded.oamd {
                assert!(samples.checked_add(payload.evo_sample_offset).is_some());
            }
            if !presentation.decoded.is_duplicate {
                samples += presentation.decoded.sample_length as u64;
            }
        }
    }
}

#[test]
fn duplicate_access_units_keep_source_branches_distinct_from_accepted_positions() {
    let first = extract(SPLICED).into_iter().next().unwrap();
    assert_eq!(first.offset, 0);
    let mut input = first.as_ref().to_vec();
    input.extend_from_slice(SPLICED);
    let source_frames = extract(&input);

    for config in [
        StreamDecoderConfig::for_presentation(3).unwrap(),
        StreamDecoderConfig::for_presentations([true; MAX_PRESENTATIONS]).unwrap(),
    ] {
        let baseline = decode(SPLICED, SPLICED.len(), config);
        let mut expected_pcm: [Vec<[i32; 16]>; MAX_PRESENTATIONS] =
            core::array::from_fn(|_| Vec::new());
        for frame in &baseline {
            for presentation in frame.presentations.iter() {
                assert!(!presentation.decoded.is_duplicate);
                expected_pcm[presentation.presentation].extend_from_slice(
                    &presentation.decoded.pcm_data[..presentation.decoded.sample_length],
                );
            }
        }

        let mut parser = Parser::default();
        parser.set_required_presentations(config.required_presentations());
        parser.set_check_fifo(false);
        let source_branches: Vec<_> = source_frames
            .iter()
            .map(|frame| {
                parser.parse(frame).unwrap();
                parser.take_branches()
            })
            .collect();

        for chunk in [1, 7, 4096, input.len()] {
            let frames = decode(&input, chunk, config);
            assert_eq!(frames.len(), baseline.len() + 1);
            let mut consumed_pcm: [Vec<[i32; 16]>; MAX_PRESENTATIONS] =
                core::array::from_fn(|_| Vec::new());
            let mut duplicate_frames = 0;
            let mut distinct_positions = 0;

            for (frame, expected_branches) in frames.iter().zip(&source_branches) {
                assert_eq!(frame.branches.len(), expected_branches.len());
                for (branch, expected) in frame.branches.iter().zip(expected_branches) {
                    assert_eq!(branch.source, *expected);
                    assert_eq!(branch.source.au_index as u64, frame.frame_index);
                    assert_eq!(branch.source.byte_offset, frame.byte_offset);
                    for (presentation, pcm) in consumed_pcm.iter().enumerate() {
                        let expected = frame
                            .presentations
                            .get(presentation)
                            .map(|decoded| decoded.position);
                        assert_eq!(branch.position(presentation), expected);
                        if let Some(position) = expected {
                            assert_eq!(position.absolute_sample, pcm.len() as u64,);
                            if branch.source.sample != position.absolute_sample {
                                distinct_positions += 1;
                            }
                        }
                    }
                    assert!(branch.position(MAX_PRESENTATIONS).is_none());
                }

                if frame.presentations.iter().any(|p| p.decoded.is_duplicate) {
                    duplicate_frames += 1;
                    assert_eq!(frame.frame_index, 1);
                }
                for presentation in frame.presentations.iter() {
                    let pcm = &mut consumed_pcm[presentation.presentation];
                    assert_eq!(presentation.position.absolute_sample, pcm.len() as u64);
                    // Playback follows the documented example: duplicate PCM and metadata
                    // remain observable but must not be consumed a second time.
                    if presentation.decoded.is_duplicate {
                        continue;
                    }
                    pcm.extend_from_slice(
                        &presentation.decoded.pcm_data[..presentation.decoded.sample_length],
                    );
                }
            }

            assert_eq!(duplicate_frames, 1);
            assert!(distinct_positions > 0);
            for (actual, expected) in consumed_pcm.iter().zip(&expected_pcm) {
                assert_eq!(actual.len(), expected.len());
                assert!(
                    actual == expected,
                    "filtered playback changed the PCM samples"
                );
            }
        }
    }
}
