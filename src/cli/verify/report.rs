//! Rendering of the verify report, human-readable and as JSON Lines.

use serde_json::{Value, json};
use truehd::process::parse::{Branch, Parser};
use truehd::process::{MAX_PRESENTATIONS, PresentationMap};
use truehd::structs::channel::ChannelLabel;
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

/// Bytes a substream carrying no channel is allowed for its headers alone.
const EMPTY_SUBSTREAM_CAP: usize = 5_000;

/// The restart sync word substream 0 must carry for a stream to be legal on any disc.
const FIRST_RESTART_SYNC_WORD: u16 = 0x31EA;

/// Which disc formats a stream is legal for.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DiscValidity {
    pub dvd_audio: bool,
    pub hd_dvd_video: bool,
    pub bluray: bool,
    /// DVD-Audio additionally requires that the six-channel downmix not clip, which a
    /// parse cannot establish.
    pub dvd_audio_needs_decode: bool,
}

impl DiscValidity {
    /// Whether any disc format admits the stream at all.
    pub fn any_format(&self) -> bool {
        self.dvd_audio || self.hd_dvd_video || self.bluray
    }
}

/// What one substream declares about itself, and how deep its own bytes went.
#[derive(Debug, Default, Clone, Copy)]
pub struct SubstreamFacts {
    pub restart_sync_word: u16,
    pub min_chan: usize,
    pub max_chan: usize,
    pub max_matrix_chan: usize,
    pub fifo_peak: usize,
    pub fifo_cap: usize,
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
    pub extended_substream_info: u8,
    /// Channels the major sync declares for the 6- and 8-channel presentations, which the
    /// disc rules check against the channels the substreams actually carry.
    pub declared_sixch_channels: usize,
    pub declared_eightch_channels: usize,
    /// Each accumulator's deepest point, split into stream bytes and overhead.
    pub fifo_records: [FifoPeak; ACCUMULATORS],
    pub substream_facts: Vec<SubstreamFacts>,
    pub branches: Vec<Branch>,
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
        self.extended_substream_info = major_sync.extended_substream_info;
        self.sampling_frequency = major_sync.format_info.sampling_frequency_1().unwrap_or(0);
        self.samples_per_au = major_sync.format_info.samples_per_au().unwrap_or(0);

