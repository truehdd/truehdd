#![doc = include_str!("../README.md")]
//!
//! ## Technical Overview
//!
//! Parser and decoder for Dolby TrueHD (MLP) bitstreams according to FBA syntax specification.
//!
//! ### Bitstream Organization
//!
//! **External Structure**: Access units containing MLP Syncs and substream segments.
//! **Internal Structure**: Blocks with optional restart headers.
//!
//! ### Audio Presentations
//!
//! - 2-channel (stereo, Lt/Rt, binaural, mono)
//! - 6-channel  
//! - 8-channel
//! - 16-channel
//!
//! ### Data Rate Management
//!
//! Variable bitrate compression with FIFO buffering. Peak data rates limited to 18 Mbps
//! for FBA streams.
//!
//! ## Quick Start
//!
//! Steps for processing audio streams:
//!
//! 1. Extract access units from a bitstream using [`process::extract::Extractor`]
//! 2. Parse access units into structured data using [`process::parse::Parser`]
//! 3. Decode audio to PCM samples using [`process::decode::Decoder`]
//!
//! ```rust,no_run
//! use truehd::process::{extract::Extractor, parse::Parser, decode::Decoder, EXAMPLE_DATA};
//!
//! // Initialize processing components
//! let mut extractor = Extractor::default();
//! let mut parser = Parser::default();
//! let mut decoder = Decoder::default();
//!
//! // Push bitstream data
//! let data = &EXAMPLE_DATA; // Example data
//! extractor.push_bytes(data);
//!
//! // Process frames with error recovery
//! for frame_result in extractor {
//!     match frame_result {
//!         Ok(frame) => {
//!             let access_unit = parser.parse(&frame)?;
//!             
//!             // Decode the first presentation
//!             let decoded = decoder.decode_presentation(&access_unit, 0)?;
//!             
//!             // Access PCM data
//!             let pcm_samples = &decoded.pcm_data;
//!         }
//!         Err(extract_error) => {
//!             // Handle extraction errors - stream continues automatically
//!             eprintln!("Frame extraction error: {}", extract_error);
//!         }
//!     }
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

/// Processing functionality for audio bitstreams.
///
/// 1. **Frame Extraction** ([`process::extract`]): Extracts access units from
///    bitstream data using sync pattern detection.
///
/// 2. **Parsing** ([`process::parse`]): Converts access unit data into structured
///    representations.
///
/// 3. **Decoding** ([`process::decode`]): Audio decoding using MLP algorithm.
pub mod process;

/// Data structures representing TrueHD format components.
///
/// - **Access Units** ([`structs::access_unit`]): Presentation units
/// - **Sync Patterns** ([`structs::sync`]): Major/minor sync detection
/// - **Substreams** ([`structs::substream`]): Audio channel groupings
/// - **Blocks** ([`structs::block`]): Compressed audio data
/// - **Restart Headers** ([`structs::restart_header`]): Decoder initialization parameters
/// - **Matrix Operations** ([`structs::matrix`]): Multi-channel decoding
/// - **Filters** ([`structs::filter`]): Prediction filters
pub mod structs;

/// Utility functions and supporting infrastructure.
///
/// - **Bitstream I/O** ([`utils::bitstream_io`]): Bit-level reading/writing
/// - **CRC Validation** ([`utils::crc`]): Error detection
/// - **Error Handling** ([`utils::errors`]): Error types
/// - **Timing** ([`utils::timing`]): FIFO timing calculations
/// - **Dithering** ([`utils::dither`]): Noise shaping
/// - **Buffer Management** ([`utils::buffer_pool`]): Memory allocation
pub mod utils;
