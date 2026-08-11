//! Channel configuration and parameter structures.
//!
//! Contains channel assignments, filter coefficients, and audio
//! processing parameters for individual channels in audio streams.

use anyhow::{Result, anyhow, bail};
use log::Level::{Error, Warn};
use std::fmt::Display;

use crate::log_or_err;
use crate::process::decode::DecoderState;
use crate::process::parse::ParserState;
use crate::structs::filter::{CoeffType, FilterCoeffs};
use crate::structs::restart_header::GuardsField;
use crate::structs::sync::MAJOR_SYNC_FBB;
use crate::utils::bitstream_io::BsIoSliceReader;
use crate::utils::errors::{ChannelError, SyncError};

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

/// The `channel_meaning` block of an FBA (Dolby TrueHD) major sync.
///
/// 64 bits of presentation-level metadata, optionally followed by an
/// [`ExtraChannelMeaning`] block.
#[derive(Debug, Clone, Default)]
pub struct FbaChannelMeaning {
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

/// The `channel_meaning` block of an FBB (DVD-Audio) major sync.
///
/// Also 64 bits, and in the same place as [`FbaChannelMeaning`], but an entirely
/// different structure: it describes the PCM source the packing was applied to rather
/// than a set of decoder presentations. Because the two are the same width, reading the
/// wrong one still leaves the bit reader aligned and the major sync CRC intact, so the
/// syntax has to be dispatched on rather than discovered.
#[derive(Debug, Clone, Default)]
pub struct FbbChannelMeaning {
    pub fs: u8,
    pub wordwidth: u8,
    pub channel_occupancy: u8,
    pub mlp_multi_channel_type: u8,
    pub speaker_layout: u16,
    pub copy_protection: u8,
    pub level_control: u16,

    /// The source PCM carries control data in its least significant bits.
    pub hdcd_process: bool,

    pub reserved2: u8,
    pub source_format: u8,
    pub summary_info: u8,
}

impl FbbChannelMeaning {
    fn read(state: &mut ParserState, reader: &mut BsIoSliceReader) -> Result<Self> {
        let cm = FbbChannelMeaning {
            fs: reader.get_n(5)?,
            wordwidth: reader.get_n(5)?,
            channel_occupancy: reader.get_n(6)?,
            mlp_multi_channel_type: reader.get_n(3)?,
            speaker_layout: reader.get_n(10)?,
            copy_protection: reader.get_n(3)?,
            level_control: reader.get_n(16)?,
            hdcd_process: reader.get()?,
            reserved2: reader.get_n(6)?,
            source_format: reader.get_n(4)?,
            summary_info: reader.get_n(5)?,
        };

        // hdcd_process and the six bits after it form one block expected to be zero, so a
        // set hdcd_process is reported the same way a reserved bit is.
        let reserved = u8::from(cm.hdcd_process) << 6 | cm.reserved2;

        if reserved != 0 {
            log_or_err!(
                state,
                log::Level::Debug,
                anyhow!(SyncError::ReservedChannelMeaningNonZero(reserved)),
                reader
            );
        }

        Ok(cm)
    }
}

/// The `channel_meaning` block, in whichever of the two layouts the syntax calls for.
#[derive(Debug, Clone)]
pub enum ChannelMeaning {
    Fba(FbaChannelMeaning),
    Fbb(FbbChannelMeaning),
}

impl Default for ChannelMeaning {
    fn default() -> Self {
        Self::Fba(FbaChannelMeaning::default())
    }
}

impl ChannelMeaning {
    pub fn read(state: &mut ParserState, reader: &mut BsIoSliceReader) -> Result<Self> {
        match state.format_sync {
            MAJOR_SYNC_FBB => Ok(Self::Fbb(FbbChannelMeaning::read(state, reader)?)),
            _ => Ok(Self::Fba(FbaChannelMeaning::read(state, reader)?)),
        }
    }

    /// The FBA block, or `None` for a stream whose syntax does not carry one.
    pub fn fba(&self) -> Option<&FbaChannelMeaning> {
        match self {
            Self::Fba(cm) => Some(cm),
            Self::Fbb(_) => None,
        }
    }

