# AppCore 服务

## 8. `deepagent-app-core` 服务门面

`deepagent-app-core` 是 UI 与内核之间的稳定边界。前端和 Tauri 不应直接操作内核 crate 的内部类型，而应经由 DTO 和服务方法。

| 服务 | 文件 | 职责 |
| --- | --- | --- |
| `AppService` | `service.rs` | 会话列表、详情、conversation 重建、fork、rewind、transcript 导出、diff |
| `ChatService` | `chat_service.rs` | 组装 Agent run：模型、工具、MCP、知识、技能、项目根、审批、成本、验证 |
| `SettingsService` | `settings.rs` | API Key 校验、OS keychain、模型发现、thinking depth、审批策略、沙箱、权限、hooks、web search |
| `SkillsService` | `skills_service.rs` | 技能加载、列出、安装、卸载、激活、AI 安全审查 |
| `McpService` | `mcp_service.rs` | MCP server 配置持久化、启停、连接 enabled server、注册远程工具 |
| `KnowledgeService` | `knowledge_service.rs` | 知识库加载、搜索、保存、删除、被动注入、自动捕获、草稿 |
| `CostService` | `cost_service.rs` | token 成本记录、预算配置、预算检查、汇总 |
| `ProjectService` | `project_service.rs` | 多项目注册、active project、置顶、重命名、移除 |
| `WorkspaceService` | `workspace_service.rs` | 当前工作区信息 |
| `ProjectMapService` | `project_map_service.rs` | 读取/生成 `.understand-anything/knowledge-graph.json`，搜索/邻居/影响分析 |
| `GitService` | `git_service.rs` | Git status、branch、changes、diff、log、stage、unstage、commit、worktree |
| `ArchiveService` | `archive_service.rs` | 会话归档、项目归档、恢复、删除 |
| `SessionStateService` | `session_state_service.rs` | 会话置顶等轻量状态 |
| `TerminalService` | `terminal_service.rs` | 在 active project 中执行非危险命令 |
| `FilePreviewService` | `file_preview_service.rs` | 文件元数据、文本提取、data URL、PDF 渲染 |
| `RecordingService` | `recording_service.rs` | 音频输入设备、录制生命周期 |
| `RuntimeService` | `runtime_service.rs` | 管理外部/内置运行时资源的下载、安装、取消、卸载 |
| `SpeechService` | `speech_service.rs` | 语音转写、会议纪要 |
| `OfficeService` | `office_service.rs` | Office 文档读取、Markdown -> docx、会议纪要导出 |
| `Doctor` | `doctor.rs` | 环境诊断 |
| `ApprovalBridge` | `approval_bridge.rs` | UI 审批队列、策略门、channel gate |
| `VerificationDispatcher` | `verification_dispatcher.rs` | 按文件类型分发验证器 |
| `VerificationDecorator` | `verification_decorator.rs` | 工具结果验证装饰 |
| `Reminder` 系列 | `plan_mode_reminder.rs`, `skill_catalog_reminder.rs`, `todo_snapshot_reminder.rs`, `system_reminder.rs` | 在运行中给模型追加系统提醒 |

