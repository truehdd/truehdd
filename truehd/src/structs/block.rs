//! Audio block structures and compression parameters.
//!
//! TrueHD audio compression operates on blocks containing 8-160 samples per channel.
//! Each block contains optional restart header, block header, and compressed audio data.
//!
//! ## Block Structure
//!
//! - **Restart header**: Decoder initialization parameters
//! - **Block header**: Parameter updates controlled by guard flags
//! - **Compressed data**: Huffman-encoded audio samples
//! - **Error protection**: Optional CRC and length validation

use anyhow::{Result, anyhow, bail};
use log::Level::Warn;
use log::{info, trace, warn};

use crate::log_or_err;
use crate::process::decode::DecoderState;
use crate::process::parse::{ParserState, ParserSubstreamState};
use crate::structs::channel::ChannelParams;
use crate::structs::matrix::Matrixing;
use crate::structs::restart_header::{Guards, GuardsField, RestartHeader};
use crate::utils::bitstream_io::BsIoSliceReader;
use crate::utils::errors::BlockError;

/// Block header containing selective parameter updates.
///
/// Block headers provide incremental updates to decoding parameters controlled
/// by guard flags. Only parameters with enabled guards can be updated.
#[derive(Debug, Default)]
pub struct BlockHeader {
    pub guards: Option<Guards>,
    pub block_size: Option<usize>,
    pub matrixing: Option<Matrixing>,
    pub output_shift: [Option<i8>; 16],
    pub quantiser_step_size: [Option<u32>; 16],
    pub channel_params: [Option<ChannelParams>; 16],
}

/// Complete audio block with compressed data and decoding parameters.
///
/// Contains 8-160 samples per channel with optional restart header,
/// block header, and compressed audio data.
#[derive(Debug)]
pub struct Block {
    pub restart_header: Option<RestartHeader>,
    pub block_header: Option<BlockHeader>,
    pub block_data_bits: Option<u16>,
    pub bypassed_lsb: [[i32; 16]; 160],
    pub block_data: [[i32; 16]; 160],
    pub block_header_crc: u8,
}

impl Default for Block {
    fn default() -> Self {
        Block {
            restart_header: None,
            block_header: None,
            block_data_bits: None,
            bypassed_lsb: [[0; 16]; 160],
            block_data: [[0; 16]; 160],
            block_header_crc: 0,
        }
    }
}

impl BlockHeader {
    fn read(state: &mut ParserState, reader: &mut BsIoSliceReader) -> Result<Self> {
        let samples_per_au = state.samples_per_au;
        let ss_state = state.substream_state_mut()?;
        let max_shift = ss_state.max_shift;

        let mut bh = BlockHeader::default();

        if ss_state.guards.need_change(GuardsField::Guards) {
            // new_guards
            if reader.get()? {
                ss_state.guards = Guards::read(reader)?;
            }
        }

        let guards = ss_state.guards;

        if guards.need_change(GuardsField::BlockSize) {
            // new_block_size
            if reader.get()? {
                let block_size = reader.get_n::<u16>(9)? as usize;
                if !(8..=160).contains(&block_size) {
                    bail!(BlockError::InvalidBlockSizeRange(block_size));
                } else if block_size > samples_per_au {
                    bail!(BlockError::BlockSizeExceedsAU {
                        max: samples_per_au,
                        actual: block_size
                    });
                } else if block_size & 7 != 0 {
                    warn!("Block size {block_size} is not a multiple of 8")
                }

                bh.block_size = Some(block_size);
                ss_state.block_size = block_size;
            }
        }

        if guards.need_change(GuardsField::Matrixing) {
            // new_matrixing
            if reader.get()? {
                bh.matrixing = Some(Matrixing::read(state, reader)?);
            }
        }

        let ss_state = state.substream_state_mut()?;

        if guards.need_change(GuardsField::OutputShift) {
            // new_output_shift
            if reader.get()? {
                for i in 0..=ss_state.max_matrix_chan {
                    let output_shift = reader.get_s(4)?;
                    if output_shift > max_shift {
                        bail!(BlockError::OutputShiftTooLarge {
                            index: i,
                            value: output_shift,
                            max: max_shift,
                            substream: state.substream_index
                        });
                    }

                    bh.output_shift[i] = Some(output_shift);
                    ss_state.output_shift[i] = output_shift;
                }
            }
        }

        if guards.need_change(GuardsField::QuantiserStepSize) {
            // new_quantiser_step_size
            if reader.get()? {
                for i in 0..=ss_state.max_chan {
                    let quantiser_step_size = reader.get_n(4)?;

                    bh.quantiser_step_size[i] = Some(quantiser_step_size);
                    ss_state.quantiser_step_size[i] = quantiser_step_size;
                }
            }
        }

        for chi in ss_state.min_chan..=ss_state.max_chan {
            // params_for_this_chan
            if reader.get()? {
                bh.channel_params[chi] = Some(ChannelParams::read(state, reader, chi)?);
            }
        }

        Ok(bh)
    }

