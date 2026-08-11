use crate::caf::{CAFWriter, ChannelLabel as CafChannelLabel};
use crate::wav::WAVWriter;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use truehd::structs::channel::ChannelLabel;

/// The CAF channel each decoded channel carries.
///
/// CAF names surrounds the way WAVE does, so the decoder's `Ls`/`Rs` (the surround
/// pair of a 5.1 bed) are CAF's `LeftSurround`/`RightSurround` ("Back Left"/"Back
/// Right"), and the decoder's `Lb`/`Rb` (the rear pair a 7.1 bed adds behind them)
/// are CAF's `RearSurroundLeft`/`RearSurroundRight`.
fn caf_channel_label(label: ChannelLabel) -> CafChannelLabel {
    match label {
        ChannelLabel::L => CafChannelLabel::Left,
        ChannelLabel::R => CafChannelLabel::Right,
        ChannelLabel::C => CafChannelLabel::Center,
        ChannelLabel::LFE => CafChannelLabel::LFEScreen,
        ChannelLabel::Ls => CafChannelLabel::LeftSurround,
        ChannelLabel::Rs => CafChannelLabel::RightSurround,
        ChannelLabel::Lb => CafChannelLabel::RearSurroundLeft,
        ChannelLabel::Rb => CafChannelLabel::RearSurroundRight,
        ChannelLabel::Cb => CafChannelLabel::CenterSurround,
        ChannelLabel::Lsc => CafChannelLabel::LeftCenter,
        ChannelLabel::Rsc => CafChannelLabel::RightCenter,
        ChannelLabel::Lsd => CafChannelLabel::LeftSurroundDirect,
        ChannelLabel::Rsd => CafChannelLabel::RightSurroundDirect,
        ChannelLabel::Lw => CafChannelLabel::LeftWide,
        ChannelLabel::Rw => CafChannelLabel::RightWide,
        ChannelLabel::Tc => CafChannelLabel::TopCenterSurround,
        ChannelLabel::Tfl => CafChannelLabel::VerticalHeightLeft,
        ChannelLabel::Tfc => CafChannelLabel::VerticalHeightCenter,
        ChannelLabel::Tfr => CafChannelLabel::VerticalHeightRight,
        ChannelLabel::Tsl => CafChannelLabel::LeftTopMiddle,
        ChannelLabel::Tsr => CafChannelLabel::RightTopMiddle,
        ChannelLabel::Tbl => CafChannelLabel::TopBackLeft,
        ChannelLabel::Tbr => CafChannelLabel::TopBackRight,
        ChannelLabel::LFE2 => CafChannelLabel::LFE2,
    }
}

/// The `dwChannelMask` bit for a label, where one exists.
const fn wave_channel_bit(label: ChannelLabel) -> Option<u32> {
    Some(match label {
        ChannelLabel::L => 0x1,       // FRONT_LEFT
        ChannelLabel::R => 0x2,       // FRONT_RIGHT
        ChannelLabel::C => 0x4,       // FRONT_CENTER
        ChannelLabel::LFE => 0x8,     // LOW_FREQUENCY
        ChannelLabel::Lb => 0x10,     // BACK_LEFT
        ChannelLabel::Rb => 0x20,     // BACK_RIGHT
        ChannelLabel::Cb => 0x100,    // BACK_CENTER
        ChannelLabel::Ls => 0x200,    // SIDE_LEFT
        ChannelLabel::Rs => 0x400,    // SIDE_RIGHT
        ChannelLabel::Tc => 0x800,    // TOP_CENTER
        ChannelLabel::Tfl => 0x1000,  // TOP_FRONT_LEFT
        ChannelLabel::Tfr => 0x4000,  // TOP_FRONT_RIGHT
        ChannelLabel::Tbl => 0x10000, // TOP_BACK_LEFT
        ChannelLabel::Tbr => 0x40000, // TOP_BACK_RIGHT
        _ => return None,
    })
}

