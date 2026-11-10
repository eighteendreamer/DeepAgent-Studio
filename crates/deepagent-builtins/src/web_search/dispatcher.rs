use std::time::Duration;

use async_trait::async_trait;
use futures::future::join_all;
use reqwest::{Client, Response};
use tokio_util::sync::CancellationToken;

use deepagent_core::clock::Clock;
use deepagent_core::error::{CoreError, Result};

use super::normalize::{normalize_engine_results, NormalizedResult};
use super::types::{
    EngineAttempt, EngineError, EngineId, EngineSettings, EvidenceFeatures, ResultEvidence,
    ResultSource, SearchErrorCode, SearchEvidence, SearchOutput, SearchRequest, SearchResult,
    SearchRuntimeConfig, SearchStrategy,
};

const RRF_K: f32 = 60.0;
const TIMELY_MAX_BONUS: f64 = 0.25;
const TIMELY_HALF_LIFE_SECS: f64 = 30.0 * 24.0 * 60.0 * 60.0;

#[derive(Clone, Default)]
pub(super) struct SearchEndpointOverrides {
    #[cfg(test)]
    deepseek: Option<String>,
    #[cfg(test)]
    duckduckgo: Option<String>,
}

impl SearchEndpointOverrides {
    pub(super) fn deepseek_url(&self) -> &str {
        #[cfg(test)]
        if let Some(endpoint) = &self.deepseek {
            return endpoint;
        }
        super::engines::DEEPSEEK_ENDPOINT
    }

    pub(super) fn duckduckgo_url(&self) -> &str {
        #[cfg(test)]
        if let Some(endpoint) = &self.duckduckgo {
            return endpoint;
        }
        super::engines::DUCKDUCKGO_ENDPOINT
    }
}

#[async_trait]
pub trait SearchEngineClient: Send + Sync {
    async fn search(
        &self,
        engine: EngineId,
        request: &SearchRequest,
        config: &SearchRuntimeConfig,
        timeout: Duration,
    ) -> std::result::Result<Vec<SearchResult>, EngineError>;
}

pub struct ReqwestSearchEngineClient {
    http: Client,
    endpoints: SearchEndpointOverrides,
}

impl Default for ReqwestSearchEngineClient {
    fn default() -> Self {
        Self::new()
    }
}

