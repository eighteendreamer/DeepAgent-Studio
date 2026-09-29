# M6 插件系统重构：以 Cordis 运行时语义为内核的组态层方案与工程级测试计划

适用范围：`crates/deepagent-app-core`（插件子系统）、`crates/deepagent-tools`（注册表与沙箱）、`apps/desktop/src-tauri`（IPC）、`apps/desktop/src`（插件页 UI）。
依据信源：`借鉴/cordis`（v4.0.0-rc.10 源码）、`借鉴/deepseek-harness`（vendor `@deepseek-ai/cordis` 4.0.4，全插件化实现）、本仓库现有代码与测试。
本文只定方案与验收，不含实现代码。

---

# 第一部分：结论与架构

## 1. 结论先行

**不引入 Node 版 Cordis 库，而是把 Cordis 的运行时组态语义移植成一个新的 Rust crate `deepagent-cordis`，用它替换现有"文件系统扫描 + 只读投影"的插件模型。**

三条决定性依据：

1. **项目铁律禁止第二套注册表。** `.qoder/rules/deepagent-studio.md` §5.1 明令"禁止新增第二套工具注册表"。`ToolRegistry::register` 在 `crates/deepagent-tools/src/registry.rs:76`，是唯一权威。若把 Cordis 组态层放进 Node 宿主进程，工具/钩子/审批注册表必然出现第二份，直接违反 §5.1 与 §5.4。
2. **Cordis 的 DI 靠对象身份做键，跨进程传不了。** `deepseek-harness` 用 `WeakMap` 做 scope 键（`packages/core/scope/src/index.ts:30-39`）、`WeakSet` 做 `concludingExecutions`（`packages/core/tools/src/index.ts:826`）。跨进程边界只能传声明，不能传引用——一旦传声明，就退回到本系统现在的"投影结构体"，等于没换。
3. **Cordis 真正的价值不是那个容器，是它的生命周期语义。** 容器本体是 `Proxy` + 原型链（`packages/core/src/reflect.ts:62-135`，约 500 行），Rust 没有也不该有对应物；Rust 的 trait 在编译期就把类型级 DI 这件事做完了。而 Fiber 状态机、可用性驱动激活、错误隔离、LIFO 析构、配置 overlay——这些本系统**一行都没有**，才是需要移植的部分。

## 2. Cordis 四层，逐层判定

| 层 | Cordis 里的东西 | 判定 | 理由 |
|---|---|---|---|
| L1 类型级 DI | `Service<out T>`、`[symbols.config]`、`InjectKey`、`declare module` 接口合并、`Proxy` reflect | **不移植** | 运行时即擦除；Rust trait + 具体类型在编译期提供更强保证 |
| L2 组态语义 | `FiberState`、epoch 可用性驱动激活、`provide/inject` 字符串键注册表、5 种事件模式、FAILED-inert、LIFO 析构、inertia 重入串行化 | **移植为 `deepagent-cordis`** | 这是 Cordis 的本体，也是本系统的空白 |
| L3 配置组态 | `EntryOptions` 行、`group`、`include`、patch overlay、journal 原子回写、`isolate` | **移植为 `cordis.patch.toml`** | 取代现在的 `state.json` 里 `BTreeMap<id,bool>` |
| L4 Node loader / HMR | `internal/modules/esm/loader` v1/v2 分支、`ModuleLoader.loadCache` 改写、`require.cache` 改写 | **不移植也不引入** | 无 Node ESM loader 可用；热重载改为 Rust 侧 fiber 重建 + WASM 模块重编译 |

**保留 Cordis 价值的部分恰好是本系统最缺的部分**：`借鉴/deepseek-harness` 的 `cordis.patch.yml` 组态层（`packages/bundle/base/cordis.patch.yml:14-50`）+ journal 原子回写（`packages/loader/src/config/include/src/index.ts:267-344`），而本系统现在的启停状态就是一个 `BTreeMap<id,bool>`（`plugin_service.rs:346`）。

## 3. 目标架构

