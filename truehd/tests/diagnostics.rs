//! Collect-mode diagnostics over a whole stream.

use truehd::process::EXAMPLE_DATA;
use truehd::process::extract::Extractor;
use truehd::process::parse::Parser;
use truehd::utils::diagnostic::{
    AccessUnitRule, BlockRule, Diagnostic, DiagnosticMode, RestartHeaderRule, RuleId, SubstreamRule,
};
use truehd::utils::errors::SubstreamError;

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
