//! Timing utilities for high-resolution output timing.
//!
//! Provides timing trait implementations and high-resolution timing
//! state management for stream synchronization.

use anyhow::Result;
use log::{trace, warn};

use crate::process::parse::ParserState;

/// Trait providing timing information access for audio processing.
pub trait Timing {
    fn au_index(&self) -> Result<usize>;
    fn samples_per_au(&self) -> Result<usize>;
    fn substream_index(&self) -> Result<usize>;
    fn output_timing(&self) -> Result<usize>;
    fn update_hires_output_timing(&mut self, hires_output_timing: usize) -> Result<()>;
}

impl Timing for ParserState {
    fn au_index(&self) -> Result<usize> {
        Ok(self.au_counter)
    }

    fn samples_per_au(&self) -> Result<usize> {
        Ok(self.samples_per_au)
    }

    fn substream_index(&self) -> Result<usize> {
        Ok(self.substream_index)
    }

    fn output_timing(&self) -> Result<usize> {
        Ok(self.output_timing)
    }

    fn update_hires_output_timing(&mut self, hires_output_timing: usize) -> Result<()> {
        self.hires_output_timing = Some(hires_output_timing);

        Ok(())
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
    // TODO: 105
    pub fn update(&mut self, state: &mut dyn Timing, hires_present: bool) -> Result<()> {
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
                    self.au_index = state.au_index()?;
                    self.au_output_timing = state.output_timing()?;

                    break 'a;
                }

                self.state_index = 0;
                if self.serialisation_counter != 0 {
                    warn!(
                        "Invalid high-resolution output timing: extra zero after data field end (AU {})",
                        self.au_index
                    );
                } else {
                    warn!(
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
                    warn!(
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
                        let hires_output_timing = (self.timing << 16)
                            .wrapping_add(self.au_output_timing)
                            .wrapping_sub(self.au_index * state.samples_per_au()?);
                        trace!(
                            "First high-resolution timing field: {} (AU {}), stream start timing: {}",
                            self.timing, self.au_index, hires_output_timing
                        );

                        state.update_hires_output_timing(hires_output_timing)?;
                    } else if self.timing - self.prev_timing
                        == (self.prev_au_output_timing
                            + ((self.au_index - self.prev_au_index) * state.samples_per_au()?))
                            >> 16
                    {
                        trace!(
                            "Valid high-resolution timing field: {} (AU {})",
                            self.timing, self.au_index
                        );
                    } else {
                        warn!(
                            "High-resolution timing sequence error: {} (AU {}) does not follow {} (AU {}) on substream {}",
                            self.timing,
                            self.au_index,
                            self.prev_timing,
                            self.prev_au_index,
                            state.substream_index()?
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
        Ok(())
    }
}
