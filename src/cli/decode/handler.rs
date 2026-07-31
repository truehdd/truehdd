use super::output::AudioWriter;
use crate::caf::{CAFWriter, parse_caf_file};
use crate::cli::command::{AudioFormat, WarpMode};
use crate::damf::{BedInstance, Configuration, Data, Event};
use anyhow::Result;
use indicatif::ProgressBar;
use log::info;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

const EMPTY_BED_INDICES: &[usize] = &[];
const TARGET_BED_CHANNELS: usize = 10;

pub struct DecodeHandler {
    output_path: Option<PathBuf>,
    format: AudioFormat,
    bed_conform: bool,
    warp_mode: Option<WarpMode>,
    audio_writer: Option<AudioWriter>,
    metadata_writer: Option<BufWriter<File>>,
    current_audio_path: Option<PathBuf>,
    segment_index: u32,
    sample_buffer: Vec<i32>,
    progress_buffer: String,
    has_atmos: bool,
    prev_events: Vec<Event>,
    bed_indices: Option<Vec<usize>>,
    pub(crate) decoded_frames: u64,
    pub total_samples: u64,
    pub final_sample_rate: u32,
    segment_start_samples: u64,
    is_segmented: bool,
    au_index: u64,
    pub start_time: Instant,

    // Bed conformance buffering
    atmos_probe_buffer: Vec<Vec<i32>>, // Buffer samples before we know channel layout
    atmos_probe_range: u64,            // How many AUs to probe before giving up
    atmos_probing: bool,               // Whether we're still probing for Atmos
    buffered_channel_count: usize,     // Channel count of buffered samples
    buffered_sample_rate: u32,         // Sample rate of buffered samples

    // Cached channel count to avoid recalculation
    effective_channel_count: Option<usize>, // Final channel count for audio writer

    // Cached bed conformance parameters to avoid recalculation during audio writes
    cached_bed_indices: Option<Vec<usize>>, // Resolved bed indices for performance
    cached_num_object_channels: usize,      // Number of object channels
    cached_bed_channel_map: Option<Vec<Option<usize>>>, // Pre-computed bed channel mapping

    // Flag to track if current frame was processed during probing finalization
    current_frame_processed: bool,
}

// Structure to hold channel count information
#[derive(Debug, Clone)]
struct ChannelInfo {
    total_channels: usize,
    bed_channels: usize,
    object_channels: usize,
    is_bed_conformed: bool,
}

impl ChannelInfo {
    fn add_s(number: usize) -> String {
        if number > 1 { "s" } else { "" }.to_string()
    }
    fn description(&self) -> String {
        if self.is_bed_conformed || self.bed_channels > 0 || self.object_channels > 0 {
            format!(
                "{} bed channel{} + {} object{}",
                self.bed_channels,
                Self::add_s(self.bed_channels),
                self.object_channels,
                Self::add_s(self.object_channels)
            )
        } else {
            format!(
                "{} channel{}",
                self.total_channels,
                Self::add_s(self.total_channels)
            )
        }
    }

    /// Create ChannelInfo for bed-conformed audio
    fn for_atmos(original_count: usize, bed_indices: &[usize], bed_conform: bool) -> Self {
        let bed_channels = bed_indices.len();
        let object_channels = original_count.saturating_sub(bed_indices.len());
        let total_channels = if bed_conform { TARGET_BED_CHANNELS } else { 0 } + object_channels;

        Self {
            total_channels,
            bed_channels,
            object_channels,
            is_bed_conformed: bed_conform,
        }
    }

    /// Create ChannelInfo for regular audio (no Atmos breakdown)
    fn for_regular(channel_count: usize) -> Self {
        Self {
            total_channels: channel_count,
            bed_channels: 0,
            object_channels: 0,
            is_bed_conformed: false,
        }
    }
}

impl DecodeHandler {
    fn extract_samples_into_buffer(
        &mut self,
        decoded: &truehd::process::decode::DecodedAccessUnit,
    ) {
        self.sample_buffer.clear();
        self.sample_buffer
            .reserve(decoded.sample_length * decoded.channel_count);
        for sample_idx in 0..decoded.sample_length {
            for ch in 0..decoded.channel_count {
                self.sample_buffer.push(decoded.pcm_data[sample_idx][ch]);
            }
        }
    }

