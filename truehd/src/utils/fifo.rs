//! Byte-domain decoder FIFO depth model.
//!
//! The time-domain model bounds *when* an access unit may arrive; this bounds *how many
//! bytes* the decoder has to hold while it does. Five accumulators sum the bytes of every
//! access unit that has arrived but has not yet been played out: one for substream 0, one
//! each for the substream sets the 6-, 8- and 16-channel decoders read, and one for the
//! whole stream. Each has its own byte cap.
//!
//! The window works as follows: a record's bytes stay buffered until
//! playback has passed its output time by more than one access unit (strictly), the record
//! for the arriving access unit is written before the drain runs, the drain runs before the
//! add, and the peak is sampled both after the drain and after the add. An underrun clamps
//! the accumulator to zero and stops that drain pass.

/// Number of depth accumulators, indexed by [`Accumulator`].
pub const ACCUMULATORS: usize = 5;

const RING: usize = 128;

/// Which set of substreams an accumulator sums over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Accumulator {
    Substream0,
    Sixch,
    Eightch,
    Sixteench,
    WholeStream,
}

/// FBB byte caps for substream 0, indexed by `substream_info - 4`. An index outside the
/// table acts as zero.
const FBB_SUBSTREAM0_CAP: [usize; 10] = [90_000, 30_000, 0, 0, 30_000, 0, 0, 0, 0, 0];

/// FBB byte caps shared by the 6-channel sum and the whole stream, indexed by
/// `substream_info - 4`.
const FBB_STREAM_CAP: [usize; 10] = [90_000, 30_000, 0, 30_000, 0, 0, 0, 0, 0, 90_000];

impl Accumulator {
    pub const ALL: [Accumulator; ACCUMULATORS] = [
        Accumulator::Substream0,
        Accumulator::Sixch,
        Accumulator::Eightch,
        Accumulator::Sixteench,
        Accumulator::WholeStream,
    ];

    /// Byte cap for FBA streams.
    pub const fn fba_cap(&self) -> usize {
        match self {
            // 15000 bytes per channel over the two channels of substream 0
            Accumulator::Substream0 => 30_000,
            Accumulator::Sixch => 90_000,
            _ => 120_000,
        }
    }

    /// Byte cap for FBB streams, or `None` for the sums FBB never checks.
    ///
    /// FBB indexes its cap tables with `substream_info - 4` and never checks the 8- or
    /// 16-channel sums; the 8-channel contribution is not even accumulated for FBB.
    pub fn fbb_cap(&self, substream_info: u8) -> Option<usize> {
        let index = (substream_info as usize).wrapping_sub(4);

        match self {
            Accumulator::Substream0 => Some(FBB_SUBSTREAM0_CAP.get(index).copied().unwrap_or(0)),
            Accumulator::Sixch | Accumulator::WholeStream => {
                Some(FBB_STREAM_CAP.get(index).copied().unwrap_or(0))
            }
            Accumulator::Eightch | Accumulator::Sixteench => None,
        }
    }

    /// Name used in the depth diagnostics.
    pub const fn group(&self) -> &'static str {
        match self {
            Accumulator::Substream0 => "substream 0",
            Accumulator::Sixch => "the 6-channel decoder",
            Accumulator::Eightch => "the 8-channel decoder",
            Accumulator::Sixteench => "the 16-channel decoder",
            Accumulator::WholeStream => "the whole stream",
        }
    }
}

/// Outcome of admitting one access unit to the window.
#[derive(Clone, Copy, Debug, Default)]
pub struct FifoDepthReport {
    /// Depth of each accumulator with the new access unit included.
    pub depths: [usize; ACCUMULATORS],
    /// Accumulator that held fewer bytes than the access unit leaving it, if any.
    pub underrun: Option<usize>,
}

/// Sliding-window byte occupancy of the decoder input FIFO.
#[derive(Clone, Copy, Debug)]
pub struct FifoDepthState {
    contribution: [[u32; RING]; ACCUMULATORS],
    removal: [usize; RING],
    read: usize,
    write: usize,
    depth: [usize; ACCUMULATORS],
    peak: [usize; ACCUMULATORS],
}

impl Default for FifoDepthState {
    fn default() -> Self {
        Self {
            contribution: [[0; RING]; ACCUMULATORS],
            removal: [0; RING],
            read: 0,
            write: 0,
            depth: [0; ACCUMULATORS],
            peak: [0; ACCUMULATORS],
        }
    }
}

