use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use deepagent_core::clock::FixedClock;
use serde_json::json;
use tokio::sync::Barrier;

use super::*;
use crate::web_search::types::{RankingProfile, RankingSettings, SearchSettings};

struct FakeClient {
    responses: HashMap<EngineId, std::result::Result<Vec<SearchResult>, EngineError>>,
    calls: Mutex<Vec<EngineId>>,
    barrier: Option<Arc<Barrier>>,
}

#[async_trait]
impl SearchEngineClient for FakeClient {
    async fn search(
        &self,
        engine: EngineId,
        _request: &SearchRequest,
        _config: &SearchRuntimeConfig,
        _timeout: Duration,
    ) -> std::result::Result<Vec<SearchResult>, EngineError> {
        self.calls.lock().unwrap().push(engine);
        if let Some(barrier) = &self.barrier {
            barrier.wait().await;
        }
        self.responses.get(&engine).cloned().unwrap()
    }
}

fn runtime(
    ids: &[EngineId],
    strategy: SearchStrategy,
    ranking: RankingProfile,
) -> SearchRuntimeConfig {
    let mut settings = SearchSettings {
        strategy,
        ranking: RankingSettings {
            method: Default::default(),
            profile: ranking,
        },
        ..Default::default()
    };
    for engine in &mut settings.engines {
        engine.enabled = ids.contains(&engine.id);
    }
    SearchRuntimeConfig {
        settings,
        deepseek_api_key: None,
        anysearch_api_key: None,
    }
}

fn rows(engine: EngineId) -> Vec<SearchResult> {
    match engine {
        EngineId::Deepseek => vec![
            SearchResult {
                date: Some("2026-11-09T00:00:00Z".into()),
                rerank_score: Some(2.2),
                authority: 3,
                ..SearchResult::text(
                    "shared".into(),
                    "https://example.com/shared?utm_source=a".into(),
                    "deepseek".into(),
                )
            },
            SearchResult::text(
                "only deepseek".into(),
                "https://example.com/deepseek".into(),
                "old".into(),
            ),
        ],
        EngineId::Searxng => vec![
            SearchResult::text(
                "shared duplicate".into(),
                "https://example.com/shared#anchor".into(),
                "searx".into(),
            ),
            SearchResult {
                date: Some("2026-01-01T00:00:00Z".into()),
                ..SearchResult::text(
                    "only searx".into(),
                    "https://example.com/searx".into(),
                    "old".into(),
                )
            },
        ],
        _ => vec![SearchResult::text(
            "other".into(),
            "https://example.com/other".into(),
            "other".into(),
        )],
    }
}

#[tokio::test]
async fn fallback_is_ordered_stops_after_first_nonempty_and_never_calls_disabled_engines() {
    let client = FakeClient {
        responses: HashMap::from([
            (EngineId::Searxng, Ok(rows(EngineId::Searxng))),
            (EngineId::Duckduckgo, Ok(rows(EngineId::Duckduckgo))),
        ]),
        calls: Mutex::new(Vec::new()),
        barrier: None,
    };
    let config = runtime(
        &[EngineId::Searxng, EngineId::Duckduckgo],
        SearchStrategy::Fallback,
        RankingProfile::Balanced,
    );
    let request = SearchRequest::parse(json!({"query":"synthetic","count":5})).unwrap();
    let out = dispatch(
        &client,
        &config,
        &request,
        Duration::from_secs(1),
        CancellationToken::new(),
        &FixedClock::new(1_800_000_000_000),
    )
    .await
    .unwrap();
    assert_eq!(*client.calls.lock().unwrap(), [EngineId::Searxng]);
    assert_eq!(out.results.len(), 2);
    assert_eq!(out.evidence.attempts.len(), 1);
}

#[tokio::test]
async fn all_sixteen_engine_switch_combinations_only_call_enabled_engines_in_priority_order() {
    for strategy in [SearchStrategy::Fallback, SearchStrategy::Verify] {
        for mask in 0u8..16 {
            let enabled: Vec<_> = EngineId::ALL
                .into_iter()
                .enumerate()
                .filter_map(|(index, id)| ((mask & (1 << index)) != 0).then_some(id))
                .collect();
            let client = FakeClient {
                responses: EngineId::ALL
                    .into_iter()
                    .map(|id| (id, Ok(Vec::new())))
                    .collect(),
                calls: Mutex::new(Vec::new()),
                barrier: None,
            };
            let request = SearchRequest::parse(json!({"query":"switch matrix"})).unwrap();
            let result = dispatch(
                &client,
                &runtime(&enabled, strategy, RankingProfile::Balanced),
                &request,
                Duration::from_secs(1),
                CancellationToken::new(),
                &FixedClock::new(1_800_000_000_000),
            )
            .await;

            if enabled.is_empty() {
                assert!(result
                    .unwrap_err()
                    .to_string()
                    .contains("工具不可用，请开启搜索引擎"));
                assert!(client.calls.lock().unwrap().is_empty());
            } else {
                let output = result.unwrap();
                assert_eq!(*client.calls.lock().unwrap(), enabled);
                assert_eq!(
                    output
                        .evidence
                        .attempts
                        .iter()
                        .map(|attempt| attempt.engine)
                        .collect::<Vec<_>>(),
                    enabled
                );
            }
        }
    }
}

