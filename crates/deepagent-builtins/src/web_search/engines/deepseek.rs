use std::time::Duration;

use super::super::types::{EngineError, EngineId, SearchErrorCode, SearchRequest, SearchResult};
use reqwest::{Client, StatusCode};
use serde::Deserialize;

pub(crate) const ENDPOINT: &str = "https://api.deepseek.com/v1/tools/search";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    results: Vec<Row>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    rerank_score: f64,
    title: String,
    url: String,
    snippet: String,
    summary: String,
    content: String,
    site_name: Option<String>,
    site_icon: Option<String>,
    date: Option<String>,
    authority: i8,
}

pub(super) async fn search(
    client: &Client,
    request: &SearchRequest,
    key: Option<&str>,
    timeout: Duration,
    endpoint: &str,
) -> std::result::Result<Vec<SearchResult>, EngineError> {
    let Some(key) = key.filter(|key| !key.trim().is_empty()) else {
        return Err(EngineError::new(
            EngineId::Deepseek,
            SearchErrorCode::ConfigurationMissing,
        ));
    };
    let body = serde_json::json!({
        "query": request.query,
        "count": request.count,
        "ori_question": request.ori_question,
        "localization": request.localization,
        "need_content": request.need_content,
    });
    let response = client
        .post(endpoint)
        .bearer_auth(key)
        .json(&body)
        .timeout(timeout)
        .send()
        .await
        .map_err(|_| EngineError::new(EngineId::Deepseek, SearchErrorCode::TransportError))?;
    if !response.status().is_success() {
        return Err(http_error(response.status()));
    }
    let bytes = super::super::dispatcher::read_bounded(response, EngineId::Deepseek).await?;
    let parsed: Response = serde_json::from_slice(&bytes)
        .map_err(|_| EngineError::new(EngineId::Deepseek, SearchErrorCode::ProtocolError))?;
    Ok(parsed
        .results
        .into_iter()
        .map(|row| SearchResult {
            title: row.title,
            url: row.url,
            snippet: row.snippet,
            summary: row.summary,
            content: if request.need_content {
                row.content
            } else {
                String::new()
            },
            date: row.date,
            rerank_score: Some(row.rerank_score),
            authority: row.authority,
            site_name: row.site_name,
            site_icon: row.site_icon,
        })
        .collect())
}

pub(super) fn http_error(status: StatusCode) -> EngineError {
    EngineError {
        engine: EngineId::Deepseek,
        code: SearchErrorCode::HttpError,
        http_status: Some(status.as_u16()),
    }
}