        let format_info = &major_sync.format_info;
        self.declared_sixch_channels =
            ChannelLabel::from_sixch_channel(format_info.sixch_decoder_channel_assignment)
                .map_or(0, |labels| labels.len());
        self.declared_eightch_channels = ChannelLabel::from_eightch_channel(
            format_info.eightch_decoder_channel_assignment,
            major_sync.flags,
        )
        .map_or(0, |labels| labels.len());
    }

    /// Everything the parse measured but the stream does not state about itself.
    pub fn adopt_measurements(&mut self, parser: &Parser) {
        self.fifo_records = parser.fifo_depth_records();
        self.branches = parser.branches().to_vec();
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
    /// whole allowance. A substream whose span is empty carries no channel but still
    /// carries headers, and is allowed a flat 5000 bytes for them.
    pub fn substream_cap(min_chan: usize, max_chan: usize) -> usize {
        let Some(span) = max_chan.checked_sub(min_chan) else {
            return EMPTY_SUBSTREAM_CAP;
        };

        (15_000 * (span + 1)).min(120_000)
    }

    /// A sample position as seconds into the stream.
    fn sample_time(&self, sample: u64) -> Option<f64> {
        (self.sampling_frequency != 0).then(|| sample as f64 / self.sampling_frequency as f64)
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

    /// Channels a presentation reconstructs, taken from the substreams it is made of.
    fn carried_channels(&self, presentation: usize) -> usize {
        let map = PresentationMap::for_format_sync(
            self.format_sync.unwrap_or_default(),
            self.substream_info,
            self.extended_substream_info,
        );
        let mask = map.substream_mask_by_index(presentation);

        self.substream_facts
            .iter()
            .enumerate()
            .filter(|&(index, _)| mask >> index & 1 != 0)
            .map(|(_, substream)| substream.max_chan + 1)
            .max()
            .unwrap_or(0)
    }

    /// Whether any presentation's substreams fail to tile its channels: every presentation
    /// must run from channel 0 upwards with no channel left out and none claimed twice.
    fn channel_numbering_broken(&self) -> bool {
        let map = PresentationMap::for_format_sync(
            self.format_sync.unwrap_or_default(),
            self.substream_info,
            self.extended_substream_info,
        );

        (0..MAX_PRESENTATIONS).any(|presentation| {
            let mask = map.substream_mask_by_index(presentation);

            let mut spans: Vec<(usize, usize)> = self
                .substream_facts
                .iter()
                .enumerate()
                .filter(|&(index, _)| mask >> index & 1 != 0)
                .map(|(_, substream)| (substream.min_chan, substream.max_chan))
                .collect();
            spans.sort_unstable();

            let mut next = 0;
            spans.into_iter().any(|(min_chan, max_chan)| {
                let broken = min_chan != next;
                next = max_chan + 1;

                broken
            })
        })
    }

    /// Which disc formats the stream is legal for.
    ///
    /// The stream must restart substream 0 on 0x31EA and number its channels without a gap
    /// or an overlap, or it is legal for none of them. Beyond that the rules are the
    /// channel counts a decoder may be asked for at each sampling frequency: at 176.4 and
    /// 192 kHz, BluRay allows six channels and HD DVD-Video two, and both the declared
    /// assignment and the substreams themselves have to stay inside that. BluRay does not
    /// admit 44.1, 88.2 or 176.4 kHz at all, and takes no DVD-Audio stream.
    pub fn disc_validity(&self) -> Option<DiscValidity> {
        let format_sync = self.format_sync?;
        let mut validity = DiscValidity::default();

        if self.substream_facts.first()?.restart_sync_word != FIRST_RESTART_SYNC_WORD
            || self.channel_numbering_broken()
        {
            return Some(validity);
        }

        if format_sync == MAJOR_SYNC_FBB {
            // Whether the six-channel downmix clips decides DVD-Audio, and only a decode
            // can say. The rest of the rule is met, so the answer is stated as the
            // condition that remains.
            validity.dvd_audio = true;
            validity.dvd_audio_needs_decode = true;
            validity.hd_dvd_video = self.substream_info & 1 != 0;
        } else if !matches!(self.sampling_frequency, 176_400 | 192_000) {
            validity.hd_dvd_video = true;
            validity.bluray = true;
        } else {
            let counts = [
                self.declared_sixch_channels,
                self.declared_eightch_channels,
                self.carried_channels(1),
                self.carried_channels(2),
            ];

            validity.bluray = counts.iter().all(|&channels| channels <= 6);
            validity.hd_dvd_video = counts.iter().all(|&channels| channels <= 2);
        }

        if matches!(self.sampling_frequency, 44_100 | 88_200 | 176_400) {
            validity.bluray = false;
        }

        Some(validity)
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
    /// Conformant as a bitstream, but outside what any disc format admits, so it can be
    /// decoded and carried in a file and cannot be authored to a disc. The codec rules and
    /// the disc rules are different rules, and a stream can satisfy one without the other.
    ConformantOffDisc,
    /// Violated a rule at or above the severity the caller fails on.
    NonConformant,
}

impl Verdict {
    pub fn of(facts: &StreamFacts, tally: &Tally, fail_on: Severity, recovered: bool) -> Self {
        if facts.access_units == 0 || !recovered {
            Verdict::Unparseable
        } else if tally.worst().is_some_and(|worst| worst >= fail_on) {
            Verdict::NonConformant
        } else if facts
            .disc_validity()
            .is_some_and(|validity| !validity.any_format())
        {
            Verdict::ConformantOffDisc
        } else {
            Verdict::Conformant
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Verdict::Unparseable => "UNPARSEABLE",
            Verdict::Conformant => "CONFORMANT",
            Verdict::ConformantOffDisc => "CONFORMANT, NOT DISC-AUTHORABLE",
            Verdict::NonConformant => "NON-CONFORMANT",
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Verdict::Unparseable => "unparseable",
            Verdict::Conformant => "conformant",
            Verdict::ConformantOffDisc => "conformant_off_disc",
            Verdict::NonConformant => "non_conformant",
        }
    }

    pub const fn exit_code(self) -> i32 {
        match self {
            Verdict::Unparseable => crate::exit::PARSE,
            // The bitstream conforms, so the run succeeded. Being inadmissible on every
            // disc is reported, not failed: it is a property of the disc rules, not a
            // defect in the stream.
            Verdict::Conformant | Verdict::ConformantOffDisc => crate::exit::SUCCESS,
            Verdict::NonConformant => crate::exit::NONCONFORMANT,
        }
    }
}

/// One row of the summary: a label and the cells under it. A row with nothing to report
/// carries a note in place of its cells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub label: String,
    pub cells: Vec<String>,
    pub note: Option<String>,
}

impl Row {
    fn value(label: impl Into<String>, value: impl ToString) -> Self {
        Self {
            label: label.into(),
            cells: vec![value.to_string()],
            note: None,
        }
    }

    fn cells(label: impl Into<String>, cells: Vec<String>) -> Self {
        Self {
            label: label.into(),
            cells,
            note: None,
        }
    }

    fn note(label: impl Into<String>, note: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            cells: Vec::new(),
            note: Some(note.into()),
        }
    }
}

/// A titled block of rows. Headings make it a table, laid out in columns; without them the
/// rows are `label   value` lines, as the `info` command lays its own out.
///
/// The sections are the report: what it says lives here, and rendering them to text is a
/// separate step that cannot change any of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub title: String,
    pub headings: Vec<String>,
    pub rows: Vec<Row>,
}

impl Section {
    fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            headings: Vec::new(),
            rows: Vec::new(),
        }
    }

    fn with_headings(title: impl Into<String>, headings: Vec<String>) -> Self {
        Self {
            title: title.into(),
            headings,
            rows: Vec::new(),
        }
    }

    fn push(&mut self, row: Row) -> &mut Self {
        self.rows.push(row);
        self
    }
}

