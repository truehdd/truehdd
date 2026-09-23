use super::handler::DecodeHandler;
use crate::cli::command::{AudioFormat, DecodeArgs, FrameRate, WarpMode};
use crate::cli::evo::{EvoKey, EvoVerifier, hex};
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
use truehd::structs::sync::{BASE_SAMPLES_PER_AU, BASE_SAMPLING_RATE_CD};

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
    pub concealed_frames: u64,
    pub branches: u64,
    pub invalid_branches: u64,
    pub evo_checked: u64,
    pub evo_failed: u64,
    pub decoded_frames: u64,
    pub total_samples: u64,
    pub final_sample_rate: u32,
    pub start_time: Instant,
    pub presentations: Vec<PresentationSummary>,
}

/// Stream statistics the parser thread reports back while it runs.
struct ParserCounters {
    branches: Arc<AtomicU64>,
    invalid_branches: Arc<AtomicU64>,
    evo_checked: Arc<AtomicU64>,
    evo_failed: Arc<AtomicU64>,
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
    // A frame the parser could not read; the decoder writes silence in its place.
    Lost(u64),
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
    let concealed_frames = Arc::new(AtomicU64::new(0));
    let branches = Arc::new(AtomicU64::new(0));
    let invalid_branches = Arc::new(AtomicU64::new(0));
    let evo_checked = Arc::new(AtomicU64::new(0));
    let evo_failed = Arc::new(AtomicU64::new(0));

    let mut outputs = PresentationOutputs {
        handlers: core::array::from_fn(|_| None),
        base_path: args.output_path.clone(),
        requested_format: args.format,
        single_output: args.presentation.is_single_output(),
        bed_conform: args.bed_conform,
        metadata_only: args.metadata_only,
        warp_mode: args.warp_mode,
        frame_rate: args.frame_rate,
        probe_range: args.probe_range,
        start_time: Instant::now(),
    };

