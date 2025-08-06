//! Object Audio Metadata (OAMD) structures
//!
//! This module contains structures for handling Object Audio Metadata,
//! which provides spatial audio information for immersive audio playback.

use std::default::Default;
use std::mem::transmute;

use crate::utils::bitstream_io::BsIoSliceReader;
use anyhow::Result;
use log::{trace, warn};

pub const MAX_OBJECT_COUNT: usize = 159;
pub const GAIN_MINUS_INFINITY: i8 = -128; // -inf gain

#[derive(Clone, Debug)]
#[repr(C)]
struct OAMDParserState {
    object_count: usize,
    program_assignment: ProgramAssignment,
    b_alternate_object_data_present: bool,

    prev_object_gain: [i8; MAX_OBJECT_COUNT],
    prev_object_basic_info: ObjectBasicInfo,
    prev_object_render_info: ObjectRenderInfo,

    object_element: Option<ObjectElement>,
    trim_element: Option<TrimElement>,
    extended_object_element: Option<ExtendedObjectElement>,
}

impl Default for OAMDParserState {
    fn default() -> Self {
        Self {
            object_count: 0,
            program_assignment: ProgramAssignment::default(),
            b_alternate_object_data_present: false,
            prev_object_gain: [0; MAX_OBJECT_COUNT],
            prev_object_basic_info: ObjectBasicInfo::default(),
            prev_object_render_info: ObjectRenderInfo::default(),
            object_element: None,
            trim_element: None,
            extended_object_element: None,
        }
    }
}

/// Speaker bed assignment for spatial audio channels
#[derive(Copy, Clone, Debug, Default, PartialEq)]
#[repr(C)]
pub struct BedAssignment(pub [bool; 17]);

impl BedAssignment {
    pub fn from_non_std(value: u32) -> Self {
        let mut ret = Self::default();

        ret.0
            .iter_mut()
            .enumerate()
            .for_each(|(i, x)| *x = (value >> i) & 1 == 1);

        ret
    }

    pub fn from_std(value: u16) -> Self {
        let mut ret = Self::default();

        for (i, &bed) in STD_BED_LIST.iter().enumerate() {
            if (value >> i) & 1 == 1 {
                for &n in bed {
                    ret.0[n] = true
                }
            }
        }

        ret
    }

    pub fn with_lfe_only() -> Self {
        let mut ret = Self::default();
        ret.0[SpeakerLabels::LFE as usize] = true;

        ret
    }

    pub fn to_index_vec(&self) -> Vec<usize> {
        self.0
            .iter()
            .enumerate()
            .filter_map(|(i, &sign)| if sign { Some(i) } else { None })
            .collect()
    }

    pub fn is_lfe_only(&self) -> bool {
        *self == Self::with_lfe_only()
    }

    pub fn count_beds(&self) -> usize {
        self.0.iter().fold(0, |count, &sign| count + sign as usize)
    }
}

pub const STD_BED_LIST: [&[usize]; 10] = [
    &[0, 1],
    &[2],
    &[3],
    &[4, 5],
    &[6, 7],
    &[8, 9],
    &[10, 11],
    &[12, 13],
    &[14, 15],
    &[16],
];

pub const ISF_COUNT_LIST: [usize; 6] = [4, 8, 10, 14, 15, 30];

#[derive(Clone, Debug, Default)]
#[repr(C)]
pub struct ProgramAssignment {
    pub b_bed_chan_distribute: bool,
    pub bed_assignment: Vec<BedAssignment>,
    pub num_bed_objects: usize,
    pub num_isf_objects: usize,
    pub num_dynamic_objects: usize,
}

impl ProgramAssignment {
    pub fn b_dyn_object_only_program(&self) -> bool {
        self.num_bed_objects <= 1 && self.num_dynamic_objects == 0 && self.num_isf_objects > 0
    }

    pub fn beds_or_isf_count(&self) -> usize {
        self.num_bed_objects + self.num_isf_objects
    }

    fn read(state: &mut OAMDParserState, reader: &mut BsIoSliceReader) -> Result<Self> {
        let mut prog = Self::default();

        // b_dyn_object_only_program
        if reader.get()? {
            prog.num_dynamic_objects = state.object_count;

            let b_lfe_present = reader.get()?;

            if b_lfe_present {
                prog.bed_assignment.push(BedAssignment::with_lfe_only());
                prog.num_dynamic_objects -= 1;
            }
        } else {
            let content_description = reader.get_n::<u8>(4)?;

            // object(s) with speaker-anchored coordinate(s) (bed objects)
            if content_description & 1 != 0 {
                prog.b_bed_chan_distribute = reader.get()?;

                // b_multiple_bed_instances_present
                let num_bed_instances = if reader.get()? {
                    // num_bed_instances_bits
                    reader.get_n::<u8>(3)? + 2
                } else {
                    1
                };

                for _ in 0..num_bed_instances {
                    // b_lfe_only
                    let bed_assignment = if reader.get()? {
                        BedAssignment::with_lfe_only()
                    } else {
                        // b_standard_chan_assign
                        if reader.get()? {
                            BedAssignment::from_std(reader.get_n::<u16>(10)?)
                        } else {
                            BedAssignment::from_non_std(reader.get_n::<u32>(17)?)
                        }
                    };

                    prog.bed_assignment.push(bed_assignment)
                }
            }

            // intermediate spatial format (ISF)
            if content_description & 2 != 0 {
                let intermediate_spatial_format_idx = reader.get_n::<u8>(3)?;
                prog.num_isf_objects = ISF_COUNT_LIST[intermediate_spatial_format_idx as usize];
            }

            // object(s) with room-anchored or screen-anchored coordinates
            if content_description & 4 != 0 {
                let mut num_dynamic_objects = reader.get_n::<u8>(5)?;
                if num_dynamic_objects == 31 {
                    num_dynamic_objects += reader.get_n::<u8>(7)?;
                };

                prog.num_dynamic_objects = num_dynamic_objects as usize + 1;
            }

            // reserved
            if content_description & 8 != 0 {
                let reserved_data_size = (reader.get_n::<u32>(4)? + 1) << 3;
                reader.skip_n(reserved_data_size)?;
            }
        };

        for &a in prog.bed_assignment.iter() {
            prog.num_bed_objects += a.count_beds();
        }

        Ok(prog)
    }
}

