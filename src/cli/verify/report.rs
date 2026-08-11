//! Rendering of the verify report, human-readable and as JSON Lines.

use serde_json::{Value, json};
use truehd::structs::sync::{MAJOR_SYNC_FBA, MAJOR_SYNC_FBB, MajorSyncInfo};
use truehd::utils::diagnostic::{Diagnostic, Location};
use truehd::utils::fifo::{ACCUMULATORS, Accumulator};

use super::severity::Severity;
use super::tally::{RuleTally, Tally};
use crate::timestamp::time_str;

const FIFO_LABELS: [&str; ACCUMULATORS] = ["ss0", "6-ch", "8-ch", "16-ch", "whole"];

/// What the stream says about itself, next to what the parse measured.
#[derive(Debug, Default)]
pub struct StreamFacts {
    pub access_units: u64,
    pub extracted_frames: u64,
    pub format_sync: Option<u32>,
    pub sampling_frequency: u32,
    pub samples_per_au: usize,
    pub substreams: usize,
    pub substream_info: u8,
    pub fifo_peaks: [usize; ACCUMULATORS],
    pub invalid_branches: usize,
}

impl StreamFacts {
    /// Takes the stream configuration from the first major sync seen.
    pub fn adopt_major_sync(&mut self, major_sync: &MajorSyncInfo) {
        if self.format_sync.is_some() {
            return;
        }

        self.format_sync = Some(major_sync.format_sync);
        self.substreams = major_sync.substreams;
        self.substream_info = major_sync.substream_info;
        self.sampling_frequency = major_sync.format_info.sampling_frequency_1().unwrap_or(0);
        self.samples_per_au = major_sync.format_info.samples_per_au().unwrap_or(0);
    }

    fn format_name(&self) -> &'static str {
        match self.format_sync {
            Some(MAJOR_SYNC_FBA) => "FBA",
            Some(MAJOR_SYNC_FBB) => "FBB",
            _ => "unknown",
        }
    }

    /// Byte cap of each accumulator, absent for the sums this format never checks.
    fn fifo_caps(&self) -> [Option<usize>; ACCUMULATORS] {
        let mut caps = [None; ACCUMULATORS];

        for (cap, accumulator) in caps.iter_mut().zip(Accumulator::ALL) {
            *cap = match self.format_sync {
                Some(MAJOR_SYNC_FBA) => Some(accumulator.fba_cap()),
                Some(MAJOR_SYNC_FBB) => accumulator.fbb_cap(self.substream_info),
                _ => None,
            };
        }

        caps
    }

    fn duration_secs(&self) -> Option<f64> {
        if self.sampling_frequency == 0 || self.samples_per_au == 0 {
            return None;
        }

        Some(
            (self.access_units * self.samples_per_au as u64) as f64
                / self.sampling_frequency as f64,
        )
    }

    /// Presentation 1 is a copy of presentation 0 unless this bit is set, in which case
    /// the 6-channel decoder reads substream 0 alone and its own accumulator stays zero.
    fn sixch_is_independent(&self) -> bool {
        self.substream_info & 0x08 != 0
    }
}

/// One diagnostic, as a severity-tagged line plus its message.
pub fn diagnostic_lines(diagnostic: &Diagnostic, severity: Severity) -> String {
    format!(
        "{:<8} {:<46} {}\n         {}",
        severity.as_str(),
        diagnostic.rule.to_string(),
        diagnostic.location,
        diagnostic.message,
    )
}

/// The line closing out a rule that fired more often than it was printed.
pub fn suppressed_line(rule: &str, tally: &RuleTally) -> String {
    format!(
        "  {rule}: {} shown, {} suppressed (au {} .. {}, {} access units)",
        tally.shown,
        tally.suppressed(),
        tally.first_au,
        tally.last_au,
        tally.access_units,
    )
}

pub fn diagnostic_json(diagnostic: &Diagnostic, severity: Severity) -> String {
    let Location { au_index, .. } = diagnostic.location;

    json!({
        "type": "diagnostic",
        "rule": diagnostic.rule.to_string(),
        "severity": severity.as_str(),
        "au": au_index,
        "byte_offset": diagnostic.location.byte_offset(),
        "bit_offset": diagnostic.location.bit_in_byte(),
        // The library locates a check to the bit, not to the substream it was reading.
        "substream": Value::Null,
        "message": diagnostic.message,
    })
    .to_string()
}

pub fn suppressed_json(rule: &str, tally: &RuleTally) -> String {
    json!({
        "type": "suppressed",
        "rule": rule,
        "shown": tally.shown,
        "count": tally.count,
        "first_au": tally.first_au,
        "last_au": tally.last_au,
        "access_units": tally.access_units,
    })
    .to_string()
}

