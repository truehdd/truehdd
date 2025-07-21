use anyhow::Result;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use log::Level;

use super::command::{Cli, InfoArgs};
use crate::input::InputReader;
use crate::timestamp::time_str;
use truehd::process::{
    PresentationMap, PresentationType,
    extract::{Extractor, Frame},
    parse::Parser,
};
use truehd::structs::access_unit::AccessUnit;
use truehd::structs::channel::{ChannelGroup, ChannelLabel};

pub fn cmd_info(args: &InfoArgs, cli: &Cli, multi: Option<&MultiProgress>) -> Result<()> {
    log::info!("Analyzing TrueHD stream: {}", args.input.display());

    let analysis_result = analyze_stream(&args.input, cli, multi)?;

    match analysis_result {
        Some((stream_info, _timestamp, frame_count, total_bytes)) => {
            // Final update with total frames and duration
            update_final_stats(&stream_info, frame_count, total_bytes);
        }
        None => {
            println!("No TrueHD major sync found in the file.");
            println!("This doesn't appear to be a valid TrueHD stream.");
        }
    }

    Ok(())
}

type AnalysisResultTuple = (
    AnalysisResult,
    Option<truehd::structs::timestamp::Timestamp>,
    usize,
    usize,
);

fn analyze_stream(
    input_path: &std::path::Path,
    cli: &Cli,
    multi: Option<&MultiProgress>,
) -> Result<Option<AnalysisResultTuple>> {
    let mut input_reader = InputReader::new(input_path)?;
    let mut extractor = Extractor::default();
    let mut parser = Parser::default();

    // Configure fail level based on strict mode
    let fail_level = if cli.strict {
        Level::Warn
    } else {
        Level::Error
    };
    parser.set_fail_level(fail_level);

    let mut context = AnalysisContext::default();

    // Create progress bar for frame counting if enabled
    if let Some(multi) = multi {
        let pb = multi.add(ProgressBar::new_spinner());
        pb.set_style(ProgressStyle::with_template("{spinner:.green} {msg}")?);
        pb.enable_steady_tick(std::time::Duration::from_millis(100));
        pb.set_message("Analyzing frames...");
        context.pb = Some(pb);
    }

    input_reader.process_chunks(64 * 1024, |chunk| {
        context.total_bytes += chunk.len();
        extractor.push_bytes(chunk);

        for frame_result in extractor.by_ref() {
            let frame = match frame_result {
                Ok(frame) => frame,
                Err(_) => continue,
            };

            context.process_frame(&frame, &mut parser, cli)?;
        }

        Ok(true)
    })?;

    Ok(context.into_result())
}

#[derive(Default)]
struct AnalysisContext {
    timestamp: Option<truehd::structs::timestamp::Timestamp>,
    analysis_result: Option<AnalysisResult>,
    hires_timing_displayed: bool,
    frame_count: usize,
    info_displayed: bool,
    pb: Option<ProgressBar>,
    total_bytes: usize,
}

struct AnalysisResult {
    stream_info: StreamInfo,
    access_unit: AccessUnit,
    hires_timing: Option<u32>,
}

