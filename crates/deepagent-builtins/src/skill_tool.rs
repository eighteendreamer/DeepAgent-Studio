//! `skill` built-in tool — channel B of the auto-activation design
//! (`.kiro/specs/skill-marketplace/design.md` §Auto-Activation §通道 B).
//!
//! When the model decides to use a skill — typically after seeing it in the
//! `<available-skills>` reminder rendered by [`SkillRegistry::formatted_catalog`]
//! — it calls the `skill` tool with `{ id, args? }`. We look up the skill by
//! id in the shared [`SkillRegistry`], substitute `${ARGS}` / `$ARGS` in its
//! body, and return [`SkillToolOutput`] (`{ id, name, body, base_dir,
//! resources }`) as the tool result. The model then has the full SKILL.md
//! body in its next-turn context and can proceed with the specialized task.
//!
//! Key invariants (mirroring requirements R6.1–R6.6):
//!
//! - [`Tool::always_load`] returns `true` (R6.1): the discovery channel must
//!   not be deferred behind itself. tool-search Auto mode never hides this
//!   tool — the model needs it on every turn to disclose skill bodies.
//! - [`SkillRegistry::body_for_invoke`] rejects skills carrying
//!   `disable-model-invocation: true` with [`BodyForInvokeError::DisabledForModel`]
//!   (R6.4). The model gets a friendly error pointing it at the user-only
//!   `/{id}` slash-command form.
//! - Unknown ids return up to 5 fuzzy-matched suggestions
//!   ([`BodyForInvokeError::NotFound`]) so the model can recover without
//!   needing the catalog refreshed (R6.3).
//! - Repeat invocations within the same session are NOT deduplicated
//!   (R6.5) — the tool is stateless; that decision lives at the call site.

use std::sync::Arc;

use async_trait::async_trait;

use deepagent_core::error::Result;
use deepagent_skills::{BodyForInvokeError, SkillRegistry};
use deepagent_tools::permission::{PermissionSet, RiskLevel};
use deepagent_tools::{Tool, ToolDescriptor, ToolOutput};

/// Reserved tool name for the `skill` discovery channel.
pub const SKILL_TOOL_NAME: &str = "skill";

/// The built-in `skill` tool.
///
/// Shares an [`Arc`]-wrapped [`SkillRegistry`] snapshot with the rest of the
/// runtime. The registry is treated as immutable for the lifetime of one
/// [`SkillTool`]: when skills are installed / uninstalled / reloaded, the
/// owning layer (chat service / Tauri host) is responsible for reconstructing
/// the [`SkillTool`] over a fresh registry snapshot. This mirrors the way
/// [`crate::ToolSearchTool`] holds an immutable deferred-tool snapshot.
pub struct SkillTool {
    registry: Arc<SkillRegistry>,
}

impl SkillTool {
    /// Build the tool over a shared registry snapshot.
    pub fn new(registry: Arc<SkillRegistry>) -> Self {
        Self { registry }
    }

    /// Number of skills the registry exposes (for diagnostics / wiring code).
    pub fn skill_count(&self) -> usize {
        self.registry.len()
    }
}

#[async_trait]
impl Tool for SkillTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: SKILL_TOOL_NAME.to_string(),
            description: SKILL_TOOL_DESCRIPTION.to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Skill id to invoke (matches the catalog reminder; \
                                        e.g. \"planning-with-files\")."
                    },
                    "args": {
                        "type": "string",
                        "description": "Optional argument string. Substituted into the \
                                        skill body wherever ${ARGS} or $ARGS appears."
                    }
                },
                "required": ["id"],
                "additionalProperties": false
            }),
            risk: RiskLevel::Safe,
            required_permissions: PermissionSet::read_only(),
        }
    }

    async fn invoke(&self, arguments: serde_json::Value) -> Result<ToolOutput> {
        // Argument validation. We surface input errors as `ToolOutput::failure`
        // (not `Err`) so the model sees the message and can self-correct on
        // the next turn — same pattern as `tool_search`.
        let Some(id_raw) = arguments.get("id").and_then(|v| v.as_str()) else {
            return Ok(ToolOutput::failure("missing 'id' field"));
        };
        let id = id_raw.trim();
        if id.is_empty() {
            return Ok(ToolOutput::failure("'id' must not be empty"));
        }
        let args = arguments.get("args").and_then(|v| v.as_str());

        match self.registry.body_for_invoke(id, args) {
            Ok(output) => match serde_json::to_value(&output) {
                Ok(value) => Ok(ToolOutput::success(value)),
                Err(e) => Ok(ToolOutput::failure(format!(
                    "skill: failed to serialize SkillToolOutput: {e}"
                ))),
            },
            Err(BodyForInvokeError::DisabledForModel { id }) => Ok(ToolOutput::failure(format!(
                "skill '{id}' is user-only (disable-model-invocation=true); \
                     ask the user to invoke /{id} via the slash-command UI."
            ))),
            Err(BodyForInvokeError::NotFound { id, suggestions }) => {
                if suggestions.is_empty() {
                    Ok(ToolOutput::failure(format!(
                        "skill '{id}' not found in registry"
                    )))
                } else {
                    Ok(ToolOutput::failure(format!(
                        "skill '{id}' not found. Did you mean: {}?",
                        suggestions.join(", ")
                    )))
                }
            }
        }
    }

    fn always_load(&self) -> bool {
        // The skill-disclosure channel must always reach the model: deferring
        // it behind tool-search would hide the only way to invoke skills the
        // model just learned about via the `<available-skills>` reminder.
        true
    }
}

