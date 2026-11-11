# 天枢（Tianshu-harness）全项目目录导读

> 用途：DeepAgent Studio 团队对 `借鉴/Tianshu-harness` 的只读架构地图，供后续问答快速定位。
> 取证日期：2026-11-11 · 对象版本：package.json 实测 **3.37.4**（`docs/architecture-overview.md` 头部仍标 v3.31.0，文档滞后于代码）。
> 路径约定：除特别注明外，本文所有路径相对 `借鉴/Tianshu-harness/`。
> 行号约定：带行号的锚点均为主代理本轮 grep/Read 亲自核验；未带行号的机制描述来自只读检索代理报告，引用前建议复核。
> 官方文档站 <https://tianshuharness.com/docs> 是轻量快速开始入口（见 §6.3）；深度文档全部在本仓 `docs/`（1035 篇）。

---

## 1. 项目定位与六层架构

全功能终端编程智能体运行时。CLI 主命令 `tianshu`（兼容别名 `rivet`），Node.js 24（engines 钉 24.18.0）+ TypeScript strict + ESM + node:test。开发代号曾用 **Rivet**，仓内大量 `.rivet/` 运行时目录、旧文档均源于此。

三大工程承诺（`docs/architecture-overview.md`）：

1. **CVM 认知虚拟机** — 每个 turn 流经五阶段 RuntimeHookPipeline，配 CognitiveLedger / AdvisoryBus，在副作用落地前检测纠偏验证债、空心完成、doom-loop。
2. **Prefix-cache-first 提示词引擎** — 冻结前缀 + 字节稳定 appendixDelta + 仅用户边界压缩，把前缀缓存命中率做成一级成本指标（官方宣称稳态 95–99%）。
3. **16 星域纪律** — 可切换认知姿态（方法论提示词 / 工具白名单 / 决策与勇气阈值），不是角色扮演皮肤。

六层架构：

```text
Layer 0 表面    TUI(纯ANSI src/tui/engine/) · Tauri桌面(desktop/，闭源不在本仓)
                VSCode插件(vscode-extension/) · Headless(tianshu -p) · HTTP/SSE sidecar(src/server/)
Layer 1 循环    src/agent/ — AgentLoop → TurnOrchestrator → 工具执行 → 收尾
                RuntimeHookPipeline(5阶段) · Coordinator/Team/Council · DeliveryGate
Layer 2 认知    src/context/ + src/memory/ — ClaimStore · TaskContract · PressureMonitor
                UnifiedMemory · Antibody · Stigmergy
Layer 3 提示    src/prompt/ + src/compact/ + src/cache/ — PromptEngine(static冻结+volatile+appendixDelta)
                边界压缩 · CacheAdvisor
Layer 4 工具    src/tools/ + src/repo/ + src/search/ + src/lsp/ — kernel≤26工具(官方宣称)
                Meridian图 · Physarum · 混合检索 · MCP · LSP
Layer 5 模型    src/api/ + src/model/ + src/auth/ — OpenAI兼容/Anthropic/Codex OAuth/Gemini
                能力卡 · 自适应路由 · 故障转移
```

规模口径：

| 口径 | 数值 | 来源 |
|------|------|------|
| 非测试 `.ts` 源文件 | 1709 个 / 约 33.9 万行 | 本轮实测（排除 `__tests__/` 与 `*.test.ts`） |
| 官方 overview 宣称 | 约 1475 文件 / 29.5 万行，测试 21514 用例 | `docs/architecture-overview.md`（2026-07-26 快照，已偏旧） |
| 最大模块 | `src/agent/` 594 个非测试文件 | 本轮实测 |
| runtime hook 模块 | 官方宣称 75 个，默认会话激活约 18+ | `src/agent/hooks/` 实测 76 条目（含 `__tests__/`） |

---

## 2. 顶层目录速查

