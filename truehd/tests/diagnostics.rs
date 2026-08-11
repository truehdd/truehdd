//! Collect-mode diagnostics over a whole stream.

use truehd::process::extract::Extractor;
use truehd::process::parse::Parser;
use truehd::process::{EXAMPLE_DATA, MAX_PRESENTATIONS};
use truehd::structs::sync::{MAJOR_SYNC_FBA, MAJOR_SYNC_FBB};
use truehd::utils::crc::{CRC_MAJOR_SYNC_INFO_ALG, Crc16};
use truehd::utils::diagnostic::{
    AccessUnitRule, BlockRule, ChannelRule, Diagnostic, DiagnosticMode, RestartHeaderRule, RuleId,
    SubstreamRule, SyncRule,
};
use truehd::utils::errors::{RestartHeaderError, SubstreamError};

/// Access units 384..536 of an FBA encode with a single substream. Its first access unit
/// splits the 40 samples over two blocks, of 8 and 32.
const FBA_2CH: &[u8] = include_bytes!("assets/fba_2ch.mlp");

/// Access units 2944..3080 of an FBA Atmos encode with four substreams, each an
/// independent presentation, so a presentation mask can leave three of them unparsed.
/// Its sixteen elements are all bed, so every presentation maps to source channels.
const FBA_ATMOS_CBI: &[u8] = include_bytes!("assets/fba_atmos_cbi.mlp");

/// An FBB (DVD-Audio) encode with two substreams, `flags` 0x4000 and `substream_info`
/// 0x0D, so substream 1 is also to be decoded.
const FBB_6CH: &[u8] = include_bytes!("assets/fbb_6ch.mlp");

/// An FBB encode with one substream, `flags` 0x4000 and `substream_info` 0x05, so
/// substream 0 alone is decodable.
const FBB_COPY: &[u8] = include_bytes!("assets/fbb_copy.mlp");

/// Two FBA encodes spliced end to end. Its substream directories carry DRC gain updates,
/// and the second segment restates a `channel_meaning` whose `drc_start_up_gain` is louder
/// than the gain those updates left the substreams running at.
const FBA_SPLICED: &[u8] = include_bytes!("assets/fba_spliced.mlp");

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

/// The example stream repeated end to end. Every repetition restates the same input
/// timing, which the timing and buffer rules read as a stream that violates them.
fn repeated_example(times: usize) -> Vec<u8> {
    let mut data = Vec::new();

    for _ in 0..times {
        data.extend_from_slice(EXAMPLE_DATA);
    }

    data
}

#[test]
fn collect_mode_enumerates_violations_over_the_whole_stream() {
    let data = repeated_example(8);

    let mut extractor = Extractor::default();
    extractor.push_bytes(&data);

    let mut parser = Parser::default();
    parser.set_diagnostic_mode(DiagnosticMode::Collect);

    let mut access_units = 0;
    let mut frames = Vec::new();

    for frame in extractor.by_ref().flatten() {
        frames.push((frame.index, frame.offset, frame.data.len() as u64));

        if parser.parse_recovering(&frame).is_some() {
            access_units += 1;
        }
    }

    assert_eq!(frames.len(), 16);
    assert_eq!(access_units, 16);

    let diagnostics = parser.take_diagnostics();
    assert!(!diagnostics.is_empty());
    assert!(parser.diagnostics().is_empty(), "taken means taken");

    for diagnostic in &diagnostics {
        let location = diagnostic.location;
        let (index, offset, length) = frames[location.au_index as usize];

        assert_eq!(location.au_index, index);
        assert_eq!(location.au_offset, offset);
        assert!(location.byte_offset() >= offset);
        assert!(location.byte_offset() < offset + length);
        assert!(diagnostic.source.is_some());
    }

    // Violations are enumerated across the stream rather than ending it at the first.
    let first = diagnostics.first().unwrap().location.au_index;
    let last = diagnostics.last().unwrap().location.au_index;
    assert!(last > first + 4);

    // At least one check reports the bit it fired at, not just its access unit.
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.location.bit_offset.is_some())
    );
}