```text
┌─ deepagent-cordis（新 crate，无业务依赖）────────────────┐
│  Container   字符串键服务注册表（provide / inject / get）  │
│  Fiber       PENDING|LOADING|ACTIVE|FAILED|DISPOSED|UNLOADING │
│  Epoch       provider 指纹串 → 可用性驱动的 load/unload   │
│  Events      emit | parallel | serial | bail | waterfall  │
│  Patch       EntryOptions 行 + group + overlay + journal  │
└──────────────────────────────────────────────────────────┘
        ▲ 注册/析构                        ▲ 组态读写
        │                                  │
┌───────┴──────────────────┐   ┌───────────┴──────────────┐
│ deepagent-app-core       │   │ cordis.patch.toml        │
│ PluginFiber：插件=服务声明 │   │ （取代 plugins/state.json）│
│ 收敛现有 7 个 plugin_*.rs │   └──────────────────────────┘
└───────┬──────────────────┘
        │ 唯一注册通道（返回 Disposer）
┌───────┴──────────────────────────────────────────────────┐
│ 内核权威注册表（不新建）：ToolRegistry / HookDefinitions /  │
│ McpRegistry / SkillsService / SubagentRegistry            │
└───────┬──────────────────────────────────────────────────┘
        │
   ┌────┴───────────────────────────────┐
   │ 插件代码形态（两条，都受 fiber 管） │
   │ a) WASM：wasmtime 27，现有         │
   │    crates/deepagent-tools/src/wasm.rs 目前是空 linker │
   │    无 host import；改为按 capability 白名单挂 host module │
   │ b) 进程侧车：MCP stdio（现有 probe_mcp_sidecar），       │
   │    但进程生命周期由 fiber 的 create/dispose 驱动          │
   └────────────────────────────────────┘
```

### 3.1 核心契约变更（这是"底层影响"的真实范围）

**变更一：注册即 effect，必须返回析构句柄。**
现状 `ToolRegistry::register(&mut self, Arc<dyn Tool>) -> Result<()>`（`registry.rs:76`）——`&mut self` 且无返回值，无法表达"插件卸载时撤掉它贡献的工具"。改为返回 `Registration`（实现 `Drop`，或提供 `unregister()`），并把内部换成 `RwLock` 以支持运行中增量变更。这是 Cordis "every contribution goes through `ctx.effect()`; a registry's `register()` returns the disposer"（`deepseek-harness/packages/AGENTS.md`）的直接对应物。

另需注意已存在的注册表复制模式：`build_hook_agent_registry(source: &ToolRegistry) -> Result<ToolRegistry>`（`hook_runtime.rs:481`）为 hook agent 整份重建了一个注册表。M6.3 改签名时这里必须一并处理——它是"第二套注册表"的雏形，正好落在 §5.1 的禁止范围内。

**变更二：`PluginRuntimeProjection` 的 11 个字段被拆成 11 个独立贡献协议。**
现状 `plugin_runtime.rs:89-104` 是一个 11 字段大结构体，由 7 个子系统各自解构（`hook_assembly.rs`、`mcp_runtime.rs`、`slash_runtime.rs`、`subagent_runner.rs`、`context_runtime.rs`、`run_assembler.rs`、`chat_service.rs`）。重构后每类贡献是一个 trait（`ContributeTools` / `ContributeSkills` / `ContributeHooks` / `ContributeMcp` / `ContributeCommands` / `ContributeAgents` / `ContributeOutputStyles` / `ContributeApps` / `ContributeProviders` / `ContributeMiddleware`），插件 fiber 激活时逐个注册、卸载时逐个析构。**投影结构体整体删除**，不留兼容路径。

**变更三：执行形态从"数数猜"变成"声明"。**
现状 `execution_kind` 由计数启发式推断（`plugin_service.rs:1960-1989`）：`mcp_servers>0 || hooks>0 || apps>0` 就标成 "DSH Sidecar"。重构后由 fiber 声明：`Native` / `Wasm` / `Subprocess` / `DataOnly`，且这个值决定容器给它挂什么 host import。

**变更四：状态持久化换成 patch 行。**
`state.json` 的 `enabled: BTreeMap<id,bool>` 换成 `cordis.patch.toml` 的 `EntryOptions { id, name, config, group, disabled, inject }` 行序列，支持 per-plugin config、group 嵌套、overlay 分层。写入沿用现有 `save_state` 的原子 temp+rename 做法（`plugin_service.rs:2946`），并加 journal 与陈旧写检测。

