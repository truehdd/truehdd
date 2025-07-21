//! Matrix operations for lossless multi-channel decorrelation.
//!
//! TrueHD uses matrix operations to remove correlation between audio channels.
//!
//! ## Matrix Primitives
//!
//! Up to 16 matrix primitives per substream, each operating on one target channel
//! using coefficients from other channels with configurable precision.
//!
//! ## Coefficient Updates
//!
//! Coefficients can be updated with delta encoding and bit masks controlling
//! which coefficients are modified.

use anyhow::{Result, anyhow};
use log::Level::Warn;

use crate::log_or_err;
use crate::process::decode::DecoderState;
use crate::process::parse::ParserState;
use crate::structs::sync::BASE_SAMPLING_RATE_CD;
use crate::utils::bitstream_io::BsIoSliceReader;
use crate::utils::errors::MatrixError;

/// Matrix primitive for single-channel decorrelation.
///
/// Applies linear combinations of other channels to one target channel
/// with configurable precision and coefficient updates.
#[derive(Clone, Copy, Debug, Default)]
pub struct Matrices {
    pub matrix_ch: u8,
    pub frac_bits: u8,

    pub lsb_bypass_used: bool,

    pub cf_shift_code: i8,
    pub lsb_bypass_bit_count: u8,
    pub dither_scale: u8,
    pub cf_mask: u16,

    pub delta_bits: u8,
    pub delta_precision: u8,
    pub delta_cf: [i32; 16],

    pub m_coeff: [i32; 16],
}

/// Complete multi-channel matrix configuration for lossless decorrelation.
///
/// Contains matrix primitives and control parameters for multi-channel decorrelation.
#[derive(Clone, Copy, Debug, Default)]
pub struct Matrixing {
    pub primitive_matrices: usize,

    pub new_matrix: bool,

    pub new_matrix_config: bool,
    pub interpolation_used: bool,
    pub new_delta: bool,
    pub new_delta_config: bool,

    pub matrices: [Matrices; 16],
}

