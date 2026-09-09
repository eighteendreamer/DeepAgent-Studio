//! Slash-command runtime extracted from [`crate::chat_service`].
//!
//! This module owns the [`SlashRuntime`] handle and the free functions that
//! dispatch slash commands. The chat service constructs a [`SlashRuntime`] from
//! its fields and delegates; the Tauri layer's public API is unchanged.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use deepagent_context::{
    HeuristicSummarizer, HeuristicTokenizer, Summarizer, TaskSummary, TokenCounter,
};
use deepagent_core::clock::{Clock, SystemClock};
use deepagent_core::error::{CoreError, Result};
use deepagent_core::event::EventPayload;
use deepagent_core::message::Message;
use deepagent_intent::{CommandContext, CommandDef, SlashAction, SlashRegistry};
use deepagent_models::ThinkingDepth;
use deepagent_persistence::Database;
use deepagent_runtime::RuntimeEvent;
use deepagent_session::Session;

use crate::cost_service::CostService;
use crate::input_runtime::conversation_from_events;
use crate::knowledge_service::KnowledgeService;
use crate::mcp_service::McpService;
use crate::plugin_runtime::PluginRuntimeProjection;
use crate::plugin_service::PluginService;
use crate::project_service::ProjectService;
use crate::settings::SettingsService;
use crate::skills_service::SkillsService;
use crate::slash_panel::{kv, SlashPanel, SlashPanelItem, SlashSection};

// ---------------------------------------------------------------------------
// SlashRuntime: borrow-checker-friendly handle over the services that slash
// commands need. The chat service builds one from its fields and calls
// `apply_slash_action` / `run_workspace_verification` on it.
// ---------------------------------------------------------------------------

/// Borrowed view of the services a slash command needs. Constructed by the
/// chat service on each call so the slash code never holds its own `Arc`.
pub struct SlashRuntime<'a> {
    pub db: &'a Database,
    pub settings: &'a SettingsService,
    pub workspace: &'a Path,
    pub cost: Option<&'a CostService>,
    pub knowledge: Option<&'a KnowledgeService>,
    pub mcp: Option<&'a McpService>,
    pub projects: Option<&'a ProjectService>,
    pub skills: Option<&'a Mutex<SkillsService>>,
    pub tool_results_dir: &'a Path,
    pub plan_modes: &'a Mutex<HashMap<String, deepagent_builtins::PlanMode>>,
    pub plugins: Option<&'a PluginService>,
}

impl<'a> SlashRuntime<'a> {
    /// The effective project root: the active project's folder when a project
    /// registry is attached and a project is active, else the default launch
    /// workspace. Mirrors [`ChatService::effective_root`].
    fn effective_root(&self) -> PathBuf {
        if let Some(projects) = self.projects {
            if let Ok(Some(active)) = projects.active() {
                if !active.trim().is_empty() {
                    return PathBuf::from(active);
                }
            }
        }
        self.workspace.to_path_buf()
    }

    fn plugin_runtime_projection(&self) -> Result<Option<PluginRuntimeProjection>> {
        match self.plugins {
            Some(plugins) => plugins.runtime_projection().map(Some),
            None => Ok(None),
        }
    }

    fn is_plan_mode(&self, session_id: &str) -> bool {
        let map = self.plan_modes.lock().unwrap_or_else(|p| p.into_inner());
        map.get(session_id)
            .map(deepagent_builtins::PlanMode::is_active)
            .unwrap_or(false)
    }

    fn set_plan_mode(&self, session_id: &str, active: bool) -> bool {
        let plan = {
            let mut map = self.plan_modes.lock().unwrap_or_else(|p| p.into_inner());
            map.entry(session_id.to_string()).or_default().clone()
        };
        plan.set(active);
        plan.is_active()
    }

