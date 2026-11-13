//! Read-only scenarios use the provider's one sampling resolver, never a UI copy.
use super::*;
use deepagent_models::{resolve_responses_sampling, EffectiveSamplingConfig};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResponsesSamplingPreview {
    pub model: String,
    /// Hypothetical schema availability; tool_choice may still disable tools.
    pub advertised_tools: bool,
    pub sampling: EffectiveSamplingConfig,
}

pub(super) fn responses_sampling_preview(settings: &AppSettings) -> Vec<ResponsesSamplingPreview> {
    let effort = match crate::model_runtime::resolve_reasoning_effort(settings, None) {
        Ok(effort) => effort,
        Err(error) => {
            // A bad legacy setting must remain editable. The runtime still
            // rejects it; do not invent an effective mode or fail get_settings.
            tracing::warn!(error = %error, "sampling preview unavailable for saved reasoning settings");
            return Vec::new();
        }
    };
    let top_p = settings.responses.effective_top_p();
    let choice = settings.responses.effective_tool_choice();
    let mut models = vec![settings.catalog.chat_model.clone()];
    if settings.catalog.reasoner_model != models[0] {
        models.push(settings.catalog.reasoner_model.clone());
    }
    models
        .into_iter()
        .flat_map(|model| {
            let choice = choice.as_ref();
            [false, true]
                .into_iter()
                .map(move |advertised_tools| ResponsesSamplingPreview {
                    sampling: resolve_responses_sampling(
                        &model,
                        top_p,
                        Some(effort),
                        advertised_tools,
                        choice,
                    ),
                    model: model.clone(),
                    advertised_tools,
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemorySecretStore;
    use deepagent_models::{MockTransport, ModelRole, SamplingReason};
    use deepagent_persistence::Database;

    fn service(db: Arc<Database>, secrets: Arc<MemorySecretStore>) -> SettingsService {
        SettingsService::new(
            db,
            Arc::new(MockTransport::with_get_json(
                r#"{"data":[{"id":"deepseek-flash"},{"id":"deepseek-v4-pro"}]}"#,
            )),
            secrets,
        )
    }

    #[tokio::test]
    async fn h166_legacy_invalid_effort_remains_editable_without_a_fabricated_preview() {
        let settings = service(
            Arc::new(Database::open_in_memory().unwrap()),
            Arc::new(MemorySecretStore::new()),
        );
        settings
            .initialize("h166-synthetic-placeholder")
            .await
            .unwrap();
        let mut legacy = settings.load().unwrap().unwrap();
        legacy.responses.reasoning_effort = Some("legacy-invalid".into());
        settings.save(&legacy).unwrap();
        let view = settings.view().unwrap().unwrap();
        assert_eq!(
            view.responses.reasoning_effort.as_deref(),
            Some("legacy-invalid")
        );
        assert!(view.responses_sampling.is_empty());
        assert!(crate::model_runtime::resolve_reasoning_effort(&legacy, None).is_err());
    }

    #[tokio::test]
    async fn h166_preview_preserves_settings_and_reopens_developer_precedence() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("settings.db");
        let secrets = Arc::new(MemorySecretStore::new());
        {
            let settings = service(Arc::new(Database::open(&path).unwrap()), secrets.clone());
            settings
                .initialize("h166-synthetic-placeholder")
                .await
                .unwrap();
            settings
                .set_model(ModelRole::Chat, "deepseek-flash")
                .unwrap();
            settings
                .set_model(ModelRole::Reasoner, "deepseek-v4-pro")
                .unwrap();
            let view = settings
                .set_responses_settings(ResponsesApiSettings {
                    top_p: Some(0.2),
                    reasoning_effort: Some("high".into()),
                    ..Default::default()
                })
                .unwrap();
            assert_eq!(view.responses.top_p, Some(0.2));
            assert_eq!(view.responses_sampling.len(), 4);
            println!(
                "H166_PREVIEW_JSON={}",
                serde_json::to_string(&view.responses_sampling).unwrap()
            );
            let values: Vec<_> = view
                .responses_sampling
                .iter()
                .map(|row| {
                    (
                        row.model.as_str(),
                        row.advertised_tools,
                        row.sampling.effective_top_p,
                    )
                })
                .collect();
            assert_eq!(
                values,
                [
                    ("deepseek-flash", false, Some(0.95)),
                    ("deepseek-flash", true, Some(0.95)),
                    ("deepseek-v4-pro", false, Some(1.0)),
                    ("deepseek-v4-pro", true, Some(0.95)),
                ]
            );
            settings
                .set_responses_developer_json(
                    r#"{"top_p":0.94,"reasoning":{"effort":"none"},"tool_choice":"none"}"#,
                )
                .unwrap();
            let saved = settings.load().unwrap().unwrap();
            assert_eq!(saved.responses.top_p, Some(0.2));
            assert!(serde_json::to_value(saved)
                .unwrap()
                .get("responses_sampling")
                .is_none());
        }
        let settings = service(Arc::new(Database::open(&path).unwrap()), secrets);
        let view = settings.view().unwrap().unwrap();
        assert_eq!(view.responses.top_p, Some(0.2));
        assert_eq!(view.responses_sampling.len(), 4);
        for row in view.responses_sampling {
            assert_eq!(row.sampling.configured_top_p, Some(0.94));
            assert!(!row.sampling.thinking_enabled);
            assert!(!row.sampling.tools_enabled);
            assert_eq!(
                row.sampling.effective_top_p,
                Some(if row.model == "deepseek-flash" {
                    1.0
                } else {
                    0.94
                })
            );
        }
    }

    #[tokio::test]
    async fn h166_failed_save_does_not_publish_candidate_sampling_and_old_views_decode() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let settings = service(db.clone(), Arc::new(MemorySecretStore::new()));
        settings
            .initialize("h166-synthetic-placeholder")
            .await
            .unwrap();
        let before = settings.view().unwrap().unwrap();
        assert!(settings
            .set_responses_settings(ResponsesApiSettings {
                top_p: Some(1.1),
                ..Default::default()
            })
            .is_err());
        assert_eq!(settings.view().unwrap().unwrap(), before);
        db.with_conn(|connection| {
            connection
                .execute_batch(
                    "CREATE TRIGGER h166_reject BEFORE UPDATE ON documents
                WHEN OLD.collection = 'settings' AND OLD.id = 'app'
                BEGIN SELECT RAISE(ABORT, 'synthetic H166 write failure'); END;",
                )
                .map_err(|error| CoreError::Persistence(error.to_string()))?;
            Ok(())
        })
        .unwrap();
        assert!(settings
            .set_responses_settings(ResponsesApiSettings {
                top_p: Some(0.94),
                ..Default::default()
            })
            .is_err());
        assert_eq!(settings.view().unwrap().unwrap(), before);
        assert!(before
            .responses_sampling
            .iter()
            .all(|row| row.sampling.wire_top_p.is_none()));
        let mut old = serde_json::to_value(before).unwrap();
        old.as_object_mut().unwrap().remove("responses_sampling");
        let decoded: SettingsView = serde_json::from_value(old).unwrap();
        assert!(decoded.responses_sampling.is_empty());
        assert_eq!(
            SamplingReason::ProThinkingToolsFixed,
            resolve_responses_sampling("deepseek-v4-pro", None, None, true, None,).reason
        );
    }
}