    fn extract_samples_from_frame(
        decoded: &truehd::process::decode::DecodedAccessUnit,
    ) -> Vec<i32> {
        let mut frame_samples = Vec::with_capacity(decoded.sample_length * decoded.channel_count);
        for sample_idx in 0..decoded.sample_length {
            for ch in 0..decoded.channel_count {
                frame_samples.push(decoded.pcm_data[sample_idx][ch]);
            }
        }
        frame_samples
    }

    pub(crate) fn new(
        output_path: Option<PathBuf>,
        format: AudioFormat,
        bed_conform: bool,
        warp_mode: Option<WarpMode>,
        atmos_probe_range: u64,
    ) -> Self {
        // For presentation 3 with bed conformance, we need to probe for Atmos
        let atmos_probing = bed_conform && format == AudioFormat::Caf;
        let atmos_probe_range = if atmos_probing { atmos_probe_range } else { 0 };

        Self {
            output_path,
            format,
            bed_conform,
            warp_mode,
            audio_writer: None,
            metadata_writer: None,
            current_audio_path: None,
            segment_index: 0,
            sample_buffer: Vec::with_capacity(160 * 16), // TrueHD theoretical maximum
            progress_buffer: String::with_capacity(64),
            has_atmos: false,
            prev_events: Vec::new(),
            bed_indices: None,
            decoded_frames: 0,
            total_samples: 0,
            final_sample_rate: 48000,
            segment_start_samples: 0,
            is_segmented: false,
            au_index: 0,
            start_time: Instant::now(),

            // Bed conformance buffering
            atmos_probe_buffer: Vec::new(),
            atmos_probe_range,
            atmos_probing,
            buffered_channel_count: 0,
            buffered_sample_rate: 48000,

            // Cached channel count
            effective_channel_count: None,

            // Cached bed conformance parameters
            cached_bed_indices: None,
            cached_num_object_channels: 0,
            cached_bed_channel_map: None,

            // Frame processing flag
            current_frame_processed: false,
        }
    }

    pub(crate) fn handle_decoded_frame(
        &mut self,
        decoded: truehd::process::decode::DecodedAccessUnit,
        pb: &Option<ProgressBar>,
        start_time: Instant,
    ) -> Result<()> {
        if decoded.is_duplicate {
            return Ok(());
        }

        // Reset frame processing flag for new frame
        self.current_frame_processed = false;
        self.final_sample_rate = decoded.sampling_frequency;

        // Handle Atmos metadata
        for oamd in &decoded.oamd {
            let was_atmos = self.has_atmos;
            self.has_atmos = true;

            if !was_atmos {
                // Always extract bed indices when Atmos is detected (needed for channel descriptions)
                self.bed_indices = BedInstance::with_oamd_payload(oamd)
                    .first()
                    .map(|bed| bed.to_index_vec());

                // Build performance cache for bed conformance if needed
                if self.bed_conform {
                    self.build_bed_conformance_cache(decoded.channel_count);
                }

                // If we were probing, finalize now that we found Atmos
                if self.atmos_probing {
                    // Store channel info if not already done (for immediate OAMD case)
                    if self.buffered_channel_count == 0 {
                        self.buffered_channel_count = decoded.channel_count;
                        self.buffered_sample_rate = decoded.sampling_frequency;
                    }

                    // Add current frame to buffer before finalizing
                    let frame_samples = Self::extract_samples_from_frame(&decoded);
                    self.atmos_probe_buffer.push(frame_samples);
                    self.finalize_probing_phase()?;

                    // Mark this frame as processed during finalization
                    self.current_frame_processed = true;
                }

                if let Some(ref base_path) = self.output_path {
                    let effective_base = if self.is_segmented {
                        self.get_segmented_base_path(base_path)
                    } else {
                        base_path.clone()
                    };

                    if self.bed_conform {
                        if self.bed_indices.is_some() {
                            rewrite_damf_header_for_bed_conform(
                                &effective_base,
                                oamd,
                                self.warp_mode,
                            )?;
                        } else {
                            create_damf_header_file(&effective_base, oamd, self.warp_mode)?;
                        }
                    } else {
                        create_damf_header_file(&effective_base, oamd, self.warp_mode)?;
                    }
                }

                // Handle first-time Atmos file rename (not for segmented mode and not if we were probing)
                if self.audio_writer.is_some() && !self.is_segmented && !was_atmos {
                    self.handle_atmos_rename(&decoded)?;
                }
            }

            self.write_metadata(oamd, decoded.sampling_frequency)?;
        }

        self.ensure_audio_writer(&decoded)?;

        // Only write audio if it wasn't already processed during probing finalization
        if !self.current_frame_processed {
            self.write_audio(&decoded)?;
        }

        self.decoded_frames += 1;
        self.total_samples += decoded.sample_length as u64;
        self.au_index += 1;

        self.update_progress(pb, start_time)?;

        Ok(())
    }

