use super::atmos::create_damf_header_file;
use super::output::{AudioWriter, create_output_paths};
use crate::caf::wrap_pcm_file_with_caf_header;
use crate::cli::command::AudioFormat;
use crate::damf::{Configuration, Event};
use crate::timestamp::time_str;
use anyhow::{Result, anyhow};
use indicatif::ProgressBar;
use log::Level;
use std::fs::File;
use std::io::{BufWriter, Seek, Write};
use std::path::{Path, PathBuf};
use truehd::log_or_err;

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
        }
    }
}

pub struct FrameHandlerContext<'a> {
    pub base_path: &'a Option<PathBuf>,
    pub format: AudioFormat,
    pub pb: &'a Option<ProgressBar>,
    pub state: &'a WriterState,
    pub start_time: std::time::Instant,
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

        self.handle_atmos_metadata(&decoded, ctx.base_path, ctx.format, ctx.state)?;
        self.create_audio_writer_if_needed(ctx.base_path, ctx.format, sample_rate, channel_count)?;
        self.write_audio_samples(&decoded, channel_count)?;
        self.update_progress_display(sample_rate, ctx.start_time, ctx.pb)?;

        Ok(())
    }

    fn handle_atmos_metadata(
        &mut self,
        decoded: &truehd::process::decode::DecodedAccessUnit,
        base_path: &Option<PathBuf>,
        format: AudioFormat,
        state: &WriterState,
    ) -> Result<()> {
        for oamd in &decoded.oamd {
            let was_atmos = self.has_atmos;
            self.has_atmos = true;

            // Handle file renaming for first Atmos detection
            if !was_atmos && self.audio_writer.is_some() {
                self.handle_atmos_file_rename(
                    base_path,
                    format,
                    decoded.sampling_frequency,
                    decoded.channel_count as u32,
                    state,
                )?;
            }

            // Create DAMF header file when we first detect Atmos
            if !was_atmos {
                if let Some(base_path) = base_path {
                    if let Err(e) = create_damf_header_file(base_path, oamd) {
                        log_or_err!(state, Level::Error, e);
                    }
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

                // Handle PCM file conversion
                if let Some(AudioWriter::Pcm(mut pcm_writer)) = self.audio_writer.take() {
                    pcm_writer.flush()?;
                    drop(pcm_writer);

                    if let Err(e) = wrap_pcm_file_with_caf_header(
                        current_path,
                        sample_rate as f64,
                        channel_count,
                        24,
                    ) {
                        log_or_err!(
                            state,
                            Level::Error,
                            anyhow!("Failed to wrap PCM file with CAF header: {e}")
                        );
                    }
                }

                // Rename the file
                if let Err(e) = std::fs::rename(current_path, &new_audio_path) {
                    log_or_err!(
                        state,
                        Level::Error,
                        anyhow!("Failed to rename audio file: {e}")
                    );
                } else {
                    self.current_audio_path = Some(new_audio_path.clone());
                }

                // Recreate CAF writer if needed
                if self.audio_writer.is_none() {
                    self.recreate_caf_writer(&new_audio_path)?;
                }
            }
        }
        Ok(())
    }

    fn recreate_caf_writer(&mut self, path: &Path) -> Result<()> {
        log::info!("Audio file converted to CAF format - resuming with new CAF writer");
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)?;

        let caf_writer = {
            let mut temp_file = file.try_clone()?;
            let file_info = crate::caf::parse_caf_file(&mut temp_file)?;
            temp_file.seek(std::io::SeekFrom::End(0))?;
            crate::caf::CAFWriter::from_parsed_info(BufWriter::new(file), file_info)?
        };
        self.audio_writer = Some(AudioWriter::Caf(caf_writer));
        Ok(())
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
                let effective_format = if self.has_atmos && format == AudioFormat::Pcm {
                    log::info!("Atmos audio detected - forcing CAF format instead of PCM");
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
