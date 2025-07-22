#[macro_export]
macro_rules! log_or_err {
    ($state:expr, $level:expr, $err:expr $(,)?) => {{
        if $level <= $state.fail_level {
            return Err($err);
        } else {
            match $level {
                ::log::Level::Error => ::log::error!("{}", $err),
                ::log::Level::Warn => ::log::warn!("{}", $err),
                ::log::Level::Info => ::log::info!("{}", $err),
                ::log::Level::Debug => ::log::debug!("{}", $err),
                ::log::Level::Trace => ::log::trace!("{}", $err),
            }
        }
    }};
}

#[derive(thiserror::Error, Debug)]
pub enum DecodeError {
    #[error("Positive saturation from the recorrelator: value = {0}")]
    RecorrelatorPositiveSaturation(i64),

    #[error("Negative saturation from the recorrelator: value = {0}")]
    RecorrelatorNegativeSaturation(i64),

    #[error("Filter B input exceeds 32-bit range: value = {0}")]
    FilterBInputTooWide32(i64),

    #[error("Filter B input exceeds 24-bit range: value = {0}")]
    FilterBInputTooWide24(i64),

    #[error("Invalid presentation index: {0}")]
    InvalidPresentation(usize),
}

#[derive(thiserror::Error, Debug)]
pub enum ExtractError {
    #[error("Mismatch in substream count: found {found}, expected {expected}")]
    SubstreamMismatch { found: usize, expected: usize },

    #[error("Parity check failed for frame")]
    ParityCheckFailed,

    #[error("Insufficient buffer data for frame extraction")]
    InsufficientData,

    #[error("Invalid sync pattern detected")]
    InvalidSyncPattern,
}

#[derive(thiserror::Error, Debug)]
pub enum ParseError {
    #[error("No substream context available")]
    NoSubstream,

    #[error("Invalid substream context index ({0} > {1})")]
    InvalidSubstreamIndex(usize, usize),
}

#[derive(thiserror::Error, Debug)]
pub enum AccessUnitError {
    #[error("Missing major sync at stream start")]
    MissingInitialSync,

    #[error("FBA stream major syncs must occur at intervals not exceeding 128 access units")]
    FbaSyncTooFar,

    #[error("No substream")]
    NoSubstream,

    #[error("mlp_sync must consist of an integral number of bytes")]
    MisalignedSync,

    #[error("mlp_sync failed the nibble check. Calculated {0:#X}")]
    NibbleParity(u8),

    #[error("Timing too short: input_timing[n]-input_timing[n-1] = {0} - {1} < {2}")]
    TimingTooShort(usize, usize, usize),

    #[error("Timing too short after jump: input_timing[n]-input_timing[n-1] < samples_per_au / 4")]
    TimingTooShortAfterJump,

    #[error("Timing shorter than previous duration")]
    TimingShorterThanPrevious,

    #[error("Timing shorter than previous duration after jump")]
    TimingShorterThanPreviousAfterJump,

    #[error("Data rate exceeds peak_data_rate")]
    DataRateExceeded,

    #[error("Data rate exceeds peak_data_rate after jump")]
    DataRateExceededAfterJump,

    #[error("input_timing[n]-input_timing[n-1] > samples_per_75ms")]
    TimingTooLong,

    #[error("input_timing[n]-input_timing[n-1] > samples_per_75ms after jump")]
    TimingTooLongAfterJump,

    #[error("Access unit too long: {0} > {1}")]
    AccessUnitTooLong(usize, usize),
}

#[derive(thiserror::Error, Debug)]
pub enum BlockError {
    #[error("block_size must be between 8 and 160. Read {0}")]
    InvalidBlockSizeRange(usize),

    #[error("block_size must not exceed samples_per_au = {max}, got {actual}")]
    BlockSizeExceedsAU { max: usize, actual: usize },

    #[error("output_shift[{index}] = {value} exceeds max_shift {max}, substream {substream}")]
    OutputShiftTooLarge {
        index: usize,
        value: i8,
        max: i8,
        substream: usize,
    },

    #[error("block_data_bits must be <= 16000. Read {0}")]
    BlockDataBitsTooLarge(u16),

    #[error("FIFO {substream} latency must be constant when bit 15 of flags is set")]
    LatencyInconsistent { substream: usize },

    #[error("duration[n] > latency[n] ({duration} > {latency})")]
    DurationExceedsLatency { duration: usize, latency: usize },

    #[error("latency[n] > samples_per_75ms ({latency} > {samples})")]
    LatencyTooHigh { latency: usize, samples: u32 },

    #[error("latency[n] < samples_per_au ({latency} < {au})")]
    LatencyTooLow { latency: usize, au: usize },

