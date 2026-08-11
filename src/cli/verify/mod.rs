mod report;
mod severity;
mod tally;

use std::io::{BufWriter, Stdout, Write};

use anyhow::Result;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use truehd::process::MAX_PRESENTATIONS;
use truehd::process::extract::{Extractor, Frame};
use truehd::process::parse::Parser;
use truehd::utils::diagnostic::{Diagnostic, DiagnosticMode, Location, Rule};
use truehd::utils::errors::ExtractError;

use self::report::{StreamFacts, Verdict};
use self::tally::Tally;
use super::command::{Cli, VerifyArgs};
use crate::exit::ExitError;
use crate::input::InputReader;

pub use self::severity::Severity;

pub fn cmd_verify(args: &VerifyArgs, cli: &Cli, multi: Option<&MultiProgress>) -> Result<()> {
    log::info!("Verifying TrueHD stream: {}", args.input.display());

    let progress = multi.map(spinner).transpose()?;

    let mut verification = Verification {
        args,
        fail_on: severity::fail_on(args.fail_on, cli.strict),
        tally: Tally::default(),
        facts: StreamFacts::default(),
        unrecovered_fatal: false,
        emitter: Emitter {
            out: BufWriter::new(std::io::stdout()),
            progress: progress.as_ref(),
        },
    };

    verification.run()?;
    verification.finish()
}

fn spinner(multi: &MultiProgress) -> Result<ProgressBar> {
    let progress = multi.add(ProgressBar::new_spinner());
    progress.set_style(ProgressStyle::with_template("{spinner:.green} {msg}")?);
    progress.enable_steady_tick(std::time::Duration::from_millis(100));
    progress.set_message("Verifying access units...");

    Ok(progress)
}

/// Report output, kept off the log so it survives `--loglevel off`.
struct Emitter<'a> {
    out: BufWriter<Stdout>,
    progress: Option<&'a ProgressBar>,
}

impl Emitter<'_> {
    fn line(&mut self, text: &str) -> Result<()> {
        match self.progress {
            // The bar owns the terminal while it is drawn.
            Some(progress) => {
                self.out.flush()?;
                progress.suspend(|| writeln!(self.out, "{text}"))?;
            }
            None => writeln!(self.out, "{text}")?,
        }

        Ok(())
    }
}

struct Verification<'a> {
    args: &'a VerifyArgs,
    fail_on: Severity,
    tally: Tally,
    facts: StreamFacts,
    /// A fatal check fired and no access unit has parsed since.
    unrecovered_fatal: bool,
    emitter: Emitter<'a>,
}

