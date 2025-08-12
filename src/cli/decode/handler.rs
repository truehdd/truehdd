use super::atmos::{create_damf_header_file, rewrite_damf_header_for_bed_conform};
use super::output::{AudioWriter, create_output_paths};
// wrap_pcm_file_with_caf_header no longer needed since presentation 3 forces CAF
use crate::cli::command::AudioFormat;
use crate::damf::{BedInstance, Configuration, Event};
use crate::timestamp::time_str;
use anyhow::{Result, anyhow};
use indicatif::ProgressBar;
use log::Level;
use std::fs::File;
use std::io::{BufWriter, Seek, Write};
use std::path::{Path, PathBuf};
use truehd::log_or_err;

struct AudioFormatHandler;

struct BedConformConversionParams<'a> {
    writer: AudioWriter,
    current_path: &'a Path,
    new_path: &'a Path,
    channel_count: u32,
    conformed_channel_count: usize,
    sample_rate: f64,
    state: &'a WriterState,
}

impl AudioFormatHandler {
    /// Handle CAF format rename for Atmos (presentation 3 always uses CAF)
    fn handle_format_specific_rename(
        writer: AudioWriter,
        current_path: &Path,
        new_path: &Path,
        _sample_rate: u32,
        _channel_count: u32,
        state: &WriterState,
    ) -> Result<AudioWriter> {
        // Since presentation 3 forces CAF format, we only expect CAF writers here
        match writer {
            AudioWriter::Caf(mut caf_writer) => {
                caf_writer.finish()?;
                drop(caf_writer);
                Self::rename_and_recreate_caf_writer(current_path, new_path, state)
            }
            // These cases should never happen for presentation 3 due to effective_format forcing CAF
            AudioWriter::Pcm(_) | AudioWriter::W64(_) => {
                unreachable!(
                    "PCM/W64 writers should not exist for presentation 3 (Atmos) due to format forcing"
                )
            }
        }
    }

    fn rename_and_recreate_caf_writer(
        current_path: &Path,
        new_path: &Path,
        state: &WriterState,
    ) -> Result<AudioWriter> {
        if let Err(e) = std::fs::rename(current_path, new_path) {
            log_or_err!(
                state,
                Level::Error,
                anyhow!("Failed to rename audio file: {e}")
            );
        }

        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(new_path)?;

        let caf_writer = {
            let mut temp_file = file.try_clone()?;
            let file_info = crate::caf::parse_caf_file(&mut temp_file)?;
            temp_file.seek(std::io::SeekFrom::End(0))?;
            crate::caf::CAFWriter::from_parsed_info(BufWriter::new(file), file_info)?
        };
        Ok(AudioWriter::Caf(caf_writer))
    }

    fn handle_bed_conform_conversion(
        params: BedConformConversionParams,
        convert_file_fn: impl Fn(&Path, &Path, usize, usize, f64, &WriterState) -> Result<()>,
    ) -> Result<AudioWriter> {
        // Close current CAF writer (only CAF expected for presentation 3)
        match params.writer {
            AudioWriter::Caf(mut w) => {
                w.finish()?;
                drop(w);
            }
            // These should not happen due to effective_format forcing CAF
            AudioWriter::Pcm(_) | AudioWriter::W64(_) => {
                unreachable!("PCM/W64 writers should not exist for presentation 3 bed conformance")
            }
        }

        // Create temporary file for conversion
        let temp_path = {
            let mut temp = params.current_path.to_path_buf();
            temp.set_extension("tmp");
            temp
        };

        // Rename current to temp
        if let Err(e) = std::fs::rename(params.current_path, &temp_path) {
            return Err(anyhow::anyhow!("Failed to rename to temp file: {e}"));
        }

        // Convert the audio data with bed conformance
        convert_file_fn(
            &temp_path,
            params.new_path,
            params.channel_count as usize,
            params.conformed_channel_count,
            params.sample_rate,
            params.state,
        )?;

        // Clean up temp file
        if let Err(e) = std::fs::remove_file(&temp_path) {
            log::warn!("Failed to remove temp file: {e}");
        }

        // Create new CAF writer
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(params.new_path)?;

        let caf_writer = {
            let mut temp_file = file.try_clone()?;
            let file_info = crate::caf::parse_caf_file(&mut temp_file)?;
            temp_file.seek(std::io::SeekFrom::End(0))?;
            crate::caf::CAFWriter::from_parsed_info(BufWriter::new(file), file_info)?
        };
        Ok(AudioWriter::Caf(caf_writer))
    }
}

