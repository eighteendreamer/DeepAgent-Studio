# Harness 多协议与模型注册表改造方案

日期：2026-11-17
状态：开发中；完成状态以执行记录和实际验证为准
基线：HEAD `738fa508`

## 0. 目标

1. 本系统自写的 Responses 协议代码整体替换为 `async-openai`，针对手写实现的旧测试弃用。
2. 新增模型注册表：供应商 + 模型两层，协议、URL、Key、上下文窗口、输出上限、思考档位全部入库。
3. 定义本系统 harness 的中性输入/输出参数，与 `async-openai`、`anthropic-sdk-rs` 一一对应；SDK 缺失处手写补齐。
4. 参照 deepseek-harness 的「协议表 + 工厂分发」组织协议层。
5. 提升缓存命中率：稳定前缀设计。
6. 余额查询改为按供应商类型分发，不再写死 DeepSeek 官方接口。
7. 设置页可自行添加供应商、选协议、拉取模型入库；对话窗口按供应商分组选模型。
8. 运行中切换模型/供应商，在下一轮用户消息生效；当前 turn 的工具调用与续写保持原模型。
9. harness 协议层统一收敛到现有 `apps/cli/src/app_server.rs`（stdio JSON-RPC）与 `jsonl.rs`，不新建第二套协议引擎（F 批次，见 §9）。

### 0.1 架构修正（2026-11-17，用户确认）

1. **无限画布与 harness 分属两套，互不合并。** 画布注册表（`canvas_provider_service.rs`）保持不动；harness 使用独立的注册表。原方案"扩展画布注册表为共享真源"作废。
2. **新增主设置页"模型"**：专门管理 harness 的模型，用于接入更多平台的大模型 API。用户在此添加供应商（协议、base_url、密钥、余额类型、logo）并拉取、激活模型。
3. **激活的模型进入会话窗口的模型列表**，按供应商分组。思考强度统一五档，按路由能力转换并合并等价选项（2026-11-18 最新决定，见 §1.6）；底部"余额"按供应商的 `balance_kind` 查询并显示。
4. **替换写死的 DeepSeek 逻辑**：
   - `DEEPSEEK_BASE_URL` 不再作为主对话的固定地址；
   - 删除旧 `ModelCatalog`、`auto_select` 和启动时自动选模；`ModelDiscovery` 仅返回模型元数据，由用户在"模型"页手动拉取、激活和选择；
   - `fetch_balance` 按供应商类型分发（P7），不再固定 DeepSeek 路径；
   - 2026-11-20 用户决定：删除旧模型配置及其导入。仅使用已经保存的新注册表；不再从 `deepseek_api_key` 或旧模型目录创建 provider/model 条目（§3.1）。
5. **删除设置页"系统视觉的大模型接入"**（`VisionSettings` 的设置 UI 与配置项）。注意：该配置还被图片准备、`VisionMode` 的路径引用，删除后向非视觉模型发送图片的行为需用户确认（见 §10-8）。
6. 方案 §3（`harness_providers` / `harness_models` 两张表）保留，它与画布注册表无关，是 harness 唯一的模型注册表。
7. **单一激活模型，取消 Chat / Reasoner 双角色**（2026-11-17，用户确认）。
   - 旧代码里的 `reasoner_model` 不是“开启思考时切换的模型”：思考强度按模型取值（`effective_reasoning_effort(&model)`），与角色无关。它唯一的运行时作用是主模型返回 `Overloaded` 时一次性切换到 reasoner（`model_agent.rs` 的 `fallback_model`）。
   - 决定：**删除过载自动切换备用模型**。过载立即以错误返回，零自动重试，也不换模型（2026-11-17 再次确认）。`ModelResolver` 只解析一个激活模型；不保留 `fallback_model` 字段、`ModelRole::Reasoner` 与 `with_fallback_model` 的运行时调用路径。2026-11-20 起，旧目录、双角色配置与兼容回退一并删除。
   - 代价：失去过载兜底。变更说明须写明该行为变化；若后续需要兜底，按显式设置重新设计，不沿用 reasoner 角色。
   - 原 `settings.catalog.chat_model` 种子导入方案已丢弃；没有新注册表选择时要求用户明确配置、激活和选择模型。

