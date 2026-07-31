use super::handler::DecodeHandler;
use crate::cli::command::{AudioFormat, DecodeArgs};
use crate::input::InputReader;
use anyhow::{Result, anyhow};
use crossbeam::channel::{Receiver, Sender, bounded};
use crossbeam::thread::scope;
use indicatif::ProgressBar;
use log::{Level, debug, info, warn};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use truehd::process::MAX_PRESENTATIONS;
use truehd::process::decode::{DecodedAccessUnit, Decoder};
use truehd::process::extract::{Extractor, Frame};
use truehd::process::parse::Parser;
use truehd::structs::access_unit::AccessUnit;

#[derive(Debug)]
pub enum PipelineError {
    Input(anyhow::Error),
    Parse(anyhow::Error),
    Decode(anyhow::Error),
    Write(anyhow::Error),
}

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

// Control messages travel in-band on the data channels, in order with the
// frames they relate to. A stage that hits a fatal error forwards it
// downstream and exits; the writer finalizes output files before reporting.
enum ExtractMsg {
    Frame(u64, Frame),
    Fatal(PipelineError),
}

enum ParseMsg {
    Au(u64, Box<AccessUnit>, bool),
    // Parser lost stream state; the decoder must reset in lockstep.
    Resync,
    Fatal(PipelineError),
}

enum DecodeMsg {
    Decoded(Box<DecodedAccessUnit>),
    Fatal(PipelineError),
}

pub fn run_threaded_pipeline(
    args: &DecodeArgs,
    format: AudioFormat,
    fail_level: Level,
    strict_mode: bool,
    pb: Option<&ProgressBar>,
    progress_counter: Arc<AtomicU64>,
) -> Result<DecodeHandler, PipelineError> {
    // Payloads are boxed, so capacity is a plain backpressure knob.
    let (tx_extract, rx_extract) = bounded::<ExtractMsg>(32);
    let (tx_parse, rx_parse) = bounded::<ParseMsg>(32);
    let (tx_decode, rx_decode) = bounded::<DecodeMsg>(32);

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
        let input_path = args.input.clone();
        let presentation = args.presentation;

        s.spawn(move |_| run_extractor_thread(input_path, tx_extract, strict_mode));

        s.spawn(move |_| {
            run_parser_thread(
                rx_extract,
                tx_parse,
                fail_level,
                required_presentations,
                strict_mode,
            )
        });

        s.spawn(move |_| {
            run_decoder_thread(rx_parse, tx_decode, fail_level, presentation, strict_mode)
        });

        match run_writer_main(rx_decode, &mut handler, &state, pb, progress_counter) {
            Ok(()) => Ok(handler),
            Err(e) => Err(e),
        }
    })
    .unwrap() // scope().unwrap() is safe here as we handle errors internally
}

fn run_extractor_thread(input_path: PathBuf, tx: Sender<ExtractMsg>, strict_mode: bool) {
    let mut extractor = Extractor::default();
    let mut frame_index = 0u64;

    let mut input_reader = match InputReader::new(&input_path) {
        Ok(reader) => reader,
        Err(e) => {
            let _ = tx.send(ExtractMsg::Fatal(PipelineError::Input(e)));
            return;
        }
    };

    let result = input_reader.process_chunks(64 * 1024, |chunk| {
        extractor.push_bytes(chunk);

        for frame_result in extractor.by_ref() {
            match frame_result {
                Ok(frame) => {
                    if tx.send(ExtractMsg::Frame(frame_index, frame)).is_err() {
                        // Downstream exited; stop reading
                        return Ok(false);
                    }
                    frame_index += 1;
                }
                Err(truehd::utils::errors::ExtractError::InsufficientData) => break,
                Err(e) => {
                    if strict_mode {
                        return Err(anyhow!("Extract error: {e}"));
                    }
                    // The extractor resyncs internally; the frame is lost
                    warn!("Extract error: {e}");
                }
            }
        }
        Ok(true)
    });

    if let Err(e) = result {
        let _ = tx.send(ExtractMsg::Fatal(PipelineError::Input(e)));
    }
}

