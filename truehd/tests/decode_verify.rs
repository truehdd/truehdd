//! Decode-correctness tests over encoded TrueHD/MLP streams.
//!
//! The fixtures are slices of encoder output, cut at major sync
//! boundaries so they decode standalone. Every digest labelled
//! "source-derived" was computed from the original input WAV files, not from
//! this decoder, so these tests fail whenever decoding stops being the
//! encoder's lossless inverse.

use truehd::process::MAX_PRESENTATIONS;
use truehd::process::decode::Decoder;
use truehd::process::extract::Extractor;
use truehd::process::parse::Parser;
use truehd::structs::channel::ChannelLabel;
use truehd::utils::bitstream_io::BsIoSliceReader;
use truehd::utils::crc::{CRC_RESTART_BLOCK_HEADER_ALG, Crc8};

/// Access units 384..536 of an FBA encode with one substream carrying a
/// two-channel presentation (presentations 1 and 2 are copies of it). The slice
/// starts at a major sync and spans two restart headers, so the second restart
/// compares the running lossless check. 152 access units, 6080 samples at 48 kHz.
const FBA_2CH_SLICE: &[u8] = include_bytes!("assets/fba_2ch.mlp");

/// Access units 2944..3080 of an FBA Atmos encode with four substreams and
/// four independent presentations (2/6/8/16 channels). The slice starts at a
/// major sync and spans two restart headers per substream. 136 access units,
/// 5440 samples at 48 kHz.
const FBA_ATMOS_CBI_SLICE: &[u8] = include_bytes!("assets/fba_atmos_cbi.mlp");

/// Access units 512..600 of an FBB (MLP) encode with two substreams: a
/// six-channel independent presentation and its two-channel encoder downmix.
/// 88 access units, 3520 samples at 48 kHz.
const FBB_6CH_SLICE: &[u8] = include_bytes!("assets/fbb_6ch.mlp");

/// Access units 512..600 of an FBB (MLP) encode with one substream: a
/// two-channel independent presentation whose six- and eight-channel
/// presentations are copies of it (substream_info 0x04). Its channel_meaning
/// sets the bit FBA uses as extra_channel_meaning_present, so it regresses the
/// FBB major sync handling. 88 access units, 3520 samples at 48 kHz.
const FBB_COPY_SLICE: &[u8] = include_bytes!("assets/fbb_copy.mlp");

/// FNV-1a 64 digest of decoded PCM: every valid sample of every decoded
/// access unit, sample-major, each 24-bit value as an i32 in little-endian
/// byte order. Matches the digests computed from the source WAVs.
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fnv1a(hash: &mut u64, bytes: &[u8]) {
    for &b in bytes {
        *hash = (*hash ^ b as u64).wrapping_mul(FNV_PRIME);
    }
}

#[derive(Debug)]
struct PresentationSummary {
    digest: u64,
    samples: usize,
    channels: usize,
    sampling_frequency: u32,
    channel_labels: Vec<ChannelLabel>,
}

/// Extract, parse and decode a stream, digesting each requested
/// presentation. `fail_level` is applied to both the parser and the decoder
/// (None keeps the default of failing on errors only).
fn decode_stream(
    data: &[u8],
    required: [bool; MAX_PRESENTATIONS],
    fail_level: Option<log::Level>,
) -> anyhow::Result<[Option<PresentationSummary>; MAX_PRESENTATIONS]> {
    let mut extractor = Extractor::default();
    extractor.push_bytes(data);

    let mut parser = Parser::default();
    let mut decoder = Decoder::default();
    if let Some(level) = fail_level {
        parser.set_fail_level(level);
        decoder.set_fail_level(level);
    }

    let mut out: [Option<PresentationSummary>; MAX_PRESENTATIONS] = Default::default();

    for result in &mut extractor {
        let Ok(frame) = result else { break };
        let access_unit = parser.parse(&frame)?;
        let decoded = decoder.decode_presentations(&access_unit, &required)?;

        for (i, decoded) in decoded.iter().enumerate() {
            let Some(decoded) = decoded else { continue };
            let entry = out[i].get_or_insert_with(|| PresentationSummary {
                digest: FNV_OFFSET,
                samples: 0,
                channels: decoded.channel_count,
                sampling_frequency: decoded.sampling_frequency,
                channel_labels: decoded.channel_labels.clone(),
            });
            assert_eq!(entry.channels, decoded.channel_count);
            assert_eq!(entry.sampling_frequency, decoded.sampling_frequency);
            for sample in &decoded.pcm_data[..decoded.sample_length] {
                for value in &sample[..decoded.channel_count] {
                    fnv1a(&mut entry.digest, &value.to_le_bytes());
                }
            }
            entry.samples += decoded.sample_length;
        }
    }

    Ok(out)
}

