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

impl PipelineError {
    pub fn exit_code(&self) -> i32 {
        match self {
            PipelineError::Input(_) => crate::exit::INPUT,
            PipelineError::Parse(_) => crate::exit::PARSE,
            PipelineError::Decode(_) => crate::exit::DECODE,
            PipelineError::Write(_) => crate::exit::WRITE,
        }
    }
}

/// Aggregated results for the final report.
pub struct DecodeSummary {
    pub skipped_frames: u64,
    pub branches: u64,
    pub invalid_branches: u64,
    pub decoded_frames: u64,
    pub total_samples: u64,
    pub final_sample_rate: u32,
    pub start_time: Instant,
    pub presentations: Vec<PresentationSummary>,
}

/// What a single presentation wrote.
pub struct PresentationSummary {
    pub index: usize,
    pub format: &'static str,
    pub channels: Option<usize>,
    pub files: Vec<PathBuf>,
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
    let skipped_frames = Arc::new(AtomicU64::new(0));
    let branches = Arc::new(AtomicU64::new(0));
    let invalid_branches = Arc::new(AtomicU64::new(0));

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

        let skipped = Arc::clone(&skipped_frames);
        s.spawn(move |_| run_extractor_thread(input_path, tx_extract, strict_mode, skipped));

        let branch_counter = Arc::clone(&branches);
        let invalid_branch_counter = Arc::clone(&invalid_branches);
        s.spawn(move |_| {
            run_parser_thread(
                rx_extract,
                tx_parse,
                fail_level,
                required_presentations,
                strict_mode,
                branch_counter,
                invalid_branch_counter,
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
            Ok(()) => Ok(DecodeSummary {
                skipped_frames: skipped_frames.load(Ordering::Relaxed),
                branches: branches.load(Ordering::Relaxed),
                invalid_branches: invalid_branches.load(Ordering::Relaxed),
                ..outputs.summary()
            }),
            Err(e) => Err(e),
        }
    })
    .unwrap() // scope().unwrap() is safe here as we handle errors internally
}

fn run_extractor_thread(
    input_path: PathBuf,
    tx: Sender<ExtractMsg>,
    strict_mode: bool,
    skipped_frames: Arc<AtomicU64>,
) {
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
        // The extractor resyncs over damaged frames on its own, so the count
        // is the only trace they leave
        let skipped = extractor.error_count() as u64;
        if skipped > skipped_frames.swap(skipped, Ordering::Relaxed) && strict_mode {
            return Err(anyhow!("{skipped} corrupt frame(s) skipped"));
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
    branches: Arc<AtomicU64>,
    invalid_branches: Arc<AtomicU64>,
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
                    // The flag also covers substream_info changes, which the
                    // segment detector reports separately
                    if au.has_valid_branch && !stream_changed {
                        branches.fetch_add(1, Ordering::Relaxed);
                    }
                    invalid_branches.store(parser.invalid_branches() as u64, Ordering::Relaxed);
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
            skipped_frames: 0,
            branches: 0,
            invalid_branches: 0,
            decoded_frames: 0,
            total_samples: 0,
            final_sample_rate: 48000,
            start_time: self.start_time,
            presentations: Vec::new(),
        };

        for (index, handler) in self.handlers.iter().enumerate() {
            let Some(handler) = handler else { continue };

            summary.decoded_frames = summary.decoded_frames.max(handler.decoded_frames);
            summary.total_samples = summary.total_samples.max(handler.total_samples);
            summary.final_sample_rate = handler.final_sample_rate;

            let format = if handler.has_atmos() {
                "damf"
            } else {
                match self.requested_format {
                    AudioFormat::Caf => "caf",
                    AudioFormat::Pcm => "pcm",
                    AudioFormat::W64 => "w64",
                }
            };

            summary.presentations.push(PresentationSummary {
                index,
                format,
                channels: handler.channel_count(),
                files: handler.produced_files().to_vec(),
            });
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

impl DecodeSummary {
    /// Result summary for callers that drive the CLI as a subprocess.
    pub fn to_json(&self, input: &Path) -> String {
        let presentations: Vec<String> = self
            .presentations
            .iter()
            .map(|presentation| {
                let files: Vec<String> = presentation
                    .files
                    .iter()
                    .map(|file| crate::json::escape(&file.to_string_lossy()))
                    .collect();
                let channels = match presentation.channels {
                    Some(channels) => channels.to_string(),
                    None => "null".to_string(),
                };
                format!(
                    r#"{{"index":{},"format":{},"channels":{},"files":[{}]}}"#,
                    presentation.index,
                    crate::json::escape(presentation.format),
                    channels,
                    files.join(",")
                )
            })
            .collect();

        format!(
            r#"{{"version":{},"input":{},"frames":{},"skippedFrames":{},"branches":{},"invalidBranches":{},"samples":{},"sampleRate":{},"presentations":[{}]}}"#,
            crate::json::escape(env!("CARGO_PKG_VERSION")),
            crate::json::escape(&input.to_string_lossy()),
            self.decoded_frames,
            self.skipped_frames,
            self.branches,
            self.invalid_branches,
            self.total_samples,
            self.final_sample_rate,
            presentations.join(",")
        )
    }
}
