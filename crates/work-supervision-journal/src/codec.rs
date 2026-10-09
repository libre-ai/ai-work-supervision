use serde_json::{Map, Value};

use crate::event::{check_value, is_valid_kind};
use crate::{Digest, Event, JournalError, MAX_LINE_BYTES, SCHEMA, Timestamp};

/// Envelope keys of a v0 entry, in canonical (sorted) order.
const ENVELOPE_KEYS: [&str; 6] = ["at", "digest", "event", "prev", "schema", "seq"];
const EVENT_KEYS: [&str; 2] = ["data", "kind"];

/// The position of the last accepted entry: its sequence number and digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Head {
    seq: u64,
    digest: Digest,
}

impl Head {
    pub(crate) const fn new(seq: u64, digest: Digest) -> Self {
        Self { seq, digest }
    }

    /// Sequence number of the entry, starting at 1.
    #[must_use]
    pub const fn seq(&self) -> u64 {
        self.seq
    }

    /// Digest of the entry.
    #[must_use]
    pub const fn digest(&self) -> &Digest {
        &self.digest
    }
}

/// One encoded entry: its canonical bytes (without the trailing newline) and its digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedLine {
    bytes: Vec<u8>,
    digest: Digest,
}

impl EncodedLine {
    /// Canonical bytes of the line, without the trailing newline.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Digest recorded in the line.
    #[must_use]
    pub const fn digest(&self) -> &Digest {
        &self.digest
    }
}

fn canonical(value: &Value, line: u64) -> Result<Vec<u8>, JournalError> {
    // serde_jcs only fails on values the JSON model cannot hold; the domain is
    // already restricted, so a failure here is reported as a refused envelope.
    serde_jcs::to_vec(value).map_err(|_| JournalError::EnvelopeInvalid { line })
}

/// Encodes entry `seq` chained to `prev`.
///
/// # Errors
///
/// [`JournalError::GenesisInvalid`] when `seq` and `prev` disagree on being the
/// first entry, [`JournalError::LineTooLong`] when the encoded line exceeds
/// [`MAX_LINE_BYTES`]. Errors carry `seq` as their line.
pub fn encode_line(
    seq: u64,
    prev: Option<&Digest>,
    at: &Timestamp,
    event: &Event,
) -> Result<EncodedLine, JournalError> {
    if (seq == 1) != prev.is_none() || seq == 0 {
        return Err(JournalError::GenesisInvalid { line: seq });
    }
    let mut body = Map::new();
    body.insert("at".to_owned(), Value::String(at.as_str().to_owned()));
    body.insert("event".to_owned(), event.to_value());
    body.insert(
        "prev".to_owned(),
        prev.map_or(Value::Null, |digest| Value::String(digest.to_hex())),
    );
    body.insert("schema".to_owned(), Value::String(SCHEMA.to_owned()));
    body.insert("seq".to_owned(), Value::from(seq));
    let mut entry = Value::Object(body);
    let digest = Digest::of(&canonical(&entry, seq)?);
    if let Value::Object(fields) = &mut entry {
        fields.insert("digest".to_owned(), Value::String(digest.to_hex()));
    }
    let bytes = canonical(&entry, seq)?;
    if bytes.len() > MAX_LINE_BYTES {
        return Err(JournalError::LineTooLong { line: seq });
    }
    Ok(EncodedLine { bytes, digest })
}

/// Decodes and verifies one line (without its newline) that follows `previous`.
pub(crate) fn decode_line(
    bytes: &[u8],
    line: u64,
    previous: Option<&Head>,
) -> Result<Head, JournalError> {
    if bytes.len() > MAX_LINE_BYTES {
        return Err(JournalError::LineTooLong { line });
    }
    let value: Value =
        serde_json::from_slice(bytes).map_err(|_| JournalError::Malformed { line })?;
    // Order of checks is part of the format (docs/work-supervision/journal-v0.md):
    // the value domain comes before the canonical comparison, so that no
    // implementation has to canonicalise a float.
    check_value(&value, 1).map_err(|error| error.at_line(line))?;
    if canonical(&value, line)? != bytes {
        return Err(JournalError::NonCanonical { line });
    }
    let Value::Object(mut fields) = value else {
        return Err(JournalError::EnvelopeInvalid { line });
    };
    if !has_exactly(&fields, &ENVELOPE_KEYS) {
        return Err(JournalError::EnvelopeInvalid { line });
    }
    if fields.get("schema").and_then(Value::as_str) != Some(SCHEMA) {
        return Err(JournalError::SchemaUnknown { line });
    }

    let seq = fields
        .get("seq")
        .and_then(Value::as_u64)
        .ok_or(JournalError::EnvelopeInvalid { line })?;
    let prev = match fields.get("prev") {
        Some(Value::Null) => None,
        Some(Value::String(text)) => {
            Some(Digest::from_hex(text).ok_or(JournalError::EnvelopeInvalid { line })?)
        }
        _ => return Err(JournalError::EnvelopeInvalid { line }),
    };
    match previous {
        None => {
            if seq != 1 || prev.is_some() {
                return Err(JournalError::GenesisInvalid { line });
            }
        }
        Some(head) => {
            if Some(seq) != head.seq.checked_add(1) {
                return Err(JournalError::SequenceInvalid { line });
            }
            if prev.as_ref() != Some(&head.digest) {
                return Err(JournalError::PreviousDigestMismatch { line });
            }
        }
    }

    let at = fields
        .get("at")
        .and_then(Value::as_str)
        .ok_or(JournalError::EnvelopeInvalid { line })?;
    Timestamp::parse(at).map_err(|error| error.at_line(line))?;
    check_event(fields.get("event"), line)?;

    let recorded = match fields.remove("digest") {
        Some(Value::String(text)) => {
            Digest::from_hex(&text).ok_or(JournalError::EnvelopeInvalid { line })?
        }
        _ => return Err(JournalError::EnvelopeInvalid { line }),
    };
    let computed = Digest::of(&canonical(&Value::Object(fields), line)?);
    if computed != recorded {
        return Err(JournalError::DigestMismatch { line });
    }
    Ok(Head {
        seq,
        digest: recorded,
    })
}

fn check_event(event: Option<&Value>, line: u64) -> Result<(), JournalError> {
    let Some(Value::Object(event)) = event else {
        return Err(JournalError::EnvelopeInvalid { line });
    };
    if !has_exactly(event, &EVENT_KEYS) || !matches!(event.get("data"), Some(Value::Object(_))) {
        return Err(JournalError::EnvelopeInvalid { line });
    }
    match event.get("kind") {
        Some(Value::String(kind)) if is_valid_kind(kind) => Ok(()),
        Some(Value::String(_)) => Err(JournalError::KindInvalid { line }),
        _ => Err(JournalError::EnvelopeInvalid { line }),
    }
}

fn has_exactly(fields: &Map<String, Value>, keys: &[&str]) -> bool {
    fields.len() == keys.len() && keys.iter().all(|key| fields.contains_key(*key))
}
