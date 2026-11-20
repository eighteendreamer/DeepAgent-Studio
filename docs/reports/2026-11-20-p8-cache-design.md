# P8：对齐天枢和 DeepSeek Harness 的模型提示缓存

实施基线：2026-11-20。范围是请求前缀、供应商缓存字段、历史整理及计量。接入现有协议表与会话事件流，不增加本地模型响应缓存、额外存储或命中率推断。

## 参考实现与选择

| 边界 | 参考实现 | DeepAgent 的落点 |
|---|---|---|
| 稳定前缀与动态上下文 | 天枢 `src/agent/turn-step-producer.ts` 与 `docs/changelog/2026-07-06-appendix-delta-byte-stability.md`：冻结已有消息；动态内容以增量进入新的用户边界，未变化的字段不重发。 | 保留现有稳定指令、按名称排序的工具与原生历史；移除空闲就清除工具结果的路径。Messages 把每个已验证的运行时快照留在原来的对话边界；顶层 `system` 不随下轮快照增长。历史仍保存完整快照，字段级增量尚未替换。 |
| Messages 显式缓存 | 天枢 `src/api/anthropic-client.ts`：工具尾和系统尾各 1h；首个用户块、末尾 15 个块内最靠前的助手边界各用默认短 TTL。 | 复用本项目现有的合并后消息投影。系统断点落在稳定指令末尾，动态运行时快照随用户边界进入历史；签名思考块不标。官方 DeepSeek 继续自动缓存。 |
| Responses 缓存路由 | DeepSeek Harness `packages/llm/llm-pi-ai/src/adapter.ts` 将会话 ID 与 `cacheRetention` 交给 `@earendil-works/pi-ai`；锁定版本 1.3.0 的 `dist/api/openai-responses.js` 在保留开启时用会话 ID 生成 `prompt_cache_key`。 | 通过现有 `SessionId` 和 Responses SDK 类型发送稳定键。一个会话的工具续写与后续用户消息共用；新会话不同；不发到官方 DeepSeek 原生端点。未引入可配的保留档位。 |
| Chat 缓存 | DeepSeek 原生和兼容服务可直接对相同前缀自动命中；DeepSeek Harness 的 pi-ai Chat 短保留默认不会给所有代理写 `prompt_cache_key`。 | 保持自动缓存，继续读取 OpenAI `cached_tokens` 或 DeepSeek 显式 hit/miss 计数。 |
| 历史压缩 | 天枢 `src/agent/compact-boundary-coordinator.ts` 只检查取消。`context-budget-preparation.ts` 对最终请求判断超预算，最多整理一次、重建并复核；`request-context-controller.ts` 提交有效候选。低命中和空闲时间不等于超限。 | 应用主代理和子代理采用天枢预算公式，输出预留和 wire 上限同源；取消额外提前阈值、自动工具清除与 Grok 后台预压缩。最终请求超预算才整理一次，候选经过同一预算才可写入边界。仅 HTTP 400/422 明确拒绝上下文且尚无流式活动时，允许一次服务端超限恢复。 |
| 签名重放与计量 | DeepSeek Harness `prefix-binding.ts` 以请求前缀摘要限制旧签名重放；天枢 `native-continuation.ts` 跟踪原生续写和缓存断点的对应位置。两者均以供应商回执计量。 | 不改变现有签名持久语义，摘要绑定尚未移植。阅读、创建与未命中字段继续来自原始 usage；没有依据单次 0 宣称缓存失效。 |

## 改造顺序与验收

