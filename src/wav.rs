use std::io::{self, BufWriter, Seek, SeekFrom, Write};

// W64 GUIDs as defined in Sony Wave64 specification
pub const W64_RIFF_GUID: [u8; 16] = [
    0x72, 0x69, 0x66, 0x66, 0x2E, 0x91, 0xCF, 0x11, 0xA5, 0xD6, 0x28, 0xDB, 0x04, 0xC1, 0x00, 0x00,
];
pub const W64_WAVE_GUID: [u8; 16] = [
    0x77, 0x61, 0x76, 0x65, 0xF3, 0xAC, 0xD3, 0x11, 0x8C, 0xD1, 0x00, 0xC0, 0x4F, 0x8E, 0xDB, 0x8A,
];
pub const W64_FMT_GUID: [u8; 16] = [
    0x66, 0x6D, 0x74, 0x20, 0xF3, 0xAC, 0xD3, 0x11, 0x8C, 0xD1, 0x00, 0xC0, 0x4F, 0x8E, 0xDB, 0x8A,
];
pub const W64_DATA_GUID: [u8; 16] = [
    0x64, 0x61, 0x74, 0x61, 0xF3, 0xAC, 0xD3, 0x11, 0x8C, 0xD1, 0x00, 0xC0, 0x4F, 0x8E, 0xDB, 0x8A,
];

/// Sony Wave64 file writer for 24-bit PCM audio (.wav extension)
const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

/// KSDATAFORMAT_SUBTYPE_PCM, the subformat the extensible header names.
const KSDATAFORMAT_SUBTYPE_PCM: [u8; 16] = [
    0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71,
];

pub struct WAVWriter<W: Write + Seek> {
    writer: BufWriter<W>,
    data_size_position: u64,
    data_written: u64,
    sample_rate: u32,
    channels: u32,
    bits_per_sample: u32,
    channel_mask: Option<u32>,
    file_size_position: u64,
}

impl<W: Write + Seek> WAVWriter<W> {
    /// Create a new W64 writer
    pub fn new(writer: W) -> Self {
        Self {
            writer: BufWriter::new(writer),
            data_size_position: 0,
            data_written: 0,
            sample_rate: 48000,
            channels: 2,
            bits_per_sample: 24,
            channel_mask: None,
            file_size_position: 0,
        }
    }

