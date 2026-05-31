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

use crate::web_tools::{SearchResult, WebClient};

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
            // A browser-like UA so search/content endpoints return real HTML
            // rather than a bot challenge page.
            .user_agent(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/120.0 Safari/537.36 DeepAgentStudio/0.1",
            )
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

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
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
            .map_err(|e| CoreError::other(format!("search request failed: {e}")))?;
        if !resp.status().is_success() {
            return Err(CoreError::other(format!(
                "search returned {}",
                resp.status()
            )));
        }
        let html = resp
            .text()
            .await
            .map_err(|e| CoreError::other(format!("reading search body failed: {e}")))?;
        let results = parse_ddg_results(&html, limit);
        if results.is_empty() {
            return Err(CoreError::other(
                "search returned no parseable results (the provider may have changed its markup)",
            ));
        }
        Ok(results)
    }
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
}
