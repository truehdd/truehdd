use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::Display;
use std::path::Path;
use truehd::structs::oamd::{ObjectAudioMetadataPayload, SpeakerLabels, Trim};

pub const DAMF_VERSION: &str = "0.5.1";

#[derive(Deserialize, Serialize)]
pub struct Data {
    version: String,
    presentations: Vec<Presentation>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Presentation {
    #[serde(rename = "type")]
    presentation_type: PresentationType,
    simplified: bool,
    metadata: String,
    audio: String,
    offset: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ffoa: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fps: Option<Fps>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sc_number_of_elements: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sc_bed_configuration: Option<VecDisplay<u32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    creation_tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    creation_tool_version: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "downmixType_5to2"
    )]
    downmix_type_5to2: Option<DownmixMode>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "51-to-20_LsRs90degPhaseShift"
    )]
    ls_rs_90_deg_phase_shift: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    warp_mode: Option<WarpMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    trim_mode: Option<TrimMode>,
    #[serde(default)]
    bed_instances: Vec<BedInstance>,
    #[serde(default)]
    objects: Vec<Object>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum PresentationType {
    Home,
    Cinema,
}

#[derive(Debug, Deserialize, Serialize)]
enum Fps {
    #[serde(rename = "23.976")]
    R23_976,
    #[serde(rename = "24")]
    R24,
    #[serde(rename = "25")]
    R25,
    #[serde(rename = "29.97")]
    R29_97,
    #[serde(rename = "29.97df")]
    R29_97df,
    #[serde(rename = "30")]
    R30,
}

#[derive(Debug, Deserialize, Serialize)]
enum DownmixMode {
    #[serde(rename = "LoRo_Stereo")]
    LoRoStereo,
    #[serde(rename = "LtRt_ProLogic")]
    LtRtProLogic,
    #[serde(rename = "LtRt_PLII")]
    LtRtPLII,
}

#[derive(Debug, Deserialize, Serialize, Default)]
enum WarpMode {
    #[default]
    #[serde(rename = "normal")]
    Normal,
    #[serde(rename = "warping")]
    Warping,
    ProLogicIIx,
    LoRo,
}