    /// Dispatch a parsed slash-command result against the live session.
    /// This is the large `match` over [`SlashAction`] variants that was
    /// previously [`ChatService::apply_slash_action`].
    pub async fn apply_slash_action(
        &self,
        session_id: &str,
        session: &mut Session<'_, SystemClock>,
        result: deepagent_intent::CommandResult,
    ) -> Result<String> {
        let message = match result.action {
            SlashAction::EnterPlanMode => {
                self.set_plan_mode(session_id, true);
                result.message
            }
            SlashAction::ExitPlanMode => {
                self.set_plan_mode(session_id, false);
                result.message
            }
            SlashAction::Compact => {
                let store =
                    deepagent_persistence::event_store::EventStore::new(self.db);
                let events = store.load_session(session.id())?;
                let history = conversation_from_events(&events);
                let rendered: Vec<String> = history
                    .iter()
                    .map(|m| format!("{:?}: {}", m.role, m.content))
                    .collect();
                let counter = HeuristicTokenizer::new();
                let tokens_before: usize =
                    rendered.iter().map(|t| counter.count(t)).sum();
                let tokens_after = tokens_before / 2;
                session.append(EventPayload::ContextCompacted {
                    tokens_before: tokens_before as u64,
                    tokens_after: tokens_after as u64,
                    strategy: "manual".to_string(),
                })?;
                format!(
                    "Compacted current session context. Tokens before: {tokens_before}; target after: {tokens_after}."
                )
            }
            SlashAction::Cost => match self.cost {
                Some(cost) => {
                    let s = cost.summary(session_id)?;
                    SlashPanel::new("费用")
                        .items(vec![
                            kv(
                                "本会话",
                                format!("{} {:.4}", s.currency, s.session_cost),
                            ),
                            kv(
                                "今日",
                                format!("{} {:.4}", s.currency, s.today_cost),
                            ),
                            kv(
                                "本月",
                                format!("{} {:.4}", s.currency, s.month_cost),
                            ),
                            kv(
                                "累计",
                                format!("{} {:.4}", s.currency, s.total_cost),
                            ),
                        ])
                        .to_fenced()
                }
                None => "费用跟踪未启用。".to_string(),
            },
            SlashAction::Doctor => {
                let root = self.effective_root();
                let results = crate::doctor::run_diagnostics(
                    self.settings,
                    self.db,
                    &root,
                    self.tool_results_dir,
                )
                .await;
                let ok = results
                    .iter()
                    .filter(|r| r.status == crate::doctor::DiagStatus::Ok)
                    .count();
                let items: Vec<SlashPanelItem> = results
                    .iter()
                    .map(|r| {
                        let accent = match r.status {
                            crate::doctor::DiagStatus::Ok => "ok",
                            crate::doctor::DiagStatus::Warning => "warn",
                            crate::doctor::DiagStatus::Error => "error",
                        };
                        let value = match &r.fix_hint {
                            Some(h) if r.status != crate::doctor::DiagStatus::Ok => {
                                format!("{} · {h}", r.detail)
                            }
                            _ => r.detail.clone(),
                        };
                        SlashPanelItem::new(&r.name).status(accent).value(value)
                    })
                    .collect();
                SlashPanel::new("环境诊断")
                    .subtitle(format!("{}/{} 项通过", ok, results.len()))
                    .items(items)
                    .to_fenced()
            }
            SlashAction::Help => {
                let registry = SlashRegistry::with_builtins();
                let items: Vec<SlashPanelItem> = registry
                    .names()
                    .iter()
                    .filter_map(|name| {
                        registry.get(name).map(|command| {
                            SlashPanelItem::new(format!("/{}", command.name))
                                .monospace()
                                .value(command.description.clone())
                        })
                    })
                    .collect();
                SlashPanel::new("斜杠命令")
                    .subtitle(format!("{} 个可用命令", items.len()))
                    .items(items)
                    .to_fenced()
            }
            SlashAction::Status => {
                let root = self.effective_root();
                let plan = if self.is_plan_mode(session_id) {
                    "开启"
                } else {
                    "关闭"
                };
                let settings = self.settings.view()?;
                let (configured, chat_model, thinking_depth, web_search) = settings
                    .as_ref()
                    .map(|s| {
                        (
                            s.configured,
                            s.chat_model.as_str(),
                            s.thinking_depth.as_str(),
                            web_search_summary(&s.web_search),
                        )
                    })
                    .unwrap_or((false, "(未初始化)", "medium", "default".to_string()));
                let approval = self.settings.approval_policy()?.label();
                SlashPanel::new("状态")
                    .section(SlashSection::new(
                        "项目",
                        vec![
                            kv("目录", root.display().to_string()).monospace(),
                            SlashPanelItem::new("已配置")
                                .status(if configured { "ok" } else { "warn" })
                                .value(if configured { "是" } else { "否" }),
                            kv("计划模式", plan),
                        ],
                    ))
                    .section(SlashSection::new(
                        "模型与运行时",
                        vec![
                            kv("聊天模型", chat_model).monospace(),
                            kv("思考档位", thinking_depth),
                            kv("审批策略", approval),
                            kv("网页搜索", web_search),
                        ],
                    ))
                    .to_fenced()
            }
            SlashAction::Settings => match self.settings.view()? {
                Some(s) => {
                    let mut items = vec![
                        SlashPanelItem::new("已配置")
                            .status(if s.configured { "ok" } else { "warn" })
                            .value(if s.configured { "是" } else { "否" }),
                        kv("API Key", s.api_key_masked.clone()).monospace(),
                        kv("Base URL", s.base_url.clone()).monospace(),
                        kv("聊天模型", s.chat_model.clone()).monospace(),
                        kv("推理模型", s.reasoner_model.clone()).monospace(),
                        kv("思考档位", s.thinking_depth.clone()),
                        kv("审批策略", s.approval_policy.clone()),
                        kv("网页搜索", web_search_summary(&s.web_search)),
                    ];
                    let mut models = SlashPanelItem::new("可用模型");
                    if s.available_models.is_empty() {
                        models = models.value("(无)").status("muted");
                    } else {
                        for m in &s.available_models {
                            models = models.badge(m.clone());
                        }
                    }
                    items.push(models);
                    SlashPanel::new("设置").items(items).to_fenced()
                }
                None => "设置尚未初始化。请先添加 DeepSeek API Key。".to_string(),
            },
            SlashAction::Permissions => {
                let policy = self.settings.approval_policy()?;
                let rules = self.settings.permission_rules()?;
                let rule_items = |list: &[String]| -> Vec<SlashPanelItem> {
                    if list.is_empty() {
                        vec![SlashPanelItem::new("(无)").status("muted")]
                    } else {
                        list.iter()
                            .map(|r| SlashPanelItem::new(r).monospace())
                            .collect()
                    }
                };
                SlashPanel::new("权限")
                    .items(vec![kv("策略", policy.label())])
                    .section(SlashSection::new(
                        format!("允许 ({})", rules.allow.len()),
                        rule_items(&rules.allow),
                    ))
                    .section(SlashSection::new(
                        format!("询问 ({})", rules.ask.len()),
                        rule_items(&rules.ask),
                    ))
                    .section(SlashSection::new(
                        format!("拒绝 ({})", rules.deny.len()),
                        rule_items(&rules.deny),
                    ))
                    .to_fenced()
            }
            SlashAction::Knowledge => match self.knowledge {
                Some(knowledge) => SlashPanel::new("知识库")
                    .items(vec![
                        kv("项目条目", knowledge.list().len().to_string()),
                        kv(
                            "待处理草稿",
                            knowledge.list_drafts().len().to_string(),
                        ),
                        SlashPanelItem::new("被动注入")
                            .status(if knowledge.passive_enabled() {
                                "ok"
                            } else {
                                "muted"
                            })
                            .value(on_off(knowledge.passive_enabled())),
                        SlashPanelItem::new("自动采集")
                            .status(if knowledge.auto_capture_enabled() {
                                "ok"
                            } else {
                                "muted"
                            })
                            .value(on_off(knowledge.auto_capture_enabled())),
                    ])
                    .to_fenced(),
                None => "当前运行时未启用知识库。".to_string(),
            },
            SlashAction::Mcp => match self.mcp {
                Some(mcp) => {
                    let plugin_projection = self.plugin_runtime_projection()?;
                    let servers = match plugin_projection.as_ref() {
                        Some(projection)
                            if !projection.mcp_config.servers.is_empty() =>
                        {
                            mcp.list_with_plugin_overlay(
                                projection.mcp_config.clone(),
                                &projection.mcp_server_sources,
                            )?
                        }
                        _ => mcp.list()?,
                    };
                    if servers.is_empty() {
                        SlashPanel::new("MCP 服务器")
                            .subtitle("还没有 MCP 服务器")
                            .items(vec![])
                            .to_fenced()
                    } else {
                        let statuses = match plugin_projection.as_ref() {
                            Some(projection)
                                if !projection.mcp_config.servers.is_empty() =>
                            {
                                mcp.connection_status_with_plugin_overlay(
                                    projection.mcp_config.clone(),
                                    &projection.mcp_server_sources,
                                )
                                .await?
                            }
                            _ => mcp.connection_status().await?,
                        };
                        let enabled =
                            servers.iter().filter(|s| s.enabled).count();
                        let items: Vec<SlashPanelItem> = servers
                            .iter()
                            .map(|s| {
                                let st =
                                    statuses.iter().find(|x| x.name == s.name);
                                let accent =
                                    match st.map(|x| x.status.as_str()) {
                                        Some("connected") => "ok",
                                        Some("failed") => "error",
                                        Some("disabled") => "muted",
                                        _ if s.enabled => "info",
                                        _ => "muted",
                                    };
                                let mut item = SlashPanelItem::new(&s.name)
                                    .status(accent)
                                    .monospace();
                                match st {
                                    Some(st) if st.status == "connected" => {
                                        item = item
                                            .value(&s.transport)
                                            .badge(format!(
                                                "{} 工具",
                                                st.tools.len()
                                            ))
                                            .children(
                                                st.tools
                                                    .iter()
                                                    .map(|t| {
                                                        let c =
                                                            SlashPanelItem::new(
                                                                &t.name,
                                                            );
                                                        if t.description
                                                            .is_empty()
                                                        {
                                                            c
                                                        } else {
                                                            c.value(
                                                                &t.description,
                                                            )
                                                        }
                                                    })
                                                    .collect(),
                                            );
                                    }
                                    Some(st) if st.status == "failed" => {
                                        item = item.value(match &st.error {
                                            Some(e) => {
                                                format!("{} · {e}", s.transport)
                                            }
                                            None => s.transport.clone(),
                                        });
                                    }
                                    _ => {
                                        item = item.value(&s.transport);
                                    }
                                }
                                if s.read_only {
                                    item = item.badge("插件");
                                }
                                if let Some(conflict) = &s.conflict {
                                    item =
                                        item.value(conflict).status("warn");
                                }
                                item
                            })
                            .collect();
                        SlashPanel::new("MCP 服务器")
                            .subtitle(format!(
                                "{enabled}/{} 已启用",
                                servers.len()
                            ))
                            .items(items)
                            .to_fenced()
                    }
                }
                None => "当前运行时未配置 MCP。".to_string(),
            },
            SlashAction::Projects => match self.projects {
                Some(projects) => {
                    let active = projects.active()?;
                    let list = projects.list()?;
                    let items: Vec<SlashPanelItem> = list
                        .iter()
                        .map(|p| {
                            let is_active =
                                active.as_deref() == Some(p.path.as_str());
                            let mut item = SlashPanelItem::new(&p.name)
                                .value(&p.path)
                                .status(if is_active {
                                    "ok"
                                } else {
                                    "muted"
                                })
                                .badge(format!("{} 会话", p.session_count));
                            if is_active {
                                item = item.badge("当前");
                            }
                            item
                        })
                        .collect();
                    SlashPanel::new("项目")
                        .subtitle(format!("{} 个已打开", list.len()))
                        .items(items)
                        .to_fenced()
                }
                None => SlashPanel::new("项目")
                    .items(vec![kv(
                        "当前",
                        self.effective_root().display().to_string(),
                    )
                    .monospace()])
                    .to_fenced(),
            },
            SlashAction::Sessions => {
                let store =
                    deepagent_persistence::event_store::EventStore::new(
                        self.db,
                    );
                let sessions = store.list_sessions()?;
                let items: Vec<SlashPanelItem> = sessions
                    .iter()
                    .take(12)
                    .map(|record| {
                        let title =
                            record.title.as_deref().unwrap_or("(未命名)");
                        let project = record
                            .project
                            .as_deref()
                            .map(crate::project_service::folder_name)
                            .unwrap_or_else(|| "(无项目)".to_string());
                        SlashPanelItem::new(title)
                            .value(record.id.to_string())
                            .badge(project)
                    })
                    .collect();
                SlashPanel::new("近期会话")
                    .subtitle(format!("{} 个会话", sessions.len()))
                    .items(items)
                    .to_fenced()
            }
            SlashAction::Thinking { depth } => match depth {
                Some(depth) => {
                    let parsed = parse_thinking_depth(&depth)?;
                    let view =
                        self.settings.set_thinking_depth(parsed)?;
                    format!("Thinking depth set to {}.", view.thinking_depth)
                }
                None => {
                    let depth = self.settings.thinking_depth()?;
                    format!(
                        "Thinking depth is {}. Usage: /thinking <simple|medium|deep>.",
                        depth.label()
                    )
                }
            },
            SlashAction::Resume { session_id: _ } => {
                let store =
                    deepagent_persistence::event_store::EventStore::new(
                        self.db,
                    );
                let events = store.load_session(session.id())?;
                let history = conversation_from_events(&events);
                let rendered: Vec<String> = history
                    .iter()
                    .map(|m| format!("{:?}: {}", m.role, m.content))
                    .collect();
                let counter = HeuristicTokenizer::new();
                let tokens_before: usize =
                    rendered.iter().map(|t| counter.count(t)).sum();
                let goal = history
                    .first()
                    .map(|m| m.content.clone())
                    .unwrap_or_else(|| {
                        format!("Resume session {session_id}")
                    });
                let summary = HeuristicSummarizer.summarize(
                    &goal,
                    &TaskSummary::default(),
                    &rendered,
                );
                let summary_block = summary.to_context_block();
                let injected = format!(
                    "[Earlier conversation compacted to summary]\n{summary_block}"
                );
                let tokens_after = counter.count(&injected);
                session.append(EventPayload::ContextCompacted {
                    tokens_before: tokens_before as u64,
                    tokens_after: tokens_after as u64,
                    strategy: "resume".to_string(),
                })?;
                session.append(EventPayload::MessageAppended {
                    message: Message::user(injected),
                })?;
                format!(
                    "Resumed session {session_id}. Loaded {} event(s), compacted recovered context from {tokens_before} to {tokens_after} estimated tokens. Continue with your next prompt.",
                    events.len()
                )
            }
            SlashAction::Model { model_id } => match model_id {
                Some(model_id) => {
                    self.settings
                        .set_model(deepagent_models::ModelRole::Chat, &model_id)?;
                    format!("Switched chat model to {model_id}.")
                }
                None => match self.settings.view()? {
                    Some(view) if !view.available_models.is_empty() => format!(
                        "可用模型:\n- {}\n\n用法: /model <model_id>",
                        view.available_models.join("\n- ")
                    ),
                    Some(_) => {
                        "没有发现可用模型。请先刷新 DeepSeek 模型列表。"
                            .to_string()
                    }
                    None => {
                        "设置尚未初始化。请先填写并验证 DeepSeek API Key。"
                            .to_string()
                    }
                },
            },
            SlashAction::Clear => {
                "Cleared the chat surface. Start a new chat from the sidebar for a fresh session."
                    .to_string()
            }
            SlashAction::Verify => self.run_workspace_verification().await,
            SlashAction::Export => {
                let store =
                    deepagent_persistence::event_store::EventStore::new(
                        self.db,
                    );
                let events = store.load_session(session.id())?;
                let history = conversation_from_events(&events);
                let title = session
                    .state()
                    .title
                    .clone()
                    .unwrap_or_else(|| "session".to_string());
                let mut md = format!("# {title}\n\n");
                for m in &history {
                    md.push_str(&format!(
                        "## {:?}\n\n{}\n\n",
                        m.role, m.content
                    ));
                }
                let dir = self
                    .effective_root()
                    .join(".deepagent")
                    .join("exports");
                std::fs::create_dir_all(&dir).map_err(|e| {
                    CoreError::other(format!("create export dir: {e}"))
                })?;
                let file =
                    dir.join(format!("{session_id}.md"));
                std::fs::write(&file, md).map_err(|e| {
                    CoreError::other(format!("write export: {e}"))
                })?;
                format!(
                    "已导出 {} 条消息到 {}",
                    history.len(),
                    file.display()
                )
            }
            SlashAction::Rewind => {
                let store =
                    deepagent_persistence::event_store::EventStore::new(
                        self.db,
                    );
                let events = store.load_session(session.id())?;
                format!(
                    "当前会话共有 {} 条事件。回退是破坏性操作：它会永久删除某个检查点之后的所有事件。请使用聊天菜单中的 Rewind 选择回退点，或用 Fork 进行非破坏性分支。",
                    events.len()
                )
            }
            SlashAction::Rename { title } => match title {
                Some(title) => {
                    let trimmed = title.trim();
                    if trimmed.is_empty() {
                        "用法: /rename <新标题>".to_string()
                    } else {
                        let store =
                            deepagent_persistence::event_store::EventStore::new(
                                self.db,
                            );
                        let clock = SystemClock;
                        if store.rename_session(
                            session.id(),
                            Some(trimmed),
                            clock.now(),
                        )? {
                            format!("会话已重命名为「{trimmed}」。")
                        } else {
                            "重命名失败：找不到当前会话。".to_string()
                        }
                    }
                }
                None => "用法: /rename <新标题>".to_string(),
            },
            SlashAction::Skills => match self.skills {
                Some(skills) => {
                    let list = skills
                        .lock()
                        .map_err(|_| {
                            CoreError::other("skills lock poisoned")
                        })?
                        .list();
                    if list.is_empty() {
                        "尚未安装任何技能。打开「技能」页可浏览技能市场。"
                            .to_string()
                    } else {
                        let mut by_origin: BTreeMap<
                            String,
                            Vec<SlashPanelItem>,
                        > = BTreeMap::new();
                        for s in &list {
                            by_origin
                                .entry(s.origin.clone())
                                .or_default()
                                .push(
                                    SlashPanelItem::new(&s.name).value(
                                        truncate_desc(&s.description, 90),
                                    ),
                                );
                        }
                        let mut panel = SlashPanel::new("已安装技能")
                            .subtitle(format!("{} 个技能", list.len()));
                        for (origin, items) in by_origin {
                            panel = panel
                                .section(SlashSection::new(origin, items));
                        }
                        panel.to_fenced()
                    }
                }
                None => "当前运行时未启用技能系统。".to_string(),
            },
            SlashAction::Plugins => {
                "插件在「插件」页管理（安装、启用、配置）。可用的运行时能力包括终端、文件预览、录音、浏览器、侧栏聊天等。".to_string()
            }
            SlashAction::Hooks => {
                let defs = self.settings.hook_definitions()?;
                let rules = self.settings.permission_rules()?;
                let event_items: Vec<SlashPanelItem> = defs
                    .hooks
                    .iter()
                    .map(|(event, groups)| {
                        let children: Vec<SlashPanelItem> = groups
                            .iter()
                            .map(|g| {
                                SlashPanelItem::new(
                                    g.matcher
                                        .clone()
                                        .unwrap_or_else(|| "*".to_string()),
                                )
                                .monospace()
                                .badge(format!("{} hook", g.hooks.len()))
                            })
                            .collect();
                        SlashPanelItem::new(event)
                            .badge(format!("{} matcher", groups.len()))
                            .children(children)
                    })
                    .collect();
                let rule_items =
                    |list: &[String]| -> Vec<SlashPanelItem> {
                        if list.is_empty() {
                            vec![SlashPanelItem::new("(无)").status("muted")]
                        } else {
                            list.iter()
                                .map(|r| {
                                    SlashPanelItem::new(r).monospace()
                                })
                                .collect()
                        }
                    };
                let mut panel = SlashPanel::new("Hooks")
                    .subtitle(format!(
                        "{} 个事件类型",
                        defs.hooks.len()
                    ));
                if event_items.is_empty() {
                    panel = panel.items(vec![SlashPanelItem::new("(未配置钩子)")
                        .status("muted")]);
                } else {
                    panel = panel
                        .section(SlashSection::new("事件", event_items));
                }
                panel
                    .section(SlashSection::new(
                        format!("允许 ({})", rules.allow.len()),
                        rule_items(&rules.allow),
                    ))
                    .section(SlashSection::new(
                        format!("询问 ({})", rules.ask.len()),
                        rule_items(&rules.ask),
                    ))
                    .section(SlashSection::new(
                        format!("拒绝 ({})", rules.deny.len()),
                        rule_items(&rules.deny),
                    ))
                    .to_fenced()
            }
            SlashAction::Theme => {
                "主题与外观在「设置 → 外观」中调整（暗色/亮色、界面选项）。"
                    .to_string()
            }
            SlashAction::Agents => {
                let mut roots =
                    vec![self.effective_root(), self.workspace.to_path_buf()];
                roots.dedup();
                let plugin_projection =
                    self.plugin_runtime_projection()?;
                let mut items: Vec<SlashPanelItem> = Vec::new();
                let mut count = 0usize;
                for root in roots {
                    let dir =
                        root.join(".deepagent").join("agents");
                    collect_agent_items(&dir, None, &mut items, &mut count);
                }
                if let Some(projection) = plugin_projection {
                    for root in projection.agent_roots {
                        collect_agent_items(
                            &root.path,
                            Some(root.plugin_name.as_str()),
                            &mut items,
                            &mut count,
                        );
                    }
                }
                if count == 0 {
                    "未发现子代理定义。可在 .deepagent/agents/ 下添加 <name>.md（YAML frontmatter + 系统提示）。".to_string()
                } else {
                    SlashPanel::new("子代理")
                        .subtitle(format!("{count} 个可用"))
                        .items(items)
                        .to_fenced()
                }
            }
            SlashAction::Context => {
                let store =
                    deepagent_persistence::event_store::EventStore::new(
                        self.db,
                    );
                let events = store.load_session(session.id())?;
                let history = conversation_from_events(&events);
                let counter = HeuristicTokenizer::new();
                let tokens: usize = history
                    .iter()
                    .map(|m| {
                        counter.count(&format!(
                            "{:?}: {}",
                            m.role, m.content
                        ))
                    })
                    .sum();
                SlashPanel::new("上下文使用")
                    .subtitle(
                        "可用 /compact 压缩上下文以降低后续请求体积",
                    )
                    .items(vec![
                        kv("消息条数", history.len().to_string()),
                        kv("事件条数", events.len().to_string()),
                        SlashPanelItem::new("估算 token")
                            .value(tokens.to_string())
                            .status(if tokens > 100_000 {
                                "warn"
                            } else {
                                "ok"
                            }),
                    ])
                    .to_fenced()
            }
            SlashAction::Init => {
                let root = self.effective_root();
                let file = root.join("AGENTS.md");
                if file.exists() {
                    format!("项目说明文档已存在：{}", file.display())
                } else {
                    let project_name = root
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("Project")
                        .to_string();
                    let template = format!(
                        "# {project_name}\n\n## 项目概述\n\n<!-- 一句话描述这个项目的目标 -->\n\n## 技术栈\n\n<!-- 主要语言、框架、构建工具 -->\n\n## 目录结构\n\n<!-- 关键目录及其职责 -->\n\n## 开发约定\n\n<!-- 代码风格、命名、提交规范 -->\n\n## 构建与测试\n\n<!-- 常用命令 -->\n\n## 注意事项\n\n<!-- 易踩的坑、需要人工确认的高风险操作 -->\n"
                    );
                    std::fs::write(&file, template).map_err(|e| {
                        CoreError::other(format!("write AGENTS.md: {e}"))
                    })?;
                    format!(
                        "已生成项目说明模板：{}。请补全其中的占位内容。",
                        file.display()
                    )
                }
            }
            SlashAction::Usage => {
                let store =
                    deepagent_persistence::event_store::EventStore::new(
                        self.db,
                    );
                let sessions = store.list_sessions()?;
                let mut items = vec![kv(
                    "会话总数",
                    sessions.len().to_string(),
                )];
                match self.cost {
                    Some(cost) => {
                        let s = cost.summary(session_id)?;
                        items.push(kv(
                            "本会话",
                            format!("{} {:.4}", s.currency, s.session_cost),
                        ));
                        items.push(kv(
                            "今日",
                            format!("{} {:.4}", s.currency, s.today_cost),
                        ));
                        items.push(kv(
                            "本月",
                            format!("{} {:.4}", s.currency, s.month_cost),
                        ));
                        items.push(kv(
                            "累计",
                            format!("{} {:.4}", s.currency, s.total_cost),
                        ));
                    }
                    None => {
                        items.push(
                            SlashPanelItem::new("费用跟踪")
                                .status("muted")
                                .value("未启用"),
                        );
                    }
                }
                SlashPanel::new("用量统计").items(items).to_fenced()
            }
            SlashAction::AddDir { path } => match path {
                Some(path) => match self.projects {
                    Some(projects) => {
                        let trimmed = path.trim();
                        if trimmed.is_empty() {
                            "用法: /add-dir <目录路径>".to_string()
                        } else if !std::path::Path::new(trimmed).is_dir() {
                            format!("目录不存在或不是文件夹：{trimmed}")
                        } else {
                            let dto = projects.add_project(trimmed)?;
                            format!(
                                "已添加工作目录：{} ({})。在侧栏切换项目即可以它为根开始新会话。",
                                dto.name, dto.path
                            )
                        }
                    }
                    None => "当前运行时未启用项目管理。".to_string(),
                },
                None => "用法: /add-dir <目录路径>".to_string(),
            },
        };
        Ok(message)
    }

