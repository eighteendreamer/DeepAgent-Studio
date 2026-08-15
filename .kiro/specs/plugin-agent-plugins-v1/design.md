## Overview

把内部插件模型换成 Agent Plugins v1，三家客户端格式降级为输入方言。核心结构是一条单向管道：

```
插件根目录
   ↓  dialect::discover        选定权威 manifest（四方言顺序 + symlink 拒绝 + codex overlay）
   ↓  spec::v1::parse          解析可移植核心（closed schema + 名称约束 + 未知字段非致命）
   ↓  component::*             固定位置发现 skills / mcp.json；缺失时回退方言约定目录
   ↓  ResolvedPlugin           唯一归一化产物，携带诊断
   ↓  runtime::project         投影到 skills / hooks / mcp / commands / agents / apps
```

设计上有三个不变量：

1. **归一化产物唯一。** 下游只认 `ResolvedPlugin`，不再有"某处再判断一次这是 Claude 插件还是 Codex 插件"的分支。方言差异全部在 `dialect` 层消化掉，只留一个 `ManifestDialect` 标记供 UI 与诊断使用。
2. **失败分层而非失败即崩。** 规范 §11.3 把失败分成三级（拒绝插件 / 标记组件类型无效 / 跳过单个条目），诊断是这三级的类型化载体，必须一路传到界面。
3. **插件来源的配置与用户来源的配置走不同展开路径。** 这是本设计最关键的取舍，理由见下。

## 为什么插件 MCP 配置必须独立展开路径

`deepagent-mcp/src/config.rs` 的 `expand` 目前是全局共用的，行为是：单次非递归扫描（符合规范），但未知变量展开为空串，且作用于 `command` / `url` / `headers`。规范 §7.2.1 与 §9.2 明确禁止后三者，并要求未识别占位符保持字面量。

直接改 `expand` 会破坏用户手工配置的 MCP server —— 他们完全可能依赖 `${API_TOKEN}` 这类展开，`unknown_var_expands_empty` 测试也固定了当前语义。这属于跨用户场景的行为回归，不能做。

所以：

- `McpServerConfig::expand_with` 保持原样，继续服务用户与项目来源的配置。
- 新增 `plugin::spec::placeholder` 模块，只实现 v1 语义（两个占位符、单次非递归、未识别保持字面量、作用域限定 `args`/`env` 值/`cwd`），供插件来源的 MCP 与 hooks 使用。
- 插件来源的 `McpServerConfig` 在交给 `mcp_service` 前已完成 v1 展开，且**不再经过** `expand_with`。这一点需要在 `plugin/runtime.rs` 里显式保证，并有测试守住（否则会被二次展开，把字面量 `${FOO}` 吃掉）。

## 模块结构

全部落在 `crates/deepagent-app-core/src/plugin/`，不新增 crate。现有 7 个平铺文件收进目录树，`plugin_service.rs`（4000+ 行）按职责拆开。

```
crates/deepagent-app-core/src/plugin/
├── mod.rs              对外 re-export，保持 lib.rs 公共 API 逐项不变
├── model.rs            ResolvedPlugin / PortableManifest / Presentation / 诊断
├── spec/
│   ├── mod.rs
│   ├── schema.rs       $schema 三态判定（常量白名单，不联网）
│   ├── name.rs         PluginName newtype（§5.5）
│   ├── path.rs         插件相对路径解析 + containment（§4.1）
│   └── placeholder.rs  v1 占位符展开（§9.2）
├── dialect/
│   ├── mod.rs          发现顺序、symlink 拒绝、overlay 合并、Presentation 优先级
│   ├── codex.rs        .codex-plugin（含现有私有超集字段）
│   ├── claude.rs       .claude-plugin + 约定目录回退
│   └── cursor.rs       .cursor-plugin
├── component/
│   ├── mod.rs
│   ├── skills.rs       §7.1 非递归发现
│   ├── mcp.rs          §7.2 closed schema + 三 transport + per-server 隔离
│   └── extended.rs     commands / agents / hooks / output-styles / apps
├── loader.rs           原 plugin_loader（origin / override / orphan）
├── marketplace/        原 plugin_marketplace 拆分（materialize 逻辑不动）
├── dependency.rs       原 plugin_dependency（不动）
├── security.rs         原 plugin_security（scan 闸门，不动）
├── runtime.rs           原 plugin_runtime（投影，输入换成 ResolvedPlugin）
├── state.rs            从 plugin_service 抽出：state.json + 缓存快照
└── service.rs          瘦身后的 PluginService 门面
```

