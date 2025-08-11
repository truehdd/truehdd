use super::decoder_thread::{DecoderThreadConfig, spawn_decoder_thread};
use super::handler::{DecodeHandler, FrameHandlerContext, WriterState};
use super::progress::{create_progress_bar, estimate_total_frames};
use crate::cli::command::{AudioFormat, Cli, DecodeArgs};
use anyhow::Result;
use indicatif::{MultiProgress, ProgressStyle};
use log::Level;
use std::sync::mpsc;
use truehd::process::{MAX_PRESENTATIONS, decode::Decoder, extract::Extractor, parse::Parser};

pub fn cmd_decode(args: &DecodeArgs, cli: &Cli, multi: Option<&MultiProgress>) -> Result<()> {
    if args.presentation > 3 {
        return Err(anyhow::anyhow!(
            "Presentation index must be 0-3, got {}",
            args.presentation
        ));
    }

    log::info!(
        "Decoding TrueHD stream: {} (strict mode: {}, presentation: {})",
        args.input.display(),
        cli.strict,
        args.presentation
    );

    let is_pipe = args.input.to_string_lossy() == "-";
    let base_path = args.output_path.clone();

    if let Some(ref path) = base_path {
        log::info!("Output path specified: {}", path.display());
    }

    // Estimate total frames if needed
    let should_estimate = !args.no_estimate_progress && !is_pipe && multi.is_some();
    let total_frames = if should_estimate {
        Some(estimate_total_frames(&args.input)?)
    } else {
        if is_pipe {
            log::debug!("Skipping progress estimation for pipe input");
        } else if args.no_estimate_progress {
            log::debug!("Progress estimation disabled by --no-estimate-progress flag");
        }
        None
    };

    // Create progress bar
    let pb = if let Some(multi) = multi {
        Some(create_progress_bar(multi, total_frames)?)
    } else {
        None
    };

    // Setup decoder components
    let (tx, rx) = mpsc::channel();
    let pb_clone = pb.clone();
    let strict_mode = cli.strict;
    let presentation = args.presentation;

    let extractor = Extractor::default();
    let mut parser = Parser::default();
    let mut decoder = Decoder::default();

    // Configure fail level based on strict mode
    let fail_level = if strict_mode {
        Level::Warn
    } else {
        Level::Error
    };
    parser.set_fail_level(fail_level);
    decoder.set_fail_level(fail_level);

    let state = WriterState { fail_level };

    // Setup required presentations
    let mut required_presentations = [false; MAX_PRESENTATIONS];
    required_presentations[..=presentation as usize]
        .iter_mut()
        .for_each(|p| *p = true);
    parser.set_required_presentations(&required_presentations);

    // Spawn decoder thread
    let decode_thread = spawn_decoder_thread(DecoderThreadConfig {
        input_path: args.input.clone(),
        presentation,
        strict_mode,
        tx,
        pb_clone,
        extractor,
        parser,
        decoder,
    });

    // Handle decoded frames
    let mut handler = DecodeHandler::default();
    let start_time = std::time::Instant::now();

    let effective_format = if args.presentation == 3 {
        if args.format != AudioFormat::Caf {
            log::info!(
                "Forcing CAF format for presentation 3, ignoring --format {:?}",
                args.format
            );
        }
        AudioFormat::Caf
    } else {
        args.format
    };

    while let Ok(result) = rx.recv() {
        match result {
            Ok(decoded) => {
                let ctx = FrameHandlerContext {
                    base_path: &base_path,
                    format: effective_format,
                    pb: &pb,
                    state: &state,
                    start_time,
                    bed_conform: args.bed_conform,
                };
                handler.handle_decoded_frame(decoded, &ctx)?;
            }
            Err(e) => {
                if let Some(pb) = pb {
                    pb.finish_with_message("decode failed");
                }
                return Err(e);
            }
        }
    }

    // Finalize output
    handler.finalize()?;

    // Wait for decode thread and finalize progress
    match decode_thread.join() {
        Ok(Ok(())) => {
            finalize_progress_bar(
                &pb,
                total_frames,
                handler.decoded_samples,
                handler.final_sample_rate,
                start_time,
            );
            log::info!("Decoding completed successfully");
        }
        Ok(Err(e)) => {
            if let Some(pb) = pb {
                pb.finish_with_message("decode failed");
            }
            return Err(e);
        }
        Err(_) => {
            if let Some(pb) = pb {
                pb.finish_with_message("decode thread panicked");
            }
            return Err(anyhow::anyhow!("Decode thread panicked"));
        }
    }

    Ok(())
}

fn finalize_progress_bar(
    pb: &Option<indicatif::ProgressBar>,
    total_frames: Option<u64>,
    decoded_samples: u64,
    final_sample_rate: u32,
    start_time: std::time::Instant,
) {
    if let Some(pb) = pb {
        let elapsed = start_time.elapsed();
        let audio_duration_secs = decoded_samples as f64 / final_sample_rate as f64;
        let realtime_multiplier = audio_duration_secs / elapsed.as_secs_f64();
        let final_time_str = crate::timestamp::time_str(audio_duration_secs);

        if total_frames.is_some() {
            pb.set_style(
                ProgressStyle::with_template(
                    "{bar:40.cyan/blue} {pos}/{len} frames ({percent}%)\n{msg} | elapsed: {elapsed_precise}",
                )
                .unwrap_or_else(|_| ProgressStyle::default_bar()),
            );
        } else {
            pb.set_style(
                ProgressStyle::with_template(
                    "{spinner:.green} {pos} frames\n{msg} | elapsed: {elapsed_precise}",
                )
                .unwrap_or_else(|_| ProgressStyle::default_spinner()),
            );
        }

        pb.finish_with_message(format!(
            "speed: {realtime_multiplier:.1}x | timestamp: {final_time_str}"
        ));
    }
}
