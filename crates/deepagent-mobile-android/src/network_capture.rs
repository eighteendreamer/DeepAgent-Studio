//! Logcat-based HTTP traffic parser for generic network observation.
//!
//! Parses `adb logcat -v threadtime` output for HTTP request/response patterns
//! from common Android HTTP libraries (OkHttp, HttpURLConnection, etc.).
//! This is a generic platform capability — not project-specific instrumentation.

use deepagent_mobile_protocol::{NetworkRecord, NetworkRequest, NetworkResponse};
use std::collections::HashMap;

/// HTTP methods recognized by the parser.
const HTTP_METHODS: &[&str] = &["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"];

/// State for correlating request/response pairs within a capture session.
pub struct NetworkCaptureState {
    records: Vec<NetworkRecord>,
    pending: Option<PendingRequest>,
    sequence: u64,
}

struct PendingRequest {
    method: String,
    url: String,
    headers: HashMap<String, String>,
    timestamp_ms: u64,
}

impl NetworkCaptureState {
    pub fn new() -> Self {
        Self {
            records: Vec::new(),
            pending: None,
            sequence: 0,
        }
    }

    /// Parse logcat lines and accumulate network records.
    pub fn parse_lines(&mut self, device_id: &str, lines: &str) {
        for line in lines.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            if let Some(parsed) = try_parse_http_line(line) {
                match parsed {
                    ParsedHttpLine::Request {
                        method,
                        url,
                        headers,
                        timestamp_ms,
                    } => {
                        if let Some(pending) = self.pending.take() {
                            self.flush_pending_as_request_only(device_id, pending);
                        }
                        self.pending = Some(PendingRequest {
                            method,
                            url,
                            headers,
                            timestamp_ms,
                        });
                    }
                    ParsedHttpLine::Response {
                        status_code,
                        status_text,
                        headers,
                        duration_ms,
                        timestamp_ms,
                    } => {
                        if let Some(pending) = self.pending.take() {
                            self.sequence += 1;
                            let record = NetworkRecord {
                                record_id: format!("net-{}-{}", device_id, self.sequence),
                                device_id: device_id.to_string(),
                                package: None,
                                request: NetworkRequest {
                                    method: pending.method,
                                    url: pending.url,
                                    headers: pending.headers,
                                    body: None,
                                    content_type: None,
                                },
                                response: Some(NetworkResponse {
                                    status_code,
                                    status_text,
                                    headers,
                                    body: None,
                                    content_type: None,
                                }),
                                timestamp_ms: pending.timestamp_ms,
                                duration_ms,
                            };
                            self.records.push(record);
                        } else {
                            self.sequence += 1;
                            let record = NetworkRecord {
                                record_id: format!("net-{}-{}", device_id, self.sequence),
                                device_id: device_id.to_string(),
                                package: None,
                                request: NetworkRequest {
                                    method: "UNKNOWN".into(),
                                    url: String::new(),
                                    headers: HashMap::new(),
                                    body: None,
                                    content_type: None,
                                },
                                response: Some(NetworkResponse {
                                    status_code,
                                    status_text,
                                    headers,
                                    body: None,
                                    content_type: None,
                                }),
                                timestamp_ms,
                                duration_ms,
                            };
                            self.records.push(record);
                        }
                    }
                }
            }
        }
    }

    fn flush_pending_as_request_only(&mut self, device_id: &str, pending: PendingRequest) {
        self.sequence += 1;
        let record = NetworkRecord {
            record_id: format!("net-{}-{}", device_id, self.sequence),
            device_id: device_id.to_string(),
            package: None,
            request: NetworkRequest {
                method: pending.method,
                url: pending.url,
                headers: pending.headers,
                body: None,
                content_type: None,
            },
            response: None,
            timestamp_ms: pending.timestamp_ms,
            duration_ms: None,
        };
        self.records.push(record);
    }

    /// Take all accumulated records.
    pub fn take_records(&mut self) -> Vec<NetworkRecord> {
        std::mem::take(&mut self.records)
    }
}

enum ParsedHttpLine {
    Request {
        method: String,
        url: String,
        headers: HashMap<String, String>,
        timestamp_ms: u64,
    },
    Response {
        status_code: u16,
        status_text: String,
        headers: HashMap<String, String>,
        duration_ms: Option<u64>,
        timestamp_ms: u64,
    },
}

fn try_parse_http_line(line: &str) -> Option<ParsedHttpLine> {
    let timestamp_ms = parse_logcat_timestamp(line);

    if let Some(rest) = extract_logcat_message(line) {
        if let Some(req) = try_parse_request_line(rest) {
            return Some(ParsedHttpLine::Request {
                method: req.0,
                url: req.1,
                headers: HashMap::new(),
                timestamp_ms,
            });
        }
        if let Some(resp) = try_parse_response_line(rest) {
            return Some(ParsedHttpLine::Response {
                status_code: resp.0,
                status_text: resp.1,
                headers: HashMap::new(),
                duration_ms: resp.2,
                timestamp_ms,
            });
        }
    }

    None
}