---

## 1. 思考等级

> **决策更新（2026-11-18）：以用户最新要求为准，改参考 Tianshu-harness，统一 `off/low/medium/high/max`，由用户控制。§1.2—§1.4 及 §1.5 的七档／供应商默认策略均丢弃，仅保留历史。当前规则见 §1.6。**

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

### 1.2 收敛到 5 档（已丢弃）

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

### 1.3 跨协议映射表（已丢弃）

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

### 1.4 每模型能力声明（已丢弃）

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

2026-11-18 用户纠正：所有模型默认具备思考能力，未声明能力时采用
`off/low/medium/high/max` 五档并允许关闭；最低档在界面显示“关闭”。
此前“必须手工配置、配置前全部置灰”的回答作废。已显式保存的模型能力仍优先，
不根据模型名称推断能力；显式空列表与坏数据不能当作缺省值放行。
这里的“预设”指方案设计，并不要求新安装默认选中关闭；保持现有初始档位和已有用户保存的选择。

---

### 1.5 历史规则：按 deepseek harness（已丢弃）

依据：`借鉴/deepseek-harness/packages/llm/llm-pi-ai/src/catalog.ts`、`adapter.ts`，
`packages/client/ui-model-selection/src/client/ModelSelect.tsx`，
`packages/client/ui-settings-models/src/client/ModelReasoningEfforts.tsx`，
以及 DeepSeek 专用的 `packages/llm/llm-deepseek/src/model-info.ts`。

- 通用中性档位为 `off / minimal / low / medium / high / xhigh / max`。`ultra` 可作为供应商实际参数值，例如 `max: "ultra"`，不新增第八个中性档位。
- 模型声明 `reasoning_efforts`（JSON DTO 为 `reasoningEfforts`）：省略表示继承已知模型声明；`false` 表示不提供思考控件；映射只开放实际声明的档位，缺少的键即不支持。
- 示例：`{"off":null,"low":"low","max":"ultra"}`。只有 `off` 可为 `null`，表示省略 effort；`off:"none"` 表示显式发送 `none`。Messages 的关闭仍由协议编码为 `thinking.type=disabled`，不将关闭值当作其正向 effort。
- 映射至少含一个非 off 档位。空映射、空值、非 off 的 null、未知中性档位均报错。请求不做 clamp，不依据模型家族或名称子串猜测映射。
- DeepSeek 专用参考入口提供 `off / low / high / max`；通用入口按模型实际声明开放七档中的一部分。当前只内置精确 DeepSeek 模型标识的已知声明，不给未知第三方模型虚构能力。
- 未知模型没有声明时仅显示“供应商默认”，不发送 effort；这不保证关闭供应商自身默认思考。原“所有模型缺省五档并允许关闭”设计一并丢弃。
- 选择器沿用参考界面的默认项规则：模型已声明默认档位时直接选中该档位，不额外列“供应商默认”；有可选档位但未声明默认值时才提供默认选项。无思考能力声明时不提供档位控件。
- 切换模型建立新的 `{provider, model, reasoningEffort?}` 选择，旧档位不随模型迁移；未选档位时采用模型/供应商默认。显式选中不支持的档位直接报错，不修改已经保存的选择。
- 现有 `harness_selection/chat` 文档承载可选 effort，与模型路由一起持久化，无新增数据表；当前 turn 及工具续写继续使用接纳时冻结的选择。
- 旧 `thinking_levels / thinking_distinct / thinking_disable` 只在读取边界兼容。编辑器保留其有效映射，保存时统一写新声明；不再写“等价档位”或旧家族表。

### 1.6 当前规则：参考 Tianshu-harness 的五档与能力转换