impl AnalysisContext {
    fn process_frame(&mut self, frame: &Frame, parser: &mut Parser, cli: &Cli) -> Result<()> {
        if self.analysis_result.is_none() || !self.hires_timing_displayed {
            match parser.parse(frame) {
                Ok(access_unit) => {
                    if let Some(ts) = &frame.timestamp {
                        if self.timestamp.is_none() {
                            self.timestamp = Some(ts.clone());
                        }
                    }

                    if let Some(major_sync) = &access_unit.major_sync_info {
                        if self.analysis_result.is_none() {
                            let stream_info = StreamInfo::from_major_sync(major_sync)?;
                            self.analysis_result = Some(AnalysisResult {
                                stream_info,
                                access_unit,
                                hires_timing: None,
                            });

                            // Display immediate info now that we have the major sync
                            if !self.info_displayed {
                                self.display_immediate_info();
                                self.info_displayed = true;
                            }
                        }

                        if !self.hires_timing_displayed {
                            if let Some(timing) = parser.hires_output_timing() {
                                if let Some(result) = &mut self.analysis_result {
                                    result.hires_timing = Some(timing as u32);
                                }

                                // Print trim detection immediately when available
                                // Temporarily pause progress bar for clean output
                                if let Some(ref pb) = self.pb {
                                    pb.suspend(|| {
                                        print!("Trim detection              ");
                                        if timing != 0 {
                                            println!("{timing} samples are trimmed from the beginning of the stream");
                                        } else {
                                            println!("No trimmed samples detected");
                                        }
                                        println!();
                                    });
                                } else {
                                    print!("Trim detection              ");
                                    if timing != 0 {
                                        println!(
                                            "{timing} samples are trimmed from the beginning of the stream"
                                        );
                                    } else {
                                        println!("No trimmed samples detected");
                                    }
                                    println!();
                                }

                                self.hires_timing_displayed = true;
                            }
                        }
                    }
                }
                Err(e) => {
                    if cli.strict {
                        return Err(e);
                    }
                    log::warn!("Parse error at frame {}: {e}", self.frame_count);
                }
            }
        }

        self.frame_count += 1;

        if self.frame_count.is_multiple_of(100) {
            if let Some(ref pb) = self.pb {
                pb.set_message(format!("Analyzing frames...       {}", self.frame_count));
                pb.tick();
            }
        }

        Ok(())
    }

    fn display_immediate_info(&self) {
        if let Some(ref analysis) = self.analysis_result {
            if let Some(ref pb) = self.pb {
                pb.suspend(|| {
                    println!();
                    println!("TrueHD Stream Information");
                    println!("=========================");
                    println!();

                    if let Some(ts) = &self.timestamp {
                        println!("SMPTE Timestamp             {ts}");
                        println!();
                    }

                    display_stream_info(&analysis.stream_info);
                    display_presentations(&analysis.access_unit);
                });
            } else {
                println!();
                println!("TrueHD Stream Information");
                println!("=========================");
                println!();

                if let Some(ts) = &self.timestamp {
                    println!("SMPTE Timestamp             {ts}");
                    println!();
                }

                display_stream_info(&analysis.stream_info);
                display_presentations(&analysis.access_unit);
            }
        }
    }

    fn into_result(self) -> Option<AnalysisResultTuple> {
        // Finish progress bar
        if let Some(ref pb) = self.pb {
            pb.finish_and_clear();
        }

        self.analysis_result
            .map(|result| (result, self.timestamp, self.frame_count, self.total_bytes))
    }
}

fn update_final_stats(analysis: &AnalysisResult, frame_count: usize, total_bytes: usize) {
    println!("Analysis Summary");
    println!("  Frames processed          {frame_count}");

    // Format file size
    let size_mb = total_bytes as f64 / 1_000_000.0;
    println!("  Size                      {size_mb:.2} MB ({total_bytes} bytes)");

    // Calculate and display duration
    if let Ok(samples_per_au) = analysis
        .access_unit
        .major_sync_info
        .as_ref()
        .unwrap()
        .format_info
        .samples_per_au()
    {
        let total_samples = frame_count * samples_per_au;
        let duration_secs = total_samples as f64 / analysis.stream_info.sampling_frequency as f64;
        let duration_str = time_str(duration_secs);
        println!("  Duration                  {duration_str}");

        // Calculate average data rate
        if duration_secs > 0.0 {
            let avg_data_rate_kbps = (total_bytes as f64 * 8.0) / (duration_secs * 1000.0);
            println!("  Average data rate         {avg_data_rate_kbps:.1} kbps");
        }
    }

    println!();
}

struct StreamInfo {
    format_sync: String,
    sampling_frequency: u32,
    variable_rate: bool,
    peak_data_rate: u32,
    substreams: usize,
    is_atmos: bool,
}

impl StreamInfo {
    fn from_major_sync(major_sync: &truehd::structs::sync::MajorSyncInfo) -> Result<Self> {
        Ok(Self {
            format_sync: format!("{:08X}", major_sync.format_sync),
            sampling_frequency: major_sync.format_info.sampling_frequency_1()?,
            variable_rate: major_sync.variable_rate,
            peak_data_rate: (major_sync.peak_data_rate as u32
                * major_sync.format_info.sampling_frequency_1()?)
                / 16000,
            substreams: major_sync.substreams,
            is_atmos: major_sync.substream_info & 0x80 != 0,
        })
    }
}