    /// Run the post-edit verifier across the active workspace's source files,
    /// returning a short summary. Used by the `/verify` slash command.
    pub async fn run_workspace_verification(&self) -> String {
        let root = self.effective_root();
        let dispatcher =
            Arc::new(crate::verification_dispatcher::VerificationDispatcher::standard());

        let mut targets: Vec<PathBuf> = Vec::new();
        collect_verifiable_files(&root, &mut targets, 50);
        if targets.is_empty() {
            return "/verify: no Rust / TS / Python / JSON files found at workspace root."
                .to_string();
        }

        let mut passed = 0usize;
        let mut failed: Vec<String> = Vec::new();
        let mut skipped = 0usize;
        let mut timed_out = 0usize;
        for path in &targets {
            match dispatcher.verify_file(path).await {
                crate::verification_dispatcher::VerificationOutcome::Passed => passed += 1,
                crate::verification_dispatcher::VerificationOutcome::Failed { detail, .. } => {
                    let display = path
                        .strip_prefix(&root)
                        .unwrap_or(path)
                        .display()
                        .to_string();
                    let trimmed: String = detail.lines().take(2).collect::<Vec<_>>().join(" / ");
                    failed.push(format!("{display}: {trimmed}"));
                }
                crate::verification_dispatcher::VerificationOutcome::Skipped { .. } => skipped += 1,
                crate::verification_dispatcher::VerificationOutcome::TimedOut => timed_out += 1,
            }
        }

        let mut lines = Vec::new();
        lines.push(format!(
            "/verify: scanned {n} files in {root}",
            n = targets.len(),
            root = root.display()
        ));
        lines.push(format!(
            "passed: {passed}, failed: {failed_n}, skipped: {skipped}, timed_out: {timed_out}",
            failed_n = failed.len()
        ));
        if !failed.is_empty() {
            lines.push("failures:".into());
            for f in failed {
                lines.push(format!("  - {f}"));
            }
        }
        lines.join("\n")
    }
}