依据：`借鉴/Tianshu-harness/src/api/provider.ts` 的 `resolveCapabilities`、
`resolveWireEffort`、`normalizeReasoningEffort`、`resolveEffortChoices`，以及
`factory.ts`、`anthropic-client.ts` 的协议投影。参考其边界与机制，不复制业务代码。

- 中性选择只展示 `off / low / medium / high / max`。`minimal/xhigh/ultra` 可以是实际 API 参数值，不新增 UI 档位；历史 `minimal/xhigh` 选择在读取边界归一为 `low/max`。
- 模型设置中的显式映射优先于发现元数据与参考能力配置；供应商地址、模型标识和所选协议共同决定传输形态。转换只发生在 `deepagent-models`，前端不自行推断供应商字段。
- 合并实际等价选项。例如 `medium→high` 时保留“高”；`off→low` 时保留“低”，不假称已经关闭思考。
- 不提供“供应商默认”选项。缺省采用应用明确选择的档位（优先高；未支持高时取实际可用的正向档位），请求明确发送其参数；用户可在模型设置与胶囊中修改。显式关闭与未选择不能混淆。
- `reasoning_efforts` 保留为能力映射存储边界，`reasoning_default` 表示应用初始档位；`thinking_format` 表示 `effort_only/enabled/adaptive/budget` 协议形态。它们均存入既有模型 capabilities JSON，不新建注册表。
- Responses 写 `reasoning.effort`；Chat Completions 写 `reasoning_effort` 并按能力决定是否附加 thinking；Messages 按能力写 `thinking` 与 `output_config.effort`，或显式 token 预算。关闭时使用对应协议的关闭表达。
- Messages 的明确能力优先；DeepSeek 官方地址包含 Claude 别名时仍按 DeepSeek 投影。已声明支持 adaptive/effort 的 Claude 代际使用该通道，其余 Messages 使用预算。预算换算与界面去重共用计算，低输出上限造成的等价预算也合并。为避免天枢固定 low=8192 在小输出窗口超过 high，本地设计将 low 限为输出上限的 1/8 与 8192 中较小值，再遵循 1024 最小预算及严格小于输出上限的限制。
- 当前 Claude 5 系列及 Fable 5.2 的 max 不降为 high；官方仅支持始终思考的型号将 off 合并到 low，不显示虚假的关闭选项。能力事实以 2026-11-18 核对的 Anthropic thinking/effort 文档为准，模型设置的显式声明可覆盖兼容网关差异。
- 未知兼容端点的正向 effort 透传属于**本地设计**：提供显式选择，不能保证网关真的执行。允许用户覆盖映射／形态、或显式关闭控件；不能用接口 200 冒充模型确实区分强度，验收同时记录请求参数和响应回显。
- 不支持的选择在发请求前报错。用户已经保存的选择与路由一起持久化；当前 turn 及其工具续写继续冻结接纳时快照，下一轮用户消息生效。
- 旧 §1.5 中“未知模型只有供应商默认”“统一七档”“不合并选项”不再执行。该更新不恢复 §1.3 的旧映射表。


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
    protocol      TEXT NOT NULL,              -- 'responses' | 'chat_completions' | 'messages'
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
    capabilities    TEXT NOT NULL DEFAULT '{}',        -- reasoning_efforts 等
    status          INTEGER NOT NULL DEFAULT 1,
    updated_at      INTEGER NOT NULL,
    UNIQUE (provider_id, model_name)
);