fn run_parser_thread(
    rx: Receiver<ExtractMsg>,
    tx: Sender<ParseMsg>,
    fail_level: Level,
    required_presentations: [bool; MAX_PRESENTATIONS],
    strict_mode: bool,
) {
    let mut parser = Parser::default();
    parser.set_fail_level(fail_level);
    parser.set_required_presentations(&required_presentations);
    let mut segment_detector = SegmentDetector::new();
    let mut resyncing = false;

    for msg in rx {
        match msg {
            ExtractMsg::Frame(index, frame) => match parser.parse(&frame) {
                Ok(au) => {
                    if resyncing {
                        info!("Recovered parsing at frame {index}");
                        resyncing = false;
                    }
                    let stream_changed = segment_detector.check(&au);
                    if tx
                        .send(ParseMsg::Au(index, Box::new(au), stream_changed))
                        .is_err()
                    {
                        return;
                    }
                }
                Err(e) => {
                    if strict_mode {
                        let _ = tx.send(ParseMsg::Fatal(PipelineError::Parse(anyhow!(
                            "Parse error at frame {index}: {e}"
                        ))));
                        return;
                    }
                    if resyncing {
                        debug!("Skipping frame {index} until next major sync: {e}");
                    } else {
                        warn!("Parse error at frame {index}: {e}; resuming at next major sync");
                        parser.reset_for_next_major_sync();
                        resyncing = true;
                        if tx.send(ParseMsg::Resync).is_err() {
                            return;
                        }
                    }
                }
            },
            ExtractMsg::Fatal(e) => {
                let _ = tx.send(ParseMsg::Fatal(e));
                return;
            }
        }
    }
}

fn run_decoder_thread(
    rx: Receiver<ParseMsg>,
    tx: Sender<DecodeMsg>,
    fail_level: Level,
    presentation: u8,
    strict_mode: bool,
) {
    let mut decoder = Decoder::default();
    decoder.set_fail_level(fail_level);
    let mut resyncing = false;

    for msg in rx {
        match msg {
            ParseMsg::Au(index, au, stream_changed) => {
                match decoder.decode_presentation(&au, presentation as usize) {
                    Ok(mut decoded) => {
                        if resyncing {
                            info!("Recovered decoding at frame {index}");
                            resyncing = false;
                        }
                        if stream_changed {
                            decoded.substream_info_changed = true;
                        }
                        if tx.send(DecodeMsg::Decoded(Box::new(decoded))).is_err() {
                            return;
                        }
                    }
                    Err(e) => {
                        if strict_mode {
                            let _ = tx.send(DecodeMsg::Fatal(PipelineError::Decode(anyhow!(
                                "Decode error at frame {index}: {e}"
                            ))));
                            return;
                        }
                        if resyncing {
                            debug!("Skipping frame {index} until next major sync: {e}");
                        } else {
                            warn!(
                                "Decode error at frame {index}: {e}; resuming at next major sync"
                            );
                            decoder.reset_for_next_major_sync();
                            resyncing = true;
                        }
                    }
                }
            }
            ParseMsg::Resync => {
                decoder.reset_for_next_major_sync();
                resyncing = true;
            }
            ParseMsg::Fatal(e) => {
                let _ = tx.send(DecodeMsg::Fatal(e));
                return;
            }
        }
    }
}

fn run_writer_main(
    rx: Receiver<DecodeMsg>,
    handler: &mut DecodeHandler,
    state: &WriterState,
    pb: Option<&ProgressBar>,
    progress_counter: Arc<AtomicU64>,
) -> Result<(), PipelineError> {
    for msg in rx {
        match msg {
            DecodeMsg::Decoded(decoded) => {
                if let Err(e) = process_frame(handler, *decoded, state, pb, &progress_counter) {
                    finalize_on_error(handler);
                    return Err(PipelineError::Write(e));
                }
            }
            DecodeMsg::Fatal(e) => {
                finalize_on_error(handler);
                return Err(e);
            }
        }
    }

    Ok(())
}

// Patch output headers so already-written audio stays playable
fn finalize_on_error(handler: &mut DecodeHandler) {
    if let Err(e) = handler.finalize() {
        warn!("Failed to finalize output files after error: {e}");
    }
}

fn process_frame(
    handler: &mut DecodeHandler,
    decoded: DecodedAccessUnit,
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