    pub(crate) fn handle_stream_restart(&mut self) -> Result<()> {
        info!(
            "Stream restart detected at AU {}, creating new segment {}",
            self.au_index,
            self.segment_index + 1
        );

        self.segment_start_samples = self.total_samples;
        self.finalize()?;
        self.segment_index += 1;
        self.is_segmented = true;

        // Reset for new segment
        self.audio_writer = None;
        self.metadata_writer = None;
        self.current_audio_path = None;
        self.prev_events.clear();

        // Reset Atmos state for new segment
        if self.has_atmos {
            self.has_atmos = false;
            self.bed_indices = None;
        }

        // Reset buffering state for new segment
        self.atmos_probe_buffer.clear();
        self.atmos_probing = self.bed_conform && self.format == AudioFormat::Caf;
        self.buffered_channel_count = 0;
        self.buffered_sample_rate = 48000;
        self.effective_channel_count = None; // Reset cached channel count
        self.cached_bed_indices = None; // Reset cached bed conformance parameters
        self.cached_num_object_channels = 0;
        self.cached_bed_channel_map = None;
        self.current_frame_processed = false; // Reset frame processing flag

        Ok(())
    }

    fn write_audio(&mut self, decoded: &truehd::process::decode::DecodedAccessUnit) -> Result<()> {
        // Handle buffering during Atmos probing phase
        if self.atmos_probing {
            return self.handle_probing_phase_audio(decoded);
        }

        // At this point, probing is done (if it was happening)
        // Calculate and cache channel count if not already done
        self.calculate_and_cache_channel_count(decoded);
        let channel_count = self.effective_channel_count.unwrap();

        if self.bed_conform && self.has_atmos {
            self.write_bed_conform_samples(decoded);
        } else {
            self.extract_samples_into_buffer(decoded);
        }

        if let Some(ref mut writer) = self.audio_writer {
            writer.write_pcm_samples(&self.sample_buffer, channel_count)?;
        }
        Ok(())
    }

    fn handle_probing_phase_audio(
        &mut self,
        decoded: &truehd::process::decode::DecodedAccessUnit,
    ) -> Result<()> {
        // Store channel info for buffered samples
        if self.buffered_channel_count == 0 {
            self.buffered_channel_count = decoded.channel_count;
            self.buffered_sample_rate = decoded.sampling_frequency;
            info!(
                "Starting Atmos probe buffering: {} channels, {} Hz (range: {} AUs)",
                decoded.channel_count, decoded.sampling_frequency, self.atmos_probe_range
            );
        }

        // Buffer the samples
        let frame_samples = Self::extract_samples_from_frame(decoded);
        self.atmos_probe_buffer.push(frame_samples);

        // Check if we should stop probing
        if self.decoded_frames + 1 >= self.atmos_probe_range {
            self.finalize_probing_phase()?;
        }

        Ok(())
    }

    fn finalize_probing_phase(&mut self) -> Result<()> {
        info!(
            "Finalizing Atmos probe phase: {} buffered, has_atmos={}",
            self.atmos_probe_buffer.len(),
            self.has_atmos
        );

        self.atmos_probing = false;

        // Get channel info and cache the count
        let channel_info = self.get_channel_info(self.buffered_channel_count);
        self.effective_channel_count = Some(channel_info.total_channels);

        // Create audio writer with correct channel count
        if let Some(ref base_path) = self.output_path {
            let audio_path = self.get_audio_path(base_path);
            info!(
                "Creating audio file: {} ({})",
                audio_path.display(),
                channel_info.description()
            );

            self.audio_writer = Some(self.create_audio_writer_for_format(
                audio_path.clone(),
                self.buffered_sample_rate,
                channel_info.total_channels,
            )?);
            self.current_audio_path = Some(audio_path);
        }

        // Write all buffered samples
        self.write_buffered_samples(channel_info.total_channels)?;

        // Clear the buffer to free memory
        self.atmos_probe_buffer.clear();

        Ok(())
    }

