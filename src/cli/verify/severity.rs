//! The four levels `verify` reports at.

use clap::ValueEnum;

/// How bad a diagnostic is, worst last.
///
/// The library carries a [`log::Level`], which cannot express the difference between a
/// violation that ends the access unit and one the parser reads past. `verify` needs that
/// difference, so it keeps its own scale and derives the rest from the level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum Severity {
    /// Observational, or a positive result.
    Info,
    /// Outside what a conformant encoder emits, but not provably a violation.
    Warning,
    /// A conformance violation. Decoding may still succeed.
    Error,
    /// The access unit could not be parsed past this point.
    Fatal,
}

impl Severity {
    /// Every level, worst last.
    pub const ALL: [Severity; 4] = [
        Severity::Info,
        Severity::Warning,
        Severity::Error,
        Severity::Fatal,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Error => "error",
            Severity::Fatal => "fatal",
        }
    }

    /// Plural used in the summary tally.
    pub const fn plural(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warning => "warnings",
            Severity::Error => "errors",
            Severity::Fatal => "fatal",
        }
    }

    /// The level a check reports at, for a check the parser read past.
    ///
    /// A check that ended the access unit is [`Severity::Fatal`] whatever it reports at,
    /// so that case never comes through here.
    pub const fn from_level(level: log::Level) -> Self {
        match level {
            log::Level::Error => Severity::Error,
            log::Level::Warn => Severity::Warning,
            _ => Severity::Info,
        }
    }
}

/// The threshold in force.
///
/// The global `--strict` means "fail on a warning", which is what `--fail-on warning`
/// says. It does not lower the parser's fail level here: that would end the access unit
/// on the first warning and hide everything after it, which is the opposite of what
/// `verify` is for.
pub fn fail_on(requested: Severity, strict: bool) -> Severity {
    if strict {
        requested.min(Severity::Warning)
    } else {
        requested
    }
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severities_order_worst_last() {
        let mut all = Severity::ALL;
        all.reverse();
        all.sort_unstable();

        assert_eq!(all, Severity::ALL);
        assert_eq!(Severity::ALL.iter().max(), Some(&Severity::Fatal));
    }

    #[test]
    fn log_levels_below_warn_are_all_observational() {
        assert_eq!(Severity::from_level(log::Level::Error), Severity::Error);
        assert_eq!(Severity::from_level(log::Level::Warn), Severity::Warning);

        for level in [log::Level::Info, log::Level::Debug, log::Level::Trace] {
            assert_eq!(Severity::from_level(level), Severity::Info);
        }
    }

    /// `--strict` tightens the threshold and never loosens one already tighter.
    #[test]
    fn strict_is_fail_on_warning() {
        assert_eq!(fail_on(Severity::Error, true), Severity::Warning);
        assert_eq!(fail_on(Severity::Fatal, true), Severity::Warning);
        assert_eq!(fail_on(Severity::Info, true), Severity::Info);

        for requested in Severity::ALL {
            assert_eq!(fail_on(requested, false), requested);
        }
    }
}