// ---------------------------------------------------------------------------
// Free functions: thin entry points that the chat service delegates to.
// ---------------------------------------------------------------------------

/// Entry point used by [`ChatService::maybe_handle_slash_command`]. Parses the
/// prompt, runs the slash registry, creates or recovers the session, and
/// dispatches the action through [`SlashRuntime::apply_slash_action`].
pub async fn maybe_handle_slash_command<F>(
    rt: &SlashRuntime<'_>,
    prompt: &str,
    continue_session: Option<&str>,
    on_event: &F,
) -> Result<Option<String>>
where
    F: Fn(RuntimeEvent) + Send + 'static,
{
    let registry = SlashRegistry::with_builtins();
    let mut ctx = CommandContext {
        session_id: continue_session.map(str::to_string),
    };
    let Some((name, _)) = parse_slash_invocation(prompt) else {
        return Ok(None);
    };
    if registry.get(name).is_none() {
        return Ok(None);
    }
    let Some(result) = registry.execute_line(prompt, &mut ctx) else {
        return Ok(None);
    };
    let result = result?;

    let root = rt.effective_root();
    let project = root.to_string_lossy().into_owned();
    let clock = SystemClock;
    let target_session = match &result.action {
        SlashAction::Resume { session_id } => Some(session_id.as_str()),
        _ => continue_session,
    };
    let mut session = match target_session {
        Some(id_str) => {
            let id = deepagent_core::id::SessionId::from_str(id_str)
                .map_err(|e| CoreError::invalid(format!("bad session id: {e}")))?;
            Session::recover(rt.db, &clock, id)?
        }
        None => Session::create_in_project(
            rt.db,
            &clock,
            Some(prompt),
            Default::default(),
            Some(&project),
        )?,
    };

    let session_id = session.id().to_string();
    let reply = rt
        .apply_slash_action(&session_id, &mut session, result)
        .await?;
    session.append(EventPayload::MessageAppended {
        message: Message::user(prompt),
    })?;
    let task = session.create_task(prompt)?;
    session.transition_task(task, deepagent_core::task::TaskState::Running)?;

    on_event(RuntimeEvent::RunStarted {
        task_id: task.to_string(),
    });
    on_event(RuntimeEvent::SessionRegistered {
        session_id: session_id.clone(),
        title: session.state().title.clone(),
    });
    on_event(RuntimeEvent::TurnStarted { step: 0 });
    on_event(RuntimeEvent::ContentDelta {
        text: reply.clone(),
    });

    session.append(EventPayload::MessageAppended {
        message: Message::assistant(&reply),
    })?;
    session.transition_task(task, deepagent_core::task::TaskState::Completed)?;
    on_event(RuntimeEvent::RunCompleted { message: reply });

    Ok(Some(session_id))
}

