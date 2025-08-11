use crate::caf::CAFWriter;
use crate::wav::WAVWriter;
use anyhow::Result;
use std::fs::File;
use std::io::{BufWriter, Seek, Write};
use std::path::{Path, PathBuf};

use super::super::command::AudioFormat;

pub fn create_path_with_suffix(base_path: &Path, suffix: &str) -> PathBuf {
    let mut path = base_path.to_path_buf();
    let new_name = format!(
        "{}.{}",
        base_path.file_name().unwrap().to_string_lossy(),
        suffix
    );
    path.set_file_name(new_name);
    path
}

pub fn create_path_with_extension(base_path: &Path, expected_ext: &str) -> PathBuf {
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

pub fn create_output_paths(
    base_path: &Path,
    format: AudioFormat,
    has_atmos: bool,
) -> (PathBuf, PathBuf) {
    let audio_ext = match (format, has_atmos) {
        (AudioFormat::Caf, false) => "caf",
        (AudioFormat::Pcm, false) => "pcm",
        (AudioFormat::W64, false) => "wav",
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

pub enum AudioWriter {
    Pcm(BufWriter<File>),
    Caf(CAFWriter<BufWriter<File>>),
    W64(WAVWriter<File>),
}

impl AudioWriter {
    pub fn create_pcm(path: PathBuf) -> Result<Self> {
        let pcm_writer = BufWriter::new(File::create(path)?);
        Ok(AudioWriter::Pcm(pcm_writer))
    }

    pub fn create_caf(path: PathBuf, sample_rate: u32, channel_count: u32) -> Result<Self> {
        let mut caf_writer = CAFWriter::new(BufWriter::new(File::create(path)?));
        caf_writer.configure_audio_format(sample_rate, channel_count, 24)?;
        caf_writer.write_header()?;
        Ok(AudioWriter::Caf(caf_writer))
    }

    pub fn create_w64(path: PathBuf, sample_rate: u32, channel_count: u32) -> Result<Self> {
        let mut w64_writer = WAVWriter::new(File::create(path)?);
        w64_writer.configure_audio_format(sample_rate, channel_count, 24)?;
        w64_writer.write_header()?;
        Ok(AudioWriter::W64(w64_writer))
    }

    pub fn write_pcm_samples(&mut self, samples: &[i32], channel_count: usize) -> Result<()> {
        match self {
            AudioWriter::Pcm(pcm_writer) => {
                for sample_idx in 0..(samples.len() / channel_count) {
                    for ch in 0..channel_count {
                        let sample = samples[sample_idx * channel_count + ch];
                        let bytes = sample.to_le_bytes();
                        pcm_writer.write_all(&bytes[..3])?;
                    }
                }
            }
            AudioWriter::Caf(caf_writer) => {
                caf_writer.write_pcm_24bit_as_packed(samples)?;
            }
            AudioWriter::W64(w64_writer) => {
                w64_writer.write_pcm_24bit_as_packed(samples)?;
            }
        }
        Ok(())
    }

    pub fn close_and_drop(self) -> Result<()> {
        match self {
            AudioWriter::Pcm(mut w) => {
                w.flush()?;
                drop(w);
            }
            AudioWriter::W64(mut w) => {
                w.finish()?;
                drop(w);
            }
            AudioWriter::Caf(mut w) => {
                w.finish()?;
                drop(w);
            }
        }
        Ok(())
    }

    pub fn finish(&mut self) -> Result<()> {
        match self {
            AudioWriter::Caf(caf_writer) => {
                caf_writer.finish()?;
            }
            AudioWriter::Pcm(pcm_writer) => {
                pcm_writer.flush()?;
            }
            AudioWriter::W64(w64_writer) => {
                w64_writer.finish()?;
            }
        }
        Ok(())
    }

    pub fn flush(&mut self) -> Result<()> {
        match self {
            AudioWriter::Pcm(pcm_writer) => {
                pcm_writer.flush()?;
            }
            AudioWriter::Caf(_) => {
                // CAF writer doesn't need explicit flush for our use case
            }
            AudioWriter::W64(_) => {
                // W64 writer handles flushing internally
            }
        }
        Ok(())
    }
}

pub fn create_caf_writer_from_existing_file(file: File) -> Result<CAFWriter<BufWriter<File>>> {
    let mut temp_file = file.try_clone()?;
    let file_info = crate::caf::parse_caf_file(&mut temp_file)?;
    temp_file.seek(std::io::SeekFrom::End(0))?;
    Ok(CAFWriter::from_parsed_info(
        BufWriter::new(file),
        file_info,
    )?)
}
