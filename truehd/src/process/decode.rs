use crate::process::{MAX_PRESENTATIONS, PresentationMap, PresentationType};
use crate::structs::access_unit::AccessUnit;
use crate::structs::channel::ChannelLabel;
use crate::structs::oamd::ObjectAudioMetadataPayload;
use crate::utils::dither::dither_31eb;
use crate::utils::errors::DecodeError;
use anyhow::{Result, bail};
use log::info;
use std::collections::VecDeque;

/// Decodes access units to PCM audio samples.
///
/// Converts parsed [`AccessUnit`] structures into 24-bit PCM audio data.
#[derive(Default)]
pub struct Decoder {
    state: DecoderState,
}

impl Decoder {
    /// Decodes an access unit to PCM audio samples.
    ///
    /// Returns a [`DecodedAccessUnit`] containing 24-bit PCM samples organized
    /// as `[sample_index][channel_index]` with up to 160 samples and 16 channels.
    pub fn decode_presentation(
        &mut self,
        access_unit: &AccessUnit,
        presentation: usize,
    ) -> Result<DecodedAccessUnit> {
        self.state.decode_access_unit(access_unit, presentation)?;
        let decoded = DecodedAccessUnit {
            channel_labels: self.state.channel_labels.clone(),
            sampling_frequency: self.state.sampling_frequency,
            sample_length: self.state.samples_per_au - self.state.zero_samples,
            channel_count: self.state.substream_state[self.state.presentation].max_matrix_chan + 1,
            pcm_data: self.state.output_buffer,
            oamd: self.state.oamd.iter().cloned().collect::<Vec<_>>(),
        };

        Ok(decoded)
    }

    /// Sets the failure level for validation errors.
    ///
    /// - `log::Level::Error`: Only fail on Error level messages (default)  
    /// - `log::Level::Warn`: Fail on Warning level and above (strict mode)
    pub fn set_fail_level(&mut self, level: log::Level) {
        self.state.fail_level = level;
    }
}

/// The result of decoding an access unit to PCM audio.
///
/// Contains 24-bit signed integer samples in sample-major ordering
/// (`pcm_data[sample_index][channel_index]`) with associated metadata.
#[derive(Debug)]
pub struct DecodedAccessUnit {
    /// Sampling frequency in Hz.
    ///
    /// This is the sampling frequency used for the audio data.
    pub sampling_frequency: u32,

    /// Number of valid samples in this access unit.
    ///
    /// This indicates how many samples in the `pcm_data` array contain
    /// valid audio data. The remaining samples should be ignored.
    pub sample_length: usize,

    /// Channel count for the audio data.
    ///
    /// This is determined by the stream configuration and indicates how many
    /// channels are present in the audio data.
    pub channel_count: usize,

    /// PCM audio samples organized as `[sample_index][channel_index]`.
    ///
    /// Contains 24-bit signed integer samples with sample-major ordering.
    /// - Array dimensions: [160 samples][16 channels]
    /// - Valid data length: Determined by `sample_length`
    /// - Channel count: Determined by stream configuration
    pub pcm_data: [[i32; 16]; 160],

    /// Channel labels for the audio data.
    ///
    /// Contains labels for each channel in the audio data, providing
    /// descriptive names for each channel.
    pub channel_labels: Vec<ChannelLabel>,

    /// Optional object audio metadata payload.
    ///
    /// Contains spatial audio metadata when present in the stream.
    pub oamd: Vec<ObjectAudioMetadataPayload>,
}

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct DecoderSubstreamState {
    pub restart_sync_word: u16,
    pub min_chan: usize,
    pub max_chan: usize,
    pub max_matrix_chan: usize,
    pub dither_shift: u32,
    pub dither_seed: u32,
    pub lossless_check_i32: i32,
    pub ch_assign: [usize; 16],

    pub block_size: usize,

    pub primitive_matrices: usize,
    pub matrix_ch: [u8; 16],
    pub frac_bits: [u8; 16],
    pub cf_shift_code: [i8; 16],
    pub dither_scale: [u8; 16],
    pub delta_precision: [u8; 16],
    pub delta_cf: [[i32; 16]; 16],
    pub m_coeff: [[i32; 16]; 16],

    pub output_shift: [i8; 16],
    pub quantiser_step_size: [u32; 16],

    pub order: [[usize; 16]; 2],
    pub coeff_q: [[i32; 16]; 2],
    pub coeff: [[[i32; 8]; 16]; 2],
    pub coeff_state: [[[i32; 8]; 16]; 2],

    pub bypassed_lsb: [[i32; 16]; 160],
    pub block_data: [[i32; 16]; 160],
    pub dither_table: [i32; 256],
    pub decoded_sample_len: usize,
}