/// Resolve a dynamic (non-builtin) slash command to its rendered body.
/// Consults plugin command roots first, then workspace `.deepagent/commands/`.
pub fn dynamic_command_prompt(rt: &SlashRuntime<'_>, prompt: &str) -> Result<Option<String>> {
    let Some((name, args)) = parse_slash_invocation(prompt) else {
        return Ok(None);
    };
    if SlashRegistry::with_builtins().get(name).is_some() {
        return Ok(None);
    }
    let Some(def) = find_dynamic_command(rt, name)? else {
        return Err(CoreError::invalid(format!(
            "unknown slash command: /{name}"
        )));
    };
    Ok(Some(def.render(args)))
}

/// Look up a non-builtin slash command by name. Plugin-scoped commands
/// (`plugin:command`) take precedence; the fallback is the workspace and
/// effective-root `.deepagent/commands/` directories.
pub fn find_dynamic_command(rt: &SlashRuntime<'_>, name: &str) -> Result<Option<CommandDef>> {
    if let Some((plugin_name, command_name)) = name.split_once(':') {
        if let Some(projection) = rt.plugin_runtime_projection()? {
            for root in projection.command_roots {
                if root.plugin_name != plugin_name {
                    continue;
                }
                let path = root.path.join(format!("{command_name}.md"));
                if !path.exists() {
                    continue;
                }
                let mut def = deepagent_prompts::load_command_file(path)?;
                def.name = name.to_string();
                def.body.push_str(&format!(
                    "\n\nPlugin runtime context:\nDEEPAGENT_PLUGIN_ROOT={}\nDEEPAGENT_PLUGIN_DATA={}\n",
                    root.path.parent().unwrap_or(&root.path).display(),
                    root.data_dir.display()
                ));
                return Ok(Some(def));
            }
        }
    }

    let effective_root = rt.effective_root();
    let mut roots = vec![effective_root, rt.workspace.to_path_buf()];
    roots.dedup();
    for dir in crate::commands::command_dirs(roots) {
        let path = dir.join(format!("{name}.md"));
        if !path.exists() {
            continue;
        }
        return deepagent_prompts::load_command_file(path).map(Some);
    }
    Ok(None)
}

