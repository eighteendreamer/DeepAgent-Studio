## Overview

本计划把 design 落成可增量执行的编码任务。顺序原则：**先纯新增（零回归风险），再纯搬移（独立提交），最后行为变更**。每步保持 `cargo test --workspace --offline`、`cargo fmt --all -- --check`、`cargo clippy --all-targets --all-features -- -D warnings` 全绿；涉及跨层契约的步骤额外要求 `cd apps/desktop && pnpm build` 通过。

最大杠杆是复用：hooks 已完整支持 Claude Code 格式（`deepagent-hooks` 的 `HookEvent` + `HookDefinitions::parse` + `matcher_matches`），marketplace materialize（git clone / npm pack / HTTP + sha256）、`state.json` 原子写与 schema 迁移、scan 风险闸门、origin 优先级与孤儿标记全部保留不动。真正新增的只有：Agent Plugins v1 的规范层、方言归一层、v1 合规的占位符与组件发现。

关键风险点集中在两处，任务设计上刻意隔离：任务 6（纯搬移，禁止夹带行为改动）与任务 12–14（`plugin_runtime` 的行为变更，逐条对照规范）。

## Tasks

- [x] 1. 契约盘点（M0-1）
  - 盘 `deepagent-hooks` 的 `HookPoint`（27 个生命周期点）与 `HookEvent`（23 个 Claude Code 命名事件）、`HookEvent::parse`/`point()`/`uses_matcher()` 映射、`HookDefinitions::parse` 对 Claude `hooks.json` 的支持、`matcher_matches` 的工具名别名与通配。
  - 盘 `deepagent-mcp::config` 的 `expand` 语义（单次非递归 ✓ 符合规范；未知变量展开空串 ✗、作用于 `command`/`url`/`headers` ✗ 违背规范）与 `TransportType`（`Stdio`/`Sse`/`Http`/`Ws`）。
  - 盘 `plugin_runtime.rs` 现有投影链路与环境注入（已有 `DEEPAGENT_PLUGIN_*` 与 `CLAUDE_PLUGIN_*`，缺规范要求的无前缀 `PLUGIN_ROOT`/`PLUGIN_DATA`）。
  - 结论写入 requirements 的"契约盘点结论"与 design 的对应章节。
  - _Requirements: 7.2, 5.10, 5.12_

- [x] 2. 建立 `plugin/` 模块骨架与规范层常量（M0-3 前置）
  - 新建 `crates/deepagent-app-core/src/plugin/mod.rs` 与 `plugin/spec/mod.rs`，在 `lib.rs` 声明 `mod plugin;`，不移动任何既有文件。
  - 在 `plugin/spec/schema.rs` 定义 `AGENT_PLUGIN_SCHEMA_URI`、`AGENT_PLUGIN_MCP_SCHEMA_URI`、`AGENT_PLUGIN_SCHEMA_PREFIX`、`AGENT_PLUGIN_MANIFEST_RELATIVE_PATH`、`DISCOVERABLE_MANIFEST_PATHS`（四方言顺序）。
  - 空骨架 `cargo build --offline` 通过。
  - _Requirements: 1.1, 3.1, 10.1_

- [x] 3. 实现 `$schema` 三态判定（M0-3）
  - 在 `plugin/spec/schema.rs` 实现 `SchemaStatus { Supported, Unsupported, Unrelated }` 与 `schema_status(contents: &str) -> SchemaStatus`：解析 JSON 取 `$schema`，等于 canonical → Supported，以规范前缀开头 → Unsupported，其余含缺失/非法 JSON → Unrelated。
  - 实现 `mcp_schema_status`，并提供"与 plugin.json 版本一致性"校验的比较函数。
  - 单元测试：canonical 命中、同前缀异版本 → Unsupported、无关 `$schema` → Unrelated、无 `$schema` → Unrelated、非法 JSON → Unrelated。
  - 断言判定过程无任何网络访问（纯字符串比较，不引入 http 依赖）。
  - _Requirements: 1.2, 1.3, 1.4, 4.5_