fn require(indices: &[usize]) -> [bool; MAX_PRESENTATIONS] {
    let mut required = [false; MAX_PRESENTATIONS];
    for &i in indices {
        required[i] = true;
    }
    required
}

/// Flip one payload bit of substream 0 in the given minor access unit and
/// recompute the segment's trailing parity and CRC-8 bytes, so the corruption
/// reaches the PCM path instead of being stopped by the segment checks.
fn corrupt_substream0(
    data: &[u8],
    target_au: usize,
    substreams: usize,
    payload_offset: usize,
) -> Vec<u8> {
    let mut d = data.to_vec();
    let mut offset = 0usize;
    let mut index = 0usize;

    loop {
        assert!(offset + 8 <= d.len(), "target access unit not found");
        let au_len = (((d[offset] as usize) << 8 | d[offset + 1] as usize) & 0xFFF) << 1;
        if index == target_au {
            assert!(
                !(d[offset + 4] == 0xF8 && d[offset + 5] == 0x72),
                "target must be a minor access unit"
            );
            let mut p = offset + 4;
            let mut end0 = 0usize;
            let mut crc_present = false;
            for i in 0..substreams {
                let entry = (d[p] as usize) << 8 | d[p + 1] as usize;
                p += 2;
                if entry >> 15 != 0 {
                    p += 2;
                }
                if i == 0 {
                    end0 = entry & 0xFFF;
                    crc_present = (entry >> 13) & 1 != 0;
                }
            }
            assert!(crc_present, "fixture substream 0 must carry parity + CRC");
            let seg_start = p;
            let data_end = seg_start + end0 * 2 - 2;
            let target = seg_start + payload_offset;
            assert!(target < data_end - 2);

            d[target] ^= 0x01;

            let mut parity = 0u8;
            let mut crc = 0xA2u8;
            for &b in &d[seg_start..data_end] {
                parity ^= b;
                for _ in 0..8 {
                    let hi = crc & 0x80 != 0;
                    crc <<= 1;
                    if hi {
                        crc ^= 0x63;
                    }
                }
                crc ^= b;
            }
            d[data_end] = parity ^ 0xA9;
            d[data_end + 1] = crc;
            return d;
        }
        offset += au_len;
        index += 1;
    }
}

/// Overwrites `n` bits at bit offset `bit`, most significant bit first.
fn set_bits(data: &mut [u8], bit: usize, n: usize, value: u32) {
    for i in 0..n {
        let position = bit + i;
        let mask = 1u8 << (7 - (position & 7));

        if (value >> (n - 1 - i)) & 1 == 1 {
            data[position >> 3] |= mask;
        } else {
            data[position >> 3] &= !mask;
        }
    }
}

