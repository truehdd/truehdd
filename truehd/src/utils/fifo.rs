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

/// Substreams the per-substream windows track, one per presentation slot.
pub const SUBSTREAMS: usize = crate::process::MAX_PRESENTATIONS;

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
///
/// A cap belongs to a decoder, not a substream: it is 15000 bytes per channel of the
/// presentation that decoder reconstructs, so a two-channel decoder allows 30000 and a
/// six-channel one 90000. Where substream 0 alone is the six-channel presentation, its
/// cap is that decoder's.
const FBB_SUBSTREAM0_CAP: [usize; 10] = [90_000, 30_000, 0, 30_000, 0, 0, 0, 0, 0, 30_000];

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
    ///
    /// Only the low nibble of `substream_info` is defined in FBB, so only it selects a cap.
    pub fn fbb_cap(&self, substream_info: u8) -> Option<usize> {
        let index = (substream_info as usize & 0xF).wrapping_sub(4);

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

/// What one access unit adds to the window, split by where the bytes come from.
///
/// `total` is what an accumulator is capped on. `stream` is the part of it that is
/// substream-segment payload — the audio the stream carries — and the remainder is the
/// container overhead priced around it: the access unit header, a major sync, the
/// directory words the accumulator covers and EXTRA_DATA. `substream` prices each
/// substream's own payload on its own, with no overhead at all.
#[derive(Clone, Copy, Debug, Default)]
pub struct FifoContribution {
    pub total: [usize; ACCUMULATORS],
    pub stream: [usize; ACCUMULATORS],
    pub substream: [usize; SUBSTREAMS],
}

/// Deepest one accumulator has been, and what it held at that moment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FifoPeak {
    pub total: usize,
    pub stream: usize,
    pub overhead: usize,
}

/// Sliding-window byte occupancy of the decoder input FIFO.
#[derive(Clone, Copy, Debug)]
pub struct FifoDepthState {
    contribution: [[u32; RING]; ACCUMULATORS],
    stream: [[u32; RING]; ACCUMULATORS],
    substream: [[u32; RING]; SUBSTREAMS],
    removal: [usize; RING],
    read: usize,
    write: usize,
    depth: [usize; ACCUMULATORS],
    depth_stream: [usize; ACCUMULATORS],
    depth_substream: [usize; SUBSTREAMS],
    peak: [FifoPeak; ACCUMULATORS],
    peak_substream: [usize; SUBSTREAMS],
}

