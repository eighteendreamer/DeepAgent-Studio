//! System search contracts. These are not any provider's raw response DTOs.
use deepagent_core::error::{CoreError, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineId {
    Deepseek,
    Anysearch,
    Searxng,
    Duckduckgo,
}

impl EngineId {
    pub const ALL: [Self; 4] = [
        Self::Deepseek,
        Self::Anysearch,
        Self::Searxng,
        Self::Duckduckgo,
    ];
    pub const fn label(self) -> &'static str {
        match self {
            Self::Deepseek => "deepseek",
            Self::Anysearch => "anysearch",
            Self::Searxng => "searxng",
            Self::Duckduckgo => "duckduckgo",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchStrategy {
    #[default]
    Fallback,
    Verify,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RankingProfile {
    #[default]
    Balanced,
    Timely,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineSettings {
    pub id: EngineId,
    pub enabled: bool,
    #[serde(default)]
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RankingSettings {
    pub method: RankingMethod,
    pub profile: RankingProfile,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RankingMethod {
    #[default]
    System,
}
impl Default for RankingSettings {
    fn default() -> Self {
        Self {
            method: RankingMethod::System,
            profile: RankingProfile::Balanced,
        }
    }
}

/// Versioned, non-secret settings. Array order alone determines priority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SearchSettings {
    pub schema_version: u8,
    pub strategy: SearchStrategy,
    pub engines: Vec<EngineSettings>,
    pub ranking: RankingSettings,
}
impl Default for SearchSettings {
    fn default() -> Self {
        Self {
            schema_version: 2,
            strategy: SearchStrategy::Fallback,
            engines: EngineId::ALL
                .into_iter()
                .map(|id| EngineSettings {
                    id,
                    enabled: id != EngineId::Anysearch,
                    base_url: (id == EngineId::Anysearch)
                        .then(|| "https://api.anysearch.com".into()),
                })
                .collect(),
            ranking: RankingSettings::default(),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct V2Settings {
    schema_version: u8,
    strategy: SearchStrategy,
    engines: Vec<EngineSettings>,
    ranking: RankingSettings,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LegacyProvider {
    #[default]
    DeepseekFirst,
    Searxng,
    Duckduckgo,
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct LegacySettings {
    enabled: bool,
    provider: LegacyProvider,
    searxng_url: Option<String>,
    anysearch_enabled: bool,
    anysearch_base_url: Option<String>,
    anysearch_api_key_configured: bool,
}
impl Default for LegacySettings {
    fn default() -> Self {
        Self {
            enabled: true,
            provider: LegacyProvider::DeepseekFirst,
            searxng_url: None,
            anysearch_enabled: false,
            anysearch_base_url: None,
            anysearch_api_key_configured: false,
        }
    }
}

impl<'de> Deserialize<'de> for SearchSettings {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Stored {
            V2(V2Settings),
            Legacy(LegacySettings),
        }
        let mut settings = match Stored::deserialize(deserializer)? {
            Stored::V2(v) => Self {
                schema_version: v.schema_version,
                strategy: v.strategy,
                engines: v.engines,
                ranking: v.ranking,
            },
            Stored::Legacy(v) => {
                let mut settings = Self::default();
                // Never persist or trust a historical view-only credential flag.
                let _ = v.anysearch_api_key_configured;
                for engine in &mut settings.engines {
                    engine.enabled = v.enabled
                        && match engine.id {
                            EngineId::Anysearch => v.anysearch_enabled,
                            EngineId::Deepseek => {
                                matches!(v.provider, LegacyProvider::DeepseekFirst)
                            }
                            EngineId::Searxng => !matches!(v.provider, LegacyProvider::Duckduckgo),
                            EngineId::Duckduckgo => true,
                        };
                    match engine.id {
                        EngineId::Anysearch => {
                            if v.anysearch_base_url.is_some() {
                                engine.base_url = v.anysearch_base_url.clone();
                            }
                        }
                        EngineId::Searxng => engine.base_url = v.searxng_url.clone(),
                        _ => {}
                    }
                }
                settings
            }
        };
        settings.normalize();
        settings.validate().map_err(serde::de::Error::custom)?;
        Ok(settings)
    }
}

impl SearchSettings {
    pub fn normalize(&mut self) {
        for engine in &mut self.engines {
            engine.base_url = engine
                .base_url
                .take()
                .map(|value| value.trim().trim_end_matches('/').to_string())
                .filter(|value| !value.is_empty());
            if engine.id == EngineId::Anysearch && engine.base_url.is_none() {
                engine.base_url = Some("https://api.anysearch.com".into());
            }
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != 2 || self.engines.len() != 4 {
            return Err(CoreError::invalid(
                "web_search requires schema_version 2 and exactly four engines",
            ));
        }
        let mut seen = std::collections::HashSet::new();
        if self.engines.iter().any(|e| !seen.insert(e.id)) {
            return Err(CoreError::invalid("web_search engine ids must be unique"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineConfigState {
    Disabled,
    NeedsConfiguration,
    Ready,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EngineConfigStatus {
    pub id: EngineId,
    pub state: EngineConfigState,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SearchSettingsView {
    #[serde(flatten)]
    pub settings: SearchSettings,
    pub engine_statuses: Vec<EngineConfigStatus>,
}

/// Short-lived credentials paired with one immutable settings snapshot.
/// Deliberately no Debug/Serialize implementation.
pub struct SearchRuntimeConfig {
    pub settings: SearchSettings,
    pub deepseek_api_key: Option<String>,
    pub anysearch_api_key: Option<String>,
}

#[async_trait::async_trait]
pub trait SearchConfigSource: Send + Sync {
    async fn snapshot(&self) -> Result<SearchRuntimeConfig>;
}

/// Input normalization is the sole compatibility boundary for the old `limit`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchRequest {
    pub query: String,
    pub count: u8,
    pub ori_question: Option<String>,
    pub localization: String,
    pub need_content: bool,
}
impl SearchRequest {
    pub fn parse(value: serde_json::Value) -> Result<Self> {
        fn present_count<'de, D: serde::Deserializer<'de>>(
            deserializer: D,
        ) -> std::result::Result<Option<u8>, D::Error> {
            u8::deserialize(deserializer).map(Some)
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            query: String,
            #[serde(default, deserialize_with = "present_count")]
            count: Option<u8>,
            #[serde(default, deserialize_with = "present_count")]
            limit: Option<u8>,
            ori_question: Option<String>,
            #[serde(default)]
            localization: String,
            #[serde(default)]
            need_content: bool,
        }
        // Never include untrusted values in a validation error (e.g. secrets
        // accidentally supplied as an unsupported argument).
        let input: Input = serde_json::from_value(value)
            .map_err(|_| CoreError::invalid("invalid web_search input fields or types"))?;
        if matches!((input.count, input.limit), (Some(a), Some(b)) if a != b) {
            return Err(CoreError::invalid(
                "web_search count and legacy limit conflict",
            ));
        }
        let count = input.count.or(input.limit).unwrap_or(10);
        if !(1..=50).contains(&count) {
            return Err(CoreError::invalid("web_search count must be 1-50"));
        }
        fn valid_text(s: &str) -> bool {
            (1..=128).contains(&s.chars().count()) && !s.trim().is_empty()
        }
        if !valid_text(&input.query)
            || input
                .ori_question
                .as_deref()
                .is_some_and(|s| !valid_text(s))
        {
            return Err(CoreError::invalid("web_search query and non-null ori_question must be 1-128 Unicode characters and nonblank"));
        }
        if !input.localization.is_empty() && input.localization.parse::<std::net::IpAddr>().is_err()
        {
            return Err(CoreError::invalid(
                "web_search localization must be empty or an IP address",
            ));
        }
        Ok(Self {
            query: input.query,
            count,
            ori_question: input.ori_question,
            localization: input.localization,
            need_content: input.need_content,
        })
    }
}

/// Cross-provider extension: absent provider scores are null, not zero.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub summary: String,
    pub content: String,
    pub date: Option<String>,
    pub rerank_score: Option<f64>,
    pub authority: i8,
    pub site_name: Option<String>,
    pub site_icon: Option<String>,
}
impl SearchResult {
    pub fn text(title: String, url: String, snippet: String) -> Self {
        Self {
            title,
            url,
            snippet,
            summary: String::new(),
            content: String::new(),
            date: None,
            rerank_score: None,
            authority: -1,
            site_name: None,
            site_icon: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineAttempt {
    pub engine: EngineId,
    pub ok: bool,
    pub result_count: usize,
    pub error: Option<EngineError>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResultSource {
    pub engine: EngineId,
    pub rank: usize,
    /// Retain each provider's actual variant; never concatenate them into
    /// synthesized source text or count engine agreement as independence.
    pub result: SearchResult,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceFeatures {
    pub has_content: bool,
    pub has_snippet: bool,
    pub has_summary: bool,
    pub has_publication_date: bool,
    pub query_relevance: Option<String>,
    pub first_party: Option<bool>,
    pub independent_support: Option<bool>,
    pub contradictory_evidence: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResultEvidence {
    pub source_id: String,
    pub url: String,
    pub sources: Vec<ResultSource>,
    pub features: EvidenceFeatures,
    pub rrf_score: f64,
    pub recency_bonus: f64,
    pub system_score: f64,
    pub ranking_reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchEvidence {
    pub retrieved_at_ms: i64,
    pub strategy: SearchStrategy,
    pub incomplete: bool,
    pub attempts: Vec<EngineAttempt>,
    pub rankings: Vec<ResultEvidence>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchOutput {
    pub query: String,
    pub count: usize,
    pub results: Vec<SearchResult>,
    pub evidence: SearchEvidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchErrorCode {
    ConfigurationMissing,
    UnsupportedParameter,
    HttpError,
    BusinessError,
    ProtocolError,
    Challenge,
    MarkupChanged,
    TransportError,
}

/// Safe local classifications, not provider error messages/codes or bodies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineError {
    pub engine: EngineId,
    pub code: SearchErrorCode,
    pub http_status: Option<u16>,
}
impl EngineError {
    pub fn new(engine: EngineId, code: SearchErrorCode) -> Self {
        Self {
            engine,
            code,
            http_status: None,
        }
    }
}

#[cfg(test)]
#[path = "types_tests.rs"]
mod tests;
