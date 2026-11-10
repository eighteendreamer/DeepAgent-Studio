use std::time::Duration;

use super::super::types::{
    EngineError, EngineId, EngineSettings, SearchErrorCode, SearchRequest, SearchResult,
};
use reqwest::Client;
use serde::Deserialize;

#[derive(Deserialize)]
struct Response {
    results: Vec<Row>,
}
#[derive(Deserialize)]
struct Row {
    title: Option<String>,
    url: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default, rename = "publishedDate")]
    published_date: Option<String>,
}

pub(super) async fn search(
    client: &Client,
    request: &SearchRequest,
    setting: &EngineSettings,
    timeout: Duration,
) -> std::result::Result<Vec<SearchResult>, EngineError> {
    if request.need_content {
        return Err(EngineError::new(
            EngineId::Searxng,
            SearchErrorCode::UnsupportedParameter,
        ));
    }
    let mut endpoint = super::configured_url(EngineId::Searxng, setting)?;
    endpoint.set_path(&format!("{}/search", endpoint.path().trim_end_matches('/')));
    endpoint
        .query_pairs_mut()
        .append_pair("q", &request.query)
        .append_pair("format", "json");
    let response = client
        .get(endpoint)
        .timeout(timeout)
        .send()
        .await
        .map_err(|_| EngineError::new(EngineId::Searxng, SearchErrorCode::TransportError))?;
    if !response.status().is_success() {
        return Err(EngineError {
            engine: EngineId::Searxng,
            code: SearchErrorCode::HttpError,
            http_status: Some(response.status().as_u16()),
        });
    }
    let bytes = super::super::dispatcher::read_bounded(response, EngineId::Searxng).await?;
    let parsed: Response = serde_json::from_slice(&bytes)
        .map_err(|_| EngineError::new(EngineId::Searxng, SearchErrorCode::ProtocolError))?;
    Ok(parsed
        .results
        .into_iter()
        .map(|row| SearchResult {
            title: row.title.unwrap_or_default(),
            url: row.url,
            snippet: row.content.unwrap_or_default(),
            summary: String::new(),
            content: String::new(),
            date: row.published_date,
            rerank_score: None,
            authority: -1,
            site_name: None,
            // SearXNG's thumbnail is a result preview, not a site icon.
            site_icon: None,
        })
        .collect())
}
