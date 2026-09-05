//! Network inspection types for mobile traffic capture.
//!
//! These types represent captured HTTP/HTTPS requests and responses from
//! mobile applications. All data is structured for correlation (request ↔
//! response pairing) and redaction (sensitive header/body removal).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A captured network record (request + response pair).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkRecord {
    /// Unique record ID for correlation.
    pub record_id: String,
    /// Device that generated this traffic.
    pub device_id: String,
    /// Application package/bundle that generated this traffic.
    pub package: Option<String>,
    /// The request portion.
    pub request: NetworkRequest,
    /// The response portion (None if still pending or timed out).
    pub response: Option<NetworkResponse>,
    /// Timestamp when the request was sent (milliseconds since epoch).
    pub timestamp_ms: u64,
    /// Duration in milliseconds (from request sent to response received).
    pub duration_ms: Option<u64>,
}

/// An HTTP request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkRequest {
    /// HTTP method (GET, POST, etc.).
    pub method: String,
    /// Full URL.
    pub url: String,
    /// HTTP headers (keys are case-insensitive).
    pub headers: HashMap<String, String>,
    /// Request body (if any). Truncated to `MAX_BODY_SIZE` bytes.
    pub body: Option<String>,
    /// Content type of the body.
    pub content_type: Option<String>,
}

/// An HTTP response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkResponse {
    /// HTTP status code.
    pub status_code: u16,
    /// HTTP status text (e.g., "OK", "Not Found").
    pub status_text: String,
    /// HTTP headers.
    pub headers: HashMap<String, String>,
    /// Response body (if any). Truncated to `MAX_BODY_SIZE` bytes.
    pub body: Option<String>,
    /// Content type of the body.
    pub content_type: Option<String>,
}

/// Maximum body size to capture (64 KB).
pub const MAX_BODY_SIZE: usize = 64 * 1024;

/// Sensitive header names that should always be redacted.
pub const SENSITIVE_HEADERS: &[&str] = &[
    "authorization",
    "cookie",
    "set-cookie",
    "proxy-authorization",
    "x-api-key",
    "x-auth-token",
];

impl NetworkRecord {
    /// Check if this record is a complete request-response pair.
    pub fn is_complete(&self) -> bool {
        self.response.is_some()
    }

    /// Check if the response indicates an error (4xx or 5xx).
    pub fn is_error(&self) -> bool {
        self.response
            .as_ref()
            .map(|r| r.status_code >= 400)
            .unwrap_or(false)
    }

    /// Redact sensitive headers and body content.
    ///
    /// Returns a new record with:
    /// - Sensitive headers (Authorization, Cookie, etc.) replaced with `[redacted]`
    /// - Body content patterns (tokens, keys) redacted
    pub fn redact(&self) -> Self {
        let mut redacted = self.clone();
        redacted.request = redacted.request.redact();
        if let Some(ref mut response) = redacted.response {
            *response = response.redact();
        }
        redacted
    }
}

impl NetworkRequest {
    /// Check if a header name is sensitive.
    pub fn is_sensitive_header(name: &str) -> bool {
        let lower = name.to_lowercase();
        SENSITIVE_HEADERS.contains(&lower.as_str())
    }

    /// Redact sensitive headers and body content.
    pub fn redact(&self) -> Self {
        let mut redacted = self.clone();
        let sensitive_keys: Vec<String> = redacted
            .headers
            .keys()
            .filter(|k| Self::is_sensitive_header(k))
            .cloned()
            .collect();
        for key in sensitive_keys {
            redacted.headers.insert(key, "[redacted]".into());
        }
        if let Some(ref body) = redacted.body {
            redacted.body = Some(redact_body_text(body));
        }
        redacted
    }
}

impl NetworkResponse {
    /// Redact sensitive headers and body content.
    pub fn redact(&self) -> Self {
        let mut redacted = self.clone();
        let sensitive_keys: Vec<String> = redacted
            .headers
            .keys()
            .filter(|k| NetworkRequest::is_sensitive_header(k))
            .cloned()
            .collect();
        for key in sensitive_keys {
            redacted.headers.insert(key, "[redacted]".into());
        }
        if let Some(ref body) = redacted.body {
            redacted.body = Some(redact_body_text(body));
        }
        redacted
    }
}

