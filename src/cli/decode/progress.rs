use crate::input::InputReader;
use crate::timestamp::time_str;
use anyhow::Result;
use indicatif::{MultiProgress, ProgressBar, style::ProgressStyle};
use log::info;
use std::path::Path;
use truehd::process::extract::Extractor;

pub fn finalize_progress_bar(
    pb: &Option<ProgressBar>,
    total_frames: Option<u64>,
    decoded_samples: u64,
    final_sample_rate: u32,
    start_time: std::time::Instant,
) {
    if let Some(pb) = pb {
        let elapsed = start_time.elapsed();
        let audio_duration_secs = decoded_samples as f64 / final_sample_rate as f64;
        let realtime_multiplier = audio_duration_secs / elapsed.as_secs_f64();
        let final_time_str = time_str(audio_duration_secs);

        if total_frames.is_some() {
            pb.set_style(
                ProgressStyle::with_template(PROGRESS_BAR_TEMPLATE)
                    .unwrap_or_else(|_| ProgressStyle::default_bar()),
            );
        } else {
            pb.set_style(
                ProgressStyle::with_template(PROGRESS_SPINNER_TEMPLATE)
                    .unwrap_or_else(|_| ProgressStyle::default_spinner()),
            );
        }

        pb.finish_with_message(format!(
            "speed: {realtime_multiplier:.1}x | timestamp: {final_time_str}"
        ));
    }
}

pub(crate) fn create_progress_bar(
    multi: &MultiProgress,
    total_frames: Option<u64>,
) -> Result<ProgressBar> {
    let pb = if let Some(total) = total_frames {
        let pb = multi.add(ProgressBar::new(total));
        pb.set_style(ProgressStyle::with_template(
            PROGRESS_BAR_WITH_ETA_TEMPLATE,
        )?);
        pb.enable_steady_tick(std::time::Duration::from_millis(100));
        pb
    } else {
        let pb = multi.add(ProgressBar::new_spinner());
        pb.set_style(ProgressStyle::with_template(PROGRESS_SPINNER_TEMPLATE)?);
        pb
    };
    pb.set_message("initializing decoder");
    Ok(pb)
}

pub(crate) fn estimate_total_frames(input_path: &Path) -> Result<u64> {
    info!("Counting frames for progress estimation");
    let count_start = std::time::Instant::now();

    let mut input_reader_count = InputReader::new(input_path)?;
    let mut extractor_count = Extractor::default();
    let mut successful_frames = 0u64;
    let mut bytes_read = 0u64;

    input_reader_count.process_chunks(64 * 1024, |chunk| {
        bytes_read += chunk.len() as u64;
        extractor_count.push_bytes(chunk);

        for frame_result in extractor_count.by_ref() {
            if frame_result.is_ok() {
                successful_frames += 1;
            }
        }

        Ok(true)
    })?;

    for frame_result in extractor_count {
        if frame_result.is_ok() {
            successful_frames += 1;
        }
    }

    let count_elapsed = count_start.elapsed();
    let read_speed_mbps = if count_elapsed.as_secs_f64() > 0.0 {
        (bytes_read as f64) / 1_000_000.0 / count_elapsed.as_secs_f64()
    } else {
        0.0
    };

    info!(
        "Found {successful_frames} extractable frames in {:.3}s ({:.1} MB/s, {} bytes)",
        count_elapsed.as_secs_f64(),
        read_speed_mbps,
        bytes_read
    );

    Ok(successful_frames)
}

// Progress bar template constants
const PROGRESS_BAR_TEMPLATE: &str =
    "{bar:40.cyan/blue} {pos}/{len} frames ({percent}%)\n{msg} | elapsed: {elapsed_precise}";
const PROGRESS_BAR_WITH_ETA_TEMPLATE: &str = "{bar:40.cyan/blue} {pos}/{len} frames ({percent}%)\n{msg} | elapsed: {elapsed_precise} | ETA: {eta_precise}";
const PROGRESS_SPINNER_TEMPLATE: &str =
    "{spinner:.green} {pos} frames\n{msg} | elapsed: {elapsed_precise}";
