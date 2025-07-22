//! Channel configuration and parameter structures.
//!
//! Contains channel assignments, filter coefficients, and audio
//! processing parameters for individual channels in audio streams.

use anyhow::{Result, anyhow, bail};
use log::Level::Error;
use log::warn;
use std::fmt::Display;

use crate::log_or_err;
use crate::process::decode::DecoderState;
use crate::process::parse::ParserState;
use crate::structs::filter::{CoeffType, FilterCoeffs};
use crate::structs::restart_header::GuardsField;
use crate::utils::bitstream_io::BsIoSliceReader;
use crate::utils::errors::ChannelError;

/// Extended channel meaning information for 16-channel presentations.
///
/// Contains dialogue normalization, mix levels, channel counts, and object
/// audio metadata for channel configurations.
#[derive(Debug, Clone, Default)]
pub struct ExtraChannelMeaning {
    pub extra_channel_meaning_length: u8,
    pub sixteench_dialogue_norm: u8,
    pub sixteench_mix_level: u8,
    pub sixteench_channel_count: u8,
    pub dyn_object_only: bool,
    pub lfe_present: bool,
    pub sixteench_content_description: u8,
    pub chan_distribute: bool,
    pub lfe_only: bool,
    pub sixteench_channel_assignment: u16,
    pub sixteench_isf: u8,
    pub sixteench_dynamic_object_count: u8,
}

impl ExtraChannelMeaning {
    fn read(state: &mut ParserState, reader: &mut BsIoSliceReader) -> Result<Self> {
        let mut end_pos = reader.position()?;

        let extra_channel_meaning_length = reader.get_n(4)?;

        end_pos += (extra_channel_meaning_length as u64 + 1) << 4;

        let mut ecm = ExtraChannelMeaning {
            extra_channel_meaning_length,
            ..Default::default()
        };

        if state.substream_info >> 7 != 0 {
            ecm.sixteench_dialogue_norm = reader.get_n(5)?;
            ecm.sixteench_mix_level = reader.get_n(6)?;
            ecm.sixteench_channel_count = reader.get_n(5)?;
            ecm.dyn_object_only = reader.get()?;

            if ecm.dyn_object_only {
                ecm.lfe_present = reader.get()?;
            } else {
                ecm.sixteench_content_description = reader.get_n(4)?;

                if ecm.sixteench_content_description & 1 != 0 {
                    ecm.chan_distribute = reader.get()?;

                    reader.skip_n(1)?;

                    ecm.lfe_only = reader.get()?;

                    if !ecm.lfe_only {
                        reader.skip_n(1)?;

                        ecm.sixteench_channel_assignment = reader.get_n(10)?;
                    }
                }

                if ecm.sixteench_content_description & 2 != 0 {
                    ecm.sixteench_isf = reader.get_n(3)?;
                }

                if ecm.sixteench_content_description & 4 != 0 {
                    ecm.sixteench_dynamic_object_count = reader.get_n(5)?;
                }
            }

            let pos = reader.position()?;

            reader.seek((end_pos - pos) as i64)?;
        }

        Ok(ecm)
    }
}

#[derive(Debug, Clone, Default)]
pub struct ChannelMeaning {
    pub heavy_drc_start_up_gain: i8,
    pub twoch_control_enabled: bool,
    pub sixch_control_enabled: bool,
    pub eightch_control_enabled: bool,
    pub reserved1: bool,
    pub drc_start_up_gain: i8,
    pub twoch_dialogue_norm: u8,
    pub twoch_mix_level: u8,
    pub sixch_dialogue_norm: u8,
    pub sixch_mix_level: u8,
    pub sixch_source_format: u8,
    pub eightch_dialogue_norm: u8,
    pub eightch_mix_level: u8,
    pub eightch_source_format: u8,
    pub reserved2: bool,
    pub extra_channel_meaning_present: bool,
    pub extra_channel_meaning: Option<ExtraChannelMeaning>,
}

