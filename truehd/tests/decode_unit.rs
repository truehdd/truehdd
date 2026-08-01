use anyhow::Result;
use std::path::PathBuf;
use truehd::process::decode::Decoder;
use truehd::process::extract::Extractor;
use truehd::process::parse::Parser;

// Unit tests for TrueHD decoder library
//
// These tests validate the core decoder functionality at the library level,
// focusing on the critical bugs identified during development:
// - Single vs multi-presentation decode consistency
// - Non-zero audio sample validation
// - Presentation-specific decode accuracy

const ASSETS_DIR: &str = "../assets";

fn get_asset_path(filename: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(ASSETS_DIR)
        .join(filename)
}

/// Helper to extract and parse a test file into access units
fn extract_access_units(
    input_path: &PathBuf,
    limit: usize,
) -> Result<Vec<truehd::structs::access_unit::AccessUnit>> {
    // For unit tests, read the file data and feed it to the extractor
    let file_data = std::fs::read(input_path)?;
    let mut extractor = Extractor::default();
    let mut parser = Parser::default();
    let mut access_units = Vec::new();

    extractor.push_bytes(&file_data);

    for frame_result in extractor.by_ref() {
        match frame_result {
            Ok(frame) => {
                // Skip parse errors in tests
                if let Ok(au) = parser.parse(&frame) {
                    access_units.push(au);
                    if access_units.len() >= limit {
                        break;
                    }
                }
            }
            Err(truehd::utils::errors::ExtractError::InsufficientData) => break,
            Err(_) => {} // Skip extract errors in tests
        }
    }

    Ok(access_units)
}

/// Validates that decoded PCM data contains non-zero samples
fn validate_decoded_audio_content(decoded: &truehd::process::decode::DecodedAccessUnit) -> bool {
    let mut non_zero_count = 0;
    for sample_idx in 0..decoded.sample_length {
        for ch in 0..decoded.channel_count {
            if decoded.pcm_data[sample_idx][ch] != 0 {
                non_zero_count += 1;
            }
        }
    }

    // Should have some non-zero samples for real audio
    non_zero_count > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_presentation_produces_audio() -> Result<()> {
        let test_file = get_asset_path("normal.mlp");
        if !test_file.exists() {
            println!("Skipping test - normal.mlp not found");
            return Ok(());
        }

        let access_units = extract_access_units(&test_file, 10)?;
        assert!(!access_units.is_empty(), "No access units extracted");

        let mut decoder = Decoder::default();

        // Test single presentation decode
        for (i, au) in access_units.iter().enumerate() {
            let decoded = decoder.decode_presentation(au, 1)?;

            // Critical: Ensure we have non-zero audio samples (regression test)
            assert!(
                validate_decoded_audio_content(&decoded),
                "Frame {i}: Single presentation decode produced zero audio samples"
            );

            assert!(
                decoded.channel_count > 0,
                "Frame {i}: No channels in decoded audio"
            );
            assert!(
                decoded.sample_length > 0,
                "Frame {i}: No samples in decoded audio"
            );

            // For the first few frames, do detailed validation
            if i < 3 {
                println!(
                    "Frame {i}: {} channels, {} samples, sampling_freq: {}",
                    decoded.channel_count, decoded.sample_length, decoded.sampling_frequency
                );
            }
        }

        Ok(())
    }

    #[test]
    fn test_multi_presentation_produces_audio() -> Result<()> {
        let test_file = get_asset_path("normal.mlp");
        if !test_file.exists() {
            println!("Skipping test - normal.mlp not found");
            return Ok(());
        }

        let access_units = extract_access_units(&test_file, 10)?;
        assert!(!access_units.is_empty(), "No access units extracted");

        let mut decoder = Decoder::default();
        let required_presentations = [false, true, false, true]; // Presentations 1 & 3

        for (i, au) in access_units.iter().enumerate() {
            let decoded_presentations =
                decoder.decode_presentations(au, &required_presentations)?;

            // Check presentation 1
            if let Some(ref decoded_1) = decoded_presentations[1] {
                assert!(
                    validate_decoded_audio_content(decoded_1),
                    "Frame {i}: Multi presentation 1 produced zero audio samples"
                );
            }

            // Check presentation 3 (Atmos)
            if let Some(ref decoded_3) = decoded_presentations[3] {
                assert!(
                    validate_decoded_audio_content(decoded_3),
                    "Frame {i}: Multi presentation 3 produced zero audio samples"
                );
            }

            if i < 3 {
                if let Some(ref decoded_1) = decoded_presentations[1] {
                    println!(
                        "Frame {i} Pres1: {} channels, {} samples",
                        decoded_1.channel_count, decoded_1.sample_length
                    );
                }
                if let Some(ref decoded_3) = decoded_presentations[3] {
                    println!(
                        "Frame {i} Pres3: {} channels, {} samples, OAMD count: {}",
                        decoded_3.channel_count,
                        decoded_3.sample_length,
                        decoded_3.oamd.len()
                    );
                }
            }
        }

        Ok(())
    }

    #[test]
    fn test_single_vs_multi_consistency() -> Result<()> {
        let test_file = get_asset_path("normal.mlp");
        if !test_file.exists() {
            println!("Skipping consistency test - normal.mlp not found");
            return Ok(());
        }

        let access_units = extract_access_units(&test_file, 10)?;
        assert!(!access_units.is_empty(), "No access units extracted");

        let mut single_decoder = Decoder::default();
        let mut multi_decoder = Decoder::default();
        let required_presentations = [false, true, false, false]; // Only presentation 1

        for (i, au) in access_units.iter().take(5).enumerate() {
            // Test first 5 frames
            // Decode using single presentation method
            let single_result = single_decoder.decode_presentation(au, 1)?;

            // Decode using multi presentation method
            let multi_results = multi_decoder.decode_presentations(au, &required_presentations)?;
            let multi_result = multi_results[1].as_ref().unwrap();

            // Critical consistency checks
            assert_eq!(
                single_result.channel_count, multi_result.channel_count,
                "Frame {i}: Channel count mismatch"
            );
            assert_eq!(
                single_result.sample_length, multi_result.sample_length,
                "Frame {i}: Sample length mismatch"
            );
            assert_eq!(
                single_result.sampling_frequency, multi_result.sampling_frequency,
                "Frame {i}: Sampling frequency mismatch"
            );

            // Ensure both have non-zero audio content
            assert!(
                validate_decoded_audio_content(&single_result),
                "Frame {i}: Single decode produced zero samples"
            );
            assert!(
                validate_decoded_audio_content(multi_result),
                "Frame {i}: Multi decode produced zero samples"
            );

            // Sample data should be identical
            for sample_idx in 0..single_result.sample_length {
                for ch in 0..single_result.channel_count {
                    assert_eq!(
                        single_result.pcm_data[sample_idx][ch],
                        multi_result.pcm_data[sample_idx][ch],
                        "Frame {i}, Sample {sample_idx}, Channel {ch}: PCM data mismatch"
                    );
                }
            }
        }

        Ok(())
    }

    #[test]
    fn test_atmos_metadata_handling() -> Result<()> {
        let test_file = get_asset_path("normal.mlp");
        if !test_file.exists() {
            println!("Skipping Atmos test - normal.mlp not found");
            return Ok(());
        }

        let access_units = extract_access_units(&test_file, 256)?;
        let mut decoder = Decoder::default();

        let mut found_atmos = false;

        for (i, au) in access_units.iter().enumerate() {
            let decoded = decoder.decode_presentation(au, 3)?; // Presentation 3 for Atmos

            if !decoded.oamd.is_empty() {
                found_atmos = true;
                println!("Frame {i}: Found {} OAMD payload(s)", decoded.oamd.len());

                // Validate Atmos-specific properties
                assert!(decoded.channel_count > 0, "Atmos should have channels");
                assert!(
                    validate_decoded_audio_content(&decoded),
                    "Atmos audio should have non-zero samples"
                );
            }
        }

        assert!(found_atmos, "Expected to find Atmos metadata in test file");
        Ok(())
    }
}

