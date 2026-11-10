use std::time::Duration;

use super::super::types::{
    EngineError, EngineId, EngineSettings, SearchErrorCode, SearchRequest, SearchResult,
};
use reqwest::Client;
use serde::Deserialize;

#[derive(Deserialize)]
struct Response {
    code: i64,
    message: String,
    request_id: String,
    data: Option<Data>,
}
#[derive(Deserialize)]
struct Data {
    results: Vec<Row>,
}
#[derive(Deserialize)]
struct Row {
    title: String,
    url: String,
    #[serde(default)]
    snippet: Option<String>,
    #[serde(default)]
    content: Option<String>,
}

pub(super) async fn search(
    client: &Client,
    request: &SearchRequest,
    setting: &EngineSettings,
    key: Option<&str>,
    timeout: Duration,
) -> std::result::Result<Vec<SearchResult>, EngineError> {
    let Some(key) = key.filter(|key| !key.trim().is_empty()) else {
        return Err(EngineError::new(
            EngineId::Anysearch,
            SearchErrorCode::ConfigurationMissing,
        ));
    };
    let base = super::configured_url(EngineId::Anysearch, setting)?;
    let mut endpoint = base;
    endpoint.set_path(&format!(
        "{}/v1/search",
        endpoint.path().trim_end_matches('/')
    ));
    let mut body =
        serde_json::json!({ "query": request.query, "max_results": request.count.min(10) });
    if request.need_content {
        body["format"] = serde_json::json!("markdown");
    }
    let response = client
        .post(endpoint)
        .bearer_auth(key)
        .json(&body)
        .timeout(timeout)
        .send()
        .await
        .map_err(|_| EngineError::new(EngineId::Anysearch, SearchErrorCode::TransportError))?;
    if !response.status().is_success() {
        return Err(EngineError {
            engine: EngineId::Anysearch,
            code: SearchErrorCode::HttpError,
            http_status: Some(response.status().as_u16()),
        });
    }
    let bytes = super::super::dispatcher::read_bounded(response, EngineId::Anysearch).await?;
    let parsed: Response = serde_json::from_slice(&bytes)
        .map_err(|_| EngineError::new(EngineId::Anysearch, SearchErrorCode::ProtocolError))?;
    let _ = (parsed.message, parsed.request_id);
    if parsed.code != 0 {
        return Err(EngineError::new(
            EngineId::Anysearch,
            SearchErrorCode::BusinessError,
        ));
    }
    let data = parsed
        .data
        .ok_or_else(|| EngineError::new(EngineId::Anysearch, SearchErrorCode::ProtocolError))?;
    Ok(data
        .results
        .into_iter()
        .map(|row| SearchResult {
            title: row.title,
            url: row.url,
            snippet: row.snippet.unwrap_or_default(),
            summary: String::new(),
            content: if request.need_content {
                row.content.unwrap_or_default()
            } else {
                String::new()
            },
            date: None,
            rerank_score: None,
            authority: -1,
            site_name: None,
            site_icon: None,
        })
        .collect())
}
