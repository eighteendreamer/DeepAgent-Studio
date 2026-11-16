//! Minimal v1 mutation receipts. Execution metadata remains in the raw response.
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;

use crate::HarnessRequest;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadListEntry {
    pub thread_id: String,
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub ended: bool,
    pub archived: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadListPage {
    pub threads: Vec<ThreadListEntry>,
    pub list_page_version: u32,
    pub has_more: bool,
    pub next_cursor: Option<String>,
}

/// Explicit paged consumers fail closed against old or malformed peers. Raw
/// legacy list methods keep their older shape compatibility; neither loops.
pub fn decode_thread_list_page(
    request: &crate::ThreadListRequest,
    value: &Value,
) -> Result<ThreadListPage, ControlResultError> {
    let error = || ControlResultError {
        method: "thread/list",
        field: "page",
        expected: "bounded catalog page v1",
        actual: "unsupported page",
    };
    let page: ThreadListPage = serde_json::from_value(value.clone()).map_err(|_| error())?;
    if page.list_page_version != 1
        || page.threads.len() > request.limit.unwrap_or(crate::MAX_SESSION_PAGE_SIZE)
        || page.threads.len() > crate::MAX_SESSION_PAGE_SIZE
        || !value
            .get("nextCursor")
            .is_some_and(|v| v.is_null() || v.is_string())
        || if page.has_more {
            page.next_cursor.as_ref().is_none_or(|s| {
                s.is_empty() || s.len() > 4096 || request.cursor.as_ref() == Some(s)
            })
        } else {
            page.next_cursor.is_some()
        }
    {
        return Err(error());
    }
    let mut seen = std::collections::BTreeSet::new();
    for row in &page.threads {
        if row.thread_id.trim().is_empty()
            || !seen.insert(&row.thread_id)
            || request
                .archived
                .is_some_and(|archived| row.archived != archived)
        {
            return Err(error());
        }
    }
    Ok(page)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadLifecycleResult {
    pub thread_id: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnResult {
    pub thread_id: String,
    pub turn_id: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnSteerResult {
    #[serde(flatten)]
    pub turn: TurnResult,
    pub replaces_turn_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRespondResult {
    pub approval_id: String,
    pub status: String,
    pub approved: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventAckResult {
    pub acknowledged_event_sequence: u64,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadArchiveResult {
    pub thread_id: String,
    pub archived: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadForkResult {
    pub thread_id: String,
    pub source_thread_id: String,
    pub forked_at: u64,
    pub restored_paths: Vec<String>,
}

/// Safe contract diagnostics: static names/shapes only, never arbitrary peer data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlResultError {
    pub method: &'static str,
    pub field: &'static str,
    pub expected: &'static str,
    pub actual: &'static str,
}

impl std::fmt::Display for ControlResultError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} result has an invalid or conflicting {}: expected {}, received {}; remote outcome unknown, reconcile before retrying", self.method, self.field, self.expected, self.actual)
    }
}
impl std::error::Error for ControlResultError {}

fn shape(value: Option<&Value>) -> &'static str {
    match value {
        None => "missing",
        Some(Value::Null) => "null",
        Some(Value::Bool(_)) => "boolean",
        Some(Value::Number(_)) => "number",
        Some(Value::String(_)) => "string",
        Some(Value::Array(_)) => "array",
        Some(Value::Object(_)) => "object",
    }
}

fn require(
    method: &'static str,
    field: &'static str,
    expected: &'static str,
    value: Option<&Value>,
    valid: bool,
) -> Result<(), ControlResultError> {
    if valid {
        Ok(())
    } else {
        Err(ControlResultError {
            method,
            field,
            expected,
            actual: shape(value),
        })
    }
}

/// One identity rule shared by the raw response gate and owned stream scope.
pub fn control_result_identity<'a>(
    value: &'a Value,
    method: &'static str,
    field: &'static str,
    expected: Option<&str>,
) -> Result<&'a str, ControlResultError> {
    let received = value.get(field).and_then(Value::as_str);
    require(
        method,
        field,
        "nonblank identity matching request when applicable",
        value.get(field),
        received.is_some_and(|id| {
            !id.trim().is_empty() && expected.is_none_or(|expected| id == expected)
        }),
    )?;
    Ok(received.expect("identity checked"))
}

fn decode<T: DeserializeOwned>(
    method: &'static str,
    value: &Value,
) -> Result<T, ControlResultError> {
    T::deserialize(value).map_err(|_| ControlResultError {
        method,
        field: "schema",
        expected: "typed v1 mutation receipt",
        actual: shape(Some(value)),
    })
}

fn status(
    method: &'static str,
    value: &Value,
    expected: &'static str,
) -> Result<(), ControlResultError> {
    require(
        method,
        "status",
        expected,
        value.get("status"),
        value.get("status").and_then(Value::as_str) == Some(expected),
    )
}

/// Validate a v1 mutation receipt before exposing success. Read-only projections
/// have their own contracts; initialize remains the separate handshake gate.
/// Unknown metadata is accepted and this function never rewrites the raw value.
pub fn validate_control_result(
    request: &HarnessRequest,
    value: &Value,
) -> Result<(), ControlResultError> {
    let id = |method, field, expected| {
        control_result_identity(value, method, field, expected).map(|_| ())
    };
    match request {
        HarnessRequest::ThreadList(request) if request.archived.is_some() => {
            let expected = request.archived.expect("explicit archive view");
            let rows = value.get("threads").and_then(Value::as_array);
            require(
                "thread/list",
                "threads",
                "explicit active/archived view with matching markers",
                value.get("threads"),
                rows.is_some_and(|rows| {
                    rows.iter()
                        .all(|row| row.get("archived").and_then(Value::as_bool) == Some(expected))
                }),
            )
        }
        HarnessRequest::ThreadStart(_) => {
            id("thread/start", "threadId", None)?;
            let _: ThreadLifecycleResult = decode("thread/start", value)?;
            status("thread/start", value, "ready")
        }
        HarnessRequest::ThreadResume(request) => {
            id("thread/resume", "threadId", Some(&request.thread_id))?;
            let _: ThreadLifecycleResult = decode("thread/resume", value)?;
            status("thread/resume", value, "ready")
        }
        HarnessRequest::ThreadFork(request) => {
            id("thread/fork", "threadId", None)?;
            id("thread/fork", "sourceThreadId", Some(&request.thread_id))?;
            let _: ThreadForkResult = decode("thread/fork", value)?;
            Ok(())
        }
        HarnessRequest::ThreadArchive(request) => {
            id("thread/archive", "threadId", Some(&request.thread_id))?;
            let result: ThreadArchiveResult = decode("thread/archive", value)?;
            require(
                "thread/archive",
                "archived",
                "true",
                value.get("archived"),
                result.archived,
            )
        }
        HarnessRequest::ThreadUnarchive(request) => {
            id("thread/unarchive", "threadId", Some(&request.thread_id))?;
            let result: ThreadArchiveResult = decode("thread/unarchive", value)?;
            require(
                "thread/unarchive",
                "archived",
                "false",
                value.get("archived"),
                !result.archived,
            )
        }
        HarnessRequest::TurnStart(request) => {
            id("turn/start", "threadId", Some(&request.thread_id))?;
            id("turn/start", "turnId", None)?;
            let _: TurnResult = decode("turn/start", value)?;
            status("turn/start", value, "started")
        }
        HarnessRequest::TurnInterrupt(request) => {
            id("turn/interrupt", "threadId", Some(&request.thread_id))?;
            id("turn/interrupt", "turnId", Some(&request.turn_id))?;
            let _: TurnResult = decode("turn/interrupt", value)?;
            status("turn/interrupt", value, "cancelling")
        }
        HarnessRequest::TurnSteer(request) => {
            id("turn/steer", "threadId", Some(&request.thread_id))?;
            id("turn/steer", "turnId", None)?;
            id("turn/steer", "replacesTurnId", Some(&request.turn_id))?;
            let result: TurnSteerResult = decode("turn/steer", value)?;
            require(
                "turn/steer",
                "turnId",
                "new turn identity",
                value.get("turnId"),
                result.turn.turn_id != request.turn_id,
            )?;
            status("turn/steer", value, "started")
        }
        HarnessRequest::ApprovalRespond(request) => {
            id("approval/respond", "approvalId", Some(&request.approval_id))?;
            let result: ApprovalRespondResult = decode("approval/respond", value)?;
            require(
                "approval/respond",
                "approved",
                "boolean matching request",
                value.get("approved"),
                result.approved == request.approved,
            )?;
            status("approval/respond", value, "resolved")
        }
        HarnessRequest::EventAck(request) => {
            let result: EventAckResult = decode("event/ack", value)?;
            require(
                "event/ack",
                "acknowledgedEventSequence",
                "u64 matching requested sequence",
                value.get("acknowledgedEventSequence"),
                result.acknowledged_event_sequence == request.event_sequence,
            )?;
            status("event/ack", value, "accepted")
        }
        _ => Ok(()),
    }
}