/// Tool description shown to the model in the per-request `tools` array. Kept
/// short — the `<available-skills>` reminder carries the actual catalog.
const SKILL_TOOL_DESCRIPTION: &str = "Execute a skill — disclose its full body to perform a specialized task.\n\
\n\
When the user's request matches a skill listed in the `<available-skills>` system reminder, invoke this tool with the skill id. The tool returns the skill's full SKILL.md body, which contains specialized instructions for the task.\n\
\n\
## Args\n\
- `id` (string, required) — the skill id from the catalog (e.g. `\"planning-with-files\"`).\n\
- `args` (string, optional) — free-form arguments. Substituted for `${ARGS}` / `$ARGS` literals in the skill body.\n\
\n\
## Result\n\
On success, returns:\n\
```json\n\
{\n\
  \"id\": \"...\", \"name\": \"...\", \"body\": \"<SKILL.md body with $ARGS substituted>\",\n\
  \"base_dir\": \"<absolute path>\" | null,\n\
  \"resources\": [\"references/...\", \"scripts/...\", ...]\n\
}\n\
```\n\
Use `base_dir` + `resources` with `read_file` / `grep` to pull deeper context as needed.\n\
\n\
## Examples\n\
- `skill({\"id\": \"planning-with-files\"})` — pull the planning skill body.\n\
- `skill({\"id\": \"code-review-skill\", \"args\": \"review src/auth/\"})` — body + custom args.\n\
\n\
## Important\n\
- Available skills are listed in `<available-skills>` system messages.\n\
- When a skill matches, invoke this tool BEFORE responding about the task.\n\
- Do not mention a skill without invoking it.\n\
- Do not invoke a skill that is already loaded in the current turn.";

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_skills::{frontmatter, Skill, SkillOrigin};

    /// Build a `Skill` from a SKILL.md-style frontmatter block.
    fn make_skill(id: &str, name: &str, description: &str, body: &str) -> Skill {
        let raw = format!("---\nname: {name}\ndescription: {description}\n---\n{body}");
        let fm = frontmatter::parse(&raw);
        Skill::from_frontmatter(id, &fm, SkillOrigin::Workspace).unwrap()
    }

    /// Build a `Skill` carrying `disable-model-invocation: true`.
    fn make_user_only_skill(id: &str, name: &str, description: &str, body: &str) -> Skill {
        let raw = format!(
            "---\nname: {name}\ndescription: {description}\ndisable-model-invocation: true\n---\n{body}"
        );
        let fm = frontmatter::parse(&raw);
        Skill::from_frontmatter(id, &fm, SkillOrigin::Workspace).unwrap()
    }

    /// Build a `SkillTool` over a registry seeded with the given skills.
    fn build_tool(skills: Vec<Skill>) -> SkillTool {
        let mut registry = SkillRegistry::new();
        for s in skills {
            registry.register(s);
        }
        SkillTool::new(Arc::new(registry))
    }

    #[test]
    fn skill_tool_descriptor_has_correct_name_and_schema() {
        // Validates: Requirement R6.1
        let tool = build_tool(vec![]);
        let desc = tool.descriptor();
        assert_eq!(desc.name, "skill");
        assert!(!desc.description.is_empty());
        // Schema requires `id`, has optional `args`, and bans extras.
        assert_eq!(desc.parameters["type"], "object");
        let required = desc.parameters["required"]
            .as_array()
            .expect("required is an array");
        assert!(required.iter().any(|v| v == "id"));
        assert!(!required.iter().any(|v| v == "args"));
        assert_eq!(desc.parameters["additionalProperties"], false);
        let props = desc.parameters["properties"]
            .as_object()
            .expect("properties is an object");
        assert!(props.contains_key("id"));
        assert!(props.contains_key("args"));
        // `args` is a string parameter.
        assert_eq!(props["args"]["type"], "string");
        // Risk classification is Safe — the tool only reads registered metadata.
        assert_eq!(desc.risk, RiskLevel::Safe);
    }

    #[test]
    fn skill_tool_always_load_is_true() {
        // Validates: Requirement R6.1 — `should_defer()` is false (default)
        // and `always_load()` is true so tool-search Auto mode never hides
        // the discovery channel.
        let tool = build_tool(vec![]);
        assert!(tool.always_load(), "always_load must be true (R6.1)");
        assert!(
            !tool.should_defer(),
            "should_defer must be false so tool-search Enabled mode keeps it loaded"
        );
    }

    #[tokio::test]
    async fn skill_tool_run_returns_body() {
        // Validates: Requirement R6.2
        let skill = make_skill(
            "planning-with-files",
            "Planning",
            "\"plan a multi-file change\"",
            "Plan the work step by step.",
        );
        let tool = build_tool(vec![skill]);
        let out = tool
            .invoke(serde_json::json!({ "id": "planning-with-files" }))
            .await
            .unwrap();
        assert!(out.ok, "expected success, got {:?}", out);
        assert_eq!(out.value["id"], "planning-with-files");
        assert_eq!(out.value["name"], "Planning");
        assert_eq!(out.value["body"], "Plan the work step by step.");
        // Programmatically registered skills carry no base_dir; SkillToolOutput
        // skips serializing it via `skip_serializing_if = "Option::is_none"`.
        assert!(
            out.value.get("base_dir").is_none()
                || out.value.get("base_dir") == Some(&serde_json::Value::Null)
        );
        assert!(out.value["resources"].is_array());
    }

    #[tokio::test]
    async fn skill_tool_substitutes_args_placeholders() {
        // Validates: Requirement R6.2 — `${ARGS}` / `$ARGS` substitution.
        let skill = make_skill(
            "echo",
            "Echo",
            "\"echo something\"",
            "Hello $ARGS! Repeated: ${ARGS}.",
        );
        let tool = build_tool(vec![skill]);
        let out = tool
            .invoke(serde_json::json!({ "id": "echo", "args": "world" }))
            .await
            .unwrap();
        assert!(out.ok);
        assert_eq!(out.value["body"], "Hello world! Repeated: world.");
    }

    #[tokio::test]
    async fn skill_tool_returns_error_for_unknown_id_with_suggestions() {
        // Validates: Requirement R6.3
        let tool = build_tool(vec![
            make_skill("pdf-rotate", "Rotate", "\"rotate a pdf\"", "rotate"),
            make_skill("pdf-merge", "Merge", "\"merge pdfs\"", "merge"),
        ]);
        let out = tool
            .invoke(serde_json::json!({ "id": "pdf" }))
            .await
            .unwrap();
        assert!(!out.ok, "unknown id should be a failure");
        let err = out.value["error"]
            .as_str()
            .expect("error message is a string");
        assert!(
            err.contains("'pdf'"),
            "error mentions the queried id: {err}"
        );
        assert!(
            err.contains("Did you mean"),
            "error includes suggestion preamble: {err}"
        );
        assert!(
            err.contains("pdf-rotate"),
            "error suggests pdf-rotate: {err}"
        );
        assert!(err.contains("pdf-merge"), "error suggests pdf-merge: {err}");
    }

    #[tokio::test]
    async fn skill_tool_rejects_disable_model_invocation() {
        // Validates: Requirement R6.4
        let skill = make_user_only_skill(
            "user-only-task",
            "User Only",
            "\"do user-only thing\"",
            "secret body",
        );
        let tool = build_tool(vec![skill]);
        let out = tool
            .invoke(serde_json::json!({ "id": "user-only-task" }))
            .await
            .unwrap();
        assert!(!out.ok);
        let err = out.value["error"].as_str().unwrap();
        assert!(
            err.contains("user-only"),
            "error mentions the user-only restriction: {err}"
        );
        assert!(
            err.contains("/user-only-task"),
            "error suggests the slash-command form: {err}"
        );
        // Body must NOT leak in the error.
        assert!(
            !err.contains("secret body"),
            "error must not disclose the skill body"
        );
    }

    #[tokio::test]
    async fn skill_tool_missing_id_field_returns_error() {
        // Defensive: malformed model output (missing required `id`) is reported
        // as a `ToolOutput::failure` so the model can self-correct.
        let tool = build_tool(vec![]);
        let out = tool.invoke(serde_json::json!({})).await.unwrap();
        assert!(!out.ok);
        let err = out.value["error"].as_str().unwrap();
        assert!(
            err.contains("'id'") || err.contains("id"),
            "error mentions the missing id field: {err}"
        );
    }

    #[tokio::test]
    async fn skill_tool_blank_id_returns_error() {
        // Defensive: a whitespace-only `id` shouldn't reach the registry.
        let tool = build_tool(vec![]);
        let out = tool
            .invoke(serde_json::json!({ "id": "   " }))
            .await
            .unwrap();
        assert!(!out.ok);
        let err = out.value["error"].as_str().unwrap();
        assert!(err.contains("empty"), "error mentions empty id: {err}");
    }

    #[tokio::test]
    async fn skill_tool_repeated_invocation_returns_body_each_time() {
        // Validates: Requirement R6.5 — no de-duplication; each call returns
        // the body. Same-skill repeat calls are an explicit non-feature.
        let skill = make_skill("repeat-me", "Repeat", "\"repeat\"", "body content");
        let tool = build_tool(vec![skill]);
        for _ in 0..3 {
            let out = tool
                .invoke(serde_json::json!({ "id": "repeat-me" }))
                .await
                .unwrap();
            assert!(out.ok);
            assert_eq!(out.value["body"], "body content");
        }
    }
}
