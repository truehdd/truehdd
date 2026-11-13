use std::process::Command;

const SAMPLE: &[u8] = include_bytes!("../truehd/tests/assets/fba_2ch.mlp");

#[test]
fn verification_rejects_inserted_middle_bytes_and_preserves_clean_streams() {
    let syncs: Vec<_> = SAMPLE
        .windows(4)
        .enumerate()
        .filter(|(_, w)| *w == [0xf8, 0x72, 0x6f, 0xba])
        .map(|(i, _)| i - 4)
        .collect();
    let mut modified = SAMPLE.to_vec();
    modified.insert(syncs[1], 0);
    let directory = tempfile::tempdir().unwrap();
    for strict in [false, true] {
        for (name, bytes, valid) in [
            ("clean.mlp", SAMPLE, true),
            ("modified.mlp", modified.as_slice(), false),
        ] {
            let path = directory.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            let mut command = Command::new(env!("CARGO_BIN_EXE_truehdd"));
            command.args(["verify", "--json"]);
            if strict {
                command.arg("--strict");
            }
            let result = command.arg(&path).output().unwrap();
            assert_eq!(
                result.status.success(),
                valid,
                "{name}, strict={strict}: {}",
                String::from_utf8_lossy(&result.stdout)
            );
        }
    }
}

#[test]
fn verification_rejects_unknown_prefixes_but_accepts_a_recognized_timestamp() {
    let timestamp = &truehd::process::EXAMPLE_DATA[..16];
    assert!(truehd::structs::timestamp::Timestamp::from_bytes(timestamp).is_ok());
    let directory = tempfile::tempdir().unwrap();
    for strict in [false, true] {
        for (prefix, valid) in [
            (vec![0], false),
            (vec![0; 24], false),
            (timestamp.to_vec(), true),
        ] {
            let path = directory.path().join("prefixed.mlp");
            std::fs::write(&path, [prefix.as_slice(), SAMPLE].concat()).unwrap();
            let mut command = Command::new(env!("CARGO_BIN_EXE_truehdd"));
            command.args(["verify", "--json"]);
            if strict {
                command.arg("--strict");
            }
            let output = command.arg(&path).output().unwrap();
            assert_eq!(
                output.status.success(),
                valid,
                "prefix length {}, strict={strict}: {}",
                prefix.len(),
                String::from_utf8_lossy(&output.stdout)
            );
        }
    }
}