    scope(|s| {
        let input_path = args.input.clone();

        let skipped = Arc::clone(&skipped_frames);
        s.spawn(move |_| run_extractor_thread(input_path, tx_extract, strict_mode, skipped));

        let counters = ParserCounters {
            branches: Arc::clone(&branches),
            invalid_branches: Arc::clone(&invalid_branches),
            evo_checked: Arc::clone(&evo_checked),
            evo_failed: Arc::clone(&evo_failed),
        };
        let evo_key = args.evo_key.clone();
        s.spawn(move |_| {
            run_parser_thread(
                rx_extract,
                tx_parse,
                fail_level,
                required_presentations,
                strict_mode,
                counters,
                evo_key,
            )
        });

        let concealment = Concealment::new(Arc::clone(&concealed_frames));
        s.spawn(move |_| {
            run_decoder_thread(
                rx_parse,
                tx_decode,
                fail_level,
                required_presentations,
                strict_mode,
                concealment,
            )
        });

        match run_writer_main(rx_decode, &mut outputs, pb, progress_counter) {
            Ok(()) if outputs.summary().decoded_frames == 0 => Err(PipelineError::Parse(anyhow!(
                "no access unit in the stream could be decoded"
            ))),
            Ok(()) => Ok(DecodeSummary {
                skipped_frames: skipped_frames.load(Ordering::Relaxed),
                concealed_frames: concealed_frames.load(Ordering::Relaxed),
                branches: branches.load(Ordering::Relaxed),
                invalid_branches: invalid_branches.load(Ordering::Relaxed),
                evo_checked: evo_checked.load(Ordering::Relaxed),
                evo_failed: evo_failed.load(Ordering::Relaxed),
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
            let _ = tx.send(ExtractMsg::Fatal(PipelineError::Parse(anyhow!(
                "{skipped} corrupt frame(s) skipped"
            ))));
            return Ok(false);
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
    counters: ParserCounters,
    evo_key: Option<EvoKey>,
) {
    let ParserCounters {
        branches,
        invalid_branches,
        evo_checked,
        evo_failed,
    } = counters;
    let mut parser = Parser::default();
    parser.set_fail_level(fail_level);
    parser.set_required_presentations(&required_presentations);
    let mut segment_detector = SegmentDetector::new();
    let mut resyncing = false;
    let mut verifier = evo_key.map(EvoVerifier::new);

    for msg in rx {
        match msg {
            ExtractMsg::Frame(index, frame) => match parser.parse(&frame) {
                Ok(au) => {
                    if resyncing {
                        info!("Recovered parsing at frame {index}");
                        resyncing = false;
                    }
                    if let Some(verifier) = &mut verifier {
                        let status = verifier.check(&au, frame.as_ref());
                        evo_checked.store(verifier.checked() as u64, Ordering::Relaxed);
                        evo_failed.store(verifier.failed() as u64, Ordering::Relaxed);

                        if let (Some(expected), Some(actual)) = (status.expected(), status.actual())
                        {
                            let message = format!(
                                "Evolution protection mismatch at frame {index}: expected {}, read {}",
                                hex(expected),
                                hex(actual)
                            );
                            if strict_mode {
                                let _ = tx
                                    .send(ParseMsg::Fatal(PipelineError::Parse(anyhow!(message))));
                                return;
                            }
                            warn!("{message}");
                        }
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
                    if tx.send(ParseMsg::Lost(index)).is_err() {
                        return;
                    }
                }
            },
            ExtractMsg::Fatal(e) => {
                let _ = tx.send(ParseMsg::Fatal(e));
                return;
            }
        }
    }

    if let Some(verifier) = &verifier
        && verifier.secondary_seen()
    {
        debug!("Secondary Evolution protection words were present but are not verified");
    }
}

fn run_decoder_thread(
    rx: Receiver<ParseMsg>,
    tx: Sender<DecodeMsg>,
    fail_level: Level,
    required_presentations: [bool; MAX_PRESENTATIONS],
    strict_mode: bool,
    mut concealment: Concealment,
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
                        concealment.end_run();
                        concealment.learn(&decoded);
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
                        if !concealment.conceal(index, &tx) {
                            return;
                        }
                    }
                }
            }
            ParseMsg::Resync => {
                decoder.reset_for_next_major_sync();
                resyncing = true;
            }
            ParseMsg::Lost(index) => {
                if !concealment.conceal(index, &tx) {
                    return;
                }
            }
            ParseMsg::Fatal(e) => {
                let _ = tx.send(DecodeMsg::Fatal(e));
                return;
            }
        }
    }

    concealment.end_run();
}

/// Silence written in place of access units that cannot be decoded.
struct Concealment {
    /// Silence in the layout last decoded.
    silence: Option<Box<[Option<DecodedAccessUnit>; MAX_PRESENTATIONS]>>,
    /// First frame and length of the run being concealed.
    run: Option<(u64, u64)>,
    concealed_frames: Arc<AtomicU64>,
}

impl Concealment {
    fn new(concealed_frames: Arc<AtomicU64>) -> Self {
        Self {
            silence: None,
            run: None,
            concealed_frames,
        }
    }

    /// Rebuilds the silence only when the layout changes, as a `DecodedAccessUnit` is sized
    /// for the largest access unit.
    fn learn(&mut self, decoded: &[Option<DecodedAccessUnit>; MAX_PRESENTATIONS]) {
        let same_layout = self.silence.as_deref().is_some_and(|silence| {
            silence
                .iter()
                .zip(decoded)
                .all(|(silence, decoded)| match (silence, decoded) {
                    (Some(silence), Some(decoded)) => {
                        silence.sampling_frequency == decoded.sampling_frequency
                            && silence.channel_count == decoded.channel_count
                            && silence.channel_labels == decoded.channel_labels
                    }
                    (None, None) => true,
                    _ => false,
                })
        });

        if !same_layout {
            self.silence = Some(Box::new(core::array::from_fn(|slot| {
                decoded[slot].as_ref().map(silence_like)
            })));
        }
    }

    /// Writes silence for frame `index`, or nothing before an access unit has been decoded
    /// to give the layout. Returns false once the writer has gone.
    fn conceal(&mut self, index: u64, tx: &Sender<DecodeMsg>) -> bool {
        let Some(silence) = &self.silence else {
            return true;
        };

        self.run = Some(match self.run {
            Some((first, frames)) => (first, frames + 1),
            None => (index, 1),
        });
        self.concealed_frames.fetch_add(1, Ordering::Relaxed);

        tx.send(DecodeMsg::Decoded(silence.clone())).is_ok()
    }

