# 仓库规范

## 最高优先级铁律

1. 先查证，再动手。任何 API、库、框架、系统行为，都必须先找到依据。
2. 禁止臆造。不要凭记忆写 API 签名、配置项、命令参数、版本行为。
3. 要根因修复，不要补丁。重复出现的问题，优先重审设计。
4. 完成以验证为准。没实际跑过编译、测试、lint 或手动验证，不算完成。
5. 忠实执行用户意图。不擅自扩范围，不顺手改无关内容。

## 项目结构

这是一个 Rust monorepo。运行时核心 crate 在 `crates/`，无头 CLI 在 `apps/cli/`，Tauri 桌面应用在 `apps/desktop/`（`src-tauri/` 为 Rust 后端，`src/` 为 React/TypeScript 前端）。`借鉴/` 目录是参考源，不是可随意照抄的代码库。`target/`、`dist/`、`node_modules/` 等生成物不要提交。

## 证据优先级

动手前先按顺序核对：

1. 当前代码库已有同类实现
2. `借鉴/claudecode`
3. `借鉴/codex/codex-rs`
4. `借鉴/grok-build`
5. `借鉴/open-code-review`
6. `借鉴/better-harness`
7. `借鉴/deepseek-harness`（DeepSeek 官方 harness，优先用于插件化、运行时编排、会话/工作流设计）
8. Rust / Tauri / DeepSeek / OpenAI / Microsoft 官方文档

如果都没有覆盖，就明确标注为“本地设计”，并写明原因。

## 开发规范

- 优先在现有架构内做最小、完整的修复。
- 没有必要不要新增 command、DTO、UI、协议或额外抽象。
- 修改跨层契约时，必须同步更新所有消费方，包括前端类型、command、事件和测试。
- Rust 代码按 `rustfmt.toml` 统一格式（`max_width = 100`）。
- Rust 命名用 `snake_case`，React 组件用 `CamelCase`。
- 不要提交密钥、API Key、机器专属配置，也不要提交应脱敏的日志或提示词内容。

## 构建、测试与运行

- `cargo fmt --all -- --check`：检查 Rust 格式。
- `cargo clippy --all-targets --all-features -- -D warnings`：检查 Rust lint。
- `cargo test --workspace --offline`：运行 Rust 测试。
- `cargo run -p deepagent-cli`：运行无头演示。
- `cd apps/desktop && pnpm install`：安装桌面依赖。
- `cd apps/desktop && pnpm build`：前端类型检查并打包。
- `cd apps/desktop && pnpm tauri dev`：运行完整桌面应用。

## 测试要求

优先写真实回归用例，不要只造理想化样本。单元测试一般跟着模块放，集成测试放在 `crates/*/tests/`。涉及 Tauri 桥接时，至少跑 `pnpm build`；如果影响界面行为，再补 `pnpm tauri dev` 验证。

## 修 Bug 纪律

- 先复现，再修复。优先用真实失败输入、日志或堆栈定位。
- 找根因，不修表象。不要只加特判绕过去。
- 同类问题反复出现，说明是架构问题，要升级方案。
- 修完必须补回归测试，并跑受影响模块的既有测试。

## Git 与 PR 规范

最近提交常用简短中文前缀，如 `【fix】`、`【refactor】`、`【test】`。提交信息要聚焦单一变更，并写清影响的子系统。PR 里要说明改了什么、怎么验证的；如果改到 UI，最好附截图或录屏。

## 安全与配置

把运行日志、本地缓存、凭据都当作敏感信息处理。凡是涉及模型接入、权限、持久化或恢复逻辑的改动，都要确认仍然会脱敏，并且 CI 相关命令保持通过。
