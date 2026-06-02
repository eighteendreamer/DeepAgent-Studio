//! Executable slash-command framework.
//!
//! The older [`crate::command`] module models prompt-template commands. This
//! module models built-in imperative commands such as `/plan`, `/cost`, and
//! `/compact`: each command resolves to a structured [`SlashAction`] that the
//! application layer can execute without sending the raw slash line to the
//! model.

use std::collections::BTreeMap;
use std::sync::Arc;

use deepagent_core::error::{CoreError, Result};

/// A structured action requested by a slash command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashAction {
    /// Enter read-only Plan mode.
    EnterPlanMode,
    /// Leave Plan mode and restore normal permissions.
    ExitPlanMode,
    /// Trigger context compaction for the current session.
    Compact,
    /// Show cost summary.
    Cost,
    /// Run environment diagnostics.
    Doctor,
    /// Show available slash commands.
    Help,
    /// Show project, model, and runtime status.
    Status,
    /// Show settings summary.
    Settings,
    /// Show permission mode/rules summary.
    Permissions,
    /// Show knowledge/memory status.
    Knowledge,
    /// Show MCP server summary.
    Mcp,
    /// Show opened projects.
    Projects,
    /// Show recent sessions.
    Sessions,
    /// Show or set thinking depth.
    Thinking {
        /// Optional target depth label.
        depth: Option<String>,
    },
    /// Resume another session id.
    Resume {
        /// Target session id.
        session_id: String,
    },
    /// Switch chat model.
    Model {
        /// Target model id.
        model_id: Option<String>,
    },
    /// Clear the current chat surface.
    Clear,
}

/// Mutable context passed to slash handlers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandContext {
    /// Current session id, if the command was run inside an existing chat.
    pub session_id: Option<String>,
}

/// Result returned by a slash handler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandResult {
    /// Structured action for the application layer.
    pub action: SlashAction,
    /// A short user-facing acknowledgement or error hint.
    pub message: String,
}

impl CommandResult {
    /// Build a command result.
    pub fn new(action: SlashAction, message: impl Into<String>) -> Self {
        Self {
            action,
            message: message.into(),
        }
    }
}

/// A slash command handler.
pub trait SlashHandler: Send + Sync {
    /// Execute the command over its raw argument string.
    fn execute(&self, args: &str, ctx: &mut CommandContext) -> Result<CommandResult>;
}

/// A slash-command definition.
#[derive(Clone)]
pub struct SlashCommand {
    /// Command name without the leading slash.
    pub name: String,
    /// Short description for UI autocomplete.
    pub description: String,
    /// Command handler.
    pub handler: Arc<dyn SlashHandler>,
}

impl std::fmt::Debug for SlashCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SlashCommand")
            .field("name", &self.name)
            .field("description", &self.description)
            .finish_non_exhaustive()
    }
}

impl SlashCommand {
    /// Build a slash-command definition.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        handler: impl SlashHandler + 'static,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            handler: Arc::new(handler),
        }
    }
}

/// Registry of slash commands keyed by command name.
#[derive(Debug, Clone, Default)]
pub struct SlashRegistry {
    commands: BTreeMap<String, SlashCommand>,
}

impl SlashRegistry {
    /// New empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register or replace a command.
    pub fn register_command(&mut self, command: SlashCommand) -> &mut Self {
        self.commands.insert(command.name.clone(), command);
        self
    }