/// A violation that ends its access unit is recorded with the error that caused it, and
/// the stream carries on from the next major sync.
#[test]
fn a_failed_access_unit_does_not_end_the_stream() {
    let mut data = repeated_example(6);

    // A payload byte of the third access unit, past everything the extractor validates.
    data[136 + 60] ^= 0xFF;

    let mut extractor = Extractor::default();
    extractor.push_bytes(&data);

    let mut parser = Parser::default();
    parser.set_diagnostic_mode(DiagnosticMode::Collect);

    let mut parsed = Vec::new();

    for frame in extractor.by_ref().flatten() {
        if parser.parse_recovering(&frame).is_some() {
            parsed.push(frame.index);
        }
    }

    let diagnostics = parser.take_diagnostics();
    let fatal = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.severity == log::Level::Error)
        .expect("the corrupted access unit fails a check");

    assert_eq!(
        fatal.rule,
        RuleId::Substream(SubstreamRule::ParityMismatch),
        "{fatal}"
    );
    assert_eq!(fatal.location.au_index, 2);
    assert!(matches!(
        fatal
            .source
            .as_ref()
            .unwrap()
            .downcast_ref::<SubstreamError>(),
        Some(SubstreamError::ParityMismatch { substream: 0, .. })
    ));

    assert!(!parsed.contains(&2), "the corrupted access unit is dropped");
    assert!(parsed.contains(&0));
    assert!(
        parsed.last() == Some(&11),
        "parsing resumed and ran to the end: {parsed:?}"
    );
}

/// Every check that fired over `data`.
fn diagnostics_of(data: &[u8]) -> Vec<Diagnostic> {
    let mut extractor = Extractor::default();
    extractor.push_bytes(data);

    let mut parser = Parser::default();
    parser.set_diagnostic_mode(DiagnosticMode::Collect);

    for frame in extractor.by_ref().flatten() {
        parser.parse_recovering(&frame);
    }

    parser.take_diagnostics()
}

/// Every check that fired over `data`, with only the given presentations required.
fn diagnostics_of_presentations(
    data: &[u8],
    required: &[bool; MAX_PRESENTATIONS],
) -> Vec<Diagnostic> {
    let mut extractor = Extractor::default();
    extractor.push_bytes(data);

    let mut parser = Parser::default();
    parser.set_diagnostic_mode(DiagnosticMode::Collect);
    parser.set_required_presentations(required);

    for frame in extractor.by_ref().flatten() {
        parser.parse_recovering(&frame);
    }

    parser.take_diagnostics()
}

/// The blocks of a substream segment carry exactly one access unit of samples between
/// them. Shrinking one block's `block_size` leaves the segment short, which the segment's
/// own parity and CRC also notice, but neither of those says how many samples were lost.
#[test]
fn a_segment_must_decode_one_access_unit_of_samples() {
    // block_size of the second block of substream 0 in the first access unit, which the
    // stream sets to 32 to complete the 40 samples the first block's 8 leave.
    const BLOCK_SIZE_BIT: usize = 796;

    let mut data = FBA_2CH.to_vec();
    set_bits(&mut data, BLOCK_SIZE_BIT, 9, 24);

    let diagnostics = diagnostics_of(&data);
    let fired = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.rule == RuleId::Substream(SubstreamRule::SampleCountMismatch))
        .unwrap_or_else(|| panic!("a short segment violates the rule: {diagnostics:?}"));

    assert_eq!(fired.severity, log::Level::Warn, "{fired}");
    assert!(fired.location.bit_offset.is_some(), "{fired}");
    assert!(matches!(
        fired.source.as_ref().unwrap().downcast_ref::<SubstreamError>(),
        Some(SubstreamError::SampleCountMismatch {
            substream: 0,
            decoded: 32,
            expected: 40,
        })
    ));

    assert!(
        !diagnostics_of(FBA_2CH)
            .iter()
            .any(|diagnostic| diagnostic.rule
                == RuleId::Substream(SubstreamRule::SampleCountMismatch)),
        "the unmutated stream decodes a full access unit per segment"
    );
}

