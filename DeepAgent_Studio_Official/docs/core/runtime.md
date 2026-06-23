# 运行循环与模型

## 10. 聊天运行全流程

用户发送消息后，前端 `runChat` 调用 Tauri `run_chat`。后端不会阻塞 UI：运行在后台任务中，运行事件通过窗口事件持续推送给前端。

```mermaid
sequenceDiagram
    participant U as 用户
    participant CV as ChatView/Composer
    participant API as api.ts runChat
    participant TC as Tauri run_chat
    participant CS as ChatService
    participant RT as RuntimeEngine
    participant MA as ModelAgent
    participant DS as DeepSeek SSE
    participant TR as ToolRegistry
    participant DB as SQLite EventStore

    U->>CV: 输入 prompt 并发送
    CV->>API: runChat(prompt, sessionId?, runId)
    API->>TC: invoke("run_chat")
    TC->>CS: run_in_session(...)
    CS->>CS: 解析 slash / active project / settings / budget
    CS->>CS: 注册内置工具、MCP 工具、知识工具、技能工具、Office/CodeGraph 工具
    CS->>RT: RuntimeEngine::run(session, task, agent)
    RT->>DB: SessionStart / TaskRunning
    RT->>MA: think(step, observations)
    MA->>DS: ChatRequest + tools + history + system prompt
    DS-->>MA: SSE token / reasoning / tool_calls / usage
    MA-->>RT: AgentDecision
    alt 完成
        RT->>DB: MessageAppended + UsageRecorded
        RT-->>TC: RunCompleted event
    else 工具调用
        RT->>DB: ToolCallRequested
        RT->>TR: invoke tool
        TR-->>RT: ToolOutput
        RT->>DB: ToolCallCompleted
        RT->>MA: 下一轮 observations
    else 需要审批
        RT->>TC: approval-requested event
        TC-->>CV: deepagent:approval-requested
        U->>CV: approve/reject
        CV->>API: resolveApproval
        API->>TC: invoke("resolve_approval")
        TC->>CS: PendingApprovals.resolve
    end
```

### 10.1 `ChatService` 的运行前组装

`ChatService` 会在每次运行前做这些事情：

1. 找到 effective root：优先 active project，否则启动 workspace。
2. 检查成本预算：`CostService::check_budget`。
3. 处理 slash 命令：如 `/plan`、`/execute`、`/model`、`/thinking`、`/help`，能在不调用模型的情况下写入会话。
4. 加载 settings：模型、thinking depth、approval policy、sandbox mode、verification policy、web search、tool search、permission rules、hooks。
5. 构建 `ToolRegistry`：
   - 文件工具：read/write/edit/multi_edit/list/glob/grep。
   - bash 工具：受 allowlist、权限、Hook 和审批约束。
   - todo/task/subagent 工具。
   - web_fetch/web_search 工具，受 provider 设置影响。
   - knowledge_search/knowledge_write。
   - codegraph/project_map 查询工具。
   - office_read/create 文档工具。
   - skill 激活工具。
   - MCP 远程工具，按 server namespace 注册。
6. 构建 system prompt：核心系统提示、工作区规则、知识被动块、技能目录、todo snapshot、plan mode reminder 等。
7. 构建 `ModelAgent`：设置 history、thinking depth、event sink。
8. 构建 `RuntimeEngine`：设置 hooks、approvals、cancel flag、verification plan、tool result budget、decorator。
9. 运行结束后记录 token 成本、自动知识捕获、刷新 UI 事件。

### 10.2 运行循环细节

```mermaid
flowchart TD
    Start[RuntimeEngine::run] --> HookStart[Hook: SessionStart]
    HookStart --> EmitStart[emit RunStarted + SessionRegistered]
    EmitStart --> Step[TurnStarted step]
    Step --> Think[Agent::think]
    Think --> Decision{AgentDecision}
    Decision -->|Complete| Verify{有验证计划?}
    Verify -->|无或通过| PersistMsg[写入 assistant message]
    Verify -->|失败可重试| Observation[把验证反思作为 observation]
    Observation --> Step
    PersistMsg --> Complete[Task Completed + RunCompleted]
    Decision -->|NeedsApproval| Await[Task WaitingApproval + RunAwaitingApproval]
    Decision -->|CallTool| Tool[execute_tool]
    Decision -->|CallTools| Tools[execute_tools 并行/串行分区]
    Tool --> Observe[生成 observation]
    Tools --> Observe
    Observe --> Step
    Step -->|取消标记| Cancel[RunCancelled + Task Failed]
    Step -->|超过 max_steps| Failed[RunFailed step limit]
    Complete --> Usage[UsageRecorded]
    Await --> Usage
    Cancel --> Usage
    Failed --> Usage
    Usage --> HookEnd[Hook: SessionEnd]
    HookEnd --> Cleanup[清理临时 tool result 文件]
```

### 10.3 并行工具调用规则

`RuntimeEngine::execute_tools` 支持同一模型 turn 内的多工具调用：

- 只有只读且并发安全的工具会并行执行，例如读取、搜索、fetch 等。
- 写文件、编辑、bash、高风险或需审批工具按顺序执行。
- 事件写入仍按模型返回顺序记录，确保 append-only event log 可预测、可回放。
- 所有并行工具会先 emit `ToolStarted`，让 UI 立即显示多个 running 工具卡。

## 13. 模型层与 DeepSeek

`deepagent-models` 负责与 DeepSeek 交互：

- `ChatRequest` / wire DTO：构造请求体。
- `ReqwestTransport`：HTTP transport。
- `SSE`/`stream`：解析流式 token、reasoning、tool calls。
- `ThinkingDepth`：控制 reasoning depth。
- `discovery`：模型发现。
- `balance`：余额查询。

运行时中的 `ModelAgent` 会：

1. 接收 system prompt、history、工具定义和 thinking depth。
2. 发起流式请求。
3. 将 token/reasoning/tool_call/usage 转成 runtime events。
4. 将模型输出归约为 `AgentDecision::Complete`、`CallTool`、`CallTools` 或 `NeedsApproval`。

```mermaid
sequenceDiagram
    participant MA as ModelAgent
    participant Transport as ReqwestTransport
    participant DS as DeepSeek API
    participant Sink as RuntimeEventSink

    MA->>Transport: ChatRequest(messages, tools, thinking_depth)
    Transport->>DS: HTTP SSE
    DS-->>Transport: data: token/reasoning/tool_call/usage
    Transport-->>MA: StreamEvent
    MA->>Sink: TokenDelta / ReasoningDelta / ToolCallDelta
    MA-->>Runtime: AgentDecision
```