#[derive(Clone, Debug, Default)]
#[repr(C)]
pub struct ObjectAudioMetadataPayload {
    pub evo_sample_offset: u64,
    pub oamd_version: u8,
    pub object_count: usize,
    pub program_assignment: ProgramAssignment,
    pub b_alternate_object_data_present: bool,

    pub object_element: Option<ObjectElement>,
    pub trim_element: Option<TrimElement>,
    pub extended_object_element: Option<ExtendedObjectElement>,
    pub oa_element_md: Vec<OAElementMD>,
}

impl ObjectAudioMetadataPayload {
    pub fn read(bytes: &[u8]) -> Result<Self> {
        let state = &mut OAMDParserState::default();
        let reader = &mut BsIoSliceReader::from_slice(bytes);

        let mut oamd_version = reader.get_n::<u8>(2)?;

        if oamd_version == 3 {
            oamd_version += reader.get_n::<u8>(3)?;
        }

        assert_eq!(oamd_version, 0, "Unsupported OAMD version {oamd_version}");

        let mut object_count_bits = reader.get_n::<u8>(5)?;

        if object_count_bits == 31 {
            object_count_bits += reader.get_n::<u8>(7)?;
        }

        let object_count = (object_count_bits + 1) as usize;
        state.object_count = object_count;

        let program_assignment = ProgramAssignment::read(state, reader)?;
        state.program_assignment = program_assignment.clone();

        let b_alternate_object_data_present = reader.get()?;
        state.b_alternate_object_data_present = b_alternate_object_data_present;

        let mut oa_element_count = reader.get_n::<u8>(4)?;
        if oa_element_count == 15 {
            oa_element_count += reader.get_n::<u8>(5)?;
        }

        let oa_element_md = (0..oa_element_count)
            .map(|_| OAElementMD::read(state, reader))
            .collect::<Result<Vec<_>>>()?;

        let payload = Self {
            evo_sample_offset: 0,
            oamd_version,
            object_count,
            program_assignment,
            b_alternate_object_data_present,
            object_element: state.object_element.clone(),
            trim_element: state.trim_element.clone(),
            extended_object_element: state.extended_object_element.clone(),
            oa_element_md,
        };

        Ok(payload)
    }

    pub fn get_damf_pos(&self) -> Vec<Vec<[f64; 3]>> {
        let mut damf_pos = vec![vec![]; self.object_count];

        if let Some(object_element) = &self.object_element {
            for (object_index, object_data) in object_element.object_data.iter().enumerate() {
                for block in object_data {
                    damf_pos[object_index].push(block.object_render_info.pos3d);
                }
            }
        }

        if let Some(extended_object_element) = &self.extended_object_element {
            for (object_index, object) in extended_object_element
                .ext_prec_pos_block
                .iter()
                .enumerate()
            {
                let pos3d_object = &mut damf_pos[object_index];
                for (block_index, block) in object.iter().enumerate() {
                    let pos3d = &mut pos3d_object[block_index];

                    pos3d[0] += block.ext_prec_pos3d_x;
                    pos3d[1] += block.ext_prec_pos3d_y;
                    pos3d[2] += block.ext_prec_pos3d_z;
                }
            }
        }

        damf_pos.iter_mut().for_each(|pos3d_object| {
            pos3d_object.iter_mut().for_each(|pos3d| {
                pos3d[0] = (pos3d[0].clamp(0.0, 1.0) - 0.5) * 2.0;
                pos3d[1] = (0.5 - pos3d[1].clamp(0.0, 1.0)) * 2.0;
                pos3d[2] = pos3d[2].clamp(-1.0, 1.0);
            })
        });

        damf_pos
    }
}