/// `output_timing` must agree across every substream of an access unit. A presentation
/// mask can skip a substream's segment entirely, and its restart header is then never
/// read, so the comparison has to reach into a segment it does not parse.
#[test]
fn output_timing_is_compared_across_skipped_substreams() {
    // output_timing of substream 1's restart header in the first access unit. Every
    // substream of the stream states 52224 there.
    const OUTPUT_TIMING_BIT: usize = 1296;

    let rule = RuleId::RestartHeader(RestartHeaderRule::OutputTimingMismatch);
    // Presentation 0 needs substream 0 alone, so substreams 1 to 3 are skipped.
    let presentation_0 = [true, false, false, false];

    let mut data = FBA_ATMOS_CBI.to_vec();
    set_bits(&mut data, OUTPUT_TIMING_BIT, 16, 52225);

    let diagnostics = diagnostics_of_presentations(&data, &presentation_0);
    let fired = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.rule == rule)
        .unwrap_or_else(|| panic!("a substream disagreeing violates the rule: {diagnostics:?}"));

    assert_eq!(fired.severity, log::Level::Warn, "{fired}");
    assert_eq!(fired.location.au_index, 0, "{fired}");
    assert!(matches!(
        fired
            .source
            .as_ref()
            .unwrap()
            .downcast_ref::<RestartHeaderError>(),
        Some(RestartHeaderError::OutputTimingMismatch {
            substream: 1,
            read: 52225,
            reference: 0,
            expected: 52224,
        })
    ));

    assert!(
        !diagnostics_of_presentations(FBA_ATMOS_CBI, &presentation_0)
            .iter()
            .any(|diagnostic| diagnostic.rule == rule),
        "every substream of the unmutated stream states the same output_timing"
    );
}

/// The example stream repeated, with one byte replaced. The crate carries no bitstream
/// that violates the rarer rules, so they are reached by mutating one that does not.
fn mutated_example(copies: usize, index: usize, value: u8) -> Vec<u8> {
    let mut data = repeated_example(copies);
    data[index] = value;

    data
}

/// Checks that were bare log calls, so no caller could see them, count them or place
/// them. Each now reports a rule at the bit it fired at, and still only warns.
#[test]
fn the_formerly_untyped_checks_carry_a_rule() {
    let cases = [
        (
            1,
            52,
            162,
            RuleId::Substream(SubstreamRule::InvalidTerminationWord),
        ),
        (
            3,
            51,
            15,
            RuleId::Substream(SubstreamRule::DrcTimeUpdateExceeded),
        ),
        (
            3,
            137,
            59,
            RuleId::RestartHeader(RestartHeaderRule::BranchDataRateExceeded),
        ),
        (
            3,
            138,
            0,
            RuleId::RestartHeader(RestartHeaderRule::BranchAdvanceExceeds75ms),
        ),
    ];

    for (copies, index, value, rule) in cases {
        let diagnostics = diagnostics_of(&mutated_example(copies, index, value));
        let fired = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.rule == rule)
            .unwrap_or_else(|| panic!("{rule} did not fire"));

        assert_eq!(fired.severity, log::Level::Warn, "{fired}");
        assert!(fired.location.bit_offset.is_some(), "{fired}");
    }
}