struct AudioDataConverter;

impl AudioDataConverter {
    fn convert_caf_bytes_to_samples(buffer: &[u8], endianness: crate::caf::Endianness) -> Vec<i32> {
        const BYTES_PER_SAMPLE: usize = 3;
        let total_samples = buffer.len() / BYTES_PER_SAMPLE;
        let mut samples = Vec::with_capacity(total_samples);

        for chunk in buffer.chunks_exact(BYTES_PER_SAMPLE) {
            let sample = match endianness {
                crate::caf::Endianness::BigEndian => {
                    i32::from_be_bytes([0, chunk[0], chunk[1], chunk[2]]) >> 8
                }
                crate::caf::Endianness::LittleEndian => {
                    i32::from_le_bytes([chunk[0], chunk[1], chunk[2], 0]) >> 8
                }
            };
            samples.push(sample);
        }
        samples
    }
}

struct BedChannelMapper;

struct ChannelCountCalculator;

impl ChannelCountCalculator {
    const TARGET_BED_CHANNELS: usize = 10; // 7.1.2 layout

    /// Calculate the effective channel count for bed conformance
    /// Returns (num_bed_channels, num_object_channels, conformed_channel_count)
    fn calculate_bed_conform_counts(
        original_channel_count: usize,
        bed_indices: &[usize],
    ) -> (usize, usize, usize) {
        let num_bed_channels = bed_indices.len();
        let num_object_channels = original_channel_count.saturating_sub(num_bed_channels);
        let conformed_channel_count = Self::TARGET_BED_CHANNELS + num_object_channels;
        (
            num_bed_channels,
            num_object_channels,
            conformed_channel_count,
        )
    }

    /// Calculate conformed channel count only (shorthand for common case)
    fn calculate_conformed_channel_count(
        original_channel_count: usize,
        bed_indices: &[usize],
    ) -> usize {
        let (_, _, conformed_count) =
            Self::calculate_bed_conform_counts(original_channel_count, bed_indices);
        conformed_count
    }
}

impl BedChannelMapper {
    fn apply_bed_conformance(
        original_samples: Vec<i32>,
        original_channel_count: usize,
        bed_indices: &[usize],
    ) -> Vec<i32> {
        let (num_bed_channels, num_object_channels, conformed_channel_count) =
            ChannelCountCalculator::calculate_bed_conform_counts(
                original_channel_count,
                bed_indices,
            );
        let samples_per_frame = original_samples.len() / original_channel_count;

        let mut conformed_samples = Vec::with_capacity(samples_per_frame * conformed_channel_count);

        for sample_idx in 0..samples_per_frame {
            // Handle bed channels (0-9)
            for target_bed_ch in 0..ChannelCountCalculator::TARGET_BED_CHANNELS {
                if let Some(source_ch_pos) =
                    bed_indices.iter().position(|&idx| idx == target_bed_ch)
                {
                    let sample =
                        original_samples[sample_idx * original_channel_count + source_ch_pos];
                    conformed_samples.push(sample);
                } else {
                    conformed_samples.push(0i32);
                }
            }

            // Handle object channels
            for obj_ch in 0..num_object_channels {
                let source_ch = num_bed_channels + obj_ch;
                let sample = original_samples[sample_idx * original_channel_count + source_ch];
                conformed_samples.push(sample);
            }
        }

        conformed_samples
    }

