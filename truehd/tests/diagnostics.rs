//! Collect-mode diagnostics over a whole stream.

use truehd::process::EXAMPLE_DATA;
use truehd::process::extract::Extractor;
use truehd::process::parse::Parser;
use truehd::utils::diagnostic::{DiagnosticMode, RuleId, SubstreamRule};
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