**变更五：错误隔离与失败语义下沉到内核。**
单个插件 create/start 抛错，现状是记进 `PluginLifecycleState::Failed`（`plugin_service.rs:127`）这个展示态。重构后必须复刻 Cordis 的硬语义：fiber 进 `FAILED` 并保持惰性，直到 `update()` 清掉 `_error` 才允许重入（`packages/core/src/fiber.ts:403`，测试 `core/tests/fiber.spec.ts:87-115`）；兄弟 fiber 不受影响（`fiber.spec.ts:65-84`）；卸载按 LIFO 且可 await（`fiber.ts:281-294`）；reload/unload 由 inertia 串行化（`fiber.ts:406-414`）。

**变更六：启动审计 fail-loud。**
对应 `deepseek-harness` 的 `auditStartupEntries`（`packages/boot/app-boot/src/index.ts:848-853`，把 PENDING 转成 `missing: string[]` 报告）与 `installFailLoud`（`:923-937`）。缺失依赖、重复注册、方言冲突一律启动即失败，不静默跳过。注意本仓库现有缺陷记录：`BadSqlGrammarException` 类"逐个修"的历史教训同样适用于这里——审计要一次性报全，不能撞一个修一个。

### 3.2 不动的部分（明确边界）

`AgentKernel`、`RunStore`、`run_events`、`RunPhase`、`TerminalKind`、`InputLeaseRegistry`、审批路由、取消、Sandboxie/Windows Sandbox 边界、DeepSeek provider adapter——这些是内核权威，Cordis 层只做**组态**，不接管执行、不接管持久化、不接管权限判定。§5.2 的事件可 replay / 顺序稳定 / 终态只落一次三条约束在重构后必须继续成立。

## 4. 迁移阶段（每步可回滚，且不出现双模式）

| 阶段 | 内容 | 退出条件 |
|---|---|---|
| M6.0 | 新建 `deepagent-cordis` crate：Container + Fiber + Epoch + Events，纯机制无业务 | T0 语义等价基线全绿（见 §5.2） |
| M6.1 | 把 `plugin_service.rs`（13,326 行）按职责拆成 registry / health / sidecar / zip-safety / marketplace / dto 六个模块，**纯搬运不改行为** | 现有 102 个内联测试 + `plugin_conformance.rs`(5) + `plugin_real_fixtures.rs`(9) 全绿，diff 审查确认无逻辑改动 |
| M6.2 | 合并两套 manifest 解析：`RawPluginManifest`（`plugin_manifest.rs:164`）与 `parse_portable`（`plugin/spec/v1.rs:142`）收敛为一个；删除 `ResolvedPlugin.manifest` 遗留字段（`plugin/model.rs:85`） | 静态门禁：全仓 `PluginManifest::` 构造点归零或仅存于兼容测试 fixture |
| M6.3 | 引入 `PluginFiber`，把 11 字段投影拆成 11 个贡献 trait；`ToolRegistry::register` 改为返回 `Registration` | T3 REAL-composition + T4 内核集成全绿 |
| M6.4 | `state.json` → `cordis.patch.toml`，含一次性迁移与迁移后删除旧文件 | T1 幂等/回滚测试 + 重启保留测试 |
| M6.5 | WASM 插件形态：给 `wasm.rs` 的空 linker 挂 capability 白名单 host module；侧车进程生命周期交给 fiber | T6 故障注入 + 权限逃逸测试 |
| M6.6 | 前端收敛：`host-plugin-registry.json` 与 `pluginRegistry.tsx:156 BUILTIN_COMPONENT_TYPES` 的重复声明合并为单一生成源；badge 标签三层实现（Rust enum → `types.ts:418-449` → `PluginsViewReal.tsx:1841-1977`）改为 Rust 单源生成 TS | `pnpm build` + 漂移门禁绿 |

**M6.1 必须先行**。13k 行的上帝对象里塞不进容器，历史上这类"先拆后换"的顺序问题已被记为本仓库纪律（同类问题连续三次即停止打补丁、先重审设计边界）。

## 4.1 底层影响面（实测数字，不是估计）

用户关心的"对底层影响比较大"，具体是这三个数：

| 影响面 | 实测 | 含义 |
|---|---|---|
| `ToolRegistry` 生产代码引用 | **208 处**（不含 `tests/`） | `register` 改签名 + 内部换 `RwLock` 的波及范围 |
| 投影字段消费点 | **115 处** | 11 字段大结构体拆成贡献协议时要逐个改的调用点 |
| 触碰 `runtime_projection()` / `PluginRuntimeProjection` 的文件 | **14 个** | 见下方分布 |