    fn apply_bed_conformance_to_frame(
        decoded: &truehd::process::decode::DecodedAccessUnit,
        channel_count: usize,
        bed_indices: &[usize],
    ) -> Vec<i32> {
        let (num_bed_channels, num_object_channels, conformed_channel_count) =
            ChannelCountCalculator::calculate_bed_conform_counts(channel_count, bed_indices);

        let mut samples = Vec::with_capacity(decoded.sample_length * conformed_channel_count);

        for sample_idx in 0..decoded.sample_length {
            // Handle bed channels (0-9)
            for target_bed_ch in 0..ChannelCountCalculator::TARGET_BED_CHANNELS {
                if let Some(source_ch_pos) =
                    bed_indices.iter().position(|&idx| idx == target_bed_ch)
                {
                    let sample = decoded.pcm_data[sample_idx][source_ch_pos];
                    samples.push(sample);
                } else {
                    samples.push(0i32);
                }
            }

            // Handle object channels
            for obj_ch in 0..num_object_channels {
                let source_ch = num_bed_channels + obj_ch;
                let sample = decoded.pcm_data[sample_idx][source_ch];
                samples.push(sample);
            }
        }

        samples
    }
}

pub struct WriterState {
    pub fail_level: Level,
}

pub struct DecodeHandler {
    pub audio_writer: Option<AudioWriter>,
    pub current_audio_path: Option<PathBuf>,
    pub damf_metadata_file_writer: Option<BufWriter<File>>,
    pub has_atmos: bool,
    pub prev_events: Vec<Event>,
    pub decoded_frames: u64,
    pub decoded_samples: u64,
    pub final_sample_rate: u32,
    pub bed_indices: Option<Vec<usize>>,
}

impl Default for DecodeHandler {
    fn default() -> Self {
        Self {
            audio_writer: None,
            current_audio_path: None,
            damf_metadata_file_writer: None,
            has_atmos: false,
            prev_events: Vec::new(),
            decoded_frames: 0,
            decoded_samples: 0,
            final_sample_rate: 48000,
            bed_indices: None,
        }
    }
}

pub struct FrameHandlerContext<'a> {
    pub base_path: &'a Option<PathBuf>,
    pub format: AudioFormat,
    pub pb: &'a Option<ProgressBar>,
    pub state: &'a WriterState,
    pub start_time: std::time::Instant,
    pub bed_conform: bool,
    pub warp_mode: Option<crate::cli::command::WarpMode>,
}

impl DecodeHandler {
    pub fn handle_decoded_frame(
        &mut self,
        decoded: truehd::process::decode::DecodedAccessUnit,
        ctx: &FrameHandlerContext,
    ) -> Result<()> {
        let sample_rate = decoded.sampling_frequency;
        let channel_count = decoded.channel_count;

        if decoded.is_duplicate {
            return Ok(());
        }

        self.decoded_frames += 1u64;
        self.decoded_samples += decoded.sample_length as u64;
        self.final_sample_rate = sample_rate;

        self.handle_atmos_metadata(
            &decoded,
            ctx.base_path,
            ctx.format,
            ctx.state,
            ctx.bed_conform,
            ctx.warp_mode,
        )?;

        let effective_channel_count = if ctx.bed_conform && self.has_atmos {
            let empty_vec = Vec::new();
            let bed_indices = self.bed_indices.as_ref().unwrap_or(&empty_vec);
            ChannelCountCalculator::calculate_conformed_channel_count(channel_count, bed_indices)
        } else {
            channel_count
        };

        self.create_audio_writer_if_needed(
            ctx.base_path,
            ctx.format,
            sample_rate,
            effective_channel_count,
        )?;

        if ctx.bed_conform && self.has_atmos {
            self.write_audio_samples_bed_conform(&decoded, channel_count)?;
        } else {
            self.write_audio_samples(&decoded, channel_count)?;
        }

        self.update_progress_display(sample_rate, ctx.start_time, ctx.pb)?;

        Ok(())
    }

