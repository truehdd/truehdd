//! Parse timing, compiled out unless the `perf` feature is on.
//!
//! Timing the parse from outside only gives a total. Attributing it to
//! bitstream structures - Huffman decoding against LSB bypass against the
//! conformance checks - needs measurements taken where those structures are
//! read, so the hooks live in the parser and cost nothing when disabled.

use std::time::Duration;

/// Where parse time went for one access unit.
///
/// Durations are sums over the access unit: block-level entries add up across
/// every block of every substream segment. Read with
/// [`Parser::last_parse_stats`](crate::process::parse::Parser::last_parse_stats).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ParserPerfStats {
    pub access_unit_total: Duration,
    pub substream_directories: Duration,
    pub substream_segments: Duration,
    pub substream_segment_blocks: Duration,
    pub substream_segment_tail: Duration,
    pub extra_data: Duration,
    pub block_header_setup: Duration,
    pub block_bypassed_lsb: Duration,
    pub block_huffman_decode: Duration,
    pub block_checks: Duration,
}

/// Start of a timed region.
///
/// Without the `perf` feature this is a zero-sized value and both of its
/// methods compile away, so no clock is read.
#[cfg(feature = "perf")]
#[derive(Clone, Copy)]
pub struct Timer(std::time::Instant);

#[cfg(not(feature = "perf"))]
#[derive(Clone, Copy)]
pub struct Timer;

impl Timer {
    #[inline(always)]
    pub fn start() -> Self {
        #[cfg(feature = "perf")]
        {
            Timer(std::time::Instant::now())
        }
        #[cfg(not(feature = "perf"))]
        {
            Timer
        }
    }

    /// Adds the time since [`Timer::start`] to `slot`.
    #[inline(always)]
    pub fn record(self, slot: &mut Duration) {
        #[cfg(feature = "perf")]
        {
            *slot += self.0.elapsed();
        }
        #[cfg(not(feature = "perf"))]
        {
            let _ = slot;
        }
    }
}