CREATE INDEX idx_harness_models_provider ON harness_models(provider_id, status);
```

### 3.1 api-key 不落这张表

现有 `secret_records(name, ciphertext, nonce, key_version, updated_at)`（`migrations.rs:214`）已做认证加密。`api_key_ref` 只是指向它的逻辑名。

**2026-11-20 决定：原旧密钥迁移与默认 Chat 种子导入方案丢弃。** 不从旧配置复制、激活或选择任何模型。V26 删除旧目录、全局思考选择字段、凭据生命周期文档及种子导入标记；已经存在的新供应商、模型、选择和所引用的加密凭据保持不变。已有 `api_key_ref` 是不透明标识，不因名称含旧字符串而改名或复制密文。

首次打开数据库时内置 Layso 和 DeepSeek 供应商资料。初始化向导只对用户选定的供应商校验并保存 API Key：复用已配置路由的认证 `GET /models`，合法的空列表也可通过，不导入、激活或选择模型。初始化完成后允许全局对话选择为空；发起对话前再通过设置启用模型，并从对话模型选择器选中。旧初始化、刷新、双角色选模与全局密钥/余额入口已删除。

V27 为 `deepagent.db` 的全部业务表统一提供 `created_at`、`updated_at`，单位为 Unix 毫秒；有历史时钟的按原值回填，其余按迁移时刻补齐。新字段由数据库触发器维护，已有时间戳继续由现有 writer 维护。SQLite 系统表及 FTS 虚拟表/影子表由 SQLite 管理，其业务源表包含时间戳。具体迁移、实际数据库校验与验证边界见 [清理报告](reports/2026-11-20-legacy-model-cutover-and-timestamps.md)。

### 3.2 `max_tokens` 的语义

**它是单次请求的输出上限，不是整个会话。**

一次请求里模型最多生成这么多 token 就停（`stop_reason: max_tokens`），下一轮是新计数。但它通过历史间接影响会话占用 —— 128K 输出跑 3 轮，下一轮输入就多了 384K。

压缩沿用现有 `deepagent-context/src/policy.rs` 的 `autocompact_threshold_tokens`
（见 §10-9 已确认决策），由选中模型提供 `context_window` / `output_limit`。
早期 0.78/0.95 比例公式作废，不新增第二套压缩阈值。

Anthropic 的 `max_tokens` 是**必填**字段，每次请求都带选中模型的输出上限。新发现的 Messages 模型若未声明上限，注册表补十进制 `128000`；显式声明的上限和已保存的配置保持原值。Layso 的 Claude 验收路由实测接受 `128000`、拒绝 `128001`（H030）。

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

参考 DeepSeek Harness 的静态协议注册表。以下摘自当前生产实现（不是另建一套工厂）：

```rust
static PROTOCOLS: &[(WireMode, &dyn WireCodec)] = &[
    (WireMode::Responses, &RESPONSES),
    (WireMode::ChatCompletions, &CHAT_COMPLETIONS),
    (WireMode::Messages, &MESSAGES),
];

pub(crate) fn codec_for(mode: WireMode) -> &'static dyn WireCodec {
    PROTOCOLS
        .iter()
        .find(|(protocol, _)| *protocol == mode)
        .map(|(_, codec)| *codec)
        .expect("every WireMode must be registered in PROTOCOLS")
}
```

各 codec 负责请求准备、协议编码和对应的流解析器；SDK 负责类型与序列化，HTTP/SSE、取消与计量由已有客户端统一处理。
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
| 压缩阈值 | 沿用现有 `autocompact_threshold_tokens`，见 §3.2 |

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
下一轮用户消息 admission → resolver 重读 → 新 provider + 新 model + 新 key
```

要改的是 `bind()`：从「读 settings 的 `deepseek_api_key`」改成「读 model 行的 `provider_id` → provider 行的 `api_key_ref`」。

`bind()` 只解析一个激活模型，不解析备用模型。主模型返回 `Overloaded` 时直接报错，不切换模型（见 §0.1-7）。

生效边界（2026-11-17 用户确认）：同一个 turn 内的工具调用、重试和模型续写保持冻结快照；
不能在每次工具续写的 prepareCall 重读选择。下一轮用户消息才解析新选择。

切换时必须发生：
- 余额按新 provider 重查
- 缓存前缀必然失效（新模型新前缀），这是预期行为
- running turn 不中断

---

## 9. 批次规划