    fn handle_atmos_metadata(
        &mut self,
        decoded: &truehd::process::decode::DecodedAccessUnit,
        base_path: &Option<PathBuf>,
        format: AudioFormat,
        state: &WriterState,
        bed_conform: bool,
        warp_mode: Option<crate::cli::command::WarpMode>,
    ) -> Result<()> {
        for oamd in &decoded.oamd {
            let was_atmos = self.has_atmos;
            self.has_atmos = true;

            // Create DAMF header file when we first detect Atmos
            if !was_atmos {
                if let Some(base_path) = base_path {
                    if bed_conform {
                        // Store bed indices first for conformance
                        self.bed_indices = BedInstance::with_oamd_payload(oamd)
                            .first()
                            .map(|bed| bed.to_index_vec());

                        // Create bed-conformed DAMF header
                        if self.bed_indices.is_some() {
                            if let Err(e) =
                                rewrite_damf_header_for_bed_conform(base_path, oamd, warp_mode)
                            {
                                log_or_err!(state, Level::Error, e);
                            }
                        } else {
                            // Fallback to regular header if no bed indices
                            if let Err(e) = create_damf_header_file(base_path, oamd, warp_mode) {
                                log_or_err!(state, Level::Error, e);
                            }
                        }
                    } else {
                        // Create regular DAMF header
                        if let Err(e) = create_damf_header_file(base_path, oamd, warp_mode) {
                            log_or_err!(state, Level::Error, e);
                        }
                    }
                }
            }

            // Handle file renaming for first Atmos detection
            if !was_atmos && self.audio_writer.is_some() {
                if bed_conform {
                    self.handle_atmos_file_rename_with_bed_conform(
                        base_path,
                        format,
                        decoded.sampling_frequency,
                        decoded.channel_count as u32,
                        state,
                    )?;
                } else {
                    self.handle_atmos_file_rename(
                        base_path,
                        format,
                        decoded.sampling_frequency,
                        decoded.channel_count as u32,
                        state,
                    )?;
                }
            }

            self.handle_metadata_writing(
                oamd,
                decoded.sampling_frequency,
                self.decoded_samples,
                base_path,
                format,
            )?;
        }
        Ok(())
    }

    fn handle_atmos_file_rename(
        &mut self,
        base_path: &Option<PathBuf>,
        format: AudioFormat,
        sample_rate: u32,
        channel_count: u32,
        state: &WriterState,
    ) -> Result<()> {
        if let (Some(base_path), Some(current_path)) = (base_path, &self.current_audio_path) {
            let (new_audio_path, _) = create_output_paths(base_path, format, true);
            if current_path != &new_audio_path {
                log::info!(
                    "Atmos detected - renaming audio file to: {}",
                    new_audio_path.display()
                );

                if let Some(writer) = self.audio_writer.take() {
                    let new_writer = AudioFormatHandler::handle_format_specific_rename(
                        writer,
                        current_path,
                        &new_audio_path,
                        sample_rate,
                        channel_count,
                        state,
                    )?;
                    self.audio_writer = Some(new_writer);
                    self.current_audio_path = Some(new_audio_path);
                }
            }
        }
        Ok(())
    }

    fn handle_atmos_file_rename_with_bed_conform(
        &mut self,
        base_path: &Option<PathBuf>,
        format: AudioFormat,
        sample_rate: u32,
        channel_count: u32,
        state: &WriterState,
    ) -> Result<()> {
        if let (Some(base_path), Some(current_path)) = (base_path, &self.current_audio_path) {
            let (new_audio_path, _) = create_output_paths(base_path, format, true);
            if current_path != &new_audio_path {
                log::info!(
                    "Atmos detected with bed conformance - converting audio file to: {}",
                    new_audio_path.display()
                );

                let empty_vec = Vec::new();
                let bed_indices = self.bed_indices.as_ref().unwrap_or(&empty_vec);
                let conformed_channel_count =
                    ChannelCountCalculator::calculate_conformed_channel_count(
                        channel_count as usize,
                        bed_indices,
                    );

                if let Some(writer) = self.audio_writer.take() {
                    let params = BedConformConversionParams {
                        writer,
                        current_path,
                        new_path: &new_audio_path,
                        channel_count,
                        conformed_channel_count,
                        sample_rate: sample_rate as f64,
                        state,
                    };
                    let new_writer = AudioFormatHandler::handle_bed_conform_conversion(
                        params,
                        |temp_path, new_path, orig_ch, conf_ch, sr, st| {
                            self.convert_audio_file_to_bed_conform(
                                temp_path, new_path, orig_ch, conf_ch, sr, st,
                            )
                        },
                    )?;
                    self.audio_writer = Some(new_writer);
                    self.current_audio_path = Some(new_audio_path);
                }
            }
        }
        Ok(())
    }

