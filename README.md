# DeepAgent Studio

一个 **DeepSeek 原生的 Agent 运行时平台**，并在其上构建了桌面 IDE —— 不只是一个AI 聊天应用。核心是一个可验证、可回放的运行时内核（上下文工程、思考模式持久化、子代理编排、自愈验证、安全/权限、MCP），并通过 Tauri + React 桌面应用对外呈现。

> 路线图与 crate 清单：[`开发计划.md`](./开发计划.md)。设计理念：
> [`开发提示词.md`](./开发提示词.md)。架构总览：[`ARCHITECTURE.md`](./ARCHITECTURE.md)。
> 与 Claude Code 的差距分析：[`Claude_Code_差距与开发文档.md`](./Claude_Code_差距与开发文档.md)。

## 当前状态

**内核已完成，桌面平台已连通。** 整个工作区由 **22 个内核 crate + 一个无头 CLI +
一个 Tauri 桌面应用**组成，构建干净、Lint 干净（`clippy -D warnings`），并由
**565 个通过的测试**覆盖（默认离线运行）。

Phase A（让平台真正可用）和 Phase B（声明式配置面）已全部完成；Phase C
（平台化与可观测）进行中。

### 已实现的能力

**运行时内核**

- Cargo 工作区，22 个内核 crate，以 SQLite 上的**仅追加事件存储**（带版本化迁移）
  作为全系统的唯一真相来源。
- **会话管理器**：纯从事件折叠出的 replay + 崩溃恢复，并支持 **fork / rewind /
  export**（Markdown · JSON transcript 导出）。
- **Agent 运行时循环**（THINK → EXECUTE → OBSERVE）驱动可插拔的模型代理，带流式
  `RuntimeEvent` 事件通道。
- **上下文工程**：五层上下文流水线、Token 预算、结构化压缩、工作区扫描。
- **多层记忆** + Anthropic 风格的 8 阶段情境化检索（跨会话持久化）。
- **DeepSeek 原生模型层**：SSE 流式、tool_calls 合并、思考模式 reasoning 持久化。
- **工具与安全**：能力注册表 + 权限/风险模型，内置工具
  （read/write/edit/multi_edit/glob/grep/bash/todo/task_list/web_fetch/web_search），
  WASM（Wasmtime）工具沙箱，以及 `SandboxedTool`。
- **Hook 与权限**：9 点 Hook 生命周期、声明式权限规则（`allow/ask/deny`）、声明式
  外部 Hook（`hooks.json`）、带三档策略的人工审批门控。
- **子代理**：DAG 调度器 + git worktree 隔离。
- **自愈验证**：验证器套件 + 反思 + 循环检测。
- **MCP**：`.mcp.json` 配置、JSON-RPC 客户端、stdio + HTTP/SSE 传输、命名空间化的
  远程工具、可视化配置 + 实时工具注册。

**桌面应用（Tauri v2 + React）**

- 登录/引导流程：连接时**校验 API Key**（无效则拒绝放行；Key 存入操作系统钥匙串，
  绝不落盘），并支持退出登录。
- 多项目侧边栏（项目 = 文件夹 → 其下挂会话）；原生选择文件夹对话框；新建会话自动
  归属当前活跃项目。
- 流式聊天 + token 实时渲染 + 审批对话框；会话 fork/rewind/export 可从聊天菜单触发。
- 设置：模型发现、审批策略、权限规则、`hooks.json` 编辑器、MCP 服务器可视化管理。
- 可用的 Terminal 与 SideChat 面板；自定义无边框标题栏。
- 40 个 Tauri 命令将 React UI 桥接到 `deepagent-app-core`。

## 环境要求

- Rust（stable，1.80+）；仓库通过 `rust-toolchain.toml` 锁定工具链。
- Node.js + [pnpm](https://pnpm.io/)（用于桌面 UI）。
- 内置 SQLite（无需系统安装）。
- 一个 DeepSeek API Key 才能进行真实对话（在应用登录界面输入；存入操作系统钥匙串）。

## 快速开始

### 内核（无头）

```bash
# 运行完整测试套件（离线）
cargo test --workspace --offline

# 运行端到端内核演示：打开数据库，跑一个脚本化代理执行一次工具调用，
# 然后纯从事件日志恢复会话。
cargo run -p deepagent-cli
```

### 桌面应用

```bash
cd apps/desktop
pnpm install
pnpm tauri dev      # 编译 Rust 外壳 + 打开桌面窗口
```

> 原生选择文件夹对话框、API Key 校验等桌面特性，只在 Tauri 窗口
> （`pnpm tauri dev`）中生效，纯 `pnpm dev` 的浏览器预览下不可用。

## 目录结构

```text
crates/
  deepagent-core         领域原语（ids、events、tasks、messages、clock）
  deepagent-tracing      tracing + 进程内 metrics
  deepagent-persistence  sqlite、迁移、仅追加事件存储、文档存储
  deepagent-session      会话管理器（replay + 恢复 + fork/rewind）
  deepagent-context      提示词编译 AST + Token 预算 + 上下文流水线
  deepagent-memory       多层记忆 + 情境化检索
  deepagent-tools        工具 trait、能力注册表、权限、WASM 沙箱
  deepagent-builtins     内置工具集（文件/bash/todo/web）+ 安全守卫
  deepagent-intent       输入调度（slash/附件/UserPromptSubmit）
  deepagent-skills       SKILL.md 发现、渐进披露、安装
  deepagent-prompts      frontmatter 命令/代理 + 系统提示组装
  deepagent-runtime      Agent 运行时循环 + 流式事件 + 审批
  deepagent-models       DeepSeek 客户端（SSE、思考模式、模型发现）
  deepagent-hooks        Hook 生命周期 + 权限规则 + 外部 hooks.json
  deepagent-workspace    工作区扫描器 → 快照
  deepagent-verification 自愈式 验证/反思 循环
  deepagent-planner      计划 DAG + 规划策略
  deepagent-subagents    DAG 调度器 + worktree 隔离
  deepagent-observation  Agent 时间线 + 会话统计 + transcript 导出
  deepagent-security     密钥扫描器 + CI 门禁
  deepagent-app-core     应用服务门面（DTO + 面向任意 UI 的服务）
  deepagent-mcp          Model Context Protocol 客户端 + 传输 + 适配器
apps/
  cli                    无头端到端演示驱动
  desktop                Tauri v2 + React 桌面应用
```

## 许可证

Apache-2.0

### 作者
程序员Eighteen

### 联系
Gmail：eighteenthstuai@gmail.com
