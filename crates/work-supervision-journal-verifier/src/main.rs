//! `ws-journal-verify <journal-file>` — verifies a Work Supervision v0 journal.
//!
//! Exit status: 0 valid, 1 invalid, 2 unreadable or wrong usage, 3 torn tail.
//! Output names codes, line numbers and counts only, never journal content.

use std::fs::File;
use std::io;
use std::process::ExitCode;

use work_supervision_journal_verifier::{Outcome, verify};

const USAGE: u8 = 2;

fn report(mut stream: impl io::Write, message: &str, status: u8) -> ExitCode {
    if writeln!(stream, "{message}").is_err() {
        return ExitCode::from(USAGE);
    }
    ExitCode::from(status)
}

fn main() -> ExitCode {
    let mut arguments = std::env::args_os().skip(1);
    let (Some(path), None) = (arguments.next(), arguments.next()) else {
        return report(
            io::stderr(),
            "usage: ws-journal-verify <journal-file>",
            USAGE,
        );
    };
    let outcome = File::open(&path).and_then(verify);
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