- [x] 4. 实现 `PluginName` newtype 与 §5.5 约束（M0-2）
  - 在 `plugin/spec/name.rs` 实现 `PluginName(String)`，`parse()` 构造即校验：长度 1–64；字符集 `[a-z0-9.-]`；首尾为字母或数字；禁连续 `--` 与 `..`。
  - 实现 `PluginNameError` 区分具体违规项（`Empty` / `TooLong` / `InvalidChar` / `BoundaryNotAlphanumeric` / `ConsecutiveHyphen` / `ConsecutivePeriod`），错误信息指明是哪项约束。
  - 单元测试：规范给出的合法样例（`my-plugin`、`acme.tools`、`lint3r`、`a`）全部通过；非法样例（`My-Plugin`、`-start`、`has--double`、`too.many..dots`、空串、65 字符）各自返回对应错误变体。
  - 边界测试：64 字符合法、65 字符非法；`a.b-c` 合法；`.start`/`end.` 非法。
  - _Requirements: 2.1, 2.2, 2.3, 2.4_

- [x] 5. 实现插件相对路径解析与 containment（§4.1）
  - 在 `plugin/spec/path.rs` 实现 `resolve_plugin_relative(root, raw) -> Result<PathBuf, PathError>`：要求以 `./` 开头；拒绝 `..` 组件；拒绝绝对路径与 Windows 盘符前缀；解析后必须仍在 root 内。
  - 实现 `contains(root, candidate) -> bool` 供组件发现层复用。
  - 单元测试：规范 §4.1 的合法样例（`./bin/server`、`./data`）与非法样例（`../bin/server`、裸 `data`、`/abs`、`C:\x`、`./`）；符号链接指向根外时被拒。
  - _Requirements: 6.1, 6.2_

- [x] 6. 实现 v1 占位符展开（M1-6）
  - 在 `plugin/spec/placeholder.rs` 实现 `expand_v1(input, root, data) -> String`：只识别 `${PLUGIN_ROOT}` 与 `${PLUGIN_DATA}`；单次非递归（替换产生的文本不再扫描）；未识别的占位符样式文本保持字面量。
  - 实现 `rewrite_dialect_aliases(input) -> String`：`${CLAUDE_PLUGIN_ROOT}`/`${CODEX_PLUGIN_ROOT}` → `${PLUGIN_ROOT}`，DATA 变体同理。
  - 实现 `has_reserved_env_key(env) -> bool` 判定 `env` 键含 `PLUGIN_ROOT`/`PLUGIN_DATA`。
  - 单元测试对照 design 的行为差异表：`${UNKNOWN}` 保持字面量（与现有 `expand` 展开空串相反）；展开结果内含占位符字面量不被二次展开；别名重写后走统一语义。
  - 明确不修改 `deepagent-mcp::config::expand`，并加注释说明原因（用户配置依赖其现有语义）。
  - _Requirements: 5.3, 5.4, 5.5, 5.6, 5.11, 5.10_

- [x] 7. 实现 `PluginDiagnostic` 并贯通到前端（M0-4）
  - 在 `plugin/model.rs` 定义 `PluginDiagnostic` 全部变体、`SkipReason`、`ComponentKind`、`DiagnosticSeverity`。
  - 在 `dto.rs` 为 `PluginDto` 新增 `diagnostics`、`dialect`、`data_dir` 字段（追加式，不改既有字段）。
  - 同步 `apps/desktop/src/types.ts` 的 `Plugin` 类型与 `api.ts` 无需改动的确认；在 `PluginsViewReal.tsx` 详情页新增"来源"方言展示与"诊断"区块（复用 `shadcn/` 组件）。
  - 验证：`cargo test --workspace --offline` 与 `cd apps/desktop && pnpm build` 均通过。
  - _Requirements: 6.10, 6.11, 1.8, 8.5, 8.6_

- [x] 8. 许可与 NOTICE 机制（M0-5）
  - 在 `THIRD_PARTY_NOTICES.md` 新增"内置插件"章节模板，记录来源仓库、版本、许可、获取时间。
  - 新增校验脚本（`crates/deepagent-app-core/scripts/` 或 `apps/desktop/scripts/`，与既有脚本位置一致）：内置插件目录下每个插件必须存在 LICENSE 文件，缺失则非零退出。
  - 在 `.github/workflows/ci.yml` 接入该校验。
  - 文档写明：`anthropics/claude-code` 的插件不得复制进仓库（保留所有权利 + 商业条款）。
  - _Requirements: 9.1, 9.2, 9.3_