impl ChannelMeaning {
    pub fn read(state: &mut ParserState, reader: &mut BsIoSliceReader) -> Result<Self> {
        let mut cm = ChannelMeaning {
            heavy_drc_start_up_gain: reader.get_s(6)?,
            twoch_control_enabled: reader.get()?,
            sixch_control_enabled: reader.get()?,
            eightch_control_enabled: reader.get()?,
            reserved1: reader.get()?,
            drc_start_up_gain: reader.get_s(7)?,
            twoch_dialogue_norm: reader.get_n(6)?,
            twoch_mix_level: reader.get_n(6)?,
            sixch_dialogue_norm: reader.get_n(5)?,
            sixch_mix_level: reader.get_n(6)?,
            sixch_source_format: reader.get_n(5)?,
            eightch_dialogue_norm: reader.get_n(5)?,
            eightch_mix_level: reader.get_n(6)?,
            eightch_source_format: reader.get_n(6)?,
            reserved2: reader.get()?,
            extra_channel_meaning_present: reader.get()?,
            ..Default::default()
        };

        if state.has_parsed_au {
            if let Some(substreams) = state.substreams {
                for i in 0..substreams {
                    let ss_state = state.substream_i_state_mut(i)?;

                    let heavy_drc_startup_gain = (cm.heavy_drc_start_up_gain as f64 * 0.25).exp2();
                    let heavy_drc_update_gain =
                        (ss_state.heavy_drc_gain_update as f64 * 0.03125).exp2();
                    if ss_state.heavy_drc_active && heavy_drc_startup_gain > heavy_drc_update_gain {
                        warn!(
                            "heavy_drc_start_up_gain too large, heavy_drc_start_up_gain={heavy_drc_startup_gain} (linear),\
                             heavy_drc_update_gain[{i}]={heavy_drc_update_gain} (linear)."
                        );
                    }

                    let drc_startup_gain = (cm.drc_start_up_gain as f64 * 0.0625).exp2();
                    let drc_update_gain = (ss_state.drc_gain_update as f64 * 0.015625).exp2();
                    if ss_state.drc_active && drc_startup_gain > drc_update_gain {
                        warn!(
                            "drc_start_up_gain too large, drc_start_up_gain={drc_startup_gain} (linear),\
                             drc_update_gain[{i}]={drc_update_gain} (linear)."
                        );
                    }
                }
            }
        }

        if cm.extra_channel_meaning_present {
            cm.extra_channel_meaning = Some(ExtraChannelMeaning::read(state, reader)?);

            // is this even needed?
            reader.align_16bit()?;
        }

        Ok(cm)
    }
}

#[derive(Debug, Default)]
pub struct ChannelParams {
    pub coeffs_a: Option<FilterCoeffs>,
    pub coeffs_b: Option<FilterCoeffs>,
    pub huff_offset: Option<i32>,
    pub huff_type: usize,
    pub huff_lsbs: u32,
}

impl ChannelParams {
    pub fn read(state: &mut ParserState, reader: &mut BsIoSliceReader, chi: usize) -> Result<Self> {
        let mut cp = ChannelParams::default();

        let mut new_filter = false;
        let guards = state.substream_state()?.guards;

        if guards.need_change(GuardsField::CoeffsA) {
            // new_coeffs_a
            if reader.get()? {
                let coeffs_a = FilterCoeffs::read(reader, CoeffType::A)?;

                new_filter = true;
                cp.coeffs_a = Some(coeffs_a);
            }
        }

        if guards.need_change(GuardsField::CoeffsB) {
            // new_coeffs_b
            if reader.get()? {
                let coeffs_b = FilterCoeffs::read(reader, CoeffType::B)?;

                new_filter = true;
                cp.coeffs_b = Some(coeffs_b);
            }
        }

        if new_filter {
            // *(a2+20152)++
        }

        if let (Some(coeffs_a), Some(coeffs_b)) = (&mut cp.coeffs_a, &cp.coeffs_b) {
            if coeffs_a.order + coeffs_b.order > 8 {
                log_or_err!(
                    state,
                    log::Level::Error,
                    anyhow!(ChannelError::FilterOrderTooHigh {
                        a: coeffs_a.order,
                        b: coeffs_b.order
                    })
                );
            }

            if coeffs_b.order != 0 {
                if coeffs_a.order != 0 && coeffs_b.coeff_q != coeffs_a.coeff_q {
                    log_or_err!(
                        state,
                        log::Level::Error,
                        anyhow!(ChannelError::CoeffQMismatch {
                            chan: chi,
                            a_q: coeffs_a.coeff_q,
                            b_q: coeffs_b.coeff_q
                        })
                    );
                }

                if coeffs_a.order == 0 {
                    coeffs_a.coeff_q = coeffs_b.coeff_q;
                }
            }
        }

        let ss_state = state.substream_state_mut()?;

        if guards.need_change(GuardsField::HuffOffset) {
            // new_huff_offset
            if reader.get()? {
                let huff_offset = reader.get_s(15)?;

                ss_state.huff_offset[chi] = huff_offset;
                cp.huff_offset = Some(huff_offset);
            }
        }

        cp.huff_type = reader.get_n::<u8>(2)? as usize;
        cp.huff_lsbs = reader.get_n(5)?;

        let max_huff_lsbs = if ss_state.restart_sync_word == 0x31EC {
            31
        } else {
            24
        };

        ss_state.huff_lsbs[chi] = cp.huff_lsbs;
        ss_state.huff_type[chi] = cp.huff_type;

        if cp.huff_lsbs > max_huff_lsbs {
            log_or_err!(
                state,
                Error,
                anyhow!(ChannelError::HuffLsbsTooLarge {
                    chan: chi,
                    max: max_huff_lsbs,
                    actual: cp.huff_lsbs
                })
            );
        }

        Ok(cp)
    }