/// What the run concluded, and the exit code that says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The stream did not parse to its end, so nothing was fully checked.
    Unparseable,
    Conformant,
    /// Violated a rule at or above the severity the caller fails on.
    NonConformant,
}

impl Verdict {
    pub fn of(facts: &StreamFacts, tally: &Tally, fail_on: Severity, recovered: bool) -> Self {
        if facts.access_units == 0 || !recovered {
            Verdict::Unparseable
        } else if tally.worst().is_some_and(|worst| worst >= fail_on) {
            Verdict::NonConformant
        } else {
            Verdict::Conformant
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Verdict::Unparseable => "UNPARSEABLE",
            Verdict::Conformant => "CONFORMANT",
            Verdict::NonConformant => "NON-CONFORMANT",
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Verdict::Unparseable => "unparseable",
            Verdict::Conformant => "conformant",
            Verdict::NonConformant => "non_conformant",
        }
    }

    pub const fn exit_code(self) -> i32 {
        match self {
            Verdict::Unparseable => crate::exit::PARSE,
            Verdict::Conformant => crate::exit::SUCCESS,
            Verdict::NonConformant => crate::exit::NONCONFORMANT,
        }
    }
}

pub fn summary(input: &str, facts: &StreamFacts, tally: &Tally, verdict: Verdict) -> String {
    let mut out = format!("\nSummary — {input}\n");

    match facts.duration_secs() {
        Some(secs) => out.push_str(&format!(
            "  access units          {:<12} duration  {}\n",
            facts.access_units,
            time_str(secs)
        )),
        None => out.push_str(&format!("  access units          {}\n", facts.access_units)),
    }

    if facts.extracted_frames != facts.access_units {
        out.push_str(&format!(
            "  frames extracted      {}\n",
            facts.extracted_frames
        ));
    }

    if facts.format_sync.is_some() {
        out.push_str(&format!(
            "  format                {} {} Hz, {} substreams, substream_info {:#04X}\n",
            facts.format_name(),
            facts.sampling_frequency,
            facts.substreams,
            facts.substream_info,
        ));
        out.push_str(&fifo_rows(facts));
        out.push_str(&format!(
            "  invalid branches      {}\n",
            facts.invalid_branches
        ));
    } else {
        out.push_str("  format                no major sync found\n");
    }

    let counts = Severity::ALL
        .iter()
        .rev()
        .map(|severity| format!("{} {}", tally.count_of(*severity), severity.plural()))
        .collect::<Vec<_>>()
        .join(", ");
    out.push_str(&format!("  diagnostics           {counts}\n"));

    out.push_str(&format!("  verdict               {}", verdict.label()));
    match tally.worst() {
        Some(worst) => out.push_str(&format!(" (worst: {worst})\n")),
        None => out.push('\n'),
    }

    out
}

fn fifo_rows(facts: &StreamFacts) -> String {
    let caps = facts.fifo_caps();
    let mut out = String::new();

    for (index, label) in FIFO_LABELS.iter().enumerate() {
        let heading = if index == 0 {
            "  FIFO peaks / caps    "
        } else {
            "                       "
        };

        let (label, peak) = match index {
            // Report substream 0's figure under the label that explains the zero.
            1 if !facts.sixch_is_independent() => ("6-ch (only ss0)", facts.fifo_peaks[0]),
            3 if facts.substreams < 4 => {
                out.push_str(&format!(
                    "{heading} {:<15} — (no 16-channel presentation)\n",
                    "16-ch"
                ));
                continue;
            }
            _ => (*label, facts.fifo_peaks[index]),
        };

        match caps[index] {
            Some(cap) => out.push_str(&format!("{heading} {label:<15} {peak:>7} / {cap}\n")),
            None => out.push_str(&format!("{heading} {label:<15} {peak:>7} / not checked\n")),
        }
    }

    out
}

