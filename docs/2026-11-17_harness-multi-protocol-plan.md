# Harness 多协议与模型注册表改造方案

日期：2026-11-17
状态：待评审
基线：HEAD `738fa508`

## 0. 目标

1. 本系统自写的 Responses 协议代码整体替换为 `async-openai`，针对手写实现的旧测试弃用。
2. 新增模型注册表：供应商 + 模型两层，协议、URL、Key、上下文窗口、输出上限、思考档位全部入库。
3. 定义本系统 harness 的中性输入/输出参数，与 `async-openai`、`anthropic-sdk-rs` 一一对应；SDK 缺失处手写补齐。
4. 参照 deepseek-harness 的「协议表 + 工厂分发」组织协议层。
5. 提升缓存命中率：稳定前缀设计。
6. 余额查询改为按供应商类型分发，不再写死 DeepSeek 官方接口。
7. 设置页可自行添加供应商、选协议、拉取模型入库；对话窗口按供应商分组选模型。
8. 运行中切换模型/供应商，在下一次请求生效，不打断当前输出。
9. harness 协议层统一收敛到现有 `apps/cli/src/app_server.rs`（stdio JSON-RPC）与 `jsonl.rs`，不新建第二套协议引擎（F 批次，见 §9）。

### 0.1 架构修正（2026-11-17，用户确认）

1. **无限画布与 harness 分属两套，互不合并。** 画布注册表（`canvas_provider_service.rs`）保持不动；harness 使用独立的注册表。原方案"扩展画布注册表为共享真源"作废。
2. **新增主设置页"模型"**：专门管理 harness 的模型，用于接入更多平台的大模型 API。用户在此添加供应商（协议、base_url、密钥、余额类型、logo）并拉取、激活模型。
3. **激活的模型进入会话窗口的模型列表**，按供应商分组。推理强度滑杆按该模型的 `thinking_levels` 显示；底部"余额"按供应商的 `balance_kind` 查询并显示。
4. **替换写死的 DeepSeek 逻辑**：
   - `DEEPSEEK_BASE_URL` 不再作为主对话的固定地址；
   - 启动时自动查询官方模型（`ModelDiscovery` / `ModelCatalog` 的 `auto_select`）停用，改为用户在"模型"页手动拉取、激活；
   - `fetch_balance` 按供应商类型分发（P7），不再固定 DeepSeek 路径；
   - 现有 `deepseek_api_key` 迁移为一个 provider 条目（§3.1），密钥不重新导出。
5. **删除设置页"系统视觉的大模型接入"**（`VisionSettings` 的设置 UI 与配置项）。注意：该配置还被图片准备、`VisionMode` 的路径引用，删除后向非视觉模型发送图片的行为需用户确认（见 §10-8）。
6. 方案 §3（`harness_providers` / `harness_models` 两张表）保留，它与画布注册表无关，是 harness 唯一的模型注册表。

---

## 1. 思考等级

### 1.1 各家实际档位（2026-11 查证）

证据来源分两类：本仓库内参考实现（A 类）、厂商官方文档（B 类）。未查到的标注为待实测。

**A 类：参考实现**

| 来源 | 档位 | 说明 |
|---|---|---|
| deepseek-harness `catalog.ts:THINKING_LEVEL_GATE` | `off` `minimal` `low` `medium` `high` `xhigh` `max` | 7 档，pi-ai 的规范 |
| cockpit-tools `types/codex.ts:60` | `low` `medium` `high` `xhigh` `max` `ultra` | 6 档，Codex 专有 |
| Anthropic Messages（`anthropic-sdk-rs`） | `low` `medium` `high` `xhigh` `max` | `disabled` 返回 400 |

**B 类：厂商官方文档**

| 厂商 / 型号 | 可用值 | 默认 | 能否关闭思考 |
|---|---|---|---|
| **Qwen3.8 系列**（qwen3.8-max / flash） | `low` `medium` `xhigh` | `xhigh` | 能（`enable_thinking=false`） |
| **DeepSeek-V4 Pro** | `high` `max` | `high` | 待确认 |
| **GLM-5.2 / 5.1 / 5** | `high` `max` | `high` | 能（`thinking.type=disabled`） |
| **GLM-5.3 / 5.3-Flash / 5.3-FlashX** | `low` `high` `max` | `max` | **不能**（`enable_thinking=false` 请求失败） |
| **Kimi K3** | `low` `high` `max` | `max` | **不能**（始终思考，无 thinking 对象） |
| **Kimi K3.1 Pro** | `low` `medium` `high` `max` | `max` | 能（`enable_thinking=false`） |
| **Kimi K2.6 / K2.7** | 开关（`thinking.type` enabled/disabled） | 开 | 能 |
| **MiMo v2.5** | 开关（`thinking.type`） | 开 | 能 |
| **OpenAI gpt-5.x** | `none` `minimal` `low` `medium` `high` | `medium` | 能（5.1+） |
| **OpenAI gpt-6-astra** | 最低 `low` | — | 否（下限是 low） |
| **OpenAI Codex 6.x** | `low` `medium` `high` `xhigh` `max` `ultra` | — | — |
| **Step** | 待实测 | — | — |

各家对「不支持的值」的处理方式不同，这点很重要：

