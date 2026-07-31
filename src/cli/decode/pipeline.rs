use super::handler::DecodeHandler;
use crate::cli::command::{AudioFormat, DecodeArgs};
use crate::input::InputReader;
use anyhow::Result;
use crossbeam::channel::{Receiver, Sender, bounded};
use crossbeam::thread::scope;
use indicatif::ProgressBar;
use log::{Level, error, info, warn};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use truehd::process::MAX_PRESENTATIONS;
use truehd::process::decode::Decoder;
use truehd::process::extract::{Extractor, Frame};
use truehd::process::parse::Parser;
use truehd::structs::access_unit::AccessUnit;

impl std::fmt::Display for PipelineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PipelineError::Input(e) => write!(f, "Input error: {e}"),
            PipelineError::Parse(e) => write!(f, "Parse error: {e}"),
            PipelineError::Decode(e) => write!(f, "Decode error: {e}"),
            PipelineError::Write(e) => write!(f, "Write error: {e}"),
        }
    }
}

impl std::error::Error for PipelineError {}

#[derive(Clone)]
pub struct WriterState {
    fail_level: Level,
}

// Shared error type for cross-thread communication
#[derive(Debug)]
pub enum PipelineError {
    Input(anyhow::Error),
    Parse(anyhow::Error),
    Decode(anyhow::Error),
    Write(anyhow::Error),
}

pub fn run_threaded_pipeline(
    args: &DecodeArgs,
    format: AudioFormat,
    fail_level: Level,
    strict_mode: bool,
    pb: Option<&ProgressBar>,
    progress_counter: Arc<AtomicU64>,
) -> Result<DecodeHandler, PipelineError> {
    // Bounded channels with appropriate buffer sizes for backpressure
    let (tx_extract, rx_extract) = bounded::<(u64, Frame)>(32);
    let (tx_parse, rx_parse) = bounded::<(u64, AccessUnit, bool)>(32);
    let (tx_decode, rx_decode) = bounded::<(u64, truehd::process::decode::DecodedAccessUnit)>(1);
    let (tx_error, rx_error) = bounded::<PipelineError>(1);

    let mut required_presentations = [false; MAX_PRESENTATIONS];
    required_presentations[..=args.presentation as usize]
        .iter_mut()
        .for_each(|p| *p = true);

    let state = WriterState { fail_level };
    let mut handler = DecodeHandler::new(
        args.output_path.clone(),
        format,
        args.bed_conform,
        args.warp_mode,
        args.probe_range,
    );
    let start_time = Instant::now();
    handler.start_time = start_time;

    scope(|s| {
        // Clone necessary data for threads
        let input_path = args.input.clone();
        let presentation = args.presentation;
        let tx_error_extract = tx_error.clone();
        let tx_error_parse = tx_error.clone();
        let tx_error_decode = tx_error.clone();

        // Extractor thread: Input reading and frame extraction
        s.spawn(move |_| {
            let result = run_extractor_thread(input_path, tx_extract, strict_mode);
            if let Err(e) = result {
                let _ = tx_error_extract.send(PipelineError::Input(e));
            }
        });

        // Parser thread: Frame parsing and segment detection
        s.spawn(move |_| {
            let result = run_parser_thread(
                rx_extract,
                tx_parse,
                fail_level,
                required_presentations,
                strict_mode,
            );
            if let Err(e) = result {
                let _ = tx_error_parse.send(PipelineError::Parse(e));
            }
        });

        // Decoder thread: Access unit decoding
        s.spawn(move |_| {
            let result =
                run_decoder_thread(rx_parse, tx_decode, fail_level, presentation, strict_mode);
            if let Err(e) = result {
                let _ = tx_error_decode.send(PipelineError::Decode(e));
            }
        });

        // Main thread: Writing and progress tracking with proper ordering
        let write_result = run_writer_main(
            rx_decode,
            rx_error,
            &mut handler,
            &state,
            pb,
            progress_counter,
            strict_mode,
        );

        match write_result {
            Ok(_) => Ok(handler),
            Err(e) => Err(PipelineError::Write(e)),
        }
    })
    .unwrap() // scope().unwrap() is safe here as we handle errors internally
}