- [ ] 9. 纯搬移重构：7 个平铺文件收进 `plugin/`（M1-1，独立提交）
  - 把 `plugin_manifest.rs`、`plugin_loader.rs`、`plugin_marketplace.rs`、`plugin_runtime.rs`、`plugin_security.rs`、`plugin_dependency.rs`、`plugin_service.rs` 移入 `plugin/` 目录树，`plugin/mod.rs` 逐项 re-export 以保持 `lib.rs` 公共 API 不变。
  - 把 `plugin_service.rs`（4000+ 行）中的 `state.json` 读写、schema 迁移、缓存快照抽到 `plugin/state.rs`；marketplace 相关抽到 `plugin/marketplace/`。
  - **禁止夹带任何行为改动**：不改函数体逻辑、不改签名、不改测试。
  - 验证：`cargo test --workspace --offline` 结果与搬移前逐项一致；`fmt` 与 `clippy` 干净。
  - _Requirements: 10.1, 10.2, 10.3, 10.4_

- [x] 10. 实现 Agent Plugins v1 closed schema 解析（M1-2）
  - 在 `plugin/spec/v1.rs` 实现 `PortableManifest` 与 `parse_portable(contents) -> Result<(PortableManifest, Vec<PluginDiagnostic>)>`。
  - closed schema 语义：仅允许规范 §5.2 的 10 个顶层字段；未知顶层字段 → 记 `UnknownManifestField` 诊断并继续；`extensions` 非对象 → 记 `ExtensionsNotObject` 并忽略该字段后继续；其余违规 → `Err` 拒绝插件。
  - `author` 只允许 `name`/`email`/`url` 且值为字符串，否则 manifest 非法。
  - 元数据宽松校验：不因 `version` 非 SemVer、URL/邮箱/SPDX 格式不合而拒绝。
  - 单元测试：规范 §5.2 的最小 manifest 与完整 manifest；未知字段非致命；`extensions` 为数组时非致命；`author` 含额外字段致命；`version` 为 `"not-semver"` 仍接受。
  - _Requirements: 1.1, 1.5, 1.6, 1.7, 6.7, 6.8, 6.9, 6.12_

- [x] 11. 实现 skills 非递归发现（M1-3）
  - 在 `plugin/component/skills.rs` 实现 `discover_skills(root) -> (Vec<SkillComponent>, Vec<PluginDiagnostic>)`：只枚举 `skills/` 直接子目录，其中 `SKILL.md` 解析为常规文件才算一个 skill。
  - 缺失 `skills/` 不算错误；`skills` 存在但不是目录 → 该组件类型无效并记诊断；单个 skill 越界或非法 → 记 `SkillSkipped` 并继续。
  - 回归测试：`skills/a/SKILL.md` 被发现；`skills/a/nested/SKILL.md` **不**被发现；`skills` 是文件时记诊断且其他组件仍加载。
  - 更正（实施时查明）：现有 `count_skill_root` **并不递归**，它的语义是「`path/SKILL.md` 存在则 path 自身算一个 skill，否则枚举直接子项」。与 v1 的唯一差异是前半句，而那正是 Codex 方言需要保留的写法（`"skills": "./my-skill"`），因此归方言层而非当作缺陷修掉。真实风险点改为**文件名大小写**：Windows/macOS 大小写不敏感，`join("SKILL.md").is_file()` 会误收 `skill.md`，§7.1 要求 named exactly，故比对目录项真实名字。
  - _Requirements: 4.1, 4.2, 4.3, 6.4, 6.5_

- [x] 12. 实现 v1 `mcp.json` 解析与 per-server 隔离（M1-4）
  - 在 `plugin/component/mcp.rs` 实现顶层校验（仅 `$schema` + `mcpServers`）、版本一致性校验、三 transport 闭合联合解析。
  - `stdio`：`command` 必需且为单 token（裸名或 `./` 相对路径，不做占位符展开）；`args`/`env`/`cwd` 可选。`streamable-http`/`sse`：`url` 必需（绝对 HTTP/HTTPS、无用户信息与片段、非回环必须 HTTPS）、`headers` 可选（同名不同大小写重复 → 无效）。
  - `cwd` 三种合法形式校验 + 展开后 containment；`env` 键含保留名 → 该 server 无效。
  - 未知字段 / 未知 `type` / 跨变体字段 → 该 server 无效，记 `McpServerSkipped` 后继续。
  - transport 映射到 `deepagent_mcp::config::TransportType`（`stdio`→`Stdio`、`streamable-http`→`Http`、`sse`→`Sse`；`Ws` 不可由插件声明）。
  - 单元测试：规范 §7.2 的 `mcp.json` 完整示例全过；一个非法 server 不影响同文件其他 server；顶层多字段 → 整体 `McpDisabled` 且 skills 仍加载；`$schema` 版本不匹配 → 整体禁用。
  - _Requirements: 4.4, 4.5, 4.6, 4.7, 4.8, 4.9, 5.6, 5.9, 6.6_

