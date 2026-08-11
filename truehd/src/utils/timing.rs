//! High-resolution output timing.
//!
//! `output_timing` is 16 bits and wraps; this field carries the high half, one bit per
//! access unit, so a decoder can recover an absolute sample position.

use log::{debug, info, trace};

use crate::process::parse::ParserState;

/// The stream facts a field decode needs, snapshotted so the decoder can run while the
/// state it owns is mutably borrowed out of the same [`ParserState`].
#[derive(Debug, Default, Clone, Copy)]
pub struct TimingContext {
    pub au_index: usize,
    pub samples_per_au: usize,
    pub substream_index: usize,
    pub output_timing: usize,
}

impl From<&ParserState> for TimingContext {
    fn from(state: &ParserState) -> Self {
        Self {
            au_index: state.au_counter,
            samples_per_au: state.samples_per_au,
            substream_index: state.substream_index,
            output_timing: state.output_timing,
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct HiresOutputTimingState {
    state_index: usize,
    serialisation_counter: usize,
    timing: usize,
    au_index: usize,
    au_output_timing: usize,
    prev_timing: usize,
    prev_au_index: usize,
    prev_au_output_timing: usize,
    counter: usize,
}

impl HiresOutputTimingState {
    /// Feeds one access unit's bit and returns the stream's start timing when the first
    /// field completes.
    ///
    /// The arithmetic is 32-bit and wrapping: the value reconstructed here is the high
    /// half of a 32-bit sample position whose low half is `output_timing`, so a field
    /// running backwards is a sequence error to report, not an underflow to panic on.
    /// Findings are informational: a stream carrying no field at all is legal.
    pub fn update(&mut self, ctx: &TimingContext, hires_present: bool) -> Option<usize> {
        let mut stream_start = None;

        match self.state_index {
            0 => {
                self.counter = 0;

                if !hires_present {
                    self.state_index = 1;
                }
            }
            1..=4 => {
                if !hires_present {
                    self.state_index += 1;
                } else {
                    self.state_index = 0;
                }
            }
            5 => 'a: {
                if hires_present {
                    self.state_index = 6;
                    self.serialisation_counter = 0;
                    self.timing = 0;
                    self.au_index = ctx.au_index;
                    self.au_output_timing = ctx.output_timing;

                    break 'a;
                }

                self.state_index = 0;
                if self.serialisation_counter != 0 {
                    info!(
                        "Invalid high-resolution output timing: extra zero after data field end (AU {})",
                        self.au_index
                    );
                } else {
                    info!(
                        "Invalid high-resolution output timing: extra zero in data field (AU {})",
                        self.au_index
                    );
                }
            }
            i @ 6..=10 => 'a: {
                if hires_present {
                    self.state_index = if i == 10 { 6 } else { 11 };

                    let i = i - 6;
                    self.serialisation_counter += i;
                    self.timing <<= i;

                    break 'a;
                }

                if i == 10 {
                    self.state_index = 0;
                    info!(
                        "Invalid high-resolution output timing: invalid zero in data field (AU {})",
                        self.au_index
                    );

                    break 'a;
                }

                self.state_index += 1;
            }
            i @ 11..=15 => 'a: {
                if hires_present {
                    self.state_index = if i == 15 { 6 } else { 11 };

                    let i = i - 10;
                    self.timing <<= i;
                    self.timing += 1 << (i - 1);
                    self.serialisation_counter += i;

                    break 'a;
                }

                if i == 15 {
                    let mut skip_refresh = false;
                    if self.counter < 3 {
                        self.counter += 1;
                    }

                    if self.counter < 2 {
                        let hires_output_timing = ((self.timing as u32) << 16)
                            .wrapping_add(self.au_output_timing as u32)
                            .wrapping_sub(
                                (self.au_index as u32).wrapping_mul(ctx.samples_per_au as u32),
                            )
                            as usize;
                        debug!(
                            "First high-resolution timing field: {} (AU {}), stream start timing: {}",
                            self.timing, self.au_index, hires_output_timing
                        );

                        stream_start = Some(hires_output_timing);
                    } else if (self.timing as u32).wrapping_sub(self.prev_timing as u32)
                        == ((self.au_index as u32)
                            .wrapping_sub(self.prev_au_index as u32)
                            .wrapping_mul(ctx.samples_per_au as u32)
                            .wrapping_add(self.prev_au_output_timing as u32))
                            >> 16
                    {
                        trace!(
                            "Valid high-resolution timing field: {} (AU {})",
                            self.timing, self.au_index
                        );
                    } else {
                        info!(
                            "High-resolution timing sequence error: {} (AU {}) does not follow {} (AU {}) on substream {}",
                            self.timing,
                            self.au_index,
                            self.prev_timing,
                            self.prev_au_index,
                            ctx.substream_index
                        );

                        self.counter = 0;
                        skip_refresh = true;
                    }

                    if !skip_refresh {
                        self.prev_timing = self.timing;
                        self.prev_au_index = self.au_index;
                        self.prev_au_output_timing = self.au_output_timing;
                    }

                    self.state_index = 5;

                    break 'a;
                }

                self.state_index += 1;
            }
            _ => unreachable!("Invalid state for parsing hires_output_timing."),
        }

        stream_start
    }

    pub fn reset_for_branch(&mut self) {
        self.state_index = 0;
        self.counter = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive(bits: &[bool]) -> (HiresOutputTimingState, Option<usize>) {
        let ctx = TimingContext {
            samples_per_au: 40,
            ..Default::default()
        };
        let mut machine = HiresOutputTimingState::default();
        let mut stream_start = None;
        for &bit in bits {
            stream_start = machine.update(&ctx, bit).or(stream_start);
        }
        (machine, stream_start)
    }

    /// Five zeros reach the field start, then the shortest field carrying 1 sets the
    /// stream's start timing from the high half of the position.
    #[test]
    fn the_first_field_sets_the_stream_start_timing() {
        let (_, stream_start) = drive(&[
            false, false, false, false, false, // preamble
            true, true, true, false, false, false, false, false, // field = 1
        ]);
        assert_eq!(stream_start, Some(1 << 16));
    }

    /// A field that runs backwards is a sequence error, not an underflow. The subtraction
    /// is 32-bit and wrapping, so this must report rather than panic under debug overflow
    /// checks, on exactly the malformed input the check exists to catch.
    #[test]
    fn a_backwards_field_reports_instead_of_underflowing() {
        let (machine, stream_start) = drive(&[
            false, false, false, false, false, // preamble
            true, true, true, false, false, false, false, false, // field = 1
            true, true, false, false, false, false, false, // field = 0, goes backwards
        ]);
        assert_eq!(stream_start, Some(1 << 16), "first field still decoded");
        assert_eq!(machine.counter, 0, "the sequence error resets the run counter");
    }
}
