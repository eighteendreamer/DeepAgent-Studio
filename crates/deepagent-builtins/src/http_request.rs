//! Typed HTTP requests routed through the normal tool permission/approval pipeline.
//! The host supplies the policy-enforcing client; nodes never own HTTP clients.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use deepagent_core::error::{CoreError, Result};
use deepagent_tools::{
    Permission, PermissionSet, RiskLevel, Tool, ToolDescriptor, ToolExecutionContext, ToolOutput,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Stable internal tool name used by workflow HTTP nodes.
pub const HTTP_REQUEST_TOOL: &str = "http_request";
/// Maximum outbound body bytes, before making a connection.
pub const MAX_REQUEST_BYTES: usize = 64 * 1024;
/// Maximum inbound body bytes (both declared and streamed).
pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
/// Maximum combined outbound header bytes.
pub const MAX_HEADER_BYTES: usize = 16 * 1024;
/// Maximum URL bytes.
pub const MAX_URL_BYTES: usize = 8192;

fn default_method() -> String {
    "GET".into()
}
fn default_timeout() -> u64 {
    30
}

/// Request shape shared by the tool and the host's network broker.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpRequest {
    /// Explicit HTTP(S) URL; no ambient base URL or credentials.
    pub url: String,
    /// HTTP verb. Unknown methods are rejected, never silently converted to GET.
    #[serde(default = "default_method")]
    pub method: String,
    /// Explicit request body.
    #[serde(default)]
    pub body: String,
    /// Explicit request headers.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Total deadline, including DNS, redirects and body reading.
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// GET/HEAD connection-establishment retries only; never replay a dispatched request.
    #[serde(default)]
    pub retry_count: u32,
}

impl HttpRequest {
    /// Validate bounded request data without DNS or other network activity.
    pub fn validate(&self) -> Result<()> {
        if !matches!(
            self.method.as_str(),
            "GET" | "HEAD" | "OPTIONS" | "POST" | "PUT" | "DELETE" | "PATCH"
        ) {
            return Err(CoreError::invalid("HttpInputInvalid: unsupported method"));
        }
        if self.url.is_empty()
            || self.url.len() > MAX_URL_BYTES
            || self.body.len() > MAX_REQUEST_BYTES
            || self
                .headers
                .iter()
                .map(|(key, value)| key.len() + value.len())
                .sum::<usize>()
                > MAX_HEADER_BYTES
        {
            return Err(CoreError::invalid(
                "HttpInputTooLarge: URL, body or headers exceed the request limits",
            ));
        }
        if !(1..=120).contains(&self.timeout_secs) || self.retry_count > 3 {
            return Err(CoreError::invalid(
                "HttpInputInvalid: timeout must be 1..120s and retry_count 0..3",
            ));
        }
        if self.retry_count > 0 && !matches!(self.method.as_str(), "GET" | "HEAD") {
            return Err(CoreError::invalid(
                "HttpRetryDenied: non-read requests cannot be automatically retried",
            ));
        }
        Ok(())
    }
}

/// Host-owned network broker. Implementations must validate all resolved IPs,
/// pin the connection, validate every redirect, and enforce cancellation/limits.
/// Injection is a trusted host capability, never a workflow JSON option.
#[async_trait]
pub trait HttpRequestClient: Send + Sync {
    /// Validate the destination/header policy without performing DNS or I/O.
    fn validate(&self, request: &HttpRequest) -> Result<()>;
    /// Execute an already authorized request with the invocation's cancellation.
    async fn request(&self, request: HttpRequest, context: ToolExecutionContext) -> Result<Value>;
}

/// A high-risk network tool: approvals cannot substitute for Network capability
/// or override a disabled run network broker.
pub struct HttpRequestTool {
    client: Arc<dyn HttpRequestClient>,
    enabled: bool,
}

impl HttpRequestTool {
    /// Bind the run's immutable network capability and host broker once.
    pub fn new(client: Arc<dyn HttpRequestClient>, enabled: bool) -> Self {
        Self { client, enabled }
    }

    fn parse(&self, arguments: Value) -> Result<HttpRequest> {
        if !self.enabled {
            return Err(CoreError::invalid(
                "NetworkDenied: selected sandbox has no HTTP broker; host fallback is forbidden",
            ));
        }
        let request: HttpRequest = serde_json::from_value(arguments)
            .map_err(|_| CoreError::invalid("HttpInputInvalid: invalid typed HTTP request"))?;
        request.validate()?;
        self.client.validate(&request)?;
        Ok(request)
    }
}

