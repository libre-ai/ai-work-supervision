//! Independent verifier of the Work Supervision v0 journal.
//!
//! This crate re-implements the format `libre-ai.work-supervision.journal.v0`
//! (`docs/work-supervision/journal-v0.md`) from its specification: canonical
//! encoding, value domain, envelope, hash chain and digest. It shares no code
//! with the writer (`work-supervision-journal`) and does not depend on
//! `serde_jcs`; it only reads bytes, so it can check a journal outside the
//! SQLite projection and outside the process that wrote it.
//!
//! ```
//! use work_supervision_journal_verifier::{Outcome, verify};
//!
//! assert_eq!(
//!     verify(&b""[..])?,
//!     Outcome::Valid { entries: 0, head: None }
//! );
//! assert!(matches!(verify(&b"{}"[..])?, Outcome::TornTail { line: 1, verified: 0 }));
//! # Ok::<(), std::io::Error>(())
//! ```

pub mod canonical;

use std::io::{self, BufRead as _, BufReader, Read};

use serde_json::{Map, Value};
use sha2::{Digest as _, Sha256};

const SCHEMA: &str = "libre-ai.work-supervision.journal.v0";
const MAX_LINE_BYTES: usize = 1 << 20;
const MAX_DEPTH: usize = 32;
const MAX_KIND_BYTES: usize = 64;
const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;
const ENVELOPE_KEYS: [&str; 6] = ["at", "digest", "event", "prev", "schema", "seq"];
const EVENT_KEYS: [&str; 2] = ["data", "kind"];

/// Why a line is refused. Codes are those of the format specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    /// The line exceeds 1 MiB without its newline.
    LineTooLong,
    /// The line is not a JSON document.
    Malformed,
    /// The line is not its own RFC 8785 canonical encoding.
    NonCanonical,
    /// The envelope does not have exactly the v0 fields with their types.
    EnvelopeInvalid,
    /// The schema is not `libre-ai.work-supervision.journal.v0`.
    SchemaUnknown,
    /// The first entry is not `seq = 1` with `prev = null`.
    GenesisInvalid,
    /// `seq` is not the previous `seq` plus one.
    SequenceInvalid,
    /// `prev` is not the previous entry's digest.
    PreviousDigestMismatch,
    /// `digest` is not the SHA-256 of the canonical entry without it.
    DigestMismatch,
    /// `at` is not `YYYY-MM-DDTHH:MM:SS.mmmZ` on a real date.
    TimestampInvalid,
    /// The event kind breaks the dotted lowercase grammar.
    KindInvalid,
    /// A float, or an integer outside ±(2^53 − 1).
    NumberInvalid,
    /// Containers nested deeper than 32.
    DepthExceeded,
}

impl Code {
    /// Stable code, identical to the writer's refusal code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::LineTooLong => "line-too-long",
            Self::Malformed => "malformed",
            Self::NonCanonical => "non-canonical",
            Self::EnvelopeInvalid => "envelope-invalid",
            Self::SchemaUnknown => "schema-unknown",
            Self::GenesisInvalid => "genesis-invalid",
            Self::SequenceInvalid => "sequence-invalid",
            Self::PreviousDigestMismatch => "previous-digest-mismatch",
            Self::DigestMismatch => "digest-mismatch",
            Self::TimestampInvalid => "timestamp-invalid",
            Self::KindInvalid => "kind-invalid",
            Self::NumberInvalid => "number-invalid",
            Self::DepthExceeded => "depth-exceeded",
        }
    }
}

/// Position of the last verified entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    /// Sequence number of the entry.
    pub seq: u64,
    /// Its digest, lowercase hexadecimal.
    pub digest: String,
}

/// Result of a verification. Every variant states how many entries were verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Every line verified.
    Valid {
        /// Number of entries verified.
        entries: u64,
        /// Last entry, `None` for an empty journal.
        head: Option<Head>,
    },
    /// `line` is refused for `code`; the `verified` entries before it are sound.
    Invalid {
        /// Refused line, 1-based.
        line: u64,
        /// Reason.
        code: Code,
        /// Entries verified before the refused line.
        verified: u64,
    },
    /// The last line has no terminating newline: an interrupted append, not a corruption.
    TornTail {
        /// Torn line, 1-based.
        line: u64,
        /// Entries verified before it.
        verified: u64,
    },
}

/// Verifies a whole journal read from `reader`.
///
/// # Errors
///
/// Any read error. An unreadable journal is an error, never an empty journal.
pub fn verify<R: Read>(reader: R) -> io::Result<Outcome> {
    let mut reader = BufReader::new(reader);
    let limit = u64::try_from(MAX_LINE_BYTES).map_err(io::Error::other)? + 1;
    let mut buffer = Vec::new();
    let mut head: Option<Head> = None;
    let mut verified: u64 = 0;
    loop {
        buffer.clear();
        let read = (&mut reader).take(limit).read_until(b'\n', &mut buffer)?;
        if read == 0 {
            return Ok(Outcome::Valid {
                entries: verified,
                head,
            });
        }
        let line = verified + 1;
        if buffer.last() == Some(&b'\n') {
            buffer.pop();
        } else if buffer.len() > MAX_LINE_BYTES {
            return Ok(Outcome::Invalid {
                line,
                code: Code::LineTooLong,
                verified,
            });
        } else {
            return Ok(Outcome::TornTail { line, verified });
        }
        match check_line(&buffer, head.as_ref()) {
            Ok(next) => {
                head = Some(next);
                verified = line;
            }
            Err(code) => {
                return Ok(Outcome::Invalid {
                    line,
                    code,
                    verified,
                });
            }
        }
    }
}