fn run_extractor_thread(
    input_path: PathBuf,
    tx_extract: Sender<(u64, Frame)>,
    strict_mode: bool,
) -> Result<()> {
    let mut extractor = Extractor::default();
    let mut frame_index = 0u64;
    let mut input_reader = InputReader::new(&input_path)?;

    input_reader.process_chunks(64 * 1024, |chunk| {
        extractor.push_bytes(chunk);

        for frame_result in extractor.by_ref() {
            match frame_result {
                Ok(frame) => {
                    if tx_extract.send((frame_index, frame)).is_err() {
                        // Channel closed, exit gracefully
                        return Ok(false);
                    }
                    frame_index += 1;
                }
                Err(truehd::utils::errors::ExtractError::InsufficientData) => break,
                Err(e) => {
                    if strict_mode {
                        return Err(anyhow::anyhow!("Extract error: {}", e));
                    }
                    warn!("Extract error: {e}");
                }
            }
        }
        Ok(true)
    })?;

    Ok(())
}

fn run_parser_thread(
    rx_extract: Receiver<(u64, Frame)>,
    tx_parse: Sender<(u64, AccessUnit, bool)>,
    fail_level: Level,
    required_presentations: [bool; MAX_PRESENTATIONS],
    strict_mode: bool,
) -> Result<()> {
    let mut parser = Parser::default();
    parser.set_fail_level(fail_level);
    parser.set_required_presentations(&required_presentations);
    let mut segment_detector = SegmentDetector::new();

    for (index, frame_bytes) in rx_extract {
        match parser.parse(&frame_bytes) {
            Ok(au) => {
                let stream_changed = segment_detector.check(&au);
                if tx_parse.send((index, au, stream_changed)).is_err() {
                    // Channel closed, exit gracefully
                    break;
                }
            }
            Err(e) => {
                if strict_mode {
                    return Err(anyhow::anyhow!("Parse error at frame {}: {}", index, e));
                }
                warn!("Parse error at frame {index}: {e}");
            }
        }
    }

    Ok(())
}

fn run_decoder_thread(
    rx_parse: Receiver<(u64, AccessUnit, bool)>,
    tx_decode: Sender<(u64, truehd::process::decode::DecodedAccessUnit)>,
    fail_level: Level,
    presentation: u8,
    strict_mode: bool,
) -> Result<()> {
    let mut decoder = Decoder::default();
    decoder.set_fail_level(fail_level);

    for (index, au, stream_changed) in rx_parse {
        match decoder.decode_presentation(&au, presentation as usize) {
            Ok(mut decoded) => {
                if stream_changed {
                    decoded.substream_info_changed = true;
                }
                if tx_decode.send((index, decoded)).is_err() {
                    // Channel closed, exit gracefully
                    break;
                }
            }
            Err(e) => {
                if strict_mode {
                    return Err(anyhow::anyhow!("Decode error at frame {}: {}", index, e));
                }
                warn!("Decode error at frame {index}: {e}");
            }
        }
    }

    Ok(())
}