/// Everything the summary states, before any of it is laid out.
pub fn sections(input: &str, facts: &StreamFacts, tally: &Tally, verdict: Verdict) -> Vec<Section> {
    let mut sections = vec![stream_information(input, facts), measurements(facts)];

    if facts.format_sync.is_some() {
        sections.push(substream_properties(facts));
        sections.push(cumulative_fifo(facts));

        if let Some(validity) = facts.disc_validity() {
            sections.push(disc_validity(&validity));
        }
    }

    // A stream with no splice in it has nothing to say about branches.
    if !facts.branches.is_empty() {
        sections.push(branch_points(facts));
    }

    sections.push(diagnostics(tally, verdict));

    sections
}

fn render(sections: &[Section]) -> String {
    let mut out = String::new();

    for (index, section) in sections.iter().enumerate() {
        if index != 0 {
            out.push('\n');
        }

        out.push_str(&section.title);
        out.push('\n');

        // Columns are as wide as what goes in them plus a space, and never narrower than
        // COLUMN, so a value that fills its column cannot run into its neighbour.
        let widths: Vec<usize> = section
            .headings
            .iter()
            .enumerate()
            .map(|(index, heading)| {
                section
                    .rows
                    .iter()
                    .filter_map(|row| row.cells.get(index))
                    .chain([heading])
                    .map(|cell| cell.chars().count() + 1)
                    .max()
                    .unwrap_or(0)
                    .max(COLUMN)
            })
            .collect();

        if !section.headings.is_empty() {
            out.push_str(&format!("  {:<LABEL$}", ""));

            for (heading, width) in section.headings.iter().zip(&widths) {
                out.push_str(&format!("{heading:>width$}"));
            }

            out.push('\n');
        }

        for row in &section.rows {
            out.push_str(&format!("  {:<LABEL$}", row.label));

            match &row.note {
                // A note stands in for the cells, over the width they would have filled.
                Some(note) if !section.headings.is_empty() => {
                    out.push_str(&format!("{note:>width$}", width = widths.iter().sum()))
                }
                Some(note) => out.push_str(note),
                None if section.headings.is_empty() => {
                    out.push_str(row.cells.first().map_or("", String::as_str))
                }
                None => {
                    for (cell, width) in row.cells.iter().zip(&widths) {
                        out.push_str(&format!("{cell:>width$}"));
                    }
                }
            }

            out.push('\n');
        }
    }

    out
}

pub fn summary(input: &str, facts: &StreamFacts, tally: &Tally, verdict: Verdict) -> String {
    format!(
        "\nVerification Summary\n====================\n\n{}",
        render(&sections(input, facts, tally, verdict))
    )
}

fn stream_information(input: &str, facts: &StreamFacts) -> Section {
    let mut section = Section::new("Stream Information");
    section.push(Row::value("Input", input));

    match facts.format_sync {
        Some(format_sync) => {
            section
                .push(Row::value(
                    "Format Sync",
                    format_args!("{} ({format_sync:08X})", facts.format_name()),
                ))
                .push(Row::value(
                    "Sampling rate",
                    format_args!("{} Hz", facts.sampling_frequency),
                ))
                .push(Row::value("Number of substreams", facts.substreams))
                .push(Row::value(
                    "substream_info",
                    format_args!("{:#04X}", facts.substream_info),
                ));
        }
        None => {
            section.push(Row::value("Format Sync", "no major sync found"));
        }
    }

    section
}

fn measurements(facts: &StreamFacts) -> Section {
    let mut section = Section::new("Stream Measurements");
    section.push(Row::value("Access units", facts.access_units));

    if facts.extracted_frames != facts.access_units {
        section.push(Row::value("Frames extracted", facts.extracted_frames));
    }

    if let Some(secs) = facts.duration_secs() {
        section.push(Row::value("Duration", time_str(secs)));
    }

    section.push(match facts.average_access_unit_size() {
        Some(average) => Row::value(
            "Access unit size",
            format_args!(
                "{average:.2} bytes average, {} bytes maximum",
                facts.max_access_unit_size
            ),
        ),
        None => Row::value(
            "Access unit size",
            format_args!("{} bytes maximum", facts.max_access_unit_size),
        ),
    });

    if facts.max_data_rate != 0 {
        section.push(Row::value(
            "Maximum data rate",
            format_args!(
                "{:.1} kbps, at access unit {}",
                facts.max_data_rate as f64 / 1000.0,
                facts.max_data_rate_au
            ),
        ));
    }

    section.push(match facts.max_fifo_latency_ms() {
        Some(ms) => Row::value(
            "Maximum FIFO latency",
            format_args!("{} samples ({ms:.3} ms)", facts.max_fifo_latency),
        ),
        None => Row::value(
            "Maximum FIFO latency",
            format_args!("{} samples", facts.max_fifo_latency),
        ),
    });

    section
}