impl Matrixing {
    pub fn read(state: &mut ParserState, reader: &mut BsIoSliceReader) -> Result<Self> {
        let current_substream_index = state.substream_index;
        let this_substream_info = state.substream_info;
        let audio_sampling_frequency_1 = state.audio_sampling_frequency_1;
        // ++*(a2+20148)
        let ss_state = state.substream_state_mut()?;

        let restart_sync_word = ss_state.restart_sync_word;
        let max_matrix_chan = ss_state.max_matrix_chan;

        let mut matrixing = Matrixing::default();

        if restart_sync_word == 0x31EC {
            matrixing.new_matrix = reader.get()?;

            if matrixing.new_matrix {
                matrixing.new_matrix_config = reader.get()?;

                if matrixing.new_matrix_config {
                    matrixing.primitive_matrices = reader.get_n::<u8>(4)? as usize + 1;
                    ss_state.primitive_matrices = matrixing.primitive_matrices;

                    for (pmi, matrices) in &mut matrixing.matrices[0..ss_state.primitive_matrices]
                        .iter_mut()
                        .enumerate()
                    {
                        matrices.matrix_ch = reader.get_n(4)?;
                        matrices.frac_bits = reader.get_n(4)?;

                        matrices.cf_shift_code = reader.get_n::<u8>(3)? as i8 - 1;
                        matrices.lsb_bypass_bit_count = reader.get_n(2)?;
                        matrices.dither_scale = reader.get_n(4)?;
                        matrices.cf_mask = reader.get_n(max_matrix_chan as u32 + 1)?;

                        ss_state.matrix_ch[pmi] = matrices.matrix_ch;
                        ss_state.frac_bits[pmi] = matrices.frac_bits;
                        ss_state.lsb_bypass_bit_count[pmi] = matrices.lsb_bypass_bit_count;
                        ss_state.cf_mask[pmi] = matrices.cf_mask;
                    }
                }

                let primitive_matrices = ss_state.primitive_matrices;

                for (pmi, matrices) in matrixing.matrices[0..primitive_matrices]
                    .iter_mut()
                    .enumerate()
                {
                    let frac_bits = ss_state.frac_bits[pmi] as u32;
                    let cf_mask = ss_state.cf_mask[pmi];

                    for chi in 0..=max_matrix_chan {
                        if (cf_mask >> chi) & 1 == 0 {
                            matrices.m_coeff[chi] = 0;
                            continue;
                        }

                        matrices.m_coeff[chi] = reader.get_s(frac_bits + 2)?;
                    }
                }
            }

            let primitive_matrices = ss_state.primitive_matrices;

            matrixing.interpolation_used = reader.get()?;

            if matrixing.interpolation_used {
                matrixing.new_delta = reader.get()?;

                if matrixing.new_delta {
                    matrixing.new_delta_config = reader.get()?;

                    if matrixing.new_delta_config {
                        for (pmi, matrices) in matrixing.matrices[0..primitive_matrices]
                            .iter_mut()
                            .enumerate()
                        {
                            matrices.delta_bits = reader.get_n(4)?;
                            matrices.delta_precision = reader.get_n(2)?;

                            ss_state.delta_bits[pmi] = matrices.delta_bits;
                        }
                    }

                    for (pmi, matrices) in matrixing.matrices[0..primitive_matrices]
                        .iter_mut()
                        .enumerate()
                    {
                        let cf_mask = ss_state.cf_mask[pmi];
                        let delta_bits = ss_state.delta_bits[pmi] as u32;

                        for chi in 0..=max_matrix_chan {
                            if delta_bits == 0 || (cf_mask >> chi) & 1 == 0 {
                                matrices.delta_cf[chi] = 0;
                                continue;
                            }

                            matrices.delta_cf[chi] = reader.get_s(delta_bits + 1)?;
                        }
                    }
                }
            } else {
                for matrices in matrixing.matrices[0..primitive_matrices].iter_mut() {
                    for chi in 0..=max_matrix_chan {
                        matrices.delta_cf[chi] = 0;
                    }
                }
            }
        } else {
            matrixing.primitive_matrices = reader.get_n::<u8>(4)? as usize;
            ss_state.primitive_matrices = matrixing.primitive_matrices;

            for (pmi, matrices) in &mut matrixing.matrices[0..ss_state.primitive_matrices]
                .iter_mut()
                .enumerate()
            {
                matrices.matrix_ch = reader.get_n(4)?;
                matrices.frac_bits = reader.get_n(4)?;
                matrices.lsb_bypass_used = reader.get()?;

                ss_state.matrix_ch[pmi] = matrices.matrix_ch;
                ss_state.frac_bits[pmi] = matrices.frac_bits;
                ss_state.lsb_bypass_used[pmi] = matrices.lsb_bypass_used;

                let coeff_bits = matrices.frac_bits as u32 + 2;

                for chi in 0..=(max_matrix_chan + if restart_sync_word == 0x31EA { 2 } else { 0 }) {
                    let m_flag = reader.get()?;

                    if !m_flag {
                        matrices.m_coeff[chi] = 0;
                        continue;
                    }

                    matrices.m_coeff[chi] = reader.get_s(coeff_bits)?;
                }

                if restart_sync_word == 0x31EB {
                    matrices.dither_scale = reader.get_n(4)?;
                }
            }
        };

        for (pmi, matrices) in matrixing.matrices[0..matrixing.primitive_matrices]
            .iter()
            .enumerate()
        {
            if matrices.matrix_ch as usize > max_matrix_chan {
                log_or_err!(
                    state,
                    Warn,
                    anyhow!(MatrixError::MatrixChannelTooHigh {
                        index: pmi,
                        max: max_matrix_chan,
                        actual: matrices.matrix_ch,
                    })
                );
            } else if matrices.frac_bits > 14 {
                log_or_err!(
                    state,
                    Warn,
                    anyhow!(MatrixError::FracBitsTooHigh(matrices.frac_bits))
                );
            } else if current_substream_index == 0
                && matrices.lsb_bypass_used
                && (this_substream_info & 2 != 0
                    || audio_sampling_frequency_1 >= BASE_SAMPLING_RATE_CD << 2)
            {
                log_or_err!(
                    state,
                    Warn,
                    anyhow!(MatrixError::InvalidLsbBypass {
                        info: this_substream_info
                    })
                );
            }
        }

        Ok(matrixing)
    }

