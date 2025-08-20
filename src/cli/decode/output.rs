use crate::caf::CAFWriter;
use crate::wav::WAVWriter;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

pub enum AudioWriter {
    Pcm(BufWriter<File>),
    Caf(CAFWriter<BufWriter<File>>),
    W64(WAVWriter<File>),
}

impl AudioWriter {
    pub fn create_pcm(path: PathBuf) -> anyhow::Result<Self> {
        let pcm_writer = BufWriter::new(File::create(path)?);
        Ok(AudioWriter::Pcm(pcm_writer))
    }

    pub fn create_caf(path: PathBuf, sample_rate: u32, channel_count: u32) -> anyhow::Result<Self> {
        let mut caf_writer = CAFWriter::new(BufWriter::new(File::create(path)?));
        caf_writer.configure_audio_format(sample_rate, channel_count, 24)?;
        caf_writer.write_header()?;
        Ok(AudioWriter::Caf(caf_writer))
    }

    pub fn create_w64(path: PathBuf, sample_rate: u32, channel_count: u32) -> anyhow::Result<Self> {
        let mut w64_writer = WAVWriter::new(File::create(path)?);
        w64_writer.configure_audio_format(sample_rate, channel_count, 24)?;
        w64_writer.write_header()?;
        Ok(AudioWriter::W64(w64_writer))
    }

    pub fn write_pcm_samples(
        &mut self,
        samples: &[i32],
        _channel_count: usize,
    ) -> anyhow::Result<()> {
        match self {
            AudioWriter::Pcm(pcm_writer) => {
                for &sample in samples {
                    let bytes = sample.to_le_bytes();
                    pcm_writer.write_all(&bytes[..3])?; // Write 24-bit
                }
                Ok(())
            }
            AudioWriter::Caf(caf_writer) => Ok(caf_writer.write_pcm_24bit_as_packed(samples)?),
            AudioWriter::W64(w64_writer) => Ok(w64_writer.write_pcm_24bit_as_packed(samples)?),
        }
    }

    pub fn finish(&mut self) -> anyhow::Result<()> {
        match self {
            AudioWriter::Pcm(pcm_writer) => Ok(pcm_writer.flush()?),
            AudioWriter::Caf(caf_writer) => Ok(caf_writer.finish()?),
            AudioWriter::W64(w64_writer) => Ok(w64_writer.finish()?),
        }
    }
}