fn run_writer_main(
    rx_decode: Receiver<(u64, truehd::process::decode::DecodedAccessUnit)>,
    rx_error: Receiver<PipelineError>,
    handler: &mut DecodeHandler,
    state: &WriterState,
    pb: Option<&ProgressBar>,
    progress_counter: Arc<AtomicU64>,
    strict_mode: bool,
) -> Result<()> {
    let mut next_index = 0u64;
    let mut reorder_buffer = BTreeMap::new();
    const MAX_REORDER_SIZE: usize = 64;

    loop {
        // Check for errors from other threads first
        if let Ok(error) = rx_error.try_recv() {
            if strict_mode {
                return Err(anyhow::anyhow!("Pipeline error: {}", error));
            } else {
                error!("Pipeline error: {error}");
                // Continue processing in non-strict mode
            }
        }

        // Process incoming decoded frames
        match rx_decode.try_recv() {
            Ok((index, decoded)) => {
                // Check reorder buffer size limit
                if reorder_buffer.len() >= MAX_REORDER_SIZE {
                    warn!("Reorder buffer limit reached, forcing drain");
                    // Force processing of oldest frames
                    while reorder_buffer.len() >= MAX_REORDER_SIZE / 2 {
                        if let Some((_, frame)) = reorder_buffer.pop_first() {
                            process_frame(handler, frame, state, pb, &progress_counter)?;
                            next_index += 1;
                        } else {
                            break;
                        }
                    }
                }

                reorder_buffer.insert(index, decoded);

                // Process frames in order
                while let Some(frame) = reorder_buffer.remove(&next_index) {
                    process_frame(handler, frame, state, pb, &progress_counter)?;
                    next_index += 1;
                }
            }
            Err(crossbeam::channel::TryRecvError::Empty) => {
                // No data available, check if all senders are disconnected
                if rx_decode.is_empty() && rx_decode.is_empty() {
                    // Channel might be closed, try blocking receive with timeout
                    match rx_decode.recv_timeout(std::time::Duration::from_millis(100)) {
                        Ok((index, decoded)) => {
                            reorder_buffer.insert(index, decoded);
                            while let Some(frame) = reorder_buffer.remove(&next_index) {
                                process_frame(handler, frame, state, pb, &progress_counter)?;
                                next_index += 1;
                            }
                        }
                        Err(crossbeam::channel::RecvTimeoutError::Disconnected) => {
                            // All senders disconnected, drain remaining frames
                            break;
                        }
                        Err(crossbeam::channel::RecvTimeoutError::Timeout) => {
                            // Continue the loop to check for errors
                            continue;
                        }
                    }
                }
            }
            Err(crossbeam::channel::TryRecvError::Disconnected) => {
                // All senders disconnected, drain remaining frames
                break;
            }
        }
    }

    // Drain any remaining frames in order
    for (_, frame) in reorder_buffer {
        process_frame(handler, frame, state, pb, &progress_counter)?;
    }

    Ok(())
}

fn process_frame(
    handler: &mut DecodeHandler,
    decoded: truehd::process::decode::DecodedAccessUnit,
    state: &WriterState,
    pb: Option<&ProgressBar>,
    progress_counter: &Arc<AtomicU64>,
) -> Result<()> {
    if decoded.substream_info_changed {
        handler.handle_stream_restart(&decoded, state)?;
    }

    handler.handle_decoded_frame(decoded, &pb.cloned(), handler.start_time)?;

    let count = progress_counter.fetch_add(1, Ordering::Relaxed) + 1;
    if let Some(pb) = pb {
        pb.set_position(count);
    }

    Ok(())
}

struct SegmentDetector {
    current_substream_info: Option<u8>,
    current_extended_substream_info: Option<u8>,
}

impl SegmentDetector {
    fn new() -> Self {
        Self {
            current_substream_info: None,
            current_extended_substream_info: None,
        }
    }

    fn check(&mut self, access_unit: &AccessUnit) -> bool {
        if let Some(major_sync) = &access_unit.major_sync_info {
            let substream_changed = match self.current_substream_info {
                Some(current) if current != major_sync.substream_info => {
                    info!(
                        "substream_info changed: {:#04X} -> {:#04X}",
                        current, major_sync.substream_info
                    );
                    true
                }
                None => {
                    self.current_substream_info = Some(major_sync.substream_info);
                    false
                }
                _ => false,
            };

            let extended_changed = match self.current_extended_substream_info {
                Some(current) if current != major_sync.extended_substream_info => {
                    info!(
                        "extended_substream_info changed: {:#04X} -> {:#04X}",
                        current, major_sync.extended_substream_info
                    );
                    true
                }
                None => {
                    self.current_extended_substream_info = Some(major_sync.extended_substream_info);
                    false
                }
                _ => false,
            };

            self.current_substream_info = Some(major_sync.substream_info);
            self.current_extended_substream_info = Some(major_sync.extended_substream_info);

            substream_changed || extended_changed
        } else {
            false
        }
    }
}
