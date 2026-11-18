# 模型请求前准备耗时排查（2026-11-19）

## 范围与计时口径

按用户澄清，首次登录、向导及基础设置约 90 秒不纳入请求耗时。本报告只分析已进入主界面后，一次模型请求内部的准备路径，不修改思考规则、缓存协议或远程连接功能。

以同一个 run ID 关联后端日志，起点为 `run_requested`，终点为 `model_agent_transport_invoked`。终点表示运行时调用模型传输层，不是抓包得到的 socket 首字节发送时间。UI 收到的 `model_request_started` 经事件队列转发，不能充当精确网络发送时间。

## 实测结果

原生桌面开发构建，DeepSeek Responses，合成问候。第三个样本是本轮通过 Playwright 脚本在同一个已登录进程中，新开对话、选择模型并点击发送取得；未使用桌面控制插件。

下表各准备项互不重叠，不叠加其父级汇总计时：

| 准备阶段 | 完成向导后首轮 | 同进程第二轮 | 本轮同进程新对话复测 |
| --- | ---: | ---: | ---: |
| 配置、工具注册、审批/Hook 组装及输入接纳 | 1053 ms | 465 ms | 744 ms |
| 上下文组装 `run_context_built` | 1297 ms | 592 ms | 945 ms |
| 挂载 snip 工具时触发投影与计量 | 2937 ms | 128 ms | 253 ms |
| 请求前 Surface 准备 | 494 ms | 242 ms | 640 ms |
| 压力检查 | 284 ms | 116 ms | 220 ms |
| 发布上下文用量 | 201 ms | 134 ms | 208 ms |
| 最终请求计量 | 206 ms | 125 ms | 190 ms |
| 其余准备、持久化、投影和日志间隙 | 237 ms | 135 ms | 221 ms |
| **传输层调用前合计** | **6709 ms** | **1937 ms** | **3421 ms** |
| 传输层调用后至首 token，另计 | 681 ms | 900 ms | 706 ms |

对应 run ID：

- 首轮：`bba584f2-dbd8-419d-bdea-17ad17b9724f`
- 第二轮：`6139c889-beb3-4f3c-b10a-a2abc2ab11d6`
- 本轮复测：`8d3f7ef4-b4af-4a3f-becf-9982b728e255`

本轮复测从 UI 点击前标记到 UI 收到请求开始事件约 3798 ms，包含前端、IPC 以及事件转发，不能与后端 3421 ms 混为同一口径。

## 已确认的原因

### 1. 分词器首次初始化落在发送关键路径

`crates/deepagent-models/src/tokenizer.rs:47` 为 V4 和 V4.1 分别定义进程级 `OnceLock`；第一次调用时才执行 `load_tokenizer`。该函数同步解压内置 gzip、验证约 6.4 MB JSON 的长度和 SHA-256、解析词表，并构造 `PromptTokenizer`。这些步骤使用本地资源，不访问网络，也不执行 Skill。

本例第一次触发发生在：

```text
RunAssembler 挂载 snip 工具
  → ModelAgent::with_snip_tool
  → sync_message_projection
  → publish_context_usage
  → context_measurement / measure_request
  → measure_response_request / count_response_prompt
  → DeepSeekTokenizer::count_wire
  → OnceLock::get_or_init / load_tokenizer
```

因此日志中的“挂载 snip 工具 2937 ms”包含首次分词器初始化及整份请求的序列化、投影和 token 计算，不是执行历史裁剪工具耗时，也不能把整个 2937 ms 都记成纯初始化耗时。同一进程同一分词器族后续复用；切换至另一分词器族仍可能首次加载。

本轮另外启动三个独立诊断进程，直接调用生产代码公开的 `DeepSeekTokenizer::V41.count_text`，输入全为合成文本：

| 测量 | 三进程结果范围 |
| --- | ---: |
| 首次仅计数 `hello`（1 token，包含初始化） | 872.788–888.241 ms |
| 初始化后，130 token | 0.921–1.272 ms |
| 初始化后，1300 token | 9.186–9.829 ms |
| 初始化后，13000 token | 96.836–105.867 ms |

