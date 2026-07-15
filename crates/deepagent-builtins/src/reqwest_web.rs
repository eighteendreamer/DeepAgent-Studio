//! A `reqwest`-backed [`WebClient`] (only compiled with `--features http`).
//!
//! Fetches a URL and reduces HTML to readable text via a tiny tag-stripping
//! pass (no heavy HTML crate): scripts/styles are dropped, tags removed, and
//! whitespace collapsed. This keeps the dependency surface small while giving
//! the model usable page text.
//!
//! Search is backed by DuckDuckGo's keyless HTML endpoint
//! (`https://html.duckduckgo.com/html/`), so `web_search` works out of the box
//! with **no API key** — matching the situation where the user has only set a
//! model API key. Result rows are scraped from the returned HTML.

use std::time::Duration;

use async_trait::async_trait;

use deepagent_core::error::{CoreError, Result};

use crate::web_tools::{SearchAttempt, SearchResponse, SearchResult, WebClient};

/// Configuration for DeepSeek's Anthropic-compatible server web-search tool.
#[derive(Debug, Clone)]
pub struct DeepSeekWebSearchConfig {
    /// DeepSeek API key.
    pub api_key: String,
    /// DeepSeek base URL, usually `https://api.deepseek.com`.
    pub base_url: String,
    /// Model used to invoke hosted web search.
    pub model: String,
    /// Maximum server-side search uses for one call.
    pub max_uses: usize,
}

/// Configuration for AnySearch's unified Search API.
#[derive(Debug, Clone)]
pub struct AnySearchConfig {
    /// Optional AnySearch API key. Empty keys use anonymous access.
    pub api_key: Option<String>,
    /// AnySearch API base URL, usually `https://api.anysearch.com`.
    pub base_url: String,
}

const ANYSEARCH_DEFAULT_BASE_URL: &str = "https://api.anysearch.com";

impl AnySearchConfig {
    /// Build a config. Empty base URLs fall back to AnySearch's public endpoint.
    pub fn new(api_key: Option<String>, base_url: impl Into<String>) -> Self {
        let base_url = base_url.into();
        Self {
            api_key: api_key
                .map(|key| key.trim().to_string())
                .filter(|key| !key.is_empty()),
            base_url: if base_url.trim().is_empty() {
                ANYSEARCH_DEFAULT_BASE_URL.to_string()
            } else {
                normalize_anysearch_base_url(&base_url)
            },
        }
    }
}

fn normalize_anysearch_base_url(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    if matches!(trimmed, "https://www.anysearch.com" | "https://anysearch.com") {
        ANYSEARCH_DEFAULT_BASE_URL.to_string()
    } else {
        trimmed.to_string()
    }
}

impl DeepSeekWebSearchConfig {
    /// Build a config. Empty base URLs fall back to DeepSeek's API endpoint;
    /// the model is intentionally not defaulted here so the host can decide.
    pub fn new(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        let base_url = base_url.into();
        let model = model.into();
        Self {
            api_key: api_key.into(),
            base_url: if base_url.trim().is_empty() {
                "https://api.deepseek.com".to_string()
            } else {
                base_url
            },
            model: model.trim().to_string(),
            max_uses: 1,
        }
    }
}

#[derive(Debug, Clone)]
enum SearchProvider {
    AnySearch(AnySearchConfig),
    DeepSeek(DeepSeekWebSearchConfig),
    Searxng { base_url: String },
    DuckDuckGo,
}

impl SearchProvider {
    fn name(&self) -> &'static str {
        match self {
            Self::AnySearch(_) => "anysearch",
            Self::DeepSeek(_) => "deepseek",
            Self::Searxng { .. } => "searxng",
            Self::DuckDuckGo => "duckduckgo",
        }
    }
}

/// `reqwest`-backed web client with a fetch timeout.
pub struct ReqwestWebClient {
    client: reqwest::Client,
    search_providers: Vec<SearchProvider>,
}