#[derive(Clone, Debug, Default)]
#[repr(C)]
pub struct OAElementMD {
    pub oa_element_id_idx: u8,
    pub alternate_object_data_id_idx: Option<u8>,
    pub b_discard_unknown_element: bool,
    // pub reserved_data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
#[repr(u8)]
pub enum OAElementType {
    Object = 1,
    Trim = 2,
    Headphone = 3,
    ObjectDescription = 4,
    ExtendObject = 5,
    BedObject = 6,
    Reserved(u8),
}

impl OAElementType {
    pub fn from_u8(n: u8) -> Self {
        match n {
            1 => Self::Object,
            2 => Self::Trim,
            3 => Self::Headphone,
            4 => Self::ObjectDescription,
            5 => Self::ExtendObject,
            6 => Self::BedObject,
            _ => Self::Reserved(n),
        }
    }
}

impl OAElementMD {
    fn read(state: &mut OAMDParserState, reader: &mut BsIoSliceReader) -> Result<Self> {
        let mut md = OAElementMD {
            oa_element_id_idx: reader.get_n::<u8>(4)?,
            ..Default::default()
        };

        let oa_element_size_bits = reader.get_variable_bits_max(4, 4)? as u64;
        let oa_element_size = (oa_element_size_bits + 1) << 3;

        let pos_start = reader.position()?;
        let pos_end = if oa_element_size > reader.available()? {
            // Does this happen?
            warn!("Truncated oa_element_md with id {}", md.oa_element_id_idx);
            reader.available()?
        } else {
            pos_start + oa_element_size
        };

        if state.b_alternate_object_data_present {
            md.alternate_object_data_id_idx = Some(reader.get_n::<u8>(4)?);
        }

        md.b_discard_unknown_element = reader.get()?;

        match OAElementType::from_u8(md.oa_element_id_idx) {
            OAElementType::Object => {
                let object_element = ObjectElement::read(state, reader)?;
                state.object_element = Some(object_element);
            }
            OAElementType::Trim => {
                let trim_element = TrimElement::read(state, reader)?;
                state.trim_element = Some(trim_element);
            }
            OAElementType::ExtendObject => {
                let extended_object_element = ExtendedObjectElement::read(state, reader)?;
                state.extended_object_element = Some(extended_object_element);
            }
            _ => {
                warn!(
                    "Unimplemented oa_element_md type {} with size={oa_element_size}, pos_end={pos_end}, please submit a sample",
                    md.oa_element_id_idx
                )
            }
        }

        // Padding
        let pos_current = reader.position()?;
        if pos_end > pos_current {
            reader.skip_n((pos_end - pos_current) as u32)?;
        }

        trace!(
            "OAMD Element ID: {}, size: {}, start: {}, expected_end: {}, actual_end: {}",
            md.oa_element_id_idx, oa_element_size, pos_start, pos_end, pos_current
        );

        Ok(md)
    }
}

pub type ObjectData = Vec<ObjectInfoBlock>;

#[derive(Clone, Debug)]
#[repr(C)]
pub struct ObjectElement {
    pub md_update_info: MDUpdateInfo,
    pub b_reserved_data_not_present: bool,
    pub reserved_data: u8,
    pub object_data: Vec<ObjectData>,
}

impl Default for ObjectElement {
    fn default() -> Self {
        Self {
            md_update_info: MDUpdateInfo::default(),
            b_reserved_data_not_present: false,
            reserved_data: 32, // TODO: investigate
            object_data: Vec::new(),
        }
    }
}

impl ObjectElement {
    fn read(state: &mut OAMDParserState, reader: &mut BsIoSliceReader) -> Result<Self> {
        let mut element = Self {
            md_update_info: MDUpdateInfo::read(reader)?,
            b_reserved_data_not_present: reader.get()?,
            object_data: Vec::with_capacity(state.object_count),
            ..Default::default()
        };

        if !element.b_reserved_data_not_present {
            element.reserved_data = reader.get_n::<u8>(5)?;
        }

        let block_count = element.md_update_info.num_obj_info_blocks;

        for object_index in 0..state.object_count {
            let mut object_data = ObjectData::default();

            for block_index in 0..block_count {
                object_data.push(ObjectInfoBlock::read(
                    state,
                    reader,
                    object_index,
                    block_index,
                )?);
            }
            element.object_data.push(object_data.clone());
        }

        Ok(element)
    }
}

#[derive(Clone, Debug, Default)]
#[repr(C)]
pub struct MDUpdateInfo {
    pub sample_offset: usize,
    pub num_obj_info_blocks: usize,
    pub block_update_info: Vec<BlockUpdateInfo>,
}

impl MDUpdateInfo {
    pub fn read(reader: &mut BsIoSliceReader) -> Result<Self> {
        // sample_offset_code
        let sample_offset = match reader.get_n::<u8>(2)? {
            0 => 0,
            // sample_offset_idx
            1 => match reader.get_n::<u8>(2)? {
                0 => 8,
                1 => 16,
                2 => 18,
                3 => 24,
                _ => unreachable!(),
            },
            // sample_offset_bits
            2 => reader.get_n::<u8>(5)? as usize,
            // 3 => // reserved
            _ => unreachable!(),
        };

        let num_obj_info_blocks = (reader.get_n::<u8>(3)? + 1) as usize;

        let mut info = Self {
            sample_offset,
            num_obj_info_blocks,
            block_update_info: Vec::with_capacity(num_obj_info_blocks),
        };

        for _block in 0..num_obj_info_blocks {
            info.block_update_info.push(BlockUpdateInfo::read(reader)?);
        }

        Ok(info)
    }
}

#[derive(Clone, Debug, Default)]
#[repr(C)]
pub struct BlockUpdateInfo {
    pub block_offset_factor_bits: u8,
    pub ramp_duration_code: u8,
    pub ramp_duration: u16,
}

impl BlockUpdateInfo {
    pub const RAMP_DURATION_LIST: [u16; 16] = [
        32, 64, 128, 256, 320, 480, 1000, 1001, 1024, 1600, 1601, 1602, 1920, 2000, 2002, 2048,
    ];

