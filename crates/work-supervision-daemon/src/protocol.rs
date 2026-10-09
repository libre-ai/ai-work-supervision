//! The local protocol between `ws` and `wsd`: one JSON object per line.
//!
//! A request is `{"op": "<name>", ...}`; a response is
//! `{"data": <value>, "meta": {"op": "<name>"}}` or
//! `{"error": {"code": "<code>"}}`. Requests and responses are at most
//! [`MAX_MESSAGE_BYTES`]. Errors carry a code only, never content.

use std::fmt;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::os::unix::net::UnixStream;
use std::path::Path;

use serde_json::{Value, json};

/// Largest request or response line, in bytes.
pub const MAX_MESSAGE_BYTES: usize = 4 << 20;

/// A refusal received from the daemon, or a transport failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientError {
    code: String,
}

impl ClientError {
    fn new(code: &str) -> Self {
        Self {
            code: code.to_owned(),
        }
    }

    /// The refusal code (`transport.*` for a connection failure).
    #[must_use]
    pub fn code(&self) -> &str {
        &self.code
    }
}

impl fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.code)
    }
}

impl std::error::Error for ClientError {}

/// A connection to `wsd`.
#[derive(Debug)]
pub struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Client {
    /// Connects to the daemon socket.
    ///
    /// # Errors
    ///
    /// `transport.connect`.
    pub fn connect(socket: &Path) -> Result<Self, ClientError> {
        let stream =
            UnixStream::connect(socket).map_err(|_| ClientError::new("transport.connect"))?;
        let writer = stream
            .try_clone()
            .map_err(|_| ClientError::new("transport.connect"))?;
        Ok(Self {
            reader: BufReader::new(stream),
            writer,
        })
    }

    /// Sends `request` and returns the response's `data`.
    ///
    /// # Errors
    ///
    /// The daemon's refusal code, or `transport.io` / `transport.closed` /
    /// `transport.malformed`.
    pub fn request(&mut self, request: &Value) -> Result<Value, ClientError> {
        let mut line =
            serde_json::to_vec(request).map_err(|_| ClientError::new("transport.malformed"))?;
        line.push(b'\n');
        self.writer
            .write_all(&line)
            .and_then(|()| self.writer.flush())
            .map_err(|_| ClientError::new("transport.io"))?;
        let response = read_line(&mut self.reader)
            .map_err(|_| ClientError::new("transport.io"))?
            .ok_or_else(|| ClientError::new("transport.closed"))?;
        let value: Value = serde_json::from_slice(&response)
            .map_err(|_| ClientError::new("transport.malformed"))?;
        if let Some(code) = value.pointer("/error/code").and_then(Value::as_str) {
            return Err(ClientError::new(code));
        }
        value
            .get("data")
            .cloned()
            .ok_or_else(|| ClientError::new("transport.malformed"))
    }
}

/// Reads one line of at most [`MAX_MESSAGE_BYTES`]; `None` at end of stream.
pub(crate) fn read_line<R: std::io::Read>(
    reader: &mut BufReader<R>,
) -> std::io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    let limit = u64::try_from(MAX_MESSAGE_BYTES).unwrap_or(u64::MAX) + 1;
    let read = reader.by_ref().take(limit).read_until(b'\n', &mut line)?;
    if read == 0 {
        return Ok(None);
    }
    if line.last() != Some(&b'\n') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "line too long or truncated",
        ));
    }
    line.pop();
    Ok(Some(line))
}

pub(crate) fn success(op: &str, data: Value) -> Value {
    json!({ "data": data, "meta": { "op": op } })
}

pub(crate) fn failure(code: &str) -> Value {
    json!({ "error": { "code": code } })
}