- [x] 13. 实现子进程环境组装与 `PLUGIN_DATA` 契约（M1-5）
  - 在 `plugin/runtime.rs` 按规范 §9.1 顺序组装：基础环境 → 配置 `env`（已 v1 展开）→ 最后写入 `PLUGIN_ROOT`/`PLUGIN_DATA` 覆盖同名项。
  - 保留既有 `DEEPAGENT_PLUGIN_ID`/`DEEPAGENT_PLUGIN_ROOT`/`DEEPAGENT_PLUGIN_DATA`/`CLAUDE_PLUGIN_ROOT`/`CLAUDE_PLUGIN_DATA`（兼容内置插件）。
  - 确保插件来源的 `McpServerConfig` 不再经过 `expand_with` 二次展开（加测试守住）。
  - `cwd` 省略时以插件根为工作目录。
  - 测试：`PLUGIN_ROOT`/`PLUGIN_DATA` 存在且为绝对路径；`env` 配置项被规范变量覆盖；`PLUGIN_DATA` 目录在启动前被创建；插件更新（`install_from_dir` 覆盖）后 data 目录内容仍存在。
  - _Requirements: 5.1, 5.2, 5.7, 5.8, 5.12, 6.6_

- [x] 14. hooks 占位符改走 v1 语义并记录未映射事件（M3 前置，本 spec 内完成）
  - `expand_hook_commands` 改用 `expand_v1` + 别名重写，替换现有 `expand_plugin_vars`（后者会把未知变量吃成空串）。
  - `HookEvent::parse` 返回 `None` 的事件名记 `HookEventUnmapped` 诊断，不静默丢。
  - 不新建 hooks 映射层，继续复用 `HookDefinitions` 与 `HookEvent`。
  - 测试：Claude `hooks.json` 中 `${CLAUDE_PLUGIN_ROOT}/hooks/x.py` 正确展开；未知事件名产生诊断且其他事件正常注册；未知占位符保持字面量。
  - _Requirements: 7.2, 7.3, 5.11, 6.10_

- [x] 15. 实现方言发现顺序、symlink 拒绝与 overlay 合并（M2-1）
  - 在 `plugin/dialect/mod.rs` 实现 `discover(root) -> Option<DiscoveredManifest>`：根 `plugin.json` 先取 `symlink_metadata`（符号链接或非常规文件 → 不作为候选），读内容判定 `$schema`，非 `Unrelated` 则选中且方言为 `AgentPluginV1`；`Unsupported` → 拒绝插件并报告版本；否则按序取 `.codex-plugin` → `.claude-plugin` → `.cursor-plugin`。
  - 实现 overlay 合并：v1 根 manifest + `.codex-plugin/plugin.json` 时，后者只补 Presentation 与扩展组件路径，不覆盖可移植核心字段。
  - 测试：四方言各一 fixture；symlink manifest 被拒；overlay 不能改 `name`/`version`；`Unsupported` 版本被拒绝并带版本信息。
  - _Requirements: 3.1, 3.2, 3.3, 3.6, 1.3, 1.8_

- [x] 16. 实现 Claude 约定目录扫描（M2-2）
  - 在 `plugin/dialect/claude.rs` 实现 `discover_conventions(root)`：扫 `skills/`、`commands/`、`agents/`、`hooks/hooks.json`、`.mcp.json`。
  - Claude `.mcp.json` 与 v1 `mcp.json` 并存时取 v1 并记诊断。
  - 保留 Codex 私有超集字段识别（`commands`/`agents`/`output-styles`/`runtime`/`interface.permissions`）——这些是 manifest 字段，已在 `plugin_manifest.rs` 解析，本步不动。
  - 测试用真实插件目录做 fixture：`借鉴/claude-code/plugins/hookify`（agents + commands + hooks + skills 全套）与 `pr-review-toolkit`（1 command + 6 agents），目录不存在时 skip。
  - **更正（实施时查明）：本任务原写「manifest 未声明组件路径时**回退**扫描」，这是错的。** 权威依据 `借鉴/claude-code/plugins/plugin-dev/skills/plugin-structure/SKILL.md` 明写 "Custom paths supplement defaults—they don't replace them. Components in both default directories and custom paths will load."。即约定目录**始终**扫描，manifest 声明的路径是**追加**。现有 `plugin_manifest::component_paths` 是替换语义（`if let Some(raw) = raw { return ... }`），对声明了额外目录的 Claude 插件会静默丢掉约定目录里的全部组件。故本步提供 `supplement(&mut Vec<PathBuf>, Option<&Path>)` 做并集，由任务 18 接线时调用。
  - 同一文档另证 `commands/` **支持子目录命名空间**（`commands/utils/helper.md` → `/helper (plugin:name:utils)`），非本任务原写的 `commands/*.md` 平铺。本步只产出目录路径，逐文件枚举在下游注册器，故不受影响。
  - `McpConventionSource{Portable,Claude}` 随路径一起返回：它决定解析契约（§7.2 禁止 `command` 含占位符，而 Claude 插件合法写 `${CLAUDE_PLUGIN_ROOT}/bin/server`），正是 design 记录的「来源区分」缺口的落点。
  - 大小写：`SKILL.md` 因 §7.1「named exactly」严格比对；方言目录无此规范文本，且 Claude Code 自身的存在性检查继承宿主文件系统语义（`Commands/` 在 Windows/macOS 上确实能用），故接受该条目但记可移植性诊断，不拒绝上游可用的插件。
  - _Requirements: 3.4, 3.7, 7.1, 7.4, 7.5_