    pub fn read(reader: &mut BsIoSliceReader) -> Result<Self> {
        let mut info = Self {
            block_offset_factor_bits: reader.get_n(6)?,
            ramp_duration_code: reader.get_n(2)?,
            ..Default::default()
        };

        info.ramp_duration = match info.ramp_duration_code {
            0 => 0,
            1 => 512,
            2 => 1536,
            3 => unsafe {
                // b_use_ramp_duration_idx
                if reader.get()? {
                    // ramp_duration_idx
                    *Self::RAMP_DURATION_LIST.get_unchecked(reader.get_n::<u64>(4)? as usize)
                } else {
                    // ramp_duration_bits
                    reader.get_n(11)?
                }
            },
            _ => unreachable!(),
        };

        Ok(info)
    }
}

#[derive(Clone, Debug, Default)]
#[repr(C)]
pub struct ObjectInfoBlock {
    pub b_object_not_active: bool,
    pub object_basic_info: ObjectBasicInfo,
    pub b_object_in_bed_or_isf: bool,
    pub object_render_info: ObjectRenderInfo,
}

impl ObjectInfoBlock {
    fn read(
        state: &mut OAMDParserState,
        reader: &mut BsIoSliceReader,
        object_index: usize,
        block_index: usize,
    ) -> Result<Self> {
        let mut info = Self {
            b_object_not_active: reader.get()?,
            ..Default::default()
        };

        let object_basic_info_status_idx = if info.b_object_not_active {
            0
        } else if block_index == 0 {
            1
        } else {
            reader.get_n::<u8>(2)?
        };

        let prev_object_basic_info = if block_index == 0 {
            ObjectBasicInfo::default()
        } else {
            state.prev_object_basic_info.clone()
        };

        info.object_basic_info = match object_basic_info_status_idx {
            0 => ObjectBasicInfo::default(),
            1 | 3 => ObjectBasicInfo::read(
                &prev_object_basic_info,
                state,
                reader,
                object_index,
                block_index,
                object_basic_info_status_idx,
            )?,
            _ => prev_object_basic_info,
        };

        state.prev_object_basic_info = info.object_basic_info.clone();

        info.b_object_in_bed_or_isf = object_index < state.program_assignment.beds_or_isf_count();

        let object_render_info_status_idx = if info.b_object_not_active {
            0
        } else if !info.b_object_in_bed_or_isf {
            if block_index == 0 {
                1
            } else {
                reader.get_n(2)?
            }
        } else {
            0
        };

        let prev_object_render_info = if block_index == 0 {
            ObjectRenderInfo::default()
        } else {
            state.prev_object_render_info.clone()
        };

        info.object_render_info = match object_render_info_status_idx {
            0 => ObjectRenderInfo::default(),
            1 | 3 => ObjectRenderInfo::read(
                &prev_object_render_info,
                reader,
                object_render_info_status_idx,
                block_index,
            )?,
            _ => prev_object_render_info,
        };

        state.prev_object_render_info = info.object_render_info.clone();

        // b_additional_table_data_exists
        if reader.get()? {
            // additional_table_data_size_bits
            let additional_table_data_size = (reader.get_n::<u32>(4)? + 1) << 3;
            reader.skip_n(additional_table_data_size)?;
        }

        Ok(info)
    }
}

#[derive(Clone, Debug)]
#[repr(C)]
pub struct ObjectBasicInfo {
    pub object_gain: i8,
    pub object_priority: f64,
}

impl Default for ObjectBasicInfo {
    fn default() -> Self {
        Self {
            object_gain: GAIN_MINUS_INFINITY,
            object_priority: 0.0,
        }
    }
}

impl ObjectBasicInfo {
    pub fn gain_string(&self) -> String {
        match self.object_gain {
            GAIN_MINUS_INFINITY => "-inf".to_string(),
            _ => format!("{}", self.object_gain),
        }
    }