这验证了首次加载成本和后续输入规模成本是两件事；该文本微测不等于完整请求编码计时。不能直接以两次真实请求总耗时之差推算初始化的精确成本。

### 2. 同一份上下文在发送前有多个全量计量入口

`crates/deepagent-runtime/src/model_agent.rs` 中，下列调用均会到达 `context_measurement` / `measure_request`：

- 挂载 snip 后的 `sync_message_projection`（842、1342 行）。
- 预压缩调度判断 `maybe_schedule_prefire`（1889 行）。
- 历史裁剪提醒判断 `maybe_inject_snip_nudge`（2092 行）。
- 压力检查 `maybe_proactive_compact`（1634 行）。
- 发布上下文用量 `publish_context_usage`（1580 行）。
- 实际发送前的 `measure_request`（2705 行）。

`deepagent-context/src/meter.rs:56` 会通过实际协议序列化器计算完整请求的 token 总量。现有 `ContextMeter` 的投影/校准并未阻止上游重复执行这项计量。仅压力检查、用量发布、最终计量这三个单独计时阶段，第二轮合计已达 **375 ms**，本轮复测合计 **618 ms**；Surface 准备和挂载阶段还有其他计量入口。

这部分是持续的本地计算成本，单纯预热分词器不能消除。

### 3. 每轮还会重新组装上下文与运行时对象

`crates/deepagent-app-core/src/context_runtime.rs:1603` 每轮执行 `build_run_context`，包括设置读取、Skill 目录提示整理、系统指令组装、用户输入附件收集和初始预算快照。

`crates/deepagent-app-core/src/system_context.rs:86` 的系统上下文路径还会读取指令、查询 Git 上下文，并按权限决定是否扫描工作区。工具注册和审批/Hook 组装也位于每轮 run 的装配路径（`run_assembler.rs`），不是进程初始化后就完全复用的静态资源。

这些阶段已经量到真实耗时，但其内部单项尚未分别计时；不能把整段上下文组装都归因于 Git、目录扫描、设置读取或 Skill 执行。现有证据也不足以把 MCP ready 事件解释为每轮重新联网连接 MCP。

## 修复边界与下一步

本轮完成定位和脱敏留痕，未修改生产行为。后续优化应分别处理：

1. 把已选模型所需分词器的初始化移出点击发送路径，并处理预热与真实请求同时到达的情况；仅移动等待位置不能作为全部性能修复。
2. 在现有计量边界按完整请求内容复用计量结果，覆盖消息、工具 schema、模型/协议、思考与输出参数、附件路由等失效因素；保留真实上下文变化后的重新计量和溢出保护，不能禁用安全检查换速度。
3. 给上下文组装、工具/Hook 准备增加细分计时，区分可复用静态资源和每轮必须重新读取的状态，再决定缓存边界。
4. 分别复测冷进程、同会话续聊、新会话、切换模型与工具续写，校验请求内容、上下文预算和取消行为，同时测量正式构建。

当前数据来自 debug 构建，本轮复测及微测期间还有 workspace 编译任务；尚未取得独占机器和 release 对照。已有证据能确认同步初始化和重复准备，不能量化系统资源竞争占了多少，也不能据此承诺所有机器 10 ms。

## 验证与本地证据

- 只读关联三次原生运行日志，按单一 run ID 核对阶段时间；各行合计与传输层标记一致。
- 已登录隔离原生窗口经脚本切换到 DeepSeek Responses、新开对话并完成一次真实发送和响应。
- 独立探针实际编译成功，并完成 3 进程 × 10 次计数；首次编译缺少 Windows 原生库搜索目录，补齐搜索目录后成功，未修改依赖或生产源码。
- 本轮仅新增本报告，检查 Markdown、diff 和敏感字段；不以既有编译/全仓测试证明任何性能修复已经完成。

脱敏分析结果：`target/chat-prep-audit-20261119/request-preparation-analysis.json`。
诊断脚本及合成探针：同目录 `analyze-preparation.py`、`component-probe/preparation-resource-probe.rs`。
UI 事件时间线：`target/harness-thinking-20261118/desktop-audit-065907/warm-responses-recheck.json`。
这些本地证据在忽略的 target 目录内，不提交数据库、原始日志、模型正文或凭据。
