//! Filtering for predictive audio compression.
//!
//! TrueHD uses adaptive FIR filters to remove temporal redundancy from audio signals.
//!
//! ## Filter Architecture
//!
//! Each channel uses two filter types:
//! - **Filter A**: Primary filter with up to 8 taps
//! - **Filter B**: Secondary filter with up to 4 taps
//!
//! ## Parameters
//!
//! Filter coefficients use configurable precision with quantization parameters
//! and filter state management.

use anyhow::{Result, bail};

use crate::process::decode::DecoderState;
use crate::utils::bitstream_io::BsIoSliceReader;
use crate::utils::errors::FilterError;

/// FIR filter coefficients for one channel.
///
/// Contains parameters for finite impulse response filter used for temporal prediction.
/// Includes filter coefficients, quantization parameters, and filter state.
#[derive(Debug, Default)]
pub struct FilterCoeffs {
    pub order: u8,

    pub coeff_q: u8,
    pub coeff_bits: u8,
    pub coeff_shift: u8,
    pub coeff: [i32; 8],

    pub new_states: bool,
    pub state_bits: u8,
    pub state_shift: u8,

    pub state: [i32; 8],
}

/// Filter type identifier for dual filter architecture.
///
/// TrueHD uses two cascaded filters per channel:
/// - **Filter A**: Primary prediction filter (up to 8 taps)
/// - **Filter B**: Secondary residual filter (up to 4 taps)
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum CoeffType {
    A = 0,
    B = 1,
}

impl FilterCoeffs {
    pub fn read(reader: &mut BsIoSliceReader, coeff_type: CoeffType) -> Result<Self> {
        let mut fc = Self {
            order: reader.get_n(4)?,
            ..Default::default()
        };

        if coeff_type == CoeffType::A && fc.order > 8 {
            bail!(FilterError::FilterAOrderTooHigh(fc.order));
        } else if coeff_type == CoeffType::B && fc.order > 4 {
            bail!(FilterError::FilterBOrderTooHigh(fc.order));
        }

        if fc.order != 0 {
            fc.coeff_q = reader.get_n(4)?;

            if fc.coeff_q < 8 {
                bail!(FilterError::InvalidCoeffQ(fc.coeff_q));
            }

            fc.coeff_bits = reader.get_n(5)?;

            if fc.coeff_bits > 16 || fc.coeff_bits == 0 {
                bail!(FilterError::InvalidCoeffBits(fc.coeff_bits));
            }

            fc.coeff_shift = reader.get_n(3)?;

            if fc.coeff_shift > 7 {
                bail!(FilterError::InvalidCoeffShift(fc.coeff_shift));
            }

            let total_bits = fc.coeff_bits + fc.coeff_shift;

            if total_bits > 16 {
                bail!(FilterError::TotalCoeffBitsTooLarge(total_bits));
            }

            for i in 0..fc.order as usize {
                if fc.coeff_bits == 0 {
                    fc.coeff[i] = 0;
                    continue;
                }

                let mut coeff = reader.get_s(fc.coeff_bits as u32)?;

                coeff <<= fc.coeff_shift;

                if coeff == -32768 {
                    bail!(FilterError::InvalidCoeffValue);
                }

                fc.coeff[i] = coeff;
            }

            fc.new_states = reader.get()?;

            if fc.new_states {
                if coeff_type == CoeffType::A {
                    bail!(FilterError::FilterANewStatesNotAllowed);
                }

                fc.state_bits = reader.get_n(4)?;
                fc.state_shift = reader.get_n(4)?;

                for i in 0..fc.order as usize {
                    if fc.state_bits == 0 {
                        fc.state[i] = 0;
                        continue;
                    }

                    let mut state = reader.get_s(fc.state_bits as u32)?;

                    state <<= fc.state_shift;

                    if !(-(1 << 23)..(1 << 23)).contains(&state) {
                        bail!(FilterError::FilterStateOutOfRange(state));
                    }

                    fc.state[i] = state;
                }
            }
        }

        Ok(fc)
    }

    pub fn update_decoder_state(
        &self,
        state: &mut DecoderState,
        coeff_type: CoeffType,
        chi: usize,
    ) -> Result<()> {
        let ss_state = state.substream_state_mut()?;
        let ci = coeff_type as usize;

        let order = self.order as usize;
        ss_state.order[ci][chi] = order;

        if order != 0 {
            ss_state.coeff_q[ci][chi] = self.coeff_q as i32;

            ss_state.coeff[ci][chi] = self.coeff;

            if self.new_states {
                ss_state.coeff_state[ci][chi] = self.state;
            }
        }

        Ok(())
    }
}