    fn convert_audio_file_to_bed_conform(
        &self,
        temp_path: &Path,
        new_path: &Path,
        original_channel_count: usize,
        conformed_channel_count: usize,
        sample_rate: f64,
        _state: &WriterState,
    ) -> Result<()> {
        log::info!(
            "Converting audio from {original_channel_count} to {conformed_channel_count} channels"
        );

        // For presentation 3, files are always CAF since format is forced by effective_format
        self.convert_caf_file_to_bed_conform(
            temp_path,
            new_path,
            original_channel_count,
            conformed_channel_count,
            sample_rate,
        )
    }

    fn convert_caf_file_to_bed_conform(
        &self,
        temp_path: &Path,
        new_path: &Path,
        original_channel_count: usize,
        conformed_channel_count: usize,
        sample_rate: f64,
    ) -> Result<()> {
        use std::io::{Read, Seek, SeekFrom};

        let mut temp_file = File::open(temp_path)?;
        let file_info = crate::caf::parse_caf_file(&mut temp_file)?;

        temp_file.seek(SeekFrom::Start(file_info.data_chunk_start))?;
        let mut audio_data = Vec::new();
        temp_file.read_to_end(&mut audio_data)?;

        let original_samples =
            AudioDataConverter::convert_caf_bytes_to_samples(&audio_data, file_info.endianness);

        let conformed_samples = self.convert_samples_to_bed_conform(
            original_samples,
            original_channel_count,
            conformed_channel_count,
        );

        let mut caf_writer = AudioWriter::create_caf(
            new_path.to_path_buf(),
            sample_rate as u32,
            conformed_channel_count as u32,
        )?;
        caf_writer.write_pcm_samples(&conformed_samples, conformed_channel_count)?;
        caf_writer.finish()?;

        Ok(())
    }

    fn convert_samples_to_bed_conform(
        &self,
        original_samples: Vec<i32>,
        original_channel_count: usize,
        _conformed_channel_count: usize,
    ) -> Vec<i32> {
        let empty_vec = Vec::new();
        let bed_indices = self.bed_indices.as_ref().unwrap_or(&empty_vec);
        BedChannelMapper::apply_bed_conformance(
            original_samples,
            original_channel_count,
            bed_indices,
        )
    }

    fn handle_metadata_writing(
        &mut self,
        oamd: &truehd::structs::oamd::ObjectAudioMetadataPayload,
        sample_rate: u32,
        sample_pos: u64,
        base_path: &Option<PathBuf>,
        format: AudioFormat,
    ) -> Result<()> {
        let mut conf = Configuration::with_oamd_payload(oamd, sample_rate, sample_pos);

        let (events_diff, remove_header) = if !self.prev_events.is_empty() {
            (
                Event::compare_event_vectors(&self.prev_events, &conf.events),
                true,
            )
        } else {
            (conf.events.clone(), false)
        };

        self.prev_events = conf.events.clone();
        conf.events = events_diff;
        let oamd_str = conf.serialize_events(remove_header);

        if let Some(base_path) = base_path {
            if self.damf_metadata_file_writer.is_none() {
                let (_, metadata_path) = create_output_paths(base_path, format, self.has_atmos);
                if !metadata_path.as_os_str().is_empty() {
                    log::info!("Creating metadata file: {}", metadata_path.display());
                    self.damf_metadata_file_writer =
                        Some(BufWriter::new(File::create(metadata_path)?));
                }
            }
            if let Some(ref mut writer) = self.damf_metadata_file_writer {
                write!(writer, "{oamd_str}")?;
                writer.flush()?;
            }
        }
        Ok(())
    }