impl Default for ReqwestWebClient {
    fn default() -> Self {
        Self::new()
    }
}

impl ReqwestWebClient {
    /// Build a client with a 30s timeout and a descriptive user agent.
    pub fn new() -> Self {
        Self::with_search_config(deepseek_config_from_env(), searxng_url_from_env())
    }

    /// Build a client with an optional DeepSeek-first search provider.
    pub fn with_deepseek_search(deepseek: Option<DeepSeekWebSearchConfig>) -> Self {
        Self::with_search_config(deepseek, searxng_url_from_env())
    }

    /// Build a client with explicit search providers.
    pub fn with_search_config(
        deepseek: Option<DeepSeekWebSearchConfig>,
        searxng_url: Option<String>,
    ) -> Self {
        Self::with_search_chain(None, deepseek, searxng_url)
    }

    /// Build a client with explicit search providers, optionally AnySearch first.
    pub fn with_search_chain(
        anysearch: Option<AnySearchConfig>,
        deepseek: Option<DeepSeekWebSearchConfig>,
        searxng_url: Option<String>,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            // A browser-like UA so search/content endpoints return real HTML
            // rather than a bot challenge page.
            .user_agent(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/120.0 Safari/537.36 DeepAgentStudio/0.1",
            )
            .build()
            .unwrap_or_default();
        Self {
            client,
            search_providers: build_search_providers(anysearch, deepseek, searxng_url),
        }
    }
}

#[async_trait]
impl WebClient for ReqwestWebClient {
    async fn fetch(&self, url: &str) -> Result<String> {
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| CoreError::other(format!("GET {url} failed: {e}")))?;
        if !resp.status().is_success() {
            return Err(CoreError::other(format!(
                "GET {url} returned {}",
                resp.status()
            )));
        }
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_lowercase();
        let body = resp
            .text()
            .await
            .map_err(|e| CoreError::other(format!("reading {url} body failed: {e}")))?;

        if content_type.contains("text/html") || looks_like_html(&body) {
            Ok(html_to_text(&body))
        } else {
            Ok(body)
        }
    }

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        Ok(self.search_response(query, limit).await?.results)
    }

    async fn search_response(&self, query: &str, limit: usize) -> Result<SearchResponse> {
        let mut attempts = Vec::new();
        for provider in &self.search_providers {
            let provider_name = provider.name();
            let result = match provider {
                SearchProvider::AnySearch(config) => {
                    self.search_anysearch(config, query, limit).await
                }
                SearchProvider::DeepSeek(config) => {
                    self.search_deepseek(config, query, limit).await
                }
                SearchProvider::Searxng { base_url } => {
                    self.search_searxng(base_url, query, limit).await
                }
                SearchProvider::DuckDuckGo => self.search_duckduckgo(query, limit).await,
            };
            match result {
                Ok(results) if !results.is_empty() => {
                    attempts.push(SearchAttempt {
                        provider: provider_name.to_string(),
                        ok: true,
                        count: Some(results.len()),
                        error: None,
                    });
                    return Ok(SearchResponse {
                        provider: provider_name.to_string(),
                        results,
                        attempts,
                    });
                }
                Ok(_) => attempts.push(SearchAttempt {
                    provider: provider_name.to_string(),
                    ok: false,
                    count: Some(0),
                    error: Some("returned no results".to_string()),
                }),
                Err(e) => attempts.push(SearchAttempt {
                    provider: provider_name.to_string(),
                    ok: false,
                    count: None,
                    error: Some(e.to_string()),
                }),
            }
        }
        let errors = attempts
            .iter()
            .map(|attempt| {
                format!(
                    "{}: {}",
                    attempt.provider,
                    attempt
                        .error
                        .as_deref()
                        .unwrap_or("returned no parseable results")
                )
            })
            .collect::<Vec<_>>();
        Err(CoreError::other(format!(
            "all search providers failed: {}",
            errors.join(" | ")
        )))
    }
}