- **DashScope 会映射而非报错**：DeepSeek-V4 与 GLM 系列（5.2/5.1/5）收到 `low`/`medium` 会映射成 `high`，收到 `xhigh` 映射成 `max`；Qwen3.8 收到 `max`/`high` 都映射成 `xhigh`，`minimal` 映射成 `low`，`none` 映射成 `enable_thinking=false`。
- **但 GLM-5.3 / Kimi K3 收到关闭指令会直接 400**，不是降级。

横向并集共 8 个：`off/none` `minimal` `low` `medium` `high` `xhigh` `max` `ultra`。

### 1.2 收敛到 5 档

逐档看「几家原生支持 / 其余能否安全映射」：

| 候选 | 原生支持方 | 其余家 | 判定 |
|---|---|---|---|
| `off` | Qwen、GLM-5.2、MiMo、Kimi K2.6、OpenAI 5.1+ | Anthropic / GLM-5.3 / Kimi K3 **拒绝**，只能降为最低档 | **保留** —— 但必须按模型禁用，见 §1.4 |
| `minimal` | 仅 OpenAI gpt-5.0 / Qwen3.8-Max（经 OpenRouter） | DashScope 明确 `minimal→low` | **删** —— 一家独有，且已被定义等价于 low |
| `low` | Qwen3.8、Kimi K3/K3.1、GLM-5.3、Anthropic、OpenAI | DeepSeek/GLM-5.2/MiMo → high | **保留** |
| `medium` | Qwen3.8、Kimi K3.1、Anthropic、OpenAI | DeepSeek/GLM → high | **保留** —— 是唯一有真实差异的中间档 |
| `high` | **全部** | — | **保留** —— 唯一全支持档，可作为任何映射的安全落点 |
| `xhigh` | Qwen3.8（默认）、Anthropic | 其余 → max | **删** —— Qwen 的 `max`/`high` 已映射到 xhigh，Anthropic 的 `max` 高于 xhigh，语义被 `max` 覆盖 |
| `max` | DeepSeek、GLM、Kimi、Anthropic、Codex 6.x | OpenAI gpt-5.x → 钳到 high | **保留** —— 最强档唯一落点 |
| `ultra` | 仅 Codex 6.x | 无 | **删** —— 一家专有，语义未公开定义 |

**结论：`off` / `low` / `medium` / `high` / `max`。**

三个被删项都有硬理由：`minimal` 和 `ultra` 各只有一家支持，`xhigh` 的语义已被 `max` 完全覆盖。这比 8 档少了 3 档，同时保住了「关闭思考」这个明确需求。

### 1.3 跨协议映射表

| UI 档位 | Anthropic | OpenAI 5.x | DeepSeek-V4 | GLM-5.2/5.1/5 | GLM-5.3 | Kimi K3 | Kimi K3.1 | Qwen3.8 | MiMo |
|---|---|---|---|---|---|---|---|---|---|
| `off` | `low` ¹ | `none` | 省略 ² | `thinking.type=disabled` | **禁止** ¹ | **禁止** ¹ | `enable_thinking=false` | `enable_thinking=false` | `thinking.type=disabled` |
| `low` | `low` | `low` | `high` ³ | `high` ³ | `low` | `low` | `low` | `low` | `enabled` ⁴ |
| `medium` | `medium` | `medium` | `high` ³ | `high` ³ | `high` ³ | `medium` | `medium` | `medium` | `enabled` ⁴ |
| `high` | `high` | `high` | `high` | `high` | `high` | `high` | `high` | `high`→`xhigh` ³ | `enabled` ⁴ |
| `max` | `max` | `high` ⁵ | `max` | `max` | `max` | `max` | `max` | `max`→`xhigh` ³ | `enabled` ⁴ |

¹ 该模型强制思考，`off` 必须**在 UI 置灰**，不能静默降级 —— 送关闭指令会 400。
² DeepSeek-V4 Flash 默认不开思考，省略 `reasoning_effort` 即关闭。
³ 供应商侧会自动映射，本系统照送原值即可；UI 需标注「该模型实际只有 N 档」。
⁴ MiMo 只有开关，所有档位等价；UI 只显示 `off` / `high`。
⁵ OpenAI gpt-5.x 无 `max`，钳到 `high`。

**映射表进代码作为唯一真源，不做隐式钳制。** 用户在 `medium` 档看到的效果与 `high` 相同，是供应商本身只有两档造成的 —— 界面必须能解释，否则用户会以为参数没生效。

### 1.4 每模型能力声明

档位可用性不入代码、不入文档，入数据库：

```json
{
  "thinking_levels": ["off", "low", "medium", "high", "max"],
  "thinking_distinct": ["low", "high", "max"],
  "thinking_disable": false
}
```

| 字段 | 用途 |
|---|---|
| `thinking_levels` | UI 下拉显示哪些档 |
| `thinking_distinct` | 哪些档实际有区别（MiMo 只 `low`/`high` 有差异时，其余标注「等价」） |
| `thinking_disable` | 能否真正关闭；`false` 时 `off` 置灰并提示「该模型不支持关闭思考」 |

输入框的下拉从当前模型这一行读，换模型自动变。

---

## 2. 协议配置粒度

**结论：供应商级默认 + 模型级可选覆盖。**