impl WarpMode {
    fn from_oamd_u8(warp_mode: u8) -> Self {
        match warp_mode {
            1 => Self::Warping,
            2 => Self::ProLogicIIx,
            3 => Self::LoRo,
            _ => Self::Normal,
        }
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
struct TrimMode {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    no_surrounds_no_heights: Option<TrimOptions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    some_surrounds_no_heights: Option<TrimOptions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    many_surrounds_no_heights: Option<TrimOptions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    no_surrounds_some_heights: Option<TrimOptions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    some_surrounds_some_heights: Option<TrimOptions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    many_surrounds_some_heights: Option<TrimOptions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    no_surrounds_many_heights: Option<TrimOptions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    some_surrounds_many_heights: Option<TrimOptions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    many_surrounds_many_heights: Option<TrimOptions>,
}

impl TrimMode {
    pub fn try_from_oamd(oamd: &ObjectAudioMetadataPayload) -> Option<Self> {
        let Some(trim) = &oamd.trim_element else {
            return None;
        };

        Some(Self {
            no_surrounds_no_heights: TrimOptions::try_from_oamd_trim(&trim.trims[0]),
            some_surrounds_no_heights: TrimOptions::try_from_oamd_trim(&trim.trims[1]),
            many_surrounds_no_heights: TrimOptions::try_from_oamd_trim(&trim.trims[2]),
            no_surrounds_some_heights: TrimOptions::try_from_oamd_trim(&trim.trims[3]),
            some_surrounds_some_heights: TrimOptions::try_from_oamd_trim(&trim.trims[4]),
            many_surrounds_some_heights: TrimOptions::try_from_oamd_trim(&trim.trims[5]),
            no_surrounds_many_heights: TrimOptions::try_from_oamd_trim(&trim.trims[6]),
            some_surrounds_many_heights: TrimOptions::try_from_oamd_trim(&trim.trims[7]),
            many_surrounds_many_heights: TrimOptions::try_from_oamd_trim(&trim.trims[8]),
        })
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct TrimOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    center_trim: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    surround_trim: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    height_trim: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    front_back_balance_overhead_floor: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    front_back_balance_listener: Option<f64>,
}

impl TrimOptions {
    pub fn try_from_oamd_trim(trim: &Option<Trim>) -> Option<Self> {
        let Some(trim) = trim else {
            return None;
        };

        Some(Self {
            center_trim: trim.trim_centre,
            surround_trim: trim.trim_surround,
            height_trim: trim.trim_height,
            front_back_balance_overhead_floor: trim.bal3d_y_tb,
            front_back_balance_listener: trim.bal3d_y_lis,
        })
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct Channel {
    channel: String,
    #[serde(rename = "ID")]
    id: u32,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Object {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    group_name: Option<String>,
    #[serde(rename = "ID")]
    id: u32,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BedInstance {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    group_name: Option<String>,
    channels: Vec<Channel>,
}

impl Data {
    pub fn serialize_damf(&self) -> String {
        format_yaml_string(serde_yaml_ng::to_string(self).unwrap())
    }
    pub fn with_oamd_payload(oamd: &ObjectAudioMetadataPayload, base_path: &Path) -> Self {
        let presentation_type = PresentationType::Home;

        let base_name = base_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap();

        // TODO: move elsewhere?
        let bed_instances = oamd
            .program_assignment
            .bed_assignment
            .iter()
            .map(|bed| BedInstance {
                description: None,
                group_name: None,
                channels: bed
                    .to_index_vec()
                    .iter()
                    .map(|&i| {
                        Channel {
                            channel: format!("{:?}", SpeakerLabels::from_u8(i as u8).unwrap()),
                            id: if i < 10 {
                                i
                            } else {
                                i + 128 // unusual bed objects are assigned to the end
                            } as u32,
                        }
                    })
                    .collect(),
            })
            .collect();

        let objects = (0..oamd.program_assignment.num_dynamic_objects)
            .map(|i| Object {
                description: None,
                group_name: None,
                id: i as u32 + 10,
            })
            .collect();

        let warp_mode = oamd
            .trim_element
            .as_ref()
            .map(|trim| WarpMode::from_oamd_u8(trim.warp_mode));
        let trim_mode = TrimMode::try_from_oamd(oamd);

        let sc_bed_configuration = oamd
            .program_assignment
            .bed_assignment
            .first()
            .map(|bed| VecDisplay(bed.to_index_vec().iter().map(|i| *i as u32).collect()));

        Self {
            version: DAMF_VERSION.to_string(),
            presentations: vec![Presentation {
                presentation_type,
                simplified: false,
                metadata: format!("{base_name}.atmos.metadata"),
                audio: format!("{base_name}.atmos.audio"),
                offset: 0.0,
                ffoa: None,
                fps: Some(Fps::R24), // TODO: derive
                sc_number_of_elements: None,
                sc_bed_configuration,
                creation_tool: Some(env!("CARGO_PKG_NAME").to_string()),
                creation_tool_version: Some(env!("CARGO_PKG_VERSION").to_string()),
                downmix_type_5to2: None,
                ls_rs_90_deg_phase_shift: None,
                warp_mode,
                trim_mode,
                bed_instances,
                objects,
            }],
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Configuration {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    pub events: Vec<Event>,
}

impl Configuration {
    pub fn serialize_events(&mut self, remove_header: bool) -> String {
        if remove_header {
            self.events.retain(|e| e != &Event::default());
            if self.events.is_empty() {
                return String::new();
            }
            self.sample_rate = None;
        }

        let mut yaml_str = serde_yaml_ng::to_string(&self).unwrap();
        let result = if remove_header {
            yaml_str.split_off(8) // remove 'events:\n'
        } else {
            yaml_str
        };

        format_yaml_string(result)
    }

    pub fn with_oamd_payload(
        oamd: &ObjectAudioMetadataPayload,
        sample_rate: u32,
        sample_pos: u64,
    ) -> Self {
        let object_count = oamd.object_count;
        let Some(object_element) = &oamd.object_element else {
            return Self {
                sample_rate: Some(sample_rate),
                events: vec![],
            };
        };

        let pos_vec = oamd.get_damf_pos();

        let trim_bypass_vec = if let Some(trim) = &oamd.trim_element {
            if trim.b_disable_trim_per_obj {
                trim.b_disable_trim.clone()
            } else if trim.global_trim_mode == 1 {
                vec![true; object_count]
            } else {
                vec![false; object_count]
            }
        } else {
            vec![false; object_count]
        };

        // TODO: implement
        assert_eq!(
            object_element.md_update_info.num_obj_info_blocks, 1,
            "Found multiple update blocks, please submit a sample"
        );

        assert_eq!(
            oamd.program_assignment.bed_assignment.len(),
            1,
            "Found multiple bed instances, please submit a sample"
        );

        assert_eq!(
            oamd.program_assignment.num_isf_objects, 0,
            "Found ISF objects, please submit a sample"
        );

        let sample_offset = object_element.md_update_info.sample_offset as u64;
        let ramp_duration =
            object_element.md_update_info.block_update_info[0].ramp_duration as usize;

        let sample_pos = sample_pos + sample_offset + oamd.evo_sample_offset;

        let mut events = Vec::with_capacity(object_count);

        let bed_index_vec = if let Some(bed) = oamd.program_assignment.bed_assignment.first() {
            bed.to_index_vec()
        } else {
            Vec::new()
        };

        for i in 0..object_count {
            let object_data = &object_element.object_data[i][0];
            let id = if object_data.b_object_in_bed_or_isf {
                let index = bed_index_vec[i];

                // assign unusual bed objects to the end
                if index >= 10 { index + 128 } else { index }
            } else {
                i + 10 - bed_index_vec.len()
            };

            let mut event: Event = Event::with_id(id as u32);
            event.active = Some(!object_data.b_object_not_active);
            event.sample_pos = Some(sample_pos);

            let basic = &object_data.object_basic_info;

            event.importance = Some(basic.object_priority);
            event.gain = Some(basic.gain_string());
            event.ramp_length = Some(ramp_duration as u32);

            if !object_data.b_object_in_bed_or_isf {
                let render = &object_data.object_render_info;

                event.elevation = Some(render.b_enable_elevation);
                event.snap = Some(render.b_object_snap);
                event.pos = Some(VecDisplay(pos_vec[i][0].to_vec()));
                event.zones = Some(Zones::from_u8(render.zone_constraints_idx));

                // size3D does not exist in Atmos
                event.size = Some(render.object_size[0]);

                event.screen_factor = Some(render.screen_factor);
                event.depth_factor = Some(render.depth_factor);

                // Unimplemented, seems in ObjectDescriptionElement
                event.dialog = Some(-1);
                event.music = Some(-1);

                event.binaural_render_mode = Some("undefined".to_string());
            } else {
                event.binaural_render_mode = Some("off".to_string());
            }

            event.trim_bypass = Some(trim_bypass_vec[i]);

            // Unimplemented
            event.head_track_mode = Some("undefined".to_string());

            events.push(event);
        }

        Self {
            sample_rate: Some(sample_rate),
            events,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[repr(u8)]
pub enum Zones {
    #[default]
    #[serde(rename = "all")]
    All,
    #[serde(rename = "no back")]
    NoBack,
    #[serde(rename = "no sides")]
    NoSides,
    #[serde(rename = "center back")]
    CenterBack,
    #[serde(rename = "screen only")]
    ScreenOnly,
    #[serde(rename = "surround only")]
    SurroundOnly,
}

impl Zones {
    fn from_u8(value: u8) -> Self {
        match value {
            0 => Zones::All,
            1 => Zones::NoBack,
            2 => Zones::NoSides,
            3 => Zones::CenterBack,
            4 => Zones::ScreenOnly,
            5 => Zones::SurroundOnly,
            _ => Zones::All, // Default case
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "ID")]
    id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sample_pos: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    active: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pos: Option<VecDisplay<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    snap: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    elevation: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    zones: Option<Zones>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    size: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "size3D")]
    size_3d: Option<VecDisplay<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    decorr: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    importance: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    gain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ramp_length: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    trim_bypass: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dialog: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    music: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    screen_factor: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    depth_factor: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    head_track_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    binaural_render_mode: Option<String>,
}

impl Event {
    pub fn with_id(id: u32) -> Self {
        Self {
            id: Some(id),
            ..Default::default()
        }
    }

    fn diff(&self, b: &Self) -> Self {
        let mut out = Self::default();

        macro_rules! diff {
            ($($f:ident),* $(,)?) => {
                $(
                    if self.$f != b.$f {
                        out.$f = b.$f.clone();
                    } else {
                        out.$f = None;
                    }
                )*
            };
        }

        diff!(
            active,
            pos,
            snap,
            elevation,
            zones,
            size,
            size_3d,
            decorr,
            importance,
            gain,
            ramp_length,
            trim_bypass,
            dialog,
            music,
            screen_factor,
            depth_factor,
            head_track_mode,
            binaural_render_mode
        );

        if out != Self::default() {
            out.id = b.id;
            out.sample_pos = b.sample_pos;
        }

        out
    }

    pub fn compare_event_vectors(events1: &[Event], events2: &[Event]) -> Vec<Event> {
        events1
            .iter()
            .zip(events2)
            .map(|(event1, event2)| event1.diff(event2))
            .collect()
    }
}

#[derive(Default, Debug, Clone, PartialEq)]
struct VecDisplay<T>(Vec<T>);

impl<T> Serialize for VecDisplay<T>
where
    T: Display,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use std::fmt::Write;

        let mut result = String::with_capacity(self.0.len() * 8 + 2); // Pre-allocate
        result.push('[');

        for (i, item) in self.0.iter().enumerate() {
            if i > 0 {
                result.push_str(", ");
            }
            write!(result, "{item}").unwrap();
        }
        result.push(']');

        serializer.collect_str(&result)
    }
}

impl<'de, T> Deserialize<'de> for VecDisplay<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let vec: Vec<T> = Vec::deserialize(deserializer)?;
        Ok(VecDisplay(vec))
    }
}

/// Helper function for common YAML string formatting
fn format_yaml_string(mut yaml_str: String) -> String {
    yaml_str.retain(|c| c != '\'');
    yaml_str.replace("  ", "    ").replace("- ", "  - ")
}

#[test]
fn roundtrip() {
    let yaml_data = format!(
        r#"version: {DAMF_VERSION}
presentations:
  - type: home
    simplified: false
    metadata: NaturesFury.atmos.metadata
    audio: NaturesFury.atmos.audio
    offset: 3598.0
    ffoa: 3600.0
    fps: 24
    scNumberOfElements: 14
    scBedConfiguration: [3]
    creationTool: Dolby Atmos Conversion Tool
    creationToolVersion: 2.1.0
    downmixType_5to2: LoRo_Stereo
    51-to-20_LsRs90degPhaseShift: false
    warpMode: LoRo
    trimMode:
        NoSurroundsNoHeights:
            surroundTrim: -0.75
        SomeSurroundsNoHeights:
            surroundTrim: -1.5
        ManySurroundsNoHeights:
            surroundTrim: -3.0
        NoSurroundsSomeHeights:
            surroundTrim: -4.5
        SomeSurroundsSomeHeights:
            surroundTrim: -6.0
        ManySurroundsSomeHeights:
            surroundTrim: -7.5
        NoSurroundsManyHeights:
            surroundTrim: -9.0
        SomeSurroundsManyHeights:
            surroundTrim: -10.5
        ManySurroundsManyHeights:
            surroundTrim: -12.0
    bedInstances:
      - description: Composite Bed
        groupName: Comp Bed
        channels:
          - channel: L
            ID: 0
          - channel: R
            ID: 1
          - channel: C
            ID: 2
          - channel: LFE
            ID: 3
          - channel: Lss
            ID: 4
          - channel: Rss
            ID: 5
          - channel: Lrs
            ID: 6
          - channel: Rrs
            ID: 7
          - channel: Lts
            ID: 8
          - channel: Rts
            ID: 9
    objects:
      - groupName: Dialog
        ID: 10
      - description: Dialog Object 2
        groupName: Dialog
        ID: 11
      - description: Dialog Object 3
        groupName: Dialog
        ID: 12
"#
    );

    let data: Data = serde_yaml_ng::from_str(&yaml_data).unwrap();
    let string = format_yaml_string(serde_yaml_ng::to_string(&data).unwrap());

    assert_eq!(yaml_data, string);
}

#[test]
fn damf() {
    use truehd::structs::oamd::TEST_DATA_TRIM;

    let test_str = format!(
        r#"version: {DAMF_VERSION}
presentations:
  - type: home
    simplified: false
    metadata: test.atmos.metadata
    audio: test.atmos.audio
    offset: 0.0
    fps: 24
    scBedConfiguration: [3]
    creationTool: truehdd
    creationToolVersion: {}
    warpMode: ProLogicIIx
    trimMode:
        NoSurroundsNoHeights:
            surroundTrim: -3.0
            heightTrim: -4.5
        SomeSurroundsNoHeights:
            surroundTrim: -9.0
            frontBackBalanceOverheadFloor: 1.0
            frontBackBalanceListener: -1.0
        ManySurroundsNoHeights:
            surroundTrim: -4.5
            heightTrim: -3.0
        SomeSurroundsSomeHeights:
            surroundTrim: -7.5
            heightTrim: -0.75
        SomeSurroundsManyHeights:
            surroundTrim: -6.0
            heightTrim: -1.5
    bedInstances:
      - channels:
          - channel: LFE
            ID: 3
    objects:
      - ID: 10
      - ID: 11
      - ID: 12
      - ID: 13
      - ID: 14
      - ID: 15
      - ID: 16
      - ID: 17
      - ID: 18
      - ID: 19
      - ID: 20
      - ID: 21
      - ID: 22
      - ID: 23
      - ID: 24
"#,
        env!("CARGO_PKG_VERSION")
    );

    let oamd = ObjectAudioMetadataPayload::read(TEST_DATA_TRIM).unwrap();
    let data = Data::with_oamd_payload(&oamd, Path::new("test"));
    let yaml_str = serde_yaml_ng::to_string(&data).unwrap();

    assert_eq!(test_str, format_yaml_string(yaml_str));
}