impl ReqwestWebClient {
    async fn search_anysearch(
        &self,
        config: &AnySearchConfig,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        let endpoint = format!("{}/v1/search", config.base_url.trim_end_matches('/'));
        let max_results = limit.clamp(1, 100);
        let body = serde_json::json!({
            "query": query,
            "max_results": max_results
        });
        let mut request = self
            .client
            .post(&endpoint)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT, "application/json")
            .body(body.to_string());
        if let Some(api_key) = config.api_key.as_deref().filter(|key| !key.trim().is_empty()) {
            request = request.bearer_auth(api_key.trim());
        }
        let resp = request
            .send()
            .await
            .map_err(|e| CoreError::other(format!("AnySearch request failed: {e}")))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let detail = resp.text().await.unwrap_or_default();
            let detail = truncate_error_detail(&detail, 500);
            return Err(CoreError::other(format!(
                "AnySearch returned {status}: {detail}"
            )));
        }
        let text = resp
            .text()
            .await
            .map_err(|e| CoreError::other(format!("reading AnySearch body failed: {e}")))?;
        parse_anysearch_results(&text, limit)
    }

    async fn search_deepseek(
        &self,
        config: &DeepSeekWebSearchConfig,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        if config.api_key.trim().is_empty() {
            return Err(CoreError::other("DeepSeek API key is empty"));
        }
        let endpoint = deepseek_anthropic_messages_endpoint(&config.base_url);
        let body = serde_json::json!({
            "model": config.model,
            "max_tokens": 512,
            "messages": [{
                "role": "user",
                "content": format!(
                    "Search the web for this query and return the most relevant source results. Query: {query}"
                )
            }],
            "tools": [{
                "type": "web_search_20250305",
                "name": "web_search",
                "max_uses": config.max_uses.clamp(1, 5)
            }]
        });
        let resp = self
            .client
            .post(&endpoint)
            .bearer_auth(config.api_key.trim())
            .header("x-api-key", config.api_key.trim())
            .header("anthropic-version", "2023-06-01")
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT, "application/json")
            .body(body.to_string())
            .send()
            .await
            .map_err(|e| CoreError::other(format!("DeepSeek web search request failed: {e}")))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let detail = resp.text().await.unwrap_or_default();
            return Err(CoreError::other(format!(
                "DeepSeek web search returned {status}: {detail}"
            )));
        }
        let text = resp
            .text()
            .await
            .map_err(|e| CoreError::other(format!("reading DeepSeek search body failed: {e}")))?;
        parse_deepseek_web_search_results(&text, limit)
    }

    async fn search_searxng(
        &self,
        base_url: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        let url = format!("{}/search", base_url.trim_end_matches('/'));
        let resp = self
            .client
            .get(&url)
            .query(&[("q", query), ("format", "json")])
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|e| CoreError::other(format!("SearXNG request failed: {e}")))?;
        if !resp.status().is_success() {
            return Err(CoreError::other(format!(
                "SearXNG returned {}",
                resp.status()
            )));
        }
        let text = resp
            .text()
            .await
            .map_err(|e| CoreError::other(format!("reading SearXNG body failed: {e}")))?;
        parse_searxng_results(&text, limit)
    }

    async fn search_duckduckgo(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        // DuckDuckGo's HTML endpoint needs no API key and returns server-rendered
        // result rows we can scrape. POSTing the query avoids some bot checks.
        let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
        let body = format!("q={encoded}");
        let resp = self
            .client
            .post("https://html.duckduckgo.com/html/")
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(body)
            .send()
            .await
            .map_err(|e| CoreError::other(format!("DuckDuckGo request failed: {e}")))?;
        if !resp.status().is_success() {
            return Err(CoreError::other(format!(
                "DuckDuckGo returned {}",
                resp.status()
            )));
        }
        let html = resp
            .text()
            .await
            .map_err(|e| CoreError::other(format!("reading DuckDuckGo body failed: {e}")))?;
        let results = parse_ddg_results(&html, limit);
        if results.is_empty() {
            return Err(CoreError::other(
                "DuckDuckGo returned no parseable results (markup may have changed)",
            ));
        }
        Ok(results)
    }
}