```ts
// deepseek-harness 原设计（llm-pi-ai/src/config.ts:110-115）
/**
 * Wire protocol every model on this route speaks. Omission keeps each
 * installed catalog model's own protocol... a route the catalog does not
 * ship must name one.
 */
api?: string
```

**为什么不能只按供应商**：newapi / sub2api 这类中转站经常同时代理 Claude（Messages）和 OpenAI（Responses）。协议只配在供应商上，这类站点只能拆成两个「供应商」各填一遍 Key。

**为什么不能强制按模型**：95% 的供应商下全是同一协议。强制每模型配是重复劳动，出错概率反而更高。

所以：`provider.protocol` 必填，`model.protocol` 可空（空 = 继承）。

---

## 3. 数据模型

`context_window` / `output_limit` 是**模型**属性而非供应商属性 —— 同一供应商下多个模型窗口不同。拆两张表。

```sql
CREATE TABLE harness_providers (
    id            TEXT PRIMARY KEY,
    channel       TEXT NOT NULL,              -- 供应商名称
    logo          TEXT NOT NULL DEFAULT '',   -- 图标标识
    model_url     TEXT NOT NULL,              -- API base URL
    protocol      TEXT NOT NULL,              -- 'responses' | 'messages'（默认）
    api_key_ref   TEXT NOT NULL,              -- → secret_records.name
    balance_kind  TEXT NOT NULL DEFAULT 'auto',
    status        INTEGER NOT NULL DEFAULT 1,
    revision      INTEGER NOT NULL DEFAULT 1,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL
);

CREATE TABLE harness_models (
    id              TEXT PRIMARY KEY,
    provider_id     TEXT NOT NULL REFERENCES harness_providers(id) ON DELETE CASCADE,
    model_name      TEXT NOT NULL,            -- 发给 provider 的原字符串
    display_name    TEXT NOT NULL,
    protocol        TEXT,                     -- NULL = 继承 provider
    context_window  INTEGER NOT NULL DEFAULT 1000000,
    output_limit    INTEGER NOT NULL DEFAULT 131072,   -- 128K
    capabilities    TEXT NOT NULL DEFAULT '{}',        -- thinking_levels 等
    status          INTEGER NOT NULL DEFAULT 1,
    updated_at      INTEGER NOT NULL,
    UNIQUE (provider_id, model_name)
);

CREATE INDEX idx_harness_models_provider ON harness_models(provider_id, status);
```

### 3.1 api-key 不落这张表

现有 `secret_records(name, ciphertext, nonce, key_version, updated_at)`（`migrations.rs:214`）已做认证加密。`api_key_ref` 只是指向它的逻辑名。

迁移：现有 `deepseek_api_key` 生成一个默认 provider 行，`api_key_ref = 'deepseek_api_key'`，不重新导出密钥。

### 3.2 `max_tokens` 的语义

**它是单次请求的输出上限，不是整个会话。**

一次请求里模型最多生成这么多 token 就停（`stop_reason: max_tokens`），下一轮是新计数。但它通过历史间接影响会话占用 —— 128K 输出跑 3 轮，下一轮输入就多了 384K。

所以压缩必须按**输入预算**算：

```
inputBudget = context_window − output_limit
compact     = 0.78 × inputBudget
ceiling     = 0.95 × inputBudget
```

默认值代入：`1M − 128K = 896K`，`compact ≈ 699K`，`ceiling ≈ 851K`。

注意 Anthropic 的 `max_tokens` 是**必填**字段，无默认值 —— 这个 128K 不是可选项，每次请求都得带。

---

## 4. 中性 I/O 参数与协议映射

### 4.1 中性内容层

现有 `ResponseItem`（`crates/deepagent-core/src/response_item.rs`，`Reasoning` 在 `:55`）已有形状，缺一个字段：

```rust
pub enum ContentBlock {
    Text { text: String },
    Thinking { text: String, signature: Option<String> },   // ← 新增 signature
    ToolUse { id: String, name: String, input: Value },
    ToolResult { tool_use_id: String, content: String, is_error: bool },
    Image { attachment: MessageAttachment },
}
```

现有 `Reasoning`（`response_item.rs:55`）只有 `id` 和 `content`，不存签名。Messages 的 thinking block 带 `signature`，丢了就只能当普通文本回放 —— 模型看不见自己当时的推理。**必须补。**

### 4.2 请求侧

| 中性 | async-openai (Responses) | anthropic-sdk-rs (Messages) | 备注 |
|---|---|---|---|
| `system` | `instructions` | 顶层 `system`（字符串或 block 数组） | — |
| `messages[]` | `input: []` items | `messages[]`，role + content block 数组 | — |
| `Thinking` | `reasoning` item（`encrypted_content`） | `thinking` block + `signature` | Responses 无签名概念 |
| `ToolUse` | `function_call` / `custom_tool_call` | `tool_use`（仅 JSON input） | **Messages 无 custom tool** |
| `ToolResult` | `function_call_output`，平铺 | `tool_result`，**必须紧跟 tool_use** | **位置约束** |
| `ToolSchema` | `{type:"function", name, description, parameters}` | `{name, description, input_schema}` | 字段名不同 |
| `max_output_tokens` | `max_output_tokens` | `max_tokens`（**必填**） | — |
| `effort` | `reasoning.effort` | `output_config.effort` | 位置不同 |
| `temperature` / `top_p` | 支持 | 支持 | — |
| `tool_choice` | 有 | 有 | — |
| `cache_control` | 无此概念 | block 级，最多 4 断点 | 仅 Messages |