#[tokio::test]
async fn verify_runs_enabled_engines_concurrently_fuses_sources_and_records_partial_errors() {
    let barrier = Arc::new(Barrier::new(2));
    let client = FakeClient {
        responses: HashMap::from([
            (EngineId::Deepseek, Ok(rows(EngineId::Deepseek))),
            (EngineId::Searxng, Ok(rows(EngineId::Searxng))),
        ]),
        calls: Mutex::new(Vec::new()),
        barrier: Some(barrier),
    };
    let config = runtime(
        &[EngineId::Deepseek, EngineId::Searxng],
        SearchStrategy::Verify,
        RankingProfile::Balanced,
    );
    let request = SearchRequest::parse(json!({"query":"synthetic","count":10})).unwrap();
    let started = Instant::now();
    let out = tokio::time::timeout(
        Duration::from_secs(1),
        dispatch(
            &client,
            &config,
            &request,
            Duration::from_secs(1),
            CancellationToken::new(),
            &FixedClock::new(1_800_000_000_000),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(client.calls.lock().unwrap().len(), 2);
    let shared = out
        .evidence
        .rankings
        .iter()
        .find(|row| row.url.contains("/shared"))
        .unwrap();
    assert_eq!(shared.sources.len(), 2);
    let evidence = serde_json::to_value(shared).unwrap();
    assert_eq!(evidence["sources"][0]["result"]["snippet"], "deepseek");
    assert_eq!(evidence["sources"][1]["result"]["snippet"], "searx");
    assert_eq!(shared.features.independent_support, None);
    assert_eq!(shared.features.contradictory_evidence, None);
    let result = out
        .results
        .iter()
        .find(|result| result.url == shared.url)
        .unwrap();
    assert_eq!(result.snippet, "deepseek");
    assert!(!out.evidence.incomplete);
    assert_eq!(out.results.len(), 3);
}

#[tokio::test]
async fn timely_profile_adds_bounded_bonus_and_balanced_has_no_bonus() {
    let client = FakeClient {
        responses: HashMap::from([(EngineId::Deepseek, Ok(rows(EngineId::Deepseek)))]),
        calls: Mutex::new(Vec::new()),
        barrier: None,
    };
    let request = SearchRequest::parse(json!({"query":"synthetic","count":10})).unwrap();
    let clock = FixedClock::new(1_800_000_000_000);
    let balanced = dispatch(
        &client,
        &runtime(
            &[EngineId::Deepseek],
            SearchStrategy::Verify,
            RankingProfile::Balanced,
        ),
        &request,
        Duration::from_secs(1),
        CancellationToken::new(),
        &clock,
    )
    .await
    .unwrap();
    let timely = dispatch(
        &client,
        &runtime(
            &[EngineId::Deepseek],
            SearchStrategy::Verify,
            RankingProfile::Timely,
        ),
        &request,
        Duration::from_secs(1),
        CancellationToken::new(),
        &clock,
    )
    .await
    .unwrap();
    let balanced_shared = balanced
        .evidence
        .rankings
        .iter()
        .find(|item| item.url.contains("/shared"))
        .unwrap();
    let timely_shared = timely
        .evidence
        .rankings
        .iter()
        .find(|item| item.url.contains("/shared"))
        .unwrap();
    assert_eq!(balanced_shared.recency_bonus, 0.0);
    assert!(timely_shared.recency_bonus > 0.0);
    assert!(timely_shared.recency_bonus <= timely_shared.rrf_score * 0.25);
}

#[tokio::test]
async fn disabled_and_single_engine_parameter_errors_are_explicit() {
    let client = FakeClient {
        responses: HashMap::from([(
            EngineId::Searxng,
            Err(EngineError::new(
                EngineId::Searxng,
                SearchErrorCode::UnsupportedParameter,
            )),
        )]),
        calls: Mutex::new(Vec::new()),
        barrier: None,
    };
    let request = SearchRequest::parse(json!({"query":"synthetic","need_content":true})).unwrap();
    let disabled = runtime(&[], SearchStrategy::Fallback, RankingProfile::Balanced);
    assert!(dispatch(
        &client,
        &disabled,
        &request,
        Duration::from_secs(1),
        CancellationToken::new(),
        &FixedClock::new(0)
    )
    .await
    .unwrap_err()
    .to_string()
    .contains("工具不可用，请开启搜索引擎"));
    let single = runtime(
        &[EngineId::Searxng],
        SearchStrategy::Verify,
        RankingProfile::Balanced,
    );
    assert!(dispatch(
        &client,
        &single,
        &request,
        Duration::from_secs(1),
        CancellationToken::new(),
        &FixedClock::new(0)
    )
    .await
    .unwrap_err()
    .to_string()
    .contains("unsupported_parameter"));
}

#[tokio::test]
async fn already_cancelled_search_never_starts_an_engine() {
    let client = FakeClient {
        responses: HashMap::from([(EngineId::Searxng, Ok(rows(EngineId::Searxng)))]),
        calls: Mutex::new(Vec::new()),
        barrier: None,
    };
    let request = SearchRequest::parse(json!({"query":"synthetic"})).unwrap();
    let token = CancellationToken::new();
    token.cancel();
    for _ in 0..32 {
        assert!(dispatch(
            &client,
            &runtime(
                &[EngineId::Searxng],
                SearchStrategy::Fallback,
                RankingProfile::Balanced
            ),
            &request,
            Duration::from_secs(1),
            token.clone(),
            &FixedClock::new(0),
        )
        .await
        .is_err());
    }
    assert!(client.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn deduplication_preserves_original_engine_rank_in_evidence_and_rrf() {
    let client = FakeClient {
        responses: HashMap::from([(
            EngineId::Searxng,
            Ok(vec![
                SearchResult::text("first".into(), "https://example.test/a".into(), "".into()),
                SearchResult::text(
                    "duplicate".into(),
                    "https://example.test/a#same".into(),
                    "".into(),
                ),
                SearchResult::text("third".into(), "https://example.test/b".into(), "".into()),
            ]),
        )]),
        calls: Mutex::new(Vec::new()),
        barrier: None,
    };
    let request = SearchRequest::parse(json!({"query":"synthetic"})).unwrap();
    let output = dispatch(
        &client,
        &runtime(
            &[EngineId::Searxng],
            SearchStrategy::Fallback,
            RankingProfile::Balanced,
        ),
        &request,
        Duration::from_secs(1),
        CancellationToken::new(),
        &FixedClock::new(0),
    )
    .await
    .unwrap();
    let third = output
        .evidence
        .rankings
        .iter()
        .find(|row| row.url.ends_with("/b"))
        .unwrap();
    assert_eq!(third.sources[0].rank, 3);
    assert!((third.rrf_score - f64::from(1.0_f32 / 63.0)).abs() < 1e-9);
}

struct WaitingClient {
    started: tokio::sync::Notify,
    calls: Mutex<Vec<EngineId>>,
}

#[async_trait]
impl SearchEngineClient for WaitingClient {
    async fn search(
        &self,
        engine: EngineId,
        _: &SearchRequest,
        _: &SearchRuntimeConfig,
        _: Duration,
    ) -> std::result::Result<Vec<SearchResult>, EngineError> {
        self.calls.lock().unwrap().push(engine);
        self.started.notify_one();
        tokio::time::sleep(Duration::from_millis(70)).await;
        Ok(Vec::new())
    }
}

#[tokio::test]
async fn fallback_has_one_total_timeout_not_a_fresh_budget_for_every_engine() {
    let client = WaitingClient {
        started: Default::default(),
        calls: Mutex::new(Vec::new()),
    };
    let started = Instant::now();
    let result = dispatch(
        &client,
        &runtime(
            &EngineId::ALL,
            SearchStrategy::Fallback,
            RankingProfile::Balanced,
        ),
        &SearchRequest::parse(json!({"query":"synthetic"})).unwrap(),
        Duration::from_millis(100),
        CancellationToken::new(),
        &FixedClock::new(0),
    )
    .await;
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("web_search_timeout"));
    assert!(started.elapsed() < Duration::from_millis(500));
    assert_eq!(client.calls.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn cancellation_during_first_engine_prevents_fallback() {
    let client = WaitingClient {
        started: Default::default(),
        calls: Mutex::new(Vec::new()),
    };
    let cancel = CancellationToken::new();
    let config = runtime(
        &EngineId::ALL,
        SearchStrategy::Fallback,
        RankingProfile::Balanced,
    );
    let request = SearchRequest::parse(json!({"query":"synthetic"})).unwrap();
    let clock = FixedClock::new(0);
    let dispatch = dispatch(
        &client,
        &config,
        &request,
        Duration::from_secs(1),
        cancel.clone(),
        &clock,
    );
    let cancel_after_start = async {
        client.started.notified().await;
        cancel.cancel();
    };
    let (result, _) = tokio::join!(dispatch, cancel_after_start);
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("web_search_cancelled"));
    assert_eq!(*client.calls.lock().unwrap(), [EngineId::Deepseek]);
}
