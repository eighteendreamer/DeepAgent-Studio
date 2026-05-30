//! Web tools, aligned with Claude Code's `WebFetch` / `WebSearch`.
//!
//! Both require outbound network ([`Permission::Network`]) and are classified
//! [`RiskLevel::Medium`]. The network itself is abstracted behind the
//! [`WebClient`] trait so the tools are fully testable offline (via a mock) and
//! the real `reqwest`-backed client lives behind the `http` feature — mirroring
//! the pattern in `deepagent-models` / `deepagent-mcp`.
//!
//! - `web_fetch` — fetch a URL and return readable text. Large HTML is
//!   converted to a compact text form and truncated to a byte cap (matching
//!   Claude Code's "truncate before HTML→markdown" behaviour) so a huge page
//!   never floods the context.
//! - `web_search` — run a search query and return result rows (title / url /
//!   snippet). The backing engine is provided by the host (the desktop app
//!   wires a real search provider); offline it returns an explanatory error.

use async_trait::async_trait;

use deepagent_core::error::{CoreError, Result};
use deepagent_tools::permission::{Permission, PermissionSet, RiskLevel};
use deepagent_tools::{Tool, ToolDescriptor, ToolOutput};

/// Default cap on returned web text (bytes), to protect the context window.
pub const DEFAULT_MAX_BYTES: usize = 100_000;

/// A single web-search result row.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SearchResult {
    /// Result title.
    pub title: String,
    /// Result URL.
    pub url: String,
    /// Short snippet / description.
    pub snippet: String,
}

/// Abstracts outbound web access for the web tools.
///
/// `fetch` returns the (already text-extracted) body of a URL. `search` returns
/// result rows for a query. Implementations enforce their own timeouts; the
/// tools apply the byte cap on top.
#[async_trait]
pub trait WebClient: Send + Sync {
    /// Fetch `url` and return readable text (HTML reduced to text).
    async fn fetch(&self, url: &str) -> Result<String>;

    /// Run a search `query`, returning up to `limit` results. The default
    /// implementation reports that search is unavailable.
    async fn search(&self, _query: &str, _limit: usize) -> Result<Vec<SearchResult>> {
        Err(CoreError::other(
            "web search is not configured for this client",
        ))
    }
}

/// Reject non-HTTP(S) URLs and obvious SSRF-y local targets before fetching.
fn validate_url(url: &str) -> Result<()> {
    let lower = url.to_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        return Err(CoreError::invalid(
            "web_fetch only supports http(s) URLs".to_string(),
        ));
    }
    // Block obvious internal/loopback/metadata targets (defense in depth; the
    // real client may add more). This is a coarse guard, not a full SSRF filter.
    for bad in [
        "://localhost",
        "://127.",
        "://0.0.0.0",
        "://169.254.169.254", // cloud metadata
        "://[::1]",
        "://10.",
        "://192.168.",
    ] {
        if lower.contains(bad) {
            return Err(CoreError::invalid(format!(
                "web_fetch refuses internal/loopback target: {url}"
            )));
        }
    }
    Ok(())
}

/// Truncate `text` to at most `max_bytes`, on a char boundary, appending a
/// marker when truncated.
fn truncate_text(text: &str, max_bytes: usize) -> (String, bool) {
    if text.len() <= max_bytes {
        return (text.to_string(), false);
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = text[..end].to_string();
    out.push_str("\n…[truncated]");
    (out, true)
}

/// `web_fetch` — fetch a URL and return readable text (truncated to a cap).
pub struct WebFetchTool<C: WebClient> {
    client: C,
    max_bytes: usize,
}

impl<C: WebClient> WebFetchTool<C> {
    /// Build with a web client and the default byte cap.
    pub fn new(client: C) -> Self {
        Self {
            client,
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }

    /// Override the byte cap (builder).
    pub fn with_max_bytes(mut self, max_bytes: usize) -> Self {
        self.max_bytes = max_bytes;
        self
    }
}

#[async_trait]
impl<C: WebClient> Tool for WebFetchTool<C> {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "web_fetch".into(),
            description: "Fetch a public http(s) URL and return its readable text content \
                (truncated to a size cap). Args: { url }."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "url": { "type": "string" } },
                "required": ["url"]
            }),
            risk: RiskLevel::Medium,
            required_permissions: PermissionSet::from_iter_perms([Permission::Network]),
        }
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let Some(url) = args.get("url").and_then(|v| v.as_str()) else {
            return Ok(ToolOutput::failure("missing 'url'"));
        };
        if let Err(e) = validate_url(url) {
            return Ok(ToolOutput::failure(e.to_string()));
        }
        match self.client.fetch(url).await {
            Ok(body) => {
                let (text, truncated) = truncate_text(&body, self.max_bytes);
                Ok(ToolOutput::success(serde_json::json!({
                    "url": url,
                    "content": text,
                    "bytes": body.len(),
                    "truncated": truncated,
                })))
            }
            Err(e) => Ok(ToolOutput::failure(format!("fetch failed: {e}"))),
        }
    }
}