| 批次 | 内容 | 会话 |
|---|---|---|
| **P0 可测性验证** | 两个 SDK 能否注入本系统 `HttpTransport`；不能则定替代方案 | 1 |
| **P1 中性内容层** | `ContentBlock` + `Thinking.signature`；旧映射表已丢弃，思考规则改按 §1.6 | 2 |
| **P2 协议表 + 工厂** | `Protocol` 枚举、`WireCodec` trait、`match` 收口一处 | 2 |
| **P3 数据模型** | 两张新表 + 迁移；`ModelResolver` 单点解析（单一激活模型，无 Reasoner 角色，见 §0.1-7） | 3 |
| **P3b 会话 JSONL 承载模型输出** | `signature` / `origin` 可选字段；旧 JSONL 回放测试；中途切换供应商的整文件回放测试；不新增表 | 2 |
| **P4 Responses codec** | async-openai 接入，替换手写实现 | 4 |
| **P5 Messages codec** | anthropic-sdk-rs + 手写缺口（§4.4） | 4 |
| **P6 设置页** | 供应商 CRUD（协议 / logo / 余额类型）+ 模型列表 + 连通性测试 | 4 |
| **P7 余额适配** | `BalanceKind` 五值分发 + `Auto` 探测链 | 2 |
| **P8 缓存治理** | 前缀稳定性；`cache_control` 断点；复用既有压缩阈值 | 3 |
| **P9 输入框思考档位** | 天枢五档；按路由能力转换与合并，不提供供应商默认，见 §1.6 | 2 |
| **P10 模型列表与切换** | 按 provider 分组；运行时切换下次生效 + 余额重查 | 2 |
| **P11 三协议验收** | 同一套行为用例跑 Responses、Chat Completions、Messages | 3 |
| **P3b 会话 JSONL 承载模型输出** | `signature` / `origin` 可选字段；旧 JSONL 回放测试；中途切换供应商的整文件回放测试；不新增表 | 2 |
| **F0 协议层只读盘点** | 逐个核对 `apps/cli/src/app_server.rs` 的方法实现与测试、`jsonl.rs`、TypeScript SDK；输出缺口清单，不改代码。依据 AGENTS §5.1 的方法面 | 1 |
| **F1+ 协议层补齐** | 按 F0 清单拆分：协议版本字段、DTO 与 UI DTO 分离、事件 replay 与断线续读、审批统一路由、SDK 覆盖。以现有 app_server 为基础，不另起协议引擎 | 待 F0 后估算 |
| **小计** | | **35 + F1+ 待定** |

---

### 9.1 执行状态（截至 2026-11-20）