impl Default for FifoDepthState {
    fn default() -> Self {
        Self {
            contribution: [[0; RING]; ACCUMULATORS],
            stream: [[0; RING]; ACCUMULATORS],
            substream: [[0; RING]; SUBSTREAMS],
            removal: [0; RING],
            read: 0,
            write: 0,
            depth: [0; ACCUMULATORS],
            depth_stream: [0; ACCUMULATORS],
            depth_substream: [0; SUBSTREAMS],
            peak: [FifoPeak::default(); ACCUMULATORS],
            peak_substream: [0; SUBSTREAMS],
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
        contribution: FifoContribution,
    ) -> FifoDepthReport {
        let mut report = FifoDepthReport::default();

        let slot = self.write;
        self.removal[slot] = removal;

        for k in 0..ACCUMULATORS {
            self.contribution[k][slot] = contribution.total[k] as u32;
            self.stream[k][slot] = contribution.stream[k] as u32;
        }

        for i in 0..SUBSTREAMS {
            self.substream[i][slot] = contribution.substream[i] as u32;
        }

        let mut drained = 0;

        while playhead > self.removal[self.read] && drained < RING {
            let read = self.read;

            for k in 0..ACCUMULATORS {
                let leaving = self.contribution[k][read] as usize;

                match self.depth[k].checked_sub(leaving) {
                    Some(remaining) => {
                        self.depth[k] = remaining;
                        self.depth_stream[k] = self.depth_stream[k]
                            .saturating_sub(self.stream[k][read] as usize);
                    }
                    None => {
                        self.depth[k] = 0;
                        self.depth_stream[k] = 0;
                        report.underrun = Some(k);
                    }
                }

                self.sample_peak(k);
            }

            for i in 0..SUBSTREAMS {
                self.depth_substream[i] =
                    self.depth_substream[i].saturating_sub(self.substream[i][read] as usize);
                self.peak_substream[i] = self.peak_substream[i].max(self.depth_substream[i]);
            }

            self.read = (read + 1) & (RING - 1);
            drained += 1;

            if report.underrun.is_some() {
                break;
            }
        }

        for k in 0..ACCUMULATORS {
            self.depth[k] += contribution.total[k];
            self.depth_stream[k] += contribution.stream[k];
            self.sample_peak(k);
        }

        for i in 0..SUBSTREAMS {
            self.depth_substream[i] += contribution.substream[i];
            self.peak_substream[i] = self.peak_substream[i].max(self.depth_substream[i]);
        }

        self.write = (slot + 1) & (RING - 1);

        report.depths = self.depth;
        report
    }

    /// Records the decomposition standing at a new deepest point.
    ///
    /// The split is only meaningful at one instant, so it is taken when the total sets a
    /// record rather than maximised on its own: the stream and overhead parts of a peak
    /// always add back up to it.
    fn sample_peak(&mut self, k: usize) {
        if self.depth[k] <= self.peak[k].total {
            return;
        }

        self.peak[k] = FifoPeak {
            total: self.depth[k],
            stream: self.depth_stream[k],
            overhead: self.depth[k] - self.depth_stream[k].min(self.depth[k]),
        };
    }

    /// Deepest each accumulator has been over the stream.
    pub fn peaks(&self) -> [usize; ACCUMULATORS] {
        let mut peaks = [0; ACCUMULATORS];

        for (peak, record) in peaks.iter_mut().zip(self.peak) {
            *peak = record.total;
        }

        peaks
    }

    /// Deepest each accumulator has been, with the stream and overhead parts it held then.
    pub fn peak_records(&self) -> [FifoPeak; ACCUMULATORS] {
        self.peak
    }

    /// Deepest each substream's own bytes have been, with no overhead priced in.
    pub fn substream_peaks(&self) -> [usize; SUBSTREAMS] {
        self.peak_substream
    }

    /// Drops every record in flight and starts the window again, keeping the peaks.
    pub fn restart(&mut self) {
        self.read = 0;
        self.write = 0;
        self.depth = [0; ACCUMULATORS];
        self.depth_stream = [0; ACCUMULATORS];
        self.depth_substream = [0; SUBSTREAMS];
    }

    /// Access units currently held in the window.
    pub fn buffered(&self) -> usize {
        self.write.wrapping_sub(self.read) & (RING - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An access unit whose bytes are half the stream's own and half overhead, with the
    /// two substreams carrying a quarter of the total each.
    fn unit(total: [usize; ACCUMULATORS]) -> FifoContribution {
        let mut substream = [0; SUBSTREAMS];
        substream[0] = total[4] / 4;
        substream[1] = total[4] / 4;

        FifoContribution {
            total,
            stream: total.map(|bytes| bytes / 2),
            substream,
        }
    }

    const UNIT: [usize; ACCUMULATORS] = [10, 20, 30, 40, 50];

    /// A two-substream FBB stream carries substream_info 0x0D, and its substream-0 cap
    /// must be real. It was once zero, which rejected every conformant stream of the
    /// shape that most DVD-Audio content uses.
    #[test]
    fn a_two_substream_fbb_stream_has_real_caps() {
        assert_eq!(Accumulator::Substream0.fbb_cap(0x0D), Some(30_000));
        assert_eq!(Accumulator::Sixch.fbb_cap(0x0D), Some(90_000));
        assert_eq!(Accumulator::WholeStream.fbb_cap(0x0D), Some(90_000));

        // the single-substream shapes are unchanged
        assert_eq!(Accumulator::Substream0.fbb_cap(0x04), Some(90_000));
        assert_eq!(Accumulator::Substream0.fbb_cap(0x05), Some(30_000));
        assert_eq!(Accumulator::Substream0.fbb_cap(0x07), Some(30_000));
        assert_eq!(Accumulator::Sixch.fbb_cap(0x07), Some(30_000));

        // only the low nibble selects a cap
        assert_eq!(Accumulator::Substream0.fbb_cap(0x2D), Some(30_000));
    }

    #[test]
    fn a_record_stays_until_playback_strictly_passes_its_removal_time() {
        let mut fifo = FifoDepthState::default();

        // Three access units 40 samples apart; each leaves the window at output time
        // plus one access unit, here arrival + 100.
        for i in 0..3 {
            let arrival = i * 40;
            fifo.push(arrival, arrival + 100, unit(UNIT));
        }
        assert_eq!(fifo.buffered(), 3);
        assert_eq!(fifo.peaks(), [30, 60, 90, 120, 150]);

        // Playback exactly at a removal time does NOT drain: the drain is strict.
        let report = fifo.push(100, 220, unit(UNIT));
        assert_eq!(fifo.buffered(), 4);
        assert_eq!(report.depths, [40, 80, 120, 160, 200]);

        // One sample later the first record leaves, and only the first.
        let report = fifo.push(101, 221, unit(UNIT));
        assert_eq!(fifo.buffered(), 4);
        assert_eq!(report.depths, [40, 80, 120, 160, 200]);
        assert!(report.underrun.is_none());
    }

    #[test]
    fn the_drain_runs_before_the_add_so_the_peak_includes_the_new_unit() {
        let mut fifo = FifoDepthState::default();

        // The first record is still buffered when the second arrives, so the peak holds
        // both, even though the first would have drained at any playhead past 50.
        fifo.push(0, 50, unit(UNIT));
        fifo.push(50, 100, unit(UNIT));
        assert_eq!(fifo.peaks(), [20, 40, 60, 80, 100]);

        // At 51 the first record drains before the third is added: same peak.
        fifo.push(51, 150, unit(UNIT));
        assert_eq!(fifo.peaks(), [20, 40, 60, 80, 100]);
    }

    #[test]
    fn an_underrun_clamps_to_zero_and_stops_the_drain_pass() {
        let mut fifo = FifoDepthState::default();

        fifo.push(0, 10, unit(UNIT));
        fifo.push(1, 11, unit(UNIT));

        // Corrupt the model by force: drain everything against a record claiming more
        // than the accumulators hold. Both stale records are past removal, but the
        // underrun on the first stops the pass before the second is touched.
        let mut broken = FifoDepthState::default();
        broken.push(0, 10, unit([100, 100, 100, 100, 100]));
        broken.push(1, 11, unit(UNIT));
        // depths now [110, 120, 130, 140, 150]; drain a record of 200 each
        broken.contribution.iter_mut().for_each(|c| c[0] = 200);

        let report = broken.push(100, 200, unit([2, 2, 2, 2, 2]));
        assert!(report.underrun.is_some());
        // clamped to zero on every accumulator, then the new unit was added, and the
        // second stale record was left in place
        assert_eq!(report.depths, [2, 2, 2, 2, 2]);
        assert_eq!(broken.buffered(), 2);
    }

    /// The split is only a split: whatever the window does to an accumulator, the stream
    /// and overhead parts recorded at its deepest point add back up to it.
    #[test]
    fn the_stream_and_overhead_parts_of_a_peak_add_up_to_it() {
        let mut fifo = FifoDepthState::default();

        for i in 0..40 {
            let arrival = i * 40;
            fifo.push(arrival, arrival + 300, unit(UNIT));
        }

        for record in fifo.peak_records() {
            assert_eq!(record.stream + record.overhead, record.total);
            assert_eq!(record.stream, record.total / 2);
        }

        assert_eq!(fifo.peak_records().map(|record| record.total), fifo.peaks());
    }

    /// A substream's window is its own payload with nothing priced around it, so its peak
    /// stays under the accumulator that carries the same bytes plus the overheads.
    #[test]
    fn a_substream_peak_carries_no_overhead() {
        let mut fifo = FifoDepthState::default();

        for i in 0..8 {
            let arrival = i * 40;
            fifo.push(arrival, arrival + 300, unit(UNIT));
        }

        let peaks = fifo.substream_peaks();
        assert_eq!(peaks, [8 * (UNIT[4] / 4), 8 * (UNIT[4] / 4), 0, 0]);
        let whole = fifo.peak_records()[Accumulator::WholeStream as usize];
        assert!(peaks.iter().sum::<usize>() < whole.total);
    }

    #[test]
    fn the_ring_overwrites_rather_than_evicts_past_128_records() {
        let mut fifo = FifoDepthState::default();

        for i in 0..(RING * 2) {
            fifo.push(i, usize::MAX, unit([2, 2, 2, 2, 2]));
        }

        // Nothing ever drained, so the depth kept the full count even though the ring
        // only remembers the last 128 records.
        assert_eq!(fifo.peaks(), [RING * 4; ACCUMULATORS]);
    }
}