    fn write_buffered_samples(&mut self, channel_count: usize) -> Result<()> {
        // Clone the buffer to avoid borrowing conflicts
        let buffered_frames = self.atmos_probe_buffer.clone();
        let apply_bed_conform = self.bed_conform && self.has_atmos;
        let original_channel_count = self.buffered_channel_count;

        if let Some(ref mut writer) = self.audio_writer {
            for frame_samples in &buffered_frames {
                if apply_bed_conform {
                    // Apply bed conformance to buffered samples
                    let bed_indices = self.bed_indices.clone();
                    let conformed_samples = Self::apply_bed_conformance_to_samples_static(
                        frame_samples,
                        original_channel_count,
                        &bed_indices,
                    );
                    writer.write_pcm_samples(&conformed_samples, channel_count)?;
                } else {
                    writer.write_pcm_samples(frame_samples, channel_count)?;
                }
            }
        }
        Ok(())
    }

    fn apply_bed_conformance_to_samples_static(
        samples: &[i32],
        original_channel_count: usize,
        bed_indices: &Option<Vec<usize>>,
    ) -> Vec<i32> {
        let bed_indices = bed_indices.as_deref().unwrap_or(EMPTY_BED_INDICES);
        let target_bed_channels = TARGET_BED_CHANNELS;
        let num_object_channels = original_channel_count.saturating_sub(bed_indices.len());
        let sample_frames = samples.len() / original_channel_count;

        let mut conformed_samples =
            Vec::with_capacity(sample_frames * (target_bed_channels + num_object_channels));

        for sample_idx in 0..sample_frames {
            // Handle bed channels (0-9)
            for target_bed_ch in 0..target_bed_channels {
                if let Some(source_ch_pos) =
                    bed_indices.iter().position(|&idx| idx == target_bed_ch)
                {
                    conformed_samples
                        .push(samples[sample_idx * original_channel_count + source_ch_pos]);
                } else {
                    conformed_samples.push(0i32);
                }
            }

            // Handle object channels
            for obj_ch in 0..num_object_channels {
                let source_ch = bed_indices.len() + obj_ch;
                conformed_samples.push(samples[sample_idx * original_channel_count + source_ch]);
            }
        }

        conformed_samples
    }

    fn write_bed_conform_samples(&mut self, decoded: &truehd::process::decode::DecodedAccessUnit) {
        if self.cached_bed_channel_map.is_some() {
            // Use optimized direct-to-buffer approach
            self.apply_bed_conformance_direct(decoded);
        } else {
            // Fallback to original method for edge cases
            let frame_samples = Self::extract_samples_from_frame(decoded);
            let conformed_samples = Self::apply_bed_conformance_to_samples_static(
                &frame_samples,
                decoded.channel_count,
                &self.bed_indices,
            );
            self.sample_buffer.clear();
            self.sample_buffer.extend(conformed_samples);
        }
    }

    fn get_channel_info(&self, original_count: usize) -> ChannelInfo {
        if self.has_atmos {
            let bed_indices = self.bed_indices.as_deref().unwrap_or(EMPTY_BED_INDICES);
            ChannelInfo::for_atmos(original_count, bed_indices, self.bed_conform)
        } else {
            ChannelInfo::for_regular(original_count)
        }
    }

    /// Get channel info that always shows Atmos breakdown when available (for display purposes)
    fn get_display_channel_info(&self, original_count: usize) -> ChannelInfo {
        if self.has_atmos && self.bed_indices.is_some() {
            let bed_indices = self.bed_indices.as_deref().unwrap_or(EMPTY_BED_INDICES);
            ChannelInfo::for_atmos(original_count, bed_indices, self.bed_conform)
        } else {
            ChannelInfo::for_regular(original_count)
        }
    }