    fn end_run(&mut self) {
        let Some((first, frames)) = self.run.take() else {
            return;
        };

        let duration_ms = self
            .silence
            .as_deref()
            .and_then(|silence| silence.iter().flatten().next())
            .map_or(0.0, |silence| {
                (frames * silence.sample_length as u64) as f64 * 1000.0
                    / silence.sampling_frequency as f64
            });

        warn!(
            "Replaced {frames} undecodable frame(s) from frame {first} with {duration_ms:.1} ms of silence"
        );
    }
}

/// Silence in the layout of `decoded`, a whole access unit long even where a terminator
/// shortened `decoded`.
fn silence_like(decoded: &DecodedAccessUnit) -> DecodedAccessUnit {
    DecodedAccessUnit {
        sampling_frequency: decoded.sampling_frequency,
        // As `FormatInfo::samples_per_au`: 40 at 44.1/48 kHz, 80 at 88.2/96, 160 at 176.4/192.
        sample_length: (decoded.sampling_frequency / BASE_SAMPLING_RATE_CD) as usize
            * BASE_SAMPLES_PER_AU,
        channel_count: decoded.channel_count,
        pcm_data: [[0; 16]; 160],
        channel_labels: decoded.channel_labels.clone(),
        oamd: Vec::new(),
        is_duplicate: false,
        substream_info_changed: false,
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
    frame_rate: Option<FrameRate>,
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
                self.frame_rate,
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
            concealed_frames: 0,
            branches: 0,
            invalid_branches: 0,
            evo_checked: 0,
            evo_failed: 0,
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

            let format = match (handler.has_atmos(), index) {
                (true, _) => "damf",
                // Presentation 3 is written as CAF whatever was requested
                (false, 3) => "caf",
                (false, _) => match self.requested_format {
                    AudioFormat::Caf => "caf",
                    AudioFormat::Pcm => "pcm",
                    AudioFormat::W64 => "w64",
                },
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
            r#"{{"version":{},"input":{},"frames":{},"skippedFrames":{},"concealedFrames":{},"branches":{},"invalidBranches":{},"evoChecked":{},"evoFailed":{},"samples":{},"sampleRate":{},"presentations":[{}]}}"#,
            crate::json::escape(env!("CARGO_PKG_VERSION")),
            crate::json::escape(&input.to_string_lossy()),
            self.decoded_frames,
            self.skipped_frames,
            self.concealed_frames,
            self.branches,
            self.invalid_branches,
            self.evo_checked,
            self.evo_failed,
            self.total_samples,
            self.final_sample_rate,
            presentations.join(",")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Concealment, DecodeMsg, DecodeSummary, Extractor, ParseMsg, PipelineError,
        run_decoder_thread, run_threaded_pipeline,
    };
    use crate::cli::command::{AudioFormat, Cli, Commands, PresentationSelection};
    use clap::Parser;
    use crossbeam::channel::unbounded;
    use log::Level;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use tempfile::TempDir;
    use truehd::process::EXAMPLE_DATA;

    /// Six copies of the example stream's two access units.
    const ACCESS_UNITS: u64 = 12;

    fn run(bytes: &[u8], format: AudioFormat) -> (TempDir, Result<DecodeSummary, PipelineError>) {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("input.thd");
        std::fs::write(&input, bytes).unwrap();
        let output = dir.path().join("out");

        let cli = Cli::try_parse_from([
            "truehdd".as_ref(),
            "decode".as_ref(),
            input.as_os_str(),
            "--output-path".as_ref(),
            output.as_os_str(),
        ])
        .unwrap();
        let Commands::Decode(mut args) = cli.command else {
            unreachable!()
        };
        args.format = format;

        let result = run_threaded_pipeline(
            &args,
            Level::Error,
            false,
            None,
            Arc::new(AtomicU64::new(0)),
        );

        (dir, result)
    }

    fn decode(bytes: &[u8]) -> Result<u64, PipelineError> {
        run(bytes, AudioFormat::Caf)
            .1
            .map(|summary| summary.decoded_frames)
    }

    fn decode_to_pcm(bytes: &[u8]) -> (DecodeSummary, Vec<u8>) {
        let (_dir, result) = run(bytes, AudioFormat::Pcm);
        let summary = result.unwrap_or_else(|e| panic!("{e}"));
        let pcm = std::fs::read(&summary.presentations[0].files[0]).unwrap();

        (summary, pcm)
    }

    /// The example stream is a 16-byte timestamp and two access units. Repeating only the
    /// access units keeps a single timestamp at the start, so the extractor skips nothing.
    fn example_stream() -> Vec<u8> {
        let mut data = EXAMPLE_DATA.to_vec();

        for _ in 1..6 {
            data.extend_from_slice(&EXAMPLE_DATA[16..]);
        }

        data
    }

    #[test]
    fn a_stream_with_nothing_decodable_is_a_parse_failure() {
        for bytes in [&[][..], &[0x5a; 4096][..]] {
            assert!(matches!(decode(bytes), Err(PipelineError::Parse(_))));
        }
    }

    #[test]
    fn a_decodable_stream_still_succeeds() {
        assert_eq!(decode(truehd::process::EXAMPLE_DATA).unwrap(), 2);
    }

    /// A payload byte of the third access unit flipped. The parser rejects it and cannot
    /// read the fourth either, which carries no major sync, so two access units are lost
    /// and decoding resumes at the fifth.
    #[test]
    fn access_units_the_parser_rejects_are_written_as_silence() {
        let clean = example_stream();
        let mut lossy = clean.clone();
        // The third access unit starts where the first copy of the example stream ends.
        lossy[EXAMPLE_DATA.len() + 60] ^= 0xFF;

        let (reference, expected) = decode_to_pcm(&clean);
        let (summary, pcm) = decode_to_pcm(&lossy);

        assert_eq!(reference.skipped_frames, 0);
        assert_eq!(reference.concealed_frames, 0);
        assert_eq!(summary.concealed_frames, 2);
        assert_eq!(
            summary.decoded_frames, ACCESS_UNITS,
            "every access unit is written"
        );
        assert_eq!(
            summary.total_samples, reference.total_samples,
            "the output keeps the length of the stream"
        );
        assert_eq!(pcm.len(), expected.len());

        // Three bytes a sample, so an access unit is 120 bytes a channel.
        let frame = 120 * summary.presentations[0].channels.unwrap();
        let lost = 2 * frame..4 * frame;

        assert!(
            expected[lost.clone()].iter().any(|&byte| byte != 0),
            "the stream carries audio where the loss is, or silence proves nothing"
        );
        assert!(
            pcm[lost.clone()].iter().all(|&byte| byte == 0),
            "the loss is silent"
        );
        assert_eq!(pcm[..lost.start], expected[..lost.start], "before the loss");
        assert_eq!(pcm[lost.end..], expected[lost.end..], "after it, in place");
    }

    /// The third access unit gets a reserved sampling frequency code after parsing. The
    /// decoder rejects it and cannot decode the fourth, which carries no major sync.
    #[test]
    fn access_units_the_decoder_rejects_are_written_as_silence() {
        let mut extractor = Extractor::default();
        extractor.push_bytes(&example_stream());
        let mut parser = truehd::process::parse::Parser::default();
        let (tx_parse, rx_parse) = unbounded();

        for (index, frame) in extractor.map_while(Result::ok).enumerate() {
            let mut au = parser.parse(&frame).unwrap();
            if index == 2 {
                // `FormatInfo::map_sampling_freq` defines 0-2 and 8-10; 5 is reserved.
                au.major_sync_info
                    .as_mut()
                    .unwrap()
                    .format_info
                    .audio_sampling_frequency_1 = 5;
            }
            tx_parse
                .send(ParseMsg::Au(index as u64, Box::new(au), false))
                .unwrap();
        }
        drop(tx_parse);

        let (tx_decode, rx_decode) = unbounded();
        let concealed_frames = Arc::new(AtomicU64::new(0));
        run_decoder_thread(
            rx_parse,
            tx_decode,
            Level::Error,
            PresentationSelection::Max.to_required_presentations(),
            false,
            Concealment::new(Arc::clone(&concealed_frames)),
        );

        let written = rx_decode
            .iter()
            .filter(|msg| matches!(msg, DecodeMsg::Decoded(_)))
            .count() as u64;
        assert_eq!(written, ACCESS_UNITS, "every access unit is written");
        assert_eq!(concealed_frames.load(Ordering::Relaxed), 2);
    }
}
