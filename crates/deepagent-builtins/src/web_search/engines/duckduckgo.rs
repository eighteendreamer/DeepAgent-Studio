use std::time::Duration;

use super::super::types::{EngineError, EngineId, SearchErrorCode, SearchRequest, SearchResult};
use reqwest::Client;

pub(crate) const ENDPOINT: &str = "https://html.duckduckgo.com/html/";

pub(super) async fn search(
    client: &Client,
    request: &SearchRequest,
    timeout: Duration,
    endpoint: &str,
) -> std::result::Result<Vec<SearchResult>, EngineError> {
    if request.need_content {
        return Err(EngineError::new(
            EngineId::Duckduckgo,
            SearchErrorCode::UnsupportedParameter,
        ));
    }
    let response = client
        .post(endpoint)
        .form(&[("q", request.query.as_str())])
        .timeout(timeout)
        .send()
        .await
        .map_err(|_| EngineError::new(EngineId::Duckduckgo, SearchErrorCode::TransportError))?;
    if !response.status().is_success() {
        return Err(EngineError {
            engine: EngineId::Duckduckgo,
            code: SearchErrorCode::HttpError,
            http_status: Some(response.status().as_u16()),
        });
    }
    let bytes = super::super::dispatcher::read_bounded(response, EngineId::Duckduckgo).await?;
    let html = std::str::from_utf8(&bytes)
        .map_err(|_| EngineError::new(EngineId::Duckduckgo, SearchErrorCode::MarkupChanged))?;
    let lower = html.to_ascii_lowercase();
    if [
        "anomaly-modal",
        "captcha",
        "challenge-form",
        "bots use duckduckgo",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
    {
        return Err(EngineError::new(
            EngineId::Duckduckgo,
            SearchErrorCode::Challenge,
        ));
    }
    if ["no-results", "no results found"]
        .iter()
        .any(|marker| lower.contains(marker))
    {
        return Ok(Vec::new());
    }
    let rows = parse_duckduckgo(html, request.count as usize);
    if rows.is_empty() {
        return Err(EngineError::new(
            EngineId::Duckduckgo,
            SearchErrorCode::MarkupChanged,
        ));
    }
    Ok(rows)
}

fn parse_duckduckgo(html: &str, limit: usize) -> Vec<SearchResult> {
    let mut output = Vec::new();
    let mut rest = html;
    while output.len() < limit {
        let Some(position) = rest.find("result__a") else {
            break;
        };
        let anchor = &rest[position..];
        let Some(href) = extract_attr(anchor, "href") else {
            rest = &anchor[9..];
            continue;
        };
        let url = decode_ddg_redirect(&href);
        let title = anchor
            .find('>')
            .and_then(|gt| {
                anchor[gt + 1..]
                    .find("</a>")
                    .map(|end| decode_entities(&strip_tags(&anchor[gt + 1..gt + 1 + end])))
            })
            .unwrap_or_default();
        let snippet = anchor
            .find("result__snippet")
            .and_then(|start| {
                let snippet = &anchor[start..];
                snippet.find('>').and_then(|gt| {
                    snippet[gt + 1..]
                        .find("</a>")
                        .map(|end| decode_entities(&strip_tags(&snippet[gt + 1..gt + 1 + end])))
                })
            })
            .unwrap_or_default();
        if !title.trim().is_empty() {
            output.push(SearchResult::text(
                title.trim().into(),
                url,
                snippet.trim().into(),
            ));
        }
        rest = &anchor[9..];
    }
    output
}

fn extract_attr(fragment: &str, attribute: &str) -> Option<String> {
    let needle = format!("{attribute}=\"");
    let start = fragment.find(&needle)? + needle.len();
    let end = fragment[start..].find('"')? + start;
    Some(fragment[start..end].to_owned())
}

fn decode_ddg_redirect(href: &str) -> String {
    if let Some(position) = href.find("uddg=") {
        let encoded = href[position + 5..].split('&').next().unwrap_or_default();
        if let Some((_, value)) =
            url::form_urlencoded::parse(format!("x={encoded}").as_bytes()).next()
        {
            if !value.is_empty() {
                return value.into_owned();
            }
        }
    }
    if let Some(host) = href.strip_prefix("//") {
        return format!("https://{host}");
    }
    href.to_owned()
}

fn strip_tags(value: &str) -> String {
    let mut output = String::new();
    let mut in_tag = false;
    for c in value.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => output.push(c),
            _ => {}
        }
    }
    output
}

fn decode_entities(value: &str) -> String {
    value
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
}