### 4.3 响应侧

| 中性事件 | Responses | Messages |
|---|---|---|
| 文本增量 | `response.output_text.delta` | `content_block_delta` → `text_delta` |
| 思考增量 | `response.reasoning_text.delta` | `thinking_delta` |
| 签名增量 | — | `signature_delta` |
| 工具参数增量 | `response.function_call_arguments.delta` | `input_json_delta` |
| 终态 | `response.completed` / `.incomplete` | `message_stop` + `message_delta.stop_reason` |
| usage | 一次性 `response.usage` | `message_start` + `message_delta`，**覆盖式** |

### 4.4 必须手写的部分

1. **`Thinking.signature`** —— `anthropic-sdk-rs` 的 `ContentBlock` 有 `signature` 可取；中性层必须有，Responses 侧写 `None`。

2. **`output_tokens_details.thinking_tokens`** —— 已核实 `anthropic-sdk-rs` 全 `src/` 无此字段（搜 `output_tokens_details|thinking_tokens|total_tokens` 零命中），`Usage.reasoning_tokens` 会恒为 0。走 raw JSON 旁路，复用 `usage.rs:12` 的 `sanitized_provider_usage` 白名单，把 `thinking_tokens` 补进 `output_tokens_details` 子键。

3. **`tool_result` 排序** —— SDK 不管。规则：同一 assistant 的多个 `tool_use`，下一条 user 里一次结清，`tool_result` 排在文本/图片之前，按原调用顺序。

4. **custom tool → function** —— `apply_patch` 这类在 Messages 里转成 `{name, description, input_schema}`；历史里的旧 raw patch 包一层 `{"patch": ...}`。

### 4.5 usage 换算

`anthropic-sdk-rs` 的 `Usage`（`resources/messages/types.rs:117`）含四个计数，`CacheCreation`（`:104`）拆了 5m / 1h。

流式累加（`message_stream.rs:453-468`）：`output_tokens` **直接赋值，永不累加**。deepseek-harness `translate.ts:35-44` 写法相同。

落到现有 `Usage`（`chat.rs:862`）：

| 现有字段 | Messages 来源 |
|---|---|
| `prompt_tokens` | `input_tokens + cache_read_input_tokens + cache_creation_input_tokens` |
| `completion_tokens` | `output_tokens` |
| `reasoning_tokens` | raw `output_tokens_details.thinking_tokens`（子集，仅展示） |
| `total_tokens` | 前两项相加（协议不给，自己算） |
| `prompt_cache_hit_tokens` | `cache_read_input_tokens` |
| `prompt_cache_miss_tokens` | `input_tokens + cache_creation_input_tokens` |

一条口径：Anthropic 与 DeepSeek 的 `input_tokens` 都只是**未命中部分**，三档相加才是上下文占用。

---

## 4.6 持久化边界：模型返回只进会话 JSONL

### 4.6.1 现状（已核实）

| 维度 | 本系统 | Codex `rollout` | Claude Code `sessionStorage.ts` | deepseek-harness `session-persistence-jsonl` |
|---|---|---|---|---|
| 文件位置 | `sessions/<id 前两位 hex>/<id>/session.g{gen:06}.v1.jsonl.zst`（`session_files.rs:1173,1191`） | `sessions/YYYY/MM/DD/rollout-<ts>-<id>.jsonl`（`recorder.rs` 注释） | `<projectDir>/<sessionId>.jsonl`（`sessionStorage.ts:204`） | `<root>/<项目目录>/<会话目录>/session.vN.jsonl[.zstd]`（README §磁盘布局） |
| 压缩 | zstd 帧（`session_files.rs:836`） | 纯文本行；`compression.rs` 存在但启用条件未核实 | 纯文本 | 默认 `zstd`，可配 `compression: 'none'` |
| 头部 | `Header { format, generation, session }` 为第 0 条记录 | 首条 `SessionMeta` 行（`RolloutItem::SessionMeta`） | 每条 entry 自带 `sessionId`、`cwd`、`version` | header 行，文件名与版本一致 |
| 记录类型 | `Event { schema, event }`、`Projection { schema, source, items }` | `RolloutLine { timestamp, item }`，`RolloutItem` 共 11 种变体（`history/src/lib.rs:211`） | entry 链，`uuid` / `parentUuid` / `isSidechain` | 每个持久事件一行 |
| 写入 | 每次 append 持有文件锁，校验和帧 | 异步 writer 任务，`flush()` 显式同步 | `FLUSH_INTERVAL_MS = 100` 批量写 | 批次 append 后 fsync，撕裂尾部恢复 |
| 版本与迁移 | 文件名 `v1` + 记录 `schema`；V18 `migration_notices` 做 cutover | 文件名与 rollout 结构 | 无显式版本 | 文件名 `vN`，generation 不可变，迁移 catalog |
| SQLite 角色 | 仅 `session_file_state` 游标（`migrations.rs:343-359` 明确"可重建"）+ `session_search_*` 派生索引 | state DB 为可重建索引 | 无 | 可选 `session-query-sqlite` |