| 批次 | 状态 | 证据 / 与原计划的偏差 |
|---|---|---|
| G0 工程门禁 | 格式与完整 Clippy 通过；完整测试未通过 | H037 完整 workspace 为 5799 passed / 36 failed / 132 ignored，9 个目标失败；其中注册表 2 项修正后定向 10 passed，未重跑完整命令。独立提交源码 models/http 为 431 passed / 0 failed / 6 ignored，HTTP/all-targets 严格 Clippy 通过；桌面前端构建及三协议原生 UI 对话通过。完整 Clippy 最终退出 0；版本边界和剩余失败详见 [H037](reports/2026-11-20-p2-p11-ui-follow-up.md)。 |
| P0 可测性验证 | 已记录结论，采用 (a) | SDK 只承担类型与序列化，HTTP/SSE/取消/预算继续复用。async-openai 0.43.0 启用 response-types 与 chat-completion-types，Messages 使用已固定的 anthropic-sdk。H037 补 Chat 请求类型验证。 |
| P1 中性内容层 + 思考声明 | 旧映射表已丢弃；按 §1.6 改造验证中 | `ResponseItem::Reasoning.signature` 保留；原 `map_thinking` 接入项不再执行。新声明、模型选择与三协议实际发送值共用解析边界，验证结果见执行记录。 |
| P2 协议表 + 工厂 | 三协议统一分发；Chat SDK 请求与具名工具补齐 | Responses / Chat Completions / Messages 共用协议表、传输与取消入口。H037 接入 Chat SDK DTO，具名工具显式关闭并行；两条真实具名请求通过，三协议新版桌面 UI 对话通过。详见 [H037](reports/2026-11-20-p2-p11-ui-follow-up.md)。 |
| P3 数据模型 + 迁移 | 新注册表单一路由；旧链路清理验证中 | V24 表、CRUD、激活列表、AppCore DTO、Tauri 与 TS API 已落地。2026-11-20 起丢弃 `b1de6059` 的旧 Chat 种子导入及所有旧目录/别名回退；模型选择仅从 SQLite 新表读取。V26 清理旧配置；V27 补全业务表时间戳。结果见 H038 和清理报告。 |
| P3c 过载与备用模型删除 | 已提交，相关回归通过 | `10234777`：过载统一不可重试，明确 503 过载也立即报错；删除运行时 fallback 路径。models lib 240 passed；runtime lib 493 passed / 2 ignored；H55 4 passed；内核 H144 7 passed；采样、计费/预算、输出上限、图像模式及模型快照串行回归通过；真实 HTTP 1 项、CLI 2 项通过。H004 已接入单一 resolver。 |
| P3b 会话 JSONL 的 `origin` | 已实现，回归通过 | `ResponseItemAppended.origin` 为可选 `ModelOrigin`，由真实模型运行链写入；旧 JSONL 无 `origin` 仍可读。core 往返 2 项、persistence 来源回放 1 项、persistence 全量 221 项、session 27 项、AppCore 注册表真实运行 7 项通过。跨供应商思考签名是否需要降级仍待实测，当前只保留签名与来源，不擅自改变回放语义。 |
| P4 Responses codec | 已完成（按 P0 类型与序列化边界） | async-openai 0.43.0 的 CreateResponse、消息、工具调用/结果、图片、工具、text 与 tool_choice 类型进入生产序列化；DeepSeek 扩展显式保留。现有 HTTP/SSE、语义组装、取消与 48 MiB 预算复用。验证见 H034。 |
| P5 Messages codec | 图片、原生请求头已补齐；真实 Messages 11 项通过 | H035 的 SDK 请求、流解析、签名、用量、图片与统一鉴权继续复用。H036 对 Layso Claude 实测文本、历史、工具续写、五档发包、图片、取消及缓存读取均通过；不能外推为所有 Anthropic 端点验收，见 [H036 验收报告](reports/2026-11-19-p11-f0-f1-acceptance.md)。 |
| P6 设置页 | 基础界面与后端已实现；流程待验收 | `ModelsSettings.tsx` 已接供应商 CRUD、模型拉取/激活和配置编辑。H015 保留已存配置，H016 支持协议/容量/思考编辑，服务 19 + resolver 12 项通过；H017 设置页迟到请求回归通过。完整原生 UI 流程仍待验收。 |
| P7 余额 | 已修复已知解析/UI 缺口；真实 API 待验收 | H009 修复未知形状误判无限、分币换算、display 单位、缺失金额伪造零与错误跨供应商回退；两处 UI 共用查询与金额格式化并丢弃迟到结果。models lib 256、前端余额专项 9 passed。完整桌面与真实余额 API 待验收。 |
| P8 缓存 | 稳定前缀与 Messages 断点已验证；未完全复现 Harness 缓存策略 | 工具稳定排序，稳定 instructions 与动态环境分开，已完成历史断点避开思考签名块；DeepSeek 官方继续自动缓存。H037 真实观察 Responses 0/11250、Chat 11008/11250、Messages 23952/23960，不承诺普遍命中；cacheRetention 与签名前缀摘要持久绑定仍未实现。详见 [H037](reports/2026-11-20-p2-p11-ui-follow-up.md)。 |
| P9 输入框思考档位 | 天枢五档控件验证中 | H012/H018 为历史验证，不证明新契约。模型编辑器支持自动匹配/无控件/自定义映射、传输形态与初始档位；未知兼容模型明确透传正向 effort，切换模型不继承旧 effort，当前验证见 H029。 |
| P10 模型列表与切换 | 已接入；桌面手动验收待完成 | Composer 消费激活模型，按 provider 分组；选择写入 harness 注册表，跨供应商触发余额刷新；当前 turn 冻结旧模型，工具续写保持旧模型，下一轮用户消息重新 admission。 |
| P11 产品验收 | 三协议新版原生 UI 通过；跨协议补验 3/3；全量门禁另列 | H037：通过脚本输入并发送，三协议 HTTP 200、运行 completed、零 JS 异常；Responses/Chat 指定工具名已修复且真实通过。当前 Claude Fable→Responses 连续三轮完成，其中一轮带签名；保留旧 Sonnet 路线失败记录，不外推全部模型与 SessionStore/origin 矩阵。详见 [H037](reports/2026-11-20-p2-p11-ui-follow-up.md)。 |
| F0 协议层盘点 | 当前 16 个方法及关键边界已复核 | 重新核对方法生产/消费方、连接退出、steer、持久终态、审批、ACK 与独立游标；已有三项思考配置断言冲突未掩盖。见 9.2 和 [H036 验收报告](reports/2026-11-19-p11-f0-f1-acceptance.md)。 |
| F1+ 协议层补齐 | 本轮版本、SDK 持续增量回放及专项通过；整仓门禁未通过 | 全部出站事件统一 protocolVersion=1，Rust/TS 校验入站版本并兼容旧 v1；SDK 复用 thread/read 做 250ms 持续拉取和 checkpoint 恢复，非服务端推送补发。补 sandbox/status 专项与真实 CLI 重连去重，见 [H036 验收报告](reports/2026-11-19-p11-f0-f1-acceptance.md)。 |

