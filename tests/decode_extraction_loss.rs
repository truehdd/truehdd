use std::io::Write;
use std::process::{Command, Output, Stdio};

const SAMPLE: &[u8] = include_bytes!("../truehd/tests/assets/fba_2ch.mlp");

fn offsets() -> Vec<usize> {
    let mut offsets = Vec::new();
    let mut position = 0;
    while position < SAMPLE.len() {
        offsets.push(position);
        position +=
            2 * (u16::from_be_bytes([SAMPLE[position], SAMPLE[position + 1]]) & 0xfff) as usize;
    }
    assert_eq!(position, SAMPLE.len());
    offsets
}

fn decode(bytes: &[u8], pipe: bool, strict: bool, format: &str) -> (Output, Vec<u8>) {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("input.mlp");
    let output = directory.path().join("decoded");
    std::fs::write(&input, bytes).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_truehdd"));
    if strict {
        command.arg("--strict");
    }
    command.arg("decode");
    command.arg(if pipe {
        std::path::Path::new("-")
    } else {
        &input
    });
    command.args(["--presentation", "0", "--format", format, "--json"]);
    command.arg("--output-path").arg(&output);
    command
        .args(["--loglevel", "error"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if pipe {
        command.stdin(Stdio::piped());
    }
    let mut child = command.spawn().unwrap();
    if pipe {
        let mut stdin = child.stdin.take().unwrap();
        // Input may stop at the first defect while the producer is still writing.
        let result = stdin.write_all(bytes);
        assert!(result.is_ok() || result.unwrap_err().kind() == std::io::ErrorKind::BrokenPipe);
    }
    let result = child.wait_with_output().unwrap();
    let audio = std::fs::read(output.with_extension(format)).unwrap_or_default();
    (result, audio)
}

#[test]
fn extraction_loss_is_a_parse_failure_before_recovered_audio() {
    let (_, clean) = decode(SAMPLE, false, false, "pcm");
    assert_eq!(clean.len(), 152 * 40 * 2 * 3);
    let offsets = offsets();
    for strict in [false, true] {
        for pipe in [false, true] {
            let mut parity = SAMPLE.to_vec();
            parity[offsets[8]] ^= 0x10;
            let (result, pcm) = decode(&parity, pipe, strict, "pcm");
            assert_eq!(result.status.code(), Some(4), "{result:?}");
            assert!(result.stdout.is_empty(), "failed decode emitted a summary");
            assert_eq!(pcm, clean[..8 * 40 * 2 * 3]);
            let mut crc = SAMPLE.to_vec();
            crc[12] ^= 1;
            let (result, pcm) = decode(&crc, pipe, strict, "pcm");
            assert_eq!(result.status.code(), Some(4), "{result:?}");
            assert!(result.stdout.is_empty());
            assert!(
                pcm.is_empty(),
                "audio after a rejected first sync was emitted"
            );
        }
    }
}

#[test]
fn skipped_and_incomplete_input_cannot_return_success() {
    let mut middle = SAMPLE.to_vec();
    middle.insert(offsets()[128], 0);
    let mut trailing = SAMPLE.to_vec();
    trailing.push(0);
    for strict in [false, true] {
        for pipe in [false, true] {
            for bytes in [
                [vec![0], SAMPLE.to_vec()].concat(),
                middle.clone(),
                trailing.clone(),
                SAMPLE[..SAMPLE.len() - 1].to_vec(),
            ] {
                let (result, _) = decode(&bytes, pipe, strict, "pcm");
                assert_eq!(result.status.code(), Some(4), "{result:?}");
                assert!(result.stdout.is_empty());
            }
        }
    }
}

#[test]
fn clean_streams_and_recognized_timestamps_keep_their_audio() {
    let (_, expected) = decode(SAMPLE, false, false, "pcm");
    let timestamped = [&truehd::process::EXAMPLE_DATA[..16], SAMPLE].concat();
    for strict in [false, true] {
        for pipe in [false, true] {
            for bytes in [SAMPLE, timestamped.as_slice()] {
                let (result, pcm) = decode(bytes, pipe, strict, "pcm");
                assert!(result.status.success(), "{result:?}");
                assert_eq!(pcm, expected);
            }
        }
    }
    let large = SAMPLE.repeat(6);
    assert!(large.len() > 65536);
    let (file, file_pcm) = decode(&large, false, false, "pcm");
    let (pipe, pipe_pcm) = decode(&large, true, false, "pcm");
    assert!(file.status.success(), "{file:?}");
    assert!(pipe.status.success(), "{pipe:?}");
    assert!(!file_pcm.is_empty());
    assert_eq!(file_pcm, pipe_pcm);
}

#[test]
fn partial_caf_is_finalized_and_input_failures_keep_their_exit_code() {
    let mut parity = SAMPLE.to_vec();
    parity[offsets()[8]] ^= 0x10;
    let (result, caf) = decode(&parity, false, false, "caf");
    assert_eq!(result.status.code(), Some(4), "{result:?}");
    assert_eq!(&caf[..4], b"caff");
    let mut position = 8;
    let mut found_data = false;
    while position < caf.len() {
        let size = i64::from_be_bytes(caf[position + 4..position + 12].try_into().unwrap());
        assert!(size >= 0, "unfinalized CAF chunk");
        let end = position + 12 + size as usize;
        assert!(end <= caf.len());
        if &caf[position..position + 4] == b"data" {
            assert_eq!(end, caf.len());
            assert_eq!(size as usize, 4 + 8 * 40 * 2 * 3);
            found_data = true;
        }
        position = end;
    }
    assert!(found_data);
    let directory = tempfile::tempdir().unwrap();
    for input in [
        directory.path().join("missing.mlp"),
        directory.path().to_path_buf(),
    ] {
        for no_estimate in [false, true] {
            let mut command = Command::new(env!("CARGO_BIN_EXE_truehdd"));
            command.arg("decode").arg(&input);
            if no_estimate {
                command.arg("--no-estimate-progress");
            }
            assert_eq!(command.output().unwrap().status.code(), Some(3));
        }
    }
}