/// The per-substream view: what each substream carries, how deep its own bytes went and
/// what it is allowed on its own.
///
/// Transposed like the stream's own description, a column per substream, so that the
/// depths sit under the channel range that earns their allowance.
fn substream_properties(facts: &StreamFacts) -> Section {
    if facts.substream_facts.is_empty() {
        let mut section = Section::new("Substream Properties");
        section.push(Row::value(
            "Substreams",
            "no substream reached a restart header",
        ));

        return section;
    }

    let column = |render: &dyn Fn(&SubstreamFacts) -> String| {
        facts.substream_facts.iter().map(render).collect::<Vec<_>>()
    };

    let mut section = Section::with_headings(
        "Substream Properties",
        (0..facts.substream_facts.len())
            .map(|index| format!("ss{index}"))
            .collect(),
    );

    section
        .push(Row::cells(
            "Restart sync word",
            column(&|s| format!("{:04X}", s.restart_sync_word)),
        ))
        .push(Row::cells(
            "Channels",
            column(&|s| format!("{}..{}", s.min_chan, s.max_chan)),
        ))
        .push(Row::cells(
            "Max matrix channel",
            column(&|s| s.max_matrix_chan.to_string()),
        ))
        .push(Row::cells(
            "Maximum FIFO depth",
            column(&|s| s.fifo_peak.to_string()),
        ))
        .push(Row::cells(
            "Allowed FIFO depth",
            column(&|s| s.fifo_cap.to_string()),
        ));

    section
}