fn truncate_error_detail(detail: &str, max_chars: usize) -> String {
    let compact = detail.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= max_chars {
        return compact;
    }
    let mut truncated = compact.chars().take(max_chars).collect::<String>();
    truncated.push_str("...");
    truncated
}

fn build_search_providers(
    anysearch: Option<AnySearchConfig>,
    deepseek: Option<DeepSeekWebSearchConfig>,
    searxng_url: Option<String>,
) -> Vec<SearchProvider> {
    let mut providers = Vec::new();
    if let Some(config) = anysearch {
        providers.push(SearchProvider::AnySearch(config));
    }
    if let Some(config) =
        deepseek.filter(|c| !c.api_key.trim().is_empty() && !c.model.trim().is_empty())
    {
        providers.push(SearchProvider::DeepSeek(config));
    }
    if let Some(base_url) = searxng_url.filter(|s| !s.trim().is_empty()) {
        providers.push(SearchProvider::Searxng { base_url });
    }
    providers.push(SearchProvider::DuckDuckGo);
    providers
}

fn deepseek_config_from_env() -> Option<DeepSeekWebSearchConfig> {
    let api_key =
        env_value("DEEPAGENT_DEEPSEEK_API_KEY").or_else(|| env_value("DEEPSEEK_API_KEY"))?;
    let base_url = env_value("DEEPAGENT_DEEPSEEK_BASE_URL")
        .unwrap_or_else(|| "https://api.deepseek.com".to_string());
    let model = env_value("DEEPAGENT_DEEPSEEK_WEB_SEARCH_MODEL")?;
    Some(DeepSeekWebSearchConfig::new(api_key, base_url, model))
}

fn searxng_url_from_env() -> Option<String> {
    env_value("DEEPAGENT_SEARXNG_URL").or_else(|| env_value("SEARXNG_URL"))
}

fn env_value(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn deepseek_anthropic_messages_endpoint(base_url: &str) -> String {
    let base = env_value("DEEPAGENT_DEEPSEEK_ANTHROPIC_BASE_URL")
        .unwrap_or_else(|| base_url.trim_end_matches('/').to_string());
    let base = base.trim_end_matches('/');
    if base.ends_with("/v1/messages") {
        base.to_string()
    } else if base.ends_with("/v1") {
        format!("{base}/messages")
    } else if base.ends_with("/anthropic") {
        format!("{base}/v1/messages")
    } else {
        format!("{base}/anthropic/v1/messages")
    }
}

fn parse_deepseek_web_search_results(body: &str, limit: usize) -> Result<Vec<SearchResult>> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| CoreError::other(format!("parsing DeepSeek search JSON failed: {e}")))?;
    let mut results = Vec::new();
    collect_deepseek_results(&value, limit, &mut results);
    dedupe_results(&mut results);
    results.truncate(limit);
    if results.is_empty() {
        return Err(CoreError::other(
            "DeepSeek web search returned no parseable result rows",
        ));
    }
    Ok(results)
}

