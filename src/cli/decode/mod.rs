mod handler;
mod output;
mod pipeline;
mod progress;

use super::command::{Cli, DecodeArgs};
use crate::exit::ExitError;
use anyhow::Result;
use indicatif::MultiProgress;
use log::{Level, info};
use pipeline::{PipelineError, run_threaded_pipeline};
use progress::finalize_progress_bar;
use progress::{create_progress_bar, estimate_total_frames};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

pub fn cmd_decode(args: &DecodeArgs, cli: &Cli, multi: Option<&MultiProgress>) -> Result<()> {
    info!(
        "Decoding TrueHD stream: {} (strict mode: {}, presentation: {})",
        args.input.display(),
        cli.strict,
        args.presentation
    );

    let is_pipe = args.input.to_string_lossy() == "-";

    if let Some(ref path) = args.output_path {
        info!("Output path specified: {}", path.display());
    }

    // Estimate total frames once if needed
    let total_frames = if !args.no_estimate_progress && !is_pipe {
        Some(
            estimate_total_frames(&args.input).map_err(|source| ExitError {
                code: crate::exit::INPUT,
                source,
            })?,
        )
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

    let result =
        run_threaded_pipeline(args, fail_level, strict_mode, pb.as_ref(), progress_counter);

    match result {
        Ok(summary) => {
            finalize_progress_bar(
                &pb,
                total_frames,
                summary.total_samples,
                summary.final_sample_rate,
                summary.start_time,
            );

            info!(
                "Processing complete: {} frames, {} samples",
                summary.decoded_frames, summary.total_samples
            );

            if args.evo_key.is_some() {
                info!(
                    "Evolution protection: {} frames checked, {} mismatched",
                    summary.evo_checked, summary.evo_failed
                );
            }
            info!("Decoding completed successfully");

            if args.json {
                println!("{}", summary.to_json(&args.input));
            }

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
            Err(ExitError {
                code: e.exit_code(),
                source: anyhow::anyhow!("{e}"),
            }
            .into())
        }
    }
}
