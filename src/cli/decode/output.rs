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

    /// Wave64 here carries no channel mask, so it makes no claim about the order and
    /// the samples are written in the decoder's own.
    pub fn create_w64(path: PathBuf, sample_rate: u32, channel_count: u32) -> anyhow::Result<Self> {
        let mut w64_writer = WAVWriter::new(File::create(path)?);
        w64_writer.configure_audio_format(sample_rate, channel_count, 24)?;
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
}
