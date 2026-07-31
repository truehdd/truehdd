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

#[derive(Debug, Clone, PartialEq)]
pub enum PresentationSelection {
    Single(u8),
    Multiple(Vec<u8>),
    All,
    Max,
}

impl PresentationSelection {
    /// Presentations to request from the decoder. Unavailable ones are
    /// remapped or dropped by the decoder's presentation map.
    pub fn to_required_presentations(&self) -> [bool; 4] {
        let mut required = [false; 4];
        match self {
            PresentationSelection::Single(p) => required[*p as usize] = true,
            PresentationSelection::Multiple(presentations) => {
                for &p in presentations {
                    required[p as usize] = true;
                }
            }
            PresentationSelection::All => required = [true; 4],
            PresentationSelection::Max => required[3] = true,
        }
        required
    }

    /// Whether at most one output file is produced (no filename suffixes).
    pub fn is_single_output(&self) -> bool {
        matches!(
            self,
            PresentationSelection::Single(_) | PresentationSelection::Max
        ) || matches!(self, PresentationSelection::Multiple(p) if p.len() == 1)
    }
}

impl std::fmt::Display for PresentationSelection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PresentationSelection::Single(p) => write!(f, "{p}"),
            PresentationSelection::Multiple(presentations) => {
                let list: Vec<String> = presentations.iter().map(|p| p.to_string()).collect();
                write!(f, "{}", list.join(","))
            }
            PresentationSelection::All => write!(f, "all"),
            PresentationSelection::Max => write!(f, "max"),
        }
    }
}

impl std::str::FromStr for PresentationSelection {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "all" => Ok(PresentationSelection::All),
            "max" => Ok(PresentationSelection::Max),
            _ if s.contains(',') => {
                let presentations: Result<Vec<u8>, _> =
                    s.split(',').map(str::trim).map(str::parse).collect();
                match presentations {
                    Ok(mut presentations) => {
                        if presentations.iter().any(|&p| p > 3) {
                            return Err("presentation indices must be 0-3".to_string());
                        }
                        presentations.sort_unstable();
                        presentations.dedup();
                        Ok(PresentationSelection::Multiple(presentations))
                    }
                    Err(_) => Err("invalid presentation list; use e.g. \"0,1,3\"".to_string()),
                }
            }
            _ => match s.parse::<u8>() {
                Ok(p) if p <= 3 => Ok(PresentationSelection::Single(p)),
                Ok(_) => Err("presentation index must be 0-3".to_string()),
                Err(_) => {
                    Err("expected an index (0-3), a list (0,1,3), \"all\" or \"max\"".to_string())
                }
            },
        }
    }
}

#[derive(Debug, Args)]
pub struct DecodeArgs {
    /// Input TrueHD bitstream (use "-" for stdin).
    #[arg(value_name = "INPUT")]
    pub input: PathBuf,

    /// Output path for audio and metadata files.
    #[arg(long, value_name = "PATH")]
    pub output_path: Option<PathBuf>,

    /// Audio format for output (presentation 3 always uses CAF).
    #[arg(long, value_enum, default_value_t = AudioFormat::Caf)]
    pub format: AudioFormat,

    /// Presentations to decode: an index (0-3), a list (0,1,3), "all",
    /// or "max" for the highest available presentation.
    #[arg(long, value_name = "SELECTION", default_value = "max")]
    pub presentation: PresentationSelection,

    /// Disable progress estimation
    #[arg(long)]
    pub no_estimate_progress: bool,

    /// Enable bed conformance for Atmos content
    #[arg(long)]
    pub bed_conform: bool,

    /// Write only object audio metadata, skipping PCM output
    #[arg(long)]
    pub metadata_only: bool,

    /// Print a machine-readable result summary on stdout
    #[arg(long)]
    pub json: bool,

    /// Specify warp mode when not present in metadata
    #[arg(long, value_enum)]
    pub warp_mode: Option<WarpMode>,

    /// Access units to probe for Atmos metadata with --bed-conform (12000 is about 10s at 48 kHz)
    #[arg(long, default_value_t = 12000)]
    pub probe_range: u64,
}
