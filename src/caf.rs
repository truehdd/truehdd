use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::byteorder::{WriteBytesBe, WriteBytesLe};
use crate::impl_u32_enum;
use truehdd_macros::{ToBytes, caf_chunk_type};

pub fn write_caf_file_header<W: Write>(writer: &mut W) -> io::Result<()> {
    writer.write_all(b"caff")?;
    writer.write_all(&1u16.to_be_bytes())?;
    writer.write_all(&0u16.to_be_bytes())?;

    Ok(())
}

pub trait CAFChunk {
    fn chunk_type(&self) -> &[u8; 4];
    fn chunk_data(&self) -> Vec<u8>;

    fn write_all<W: Write>(&self, writer: &mut W) -> io::Result<()> {
        writer.write_all(self.chunk_type())?;

        let chunk_date = self.chunk_data();
        writer.write_all(&(chunk_date.len() as u64).to_be_bytes())?;
        writer.write_all(&self.chunk_data())?;

        Ok(())
    }
}

#[derive(ToBytes)]
#[caf_chunk_type(b"desc")]
pub struct AudioFormat {
    pub sample_rate: f64,
    pub format_id: u32,
    pub format_flags: u32,
    pub bytes_per_packet: u32,
    pub frames_per_packet: u32,
    pub channels_per_frame: u32,
    pub bits_per_channel: u32,
}

#[derive(Debug, ToBytes)]
#[caf_chunk_type(b"chan")]
pub struct ChannelLayout {
    pub channel_layout_tag: ChannelLayoutTag,
    pub channel_bitmap: ChannelBitmap,
    pub chennel_description: Vec<ChennelDescription>,
}

