//! Rendering of the verify report, human-readable and as JSON Lines.

use serde_json::{Value, json};
use truehd::process::parse::Parser;
use truehd::structs::sync::{MAJOR_SYNC_FBA, MAJOR_SYNC_FBB, MajorSyncInfo};
use truehd::utils::diagnostic::{Diagnostic, Location};
use truehd::utils::fifo::{ACCUMULATORS, Accumulator, FifoPeak, SUBSTREAMS};

use super::severity::Severity;
use super::tally::{RuleTally, Tally};
use crate::timestamp::time_str;

/// Width of the label column, after the two-space indent `info` also uses.
const LABEL: usize = 28;

/// Width of one numeric column in the two tables.
const COLUMN: usize = 11;

/// The decoder each accumulator belongs to, for the row that reports it.
const FIFO_LABELS: [&str; ACCUMULATORS] = [
    "2-channel decoder",
    "6-channel decoder",
    "8-channel decoder",
    "16-channel decoder",
    "Whole stream",
];

/// What one substream declares about itself, and how deep its own bytes went.
#[derive(Debug, Default, Clone, Copy)]
pub struct SubstreamFacts {
    pub restart_sync_word: u16,
    pub min_chan: usize,
    pub max_chan: usize,
    pub max_matrix_chan: usize,
    pub fifo_peak: usize,
    pub fifo_cap: Option<usize>,
}

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
    /// Each accumulator's deepest point, split into stream bytes and overhead.
    pub fifo_records: [FifoPeak; ACCUMULATORS],
    pub substream_facts: Vec<SubstreamFacts>,
    pub invalid_branches: usize,
    pub max_data_rate: usize,
    pub max_data_rate_au: usize,
    pub max_fifo_latency: usize,
    pub max_access_unit_size: usize,
    pub total_access_unit_bytes: usize,
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

    /// Everything the parse measured but the stream does not state about itself.
    pub fn adopt_measurements(&mut self, parser: &Parser) {
        self.fifo_records = parser.fifo_depth_records();
        self.invalid_branches = parser.invalid_branches();
        self.max_data_rate = parser.max_data_rate();
        self.max_data_rate_au = parser.max_data_rate_au();
        self.max_fifo_latency = parser.max_fifo_latency();
        self.max_access_unit_size = parser.max_access_unit_size();
        self.total_access_unit_bytes = parser.total_access_unit_bytes();

        let peaks = parser.fifo_substream_peaks();

        self.substream_facts = (0..self.substreams.min(SUBSTREAMS))
            .filter_map(|index| {
                let state = parser.substream_state(index)?;

                Some(SubstreamFacts {
                    restart_sync_word: state.restart.restart_sync_word,
                    min_chan: state.restart.min_chan,
                    max_chan: state.restart.max_chan,
                    max_matrix_chan: state.restart.max_matrix_chan,
                    fifo_peak: peaks[index],
                    fifo_cap: Self::substream_cap(state.restart.min_chan, state.restart.max_chan),
                })
            })
            .collect();
    }

    fn format_name(&self) -> &'static str {
        match self.format_sync {
            Some(MAJOR_SYNC_FBA) => "FBA",
            Some(MAJOR_SYNC_FBB) => "FBB",
            _ => "unknown",
        }
    }

    fn fifo_peaks(&self) -> [usize; ACCUMULATORS] {
        self.fifo_records.map(|record| record.total)
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

    /// A substream is allowed 15000 bytes for each channel it carries, to a ceiling of
    /// 120000. Its channel span is what counts: substreams that partition the channels
    /// each state their own share, while cumulative ones each state their presentation's
    /// whole allowance.
    pub fn substream_cap(min_chan: usize, max_chan: usize) -> Option<usize> {
        let channels = max_chan.checked_sub(min_chan)? + 1;

        Some((15_000 * channels).min(120_000))
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

    /// Whether the stream declares a 16-channel presentation and carries the substream
    /// that would hold it. Only then is the 16-channel sum accumulated or capped.
    fn has_sixteench_presentation(&self) -> bool {
        self.format_sync == Some(MAJOR_SYNC_FBA)
            && self.substream_info & 0x80 != 0
            && self.substreams >= 4
    }

    /// Channels the decoder reading substream 0 alone reconstructs. FBA always gives it
    /// two; an FBB stream may put the whole six-channel presentation there, and then no
    /// two-channel decoder exists to report.
    fn substream0_channels(&self) -> Option<usize> {
        let substream = self.substream_facts.first()?;

        Some(substream.max_chan.checked_sub(substream.min_chan)? + 1)
    }

    fn max_fifo_latency_ms(&self) -> Option<f64> {
        if self.sampling_frequency == 0 {
            return None;
        }

        Some(self.max_fifo_latency as f64 * 1000.0 / self.sampling_frequency as f64)
    }

    fn average_access_unit_size(&self) -> Option<f64> {
        if self.access_units == 0 {
            return None;
        }

        Some(self.total_access_unit_bytes as f64 / self.access_units as f64)
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

/// One `  label   value` line, laid out as the `info` command lays its own out.
fn field(label: &str, value: impl std::fmt::Display) -> String {
    format!("  {label:<LABEL$}{value}\n")
}

pub fn summary(input: &str, facts: &StreamFacts, tally: &Tally, verdict: Verdict) -> String {
    let mut out = String::from("\nVerification Summary\n====================\n\n");

    out.push_str("Stream Information\n");
    out.push_str(&field("Input", input));

    match facts.format_sync {
        Some(format_sync) => {
            out.push_str(&field(
                "Format Sync",
                format_args!("{} ({format_sync:08X})", facts.format_name()),
            ));
            out.push_str(&field(
                "Sampling rate",
                format_args!("{} Hz", facts.sampling_frequency),
            ));
            out.push_str(&field("Number of substreams", facts.substreams));
            out.push_str(&field(
                "substream_info",
                format_args!("{:#04X}", facts.substream_info),
            ));
        }
        None => out.push_str(&field("Format Sync", "no major sync found")),
    }

    out.push('\n');
    out.push_str(&measurements(facts));

    if facts.format_sync.is_some() {
        out.push('\n');
        out.push_str(&substream_table(facts));
        out.push('\n');
        out.push_str(&fifo_table(facts));
    }

    out.push('\n');
    out.push_str("Diagnostics\n");

    for severity in Severity::ALL.iter().rev() {
        let label = match severity {
            Severity::Fatal => "Fatal",
            Severity::Error => "Errors",
            Severity::Warning => "Warnings",
            Severity::Info => "Info",
        };

        out.push_str(&field(label, tally.count_of(*severity)));
    }

    match tally.worst() {
        Some(worst) => out.push_str(&field(
            "Verdict",
            format_args!("{} (worst: {worst})", verdict.label()),
        )),
        None => out.push_str(&field("Verdict", verdict.label())),
    }

    out
}

fn measurements(facts: &StreamFacts) -> String {
    let mut out = String::from("Stream Measurements\n");

    out.push_str(&field("Access units", facts.access_units));

    if facts.extracted_frames != facts.access_units {
        out.push_str(&field("Frames extracted", facts.extracted_frames));
    }

    if let Some(secs) = facts.duration_secs() {
        out.push_str(&field("Duration", time_str(secs)));
    }

    match facts.average_access_unit_size() {
        Some(average) => out.push_str(&field(
            "Access unit size",
            format_args!(
                "{average:.2} bytes average, {} bytes maximum",
                facts.max_access_unit_size
            ),
        )),
        None => out.push_str(&field(
            "Access unit size",
            format_args!("{} bytes maximum", facts.max_access_unit_size),
        )),
    }

    if facts.max_data_rate != 0 {
        out.push_str(&field(
            "Maximum data rate",
            format_args!(
                "{:.1} kbps, at access unit {}",
                facts.max_data_rate as f64 / 1000.0,
                facts.max_data_rate_au
            ),
        ));
    }

    match facts.max_fifo_latency_ms() {
        Some(ms) => out.push_str(&field(
            "Maximum FIFO latency",
            format_args!("{} samples ({ms:.3} ms)", facts.max_fifo_latency),
        )),
        None => out.push_str(&field(
            "Maximum FIFO latency",
            format_args!("{} samples", facts.max_fifo_latency),
        )),
    }

    out.push_str(&field("Invalid branches", facts.invalid_branches));

    out
}

/// The per-substream view: what each substream carries, how deep its own bytes went and
/// what it is allowed on its own.
///
/// Transposed like the stream's own description, a column per substream, so that the
/// depths sit under the channel range that earns their allowance.
fn substream_table(facts: &StreamFacts) -> String {
    let mut out = String::from("Substream Properties\n");

    if facts.substream_facts.is_empty() {
        out.push_str(&field("Substreams", "no substream reached a restart header"));

        return out;
    }

    let columns = |values: Vec<String>| {
        let mut line = String::new();

        for value in values {
            line.push_str(&format!("{value:>COLUMN$}"));
        }

        line
    };

    let heading = columns(
        (0..facts.substream_facts.len())
            .map(|index| format!("ss{index}"))
            .collect(),
    );
    out.push_str(&format!("  {:<LABEL$}{heading}\n", ""));

    let row = |label: &str, values: Vec<String>| format!("  {label:<LABEL$}{}\n", columns(values));

    out.push_str(&row(
        "Restart sync word",
        facts
            .substream_facts
            .iter()
            .map(|substream| format!("{:04X}", substream.restart_sync_word))
            .collect(),
    ));
    out.push_str(&row(
        "Channels",
        facts
            .substream_facts
            .iter()
            .map(|substream| format!("{}..{}", substream.min_chan, substream.max_chan))
            .collect(),
    ));
    out.push_str(&row(
        "Max matrix channel",
        facts
            .substream_facts
            .iter()
            .map(|substream| substream.max_matrix_chan.to_string())
            .collect(),
    ));
    out.push_str(&row(
        "Maximum FIFO depth",
        facts
            .substream_facts
            .iter()
            .map(|substream| substream.fifo_peak.to_string())
            .collect(),
    ));
    out.push_str(&row(
        "Allowed FIFO depth",
        facts
            .substream_facts
            .iter()
            .map(|substream| match substream.fifo_cap {
                Some(cap) => cap.to_string(),
                None => "-".to_owned(),
            })
            .collect(),
    ));

    out
}

/// The cumulative view: one row per accumulator, with the stream's own bytes separated
/// from the container overhead priced around them.
fn fifo_table(facts: &StreamFacts) -> String {
    let caps = facts.fifo_caps();
    let mut out = String::from("FIFO Depth, cumulative\n");

    out.push_str(&format!(
        "  {:<LABEL$}{:>COLUMN$}{:>COLUMN$}{:>COLUMN$}{:>COLUMN$}\n",
        "", "stream", "overhead", "total", "allowed"
    ));

    for (index, label) in FIFO_LABELS.iter().enumerate() {
        // A decoder the stream does not carry has nothing to say in any column.
        if index == 0 && facts.substream0_channels().is_some_and(|channels| channels != 2) {
            out.push_str(&format!("  {label:<LABEL$}"));
            out.push_str(&format!("{:>COLUMN$}", "-").repeat(4));
            out.push('\n');

            continue;
        }

        // A sum this stream never accumulates has no depth to report, only the reason.
        let unavailable = match index {
            1 if !facts.sixch_is_independent() => None,
            2 | 3 if facts.format_sync == Some(MAJOR_SYNC_FBB) => Some("not checked"),
            3 if !facts.has_sixteench_presentation() => Some("no 16-channel presentation"),
            _ => None,
        };

        if let Some(reason) = unavailable {
            out.push_str(&format!(
                "  {label:<LABEL$}{reason:>width$}\n",
                width = 4 * COLUMN
            ));

            continue;
        }

        // Report substream 0's figure under the label that explains the zero: the
        // 6-channel decoder reads substream 0 alone, so its own accumulator stays empty.
        let (label, record) = match index {
            1 if !facts.sixch_is_independent() => {
                ("6-channel decoder (ss0 only)", facts.fifo_records[0])
            }
            _ => (*label, facts.fifo_records[index]),
        };

        out.push_str(&format!(
            "  {label:<LABEL$}{:>COLUMN$}{:>COLUMN$}{:>COLUMN$}",
            record.stream, record.overhead, record.total
        ));

        match caps[index] {
            Some(cap) => out.push_str(&format!("{cap:>COLUMN$}\n")),
            None => out.push_str(&format!("{:>COLUMN$}\n", "not checked")),
        }
    }

    out
}

pub fn summary_json(input: &str, facts: &StreamFacts, tally: &Tally, verdict: Verdict) -> String {
    let by_rule: serde_json::Map<String, Value> = tally
        .rules()
        .map(|(rule, tally)| (rule.to_owned(), json!(tally.count)))
        .collect();

    let counts: serde_json::Map<String, Value> = Severity::ALL
        .iter()
        .map(|severity| (severity.to_string(), json!(tally.count_of(*severity))))
        .collect();

    let substreams: Vec<Value> = facts
        .substream_facts
        .iter()
        .enumerate()
        .map(|(index, substream)| {
            json!({
                "index": index,
                "restart_sync_word": format!("{:04X}", substream.restart_sync_word),
                "min_chan": substream.min_chan,
                "max_chan": substream.max_chan,
                "max_matrix_chan": substream.max_matrix_chan,
                "fifo_peak": substream.fifo_peak,
                "fifo_cap": substream.fifo_cap,
            })
        })
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
        "fifo_peaks": facts.fifo_peaks(),
        "fifo_caps": facts.fifo_caps(),
        "fifo_stream_bytes": facts.fifo_records.map(|record| record.stream),
        "fifo_overhead_bytes": facts.fifo_records.map(|record| record.overhead),
        "substream_properties": substreams,
        "invalid_branches": facts.invalid_branches,
        "max_data_rate": facts.max_data_rate,
        "max_data_rate_au": facts.max_data_rate_au,
        "max_fifo_latency_samples": facts.max_fifo_latency,
        "max_fifo_latency_ms": facts.max_fifo_latency_ms(),
        "max_access_unit_size": facts.max_access_unit_size,
        "average_access_unit_size": facts.average_access_unit_size(),
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
    use truehd::process::MAX_PRESENTATIONS;
    use truehd::process::extract::Extractor;
    use truehd::utils::diagnostic::Rule;

    use super::*;

    /// Four independent presentations in four substreams, so every accumulator sums a
    /// different set and the 6-channel one is a substream that carries its presentation
    /// whole.
    const FBA_ATMOS_CBI: &[u8] = include_bytes!("../../../truehd/tests/assets/fba_atmos_cbi.mlp");

    /// One FBA substream, so the 6- and 8-channel presentations are copies of the first.
    const FBA_2CH: &[u8] = include_bytes!("../../../truehd/tests/assets/fba_2ch.mlp");

    /// FBB over two substreams, `substream_info` 0x0D.
    const FBB_6CH: &[u8] = include_bytes!("../../../truehd/tests/assets/fbb_6ch.mlp");

    /// FBB carrying all six channels in substream 0, `substream_info` 0x04, so the stream
    /// has no two-channel decoder at all.
    const FBB_6CH_SINGLE: &[u8] =
        include_bytes!("../../../truehd/tests/assets/fbb_6ch_single.mlp");

    /// FBB over one substream of two channels, `substream_info` 0x05.
    const FBB_COPY: &[u8] = include_bytes!("../../../truehd/tests/assets/fbb_copy.mlp");

    /// The facts a run gathers from a stream, taken as `Verification` takes them.
    fn facts_of(data: &[u8]) -> StreamFacts {
        let mut extractor = Extractor::default();
        extractor.push_bytes(data);

        let mut parser = Parser::default();
        parser.set_required_presentations(&[true; MAX_PRESENTATIONS]);

        let mut facts = StreamFacts::default();

        for frame in extractor.flatten() {
            facts.extracted_frames += 1;

            let Ok(access_unit) = parser.parse(&frame) else {
                continue;
            };

            facts.access_units += 1;

            if let Some(major_sync) = &access_unit.major_sync_info {
                facts.adopt_major_sync(major_sync);
            }
        }

        facts.adopt_measurements(&parser);

        facts
    }

    fn facts() -> StreamFacts {
        facts_of(FBA_ATMOS_CBI)
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
        let facts = facts_of(FBB_COPY);
        assert_eq!(facts.substream_info, 0x05);

        let caps = facts.fifo_caps();
        assert_eq!(caps[Accumulator::Eightch as usize], None);
        assert_eq!(caps[Accumulator::Sixteench as usize], None);
        assert_eq!(caps[Accumulator::Substream0 as usize], Some(30_000));
    }

    /// A substream is allowed 15000 bytes per channel it carries, to a 120000 ceiling.
    /// Substreams that partition the channels each state their own share; cumulative
    /// ones each state their presentation's whole allowance. Both shapes match the
    /// figures an encoder states for them.
    #[test]
    fn a_substream_is_allowed_fifteen_thousand_bytes_a_channel() {
        // partitioned: a two-substream DVD-Audio stream, ss1 carrying channels 2..5
        assert_eq!(StreamFacts::substream_cap(0, 1), Some(30_000));
        assert_eq!(StreamFacts::substream_cap(2, 5), Some(60_000));

        // cumulative: four independent presentations, each spanning from channel 0
        assert_eq!(StreamFacts::substream_cap(0, 5), Some(90_000));
        assert_eq!(StreamFacts::substream_cap(0, 7), Some(120_000));
        // sixteen channels would ask 240000; the ceiling holds it at 120000
        assert_eq!(StreamFacts::substream_cap(0, 15), Some(120_000));

        // a span that cannot be read states nothing
        assert_eq!(StreamFacts::substream_cap(5, 0), None);
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

        assert!((secs - 136.0 * 40.0 / 48000.0).abs() < f64::EPSILON);
    }

    /// Latency in milliseconds is the sample count against the sampling rate, and the
    /// average access unit is the parsed bytes over the parsed access units.
    #[test]
    fn the_derived_measurements_follow_the_stream_configuration() {
        let facts = facts();

        assert!((facts.max_fifo_latency_ms().unwrap() - 1.1666).abs() < 0.001);
        assert!((facts.average_access_unit_size().unwrap() - 34334.0 / 136.0).abs() < f64::EPSILON);

        let empty = StreamFacts::default();
        assert_eq!(empty.max_fifo_latency_ms(), None);
        assert_eq!(empty.average_access_unit_size(), None);
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
        assert!(text.contains("Verdict                     CONFORMANT\n"), "{text}");
        assert!(text.contains("Fatal                       0\n"), "{text}");
    }

    /// Every measurement the report gained has a line of its own.
    #[test]
    fn the_measurements_reach_the_report() {
        let text = summary("x.mlp", &facts(), &Tally::default(), Verdict::Conformant);

        assert!(
            text.contains("Maximum data rate           3840.0 kbps, at access unit 0\n"),
            "{text}"
        );
        assert!(
            text.contains("Maximum FIFO latency        56 samples (1.167 ms)\n"),
            "{text}"
        );
        assert!(
            text.contains("Access unit size            252.46 bytes average, 556 bytes maximum\n"),
            "{text}"
        );
    }

    /// The per-substream table stands beside the cumulative one, with each substream's
    /// own depth and its own allowance.
    #[test]
    fn the_substream_table_reports_a_column_per_substream() {
        let text = summary("x.mlp", &facts(), &Tally::default(), Verdict::Conformant);

        assert!(text.contains("Substream Properties\n"), "{text}");
        assert!(
            text.contains("Restart sync word                  31EA       31EB       31EB       31EC\n"),
            "{text}"
        );
        assert!(
            text.contains("Channels                           0..1       0..5       0..7      0..15\n"),
            "{text}"
        );
        assert!(
            text.contains("Maximum FIFO depth                  240        246        250        274\n"),
            "{text}"
        );
        assert!(
            text.contains("Allowed FIFO depth                30000      90000     120000     120000\n"),
            "{text}"
        );
    }

    /// The cumulative rows split into stream and overhead, and the two add back up.
    #[test]
    fn the_fifo_table_separates_the_stream_from_its_overhead() {
        let text = summary("x.mlp", &facts(), &Tally::default(), Verdict::Conformant);

        assert!(
            text.contains("                                 stream   overhead      total    allowed\n"),
            "{text}"
        );
        assert!(
            text.contains("2-channel decoder                   240         54        294      30000\n"),
            "{text}"
        );
        assert!(
            text.contains("Whole stream                       1010         72       1082     120000\n"),
            "{text}"
        );

        // Substream 1 carries the whole 6-channel presentation here, so that row is its
        // own bytes and not substream 0's as well.
        let facts = facts();
        assert!(
            text.contains("6-channel decoder                   246         52        298      90000\n"),
            "{text}"
        );
        assert_eq!(
            facts.fifo_records[1].stream,
            facts.substream_facts[1].fifo_peak
        );
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

        assert!(
            text.contains("Format Sync                 no major sync found\n"),
            "{text}"
        );
        assert!(!text.contains("FIFO Depth"), "{text}");
        assert!(text.contains("Verdict                     UNPARSEABLE\n"), "{text}");
    }

    /// A single-substream stream declares its 6-channel presentation as a copy of
    /// presentation 0, so that decoder reads substream 0 alone. The row says so, and
    /// carries substream 0's own figure rather than a zero.
    #[test]
    fn the_sixch_row_says_so_when_the_presentation_is_a_copy() {
        let facts = facts_of(FBA_2CH);

        let text = summary("x.mlp", &facts, &Tally::default(), Verdict::Conformant);
        assert!(
            text.contains("6-channel decoder (ss0 only)       2210        174       2384      90000\n"),
            "{text}"
        );

        let sixteench = text
            .lines()
            .find(|line| line.contains("16-channel"))
            .unwrap_or_default();
        assert!(sixteench.contains("no 16-channel presentation"), "{text}");
    }

    /// An FBB stream may carry its whole six-channel presentation in substream 0. There is
    /// then no two-channel decoder to report, and substream 0's bytes belong to the
    /// six-channel row.
    #[test]
    fn a_six_channel_substream_zero_leaves_no_two_channel_decoder() {
        let facts = facts_of(FBB_6CH_SINGLE);
        assert_eq!(facts.substream_info, 0x04);
        assert_eq!(facts.substream0_channels(), Some(6));

        let text = summary("x.mlp", &facts, &Tally::default(), Verdict::Conformant);
        assert!(
            text.contains("2-channel decoder                     -          -          -          -\n"),
            "{text}"
        );
        assert!(
            text.contains("6-channel decoder (ss0 only)         44         46         90      90000\n"),
            "{text}"
        );
    }

    /// FBB accumulates neither the 8- nor the 16-channel sum, and the rows must say so
    /// rather than print the zeros the accumulators legitimately hold.
    #[test]
    fn the_fbb_rows_the_format_never_sums_are_not_reported_as_zero() {
        let facts = facts_of(FBB_6CH);
        assert_eq!(facts.substream_info, 0x0D);

        let text = summary("x.mlp", &facts, &Tally::default(), Verdict::Conformant);

        for label in ["8-channel decoder", "16-channel decoder"] {
            let row = text
                .lines()
                .find(|line| line.contains(label))
                .unwrap_or_default();

            assert!(row.ends_with("not checked"), "{text}");
            assert!(!row.contains('0'), "{text}");
        }
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

    /// Every figure the human report gained is in the JSON summary under a name of its
    /// own, so the two never drift apart.
    #[test]
    fn the_summary_json_carries_every_new_measurement() {
        let value: Value = serde_json::from_str(&summary_json(
            "x.mlp",
            &facts(),
            &Tally::default(),
            Verdict::Conformant,
        ))
        .unwrap();

        assert_eq!(value["fifo_peaks"][0], 294);
        assert_eq!(value["fifo_stream_bytes"][0], 240);
        assert_eq!(value["fifo_overhead_bytes"][0], 54);
        assert_eq!(value["max_data_rate"], 3_840_000);
        assert_eq!(value["max_data_rate_au"], 0);
        assert_eq!(value["max_fifo_latency_samples"], 56);
        assert_eq!(value["max_access_unit_size"], 556);
        assert_eq!(value["substream_properties"][1]["max_chan"], 5);
        assert_eq!(value["substream_properties"][1]["fifo_peak"], 246);
        assert_eq!(value["substream_properties"][1]["fifo_cap"], 90_000);
        assert_eq!(value["substream_properties"][0]["restart_sync_word"], "31EA");
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