pub fn summary_json(
    input: &str,
    facts: &StreamFacts,
    tally: &Tally,
    verdict: Verdict,
) -> String {
    let by_rule: serde_json::Map<String, Value> = tally
        .rules()
        .map(|(rule, tally)| (rule.to_owned(), json!(tally.count)))
        .collect();

    let counts: serde_json::Map<String, Value> = Severity::ALL
        .iter()
        .map(|severity| (severity.to_string(), json!(tally.count_of(*severity))))
        .collect();

    json!({
        "type": "summary",
        "input": input,
        "access_units": facts.access_units,
        "extracted_frames": facts.extracted_frames,
        "format_sync": facts.format_name(),
        "sampling_frequency": facts.sampling_frequency,
        "substreams": facts.substreams,
        "substream_info": facts.substream_info,
        "duration_seconds": facts.duration_secs(),
        "fifo_peaks": facts.fifo_peaks,
        "fifo_caps": facts.fifo_caps(),
        "invalid_branches": facts.invalid_branches,
        "counts": counts,
        "by_rule": by_rule,
        "diagnostics": tally.total(),
        "worst_severity": tally.worst().map(Severity::as_str),
        "verdict": verdict.as_str(),
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use truehd::utils::diagnostic::Rule;

    use super::*;

    fn facts() -> StreamFacts {
        StreamFacts {
            access_units: 100,
            extracted_frames: 100,
            format_sync: Some(MAJOR_SYNC_FBA),
            sampling_frequency: 48000,
            samples_per_au: 40,
            substreams: 3,
            substream_info: 0x7C,
            fifo_peaks: [5470, 13626, 10354, 0, 23542],
            invalid_branches: 0,
        }
    }

    #[test]
    fn fba_caps_are_reported_for_every_accumulator() {
        assert_eq!(
            facts().fifo_caps(),
            [
                Some(30_000),
                Some(90_000),
                Some(120_000),
                Some(120_000),
                Some(120_000)
            ]
        );
    }

    /// FBB never checks the 8- and 16-channel sums, and its caps depend on
    /// `substream_info`, so they must not be filled in from the FBA table.
    #[test]
    fn fbb_caps_are_absent_where_the_format_does_not_check_them() {
        let facts = StreamFacts {
            format_sync: Some(MAJOR_SYNC_FBB),
            substream_info: 5,
            ..facts()
        };

        let caps = facts.fifo_caps();
        assert_eq!(caps[Accumulator::Eightch as usize], None);
        assert_eq!(caps[Accumulator::Sixteench as usize], None);
        assert_eq!(caps[Accumulator::Substream0 as usize], Some(30_000));
    }

    #[test]
    fn a_stream_without_a_major_sync_claims_no_caps() {
        let facts = StreamFacts::default();

        assert_eq!(facts.fifo_caps(), [None; ACCUMULATORS]);
        assert_eq!(facts.duration_secs(), None);
        assert_eq!(facts.format_name(), "unknown");
    }

    #[test]
    fn duration_follows_the_access_unit_count() {
        let secs = facts().duration_secs().unwrap();

        assert!((secs - 100.0 * 40.0 / 48000.0).abs() < f64::EPSILON);
    }

    #[test]
    fn a_clean_stream_is_conformant_at_every_threshold() {
        let tally = Tally::default();

        for fail_on in Severity::ALL {
            let verdict = Verdict::of(&facts(), &tally, fail_on, true);
            assert_eq!(verdict, Verdict::Conformant);
            assert_eq!(verdict.exit_code(), crate::exit::SUCCESS);
        }

        let text = summary("x.mlp", &facts(), &tally, Verdict::Conformant);
        assert!(text.contains("verdict               CONFORMANT\n"));
        assert!(text.contains("0 fatal, 0 errors, 0 warnings, 0 info"));
    }

    #[test]
    fn the_threshold_decides_the_verdict_not_the_worst_severity() {
        let mut tally = Tally::default();
        tally.record("access_unit.timing_too_short", Severity::Warning, 3, 0);

        let verdict = |fail_on| Verdict::of(&facts(), &tally, fail_on, true);
        assert_eq!(verdict(Severity::Error), Verdict::Conformant);
        assert_eq!(verdict(Severity::Warning), Verdict::NonConformant);
        assert_eq!(verdict(Severity::Info), Verdict::NonConformant);
        assert_eq!(
            verdict(Severity::Warning).exit_code(),
            crate::exit::NONCONFORMANT
        );

        let text = summary("x.mlp", &facts(), &tally, Verdict::NonConformant);
        assert!(text.contains("NON-CONFORMANT (worst: warning)"));

        let text = summary("x.mlp", &facts(), &tally, Verdict::Conformant);
        assert!(text.contains("CONFORMANT (worst: warning)"));
    }

    /// A stream that never parsed to its end was never fully checked, so it cannot be
    /// called conformant however few diagnostics were counted.
    #[test]
    fn an_unparsed_stream_is_never_conformant() {
        let tally = Tally::default();

        let no_access_units = StreamFacts {
            access_units: 0,
            ..facts()
        };
        let verdict = Verdict::of(&no_access_units, &tally, Severity::Fatal, true);
        assert_eq!(verdict, Verdict::Unparseable);
        assert_eq!(verdict.exit_code(), crate::exit::PARSE);

        assert_eq!(
            Verdict::of(&facts(), &tally, Severity::Fatal, false),
            Verdict::Unparseable
        );
    }

    /// Without a major sync there is nothing to report peaks or caps against.
    #[test]
    fn a_stream_without_a_major_sync_reports_no_format_block() {
        let text = summary(
            "x.mlp",
            &StreamFacts::default(),
            &Tally::default(),
            Verdict::Unparseable,
        );

        assert!(text.contains("  format                no major sync found\n"));
        assert!(!text.contains("FIFO"));
        assert!(text.contains("verdict               UNPARSEABLE\n"));
    }

    /// A 6-channel presentation that is a copy of presentation 0 has a legitimately
    /// empty accumulator; the row shows substream 0's figure and says so.
    #[test]
    fn the_sixch_row_falls_back_when_the_presentation_is_a_copy() {
        let facts = StreamFacts {
            substream_info: 0x7C & !0x08,
            fifo_peaks: [5470, 0, 10354, 0, 23542],
            ..facts()
        };

        let text = summary("x.mlp", &facts, &Tally::default(), Verdict::Conformant);
        assert!(text.contains("6-ch (only ss0)    5470 / 90000"), "{text}");

        let sixteench = text
            .lines()
            .find(|line| line.contains("16-ch"))
            .unwrap_or_default();
        assert!(sixteench.contains("(no 16-channel presentation)"), "{text}");
        assert!(!sixteench.contains('/'), "{text}");
    }

    #[test]
    fn the_summary_json_is_one_line_and_carries_the_tally() {
        let mut tally = Tally::default();
        tally.record("fifo.underrun", Severity::Error, 12, 0);
        tally.record("fifo.underrun", Severity::Error, 13, 0);

        let line = summary_json("x.mlp", &facts(), &tally, Verdict::NonConformant);
        assert!(!line.contains('\n'));

        let value: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["type"], "summary");
        assert_eq!(value["verdict"], "non_conformant");
        assert_eq!(value["worst_severity"], "error");
        assert_eq!(value["by_rule"]["fifo.underrun"], 2);
        assert_eq!(value["counts"]["error"], 2);
        assert_eq!(value["fifo_caps"][3], 120_000);
        assert_eq!(value["format_sync"], "FBA");
    }

    #[test]
    fn a_diagnostic_json_carries_the_position_split_as_byte_and_bit() {
        let diagnostic = Diagnostic {
            rule: truehd::utils::errors::FifoError::Underrun { index: 0 }.rule_id(),
            severity: log::Level::Error,
            location: Location {
                au_index: 7,
                au_offset: 0x100,
                bit_offset: Some(19),
            },
            message: "under".to_owned(),
            source: None,
        };

        let value: Value =
            serde_json::from_str(&diagnostic_json(&diagnostic, Severity::Fatal)).unwrap();

        assert_eq!(value["type"], "diagnostic");
        assert_eq!(value["rule"], "fifo.underrun");
        assert_eq!(value["severity"], "fatal");
        assert_eq!(value["au"], 7);
        assert_eq!(value["byte_offset"], 0x102);
        assert_eq!(value["bit_offset"], 3);
        assert_eq!(value["substream"], Value::Null);
        assert_eq!(value["message"], "under");

        let diagnostic = Diagnostic {
            location: Location {
                bit_offset: None,
                ..diagnostic.location
            },
            ..diagnostic
        };
        let value: Value =
            serde_json::from_str(&diagnostic_json(&diagnostic, Severity::Info)).unwrap();

        assert_eq!(value["byte_offset"], 0x100);
        assert_eq!(value["bit_offset"], Value::Null);
    }

    #[test]
    fn a_suppressed_rule_reports_shown_and_counted_alike() {
        let mut tally = Tally::default();
        for au in 0..50 {
            tally.record("access_unit.timing_too_short", Severity::Warning, au, 20);
        }

        let (rule, counts) = tally.suppressed().next().unwrap();
        assert_eq!(
            suppressed_line(rule, counts),
            "  access_unit.timing_too_short: 20 shown, 30 suppressed (au 0 .. 49, 50 access units)"
        );

        let value: Value = serde_json::from_str(&suppressed_json(rule, counts)).unwrap();
        assert_eq!(value["shown"], 20);
        assert_eq!(value["count"], 50);
        assert_eq!(value["access_units"], 50);
    }
}