    pub fn update_decoder_state(&self, state: &mut DecoderState) -> Result<()> {
        if let Some(block_size) = self.block_size {
            state.substream_state_mut()?.block_size = block_size;
        }

        if let Some(matrixing) = &self.matrixing {
            matrixing.update_decoder_state(state)?;
        }

        let ss_state = state.substream_state_mut()?;

        for (i, output_shift) in self.output_shift.iter().enumerate() {
            if let Some(output_shift) = output_shift {
                ss_state.output_shift[i] = *output_shift;
            }
        }

        for (i, quantiser_step_size) in self.quantiser_step_size.iter().enumerate() {
            if let Some(quantiser_step_size) = quantiser_step_size {
                ss_state.quantiser_step_size[i] = *quantiser_step_size;
            }
        }

        for (i, channel_param) in self.channel_params.iter().enumerate() {
            if let Some(channel_param) = channel_param {
                channel_param.update_decoder_state(state, i)?;
            }
        }

        Ok(())
    }
}

impl Block {
    pub fn read(state: &mut ParserState, reader: &mut BsIoSliceReader) -> Result<Self> {
        let mut b = Block::default();

        // block_header_exists
        if reader.get()? {
            // restart_header_exists
            if reader.get()? {
                b.restart_header = Some(RestartHeader::read(state, reader)?);
            }

            b.block_header = Some(BlockHeader::read(state, reader)?);
        }

        b.block_data_bits = if state.substream_state()?.error_protect {
            let block_data_bits = reader.get_n(16)?;
            if block_data_bits > 16000 {
                bail!(BlockError::BlockDataBitsTooLarge(block_data_bits));
            }
            Some(block_data_bits)
        } else {
            None
        };

        // TODO: for all substreams
        if !state.has_parsed_substream && state.substream_state()?.block_index == 0 {
            // latency
            let au_offset = state.au_counter - state.last_major_sync_index;
            let samples_per_au = state.samples_per_au;
            let sample_offset = au_offset * samples_per_au;

            let output_timing = state.output_timing;
            let input_timing = state.input_timing;

            trace!(
                "AU {}: au_offset = {}, samples_per_au = {}",
                // , wrapped output_timing = {}, wrapped input_timing = {}",
                state.au_counter,
                au_offset,
                samples_per_au,
                // output_timing,
                // input_timing
            );

            let mut prev_latency = state.substream_state()?.latency;
            let latency = sample_offset
                .wrapping_add(output_timing)
                .wrapping_sub(input_timing)
                & 0xFFFF;

            {
                let ss_state = state.substream_state_mut()?;
                ss_state.prev_latency = prev_latency;
                ss_state.latency = latency;
            }

            if !state.is_major_sync {
                state.advance = latency.wrapping_sub(samples_per_au);
            }

            trace!(
                "AU {}: latency = {}, prev_latency = {}, advance = {}",
                state.au_counter, latency, prev_latency, state.advance
            );

            if state.flags & 0x8000 == 0 || (!state.has_parsed_au) {
                prev_latency = latency;
            } else if prev_latency != latency {
                log_or_err!(
                    state,
                    Warn,
                    anyhow!(BlockError::LatencyInconsistent {
                        substream: state.substream_index
                    })
                );
            }

            if state.fifo_duration > prev_latency {
                log_or_err!(
                    state,
                    Warn,
                    anyhow!(BlockError::DurationExceedsLatency {
                        duration: state.fifo_duration,
                        latency
                    })
                );
            }

            let samples_per_75ms = (state.audio_sampling_frequency_1 * 3).div_ceil(40);

            if prev_latency as u32 > samples_per_75ms {
                log_or_err!(
                    state,
                    Warn,
                    anyhow!(BlockError::LatencyTooHigh {
                        latency: prev_latency,
                        samples: samples_per_75ms
                    })
                );
            }

            if prev_latency < samples_per_au {
                log_or_err!(
                    state,
                    Warn,
                    anyhow!(BlockError::LatencyTooLow {
                        latency: prev_latency,
                        au: samples_per_au
                    })
                );
            }

            // update output timing
            {
                let i = state.substream_state()?.history_index;
                let output_timing = if !state.has_parsed_au {
                    state.output_timing
                } else {
                    let i = i.wrapping_sub(1) & 0x7F;

                    state.substream_state()?.output_timing_history[i] + state.samples_per_au
                };

                state.substream_state_mut()?.output_timing_history[i] = output_timing;

                let substream_size = if state.substream_index == 0 {
                    state.substream_state()?.substream_end_ptr
                        - (state.substream_segment_start_pos >> 4) as u16
                } else {
                    state.substream_state()?.substream_end_ptr
                        - state.substream_state[state.substream_index - 1].substream_end_ptr
                };

                state.substream_state_mut()?.substream_size_history[i] =
                    (substream_size as usize) << 1;

                state.substream_state_mut()?.history_index = i.wrapping_add(1) & 0x7F;

                trace!(
                    "AU {}: unwrapped_output_timing = {}",
                    // , substream_size = {}, history_index = {}",
                    state.au_counter,
                    // state.substream_index,
                    output_timing,
                    // state.substream_state()?.substream_size_history[i],
                    // state.substream_state()?.history_index
                );
            }
        }

        let ParserSubstreamState {
            restart_sync_word,
            min_chan,
            max_chan,
            max_lsbs,
            block_size,
            error_protect,

            primitive_matrices,

            huff_offset,
            huff_lsbs,
            huff_type,
            lsb_bypass_used,
            lsb_bypass_bit_count,
            quantiser_step_size,
            ..
        } = *state.substream_state()?;

        let block_data_start_pos = reader.position()?;

        for (chi, &lsbs) in huff_lsbs
            .iter()
            .enumerate()
            .take(max_chan + 1)
            .skip(min_chan)
        {
            if lsbs > max_lsbs {
                bail!(BlockError::HuffLsbsTooLarge {
                    channel: chi,
                    actual: lsbs as usize,
                    max: max_lsbs as usize
                });
            }
        }

        for blki in 0..block_size {
            // bypassed_lsb
            let bypassed_lsb_start_pos = reader.position()?;

            if restart_sync_word == 0x31EC {
                for pmi in 0..primitive_matrices {
                    let lsb_bypass_bit_count = lsb_bypass_bit_count[pmi];

                    b.bypassed_lsb[blki][pmi] = if lsb_bypass_bit_count != 0 {
                        reader.get_n::<u8>(lsb_bypass_bit_count as u32)?
                    } else {
                        0
                    } as i32;
                }
            } else {
                for pmi in 0..primitive_matrices {
                    let lsb_bypass_used = lsb_bypass_used[pmi];

                    b.bypassed_lsb[blki][pmi] = if lsb_bypass_used {
                        reader.get_n::<u8>(1)?
                    } else {
                        0
                    } as i32;
                }
            }

            let bypassed_lsb_bits = reader.position()? - bypassed_lsb_start_pos;
            let block_data = &mut b.block_data[blki];

            let mut channel_data = [0i32; 16];
            let mut position_checks_needed = false;

            // huff decode
            for chi in min_chan..=max_chan {
                let huff_offset = huff_offset[chi];
                let huff_type = huff_type[chi];
                let huff_lsbs = huff_lsbs[chi];
                let quantiser_step_size = quantiser_step_size[chi];
                if quantiser_step_size > huff_lsbs {
                    bail!(BlockError::QuantiserStepTooLarge);
                }

                let lsbs_bits = huff_lsbs - quantiser_step_size;
                let huff_start_pos = if restart_sync_word != 0x31EC {
                    position_checks_needed = true;
                    reader.position()?
                } else {
                    0
                };

                let mut audio_data = if huff_type != 0 {
                    let huff_code = reader.get_huffman(huff_type)?;
                    let lsbs = if lsbs_bits > 0 {
                        reader.get_n::<u32>(lsbs_bits)? as i32
                    } else {
                        0
                    };
                    let shift = lsbs_bits as i32 + (2 - huff_type as i32);

                    lsbs + (huff_code << lsbs_bits) - if shift < 0 { 0 } else { 1 << shift }
                } else {
                    let lsbs = if lsbs_bits > 0 {
                        reader.get_n::<u32>(lsbs_bits)? as i32
                    } else {
                        0
                    };
                    lsbs - (if lsbs_bits > 0 {
                        1 << (lsbs_bits - 1)
                    } else {
                        0
                    })
                };

                audio_data += huff_offset;
                audio_data <<= quantiser_step_size;

                if position_checks_needed {
                    let huff_size = reader.position()? - huff_start_pos;

                    if audio_data >= 1 << 23 {
                        bail!(BlockError::HuffmanPositiveSaturation);
                    } else if audio_data < -(1 << 23) {
                        bail!(BlockError::HuffmanNegativeSaturation);
                    }

                    if chi == min_chan && huff_size + bypassed_lsb_bits > 32 {
                        warn!(
                            "Channel {chi}: LSB + Huffman bits ({}) exceed 32-bit limit",
                            huff_size + bypassed_lsb_bits
                        )
                    } else if huff_size > 29 {
                        bail!(BlockError::HuffmanSampleTooLong);
                    }
                }

                channel_data[chi] = audio_data;
            }

            block_data[min_chan..(max_chan + 1)]
                .copy_from_slice(&channel_data[min_chan..(max_chan + 1)]);
        }

        if let Some(block_data_bits) = b.block_data_bits {
            let actual_block_data_bits = reader.position()? - block_data_start_pos;
            if actual_block_data_bits != block_data_bits as u64 {
                bail!(BlockError::BlockDataBitCountMismatch {
                    expected: block_data_bits,
                    actual: actual_block_data_bits,
                });
            }
        }

        if error_protect {
            b.block_header_crc = reader.get_n(8)?;
            info!(
                "Block header CRC found: {:#02X} (error protection enabled)",
                b.block_header_crc
            );
        }

        Ok(b)
    }

    pub fn update_decoder_state(&self, state: &mut DecoderState) -> Result<()> {
        if let Some(restart_header) = &self.restart_header {
            restart_header.update_decoder_state(state)?;
        }

        if let Some(block_header) = &self.block_header {
            block_header.update_decoder_state(state)?;
        }

        let ss_state = state.substream_state_mut()?;
        ss_state.bypassed_lsb = self.bypassed_lsb;
        ss_state.block_data = self.block_data;

        Ok(())
    }
}
