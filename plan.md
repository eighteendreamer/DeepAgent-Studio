# DeepAgent Claude Code 级执行内核重构计划

## 总体目标

重建从用户输入到最终结果的完整执行链路，不再修补现有 `ChatService + RuntimeEngine` 循环。新内核以 Claude Code 的 Query 状态机、工具执行管线、上下文管理和终态协议为行为基准，同时保留 DeepSeek/OpenAI 兼容模型、桌面 UI 和现有数据库数据。

依据包括[现有审计报告](G:/Code_Warehouse/DeepAgent-Studio/docs/agent-execution-chain-audit-2026-07-27.md)、[本地 Claude Code 源码](G:/Code_Warehouse/DeepAgent-Studio/借鉴/claudecode/restored-src/src/query.ts)、[官方 Agent Loop](https://code.claude.com/docs/en/agent-sdk/agent-loop)、[Hooks](https://code.claude.com/docs/en/hooks)、[权限](https://code.claude.com/docs/en/permissions)和[子代理](https://code.claude.com/docs/en/sub-agents)。

## 核心架构

1. **统一执行状态机**
   - 新建 `AgentKernel`，状态固定为 `Accepted -> Preparing -> RunningTurn -> ExecutingTools -> Verifying -> Finalizing -> Terminal`。
   - 终态统一为 `Succeeded | Cancelled | Blocked | Failed | MaxTurns | BudgetExceeded | ContextExhausted`。
   - 所有成功、错误、断流、Hook 阻断和取消都经过同一个 finalizer，禁止通过 `?` 跳过任务终态、用量、SessionEnd 和资源清理。
   - `ChatService` 降级为配置装配和 Tauri 适配器，不再持有主循环业务逻辑。

2. **输入与会话入口**
   - `InputIngress` 负责运行互斥、输入排队、发送时中断、图片/粘贴/引用归一化、Slash Command、Skill 直调和普通 Prompt 分流。
   - 原始输入与 Hook 处理后的有效输入分别保存；Hook stdout 只能作为附加上下文，不能混入用户消息或模型正文。
   - 用户输入一经接受就先持久化，再请求模型，确保请求期间退出仍可恢复。
   - 每个用户输入建立文件检查点；恢复、fork、rewind 同时处理消息链和文件状态。

3. **配置与上下文装配**
   - 同时读取 `.deepagent`、`.claude`、`AGENTS.md`、`CLAUDE.md`、rules、skills、agents、plugins 和 MCP。
   - 标量设置优先级：托管策略 > 本次运行 > 本地 > 项目 > 用户 > 插件 > 默认值；同一层级 `.deepagent` 优先于 `.claude`。
   - 权限规则不使用普通覆盖：任意来源 `deny` 优先于 `ask`，`ask` 优先于 `allow`，托管规则不可被低层覆盖。
   - 用一个 `ContextAssembler` 替换桌面端手工字符串拼接，输出带来源、优先级、token 数和缓存属性的 `ContextManifest`。
   - Skills 只常驻名称和描述，正文按需加载；MCP 默认只暴露名称，schema 经 Tool Search 加载。
   - 嵌套规则和 Skills 在访问对应目录时动态发现；加载过程发出 `InstructionsLoaded` 事件。
   - 压缩分为工具结果裁剪、micro-compact、完整 summary；压缩后重新注入项目规则、任务目标、修改文件、失败测试、已调用 Skills 和最近有效轮次，并设置防抖断路器。

4. **模型流与 Query Loop**
   - `deepagent-models` 输出统一的 `ModelStreamEvent`：文本、推理、tool start、tool arguments delta、usage、stop reason、provider error。
   - 增加连接、首 token、SSE idle、单轮和整任务 deadline；显式设置 `max_tokens`、最大轮次和费用预算。
   - 完整工具参数块到达后即可调度，不等待整个 assistant 响应结束。
   - 流重试必须丢弃失败尝试产生的工具结果，并为所有已发出的 `tool_use_id` 生成配对结果，禁止孤儿 tool call。
   - 覆盖 provider 断流、fallback、429/5xx、context overflow、max output、部分响应和不可重试错误。
   - API 错误不运行 Stop Hook；正常无工具响应才进入完成校验和 Stop Hook。

5. **ToolExecutionPipeline**
   - 固定顺序：工具解析 -> JSON Schema -> 工具值校验/规范化 -> PreToolUse -> rules -> permission/classifier -> approval -> sandbox -> execute -> 结果预算 -> PostToolUse/PostToolUseFailure -> 结构化 tool result。
   - 无效 schema 不运行 Hook、不申请权限、不启动进程。
   - 连续只读工具并发；写入、命令和有上下文副作用的工具串行。Shell 失败可取消同批 sibling process。
   - 大输出落入 run-scoped artifact 文件，模型只收到有界预览、摘要和读取路径。
   - Write/Edit 实现 read-before-write、原子替换、修改前备份和修改后文件校验。
   - Windows 暴露 PowerShell，按配置启用 CMD/WSL；Linux 暴露 Bash，macOS 暴露 Zsh/Bash。模型不再每次从七种 shell 中选择。
   - PowerShell/CMD 计算 argv 上限，长脚本自动使用临时文件；Windows 用 Job Object 杀进程树，Unix 用 process group 终止。

6. **Hooks、权限与沙箱**
   - Hook 支持 `command | http | mcp | prompt | agent`，同事件 handler 并行、去重、确定性聚合。
   - 覆盖 Session、Prompt、Instructions、Tool、Permission、Compact、Stop、Subagent、Task、Worktree、File/Cwd 等 Claude 生命周期事件。
   - PreToolUse 阻断优先于 allow；Hook allow 不能覆盖 deny/ask 规则；权限和沙箱保持独立。
   - Hook 测试与真实运行共用 runner、cwd、env、stdin、timeout 和 shell 解析；保存时拒绝重复的 `powershell -Command` launcher。
   - 保留变量、密钥和系统环境不可被 Hook 覆盖；日志默认脱敏。

7. **子代理、校验与总结**
   - `Agent` 工具支持同步、后台、恢复、取消、独立 transcript、工具白名单、模型/effort、权限继承、Skills 预载和 Git worktree。
   - 普通子代理使用全新上下文，仅最终摘要返回父代理；fork 模式显式继承父上下文；默认禁止递归创建子代理。
   - `SubagentStart/Stop` 可注入上下文或要求继续，后台结果通过任务通知进入父会话。
   - `CompletionGate` 根据实际副作用生成验收：读任务无需构建；写任务校验目标文件；删除任务验证路径不存在；代码任务运行发现到的 build/test/lint。
   - 校验失败作为结构化反馈返回模型并允许有限修复轮次；重复失败、无进展和预算耗尽进入明确终态。
   - 最终模型正文作为用户总结；错误、取消和阻断由内核生成统一的事实型终态摘要。

## 公共接口与持久化

- 新接口：`AgentKernel::start(RunRequest) -> RunHandle`、`RunHandle::cancel()`、`ToolPipeline::execute()`、`HookDispatcher::dispatch()`、`ContextAssembler::build()`。
- Tauri 接口：`start_chat_v2 -> {run_id, session_id}`、`cancel_run(run_id) -> {accepted, state}`、`run_events(run_id, after_seq)`；旧 `stop_chat` 暂作兼容适配器。
- `RunEventEnvelope` 必含单调 `seq`、run/session/turn/tool/hook/subagent ID、phase、status、elapsed 和脱敏 data。
- 主数据库新增 runs、run_events、checkpoints、tool_artifacts、subagent_runs；迁移只增表，不破坏现有 session/event 数据。
- `runtime-logs.db` 使用单写入器和统一 span 协议；content delta 批量落库，业务事件不再绕过事件泵直接写库。
- 日志路径沿用安装目录策略；不可写时明确报错，不静默回退到系统盘。

## 实施阶段

1. 建立 Claude/DeepAgent golden trace、状态机契约和可重放 fake model；冻结旧接口行为。
2. 实现 v2 状态机、事件写入器、终态 finalizer、树状取消和数据库迁移。
3. 接入 InputIngress、双格式配置、统一 ContextAssembler、Skills/rules/MCP 懒加载和会话检查点。
4. 重写模型流、Query Loop、预算、重试、压缩和 tool-use 配对恢复。
5. 实现 ToolExecutionPipeline、平台 shell、Hooks、权限、沙箱和结果 artifact。
6. 接入完整子代理、后台任务、worktree、CompletionGate、恢复和最终总结。
7. 桌面端接入 `agent_kernel_v2`；先开发者开关，再小流量默认，达到验收门槛后设为默认并删除旧 RuntimeEngine 主链。禁止生产环境双执行有副作用的工具。

## 测试与验收

- 所有本机系统测试使用 `G:\Code\Kotlin_code\_deepagent-e2e\<run-id>`；不得修改现有 `del_dir.py` 和 `新建文件夹 (3)`，每次创建独立 fixture 并记录清理清单。
- 小任务：问答、列目录、读文件、精确搜索；要求 1–2 个模型轮次，无无关 Skill、计划或 shell。
- 写入任务：创建、编辑、重命名、删除和长脚本；验证真实绝对路径、文件内容、检查点和 rewind。
- 复杂任务：生成带 Git 的多文件 Java 工程，使用 `javac` 和测试脚本执行功能增加、跨文件重构、故障修复和回归验证。
- 大任务：20–30 文件仓库、并行只读分析、同步/后台子代理、人工扩大工具输出触发压缩，再中断、重启和恢复。
- 安全矩阵：三种权限模式、workspace/full access、allow/ask/deny 冲突、Hook 阻断、审批拒绝、越界路径、网络和密钥脱敏。
- Shell 矩阵：PowerShell 5/7、CMD、WSL Ubuntu；Linux/macOS 在 CI 使用 Bash/Zsh 执行等价 fixture。
- 故障注入：无效 schema、Hook 超时、进程挂起、SSE 空响应、部分响应、429/5xx、max output、context overflow、MCP 掉线和子代理失败。
- 硬指标：取消请求 200ms 内确认、2s 内模型流和进程树停止；任意终态后不得残留 running task；无孤儿 tool result；日志序列可完整重放。
- 稳定性门槛：小/写任务各连续 20 次成功率至少 95%，复杂/大任务各 10 次成功率至少 90%；失败必须有可定位的 terminal reason 和完整 span。
- v2 设为默认前，必须通过 Rust workspace、Tauri、前端测试、跨平台 CI、数据库升级/回滚测试以及上述真实工作区测试。

## 已锁定假设

- 行为完整对标 Claude Code，但采用独立 Rust 实现，不直接复制 TypeScript 源码。
- 保留 DeepAgent 多模型能力，不把内核绑定到 Anthropic API。
- 同时兼容 `.deepagent` 与 `.claude`。
- 使用双内核分阶段切换，最终删除旧主链。
- 本轮实现完整子代理能力，不实现实验性的 Agent Teams。