fn collect_deepseek_results(value: &serde_json::Value, limit: usize, out: &mut Vec<SearchResult>) {
    if out.len() >= limit {
        return;
    }
    match value {
        serde_json::Value::Object(map) => {
            let is_search_result = map
                .get("type")
                .and_then(|v| v.as_str())
                .map(|s| s == "web_search_result")
                .unwrap_or(false);
            if is_search_result {
                if let Some(result) = result_from_json_object(map) {
                    out.push(result);
                }
            }
            for child in map.values() {
                collect_deepseek_results(child, limit, out);
                if out.len() >= limit {
                    break;
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_deepseek_results(item, limit, out);
                if out.len() >= limit {
                    break;
                }
            }
        }
        _ => {}
    }
}

fn parse_searxng_results(body: &str, limit: usize) -> Result<Vec<SearchResult>> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| CoreError::other(format!("parsing SearXNG JSON failed: {e}")))?;
    let mut results = value
        .get("results")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_object())
        .filter_map(result_from_json_object)
        .collect::<Vec<_>>();
    dedupe_results(&mut results);
    results.truncate(limit);
    if results.is_empty() {
        return Err(CoreError::other("SearXNG returned no parseable results"));
    }
    Ok(results)
}

fn parse_anysearch_results(body: &str, limit: usize) -> Result<Vec<SearchResult>> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| CoreError::other(format!("parsing AnySearch JSON failed: {e}")))?;
    let rows = value
        .get("results")
        .and_then(|v| v.as_array())
        .or_else(|| {
            value
                .get("data")
                .and_then(|data| data.get("results"))
                .and_then(|v| v.as_array())
        })
        .ok_or_else(|| CoreError::other("AnySearch response missing results array"))?;
    let mut results = rows
        .iter()
        .filter_map(|v| v.as_object())
        .filter_map(result_from_anysearch_object)
        .collect::<Vec<_>>();
    dedupe_results(&mut results);
    results.truncate(limit);
    if results.is_empty() {
        return Err(CoreError::other("AnySearch returned no parseable results"));
    }
    Ok(results)
}

fn result_from_anysearch_object(
    map: &serde_json::Map<String, serde_json::Value>,
) -> Option<SearchResult> {
    let url = json_string(map, &["url", "href", "link"])?;
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return None;
    }
    let title = json_string(map, &["title", "name"]).unwrap_or_else(|| url.clone());
    let snippet = json_string(map, &["snippet", "summary", "description", "content"])
        .map(|s| truncate_snippet(&s, 500))
        .unwrap_or_default();
    Some(SearchResult {
        title,
        url,
        snippet,
    })
}

fn result_from_json_object(
    map: &serde_json::Map<String, serde_json::Value>,
) -> Option<SearchResult> {
    let url = json_string(map, &["url", "href", "link"])?;
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return None;
    }
    let title = json_string(map, &["title", "name"]).unwrap_or_else(|| url.clone());
    let snippet =
        json_string(map, &["content", "snippet", "description", "page_age"]).unwrap_or_default();
    Some(SearchResult {
        title: title.trim().to_string(),
        url,
        snippet: snippet.trim().to_string(),
    })
}

fn truncate_snippet(value: &str, max_chars: usize) -> String {
    let trimmed = value.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let mut snippet = trimmed.chars().take(max_chars).collect::<String>();
    snippet.push_str("...");
    snippet
}

fn json_string(map: &serde_json::Map<String, serde_json::Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        map.get(*key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
    })
}

fn dedupe_results(results: &mut Vec<SearchResult>) {
    let mut seen = std::collections::HashSet::new();
    results.retain(|r| seen.insert(r.url.clone()));
}