`plugin_manifest.rs` 的现有解析逻辑（三级发现、`paths` 对象与顶层组件字段并存、mcp/hooks 的 path|inline 混合）拆进 `dialect/codex.rs` 与 `component/`，不丢功能。

## 数据模型

```rust
/// $schema 判定结果（规范 §5.2）。判定只查常量白名单，绝不联网。
pub enum SchemaStatus { Supported, Unsupported, Unrelated }

pub const AGENT_PLUGIN_SCHEMA_URI: &str =
    "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json";
pub const AGENT_PLUGIN_MCP_SCHEMA_URI: &str =
    "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json";
pub const AGENT_PLUGIN_SCHEMA_PREFIX: &str = "https://agent-plugins.org/schemas/";

/// §5.5 约束的插件名。构造即校验，无法绕过。
pub struct PluginName(String);

impl PluginName {
    pub fn parse(raw: &str) -> Result<Self, PluginNameError>;
}

pub enum PluginNameError {
    Empty, TooLong { len: usize }, InvalidChar { ch: char, index: usize },
    BoundaryNotAlphanumeric, ConsecutiveHyphen, ConsecutivePeriod,
}

/// 规范 §5.2 的可移植核心，字段与 closed schema 一一对应。
pub struct PortableManifest {
    pub name: PluginName,
    pub version: Option<String>,
    pub description: Option<String>,
    pub author: Option<PluginAuthor>,        // 仅 name/email/url
    pub homepage: Option<String>,
    pub repository: Option<String>,
    pub license: Option<String>,
    pub keywords: Vec<String>,
    pub extensions: BTreeMap<String, serde_json::Value>,
}

pub enum ManifestDialect { AgentPluginV1, Codex, Claude, Cursor, DeepAgentLegacy }

/// 加载器唯一产物。下游只认这个。
pub struct ResolvedPlugin {
    pub id: String,
    pub origin: PluginOrigin,               // 沿用现有枚举
    pub root: PathBuf,
    pub data_dir: PathBuf,                  // §9.1 PLUGIN_DATA
    pub manifest_path: PathBuf,
    pub dialect: ManifestDialect,
    pub portable: PortableManifest,
    pub presentation: Presentation,
    pub skills: Vec<SkillComponent>,
    pub mcp: McpComponent,
    pub extended: ExtendedComponents,
    pub diagnostics: Vec<PluginDiagnostic>,
}

/// MCP 组件带整体状态，因为 §7.2.2 允许"整体禁用但其他组件继续"。
pub struct McpComponent {
    pub status: McpStatus,                  // Absent | Disabled { reason } | Loaded
    pub servers: Vec<PluginMcpServer>,      // 只含通过校验的
}

/// §11.3 三级失败的类型化构造器（实现见 plugin/model.rs）。
pub enum PluginDiagnostic {
    UnknownManifestField { field: String },
    ExtensionsNotObject,
    SkillSkipped { path: PathBuf, reason: String },
    McpDisabled { reason: String },
    McpServerSkipped { server: String, reason: String },
    ComponentInvalid { component: ComponentKind, path: Option<PathBuf>, reason: String },
    HookEventUnmapped { event: String },
}

/// 附到现有 PluginLoadError 上的分级。Default 取 Error（保守）。
pub enum DiagnosticSeverity { Info, Warning, Error }
```

### 诊断通道：扩展 `PluginLoadError` 而非新增并行数组

初版设计写的是"新增 `diagnostics` 取代粗粒度 `errors`"。实现时改成**给 `PluginLoadError` 加 `severity` 字段**，理由如下，这也是最终形态：

现有 `errors` 通道已经同时承载两个层级，只是没有区分：`manifest-parse-error` 让插件不可用，而 `path-not-found` 只是声明的路径缺失、插件照常可用。前端 `ErrorSection` 把两者一律渲染成"加载错误"，高估了后者。新增一个并行数组只会把同一份数据复制一遍，并不修正这个既有缺陷。

所以：

- `PluginLoadError` 增加 `severity: DiagnosticSeverity`，`#[serde(default)]` 取 `Error`（旧载荷不会被静默降级为提示）。
- `PluginDiagnostic` 保留为**类型安全构造器**：变体一次性绑定 `kind`、`component`、`severity` 三者，调用点无法拼出不一致组合；`into_load_error()` 投影到既有通道。
- 前端按 severity 排序与配色，全部非致命时标题改为"加载诊断"。