// ---------------------------------------------------------------------------
// Private free functions (slash-command helpers).
// ---------------------------------------------------------------------------

fn web_search_summary(settings: &crate::settings::WebSearchSettings) -> String {
    if !settings.enabled {
        return "disabled".to_string();
    }
    let provider = settings.provider.label();
    match settings.searxng_url.as_deref() {
        Some(url) if !url.trim().is_empty() => {
            format!("{provider} (SearXNG: {url})")
        }
        _ => provider.to_string(),
    }
}

fn parse_slash_invocation(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix('/')?;
    let (name, args) = match rest.find(char::is_whitespace) {
        Some(idx) => (&rest[..idx], rest[idx..].trim()),
        None => (rest, ""),
    };
    if name.is_empty() {
        None
    } else {
        Some((name, args))
    }
}

fn collect_agent_items(
    dir: &std::path::Path,
    plugin_name: Option<&str>,
    items: &mut Vec<SlashPanelItem>,
    count: &mut usize,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Some(def) = deepagent_prompts::AgentDef::parse(&content) {
            *count += 1;
            if items.len() < 30 {
                let label = match plugin_name {
                    Some(plugin) => format!("{plugin}:{}", def.name),
                    None => def.name,
                };
                items.push(SlashPanelItem::new(label).value(truncate_desc(&def.description, 90)));
            }
        }
    }
}