    /// Build the built-in command set.
    pub fn with_builtins() -> Self {
        let mut registry = Self::new();
        registry
            .register_command(SlashCommand::new(
                "compact",
                "压缩当前会话上下文，降低后续请求的上下文体积",
                StaticHandler::new(SlashActionTemplate::Compact),
            ))
            .register_command(SlashCommand::new(
                "cost",
                "查看当前会话、当天、本月和累计费用",
                StaticHandler::new(SlashActionTemplate::Cost),
            ))
            .register_command(SlashCommand::new(
                "doctor",
                "运行环境诊断，检查配置、数据库、权限和 API Key",
                StaticHandler::new(SlashActionTemplate::Doctor),
            ))
            .register_command(SlashCommand::new(
                "help",
                "查看可用的 slash 命令列表",
                StaticHandler::new(SlashActionTemplate::Help),
            ))
            .register_command(SlashCommand::new(
                "status",
                "查看当前项目、模型、思考深度和运行状态",
                StaticHandler::new(SlashActionTemplate::Status),
            ))
            .register_command(SlashCommand::new(
                "settings",
                "查看 DeepSeek 配置、模型、权限和思考设置",
                StaticHandler::new(SlashActionTemplate::Settings),
            ))
            .register_command(SlashCommand::new(
                "config",
                "/settings 的别名，查看当前配置摘要",
                StaticHandler::new(SlashActionTemplate::Settings),
            ))
            .register_command(SlashCommand::new(
                "permissions",
                "查看当前工具权限策略和 allow/ask/deny 规则",
                StaticHandler::new(SlashActionTemplate::Permissions),
            ))
            .register_command(SlashCommand::new(
                "knowledge",
                "查看项目知识库、草稿、被动注入和自动捕获状态",
                StaticHandler::new(SlashActionTemplate::Knowledge),
            ))
            .register_command(SlashCommand::new(
                "memory",
                "/knowledge 的别名，查看知识库状态",
                StaticHandler::new(SlashActionTemplate::Knowledge),
            ))
            .register_command(SlashCommand::new(
                "mcp",
                "查看已配置的 MCP 服务及启用状态",
                StaticHandler::new(SlashActionTemplate::Mcp),
            ))
            .register_command(SlashCommand::new(
                "projects",
                "查看已打开项目和当前激活项目",
                StaticHandler::new(SlashActionTemplate::Projects),
            ))
            .register_command(SlashCommand::new(
                "sessions",
                "查看最近会话列表",
                StaticHandler::new(SlashActionTemplate::Sessions),
            ))
            .register_command(SlashCommand::new(
                "thinking",
                "查看或设置思考深度：simple、medium、deep",
                StaticHandler::new(SlashActionTemplate::Thinking),
            ))
            .register_command(SlashCommand::new(
                "effort",
                "/thinking 的别名，查看或设置思考深度",
                StaticHandler::new(SlashActionTemplate::Thinking),
            ))
            .register_command(SlashCommand::new(
                "plan",
                "进入只读 Plan 模式，先规划再执行",
                StaticHandler::new(SlashActionTemplate::EnterPlanMode),
            ))
            .register_command(SlashCommand::new(
                "execute",
                "退出 Plan 模式，恢复正常执行权限",
                StaticHandler::new(SlashActionTemplate::ExitPlanMode),
            ))
            .register_command(SlashCommand::new(
                "resume",
                "按会话 ID 恢复历史会话上下文",
                StaticHandler::new(SlashActionTemplate::Resume),
            ))
            .register_command(SlashCommand::new(
                "model",
                "切换当前聊天模型，例如 /model deepseek-v4-pro",
                StaticHandler::new(SlashActionTemplate::Model),
            ))
            .register_command(SlashCommand::new(
                "clear",
                "清空当前聊天输入界面提示",
                StaticHandler::new(SlashActionTemplate::Clear),
            ));
        registry
    }

    /// Look up a command by name.
    pub fn get(&self, name: &str) -> Option<&SlashCommand> {
        self.commands.get(name)
    }

    /// All registered command names.
    pub fn names(&self) -> Vec<String> {
        self.commands.keys().cloned().collect()
    }

