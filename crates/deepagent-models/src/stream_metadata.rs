//! Bounded diagnostic snapshots for one physical Responses request.
//! These are observed provider identifiers, never execution or model-selection authority.
use serde_json::Value;

#[derive(Debug, Default)]
pub(super) struct Metadata {
    pub response_id: Option<String>,
    pub provider_model: Option<String>,
}

impl Metadata {
    pub fn observe(&mut self, kind: &str, event: &Value) {
        if !matches!(
            kind,
            "response.created"
                | "response.in_progress"
                | "response.completed"
                | "response.incomplete"
                | "response.failed"
        ) {
            return;
        }
        if kind == "response.created" {
            *self = Self::default();
        }
        let Some(response) = event.get("response").and_then(Value::as_object) else {
            return;
        };
        if let Some(id) = response.get("id") {
            let id = diagnostic_identifier(id);
            if id != self.response_id {
                // A changed or explicitly unusable identity must not inherit
                // another response's model snapshot. Missing fields remain optional.
                self.provider_model = None;
            }
            self.response_id = id;
        }
        if let Some(model) = response.get("model") {
            self.provider_model = diagnostic_identifier(model);
        }
    }
}

pub(super) fn diagnostic_identifier(value: &Value) -> Option<String> {
    // Local diagnostic policy, not an additional provider schema restriction:
    // omit unsuitable metadata rather than failing an otherwise valid answer.
    // Never truncate into a different identity or copy free text/response extensions.
    let value = value.as_str()?;
    (!value.is_empty()
        && value.len() <= 256
        && !value.to_ascii_lowercase().starts_with("sk-")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:/+@".contains(&byte)))
    .then(|| value.to_owned())
}
