# 插件、MCP 与按需工具披露

## 结论与查证依据

安装并启用插件后，宿主可以连接 MCP、获取 `tools/list` 并注册工具。这些操作发生在主机内，不代表模型收到全部工具定义。

需要工具时，模型应先搜索，获得命中工具的完整定义，再调用工具。用户无需每次显式点名插件；插件的 Skill 摘要、来源信息和任务需求可以帮助模型决定搜索什么。不能先调用未知工具，再补参数 schema。

查证日期：2026-10-13。本地 Codex 参考实现：

- `借鉴/codex/codex-rs/core/src/tools/handlers/tool_search_spec.rs`：搜索入口按来源去重展示目录，搜索命中工具后提供下一次模型调用所需定义。
- `借鉴/codex/codex-rs/core/src/tools/handlers/mcp.rs`：MCP 搜索信息包含服务/连接器来源，以及可选的来源描述。
- `借鉴/codex/codex-rs/core/src/mcp_tool_exposure.rs`：搜索开启时 MCP 工具为 Deferred；未开启时为 Direct。不能概括为 Codex 在所有模式下都延迟所有工具。
- `借鉴/codex/codex-rs/core/src/plugins/render.rs`：显式选中插件会提示优先搜索相关能力，插件不是必须先调用的工具入口。
- `借鉴/codex/codex-rs/ext/skills/src/render.rs`：Skill 目录展示名称、摘要与路径，按上下文预算截断摘要或省略条目；并非全量 Skill 正文。

Codex 的 `spec_plan.rs::search_tool_enabled()` 还检查模型支持搜索与 provider 支持 namespace。上面的延迟行为有能力与配置前提，不能直接作为所有模型的协议假设。

[OpenAI Tool search 官方说明](https://developers.openai.com/api/docs/guides/tools-tool-search)建议按 namespace/MCP server 分组，初始展示来源名称和描述，内部函数定义按需加载；单独延迟函数仍会展示函数名称和描述。[插件打包说明](https://developers.openai.com/plugins/build/plugins)说明插件组织 Skill、MCP 等资源。

## 本系统的链路

```text
插件启用
  → 宿主连接 MCP / tools/list / 现有 ToolRegistry
  → 模型看到常用工具、tool_search、未发现工具的来源目录
  → 模型根据任务搜索相关工具
  → 主机搜索名称与描述，更新当前会话的 discovered set
  → 同一任务下一次模型请求加入命中工具的完整 schema
  → 现有工具调用、参数校验、权限、审批和取消链路
```

源码中的 `Tool::discovery_source()` 提供来源标识；MCP adapter 使用 `RemoteTool.server`，避免按 `__` 拆分导致 `plugin__rea__rea` 被错误合并。没有来源元数据的延迟工具归入 `Other deferred tools`。来源目录不广播内部工具名称、描述或参数。

模型工具列表通过 `ModelAgent::with_tool_schema_source()` 在每次模型推理前刷新；主运行和子运行复用 `build_visible_tool_schemas()`。子运行每次刷新继续应用 agent profile 与工具 allowlist，发现集合独立于父运行。工具搜索仅影响披露，不授予执行权限。

配置保持原有三态语义：

| 模式 | 行为 |
| --- | --- |
| Enabled | 可延迟工具经搜索后提供完整定义 |
| Auto（默认） | 可延迟定义总长度达到配置阈值后使用搜索；低于阈值直接提供工具 |
| Disabled | 直接提供可见工具的完整定义 |

本轮同时修复 Auto 低于阈值时未注册搜索入口、却仍隐藏工具的问题。

## REA 与边界

REA 的 170 个工具仍注册在主机同一个工具注册表里。搜索激活时，初始来源目录只增加 `plugin__rea__rea` 一项，完整 schema 只披露搜索命中工具。插件 Skill 摘要及 MCP 初始化 instructions 仍通过已有上下文机制提供；这不是零 token 的插件。

本系统使用普通函数形式的 `tool_search`，由宿主刷新后续请求的工具列表，适用于已有模型适配链。未向 DeepSeek 请求添加 OpenAI Responses 专有的 `defer_loading`、`tool_search_output` 字段，也未实现 OpenAI 的尾部定义注入/缓存机制。

MCP 服务进程仍按既有连接生命周期启动，本轮没有实现服务进程的按需启动。已发现工具保留在当前会话集合内，不会因本条消息没有使用而自动卸载；禁用插件后新的运行以当前注册表为准。减少目录披露可以降低上下文成本，但不能保证消除幻觉。

## 验证

回归覆盖：170 个工具合并成一个来源、目录大小不随该来源工具数增长、真实 `ToolSearchTool` 搜索只加载命中定义、跨会话隔离、父子搜索通道重绑定、权限过滤、Auto 小目录可达性，以及模型下一次请求实际携带新定义。

测试使用本地工具目录和记录模型请求的 transport，不调用真实模型。REA ZIP 和运行包未发生变化。桌面 UI 与真实模型自动选择工具尚未在本轮验证。

实际执行结果：

- `cargo test --workspace --offline`：2847 passed，0 failed，30 ignored。
- `cargo clippy -p deepagent-app-core -p deepagent-runtime -p deepagent-mcp -p deepagent-builtins -p deepagent-tools --all-targets --offline -- -D warnings`：通过（默认 features）。
- 本轮 Rust 文件 `rustfmt --check --config skip_children=true` 与 `git diff --check`：通过。
- `cargo fmt --all -- --check`：未通过，唯一差异是本轮未修改的 `crates/deepagent-runtime/src/kernel.rs:384` 既有长类型格式。

未执行全 features clippy、桌面构建/手动 UI 验证及真实模型联调；本轮没有前端或 Tauri 契约变更，也没有重建桌面发布程序。运行中的旧程序需重新构建才能应用这些宿主改动。