本系统 V18 注释写明：旧 SQLite 事件行"协议时代无法安全经当前 Responses 适配器回放"，因此直接删除而不双读（`migrations.rs:346-347`）。这条经验要沿用到新协议上。

### 4.6.2 结论

1. **模型返回继续只进会话 JSONL。** 不新增任何存放模型正文的表。本系统已是这个结构，不需要搬迁。
2. **保留本系统相对 Codex / Claude Code 的差异**：zstd 帧、generation、header、文件锁、projection 原子性。这些是 `session_files` 崩溃恢复和 trash 逻辑的前提，改成纯文本会退化。外部工具直接读取的需求，用类似 deepseek 的 `compression: 'none'` 配置项解决，不改默认值。
3. **SQLite 的边界**：会话目录、游标、可重建搜索索引、运行账本（`runs` / `run_events` / `checkpoints` / `artifacts`）、费用（`costs`）、§3 的供应商与模型配置。模型输出正文不进入这些表。`costs` 是否可由 `UsageRecorded` 事件重建，待核实。
4. **JSONL 写入中性内容，不写供应商 wire JSON。** 同一会话可能中途切换供应商（§8），如果落盘的是 Responses 或 Messages 原始 JSON，切换后就无法回放。中性 `ContentBlock`（§4.1）才是落盘形态。
5. **每条模型产出记录带来源。** 草案：`ResponseItemAppended` 增加可选字段 `origin: Option<ModelOrigin>`，`ModelOrigin { provider_id, model_name, protocol }`。用途：审计；判断思考签名能否在另一模型上回放（§10-5）。
6. **兼容策略**：新增字段一律 `#[serde(default, skip_serializing_if = "Option::is_none")]`，旧 JSONL 仍可读，不需要 bump `schema`。只有无法兼容的形状变化才 bump，并走 V18 的 cutover 方式（`migration_notices`），不双读旧协议 JSON。
7. **写入节奏沿用现有路径**：每次 append 加锁、写校验和帧。不采用 Claude Code 的 100ms 批量写，因为它扩大崩溃丢失窗口，与 projection 原子性要求冲突（本地设计，依据是现有 `session_files` 的 append 语义）。

### 4.6.3 要改的代码边界（只列契约，不改实现）

| 位置 | 变更 |
|---|---|
| `crates/deepagent-core/src/response_item.rs` | `Reasoning` 增 `signature: Option<String>`（§4.1）；`ContentBlock` 与之对齐 |
| `crates/deepagent-core/src/event.rs` `ResponseItemAppended` | 增可选 `origin`（见 4.6.2-5） |
| `crates/deepagent-persistence/src/session_files.rs` | 不改格式；写入路径保持不变，补一条"旧记录缺新字段仍可读"的测试 |
| `crates/deepagent-persistence/src/migrations.rs` | 本批次不新增表 |
| 测试 | 旧 JSONL（无 `signature` / `origin`）回放测试；新字段往返测试；会话中途切换供应商后整文件回放测试 |

---

## 5. 协议表 + 工厂分发

照 deepseek-harness `provider.ts:48-53`：

```rust
// crates/deepagent-models/src/protocol.rs
pub enum Protocol { Responses, Messages }

type CodecFactory = fn() -> Box<dyn WireCodec>;

static PROTOCOLS: &[(Protocol, CodecFactory)] = &[
    (Protocol::Responses, || Box::new(async_openai_codec::Codec::new())),
    (Protocol::Messages,  || Box::new(anthropic_codec::Codec::new())),
];

pub trait WireCodec: Send + Sync {
    fn encode_request(&self, req: &ModelRequest, cfg: &CodecConfig) -> Result<Value>;
    fn decode_stream(&self, chunk: &[u8], sink: &mut dyn EventSink) -> Result<()>;
    fn map_usage(&self, raw: &Value) -> Usage;
    fn map_stop(&self, raw: &Value) -> FinishReason;
    fn map_thinking(&self, level: ThinkingLevel) -> ThinkingWire;
}
```

**硬约束：`match protocol` 只允许出现在工厂里。** `core` / `runtime` / `persistence` 不允许出现协议分支。

现状核查：`WireMode::` 在 `runtime` 有 17 处，全部在测试文件；生产代码干净。这个边界要守住。

---

## 6. 缓存命中率

| 措施 | 说明 |
|---|---|
| system 前缀冻结 | 现有 `runtime_snapshot.rs` 已有 stable rules hash，保留 |
| 工具 schema 顺序确定 | 按 name 排序后序列化，禁止 HashMap 迭代序 |
| 动态内容后置 | 时间戳、动态环境放 system 末尾，不插开头 |
| `cache_control` 断点 | Messages 最多 4 个：system 尾、工具定义尾、历史最后一条 |
| DeepSeek 自动缓存 | 不配 `cache_control`，但前缀必须稳定 |
| 压缩时机 | 只在 turn 边界压，不中途压 |
| 压缩阈值 | 比例化，见 §3.2 |

**唯一诊断信号**：`cache_read_input_tokens` 长期为 0，说明有东西在抖。这个值要进 UI。

---

## 7. 余额查询