14 个文件的分布，按风险从高到低：

```text
高风险（内核执行链，改错即影响每一次对话）
  crates/deepagent-runtime/src/model_agent.rs   ← 模型主循环直接依赖投影
  crates/deepagent-app-core/src/run_assembler.rs
  crates/deepagent-app-core/src/chat_service.rs
  crates/deepagent-app-core/src/tool_runtime.rs

中风险（能力装配）
  hook_assembly.rs / hook_runtime 相关
  mcp_runtime.rs / slash_runtime.rs / subagent_runner.rs / context_runtime.rs

低风险（投影生产方与边界）
  plugin_runtime.rs / plugin_service.rs / app-core lib.rs
  apps/desktop/src-tauri/src/lib.rs
  crates/deepagent-app-core/tests/plugin_real_fixtures.rs
```

`model_agent.rs` 出现在这个列表里是本方案最需要警惕的一点：**投影已经渗到模型请求组装路径**。这意味着 M6.3 拆投影不是"插件子系统内部重构"，而是一次跨到 runtime crate 的改动，必须靠 T4（内核集成）+ T5（快照 replay）两层兜住，否则回归会直接表现为模型行为变化。

同时确认：`apps/cli` 对插件零引用。也就是说 CLI 目前完全绕过插件系统——这是 §5.6 `verify-cli-plugin-parity` 门禁存在的原因，也是重构后必须补齐的验收项（T8 第 4 条）。

---

# 第二部分：工程级测试方案

## 5. 测试分层

比例锚点：Cordis 核心约 1.5k LOC 配了约 6.1k LOC 测试（`packages/core/tests`、`loader/tests`、`hmr/tests`，其中 `hmr/tests` 单文件 spec 1294 行 + 14 个 fixture 插件）。**运行时语义的测试成本远高于业务代码**，这是本计划预算的依据。

### 5.1 分层总表

| 层 | 名称 | 覆盖对象 | 载体 | 门槛 |
|---|---|---|---|---|
| T0 | 语义等价基线 | Fiber/Epoch/Events/析构是否真按 Cordis 语义走 | `crates/deepagent-cordis/tests/semantics_vectors.rs` | 100%，缺一不放行 |
| T1 | 单元 | 容器、patch 解析、manifest 归一、贡献 trait | 各 crate `#[cfg(test)]` | 语句覆盖 ≥90%，分支 ≥85% |
| T2 | 契约 invariant | 每个贡献协议"注册了就必须能观测到、析构了就必须消失" | 每模块 `invariant.rs` + 全局聚合测试 | 每个 trait 至少 3 条 |
| T3 | REAL-composition | 真临时目录 + 真 patch 文件 + 真插件包，走完整 loader | `tests/cordis_loader_smoke.rs` | 每个产品可见插件 1 个 |
| T4 | 内核集成 | 注册→tool loop→审批→取消→持久化→replay | 扩展现有 `kernel_v2_e2e.rs` | 六条链路全绿 |
| T5 | 快照/replay | 模型可见输出与事件序列 | 扩展现有 `golden_trace.rs` | 字节级一致 |
| T6 | 故障注入 | panic/hang/OOM/断连/重入/FAILED-inert/泄漏 | `tests/cordis_faults.rs` | 8 类各 ≥1 |
| T7 | 静态门禁 | 架构约束、漂移、重复实现、克隆 | CI 脚本 | 全绿 |
| T8 | 真实验收 | 桌面 UI + CLI + 真机/真插件 | 手动 + 截图 artifact | 逐条留证 |

### 5.2 T0 语义等价基线（本项目最重要的一层）

做法：**先把 Cordis 自己的测试逐条翻译成 Rust 断言**，作为不可协商的行为规格。来源清单（每条都要有对应 Rust 测试）：