    pub fn update_decoder_state(&self, state: &mut DecoderState, chi: usize) -> Result<()> {
        if let Some(coeffs_a) = &self.coeffs_a {
            coeffs_a.update_decoder_state(state, CoeffType::A, chi)?;
        }

        if let Some(coeffs_b) = &self.coeffs_b {
            coeffs_b.update_decoder_state(state, CoeffType::B, chi)?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelLabel {
    L,
    R,
    C,
    LFE,
    Ls,
    Rs,
    Tfl,
    Tfr,
    Tsl,
    Tsr,
    Tbl,
    Tbr,
    Lsc,
    Rsc,
    Lb,
    Rb,
    Cb,
    Tc,
    Lsd,
    Rsd,
    Lw,
    Rw,
    Tfc,
    LFE2,
}

impl ChannelLabel {
    pub fn from_sixch_channel(sixch_channel_assignment: u8) -> Result<Vec<Self>> {
        let mut labels = Vec::new();

        for i in 0..5 {
            if sixch_channel_assignment >> i & 1 == 1 {
                match i {
                    0 => labels.extend(vec![Self::L, Self::R]),
                    1 => labels.push(Self::C),
                    2 => labels.push(Self::LFE),
                    3 => labels.extend(vec![Self::Ls, Self::Rs]),
                    4 => labels.extend(vec![Self::Tfl, Self::Tfr]),
                    _ => unreachable!(),
                }
            }
        }

        Ok(labels)
    }

    pub fn from_eightch_channel(eightch_channel_assignment: u16, flags: u16) -> Result<Vec<Self>> {
        let mut labels = Vec::new();

        if flags & 0x800 != 0 {
            for i in 0..5 {
                if eightch_channel_assignment >> i & 1 == 1 {
                    match i {
                        0 => labels.extend(vec![Self::L, Self::R]),
                        1 => labels.push(Self::C),
                        2 => labels.push(Self::LFE),
                        3 => labels.extend(vec![Self::Ls, Self::Rs]),
                        4 => labels.extend(vec![Self::Tsl, Self::Tsr]),
                        _ => unreachable!(),
                    }
                }
            }
        } else {
            for i in 0..13 {
                if eightch_channel_assignment >> i & 1 == 1 {
                    match i {
                        0 => labels.extend(vec![Self::L, Self::R]),
                        1 => labels.push(Self::C),
                        2 => labels.push(Self::LFE),
                        3 => labels.extend(vec![Self::Ls, Self::Rs]),
                        4 => labels.extend(vec![Self::Tfl, Self::Tfr]),
                        5 => labels.extend(vec![Self::Lsc, Self::Rsc]),
                        6 => labels.extend(vec![Self::Lb, Self::Rb]),
                        7 => labels.push(Self::Cb),
                        8 => labels.push(Self::Tc),
                        9 => labels.extend(vec![Self::Lsd, Self::Rsd]),
                        10 => labels.extend(vec![Self::Lw, Self::Rw]),
                        11 => labels.push(Self::Tfc),
                        12 => labels.push(Self::LFE2),
                        _ => unreachable!(),
                    }
                }
            }
        }

        Ok(labels)
    }

    pub fn from_sixteenth_channel(sixteench_channel_assignment: u16) -> Result<Vec<Self>> {
        let mut labels = Vec::new();

        for i in 0..10 {
            if sixteench_channel_assignment >> i & 1 == 1 {
                match i {
                    0 => labels.extend(vec![Self::L, Self::R]),
                    1 => labels.push(Self::C),
                    2 => labels.push(Self::LFE),
                    3 => labels.extend(vec![Self::Ls, Self::Rs]),
                    4 => labels.extend(vec![Self::Lb, Self::Rb]),
                    5 => labels.extend(vec![Self::Tfl, Self::Tfr]),
                    6 => labels.extend(vec![Self::Tsl, Self::Tsr]),
                    7 => labels.extend(vec![Self::Tbl, Self::Tbr]),
                    8 => labels.extend(vec![Self::Lw, Self::Rw]),
                    9 => labels.push(Self::LFE2),
                    _ => unreachable!(),
                }
            }
        }

        Ok(labels)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelGroup {
    Stereo,
    LtRt,
    LbinRbin,
    Mono,
}

impl Display for ChannelGroup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChannelGroup::Stereo => write!(f, "Stereo"),
            ChannelGroup::LtRt => write!(f, "Lt/Rt"),
            ChannelGroup::LbinRbin => write!(f, "Lbin/Rbin"),
            ChannelGroup::Mono => write!(f, "Dual Mono"),
        }
    }
}

impl ChannelGroup {
    pub fn from_modifier(modifier: u8) -> Result<Self> {
        match modifier {
            0 => Ok(ChannelGroup::Stereo),
            1 => Ok(ChannelGroup::LtRt),
            2 => Ok(ChannelGroup::LbinRbin),
            3 => Ok(ChannelGroup::Mono),
            _ => bail!("Invalid channel group modifier: {}", modifier),
        }
    }
}