fn parse_thinking_depth(depth: &str) -> Result<ThinkingDepth> {
    match depth.trim().to_ascii_lowercase().as_str() {
        "simple" => Ok(ThinkingDepth::Simple),
        "medium" => Ok(ThinkingDepth::Medium),
        "deep" => Ok(ThinkingDepth::Deep),
        _ => Err(CoreError::invalid("usage: /thinking <simple|medium|deep>")),
    }
}

/// Format a rule count with an optional inline listing (diagnostics helper).
#[allow(dead_code)]
fn format_rule_count(rules: &[String]) -> String {
    if rules.is_empty() {
        "0".to_string()
    } else {
        format!("{} ({})", rules.len(), rules.join(", "))
    }
}

/// Truncate a one-line description to at most `max` characters (on a char
/// boundary), collapsing internal newlines to spaces so it renders cleanly as
/// a single Markdown list item. Appends an ellipsis when truncated.
fn truncate_desc(s: &str, max: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        let head: String = flat.chars().take(max).collect();
        format!("{head}\u{2026}")
    }
}

fn on_off(value: bool) -> &'static str {
    if value {
        "on"
    } else {
        "off"
    }
}

/// Collect up to `cap` verifier-eligible files from `root`, walking one
/// directory level deep. Used by the `/verify` slash command.
fn collect_verifiable_files(root: &std::path::Path, out: &mut Vec<PathBuf>, cap: usize) {
    fn is_eligible(path: &std::path::Path) -> bool {
        matches!(
            path.extension().and_then(|s| s.to_str()),
            Some("rs" | "ts" | "tsx" | "js" | "jsx" | "mts" | "cts" | "py" | "json")
        )
    }
    fn skip_dir(name: &str) -> bool {
        matches!(
            name,
            ".git" | "target" | "node_modules" | "dist" | ".venv" | "__pycache__" | "build" | "out"
        )
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= cap {
            return;
        }
        let path = entry.path();
        if path.is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || skip_dir(&name) {
                continue;
            }
            // One directory level deep — keep the scan bounded.
            if let Ok(children) = std::fs::read_dir(&path) {
                for c in children.flatten() {
                    if out.len() >= cap {
                        return;
                    }
                    let cp = c.path();
                    if cp.is_file() && is_eligible(&cp) {
                        out.push(cp);
                    }
                }
            }
        } else if is_eligible(&path) {
            out.push(path);
        }
    }
}