1. **协议前缀与定位**。先录制现有实际发送的 JSON。长历史旧 Messages 只标最近助手；新测试要求首条用户与远端滚动断点，并覆盖严格 `< 15` 的边界。已有回环与编码回归同时核对 TTL、工具配对、思考签名和自动缓存路由。
2. **运行时改写**。先让预算内请求进入真实运行循环，复现空闲后工具结果被替换成占位符，再移除定时改写。预算测试复现短消息也压缩、无效候选仍发送；绿测覆盖预算内保留、合格候选发送、无效候选不发送。运行时用实际工具和输出上限判定；有匹配模板就使用现有准确 tokenizer，无匹配模板则采用天枢的混合文字估算（拉丁等字符约 4 个/token，中日韩字符约 1.2 个/token）。保留原有供应商用量校准。
3. **Responses 稳定键**。先要求发送请求同会话同键、不同会话不同键、官方 DeepSeek 无键；在 AppCore 工具续写测试检查同一持久会话键。稳定键只用于缓存路由，历史仍从原有事件读出。
4. **真实多轮**。使用 Layso、随机合成说明和三条不同用户消息；追加实际上一轮的可见回复。冷轮、续写轮分开记录；不把重复请求体、回环夹具用量或未执行的桌面路径算成真实命中。`prompt_cache_live.rs` 提供可显式运行的测试，只输出 token 数。

未完成的验收：天枢字段级动态附录、签名摘要绑定，以及 Messages、Chat 真实压缩前后和更多模型配置下的长期多轮曲线。Responses 真实压缩验收见下文。旧的阈值和预压缩接口仍在历史测试审计中，生产组装不再启用。真实纯文本结果只说明所测模型和网关路线可命中，不能换算成系统的长期平均命中率。

## 第一批实测（2026-11-20）

使用 Layso，经项目 `ModelClient` 的流式入口连续发送三轮。解除冷启动影响后，第二、三轮均收到供应商报告的非零缓存读取量。

| 协议 / 模型 | 第 1 轮读取 | 第 2 轮读取 / 输入 | 第 3 轮读取 / 输入 |
|---|---:|---:|---:|
| Messages / `claude-haiku-5-5` | 0 | 11,566 / 11,591 | 11,587 / 11,612 |
| Chat / `gpt-5.6-sol` | 0 | 6,272 / 6,454 | 6,272 / 6,473 |
| Responses / `gpt-5.4-mini` | 0 | 5,888 / 6,454 | 5,888 / 6,473 |

Responses 另有独立的无键对照：一次流式三轮都为 0，另一次非流式到第三轮也有 5,888 个读取 token。会话键用于稳定路由，不保证固定命中率。AppCore `harness_registry_run` 10 项通过；其中本地传输核对工具续写携带相同的 Responses 会话键，切换 Chat 后不携带该字段。这一项验证请求参数；表中的供应商实测用纯文本多轮。

第二批红绿记录：预算内零整理、超预算只发一条合格重建请求、拒绝无效候选的三项运行时测试通过；基于原始模型输入录制的 Messages 测试复现顶层 system 膨胀，修复后该测试及 Messages client 共 8 项通过。

同一真实 Messages 会话另测了运行环境变化和工具续写：首轮读取 0/12,146，运行环境 A→B 后读取 12,142/12,390，投递模型实际发出的工具调用的回填结果后读取 12,386/12,477。工具为只返回合成标记的测试回显，未访问工作区或外部系统。

最终验证：`deepagent-runtime` 单元测试 507 项通过（3 项 ignored），另有 2 项离线 tokenizer 预算回归通过；`deepagent-context` 单元测试 71 项通过；AppCore `harness_registry_run` 10 项通过，生产压缩器的 SQLite 回归确认被拒候选没有写入边界，回放历史保持一致。Models 的 Messages、图片、协议工厂、SSE 和思考声明定向回归合计 33 项通过；`cargo fmt --all -- --check` 通过。

下一批验收的参考边界：天枢的 `src/prompt/engine.ts::buildAppendixBody` 按字段比较渲染后的字节，`resetAppendixBaseline` 在压缩后重新发送基线；必须验证字段不变时不重发、字段消失时旧权限提示不会复活、压缩和重启后发送同一份基线。DeepSeek Harness 在 `llm-pi-ai/src/adapter.ts` 将 `prefix-binding.ts` 产出的摘要与该次模型回应一起保存；验证相同前缀续写可重放签名，改写后签名不会重放，签名失效不会改写原始记录。二者应各自沿现有会话事件和协议边界实现。