一个附带收益：加显式字段（而非给结构体实现 `Default`）让编译器把所有构造点列出来。实际有 7 处，其中 `plugin_dependency.rs` 的 2 处是初次盘点时漏掉的：

| 构造点 | kind | severity |
|---|---|---|
| `plugin_loader.rs` | `manifest-not-found` | Error |
| `plugin_loader.rs` | `manifest-parse-error` | Error |
| `plugin_loader.rs` | `reserved-name` | Error |
| `plugin_loader.rs` | `blocklist` | Error |
| `plugin_loader.rs` | `path-not-found` | Warning（§6.2 组件位置缺失不算错误） |
| `plugin_dependency.rs` | `dependency-cycle` | Error |
| `plugin_dependency.rs` | `dependency-unsatisfied` | Error |

`ResolvedPlugin.diagnostics` 仍然保留：加载期内部用它累积，投影到 DTO 时并入 `errors`。

`PluginDiagnostic` 自身的变体只产生两级：`Warning`（组件或条目被跳过，插件仍可用）与 `Info`（未知字段、`extensions` 非对象）。插件级致命错误不走诊断，而是 `Result::Err` 拒绝整个插件——§11.3 禁止发现被拒插件的任何组件。`Error` 级别只出现在既有的 policy 与 manifest 失败路径上。

## 关键流程

### manifest 发现（`dialect::discover`）

照 Codex 已验证的实现（`codex-rs/utils/plugins/src/plugin_namespace.rs` + `exec-server-protocol/src/protocol.rs:46`）：

1. 取 `plugin_root/plugin.json` 的 `symlink_metadata`。是符号链接或非常规文件 → 不作为候选（防止 manifest 指向根外）。
2. 读其内容判定 `$schema`：非 `Unrelated` 则选中它，方言为 `AgentPluginV1`；此时若 `.codex-plugin/plugin.json` 存在，作为 overlay。
3. 否则按序取第一个存在的常规文件：`.codex-plugin/plugin.json` → `.claude-plugin/plugin.json` → `.cursor-plugin/plugin.json`。
4. 都没有 → 该目录不是插件（不是错误）。

`Unsupported` 状态要区别处理：它是"目标是 Agent Plugins 但版本不认识"，必须拒绝插件并报告版本，不能退到方言路径。

### overlay 合并规则

只补不覆盖，防止 overlay 篡改可移植语义：

| 字段 | 来源 |
|---|---|
| `name` / `version` / `description` / `author` / `homepage` / `repository` / `license` / `keywords` | 只取根 `plugin.json` |
| `extensions` | 根 `plugin.json`；overlay 的 `com.deepagent.studio` 键可合并 |
| Presentation（displayName / category / logo / brandColor / longDescription / capabilities） | 优先 overlay 的 `interface` |
| 组件路径 | v1 固定位置优先；overlay 声明的扩展组件路径（commands 等）可补充 |

### Presentation 优先级链

四级回退，每级一个测试固定：

1. Codex `interface`（`displayName` / `shortDescription` / `longDescription` / `developerName` / `category` / `capabilities` / `brandColor` / `logo` / `logoDark` / `composerIcon`）
2. Claude 市场清单条目（`description` / `version` / `author` / `category`）
3. 可移植核心 `description` + `author.name`
4. 插件目录名

### 组件发现

**skills（§7.1）**：只读 `skills/` 的直接子项，是目录且其中名字**精确为** `SKILL.md` 的条目解析为常规文件 → 一个 skill。

关于现有实现的一处更正：`count_skill_root` **并不递归**（初版 design 的描述有误）。它的语义是「`path/SKILL.md` 存在则 path 自身算一个 skill，否则枚举 path 的直接子项」。与 v1 的唯一差异是前半句——v1 的固定位置 `skills/` 只承认直接子目录，`skills/SKILL.md` 不构成 skill。而「path 自身即 skill」在 Codex 方言下是**需要保留的**行为（manifest 可写 `"skills": "./my-skill"` 直指单个 skill 目录）。所以这不是缺陷，而是分层问题：严格 v1 语义在 `component/skills.rs`，方言的单目录写法留在 `dialect/codex.rs`。

大小写是这里的实际风险点：Windows 与 macOS 默认大小写不敏感，`path.join("SKILL.md").is_file()` 会接受 `skill.md`。§7.1 要求「named exactly」，因此实现比对目录项真实名字而非 join 后探测，保证同一插件在各平台被一致接受或拒绝。