    fn create_audio_writer_if_needed(
        &mut self,
        base_path: &Option<PathBuf>,
        format: AudioFormat,
        sample_rate: u32,
        channel_count: usize,
    ) -> Result<()> {
        if let Some(base_path) = base_path {
            if self.audio_writer.is_none() {
                // For Atmos content, always use CAF format
                let effective_format = if self.has_atmos {
                    if format != AudioFormat::Caf {
                        log::info!(
                            "Atmos audio detected - forcing CAF format instead of {format:?}"
                        );
                    }
                    AudioFormat::Caf
                } else {
                    format
                };

                let (audio_path, _) =
                    create_output_paths(base_path, effective_format, self.has_atmos);
                log::info!("Creating audio file: {}", audio_path.display());

                self.current_audio_path = Some(audio_path.clone());

                match effective_format {
                    AudioFormat::Caf => {
                        self.audio_writer = Some(AudioWriter::create_caf(
                            audio_path,
                            sample_rate,
                            channel_count as u32,
                        )?);
                    }
                    AudioFormat::Pcm => {
                        self.audio_writer = Some(AudioWriter::create_pcm(audio_path)?);
                    }
                    AudioFormat::W64 => {
                        self.audio_writer = Some(AudioWriter::create_w64(
                            audio_path,
                            sample_rate,
                            channel_count as u32,
                        )?);
                    }
                }
            }
        }
        Ok(())
    }

    fn write_audio_samples(
        &mut self,
        decoded: &truehd::process::decode::DecodedAccessUnit,
        channel_count: usize,
    ) -> Result<()> {
        if let Some(ref mut writer) = self.audio_writer {
            let mut samples = Vec::with_capacity(decoded.sample_length * channel_count);
            for sample_idx in 0..decoded.sample_length {
                for ch in 0..channel_count {
                    let sample = decoded.pcm_data[sample_idx][ch];
                    samples.push(sample);
                }
            }
            writer.write_pcm_samples(&samples, channel_count)?;
        }
        Ok(())
    }

    fn write_audio_samples_bed_conform(
        &mut self,
        decoded: &truehd::process::decode::DecodedAccessUnit,
        channel_count: usize,
    ) -> Result<()> {
        if let Some(ref mut writer) = self.audio_writer {
            let empty_vec = Vec::new();
            let bed_indices = self.bed_indices.as_ref().unwrap_or(&empty_vec);
            let conformed_channel_count = ChannelCountCalculator::calculate_conformed_channel_count(
                channel_count,
                bed_indices,
            );

            let samples = BedChannelMapper::apply_bed_conformance_to_frame(
                decoded,
                channel_count,
                bed_indices,
            );

            writer.write_pcm_samples(&samples, conformed_channel_count)?;
        }
        Ok(())
    }

    fn update_progress_display(
        &self,
        sample_rate: u32,
        start_time: std::time::Instant,
        pb: &Option<ProgressBar>,
    ) -> Result<()> {
        if self.decoded_frames.is_multiple_of(30) {
            let elapsed = start_time.elapsed();
            let audio_duration_secs = self.decoded_samples as f64 / sample_rate as f64;
            let realtime_multiplier = audio_duration_secs / elapsed.as_secs_f64();
            let time_str = time_str(audio_duration_secs);

            if let Some(pb) = pb {
                pb.set_message(format!(
                    "speed: {realtime_multiplier:.1}x | timestamp: {time_str}"
                ));
            }
        }
        Ok(())
    }

    pub fn finalize(&mut self) -> Result<()> {
        if let Some(ref mut writer) = self.audio_writer {
            writer.finish()?;
        }

        if let Some(ref mut writer) = self.damf_metadata_file_writer {
            writer.flush()?;
        }

        Ok(())
    }
}
