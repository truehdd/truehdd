//! Data structures representing format components.
//!
//! Contains structured representations of bitstream elements including
//! access units, audio blocks, sync patterns, channel configurations, and
//! matrix parameters used throughout the decoding pipeline.

pub mod access_unit;
pub mod block;
pub mod channel;
pub mod evolution;
pub mod extra_data;
pub mod filter;
pub mod matrix;
pub mod oamd;
pub mod restart_header;
pub mod substream;
pub mod sync;
pub mod timestamp;