实测结论：9 个内置插件**都没有 `skills/` 目录**，故 skills 相关改动对既有计数零影响。

**mcp.json（§7.2）**：
- 顶层只允许 `$schema` 与 `mcpServers`，其余字段 → 整体禁用 MCP，记 `McpDisabled`。
- `$schema` 版本必须与 `plugin.json` 一致 → 不一致同样整体禁用。
- 逐 server 按 `type` 匹配闭合联合。任一不合规 → 记 `McpServerSkipped`，继续下一个。**跨变体字段也要拒**（`stdio` 上出现 `url`、远程上出现 `command`），这才是"闭合"而非仅"带 tag"。
- transport 映射到现有 `deepagent_mcp::config::TransportType`：`stdio`→`Stdio`、`streamable-http`→`Http`、`sse`→`Sse`。现有枚举还有 `Ws`，v1 没有对应值，插件无法声明它（这是正确的收紧）。

**已确认的能力缺口：`cwd` 在共享 MCP 配置里无处安放。** `deepagent_mcp::config::McpServerConfig` 的字段是 `transport` / `command` / `args` / `env` / `url` / `headers`，**没有工作目录**。而 v1 的 stdio 允许 `cwd`，且 §7.2.1 还规定省略 `cwd` 时以插件根为工作目录。因此 `component/mcp.rs` 产出自带 `cwd` 的 `PluginMcpServer` 中间表示，不立即映射到共享配置。M2 接线时必须解决：要么给 `McpServerConfig` 加字段，要么在 spawn 处设置工作目录。**在接线处直接丢掉 `cwd` 会静默破坏合规插件**，这一点要有测试守住。

**扩展组件**：Claude 方言在 manifest 未声明时回退约定目录：`commands/*.md`、`agents/*.md`、`hooks/hooks.json`、`.mcp.json`。注意 Claude 的 `.mcp.json` 与 v1 的 `mcp.json` 是**不同文件名**，两者都要认，v1 优先。

### 占位符展开（`spec::placeholder`）

```rust
/// v1 §9.2：单次非递归，只认两个占位符，未识别的保持字面量。
pub fn expand_v1(input: &str, root: &Path, data: &Path) -> String;
```

与现有 `deepagent_mcp::config::expand` 的行为差异必须有测试明确固定：

| 输入 | 现有 `expand` | `expand_v1` |
|---|---|---|
| `${PLUGIN_ROOT}/bin` | 需 lookup 提供 | 展开 |
| `${UNKNOWN}` | 展开为空串 | 保持 `${UNKNOWN}` |
| 展开结果内含 `${PLUGIN_DATA}` 字面量 | 不再扫描 | 不再扫描（一致） |

作用域：`args` 每个元素、`env` 每个值、`cwd`。不作用于 `env` 键、`command`、`url`、`headers`。

方言别名在**归一化阶段前置重写**：`${CLAUDE_PLUGIN_ROOT}` / `${CODEX_PLUGIN_ROOT}` → `${PLUGIN_ROOT}`，DATA 变体同理。这样 `expand_v1` 内部只需处理两个规范占位符，别名不污染核心语义。

### 子进程环境组装（§9.1）

顺序不能反：

1. 客户端选定的基础环境。
2. 叠加配置的 `env`（已完成 v1 展开）。
3. 最后写入 `PLUGIN_ROOT` 与 `PLUGIN_DATA`，覆盖同名项。
4. 额外保留现有 `DEEPAGENT_PLUGIN_ID` / `DEEPAGENT_PLUGIN_ROOT` / `DEEPAGENT_PLUGIN_DATA` / `CLAUDE_PLUGIN_ROOT` / `CLAUDE_PLUGIN_DATA`（兼容既有内置插件，Requirement 5.12）。

`env` 键里出现 `PLUGIN_ROOT` 或 `PLUGIN_DATA` → 该 server 无效（§9.2 末段）。这条容易漏，要单测。

`PLUGIN_DATA` 复用现有 `data_root.join(sanitize_file_name(id))`，语义上已满足"跨更新保留"（`install_from_dir` 只动 `install_path`，不碰 data 目录），但需补测试证明更新后目录内容仍在。

### hooks（复用既有实现）

盘点确认 `deepagent-hooks` 的 `HookEvent` 已采用 Claude Code 命名并覆盖其全部事件，`HookDefinitions::parse` 已能解析 Claude `hooks.json`，`matcher_matches` 已实现工具名别名。因此本设计**不新建 hooks 映射层**，只做两件事：