    #[error("huff_lsbs[{channel}] = {actual} exceeds max_lsbs {max}")]
    HuffLsbsTooLarge {
        channel: usize,
        actual: usize,
        max: usize,
    },

    #[error("quantiser_step_size must not exceed huff_lsbs")]
    QuantiserStepTooLarge,

    #[error("ninth bit of huffman msbs must be 1")]
    HuffmanNinthBitMissing,

    #[error("Positive saturation from huffman decode")]
    HuffmanPositiveSaturation,

    #[error("Negative saturation from huffman decode")]
    HuffmanNegativeSaturation,

    #[error("A huffman-encoded sample must not use more than 29 bits")]
    HuffmanSampleTooLong,

    #[error("block_data bit count mismatch: expected {expected}, got {actual}")]
    BlockDataBitCountMismatch { expected: u16, actual: u64 },
}

#[derive(thiserror::Error, Debug)]
pub enum ChannelError {
    #[error("Total filter order for Filters A and B must be ≤ 8. Got {a} + {b}")]
    FilterOrderTooHigh { a: u8, b: u8 },

    #[error("Mismatched coeff_Q for Filters A and B on channel {chan}: A = {a_q}, B = {b_q}")]
    CoeffQMismatch { chan: usize, a_q: u8, b_q: u8 },

    #[error("huff_lsbs[{chan}] must be ≤ {max}, got {actual}")]
    HuffLsbsTooLarge { chan: usize, max: u32, actual: u32 },
}

#[derive(thiserror::Error, Debug)]
pub enum ExtraDataError {
    #[error("EXTRA_DATA does not begin on a byte boundary")]
    MisalignedExtraDataStart,

    #[error("EXTRA_DATA should be all zeros")]
    PaddingNotZero,

    #[error("extra_data_length check nibble failed. Calculated {0:#X}")]
    LengthParityFailed(u8),

    #[error(
        "extra_data_length exceeds remaining bits in AU. extra_data_length = {length}, only {remaining} bits remain"
    )]
    ExtraDataTooLong { length: u16, remaining: usize },

    #[error(
        "evo_frame_byte_length exceeds extra_data_length capacity. evo_frame_byte_length = {evo_len}, extra_data_length = {extra_len}"
    )]
    EvoFrameTooLong { evo_len: u16, extra_len: u16 },

    #[error("evo_frame() in extra_data does not begin on a byte boundary")]
    EvoFrameMisaligned,

    #[error(
        "evo_frame() padding bits should be all zeros. Please submit a sample if you see this."
    )]
    EvoFramePaddingNotZero,

    #[error(
        "extra_data_parity check failed on evolution payload. Expected {expected:#X}, Read {actual:#X}"
    )]
    ExtraDataParityMismatch { expected: u8, actual: u8 },
}

#[derive(thiserror::Error, Debug)]
pub enum FilterError {
    #[error("Filter A must have order ≤ 8. Got {0}")]
    FilterAOrderTooHigh(u8),

    #[error("Filter B must have order ≤ 4. Got {0}")]
    FilterBOrderTooHigh(u8),

    #[error("coeff_Q must be ≥ 8 and ≤ 15. Got {0}")]
    InvalidCoeffQ(u8),

    #[error("coeff_bits must be between 1 and 16. Got {0}")]
    InvalidCoeffBits(u8),

    #[error("coeff_shift must be ≤ 15. Got {0}")]
    InvalidCoeffShift(u8),

    #[error("coeff_bits + coeff_shift must be ≤ 16. Got {0}")]
    TotalCoeffBitsTooLarge(u8),

    #[error("coeff cannot take value -32768")]
    InvalidCoeffValue,

    #[error("Filter A cannot use new filter states (new_states must be false)")]
    FilterANewStatesNotAllowed,

    #[error("state must be within 24-bit range, got {0:#08X}")]
    FilterStateOutOfRange(i32),
}

#[derive(thiserror::Error, Debug)]
pub enum MatrixError {
    #[error("matrix_ch[{index}] must be ≤ {max} (max_matrix_chan). Read {actual}")]
    MatrixChannelTooHigh {
        index: usize,
        max: usize,
        actual: u8,
    },

    #[error("frac_bits must be ≤ 14. Read {0}")]
    FracBitsTooHigh(u8),

    #[error(
        "Cannot use lsb_bypass in substream 0 when substream_info = {info:#02X} or sampling frequency is 192kHz or 176.4kHz"
    )]
    InvalidLsbBypass { info: u8 },
}