/// Restates `max_bits` in the first access unit's restart header, leaving a stream that
/// is valid in every other respect: the restart header CRC and the segment's parity and
/// CRC-8 are recomputed, and the decoded samples are untouched, so only a check that
/// compares the outputs against the declaration can notice.
fn understate_max_bits(data: &[u8], max_bits: u32) -> Vec<u8> {
    // The restart header of substream 0's segment, which the stream places at bit 274:
    // a 32-bit access unit header, a major sync info, one directory entry, then the
    // block_header_exists and restart_header_exists flags. Its length follows from
    // max_matrix_chan, 1 here: 113 bits plus six per matrix channel.
    const RESTART_START: usize = 274;
    const RESTART_LEN: usize = 125;
    // max_shift, max_lsbs and then the two copies of max_bits, 78 bits in.
    const MAX_BITS: usize = RESTART_START + 78;

    let mut d = data.to_vec();

    set_bits(&mut d, MAX_BITS, 5, max_bits);
    set_bits(&mut d, MAX_BITS + 5, 5, max_bits);

    let crc = BsIoSliceReader::from_slice(&d.clone())
        .crc8_check(
            &Crc8::new(&CRC_RESTART_BLOCK_HEADER_ALG),
            RESTART_START as u64,
            RESTART_LEN as u64,
        )
        .unwrap();
    set_bits(&mut d, RESTART_START + RESTART_LEN, 8, crc as u32);

    // The segment starts two bits before its restart header, on a byte boundary, and its
    // directory entry is the word before that.
    let seg_start = (RESTART_START - 2) / 8;
    let entry = (d[seg_start - 2] as usize) << 8 | d[seg_start - 1] as usize;
    assert!((entry >> 13) & 1 != 0, "fixture must carry parity + CRC");
    let data_end = seg_start + (entry & 0xFFF) * 2 - 2;

    let mut parity = 0u8;
    let mut segment_crc = 0xA2u8;
    for &b in &d[seg_start..data_end] {
        parity ^= b;
        for _ in 0..8 {
            let hi = segment_crc & 0x80 != 0;
            segment_crc <<= 1;
            if hi {
                segment_crc ^= 0x63;
            }
        }
        segment_crc ^= b;
    }
    d[data_end] = parity ^ 0xA9;
    d[data_end + 1] = segment_crc;

    d
}

/// A restart header states how many bits the substream's outputs use, sign apart. One
/// that understates it has to be reported, and only the decoded samples can show it: the
/// stream is otherwise intact, and decodes to exactly the same PCM.
#[test]
fn outputs_wider_than_max_bits_are_reported() {
    let understated = understate_max_bits(FBA_2CH_SLICE, 1);

    let err = decode_stream(&understated, require(&[0]), Some(log::Level::Warn))
        .expect_err("outputs wider than max_bits must be reported");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("Outputs in substream 0 use more than max_bits 1 bits"),
        "unexpected error: {msg}"
    );

    // Nothing but the declaration changed: the same PCM comes out, and it is only the
    // check that objects.
    let out = decode_stream(&understated, require(&[0]), None).unwrap();
    let p0 = out[0].as_ref().unwrap();
    assert_eq!(p0.samples, 6080);
    assert_eq!(p0.digest, 0xC566_B961_F699_3F02, "PCM must be unchanged");
}

/// FBA, single substream: the decoded PCM must be bit-exact against the
/// encoder's source WAVs (digest is source-derived), including under a
/// warnings-are-fatal fail level, which makes every restart header's
/// lossless_check comparison a hard assertion.
#[test]
fn fba_two_channel_slice_is_lossless() {
    let out = decode_stream(FBA_2CH_SLICE, require(&[0]), Some(log::Level::Warn)).unwrap();
    let p0 = out[0].as_ref().unwrap();
    assert_eq!(p0.channels, 2);
    assert_eq!(p0.sampling_frequency, 48000);
    assert_eq!(p0.samples, 6080);
    assert_eq!(p0.channel_labels, [ChannelLabel::L, ChannelLabel::R]);
    assert_eq!(p0.digest, 0xC566_B961_F699_3F02, "PCM differs from source");
}

/// FBA Atmos, four substreams: all four presentations decoded in one pass
/// must each be bit-exact against their source WAVs (digests are
/// source-derived), with the channel order the stream declares.
#[test]
fn fba_atmos_cbi_slice_decodes_all_presentations_losslessly() {
    let out = decode_stream(
        FBA_ATMOS_CBI_SLICE,
        require(&[0, 1, 2, 3]),
        Some(log::Level::Warn),
    )
    .unwrap();

    let expected: [(usize, u64); 4] = [
        (2, 0x6F59_AF85_6227_A146),
        (6, 0xEECE_07DA_F0F4_FE05),
        (8, 0xF3D3_B4D5_489A_F57A),
        (16, 0x879B_1139_FAF1_4378),
    ];
    for (i, (channels, digest)) in expected.iter().enumerate() {
        let p = out[i].as_ref().unwrap();
        assert_eq!(p.channels, *channels, "presentation {i} channel count");
        assert_eq!(p.samples, 5440, "presentation {i} sample count");
        assert_eq!(
            p.digest, *digest,
            "presentation {i} PCM differs from source"
        );
    }

    use ChannelLabel::*;
    assert_eq!(
        out[1].as_ref().unwrap().channel_labels,
        [L, R, C, LFE, Ls, Rs]
    );
    assert_eq!(
        out[3].as_ref().unwrap().channel_labels,
        [
            L, R, C, LFE, Ls, Rs, Lb, Rb, Tfl, Tfr, Tsl, Tsr, Tbl, Tbr, Lw, Rw
        ]
    );
}