现有 `balance.rs:18` 写死 DeepSeek 的 `/user/balance`，路径、字段名、金额格式全部绑定。改为按类型分发。

`cockpit-tools` 的实测做法（`src-tauri/src/commands/codex_model_provider_commands.rs`）：

```
官方 DeepSeek  → GET /user/balance                → is_available + balance_infos[]
new_api       → GET dashboard/billing/subscription
                GET dashboard/billing/usage
                GET api/usage/token/               （可选，失败不阻断）
sub2api       → GET <usage_url>                    → 单次，形状另判
无类型        → 先试 new_api，失败再试 sub2api
```

三条直接照抄：

1. **`quota_unlimited` 单独字段**（`:1916`），不把无限塞成 `u64::MAX`
2. **数值用 micros 存**，展示层除法：`value as f64 / 1_000_000.0`
3. **未知形状显式失败** —— 返回 `PROVIDER_USAGE_DETECT_FAILED` 并带两家错误，不显示 0

```rust
pub enum BalanceKind { None, Auto, Official, NewApi, Sub2Api }

pub struct BalanceAmount {
    pub amount: Option<f64>,
    pub unit: String,        // "USD" | "CNY" | "tokens"
    pub unlimited: bool,
}
```

切换模型时若 `provider_id` 变化，重查余额。

---

## 8. 运行时切换模型

沿用现有骨架：admission 冻结快照，执行时绑定。

```
用户切换 → 写 DB（新 revision）
              ↓
当前 turn 继续用已冻结快照（不打断）
              ↓
下一次 prepareCall → resolver 重读 → 新 provider + 新 model + 新 key
```

要改的是 `bind()`：从「读 settings 的 `deepseek_api_key`」改成「读 model 行的 `provider_id` → provider 行的 `api_key_ref`」。

切换时必须发生：
- 余额按新 provider 重查
- 缓存前缀必然失效（新模型新前缀），这是预期行为
- running turn 不中断

---

## 9. 批次规划

| 批次 | 内容 | 会话 |
|---|---|---|
| **P0 可测性验证** | 两个 SDK 能否注入本系统 `HttpTransport`；不能则定替代方案 | 1 |
| **P1 中性内容层** | `ContentBlock` + `Thinking.signature`；`ThinkingLevel` 5 档 + 映射表 | 2 |
| **P2 协议表 + 工厂** | `Protocol` 枚举、`WireCodec` trait、`match` 收口一处 | 2 |
| **P3 数据模型** | 两张新表 + 迁移；`ModelResolver` 单点解析 | 3 |
| **P3b 会话 JSONL 承载模型输出** | `signature` / `origin` 可选字段；旧 JSONL 回放测试；中途切换供应商的整文件回放测试；不新增表 | 2 |
| **P4 Responses codec** | async-openai 接入，替换手写实现 | 4 |
| **P5 Messages codec** | anthropic-sdk-rs + 手写缺口（§4.4） | 4 |
| **P6 设置页** | 供应商 CRUD（协议 / logo / 余额类型）+ 模型列表 + 连通性测试 | 4 |
| **P7 余额适配** | `BalanceKind` 五值分发 + `Auto` 探测链 | 2 |
| **P8 缓存治理** | 前缀稳定性；`cache_control` 断点；比例化阈值 | 3 |
| **P9 输入框思考档位** | 5 档下拉，按模型 `thinking_levels` 置灰 | 2 |
| **P10 模型列表与切换** | 按 provider 分组；运行时切换下次生效 + 余额重查 | 2 |
| **P11 双协议验收** | 同一套行为用例跑两个协议 | 3 |
| **P3b 会话 JSONL 承载模型输出** | `signature` / `origin` 可选字段；旧 JSONL 回放测试；中途切换供应商的整文件回放测试；不新增表 | 2 |
| **F0 协议层只读盘点** | 逐个核对 `apps/cli/src/app_server.rs` 的方法实现与测试、`jsonl.rs`、TypeScript SDK；输出缺口清单，不改代码。依据 AGENTS §5.1 的方法面 | 1 |
| **F1+ 协议层补齐** | 按 F0 清单拆分：协议版本字段、DTO 与 UI DTO 分离、事件 replay 与断线续读、审批统一路由、SDK 覆盖。以现有 app_server 为基础，不另起协议引擎 | 待 F0 后估算 |
| **小计** | | **35 + F1+ 待定** |

---

### 9.1 执行状态（截至 2026-11-17）