impl FifoDepthState {
    /// Admits an access unit and evicts every record playback has strictly passed.
    ///
    /// `playhead` is the branch-adjusted, unwrapped input timing of the arriving access
    /// unit; `removal` is its output time plus one access unit, the moment its own bytes
    /// stop being needed. A record leaves only once `playhead` exceeds its removal time
    /// strictly, which is what keeps the departing unit in the window one access unit
    /// longer than a `<=` drain would.
    ///
    /// The window keeps no occupancy count: the new
    /// record is written into its slot before the drain runs, and a stream that buffers
    /// more than 128 access units silently overwrites its oldest record rather than
    /// evicting it. The one divergence is a guard stopping a single push from draining
    /// more than one full ring.
    pub fn push(
        &mut self,
        playhead: usize,
        removal: usize,
        contribution: [usize; ACCUMULATORS],
    ) -> FifoDepthReport {
        let mut report = FifoDepthReport::default();

        let slot = self.write;
        self.removal[slot] = removal;

        for (k, item) in contribution.iter().enumerate() {
            self.contribution[k][slot] = *item as u32;
        }

        let mut drained = 0;

        while playhead > self.removal[self.read] && drained < RING {
            let read = self.read;

            for (k, depth) in self.depth.iter_mut().enumerate() {
                let leaving = self.contribution[k][read] as usize;

                match depth.checked_sub(leaving) {
                    Some(remaining) => *depth = remaining,
                    None => {
                        *depth = 0;
                        report.underrun = Some(k);
                    }
                }

                self.peak[k] = self.peak[k].max(*depth);
            }

            self.read = (read + 1) & (RING - 1);
            drained += 1;

            if report.underrun.is_some() {
                break;
            }
        }

        for (k, depth) in self.depth.iter_mut().enumerate() {
            *depth += contribution[k];
            self.peak[k] = self.peak[k].max(*depth);
        }

        self.write = (slot + 1) & (RING - 1);

        report.depths = self.depth;
        report
    }

    /// Deepest each accumulator has been over the stream.
    pub fn peaks(&self) -> [usize; ACCUMULATORS] {
        self.peak
    }

    /// Access units currently held in the window.
    pub fn buffered(&self) -> usize {
        self.write.wrapping_sub(self.read) & (RING - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNIT: [usize; ACCUMULATORS] = [10, 20, 30, 40, 50];

    #[test]
    fn a_record_stays_until_playback_strictly_passes_its_removal_time() {
        let mut fifo = FifoDepthState::default();

        // Three access units 40 samples apart; each leaves the window at output time
        // plus one access unit, here arrival + 100.
        for i in 0..3 {
            let arrival = i * 40;
            fifo.push(arrival, arrival + 100, UNIT);
        }
        assert_eq!(fifo.buffered(), 3);
        assert_eq!(fifo.peaks(), [30, 60, 90, 120, 150]);

        // Playback exactly at a removal time does NOT drain: the drain is strict.
        let report = fifo.push(100, 220, UNIT);
        assert_eq!(fifo.buffered(), 4);
        assert_eq!(report.depths, [40, 80, 120, 160, 200]);

        // One sample later the first record leaves, and only the first.
        let report = fifo.push(101, 221, UNIT);
        assert_eq!(fifo.buffered(), 4);
        assert_eq!(report.depths, [40, 80, 120, 160, 200]);
        assert!(report.underrun.is_none());
    }

    #[test]
    fn the_drain_runs_before_the_add_so_the_peak_includes_the_new_unit() {
        let mut fifo = FifoDepthState::default();

        // The first record is still buffered when the second arrives, so the peak holds
        // both, even though the first would have drained at any playhead past 50.
        fifo.push(0, 50, UNIT);
        fifo.push(50, 100, UNIT);
        assert_eq!(fifo.peaks(), [20, 40, 60, 80, 100]);

        // At 51 the first record drains before the third is added: same peak.
        fifo.push(51, 150, UNIT);
        assert_eq!(fifo.peaks(), [20, 40, 60, 80, 100]);
    }

    #[test]
    fn an_underrun_clamps_to_zero_and_stops_the_drain_pass() {
        let mut fifo = FifoDepthState::default();

        fifo.push(0, 10, [10, 20, 30, 40, 50]);
        fifo.push(1, 11, [10, 20, 30, 40, 50]);

        // Corrupt the model by force: drain everything against a record claiming more
        // than the accumulators hold. Both stale records are past removal, but the
        // underrun on the first stops the pass before the second is touched.
        let mut broken = FifoDepthState::default();
        broken.push(0, 10, [100, 100, 100, 100, 100]);
        broken.push(1, 11, [10, 20, 30, 40, 50]);
        // depths now [110, 120, 130, 140, 150]; drain a record of 200 each
        broken.contribution.iter_mut().for_each(|c| c[0] = 200);

        let report = broken.push(100, 200, [1, 1, 1, 1, 1]);
        assert!(report.underrun.is_some());
        // clamped to zero on every accumulator, then the new unit was added, and the
        // second stale record was left in place
        assert_eq!(report.depths, [1, 1, 1, 1, 1]);
        assert_eq!(broken.buffered(), 2);
    }

    #[test]
    fn the_ring_overwrites_rather_than_evicts_past_128_records() {
        let mut fifo = FifoDepthState::default();

        for i in 0..(RING * 2) {
            fifo.push(i, usize::MAX, [1, 1, 1, 1, 1]);
        }

        // Nothing ever drained, so the depth kept the full count even though the ring
        // only remembers the last 128 records.
        assert_eq!(fifo.peaks(), [RING * 2; ACCUMULATORS]);
    }
}
