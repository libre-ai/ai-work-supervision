//! `ws-journal-verify <journal-file> [--expected <expected-state-file>]` —
//! verifies a Work Supervision v0 journal, and with `--expected` compares it
//! with artefacts held outside it.
//!
//! Exit status: 0 valid (and consistent with the expectation when one is
//! given), 1 invalid, 2 unreadable or wrong usage (an unreadable or refused
//! expectation included), 3 torn tail, 4 diverged from the expectation.
//! Output names codes, line numbers, counts and the identifiers the
//! expectation supplied; it never echoes journal content.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io;
use std::process::ExitCode;

use work_supervision_journal_verifier::{
    Comparison, Expectation, ExpectedOutcome, Outcome, verify, verify_expected,
};

const USAGE: u8 = 2;
const DIVERGED: u8 = 4;
const USAGE_TEXT: &str =
    "usage: ws-journal-verify <journal-file> [--expected <expected-state-file>]";

fn report(mut stream: impl io::Write, message: &str, status: u8) -> ExitCode {
    if writeln!(stream, "{message}").is_err() {
        return ExitCode::from(USAGE);
    }
    ExitCode::from(status)
}

fn main() -> ExitCode {
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    match arguments.as_slice() {
        [path] => report_chain(File::open(path).and_then(verify)),
        [path, flag, expected] if flag == "--expected" => check_expected(path, expected),
        _ => report(io::stderr(), USAGE_TEXT, USAGE),
    }
}

fn check_expected(path: &OsString, expected: &OsString) -> ExitCode {
    // An expectation that cannot be read or parsed is a failure before any
    // comparison: it is never an expectation of nothing.
    let Ok(bytes) = fs::read(expected) else {
        return report(
            io::stderr(),
            "unreadable: the expected-state file could not be read",
            USAGE,
        );
    };
    let expectation = match Expectation::parse(&bytes) {
        Ok(expectation) => expectation,
        Err(error) => {
            return report(
                io::stderr(),
                &format!("expectation-refused: {}", error.code()),
                USAGE,
            );
        }
    };
    let outcome = fs::read(path).and_then(|journal| verify_expected(&journal, &expectation));
    match outcome {
        Err(error) => report_chain(Err(error)),
        Ok(ExpectedOutcome::ChainRefused(outcome)) => report_chain(Ok(outcome)),
        Ok(ExpectedOutcome::Consistent(comparison)) => {
            report(io::stdout(), &render("consistent", &comparison), 0)
        }
        Ok(ExpectedOutcome::Diverged(comparison)) => {
            report(io::stderr(), &render("diverged", &comparison), DIVERGED)
        }
    }
}

/// The verdict, then what was compared: every count belongs to a printed sum.
fn render(verdict: &str, comparison: &Comparison) -> String {
    let head = comparison.head.as_ref().map_or_else(
        || "no head".to_owned(),
        |head| format!("head seq {} digest {}", head.seq, head.digest),
    );
    let mut lines = vec![
        format!("{verdict}: {} entries verified; {head}", comparison.entries),
        format!(
            "missions: {} expected = {} found + {} missing; {} submitted in the journal = {} covered + {} outside the expectation",
            comparison.missions_expected,
            comparison.missions_found,
            comparison.missions_missing,
            comparison.journal_submitted,
            comparison.missions_found,
            comparison.journal_outside
        ),
        format!(
            "comparisons: {} planned = {} matched + {} diverged + {} not applicable",
            comparison.planned(),
            comparison.matched,
            comparison.diverged(),
            comparison.not_applicable
        ),
        format!(
            "planned: {} results + {} commits + {} evidence digests + {} check declarations + {} check executions + {} undeclared-check sweeps + {} acceptances",
            comparison.results,
            comparison.commits,
            comparison.evidence,
            comparison.declarations,
            comparison.executions,
            comparison.sweeps,
            comparison.acceptances
        ),
    ];
    for divergence in &comparison.divergences {
        let mut line = format!(
            "gap: {} mission {}",
            divergence.gap.code(),
            divergence.mission
        );
        if let Some(criterion) = divergence.criterion {
            line.push_str(&format!(" criterion {criterion}"));
        }
        if let Some(at) = divergence.line {
            line.push_str(&format!(" line {at}"));
        }
        lines.push(line);
    }
    lines.join("\n")
}

fn report_chain(outcome: io::Result<Outcome>) -> ExitCode {
    match outcome {
        Err(_) => report(
            io::stderr(),
            "unreadable: the journal file could not be read",
            USAGE,
        ),
        Ok(Outcome::Valid { entries, head }) => {
            let message = match head {
                Some(head) => format!(
                    "valid: {entries} entries verified; head seq {} digest {}",
                    head.seq, head.digest
                ),
                None => format!("valid: {entries} entries verified; no head"),
            };
            report(io::stdout(), &message, 0)
        }
        Ok(Outcome::Invalid {
            line,
            code,
            verified,
        }) => report(
            io::stderr(),
            &format!(
                "invalid: {} at line {line}; {verified} entries verified before it",
                code.code()
            ),
            1,
        ),
        Ok(Outcome::TornTail { line, verified }) => report(
            io::stderr(),
            &format!(
                "torn-tail: line {line} has no terminating newline; {verified} entries verified before it"
            ),
            3,
        ),
    }
}