/// FBB (MLP), two substreams: the six-channel independent presentation must
/// be bit-exact against the source WAVs (digest is source-derived). The
/// two-channel presentation is an encoder-side downmix with no bit-exact
/// source; its digest was cross-checked sample-for-sample against an
/// independent TrueHD/MLP decoder (ffmpeg 8.1.2) and pins that behaviour.
#[test]
fn fbb_six_channel_slice_is_lossless() {
    let out = decode_stream(FBB_6CH_SLICE, require(&[0, 1]), None).unwrap();

    let p1 = out[1].as_ref().unwrap();
    assert_eq!(p1.channels, 6);
    assert_eq!(p1.samples, 3520);
    assert_eq!(p1.digest, 0xB56A_680B_1BB8_5DCF, "PCM differs from source");

    let p0 = out[0].as_ref().unwrap();
    assert_eq!(p0.channels, 2);
    assert_eq!(p0.samples, 3520);
    assert_eq!(p0.digest, 0x4646_6525_2D6E_4F3E, "downmix PCM changed");
}

/// FBB stream whose higher presentations are copies of the two-channel one
/// and whose channel_meaning sets the bit FBA reads as
/// extra_channel_meaning_present. The extractor used to find no frames in it
/// at all. It must extract, parse, decode bit-exact against the source WAVs,
/// and resolve a request for the copy presentation to the copied one.
#[test]
fn fbb_copy_of_two_slice_decodes() {
    let mut extractor = Extractor::default();
    extractor.push_bytes(FBB_COPY_SLICE);
    let frames = extractor.by_ref().filter(|r| r.is_ok()).count();
    assert_eq!(frames, 88, "extractor must find every access unit");

    let out = decode_stream(FBB_COPY_SLICE, require(&[0]), None).unwrap();
    let p0 = out[0].as_ref().unwrap();
    assert_eq!(p0.channels, 2);
    assert_eq!(p0.samples, 3520);
    assert_eq!(p0.digest, 0x9ABC_9B5F_237F_2DEE, "PCM differs from source");

    // Requesting presentation 1 (a copy of 0) must decode presentation 0.
    let copy = decode_stream(FBB_COPY_SLICE, require(&[1]), None).unwrap();
    assert!(copy[1].is_none());
    let resolved = copy[0].as_ref().unwrap();
    assert_eq!(resolved.digest, p0.digest);
    assert_eq!(resolved.samples, p0.samples);
}

/// The stream's own lossless_check must catch decoded PCM that no longer
/// matches what the encoder saw. The corruption flips one payload bit and
/// repairs the segment parity and CRC, so only the restart header comparison
/// can notice it.
#[test]
fn lossless_check_fires_on_corrupted_payload() {
    let bad = corrupt_substream0(FBA_2CH_SLICE, 40, 1, 20);
    let err = decode_stream(&bad, require(&[0]), Some(log::Level::Warn))
        .expect_err("corrupted PCM must fail the lossless check");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("lossless_check failed for substream 0"),
        "unexpected error: {msg}"
    );
}

/// In a multi-presentation decode the lossless_check must be compared for
/// every decoded presentation, not only the highest one: corruption confined
/// to substream 0 has to surface even though presentation 3 is also being
/// decoded.
#[test]
fn lossless_check_covers_lower_presentations() {
    let bad = corrupt_substream0(FBA_ATMOS_CBI_SLICE, 40, 4, 20);
    let err = decode_stream(&bad, require(&[0, 1, 2, 3]), Some(log::Level::Warn))
        .expect_err("corrupted presentation 0 PCM must fail the lossless check");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("lossless_check failed for substream 0"),
        "unexpected error: {msg}"
    );
}
