use crate::input::InputReader;
use anyhow::Result;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::path::Path;
use truehd::process::extract::Extractor;

pub fn estimate_total_frames(input_path: &Path) -> Result<u64> {
    log::info!("Counting frames for progress estimation");
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

    log::info!(
        "Found {successful_frames} extractable frames in {:.3}s ({:.1} MB/s, {} bytes)",
        count_elapsed.as_secs_f64(),
        read_speed_mbps,
        bytes_read
    );

    Ok(successful_frames)
}

pub fn create_progress_bar(
    multi: &MultiProgress,
    total_frames: Option<u64>,
) -> Result<ProgressBar> {
    let pb = if let Some(total) = total_frames {
        let pb = multi.add(ProgressBar::new(total));
        pb.set_style(ProgressStyle::with_template(
            "{bar:40.cyan/blue} {pos}/{len} frames ({percent}%)\n{msg} | elapsed: {elapsed_precise} | ETA: {eta_precise}",
        )?);

        pb.enable_steady_tick(std::time::Duration::from_millis(100));
        pb
    } else {
        let pb = multi.add(ProgressBar::new_spinner());
        pb.set_style(ProgressStyle::with_template(
            "{spinner:.green} {pos} frames\n{msg} | elapsed: {elapsed_precise}",
        )?);

        pb
    };
    pb.set_message("initializing decoder");
    Ok(pb)
}