/// A start-up gain is what a decoder that joins the stream at a major sync applies until
/// the first gain update reaches it, so it may not be louder than the gain the substream
/// is already running at. The spliced stream restates one that is.
#[test]
fn a_drc_start_up_gain_louder_than_the_running_gain_is_reported() {
    let rule = RuleId::Channel(ChannelRule::DrcStartUpGainTooLarge);

    let fired = diagnostics_of(FBA_SPLICED)
        .into_iter()
        .find(|diagnostic| diagnostic.rule == rule)
        .expect("the second segment states a start-up gain above the running gain");

    assert_eq!(fired.severity, log::Level::Warn, "{fired}");
    assert!(fired.location.bit_offset.is_some(), "{fired}");

    // The first segment states one that is not, and nothing has updated a gain when its
    // channel_meaning is read, so no substream has a gain to be louder than.
    assert!(
        !diagnostics_of(&repeated_example(2))
            .iter()
            .any(|diagnostic| diagnostic.rule == rule),
        "the example stream carries no gain updates"
    );
}

/// The other two seamless-branch causes need no mutation: the repeated example splices
/// itself onto its own start.
#[test]
fn the_branch_causes_are_reported_one_by_one() {
    let rules = rules_with_seamless_branch(true);

    assert!(rules.contains(&RuleId::RestartHeader(
        RestartHeaderRule::BranchAdvanceTooLarge
    )));
    assert!(rules.contains(&RuleId::RestartHeader(
        RestartHeaderRule::BranchAdvanceExceedsBuffer
    )));
    assert!(rules.contains(&RuleId::RestartHeader(
        RestartHeaderRule::InvalidSeamlessBranch
    )));
}

/// Every huffman table reaches its deepest leaf through a nine-bit code, and only the
/// code ending in 1 is legal. The tables decode both, so the rule has to be checked.
#[test]
fn a_nine_bit_huffman_code_must_end_in_one() {
    let diagnostics = diagnostics_of(&mutated_example(1, 71, 36));

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.rule == RuleId::Block(BlockRule::HuffmanNinthBitMissing)),
        "{diagnostics:?}"
    );
}

/// Rules that fired over the repeated example, with seamless-branch tolerance as given.
fn rules_with_seamless_branch(allow: bool) -> std::collections::HashSet<RuleId> {
    let data = repeated_example(8);

    let mut extractor = Extractor::default();
    extractor.push_bytes(&data);

    let mut parser = Parser::default();
    parser.set_diagnostic_mode(DiagnosticMode::Collect);
    parser.set_allow_seamless_branch(allow);

    for frame in extractor.by_ref().flatten() {
        parser.parse_recovering(&frame);
    }

    parser
        .take_diagnostics()
        .iter()
        .map(|diagnostic| diagnostic.rule)
        .collect()
}

/// Five checks are skipped at a major sync while seamless branches are tolerated, so a
/// verifier that knows the stream is not spliced has to be able to turn the tolerance off.
#[test]
fn seamless_branch_tolerance_gates_the_timing_checks() {
    let tolerant = rules_with_seamless_branch(true);
    let strict = rules_with_seamless_branch(false);

    for rule in [
        RuleId::AccessUnit(AccessUnitRule::TimingTooLong),
        RuleId::RestartHeader(RestartHeaderRule::InvalidOutputTiming),
    ] {
        assert!(!tolerant.contains(&rule), "{rule} fired while tolerated");
        assert!(strict.contains(&rule), "{rule} did not fire");
    }

    assert!(tolerant.is_subset(&strict));
}

/// The example stream is a 16-byte timestamp, an 84-byte major sync access unit and a
/// 20-byte continuation. Repeating only the major sync one puts a major sync in every
/// access unit, which is the one short restart gap the rule allows.
fn repeated_major_sync(times: usize) -> Vec<u8> {
    let mut data = EXAMPLE_DATA[..100].to_vec();

    for _ in 1..times {
        data.extend_from_slice(&EXAMPLE_DATA[16..100]);
    }

    data
}

