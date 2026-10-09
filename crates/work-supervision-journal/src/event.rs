use serde_json::{Map, Value};

use crate::{JournalError, MAX_DEPTH, MAX_KIND_BYTES, MAX_SAFE_INTEGER};

/// Container depth of the `data` object inside an entry: entry (1) › event (2) › data (3).
const DATA_DEPTH: usize = 3;

/// One journal event: a kind and an object of data.
///
/// Values are restricted to strings, booleans, `null`, integers within
/// ±(2^53 − 1), arrays and objects. Floats are refused so that the canonical
/// encoding of numbers is the same in every implementation of the format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    kind: String,
    data: Map<String, Value>,
}

impl Event {
    /// Builds an event after validating its kind and data.
    ///
    /// # Errors
    ///
    /// [`JournalError::KindInvalid`], [`JournalError::NumberInvalid`] or
    /// [`JournalError::DepthExceeded`], all at line 0.
    pub fn new(kind: &str, data: Map<String, Value>) -> Result<Self, JournalError> {
        if !is_valid_kind(kind) {
            return Err(JournalError::KindInvalid { line: 0 });
        }
        for value in data.values() {
            check_value(value, DATA_DEPTH + 1)?;
        }
        Ok(Self {
            kind: kind.to_owned(),
            data,
        })
    }

    /// The dotted lowercase kind, e.g. `mission.note`.
    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// The event data.
    #[must_use]
    pub const fn data(&self) -> &Map<String, Value> {
        &self.data
    }

    pub(crate) fn to_value(&self) -> Value {
        let mut event = Map::new();
        event.insert("data".to_owned(), Value::Object(self.data.clone()));
        event.insert("kind".to_owned(), Value::String(self.kind.clone()));
        Value::Object(event)
    }
}

/// `segment ("." segment)*`, each segment `[a-z][a-z0-9-]*`, at most 64 bytes in total.
pub(crate) fn is_valid_kind(kind: &str) -> bool {
    if kind.is_empty() || kind.len() > MAX_KIND_BYTES {
        return false;
    }
    kind.split('.').all(|segment| {
        let mut characters = segment.bytes();
        matches!(characters.next(), Some(b'a'..=b'z'))
            && characters.all(|character| matches!(character, b'a'..=b'z' | b'0'..=b'9' | b'-'))
    })
}

/// Checks the value domain and the container depth; `depth` is the depth `value` would sit at.
pub(crate) fn check_value(value: &Value, depth: usize) -> Result<(), JournalError> {
    match value {
        Value::Null | Value::Bool(_) | Value::String(_) => Ok(()),
        Value::Number(number) => {
            let exact = number
                .as_i64()
                .is_some_and(|integer| (-MAX_SAFE_INTEGER..=MAX_SAFE_INTEGER).contains(&integer));
            if exact {
                Ok(())
            } else {
                Err(JournalError::NumberInvalid { line: 0 })
            }
        }
        Value::Array(items) => {
            if depth > MAX_DEPTH {
                return Err(JournalError::DepthExceeded { line: 0 });
            }
            items
                .iter()
                .try_for_each(|item| check_value(item, depth + 1))
        }
        Value::Object(fields) => {
            if depth > MAX_DEPTH {
                return Err(JournalError::DepthExceeded { line: 0 });
            }
            fields
                .values()
                .try_for_each(|field| check_value(field, depth + 1))
        }
    }
}