#[async_trait]
impl Tool for HttpRequestTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: HTTP_REQUEST_TOOL.into(),
            description: "Make one approved public HTTP(S) request through the host network broker. Private targets and cross-origin redirects are denied; ambiguous outcomes are never replayed.".into(),
            parameters: serde_json::json!({
                "type":"object", "additionalProperties":false,
                "properties":{
                    "url":{"type":"string","minLength":1,"maxLength":MAX_URL_BYTES},
                    "method":{"enum":["GET","HEAD","OPTIONS","POST","PUT","DELETE","PATCH"]},
                    "body":{"type":"string","maxLength":MAX_REQUEST_BYTES},
                    "headers":{"type":"object","additionalProperties":{"type":"string"}},
                    "timeout_secs":{"type":"integer","minimum":1,"maximum":120},
                    "retry_count":{"type":"integer","minimum":0,"maximum":3}
                }, "required":["url"]
            }),
            risk: RiskLevel::High,
            required_permissions: PermissionSet::from_iter_perms([Permission::Network]),
        }
    }

    fn validate_input(&self, arguments: Value) -> Result<Value> {
        Ok(serde_json::to_value(self.parse(arguments)?)?)
    }

    async fn invoke(&self, arguments: Value) -> Result<ToolOutput> {
        self.invoke_with_context(arguments, ToolExecutionContext::default())
            .await
    }

    async fn invoke_with_context(
        &self,
        arguments: Value,
        context: ToolExecutionContext,
    ) -> Result<ToolOutput> {
        let request = self.parse(arguments)?;
        self.client
            .request(request, context)
            .await
            .map(ToolOutput::success)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_tools::ToolRegistry;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct Client(AtomicUsize);

    #[async_trait]
    impl HttpRequestClient for Client {
        fn validate(&self, request: &HttpRequest) -> Result<()> {
            request.validate()
        }
        async fn request(&self, _: HttpRequest, _: ToolExecutionContext) -> Result<Value> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(serde_json::json!({"body":"OK","status_code":200,"headers":{}}))
        }
    }

    #[tokio::test]
    async fn network_capability_approval_and_enabled_broker_are_independent() {
        let client = Arc::new(Client::default());
        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(HttpRequestTool::new(client.clone(), true)))
            .unwrap();
        let input = serde_json::json!({"url":"https://fixture.invalid/"});
        assert!(registry
            .invoke(
                HTTP_REQUEST_TOOL,
                input.clone(),
                &PermissionSet::read_only(),
                true
            )
            .await
            .is_err());
        assert!(registry
            .invoke(
                HTTP_REQUEST_TOOL,
                input.clone(),
                &PermissionSet::developer(),
                false
            )
            .await
            .is_err());
        assert_eq!(client.0.load(Ordering::SeqCst), 0);
        assert!(
            registry
                .invoke(
                    HTTP_REQUEST_TOOL,
                    input.clone(),
                    &PermissionSet::developer(),
                    true
                )
                .await
                .unwrap()
                .ok
        );
        assert_eq!(client.0.load(Ordering::SeqCst), 1);
        let disabled = HttpRequestTool::new(client.clone(), false);
        assert!(disabled.invoke(input).await.is_err());
        assert_eq!(client.0.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn rejects_invalid_method_unbounded_requests_and_unsafe_retries() {
        let tool = HttpRequestTool::new(Arc::new(Client::default()), true);
        for input in [
            serde_json::json!({"url":"https://fixture.invalid/","method":"NOT_A_METHOD"}),
            serde_json::json!({"url":"https://fixture.invalid/","timeout_secs":0}),
            serde_json::json!({"url":"https://fixture.invalid/","timeout_secs":121}),
            serde_json::json!({"url":"https://fixture.invalid/","retry_count":4}),
            serde_json::json!({"url":"https://fixture.invalid/","method":"POST","retry_count":1}),
            serde_json::json!({"url":"https://fixture.invalid/","body":"x".repeat(MAX_REQUEST_BYTES+1)}),
            serde_json::json!({"url":"https://fixture.invalid/","headers":{"X-Large":"x".repeat(MAX_HEADER_BYTES+1)}}),
        ] {
            assert!(tool.validate_input(input).is_err());
        }
    }
}