/// Parse "MM-DD HH:MM:SS.mmm" from logcat threadtime format.
fn parse_logcat_timestamp(line: &str) -> u64 {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 3 {
        return 0;
    }
    let date = parts[0];
    let time = parts[1];

    let month_day: Vec<&str> = date.split('-').collect();
    let month: u64 = month_day.first().and_then(|m| m.parse().ok()).unwrap_or(1);
    let day: u64 = month_day.get(1).and_then(|d| d.parse().ok()).unwrap_or(1);

    let time_parts: Vec<&str> = time.split(':').collect();
    let hour: u64 = time_parts.first().and_then(|h| h.parse().ok()).unwrap_or(0);
    let minute: u64 = time_parts.get(1).and_then(|m| m.parse().ok()).unwrap_or(0);
    let sec_ms: Vec<&str> = time_parts
        .get(2)
        .map(|s| s.split('.').collect())
        .unwrap_or_default();
    let second: u64 = sec_ms.first().and_then(|s| s.parse().ok()).unwrap_or(0);
    let ms: u64 = sec_ms.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);

    ((month * 30 + day) * 86400 + hour * 3600 + minute * 60 + second) * 1000 + ms
}

/// Extract the message portion from a logcat threadtime line.
/// Format: "MM-DD HH:MM:SS.mmm PID TID LEVEL TAG: message"
fn extract_logcat_message(line: &str) -> Option<&str> {
    let mut pos = 0;
    let bytes = line.as_bytes();

    for _ in 0..5 {
        while pos < bytes.len() && bytes[pos] == b' ' {
            pos += 1;
        }
        while pos < bytes.len() && bytes[pos] != b' ' {
            pos += 1;
        }
    }

    while pos < bytes.len() && bytes[pos] == b' ' {
        pos += 1;
    }

    if let Some(colon_offset) = line[pos..].find(": ") {
        Some(&line[pos + colon_offset + 2..])
    } else {
        None
    }
}

/// Try to parse "--> METHOD URL" pattern (OkHttp LoggingInterceptor).
fn try_parse_request_line(msg: &str) -> Option<(String, String)> {
    let trimmed = msg.trim();
    if !trimmed.starts_with("-->") {
        return None;
    }
    let rest = trimmed[3..].trim();
    let mut parts = rest.split_whitespace();
    let method = parts.next()?;
    if !HTTP_METHODS.contains(&method) {
        return None;
    }
    let url = parts.next()?;
    Some((method.to_string(), url.to_string()))
}

/// Try to parse "<-- STATUS TEXT (DURATION)" pattern.
fn try_parse_response_line(msg: &str) -> Option<(u16, String, Option<u64>)> {
    let trimmed = msg.trim();
    if !trimmed.starts_with("<--") {
        return None;
    }
    let rest = trimmed[3..].trim();
    let mut parts = rest.split_whitespace();
    let status_str = parts.next()?;
    let status_code: u16 = status_str.parse().ok()?;
    if !(100..=599).contains(&status_code) {
        return None;
    }

    let mut status_text_parts = Vec::new();
    let mut duration_ms = None;

    for part in parts {
        if let Some(ms) = try_parse_duration(part) {
            duration_ms = Some(ms);
            break;
        }
        status_text_parts.push(part);
    }

    let status_text = status_text_parts.join(" ");
    Some((status_code, status_text, duration_ms))
}

