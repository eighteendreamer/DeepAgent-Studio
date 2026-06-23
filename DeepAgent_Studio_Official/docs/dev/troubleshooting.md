# 排错与维护

## 37. 排错指南

| 现象 | 优先检查 |
| --- | --- |
| 桌面端无法连接 API Key | 是否在 `pnpm tauri dev` 中运行；DeepSeek key 是否有效；`SettingsService::validate_api_key` 错误 |
| 浏览器预览功能缺失 | `pnpm dev` 没有 Tauri internals，原生功能只能在 Tauri 窗口中使用 |
| 聊天无响应 | `run_chat` 是否报错；API key 是否存在；预算是否超限；模型发现是否为空 |
| 工具被拒绝 | 审批策略、sandbox mode、permission rules、hooks、敏感路径检查 |
| MCP 工具不出现 | server 是否 enabled；transport 配置是否 valid；`${ENV}` 是否可展开；连接失败会被跳过 |
| 技能安装后模型不知道 | 是否调用了安装/重载命令；`ChatService::reset_all_sent_skills` 是否触发；技能目录功能是否启用 |
| 历史会话工具卡丢失 | 检查事件流中是否有 `ToolCallRequested/Completed`；`session_conversation` 重建逻辑 |
| Git 面板为空 | 项目路径是否 git repo；`git` 是否可用；非 repo 返回空而非错误 |
| 项目地图为空 | 是否存在 `.understand-anything/knowledge-graph.json`；是否执行 refresh_deep；源码是否被过滤 |
| 文件预览失败 | 文件类型是否支持；pdfium/Office runtime 是否安装；路径是否可读 |
| 录音失败 | 输入设备权限；`audio` feature；系统麦克风占用 |
| 语音转写失败 | whisper 模型/引擎是否安装；wav path 是否存在 |

## 38. 维护约定

- 保持 `deepagent-app-core` 是 UI 合同层；不要让 React 直接理解内核内部结构。
- 所有新增 Tauri 命令都应同步添加到 `api.ts`、`types.ts` 和本指南命令表。
- 任何影响历史 replay 的事件变更，都要补充 migration 或兼容重建逻辑。
- 高风险能力必须先明确权限等级和审批路径，再接入 UI。
- 文档、Office、语音、运行时下载等功能要考虑离线/缺运行时降级路径。
- 对外部工具调用，失败应尽可能返回结构化结果，让 UI 可展示，而不是 panic。
- 重要路径使用 UTF-8 文档；PowerShell 查看中文时请使用 `Get-Content -Encoding UTF8`。

## 40. 快速定位表

| 想改的功能 | 首先看 |
| --- | --- |
| 聊天流式事件 | `apps/desktop/src/App.tsx`, `apps/desktop/src/api.ts`, `crates/deepagent-app-core/src/chat_service.rs`, `crates/deepagent-runtime/src/events.rs` |
| 工具执行 | `crates/deepagent-runtime/src/loop_engine.rs`, `crates/deepagent-tools`, `crates/deepagent-builtins` |
| 会话列表/历史 | `crates/deepagent-app-core/src/service.rs`, `crates/deepagent-observation`, `crates/deepagent-session` |
| 设置/API Key | `crates/deepagent-app-core/src/settings.rs`, `secret_store.rs`, `apps/desktop/src/components/OnboardingWizard.tsx` |
| 技能 | `crates/deepagent-skills`, `crates/deepagent-app-core/src/skills_service.rs`, `apps/desktop/src/components/SkillsView.tsx` |
| MCP | `crates/deepagent-mcp`, `crates/deepagent-app-core/src/mcp_service.rs`, `MCPSettings.tsx` |
| 知识库 | `crates/deepagent-knowledge`, `knowledge_service.rs`, `KnowledgeView.tsx` |
| 项目地图 | `crates/deepagent-codegraph`, `project_map_service.rs`, `ProjectMapPanel.tsx` |
| Git UI | `git_service.rs`, `components/git/*`, `hooks/useGitStatus.ts` |
| Office/文件预览 | `office_service.rs`, `file_preview_service.rs`, `FilePreviewPlugin.tsx` |
| 录音/语音 | `recording_service.rs`, `speech_service.rs`, `RecordingPlugin.tsx` |
| 发布 | `.github/workflows/release-desktop.yml`, `apps/desktop/src-tauri/tauri.conf.json` |

