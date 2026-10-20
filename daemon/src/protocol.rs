// SPDX-License-Identifier: AGPL-3.0-or-later
//! The line protocol between the user interface and the service.
//!
//! One JSON object per line, both ways:
//!
//! * request:  `{"id": 1, "cmd": "conversations.list", "args": {}}`
//! * response: `{"id": 1, "ok": true, "result": {...}}`
//! * event:    `{"event": "message.received", "data": {...}}`

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Request {
    pub id: u64,
    pub cmd: String,
    #[serde(default)]
    pub args: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Error {
    pub code: String,
    pub message: String,
}

impl Error {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Response {
    pub id: u64,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Error>,
}

impl Response {
    pub fn ok(id: u64, result: Value) -> Self {
        Self {
            id,
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: u64, code: &str, message: impl Into<String>) -> Self {
        Self {
            id,
            ok: false,
            result: None,
            error: Some(Error::new(code, message)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Event {
    pub event: String,
    pub data: Value,
}

impl Event {
    pub fn new(event: &str, data: Value) -> Self {
        Self {
            event: event.to_string(),
            data,
        }
    }
}

/// Parse one request line. The error is the same shape the service sends back,
/// so a broken line is answered with `bad.request` instead of being dropped.
pub fn parse_request(line: &str) -> Result<Request, Error> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Err(Error::new("bad.request", "tyhjä rivi"));
    }
    let request: Request = serde_json::from_str(trimmed)
        .map_err(|error| Error::new("bad.request", format!("virheellinen JSON: {error}")))?;
    if request.cmd.trim().is_empty() {
        return Err(Error::new("bad.request", "komento puuttuu"));
    }
    Ok(request)
}

/// Serialise anything as one line (no embedded newlines).
pub fn to_line<T: Serialize>(value: &T) -> String {
    let mut line = serde_json::to_string(value).unwrap_or_else(|_| {
        "{\"id\":0,\"ok\":false,\"error\":{\"code\":\"internal\",\"message\":\"serialointi epäonnistui\"}}"
            .to_string()
    });
    line.push('\n');
    line
}