/// A major sync every second access unit is a restart gap of 2, which is not allowed;
/// one in every access unit is a gap of 1, which is. The rule is only checked for a
/// single gap, so a stream with gaps of 128 (every real stream here) never reaches it.
#[test]
fn a_restart_gap_must_be_one_or_at_least_eight() {
    let rule = RuleId::AccessUnit(AccessUnitRule::RestartGapInvalid);

    let gap_of_two = diagnostics_of(&repeated_example(4));
    let fired = gap_of_two
        .iter()
        .find(|diagnostic| diagnostic.rule == rule)
        .expect("a gap of 2 violates the rule");

    assert_eq!(fired.severity, log::Level::Warn, "{fired}");
    assert_eq!(fired.message, "Restart gap must be 1 or >= 8. Read 2");
    assert!(fired.location.bit_offset.is_some(), "{fired}");

    let gap_of_one = diagnostics_of(&repeated_major_sync(4));
    assert!(
        !gap_of_one
            .iter()
            .any(|diagnostic| diagnostic.rule == rule),
        "a gap of 1 is allowed: {gap_of_one:?}"
    );
}

/// Nothing changes for a caller that did not ask for diagnostics.
#[test]
fn fail_fast_stays_the_default() {
    let data = repeated_example(8);

    let mut extractor = Extractor::default();
    extractor.push_bytes(&data);

    let mut parser = Parser::default();
    assert_eq!(parser.diagnostic_mode(), DiagnosticMode::FailFast);

    for frame in extractor.by_ref().flatten() {
        let _ = parser.parse(&frame);
        let _ = parser.parse_recovering(&frame);
    }

    assert!(parser.diagnostics().is_empty());
}

/// Both syntaxes place the major sync info CRC in the last two of 28 bytes from the
/// format_sync, unless an FBA stream carries an extra channel meaning block. None of
/// these fixtures does.
const MAJOR_SYNC_INFO_BYTES: usize = 28;

/// Byte offsets of every major sync, matched on the format sync and on the signature
/// that follows it four bytes of format_info later.
fn major_syncs(data: &[u8], format_sync: u32) -> Vec<usize> {
    let sync = format_sync.to_be_bytes();

    (0..data.len().saturating_sub(MAJOR_SYNC_INFO_BYTES))
        .filter(|&i| data[i..i + 4] == sync && data[i + 8..i + 10] == [0xB7, 0x52])
        .collect()
}

/// Rewrites a mutated major sync info's CRC, so the mutated field is judged on its own
/// rather than behind a CRC failure.
fn repair_major_sync_crc(data: &mut [u8], at: usize) {
    let crc = Crc16::new(&CRC_MAJOR_SYNC_INFO_ALG);
    let end = at + MAJOR_SYNC_INFO_BYTES - 2;
    let value = crc.update(crc.init, &data[at..end]);

    data[end..end + 2].copy_from_slice(&value.to_be_bytes());
}

/// Applies `edit` to every major sync from the `skip`th onwards, repairing each CRC.
fn edit_major_syncs(
    data: &[u8],
    format_sync: u32,
    skip: usize,
    edit: impl Fn(&mut [u8], usize),
) -> Vec<u8> {
    let syncs = major_syncs(data, format_sync);
    assert!(syncs.len() > skip, "the fixture has a major sync to edit");

    let mut out = data.to_vec();

    for at in syncs.into_iter().skip(skip) {
        edit(&mut out, at);
        repair_major_sync_crc(&mut out, at);
    }

    out
}

/// `flags` is the sixteen bits ten bytes into the major sync info.
fn map_flags(data: &[u8], format_sync: u32, skip: usize, f: impl Fn(u16) -> u16) -> Vec<u8> {
    edit_major_syncs(data, format_sync, skip, |out, at| {
        let flags = u16::from_be_bytes([out[at + 10], out[at + 11]]);
        out[at + 10..at + 12].copy_from_slice(&f(flags).to_be_bytes());
    })
}

/// The four bits before `substream_info`, and `substream_info` itself.
fn set_substream_info(data: &[u8], format_sync: u32, before: u8, substream_info: u8) -> Vec<u8> {
    edit_major_syncs(data, format_sync, 0, |out, at| {
        out[at + 16] = (out[at + 16] & 0xF0) | (before & 0xF);
        out[at + 17] = substream_info;
    })
}