fn display_stream_info(info: &StreamInfo) {
    println!("Stream Information");
    println!("  Format Sync               {}", info.format_sync);
    println!("  Sampling rate             {} Hz", info.sampling_frequency);
    println!("  Variable rate             {}", info.variable_rate);
    println!("  Peak data rate            {} kbps", info.peak_data_rate);
    println!("  Number of substreams      {}", info.substreams);
    println!("  Dolby Atmos               {}", info.is_atmos);
    println!();
}

#[derive(Default, Clone)]
struct PresentationInfo {
    index: usize,
    channels: u8,
    presentation_type: Option<PresentationType>,
    twoch_format: Option<ChannelGroup>,
    sixch_ex: Option<String>,
    assignments: Vec<ChannelLabel>,
    control: Option<bool>,
    dialogue_level: i8,
    mix_level: u8,
    // 16ch
    chan_distribution: Option<bool>,
}

fn display_presentation_info(info: &PresentationInfo) {
    println!("  Presentation {}", info.index);

    display_basic_info(info);
    display_format_info(info);
    display_channel_info(info);
    display_audio_control_info(info);
}

fn display_basic_info(info: &PresentationInfo) {
    let entity_type = if info.index == 3 {
        "elements"
    } else {
        "channels"
    };
    println!("    Number of {entity_type:10}    {}", info.channels);

    if let Some(presentation_type) = &info.presentation_type {
        println!("    Presentation type       {presentation_type}");
    }
}

fn display_format_info(info: &PresentationInfo) {
    if let Some(format) = &info.twoch_format {
        println!("    Channel format          {format}");
    }

    if let Some(ex) = &info.sixch_ex {
        println!("    Dolby Surround EX       {ex}");
    }
}