| 批次 | 状态 | 证据 / 与原计划的偏差 |
|---|---|---|
| G0 收口在途改动 | 部分完成 | `f1434b1c` 修复 `canvas.rs` clippy；R012 远程终端改动未提交（执行记录无 R012 章节，无 RED/GREEN 证据）。AppCore 有 5 处既有 clippy 告警未处理 |
| P0 可测性验证 | 完成，采用 (a) | §10-1 有实测结论；async-openai 注入点的 `HttpRequestFactory` 为私有类型 |
| P1 中性内容层 + 五档映射 | 完成 | `b6774635`。`ThinkingLevel` 与 §1.3 全部 45 个单元格有测试；`map_thinking` 为唯一真源。**偏差**：未新增 `ContentBlock` 枚举，而是在现有 `ResponseItem::Reasoning` 上增加 `signature: Option<String>`（`serde(default)`，旧记录兼容）。`ReasoningEffort` 旧类型保留到 P9。**待实测**：§1.3 脚注 ³ 的理解为"照送原值"，需用真实供应商确认 |
| P2 协议表 + 工厂 | 部分完成 | `8376e0be`。`protocol::codec_for(WireMode)` 为唯一生产分发点，收口请求体编码与工具投影；审计测试约束 `client.rs` 生产段不再含 `WireMode::<variant> =>` 分支。**偏差**：现有协议是 `Responses` 与 `ChatCompletions`（生产中用于 DeepSeek chat 路径），而非方案写的 `Responses/Messages`；`Messages` 随 P5 加入。**残留分支**：`tokenizer.rs:91,96`（count_wire）、`client.rs` 的 `WireMode::validate_image_role` 内部 `match (self, role)`、若干 `!=` 判断（`client.rs`、`client/files.rs`），后续批次收口 |
| P3 数据模型 + 迁移 | 部分完成 | `549a82ba` V24 表结构与旧密钥导入。`32d1ff15` provider 保存（协议校验、revision 递增）、列出、删除（级联模型）、模型批量保存（重新拉取时保留激活状态）、激活开关、激活列表（需模型与 provider 同时启用）；10 项 RED→GREEN。未提交：`HarnessModelView` DTO（`fetch_models` / `list_models` 返回视图，持久层 `HarnessModel` 不跨 Tauri 边界）；Tauri 命令 7 个（`harness_providers_list` / `harness_provider_save` / `harness_provider_remove` / `harness_models_list` / `harness_models_fetch` / `harness_model_set_active` / `harness_active_models`）已注册到 `generate_handler!`。验证：`deepagent-app-core` 的 `harness_model_service` 8 项测试通过；`deepagent-persistence` 全部测试通过（221 + 9 + 5）；`cargo fmt --check` 通过；`src-tauri` `cargo check` 通过；app-core 与 persistence 的 lib clippy `-D warnings` 通过。**未做**：前端类型与设置页 UI、`ModelResolver`、旧 `settings.catalog` 的种子导入、视觉接入删除、替换写死的 DeepSeek 逻辑 |
| P3b 会话 JSONL 的 `origin` | 未开始 | — |
| F0 协议层盘点 | 完成（只读） | 见 9.2 |
| F1+ 协议层补齐 | 未开始 | 待 9.2 清单拆分 |

### 9.2 F0 盘点结论

- **已有 stdio JSON-RPC 服务**（`apps/cli/src/app_server.rs`，dispatcher 在 `:273-303`）。`thread/start`、`thread/resume`、`thread/read`、`thread/fork`、`thread/list`、`turn/start`、`turn/interrupt`、`turn/steer`、`approval/respond`、`tool/list`、`config/read` 均已实现并有测试。
- **缺口 1**：`turn/stream` 不是独立方法。流式输出通过 `turn/start` 之后的通知推送（`event_outbox.rs:115-118`）。AGENTS §5.1 列出了该方法名，需决定：补为方法，或在文档中明确为通知。
- **缺口 2**：`sandbox/status` 没有专门的单元测试，仅在 `method_routing_tests.rs` 中校验方法名存在；线上覆盖来自 `crates/deepagent-harness-sdk/tests/e2e_stdio.rs`。
- **缺口 3**：TypeScript SDK（`packages/sdk`）未封装 `thread/fork`、`sandbox/status`、`event/ack`。Rust 客户端（`crates/deepagent-harness-sdk/src/client.rs`）已封装全部方法。
- **缺口 4**：没有实时 replay。`event/ack` 只是连接内回执，不是持久游标；持久回放依赖 `thread/read` 的 `afterSequence` / `runAfterSequence`。
- **版本**：`PROTOCOL_VERSION = 1`（`crates/deepagent-harness-protocol/src/requests.rs:6`），`initialize` 校验它；只有 `thread.started` 携带该版本，其他事件类型没有版本字段。
- **DTO 分离已成立**：`crates/deepagent-harness-protocol/` 独立于 Tauri 的 DTO，桌面端未引用它。
- **审批路由已统一**：CLI 与桌面都调用 `ChatService::resolve_approval_scoped`，最终进入 `RunCoordinator::resolve_approval_scoped`（`run_coordinator.rs:371`）。
- **未核实**：`apps/cli/src/app_server/connection.rs`、`terminal_audit.rs` 未通读；`turn/steer` 的运行时路径只部分核实。
- **注意**：子 agent 的盘点依据仅为静态阅读，未运行测试。

## 10. 待定

