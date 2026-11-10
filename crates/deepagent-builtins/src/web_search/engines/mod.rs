mod anysearch;
mod deepseek;
mod duckduckgo;
mod searxng;

pub(super) const DEEPSEEK_ENDPOINT: &str = deepseek::ENDPOINT;
pub(super) const DUCKDUCKGO_ENDPOINT: &str = duckduckgo::ENDPOINT;

#[cfg(test)]
#[path = "http_tests.rs"]
mod http_tests;

use std::time::Duration;

use reqwest::{Client, Url};

use super::dispatcher::SearchEndpointOverrides;
use super::types::{
    EngineError, EngineId, EngineSettings, SearchRequest, SearchResult, SearchRuntimeConfig,
};

pub(super) async fn search(
    client: &Client,
    engine: EngineId,
    request: &SearchRequest,
    config: &SearchRuntimeConfig,
    timeout: Duration,
    endpoints: &SearchEndpointOverrides,
) -> std::result::Result<Vec<SearchResult>, EngineError> {
    // These fields have DeepSeek-only semantics. Never silently discard a
    // caller's explicit intent or reinterpret an IP as language/region.
    if engine != EngineId::Deepseek
        && (request.ori_question.is_some() || !request.localization.is_empty())
    {
        return Err(EngineError::new(
            engine,
            super::types::SearchErrorCode::UnsupportedParameter,
        ));
    }
    let settings = config
        .settings
        .engines
        .iter()
        .find(|e| e.id == engine)
        .expect("dispatcher only selects listed engine settings");
    match engine {
        EngineId::Deepseek => {
            deepseek::search(
                client,
                request,
                config.deepseek_api_key.as_deref(),
                timeout,
                endpoints.deepseek_url(),
            )
            .await
        }
        EngineId::Anysearch => {
            anysearch::search(
                client,
                request,
                settings,
                config.anysearch_api_key.as_deref(),
                timeout,
            )
            .await
        }
        EngineId::Searxng => searxng::search(client, request, settings, timeout).await,
        EngineId::Duckduckgo => {
            duckduckgo::search(client, request, timeout, endpoints.duckduckgo_url()).await
        }
    }
}

fn configured_url(
    engine: EngineId,
    setting: &EngineSettings,
) -> std::result::Result<Url, EngineError> {
    let Some(base) = setting.base_url.as_deref() else {
        return Err(EngineError::new(
            engine,
            super::types::SearchErrorCode::ConfigurationMissing,
        ));
    };
    let Ok(mut url) = Url::parse(base) else {
        return Err(EngineError::new(
            engine,
            super::types::SearchErrorCode::ConfigurationMissing,
        ));
    };
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(EngineError::new(
            engine,
            super::types::SearchErrorCode::ConfigurationMissing,
        ));
    }
    while url.path().ends_with('/') && url.path() != "/" {
        let path = url.path().trim_end_matches('/').to_string();
        url.set_path(&path);
    }
    Ok(url)
}