- [x] 17. 实现 Presentation 四级优先级回退（M2-3）
  - 在 `plugin/dialect/mod.rs` 实现优先级链：Codex `interface` → Claude 市场清单条目 → 可移植核心 `description` + `author.name` → 插件目录名。
  - 测试：四级各一个用例，逐级剥离上层来源验证回退。
  - _Requirements: 3.5_

- [x] 18. 接线：`loader` 产出 `ResolvedPlugin`，`runtime` 消费它（M2 收口）
  - `plugin/loader.rs` 的发现逻辑改调 `dialect::discover`，origin 优先级、override 关系、孤儿标记语义保持不变。
  - `plugin/runtime.rs` 的投影输入换成 `ResolvedPlugin`。
  - `plugin/service.rs` 的 `list`/`read`/`set_enabled`/`install_*`/`uninstall` 对外行为不变；`state.json` 向后兼容。
  - 测试：既有 `state.json` 文件无需迁移即可读；origin 覆盖关系测试全绿。
  - _Requirements: 8.2, 8.3, 8.4, 8.5_

- [x] 19. 既有 9 个内置插件零回归验证（M2-5）
  - 新增 `tests/plugin_real_fixtures.rs`，对 `apps/desktop/src-tauri/resources/plugins/` 下 9 个插件逐一断言 skill / command / agent / app / hook / MCP server / output-style 计数与改造前相等。
  - 改造前基线：记录一份计数快照作为断言值，不凭推测填写。
  - skills 部分已实测结清：9 个内置插件**均无 `skills/` 目录**，故 skills 发现的任何改动对既有计数零影响。剩余需核对的是 command / agent / app / hook / MCP server / output-style。
  - _Requirements: 8.1_

- [x] 20. 规范一致性测试套件
  - 新增 `tests/plugin_conformance.rs`，把规范 Appendix A 的 conformance checklist 逐项对应一个测试名。
  - 覆盖：§4.1 合法/非法路径对照、§5.2 最小与完整 manifest、§5.5 全部名称样例、§7.2 三 transport 示例与 `mcp.json` 完整示例、§9.2 占位符展开语义。
  - AWS 插件（Apache-2.0）复制进 `tests/fixtures/plugins/` 并附 LICENSE；Anthropic 插件从 `借鉴/` 读取且缺失时 skip。
  - _Requirements: 9.4, 全部 Requirement 的规范条款追溯_

- [x] 21. M0–M2 收口门禁
  - `cargo fmt --all -- --check`
  - `cargo clippy --all-targets --all-features -- -D warnings`
  - `cargo test --workspace --offline`
  - `cd apps/desktop && pnpm build`
  - 插件界面手工验证（诊断区块与方言展示可见），必要时 `pnpm tauri dev` 留截图。
  - 当前状态：`cargo fmt --all -- --check`、`cargo clippy --all-targets -- -D warnings`、`cargo clippy --all-targets --all-features -- -D warnings`、`cargo test --workspace --offline`、`cd apps/desktop && pnpm build` 已通过。`all-features` 收口时移除了已被 `whisper-cli` sidecar 取代的 in-process `whisper-rs` 可选特性，避免门禁继续绑定本机 whisper.cpp FFI 工具链。
  - _Requirements: 8.5, 8.6, 6.11_