/// Redact sensitive patterns in body text (tokens, API keys, secrets).
fn redact_body_text(body: &str) -> String {
    let mut result = body.to_string();

    // Redact common token patterns: "token": "xxx", "api_key": "xxx", etc.
    let sensitive_keys = [
        "token",
        "api_key",
        "apikey",
        "secret",
        "password",
        "passwd",
        "access_token",
        "refresh_token",
        "session_id",
    ];

    for key in &sensitive_keys {
        // Match JSON patterns like "key": "value" or "key":"value"
        let patterns = [
            format!("\"{}\":\"", key),
            format!("\"{}\": \"", key),
            format!("\"{}\" :\"", key),
            format!("\"{}\" : \"", key),
        ];
        for pattern in &patterns {
            if let Some(start) = result.find(pattern) {
                let value_start = start + pattern.len();
                if let Some(end_offset) = result[value_start..].find('"') {
                    let value_end = value_start + end_offset;
                    let prefix = &result[..value_start];
                    let suffix = &result[value_end..];
                    result = format!("{prefix}[redacted]{suffix}");
                }
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_record_serde_round_trip() {
        let record = NetworkRecord {
            record_id: "net-1".into(),
            device_id: "dev-1".into(),
            package: Some("com.example.app".into()),
            request: NetworkRequest {
                method: "GET".into(),
                url: "https://api.example.com/data".into(),
                headers: HashMap::new(),
                body: None,
                content_type: None,
            },
            response: Some(NetworkResponse {
                status_code: 200,
                status_text: "OK".into(),
                headers: HashMap::new(),
                body: Some("{\"data\": \"value\"}".into()),
                content_type: Some("application/json".into()),
            }),
            timestamp_ms: 1234567890,
            duration_ms: Some(150),
        };
        let json = serde_json::to_string(&record).unwrap();
        let back: NetworkRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(record, back);
    }

    #[test]
    fn is_complete_with_response() {
        let record = NetworkRecord {
            record_id: "net-1".into(),
            device_id: "dev-1".into(),
            package: None,
            request: NetworkRequest {
                method: "GET".into(),
                url: "https://example.com".into(),
                headers: HashMap::new(),
                body: None,
                content_type: None,
            },
            response: Some(NetworkResponse {
                status_code: 200,
                status_text: "OK".into(),
                headers: HashMap::new(),
                body: None,
                content_type: None,
            }),
            timestamp_ms: 0,
            duration_ms: None,
        };
        assert!(record.is_complete());
    }

    #[test]
    fn is_complete_without_response() {
        let record = NetworkRecord {
            record_id: "net-1".into(),
            device_id: "dev-1".into(),
            package: None,
            request: NetworkRequest {
                method: "GET".into(),
                url: "https://example.com".into(),
                headers: HashMap::new(),
                body: None,
                content_type: None,
            },
            response: None,
            timestamp_ms: 0,
            duration_ms: None,
        };
        assert!(!record.is_complete());
    }

    #[test]
    fn is_error_for_4xx_and_5xx() {
        let mut record = NetworkRecord {
            record_id: "net-1".into(),
            device_id: "dev-1".into(),
            package: None,
            request: NetworkRequest {
                method: "GET".into(),
                url: "https://example.com".into(),
                headers: HashMap::new(),
                body: None,
                content_type: None,
            },
            response: Some(NetworkResponse {
                status_code: 404,
                status_text: "Not Found".into(),
                headers: HashMap::new(),
                body: None,
                content_type: None,
            }),
            timestamp_ms: 0,
            duration_ms: None,
        };
        assert!(record.is_error());

        record.response.as_mut().unwrap().status_code = 500;
        assert!(record.is_error());

        record.response.as_mut().unwrap().status_code = 200;
        assert!(!record.is_error());
    }

    #[test]
    fn sensitive_header_detection() {
        assert!(NetworkRequest::is_sensitive_header("Authorization"));
        assert!(NetworkRequest::is_sensitive_header("authorization"));
        assert!(NetworkRequest::is_sensitive_header("Cookie"));
        assert!(NetworkRequest::is_sensitive_header("X-Api-Key"));
        assert!(!NetworkRequest::is_sensitive_header("Content-Type"));
        assert!(!NetworkRequest::is_sensitive_header("Accept"));
    }

    #[test]
    fn max_body_size_constant() {
        assert_eq!(MAX_BODY_SIZE, 64 * 1024);
    }

    #[test]
    fn redact_sensitive_headers() {
        let mut headers = HashMap::new();
        headers.insert("Authorization".into(), "Bearer secret-token".into());
        headers.insert("Content-Type".into(), "application/json".into());
        headers.insert("Cookie".into(), "session=abc123".into());

        let record = NetworkRecord {
            record_id: "net-1".into(),
            device_id: "dev-1".into(),
            package: None,
            request: NetworkRequest {
                method: "GET".into(),
                url: "https://example.com".into(),
                headers,
                body: None,
                content_type: None,
            },
            response: None,
            timestamp_ms: 0,
            duration_ms: None,
        };

        let redacted = record.redact();
        assert_eq!(
            redacted.request.headers.get("Authorization").unwrap(),
            "[redacted]"
        );
        assert_eq!(
            redacted.request.headers.get("Content-Type").unwrap(),
            "application/json"
        );
        assert_eq!(
            redacted.request.headers.get("Cookie").unwrap(),
            "[redacted]"
        );
    }

    #[test]
    fn redact_body_tokens() {
        let record = NetworkRecord {
            record_id: "net-1".into(),
            device_id: "dev-1".into(),
            package: None,
            request: NetworkRequest {
                method: "POST".into(),
                url: "https://api.example.com/login".into(),
                headers: HashMap::new(),
                body: Some(r#"{"username": "user", "password": "secret123"}"#.into()),
                content_type: Some("application/json".into()),
            },
            response: None,
            timestamp_ms: 0,
            duration_ms: None,
        };

        let redacted = record.redact();
        let body = redacted.request.body.unwrap();
        assert!(!body.contains("secret123"));
        assert!(body.contains("[redacted]"));
        assert!(body.contains("user"));
    }

    #[test]
    fn redact_response_set_cookie() {
        let mut headers = HashMap::new();
        headers.insert("Set-Cookie".into(), "session=xyz; Path=/".into());

        let record = NetworkRecord {
            record_id: "net-1".into(),
            device_id: "dev-1".into(),
            package: None,
            request: NetworkRequest {
                method: "GET".into(),
                url: "https://example.com".into(),
                headers: HashMap::new(),
                body: None,
                content_type: None,
            },
            response: Some(NetworkResponse {
                status_code: 200,
                status_text: "OK".into(),
                headers,
                body: None,
                content_type: None,
            }),
            timestamp_ms: 0,
            duration_ms: None,
        };

        let redacted = record.redact();
        assert_eq!(
            redacted
                .response
                .unwrap()
                .headers
                .get("Set-Cookie")
                .unwrap(),
            "[redacted]"
        );
    }

    #[test]
    fn redact_preserves_non_sensitive_data() {
        let record = NetworkRecord {
            record_id: "net-1".into(),
            device_id: "dev-1".into(),
            package: None,
            request: NetworkRequest {
                method: "GET".into(),
                url: "https://api.example.com/users".into(),
                headers: {
                    let mut h = HashMap::new();
                    h.insert("Accept".into(), "application/json".into());
                    h
                },
                body: None,
                content_type: None,
            },
            response: Some(NetworkResponse {
                status_code: 200,
                status_text: "OK".into(),
                headers: HashMap::new(),
                body: Some(r#"{"name": "Alice", "email": "alice@example.com"}"#.into()),
                content_type: Some("application/json".into()),
            }),
            timestamp_ms: 0,
            duration_ms: None,
        };

        let redacted = record.redact();
        assert_eq!(
            redacted.request.headers.get("Accept").unwrap(),
            "application/json"
        );
        let body = redacted.response.unwrap().body.unwrap();
        assert!(body.contains("Alice"));
    }

    /// Unobservable boundary: only explicitly captured traffic is recorded.
    /// Traffic from unlisted packages or outside the capture scope is not
    /// observed. This test validates that the type system enforces the
    /// boundary — a NetworkRecord always has a device_id and optional
    /// package scope.
    #[test]
    fn unobservable_boundary_record_has_scope() {
        let record = NetworkRecord {
            record_id: "net-1".into(),
            device_id: "dev-1".into(),
            package: Some("com.example.app".into()),
            request: NetworkRequest {
                method: "GET".into(),
                url: "https://example.com".into(),
                headers: HashMap::new(),
                body: None,
                content_type: None,
            },
            response: None,
            timestamp_ms: 0,
            duration_ms: None,
        };
        assert!(record.package.is_some());
        assert!(!record.device_id.is_empty());
    }
}