impl Default for DecoderSubstreamState {
    fn default() -> Self {
        Self {
            restart_sync_word: 0,
            min_chan: 0,
            max_chan: 0,
            max_matrix_chan: 0,
            dither_shift: 0,
            dither_seed: 0,
            lossless_check_i32: 0,
            ch_assign: [0; 16],

            block_size: 8,

            primitive_matrices: 0,
            matrix_ch: [0; 16],
            frac_bits: [0; 16],
            cf_shift_code: [0; 16],
            dither_scale: [0; 16],
            delta_precision: [0; 16],
            delta_cf: [[0; 16]; 16],
            m_coeff: [[0; 16]; 16],

            output_shift: [0; 16],
            quantiser_step_size: [0; 16],

            order: [[0; 16]; 2],
            coeff_q: [[0; 16]; 2],
            coeff: [[[0; 8]; 16]; 2],
            coeff_state: [[[0; 8]; 16]; 2],

            bypassed_lsb: [[0; 16]; 160],
            block_data: [[0; 16]; 160],
            dither_table: [0; 256],
            decoded_sample_len: 0,
        }
    }
}

#[derive(Debug)]
#[repr(C)]
pub struct DecoderState {
    pub fail_level: log::Level,

    pub valid: bool,
    pub counter: usize,

    pub sampling_frequency: u32,
    pub samples_per_au: usize,

    pub presentation_map: Option<PresentationMap>,
    pub presentation: usize,

    pub channel_labels: Vec<ChannelLabel>,

    pub substreams: usize,
    pub substream_mask: u8,
    pub substream_info: u8,
    pub extended_substream_info: u8,

    pub substream_index: usize,
    pub substream_state: [DecoderSubstreamState; MAX_PRESENTATIONS],

    pub rematrix_buffer: [[i32; 16]; 160],
    pub output_buffer: [[i32; 16]; 160],
    pub zero_samples: usize,
    pub oamd: VecDeque<ObjectAudioMetadataPayload>,
}

impl Default for DecoderState {
    fn default() -> Self {
        Self {
            fail_level: log::Level::Error,
            valid: false,
            counter: 0,
            sampling_frequency: 0,
            samples_per_au: 0,
            presentation_map: None,
            presentation: 0,
            channel_labels: vec![],
            substreams: 0,
            substream_mask: 0,
            substream_info: 0,
            extended_substream_info: 0,
            substream_index: 0,
            substream_state: [DecoderSubstreamState::default(); MAX_PRESENTATIONS],
            rematrix_buffer: [[0; 16]; 160],
            output_buffer: [[0; 16]; 160],
            zero_samples: 0,
            oamd: VecDeque::with_capacity(4),
        }
    }
}

impl DecoderState {
    pub fn substream_state_mut(&mut self) -> Result<&mut DecoderSubstreamState> {
        Ok(&mut self.substream_state[self.substream_index])
    }

