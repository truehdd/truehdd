use std::path::PathBuf;

use clap::{Args, Parser as ClapParser, Subcommand, ValueEnum};

pub const VERSION_INFO: &str = concat!(
    env!("VERGEN_GIT_DESCRIBE"),
    " (truehd library ",
    env!("TRUEHD_VERSION"),
    ") Built: ",
    env!("BUILD_TIMESTAMP")
);

#[derive(Debug, ClapParser)]
#[command(
    name       = env!("CARGO_PKG_NAME"),
    version    = VERSION_INFO,
    author     = env!("CARGO_PKG_AUTHORS"),
    about      = env!("CARGO_PKG_DESCRIPTION"),
    long_about = None,
)]
pub struct Cli {
    /// Set the log level
    #[arg(long, global = true, value_enum, default_value_t = LogLevel::Info)]
    pub loglevel: LogLevel,

    /// Treat warnings as fatal errors (fail on first warning).
    #[arg(long, global = true)]
    pub strict: bool,

    /// Log output format.
    #[arg(long, global = true, value_enum, default_value_t = LogFormat::Plain)]
    pub log_format: LogFormat,

    /// Show progress bars during operations.
    #[arg(long, global = true)]
    pub progress: bool,

    /// Choose an operation to perform.
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Decode the specified TrueHD stream into PCM audio.
    Decode(DecodeArgs),

    /// Print stream information
    Info(InfoArgs),
}

#[derive(Debug, Args)]
pub struct DecodeArgs {
    /// Input TrueHD bitstream (use "-" for stdin).
    #[arg(value_name = "INPUT")]
    pub input: PathBuf,

    /// Output path for audio and metadata files.
    #[arg(long, value_name = "PATH")]
    pub output_path: Option<PathBuf>,

    /// Audio format for output (ignored for presentation 3 which always uses CAF).
    #[arg(long, value_enum, default_value_t = AudioFormat::Caf)]
    pub format: AudioFormat,

    /// Presentation index (0-3).
    #[arg(long, value_name = "INDEX", default_value_t = 3)]
    pub presentation: u8,

    /// Disable progress estimation
    #[arg(long)]
    pub no_estimate_progress: bool,

    /// Enable bed conformance for Atmos content
    #[arg(long)]
    pub bed_conform: bool,

    /// Specify warp mode when not present in metadata
    #[arg(long, value_enum)]
    pub warp_mode: Option<WarpMode>,
}

#[derive(Debug, Args)]
pub struct InfoArgs {
    /// Input TrueHD bitstream.
    #[arg(value_name = "INPUT")]
    pub input: PathBuf,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum LogLevel {
    /// Disable logging output.
    Off,
    /// No output except errors.
    Error,
    /// Show warnings and errors.
    Warn,
    /// Show info, warnings and errors (default).
    Info,
    /// Show debug, info, warnings and errors.
    Debug,
    /// Show all log messages including trace.
    Trace,
}

impl LogLevel {
    /// Convert LogLevel to log::LevelFilter
    pub fn to_level_filter(self) -> log::LevelFilter {
        match self {
            LogLevel::Off => log::LevelFilter::Off,
            LogLevel::Error => log::LevelFilter::Error,
            LogLevel::Warn => log::LevelFilter::Warn,
            LogLevel::Info => log::LevelFilter::Info,
            LogLevel::Debug => log::LevelFilter::Debug,
            LogLevel::Trace => log::LevelFilter::Trace,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum LogFormat {
    /// Colorized human-readable text.
    Plain,
    /// Structured JSON per log record.
    Json,
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq)]
pub enum AudioFormat {
    /// Core Audio Format.
    Caf,
    /// Raw PCM format (24-bit little-endian).
    Pcm,
    /// Wave64 format (.wav extension).
    W64,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum WarpMode {
    /// Direct render
    Normal,
    /// Direct render with room balance
    Warping,
    /// Dolby Pro Logic IIx
    #[value(name = "prologiciix")]
    ProLogicIIx,
    /// Standard (Lo/Ro)
    #[value(name = "loro")]
    LoRo,
}

impl From<WarpMode> for crate::damf::WarpMode {
    fn from(warp_mode: WarpMode) -> Self {
        match warp_mode {
            WarpMode::Normal => Self::Normal,
            WarpMode::Warping => Self::Warping,
            WarpMode::ProLogicIIx => Self::ProLogicIIx,
            WarpMode::LoRo => Self::LoRo,
        }
    }
}