### 9.2 F0 复核结论（2026-11-19）

- 现有协议唯一方法声明包含 16 项：initialize；thread/start、resume、list、read、fork、archive、unarchive；turn/start、interrupt、steer；approval/respond；event/ack；tool/list；config/read；sandbox/status。Rust 与 TypeScript 均有调用入口，分发继续复用现有 app_server。
- 用户决策保持：流式输出沿用 turn/start 后的通知，不新增 turn/stream。
- 全部 CLI JSONL 与 app-server 出站事件统一携带 protocolVersion=1。SDK 兼容旧 v1 的无字段事件，明确错误版本拒绝交付并报错。
- event/ack 是连接内已经 flush 的通知回执，不是持久游标，也不触发补发。持久恢复使用 sessionAfterSequence 与每个 run 的 runAfterSequences；续页 cursor 不与新过滤器混用。
- Rust/TS 新增持续增量回放入口，复用 thread/read；追平后每 250ms 读取一次。它是本地 SDK 拉取设计，不是服务端主动推送的历史追赶；不自动重连、重发 turn 或审批。
- checkpoint 绑定线程并分开保存会话/run 高水位；验证行身份、有序性和整数边界。真实 CLI 验证追加、重启和断点恢复不重复交付；可变 action/approval/artifact/graph 快照仍用 thread/read 查询。
- sandbox/status 已补专门测试：初始化门禁、实际 host/协调器能力、查询无准入副作用。
- connection.rs、terminal_audit.rs 及 steering 关键执行和测试边界已复核：单一 writer、ACK/flush、10s EOF 清理、单次持久替换 claim、真实 shell 副作用和持久终态；继续使用原内核、存储和取消树。
- 机器协议 DTO 独立于 Tauri UI DTO；审批继续通过 ChatService::resolve_approval_scoped 进入既有协调器。
- 本轮证据与未通过门禁详见 [H036 验收报告](reports/2026-11-19-p11-f0-f1-acceptance.md)。三项思考配置旧断言仍失败，不能宣称 config/read 与全仓门禁全绿。

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
10. **过载兜底与 Reasoner 角色**（已决定：A，删除）：删除过载自动切换备用模型，过载直接报错；取消 `ModelRole::Reasoner` 在运行时的使用。依据与影响见 §0.1-7。

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