#[allow(non_camel_case_types)]
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelLayoutTag {
    /// Use the array of AudioChannelDescriptions to define the mapping.
    UseChannelDescriptions = 0 << 16,
    /// Use the bitmap to define the mapping.
    UseChannelBitmap = 1 << 16,

    // 1 channel layout
    /// Standard mono stream
    Mono = (100 << 16) | 1,

    // 2 channel layouts
    /// Standard stereo stream (L R)
    Stereo = (101 << 16) | 2,
    /// Standard stereo stream (L R) - implied headphone playback
    StereoHeadphones = (102 << 16) | 2,
    /// Matrix encoded stereo stream (Lt, Rt)
    MatrixStereo = (103 << 16) | 2,
    /// Mid/Side recording
    MidSide = (104 << 16) | 2,
    /// Coincident mic pair (often 2 figure 8's)
    XY = (105 << 16) | 2,
    /// Binaural stereo (left, right)
    Binaural = (106 << 16) | 2,

    // Symmetric arrangements
    /// Ambisonic B Format (W, X, Y, Z)
    AmbisonicBFormat = (107 << 16) | 4,
    /// Quadraphonic (front left, front right, back left, back right)
    Quadraphonic = (108 << 16) | 4,
    /// Pentagonal (left, right, rear left, rear right, center)
    Pentagonal = (109 << 16) | 5,
    /// Hexagonal (left, right, rear left, rear right, center, rear)
    Hexagonal = (110 << 16) | 6,
    /// Octagonal (front left, front right, rear left, rear right, front center, rear center, side left, side right)
    Octagonal = (111 << 16) | 8,
    /// Cube (left, right, rear left, rear right, top left, top right, top rear left, top rear right)
    Cube = (112 << 16) | 8,

    // MPEG defined layouts
    MPEG_3_0_A = (113 << 16) | 3,        // L R C
    MPEG_3_0_B = (114 << 16) | 3,        // C L R
    MPEG_4_0_A = (115 << 16) | 4,        // L R C Cs
    MPEG_4_0_B = (116 << 16) | 4,        // C L R Cs
    MPEG_5_0_A = (117 << 16) | 5,        // L R C Ls Rs
    MPEG_5_0_B = (118 << 16) | 5,        // L R Ls Rs C
    MPEG_5_0_C = (119 << 16) | 5,        // L C R Ls Rs
    MPEG_5_0_D = (120 << 16) | 5,        // C L R Ls Rs
    MPEG_5_1_A = (121 << 16) | 6,        // L R C LFE Ls Rs
    MPEG_5_1_B = (122 << 16) | 6,        // L R Ls Rs C LFE
    MPEG_5_1_C = (123 << 16) | 6,        // L C R Ls Rs LFE
    MPEG_5_1_D = (124 << 16) | 6,        // C L R Ls Rs LFE
    MPEG_6_1_A = (125 << 16) | 7,        // L R C LFE Ls Rs Cs
    MPEG_7_1_A = (126 << 16) | 8,        // L R C LFE Ls Rs Lc Rc
    MPEG_7_1_B = (127 << 16) | 8,        // C Lc Rc L R Ls Rs LFE
    MPEG_7_1_C = (128 << 16) | 8,        // L R C LFE Ls R Rls Rrs
    EmagicDefault_7_1 = (129 << 16) | 8, // L R Ls Rs C LFE Lc Rc
    SMPTE_DTV = (130 << 16) | 8,         // L R C LFE Ls Rs Lt Rt

    // ITU defined layouts
    ITU_2_1 = (131 << 16) | 3, // L R Cs
    ITU_2_2 = (132 << 16) | 4, // L R Ls Rs

    // DVD defined layouts
    DVD_4 = (133 << 16) | 3,  // L R LFE
    DVD_5 = (134 << 16) | 4,  // L R LFE Cs
    DVD_6 = (135 << 16) | 5,  // L R LFE Ls Rs
    DVD_10 = (136 << 16) | 4, // L R C LFE
    DVD_11 = (137 << 16) | 5, // L R C LFE Cs
    DVD_18 = (138 << 16) | 5, // L R Ls Rs LFE
    DVD_20 = (139 << 16) | 6, // L R Ls Rs C Cs
    DVD_21 = (140 << 16) | 7, // L R Ls Rs C Rls Rrs

    // AAC/MPEG-4
    AAC_6_0 = (141 << 16) | 6,       // C L R Ls Rs Cs
    AAC_6_1 = (142 << 16) | 7,       // C L R Ls Rs Cs Lfe
    AAC_7_0 = (143 << 16) | 7,       // C L R Ls Rs Rls Rrs
    AAC_Octagonal = (144 << 16) | 8, // C L R Ls Rs Rls Rrs Cs

    // TMH
    TMH_10_2_std = (145 << 16) | 16,
    TMH_10_2_full = (146 << 16) | 21,

    /// Reserved, do not use
    ReservedDoNotUse = 147 << 16,
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelBitmap {
    Left = 1 << 0,
    Right = 1 << 1,
    Center = 1 << 2,
    LFEScreen = 1 << 3,
    /// WAVE: "Back Left"
    LeftSurround = 1 << 4,
    /// WAVE: "Back Right"
    RightSurround = 1 << 5,
    LeftCenter = 1 << 6,
    RightCenter = 1 << 7,
    /// WAVE: "Back Center"
    CenterSurround = 1 << 8,
    /// WAVE: "Side Left"
    LeftSurroundDirect = 1 << 9,
    /// WAVE: "Side Right"
    RightSurroundDirect = 1 << 10,
    TopCenterSurround = 1 << 11,
    /// WAVE: "Top Front Left"
    VerticalHeightLeft = 1 << 12,
    /// WAVE: "Top Front Center"
    VerticalHeightCenter = 1 << 13,
    /// WAVE: "Top Front Right"
    VerticalHeightRight = 1 << 14,
    TopBackLeft = 1 << 15,
    TopBackCenter = 1 << 16,
    TopBackRight = 1 << 17,
}

#[derive(Debug, Clone, ToBytes)]
pub struct ChennelDescription {
    pub channel_label: ChannelLabel,
    pub channel_flags: u32,
    pub coordinates: [f32; 3],
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types, clippy::upper_case_acronyms)]
pub enum ChannelLabel {
    /// unknown role or unspecified other use for channel
    Unknown = 0xFFFFFFFF,
    /// channel is present, but has no intended role or destination
    Unused = 0,
    /// channel is described solely by the mCoordinates fields
    UseCoordinates = 100,