/// Parse DuckDuckGo HTML result rows into [`SearchResult`]s (best-effort).
///
/// The HTML endpoint renders each hit as an `<a class="result__a" href=...>`
/// title link followed by an `<a class="result__snippet">` description. DDG
/// wraps outbound links in a redirect (`/l/?uddg=<encoded-url>`); we decode that
/// back to the real target. This is intentionally a light scraper (no HTML
/// crate); if the markup drifts, `search` reports "no parseable results".
fn parse_ddg_results(html: &str, limit: usize) -> Vec<SearchResult> {
    let mut out = Vec::new();
    let mut rest = html;
    while out.len() < limit {
        let Some(a_rel) = rest.find("result__a") else {
            break;
        };
        let after_class = &rest[a_rel..];
        // href="..."
        let Some(href) = extract_attr(after_class, "href") else {
            rest = &after_class[9..];
            continue;
        };
        let url = decode_ddg_redirect(&href);
        // The title is the text between this <a ...> and its </a>.
        let title = after_class
            .find('>')
            .and_then(|gt| {
                after_class[gt + 1..]
                    .find("</a>")
                    .map(|end| strip_tags(&after_class[gt + 1..gt + 1 + end]))
            })
            .unwrap_or_default();
        // Snippet: the next result__snippet block after this title.
        let snippet = after_class
            .find("result__snippet")
            .and_then(|s| {
                let seg = &after_class[s..];
                seg.find('>').and_then(|gt| {
                    seg[gt + 1..]
                        .find("</a>")
                        .map(|end| strip_tags(&seg[gt + 1..gt + 1 + end]))
                })
            })
            .unwrap_or_default();

        if !url.is_empty() && !title.trim().is_empty() {
            out.push(SearchResult {
                title: decode_entities(title.trim()),
                url,
                snippet: decode_entities(snippet.trim()),
            });
        }
        // Advance past this match to find the next row.
        rest = &after_class[9..];
    }
    out
}

/// Extract the value of `attr="..."` from the start of an HTML tag fragment.
fn extract_attr(fragment: &str, attr: &str) -> Option<String> {
    let needle = format!("{attr}=\"");
    let start = fragment.find(&needle)? + needle.len();
    let end = fragment[start..].find('"')? + start;
    Some(fragment[start..end].to_string())
}

/// Decode DuckDuckGo's `/l/?uddg=<encoded>` redirect wrapper to the real URL.
fn decode_ddg_redirect(href: &str) -> String {
    let marker = "uddg=";
    if let Some(pos) = href.find(marker) {
        let encoded = &href[pos + marker.len()..];
        // Stop at the next param separator.
        let encoded = encoded.split('&').next().unwrap_or(encoded);
        let decoded: String = url::form_urlencoded::parse(format!("x={encoded}").as_bytes())
            .next()
            .map(|(_, v)| v.into_owned())
            .unwrap_or_default();
        if !decoded.is_empty() {
            return decoded;
        }
    }
    // Protocol-relative links → https.
    if let Some(stripped) = href.strip_prefix("//") {
        return format!("https://{stripped}");
    }
    href.to_string()
}

/// Remove any HTML tags from a fragment, returning the text content.
fn strip_tags(fragment: &str) -> String {
    let mut out = String::with_capacity(fragment.len());
    let mut in_tag = false;
    for ch in fragment.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

/// Decode the handful of HTML entities DDG emits in titles/snippets.
fn decode_entities(s: &str) -> String {
    s.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
}

/// Cheap heuristic: does this look like HTML?
fn looks_like_html(s: &str) -> bool {
    let head = &s[..s.len().min(512)].to_lowercase();
    head.contains("<!doctype html") || head.contains("<html") || head.contains("<body")
}

/// Reduce HTML to readable text without a full parser: drop script/style
/// blocks, strip tags, decode a few common entities, and collapse whitespace.
fn html_to_text(html: &str) -> String {
    let without_blocks = strip_blocks(html, "script");
    let without_blocks = strip_blocks(&without_blocks, "style");

    let mut out = String::with_capacity(without_blocks.len() / 2);
    let mut in_tag = false;
    for ch in without_blocks.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }

    let out = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");

    // Collapse runs of whitespace, preserving paragraph breaks.
    let mut collapsed = String::with_capacity(out.len());
    let mut last_blank = false;
    for line in out.lines() {
        let trimmed = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if trimmed.is_empty() {
            if !last_blank {
                collapsed.push('\n');
            }
            last_blank = true;
        } else {
            collapsed.push_str(&trimmed);
            collapsed.push('\n');
            last_blank = false;
        }
    }
    collapsed.trim().to_string()
}