1. **P0 若 SDK 注不进自定义 transport，接受哪种？**
   - (a) SDK 只做类型与序列化，HTTP 仍走本系统 `HttpTransport` —— 离线 mock 全保留
   - (b) SDK 全接管，测试改成对 SDK 层 mock —— 失去对 socket 层的控制

   **P0 结论（2026-11-17 实测）：采用 (a)。**
   - async-openai 0.43.0（本地借鉴副本与 crates.io 最新版一致）：`with_http_service` 需要 `middleware` feature，其 bound 的 `HttpRequestFactory` 位于私有模块 `executor` 且未 re-export。仓库外探针两种写法（具名 `Service` 实现、`service_fn` 闭包推断）均报 `E0603` / `E0282`，无法从外部实现自定义 transport。强行接入需要 fork 或 patch，不采用。
   - async-openai 的 Response 类型是 `reqwest::Response`（reqwest 0.13），本仓库 `deepagent-models` 使用 reqwest 0.12，两者类型不相容。
   - anthropic-sdk-rs 0.74.0（本地副本）：唯一注入点是 `ClientOptions.http_client: Option<reqwest::Client>`（`client.rs:315`），同样是 reqwest 0.13，没有可替换的 transport 边界。
   - 因此两个 SDK 都只能作为"类型与序列化"使用；HTTP、SSE 解析、取消、UTF-8 与请求体预算仍由本系统 `HttpTransport` 负责。离线 mock 全部保留。
   - 后续若要用 `reqwest::Client` 注入，需先统一 reqwest 版本，这是独立的依赖升级决策，不在本批次内完成。

2. **旧测试「全丢」的边界。**
   `reqwest_utf8_audit.rs`（UTF-8 逐字节边界）、`client_body_budget_tests.rs`（请求体预算）、`client_input_identity_tests.rs` 测的是**本系统自己的传输层与投影层**，不是 async-openai 的 DTO。
   建议：DTO 形状测试全丢，传输层与投影层测试保留并改写。

3. **Step 与 MiMo 的档位阶梯**：官方文档未查到完整阶梯，暂按「思考开关」处理，需实测确认。

4. **DeepSeek-V4 Pro 能否关闭思考**：DashScope 文档只给了 `high` / `max`，未说明关闭方式，需实测。
5. **思考签名能否跨模型回放**：同一会话中途从 A 模型切到 B 模型，A 产出的 `Thinking.signature` 在 B 上是否被接受，未查证。若不被接受，需要在切换时降级为普通文本，并在 `origin` 中记录来源以便判断。需实测。
6. **`costs` 表是否可由 `UsageRecorded` 重建**：若可以，费用可降为派生数据；若不可以，保留为账本。需先核对 `cost_store` 的写入来源。
7. **JSONL 是否默认压缩**：本系统默认 zstd，Codex 与 Claude Code 为纯文本。建议保持 zstd 为默认，增加 `compression: none` 配置项供外部读取。请确认。
8. **删除"系统视觉的大模型接入"后，图片的处理方式**（已决定）：`VisionSettings` 整体删除（设置页、配置项、`VisionMode` 与图片准备中的视觉模型路径）。不再为无视觉模型做降级。当前模型不支持图片时，用户发送图片后直接回复"当前模型不支持视觉"，不做任何模型侧的图片转述。
9. **压缩阈值公式**（已决定）：保留 `deepagent-context/src/policy.rs` 的 `autocompact_threshold_tokens`，不引入 §3.2 的 0.78/0.95 比例公式；每个模型的 `context_window` / `output_limit` 作为输入传入。

---

## 附录：证据来源

| 结论 | 来源 |
|---|---|
| pi-ai 7 档 | `借鉴/deepseek-harness/packages/llm/llm-pi-ai/src/catalog.ts` `THINKING_LEVEL_GATE` |
| Codex 6 档含 `ultra` | `借鉴/cockpit-tools/src/types/codex.ts:60` |
| Qwen3.8 三档与默认 xhigh | Alibaba Cloud Model Studio DashScope API 参考 |
| DeepSeek-V4 / GLM-5.2 两档 | 同上（明确写出 low/medium→high、xhigh→max） |
| GLM-5.3 / Kimi K3 强制思考 | 同上（`enable_thinking` 仅接受 true，false 请求失败） |
| Kimi K3.1 Pro 四档 | 同上（max 默认，low/high/medium 可选） |
| GLM-5.3 档位 | OpenRouter 模型能力表（low/high/max） |
| anthropic-sdk-rs `Usage` 字段 | `借鉴/anthropic-sdk-rs/src/resources/messages/types.rs:104-138` |
| 流式 usage 覆盖式累加 | `借鉴/anthropic-sdk-rs/src/lib/message_stream.rs:453-468`、`借鉴/deepseek-harness/.../translate.ts:35-44` |
| 协议表 + 工厂 | `借鉴/deepseek-harness/packages/llm/llm-pi-ai/src/provider.ts:48-53` |
| 余额分发与探测链 | `借鉴/cockpit-tools/src-tauri/src/commands/codex_model_provider_commands.rs:1865-2178` |
| 本系统会话 JSONL 与 zstd 帧 | `crates/deepagent-persistence/src/session_files.rs:15-17,107,836,1173-1192` |
| 本系统 V18 事件迁出 SQLite | `crates/deepagent-persistence/src/migrations.rs:343-359` |
| Codex rollout 行与 `RolloutItem` 变体 | `借鉴/codex/codex-rs/history/src/lib.rs:211-227`；`借鉴/codex/codex-rs/rollout/src/recorder.rs` |
| Claude Code 会话 JSONL 路径与批量写 | `借鉴/claudecode/restored-src/src/utils/sessionStorage.ts:203-204,567` |
| deepseek-harness 会话 JSONL 布局与压缩默认值 | `借鉴/deepseek-harness/packages/session/session-persistence-jsonl/README.zh.md:48,57-70,77` |