    fn read(
        prev: &Self,
        state: &mut OAMDParserState,
        reader: &mut BsIoSliceReader,
        object_index: usize,
        block_index: usize,
        object_basic_info_status_idx: u8,
    ) -> Result<Self> {
        let mut basic = prev.clone();

        let obj_basic_info_bits = if object_basic_info_status_idx == 1 {
            3u8
        } else {
            reader.get_n(2)?
        };

        if obj_basic_info_bits & 1 != 0 {
            let prev_object_gain = if object_index == 0 {
                0
            } else {
                state.prev_object_gain[block_index]
            };

            // object_gain_idx
            basic.object_gain = match reader.get_n::<u8>(2)? {
                0 => 0,
                1 => GAIN_MINUS_INFINITY,
                2 => match reader.get_n::<u8>(6)? {
                    0..=14 => 15 - reader.get_n::<u8>(6)? as i8,
                    15..=63 => 14 - reader.get_n::<u8>(6)? as i8,
                    _ => unreachable!(),
                },
                3 => prev_object_gain,
                _ => unreachable!(),
            };

            state.prev_object_gain[block_index] = basic.object_gain;
        }

        if obj_basic_info_bits & 2 != 0 {
            // b_default_object_priority
            basic.object_priority = if reader.get()? {
                1.0
            } else {
                reader.get_n::<u8>(5)? as f64 / 32.0
            }
        }

        Ok(basic)
    }
}

#[derive(Clone, Debug)]
#[repr(C)]
pub struct ObjectRenderInfo {
    pub b_differential_position_specified: bool,
    pub pos3d: [f64; 3],
    pub b_object_distance_specified: bool,
    pub b_object_at_infinity: bool,
    pub distance_factor_idx: u8,
    pub zone_constraints_idx: u8,
    pub b_enable_elevation: bool,
    pub object_size: [f64; 3],
    pub b_object_use_screen_ref: bool,
    pub screen_factor: f64,
    pub depth_factor: f64,
    pub b_object_snap: bool,
}

impl Default for ObjectRenderInfo {
    fn default() -> Self {
        Self {
            b_differential_position_specified: false,
            pos3d: [0.5, 0.5, 0.0],
            b_object_distance_specified: false,
            b_object_at_infinity: false,
            distance_factor_idx: 0,
            zone_constraints_idx: 0,
            b_enable_elevation: true,
            object_size: [0.0, 0.0, 0.0],
            b_object_use_screen_ref: false,
            screen_factor: 0.0,
            depth_factor: 0.25,
            b_object_snap: false,
        }
    }
}

impl ObjectRenderInfo {
    fn read(
        prev: &Self,
        reader: &mut BsIoSliceReader,
        object_render_info_status_idx: u8,
        block_index: usize,
    ) -> Result<Self> {
        let mut render = prev.clone();
        let object_render_info_bits = if object_render_info_status_idx == 1 {
            15
        } else {
            reader.get_n::<u8>(4)?
        };

        if object_render_info_bits & 1 != 0 {
            render.b_differential_position_specified = if block_index == 0 {
                false
            } else {
                reader.get()?
            };

            render.pos3d = if render.b_differential_position_specified {
                let (prev_x, prev_y, prev_z) = (prev.pos3d[0], prev.pos3d[1], prev.pos3d[2]);

                let x = prev_x + reader.get_s::<i8>(3)? as f64 / 62.0;
                let y = prev_y + reader.get_s::<i8>(3)? as f64 / 62.0;
                let z = prev_z + reader.get_s::<i8>(3)? as f64 / 15.0;

                [x, y, z]
            } else {
                let x = reader.get_n::<u64>(6)? as f64 / 62.0;
                let y = reader.get_n::<u64>(6)? as f64 / 62.0;

                let sign_z = if reader.get()? { 1.0 } else { -1.0 };
                let z = reader.get_n::<u64>(4)? as f64 / 15.0 * sign_z;

                [x, y, z]
            };

            render.b_object_distance_specified = reader.get()?;

            // TODO: parse distance
            if render.b_object_distance_specified {
                render.b_object_at_infinity = reader.get()?;
                // object_distance = inf
                if !render.b_object_at_infinity {
                    render.distance_factor_idx = reader.get_n(4)?;
                }
            }
        }

        if object_render_info_bits & 2 != 0 {
            render.zone_constraints_idx = reader.get_n(3)?;
            render.b_enable_elevation = reader.get()?;
        }

        if object_render_info_bits & 4 != 0 {
            // object_size_idx
            render.object_size = match reader.get_n::<u8>(2)? {
                1 => {
                    let object_size = reader.get_n::<u8>(5)? as f64 / 31.0;
                    [object_size; 3]
                }
                2 => {
                    // seems not allowed in Atmos
                    let width = reader.get_n::<u8>(5)? as f64 / 31.0;
                    let depth = reader.get_n::<u8>(5)? as f64 / 31.0;
                    let height = reader.get_n::<u8>(5)? as f64 / 31.0;
                    [width, depth, height]
                }
                // 3 => // reserved
                _ => [0.0; 3],
            };
        }

        if object_render_info_bits & 8 != 0 {
            render.b_object_use_screen_ref = reader.get()?;

            if render.b_object_use_screen_ref {
                render.screen_factor = (reader.get_n::<u8>(3)? + 1) as f64 / 8.0;
                render.depth_factor = 0.25 * (reader.get_n::<u8>(2)? + 1) as f64;
            } else {
                render.screen_factor = 0.0
            }
        }

        render.b_object_snap = reader.get()?;

        Ok(render)
    }
}

pub const NUM_TRIM_CONFIGS: usize = 9;

#[rustfmt::skip]
pub const TRIM_LUT: [f64; 16] = [
    6.0, 3.0, 1.5, 0.75, -0.75, -1.5, -3.0, -4.5, -6.0, -7.5, -9.0, -10.5, -12.0, -13.5, -16.0, -36.0,
];

// TODO: TrimElement & ExtendObjectElement
#[derive(Clone, Debug, Default)]
#[repr(C)]
pub struct TrimElement {
    pub warp_mode: u8,
    pub reserved: u8,
    pub global_trim_mode: u8,
    pub trims: [Option<Trim>; NUM_TRIM_CONFIGS],
    pub b_disable_trim_per_obj: bool,
    pub b_disable_trim: Vec<bool>,
}

#[derive(Clone, Debug)]
#[repr(C)]
pub struct Trim {
    pub b_default_trim: bool,
    pub b_disable_trim: bool,