    pub fn update_decoder_state(&self, state: &mut DecoderState) -> Result<()> {
        let ss_state = state.substream_state_mut()?;

        let restart_sync_word = ss_state.restart_sync_word;
        let max_matrix_chan = ss_state.max_matrix_chan;

        if restart_sync_word == 0x31EC {
            if self.new_matrix {
                if self.new_matrix_config {
                    ss_state.primitive_matrices = self.primitive_matrices;

                    for (pmi, matrices) in self.matrices[0..ss_state.primitive_matrices]
                        .iter()
                        .enumerate()
                    {
                        ss_state.matrix_ch[pmi] = matrices.matrix_ch;
                        ss_state.frac_bits[pmi] = matrices.frac_bits;
                        ss_state.cf_shift_code[pmi] = matrices.cf_shift_code;
                        ss_state.dither_scale[pmi] = matrices.dither_scale;
                    }
                }

                let primitive_matrices = ss_state.primitive_matrices;

                for (pmi, matrices) in self.matrices[0..primitive_matrices].iter().enumerate() {
                    let frac_bits = ss_state.frac_bits[pmi] as u32;
                    let cf_shift_code = ss_state.cf_shift_code[pmi];

                    for chi in 0..=max_matrix_chan {
                        ss_state.m_coeff[pmi][chi] =
                            (matrices.m_coeff[chi]) << (18 + cf_shift_code - frac_bits as i8);
                    }
                }
            }

            let primitive_matrices = ss_state.primitive_matrices;

            if self.interpolation_used {
                if self.new_delta {
                    if self.new_delta_config {
                        for (pmi, matrices) in
                            self.matrices[0..primitive_matrices].iter().enumerate()
                        {
                            ss_state.delta_precision[pmi] = matrices.delta_precision;
                        }
                    }

                    for (pmi, matrices) in self.matrices[0..primitive_matrices].iter().enumerate() {
                        let frac_bits = ss_state.frac_bits[pmi] as i8;
                        let cf_shift_code = ss_state.cf_shift_code[pmi];
                        let delta_precision = ss_state.delta_precision[pmi] as i8;

                        let scale = cf_shift_code - frac_bits - delta_precision;

                        let delta_cf = &mut ss_state.delta_cf[pmi];

                        for (chi, delta_cf) in
                            delta_cf.iter_mut().enumerate().take(max_matrix_chan + 1)
                        {
                            *delta_cf = matrices.delta_cf[chi] << (18 + scale);
                        }
                    }
                }
            } else {
                for (pmi, matrices) in self.matrices[0..primitive_matrices].iter().enumerate() {
                    ss_state.delta_cf[pmi] = matrices.delta_cf;
                }
            }
        } else {
            ss_state.primitive_matrices = self.primitive_matrices;

            for (pmi, matrices) in &mut self.matrices[0..ss_state.primitive_matrices]
                .iter()
                .enumerate()
            {
                ss_state.matrix_ch[pmi] = matrices.matrix_ch;
                ss_state.frac_bits[pmi] = matrices.frac_bits;

                let m_coeff = &mut ss_state.m_coeff[pmi];

                for (chi, m_coeff) in m_coeff
                    .iter_mut()
                    .enumerate()
                    .take(max_matrix_chan + if restart_sync_word == 0x31EA { 2 } else { 0 } + 1)
                {
                    *m_coeff = matrices.m_coeff[chi] << (18 - matrices.frac_bits);
                }

                if restart_sync_word == 0x31EB {
                    ss_state.dither_scale[pmi] = matrices.dither_scale;
                }
            }
        }

        Ok(())
    }
}