/// The cumulative view: one row per accumulator, with the stream's own bytes separated
/// from the container overhead priced around them.
fn cumulative_fifo(facts: &StreamFacts) -> Section {
    let caps = facts.fifo_caps();
    let mut section = Section::with_headings(
        "Cumulative FIFO Depth",
        ["stream", "overhead", "total", "allowed"]
            .map(str::to_owned)
            .to_vec(),
    );

    for (index, label) in FIFO_LABELS.iter().enumerate() {
        // A decoder the stream does not carry has nothing to say in any column.
        if index == 0
            && facts
                .substream0_channels()
                .is_some_and(|channels| channels != 2)
        {
            section.push(Row::cells(*label, vec!["-".to_owned(); 4]));

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
            section.push(Row::note(*label, reason));

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

        section.push(Row::cells(
            label,
            vec![
                record.stream.to_string(),
                record.overhead.to_string(),
                record.total.to_string(),
                caps[index].map_or_else(|| "not checked".to_owned(), |cap| cap.to_string()),
            ],
        ));
    }

    section
}

/// The disc formats the stream is legal for, one row each.
fn disc_validity(validity: &DiscValidity) -> Section {
    let yes_no = |legal: bool| if legal { "yes" } else { "no" };
    let mut section = Section::new("Disc Format Validity");

    section
        .push(
            match (validity.dvd_audio, validity.dvd_audio_needs_decode) {
                (true, true) => {
                    Row::value("DVD-Audio", "yes, if the 6-channel downmix does not clip")
                }
                (legal, _) => Row::value("DVD-Audio", yes_no(legal)),
            },
        )
        .push(Row::value("HD DVD-Video", yes_no(validity.hd_dvd_video)))
        .push(Row::value("BluRay", yes_no(validity.bluray)));

    section
}

/// Every point the stream's timing restarts at, which is what a splice leaves behind.
///
/// A branch is seamless only if a decoder starting there can play across it, so the row
/// carries the advance it asks for and, where one failed, which of the buffer-model
/// conditions it broke.
fn branch_points(facts: &StreamFacts) -> Section {
    let mut section = Section::with_headings(
        "Branch Points",
        ["access unit", "offset", "time", "advance", "status"]
            .map(str::to_owned)
            .to_vec(),
    );

    for (index, branch) in facts.branches.iter().enumerate() {
        section.push(Row::cells(
            format!("Branch {index}"),
            vec![
                branch.au_index.to_string(),
                format!("{:#X}", branch.byte_offset),
                facts
                    .sample_time(branch.sample)
                    .map_or_else(|| branch.sample.to_string(), time_str),
                branch.advance.to_string(),
                if branch.is_valid() {
                    "valid"
                } else {
                    "invalid"
                }
                .to_owned(),
            ],
        ));

        let failed = branch.conditions.failed();

        if !failed.is_empty() {
            section.push(Row::note("  failed", failed.join(", ")));
        }
    }

    let invalid = facts.branches.iter().filter(|b| !b.is_valid()).count();
    section.push(Row::note(
        "Total",
        format!("{} branches, {invalid} invalid", facts.branches.len()),
    ));

    section
}

fn diagnostics(tally: &Tally, verdict: Verdict) -> Section {
    let mut section = Section::new("Diagnostics");

    for severity in Severity::ALL.iter().rev() {
        let label = match severity {
            Severity::Fatal => "Fatal",
            Severity::Error => "Errors",
            Severity::Warning => "Warnings",
            Severity::Info => "Info",
        };

        section.push(Row::value(label, tally.count_of(*severity)));
    }

    section.push(match tally.worst() {
        Some(worst) => Row::value(
            "Verdict",
            format_args!("{} (worst: {worst})", verdict.label()),
        ),
        None => Row::value("Verdict", verdict.label()),
    });

    section
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
        "disc_validity": facts.disc_validity().map(|validity| json!({
            "dvd_audio": validity.dvd_audio,
            "dvd_audio_needs_decode": validity.dvd_audio_needs_decode,
            "hd_dvd_video": validity.hd_dvd_video,
            "bluray": validity.bluray,
        })),
        "branches": facts.branches.iter().map(|branch| json!({
            "au": branch.au_index,
            "byte_offset": branch.byte_offset,
            "sample": branch.sample,
            "advance": branch.advance,
            "valid": branch.is_valid(),
            "failed": branch.conditions.failed(),
        })).collect::<Vec<_>>(),
        "invalid_branches": facts.branches.iter().filter(|b| !b.is_valid()).count(),
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
    use truehd::process::extract::Extractor;
    use truehd::utils::diagnostic::Rule;

    use super::*;

    /// Four independent presentations in four substreams, so every accumulator sums a
    /// different set and the 6-channel one is a substream that carries its presentation
    /// whole.
    const FBA_ATMOS_CBI: &[u8] = include_bytes!("../../../truehd/tests/assets/fba_atmos_cbi.mlp");

    /// One FBA substream, so the 6- and 8-channel presentations are copies of the first.
    const FBA_2CH: &[u8] = include_bytes!("../../../truehd/tests/assets/fba_2ch.mlp");

    /// FBA at 192 kHz whose 8-channel presentation carries six channels, the only rate
    /// class where the disc rules count channels.
    const FBA_192K: &[u8] = include_bytes!("../../../truehd/tests/assets/fba_192k.mlp");

    /// Three independently scheduled clips butt-joined, so the stream restarts its timing
    /// twice. Neither join is seamless: the clips were scheduled to different peak rates
    /// and their arrival times do not meet across the seam.
    const FBA_SPLICED: &[u8] = include_bytes!("../../../truehd/tests/assets/fba_spliced.mlp");

    /// FBA at 176.4 kHz, two channels. BluRay does not admit the rate whatever the
    /// channel count, so it is the only asset that exercises that clause.
    const FBA_176K: &[u8] = include_bytes!("../../../truehd/tests/assets/fba_176k.mlp");

    /// FBA at 192 kHz carrying eight channels, which no disc format accepts.
    const FBA_192K_8CH: &[u8] = include_bytes!("../../../truehd/tests/assets/fba_192k_8ch.mlp");

    /// Three DVD-Audio clips butt-joined. FBB carries its own branch arm and nothing
    /// exercised it; the figures below are the measured ones for the same stream.
    const FBB_SPLICED: &[u8] = include_bytes!("../../../truehd/tests/assets/fbb_spliced.mlp");

    /// FBB over two substreams, `substream_info` 0x0D.
    const FBB_6CH: &[u8] = include_bytes!("../../../truehd/tests/assets/fbb_6ch.mlp");

    /// FBB carrying all six channels in substream 0, `substream_info` 0x04, so the stream
    /// has no two-channel decoder at all.
    const FBB_6CH_SINGLE: &[u8] = include_bytes!("../../../truehd/tests/assets/fbb_6ch_single.mlp");

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

    /// The summary as sections, which is what a test should be looking at: the labels and
    /// the values are the contract, and the layout is the renderer's business.
    fn report(facts: &StreamFacts) -> Vec<Section> {
        sections("x.mlp", facts, &Tally::default(), Verdict::Conformant)
    }

    /// One row, named by its section and label, with a failure that says which of the two
    /// went missing rather than only that some string was absent.
    fn row<'a>(sections: &'a [Section], title: &str, label: &str) -> &'a Row {
        let section = sections
            .iter()
            .find(|section| section.title == title)
            .unwrap_or_else(|| {
                let titles: Vec<&str> = sections.iter().map(|s| s.title.as_str()).collect();
                panic!("no section {title:?}, have {titles:?}")
            });

        section
            .rows
            .iter()
            .find(|row| row.label == label)
            .unwrap_or_else(|| {
                let labels: Vec<&str> = section.rows.iter().map(|r| r.label.as_str()).collect();
                panic!("no row {label:?} in {title:?}, have {labels:?}")
            })
    }

    /// The cells of one row, for comparison against a literal array.
    fn cells<'a>(sections: &'a [Section], title: &str, label: &str) -> Vec<&'a str> {
        row(sections, title, label)
            .cells
            .iter()
            .map(String::as_str)
            .collect()
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
        assert_eq!(StreamFacts::substream_cap(0, 1), 30_000);
        assert_eq!(StreamFacts::substream_cap(2, 5), 60_000);

        // cumulative: four independent presentations, each spanning from channel 0
        assert_eq!(StreamFacts::substream_cap(0, 5), 90_000);
        assert_eq!(StreamFacts::substream_cap(0, 7), 120_000);
        // sixteen channels would ask 240000; the ceiling holds it at 120000
        assert_eq!(StreamFacts::substream_cap(0, 15), 120_000);

        // an empty span carries no channel, and is allowed its headers alone
        assert_eq!(StreamFacts::substream_cap(6, 5), 5_000);
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

        let report = sections("x.mlp", &facts(), &tally, Verdict::Conformant);
        assert_eq!(cells(&report, "Diagnostics", "Verdict"), ["CONFORMANT"]);
        assert_eq!(cells(&report, "Diagnostics", "Fatal"), ["0"]);
    }

    /// Every measurement the report gained has a line of its own.
    /// The one test that looks at the layout, so that a renderer change is caught here
    /// rather than in every test that happens to mention a row. Everything else asserts
    /// against the sections, which carry no widths at all.
    #[test]
    fn the_renderer_lays_a_section_out_in_columns() {
        let mut section =
            Section::with_headings("Example", ["stream", "total"].map(str::to_owned).to_vec());
        section
            .push(Row::cells(
                "2-channel decoder",
                vec!["240".into(), "294".into()],
            ))
            .push(Row::note("8-channel decoder", "not checked"));

        let mut field = Section::new("Fields");
        field.push(Row::value("Sampling rate", "48000 Hz"));

        assert_eq!(
            render(&[section, field]),
            concat!(
                "Example\n",
                "                                   stream      total\n",
                "  2-channel decoder                   240        294\n",
                "  8-channel decoder                      not checked\n",
                "\n",
                "Fields\n",
                "  Sampling rate               48000 Hz\n",
            )
        );
    }

    #[test]
    fn the_measurements_reach_the_report() {
        let report = report(&facts());

        assert_eq!(
            cells(&report, "Stream Measurements", "Maximum data rate"),
            ["3840.0 kbps, at access unit 0"]
        );
        assert_eq!(
            cells(&report, "Stream Measurements", "Maximum FIFO latency"),
            ["56 samples (1.167 ms)"]
        );
        assert_eq!(
            cells(&report, "Stream Measurements", "Access unit size"),
            ["252.46 bytes average, 556 bytes maximum"]
        );
    }

    /// The per-substream table stands beside the cumulative one, with each substream's
    /// own depth and its own allowance.
    #[test]
    fn the_substream_table_reports_a_column_per_substream() {
        let report = report(&facts());

        assert_eq!(
            cells(&report, "Substream Properties", "Restart sync word"),
            ["31EA", "31EB", "31EB", "31EC"]
        );
        assert_eq!(
            cells(&report, "Substream Properties", "Channels"),
            ["0..1", "0..5", "0..7", "0..15"]
        );
        assert_eq!(
            cells(&report, "Substream Properties", "Maximum FIFO depth"),
            ["240", "246", "250", "274"]
        );
        assert_eq!(
            cells(&report, "Substream Properties", "Allowed FIFO depth"),
            ["30000", "90000", "120000", "120000"]
        );
    }

    /// The cumulative rows split into stream and overhead, and the two add back up.
    #[test]
    fn the_fifo_table_separates_the_stream_from_its_overhead() {
        let facts = facts();
        let report = report(&facts);

        assert_eq!(
            cells(&report, "Cumulative FIFO Depth", "2-channel decoder"),
            ["240", "54", "294", "30000"]
        );
        assert_eq!(
            cells(&report, "Cumulative FIFO Depth", "Whole stream"),
            ["1010", "72", "1082", "120000"]
        );

        // Substream 1 carries the whole 6-channel presentation here, so that row is its
        // own bytes and not substream 0's as well.
        assert_eq!(
            cells(&report, "Cumulative FIFO Depth", "6-channel decoder"),
            ["246", "52", "298", "90000"]
        );
        assert_eq!(
            facts.fifo_records[1].stream,
            facts.substream_facts[1].fifo_peak
        );
    }

    /// The disc rules only count channels at 176.4 and 192 kHz. Below that an FBA stream
    /// is legal for both video formats whatever it carries, and DVD-Audio takes no FBA
    /// stream at all.
    #[test]
    fn an_fba_stream_below_the_high_rates_is_legal_for_both_video_formats() {
        let validity = facts().disc_validity().unwrap();

        assert_eq!(
            validity,
            DiscValidity {
                dvd_audio: false,
                hd_dvd_video: true,
                bluray: true,
                dvd_audio_needs_decode: false,
            }
        );

        let report = report(&facts());
        assert_eq!(
            cells(&report, "Disc Format Validity", "HD DVD-Video"),
            ["yes"]
        );
        assert_eq!(cells(&report, "Disc Format Validity", "BluRay"), ["yes"]);
    }

    /// At 192 kHz BluRay allows six channels and HD DVD-Video two. This stream carries
    /// six, so it keeps BluRay and loses HD DVD-Video.
    #[test]
    fn six_channels_at_the_high_rates_keep_bluray_and_lose_hd_dvd() {
        let facts = facts_of(FBA_192K);
        assert_eq!(facts.sampling_frequency, 192_000);
        assert_eq!(facts.carried_channels(2), 6);

        let validity = facts.disc_validity().unwrap();
        assert!(validity.bluray);
        assert!(!validity.hd_dvd_video);
        assert!(!validity.dvd_audio);
    }

    /// BluRay admits neither 44.1, 88.2 nor 176.4 kHz, so a two-channel stream that clears
    /// every channel-count rule still loses BluRay on its sampling frequency alone.
    #[test]
    fn bluray_refuses_the_rates_it_does_not_carry() {
        let facts = facts_of(FBA_176K);
        assert_eq!(facts.sampling_frequency, 176_400);
        assert_eq!(facts.carried_channels(2), 2);

        let validity = facts.disc_validity().unwrap();
        assert!(validity.hd_dvd_video, "two channels satisfy HD DVD-Video");
        assert!(!validity.bluray, "the rate alone disqualifies BluRay");
    }

    /// Eight channels at 192 kHz pass the limit of no disc format, so the stream is legal
    /// for none of them. Its bitstream is still well formed, which the verdict reports
    /// separately: conformance and disc legality are different questions.
    #[test]
    fn eight_channels_at_the_high_rates_are_legal_nowhere() {
        let facts = facts_of(FBA_192K_8CH);
        assert_eq!(facts.sampling_frequency, 192_000);
        assert_eq!(facts.carried_channels(2), 8);
        assert_eq!(facts.declared_eightch_channels, 8);

        assert_eq!(facts.disc_validity().unwrap(), DiscValidity::default());

        let report = report(&facts);
        for label in ["DVD-Audio", "HD DVD-Video", "BluRay"] {
            assert_eq!(cells(&report, "Disc Format Validity", label), ["no"]);
        }
    }

    /// A stream no disc format admits is still a conformant bitstream, so it gets a
    /// verdict of its own rather than an error: it decodes, it just cannot be authored.
    /// A stream one format does admit is plainly conformant.
    #[test]
    fn a_stream_no_disc_format_admits_is_conformant_but_not_authorable() {
        let tally = Tally::default();

        let verdict = Verdict::of(&facts_of(FBA_192K_8CH), &tally, Severity::Error, true);
        assert_eq!(verdict, Verdict::ConformantOffDisc);
        assert_eq!(verdict.exit_code(), crate::exit::SUCCESS);

        let report = sections(
            "x.mlp",
            &facts_of(FBA_192K_8CH),
            &tally,
            Verdict::ConformantOffDisc,
        );
        assert_eq!(
            cells(&report, "Diagnostics", "Verdict"),
            ["CONFORMANT, NOT DISC-AUTHORABLE"]
        );

        // BluRay takes the six-channel stream at the same rate, so it is plain conformant
        assert_eq!(
            Verdict::of(&facts_of(FBA_192K), &tally, Severity::Error, true),
            Verdict::Conformant
        );

        // and a codec-level violation still outranks the disc question
        let mut broken = Tally::default();
        broken.record("fifo.underrun", Severity::Error, 3, 0);
        assert_eq!(
            Verdict::of(&facts_of(FBA_192K_8CH), &broken, Severity::Error, true),
            Verdict::NonConformant
        );
    }

    /// A DVD-Audio stream is never legal for BluRay, and its own verdict rests on a
    /// clipping check no parse can make, so it is stated as the condition it leaves open.
    #[test]
    fn a_dvd_audio_stream_states_the_condition_a_parse_cannot_settle() {
        let report = report(&facts_of(FBB_6CH));

        assert_eq!(
            cells(&report, "Disc Format Validity", "DVD-Audio"),
            ["yes, if the 6-channel downmix does not clip"]
        );
        assert_eq!(cells(&report, "Disc Format Validity", "BluRay"), ["no"]);

        // Bit 0 of substream_info decides HD DVD-Video: 0x0D carries it, 0x04 does not.
        assert!(facts_of(FBB_6CH).disc_validity().unwrap().hd_dvd_video);
        assert!(
            !facts_of(FBB_6CH_SINGLE)
                .disc_validity()
                .unwrap()
                .hd_dvd_video
        );
    }

    /// A spliced stream reports each branch with its place and the conditions it met, and
    /// a stream with no splice in it reports no section at all rather than a zero. The
    /// depths and rates either side of the seam are exact, which they only are because a
    /// rejected branch restarts the model.
    #[test]
    fn a_spliced_stream_reports_every_branch_point() {
        let spliced = facts_of(FBA_SPLICED);
        assert_eq!(spliced.branches.len(), 2);
        assert_eq!(spliced.branches.iter().filter(|b| !b.is_valid()).count(), 2);

        let report = report(&spliced);
        assert_eq!(
            cells(&report, "Branch Points", "Branch 0"),
            ["44", "0x13B4", "00:00:00.036", "3527", "invalid"]
        );

        // The figures for this stream, with branch tolerance on.
        assert_eq!(spliced.fifo_records[0].total, 6996);
        assert_eq!(spliced.fifo_records[4].total, 7904);
        assert_eq!(spliced.substream_facts[0].fifo_peak, 6538);
        assert_eq!(spliced.max_data_rate_au, 350);
        assert_eq!(cells(&report, "Branch Points", "Branch 1")[0], "344");

        // The conditions that failed are named, so a reader can see which bound was broken.
        assert_eq!(
            spliced.branches[0].conditions.failed(),
            ["advance step", "FIFO duration"]
        );
        assert_eq!(
            spliced.branches[1].conditions.failed(),
            ["advance step", "FIFO duration"]
        );

        let unspliced = sections("x.mlp", &facts(), &Tally::default(), Verdict::Conformant);
        assert!(
            !unspliced.iter().any(|s| s.title == "Branch Points"),
            "an unspliced stream has no branches to report"
        );
    }

    /// A stream can be longer than 4 GB, so a branch offset is a 64-bit value and its
    /// column is sized from what it holds. Padding it to a fixed eight digits would have
    /// invented leading zeros below 4 GB and stopped being uniform above it.
    #[test]
    fn a_branch_past_four_gigabytes_keeps_its_offset_and_its_column() {
        let mut facts = facts_of(FBA_SPLICED);
        let far = 0x1_2345_6789u64;
        facts.branches[1].byte_offset = far;

        let report = report(&facts);
        assert_eq!(
            cells(&report, "Branch Points", "Branch 1")[1],
            "0x123456789"
        );
        assert_eq!(cells(&report, "Branch Points", "Branch 0")[1], "0x13B4");

        // The renderer sizes the column from the widest cell, so the two offsets still end
        // in the same column: the wide one is not truncated and the narrow one is padded.
        let rendered = render(&report);
        let end_of = |needle: &str| {
            let line = rendered
                .lines()
                .find(|line| line.contains(needle))
                .unwrap_or_else(|| panic!("no rendered row holding {needle}"));
            line.find(needle).unwrap() + needle.len()
        };

        assert_eq!(end_of("0x123456789"), end_of("0x13B4"));
    }

    /// A DVD-Audio stream branches by the same rules, and its depths hold as the TrueHD
    /// ones do.
    #[test]
    fn a_dvd_audio_splice_branches_like_a_truehd_one() {
        let facts = facts_of(FBB_SPLICED);
        assert_eq!(facts.substream_info, 0x0D);
        assert_eq!(facts.branches.len(), 2);
        assert!(facts.branches.iter().all(|branch| !branch.is_valid()));

        assert_eq!(facts.substream_facts[0].fifo_peak, 4204);
        assert_eq!(facts.substream_facts[1].fifo_peak, 2220);
        assert_eq!(facts.fifo_records[0].total, 4638);
        assert_eq!(facts.fifo_records[4].total, 6982);
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

        let stated = |verdict| {
            let report = sections("x.mlp", &facts(), &tally, verdict);
            cells(&report, "Diagnostics", "Verdict")[0].to_owned()
        };
        assert_eq!(
            stated(Verdict::NonConformant),
            "NON-CONFORMANT (worst: warning)"
        );
        assert_eq!(stated(Verdict::Conformant), "CONFORMANT (worst: warning)");
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
        let report = sections(
            "x.mlp",
            &StreamFacts::default(),
            &Tally::default(),
            Verdict::Unparseable,
        );

        assert_eq!(
            cells(&report, "Stream Information", "Format Sync"),
            ["no major sync found"]
        );
        assert_eq!(cells(&report, "Diagnostics", "Verdict"), ["UNPARSEABLE"]);
        assert!(
            !report.iter().any(|section| section.title.contains("FIFO")),
            "{report:?}"
        );
    }

    /// A single-substream stream declares its 6-channel presentation as a copy of
    /// presentation 0, so that decoder reads substream 0 alone. The row says so, and
    /// carries substream 0's own figure rather than a zero.
    #[test]
    fn the_sixch_row_says_so_when_the_presentation_is_a_copy() {
        let facts = facts_of(FBA_2CH);

        let report = report(&facts);
        assert_eq!(
            cells(
                &report,
                "Cumulative FIFO Depth",
                "6-channel decoder (ss0 only)"
            ),
            ["2210", "174", "2384", "90000"]
        );
        assert_eq!(
            row(&report, "Cumulative FIFO Depth", "16-channel decoder")
                .note
                .as_deref(),
            Some("no 16-channel presentation")
        );
    }

    /// An FBB stream may carry its whole six-channel presentation in substream 0. There is
    /// then no two-channel decoder to report, and substream 0's bytes belong to the
    /// six-channel row.
    #[test]
    fn a_six_channel_substream_zero_leaves_no_two_channel_decoder() {
        let facts = facts_of(FBB_6CH_SINGLE);
        assert_eq!(facts.substream_info, 0x04);
        assert_eq!(facts.substream0_channels(), Some(6));

        let report = report(&facts);
        assert_eq!(
            cells(&report, "Cumulative FIFO Depth", "2-channel decoder"),
            ["-", "-", "-", "-"]
        );
        assert_eq!(
            cells(
                &report,
                "Cumulative FIFO Depth",
                "6-channel decoder (ss0 only)"
            ),
            ["44", "46", "90", "90000"]
        );
    }

    /// FBB accumulates neither the 8- nor the 16-channel sum, and the rows must say so
    /// rather than print the zeros the accumulators legitimately hold.
    #[test]
    fn the_fbb_rows_the_format_never_sums_are_not_reported_as_zero() {
        let facts = facts_of(FBB_6CH);
        assert_eq!(facts.substream_info, 0x0D);

        let report = report(&facts);

        for label in ["8-channel decoder", "16-channel decoder"] {
            let row = row(&report, "Cumulative FIFO Depth", label);
            assert_eq!(row.note.as_deref(), Some("not checked"));
            assert!(row.cells.is_empty(), "a note replaces the cells");
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
        assert_eq!(
            value["substream_properties"][0]["restart_sync_word"],
            "31EA"
        );
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