/// The checks of one line, in the order fixed by the format specification.
fn check_line(bytes: &[u8], previous: Option<&Head>) -> Result<Head, Code> {
    if bytes.len() > MAX_LINE_BYTES {
        return Err(Code::LineTooLong);
    }
    let value: Value = serde_json::from_slice(bytes).map_err(|_| Code::Malformed)?;
    check_domain(&value, 1)?;
    if canonical::encode(&value).as_deref() != Some(bytes) {
        return Err(Code::NonCanonical);
    }
    let Value::Object(mut fields) = value else {
        return Err(Code::EnvelopeInvalid);
    };
    if !has_exactly(&fields, &ENVELOPE_KEYS) {
        return Err(Code::EnvelopeInvalid);
    }
    if fields.get("schema").and_then(Value::as_str) != Some(SCHEMA) {
        return Err(Code::SchemaUnknown);
    }
    let seq = fields
        .get("seq")
        .and_then(Value::as_u64)
        .ok_or(Code::EnvelopeInvalid)?;
    let prev = match fields.get("prev") {
        Some(Value::Null) => None,
        Some(Value::String(text)) if is_digest(text) => Some(text.clone()),
        _ => return Err(Code::EnvelopeInvalid),
    };
    match previous {
        None if seq != 1 || prev.is_some() => return Err(Code::GenesisInvalid),
        None => {}
        Some(head) => {
            if Some(seq) != head.seq.checked_add(1) {
                return Err(Code::SequenceInvalid);
            }
            if prev.as_deref() != Some(head.digest.as_str()) {
                return Err(Code::PreviousDigestMismatch);
            }
        }
    }
    match fields.get("at") {
        Some(Value::String(at)) if is_timestamp(at.as_bytes()) => {}
        Some(Value::String(_)) => return Err(Code::TimestampInvalid),
        _ => return Err(Code::EnvelopeInvalid),
    }
    check_event(fields.get("event"))?;
    let recorded = match fields.remove("digest") {
        Some(Value::String(text)) if is_digest(&text) => text,
        _ => return Err(Code::EnvelopeInvalid),
    };
    let body = canonical::encode(&Value::Object(fields)).ok_or(Code::NumberInvalid)?;
    if hex(&Sha256::digest(&body)) != recorded {
        return Err(Code::DigestMismatch);
    }
    Ok(Head {
        seq,
        digest: recorded,
    })
}

fn check_domain(value: &Value, depth: usize) -> Result<(), Code> {
    match value {
        Value::Null | Value::Bool(_) | Value::String(_) => Ok(()),
        Value::Number(number) => match number.as_i64() {
            Some(integer) if (-MAX_SAFE_INTEGER..=MAX_SAFE_INTEGER).contains(&integer) => Ok(()),
            _ => Err(Code::NumberInvalid),
        },
        Value::Array(items) => {
            if depth > MAX_DEPTH {
                return Err(Code::DepthExceeded);
            }
            items
                .iter()
                .try_for_each(|item| check_domain(item, depth + 1))
        }
        Value::Object(fields) => {
            if depth > MAX_DEPTH {
                return Err(Code::DepthExceeded);
            }
            fields
                .values()
                .try_for_each(|field| check_domain(field, depth + 1))
        }
    }
}

fn check_event(event: Option<&Value>) -> Result<(), Code> {
    let Some(Value::Object(event)) = event else {
        return Err(Code::EnvelopeInvalid);
    };
    if !has_exactly(event, &EVENT_KEYS) || !matches!(event.get("data"), Some(Value::Object(_))) {
        return Err(Code::EnvelopeInvalid);
    }
    match event.get("kind") {
        Some(Value::String(kind)) if is_kind(kind) => Ok(()),
        Some(Value::String(_)) => Err(Code::KindInvalid),
        _ => Err(Code::EnvelopeInvalid),
    }
}

fn has_exactly(fields: &Map<String, Value>, keys: &[&str]) -> bool {
    fields.len() == keys.len() && keys.iter().all(|key| fields.contains_key(*key))
}

fn is_digest(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(64), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
}

fn is_kind(kind: &str) -> bool {
    !kind.is_empty()
        && kind.len() <= MAX_KIND_BYTES
        && kind.split('.').all(|segment| {
            let bytes = segment.as_bytes();
            bytes.first().is_some_and(u8::is_ascii_lowercase)
                && bytes
                    .iter()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
        })
}

fn is_timestamp(text: &[u8]) -> bool {
    if text.len() != 24 {
        return false;
    }
    let separators = [
        (4, b'-'),
        (7, b'-'),
        (10, b'T'),
        (13, b':'),
        (16, b':'),
        (19, b'.'),
        (23, b'Z'),
    ];
    if !separators
        .iter()
        .all(|(index, expected)| text.get(*index) == Some(expected))
    {
        return false;
    }
    let field = |start: usize, end: usize| -> Option<u32> {
        let digits = text.get(start..end)?;
        digits.iter().try_fold(0_u32, |value, digit| {
            digit
                .is_ascii_digit()
                .then(|| value * 10 + u32::from(digit - b'0'))
        })
    };
    let (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second), Some(_)) = (
        field(0, 4),
        field(5, 7),
        field(8, 10),
        field(11, 13),
        field(14, 16),
        field(17, 19),
        field(20, 23),
    ) else {
        return false;
    };
    let leap = (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400);
    let last_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=last_day).contains(&day) && hour < 24 && minute < 60 && second < 60
}