| Cordis 测试 | 断言的语义 | Rust 测试名 |
|---|---|---|
| `core/tests/service.spec.ts:7-34` | `Service.init` 未就绪时消费者不得激活 | `init_gating_blocks_consumers` |
| `core/tests/service.spec.ts:128-165` | Foo→Bar→Qux 乱序注册，三者各初始化一次 | `out_of_order_registration_initializes_once` |
| `core/tests/fiber.spec.ts:7-64` | inertia 使 reload/unload 串行化，不重入 | `inertia_serializes_reload` |
| `core/tests/fiber.spec.ts:65-84` | 一个 fiber FAILED，兄弟 fiber 仍 ACTIVE | `failed_fiber_does_not_take_down_scope` |
| `core/tests/fiber.spec.ts:87-100` | FAILED 后不得重入 | `failed_fiber_is_inert` |
| `core/tests/fiber.spec.ts:102-115` | `update()` 清 error 后恢复 | `update_recovers_failed_fiber` |
| `fiber.ts:281-294` | 析构 LIFO 且可 await | `disposal_is_lifo_and_awaitable` |
| `reflect.ts:189-191` | 同名重复注册硬失败并报出首个注册者 | `duplicate_service_registration_throws_with_owner` |
| `reflect.ts:71,89` | 未注入即取服务报错文案可区分 | `missing_service_error_distinguishes_inject_and_required` |
| `events.ts:117-132` | waterfall 不调 `next()` 即短路，且有只调一次守卫 | `waterfall_requires_next_and_guards_double_call` |
| `context.ts:65-69` | `isolate` 重绑键到全新符号 | `isolate_rebinds_service_key` |
| `fiber.ts:34-46` | config 校验失败在加载期即报错，不延后 | `config_validation_fails_at_load_not_first_use` |

再加本系统特有、Cordis 没有对应物的两条：
- `epoch_refresh_on_provider_swap`：provider fiber 重建后，消费者按 epoch 指纹重算而非全量重启。
- `terminal_state_written_once_across_fiber_restart`：插件 fiber 在 run 中途卸载，运行终态仍只落一次（守 §5.2）。

### 5.3 T3 REAL-composition（对齐 DSH 的硬性政策）

DSH 政策原文（`packages/AGENTS.md`）："Product-visible plugins require a non-unit REAL-composition test. Hand-built `ctx.plugin(...)` suites are insufficient. Boot test-only config through the Loader and app/process; mock only external services or nondeterministic inputs and assert model-visible, durable, or user-visible output."

翻成本项目的执行标准：
1. 在临时目录铺一个真插件包（manifest + skills/ + mcp.json + hooks/ + `.app.json`）；
2. 写一份真 `cordis.patch.toml`；
3. 起真 `PluginService` + 真容器（不 mock 容器、不 mock loader）；
4. 断言的是**模型可见或持久化可见的输出**：工具 schema 出现在请求里、skill 被激活、hook 事件被触发、MCP 工具被列出、`run_events` 里能还原这条链路。
5. 只 mock 外部服务（DeepSeek API、GitHub 市场）与不确定性输入。

现有 12 个内置插件（Boltz / Browser / Computer Use / Figma / Files / 会议记录 / Office Agent / Project Map / Side Chat / Superpowers / Terminal / Wedecode）**每个至少 1 个 T3 用例**。已有的 `plugin_real_fixtures.rs` 9 个测试是起点，不是终点——它现在断的是组件计数与健康状态，不是"模型可见输出"。

### 5.4 T2 契约 invariant（对齐 DSH 的 disposal 证明要求）

DSH 政策："Registry contributions prove disposal through the HMR-safety test: dispose the fiber and observe removal."

每个贡献 trait 必须有三条自动化契约测试：
- `register_then_observe`：注册后，从**权威注册表**（`ToolRegistry` / `SkillsService` / `HookDefinitions` / `McpRegistry`）能读到；
- `dispose_then_absent`：析构 fiber 后，权威注册表读不到，且**没有残留**；
- `no_leak_after_cycle`：反复 enable/disable N 次后，注册表条目数回到基线。

第三条是本系统当前最可能出问题的地方——`sync_plugin_skill_roots`（`lib.rs:1162-1170`）与 `invalidate_connected_registry()`（`lib.rs:1167`）现在是手工调用的失效通知，改成 fiber 驱动后必须证明不累积。Cordis 用 `getHookSnapshot` 对 `ctx.events._hooks` 做泄漏断言（`core/tests/utils.ts:5-94`），Rust 侧对应做法是给事件服务加 `listener_count()` 测试钩子。

注意 DSH 另一条反向约束：invariant 只为"独立观测会发散"的关系发布，空 companion 和被忽略的 reporter 是无效项（`packages/AGENTS.md`）。不要给每个模块机械补一个空 invariant 凑数。