    /// Configure audio format parameters
    pub fn configure_audio_format(
        &mut self,
        sample_rate: u32,
        channels: u32,
        bits_per_sample: u32,
        channel_mask: Option<u32>,
    ) -> io::Result<()> {
        if self.data_written > 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Cannot change format after writing data",
            ));
        }

        self.channel_mask = channel_mask;
        self.sample_rate = sample_rate;
        self.channels = channels;
        self.bits_per_sample = bits_per_sample;
        Ok(())
    }

    /// Write W64 file header
    pub fn write_header(&mut self) -> io::Result<()> {
        // W64 RIFF chunk
        self.writer.write_all(&W64_RIFF_GUID)?;
        self.file_size_position = self.writer.stream_position()?;
        self.writer.write_all(&0u64.to_le_bytes())?; // File size (to be updated later)
        self.writer.write_all(&W64_WAVE_GUID)?;

        // W64 fmt chunk. A channel mask needs the extensible form, which is the 16-byte
        // fmt data plus cbSize, the valid-bit count, the mask and a subformat GUID.
        self.writer.write_all(&W64_FMT_GUID)?;
        let fmt_data_size = if self.channel_mask.is_some() {
            40u64
        } else {
            16u64
        };
        self.writer
            .write_all(&(24u64 + fmt_data_size).to_le_bytes())?;

        let format_tag = if self.channel_mask.is_some() {
            WAVE_FORMAT_EXTENSIBLE
        } else {
            WAVE_FORMAT_PCM
        };
        self.writer.write_all(&format_tag.to_le_bytes())?;
        self.writer
            .write_all(&(self.channels as u16).to_le_bytes())?;
        self.writer.write_all(&self.sample_rate.to_le_bytes())?;

        let byte_rate = self.sample_rate * self.channels * (self.bits_per_sample / 8);
        self.writer.write_all(&byte_rate.to_le_bytes())?;

        let block_align = self.channels * (self.bits_per_sample / 8);
        self.writer.write_all(&(block_align as u16).to_le_bytes())?;
        self.writer
            .write_all(&(self.bits_per_sample as u16).to_le_bytes())?;

        if let Some(mask) = self.channel_mask {
            self.writer.write_all(&22u16.to_le_bytes())?; // cbSize
            self.writer
                .write_all(&(self.bits_per_sample as u16).to_le_bytes())?; // valid bits
            self.writer.write_all(&mask.to_le_bytes())?;
            self.writer.write_all(&KSDATAFORMAT_SUBTYPE_PCM)?;
        }

        // W64 data chunk
        self.writer.write_all(&W64_DATA_GUID)?;
        self.data_size_position = self.writer.stream_position()?;
        self.writer.write_all(&0u64.to_le_bytes())?; // Data size (to be updated later)

        Ok(())
    }

    /// Write 24-bit PCM samples (input as i32, written as 24-bit little-endian)
    pub fn write_pcm_24bit_as_packed(&mut self, samples: &[i32]) -> io::Result<()> {
        for &sample in samples {
            // Convert i32 to 24-bit little-endian
            let bytes = sample.to_le_bytes();
            self.writer.write_all(&bytes[0..3])?; // Take the 3 least significant bytes
            self.data_written += 3;
        }
        Ok(())
    }

    /// Finish writing and update file size headers
    pub fn finish(&mut self) -> io::Result<()> {
        // Flush any remaining data
        self.writer.flush()?;

        let current_pos = self.writer.stream_position()?;

        // Update data chunk size (includes GUID + size = 24 bytes)
        self.writer.seek(SeekFrom::Start(self.data_size_position))?;
        let data_chunk_size = self.data_written + 24;
        self.writer.write_all(&data_chunk_size.to_le_bytes())?;

        // Update W64 file size
        self.writer.seek(SeekFrom::Start(self.file_size_position))?;
        self.writer.write_all(&current_pos.to_le_bytes())?;

        // Return to end of file
        self.writer.seek(SeekFrom::Start(current_pos))?;
        self.writer.flush()?;

        Ok(())
    }

    /// Get the underlying writer
    pub fn into_inner(self) -> io::Result<W> {
        self.writer.into_inner().map_err(|e| e.into_error())
    }

    /// Get statistics about written data
    pub fn stats(&self) -> WAVStats {
        WAVStats {
            data_written: self.data_written,
            sample_rate: self.sample_rate,
            channels: self.channels,
            bits_per_sample: self.bits_per_sample,
        }
    }
}

/// Statistics about W64 file writing
#[derive(Debug, Clone)]
pub struct WAVStats {
    pub data_written: u64,
    pub sample_rate: u32,
    pub channels: u32,
    pub bits_per_sample: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_w64_header_write() -> io::Result<()> {
        let buffer = Vec::new();
        let cursor = Cursor::new(buffer);
        let mut writer = WAVWriter::new(cursor);

        writer.configure_audio_format(48000, 2, 24, None)?;
        writer.write_header()?;

        let cursor = writer.into_inner()?;
        let buffer = cursor.into_inner();

        // Check W64 RIFF GUID
        assert_eq!(&buffer[0..16], &W64_RIFF_GUID);
        // Check W64 WAVE GUID (starts at offset 24)
        assert_eq!(&buffer[24..40], &W64_WAVE_GUID);
        // Check W64 FMT GUID (starts at offset 40)
        assert_eq!(&buffer[40..56], &W64_FMT_GUID);

        Ok(())
    }

    #[test]
    fn test_w64_sample_write() -> io::Result<()> {
        let buffer = Vec::new();
        let cursor = Cursor::new(buffer);
        let mut writer = WAVWriter::new(cursor);

        writer.configure_audio_format(48000, 2, 24, None)?;
        writer.write_header()?;

        // Write some test samples
        let samples = vec![0x123456i32, 0x789ABCi32];
        writer.write_pcm_24bit_as_packed(&samples)?;

        let stats = writer.stats();
        assert_eq!(stats.data_written, 6); // 2 samples x 3 bytes each

        writer.finish()?;

        Ok(())
    }
}