    fn build_bed_conformance_cache(&mut self, original_channel_count: usize) {
        if self.bed_conform && self.has_atmos && self.cached_bed_indices.is_none() {
            let bed_indices = self
                .bed_indices
                .as_deref()
                .unwrap_or(EMPTY_BED_INDICES)
                .to_vec();
            self.cached_num_object_channels =
                original_channel_count.saturating_sub(bed_indices.len());

            // Pre-compute bed channel mapping for faster lookup
            let mut channel_map = vec![None; TARGET_BED_CHANNELS];
            for (source_pos, &target_ch) in bed_indices.iter().enumerate() {
                if target_ch < TARGET_BED_CHANNELS {
                    channel_map[target_ch] = Some(source_pos);
                }
            }

            self.cached_bed_indices = Some(bed_indices);
            self.cached_bed_channel_map = Some(channel_map);
        }
    }

    fn apply_bed_conformance_direct(
        &mut self,
        decoded: &truehd::process::decode::DecodedAccessUnit,
    ) {
        let channel_map = self.cached_bed_channel_map.as_ref().unwrap();
        let bed_indices_len = self.cached_bed_indices.as_ref().unwrap().len();
        let total_output_channels = TARGET_BED_CHANNELS + self.cached_num_object_channels;

        self.sample_buffer.clear();
        self.sample_buffer
            .reserve(decoded.sample_length * total_output_channels);

        for sample_idx in 0..decoded.sample_length {
            // Handle bed channels using pre-computed mapping
            for &source_pos in channel_map.iter().take(TARGET_BED_CHANNELS) {
                if let Some(source_pos) = source_pos {
                    self.sample_buffer
                        .push(decoded.pcm_data[sample_idx][source_pos]);
                } else {
                    self.sample_buffer.push(0i32);
                }
            }

            // Handle object channels
            for obj_ch in 0..self.cached_num_object_channels {
                let source_ch = bed_indices_len + obj_ch;
                self.sample_buffer
                    .push(decoded.pcm_data[sample_idx][source_ch]);
            }
        }
    }

    fn create_audio_writer_for_format(
        &self,
        path: PathBuf,
        sample_rate: u32,
        channel_count: usize,
    ) -> Result<AudioWriter> {
        match self.format {
            AudioFormat::Caf => AudioWriter::create_caf(path, sample_rate, channel_count as u32),
            AudioFormat::Pcm => AudioWriter::create_pcm(path),
            AudioFormat::W64 => AudioWriter::create_w64(path, sample_rate, channel_count as u32),
        }
    }

    fn calculate_and_cache_channel_count(
        &mut self,
        decoded: &truehd::process::decode::DecodedAccessUnit,
    ) {
        if self.effective_channel_count.is_none() {
            let channel_info = self.get_channel_info(decoded.channel_count);
            self.effective_channel_count = Some(channel_info.total_channels);

            // Also store original channel info for buffering purposes
            if self.buffered_channel_count == 0 {
                self.buffered_channel_count = decoded.channel_count;
                self.buffered_sample_rate = decoded.sampling_frequency;
            }
        }
    }

    fn write_metadata(
        &mut self,
        oamd: &truehd::structs::oamd::ObjectAudioMetadataPayload,
        sample_rate: u32,
    ) -> Result<()> {
        if self.output_path.is_none() {
            return Ok(());
        }

        if self.metadata_writer.is_none()
            && let Some(ref base_path) = self.output_path
        {
            let metadata_path = self.get_metadata_path(base_path);
            if !metadata_path.as_os_str().is_empty() {
                info!("Creating metadata file: {}", metadata_path.display());
                self.metadata_writer = Some(BufWriter::new(File::create(metadata_path)?));
            }
        }

        if let Some(ref mut writer) = self.metadata_writer {
            let segment_relative_pos = if self.is_segmented {
                self.total_samples
                    .saturating_sub(self.segment_start_samples)
            } else {
                self.total_samples
            };

            let mut conf =
                Configuration::with_oamd_payload(oamd, sample_rate, segment_relative_pos);

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

            write!(writer, "{oamd_str}")?;
            writer.flush()?;
        }
        Ok(())
    }