impl ReqwestSearchEngineClient {
    pub fn new() -> Self {
        Self {
            http: Client::builder()
                .user_agent("DeepAgentStudio/0.1 WebSearch")
                .build()
                .unwrap_or_default(),
            endpoints: SearchEndpointOverrides::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_test_endpoints(deepseek: String, duckduckgo: String) -> Self {
        Self {
            endpoints: SearchEndpointOverrides {
                deepseek: Some(deepseek),
                duckduckgo: Some(duckduckgo),
            },
            ..Self::new()
        }
    }

    pub async fn search_one(
        &self,
        engine: EngineId,
        request: &SearchRequest,
        config: &SearchRuntimeConfig,
        timeout: Duration,
    ) -> std::result::Result<Vec<SearchResult>, EngineError> {
        super::engines::search(
            &self.http,
            engine,
            request,
            config,
            timeout,
            &self.endpoints,
        )
        .await
    }
}

#[async_trait]
impl SearchEngineClient for ReqwestSearchEngineClient {
    async fn search(
        &self,
        engine: EngineId,
        request: &SearchRequest,
        config: &SearchRuntimeConfig,
        timeout: Duration,
    ) -> std::result::Result<Vec<SearchResult>, EngineError> {
        self.search_one(engine, request, config, timeout).await
    }
}

pub(crate) async fn read_bounded(
    response: Response,
    engine: EngineId,
) -> std::result::Result<Vec<u8>, EngineError> {
    use super::normalize::MAX_ENGINE_RESPONSE_BYTES;
    use futures::StreamExt;
    if response
        .content_length()
        .is_some_and(|len| len > MAX_ENGINE_RESPONSE_BYTES as u64)
    {
        return Err(EngineError::new(engine, SearchErrorCode::ProtocolError));
    }
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| EngineError::new(engine, SearchErrorCode::TransportError))?;
        if body.len().saturating_add(chunk.len()) > MAX_ENGINE_RESPONSE_BYTES {
            return Err(EngineError::new(engine, SearchErrorCode::ProtocolError));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

pub async fn dispatch(
    client: &dyn SearchEngineClient,
    config: &SearchRuntimeConfig,
    request: &SearchRequest,
    timeout: Duration,
    cancel: CancellationToken,
    clock: &dyn Clock,
) -> Result<SearchOutput> {
    let enabled: Vec<&EngineSettings> = config
        .settings
        .engines
        .iter()
        .filter(|engine| engine.enabled)
        .collect();
    if enabled.is_empty() {
        return Err(CoreError::other("工具不可用，请开启搜索引擎"));
    }
    let execution = async {
        if config.settings.strategy == SearchStrategy::Verify && enabled.len() > 1 {
            dispatch_verify(client, config, request, timeout, &enabled, clock).await
        } else {
            dispatch_fallback(client, config, request, timeout, &enabled, clock).await
        }
    };
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(CoreError::other("web_search_cancelled")),
        result = tokio::time::timeout(timeout, execution) => match result {
            Ok(result) => result,
            Err(_) => Err(CoreError::other("web_search_timeout")),
        }
    }
}

async fn dispatch_fallback(
    client: &dyn SearchEngineClient,
    config: &SearchRuntimeConfig,
    request: &SearchRequest,
    timeout: Duration,
    enabled: &[&EngineSettings],
    clock: &dyn Clock,
) -> Result<SearchOutput> {
    let mut attempts = Vec::new();
    let mut selected = None;
    let mut errors = Vec::new();
    for setting in enabled {
        match client.search(setting.id, request, config, timeout).await {
            Ok(rows) => match normalize_engine_results(setting.id, rows, request.count as usize) {
                Ok(rows) => {
                    attempts.push(EngineAttempt {
                        engine: setting.id,
                        ok: true,
                        result_count: rows.len(),
                        error: None,
                    });
                    if !rows.is_empty() {
                        selected = Some(vec![(setting.id, rows)]);
                        break;
                    }
                }
                Err(error) => {
                    attempts.push(attempt_error(error.clone()));
                    errors.push(error);
                }
            },
            Err(error) => {
                attempts.push(attempt_error(error.clone()));
                errors.push(error);
            }
        }
    }
    if selected.is_none() && !errors.is_empty() && attempts.iter().all(|attempt| !attempt.ok) {
        return Err(error_output(&errors));
    }
    let incomplete = !errors.is_empty();
    build_output(
        request,
        selected.unwrap_or_default(),
        attempts,
        incomplete,
        config,
        clock,
    )
}

async fn dispatch_verify(
    client: &dyn SearchEngineClient,
    config: &SearchRuntimeConfig,
    request: &SearchRequest,
    timeout: Duration,
    enabled: &[&EngineSettings],
    clock: &dyn Clock,
) -> Result<SearchOutput> {
    let calls = enabled
        .iter()
        .map(|setting| client.search(setting.id, request, config, timeout));
    let responses = join_all(calls).await;
    let mut attempts = Vec::new();
    let mut batches = Vec::new();
    let mut errors = Vec::new();
    for (setting, response) in enabled.iter().zip(responses) {
        match response {
            Ok(rows) => match normalize_engine_results(setting.id, rows, request.count as usize) {
                Ok(rows) => {
                    attempts.push(EngineAttempt {
                        engine: setting.id,
                        ok: true,
                        result_count: rows.len(),
                        error: None,
                    });
                    batches.push((setting.id, rows));
                }
                Err(error) => {
                    attempts.push(attempt_error(error.clone()));
                    errors.push(error);
                }
            },
            Err(error) => {
                attempts.push(attempt_error(error.clone()));
                errors.push(error);
            }
        }
    }
    if batches.is_empty() && !errors.is_empty() {
        return Err(error_output(&errors));
    }
    build_output(
        request,
        batches,
        attempts,
        !errors.is_empty(),
        config,
        clock,
    )
}

fn attempt_error(error: EngineError) -> EngineAttempt {
    EngineAttempt {
        engine: error.engine,
        ok: false,
        result_count: 0,
        error: Some(error),
    }
}

fn error_output(errors: &[EngineError]) -> CoreError {
    let unsupported = errors
        .iter()
        .all(|error| error.code == SearchErrorCode::UnsupportedParameter);
    let code = if unsupported {
        "unsupported_parameter"
    } else {
        "search_failed"
    };
    let engines = errors
        .iter()
        .map(|error| error.engine.label())
        .collect::<Vec<_>>()
        .join(", ");
    CoreError::other(format!("{code}: all configured engines failed ({engines})"))
}

fn build_output(
    request: &SearchRequest,
    batches: Vec<(EngineId, Vec<NormalizedResult>)>,
    attempts: Vec<EngineAttempt>,
    incomplete: bool,
    config: &SearchRuntimeConfig,
    clock: &dyn Clock,
) -> Result<SearchOutput> {
    let retrieved_at = clock.now();
    let retrieved_at_ms = retrieved_at.as_millis();
    let mut source_lists = Vec::new();
    let mut entries = std::collections::HashMap::<String, (SearchResult, Vec<ResultSource>)>::new();
    for (engine, batch) in batches {
        let mut source_list = Vec::new();
        for NormalizedResult {
            identity,
            rank,
            result,
        } in batch
        {
            source_list.push((identity.clone(), rank));
            let entry = entries
                .entry(identity)
                .or_insert_with(|| (result.clone(), Vec::new()));
            entry.1.push(ResultSource {
                engine,
                rank,
                result,
            });
        }
        source_lists.push(source_list);
    }
    let lists: Vec<_> = source_lists.iter().map(Vec::as_slice).collect();
    let fused = deepagent_memory::fusion::reciprocal_rank_fusion_ranked(&lists, RRF_K, Ord::cmp);
    let mut ranked: Vec<_> = fused
        .into_iter()
        .map(|(url, rrf)| {
            let (result, sources) = entries.remove(&url).expect("fused URL was collected");
            let recency_bonus = if config.settings.ranking.profile
                == super::types::RankingProfile::Timely
            {
                result
                    .date
                    .as_deref()
                    .and_then(|date| {
                        time::OffsetDateTime::parse(
                            date,
                            &time::format_description::well_known::Rfc3339,
                        )
                        .ok()
                    })
                    .map(|date| {
                        let age = (retrieved_at.to_datetime().unix_timestamp()
                            - date.unix_timestamp())
                        .max(0) as f64;
                        f64::from(rrf) * TIMELY_MAX_BONUS * 2f64.powf(-age / TIMELY_HALF_LIFE_SECS)
                    })
                    .unwrap_or(0.0)
            } else {
                0.0
            };
            let rrf_score = f64::from(rrf);
            let system_score = rrf_score + recency_bonus;
            let contributing: std::collections::HashSet<_> =
                sources.iter().map(|source| source.engine).collect();
            let mut reasons = vec![format!(
                "RRF across {} enabled engine result list(s)",
                contributing.len()
            )];
            if recency_bonus > 0.0 {
                reasons
                    .push("bounded 30-day half-life publication-date bonus (maximum 25%)".into());
            }
            (
                url,
                result,
                sources,
                rrf_score,
                recency_bonus,
                system_score,
                reasons,
            )
        })
        .collect();
    ranked.sort_by(|a, b| b.5.total_cmp(&a.5).then_with(|| a.0.cmp(&b.0)));
    ranked.truncate(request.count as usize);
    let results: Vec<_> = ranked.iter().map(|item| item.1.clone()).collect();
    let rankings = ranked
        .into_iter()
        .map(
            |(
                source_id,
                result,
                sources,
                rrf_score,
                recency_bonus,
                system_score,
                ranking_reasons,
            )| {
                ResultEvidence {
                    source_id,
                    url: result.url.clone(),
                    sources,
                    features: EvidenceFeatures {
                        has_content: !result.content.is_empty(),
                        has_snippet: !result.snippet.is_empty(),
                        has_summary: !result.summary.is_empty(),
                        has_publication_date: result.date.is_some(),
                        query_relevance: None,
                        first_party: None,
                        independent_support: None,
                        contradictory_evidence: None,
                    },
                    rrf_score,
                    recency_bonus,
                    system_score,
                    ranking_reasons,
                }
            },
        )
        .collect();
    Ok(SearchOutput {
        query: request.query.clone(),
        count: results.len(),
        results,
        evidence: SearchEvidence {
            retrieved_at_ms,
            strategy: config.settings.strategy,
            incomplete,
            attempts,
            rankings,
        },
    })
}

#[cfg(test)]
#[path = "dispatcher_tests.rs"]
mod tests;