/// The `dwChannelMask` for a decoded order, or `None` where the format cannot state it.
///
/// A mask names which speakers are present, and the extensible header then *implies* the
/// order: ascending bit order, with no way to say anything else. So a mask is written only
/// where the decoder's own order already matches that, and the samples are never reordered
/// to make one fit. A 5.1 stream carrying its surrounds before its centre, which is how
/// DVD-Audio carries one, gets no mask rather than a wrong one.
pub fn wave_channel_mask(labels: &[ChannelLabel]) -> Option<u32> {
    if labels.is_empty() {
        return None;
    }

    let mut mask = 0u32;
    let mut previous = 0u32;

    for &label in labels {
        let bit = wave_channel_bit(label)?;
        if bit <= previous {
            return None;
        }
        previous = bit;
        mask |= bit;
    }

    Some(mask)
}

pub fn caf_channel_labels(labels: &[ChannelLabel]) -> Vec<CafChannelLabel> {
    labels.iter().copied().map(caf_channel_label).collect()
}

pub enum AudioWriter {
    Pcm(BufWriter<File>),
    Caf(CAFWriter<BufWriter<File>>),
    W64(WAVWriter<File>),
}

impl AudioWriter {
    pub fn create_pcm(path: PathBuf) -> anyhow::Result<Self> {
        let pcm_writer = BufWriter::new(File::create(path)?);
        Ok(AudioWriter::Pcm(pcm_writer))
    }

    /// `channel_labels` is the order the decoder produced, one label per written
    /// channel. Anything else leaves the file without a channel layout rather than
    /// claiming an order the samples are not in.
    pub fn create_caf(
        path: PathBuf,
        sample_rate: u32,
        channel_count: u32,
        channel_labels: &[ChannelLabel],
    ) -> anyhow::Result<Self> {
        let mut caf_writer = CAFWriter::new(BufWriter::new(File::create(path)?));
        caf_writer.configure_audio_format(
            sample_rate,
            channel_count,
            24,
            &caf_channel_labels(channel_labels),
        )?;
        caf_writer.write_header()?;
        Ok(AudioWriter::Caf(caf_writer))
    }

    /// Wave64 states the layout as a channel mask where the decoded order is one a mask
    /// can describe, and says nothing otherwise. See [`wave_channel_mask`].
    pub fn create_w64(
        path: PathBuf,
        sample_rate: u32,
        channel_count: u32,
        channel_labels: &[ChannelLabel],
    ) -> anyhow::Result<Self> {
        let mut w64_writer = WAVWriter::new(File::create(path)?);
        let mask = wave_channel_mask(channel_labels)
            .filter(|_| channel_labels.len() == channel_count as usize);
        w64_writer.configure_audio_format(sample_rate, channel_count, 24, mask)?;
        w64_writer.write_header()?;
        Ok(AudioWriter::W64(w64_writer))
    }

    pub fn write_pcm_samples(
        &mut self,
        samples: &[i32],
        _channel_count: usize,
    ) -> anyhow::Result<()> {
        match self {
            AudioWriter::Pcm(pcm_writer) => {
                for &sample in samples {
                    let bytes = sample.to_le_bytes();
                    pcm_writer.write_all(&bytes[..3])?; // Write 24-bit
                }
                Ok(())
            }
            AudioWriter::Caf(caf_writer) => Ok(caf_writer.write_pcm_24bit_as_packed(samples)?),
            AudioWriter::W64(w64_writer) => Ok(w64_writer.write_pcm_24bit_as_packed(samples)?),
        }
    }