    fn ensure_audio_writer(
        &mut self,
        decoded: &truehd::process::decode::DecodedAccessUnit,
    ) -> Result<()> {
        // Don't create audio writer if we're still in probing phase
        if self.atmos_probing {
            return Ok(());
        }

        if self.audio_writer.is_none() {
            // Make sure we have calculated the channel count
            self.calculate_and_cache_channel_count(decoded);
            let channel_count = self.effective_channel_count.unwrap();

            if let Some(ref base_path) = self.output_path {
                let audio_path = if self.has_atmos {
                    self.get_atmos_audio_path(base_path)
                } else {
                    self.get_audio_path(base_path)
                };

                let channel_info = self.get_channel_info(decoded.channel_count);
                info!(
                    "Creating audio file: {} ({})",
                    audio_path.display(),
                    channel_info.description()
                );

                self.audio_writer = Some(self.create_audio_writer_for_format(
                    audio_path.clone(),
                    decoded.sampling_frequency,
                    channel_count,
                )?);
                self.current_audio_path = Some(audio_path);
            }
        }
        Ok(())
    }

    fn handle_atmos_rename(
        &mut self,
        decoded: &truehd::process::decode::DecodedAccessUnit,
    ) -> Result<()> {
        if let (Some(base_path), Some(current_path)) = (&self.output_path, &self.current_audio_path)
        {
            let new_path = self.get_atmos_audio_path(base_path);
            if current_path != &new_path {
                // Use centralized display logic that shows Atmos breakdown when available
                let channel_info = self.get_display_channel_info(decoded.channel_count);

                info!(
                    "Renaming audio file to: {} ({})",
                    new_path.display(),
                    channel_info.description()
                );

                if let Some(mut writer) = self.audio_writer.take() {
                    writer.finish()?;
                    drop(writer);
                    std::fs::rename(current_path, &new_path)?;

                    // Channel count is already calculated and cached

                    // Reopen the renamed file
                    let file = std::fs::OpenOptions::new()
                        .read(true)
                        .write(true)
                        .open(&new_path)?;

                    self.audio_writer = Some(match self.format {
                        AudioFormat::Caf => {
                            use std::io::{BufWriter, Seek};
                            let mut temp_file = file.try_clone()?;
                            let file_info = parse_caf_file(&mut temp_file)?;
                            temp_file.seek(std::io::SeekFrom::End(0))?;
                            AudioWriter::Caf(CAFWriter::from_parsed_info(
                                BufWriter::new(file),
                                file_info,
                            )?)
                        }
                        _ => unreachable!("Presentation 3 should always use CAF format"),
                    });
                    self.current_audio_path = Some(new_path);
                }
            }
        }
        Ok(())
    }

    fn update_progress(&mut self, pb: &Option<ProgressBar>, start_time: Instant) -> Result<()> {
        if let Some(pb) = pb
            && self.decoded_frames.is_multiple_of(30)
        {
            let elapsed = start_time.elapsed();
            let audio_duration = self.total_samples as f64 / self.final_sample_rate as f64;
            let speed = audio_duration / elapsed.as_secs_f64();

            // Use reusable buffer for progress message to avoid allocations
            self.progress_buffer.clear();
            use crate::timestamp::time_str;
            use std::fmt::Write;
            write!(
                &mut self.progress_buffer,
                "speed: {:.1}x | timestamp: {}",
                speed,
                time_str(audio_duration)
            )
            .expect("Writing to String should not fail");
            pb.set_message(self.progress_buffer.clone());
        }
        Ok(())
    }

    fn get_segmented_base_path(&self, base_path: &Path) -> PathBuf {
        if let Some(ref current_path) = self.current_audio_path {
            // For segmented mode, derive base path by removing specific audio extensions
            // but preserve the original base structure
            let mut segmented_base = current_path.clone();
            let path_str = current_path.to_string_lossy();

            if path_str.ends_with(".atmos.audio") {
                // Remove .atmos.audio extension
                let base_name = path_str.strip_suffix(".atmos.audio").unwrap();
                segmented_base = PathBuf::from(base_name);
            } else if path_str.ends_with(".atmos.metadata") {
                // Remove .atmos.metadata extension
                let base_name = path_str.strip_suffix(".atmos.metadata").unwrap();
                segmented_base = PathBuf::from(base_name);
            } else {
                // For other extensions, remove just the last extension
                if let Some(stem) = current_path.file_stem() {
                    segmented_base = current_path.with_file_name(stem);
                }
            }
            segmented_base
        } else {
            // Use the segment naming convention for base path
            self.get_base_path_with_segment(base_path)
        }
    }