/// Recovery: after a mid-stream lockstep reset, parsing and decoding must
/// resume at the next major sync and produce audio again.
#[test]
fn test_recovery_after_reset() -> Result<()> {
    let test_file = get_asset_path("normal.mlp");
    if !test_file.exists() {
        eprintln!("Skipping: {} not found", test_file.display());
        return Ok(());
    }

    // Read enough of the stream to span at least two major syncs
    let file_data = std::fs::read(&test_file)?;
    let take = file_data.len().min(4_000_000);
    let mut extractor = Extractor::default();
    extractor.push_bytes(&file_data[..take]);

    let mut frames = Vec::new();
    for frame_result in extractor.by_ref() {
        match frame_result {
            Ok(frame) => frames.push(frame),
            Err(truehd::utils::errors::ExtractError::InsufficientData) => break,
            Err(_) => continue,
        }
    }

    let sync_indices: Vec<usize> = frames
        .iter()
        .enumerate()
        .filter(|(_, f)| f.is_major_sync())
        .map(|(i, _)| i)
        .collect();
    if sync_indices.len() < 2 {
        eprintln!("Skipping: stream has fewer than two major syncs");
        return Ok(());
    }

    let mut parser = Parser::default();
    let mut decoder = Decoder::default();

    // Decode normally past the first major sync
    let cut = sync_indices[0] + (sync_indices[1] - sync_indices[0]) / 2;
    for frame in &frames[..cut] {
        let au = parser.parse(frame)?;
        decoder.decode_presentation(&au, 0)?;
    }

    // Simulate a fatal mid-stream failure: lockstep reset
    parser.reset_for_next_major_sync();
    decoder.reset_for_next_major_sync();

    // Frames before the next major sync may fail to parse; from the next
    // major sync onward, decoding must succeed and produce samples again.
    let mut recovered_samples = 0usize;
    for frame in &frames[cut..] {
        if let Ok(au) = parser.parse(frame)
            && let Ok(decoded) = decoder.decode_presentation(&au, 0)
        {
            recovered_samples += decoded.sample_length;
        }
    }

    assert!(
        recovered_samples > 0,
        "no audio decoded after lockstep reset; recovery failed"
    );
    Ok(())
}
