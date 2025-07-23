use crate::caf::{CAFWriter, wrap_pcm_file_with_caf_header};
use crate::damf::{Configuration, Data, Event};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use anyhow::Result;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use log::Level;

use super::command::{AudioFormat, Cli, DecodeArgs};
use crate::input::InputReader;
use crate::timestamp::time_str;
use truehd::process::{MAX_PRESENTATIONS, decode::Decoder, extract::Extractor, parse::Parser};

fn create_path_with_extension(base_path: &Path, expected_ext: &str) -> PathBuf {
    if let Some(existing_ext) = base_path.extension() {
        if existing_ext == expected_ext {
            base_path.to_path_buf()
        } else {
            let mut path = base_path.to_path_buf();
            let new_name = format!(
                "{}.{}",
                base_path.file_name().unwrap().to_string_lossy(),
                expected_ext
            );
            path.set_file_name(new_name);
            path
        }
    } else {
        let mut path = base_path.to_path_buf();
        path.set_extension(expected_ext);
        path
    }
}

fn create_output_paths(
    base_path: &Path,
    format: AudioFormat,
    has_atmos: bool,
) -> (PathBuf, PathBuf) {
    let audio_ext = match (format, has_atmos) {
        (AudioFormat::Caf, false) => "caf",
        (AudioFormat::Pcm, false) => "pcm",
        (_, true) => "atmos.audio",
    };

    let audio_path = create_path_with_extension(base_path, audio_ext);

    let metadata_path = if has_atmos {
        create_path_with_extension(base_path, "atmos.metadata")
    } else {
        PathBuf::new() // Empty path for non-atmos
    };

    (audio_path, metadata_path)
}

enum AudioWriter {
    Pcm(BufWriter<File>),
    Caf(CAFWriter<BufWriter<File>>),
}

