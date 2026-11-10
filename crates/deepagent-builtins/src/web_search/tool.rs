use async_trait::async_trait;
use deepagent_core::clock::SystemClock;
use deepagent_core::error::Result;
use deepagent_tools::permission::{Permission, PermissionSet, RiskLevel};
use deepagent_tools::{Tool, ToolDescriptor, ToolExecutionContext, ToolOutput};

use super::dispatcher::{dispatch, ReqwestSearchEngineClient, SearchEngineClient};
use super::types::{SearchConfigSource, SearchRequest};

const TOOL_NAME: &str = "web_search";

/// Single registered web_search. Configuration, engine credentials, strategy
/// and ordering are snapshotted for each invocation from the existing source.
pub struct WebSearchTool<C, S> {
    engines: C,
    config: S,
}

impl<S: SearchConfigSource> WebSearchTool<ReqwestSearchEngineClient, S> {
    pub fn new(config: S) -> Self {
        Self {
            engines: ReqwestSearchEngineClient::new(),
            config,
        }
    }
}

impl<C, S> WebSearchTool<C, S> {
    pub fn with_engines(engines: C, config: S) -> Self {
        Self { engines, config }
    }
}

#[async_trait]
impl<C: SearchEngineClient, S: SearchConfigSource> Tool for WebSearchTool<C, S> {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: TOOL_NAME.into(),
            description: "Search the web through configured engines and return normalized, source-linked results. The active chat model never selects an engine. Args: { query, count?, ori_question?, localization?, need_content? }. Legacy `limit` remains accepted for existing callers.".into(),
            parameters: serde_json::json!({
                "type":"object",
                "properties": {
                    "query":{"type":"string","minLength":1,"maxLength":128},
                    "count":{"type":"integer","minimum":1,"maximum":50},
                    "limit":{"type":"integer","minimum":1,"maximum":50},
                    "ori_question":{"type":["string","null"],"minLength":1,"maxLength":128},
                    "localization":{"type":"string"},
                    "need_content":{"type":"boolean"}
                },
                "required":["query"],"additionalProperties":false
            }),
            risk: RiskLevel::Medium,
            required_permissions: PermissionSet::from_iter_perms([Permission::Network]),
        }
    }

    fn validate_input(&self, arguments: serde_json::Value) -> Result<serde_json::Value> {
        let request = SearchRequest::parse(arguments)?;
        Ok(serde_json::to_value(request)?)
    }

    async fn invoke(&self, arguments: serde_json::Value) -> Result<ToolOutput> {
        self.invoke_with_context(arguments, ToolExecutionContext::default())
            .await
    }

    async fn invoke_with_context(
        &self,
        arguments: serde_json::Value,
        context: ToolExecutionContext,
    ) -> Result<ToolOutput> {
        let request = match SearchRequest::parse(arguments) {
            Ok(request) => request,
            Err(error) => return Ok(ToolOutput::failure(error.to_string())),
        };
        let config = match self.config.snapshot().await {
            Ok(config) => config,
            Err(_) => return Ok(ToolOutput::failure("web_search_configuration_unavailable")),
        };
        match dispatch(
            &self.engines,
            &config,
            &request,
            context.timeout(),
            context.cancellation_token(),
            &SystemClock,
        )
        .await
        {
            Ok(output) => Ok(ToolOutput::success(serde_json::to_value(output)?)),
            Err(error) => Ok(ToolOutput::failure(error.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web_search::types::{EngineError, EngineId, SearchRuntimeConfig, SearchSettings};
    use std::time::Duration;

    struct DisabledConfig;
    #[async_trait]
    impl SearchConfigSource for DisabledConfig {
        async fn snapshot(&self) -> Result<SearchRuntimeConfig> {
            let mut settings = SearchSettings::default();
            for engine in &mut settings.engines {
                engine.enabled = false;
            }
            Ok(SearchRuntimeConfig {
                settings,
                deepseek_api_key: None,
                anysearch_api_key: None,
            })
        }
    }
    struct NoNetwork;
    #[async_trait]
    impl SearchEngineClient for NoNetwork {
        async fn search(
            &self,
            _: EngineId,
            _: &SearchRequest,
            _: &SearchRuntimeConfig,
            _: Duration,
        ) -> std::result::Result<Vec<crate::web_search::types::SearchResult>, EngineError> {
            panic!("all engines disabled must produce zero requests");
        }
    }

    #[test]
    fn search_keeps_the_existing_pipeline_deadline_enforcement() {
        let tool = WebSearchTool::with_engines(NoNetwork, DisabledConfig);
        assert!(tool.bounded_by_run_timeout());
    }

    #[tokio::test]
    async fn disabled_search_message_is_exact_and_has_no_requests() {
        let output = WebSearchTool::with_engines(NoNetwork, DisabledConfig)
            .invoke(serde_json::json!({"query":"synthetic"}))
            .await
            .unwrap();
        assert!(!output.ok);
        assert_eq!(output.value["error"], "工具不可用，请开启搜索引擎");
    }
}