| 路径 | 职责 |
|------|------|
| `src/` | 全部内核源码（40 个子系统，见 §3） |
| `docs/` | 1035 篇文档：用户手册、架构、plan/spec/analysis、changelog、 releases（见 §6） |
| `plugins/` | 示例插件：hello-world、office-docx/excel/pdf/ppt、design、tianshu-imbridge |
| `prompts/` | **遗留**英文 Rivet 模板（base.md + tools/*.md），未发现运行时引用 |
| `runtime-assets/` | 内置技能（brainstorming、visual-acceptance）+ cli-harness-sop.md，随 dist 解析 |
| `native/` | Windows Job Object 助手 `job-launch.c`（杀 nohup 逃逸的后台子孙进程）+ supervisor 头 + 构建 bat |
| `benchmark/` | 评测任务 JSON 题库（domains/、tasks/） |
| `scripts/` | 构建、测试 runner（run-node-tests.ts）、门禁/verify、公开仓同步、docs-index 生成器 |
| `vscode-extension/` | 开源 VS Code/Cursor 插件：sidecar 客户端 + webview-ui + E4 客户端工具委托 |
| `website/` | 官网已拆分独立仓，此处仅剩 README/DESIGN/index.html 残留 |
| `completions/` | shell 补全脚本 |
| `config.example.json` / `.toml` | 配置示例 |
| `AGENTS.md` / `CLAUDE.md` | agent 导航 / 构建约定与 Known Constraints（缓存硬约束清单） |
| `star.md` | 星图叙事（16 星域命名体系） |
| `desktop/` | **不在本仓**（闭源边界见 `EXTERNAL-PRS.md`）；桌面端经 `src/server/` sidecar 驱动同一内核 |

构建与测试（`AGENTS.md`）：`npm install && npm run build` · `npm test` · `npm run typecheck` · `node dist/cli/entry.js`。

---

## 3. src/ 子系统总表

文件数为非测试 `.ts` 实测（2026-11-11）。

| 目录 | 文件数 | 一句话职责 | 核心入口 |
|------|-------|-----------|---------|
| `agent/` | 594 | 心脏：主循环、CVM、工具管线、编排、韧性 | `loop.ts:192` AgentLoop |
| `tools/` | 254 | 工具实现 + 注册表 + 门控 + 沙箱 | `registry.ts:62` ToolRegistry |
| `tui/` | 216 | 纯 ANSI 自研终端引擎（零 React/Ink） | `engine/app.ts:412` TuiApp |
| `server/` | 158 | HTTP+SSE sidecar（桌面/VSCode/LAN 共用） | `serve.ts` |
| `api/` | 69 | 流式客户端、供应商协议、重试、故障转移 | `openai-client.ts`、`factory.ts` |
| `config/` | 51 | 分层配置 schema、profile、密钥信封 | `manager.ts`、`secrets-store.ts` |
| `prompt/` | 34 | 提示词装配：冻结前缀/volatile/appendixDelta | `engine.ts:97` PromptEngine |
| `context/` | 33 | 认知状态：claims、anchors、rounds、压力、TaskContract | `claim-store.ts`、`task-contract.ts:11` |
| `skills/` | 27 | 技能三级渐进装载 + Python 运行时 | `skill-loader.ts` |
| `utils/` | 22 | 通用工具函数 | — |
| `repo/` | 22 | Meridian 代码图 + Physarum + 项目感知索引 | `meridian-db.ts` |
| `mcp/` | 19 | MCP 客户端：连接/包装/审批/清单门 | `manager.ts:57` McpManager |
| `memory/` | 18 | 统一长期记忆 + 知识索引 + essence-gate | `unified-memory.ts` |
| `auth/` | 17 | API key / OAuth PKCE / 设备流 / OS 密钥库 | `oauth.ts`、`secure-store.ts` |
| `cache/` | 16 | 缓存顾问、自适应阈值、冷热、命中诊断 | `advisor.ts:43` CacheAdvisor |
| `compact/` | 14 | 压缩动作词表、阈值、边界归档、召回指针 | `compaction-profile.ts:26` |
| `workers/` | 13 | CPU 线程池 + 验证/图像子进程 + 委派事件索引 | `cpu-pool.ts` |
| `cli/` | 13 | 入口快路径、子命令早路由、提示词版本警告 | `entry.ts`、`early-routing.ts` |
| `benchmark/` | 12 | 评测 runner + 星域题库 + 恢复场景评估 | `runner.ts` |
| `hooks/` | 11 | 进程内 HookRegistry（Claude Code 风格事件） | `registry.ts:9`、`types.ts:1` |
| `platform/` | 10 | Node 安全版本线、EPERM 过滤、TLS MITM 探测 | `node-security-policy.ts` |
| `plan/` | 10 | 计划模式文档生命周期（.rivet/plans/*.md） | `plan-store.ts` |
| `token-ledger/` | 9 | 消耗计费账本（非上下文账本） | `store.ts` |
| `lsp/` | 9 | 约 25 个 language server + 诊断回流 | `server-registry.ts` |
| `search/` | 8 | BM25+向量 RRF 混合代码检索 | `hybrid-search.ts` |
| `storage/` | 7 | 存储扫描/容量审计/写租约/清理 | `scan.ts`、`audit.ts` |
| `plugins/` | 5 | 插件 manifest/加载/安装（进程内+声明式） | `manifest.ts`、`plugin-loader.ts` |
| `bootstrap/` | 5 | 装配助手：模型解析、headless 工具注册、/init | — |
| `recovery/` | 4 | 受限终端恢复前端辅助 | `recovery-cli.ts`（根级） |
| `model/` | 4 | 能力卡、任务推断、路由指标、worker 绑定 | `capability.ts` |
| `constellation/` | 4 | 项目工程编年史 `.rivet/constellation.json` | `store.ts` |
| `artifact/` | 3 | 按会话产物库（范围读上限 5000 行） | `store.ts` |
| `workflows/` | 2 | slash 生态工作流解析器（/plan /team /council…） | `ecosystem-workflows.ts` |
| `diagnostics/` | 2 | 日志落点单一事实源 + `rivet logs` | `log-locations.ts` |
| `system/` | 1 | 用户空闲探测（computer_use 让出护栏） | `user-idle.ts` |
| `failures/` | 1 | 脱敏失败样本归档 | `sample.ts` |
| `docs/` | 1 | 文档 Status 行解析 | `doc-status.ts` |
| `commands/` | 1 | `.rivet/commands/*.md` 自定义命令加载 | `loader.ts` |
| 根级 | — | `main.ts`、`bootstrap.ts`、`headless.ts`、`stream-json.ts`、`onboarding.ts`、`fs-atomic.ts`、`path-format.ts`、`validation.ts` | 见 §4.1 |

---

## 4. 子系统详解

### 4.1 入口与装配

- 启动链：bin `rivet`/`tianshu` → `dist/cli/entry.js`（package.json bin）。`cli/entry.ts` 先启 V8 编译缓存、`--help/--version` 叶子快路径，再经 `cli/early-routing.ts` 分流 config/provider/serve/sessions/browser/logs/web/storage/skills 子命令，未命中才动态 import `main.ts`。
- `main.ts`：EPERM 过滤 → staged 运行时自检 → headless 走 `runHeadless` → 交互走 `bootstrapInteractiveSession` → 受限终端走 `runRecoveryCli` → `new TuiApp`。
- `bootstrap.ts` 装配序：代理 → 分层配置 → 沙箱/路径授权 → provider+auth → 会话基础设施 → SessionPersist+历史恢复 → 工具注册表 → `new AgentLoop`。`bootstrap/` 是拆出的助手（模型解析、headless 工具注册、/init 脚手架）。
- Headless：`headless.ts` 支持 `-p/--json/--stream-json/--goal`；`stream-json.ts` 是对齐 Claude Code 的 NDJSON 协议（system/init 开场、result 收尾）。

### 4.2 Agent 循环（loop/harness 工程）

- `agent/loop.ts:192` AgentLoop；每 turn 交给 `agent/turn-orchestrator.ts:387` TurnOrchestrator（`loop-factory.ts` 装配）。
- turn 阶段顺序（检索代理报告）：压缩 → prewarm → 感知 → 收敛检查 → replan → 装配请求 → 流式 → 工具批（`tool-execution.ts` executeBatch）→ completeTurn → 续轮门（goal / no-answer / action-intent）；run 收尾 postSession。
- 流式：`turn-stream.ts`；工具派发在流结束后批量执行（**没有**字面意义的"边流边执行工具"执行器；流式仅指 LLM token、工具 onOutput、投机预热）。
- TurnHarness（`turn-harness.ts`）：瞬时重试 2 次 + 轨迹记录。
- 上下文天花板：turn 边界调用 enforceContextCeiling（见 §4.8）。

### 4.3 CVM 认知运行时

- **TaskContract**（`context/task-contract.ts:11`）：目标/范围/约束/验收标准/阶段；在输入边界抽取（`agent/human-input-boundary.ts`），投影注入 prompt。
- **CognitiveFrame**（`agent/cognitive-frame.ts:73`）：turn 边界只读事实帧，把工作事实、计划、证据、进展整理成带质量标记与 fingerprint 的结构；loop 装配后投影给控制器。
- **Sensorium**（`agent/sensorium.ts:63`，`computeSensorium`:321）：多维态势向量。压力合成权重（:188-190 已核验）：上下文填充 0.50 + 验证债 0.30 + CVM 开销 0.15 + 增速 0.05；稳定性分量 doom 0.40 / prediction 0.25 / diversity 0.20 / verification 0.15（:87）。由 preTurn 的 `perception-hook.ts` 计算。
- **RuntimeHookPipeline**（`agent/runtime-hooks.ts:265`）：五阶段 `preTurn → afterPerception → postTool → postTurn → postSession`（:10 已核验），逐 hook 10s 预算；装配入口 `agent/create-runtime-hooks.ts:344` createDefaultRuntimeHooks，按 deps/环境变量条件装配 75 个 hook 模块（`agent/hooks/`），默认激活约 18+。
- **硬门禁 vs 注入提醒**：多数 hook 是 advisory，经 AdvisoryBus 注入 system-reminder，不重写 frozenBase/volatileBlock，由模型决定是否遵循；硬门禁在工具管线（§4.5）与个别 fail-closed hook（如 `hooks/essence-gate-hook.ts` 知识准入）。CVM 是"带状态和监督的运行时"，不是确定性控制模型每一步的虚拟机。
- 常驻基线 hook：perception、signal-consumer、kick、vigor、theta、stigmergy、radio。改 hook 行为前先看 gate 条件，勿假设某 hook 一定在跑。

### 4.4 Hook 体系（三条轨，勿混淆）

| 轨 | 位置 | 事件 | 形态与现状 |
|----|------|------|-----------|
| ① 运行时 hook | `agent/hooks/`（75 模块）+ `agent/runtime-hooks.ts:265` | preTurn/afterPerception/postTool/postTurn/postSession | 生产主轨，条件装配，CVM 的载体 |
| ② 进程内 HookRegistry | `hooks/registry.ts:9`、`hooks/types.ts:1` | PreToolUse/PostToolUse/Notification/SubagentStop/UserPromptSubmit/PreCompact | Claude Code 风格同步 handler，可改写输入/block；`new HookRegistry(` 仅见于测试（本轮 grep 核验），tool-pipeline 消费可选 config.hooks，未发现生产实例化 |
| ③ 外部脚本 hook | `.rivet/hooks.json`（`hooks/hooks-config.ts`）+ `hooks/user-hooks-runner.ts` | preTurn/postTurn/postTool/postSession/onError | 跑 shell（env+stdin），需项目信任；经 `agent/hooks/user-hooks-bridge.ts` 接入运行时管线；`hooks/external-hook-host.ts` 作业队列 + `hooks/hook-frames.ts` 帧协议 + `hooks/hook-host-rpc.ts` 桥接 worker 子进程 |

插件也能贡献 hook（轨③事件集，见 §4.13）。

### 4.5 工具执行管线与审批

`agent/tool-pipeline.ts:592` executeToolUseInner，阶段顺序（检索代理报告，行号未复核）：

```text
TDD门 → 义务RED门 → PreToolUse → 多遍输入修复 → reliability降级门
→ doom-loop拦截 → plan/ask模式门 → 风险判定(approval-risk.ts + approval-input-boundary.ts)
→ 审批(可编辑输入) → 自动checkpoint → TurnHarness执行+瞬时重试 → PostToolUse
→ (编辑后)LSP诊断回流
```

- 权限规则：`tools/permissions.ts`（allow/deny、bash 黑白名单）；风险评估 `agent/approval-risk.ts`（doom-loop、路径穿越、破坏性命令）。
- 批执行：`agent/tool-execution.ts` executeBatch；`prepared-tool-batch.ts` 源序 prepare、只读段限流并发 + 写屏障（并行工具调用）。
- 输出裁剪：per-tool-type / per-message / turn-read 预算、上下文压力联动、ToolAccumulator 风暴折叠、1M+ 窗口分级裁剪、output-sanitizer。
- 沙箱：`tools/sandbox-profile.ts`（seatbelt/bwrap/firejail/landlock/none）+ `sandbox-exec.ts`；bash 经 `RIVET_SANDBOX=1` 启用（默认关）。对应用户手册 `docs/user-guide-sandbox-permissions.md`。

### 4.6 上下文工程（src/context/）

- 旧称 ECF（Evolutionary Context Fabric）**仅存于 changelog**，src 无此词；当前实体是 ClaimStore + Ledger + Anchor + Pressure。
- **ClaimStore**（`claim-store.ts`）：JSONL 事件溯源 + 快照 checkpoint。生命周期：propose 即 active；≥3 个不同 consumer → durable_candidate；≥10min 且 ≥5 → durable；晋升前 recall-gate 验证证据文件仍在（`promotion.ts`）。50 条上限驱逐（`claim-budget.ts`），rule/constraint/preference 豁免。
- **Antibody**（`antibody.ts`）：失败模式 → 防御性 claim，TTL 4h；提取/冲突/antibody 管线在 tool-pipeline 中运行。
- **注意（检索代理报告）**：boostFitness/recordClaimUsed、主 prompt `<active-claims>` 渲染、relevance 打分目前仅测试引用；claims 主要注入 worker（`agent/worker-knowledge.ts`）。引用前先复核。
- **Anchor**（`anchor-registry.ts`）：用户约束正则提取 + salience + 2000 token 预算。
- **Rounds**（`rounds.ts`）：API 轮分组 + token 估算 + tool 配对不变量 + 安全切点。
- **PressureMonitor**（`pressure-monitor.ts`）：thrashing（4 轮 ≥3 次压缩）、快增 ≥0.15、相对压力 log2(ratio/p90)、CVM 开销 5% 节流 / 8% 硬停。
- **Ledger**（`ledger.ts`）：0.5/0.8/0.95 分级 + token 预算 + API 不变量。
- **相关性打分**（`claim-relevance.ts`，scoreClaimRelevance:95 / selectRelevantClaims:161 已核验）：kind 基分、query 标签/文本命中、workingSet 证据、近期工具目标/使用、陈旧罚分、durable 年龄权重、hard-keep；top6/min25。是显式启发式权重，不是模型权重。

### 4.7 记忆体系（src/memory/ + context/session-memory.ts）

三层：

| 层 | 存储 | 要点 |
|----|------|------|
| session 侧车 | `.memory.json`（50 条） | `context/session-memory.ts` |
| 项目长期记忆 | `.rivet/knowledge/memory.jsonl`（`unified-memory.ts:109` 已核验） | JSONL 权威 + SQLite FTS5 可重建投影，失败降级内存 BM25 |
| adaptive STM | 意图缓存块 ≤6 条 1400 字符 | `memory/adaptive-stm.ts`，默认开 |

- 检索：结构预过滤 + BM25 + 时间衰减 + 可选向量 RRF（`knowledge-index.ts`）。
- 注入分层：Tier1 仅 project_rule/user_constraint 且 conf≥0.95 冻结注入（1500 字符上限），其余 recall-only（`context/project-memory-loader.ts`）；跨会话全量 push 默认关（`agent/cross-session-memory-config.ts`）。
- memory 工具四 action：recall / remember / recall_feedback / deep_recall（`tools/memory.ts`）；project remember 走 postSession **essence-gate**（fail-closed、必填 transferableTo，`memory/essence-gate.ts`）；deep_recall 侧路蒸馏历史会话原文（`memory/deep-recall.ts`）。
- observation 写路径与规则自动生成已停用（`observation-store.ts`、`rule-generator.ts` 头部声明）。

### 4.8 压缩（src/compact/ + agent/compaction-controller.ts）

- 动作词表（`compaction-profile.ts:26` 已核验）：`none / stale-round / micro / partial-llm / full-llm / session-split / checkpoint`。**无 smart 模式**（旧索引里的 smartCompact 已过时）。
- 决策：`agent/compaction-controller.ts` maybeCompact → decideCompactAction；micro = 确定性截断 + 语义折叠（`compact/micro.ts`、`context-collapse.ts`）。
- 阈值三档（`compact/constants.ts`，检索代理报告）：balanced .6/.78/.88/.95、cache-preserving .72/.86/.92/.95、aggressive .5/.7/.84/.95（watch/compact/reactive/ceiling），随缓存命中率平移；大窗精度天花板 0.7；1M 窗 LLM 梯 .60/.75（`context/compact-policy.ts`）。
- **缓存锚**：CACHE_ANCHOR_MESSAGES=2 保首 2 条前缀锚，各重写均避开；reclaim gate 防碎缓存的无效重写（`compact/reclaim-estimate.ts`）。
- 落盘：丢弃区归档为 compact-history artifact + 召回指针（read_section 取回、老化折回指针，`compact/recall-marker.ts`）；有损重写前先边界归档、失败 fail-open 保原文（`compact/boundary-archive.ts`）；压缩事件落 meta.compactEvents。
- 天花板执行：95% 硬顶——≥1M 窗强制 checkpoint、小窗强制 micro；enforceContextCeiling 先试 LLM compact（30s），失败回退结构化 `<checkpoint-resume>`（保 2 锚+摘要+任务锚）；86% session-split（≥500K 窗）。
- 请求级硬闸：`context/request-budget.ts` inputBudget = 窗口 − 输出预留 − 安全边际，超限抛 ContextBudgetExceededError。

### 4.9 提示词工程（src/prompt/）

PromptEngine（`engine.ts:97`，实测 1809 行）装配结构：`System = static冻结基座 + volatile动态区 + appendixDelta跨回合增量`。

- 装配入口 `buildOaiRequest`:551（已核验）；用户消息 trailer 合并 buildTraileredUserContent。
- **冻结机制**：历史用户消息字节级快照（getNextFrozen）；`commitFrozenSnapshot`:487（已核验）仅在真实用户边界提交；FrozenAnchors（出现序 key）+ FrozenRenderState（publish/耐久提交）保证"只有发布才冻结"；快照落盘、resume//cd 继承（inheritFrozenFrom）。`rebuildFrozenBase`:1113（已核验）只重建 base，volatileBlock 交换推迟到下一用户边界。
- **增量 appendix**：`buildAppendixBody`:1485（已核验）按子块名 diff，只发变化块（seq/mode="delta"）；压缩后 resetAppendixBaseline 恢复全量。
- **一次性 vs 累积分离**：`setCognitiveProjection`:1421（已核验）拆出 ephemeral，withEphemeralProjection 在 delta 外渲染；plan 退出提醒渲染一次即清。
- **边界纪律**：invalidateFreshCache、updateTools 发布后拒改、system-reminder 伪用户消息不构成边界；`stream()` 对 request 的变换必须 copy-on-write（CLAUDE.md Known Constraints）。
- 诊断：recordPrefixDivergence 逐消息哈希探针；requestTimeCollapse + T7 水位（50K token 步进）。
- 其他文件：`static.ts` BASE_PROMPT 中文主提示（identity/beliefs/rules/workflow/security）+ gateMainPrompt 按实际工具门控 + MODEL_CALIBRATIONS 模型族校准；`context-layer.ts` stability/channel/fingerprint 分层账本；`volatile.ts` buildStableVolatileBlock 的 strip 列表是 frozen/dynamic 唯一分界，渲染 frozen `<context>`（环境/项目指令/记忆/知识索引/种子胶囊/星域）+ GWT Top-K salience 选择；`volatile-snapshot.ts` 启动期快照（AGENTS.md/.rivet.md、信任门）；`fingerprint.ts` sha256+drift；`cache-diagnostic.ts` miss 归因；`block-policy.ts` 用户可选 standard/lean/full 档——只调参考类块、永不触碰行为护栏，优先级 env>项目>用户；`field-habituation.ts` 字段习惯化晋升→`<consolidated>`。
- **版本管理三道防线**（无运行时版本号）：① `cli/prompt-version-warning.ts` 把主提示哈希存 `~/.rivet/.static-prompt-hash`，变更警告旧会话前缀缓存失效；② 黄金字节测试钉死 MAIN_BASE_PROMPT；③ `docs/prompt-changelog/` 逐条记录改动+理由+.bak 全量备份，`docs/prompt-versions/` 存精简前后快照。

### 4.10 前缀缓存工程（src/cache/ + api/provider-profile.ts）

- **CacheAdvisor**（`advisor.ts:43`）：`onTurnEnd`:64 输入 TurnMetrics（cacheRead/Creation、prefixChanged、artifact 逐出/访问）；`shouldDelayCompact`:121 核心公式 protection = hitRate×(1−窗口压力) ≥ 0.45 时延迟压缩，tier≥3 永不延迟；`getArtifactThreshold`:164 相位乘数。
- **自适应阈值**（`adaptive-threshold.ts`）：ghost 回读抬阈值、命中率 ≥0.8 升 / <0.3 降、随窗口缩放。
- **冷热**（`session-warmth.ts`）：hot(<60s)/warm(<1h)/cold；`behavior-learner.ts` 从观测推断供应商缓存行为。
- **供应商策略分类**（`api/provider-profile.ts:3` 已核验）：`exact-prefix / explicit-breakpoint / partial-prefix / block-kv / none`。deepseek/glm/grok/longcat 持久 exact-prefix；anthropic/google/qwen 显式断点；openai/codex partial-prefix（128 粒度）；vllm block-kv。压缩档据此选择（cache-preserving vs 常规）。
- **命中诊断**：`cache-diagnostic-class.ts` 证据标签、`billed-hit-rate.ts` 全账单口径、`usage-aggregator.ts` 聚合 cache-log.jsonl、`cache-audit.ts` 变更文件风险审计。
- **预热**：文件级投机预热在 `agent/prewarm.ts` + `prewarm-controller.ts`，由 onToolCallHint 触发；**不提前发建缓存请求**（docs/plans/2026-07-31-galaxy-prewarm-and-cache-affinity.md 明确）。
- 实测口径：README 报告 5 个 DeepSeek 自举会话、2001 次请求、99.3–99.6% 命中率，日志与复算命令见 `docs/reference/observability-harness.md`。"越用命中越高"来自长会话复用稳定前缀 + 上下文变长后新增未命中占比变小，不是缓存引擎在学习。

### 4.11 API 客户端与模型路由（src/api/ + src/model/）

- 统一接口 `api/stream-client.ts` StreamClient + 回调 + wire 分歧探针。
- **openai-client.ts**（约 1923 行，DeepSeek 走此 OpenAI 兼容路径）：stream 组 body（thinking/effort）→ withStructuredRetry → SSE 解析；EOF 无 [DONE] 但有终态 finish_reason 宽容收尾，否则 IncompleteStreamError；reasoning_content→thinking，content 开始后重分类；tool_calls 缓冲 resolveToolCallIndex 防跨工具污染、两段 flush、不可解析标 argsTruncated 拒执行；截断 JSON 恢复（转义修复 + salvageFirstJsonObject）；**DeepSeek tool-json-in-content bug 专用修复** tryParseToolJsonFromContent + DSML 兜底；usage 映射 prompt_cache_hit/miss→cache_read/creation + calibrateUsage；reasoning 回显 mapWireMessages。
- 重试：`retry-engine.ts` 分类重试（error-classifier）+ 抖动退避 + Retry-After 优先 + 共享预算；`fallback-client.ts` 供应商故障转移。
- 协议分派：`factory.ts` Codex OAuth→Responses→Anthropic→Gemini→OpenAI 兼容；`provider-catalog.ts` wire 怪癖（会话头/UA/思考停滞超时）；`provider.ts` 能力卡 + WELL_KNOWN_DEFAULTS 覆盖 deepseek/kimi/glm/minimax/mimo/openai/grok/codex/claude/longcat/siliconflow/dashscope/openrouter/stepfun 等，resolveCapabilities 三层合并。
- `deepseek-platform-client.ts` 是平台**账单** API，不是聊天客户端；`json-syntax-repair.ts` 修 worker 报告裸引号/尾逗号。
- **src/model/**：`capability.ts` 能力卡（工具可靠性/JSON 稳定性/编辑成功率/缓存经济性）+ 按任务加权选模；`task-inferrer.ts` 从近期工具调用推断任务类型；`routing-metrics.ts` 路由事件环形缓冲；`model-binding.ts` worker 侧模型绑定，凭据永不跨 worker 协议。

### 4.12 工具系统（src/tools/）

- **注册表**：`registry.ts:62` ToolRegistry（Map，后到者赢；register/execute/needsApproval/外框架别名 resolveName）；Tool 接口 `types.ts`（definition/execute/requiresApproval/isConcurrencySafe/isEnabled/timeoutMs）；装配 `default-registry.ts`。
- **两级门控**：① 装配档 `tool-preset.ts`（minimal/frontend/orchestration/full/taiyi）；② 主控可见性 `agent/tool-tiers.ts:8` CORE_TOOLS / :58 EXTENDED_TOOLS / :177 gateToolDefinitions（allow/deny-list，extraCore/mountedExtras 豁免；均已核验）。状态面 `tool-surface.ts`。官方宣称 kernel budget ≤26，超出触发认知过载退化（kernel-budget 测试）。
- 主工具：bash、read_file、write_file、edit_file、hash_edit、grep、glob、ast_grep、git、run_tests、web_fetch、web_search、todo、delegate_task/batch、plan、deliver_task、memory、recall_capsule、job、monitor、repo_map。
- 特色工具：computer_use（pro 门控，stub+bridge）、ask_image 视觉、ask_user_question（endTurn）、office 系列（create_document/spreadsheet/presentation/pdf）、generate_image、galaxy/starflow/council_convene/team_orchestrate、schedule_*、sandbox_exec。
- 执行基础设施见 §4.5（并行/裁剪/沙箱）。

### 4.13 插件系统（src/plugins/ + 顶层 plugins/）

- 形态：**进程内 + 声明式**。manifest 在 package.json 的 `tianshu` 字段（zod 校验，`manifest.ts`）。
- 贡献面：tools / skills / hooks（preTurn/postTurn/postTool/postSession/onError）/ commands（slash .md）。
- **permissions 仅声明不强制**（`manifest.ts:11/36/49` 已核验，ADVISORY ONLY 明示）：无运行时沙箱，插件可触达文件系统/子进程/环境；审批边界 = 安装确认。
- 加载：`plugin-loader.ts` 扫 `~/.rivet/plugins`、动态 import、wrapPluginTool 展平 ABI + validatePathSafe、同名拒整插件、让位 map。安装：`plugin-installer.ts`（npm --ignore-scripts）+ `git-source.ts`。
- 示例插件（顶层 `plugins/`）：hello-world、office-docx/excel/pdf/ppt、design、tianshu-imbridge。
- 对照本仓已知缺口：与 DeepAgent 的"MCP 适配器硬编码 Medium+Network、插件工具永不审批"问题同构，天枢此处也是声明式而非强制边界。

### 4.14 技能系统（src/skills/）

- **三级渐进装载**（`docs/skills-architecture.md`）：L1 发现块 `renderDiscoveryBlock`（`skill-loader.ts:227` 已核验）每轮 volatile 附录、cache-safe、不注入正文；L2 `skill` 工具全文 inline（FIDELITY_EXEMPT 不截断）；L3 子文件按需 read（`listSkillFiles`:347 已核验）。
- SKILL.md：`parseSkillMarkdown`:62（已核验，YAML frontmatter + body）；扁平 .md 或目录/SKILL.md；`skill-metadata.ts`（openai.yaml/runtime.yaml）。
- 注入链：turn-step-producer → prompt/engine → volatile；invoked-skills 为 protected block；单例 skillRegistry，source 含 'plugin'。
- 与插件区别：技能纯提示词无二进制依赖；插件完整 Node 模块。Python 运行时 `src/skills/runtime/`（skill_runtime 工具）。
- 内置技能在顶层 `runtime-assets/`（brainstorming、visual-acceptance）。

### 4.15 MCP（src/mcp/）

- `manager.ts:57` McpManager（连接/发现/重连/health，listTools）；`wrapper.ts` 包装为 `mcp__server__tool`，描述含 rug-pull 警示、连接器 consent。
- 策略：`policy.ts` capability→allow/confirm/block；**审批** `server-approval.ts` 指纹（command/args/cwd/envKeys/url）、headless fail-closed；**清单快照门** `tool-inventory.ts` diff/armed 防 TOCTOU。
- 失败分类：`failure-classifier.ts`（config/auth/network/protocol/process/process_env/process_install/tool_error）。
- "deferred tools"实为**晚到注册**：registry beginExtraRegistration + bootstrap asyncExtras fire-and-forget；未见独立懒发现特性。
- 另有 transport-factory / health-check / oauth / presets / workspace-policy。

### 4.16 LSP（src/lsp/）

- `server-registry.ts` 约 25 个 server（typescript/pyright/gopls/rust-analyzer/clangd/jdtls/roslyn/kotlin/sourcekit/dart/metals/intelephense/ruby-lsp/lua/zls/bash/terraform/clojure/ocaml/haskell/nil/vue/svelte 等）。
- 工具仅注册 ready+支持者：`lsp_goto_definition`、`lsp_find_references`（`tools.ts`）；`manager.ts` gotoDef/findRefs/诊断 pull+push；`multi-manager.ts` 按文件路由。
- 编辑后诊断追加回流工具管线（changeFile + filterDiagnosticsForEdit）。

### 4.17 仓库智能（src/repo/ + src/search/）

- **Meridian**（`repo/meridian-*`）：`.rivet/meridian.db`（better-sqlite3）存 symbols/edges/access/co-edit/physarum；tree-sitter 解析 TS/JS/Py/Go；导入图/影响半径。
- **项目感知层**：`repo/codebase-index.ts` 自称 "Project Perception Layer"，模块/CLI 索引注入 frozen 前缀、verifiedAtCommit 防陈旧。
- **Physarum**（`repo/physarum-engine.ts`）：电导拓扑/信息素式路径强化，预测文件访问偏好。
- 工具面：repo_map=文件树、repo_graph=扩散激活、impact=反向 BFS 推测试。
- **search/**：代码 BM25+向量 RRF 混合检索，快照 `.rivet/semantic-index.json`，无 embedding 降级 BM25；与记忆知识库索引 namespace 隔离。

### 4.18 多代理编排与星域（src/agent/ 编排族 + workers/ + constellation/）

- **委派**：`agent/coordinator.ts:659` DelegationCoordinator（预算门、模型路由、批量派发）；工具 delegate_task/delegate_batch；WorkerProfile + WorkOrder → 独立无头会话；worker 可进程外隔离（worker-process/parent.ts）；自适应路由按 profile×model 打分；聚合策略可配。
- **team**：`team-orchestrator.ts` 波次执行、文件冲突感知调度。
- **council 议事会**：`council/council-orchestrator.ts` 席位=星域、三柱对抗（council-routing）、契约密封（council-seal）、可选反驳轮、席位 tierFloor 只抬升不降级。
- **starflow**：`starflow-orchestrator.ts` council→team→galaxy 硬门禁状态机；galaxy 多维并行。
- **16 星域**：`agent/star-domain-data.ts:61` STAR_DOMAINS（已核验）——紫微星名 persona（tianshu/pojun/tianfu…qisha/taiyi），带关键词/工具白名单/勇气阈值；deriveAuthority 路由委派；拼错 authority fail-closed 返回 []。当前内置白名单为全集→交集退化为恒等（太一域另经 toolPreset:"taiyi" 装配 14 件核心工具）。
- **src/workers/**：CPU 线程池（diff/AST）+ 测试/图像子进程（verification-worker、image-worker）+ delegation-state 委派事件索引。
- **src/constellation/**：项目工程编年史 `.rivet/constellation.json`（里程碑 + agent 自选离场符），**不是**派发机制。

### 4.19 计划模式与工作流（src/plan/ + src/workflows/）

- plan = 计划模式文档生命周期：`plan-store.ts`（`.rivet/plans/*.md` 提交/批准）、`plan-approval.ts` 批准 kickoff、`plan-close.ts` 勾选闭环、`restore-plan-mode.ts` 会话恢复；运行态 enterPlanMode 在 loop。流程：调研→结构化计划→审批→分波执行（wave-gate）。
- workflows = slash 命令生态工作流解析器（`ecosystem-workflows.ts`：/plan /team /council /scout /starflow → prompt + requiredTools；required-tools.ts 显式挂载工具），**不是**通用工作流引擎。

### 4.20 韧性与交付门禁

| 机制 | 位置 | 作用 |
|------|------|------|
| doom-loop 指纹 | `agent/trace-store.ts` + loop 分级 | 重复工具模式检测拦截 |
| behavior-mirror | `agent/behavior-mirror.ts` | 重复错误/编辑镜检 |
| strategy-shift | `agent/strategy-shift.ts` → execution-guidance | 4 模式检测器，撞墙时建议换策略 |
| watchdog | `agent/turn-heartbeat.ts` + watchdog-recovery-policy.ts | 静默心跳 + hardStall 中止 + 自动续跑配额 |
| GoalMode | `agent/goal-tracker.ts` + goal-judge.ts + goal-continuation.ts | 目标检查、完成判定、自动继续（含上下文接力） |
| 交付证据评估 | `agent/delivery-gate.ts` + delivery-gate-v2.ts | v2 GREEN/YELLOW/RED 归因评估；当前 `agent/deliver-task.ts`（2026-10-25 契约拆分）只做安全提交，验证待办用于披露，不再作为该工具提交前置闸门；测试/审查独立 |
| checkpoint | `agent/checkpoint.ts` | git 基线/回滚，跨会话 ownership guard（仅 agent-owned 文件） |
| undo | `tools/undo.ts` + agent/file-history.ts | 文件级快照撤销 |
| wave-gate | `agent/wave-gate.ts` | 计划分波间硬门禁 |
| 修复管线 | `agent/repair-pipeline.ts` + repair-passes.ts | 可插拔修复遍（fourHorsemen、语义修复、DeepSeek tool-JSON） |

### 4.21 表面层（TUI / sidecar / headless / VSCode）

- **TUI**：纯 ANSI 自研、零 React/Ink（依赖表核验）；`tui/engine/app.ts:412` TuiApp 事件驱动 class，Commit/Live/Overlay 三渲染引擎；cockpit 8 面板（summary/trace/verify/context/safety/model/mcp/advisory）；SteerBuffer 承接流式中输入。
- **server sidecar**：`rivet serve` HTTP+SSE（未发现 WebSocket），默认 127.0.0.1 + Bearer fail-closed，复用同一 AgentLoop 内核。能力面：/status /abort /prompt、/sessions/*（创建/prompt/steer/fork）、审批 interventions、全局 SSE /events，另有 cron/mission/git 工作台/browser/speech/MCP/插件/LAN 直连 /mobile。RuntimeSessionManager 管 N 个 AgentLoop，每会话单调 seq 可重放事件日志。
- **VSCode/Cursor**：`vscode-extension/` 开源 sidecar 客户端（launcher/client/runtime-downloader）+ webview-ui + E4 客户端工具委托（原生 diff/可见终端）+ 自包含运行时自举。
- **Headless**：见 §4.1。

### 4.22 配置与认证（src/config/ + src/auth/）

- 分层：defaults → user → project（`.rivet-config.json` 向上查找）→ profile（`$RIVET_HOME/profiles/<name>.json`）→ session overlay（`manager.ts`、`profile.ts`）；zod schema。
- 密钥：config 只存 keyRef；密钥入 secrets.json AES-256-GCM 信封（`secrets-store.ts`）；数据密钥托管 OS 密钥库（macOS security / Windows DPAPI，`auth/secure-store.ts`）。
- auth：API key（api-key.ts）、provider OAuth/PKCE 含 Codex 导入（oauth.ts、registry.ts）、天枢账号 RFC8628 device flow（account.ts）。

### 4.23 会话持久化

- CLI transcript：JSONL（zstd 帧压缩），`<RIVET_HOME>/sessions/<project-slug>/<id>.jsonl`（`agent/session-persist.ts`、paths.ts），另有 claims/frozen/cache-log 侧车。
- 桌面 sidecar：`<id>/index.json + events.jsonl + seq 偏移索引`（`server/session-persistence.ts`）。
- SQLite（better-sqlite3 可选依赖）仅用于跨会话注册表 `state/registry.db`（sessions/claims/events 表，`session-registry.ts`），缺失降级内存。
- 产物库：`artifact/store.ts` 按会话产物、范围读上限 5000 行。

### 4.24 token-ledger 计费账本

**是消耗计费账本，不是上下文账本**：按字节偏移增量消费各会话 cache-log.jsonl 计费行，折叠 day×model(×provider)+hour 的 requests/input/output/cacheRead/cacheCreate/cost，原子落盘 `~/.rivet/token-ledger.json`（`token-ledger/store.ts`）。行源自 API usage（loop-factory 接线），解析复用 cache/usage-aggregator，side_path（压缩/投机）同样计费；service 编排论坛快照推送。

### 4.25 外围小模块

| 模块 | 职责 |
|------|------|
| `storage/` | 存储扫描/容量审计/技能快照压缩清理/写租约；自动淘汰已禁用（history-retention.ts） |
| `recovery/` + `recovery-cli.ts` | 受限终端行式恢复前端 + 恢复待答问题 |
| `diagnostics/` | 日志落点单一事实源 + `rivet logs` |
| `failures/sample.ts` | 脱敏失败样本归档 |
| `platform/` | Node 安全版本线、EPERM 过滤、退出保险丝、TLS MITM 探测、测试隔离 |
| `benchmark/` | 评测 runner + 星域题库 + 恢复场景评估 |
| `onboarding.ts` | /connect 向导抑制哨兵 |
| `commands/loader.ts` | `.rivet/commands/*.md` + 插件自定义命令 |
| `system/user-idle.ts` | 跨平台用户空闲探测（macOS JXA / Win GetLastInputInfo / Linux null），"用户接管即让出"护栏数据源，默认 1200ms |
| `docs/doc-status.ts` | 文档 Status 行解析 |

---

## 5. 问题 → 文件路由表（快速问答用）

| 问题类型 | 首查文件 |
|----------|---------|
| 一个 turn 怎么走完 | `src/agent/loop.ts:192` → `turn-orchestrator.ts:387` → `tool-execution.ts` |
| CVM 五阶段/某 hook 为何没跑 | `src/agent/runtime-hooks.ts:265`、`create-runtime-hooks.ts:344`（gate 条件）、`agent/hooks/` |
| 目标/验收标准从哪来 | `src/context/task-contract.ts:11`、`agent/human-input-boundary.ts` |
| 态势压力怎么算 | `src/agent/sensorium.ts:321`（权重 :188-190） |
| 外部 hook 脚本怎么接 | `.rivet/hooks.json` → `src/hooks/hooks-config.ts` → `src/hooks/user-hooks-runner.ts` → `src/agent/hooks/user-hooks-bridge.ts` |
| 工具审批/风险判定 | `src/agent/tool-pipeline.ts:592`、`approval-risk.ts`、`tools/permissions.ts` |
| 工具为什么不可见 | `src/agent/tool-tiers.ts:177`、`tools/tool-preset.ts`、星域 toolWhitelist |
| 并行工具/写冲突 | `src/agent/prepared-tool-batch.ts`、`tool-execution.ts` |
| 沙箱后端 | `src/tools/sandbox-profile.ts`、`sandbox-exec.ts`、`docs/user-guide-sandbox-permissions.md` |
| claim 晋升/驱逐/抗体 | `src/context/claim-store.ts`、`promotion.ts`、`claim-budget.ts`、`antibody.ts` |
| 记忆怎么存/注入/召回 | `src/memory/unified-memory.ts:109`、`knowledge-index.ts`、`context/project-memory-loader.ts`、`tools/memory.ts` |
| 记忆写入门禁 | `src/memory/essence-gate.ts`（fail-closed） |
| 压缩阈值/动作/缓存锚 | `src/compact/constants.ts`、`compaction-profile.ts:26`、`agent/compaction-controller.ts` |
| 压缩丢了什么/怎么找回 | `compact/boundary-archive.ts`、`recall-marker.ts`、`artifact/store.ts` |
| 上下文 95% 硬顶 | `src/context/compact-policy.ts`、`agent/compaction-controller.ts`、`context/request-budget.ts` |
| 前缀为什么断了 | `src/prompt/engine.ts`（recordPrefixDivergence）、`prompt/cache-diagnostic.ts`、`fingerprint.ts` |
| 冻结/增量/一次性提示 | `src/prompt/engine.ts:487/551/1113/1421/1485`、`volatile.ts`、`frozen-anchors.ts` |
| 提示词档位/模型校准 | `src/prompt/block-policy.ts`、`static.ts`（MODEL_CALIBRATIONS） |
| 提示词改动纪律 | `CLAUDE.md` Known Constraints、`docs/prompt-changelog/`、`cli/prompt-version-warning.ts` |
| 压缩该不该延迟 | `src/cache/advisor.ts:121`（protection≥0.45）、`adaptive-threshold.ts` |
| 供应商缓存类型 | `src/api/provider-profile.ts:3` |
| 命中率口径/复算 | `src/cache/billed-hit-rate.ts`、`usage-aggregator.ts`、`docs/reference/observability-harness.md` |
| DeepSeek 流式怪癖 | `src/api/openai-client.ts`（reasoning_content、tool-json-in-content、usage 映射） |
| 重试/故障转移 | `src/api/retry-engine.ts`、`error-classifier.ts`、`fallback-client.ts` |
| 选模/能力卡 | `src/model/capability.ts`、`task-inferrer.ts`、`api/provider.ts` |
| 子代理派发/预算 | `src/agent/coordinator.ts:659`、`work-order.ts`、`worker-session.ts` |
| team/council/starflow | `src/agent/team-orchestrator.ts`、`council/`、`starflow-orchestrator.ts` |
| 星域 persona/阈值 | `src/agent/star-domain-data.ts:61`、`star-domain.ts`、顶层 `star.md` |
| doom-loop/撞墙换策略 | `src/agent/trace-store.ts`、`behavior-mirror.ts`、`strategy-shift.ts` |
| 目标自动继续 | `src/agent/goal-tracker.ts`、`goal-judge.ts`、`goal-continuation.ts` |
| 交付证据/安全提交的边界 | `src/agent/delivery-gate.ts`、`delivery-gate-v2.ts`、`agent/deliver-task.ts`（评估不等于提交硬门） |
| 回滚/撤销 | `src/agent/checkpoint.ts`、`file-history.ts`、`tools/undo.ts` |
| 插件能贡献什么/权限 | `src/plugins/manifest.ts`（permissions 仅声明）、`plugin-loader.ts` |
| 技能三级装载 | `src/skills/skill-loader.ts:227/347`、`docs/skills-architecture.md` |
| MCP 审批/清单门 | `src/mcp/server-approval.ts`、`tool-inventory.ts`、`policy.ts` |
| LSP 诊断回流 | `src/lsp/manager.ts`、`tool-pipeline.ts`（编辑后诊断） |
| 代码图/影响半径 | `src/repo/meridian-db.ts`、`meridian-impact.ts`、`physarum-engine.ts` |
| 会话存哪/怎么恢复 | `src/agent/session-persist.ts`、`server/session-persistence.ts`、`session-registry.ts` |
| 配置分层/密钥 | `src/config/manager.ts`、`secrets-store.ts`、`auth/secure-store.ts` |
| 计费/成本 | `src/token-ledger/store.ts`、`cache/usage-aggregator.ts` |
| sidecar 路由/事件重放 | `src/server/serve.ts`、`session-manager.ts`、`server-events-route.ts` |
| TUI 渲染 | `src/tui/engine/app.ts:412`、cockpit/ |
| headless/NDJSON 协议 | `src/headless.ts`、`stream-json.ts` |
| 日志在哪 | `src/diagnostics/log-locations.ts`（`rivet logs`） |
| 用户空闲让出 | `src/system/user-idle.ts` |
| 评测 | `src/benchmark/runner.ts` + 顶层 `benchmark/` 题库 |

---

## 6. 文档体系地图

### 6.1 仓内 docs/（1035 篇）

- 索引：`docs/INDEX.md`（scripts/docs-index.ts 自动生成，勿手编）；`docs/docs.json` 是**扁平索引**（path/title/type/status/date/related），**不是站点导航**；`docs/MINDMAP.md` markmap 思维导图。
- 类型分布：plan 350 / spec 213 / analysis 174 / design 58 / reference 49 / changelog 48 / issue 33 / release 22 / research 21 / guide 14 / unclassified 53。
- 注意：大量 plan/spec 是历史执行记录，状态以文档内 Status 行为准，不代表当前代码状态。

### 6.2 关键文档（按用途）

| 用途 | 文档 |
|------|------|
| 架构总览（六层/CVM/缓存/星域） | `docs/architecture-overview.md` |
| 用户手册 | `docs/user-guide.md` + provider-config / prompt-profile / sandbox-permissions / vision 分册 |
| Agent 导航/运行时数据/缓存排查 | 根 `AGENTS.md`；构建硬约束根 `CLAUDE.md` |
| 缓存命中实证（5 会话 2001 请求） | `docs/reference/observability-harness.md` |
| CVM 行为 A/B（5 任务单模型 LLM 审查） | `docs/CVM运行时对Agent模型的实证影响.md` |
| CVM 参考 | `docs/reference/cvm-cognitive-runtime*` |
| 压缩调参 | `docs/compaction-tuning.md` |
| 技能架构 | `docs/skills-architecture.md`、`docs/skills-guide.md` |
| 插件 | `docs/plugins.md` |
| 子代理架构 | `docs/architecture-subagent.md` |
| 经络图（Meridian） | `docs/meridian-architecture.md` |
| headless 协议 | `docs/headless-stream-json.md` |
| 远程访问 | `docs/remote-access.md` |
| 工程指标 | `docs/engineering-metrics.md` |
| 审查纪律 | `docs/review-discipline.md` |
| 提示词版本 | `docs/prompt-versions/`、`docs/prompt-changelog/` |
| 旧代码索引（**已过时，勿信**） | `docs/codebase-index.md` |

### 6.3 官方文档站（tianshuharness.com）

本轮抓取确认是**轻量快速开始站**，不是深度文档库：

| 页面 | 内容 |
|------|------|
| `/docs`、`/docs/getting-started` | 注册、读者对象、准备、安装、首次成功、Repo Map |
| `/docs/install-cli` | Node≥24、四种安装方式（桌面/脚本/npm/源码）、init、shell 补全、更新 |
| `/docs/tui-activation` | 桌面端与 CLI 激活：定义、能力、使用、入口、排查 |
| `/docs/open-source-ecosystem` | Tianshu-harness 与 dsh-tianshu-tui 两仓角色、贡献指南 |
| `/oss`、`/oss-tianshu-harness`、`/oss-dsh-tianshu-tui`、`/download` | 开源生态与下载 |

深度内容一律回仓内 `docs/` 查证。官网源码已拆独立仓（`website/README.md` 声明；残留 README 称 Vue3 而 DESIGN 称 Next.js，二者不一致，不作为事实依据）。

---

## 7. 证据边界与已知陷阱

1. **`docs/codebase-index.md` 是 Rivet 时代旧快照**（182 文件、Ink TUI、prompt/engine 214 行），与现状（1709 文件、纯 ANSI、engine 1809 行）严重不符，只能当线索。
2. **ECF 名称仅存于 changelog**；boostFitness/`<active-claims>` 主渲染/relevance 打分据检索代理报告仅测试引用——引用前先 grep 复核。
3. **进程内 HookRegistry 无生产实例化**（本轮 grep 核验 `new HookRegistry(` 仅测试）；生产 hook 主轨是 runtime hooks + 外部 hooks.json。
4. **插件 permissions 是声明式意图，不是沙箱**（manifest.ts 明示 ADVISORY ONLY）；审批边界只在安装确认。
5. **MCP 无独立 deferred 懒发现**；对应物是注册表晚到注册闸门。工具派发在流结束后批量，无字面"边流边执行"。
6. **顶层 `prompts/` 是遗留语料**，未发现运行时引用；真正的提示词在 `src/prompt/static.ts`。
7. **desktop/ 闭源不在本仓**；website/ 已拆分。涉及桌面端实现只能经 `src/server/` sidecar 契约推断。
8. 官方宣称（95–99% 命中、75 hooks、kernel≤26、21514 用例）与代码事实要分开引用；缓存实测仅覆盖 DeepSeek 自举会话，A/B 报告为 5 任务单模型 LLM 审查，不足以证明对 Codex/Claude Code 的整体优势。
9. 本文未带行号的机制描述来自只读检索代理，行号可能漂移；带行号锚点为本轮主代理核验（2026-11-11）。
10. **2026-11-11 二次源码复核纠正**：`agent/deliver-task.ts` 当前将 Git 安全提交与测试/审查分开，验证失败/未验证台账用于交付披露，并非该工具的提交前置闸门；不能仅凭 `DeliveryGateV2` 类名和旧计划描述声称“所有未验证改动都不能提交”。详见 [与 DeepAgent Studio 的对比调研](tianshu-harness-与-deepagent-studio-对比调研.md)。

---

## 8. 与本仓（DeepAgent Studio）的对照锚点

| 天枢机制 | 本仓对应 seam | 参考价值 |
|----------|--------------|---------|
| CognitiveFrame（turn 边界只读事实帧） | `AgentKernel` + 持久化 `run_events`（crates/deepagent-runtime、deepagent-app-core） | 在现有运行事件/内核状态上投影统一事实帧，而非重建持久化 |
| TaskContract 输入边界抽取 | 会话/turn 启动契约 | 目标、范围、验收标准结构化，供门禁消费 |
| 冻结前缀 + appendixDelta + 用户边界压缩 | 生产装配走 `ContextAssembler`（crates/deepagent-context）；旧 SystemPromptBuilder/PromptBudget.fit 已退役 | DeepSeek 稳定前缀、增量注入、命中诊断做成同一套可验证链路 |
| CacheAdvisor（命中率×窗口压力延迟压缩） | 压缩/缓存决策 | 显式启发式公式，可移植可测试 |
| 五阶段 RuntimeHookPipeline + AdvisoryBus | 内核 hook/提醒链路 | 区分硬门禁与注入提醒，条件装配而非固定清单 |
| essence-gate fail-closed 记忆准入 | 记忆体系（memory-optimization initiative） | 写入门禁 + transferableTo 必填 |
| 压缩边界归档 + 召回指针 | 压缩边界落盘两条轨（compaction-boundary initiative） | 丢弃区可召回、失败 fail-open 保原文 |
| MCP 清单快照门（防 TOCTOU）+ 指纹审批 | MCP registry/adapter/审批（已知插件工具审批缺口） | 审批指纹 + headless fail-closed 的现成参照 |

> 借鉴纪律：`借鉴/` 只作设计与实现参考，不直接复制业务代码；移植须收敛到本仓既有 AppCore/Runtime/Tool/权限/事件边界。