    pub fn substream_state(&self) -> Result<&DecoderSubstreamState> {
        Ok(&self.substream_state[self.substream_index])
    }
    pub fn decode_access_unit(
        &mut self,
        access_unit: &AccessUnit,
        presentation: usize,
    ) -> Result<()> {
        access_unit.update_decoder_state(self)?;

        if !self.valid {
            self.update_presentation(presentation)?;
            self.channel_labels = access_unit
                .get_channel_labels(self.presentation)
                .unwrap_or_default();
        }

        self.oamd.clear();

        for i in 0..=self.presentation {
            if (self.substream_mask >> i) & 1 == 0 {
                continue;
            }

            let substream_segment = &access_unit.substream_segment[i];
            if i == presentation
                && let Some(terminator) = &substream_segment.terminator
                && terminator.zero_samples_indicated
            {
                self.zero_samples = terminator.zero_samples as usize;
            }

            if i == 3
                && let Some(extra_data) = &access_unit.extra_data
                && let Some(evo_frame) = &extra_data.evo_frame
            {
                for evo_payload in &evo_frame.evo_payloads {
                    if evo_payload.evo_payload_id == 11 {
                        let smploffst =
                            evo_payload.evo_payload_config.smploffst.unwrap_or_default() as u64;
                        let mut oamd =
                            ObjectAudioMetadataPayload::read(&evo_payload.evo_payload_byte)?;
                        oamd.evo_sample_offset = smploffst;
                        self.oamd.push_back(oamd);
                    }
                }
            }

            self.substream_index = i;
            let ss_state = &mut self.substream_state[self.substream_index];
            ss_state.decoded_sample_len = 0;

            for block in substream_segment.block.iter() {
                block.update_decoder_state(self)?;
                self.decode()?;
            }
        }

        self.valid = true;
        self.counter += 1;

        Ok(())
    }

    fn update_presentation(&mut self, presentation: usize) -> Result<()> {
        let Some(presentation_map) = self.presentation_map else {
            bail!("Presentation map not initialized");
        };

        let mut presentations = [false; MAX_PRESENTATIONS];
        presentations[presentation] = true;

        self.substream_mask =
            presentation_map.substream_mask_by_required_presentations(&presentations);
        match presentation_map.presentation_type_by_index(presentation) {
            PresentationType::Invalid => {
                if !self.valid {
                    let max_independent = presentation_map.max_independent_presentation();
                    info!(
                        "Presentation {presentation} is not available, using presentation {max_independent}"
                    );
                    self.presentation = max_independent;
                }
            }
            PresentationType::CopyOf(copy_index) => {
                if !self.valid {
                    info!("Presentation {presentation} is a copy of presentation {copy_index}")
                }
                self.presentation = copy_index;
            }
            _ => {
                self.presentation = presentation;
            }
        };

        Ok(())
    }

    pub fn reset_decoder_substream_state(&mut self) {
        let ss_state = &mut self.substream_state[self.substream_index];
        *ss_state = DecoderSubstreamState::default();
    }

