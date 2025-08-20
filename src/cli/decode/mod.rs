mod handler;
mod output;
mod pipeline;
mod progress;

use super::command::{AudioFormat, Cli, DecodeArgs};
use anyhow::Result;
use indicatif::MultiProgress;
use log::{Level, info};
use pipeline::{PipelineError, run_threaded_pipeline};
use progress::finalize_progress_bar;
use progress::{create_progress_bar, estimate_total_frames};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

pub fn cmd_decode(args: &DecodeArgs, cli: &Cli, multi: Option<&MultiProgress>) -> Result<()> {
    if args.presentation > 3 {
        return Err(anyhow::anyhow!(
            "Presentation index must be 0-3, got {}",
            args.presentation
        ));
    }

    info!(
        "Decoding TrueHD stream: {} (strict mode: {}, presentation: {})",
        args.input.display(),
        cli.strict,
        args.presentation
    );

    let is_pipe = args.input.to_string_lossy() == "-";
    let effective_format = if args.presentation == 3 {
        if args.format != AudioFormat::Caf {
            info!(
                "Forcing CAF format for presentation 3, ignoring --format {:?}",
                args.format
            );
        }
        AudioFormat::Caf
    } else {
        args.format
    };

    if let Some(ref path) = args.output_path {
        info!("Output path specified: {}", path.display());
    }

    // Estimate total frames once if needed
    let total_frames = if !args.no_estimate_progress && !is_pipe {
        Some(estimate_total_frames(&args.input)?)
    } else {
        None
    };

    // Create progress bar
    let pb = if let Some(multi) = multi {
        if cli.progress {
            Some(create_progress_bar(multi, total_frames)?)
        } else {
            None
        }
    } else {
        None
    };

    let fail_level = if cli.strict {
        Level::Warn
    } else {
        Level::Error
    };
    let strict_mode = cli.strict;

    // Shared progress counter for thread-safe updates
    let progress_counter = Arc::new(AtomicU64::new(0));

    // Enhanced pipeline with proper error handling and bounded channels
    let result = run_threaded_pipeline(
        args,
        effective_format,
        fail_level,
        strict_mode,
        pb.as_ref(),
        progress_counter,
    );

    match result {
        Ok(mut handler) => {
            handler.finalize()?;
            finalize_progress_bar(
                &pb,
                total_frames,
                handler.total_samples,
                handler.final_sample_rate,
                handler.start_time,
            );

            info!(
                "Processing complete: {} frames, {} samples",
                handler.decoded_frames, handler.total_samples
            );
            info!("Decoding completed successfully");
            Ok(())
        }
        Err(e) => {
            if let Some(pb) = pb {
                let error_msg = match &e {
                    PipelineError::Input(_) => "Input reading failed",
                    PipelineError::Parse(_) => "Frame parsing failed",
                    PipelineError::Decode(_) => "Audio decoding failed",
                    PipelineError::Write(_) => "File writing failed",
                };
                pb.abandon_with_message(error_msg);
            }
            Err(anyhow::anyhow!("Pipeline error: {}", e))
        }
    }
}