    pub trim_centre: Option<f64>,
    pub trim_surround: Option<f64>,
    pub trim_height: Option<f64>,
    pub bal3d_y_tb: Option<f64>,
    pub bal3d_y_lis: Option<f64>,
}

impl Default for Trim {
    fn default() -> Trim {
        Trim {
            b_default_trim: true,
            b_disable_trim: false,
            trim_centre: None,
            trim_surround: None,
            trim_height: None,
            bal3d_y_tb: None,
            bal3d_y_lis: None,
        }
    }
}

impl TrimElement {
    fn read(state: &OAMDParserState, reader: &mut BsIoSliceReader) -> Result<Self> {
        let mut element = Self {
            warp_mode: reader.get_n(2)?,
            reserved: reader.get_n(2)?,
            global_trim_mode: reader.get_n(2)?,
            ..Default::default()
        };

        if element.global_trim_mode == 2 {
            for trim in &mut element.trims {
                let mut t = Trim {
                    b_default_trim: reader.get()?,
                    ..Default::default()
                };

                if t.b_default_trim {
                    continue;
                }

                t.b_disable_trim = reader.get()?;
                if !t.b_disable_trim {
                    let trim_balance_presence = reader.get_n::<u8>(5)?;

                    if trim_balance_presence & 0x1 != 0 {
                        t.trim_centre = Some(TRIM_LUT[reader.get_n::<u8>(4)? as usize]);
                    }

                    if trim_balance_presence & 0x2 != 0 {
                        t.trim_surround = Some(TRIM_LUT[reader.get_n::<u8>(4)? as usize]);
                    }

                    if trim_balance_presence & 0x4 != 0 {
                        t.trim_height = Some(TRIM_LUT[reader.get_n::<u8>(4)? as usize]);
                    }

                    if trim_balance_presence & 0x8 != 0 {
                        let sign = if reader.get()? { 1.0 } else { -1.0 };
                        t.bal3d_y_tb = Some((reader.get_n::<u8>(4)? + 1) as f64 / 16.0 * sign);
                    }

                    if trim_balance_presence & 0x10 != 0 {
                        let sign = if reader.get()? { 1.0 } else { -1.0 };
                        t.bal3d_y_lis = Some((reader.get_n::<u8>(4)? + 1) as f64 / 16.0 * sign);
                    }
                }

                *trim = Some(t)
            }
        }

        element.b_disable_trim_per_obj = reader.get()?;
        if element.b_disable_trim_per_obj {
            for _ in 0..state.object_count {
                element.b_disable_trim.push(reader.get()?);
            }
        }

        Ok(element)
    }
}

#[derive(Clone, Debug, Default)]
#[repr(C)]
pub struct ExtendedObjectElement {
    pub b_obj_div_block: bool,
    pub object_div_block: Vec<Vec<ObjectDivergenceBlock>>,

    pub b_ext_prec_pos_block: bool,
    pub ext_prec_pos_block: Vec<Vec<ExtendedPrecisionPositionBlock>>,
}

impl ExtendedObjectElement {
    fn read(state: &mut OAMDParserState, reader: &mut BsIoSliceReader) -> Result<Self> {
        let mut element = Self {
            b_obj_div_block: reader.get()?,
            ..Default::default()
        };

        let object_count = state.object_count;
        let Some(object_element) = &state.object_element else {
            return Ok(element);
        };

        let block_count = object_element.md_update_info.num_obj_info_blocks;

        if element.b_obj_div_block {
            let mut obj_blk = Vec::with_capacity(object_count);

            for object_index in 0..object_count {
                let mut blk_blk = Vec::with_capacity(block_count);

                let object_data = &object_element.object_data[object_index];
                for (block_index, object_info_block) in object_data.iter().enumerate() {
                    let mut blk = ObjectDivergenceBlock::default();

                    if !object_info_block.b_object_not_active
                        && !object_info_block.b_object_in_bed_or_isf
                    {
                        blk.b_object_divergence = reader.get()?;

                        if blk.b_object_divergence {
                            // object_div_mode
                            match reader.get_n::<u8>(2)? {
                                0 => {
                                    blk.object_divergence =
                                        // object_div_table
                                        ObjectDivergenceBlock::OBJECT_DIV_TABLE_TABLE
                                            [reader.get_n::<u8>(2)? as usize];
                                }
                                1 => {
                                    if let Some(prev_blk) = blk_blk.last().cloned() {
                                        blk = prev_blk;
                                    } else {
                                        warn!(
                                            "No previous block for object {object_index} block {block_index}"
                                        );
                                    }
                                }
                                _ => {
                                    blk.object_div_code = reader.get_n(6)?;
                                    if let Some(div) = ObjectDivergenceBlock::OBJECT_DIV_CODE_TABLE
                                        [blk.object_div_code as usize]
                                    {
                                        blk.object_divergence = div;
                                    } else {
                                        warn!(
                                            "Invalid object_div_code for object {object_index} block {block_index}"
                                        )
                                    }
                                }
                            }
                        }
                    }

                    blk_blk.push(blk);
                }

                obj_blk.push(blk_blk);
            }

            element.object_div_block = obj_blk;
        }

        element.b_ext_prec_pos_block = reader.get()?;

        if element.b_ext_prec_pos_block {
            let mut pos_blk = Vec::with_capacity(object_count);

            for object_index in 0..object_count {
                let mut blk_blk = Vec::with_capacity(block_count);

                let object_data = &object_element.object_data[object_index]; // has length == block_count
                for object_info_block in object_data.iter() {
                    let mut blk = ExtendedPrecisionPositionBlock::default();

                    if !object_info_block.b_object_not_active {
                        // b_ext_prec_pos
                        if !object_info_block.b_object_in_bed_or_isf && reader.get()? {
                            let ext_prec_pos_presence = reader.get_n::<u8>(3)?;

                            if ext_prec_pos_presence & 1 != 0 {
                                blk.ext_prec_pos3d_x =
                                    ExtendedPrecisionPositionBlock::EXT_PREC_POS3D_LUT
                                        [reader.get_n::<u8>(2)? as usize]
                                        / 310.0;
                            }

                            if ext_prec_pos_presence & 2 != 0 {
                                blk.ext_prec_pos3d_y =
                                    ExtendedPrecisionPositionBlock::EXT_PREC_POS3D_LUT
                                        [reader.get_n::<u8>(2)? as usize]
                                        / 310.0;
                            }

                            if ext_prec_pos_presence & 4 != 0 {
                                blk.ext_prec_pos3d_z =
                                    ExtendedPrecisionPositionBlock::EXT_PREC_POS3D_LUT
                                        [reader.get_n::<u8>(2)? as usize]
                                        / 75.0;
                            }
                        }
                    }

                    blk_blk.push(blk);
                }

                pos_blk.push(blk_blk);
            }

            element.ext_prec_pos_block = pos_blk;
        }

        Ok(element)
    }
}

#[derive(Clone, Debug, Default)]
#[repr(C)]
pub struct ObjectDivergenceBlock {
    pub object_divergence: f64,
    pub b_object_divergence: bool,
    pub object_div_mode: u8,
    pub object_div_table: u8,
    pub object_div_code: u8,
}

impl ObjectDivergenceBlock {
    pub const OBJECT_DIV_TABLE_TABLE: [f64; 4] = [0.500755, 0.608529, 0.704833, 1.0]; // 26, 29, 32, 63