    fn decode(&mut self) -> Result<()> {
        let DecoderSubstreamState {
            restart_sync_word,
            min_chan,
            max_chan,
            max_matrix_chan,
            dither_shift,
            // TODO: max_lsbs
            ch_assign,

            block_size,

            primitive_matrices,
            matrix_ch,
            dither_scale,
            delta_cf,

            output_shift,
            quantiser_step_size,

            order,
            coeff,
            coeff_q,
            ..
        } = *self.substream_state()?;

        let samples_per_au = self.samples_per_au;

        let ss_state = &mut self.substream_state[self.substream_index];

        let decoded_sample_len = &mut ss_state.decoded_sample_len;
        let dither_seed = &mut ss_state.dither_seed;
        let bypassed_lsb = &mut ss_state.bypassed_lsb;
        let coeff_state = &mut ss_state.coeff_state;
        let m_coeff = &mut ss_state.m_coeff;

        let (max_val, min_val) = if restart_sync_word == 0x31EC {
            (1 << 31, -(1 << 31))
        } else {
            (1 << 23, -(1 << 23))
        };

        // recorrelation
        {
            let block_data = &ss_state.block_data;
            let rematrix_buffer = &mut self.rematrix_buffer[*decoded_sample_len..];

            #[allow(clippy::needless_range_loop)]
            for chi in min_chan..=max_chan {
                let mut state_buffer = [[0; 168]; 2];

                state_buffer[0][160..].copy_from_slice(&coeff_state[0][chi]);
                state_buffer[1][160..].copy_from_slice(&coeff_state[1][chi]);

                let fir_order = order[0][chi];
                let iir_order = order[1][chi];
                let coeff_q_shift = coeff_q[0][chi];
                let quantiser_mask = !((1 << quantiser_step_size[chi]) - 1);
                let fir_coeff = &coeff[0][chi];
                let iir_coeff = &coeff[1][chi];

                for blki in 0..block_size {
                    let audio_data = block_data[blki][chi] as i64;
                    let state_base = 160 - blki;

                    let mut acc = 0i64;

                    for oi in 0..fir_order {
                        acc += (fir_coeff[oi] as i64) * (state_buffer[0][state_base + oi] as i64);
                    }

                    for oi in 0..iir_order {
                        acc += (iir_coeff[oi] as i64) * (state_buffer[1][state_base + oi] as i64);
                    }

                    let pred = acc >> coeff_q_shift;
                    let fir_state = audio_data + (pred & quantiser_mask);
                    let iir_state = fir_state - pred;

                    if fir_state >= max_val {
                        bail!(DecodeError::RecorrelatorPositiveSaturation(fir_state));
                    } else if fir_state < min_val {
                        bail!(DecodeError::RecorrelatorNegativeSaturation(fir_state));
                    }

                    if !(min_val..max_val).contains(&iir_state) {
                        if restart_sync_word == 0x31EC {
                            bail!(DecodeError::FilterBInputTooWide32(iir_state));
                        } else {
                            bail!(DecodeError::FilterBInputTooWide24(iir_state));
                        }
                    }

                    state_buffer[0][159 - blki] = fir_state as i32;
                    state_buffer[1][159 - blki] = iir_state as i32;

                    rematrix_buffer[blki][chi] = fir_state as i32;
                }

                coeff_state[0][chi][..].copy_from_slice(&state_buffer[0][160 - block_size..][..8]);
                coeff_state[1][chi][..].copy_from_slice(&state_buffer[1][160 - block_size..][..8]);
            }
        }

        // lossless matrix
        if self.substream_index == self.presentation {
            let dither_table = &mut ss_state.dither_table;
            let rematrix_buffer = &mut self.rematrix_buffer[*decoded_sample_len..];

            match restart_sync_word {
                0x31EA => {
                    for blki in 0..block_size {
                        let rematrix_buffer = &mut rematrix_buffer[blki];
                        let bypassed_lsb = &mut bypassed_lsb[blki];
                        let dither_seed_shr7 = *dither_seed >> 7;

                        rematrix_buffer[max_matrix_chan + 1] =
                            (((*dither_seed >> 15) as i8) << dither_shift) as i32;
                        rematrix_buffer[max_matrix_chan + 2] =
                            ((dither_seed_shr7 as i8) << dither_shift) as i32;

                        *dither_seed =
                            (dither_seed_shr7 ^ (dither_seed_shr7 << 5) ^ (*dither_seed << 16))
                                & 0x7FFFFF;

                        for pmi in 0..primitive_matrices {
                            let mut acc = 0;
                            let matrix_ch = matrix_ch[pmi] as usize;
                            let m_coeff = &m_coeff[pmi];

                            for chi in 0..=max_matrix_chan + 2 {
                                acc += rematrix_buffer[chi] as i64 * m_coeff[chi] as i64;
                            }

                            rematrix_buffer[matrix_ch] = (((acc >> 18) as i32)
                                & (!((1 << quantiser_step_size[matrix_ch]) - 1)))
                                + bypassed_lsb[pmi];
                        }
                    }
                }
                0x31EB => {
                    if *decoded_sample_len == 0 {
                        dither_table[..samples_per_au.next_power_of_two()]
                            .copy_from_slice(&dither_31eb(samples_per_au, dither_seed));
                    }

                    let dither_index_mask = samples_per_au.next_power_of_two() - 1;

                    for blki in 0..block_size {
                        let rematrix_buffer = &mut rematrix_buffer[blki];
                        let bypassed_lsb = &mut bypassed_lsb[blki];
                        let blki_abs = blki + *decoded_sample_len;

                        for pmi in 0..primitive_matrices {
                            let mut acc = 0;
                            let m_coeff = &m_coeff[pmi];
                            let dither_scale = dither_scale[pmi] as i64;
                            let matrix_ch = matrix_ch[pmi] as usize;

                            let dither_index =
                                (primitive_matrices - pmi) * (2 * blki_abs + 1) + blki_abs;

                            for chi in 0..=max_matrix_chan {
                                acc += rematrix_buffer[chi] as i64 * m_coeff[chi] as i64;
                            }

                            if dither_scale != 0 {
                                acc += (dither_table[dither_index & dither_index_mask] as i64)
                                    << (11 + dither_scale);
                            }

                            rematrix_buffer[matrix_ch] = (((acc >> 18) as i32)
                                & (!((1 << quantiser_step_size[matrix_ch]) - 1)))
                                + bypassed_lsb[pmi];
                        }
                    }
                }
                0x31EC => {
                    if *decoded_sample_len == 0 {
                        dither_table[..samples_per_au.next_power_of_two()]
                            .copy_from_slice(&dither_31eb(samples_per_au, dither_seed));
                    }

                    let dither_index_mask = samples_per_au.next_power_of_two() - 1;

                    let samples_per_au_recip = (1 << 16) / samples_per_au as i64;

                    for blki in 0..block_size {
                        let rematrix_buffer = &mut rematrix_buffer[blki];
                        let bypassed_lsb = &mut bypassed_lsb[blki];
                        let blki_abs = blki + *decoded_sample_len;

                        for pmi in 0..primitive_matrices {
                            let mut acc = 0;
                            let mut acc_delta = 0;
                            let dither_scale = dither_scale[pmi] as u64;
                            let matrix_ch = matrix_ch[pmi] as usize;
                            let m_coeff = &m_coeff[pmi];
                            let delta_cf = &delta_cf[pmi];

                            let dither_index =
                                (primitive_matrices - pmi) * (2 * blki_abs + 1) + blki_abs;

                            for chi in 0..=max_matrix_chan {
                                acc += rematrix_buffer[chi] as i64 * m_coeff[chi] as i64;
                                acc_delta += rematrix_buffer[chi] as i64 * delta_cf[chi] as i64;
                            }

                            if dither_scale != 0 {
                                acc += (dither_table[dither_index & dither_index_mask] as i64)
                                    << (11 + dither_scale);
                            }

                            acc +=
                                (acc_delta >> 18) * (blki_abs as i64) * (samples_per_au_recip << 2);

                            rematrix_buffer[matrix_ch] = (((acc >> 18) as i32)
                                & (!((1 << quantiser_step_size[matrix_ch]) - 1)))
                                + bypassed_lsb[pmi];
                        }
                    }

                    if *decoded_sample_len + block_size == samples_per_au {
                        for pmi in 0..primitive_matrices {
                            let m_coeff = &mut m_coeff[pmi];
                            let delta_cf = &delta_cf[pmi];
                            for chi in 0..=max_matrix_chan {
                                m_coeff[chi] += delta_cf[chi];
                            }
                        }
                    }
                }
                _ => {}
            }

            // remap
            {
                let output_buffer = &mut self.output_buffer[*decoded_sample_len..];

                let mut lossless_check_data = 0;

                for blki in 0..block_size {
                    let sample = rematrix_buffer[blki];
                    let mut output = [0; 16];

                    for chi in 0..=max_matrix_chan {
                        let ch_assign = ch_assign[chi];
                        let output = &mut output[ch_assign];

                        *output = sample[chi];

                        let output_shift = output_shift[chi];
                        if output_shift < 0 {
                            *output >>= -output_shift;
                        } else {
                            *output <<= output_shift;
                        }

                        lossless_check_data ^= (*output & 0xFFFFFF) << (chi & 7);
                    }

                    output_buffer[blki] = output;
                }

                ss_state.lossless_check_i32 ^= lossless_check_data;
            }
        }

        *decoded_sample_len += block_size;

        Ok(())
    }
}