/// Remove `<tag>…</tag>` blocks (case-insensitive) entirely.
fn strip_blocks(html: &str, tag: &str) -> String {
    let lower = html.to_lowercase();
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut out = String::with_capacity(html.len());
    let mut i = 0;
    while i < html.len() {
        if let Some(rel) = lower[i..].find(&open) {
            let start = i + rel;
            out.push_str(&html[i..start]);
            // Find the matching close tag after the open.
            if let Some(crel) = lower[start..].find(&close) {
                i = start + crel + close.len();
            } else {
                // No close tag; drop the rest.
                break;
            }
        } else {
            out.push_str(&html[i..]);
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_script_and_style() {
        let html = "<html><head><style>body{color:red}</style></head>\
                    <body>Hello <script>alert(1)</script>World</body></html>";
        let text = html_to_text(html);
        assert!(text.contains("Hello"));
        assert!(text.contains("World"));
        assert!(!text.contains("alert"));
        assert!(!text.contains("color:red"));
    }

    #[test]
    fn decodes_entities_and_collapses_ws() {
        let html = "<p>a &amp; b</p>\n\n\n<p>c   d</p>";
        let text = html_to_text(html);
        assert!(text.contains("a & b"));
        assert!(text.contains("c d"));
    }

    #[test]
    fn detects_html() {
        assert!(looks_like_html("<!DOCTYPE html><html>"));
        assert!(looks_like_html("<HTML><body>"));
        assert!(!looks_like_html("plain text response"));
    }

    #[test]
    fn decodes_ddg_redirect_to_real_url() {
        let href = "//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fpage&rut=abc";
        assert_eq!(decode_ddg_redirect(href), "https://example.com/page");
        // Protocol-relative passthrough.
        assert_eq!(
            decode_ddg_redirect("//example.org/x"),
            "https://example.org/x"
        );
    }

    #[test]
    fn parses_ddg_result_rows() {
        // A trimmed-down shape of DuckDuckGo's HTML endpoint output.
        let html = r#"
            <div class="result results_links">
              <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fweather.com%2Fchangsha">
                Changsha Weather &amp; Forecast
              </a>
              <a class="result__snippet" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fweather.com%2Fchangsha">
                Today in Changsha: <b>sunny</b>, 25&#176;C.
              </a>
            </div>
            <div class="result results_links">
              <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fb">
                Second Result
              </a>
              <a class="result__snippet">Another snippet</a>
            </div>
        "#;
        let results = parse_ddg_results(html, 5);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].url, "https://weather.com/changsha");
        assert_eq!(results[0].title, "Changsha Weather & Forecast");
        assert!(results[0].snippet.contains("sunny"));
        assert_eq!(results[1].url, "https://example.com/b");
        assert_eq!(results[1].title, "Second Result");
    }

    #[test]
    fn parser_respects_limit_and_skips_empty() {
        let html = r#"
            <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fa.com">One</a>
            <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fb.com">Two</a>
            <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fc.com">Three</a>
        "#;
        let results = parse_ddg_results(html, 2);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn parser_returns_empty_on_foreign_markup() {
        assert!(parse_ddg_results("<html><body>no results here</body></html>", 5).is_empty());
    }

    #[test]
    fn strip_tags_and_entities() {
        assert_eq!(strip_tags("a <b>bold</b> c"), "a bold c");
        assert_eq!(decode_entities("x &amp; y &#39;z&#39;"), "x & y 'z'");
    }

    #[test]
    fn builds_deepseek_anthropic_endpoint() {
        assert_eq!(
            deepseek_anthropic_messages_endpoint("https://api.deepseek.com"),
            "https://api.deepseek.com/anthropic/v1/messages"
        );
        assert_eq!(
            deepseek_anthropic_messages_endpoint("https://api.deepseek.com/anthropic"),
            "https://api.deepseek.com/anthropic/v1/messages"
        );
        assert_eq!(
            deepseek_anthropic_messages_endpoint("https://proxy.example/v1"),
            "https://proxy.example/v1/messages"
        );
    }

    #[test]
    fn parses_deepseek_web_search_tool_results() {
        let body = r#"
        {
          "content": [
            {
              "type": "web_search_tool_result",
              "content": [
                {
                  "type": "web_search_result",
                  "title": "DeepSeek Docs",
                  "url": "https://api-docs.deepseek.com/guides/anthropic_api",
                  "page_age": "June 2026"
                },
                {
                  "type": "web_search_result",
                  "title": "Ignored duplicate",
                  "url": "https://api-docs.deepseek.com/guides/anthropic_api"
                }
              ]
            }
          ]
        }
        "#;
        let results = parse_deepseek_web_search_results(body, 5).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "DeepSeek Docs");
        assert_eq!(
            results[0].url,
            "https://api-docs.deepseek.com/guides/anthropic_api"
        );
        assert_eq!(results[0].snippet, "June 2026");
    }

    #[test]
    fn parses_searxng_json_results() {
        let body = r#"
        {
          "results": [
            {
              "title": "Rust",
              "url": "https://www.rust-lang.org/",
              "content": "A language empowering everyone."
            },
            {
              "title": "Skip non-http",
              "url": "ftp://example.com/file"
            }
          ]
        }
        "#;
        let results = parse_searxng_results(body, 5).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Rust");
        assert_eq!(results[0].snippet, "A language empowering everyone.");
    }

    #[test]
    fn parses_anysearch_json_results() {
        let body = r#"
        {
          "results": [
            {
              "title": "AnySearch Docs",
              "url": "https://www.anysearch.com/docs",
              "snippet": "Unified search infrastructure"
            },
            {
              "title": "Skip non-http",
              "url": "mailto:support@example.com"
            }
          ],
          "metadata": {
            "total_results": 2
          }
        }
        "#;
        let results = parse_anysearch_results(body, 5).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "AnySearch Docs");
        assert_eq!(results[0].url, "https://www.anysearch.com/docs");
        assert_eq!(results[0].snippet, "Unified search infrastructure");
    }

    #[test]
    fn builds_provider_chain_in_deepseek_first_order() {
        let providers = build_search_providers(
            None,
            Some(DeepSeekWebSearchConfig::new(
                "sk-test",
                "https://api.deepseek.com",
                "test-search-model",
            )),
            Some("https://search.example.com".to_string()),
        );
        let names: Vec<&str> = providers.iter().map(SearchProvider::name).collect();
        assert_eq!(names, vec!["deepseek", "searxng", "duckduckgo"]);
    }

    #[test]
    fn builds_provider_chain_with_anysearch_first() {
        let providers = build_search_providers(
            Some(AnySearchConfig::new(
                Some("as-test".to_string()),
                "https://www.anysearch.com",
            )),
            Some(DeepSeekWebSearchConfig::new(
                "sk-test",
                "https://api.deepseek.com",
                "test-search-model",
            )),
            Some("https://search.example.com".to_string()),
        );
        let names: Vec<&str> = providers.iter().map(SearchProvider::name).collect();
        assert_eq!(names, vec!["anysearch", "deepseek", "searxng", "duckduckgo"]);
    }

    #[test]
    fn anysearch_config_maps_website_origin_to_api_origin() {
        let config = AnySearchConfig::new(Some("as-test".to_string()), "https://www.anysearch.com/");
        assert_eq!(config.base_url, "https://api.anysearch.com");
    }

    #[test]
    fn provider_chain_skips_deepseek_when_model_missing() {
        let providers = build_search_providers(
            None,
            Some(DeepSeekWebSearchConfig::new(
                "sk-test",
                "https://api.deepseek.com",
                "",
            )),
            Some("https://search.example.com".to_string()),
        );
        let names: Vec<&str> = providers.iter().map(SearchProvider::name).collect();
        assert_eq!(names, vec!["searxng", "duckduckgo"]);
    }
}