    Left = 1,
    Right = 2,
    Center = 3,
    LFEScreen = 4,
    /// WAVE (.wav files): "Back Left"
    LeftSurround = 5,
    /// WAVE: "Back Right"
    RightSurround = 6,
    LeftCenter = 7,
    RightCenter = 8,
    /// WAVE: "Back Center or plain \"Rear Surround\""
    CenterSurround = 9,
    /// WAVE: "Side Left"
    LeftSurroundDirect = 10,
    /// WAVE: "Side Right"
    RightSurroundDirect = 11,
    TopCenterSurround = 12,
    /// WAVE: "Top Front Left"
    VerticalHeightLeft = 13,
    /// WAVE: "Top Front Center"
    VerticalHeightCenter = 14,
    /// WAVE: "Top Front Right"
    VerticalHeightRight = 15,
    TopBackLeft = 16,
    TopBackCenter = 17,
    TopBackRight = 18,
    RearSurroundLeft = 33,
    RearSurroundRight = 34,
    LeftWide = 35,
    RightWide = 36,
    LFE2 = 37,
    /// matrix encoded 4 channels
    LeftTotal = 38,
    /// matrix encoded 4 channels
    RightTotal = 39,
    HearingImpaired = 40,
    Narration = 41,
    Mono = 42,
    DialogCentricMix = 43,
    /// back center, non diffuse
    CenterSurroundDirect = 44,

    // first order ambisonic channels
    AmbisonicW = 200,
    AmbisonicX = 201,
    AmbisonicY = 202,
    AmbisonicZ = 203,

    // Mid/Side Recording
    MSMid = 204,
    MSSide = 205,

    // X-Y Recording
    XYX = 206,
    XYY = 207,

    // other
    HeadphonesLeft = 301,
    HeadphonesRight = 302,
    ClickTrack = 304,
    ForeignLanguage = 305,
}

impl_u32_enum!(ChannelLayoutTag);
impl_u32_enum!(ChannelBitmap);
impl_u32_enum!(ChannelLabel);

/// PCM data type (integer vs floating point)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PCMDataType {
    SignedInteger,
    Float,
}

/// Audio data endianness
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endianness {
    BigEndian,
    LittleEndian,
}

/// Linear PCM format flags builder following Core Audio specification
#[derive(Debug, Clone, Copy)]
pub struct LinearPCMFormatFlags {
    /// kLinearPCMFormatFlagIsFloat (bit 0)
    pub is_float: bool,
    /// kLinearPCMFormatFlagIsLittleEndian (bit 1)
    pub is_little_endian: bool,
}

impl LinearPCMFormatFlags {
    /// Create format flags for the given PCM configuration
    pub fn new(data_type: PCMDataType, endianness: Endianness, _bits_per_channel: u32) -> Self {
        Self {
            is_float: matches!(data_type, PCMDataType::Float),
            is_little_endian: matches!(endianness, Endianness::LittleEndian),
        }
    }

    /// Convert to u32 format flags value
    pub fn to_u32(self) -> u32 {
        let mut flags = 0u32;

        if self.is_float {
            flags |= 1 << 0; // kLinearPCMFormatFlagIsFloat
        }
        if self.is_little_endian {
            flags |= 1 << 1; // kLinearPCMFormatFlagIsLittleEndian
        }

        flags
    }

    /// Create flags for big-endian signed integer PCM (most common for CAF)
    pub fn big_endian_signed_integer(bits_per_channel: u32) -> Self {
        Self::new(
            PCMDataType::SignedInteger,
            Endianness::BigEndian,
            bits_per_channel,
        )
    }