fn fired(diagnostics: &[Diagnostic], rule: RuleId) -> bool {
    diagnostics.iter().any(|diagnostic| diagnostic.rule == rule)
}

/// A conformant DVD-Audio stream must raise nothing about its major sync. Both fixtures
/// were judged by FBA-only rules and failed a reserved-bits check on every major sync.
///
/// `fbb_6ch` still trips the substream 0 FIFO cap, which its `substream_info` selects as
/// zero. That is a separate question about the FBB cap tables, so this only asserts over
/// the major sync itself.
#[test]
fn a_conformant_fbb_stream_raises_nothing_about_its_major_sync() {
    for (name, data) in [("fbb_6ch", FBB_6CH), ("fbb_copy", FBB_COPY)] {
        let diagnostics = diagnostics_of(data);
        let sync: Vec<_> = diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.rule.domain() == "sync")
            .collect();

        assert!(sync.is_empty(), "{name}: {sync:?}");
    }

    assert!(
        diagnostics_of(FBB_COPY).is_empty(),
        "fbb_copy is silent end to end"
    );
}

/// Bit 14 of `flags` marks which syntax the stream is, so the same value is a violation
/// in one and mandatory in the other.
#[test]
fn flags_bit_14_marks_the_syntax() {
    let marker = RuleId::Sync(SyncRule::InvalidFlagsSyntaxMarker);
    let crc = RuleId::Sync(SyncRule::MajorSyncCrcMismatch);

    let fbb = diagnostics_of(&map_flags(FBB_COPY, MAJOR_SYNC_FBB, 0, |flags| {
        flags & !0x4000
    }));
    assert!(fired(&fbb, marker), "FBB must set bit 14: {fbb:?}");
    assert!(!fired(&fbb, crc), "the edit repairs the CRC: {fbb:?}");

    let fba = diagnostics_of(&map_flags(FBA_2CH, MAJOR_SYNC_FBA, 0, |flags| {
        flags | 0x4000
    }));
    assert!(fired(&fba, marker), "FBA must leave bit 14 clear: {fba:?}");
    assert!(!fired(&fba, crc), "the edit repairs the CRC: {fba:?}");
}

/// The reserved masks differ: bit 11 selects a restricted eight-channel presentation in
/// FBA and is reserved in FBB.
#[test]
fn flags_reserved_bits_are_syntax_specific() {
    let reserved = RuleId::Sync(SyncRule::ReservedFlagsNonZero);

    let fbb = diagnostics_of(&map_flags(FBB_COPY, MAJOR_SYNC_FBB, 0, |flags| {
        flags | 0x0800
    }));
    assert!(fired(&fbb, reserved), "bit 11 is reserved in FBB: {fbb:?}");

    let fba = diagnostics_of(&map_flags(FBA_2CH, MAJOR_SYNC_FBA, 0, |flags| {
        flags | 0x0800
    }));
    assert!(
        !fired(&fba, reserved),
        "bit 11 is meaningful in FBA: {fba:?}"
    );
}

/// Only the meaningful bits have to be constant. A reserved bit that changes between
/// major syncs is a reserved-bit violation, not a change of configuration.
#[test]
fn flags_constancy_covers_only_the_meaningful_bits() {
    let mismatch = RuleId::Sync(SyncRule::FlagsMismatch);

    let reserved_bit = diagnostics_of(&map_flags(FBB_COPY, MAJOR_SYNC_FBB, 1, |flags| flags | 1));
    assert!(
        fired(&reserved_bit, RuleId::Sync(SyncRule::ReservedFlagsNonZero)),
        "{reserved_bit:?}"
    );
    assert!(!fired(&reserved_bit, mismatch), "{reserved_bit:?}");

    let meaningful_bit = diagnostics_of(&map_flags(FBB_COPY, MAJOR_SYNC_FBB, 1, |flags| {
        flags | 0x8000
    }));
    assert!(fired(&meaningful_bit, mismatch), "{meaningful_bit:?}");
}