### 5.5 T6 故障注入矩阵

| # | 注入 | 期望 | 现有依据 |
|---|---|---|---|
| 1 | 插件 create 阶段 panic | fiber=FAILED，兄弟 ACTIVE，错误进诊断事件 | `fiber.spec.ts:65-84` |
| 2 | 插件 start 挂起（无返回） | 超时后 FAILED，不阻塞 scope 其余激活 | DSH `loader-smoke` 的 `hang.ts` fixture |
| 3 | WASM 耗尽 fuel | `OutOfFuel` 映射为稳定错误类型 | `wasm.rs:176` |
| 4 | WASM 超墙钟 | epoch interruption 触发 `Interrupt` | `wasm.rs:179` |
| 5 | WASM 尝试调未授予的 host import | 实例化失败，非运行时 panic | 现为空 linker，需新增白名单测试 |
| 6 | 侧车进程中途被杀 | fiber 转 FAILED，MCP 注册表条目撤除 | 现有 `probe_mcp_sidecar` 5s 超时（`plugin_service.rs:3675`） |
| 7 | FAILED 后 update 清错再启用 | 恢复 ACTIVE，不重复注册 | `fiber.spec.ts:102-115` |
| 8 | 卸载中途新注册到达 | inertia 串行化，不出现半注册态 | `fiber.ts:406-414` |

### 5.6 T7 静态门禁（把已知痛点变成 CI 红灯）

| 门禁 | 规则 | 针对的现存缺陷 |
|---|---|---|
| `verify-no-second-registry` | 禁止在 `deepagent-app-core` 内出现新的工具/钩子/skill 注册集合；唯一入口是贡献 trait | §5.1 铁律；现状 11 字段各自解构 |
| `verify-single-manifest-parser` | `RawPluginManifest` 与 `parse_portable` 不得同时有生产调用点 | 两套模型并存（`plugin_manifest.rs:164` vs `spec/v1.rs:142`） |
| `verify-host-registry-no-drift` | 宿主组件表由单一源生成，JSON 与 TS 不再各写一份 | `host-plugin-registry.json:11-21` 缺 `computer-use`，而 `computer-use/.app.json` 声明了 `builtin:computer-use` → `pluginAppToToolCard` 返回 null（已发生的真实漂移） |
| `verify-badge-single-source` | 状态/健康/形态/许可标签只在 Rust 定义，TS 侧为生成物 | 现状三层重复实现（`plugin_service.rs` → `types.ts:418-449` → `PluginsViewReal.tsx:1841-1977`） |
| `verify-no-count-heuristic` | `execution_kind` 不得由 `len()>0` 推断 | `plugin_service.rs:1979` |
| `verify-skill-discovery-single-source` | 插件 skill 计数与运行时 skill 发现必须同一实现 | `plugin/component/skills.rs:56` 非递归 vs `deepagent-skills/loader.rs:24` 递归 |
| `verify-cli-plugin-parity` | CLI 与 Desktop 消费同一容器实例 | 现状 `apps/cli` 零插件引用 |
| `verify-file-budget` | `plugin_*.rs` 单文件行数上限（建议 1500） | `plugin_service.rs` 13,326 行 |
| `verify-no-hardcoded-plugin-names` | 生产代码不得出现 `boltz`/`wedecode`/`superpowers` 字面量分支 | `plugin_service.rs:4014` 的 runtime payload 特判、`bundled-plugins.json` `include_str!`（`:6065-6080`） |

DSH 用 `pnpm run duplication` 做跨文件克隆检测（`package.json:145-157`）。Rust 侧等价做法：CI 加 `cargo clippy --all-targets --all-features -- -D warnings` 之外，再跑一次 `rtk`/`jscpd` 级别的原仓克隆扫描，只针对 `crates/deepagent-app-core/src/plugin*`。

### 5.7 T8 真实验收清单

必须留 artifact，不允许"代码已写好"充当结果：

