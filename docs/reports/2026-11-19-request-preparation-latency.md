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

后端发出请求开始事件后，仍会发布上下文用量、做最终计量并记录工具结构。三次样本中，后端请求开始事件到调用传输层分别相隔 447、283、449 ms。表中首 token 耗时取运行时的单调计时值，根据日志事件的时间戳计算则分别为 683、901、712 ms。

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

工具和 Hook 准备已有两个更细的日志区间，属于实测表第一行，不能再次加到总耗时上：

| 日志边界 | 完成向导后首轮 | 同进程第二轮 | 同进程新对话 |
| --- | ---: | ---: | ---: |
| 会话绑定结束至工具注册表就绪 | 315 ms | 122 ms | 225 ms |
| 工具注册表就绪至审批与 Hook 装配完成 | 657 ms | 334 ms | 491 ms |

后一段包含审批和权限对象构建、项目 Hook 配置读取与组装。三个样本中，Hook 装配完成到用户输入被接纳分别只有 6、1、1 ms，指令加载 Hook 的分发也只有 0、2、0 ms。现有日志可以把这段耗时定位到准备阶段，进一步定位仍需给段内各个操作分别计时。

## 新版桌面及其他协议复验

本轮原生开发构建已完成（8 分 58 秒），复用已登录的隔离数据目录，用 Playwright 经 WebView2 CDP 操作实际桌面窗口。按用户指定将 Responses 路由改为 Layso；两个模型均经胶囊选择、显示余额、发送合成消息，页面收到预期回复，后端均为 `run_completed`。这里不包含首次登录和向导，也没有使用桌面控制插件。

仍以第一次 `model_agent_transport_invoked` 为准备终点，不把同一 turn 后续工具续写的第二次请求误算成首次准备。下列各行互不重叠：

| 阶段 | Layso Responses / gpt-5.6-sol | Layso Messages / claude-opus-5-5 |
| --- | ---: | ---: |
| 配置、工具、审批和 Hook 准备及输入接纳 | 2499 ms | 870 ms |
| 上下文组装 | 1810 ms | 1469 ms |
| snip 挂载、Surface 准备、压力检查及两次独立用量计量 | 338 ms | 301 ms |
| 其余准备、持久化、工具结构审计与日志间隙 | 351 ms | 437 ms |
| **首次调用传输层前合计** | **4998 ms** | **3077 ms** |

对应 run ID 为 `2c72d25c-2669-445f-af68-7b3867373cd1` 和 `18de93b8-f945-44ff-92a8-72d20da727c3`。这两条路线没有使用上述 DeepSeek 精确分词器，仍有秒级本地准备；所以仅预热 DeepSeek 分词器不能解决全部问题。

Responses 这次实际执行了两次模型请求：第一轮返回 `skill` 工具调用，工具记录的执行耗时为 2 ms，之后请求模型继续回复。从 `run_requested` 到第一个文本 token 为 12701 ms，包含两次请求及其间的处理，不能整体归因为本地初始化或一次模型首 token 等待。Messages 只有一次模型请求，传输层调用至首 token 的单调计时为 5089 ms；它和本地准备的 3077 ms 分开统计。

### 本地组件对照与尚未细分的边界

直接链接生产 `deepagent-workspace` 的诊断程序，每个目录执行 8 次 `detect_git_context` 和 `WorkspaceScanner::scan`，只输出耗时、是否 Git 仓库及上下文长度，不输出文件内容：

| 对照 | Git 上下文查询 | 目录扫描 |
| --- | ---: | ---: |
| 当前代码仓库 | 225.817–281.856 ms | 17.030–19.803 ms |
| D 盘仓库外空目录 | 49.511–85.964 ms | 0.095–0.133 ms |

代码仓库内新建的空目录仍属于父级 Git 仓库，不能作为“非仓库”对照；正式结果使用仓库外目录。以上是组件微测，不是对真实 `run_context_built` 的内部计时，不能直接从 1810/1469 ms 中扣除这组数，或把余量归给某个未测操作。

另查到每轮 Hook 装配会调用 `RuntimeBroker::build_process_environment`（`run_assembler.rs`），后者调用 `RuntimeService::build_process_environment`，依次解析 Node、Python、Java；`resolve_runtime` 的 `inspect_runtime` 同步启动候选程序执行 `--version`，当前实现没有复用检查结果。这里是运行环境探测，不是安装或下载资源。它发生在 `registry_ready` 到 `hooks_registered` 区间，两次新样本分别为 1858 和 639 ms，但这个区间还包含权限、配置和 Hook 构造，不能全归给版本探测。

对当前诊断进程 PATH 上可用的程序各做 6 次独立 `--version`：Node 为 81.938–120.001 ms、Python 为 30.162–53.437 ms。此进程 PATH 未找到 Java，且未证明与桌面运行环境相同，故这些值只证明启动探测有持续成本。直接链接整个 AppCore 的环境探针因 `E0463` 未能运行，尚未得到完整生产环境探测函数的独立计时；没有把失败探针写为通过。

本轮仍未修改生产性能行为。新增证据位于 `target/chat-prep-audit-20261119/layso-preparation-analysis.json`、`component-probe/workspace-cost-probe.csv`、`component-probe/path-runtime-version-probe.json` 及 `target/harness-thinking-20261118/h030-native-layso-conversations.json`。Messages 新模型发现的真实复验此次没有取得新增样本：该隔离供应商已有接口返回的全部模型，因此刷新不能证明新默认值入库；已有模型配置保留与成功对话不能替代该项验收。

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