#[derive(thiserror::Error, Debug)]
pub enum RestartHeaderError {
    #[error(
        "output_timing must match across all substreams. Read {read}, substream {substream}, expected {expected}"
    )]
    OutputTimingMismatch {
        read: u16,
        substream: usize,
        expected: usize,
    },

    #[error("output_timing failure after jump: Read {read}, expected {expected}")]
    OutputTimingAfterJump { read: usize, expected: usize },

    #[error("Invalid output_timing. Read {read}, expected {expected}")]
    InvalidOutputTiming { read: usize, expected: usize },

    #[error("Substream 1 must use sync word 0x31EB unless it is last in 6ch presentation")]
    InvalidSyncBForSubstream1,

    #[error("Substream 0 must use sync word 0x31EA. Got 0x31EB")]
    InvalidSyncBForSubstream0,

    #[error("Sync word 0x31EC only allowed for substream 3. Got {0}")]
    InvalidSyncC(u16),

    #[error(
        "Second occurrence of max_bits does not match first. First: {first:#02X}, Second: {second:#02X}"
    )]
    MaxBitsMismatch { first: u8, second: u8 },

    #[error("Channel assignment ch_assign[{index}] = {value} exceeds max_matrix_chan {max}")]
    ChannelAssignTooHigh { index: usize, value: u8, max: u8 },

    #[error(
        "In substream 0, ch_assign[{index}] = {value} must equal index when sampling rate is ≥ 176.4kHz"
    )]
    ChannelAssignMisordered { index: usize, value: u8 },

    #[error("ch_assign must be a permutation of 0..{0}")]
    ChannelAssignDuplicate(u8),

    #[error("CRC mismatch in restart_header. Calculated {calculated:#02X}, Read {read:#02X}")]
    RestartHeaderCrcMismatch { calculated: u8, read: u8 },

    #[error("Stream is invalid.")]
    InvalidStream,

    #[error(
        "lossless_check failed for substream {substream}. Calculated {calculated:#02X}, Read {read:#02X}"
    )]
    LosslessCheckMismatch {
        substream: usize,
        calculated: i32,
        read: u8,
    },
}

#[derive(thiserror::Error, Debug)]
pub enum SubstreamError {
    #[error("extra_substream_word must be false in FBB streams")]
    InvalidExtraSubstreamWordFbb,

    #[error("restart_nonexistent must be {expected} in access_unit with{suffix} major_sync_info")]
    InvalidRestartNonexistent { expected: bool, suffix: String },

    #[error("Too many blocks in the substream segment. Got {0}")]
    TooManyBlocks(usize),

    #[error("substream_segment for substream {0} does not start on an even byte boundary")]
    UnalignedSegmentStart(usize),

    #[error("substream_segment for substream {0} does not end on an even byte boundary")]
    UnalignedSegmentEnd(usize),

    #[error(
        "substream_end address does not match substream_end_ptr for substream {substream}. Read {read:#03X}, expected {expected:#03X}"
    )]
    SubstreamEndMismatch {
        substream: usize,
        read: u64,
        expected: u64,
    },

    #[error(
        "Parity check failed on substream_segment for substream {substream}. Calculated {calculated:#X}, Read {read:#X}"
    )]
    ParityMismatch {
        substream: usize,
        calculated: u8,
        read: u8,
    },

    #[error(
        "CRC failed on substream_segment for substream {substream}. Calculated {calculated:#X}, Read {read:#X}"
    )]
    CrcMismatch {
        substream: usize,
        calculated: u8,
        read: u8,
    },
}

#[derive(thiserror::Error, Debug)]
pub enum SyncError {
    #[error("Invalid format_sync, Read {0:#08X}")]
    InvalidFormatSync(u32),

    #[error("Invalid format_info: audio_sampling_frequency_{index}. Read {value:#01X}")]
    InvalidAudioSamplingFreq { index: u8, value: u8 },

    #[error("Invalid signature in major_sync_info. Read {0:#04X}, expected 0xB752")]
    InvalidMajorSyncSignature(u16),

    #[error("Reserved bits in flags should be 0. Read {0:#04X}")]
    ReservedFlagsNonZero(u16),

    #[error(
        "Flags must be constant throughout the stream. Read {read:#04X}, expected {expected:#04X}"
    )]
    FlagsMismatch { read: u16, expected: u16 },

    #[error(
        "peak_data_rate must be constant throughout the stream. Read {read}, expected {expected}"
    )]
    PeakDataRateMismatch { read: u16, expected: usize },

    #[error("substreams must be constant throughout the stream. Read {read}, expected {expected}")]
    SubstreamCountMismatch { read: usize, expected: usize },

    #[error("Invalid major_sync_info, CRC failed. Calculated {calculated:#04X}, Read {read:#04X}")]
    MajorSyncCrcMismatch { calculated: u16, read: u16 },
}

#[derive(thiserror::Error, Debug)]
pub enum TimestampError {
    #[error("Invalid Timestamp sync bytes")]
    InvalidSyncBytes,

    #[error("parse_bcd16: Invalid BCD digit")]
    InvalidBcdDigit,
}