    /// The FBB block, or `None` for a stream whose syntax does not carry one.
    pub fn fbb(&self) -> Option<&FbbChannelMeaning> {
        match self {
            Self::Fbb(cm) => Some(cm),
            Self::Fba(_) => None,
        }
    }

    /// The extra channel meaning block, which only the FBA syntax can carry.
    pub fn extra_channel_meaning(&self) -> Option<&ExtraChannelMeaning> {
        self.fba()?.extra_channel_meaning.as_ref()
    }
}

impl FbaChannelMeaning {
    fn read(state: &mut ParserState, reader: &mut BsIoSliceReader) -> Result<Self> {
        let mut cm = FbaChannelMeaning {
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

        if state.has_parsed_au
            && let Some(substreams) = state.substreams
        {
            for i in 0..substreams {
                let ss_state = state.substream_i_state(i)?;
                let (drc_active, drc_gain_update) = (ss_state.drc_active, ss_state.drc_gain_update);
                let (heavy_drc_active, heavy_drc_gain_update) =
                    (ss_state.heavy_drc_active, ss_state.heavy_drc_gain_update);

                // What a decoder joining here applies until the first update reaches it,
                // and it may not exceed the gain the substream already runs at.
                let heavy_drc_startup_gain = (cm.heavy_drc_start_up_gain as f64 * 0.25).exp2();
                let heavy_drc_update_gain = (heavy_drc_gain_update as f64 * 0.03125).exp2();
                if heavy_drc_active && heavy_drc_startup_gain > heavy_drc_update_gain {
                    log_or_err!(
                        state,
                        Warn,
                        anyhow!(ChannelError::HeavyDrcStartUpGainTooLarge {
                            index: i,
                            start_up_gain: heavy_drc_startup_gain,
                            update_gain: heavy_drc_update_gain,
                        }),
                        reader
                    );
                }

                let drc_startup_gain = (cm.drc_start_up_gain as f64 * 0.0625).exp2();
                let drc_update_gain = (drc_gain_update as f64 * 0.015625).exp2();
                if drc_active && drc_startup_gain > drc_update_gain {
                    log_or_err!(
                        state,
                        Warn,
                        anyhow!(ChannelError::DrcStartUpGainTooLarge {
                            index: i,
                            start_up_gain: drc_startup_gain,
                            update_gain: drc_update_gain,
                        }),
                        reader
                    );
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
        let guards = state.substream_state()?.restart.guards;

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
                    }),
                    reader
                );
            }

            if coeffs_b.order != 0 && coeffs_a.order != 0 && coeffs_b.coeff_q != coeffs_a.coeff_q {
                log_or_err!(
                    state,
                    Error,
                    anyhow!(ChannelError::CoeffQMismatch {
                        chan: chi,
                        a_q: coeffs_a.coeff_q,
                        b_q: coeffs_b.coeff_q
                    }),
                    reader
                );
            }
        }

        let restart = &mut state.substream_state_mut()?.restart;

        if guards.need_change(GuardsField::HuffOffset) {
            // new_huff_offset
            if reader.get()? {
                let huff_offset = reader.get_s(15)?;

                restart.huff_offset[chi] = huff_offset;
                cp.huff_offset = Some(huff_offset);
            }
        }

        cp.huff_type = reader.get_n::<u8>(2)? as usize;
        cp.huff_lsbs = reader.get_n(5)?;

        let max_huff_lsbs = if restart.restart_sync_word == 0x31EC {
            31
        } else {
            24
        };

        restart.huff_lsbs[chi] = cp.huff_lsbs;
        restart.huff_type[chi] = cp.huff_type;

        if cp.huff_lsbs > max_huff_lsbs {
            log_or_err!(
                state,
                Error,
                anyhow!(ChannelError::HuffLsbsTooLarge {
                    chan: chi,
                    max: max_huff_lsbs,
                    actual: cp.huff_lsbs
                }),
                reader
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

        let ss_state = state.substream_state_mut()?;

        if ss_state.order[0][chi] == 0 && ss_state.order[1][chi] != 0 {
            ss_state.coeff_q[0][chi] = ss_state.coeff_q[1][chi];
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