impl Verification<'_> {
    fn run(&mut self) -> Result<()> {
        let mut input = InputReader::new(&self.args.input).map_err(|source| ExitError {
            code: crate::exit::INPUT,
            source,
        })?;

        let mut extractor = Extractor::default();
        let mut parser = Parser::default();

        parser.set_diagnostic_mode(DiagnosticMode::Collect);
        // Every presentation, and the loosest fail level, so the parse reaches as much of
        // the stream as it can. What counts as a failure is the caller's threshold alone.
        parser.set_required_presentations(&[true; MAX_PRESENTATIONS]);
        parser.set_fail_level(log::Level::Error);

        // Where the extractor had reached, for the diagnostics that have no access unit.
        let mut extracted_end = 0;

        let read = input.process_chunks(64 * 1024, |chunk| {
            extractor.push_bytes(chunk);

            for frame in extractor.by_ref() {
                match frame {
                    Ok(frame) => {
                        extracted_end = frame.offset + frame.data.len() as u64;
                        self.access_unit(&frame, &mut parser)?;
                    }
                    // A chunk boundary, not a defect.
                    Err(ExtractError::InsufficientData) => break,
                    Err(error) => self.extract_failure(&error, extracted_end)?,
                }
            }

            Ok(true)
        });

        read.map_err(|source| ExitError {
            code: crate::exit::INPUT,
            source,
        })?;

        self.facts.adopt_measurements(&parser);

        Ok(())
    }

    fn access_unit(&mut self, frame: &Frame, parser: &mut Parser) -> Result<()> {
        self.facts.extracted_frames += 1;

        let access_unit = parser.parse_recovering(frame);
        let diagnostics = parser.take_diagnostics();

        // The check that ended the access unit is the last one recorded for it, and it is
        // the only kind of violation the parser cannot read past.
        let fatal = if access_unit.is_none() {
            diagnostics.len().checked_sub(1)
        } else {
            None
        };

        for (index, diagnostic) in diagnostics.iter().enumerate() {
            let severity = if Some(index) == fatal {
                Severity::Fatal
            } else {
                Severity::from_level(diagnostic.severity)
            };

            self.report(diagnostic, severity)?;
        }

        if let Some(access_unit) = access_unit {
            self.facts.access_units += 1;
            self.unrecovered_fatal = false;

            if let Some(major_sync) = &access_unit.major_sync_info {
                self.facts.adopt_major_sync(major_sync);
            }
        } else if fatal.is_some() {
            self.unrecovered_fatal = true;
        }

        if let Some(progress) = self.emitter.progress
            && self.facts.extracted_frames.is_multiple_of(100)
        {
            progress.set_message(format!(
                "Verifying access units...  {}",
                self.facts.extracted_frames
            ));
        }

        Ok(())
    }

    /// A frame the extractor could not hand over. It has no access unit of its own, so it
    /// is located at the end of the last one that extracted.
    fn extract_failure(&mut self, error: &ExtractError, offset: u64) -> Result<()> {
        let diagnostic = Diagnostic {
            rule: error.rule_id(),
            severity: log::Level::Error,
            location: Location {
                au_index: self.facts.extracted_frames,
                au_offset: offset,
                bit_offset: None,
            },
            message: error.to_string(),
            source: None,
        };

        self.report(&diagnostic, Severity::Fatal)
    }

    fn report(&mut self, diagnostic: &Diagnostic, severity: Severity) -> Result<()> {
        let show = self.tally.record(
            &diagnostic.rule.to_string(),
            severity,
            diagnostic.location.au_index,
            self.args.max_per_rule,
        );

        if !show || self.args.summary_only {
            return Ok(());
        }

        let line = if self.args.json {
            report::diagnostic_json(diagnostic, severity)
        } else {
            report::diagnostic_lines(diagnostic, severity)
        };

        self.emitter.line(&line)
    }

    fn finish(&mut self) -> Result<()> {
        if !self.args.summary_only {
            let lines: Vec<String> = self
                .tally
                .suppressed()
                .map(|(rule, tally)| {
                    if self.args.json {
                        report::suppressed_json(rule, tally)
                    } else {
                        report::suppressed_line(rule, tally)
                    }
                })
                .collect();

            if !lines.is_empty() && !self.args.json {
                self.emitter.line("")?;
            }

            for line in lines {
                self.emitter.line(&line)?;
            }
        }

        let verdict = Verdict::of(
            &self.facts,
            &self.tally,
            self.fail_on,
            !self.unrecovered_fatal,
        );

        let input = self.args.input.display().to_string();
        let summary = if self.args.json {
            report::summary_json(&input, &self.facts, &self.tally, verdict)
        } else {
            report::summary(&input, &self.facts, &self.tally, verdict)
        };
        self.emitter.line(&summary)?;

        if let Some(progress) = self.emitter.progress {
            progress.finish_and_clear();
        }
        self.emitter.out.flush()?;

        self.exit_with(verdict)
    }

    /// The verdict as an error, so its exit code travels the path every other one does.
    fn exit_with(&self, verdict: Verdict) -> Result<()> {
        let source = match verdict {
            Verdict::Conformant | Verdict::ConformantOffDisc => return Ok(()),
            Verdict::Unparseable if self.facts.access_units == 0 => {
                anyhow::anyhow!("no access unit in the stream could be parsed")
            }
            Verdict::Unparseable => anyhow::anyhow!("the stream could not be parsed to its end"),
            Verdict::NonConformant => anyhow::anyhow!(
                "stream is non-conformant: {} diagnostics, worst {}",
                self.tally.total(),
                self.tally.worst().expect("a verdict counted diagnostics"),
            ),
        };

        Err(ExitError {
            code: verdict.exit_code(),
            source,
        }
        .into())
    }
}