    pub const OBJECT_DIV_CODE_TABLE: [Option<f64>; 64] = [
        None,           // 0: reserved
        Some(0.0),      // 1
        Some(0.004026), // 2
        Some(0.00716),  // 3
        Some(0.012731), // 4
        Some(0.020173), // 5
        Some(0.028485), // 6
        Some(0.04021),  // 7
        Some(0.050582), // 8
        Some(0.063601), // 9
        Some(0.079914), // 10
        Some(0.100299), // 11
        Some(0.125666), // 12
        Some(0.140532), // 13
        Some(0.157027), // 14
        Some(0.175282), // 15
        Some(0.195417), // 16
        Some(0.217536), // 17
        Some(0.241718), // 18
        Some(0.268002), // 19
        Some(0.296377), // 20
        Some(0.326766), // 21
        Some(0.359017), // 22
        Some(0.392895), // 23
        Some(0.428081), // 24
        Some(0.464184), // 25
        Some(0.500755), // 26
        Some(0.537316), // 27
        Some(0.573389), // 28
        Some(0.608529), // 29
        Some(0.642346), // 30
        Some(0.674524), // 31
        Some(0.704833), // 32
        Some(0.733123), // 33
        Some(0.75932),  // 34
        Some(0.783416), // 35
        Some(0.805451), // 36
        Some(0.825506), // 37
        Some(0.843686), // 38
        Some(0.860112), // 39
        Some(0.874914), // 40
        Some(0.888222), // 41
        Some(0.900168), // 42
        Some(0.910875), // 43
        Some(0.920461), // 44
        Some(0.929035), // 45
        Some(0.936698), // 46
        Some(0.943544), // 47
        Some(0.949656), // 48
        Some(0.955112), // 49
        Some(0.95998),  // 50
        Some(0.964322), // 51
        Some(0.968195), // 52
        Some(0.974729), // 53
        Some(0.979923), // 54
        Some(0.98405),  // 55
        Some(0.98733),  // 56
        Some(0.989935), // 57
        Some(0.992874), // 58
        Some(0.994955), // 59
        Some(0.996817), // 60
        Some(0.99821),  // 61
        Some(0.998993), // 62
        Some(1.0),      // 63
    ];
}

#[derive(Clone, Debug, Default)]
#[repr(C)]
pub struct ExtendedPrecisionPositionBlock {
    pub ext_prec_pos3d_x: f64,
    pub ext_prec_pos3d_y: f64,
    pub ext_prec_pos3d_z: f64,
}

impl ExtendedPrecisionPositionBlock {
    pub const EXT_PREC_POS3D_LUT: [f64; 4] = [1.0, 2.0, -1.0, -2.0];
}

#[allow(clippy::upper_case_acronyms)]
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum SpeakerLabels {
    L,
    R,
    C,
    LFE,
    Lss,
    Rss,
    Lrs,
    Rrs,
    Lfh,
    Rfh,
    Lts,
    Rts,
    Lrh,
    Rrh,
    Lw,
    Rw,
    LFE2,
}

impl SpeakerLabels {
    const W: f32 = 42.0 / 62.0;