pub fn cmd_decode(args: &DecodeArgs, cli: &Cli, multi: Option<&MultiProgress>) -> Result<()> {
    macro_rules! pb_update {
        ($pb:expr, $method:ident($($args:expr),*)) => {
            if let Some(ref pb) = $pb {
                pb.$method($($args),*);
            }
        };
    }

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

    let mut audio_writer = None;

    let base_path = if let Some(base_path) = &args.output_path {
        log::info!("Output path specified: {}", base_path.display());
        Some(base_path.clone())
    } else {
        None
    };

    let should_estimate = !args.no_estimate_progress && !is_pipe && multi.is_some();
    let total_frames = if should_estimate {
        log::info!("Counting frames for progress estimation");
        let count_start = std::time::Instant::now();

        let mut input_reader_count = InputReader::new(&args.input)?;
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
        Some(successful_frames)
    } else {
        if is_pipe {
            log::debug!("Skipping progress estimation for pipe input");
        } else if args.no_estimate_progress {
            log::debug!("Progress estimation disabled by --no-estimate-progress flag");
        }
        None
    };

    let pb = if let Some(multi) = multi {
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
        Some(pb)
    } else {
        None
    };

    let (tx, rx) = mpsc::channel();
    let pb_clone = pb.clone();
    let strict_mode = cli.strict;
    let presentation = args.presentation;

    let mut extractor = Extractor::default();
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

    let mut required_presentations = [false; MAX_PRESENTATIONS];
    required_presentations[presentation as usize] = true;

    // Track audio file path for potential renaming if Atmos is detected later
    let mut original_audio_path: Option<PathBuf> = None;
    let mut audio_created_without_atmos = false;
    let mut original_format_used: Option<AudioFormat> = None;
    let mut audio_params: Option<(u32, u32)> = None; // (sample_rate, channel_count)
    parser.set_required_presentations(&required_presentations);

    let input_path = args.input.clone();
    let decode_thread = thread::spawn(move || -> Result<()> {
        let mut frame_count: u64 = 0;
        let mut total_samples = 0u64;
        let mut frames_processed = 0;

        let mut input_reader = InputReader::new(&input_path)?;

        let process_frames = |extractor: &mut Extractor,
                              parser: &mut Parser,
                              decoder: &mut Decoder,
                              frames_processed: &mut u64,
                              frame_count: &mut u64,
                              total_samples: &mut u64|
         -> Result<bool> {
            loop {
                match extractor.next() {
                    Some(Ok(frame)) => {
                        *frames_processed += 1;
                        pb_update!(pb_clone, set_position(*frames_processed));
                        *frame_count += 1;

                        match parser.parse(&frame) {
                            Ok(access_unit) => {
                                match decoder
                                    .decode_presentation(&access_unit, presentation as usize)
                                {
                                    Ok(decoded) => {
                                        *total_samples += decoded.sample_length as u64;
                                        if tx.send(Ok(decoded)).is_err() {
                                            return Ok(true);
                                        }
                                    }
                                    Err(e) => {
                                        log::error!("Decode error at frame {}: {e}", *frame_count);
                                        if strict_mode {
                                            let _ = tx.send(Err(e));
                                            return Ok(true);
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                log::error!("Parse error at frame {}: {e}", *frame_count);
                                if strict_mode {
                                    let _ = tx.send(Err(e));
                                    return Ok(true);
                                }
                            }
                        }
                    }
                    Some(Err(ref e))
                        if matches!(e, truehd::utils::errors::ExtractError::InsufficientData) =>
                    {
                        break;
                    }
                    Some(Err(_extract_error)) => {
                        pb_update!(pb_clone, set_message("processing (some extraction errors)"));
                    }
                    None => {
                        break;
                    }
                }
            }
            Ok(false)
        };

        input_reader.process_chunks(64 * 1024, |chunk| {
            extractor.push_bytes(chunk);

            let should_exit = process_frames(
                &mut extractor,
                &mut parser,
                &mut decoder,
                &mut frames_processed,
                &mut frame_count,
                &mut total_samples,
            )?;

            Ok(!should_exit) // Convert exit signal to continue signal
        })?;

        log::info!("Processing complete: {frame_count} frames, {total_samples} samples");
        Ok(())
    });

    let mut damf_metadata_file_writer: Option<BufWriter<File>> = None;

    let mut has_atmos = false;
    let mut temp_oamd: Option<truehd::structs::oamd::ObjectAudioMetadataPayload> = None; // Store one temp OAMD for header generation

    let mut prev_events = Vec::new();
    let mut decoded_frames = 0;
    let mut decoded_samples = 0;
    let mut final_sample_rate = 48000u32; // Default fallback
    let start_time = std::time::Instant::now();

    while let Ok(result) = rx.recv() {
        match result {
            Ok(decoded) => {
                let sample_rate = decoded.sampling_frequency;
                let sample_pos = decoded_samples;
                let channel_count = decoded.channel_count;

                decoded_samples += decoded.sample_length as u64;
                decoded_frames += 1u64;
                final_sample_rate = sample_rate;

                for oamd in decoded.oamd {
                    has_atmos = true;

                    if temp_oamd.is_none() {
                        temp_oamd = Some(oamd.clone());
                    }

                    let mut conf = Configuration::with_oamd_payload(&oamd, sample_rate, sample_pos);

                    let (events_diff, remove_header) = if !prev_events.is_empty() {
                        (
                            Event::compare_event_vectors(&prev_events, &conf.events),
                            true,
                        )
                    } else {
                        (conf.events.clone(), false)
                    };

                    prev_events = conf.events.clone();
                    conf.events = events_diff;
                    let oamd_str = conf.serialize_events(remove_header);

                    if let Some(ref base_path) = base_path {
                        if damf_metadata_file_writer.is_none() {
                            let (_, metadata_path) =
                                create_output_paths(base_path, args.format, has_atmos);
                            if !metadata_path.as_os_str().is_empty() {
                                log::info!("Creating metadata file: {}", metadata_path.display());
                                damf_metadata_file_writer =
                                    Some(BufWriter::new(File::create(metadata_path)?));
                            }
                        }
                        if let Some(ref mut writer) = damf_metadata_file_writer {
                            write!(writer, "{oamd_str}")?;
                        }
                    }
                }

                if let Some(ref base_path) = base_path {
                    if audio_writer.is_none() {
                        let effective_format = if has_atmos && args.format == AudioFormat::Pcm {
                            log::info!("Atmos audio detected - forcing CAF format instead of PCM");
                            AudioFormat::Caf
                        } else {
                            args.format
                        };

                        let (audio_path, _) =
                            create_output_paths(base_path, effective_format, has_atmos);
                        log::info!("Creating audio file: {}", audio_path.display());

                        // Track the original path and whether it was created without Atmos detection
                        original_audio_path = Some(audio_path.clone());
                        audio_created_without_atmos = !has_atmos;
                        original_format_used = Some(effective_format);
                        audio_params = Some((sample_rate, channel_count as u32));

                        match effective_format {
                            AudioFormat::Caf => {
                                let mut caf_writer =
                                    CAFWriter::new(BufWriter::new(File::create(audio_path)?));
                                caf_writer.configure_audio_format(
                                    sample_rate,
                                    channel_count as u32,
                                    24,
                                )?;
                                caf_writer.write_header()?;
                                audio_writer = Some(AudioWriter::Caf(caf_writer));
                            }
                            AudioFormat::Pcm => {
                                let pcm_writer = BufWriter::new(File::create(audio_path)?);
                                audio_writer = Some(AudioWriter::Pcm(pcm_writer));
                            }
                        }
                    }
                }

                if let Some(ref mut writer) = audio_writer {
                    match writer {
                        AudioWriter::Pcm(pcm_writer) => {
                            for sample_idx in 0..decoded.sample_length {
                                for ch in 0..channel_count {
                                    let sample = decoded.pcm_data[sample_idx][ch];
                                    let bytes = sample.to_le_bytes();
                                    pcm_writer.write_all(&bytes[..3])?;
                                }
                            }
                        }
                        AudioWriter::Caf(caf_writer) => {
                            let mut samples =
                                Vec::with_capacity(decoded.sample_length * channel_count);
                            for sample_idx in 0..decoded.sample_length {
                                for ch in 0..channel_count {
                                    let sample = decoded.pcm_data[sample_idx][ch];
                                    samples.push(sample);
                                }
                            }

                            caf_writer.write_pcm_24bit_as_packed(&samples)?;
                        }
                    }
                }

                if decoded_frames.is_multiple_of(30) {
                    let elapsed = start_time.elapsed();
                    let audio_duration_secs = decoded_samples as f64 / sample_rate as f64;
                    let realtime_multiplier = audio_duration_secs / elapsed.as_secs_f64();

                    let time_str = time_str(audio_duration_secs);

                    if let Some(ref pb) = pb {
                        pb.set_message(format!(
                            "speed: {realtime_multiplier:.1}x | timestamp: {time_str}"
                        ));
                    }
                }
            }
            Err(e) => {
                pb_update!(pb, finish_with_message("decode failed"));
                return Err(e);
            }
        }
    }

    // Finalize output writers
    if let Some(ref mut writer) = audio_writer {
        match writer {
            AudioWriter::Caf(caf_writer) => {
                caf_writer.finish()?;
            }
            AudioWriter::Pcm(pcm_writer) => {
                pcm_writer.flush()?;
            }
        }
    }

    if let Some(ref mut writer) = damf_metadata_file_writer {
        writer.flush()?;
    }

    if let Some(ref base_path) = base_path {
        if has_atmos {
            if let Some(temp_oamd_data) = temp_oamd {
                let header_path = {
                    let mut path = base_path.clone();
                    let new_name =
                        format!("{}.atmos", base_path.file_name().unwrap().to_string_lossy());
                    path.set_file_name(new_name);
                    path
                };

                log::info!("Creating DAMF header file: {}", header_path.display());
                let mut header_writer = BufWriter::new(File::create(header_path)?);

                let damf_data = Data::with_oamd_payload(&temp_oamd_data, base_path);
                let header_str = &damf_data.serialize_damf();
                write!(header_writer, "{header_str}")?;
                header_writer.flush()?;
            }
        }

        // Handle audio file conversion/renaming if Atmos was detected after initial creation
        if audio_created_without_atmos && has_atmos {
            if let (Some(original_path), Some(original_format), Some((sample_rate, channel_count))) = 
                (&original_audio_path, &original_format_used, &audio_params) {
                
                // If original format was PCM, wrap it with CAF header first
                if *original_format == AudioFormat::Pcm {
                    log::info!("Wrapping PCM file with CAF header for Atmos: {}", original_path.display());
                    
                    if let Err(e) = wrap_pcm_file_with_caf_header(
                        original_path, 
                        *sample_rate as f64, 
                        *channel_count, 
                        24 // TrueHD is always 24-bit
                    ) {
                        log::warn!("Failed to wrap PCM file with CAF header: {e}");
                    } else {
                        log::info!("Successfully converted PCM to CAF format");
                    }
                }
                
                // Now rename to .atmos.audio regardless of original format
                let (new_audio_path, _) = create_output_paths(base_path, *original_format, true);
                
                if original_path != &new_audio_path {
                    log::info!(
                        "Renaming audio file to: {}",
                        new_audio_path.display()
                    );

                    if let Err(e) = std::fs::rename(original_path, &new_audio_path) {
                        log::warn!("Failed to rename audio file: {e}");
                    }
                }
            }
        }
    }

    // Wait for decode thread to complete
    match decode_thread.join() {
        Ok(Ok(())) => {
            if let Some(ref pb) = pb {
                let elapsed = start_time.elapsed();
                let audio_duration_secs = decoded_samples as f64 / final_sample_rate as f64;
                let realtime_multiplier = audio_duration_secs / elapsed.as_secs_f64();

                let final_time_str = time_str(audio_duration_secs);

                if total_frames.is_some() {
                    pb.set_style(ProgressStyle::with_template(
                        "{bar:40.cyan/blue} {pos}/{len} frames ({percent}%)\n{msg} | elapsed: {elapsed_precise}",
                    ).unwrap_or_else(|_| ProgressStyle::default_bar()));
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
            log::info!("Decoding completed successfully");
        }
        Ok(Err(e)) => {
            pb_update!(pb, finish_with_message("decode failed"));
            return Err(e);
        }
        Err(_) => {
            pb_update!(pb, finish_with_message("decode thread panicked"));
            return Err(anyhow::anyhow!("Decode thread panicked"));
        }
    }

    Ok(())
}
