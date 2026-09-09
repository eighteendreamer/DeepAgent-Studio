//! Session title generation from conversation history.
//!
//! Extracted from `chat_service` so the title-generation concern lives in its
//! own module. The public entry point mirrors the old `ChatService` method so
//! callers do not change.

use std::str::FromStr;
use std::sync::Arc;

use deepagent_core::clock::{Clock, SystemClock};
use deepagent_core::error::{CoreError, Result};
use deepagent_core::id::SessionId;
use deepagent_core::message::Role;
use deepagent_models::transport::HttpTransport;
use deepagent_models::{ModelClient, ModelConfig, ModelRole, ThinkingDepth};
use deepagent_persistence::event_store::EventStore;
use deepagent_persistence::Database;

use crate::input_runtime::conversation_from_events;
use crate::settings::{SettingsService, WebSearchProvider};

const SESSION_TITLE_SYSTEM_PROMPT: &str = concat!(
    "You generate concise conversation titles for a coding assistant session.\n",
    "Return only the title text.\n",
    "Do not use quotes, markdown, numbering, or any explanation.\n",
    "Use the user's language when possible.\n",
    "Focus on the user's concrete task or goal, not greetings or assistant boilerplate.\n",
    "Keep it short and specific."
);

/// Generate and persist an AI title for a session when it is still
/// untitled. Intended for post-run refinement: if the user renames the
/// session before generation finishes, the second title check prevents the
/// auto title from overwriting the explicit one.
pub async fn generate_session_title(
    db: &Database,
    settings_svc: &SettingsService,
    transport: Arc<dyn HttpTransport>,
    session_id: &str,
) -> Result<Option<String>> {
    let id = SessionId::from_str(session_id)
        .map_err(|e| CoreError::invalid(format!("bad session id: {e}")))?;
    let store = EventStore::new(db);
    let Some(record) = store.get_session(id)? else {
        return Err(CoreError::not_found(format!("session {session_id}")));
    };
    if record
        .title
        .as_deref()
        .map(|title| !title.trim().is_empty())
        .unwrap_or(false)
    {
        return Ok(None);
    }

    let events = store.load_session(id)?;
    let history = conversation_from_events(&events);
    let mut lines = Vec::new();
    let mut user_messages = 0usize;
    for message in &history {
        let text = message.content.trim();
        if text.is_empty() {
            continue;
        }
        match message.role {
            Role::User => {
                user_messages += 1;
                lines.push(format!("User: {text}"));
                if user_messages >= 3 {
                    break;
                }
            }
            Role::Assistant => {
                if user_messages == 0 {
                    continue;
                }
                lines.push(format!("Assistant: {text}"));
            }
            _ => {}
        }
    }
    if lines.is_empty() {
        return Ok(None);
    }

    let settings = settings_svc
        .load()?
        .ok_or_else(|| CoreError::invalid("project not initialized: set an API key first"))?;
    let api_key = settings_svc
        .api_key()?
        .ok_or_else(|| CoreError::invalid("API key not set: initialize the project first"))?;
    let model = settings.catalog.model_for(ModelRole::Chat).to_string();
    let config = ModelConfig::from_catalog(api_key, &settings.catalog, ModelRole::Chat)
        .with_defaults(deepagent_models::ResponseDefaults {
            temperature: settings.responses.effective_temperature(),
            top_p: settings.responses.effective_top_p(),
            max_output_tokens: settings.responses.effective_max_output_tokens(),
            top_logprobs: settings.responses.effective_top_logprobs(),
            reasoning_effort: settings.responses.effective_reasoning_effort(),
            text: settings.responses.effective_text(),
            tool_choice: settings.responses.effective_tool_choice(),
            user: settings.responses.effective_user(),
            native_web_search: settings.web_search.enabled
                && matches!(
                    settings.web_search.provider,
                    WebSearchProvider::DeepSeekFirst
                ),
        });
    let client = Arc::new(ModelClient::new(transport, config));

    let request = deepagent_models::chat::ResponseRequest::with_instructions_and_user_input(
        model,
        SESSION_TITLE_SYSTEM_PROMPT,
        format!(
            "Create a short conversation title from this transcript:\n\n{}",
            lines.join("\n")
        ),
    )
    .streaming()
    .with_max_output_tokens(48)
    .with_thinking_depth(ThinkingDepth::Simple);
    let response = client.stream_response(request).await?;
    let Some(title) = normalize_generated_session_title(&response.output_text_projection()) else {
        return Ok(None);
    };

    let current = store.get_session(id)?;
    if current
        .as_ref()
        .and_then(|session| session.title.as_deref())
        .map(|title| !title.trim().is_empty())
        .unwrap_or(false)
    {
        return Ok(None);
    }

    let clock = SystemClock;
    if !store.rename_session(id, Some(&title), clock.now())? {
        return Err(CoreError::not_found(format!("session {session_id}")));
    }
    Ok(Some(title))
}

fn normalize_generated_session_title(raw: &str) -> Option<String> {
    let mut title = raw.trim().replace(['\r', '\n'], " ");
    for prefix in ["Title:", "title:", "标题：", "标题:"] {
        if let Some(stripped) = title.strip_prefix(prefix) {
            title = stripped.trim().to_string();
            break;
        }
    }
    title = title
        .trim_matches(|c: char| {
            matches!(
                c,
                '"' | '\'' | '`' | '\u{201c}' | '\u{201d}' | '\u{2018}' | '\u{2019}'
            )
        })
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if title.is_empty() {
        return None;
    }
    let max_chars = 48usize;
    let normalized = if title.chars().count() > max_chars {
        let mut truncated = title.chars().take(max_chars).collect::<String>();
        truncated = truncated
            .trim()
            .trim_end_matches([':', '-', ' ', '，', '。'])
            .to_string();
        truncated
    } else {
        title
    };
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}