    #[rustfmt::skip]
    const POSITIONS: [[f32; 3]; 17] = [
        [-1.0,      1.0,      0.0],
        [ 1.0,      1.0,      0.0],
        [ 0.0,      1.0,      0.0],
        [-1.0,      1.0,     -1.0],
        [-1.0,      0.0,      0.0],
        [ 1.0,      0.0,      0.0],
        [-1.0,     -1.0,      0.0],
        [ 1.0,     -1.0,      0.0],
        [-1.0,      1.0,      1.0],
        [ 1.0,      1.0,      1.0],
        [-1.0,      0.0,      1.0],
        [ 1.0,      0.0,      1.0],
        [-1.0,     -1.0,      1.0],
        [ 1.0,     -1.0,      1.0],
        [-1.0,  Self::W,      0.0],
        [ 1.0,  Self::W,      0.0],
        [ 1.0,      1.0,     -1.0],
    ];

    pub fn from_u8(n: u8) -> Option<Self> {
        if n <= SpeakerLabels::LFE2 as u8 {
            Some(unsafe { transmute::<u8, Self>(n) })
        } else {
            None
        }
    }

    pub fn pos(&self) -> &[f32; 3] {
        &Self::POSITIONS[*self as usize]
    }
}

pub const TEST_DATA: &[u8] = &[
    0x1F, 0x88, 0x4B, 0x80, 0x00, 0xA2, 0x70, 0x00, 0x80, 0x40, 0xE0, 0x01, 0x00, 0x81, 0xC0, 0x02,
    0x01, 0x03, 0x80, 0x04, 0x02, 0x07, 0x00, 0x08, 0x04, 0x0E, 0x00, 0x10, 0x08, 0x1C, 0x00, 0x20,
    0x10, 0x38, 0x00, 0x40, 0x20, 0x70, 0x00, 0x80, 0x40, 0xE0, 0x01, 0x00, 0x81, 0xC0, 0x02, 0x01,
    0x03, 0x80, 0x04, 0x02, 0x07, 0x00, 0x08, 0x04, 0x0E, 0x00, 0x10, 0x08, 0x1C, 0x00, 0x20, 0x10,
    0x02, 0x40, 0x24, 0x33, 0x33, 0xF8, 0x00,
];

pub const TEST_DATA_TRIM: &[u8] = &[
    0x1F, 0x88, 0x4B, 0x80, 0x00, 0xA2, 0x70, 0x00, 0x80, 0x40, 0xE4, 0x0B, 0x40, 0x81, 0xDF, 0x02,
    0x01, 0x03, 0x80, 0xFC, 0x02, 0x07, 0xD4, 0x5A, 0x04, 0x0F, 0xF0, 0x10, 0x08, 0x1C, 0x0F, 0xA0,
    0x10, 0x38, 0x00, 0x7C, 0x20, 0x7F, 0x9F, 0x80, 0x40, 0xFF, 0x7D, 0x00, 0x81, 0xFE, 0x03, 0xE1,
    0x03, 0x81, 0xF7, 0xC2, 0x07, 0xFB, 0xEF, 0x84, 0x0E, 0x00, 0x10, 0x08, 0x1C, 0x00, 0x20, 0x10,
    0x02, 0xB2, 0x20, 0xCC, 0xE6, 0xAB, 0xEF, 0x0C, 0xED, 0x0D, 0x29, 0x86, 0x85, 0x80,
];

pub const TEST_DATA_BROKEN: &[u8] = &[
    0x1F, 0x88, 0x4B, 0x80, 0x00, 0xA2, 0x70, 0x00, 0x80, 0x40, 0xE4, 0x0B, 0x40, 0x81, 0xDF, 0x02,
    0x01, 0x03, 0x80, 0xFC, 0x02, 0x07, 0xD4, 0x5A, 0x04, 0x0F, 0xF0, 0x10, 0x08, 0x1C, 0x0F, 0xA0,
    0x10, 0x38, 0x00, 0x7C, 0x20, 0x7F, 0x9F, 0x80, 0x40, 0xFF, 0x7D, 0x00, 0x81, 0xFE, 0x03, 0xE1,
    0x03, 0x81, 0xF7, 0xC2, 0x07, 0xFB, 0xEF, 0x84, 0x0E, 0x00, 0x10, 0x08, 0x1C, 0x00, 0x20, 0x10,
    0x02, 0x30, 0x20, 0xCC, 0xFF, 0xE0,
];

#[cfg(test)]
mod tests {
    use crate::structs::oamd::{
        ObjectAudioMetadataPayload, TEST_DATA, TEST_DATA_BROKEN, TEST_DATA_TRIM,
    };
    use anyhow::Result;

    #[test]
    fn test1() -> Result<()> {
        let _ = ObjectAudioMetadataPayload::read(TEST_DATA)?;
        Ok(())
    }

    #[test]
    fn trim() -> Result<()> {
        let _ = ObjectAudioMetadataPayload::read(TEST_DATA_TRIM)?;

        Ok(())
    }

    #[test]
    fn broken() -> Result<()> {
        let _ = ObjectAudioMetadataPayload::read(TEST_DATA_BROKEN)?;

        Ok(())
    }
}