fn display_channel_info(info: &PresentationInfo) {
    if !info.assignments.is_empty() {
        let label = if info.index == 3 {
            "Bed configuration "
        } else {
            "Channel assignment"
        };
        let assignments = info
            .assignments
            .iter()
            .map(|c| format!("{c:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        println!("    {label:20}    {assignments}");
    }
}

fn display_audio_control_info(info: &PresentationInfo) {
    if let Some(control) = info.control {
        println!("    DRC on by default       {control}");
    }

    println!(
        "    Dialogue Level          {:>3} dBFS",
        info.dialogue_level
    );
    println!("    Mix Level               {:>3} dB", info.mix_level);

    if let Some(chan_distribution) = &info.chan_distribution {
        println!("    Channel distribution    {chan_distribution}");
    }
}

fn display_presentations(access_unit: &AccessUnit) {
    println!("Presentation Information");
    let major_sync = access_unit.major_sync_info.as_ref().unwrap();

    let presentation_builder = PresentationBuilder::new(major_sync, access_unit);
    let presentations = presentation_builder.build_all_presentations();

    for presentation in presentations {
        display_presentation_info(&presentation);
    }
    println!();
}

struct PresentationBuilder<'a> {
    major_sync: &'a truehd::structs::sync::MajorSyncInfo,
    access_unit: &'a AccessUnit,
    presentation_map: PresentationMap,
}

impl<'a> PresentationBuilder<'a> {
    fn new(
        major_sync: &'a truehd::structs::sync::MajorSyncInfo,
        access_unit: &'a AccessUnit,
    ) -> Self {
        let presentation_map = PresentationMap::with_substream_info(
            major_sync.substream_info,
            major_sync.extended_substream_info,
        );

        Self {
            major_sync,
            access_unit,
            presentation_map,
        }
    }

    fn build_all_presentations(&self) -> Vec<PresentationInfo> {
        let mut presentations = Vec::new();
        let mut last_presentation = PresentationInfo::default();

        for index in 0..self.major_sync.substreams.max(3) {
            let presentation = if index < self.major_sync.substreams {
                let info = self.build_presentation_for_substream(index);
                last_presentation = info.clone();
                info
            } else {
                last_presentation.clone()
            };

            presentations.push(self.finalize_presentation(presentation, index));
        }

        presentations
    }

    fn build_presentation_for_substream(&self, index: usize) -> PresentationInfo {
        let mut presentation = PresentationInfo {
            channels: self.access_unit.substream_segment[index].block[0]
                .restart_header
                .as_ref()
                .unwrap()
                .max_matrix_chan
                + 1,
            ..Default::default()
        };

        match index {
            0 => self.configure_twoch_presentation(&mut presentation),
            1 => self.configure_sixch_presentation(&mut presentation),
            2 => self.configure_eightch_presentation(&mut presentation),
            3 => self.configure_sixteench_presentation(&mut presentation),
            _ => unreachable!(),
        }

        presentation
    }

    fn configure_twoch_presentation(&self, presentation: &mut PresentationInfo) {
        let format_info = &self.major_sync.format_info;
        let channel_meaning = &self.major_sync.channel_meaning;

        presentation.twoch_format =
            Some(ChannelGroup::from_modifier(format_info.twoch_decoder_channel_modifier).unwrap());
        presentation.control = Some(channel_meaning.twoch_control_enabled);
        presentation.dialogue_level = -(channel_meaning.twoch_dialogue_norm as i8);
        presentation.mix_level = channel_meaning.twoch_mix_level + 70;
    }

    fn configure_sixch_presentation(&self, presentation: &mut PresentationInfo) {
        let format_info = &self.major_sync.format_info;
        let channel_meaning = &self.major_sync.channel_meaning;

        let assignment = format_info.sixch_decoder_channel_assignment;
        if assignment == 1 {
            presentation.twoch_format = Some(
                ChannelGroup::from_modifier(format_info.twoch_decoder_channel_modifier).unwrap(),
            );
        }

        if assignment & 8 != 0 {
            presentation.sixch_ex = Some(
                match format_info.twoch_decoder_channel_modifier {
                    0 => "Not indicated",
                    1 => "Not encoded",
                    2 => "Encoded",
                    _ => "Reserved",
                }
                .to_string(),
            );
        }

        presentation.assignments =
            ChannelLabel::from_sixch_channel(format_info.sixch_decoder_channel_assignment).unwrap();
        presentation.control = Some(channel_meaning.sixch_control_enabled);
        presentation.dialogue_level = -(channel_meaning.sixch_dialogue_norm as i8);
        presentation.mix_level = channel_meaning.sixch_mix_level + 70;
    }

    fn configure_eightch_presentation(&self, presentation: &mut PresentationInfo) {
        let format_info = &self.major_sync.format_info;
        let channel_meaning = &self.major_sync.channel_meaning;

        presentation.assignments = ChannelLabel::from_eightch_channel(
            format_info.eightch_decoder_channel_assignment,
            self.major_sync.flags,
        )
        .unwrap();
        presentation.control = Some(channel_meaning.eightch_control_enabled);
        presentation.dialogue_level = -(channel_meaning.eightch_dialogue_norm as i8);
        presentation.mix_level = channel_meaning.eightch_mix_level + 70;
    }

    fn configure_sixteench_presentation(&self, presentation: &mut PresentationInfo) {
        let channel_meaning = &self.major_sync.channel_meaning;

        let Some(extra) = &channel_meaning.extra_channel_meaning else {
            return;
        };

        presentation.dialogue_level = -(extra.sixteench_dialogue_norm as i8);
        presentation.mix_level = extra.sixteench_mix_level + 70;

        if extra.dyn_object_only && extra.lfe_present {
            presentation.assignments = vec![ChannelLabel::LFE];
        } else {
            let desc = extra.sixteench_content_description;

            if desc & 1 != 0 {
                presentation.chan_distribution = Some(extra.chan_distribute);
                if !extra.lfe_only {
                    presentation.assignments =
                        ChannelLabel::from_sixteenth_channel(extra.sixteench_channel_assignment)
                            .unwrap();
                }
            }
        }
    }

    fn finalize_presentation(
        &self,
        mut presentation: PresentationInfo,
        index: usize,
    ) -> PresentationInfo {
        presentation.index = index;
        presentation.presentation_type =
            Some(self.presentation_map.presentation_type_by_index(index));
        presentation
    }
}