## 长会话续测（2026-11-20）

这次按协议各跑 16 轮。每轮追加不同的合成资料和真实上一轮可见回复。每个会话使用独立前缀，首轮均为 0；随后 15 轮均有非零读取。缓存读取率按 `缓存读取 token / 总输入 token` 计算，区间和加权值排除冷轮。以下是同一 Layso 路由中的样本，不能外推为全系统命中率。

| 协议 / 模型 | 热轮有命中 | 读取率区间 | 热轮加权读取率 | 第 16 轮读取 / 输入 |
|---|---:|---:|---:|---:|
| Messages / `claude-haiku-5-5` | 15/15 | 96.41%–97.60% | 97.13% | 18,175 / 18,621 |
| Chat / `gpt-5.6-sol` | 15/15 | 93.15%–96.23% | 94.92% | 10,624 / 11,040 |
| Responses / `gpt-5.4-mini` | 15/15 | 87.88%–95.83% | 92.93% | 10,496 / 11,040 |

Messages 最后一轮同时报告未命中 446、创建 442；创建计入未命中，不能再加一次。最后一轮 Chat、Responses 未命中分别为 416、544，两条路线都没有返回创建计数。第 16 轮量比第 2 轮高，不代表每轮读取比例都在上升（Responses 波动较明显）。

### Responses 真实压缩与重开 SQLite

Layso `gpt-5.4-mini`，使用 `ModelAgent` 的最终请求预算检查和生产 `HookedReactiveContextCompactor`。测试覆盖 5 个对话请求和 1 个真实摘要请求。使用临时 SQLite 和合成历史，把测试模型的本地上下文窗口设为 24,000，输出上限为 512，以触发同一生产预算检查；这不是模型的真实容量配置。各轮使用同一会话键，实际模型回复以原生条目持久化。摘要请求单列，不计入对话命中。

| 阶段 | 读取 / 输入 | 未命中 |
|---|---:|---:|
| 冷轮 | 0 / 10,774 | 10,774 |
| 压缩前热轮 | 10,496 / 10,808 | 312 |
| 独立摘要请求 | 0 / 5,221 | 5,221 |
| 压缩后首轮 | 2,816 / 9,569 | 6,753 |
| 关闭并重开 SQLite 后首轮 | 9,472 / 9,603 | 131 |
| 再续一轮 | 9,472 / 9,637 | 165 |

该样本压缩后首轮部分命中，下一轮恢复到 98.64%。只生成一条持久压缩边界；当前用户消息及其稳定 ID 完整保留；重开前后的回放一致，重开后的实际出站历史与回放逐项一致。压缩事件自己的诊断估算为 88,569 → 59,216，使用不同的计量口径，不能与供应商 token 数或触发预算比较。

相关 AppCore `context_runtime` 单元测试 32 项通过，2 项原有忽略用例未运行。长会话三协议和 Responses 压缩为显式真实请求；尚未测 Messages / Chat 的生产压缩曲线、桌面 UI 或整个 workspace。字段级动态附录、思考签名前缀摘要绑定仍未实现，P8 仍标记为部分完成。

复测时通过本地环境注入 `DEEPAGENT_CACHE_TEST_KEY`、`DEEPAGENT_CACHE_TEST_URL` 和 `DEEPAGENT_CACHE_TEST_MODEL`；长会话另需 `DEEPAGENT_CACHE_TEST_PROTOCOL`（`messages`、`chat` 或 `responses`）。按用例名精确选择，避免一起执行其他真实用例：

```powershell
cargo test --offline -p deepagent-models --features http --test prompt_cache_live extended_history_reports_real_provider_cache_usage -- --exact --ignored --nocapture
cargo test --offline -p deepagent-app-core --features web --lib context_runtime::cache_live_tests::real_request_budget_compaction_survives_reopen_and_reports_cache -- --exact --ignored --nocapture
```
