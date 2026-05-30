//! A `reqwest`-backed [`WebClient`] (only compiled with `--features http`).
//!
//! Fetches a URL and reduces HTML to readable text via a tiny tag-stripping
//! pass (no heavy HTML crate): scripts/styles are dropped, tags removed, and
//! whitespace collapsed. This keeps the dependency surface small while giving
//! the model usable page text. Search is left unimplemented here (the default
//! trait impl reports "unavailable"); the desktop app wires a real search
//! provider.

use std::time::Duration;

use async_trait::async_trait;

use deepagent_core::error::{CoreError, Result};

use crate::web_tools::WebClient;

/// `reqwest`-backed web client with a fetch timeout.
pub struct ReqwestWebClient {
    client: reqwest::Client,
}

impl Default for ReqwestWebClient {
    fn default() -> Self {
        Self::new()
    }
}

impl ReqwestWebClient {
    /// Build a client with a 30s timeout and a descriptive user agent.
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("DeepAgentStudio/0.1 (+web_fetch)")
            .build()
            .unwrap_or_default();
        Self { client }
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
}