/// `web_search` — run a search query and return result rows.
pub struct WebSearchTool<C: WebClient> {
    client: C,
    default_limit: usize,
}

impl<C: WebClient> WebSearchTool<C> {
    /// Build with a web client (default limit 5).
    pub fn new(client: C) -> Self {
        Self {
            client,
            default_limit: 5,
        }
    }
}

#[async_trait]
impl<C: WebClient> Tool for WebSearchTool<C> {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "web_search".into(),
            description: "Search the web and return result rows (title, url, snippet). \
                Args: { query, limit? }."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "limit": { "type": "integer" }
                },
                "required": ["query"]
            }),
            risk: RiskLevel::Medium,
            required_permissions: PermissionSet::from_iter_perms([Permission::Network]),
        }
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let Some(query) = args.get("query").and_then(|v| v.as_str()) else {
            return Ok(ToolOutput::failure("missing 'query'"));
        };
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(self.default_limit)
            .clamp(1, 20);
        match self.client.search(query, limit).await {
            Ok(results) => Ok(ToolOutput::success(serde_json::json!({
                "query": query,
                "results": results,
                "count": results.len(),
            }))),
            Err(e) => Ok(ToolOutput::failure(format!("search failed: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A canned web client for offline tests.
    struct MockWeb;

    #[async_trait]
    impl WebClient for MockWeb {
        async fn fetch(&self, url: &str) -> Result<String> {
            Ok(format!("fetched body of {url}"))
        }
        async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
            Ok((0..limit)
                .map(|i| SearchResult {
                    title: format!("Result {i} for {query}"),
                    url: format!("https://example.com/{i}"),
                    snippet: "snippet".into(),
                })
                .collect())
        }
    }

    #[test]
    fn url_validation() {
        assert!(validate_url("https://example.com").is_ok());
        assert!(validate_url("http://example.com").is_ok());
        assert!(validate_url("ftp://example.com").is_err());
        assert!(validate_url("file:///etc/passwd").is_err());
        assert!(validate_url("http://localhost:8080").is_err());
        assert!(validate_url("http://169.254.169.254/latest/meta-data").is_err());
        assert!(validate_url("http://192.168.1.1").is_err());
    }

    #[test]
    fn truncation() {
        let (s, t) = truncate_text("hello", 100);
        assert_eq!(s, "hello");
        assert!(!t);
        let (s, t) = truncate_text("hello world", 5);
        assert!(t);
        assert!(s.starts_with("hello"));
        assert!(s.contains("truncated"));
    }

    #[tokio::test]
    async fn web_fetch_returns_text() {
        let tool = WebFetchTool::new(MockWeb);
        let out = tool
            .invoke(serde_json::json!({"url": "https://example.com/doc"}))
            .await
            .unwrap();
        assert!(out.ok);
        assert!(out.value["content"]
            .as_str()
            .unwrap()
            .contains("example.com/doc"));
        assert_eq!(out.value["truncated"], false);
    }

    #[tokio::test]
    async fn web_fetch_rejects_bad_url() {
        let tool = WebFetchTool::new(MockWeb);
        let out = tool
            .invoke(serde_json::json!({"url": "file:///etc/passwd"}))
            .await
            .unwrap();
        assert!(!out.ok);
    }

    #[tokio::test]
    async fn web_fetch_truncates_large_body() {
        let tool = WebFetchTool::new(MockWeb).with_max_bytes(10);
        let out = tool
            .invoke(serde_json::json!({"url": "https://example.com/huge-page-with-long-url"}))
            .await
            .unwrap();
        assert!(out.ok);
        assert_eq!(out.value["truncated"], true);
    }

    #[tokio::test]
    async fn web_search_returns_rows() {
        let tool = WebSearchTool::new(MockWeb);
        let out = tool
            .invoke(serde_json::json!({"query": "rust async", "limit": 3}))
            .await
            .unwrap();
        assert!(out.ok);
        assert_eq!(out.value["count"], 3);
        assert_eq!(out.value["results"][0]["url"], "https://example.com/0");
    }

    #[tokio::test]
    async fn web_search_default_client_unavailable() {
        struct NoSearch;
        #[async_trait]
        impl WebClient for NoSearch {
            async fn fetch(&self, _url: &str) -> Result<String> {
                Ok(String::new())
            }
        }
        let tool = WebSearchTool::new(NoSearch);
        let out = tool
            .invoke(serde_json::json!({"query": "x"}))
            .await
            .unwrap();
        assert!(!out.ok); // default search() errors → failure output
    }
}
