//! Per-rule counting and print rate limiting.
//!
//! Every diagnostic is counted; only printing is limited. The exit code is keyed to what
//! was counted, so raising or lowering `--max-per-rule` can never change it.

use std::collections::BTreeMap;

use super::severity::Severity;

/// What one rule did over the whole stream.
#[derive(Debug)]
pub struct RuleTally {
    /// Times the rule fired.
    pub count: u64,
    /// Times it was printed.
    pub shown: u64,
    /// Worst severity it fired at.
    pub worst: Severity,
    pub first_au: u64,
    pub last_au: u64,
    /// Access units it fired in, counting each one once.
    pub access_units: u64,
}

impl RuleTally {
    pub const fn suppressed(&self) -> u64 {
        self.count - self.shown
    }
}

/// Counts of every rule that fired, keyed by rule ID so the report is ordered.
#[derive(Debug, Default)]
pub struct Tally {
    rules: BTreeMap<String, RuleTally>,
    by_severity: [u64; Severity::ALL.len()],
    worst: Option<Severity>,
    total: u64,
}

impl Tally {
    /// Counts one diagnostic and answers whether it should be printed.
    ///
    /// `max_per_rule` of 0 prints every occurrence.
    pub fn record(&mut self, rule: &str, severity: Severity, au: u64, max_per_rule: u64) -> bool {
        self.total += 1;
        self.by_severity[severity as usize] += 1;
        self.worst = Some(self.worst.map_or(severity, |worst| worst.max(severity)));

        let tally = self.rules.entry(rule.to_owned()).or_insert(RuleTally {
            count: 0,
            shown: 0,
            worst: severity,
            first_au: au,
            last_au: au,
            access_units: 0,
        });

        // Diagnostics arrive in access unit order, so a new access unit is one that
        // differs from the last counted.
        if tally.count == 0 || tally.last_au != au {
            tally.access_units += 1;
        }

        tally.count += 1;
        tally.last_au = au;
        tally.worst = tally.worst.max(severity);

        let show = max_per_rule == 0 || tally.shown < max_per_rule;
        if show {
            tally.shown += 1;
        }

        show
    }

    pub fn total(&self) -> u64 {
        self.total
    }

    pub fn worst(&self) -> Option<Severity> {
        self.worst
    }

    pub fn count_of(&self, severity: Severity) -> u64 {
        self.by_severity[severity as usize]
    }

    pub fn rules(&self) -> impl Iterator<Item = (&str, &RuleTally)> {
        self.rules
            .iter()
            .map(|(rule, tally)| (rule.as_str(), tally))
    }

    /// Rules that fired more often than they were printed.
    pub fn suppressed(&self) -> impl Iterator<Item = (&str, &RuleTally)> {
        self.rules().filter(|(_, tally)| tally.suppressed() > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printing_stops_at_the_limit_but_counting_does_not() {
        let mut tally = Tally::default();

        let shown = (0..50)
            .filter(|au| tally.record("a.b", Severity::Error, *au, 20))
            .count();

        assert_eq!(shown, 20);

        let (_, rule) = tally.rules().next().unwrap();
        assert_eq!(rule.count, 50);
        assert_eq!(rule.shown, 20);
        assert_eq!(rule.suppressed(), 30);
        assert_eq!(tally.total(), 50);
        assert_eq!(tally.count_of(Severity::Error), 50);
    }

    #[test]
    fn a_zero_limit_prints_everything() {
        let mut tally = Tally::default();

        let shown = (0..50)
            .filter(|au| tally.record("a.b", Severity::Error, *au, 0))
            .count();

        assert_eq!(shown, 50);
        assert!(tally.suppressed().next().is_none());
    }

    /// One rule firing 5000 times in one access unit is a different stream from one
    /// firing once in each of 5000, so the distinct count is reported separately.
    #[test]
    fn repeats_within_one_access_unit_count_once() {
        let mut tally = Tally::default();

        for _ in 0..10 {
            tally.record("a.b", Severity::Warning, 4, 0);
        }
        tally.record("a.b", Severity::Warning, 9, 0);

        let (_, rule) = tally.rules().next().unwrap();
        assert_eq!(rule.count, 11);
        assert_eq!(rule.access_units, 2);
        assert_eq!((rule.first_au, rule.last_au), (4, 9));
    }

    #[test]
    fn the_worst_severity_is_kept_per_rule_and_overall() {
        let mut tally = Tally::default();

        tally.record("a.b", Severity::Info, 0, 0);
        tally.record("a.b", Severity::Error, 1, 0);
        tally.record("a.b", Severity::Warning, 2, 0);
        tally.record("c.d", Severity::Warning, 3, 0);

        assert_eq!(tally.worst(), Some(Severity::Error));

        let worst: Vec<_> = tally.rules().map(|(_, tally)| tally.worst).collect();
        assert_eq!(worst, vec![Severity::Error, Severity::Warning]);
    }

    /// Suppression is a display choice; the exit code reads the counts.
    #[test]
    fn suppression_does_not_change_the_worst_severity() {
        let mut limited = Tally::default();
        let mut unlimited = Tally::default();

        for au in 0..100 {
            let severity = if au == 99 {
                Severity::Fatal
            } else {
                Severity::Info
            };
            limited.record("a.b", severity, au, 1);
            unlimited.record("a.b", severity, au, 0);
        }

        assert_eq!(limited.worst(), unlimited.worst());
        assert_eq!(limited.total(), unlimited.total());
    }
}
