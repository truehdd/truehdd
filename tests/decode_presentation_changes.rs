use serde::Deserialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use truehd::process::extract::{Extractor, Frame};
use truehd::utils::crc::{CRC_MAJOR_SYNC_INFO_ALG, Crc16};
use truehd::utils::errors::ExtractError;

const ATMOS_CBI: &[u8] = include_bytes!("../truehd/tests/assets/fba_atmos_cbi.mlp");
const COPIES: usize = 6;

#[derive(Debug, Deserialize)]
struct Summary {
    frames: u64,
    samples: u64,
    presentations: Vec<PresentationSummary>,
}

#[derive(Debug, Deserialize)]
struct PresentationSummary {
    index: usize,
    files: Vec<PathBuf>,
}

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

fn alternating_fixture() -> (Vec<u8>, u64) {
    let frames = extract(ATMOS_CBI);
    let mut data = ATMOS_CBI.repeat(COPIES);
    let crc = Crc16::new(&CRC_MAJOR_SYNC_INFO_ALG);
    for copy in (1..COPIES).step_by(2) {
        for frame in frames.iter().filter(|frame| frame.is_major_sync()) {
            let offset = copy * ATMOS_CBI.len() + frame.offset as usize;
            assert_eq!(data[offset + 21], 0xc8);
            // Presentation 2 now copies presentation 0; the encoded PCM is unchanged.
            data[offset + 21] = 0x98;
            let length = if data[offset + 29] & 1 == 0 {
                26
            } else {
                28 + ((data[offset + 30] >> 3) & 0x1e) as usize
            };
            let end = offset + 4 + length;
            let value = crc.update(crc.init, &data[offset + 4..end]);
            data[end..end + 2].copy_from_slice(&value.to_be_bytes());
        }
    }
    (data, frames.len() as u64)
}

fn decode(input: &Path, base: &Path, format: &str, presentation: &str) -> Summary {
    let output = Command::new(env!("CARGO_BIN_EXE_truehdd"))
        .arg("decode")
        .arg(input)
        .arg("--output-path")
        .arg(base)
        .args([
            "--format",
            format,
            "--presentation",
            presentation,
            "--no-estimate-progress",
            "--json",
            "--loglevel",
            "error",
        ])
        .output()
        .expect("run truehdd");
    assert!(
        output.status.success(),
        "decode failed: {}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("decode JSON summary")
}

#[test]
fn single_output_preserves_every_segment_when_presentations_change() {
    let dir = tempfile::tempdir().unwrap();
    let original_path = dir.path().join("original.mlp");
    let alternating_path = dir.path().join("alternating.mlp");
    let (alternating, frames_per_copy) = alternating_fixture();
    std::fs::write(&original_path, ATMOS_CBI).unwrap();
    std::fs::write(&alternating_path, alternating).unwrap();

    for (format, extension) in [("pcm", "pcm"), ("caf", "caf"), ("w64", "wav")] {
        let mut references = Vec::new();
        let mut samples_per_copy = None;
        for presentation in [0, 2] {
            let reference = decode(
                &original_path,
                &dir.path()
                    .join(format!("reference_{format}_{presentation}")),
                format,
                &presentation.to_string(),
            );
            assert_eq!(reference.frames, frames_per_copy);
            assert_eq!(reference.presentations.len(), 1);
            assert_eq!(reference.presentations[0].index, presentation);
            assert_eq!(reference.presentations[0].files.len(), 1);
            if let Some(samples) = samples_per_copy {
                assert_eq!(reference.samples, samples);
            }
            samples_per_copy = Some(reference.samples);
            references.push(std::fs::read(&reference.presentations[0].files[0]).unwrap());
        }

        let base_name = format!("alternating_{format}");
        let summary = decode(&alternating_path, &dir.path().join(&base_name), format, "2");
        assert_eq!(summary.frames, frames_per_copy * COPIES as u64);
        assert_eq!(summary.samples, samples_per_copy.unwrap() * COPIES as u64);
        assert_eq!(summary.presentations.len(), 2);
        let files: Vec<_> = summary
            .presentations
            .iter()
            .flat_map(|presentation| &presentation.files)
            .collect();
        assert_eq!(files.len(), COPIES);
        assert_eq!(files.iter().collect::<HashSet<_>>().len(), COPIES);

        for copy in 0..COPIES {
            let suffix = if copy == 0 {
                String::new()
            } else {
                format!("_{}", copy as u64 * frames_per_copy)
            };
            let path = dir.path().join(format!("{base_name}{suffix}.{extension}"));
            let presentation = if copy % 2 == 0 { 2 } else { 0 };
            let output = summary
                .presentations
                .iter()
                .find(|output| output.index == presentation)
                .unwrap();
            assert!(output.files.contains(&path), "missing segment {path:?}");
            assert_eq!(output.files.len(), COPIES / 2);
            assert!(
                std::fs::read(&path).unwrap() == references[presentation / 2],
                "{format} segment {copy} differs from its stable presentation"
            );
        }

        if format == "pcm" {
            let multiple = decode(
                &alternating_path,
                &dir.path().join("multiple"),
                format,
                "0,2",
            );
            assert_eq!(multiple.frames, summary.frames);
            assert_eq!(multiple.samples, summary.samples);
            assert_eq!(multiple.presentations.len(), 2);
        }
    }
}