    /// Parse and execute a line. Returns `None` when the line is not a slash
    /// command; returns an error for an unknown slash command.
    pub fn execute_line(
        &self,
        line: &str,
        ctx: &mut CommandContext,
    ) -> Option<Result<CommandResult>> {
        let trimmed = line.trim_start();
        let rest = trimmed.strip_prefix('/')?;
        let (name, args) = split_command(rest);
        if name.is_empty() {
            return None;
        }
        let Some(command) = self.get(name) else {
            return Some(Err(CoreError::invalid(format!(
                "unknown slash command: /{name}"
            ))));
        };
        Some(command.handler.execute(args, ctx))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlashActionTemplate {
    EnterPlanMode,
    ExitPlanMode,
    Compact,
    Cost,
    Doctor,
    Help,
    Status,
    Settings,
    Permissions,
    Knowledge,
    Mcp,
    Projects,
    Sessions,
    Thinking,
    Resume,
    Model,
    Clear,
}

#[derive(Debug, Clone)]
struct StaticHandler {
    action: SlashActionTemplate,
}

impl StaticHandler {
    fn new(action: SlashActionTemplate) -> Self {
        Self { action }
    }
}

impl SlashHandler for StaticHandler {
    fn execute(&self, args: &str, _ctx: &mut CommandContext) -> Result<CommandResult> {
        let args = args.trim();
        let result = match self.action {
            SlashActionTemplate::EnterPlanMode => CommandResult::new(
                SlashAction::EnterPlanMode,
                "Entered Plan mode. Write operations are disabled; use /execute when ready.",
            ),
            SlashActionTemplate::ExitPlanMode => CommandResult::new(
                SlashAction::ExitPlanMode,
                "Exited Plan mode. Normal tool permissions are restored.",
            ),
            SlashActionTemplate::Compact => {
                CommandResult::new(SlashAction::Compact, "Compacted current session context.")
            }
            SlashActionTemplate::Cost => CommandResult::new(SlashAction::Cost, "Cost summary."),
            SlashActionTemplate::Doctor => {
                CommandResult::new(SlashAction::Doctor, "Environment diagnostics.")
            }
            SlashActionTemplate::Help => {
                CommandResult::new(SlashAction::Help, "Available slash commands.")
            }
            SlashActionTemplate::Status => {
                CommandResult::new(SlashAction::Status, "Runtime status.")
            }
            SlashActionTemplate::Settings => {
                CommandResult::new(SlashAction::Settings, "Settings summary.")
            }
            SlashActionTemplate::Permissions => {
                CommandResult::new(SlashAction::Permissions, "Permission summary.")
            }
            SlashActionTemplate::Knowledge => {
                CommandResult::new(SlashAction::Knowledge, "Knowledge status.")
            }
            SlashActionTemplate::Mcp => CommandResult::new(SlashAction::Mcp, "MCP server summary."),
            SlashActionTemplate::Projects => {
                CommandResult::new(SlashAction::Projects, "Opened projects.")
            }
            SlashActionTemplate::Sessions => {
                CommandResult::new(SlashAction::Sessions, "Recent sessions.")
            }
            SlashActionTemplate::Thinking => CommandResult::new(
                SlashAction::Thinking {
                    depth: if args.is_empty() {
                        None
                    } else {
                        Some(args.to_string())
                    },
                },
                "Thinking depth.",
            ),
            SlashActionTemplate::Resume => {
                if args.is_empty() {
                    return Err(CoreError::invalid("usage: /resume <session_id>"));
                }
                CommandResult::new(
                    SlashAction::Resume {
                        session_id: args.to_string(),
                    },
                    format!("Resumed session {args}."),
                )
            }
            SlashActionTemplate::Model => CommandResult::new(
                SlashAction::Model {
                    model_id: if args.is_empty() {
                        None
                    } else {
                        Some(args.to_string())
                    },
                },
                "Model selection.",
            ),
            SlashActionTemplate::Clear => {
                CommandResult::new(SlashAction::Clear, "Cleared the chat surface.")
            }
        };
        Ok(result)
    }
}

fn split_command(rest: &str) -> (&str, &str) {
    match rest.find(char::is_whitespace) {
        Some(idx) => (&rest[..idx], rest[idx..].trim()),
        None => (rest, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_include_required_commands() {
        let names = SlashRegistry::with_builtins().names();
        for expected in [
            "compact",
            "cost",
            "doctor",
            "help",
            "status",
            "settings",
            "config",
            "permissions",
            "knowledge",
            "memory",
            "mcp",
            "projects",
            "sessions",
            "thinking",
            "effort",
            "plan",
            "execute",
            "resume",
            "model",
            "clear",
        ] {
            assert!(names.contains(&expected.to_string()), "missing {expected}");
        }
    }

    #[test]
    fn executes_known_command() {
        let reg = SlashRegistry::with_builtins();
        let mut ctx = CommandContext::default();
        let out = reg.execute_line("/plan", &mut ctx).unwrap().unwrap();
        assert_eq!(out.action, SlashAction::EnterPlanMode);
    }

    #[test]
    fn slash_with_args_maps_to_structured_action() {
        let reg = SlashRegistry::with_builtins();
        let mut ctx = CommandContext::default();
        let out = reg
            .execute_line("/model deepseek-v4-pro", &mut ctx)
            .unwrap()
            .unwrap();
        assert_eq!(
            out.action,
            SlashAction::Model {
                model_id: Some("deepseek-v4-pro".into())
            }
        );
    }

    #[test]
    fn thinking_with_args_maps_to_structured_action() {
        let reg = SlashRegistry::with_builtins();
        let mut ctx = CommandContext::default();
        let out = reg
            .execute_line("/thinking deep", &mut ctx)
            .unwrap()
            .unwrap();
        assert_eq!(
            out.action,
            SlashAction::Thinking {
                depth: Some("deep".into())
            }
        );
    }

    #[test]
    fn model_without_args_maps_to_selection_action() {
        let reg = SlashRegistry::with_builtins();
        let mut ctx = CommandContext::default();
        let out = reg.execute_line("/model", &mut ctx).unwrap().unwrap();
        assert_eq!(out.action, SlashAction::Model { model_id: None });
    }

    #[test]
    fn unknown_slash_errors_but_plain_chat_is_none() {
        let reg = SlashRegistry::with_builtins();
        let mut ctx = CommandContext::default();
        assert!(reg.execute_line("hello", &mut ctx).is_none());
        assert!(reg.execute_line("/missing", &mut ctx).unwrap().is_err());
    }
}