/// FBB defines only the low nibble of `substream_info`, and only four of its sixteen
/// values. 0x05 is one of them, and used to be read as FBA reserved bits.
#[test]
fn fbb_substream_info_is_a_low_nibble_whitelist() {
    let invalid = RuleId::Sync(SyncRule::InvalidSubstreamInfo);

    assert!(!fired(
        &diagnostics_of(FBB_COPY),
        RuleId::Sync(SyncRule::ReservedSubstreamInfo)
    ));

    let upper = diagnostics_of(&set_substream_info(FBB_COPY, MAJOR_SYNC_FBB, 0, 0xF5));
    assert!(
        upper.is_empty(),
        "the upper nibble carries nothing: {upper:?}"
    );

    for value in [0x06, 0x08, 0x0C] {
        let diagnostics = diagnostics_of(&set_substream_info(FBB_COPY, MAJOR_SYNC_FBB, 0, value));
        assert!(
            fired(&diagnostics, invalid),
            "{value:#04X}: {diagnostics:?}"
        );
    }

    for value in [0x04, 0x07] {
        let diagnostics = diagnostics_of(&set_substream_info(FBB_COPY, MAJOR_SYNC_FBB, 0, value));
        assert!(
            !fired(&diagnostics, invalid),
            "{value:#04X}: {diagnostics:?}"
        );
    }
}

/// Bit 3 is the only presence bit in FBB: it says substream 1 is also to be decoded, so
/// the stream has to carry one.
#[test]
fn fbb_substream_info_bit_3_needs_a_second_substream() {
    let rule = RuleId::Sync(SyncRule::SubstreamCountInsufficient);

    let one_substream = diagnostics_of(&set_substream_info(FBB_COPY, MAJOR_SYNC_FBB, 0, 0x0D));
    assert!(fired(&one_substream, rule), "{one_substream:?}");

    assert!(
        !fired(&diagnostics_of(FBB_6CH), rule),
        "fbb_6ch carries the substream it claims"
    );
}

/// FBB has no `extended_substream_info`: the four bits before `substream_info` are
/// wholly reserved. Reading them as one made a conformant stream look incompatible with
/// itself.
#[test]
fn fbb_has_no_extended_substream_info() {
    let diagnostics = diagnostics_of(&set_substream_info(FBB_COPY, MAJOR_SYNC_FBB, 0x1, 0x05));

    assert!(
        fired(
            &diagnostics,
            RuleId::Sync(SyncRule::ReservedBeforeSubstreamInfo)
        ),
        "{diagnostics:?}"
    );
    assert!(
        !fired(
            &diagnostics,
            RuleId::Sync(SyncRule::SubstreamInfoInCompatible)
        ),
        "{diagnostics:?}"
    );
    assert!(
        !fired(
            &diagnostics,
            RuleId::Sync(SyncRule::ReservedExtendedSubstreamInfo)
        ),
        "{diagnostics:?}"
    );

    // It describes the configuration, so it is reported once and not per major sync.
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
}

/// `hdcd_process` and the six bits after it are one block the syntax expects to be zero.
/// Finding it needs the FBB `channel_meaning` layout: under the FBA one those bits are
/// the tail of `eightch_source_format`.
#[test]
fn fbb_channel_meaning_reserved_block_is_reported() {
    let data = edit_major_syncs(FBB_COPY, MAJOR_SYNC_FBB, 0, |out, at| out[at + 24] |= 0x80);
    let diagnostics = diagnostics_of(&data);

    assert!(
        fired(
            &diagnostics,
            RuleId::Sync(SyncRule::ReservedChannelMeaningNonZero)
        ),
        "{diagnostics:?}"
    );
}