/// Parse duration strings like "42ms", "1.5s", "(1.5s)", "(42ms)".
fn try_parse_duration(s: &str) -> Option<u64> {
    let cleaned = s.trim_matches(|c: char| c == '(' || c == ')' || c == ',');
    if let Some(ms_str) = cleaned.strip_suffix("ms") {
        ms_str.parse().ok()
    } else if let Some(secs_str) = cleaned.strip_suffix('s') {
        let secs: f64 = secs_str.parse().ok()?;
        Some((secs * 1000.0) as u64)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_okhttp_request_line() {
        let line = "09-05 12:00:00.000  1234  5678 D OkHttp: --> GET https://api.example.com/data";
        let result = try_parse_http_line(line);
        assert!(result.is_some());
        match result.unwrap() {
            ParsedHttpLine::Request { method, url, .. } => {
                assert_eq!(method, "GET");
                assert_eq!(url, "https://api.example.com/data");
            }
            _ => panic!("expected request"),
        }
    }

    #[test]
    fn parse_okhttp_response_line() {
        let line = "09-05 12:00:00.100  1234  5678 D OkHttp: <-- 200 OK (150ms)";
        let result = try_parse_http_line(line);
        assert!(result.is_some());
        match result.unwrap() {
            ParsedHttpLine::Response {
                status_code,
                status_text,
                duration_ms,
                ..
            } => {
                assert_eq!(status_code, 200);
                assert_eq!(status_text, "OK");
                assert_eq!(duration_ms, Some(150));
            }
            _ => panic!("expected response"),
        }
    }

    #[test]
    fn parse_response_with_seconds_duration() {
        let line = "09-05 12:00:00.100  1234  5678 D OkHttp: <-- 200 OK (1.5s)";
        let result = try_parse_http_line(line);
        assert!(result.is_some());
        match result.unwrap() {
            ParsedHttpLine::Response { duration_ms, .. } => {
                assert_eq!(duration_ms, Some(1500));
            }
            _ => panic!("expected response"),
        }
    }

    #[test]
    fn non_http_lines_return_none() {
        assert!(try_parse_http_line("09-05 12:00:00.000 1 2 I SystemServer: Start").is_none());
        assert!(try_parse_http_line("").is_none());
        assert!(try_parse_http_line("random text").is_none());
    }

    #[test]
    fn parse_post_request() {
        let line = "09-05 12:00:00.000  1  2 D OkHttp: --> POST https://api.example.com/login";
        match try_parse_http_line(line).unwrap() {
            ParsedHttpLine::Request { method, url, .. } => {
                assert_eq!(method, "POST");
                assert_eq!(url, "https://api.example.com/login");
            }
            _ => panic!("expected request"),
        }
    }

    #[test]
    fn parse_404_response() {
        let line = "09-05 12:00:00.100  1  2 D OkHttp: <-- 404 Not Found (42ms)";
        match try_parse_http_line(line).unwrap() {
            ParsedHttpLine::Response {
                status_code,
                status_text,
                duration_ms,
                ..
            } => {
                assert_eq!(status_code, 404);
                assert_eq!(status_text, "Not Found");
                assert_eq!(duration_ms, Some(42));
            }
            _ => panic!("expected response"),
        }
    }

    #[test]
    fn capture_state_correlates_pair() {
        let mut state = NetworkCaptureState::new();
        state.parse_lines(
            "dev-1",
            "09-05 12:00:00.000  1  2 D OkHttp: --> GET https://example.com\n\
             09-05 12:00:00.150  1  2 D OkHttp: <-- 200 OK (150ms)\n",
        );
        let records = state.take_records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].request.method, "GET");
        assert_eq!(records[0].request.url, "https://example.com");
        assert!(records[0].response.is_some());
        assert_eq!(records[0].response.as_ref().unwrap().status_code, 200);
        assert_eq!(records[0].duration_ms, Some(150));
    }

    #[test]
    fn capture_state_request_without_response() {
        let mut state = NetworkCaptureState::new();
        state.parse_lines(
            "dev-1",
            "09-05 12:00:00.000  1  2 D OkHttp: --> GET https://pending.com\n",
        );
        let records = state.take_records();
        assert!(records.is_empty());
        assert!(state.pending.is_some());
    }

    #[test]
    fn capture_state_response_without_request() {
        let mut state = NetworkCaptureState::new();
        state.parse_lines(
            "dev-1",
            "09-05 12:00:00.100  1  2 D OkHttp: <-- 200 OK (50ms)\n",
        );
        let records = state.take_records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].request.method, "UNKNOWN");
        assert!(records[0].response.is_some());
    }

    #[test]
    fn capture_state_multiple_pairs() {
        let mut state = NetworkCaptureState::new();
        state.parse_lines(
            "dev-1",
            "09-05 12:00:00.000  1  2 D OkHttp: --> GET https://first.com\n\
             09-05 12:00:00.100  1  2 D OkHttp: <-- 200 OK (100ms)\n\
             09-05 12:00:01.000  1  2 D OkHttp: --> POST https://second.com\n\
             09-05 12:00:01.200  1  2 D OkHttp: <-- 201 Created (200ms)\n",
        );
        let records = state.take_records();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].request.url, "https://first.com");
        assert_eq!(records[1].request.url, "https://second.com");
        assert_eq!(records[0].record_id, "net-dev-1-1");
        assert_eq!(records[1].record_id, "net-dev-1-2");
    }

    #[test]
    fn capture_state_non_http_lines_ignored() {
        let mut state = NetworkCaptureState::new();
        state.parse_lines(
            "dev-1",
            "09-05 12:00:00.000  1  2 I SystemServer: Start\n\
             09-05 12:00:00.100  1  2 D ActivityManager: Killing\n\
             09-05 12:00:00.200  1  2 D OkHttp: --> GET https://example.com\n\
             09-05 12:00:00.300  1  2 D OkHttp: <-- 200 OK (50ms)\n",
        );
        let records = state.take_records();
        assert_eq!(records.len(), 1);
    }

    #[test]
    fn timestamp_parsing_basic() {
        let ts = parse_logcat_timestamp("09-05 12:30:45.123  1  2 D Tag: msg");
        assert!(ts > 0, "timestamp should be non-zero");
    }

    #[test]
    fn extract_message_from_threadtime() {
        let msg =
            extract_logcat_message("09-05 12:00:00.000  1234  5678 D OkHttp: --> GET https://x");
        assert_eq!(msg, Some("--> GET https://x"));
    }

    #[test]
    fn duration_parsing_variants() {
        assert_eq!(try_parse_duration("42ms"), Some(42));
        assert_eq!(try_parse_duration("(42ms)"), Some(42));
        assert_eq!(try_parse_duration("1.5s"), Some(1500));
        assert_eq!(try_parse_duration("(1.5s)"), Some(1500));
        assert_eq!(try_parse_duration("hello"), None);
    }
}