1. hooks 命令与 env 的占位符改走 `expand_v1`（现在走 `expand_plugin_vars`，会展开未知变量为空串）。
2. `HookEvent::parse` 返回 `None` 的事件名，记 `HookEventUnmapped` 诊断，不静默丢。

Claude 侧事件与内部生命周期点的对应（既有实现，此处仅作记录）：

| Claude `hooks.json` 键 | 内部 `HookPoint` |
|---|---|
| `SessionStart` | `SessionStart` |
| `UserPromptSubmit` | `UserPromptSubmit` |
| `PreToolUse` | `BeforeToolUse` |
| `PostToolUse` | `AfterToolUse` |
| `Stop` | `Stop` |
| `PreCompact` | `BeforeCompact` |
| `SubagentStop` | `SubagentStop` |
| `SessionEnd` | `SessionEnd` |

## 现有代码改造映射

| 文件 | 处置 | 风险 |
|---|---|---|
| `plugin_manifest.rs` | 拆入 `spec/` + `dialect/`，逻辑保留并补 v1 与 cursor | 中：解析分支多，靠既有测试守 |
| `plugin_service.rs` | 拆为 `service.rs` + `state.rs` + `marketplace/` | 中：纯搬移，独立提交 |
| `plugin_loader.rs` | 发现逻辑改调 `dialect::discover`，origin/override/orphan 不动 | 低 |
| `plugin_marketplace.rs` | 拆目录，materialize 不动 | 低 |
| `plugin_runtime.rs` | 输入换 `ResolvedPlugin`；占位符改 `expand_v1`；补 `PLUGIN_ROOT`/`PLUGIN_DATA` | 高：这里是行为变更集中处 |
| `plugin_security.rs` / `plugin_dependency.rs` | 原样搬入 | 低 |
| `dto.rs` | `PluginDto` 新增 `dialect`、`diagnostics`、`data_dir` | 低 |
| `src-tauri/src/lib.rs` | 命令签名不变，跟随 DTO | 低 |
| `apps/desktop/src/types.ts` / `api.ts` | 同步 DTO 字段 | 低 |
| `PluginsViewReal.tsx` | 详情页增"来源方言"与"诊断"区块 | 低 |

## 测试策略

四层，重点是真实语料。

**规范一致性测试**（`tests/plugin_conformance.rs`）：把规范正文的 example 做成 fixture，Appendix A 的 conformance checklist 逐项对应一个测试名。含规范 §4.1 给的合法/非法路径对照、§5.2 的最小与完整 manifest、§7.2 的三 transport 示例、§5.5 的全部合法与非法名称样例。

**真实插件回归**（`tests/plugin_real_fixtures.rs`）：
- AWS 插件（Apache-2.0）复制进 `tests/fixtures/plugins/`，可提交。
- Anthropic 的 13 个插件不复制。测试从 `借鉴/claude-code/plugins/` 读取，**目录不存在时 skip 而非 fail**，保证 CI 不依赖非产物目录且不碰许可。

**失败边界测试**：每个 `PluginDiagnostic` 变体至少一个用例，且每个用例都要断言"其余组件仍加载成功"。

**行为差异测试**：`expand_v1` 与 `expand` 的差异表逐行固定；额外一个测试证明插件来源的 MCP 配置没有被二次 `expand_with`。

**跨层契约**：`pnpm build` 类型检查通过，前端能渲染诊断。

## 取舍与已知限制

- **不新增 crate。** 依赖方向核对结果：`app-core` 依赖 `skills`/`hooks`/`mcp`，后者只依赖 `core`/`context`/`tools`。现有"app-core 编排、向下投影"无环，抽 crate 不解决问题只加层。
- **`Ws` transport 插件不可用。** v1 闭合联合没有 WebSocket。用户手工配置的 MCP 仍可用 `Ws`，只是插件不能声明。
- **skills 发现对既有计数无影响。** 已实测：9 个内置插件均无 `skills/` 目录。原先担心的「非递归改动会改变计数」不成立，因为现有实现本来就不递归。
- **Claude 插件的 `.mcp.json` 与 v1 `mcp.json` 并存。** 两个文件名都认，v1 优先。若插件同时提供两者且内容冲突，取 v1 并记诊断。
- **本 spec 不含 dsh。** Cordis 侧车（M6/M7）是独立子系统，`ResolvedPlugin` 不为它预留字段，避免过早抽象；届时它走自己的模型。