    fn get_base_path_with_segment(&self, base_path: &Path) -> PathBuf {
        if self.segment_index > 0 {
            let filename = base_path.file_name().unwrap().to_string_lossy();
            let parent = base_path.parent().unwrap_or(Path::new("."));
            parent.join(format!("{}_{}", filename, self.au_index))
        } else {
            base_path.to_path_buf()
        }
    }

    fn get_audio_path(&self, base_path: &Path) -> PathBuf {
        let base = self.get_base_path_with_segment(base_path);
        if self.has_atmos {
            self.get_atmos_audio_path_from_base(&base)
        } else {
            match self.format {
                AudioFormat::Caf => base.with_extension("caf"),
                AudioFormat::Pcm => base.with_extension("pcm"),
                AudioFormat::W64 => base.with_extension("wav"),
            }
        }
    }

    fn get_atmos_audio_path(&self, base_path: &Path) -> PathBuf {
        let base = self.get_base_path_with_segment(base_path);
        self.get_atmos_audio_path_from_base(&base)
    }

    fn get_atmos_audio_path_from_base(&self, base: &Path) -> PathBuf {
        base.with_extension("atmos.audio")
    }

    fn get_metadata_path(&self, base_path: &Path) -> PathBuf {
        let base = self.get_base_path_with_segment(base_path);
        if self.has_atmos {
            base.with_extension("atmos.metadata")
        } else {
            PathBuf::new() // Empty path for non-Atmos
        }
    }

    pub(crate) fn finalize(&mut self) -> Result<()> {
        if let Some(ref mut writer) = self.audio_writer {
            writer.finish()?;
        }
        if let Some(ref mut writer) = self.metadata_writer {
            writer.flush()?;
        }
        Ok(())
    }
}

fn apply_warp_mode_override(damf_data: &mut Data, warp_mode: Option<WarpMode>) {
    if let Some(cli_warp_mode) = warp_mode
        && let Some(presentation) = damf_data.presentations_mut().first_mut()
        && presentation.warp_mode.is_none()
    {
        presentation.warp_mode = Some(cli_warp_mode.into());
    }
}

pub fn create_damf_header_file(
    base_path: &Path,
    oamd: &truehd::structs::oamd::ObjectAudioMetadataPayload,
    warp_mode: Option<WarpMode>,
) -> Result<()> {
    let header_path = create_path_with_suffix(base_path, "atmos");
    let mut damf_data = Data::with_oamd_payload(oamd, base_path);
    apply_warp_mode_override(&mut damf_data, warp_mode);
    write_damf_header_to_file(&header_path, &damf_data)
}

pub fn rewrite_damf_header_for_bed_conform(
    base_path: &Path,
    oamd: &truehd::structs::oamd::ObjectAudioMetadataPayload,
    warp_mode: Option<WarpMode>,
) -> Result<()> {
    let header_path = create_path_with_suffix(base_path, "atmos");
    let mut damf_data = Data::with_oamd_payload_bed_conform(oamd, base_path);
    apply_warp_mode_override(&mut damf_data, warp_mode);
    write_damf_header_to_file(&header_path, &damf_data)
}

fn write_damf_header_to_file(header_path: &Path, damf_data: &Data) -> Result<()> {
    info!("Creating DAMF header file: {}", header_path.display());
    let mut header_writer = BufWriter::new(File::create(header_path)?);
    let header_str = &damf_data.serialize_damf();
    write!(header_writer, "{header_str}")?;
    header_writer.flush()?;
    Ok(())
}

fn create_path_with_suffix(base_path: &Path, suffix: &str) -> PathBuf {
    let mut path = base_path.to_path_buf();
    let new_name = format!(
        "{}.{}",
        base_path.file_name().unwrap().to_string_lossy(),
        suffix
    );
    path.set_file_name(new_name);
    path
}