1. 桌面端 `pnpm tauri dev`，插件页 12 个内置插件逐个开关一次，截图前后状态；确认 `已启用/已禁用`、`需要运行时`、`待补全` 徽章来自声明而非计数猜测。
2. 目录安装 + Zip 安装各一次，含一次扫描拒绝（高风险未确认）。
3. 市场安装三段式 `prepare → commit → cancel` 各一次，含 cancel 后无残留目录。
4. CLI 侧 `cargo run -p deepagent-cli` 列出同一组插件，与桌面端 `DeviceRegistry` 式单一来源一致（`list_plugins` 与 CLI 输出 diff 为空）。
5. 一次真实 DeepSeek 请求，证明某插件贡献的工具出现在模型请求 tool schema 中，且 `run_events` 可 replay 出该 turn。
6. 禁用插件后重启应用，状态保留；启用插件后重启，工具重新出现。
7. 卸载一个带侧车的插件，确认子进程被回收（进程列表取证，不靠日志）。

## 6. 回归保护网与放行门槛

**保护网**：M6.1 拆分之前，先把 `plugin_real_fixtures.rs` 的 9 个测试与 `plugin_conformance.rs` 的 5 个测试跑成基线并记录输出；M6.1 的"纯搬运"提交必须证明这 14 个测试断言未改动。这是唯一能在 13k 行重构中防误改的东西。

**每阶段放行门槛**（全部满足才进下一阶段）：

```text
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace --offline
cargo test -p deepagent-cordis --all-features -- --nocapture
cargo test -p deepagent-cordis --features wasm           # M6.5 起
cargo test -p deepagent-app-core --test plugin_conformance --test plugin_real_fixtures
cd apps/desktop && pnpm build                            # M6.6 起
```

注意 `wasm.rs:245` 已有一个 `#[ignore]`，原因是 wasmtime trap unwinding 在该 Windows 工具链上会打崩测试进程。M6.5 之前必须先解决这个，否则 T6 的第 3、4 条在本地根本跑不出来——不能带着 ignore 声称故障注入已验证。

**评分卡**（沿用 §11.4 的口径，按插件重构调整权重）：

| 维度 | 分值 | 判据 |
|---|---|---|
| 架构边界 | 20 | 无第二套注册表；容器 crate 无业务依赖；投影结构体已删除而非并存 |
| 语义正确性 | 25 | T0 十二条语义向量全绿 |
| 通用性 | 15 | 无插件名硬编码；CLI/Desktop 同源 |
| 测试证据 | 20 | T3 覆盖 12 个内置插件；T2 三契约齐；T6 八类故障 |
| 安全与可恢复 | 10 | WASM capability 白名单、侧车回收、patch 原子写、FAILED-inert |
| 复查质量 | 10 | diff 审查、漂移门禁、未验证项如实列出 |

90 分以下不进下一阶段。出现以下任一情形，本轮 0 分并回退设计：为某个内置插件加名称特判分支；用 mock 容器冒充 REAL-composition；把 `PluginRuntimeProjection` 保留成"新旧并存双模式"；只跑单测就宣称插件贡献能力已验证。

## 7. 已识别风险与未验证项

1. **`ToolRegistry::register` 改签名是破坏性变更**，影响面已实测为 208 处生产引用 + 115 处投影消费点（见 §4.1）。M6.3 动手前需要把这 323 处按"必须改 / 自动兼容 / 无需改"三分类列成清单，作为该阶段的提交切分依据；未做该清单前不得开始改签名。
2. **wasmtime 测试在本机 Windows 工具链上会崩**（`wasm.rs:245` 的 `#[ignore]` 注释）。M6.5 阻塞项，未验证是否可通过 `wasm_backtrace(false)` 之外的配置规避。
3. **热更新语义弱于 Cordis**。Cordis HMR 直接改 `ModuleLoader.loadCache` 与 `require.cache`（`hmr/src/index.ts:359-387`），Rust 侧无等价物；M6.5 的"WASM 模块重编译 + fiber 重建"是本地设计，其正确性完全依赖 T6 第 7、8 条，需实测。
4. **DSH 的 `!!js` 表达式插值**（`loader/src/config/utils.ts:10-20`）在 TOML 侧无对应，M6.4 需要决定是放弃条件组态还是引入受限表达式求值——本文未定，属遗留决策点。
5. 本文所有关于 Cordis / DSH 的行号引自 `借鉴/` 下的本地源码快照，未联网核对上游最新版本；若后续升级 Cordis 版本，T0 语义向量需重新比对。
6. 未执行任何编译、测试或真实设备验证——本轮为方案交付，按规则文档变更只做了 diff 与内容一致性自查。