    /// Create flags for little-endian signed integer PCM
    pub fn little_endian_signed_integer(bits_per_channel: u32) -> Self {
        Self::new(
            PCMDataType::SignedInteger,
            Endianness::LittleEndian,
            bits_per_channel,
        )
    }

    /// Create flags for big-endian floating point PCM
    pub fn big_endian_float(bits_per_channel: u32) -> Self {
        Self::new(PCMDataType::Float, Endianness::BigEndian, bits_per_channel)
    }

    /// Create flags for little-endian floating point PCM
    pub fn little_endian_float(bits_per_channel: u32) -> Self {
        Self::new(
            PCMDataType::Float,
            Endianness::LittleEndian,
            bits_per_channel,
        )
    }
}

/// CAF writer that supports writing headers with unknown length and updating them later
pub struct CAFWriter<W: Write + Seek> {
    writer: W,
    audio_format: Option<AudioFormat>,
    channel_layout: Option<ChannelLayout>,
    data_chunk_start: Option<u64>,
    data_size_position: Option<u64>,
    data_written: u64,
    finished: bool,
}

impl<W: Write + Seek> CAFWriter<W> {
    /// Create a new CAF writer
    pub fn new(writer: W) -> Self {
        Self {
            writer,
            audio_format: None,
            channel_layout: None,
            data_chunk_start: None,
            data_size_position: None,
            data_written: 0,
            finished: false,
        }
    }