    pub fn finish(&mut self) -> anyhow::Result<()> {
        match self {
            AudioWriter::Pcm(pcm_writer) => Ok(pcm_writer.flush()?),
            AudioWriter::Caf(caf_writer) => Ok(caf_writer.finish()?),
            AudioWriter::W64(w64_writer) => Ok(w64_writer.finish()?),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caf::ChannelLayoutTag;

    fn tag_for(labels: &[ChannelLabel]) -> Option<ChannelLayoutTag> {
        ChannelLayoutTag::for_channel_labels(&caf_channel_labels(labels))
    }

    /// The 5.1 order an FBA (TrueHD) stream decodes in.
    #[test]
    fn fba_five_one_order_is_mpeg_5_1_a() {
        use ChannelLabel::*;
        assert_eq!(
            tag_for(&[L, R, C, LFE, Ls, Rs]),
            Some(ChannelLayoutTag::MPEG_5_1_A)
        );
    }

    /// The 5.1 order an FBB (DVD-Audio) stream decodes in. Same channels, different
    /// order, so it must not be given the FBA tag.
    #[test]
    fn fbb_five_one_order_is_mpeg_5_1_b() {
        use ChannelLabel::*;
        assert_eq!(
            tag_for(&[L, R, Ls, Rs, C, LFE]),
            Some(ChannelLayoutTag::MPEG_5_1_B)
        );
    }

    #[test]
    fn stereo_and_mono_orders_keep_their_tags() {
        assert_eq!(
            tag_for(&[ChannelLabel::L, ChannelLabel::R]),
            Some(ChannelLayoutTag::Stereo)
        );
        assert_eq!(tag_for(&[ChannelLabel::C]), Some(ChannelLayoutTag::Mono));
    }

    /// 7.1 here is the 5.1 bed plus a rear pair, which is MPEG_7_1_C. MPEG_7_1_A is
    /// the same count with a front centre pair instead, and would put the rears in
    /// front of the listener.
    #[test]
    fn seven_one_with_rear_surrounds_is_mpeg_7_1_c() {
        use ChannelLabel::*;
        assert_eq!(
            tag_for(&[L, R, C, LFE, Ls, Rs, Lb, Rb]),
            Some(ChannelLayoutTag::MPEG_7_1_C)
        );
    }

    /// An order no standard tag names must be spelled out rather than approximated.
    #[test]
    fn unnamed_order_is_written_as_channel_descriptions() {
        use ChannelLabel::*;
        let labels = [L, R, C, LFE, Ls, Rs, Lb, Rb, Tfl, Tfr, Tsl, Tsr];
        assert_eq!(tag_for(&labels), None);

        let layout = crate::caf::ChannelLayout::for_channel_labels(&caf_channel_labels(&labels));
        assert_eq!(
            layout.channel_layout_tag,
            ChannelLayoutTag::UseChannelDescriptions
        );
        assert_eq!(layout.number_channel_descriptions, 12);
        assert_eq!(layout.channel_descriptions.len(), 12);
        assert_eq!(
            layout.channel_descriptions[10].channel_label,
            CafChannelLabel::LeftTopMiddle
        );
    }

    /// Labels that do not account for every channel describe nothing, so no layout is
    /// written at all.
    #[test]
    fn partial_labels_write_no_layout() {
        use crate::caf::{CAFWriter, PCMDataType};
        use std::io::Cursor;

        let mut writer = CAFWriter::new(Cursor::new(Vec::new()));
        writer
            .configure_audio_format(
                48000,
                6,
                24,
                &caf_channel_labels(&[ChannelLabel::L, ChannelLabel::R]),
            )
            .unwrap();
        writer.write_header().unwrap();
        writer.finish().unwrap();

        let buffer = writer.into_inner().unwrap().into_inner();
        assert!(
            !buffer.windows(4).any(|w| w == b"chan"),
            "a partial label list must not produce a channel layout"
        );

        // The format itself is still described.
        let _ = PCMDataType::SignedInteger;
        assert!(buffer.windows(4).any(|w| w == b"desc"));
    }

    /// A mask states which speakers are there, and the order follows from the bits, so one
    /// can only be written for an order that already ascends.
    #[test]
    fn a_mask_is_written_only_for_an_order_it_can_state() {
        use ChannelLabel::*;

        assert_eq!(wave_channel_mask(&[L, R]), Some(0x3));
        assert_eq!(wave_channel_mask(&[L, R, C, LFE, Ls, Rs]), Some(0x60F));

        // DVD-Audio carries 5.1 with its surrounds before the centre, and eight channels
        // put the sides before the backs. Neither is the order a mask implies, so neither
        // gets one: the alternative is mislabelling the samples.
        assert_eq!(wave_channel_mask(&[L, R, Ls, Rs, C, LFE]), None);
        assert_eq!(wave_channel_mask(&[L, R, C, LFE, Ls, Rs, Lb, Rb]), None);

        // A channel with no mask bit of its own, and the empty case.
        assert_eq!(wave_channel_mask(&[L, R, Lw]), None);
        assert_eq!(wave_channel_mask(&[]), None);
    }
}
