use super::handler::DecodeHandler;
use crate::cli::command::{AudioFormat, DecodeArgs, WarpMode};
use crate::input::InputReader;
use anyhow::{Result, anyhow};
use crossbeam::channel::{Receiver, Sender, bounded};
use crossbeam::thread::scope;
use indicatif::ProgressBar;
use log::{Level, debug, info, warn};
use std::path::{Path, PathBuf};
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

/// Aggregated results for the final report.
pub struct DecodeSummary {
    pub decoded_frames: u64,
    pub total_samples: u64,
    pub final_sample_rate: u32,
    pub start_time: Instant,
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
    // One entry per presentation; substreams are decoded once and shared
    Decoded(Box<[Option<DecodedAccessUnit>; MAX_PRESENTATIONS]>),
    Fatal(PipelineError),
}

pub fn run_threaded_pipeline(
    args: &DecodeArgs,
    fail_level: Level,
    strict_mode: bool,
    pb: Option<&ProgressBar>,
    progress_counter: Arc<AtomicU64>,
) -> Result<DecodeSummary, PipelineError> {
    // Payloads are boxed, so capacity is a plain backpressure knob.
    let (tx_extract, rx_extract) = bounded::<ExtractMsg>(32);
    let (tx_parse, rx_parse) = bounded::<ParseMsg>(32);
    let (tx_decode, rx_decode) = bounded::<DecodeMsg>(32);

    let required_presentations = args.presentation.to_required_presentations();

    let mut outputs = PresentationOutputs {
        handlers: core::array::from_fn(|_| None),
        base_path: args.output_path.clone(),
        requested_format: args.format,
        single_output: args.presentation.is_single_output(),
        bed_conform: args.bed_conform,
        metadata_only: args.metadata_only,
        warp_mode: args.warp_mode,
        probe_range: args.probe_range,
        start_time: Instant::now(),
    };

    scope(|s| {
        let input_path = args.input.clone();

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
            run_decoder_thread(
                rx_parse,
                tx_decode,
                fail_level,
                required_presentations,
                strict_mode,
            )
        });

        match run_writer_main(rx_decode, &mut outputs, pb, progress_counter) {
            Ok(()) => Ok(outputs.summary()),
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
    required_presentations: [bool; MAX_PRESENTATIONS],
    strict_mode: bool,
) {
    let mut decoder = Decoder::default();
    decoder.set_fail_level(fail_level);
    let mut resyncing = false;

    for msg in rx {
        match msg {
            ParseMsg::Au(index, au, stream_changed) => {
                match decoder.decode_presentations(&au, &required_presentations) {
                    Ok(mut decoded) => {
                        if resyncing {
                            info!("Recovered decoding at frame {index}");
                            resyncing = false;
                        }
                        if stream_changed {
                            for slot in decoded.iter_mut().flatten() {
                                slot.substream_info_changed = true;
                            }
                        }
                        if tx.send(DecodeMsg::Decoded(decoded)).is_err() {
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

/// Per-presentation output handlers, created lazily when a presentation
/// first produces audio (the decoder may remap unavailable presentations
/// to lower indices, so which slots fill up is only known at runtime).
struct PresentationOutputs {
    handlers: [Option<DecodeHandler>; MAX_PRESENTATIONS],
    base_path: Option<PathBuf>,
    requested_format: AudioFormat,
    single_output: bool,
    bed_conform: bool,
    metadata_only: bool,
    warp_mode: Option<WarpMode>,
    probe_range: u64,
    start_time: Instant,
}

impl PresentationOutputs {
    fn handler_for(&mut self, slot: usize) -> &mut DecodeHandler {
        if self.handlers[slot].is_none() {
            let format = if slot == 3 {
                if self.requested_format != AudioFormat::Caf {
                    info!(
                        "Presentation 3 output uses CAF format, ignoring --format {:?}",
                        self.requested_format
                    );
                }
                AudioFormat::Caf
            } else {
                self.requested_format
            };

            let path = self.base_path.as_ref().map(|base| {
                if self.single_output {
                    base.clone()
                } else {
                    path_with_presentation_suffix(base, slot)
                }
            });

            if let Some(ref p) = path {
                info!("Presentation {slot} output: {}", p.display());
            }

            // Bed conformance only applies to the object audio presentation
            let mut handler = DecodeHandler::new(
                path,
                format,
                self.bed_conform && slot == 3,
                self.metadata_only,
                self.warp_mode,
                self.probe_range,
            );
            handler.start_time = self.start_time;
            self.handlers[slot] = Some(handler);
        }

        self.handlers[slot].as_mut().unwrap()
    }

    fn finalize_all(&mut self) -> Result<()> {
        for handler in self.handlers.iter_mut().flatten() {
            handler.finalize()?;
        }
        Ok(())
    }

    fn finalize_best_effort(&mut self) {
        // Patch output headers so already-written audio stays playable
        if let Err(e) = self.finalize_all() {
            warn!("Failed to finalize output files after error: {e}");
        }
    }

    fn summary(&self) -> DecodeSummary {
        let mut summary = DecodeSummary {
            decoded_frames: 0,
            total_samples: 0,
            final_sample_rate: 48000,
            start_time: self.start_time,
        };
        for handler in self.handlers.iter().flatten() {
            summary.decoded_frames = summary.decoded_frames.max(handler.decoded_frames);
            summary.total_samples = summary.total_samples.max(handler.total_samples);
            summary.final_sample_rate = handler.final_sample_rate;
        }
        summary
    }
}

fn path_with_presentation_suffix(base: &Path, slot: usize) -> PathBuf {
    let mut path = base.as_os_str().to_owned();
    path.push(format!("_p{slot}"));
    PathBuf::from(path)
}

fn run_writer_main(
    rx: Receiver<DecodeMsg>,
    outputs: &mut PresentationOutputs,
    pb: Option<&ProgressBar>,
    progress_counter: Arc<AtomicU64>,
) -> Result<(), PipelineError> {
    for msg in rx {
        match msg {
            DecodeMsg::Decoded(mut slots) => {
                for slot in 0..MAX_PRESENTATIONS {
                    let Some(decoded) = slots[slot].take() else {
                        continue;
                    };

                    let handler = outputs.handler_for(slot);
                    if let Err(e) = process_frame(handler, decoded, pb) {
                        outputs.finalize_best_effort();
                        return Err(PipelineError::Write(e));
                    }
                }

                let count = progress_counter.fetch_add(1, Ordering::Relaxed) + 1;
                if let Some(pb) = pb {
                    pb.set_position(count);
                }
            }
            DecodeMsg::Fatal(e) => {
                outputs.finalize_best_effort();
                return Err(e);
            }
        }
    }

    outputs
        .finalize_all()
        .map_err(|e| PipelineError::Write(e.context("finalizing output files")))
}

fn process_frame(
    handler: &mut DecodeHandler,
    decoded: DecodedAccessUnit,
    pb: Option<&ProgressBar>,
) -> Result<()> {
    if decoded.substream_info_changed {
        handler.handle_stream_restart()?;
    }

    handler.handle_decoded_frame(decoded, &pb.cloned(), handler.start_time)?;

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