    /// Helper method to check if writer is already finished
    fn check_not_finished(&self) -> io::Result<()> {
        if self.finished {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Writer already finished",
            ));
        }
        Ok(())
    }

    /// Helper method to ensure header has been written
    fn ensure_header_written(&self) -> io::Result<()> {
        if self.data_chunk_start.is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Must call write_header() before this operation",
            ));
        }
        Ok(())
    }

    /// Helper method to get data size position, ensuring header was written
    fn get_data_size_position(&self) -> io::Result<u64> {
        self.data_size_position.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Must call write_header() before finish()",
            )
        })
    }

    /// Helper method to ensure audio format is configured
    fn ensure_audio_format(&self) -> io::Result<&AudioFormat> {
        self.audio_format.as_ref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Audio format must be set before writing header",
            )
        })
    }

    /// Set audio format parameters from decoded stream info
    pub fn set_audio_format(
        &mut self,
        sample_rate: f64,
        channels: u32,
        bits_per_channel: u32,
    ) -> io::Result<()> {
        self.set_audio_format_with_options(
            sample_rate,
            channels,
            bits_per_channel,
            PCMDataType::SignedInteger,
            Endianness::BigEndian,
        )
    }

    /// Set audio format with explicit PCM data type and endianness
    pub fn set_audio_format_with_options(
        &mut self,
        sample_rate: f64,
        channels: u32,
        bits_per_channel: u32,
        data_type: PCMDataType,
        endianness: Endianness,
    ) -> io::Result<()> {
        let format_flags = LinearPCMFormatFlags::new(data_type, endianness, bits_per_channel);

        self.audio_format = Some(AudioFormat {
            sample_rate,
            format_id: u32::from_be_bytes(*b"lpcm"),
            format_flags: format_flags.to_u32(),
            bytes_per_packet: (bits_per_channel / 8) * channels,
            frames_per_packet: 1,
            channels_per_frame: channels,
            bits_per_channel,
        });
        Ok(())
    }

    /// Set channel layout (optional)
    pub fn set_channel_layout(&mut self, layout: ChannelLayout) {
        self.channel_layout = Some(layout);
    }

    /// Create a basic channel layout for common configurations
    pub fn set_basic_channel_layout(&mut self, channels: u32) -> io::Result<()> {
        let layout_tag = match channels {
            1 => ChannelLayoutTag::Mono,
            2 => ChannelLayoutTag::Stereo,
            3 => ChannelLayoutTag::MPEG_3_0_A, // L R C
            4 => ChannelLayoutTag::Quadraphonic,
            5 => ChannelLayoutTag::MPEG_5_0_A, // L R C Ls Rs
            6 => ChannelLayoutTag::MPEG_5_1_A, // L R C LFE Ls Rs
            8 => ChannelLayoutTag::MPEG_7_1_A, // L R C LFE Ls Rs Lc Rc
            _ => return Ok(()),                // No standard layout for this channel count
        };

        self.channel_layout = Some(ChannelLayout {
            channel_layout_tag: layout_tag,
            channel_bitmap: ChannelBitmap::Left, // Not used when layout_tag is set
            chennel_description: Vec::new(),
        });
        Ok(())
    }

    /// Begin writing the CAF file. Must be called before write_data.
    pub fn write_header(&mut self) -> io::Result<()> {
        self.check_not_finished()?;
        self.ensure_audio_format()?;

        // Write CAF file header
        write_caf_file_header(&mut self.writer)?;

        // Write Audio Description chunk
        if let Some(ref audio_format) = self.audio_format {
            audio_format.write_all(&mut self.writer)?;
        }

        // Write Channel Layout chunk if present
        if let Some(ref layout) = self.channel_layout {
            layout.write_all(&mut self.writer)?;
        }

        // Write Data chunk header with unknown size (-1)
        self.writer.write_all(b"data")?; // chunk type
        self.data_size_position = Some(self.writer.stream_position()?);
        self.writer.write_all(&(-1i64).to_be_bytes())?; // unknown size

        // Write edit count (always 0 for new files)
        self.writer.write_all(&0u32.to_be_bytes())?;

        self.data_chunk_start = Some(self.writer.stream_position()?);
        Ok(())
    }

    /// Write audio data (PCM samples)
    pub fn write_data(&mut self, data: &[u8]) -> io::Result<()> {
        self.ensure_header_written()?;
        self.check_not_finished()?;

        self.writer.write_all(data)?;
        self.data_written += data.len() as u64;
        Ok(())
    }

    /// Finish writing and update the data chunk size
    pub fn finish(&mut self) -> io::Result<()> {
        if self.finished {
            return Ok(()); // Already finished
        }

        let data_size_pos = self.get_data_size_position()?;

        let current_pos = self.writer.stream_position()?;

        // Calculate the data chunk size (data written + 4 bytes for edit count)
        let chunk_size = self.data_written + 4;

        // Seek back to data size position and update it
        self.writer.seek(SeekFrom::Start(data_size_pos))?;
        self.writer.write_all(&(chunk_size as i64).to_be_bytes())?;

        // Seek back to end of file
        self.writer.seek(SeekFrom::Start(current_pos))?;

        self.finished = true;
        Ok(())
    }

    /// Get the underlying writer (consumes the CAFWriter)
    pub fn into_inner(mut self) -> io::Result<W> {
        use std::mem::ManuallyDrop;
        use std::ptr;

        if !self.finished {
            self.finish()?;
        }

        // Convert self to ManuallyDrop to prevent Drop from running
        let manual = ManuallyDrop::new(self);

        // Safety: We're taking ownership of writer and preventing Drop from running
        unsafe { Ok(ptr::read(&manual.writer)) }
    }

    /// Get statistics about the written data
    pub fn stats(&self) -> CAFWriterStats {
        CAFWriterStats {
            data_written: self.data_written,
            finished: self.finished,
        }
    }
}

/// Statistics about the CAF writer
#[derive(Debug, Clone)]
pub struct CAFWriterStats {
    pub data_written: u64,
    pub finished: bool,
}

/// Helper methods for working with TrueHD streams
impl<W: Write + Seek> CAFWriter<W> {
    /// Configure the writer from TrueHD stream parameters
    pub fn configure_audio_format(
        &mut self,
        sample_rate: u32,
        channels: u32,
        bits_per_channel: u32,
    ) -> io::Result<()> {
        // Set audio format
        self.set_audio_format(sample_rate as f64, channels, bits_per_channel)?;

        // Set basic channel layout based on channel count
        self.set_basic_channel_layout(channels)?;

        Ok(())
    }

