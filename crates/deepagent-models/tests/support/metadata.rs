//! Unit-level serializer fixtures consume raw provider metadata, without a
//! persisted catalog, role assignment or default-model selection mechanism.
#![allow(dead_code)]
pub fn config(
    key: impl Into<String>,
    models: &[deepagent_models::ModelInfo],
) -> deepagent_models::ModelConfig {
    let mut config = deepagent_models::ModelConfig::deepseek(key);
    config.capabilities = models
        .iter()
        .map(|model| deepagent_models::ModelCapabilityResolver::new().resolve_model(model))
        .collect();
    config
}