    /// Convenience method to write 24-bit PCM data from TrueHD decoder
    /// Expects interleaved samples in i32 format (with 24-bit of effective data)
    pub fn write_pcm_24bit_as_packed(&mut self, samples: &[i32]) -> io::Result<()> {
        let mut buffer = Vec::with_capacity(samples.len() * 3);

        for &sample in samples {
            // Convert i32 to 24-bit big-endian (CAF standard)
            let bytes = sample.to_be_bytes();
            buffer.extend_from_slice(&bytes[1..4]); // Skip the most significant byte for 24-bit
        }

        self.write_data(&buffer)
    }
}

// Implement Drop to ensure finish() is called
impl<W: Write + Seek> Drop for CAFWriter<W> {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.finish();
        }
    }
}

/// Wrap an existing PCM file with a CAF header, converting it to a proper CAF file
///
/// This function is used when PCM data was written first, then Atmos was detected,
/// requiring the PCM data to be wrapped in a CAF container.
pub fn wrap_pcm_file_with_caf_header(
    pcm_file_path: &Path,
    sample_rate: f64,
    channels: u32,
    bits_per_channel: u32,
) -> io::Result<()> {
    // Open PCM file for reading
    let pcm_file = File::open(pcm_file_path)?;

    // Create a temporary file for the CAF output
    let temp_path = pcm_file_path.with_extension("caf.tmp");
    let temp_file = File::create(&temp_path)?;

    // Create CAF writer with little-endian format (since our PCM data is little-endian)
    let mut caf_writer = CAFWriter::new(BufWriter::new(temp_file));
    caf_writer.set_audio_format_with_options(
        sample_rate,
        channels,
        bits_per_channel,
        PCMDataType::SignedInteger,
        Endianness::LittleEndian, // Match the PCM data endianness
    )?;
    caf_writer.set_basic_channel_layout(channels)?;
    caf_writer.write_header()?;

    // Copy PCM data to the CAF file
    let mut pcm_reader = BufReader::new(pcm_file);
    let mut buffer = vec![0u8; 64 * 1024]; // 64KB buffer

    loop {
        let bytes_read = pcm_reader.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        caf_writer.write_data(&buffer[..bytes_read])?;
    }

    // Finalize the CAF file
    caf_writer.finish()?;
    drop(caf_writer);

    // Replace the original PCM file with the CAF file
    std::fs::rename(&temp_path, pcm_file_path)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_caf_writer_basic() -> io::Result<()> {
        let buffer = Vec::new();
        let cursor = Cursor::new(buffer);
        let mut writer = CAFWriter::new(cursor);

        // Configure for stereo 24-bit at 48kHz
        writer.set_audio_format(48000.0, 2, 24)?;
        writer.set_basic_channel_layout(2)?;
        writer.write_header()?;

        // Write some dummy audio data
        let audio_data = vec![0u8; 1024];
        writer.write_data(&audio_data)?;

        writer.finish()?;

        // Get the buffer back from the writer
        let cursor = writer.into_inner()?;
        let buffer = cursor.into_inner();

        // Check that we have a valid CAF file
        assert_eq!(&buffer[0..4], b"caff");
        assert!(buffer.len() > 50); // Should have header + chunks

        Ok(())
    }

    #[test]
    fn test_caf_writer_pcm_conversion() -> io::Result<()> {
        let buffer = Vec::new();
        let cursor = Cursor::new(buffer);
        let mut writer = CAFWriter::new(cursor);

        writer.configure_audio_format(48000, 2, 24)?;
        writer.write_header()?;

        // Test PCM conversion
        let samples = vec![0x123456i32, 0x789ABCi32]; // 24-bit samples
        writer.write_pcm_24bit_as_packed(&samples)?;

        let stats = writer.stats();
        assert_eq!(stats.data_written, 6); // 2 samples × 3 bytes each

        writer.finish()?;

        Ok(())
    }
}
