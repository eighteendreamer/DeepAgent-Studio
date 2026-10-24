# DeepAgent Studio 全系统 Harness 审查

## 审查口径

- 开始日期：2026-10-21；基线提交：`e70504e`。
- 目标：逐项核查 Harness、DeepSeek 适配、上下文、运行与取消、工具与审批、MCP/插件、权限与沙箱、持久化恢复和各宿主入口；与仓库中的成熟 Agent 实现对照。
- 本文随验证推进更新。代码疑点、失败的测试断言和已复现的产品缺陷分别记录，不能互相替代。
- 每项问题需具备触发条件、生产路径、可重跑验证、参考实现、影响及修复方向。尚未覆盖的模块不视为通过。
- 测试密钥、完整请求、用户历史和机器私有配置不进入文档。运行数据位于忽略提交的 `target/`。

## 覆盖进度

| 范围 | 当前证据 | 审查状态 |
|---|---|---|
| Windows Tauri → AppCore → DeepSeek | 当前源码编译、真实连接、连续对话、取消和数据库终态 | 已获得基线，仍需故障注入与边界验证 |
| SSH 远端主机身份验证 | 正式 SshService 通过 loopback 连上临时随机服务器密钥；同一地址的 known_hosts 对照拒绝不匹配密钥 | 已确认 H-125；未连接公网或用户服务器 |
| 本地 MCP 子进程环境授权 | 隔离宿主只注入虚构 canary，经正式 transport 完成 initialize/tools/list；空配置仍继承未声明 secret 和小写 token，显式覆盖对照通过 | 已确认 H-135；仅 Windows 本机 stdio，没有读取真实凭据或验证公网外传 |
| MCP 请求写入背压与取消 | 正式 ChatService/Kernel 发出 512 KiB（初轮 2 MiB）虚构工具参数；真实 Node 读首块后暂停 stdin，取消已受理但写入未结束；释放后完整请求继续到达 | 已确认 H-136；本机管道故障注入，未执行真实远端副作用或桌面按钮操作 |
| MCP 动态工具目录 | 真实 Node 声明 tools.listChanged，在工具响应前发送变更通知并将目录由 initial_tool 切为 updated_tool；正式宿主未重新发现，下一独立运行的真实序列化 schema 仍为旧工具 | 已确认 H-137；本机 stdio、离线模型；1 秒为测试观察窗口，不是协议规定期限 |
| MCP 结果内容与客户端元数据 | 正式 MCP 返回公开文本、structuredContent 与仅位于顶层 _meta 的虚构 canary；第一次模型 wire 无 canary，工具返回后的下一请求包含它 | 已确认 H-138；仅本机 stdio + 离线模型；这是客户端/模型投影缺口，不宣称 2024-11-05 基础协议为所有 _meta 提供统一保密保证 |
| 已执行工具与后置 hook 的结果提交 | 正式 write_file 已写入磁盘且 run_events 有成功 ToolCompleted；PostToolUse prompt 返回无效 JSON 后，session 没有真实结果，重开将动作标为进程重启失败，下一请求只有补造的结果缺失错误 | 已确认 H-139；正常 hook 对照通过；真实临时文件、文件数据库、正式启动恢复与离线模型，不含真实桌面或公网请求 |
| 输入 hook 的模型上下文输出 | 正式 UserPromptSubmit PowerShell 命令成功返回普通文字或标准 additionalContext，实时 HookCompleted 保留原 stdout，但下一正式 wire 与会话消息均没有该内容；直接输入和拒绝决定对照正常 | 已确认 H-140；Windows 真实子进程、临时输入捕获文件与离线模型；未覆盖其他 hook 事件、续聊或重启 |
| 跨独立用户回合的 todo reminder 生命周期 | 同一 session 连续 11 次正式 ChatService/Kernel 请求，每次重新组装 ModelAgent；模型每轮均成功返回，但任何实际 wire 请求都没有周期提醒 | 已确认 H-141；真实 session 事件恢复与离线 provider；未修改生产行为 |
| stdio Harness / SDK 事件交付与恢复 | 事件队列在确定性消费者停读和真实 Windows 子进程 stdout 管道背压下均丢失通知；另有立即中断窗口、恢复丢覆盖配置、最终钩子失败时先完成后失败、审批事件重复且有一条缺少 ID、并发生产时实时事件序号倒退、EOF 时运行时直接丢弃活动 turn、一个已被替代 turn 重复派生多个新 turn，以及 SDK 丢弃通知外层 eventSequence 均已复现；两轮各自游标和独立磁盘补读对照正常 | 已确认 H-01、H-02、H-04、H-83、H-84、H-91、H-92、H-130、H-205；通知丢失尚无自动恢复，H-44 的恢复终态投影仍未修复 |
| Harness thread 归档生命周期 | 正式 `thread/archive` 已成功并写入 durable archive marker 后，同一 dispatcher 的 `thread/list` 仍公开该 thread，随后 `turn/start` 仍返回成功并创建 `TurnLaunch` | 已确认 H-215；未实际 spawn turn、调用模型或工具；重启后 `thread/resume` 仅完成静态路径核查 |
| Harness 冷分叉响应就绪性 | 文件 SQLite 中的 source 在新 ServerState 尚未加载时，正式 `thread/fork` 成功创建 durable child，但 child 未进入活动 map，紧接的 `turn/start` 被拒绝；显式 `thread/resume` 后正向对照可运行 | 已确认 H-216；只创建 `TurnLaunch` 对照，未 spawn turn、调用模型或工具 |
| DeepSeek 请求、reasoning、工具协议、usage | 真实请求和 usage 返回；三轮真实 session 请求观察到 cache hit 为 12288 / 2944 / 13184 tokens（仅为本次样本）；同一 AppCore 会话双轮 wire 测试确认完整 Responses input 回传、稳定 instructions 前缀逐字节相同、未变工具 schema 顺序相同；Chat 工具投影拆分、MCP 图像工具结果被序列化为普通 JSON 文本、Responses 工具输出图片附件被静默降为文本、截断调用、命名工具选择字段形状和 Thinking 组合限制、连续 reasoning item 丢段、JSON 输出模式拒绝/静默丢弃、top_logprobs 默认值静默忽略、错误解析/重试/取消缺口、流内错误丢码阻断溢出恢复、固定 300 秒 stream deadline、SSE 分帧不完整、备用模型输出上限分裂，以及 Chat `content_filter`、合法 `aborted`/`insufficient_system_resource` 终态、Responses failed usage 丢失、空/重复 `call_id`、空输入、malformed function-call 输出边界、官方 `tool_search_call` 静默丢失、Responses 兼容 effort 别名拒绝、模型目录丢失 model-level effort、条件性 `apply_patch` custom schema 超出文档明示 shape、Responses named `tool_choice` 接受官方不支持类型、MCP 非对象 `inputSchema` 原样进入 function `parameters`、end-user 标识符未校验、Responses 响应身份/序号未进入运行事件、非单调 `sequence_number` 被接受，以及温度/top_p/top_logprobs 越界值可通过公开 ModelClient 进入 wire 均已核查 | 已确认并修复 H-24、H-25、H-29、H-118；已确认 H-05、H-18、H-30、H-49、H-93、H-94、H-98、H-99、H-127、H-131、H-132、H-133、H-144、H-145、H-146、H-147、H-148、H-149、H-150、H-151、H-152、H-154、H-155、H-156、H-157、H-158、H-159、H-172、H-173、H-174、H-200、H-208、H-209、H-210、H-211、H-212；H-153 的规范值请求链已修复，别名缺口见 H-155；H-156 默认工具集不触发且未证明 provider 拒绝；H-157 仅为按公开元数据形状复现的条件性风险，当前官方模型触发情况未知；H-158 已证明两种不支持的对象进入 wire，未向真实 provider 验证其拒绝/忽略方式；H-159 已证明三类非法 MCP schema 进入 wire，未向真实 provider 验证其具体错误；H-172 的当前 AppCore 设置入口已有范围校验，问题限定于公共 ModelClient/ResponseDefaults 边界；H-173 证明公共消息/Responses 类型无法保留工具输出图片，但未证明当前默认工具已有可达图片附件生产路径；H-174 限定于公开 `replace_messages` mutator/未来 Harness 或 SDK 复用，不代表默认桌面路径已触发；H-200 的正常答案不受影响，但 provider 关联与流完整性诊断不可回放；H-208 限定于能到达 Responses parser 的畸形/截断终态，不代表 DeepSeek 正常服务会主动返回该对象；H-209 限定于能到达 Responses parser 的缺失 output-message role，不代表正常供应商响应会省略该字段；H-210 限定于能到达 Responses parser 的重复、跳号或逆序 sequence，不代表正常 DeepSeek 服务会主动发送非法序列；H-211 限定于 JSON-RPC server 返回字符串 response id，不代表当前默认 server 一定使用该 id 形状；H-212 限定于 JSON-RPC server 返回非 `2.0` 的响应版本，不代表当前默认 server 一定发送错误版本；H-93/H-94/H-98/H-99 均不覆盖默认 Responses 路由 |
| Responses reasoning 与 user 标识 | H-153/H-154 的规范值贯通与 user 标识符校验已修复；本轮复核发现开发者 JSON 拒绝官方兼容 effort 别名，且 thinking guide 与 endpoint schema 对强度字段有差异；模型目录会丢失模型级支持档位 | H-153 仅关闭规范值贯通缺陷；H-155 已确认设置校验缺口，真实服务端对 guide 字段差异未验证；H-154 已修复；H-157 仅证明未来模型能力收窄时的条件性风险，当前官方模型档位无触发证据 |
| 上下文、压缩、记忆 | 预取生命周期、压缩回放、压缩计费、旧笔记覆盖新分支及同步摘要取消缺口均已复现；真实笔记提取请求计数假设错误已澄清；原生摘要取消已接入运行取消树并通过回归 | 已确认并修复 H-31；已确认 H-10、H-11、H-14、H-23；其他压缩边界仍在核查 |
| 工具、审批、权限与沙箱 | 工作区拦截、backend override；迟到审批、跨运行审批冲突、审批事件重复且有一条缺少 ID、子 Agent 工具限制失效、未信任项目提权、批量工具绕过超时及读操作越过写屏障 | 已确认 H-03、H-15、H-16、H-19、H-28、H-34、H-35、H-84；其他边界未完成 |
| MCP、插件与 hooks 生命周期 | MCP 协议、stdio/HTTP 响应无 transport 字节上限、图像工具结果未转为模型多模态内容、一次性测试连接在 initialize/tools/list 阶段无界等待（已修复并验证 30 秒总握手上限）、禁用插件技能残留、标准 hooks 决策忽略、内联项目 hooks 绕过信任、hook 公共输入缺字段及事件专属字段缺失、Pre/PostCompact 输入契约/指令未生效、工作树/FileChanged 字段不兼容、后台通知缺类别、权限拒绝缺调用 ID、步数上限失败仍发完成 hook、API 失败漏发 StopFailure、Windows 后代清理、配置变更误取消、断线耗尽后能力未退役，以及续聊时 SessionEnd 按每个 turn 派发均已复现 | 已确认并修复 H-128、H-129；其他已确认 H-06 至 H-09、H-12、H-13、H-26、H-27、H-46 至 H-48、H-102 至 H-117、H-126 至 H-127；其他 hooks 和准备边界未完成 |
| 持久化、崩溃恢复、并发与输入租约 | 取消存储故障不挡信号；排队取消推进；桌面吞掉保存失败、备用宿主误终结活跃子记录、双宿主同会话同时发模型、完整工具结果引用结束即失效、启动恢复缺少终态投影，以及同一 session 多 run 的中断原因串扰均已复现 | 已确认 H-17、H-32、H-33、H-36、H-44、H-95；完整恢复及并发压力未完成 |
| 共享设置的多宿主提交 | 两条独立文件 SQLite 连接下，刷新模型用宿主缓存的旧完整设置覆盖另一服务已成功收紧的沙箱默认值 | 已确认 H-87；第三个新连接读回落盘结果，未启动真实双进程桌面 |
| 在途刷新与凭据撤销/替换 | 正式 SQLite 加密存储下，另一连接已退出或换 Key，旧模型刷新返回后仍写回旧凭据，并读回 configured=true | 已确认 H-88；两个独立数据库连接和第三连接回读，未验证真实 Tauri 多进程操作 |
| 模型目录刷新与用户选型 | 同一 SettingsService 顺序选择仍在目录中的 Pro，刷新完全相同目录后，下一正式 Responses wire 从 Pro 改成 Flash，新数据库连接也读到 Flash；默认 Flash 对照不变 | 已确认 H-223；正式 SettingsService/ChatService/Kernel/序列化与文件 SQLite，无并发、无公网、无工具执行；不是 H-87 的多宿主旧快照覆盖 |
| Responses 消息角色 provider 校验 | 公开 `ResponseInputItem::Message` 与 `InputMessage` 使用任意字符串 role；`moderator` 经正式 ModelClient 进入 wire，官方四种角色的正向对照通过 | 已确认 H-203；离线 recording transport；普通 AppCore 的 `Role` enum 不产生该非法值，未请求真实 provider |
| Harness 显式模型选择 | 未知模型被正式 AppCore 请求静默替换为 catalog 默认模型，运行仍以 succeeded 终止 | 已确认 H-89；实际捕获序列化 wire model；未调用公网供应商或启动真实 stdio/Tauri |
| 画布工作流模型身份 | 新建 LLM/分类/提取/Agent V2 节点动态生成裸 `deepseek-chat` 默认值；正式 Canvas bridge 将该非空但不可解码的值解释为自动路由，受控 HTTP wire 实际发送 `other-text-model` 并成功返回 | 已确认 H-160 的前端默认与 gateway 缺口；H-162 证明当前四类专业节点会在更早阶段失败，故尚未证明它们已通过完整工作流把数据发往错误供应商；本机 HTTP provider，无真实 Tauri 或公网请求 |
| 画布工作流生成参数 | LLM 节点的 `llmTemperature` / `llmMaxTokens` 经 WorkflowAgent 完整进入 CanvasCompletionRequest，但 Canvas gateway 重建内部请求时丢失；正式 Chat wire 只有 model/messages/stream | 已确认 H-161 的 bridge/serializer 缺口；H-162 当前先阻断专业 LLM 节点，创作 text 节点没有这两个 UI 字段；DeepSeek 思考模式下 temperature 本就不生效，但 max_tokens 仍有效 |
| 专业工作流 AI 节点执行能力 | 前端标为 enabled 的 LLM/Agent V2/问题分类/参数提取节点经正式图编译与 WorkflowAgent 执行：前两者被 Canvas prompt contract 拒绝，后两者没有 executor；四组 provider 调用数均为零 | 已确认 H-162；本机完整 WorkflowAgent→Canvas gateway 路径，无 Tauri/公网；同时收窄 H-160/H-161 的当前可触发范围 |
| 工作流模型 usage 与成本归集 | 可执行的创作 text 节点收到完整 DeepSeek Chat usage 后，bridge response、WorkflowAgent 和正式 CostService 账本依次变成无 usage、全零、全零；run-level 旧分支仅保留 prompt/completion/total gauge，billed input、cache 与 reasoning 仍为零 | 已确认 H-163；本机 provider/MockTransport 与正式 finalizer/SQLite 成本库，无公网或真实费用；H-63 仅覆盖复核/分类附属请求，不包含本链 |
| 成本日历边界与真实预算接纳 | 正式成本库中本日/本月合成历史费用共 13 元、限额 5 元，但 `summary` 按 UTC 日界/epoch 固定 30 天桶只计 1 元；两组完整 ChatService 仍调用模型且各有唯一 succeeded；当前时刻账单的日/月拦截对照正常 | 已确认 H-224；当前宿主 UTC+08、实际系统时间、临时文件 SQLite 与离线 transport，没有修改系统时钟或产生真实扣费 |
| 首次登录的运行时计价激活 | 未配置启动的报价刷新先结束后，正式 initialize 已持久化 Flash 价格，但既有 CostService 仍空；离线 Responses usage 经正式链路完整入账，费用行 NULL、summary=0，有限预算下第二轮仍成功调用模型 | 已确认 H-225；正式服务生命周期/完整 ChatService/文件 SQLite 与离线 transport；已配置启动、显式刷新同步两组对照可正确计价并阻断第二轮，未运行真实 WebView |
| 备用模型切换的成本归属 | 正式 ChatService 在受控过载后真实 wire 切换 Flash/Pro，但 finalizer 把完整累计 usage 按初始模型入账；双向切换分别导致下一轮预算误放行/误拦截，混合模型工具运行亦错价 | 已确认 H-226；文件 SQLite、正式模型选型/Responses parser/Kernel/CostService，四组不切换对照正常；仅离线响应与合成费用，没有真实扣费 |
| 画布同步模型请求的运行取消 | 正式 `text-gen` 已进入本机 provider 且响应保持 pending 后，取消被受理但工作流 300ms 内不退出；释放响应后 text 节点发布 Completed，运行却落唯一 cancelled 终态 | 已确认 H-164；动态仅覆盖 text。image/audio/embedding 只确认请求 DTO 与共用 HTTP `call()` 同样没有取消通道，尚未逐项动态复现 |
| 画布供应商返回资产的下载边界 | 正常 image-gen 供应商在成功 JSON 中返回 loopback 图片 URL 后，宿主直接第二跳 GET，零审批，并把私网 canary 完整存入 artifact；运行仍 succeeded | 已确认 H-165；正式 WorkflowAgent/Canvas gateway/文件 SQLite/RunStore，本机两台 loopback 服务；没有访问公网、真实内网或元数据服务，也未证明数据已外传给供应商 |
| DeepSeek Responses 思考模式的 top_p 有效值 | 正式 ModelClient 在 `reasoning.effort=high` 且设置 `top_p=0.2` 时对 Flash、Pro（无工具/有工具）均将 0.2 原样发到 Responses wire；官方对这些组合分别规定 0.95、1.0、0.95 的有效行为 | 已确认 H-166；离线正式 serializer/wire 探针；没有真实 API 调用或回答质量 A/B，不能证明每次输出必然改变 |
| 画布生成图片响应无字节上限 | provider 资产 URL 返回 32 MiB+1 的本机 PNG 响应后，宿主完整缓冲并存入 artifact，image 节点与 run 均 succeeded | 已确认 H-167；和 H-165 共用双 loopback 故障注入，容量边界独立于地址授权；未测试进程内存耗尽或长期磁盘配额 |
| DeepSeek 多轮缓存与动态 instructions | 工作区结构在第一轮后变化时，第二轮虽完整保留第一轮 `input`，但 leading `instructions` 在抵达历史前已经变化；`<<<DYNAMIC>>>` 只保护基础 system 前缀，不能保护此前会话历史的缓存资格 | 已确认 H-168；正式 AppCore 双轮离线 wire 复现；这是缓存命中潜力、延迟与费用缺口，不是回答正确性缺陷，也未调用真实 provider 证明本次必然 miss |
| DeepSeek 图片输入格式边界 | 桌面同形入口把 SVG 归为 image 并成功持久化；第二轮正式 Responses wire 原样发送 `data:image/svg+xml;base64,...`，而官方只支持 JPEG/PNG/GIF/WebP | 已确认 H-169；真实 AttachmentService、ChatService、Kernel、ModelClient 与 serializer，离线 transport；未调用真实 provider，未声称已观测其具体错误码 |
| DeepSeek Files API 与历史图片复用 | 同一合法 PNG 的后两轮正式 Responses 请求均重复携带完整 data URL，`file_id` 始终为零；生产模型链只有 attachment→data URL，没有 Files 上传、provider file identity 或失效刷新 | 已确认 H-170；真实 AttachmentService、ChatService、Kernel、ModelClient 与 serializer，三轮离线 wire；未调用真实 Files API，不声称 token 或缓存必然改善 |
| Responses 图片角色契约 | 合法 `developer` 图片被静默降为纯文本；官方禁止的 `assistant` 图片也被去图后成功发送，provider adapter 没有按 Responses 角色矩阵保留或拒绝内容 | 已确认 H-171；公开 ResponseRequest/ResponseInputItem→ModelClient→wire 离线复现；普通桌面当前只产生 user 图片，故触发面明确限定为 provider seam、未来 Harness/SDK 或导入历史 |
| Responses 数值参数最终校验 | AppCore 设置解析拒绝越界值，但公共 ModelClient 对显式 request 和 ResponseDefaults 都把 `temperature=2.1`、`top_p=1.1`、`top_logprobs=21` 原样送入 transport | 已确认 H-172；离线正式 ModelClient wire 探针；限定为越过设置层直接构造调用，不代表常规桌面设置当前可触发；未请求真实 provider |
| Responses 流身份与顺序元数据 | 合法流为 `response.created`/`response.completed` 提供同一 response ID、实际模型和单调序号；正式 ChatService→Kernel 落盘后两条事件仅保留 Runtime envelope 与 event_type | 已确认 H-200；正常连续离线 Responses fixture、正式 run_events；未制造乱序/丢包，也未声称已观察真实供应商流损坏 |
| 无限画布工作流视口缩放锚点 | 画布已平移后，底栏放大/缩小/重置/滑杆与小地图放大/缩小只改 zoom、不重算平移 offset；缩放前后的视口中心映射到不同世界坐标 | 已确认 H-201；动态执行六个生产 TSX handlers；与 Penguin-Magic 中心锚定按钮缩放及指针锚定滚轮缩放实现对照；没有图数据或运行状态副作用 |
| 无限画布工作流视口持久化 | 正式图写入只序列化 `nodes/edges`，新 WebView 的 viewport 回到 `{x:0,y:0,zoom:1}`；ReactFlow 初次挂载执行 fitView，没有 per-mode default viewport | 已确认 H-202；动态调用真实 `workflowPersistence` 的读/写/flush 并创建新 store 实例；图节点恢复对照通过；未启动 Tauri WebView |
| Harness SDK 请求生命周期与协议验证 | 关闭 stdin 后，仍存活的客户端克隆调用返回 BrokenPipe，但 request id 留在 pending reply 表；initialize 接受既无 `result` 也无 `error` 的非法 JSON-RPC response 并标记已初始化；`thread/start` 对缺少 `threadId` 的成功结果仍返回 Ok；畸形 stdout 行被忽略后请求一直 pending 到 EOF；对端收到合法请求后静默且保持连接时，SDK 调用没有期限，调用方取消后 pending 仍保留；字符串 JSON-RPC response id 被强制转成整数 0，真实响应无法关联到 pending 请求；非 `2.0` 的 JSON-RPC response 仍被当作成功交付；任意 notification method 只要 params 可解码为 HarnessEvent 就会被公开广播；调用方在请求返回后才订阅时，请求响应前已到达的合法通知永久丢失 | 已确认 H-90、H-100、H-101、H-204、H-207、H-211、H-212、H-213、H-214；Windows 子进程夹具；未验证真实第三方 server 对畸形响应/输出的兼容行为，H-214 另有预先订阅正常对照 |
| Harness JSON-RPC 方法分派 | 未知无斜杠 method 返回 `-32601`，但未知 namespaced method 因包含 `/` 被误报为 `-32602`；已知方法坏参数仍正确返回 `-32602` | 已确认 H-217；正式 `ServerState` dispatcher，无模型、工具、文件或网络副作用 |
| Harness thread/read 历史读取 | 对含 600 条消息的持久化 thread，正式 `thread/read` 一次返回 1201 条 session events、约 1.57 MiB JSON；没有页大小、`nextCursor` 或 `hasMore` | 已确认 H-218；正式 SQLite/EventStore/ServerState，未启动模型、工具、子进程或网络 |
| Harness tool/list 能力作用域 | 两个不同 workspace、`read_only/direct` 与 `full_access/windows_sandbox` thread 连续调用 `tool/list`，均返回相同 28 个工具；响应没有 thread/run 身份、snapshot ID、schema hash 或 backend 条件 | 已确认 H-219；正式 ServerState/ChatService dispatcher，无模型、工具执行、子进程或网络 |
| Harness thread/list 历史列表 | 创建 120 个真实持久化 thread 后，正式 `thread/list` 一次返回全部 120 条、约 25 KB JSON；请求没有 limit、cursor 或 continuation | 已确认 H-220；正式 SQLite/EventStore/ServerState，无模型、工具、子进程或网络 |
| Harness config/read 有效配置可观测性 | thread 已指定模型、reasoning、permission 和 sandbox 后，正式 `config/read` 仍只返回 `protocolVersion`、固定 `provider` 和 `transport` 三个字段 | 已确认 H-221；正式 ServerState/Settings/dispatcher，无模型、工具、子进程或网络 |
| Harness thread/start 的 DeepSeek reasoning 配置贯通 | `thread/start` 请求携带 `reasoningEffort=high` 后，正式 `turn/start` 无覆盖的 launch 仍为 `reasoning_effort=None`；请求字段被 Serde 静默忽略 | 已确认 H-222；正式 ServerState/dispatcher，未启动模型、工具、子进程或网络 |
| CLI / SDK / 桌面 / 工作流与子 Agent 一致性 | 子 Agent 工具限制、DAG 文件交付、工作流装配/调度/配置、完成事件串话、订阅竞态、旧画布 run 的迟到节点事件覆盖新状态、父任务超时丢子运行收尾，以及 app-server 旧 turn 重复创建 steer 替代运行均已复现 | 已确认 H-19 至 H-22、H-38 至 H-41、H-43、H-45、H-130、H-134；记忆现有回归通过，其他图执行及重启边界未完成 |
| 定时任务与无人值守运行 | Cron 不可达审批、两进程重复触发同一任务、派发后记账失败重启重触发，以及 scheduler 停止后已派发 run 仍活跃均已复现；基础 cron/store 回归通过 | 已确认 H-37、H-42、H-96、H-97；真实模型副作用、原子接纳与宿主进程退出恢复边界未完成 |
| 移动设备操作与 UI 快照 | 正式 MobileService/Android backend 接受已被替代和未知快照，登记 ID 与返回 ID 不一致 | 已确认 H-50；底层命令 runner 为离线记录器，真机及并发输入未验收 |
| 项目记忆与知识检索作用域 | 捕获 A 项目的正式工具/检索后端后激活 B，原后端实际读取 B 并将 project 记忆写入 B 的磁盘目录；工作流节点声明 `knowledgeBaseId=project` 时仍混入 global 命中 | 已确认 H-51、H-176；H-51 是共享服务身份漂移，H-176 是工作流配置到检索 trait 的作用域契约缺失 |
| 工作流目标节点与重跑 admission | `target_node_id` 传入不存在的节点时，正式工作流只完成 `start`、跳过 `end`，仍记录 `succeeded` | 已确认 H-177；无效目标未在编译/接纳边界拒绝，属于空执行成功，不是 H-39 的条件分支派发问题 |
| 工作流合法目标节点的依赖闭包 | 整图执行时 target 正常得到中间祖先的 `SOURCE_VALUE`；指定同一个合法 target 后，祖先被标记 Skipped，target 因 required output unavailable 失败 | 已确认 H-196；节点运行按钮既不重算依赖闭包，也不加载带 revision 的既有输出快照 |
| 工作流公开输出类型契约 | end 声明 `count:number`，实际引用 start 的字符串 `NOT_A_NUMBER`；正式运行仍发布 end=Completed、输出 string 并记录 `succeeded` | 已确认 H-197；编译器只验证类型名称，运行时不验证成功输出是否符合声明类型 |
| 工作流 OR 条件求值 | 使用后端当前原生条件字段，两个 OR 子条件都为 false，条件节点仍输出 `branch=true` 并成功 | 已确认 H-198；OR 累加器错误地从 true 开始；当前 H-39 的双分支派发会掩盖一部分执行范围后果 |
| 工作流多 case 的 ELSE 端口 | 两个 case 均不匹配时 executor 定义返回 `false`，但 UI 与 graph compiler 在 case 数大于 1 时都不提供 `false` handle；显式 ELSE 边被 admission 拒绝 | 已确认 H-199；分支结果集合与可连接 source handle 集合不闭合 |
| 专业画布代码节点 | 前端公开 `codeScript`/`codeInputVariables` 并标记 `enabled`，后端只读取未使用的 `code`/`codeVariables`，没有 JavaScript/Python 执行器；明确无效脚本仍完成并返回空对象 | 已确认 H-178；能力注册、配置 schema、运行时 executor 契约断裂，属于静默伪成功 |
| 专业画布文档提取节点 | 前端 `document-extractor` 标记 `enabled`，但正式工作流执行到节点即返回 `UnsupportedNode` 并失败 | 已确认 H-179；这是公开能力目录与实际 executor 不一致，和 H-178 的静默成功不同 |
| 专业画布人工输入节点 | 前端 `human-input` 标记 `enabled` 并提供表单/超时，正式工作流没有进入 `NeedsApproval`、输入租约或可恢复等待态，而是直接 `UnsupportedNode` 失败 | 已确认 H-180；人工输入 schema 与统一人机协作生命周期断裂 |
| 专业画布迭代节点 | 前端 `iteration` 提供 `inputVariable/outputVariable/parallel/errorHandling/flatten`，合法数组输入进入后端后却得到空数组并成功；executor 只读取未公开的 `iterationCollection` 等字段，也不调度迭代体 | 已确认 H-181；控制流与数据契约断裂，属于静默丢数据/伪完成 |
| 专业画布直接回答节点 | 前端 `answer` 公开 `answerTemplate`/`answerVariables`，正式运行时 `execute_answer()` 读取另一套 `text`/`variables`，配置的答案变成空字符串但 run 仍成功 | 已确认 H-182；用户可见输出契约断裂，属于静默空结果 |
| 专业画布变量聚合节点 | 前端 `variable-aggregator` 公开 `aggregatorVariables`，正式运行时只读取 `variables`；合法绑定被静默丢弃，节点输出空数组但 run 仍成功 | 已确认 H-183；数据变换输出契约断裂，属于静默丢数据 |
| 工作流必填输入 admission | `start.inputVariables` 中完整声明 `required=true`，调用者不提供该输入时，正式运行把它填成 `null` 并记录 `succeeded` | 已确认 H-184；输入校验缺失，必填约束只存在于前端 schema |
| 工作流输入类型 admission | `start.inputVariables` 声明 `type=number`，调用者却提交字符串时，正式运行原样接受并记录 `succeeded` | 已确认 H-185；类型约束只存在于 schema，未贯通运行边界 |
| 工作流未声明输入 admission | start 只声明 `known` 时，`WorkflowRequest.inputs.unexpected` 仍被复制到 start 输出并随成功运行继续传播 | 已确认 H-186；输入 allowlist 缺失，未声明字段不是 rejected/ignored，而是进入执行上下文 |
| 工作流声明默认值 | start 声明可选输入 `default="FALLBACK"` 且调用方省略该字段时，后端写入 `null` 并记录 `succeeded`，未应用前端公开的默认值 | 已确认 H-187；默认值只存在 UI schema，未进入后端 admission/执行计划 |
| 工作流字符串选项 admission | start 声明 `options=["safe","fast"]` 时，实际输入 `"UNDECLARED_MODE"` 仍进入 start 输出并记录 `succeeded` | 已确认 H-188；枚举约束只存在 UI schema，未进入后端 admission/执行计划 |
| 工作流数值范围 admission | start 声明 `type=number, minimum=10, maximum=20` 时，实际输入 `3` 仍进入 start 输出并记录 `succeeded` | 已确认 H-189；数值边界只存在 UI schema，未进入后端 admission/执行计划 |
| 工作流字符串长度 admission | start 声明 `minLength=5,maxLength=8` 时，实际输入单字符字符串仍进入成功路径并记录 `succeeded` | 已确认 H-190；字符串长度约束只存在 UI schema，未进入后端 admission/执行计划 |
| 工作流图入口结构 | 只有 `end` 节点、没有 `start` 的定义仍被编译、执行并记录 `succeeded` | 已确认 H-191；图编译只禁止多个 start，不要求入口/终点结构完整 |
| 工作流图终点结构 | 只有 `start` 节点、没有 `end` 的定义仍被编译、执行并记录 `succeeded` | 已确认 H-192；拓扑 order 用尽被当作正常完成，缺少明确 end 终态 |
| 工作流入口到终点可达性 | `start` 与 `end` 都存在但无连接边时，两个独立根仍按全图拓扑顺序执行并记录 `succeeded` | 已确认 H-193；执行计划不是从 start 出发的可达子图，孤立节点可参与成功运行 |
| 工作流执行范围的声明顺序稳定性 | 保持节点 ID、配置和边不变，只交换孤立节点与 `end` 的数组位置，完成节点集合从 `start,end` 变为 `start,orphan,end`，两个 run 却都为 `succeeded` | 已确认 H-194；`end` 立即完成整个 Agent，拓扑 ready 的数组下标优先级决定孤立节点执行或静默消失 |
| 工作流多个可达终点的结算语义 | `start` 分叉到左右两条都可达的普通路径、每条各有 end；正式运行完成左侧 end 后即成功，右侧可达分支没有任何终态事件 | 已确认 H-195；编译允许多个 end，但执行器把遇到的第一个 end 当作全 run 终态，没有 reject、join 或 choice 语义 |
| 知识库多宿主写入一致性 | 两个正式服务持有旧索引，依次保存不同标题但同 slug 的笔记，两次成功却只剩后一条；主动刷新对照保留两条 | 已确认 H-52；独立服务/真实磁盘复现，未启动两个桌面进程 |
| 压缩后的后台记忆去重与预算 | 原生 adoption 移除摘录后，旧 ID 仍阻止再次注入；12 条旧记录使后续预取停止 | 已确认 H-53；同运行 adoption/wire 探针，未覆盖真实摘要模型和回放 |
| 每轮模型参数优先级 | 显式 effort 及持久化输出上限经正式 ChatService 执行后，实际请求仍服从桌面深度，输出预算也随阶段变化 | 已确认 H-54；共享 AppCore 入口、事件回调及真实序列化，未调用公网供应商 |
| 两协议思考模式映射 | 两协议三档中仅 Chat Simple 在请求前本地校验失败，Responses 三档与 Chat 另两档完成 | 已确认 H-55；公开 Chat adapter 的条件缺陷，默认生产组装仍为 Responses |
| MCP 快照合并后的名称唯一性 | 两真实 stdio 服务独立 Ready 后，冲突工具名在每轮 ToolRegistry 注册才被拒绝，普通聊天没有发模型 | 已确认 H-56；正式宿主准备和 ChatService，未覆盖桌面错误展示 |
| 能力目录和说明在后续无状态请求中的保留 | MCP 说明和技能目录均在首轮存在，第二轮完整 wire 中消失；新会话恢复，技能 reset 也恢复 | 已确认 H-57；正式 AppCore 连续运行与请求序列化，未覆盖重启/压缩和真实供应商 |
| 附件导入/删除的身份与路径边界 | 正式 remove 接受空/点号 ID 删除整桶；ingest 接受点目录并覆盖上级文件；父目录 session 可使写入与删除越出 root | 已确认 H-58；Windows 临时目录真实文件复现，未调用真实用户目录或桌面 IPC |
| 可选 WASM 工具的超时归属 | 成功调用遗留的 watchdog 增加共享 Engine epoch，使后续调用在自己的期限前中断 | 已确认 H-59；Windows 隔离子进程对照；默认未启用 wasm，不外推为普通桌面卡顿原因 |
| 命令输出读取任务的归属 | drain 超时后正式 helper 仍持有缓冲，调用方释放后继续读入迟到输出；正常 EOF 释放 | 已确认 H-60；真实 Windows 进程管道/helper 复现，完整执行器/UI 内存与其他平台未验收 |
| 可选对抗复核的取消与任务归属 | Kernel 已受理取消仍等待三个复核请求，返回 PASS 后记录成功；总时限结束后请求仍存活 | 已确认 H-61；正式 Kernel/ModelAgent/模型复核及真实临时文件证据，传输为离线受控响应，未验桌面和公网 |
| 可选停滞检测期间的停止 | 主答案之后同步分类不接收取消，取消只能等分类结束再成为 Cancelled | 已确认 H-62；正式 ModelStallClassifier/ModelAgent/Kernel 离线请求对照，正常完成及最终取消映射成立；生产分类超时 20 秒，未实等该上限 |
| 可选复核/分类的用量归属 | 完整 provider Response 返回 usage 后仅保留判定文本，真实 Finalizer/成本库只保存主调用量 | 已确认 H-63；两条正式模型检查及 SQLite 账本复现，未测真实供应商费用；已有摘要 side_usage 账本能力不因此失效 |
| 自动完成验证的真实构建生命周期 | 正式发现的 cargo check 已启动 build.rs 后，用户取消仍等待；Kernel 超时落终态后 build.rs 仍可写文件 | 已确认 H-64；Windows 真实临时 Cargo 工程、SystemCommandRunner、ModelAgent/Kernel/SQLite 对照，未覆盖其他构建系统和桌面 UI |
| 自动验证与宿主命令边界 | 显式 cargo check 经配置后端被拒绝，但 .txt 修改后的完成验证、创建 .rs 后的单文件验证均绕过该后端，真实 build.rs 写到 workspace 外 | 已确认 H-65；完整 ChatService/工具/Kernel/真实 Cargo 与拒绝型 backend 复现；不是实际 Sandboxie/Windows Sandbox 容器逃逸测试 |
| DeepSeek 原生搜索的模型能力选择 | Flash 开启 DeepSeekFirst 时，正式请求只剩官方明确会忽略的原生 web_search，本地 function 搜索描述被替换；Pro、外部搜索、关闭搜索对照成立 | 已确认 H-66；启用桌面同用的 web feature，正式 ChatService/Kernel/序列化离线复现，未实测公网搜索或回答质量 |
| 当前轮图片与原生输入投影 | 正式导入的可读图片已在用户事件中持久化，本轮实时 Surface 却无图片；已有会话再发新图，下一轮仍无图，原生镜像也未保留附件 | 已确认 H-67；正式 AppCore 两组共五轮、真实附件文件及 wire 对照，模型 transport 为离线原生回答，未测真实识图质量或桌面点击 |
| 历史图片读取失败的语义 | 相同初始图片在续聊中可正常携带；临时原图丢失后，同一路径静默发送纯文字且运行成功，调用方未收到附件错误 | 已确认 H-68；正式 AppCore/Kernel/文件解析/序列化两组对照，只删除测试自己导入的临时文件，未验证真实桌面提示及其他文件故障 |
| 多图请求的字节预算 | 两张各约 20 MiB 的合法 PNG 均通过导入和单图限制，续聊实际序列化出 55,977,624 字节请求，超过官方 48 MiB 上限仍交给 transport | 已确认 H-69；真实图片解码、AppCore/Kernel/序列化单图与双图对照，未向公网发送超限图片或验证供应商错误码 |
| 历史附件的内容身份 | 导入时已计算并记录 SHA-256；原图文件被另一张合法 PNG 替换后，同一事件附件 ID 在续聊中发送新字节，旧 metadata 未变且运行成功 | 已确认 H-70；真实导入、文件替换、解码、正式续聊和 wire 字节哈希对照，仅操作测试临时文件，未证明非授权写入能力 |
| 发现能力与请求输入模态 | 官方形状的 input_modalities 在发现/目录恢复中丢失，声明仅文字的 Pro 仍经正式客户端发送 input_image；Flash 图片和 Pro 文字对照正常 | 已确认 H-71；ModelDiscovery/目录 serde/ModelClient/实际序列化离线复现，不含完整 AppCore 图片装配或真实供应商错误 |
| 检查点备份丢失时的回退与分叉 | 正式 rewind/fork 均先恢复健康文件，再删除缺少备份的当前文件后报错；原会话历史和检查点记录未变，分叉未创建 | 已确认 H-72；真实临时文件、正式检查点及 AppService 双入口故障注入，完整备份对照通过；未操作用户文件或真实桌面 |
| 回退后检查点的分支归属 | 旧分支回退后其检查点仍参与新分支的序号查询；新分支仅回退纯文字尾部，文件却被成功恢复为旧分支内容 | 已确认 H-73；正常 Session/检查点/AppService 两次回退和临时磁盘复现，所有备份完整，无磁盘故障注入或真实桌面操作 |
| 源事件与原生镜像之间的分支边界 | 以用户 MessageAppended 的 sequence 为保留点，正式 fork/rewind 成功且源消息存在，下一次 ChatService wire 却缺该消息；包含下一条镜像的对照正常 | 已确认 H-74；两个入口各两组、正式恢复及请求序列化离线复现；限定原始序号锚点，未宣称桌面默认最新位置分支同样触发 |
| 运行中回退与迟到结果提交 | 模型请求已发出但回答未释放时，正式 rewind 成功删除本轮输入；旧回答随后仍写入回退后的历史，原运行成功终结 | 已确认 H-75；真实 ChatService/RunStore/Session/AppService 与可控离线 transport 双组复现，没有真实桌面点击、文件工具或公网请求 |
| 普通文件工具的读取版本与覆盖保护 | 已读文件被外部保存后，write_file 仍凭旧缓存存在即成功覆盖；不改文件及外部改动后重读的对照正常 | 已确认 H-76；正式 ChatService/工具注册/流水线/真实临时文件及下一次 wire 结果复现，没有时间竞争或真实用户文件操作 |
| 搜索与敏感文件读取策略的一致性 | read_file 被中央 path_guard 拒绝的 .env/credentials/server.key，仍可通过 grep 返回内容并进入下一实际请求 | 已确认 H-77；正式 ChatService/中央 hook/文件工具/序列化三组及普通文件对照；仅临时虚构 canary，无真实凭据或公网传输 |
| 工作区物理路径与 Windows 目录联接 | WorkspaceWrite 下词法位于根目录的 junction 指向根外时，read_file 仍能读取外部临时文件并成功完成 | 已确认 H-78；Windows 临时 workspace/outside、真实 `mklink /J`、正式 ChatService/PathGuard/文件工具与下一 wire 复现，未接触用户目录 |
| 文件发现与敏感读取名称策略 | read_file 被拒绝的 `.env` 仍通过 `glob("**")` 和 `list_dir` 返回文件名并进入下一实际请求；普通文件名仍可见 | 已确认 H-79；正式 ChatService/中央 hook/文件工具/序列化与普通文件对照，缺陷探针显式失败；仅临时 fixture，无真实凭据或公网传输 |
| Responses 工具调用类型配对 | 公共 `ResponseRequest` 修复入口只按 call_id 选择已有 output；若外部传入同 call_id 的混合 native kind，错误类型仍进入下一次 wire | H-80 条件性确认；正常 Session 入口的 custom/function 配对对照通过，只有手工构造或未证实的坏历史才触发失败探针；未调用公网供应商 |
| Responses 多文本 item 与增量去重 | 同一响应的两段完成文本被保留后，整次响应的增量全文又作为第三项加入；done-only 则额外补空 assistant，均进入恢复后的 wire | 已确认 H-81；正式 ChatService/ModelClient/SSE 累加器/磁盘存储重开/下一 wire，单 item 对照正常；离线事件，无真实模型频率或计费结论 |
| Responses incomplete 的原因与恢复策略 | content_filter 与 max_output_tokens 均触发加大输出上限及注入长度恢复提示，离线第三响应完成后运行成功终结 | 已确认 H-82；正式 ChatService/Kernel/ModelAgent/请求序列化/SQLite 终态，长度对照正常；未测试真实供应商过滤或桌面展示 |
| 运行完成通知与最终钩子的提交边界 | 正式 TaskCompleted prompt hook 输出无效决策 JSON 后，stdio 同一 turn 先通知 completed 再通知 failed，thread/read 保持唯一 failed 终态 | 已确认 H-83；真实 dispatcher/spawn_turn/AppCore/Kernel/磁盘事件及独立 reader；正常钩子和普通模型错误对照正常，未跑真实 stdio 管道或桌面 |
| 画布云端视频任务的取消等待 | 正式 ChatService/WorkflowAgent/Kernel、本机 HTTP、文件 SQLite 与 artifact；视频状态响应停滞和下载 body 停滞取消均已验证能中断本地等待 | 已修复 H-119；结论仅覆盖 video submit/poll/download，不覆盖 text/image/audio/embedding（见 H-164）；未执行真实云端生成或桌面点击，云端任务撤销仍不在本地保证范围 |
| 工作流节点事件的持久化通道 | 正式视频工作流实时与重开后回放均得到相同 6 条节点事件；包含 task ID、artifact 来源和运行终态 | 已修复 H-120；未验真实桌面崩溃/断线重连 |
| 视频运行中 task ID 的前端投影与续跑 | 正式 Rust publisher 的 Running/job_id 经生产 TS executor 保存到节点，停止后下一请求继续携带同一 videoTaskId；Completed 对照保持一致 | 已修复 H-121；实际跨语言事件/序列化探针，Tauri 传输替换为本机内存桥，未发生真实云端重复收费 |
| 视频修改输入及配置后的执行身份 | 修复前真实 TS/Rust 往返曾轮询旧任务；修复后输入/provider 指纹绑定 task，输入或来源变化提交新任务，同输入同来源仍可续跑 | 已修复 H-122；本机 HTTP 往返和指纹单测通过；未验真实供应商视频、远程取消或云端计费 |
| 本地与 SSH 终端的分页读取 | 修复前真实 Windows ConPTY / SQLite 与正式 SSH reader 会在 64 KiB 返回上限时把游标推进到缓存末尾；修复后两端均按实际返回末尾推进，后页可完整取回保留输出 | 已修复 H-123；未验证真实桌面游标重连、跨进程恢复、Linux/macOS PTY 与 SSH 网络 |
| 系统视觉发送前图片预处理与停止 | 修复前正式 App submit/stop callback 与 API client 的受控图像请求中，run 尚未进入后端就显示运行；cancel 返回 not_found，释放请求后仍识别下一张并提交主模型；修复后取消信号覆盖视觉预处理和 admission 竞态 | 已修复 H-124；修复后回归 3/3 通过（续聊、新会话预处理取消及已接纳文本对照）；使用实际 App 回调但 React/Tauri/provider 为隔离 harness，不是真实窗口/网络 |

## 阶段性架构判断（审计仍在进行）

### 现有内核有可保留的基础，入口与生命周期的一致性仍不足

`AgentKernel` 已定义 Accepted → Preparing → RunningTurn / ExecutingTools → Finalizing → Terminal，使用现有 RunStore、事件与取消树；`RunCoordinator` 提供跨入口控制信号，provider wire 集中在 `deepagent-models`。这组职责分离有明确价值：本报告的真实桌面普通对话、只读工具、取消与重启继续，以及正式 AppCore 的工具副作用和子运行测试，已证明部分组合能实际闭环。运行包与 MCP 慢准备不进入聊天等待、取消审计失败仍传播信号、原生摘要压缩沿运行取消树退出，也有独立动态证据。

当前结构性缺口集中在这些抽象被不同入口组合后的不变量：ACK 早于真正接纳（H-02），会话租约的 owner 仅存在单个宿主内（H-33），子运行配置/工作树/产物归属分裂（H-19～H-22），工作流另行装配工具与结果解释（H-38～H-40），启动恢复缺少规范终态和会话投影（H-44），且恢复中断原因按 session 全日志分类而没有按 run/task 隔离（H-95）。因此本轮建议在既有 AppCore/Kernel 统一执行配置、运行归属和提交边界，按真实入口回归；这些判断并不要求新增第二套执行器或持久化。

### 对 DeepSeek 的适配：已有可用链路，协议与配置边界存在确定性缺口

- 当前 `model_runtime.rs::select_run_model` 组装默认 Responses 路由，真实桌面基线验证了该配置的连接、连续对话、工具和 usage。另有公开 Chat Completions adapter；两者的通过范围应分别记录，不能用默认路由成功覆盖其他协议。
- H-05 已证明 Chat 往返转换改变同次 assistant 的 reasoning / text / tool-call 归属；H-18 已证明截断调用仍进入提交；H-55 则在 Chat Simple 发请求之前就本地失败。这些直接影响无状态续轮中模型看到的历史和工具协议，不能只靠提示词弥补。DeepSeek Harness 的 `llm-deepseek/serialize.ts` 保留块归属并检查未闭合工具，提供了对应的协议边界参考；其 Messages 字段与本系统 adapter 字段应分别处理。
- H-54 的显式 effort / 输出上限与实际 wire 不一致，以及 H-49 的备用模型预算分裂，会使用户选择、上下文预算和供应商参数相互矛盾。应由一次有效请求配置解析同时供 meter、请求和恢复使用；参考实现对本次 options 与连接 defaults 的优先级有显式处理。
- H-66 进一步证明能力启用不能只看全局开关：默认 DeepSeekFirst 在 Flash 上将本地可调用搜索替换为官方会忽略的原生工具。原生搜索是否可用应由有效模型与协议共同决定；不支持时采用明确的外部搜索配置或返回设置不可用，不能宣称搜索已开启却移除可调用入口。
- 无状态请求必须重新携带当前有效内容。H-57 的 MCP 说明和 skill 目录首轮存在、第二轮正文消失，是当前确定性不适配点。需要保存实际内容或在每次请求边界重新注入；仅记录“曾发送过这个名字”不满足续轮要求。
- H-67 显示当前多模态输入也有双源问题：附件虽已落盘，实时 Surface 却由纯文字 goal 创建；Session 自动生成的原生镜像又把带图消息转成纯文字。首条图片可经旧格式兼容转换在下一轮出现，已有原生会话再发的图片则在下一轮仍缺失。应统一当前用户输入的文字、附件与持久化归属，不能用底层图片 serializer 的单测通过替代 AppCore 请求与恢复验收。
- H-68 是另一处附件语义缺口：即使图片已经能从历史恢复，读取失败仍被 Option 映射成正常纯文字请求，运行可成功。附件恢复、请求准备和错误通知必须区分“没有图片”与“应有图片但不可读”；修复当前输入的归属并不能自动修复这条降级路径。
- H-69 表明 token 容量不能替代媒体传输预算：两张均符合单图限制的图片在 base64 后已超过请求体上限，却仍进入发送边界。参考 Harness 的请求图像转换、Files/inline 预算及显式 offload，本系统需在既有 provider seam 单独校验字节/图片数量，不能仅提高 context window 或依赖 provider 拒绝。
- H-70 进一步表明附件路径不是稳定内容身份：即使 metadata 已有导入哈希，请求时不校验仍可在相同历史引用下发送不同图片。DeepSeek Harness 将内容摘要作为 durable ref 并验证读取结果；本系统需把已有摘要接入持久化引用与读取边界，不能只在 UI 导入结果中显示哈希。
- H-71 补充模型能力契约的另一断点：容量数字能够保留，但 input_modalities 被丢弃，正式客户端也未按目标模型限制图片。能力解析需同时供正常请求、模型切换、备用模型和附件准备使用；不能把 H-66 的搜索特判扩写成分散的模态特判，或仅以模型名称推测能力。
- H-81 补充 Responses 实时增量与完成 item 的双源问题：同一响应含两个文本 item 时，已保存的两段又被整段 fallback 重复补入；最终回答、持久化和重开后的下一请求一致地保留重复内容。Codex 将 delta 用于活动 item 的流式展示、done item 用于完成处理；本系统需要按 item 身份确认哪些增量尚未由完成项覆盖，不能用整次全文与某一 item 的字符串相等替代归属判断。
- H-82 表明供应商终态类型还不足以决定恢复方式：官方 `response.incomplete` 的原因包含输出上限和内容过滤，本系统丢掉原因，统一生成 Length，导致后者也升级输出预算并注入“输出到限”的自动续写提示。Codex 保留独立 ContentFilter 类型及对应处理；本系统需在 provider seam 保真原因，再决定是否及如何恢复，不能让通用长度恢复接管所有 incomplete。
- 上下文占用已有正向证据：运行内工具循环、跨用户 turn 和数据库重开样本都保留完整历史，并区分 provider 输入锚点、下一请求投影和累计计费。DeepSeek Harness 的 token-meter 同样区分这些概念；其 UI 分项是启发式组成且不要求与 provider 投影相加一致，本系统的分项守恒属于自身实现选择，不能把它写成参考项目的共同保证。真实 tokenizer 误差和全部压缩/附件路径仍待验证。

### “能增强模型”的证据与尚未证明的部分

这里的增强指运行系统提供可执行工具、工作区与权限、可恢复历史、检索、压缩及任务编排，使模型能完成需要环境操作的任务。本报告已有真实文件读取、正式写入/校验、上下文恢复等行为证据；普通对话中共享插件准备不会阻塞的证据也已成立。

可靠性缺口会直接抵消这些能力：工具目录/说明丢失使能力难以被模型发现（H-57），能力重名使普通对话无法开始（H-56），工具失败冒充节点成功使后续推理依赖错误前提（H-38），完整结果资产结束即失效使续轮无法读取依据（H-36），记忆作用域漂移会读写另一项目的数据（H-51）。这些是对增强机制的具体阻断，有各自复现条件，不能按缺陷数量推断每轮都会失败。

目前未完成固定任务集下的同模型、同权限、同工具与供应商条件的成对评测，没有总体任务成功率、成本/正确结果或长任务恢复率对照。因此可以确认系统已经提供若干环境能力，也可以确认上述可靠性问题；尚不能证明整体任务表现增强幅度，或宣称优于 DeepSeek Harness / Codex。

### 与 Codex 等成熟实现的主要设计差距

| 边界 | 本系统已复现差距 | 参考实现中的具体机制 | 触发后的主要后果 |
|---|---|---|---|
| 执行身份与控制 | ACK、租约、审批和实际运行的归属未全部统一（H-02/H-15/H-16/H-33） | Codex 按 turn 管理 waiter、thread writer 保护；Claude 活跃 owner/锁保护（见各项源码与探针） | 停止虚假确认、迟到或串运行审批、同会话并发推进 |
| 归档状态与运行接纳 | archive marker 未进入活动列表、恢复和 turn admission（H-215） | DeepSeek Harness 以 `ArchivedSessionGate` 在 pre-step 拒绝归档 lineage；Codex 归档前卸载、Shutdown 并 teardown，resume 拒绝 archived thread，恢复需显式 unarchive | 已归档 thread 仍可见并创建新 turn；隐藏/归档语义不能阻止后续运行 |
| Thread 生命周期响应原子性 | 冷 source 的 fork 已持久化 child，但成功响应未使 child 可立即接纳 turn（H-216） | DeepSeek Harness fork 在返回前 await `agents.create`，客户端声明同步可寻址；Codex 构造并注册 live fork、附加 listener，并明确 response 到达即 usable | 客户端收到成功后首个 turn 失败；重试 fork 可留下多个非预期分支 |
| JSON-RPC 方法错误分类 | 未知 namespaced method 由字符串是否含 `/` 决定为 Invalid params，而非按方法注册表返回 Method not found（H-217） | JSON-RPC 规范将 `-32601` 定义为 method 不存在、`-32602` 定义为参数无效；Codex 对 unsupported/unknown operation 使用独立 `method_not_found()` | SDK/代理按错误码选择重试、降级或兼容路径时得到错误结论；未知扩展与参数错误无法区分 |
| 历史读取与响应预算 | `thread/read` 将完整 session events 与每个 run 的 events/actions/approvals/artifacts/graph 聚合到单个 JSON response，无 page size、响应字节预算或 continuation（H-218） | DeepSeek Harness `SessionHistoryController.page()` 按消息对齐分页并有 500 条上限；Codex 将 full-history hydration 标为 deprecated，改用 `thread/turns/list` 与 `thread/items/list` 分页 | 长历史在 app-server、SDK、CLI 或 UI 同时占用内存并阻塞单次响应；断线重试放大成本，无法在有限预算内恢复或只取窗口 |
| 能力目录与运行作用域 | `tool/list` 只调用一个 `ChatService::tool_descriptors()`，没有 thread/run、workspace、permission、sandbox、MCP 代次或 schema hash；不同线程得到相同静态数组（H-219） | Warp 根据 execution mode/session/feature flag 动态协商能力；Codex 将工具、permission profile、sandbox/root 等作为 thread/turn 配置与可回放快照；项目自身升级方案也要求 run-scoped capability snapshot | 客户端把全局目录误当作本次 run 的可执行能力；backend、权限或 MCP 变化后模型可能生成当前入口不能执行的工具，历史重放无法确认当时工具面 |
| Thread 列表与控制面规模 | `thread/list` 直接枚举 `EventStore::list_sessions()` 并把所有 session 放进一个 response，没有 limit/cursor/filter/快照语义（H-220） | Codex `thread/list` 默认 25、上限 100，支持 sort/filter 与 opaque cursor；列表和 thread/read、turns/items hydration 分开 | 大量历史 thread 让 app-server、SDK 和 UI 每次刷新都承担全量扫描与 JSON 解析；列表刷新延迟和内存随历史增长，断线重试放大负载 |
| 费用周期与预算准入 | 声明的本地本日/本月费用被 UTC 午夜/epoch 固定 30 天桶代替；当前月已达限的历史费用可被漏计，完整 ChatService 仍调用模型（H-224） | Grok billing 显式传递 period type/start/end，Codex 的 provider limit 保留 window/reset 身份；Claude 日期统计按日历日期归组（helper 用 UTC，不能冒充本地日预算）；没有查到可直接移植的同类本地月预算 | 本地限额错误放行；其他日期/时区可能将上期费用带入而误拒绝。日历周期、本地策略和供应商账期未建立明确契约 |
| 登录配置与计价 owner 一致性 | 首次 initialize 更新 settings 却不激活已有 CostService，正常有价模型仍产生 NULL 费用；有限预算聚合将其当 0 后继续接纳（H-225） | Claude 在公共 usage 处理路径按 resolved model 查询成本规则，并对未知成本显式标记不准确；Grok 从当前认证上下文读取 billing，错误明确返回。它们不是同类 Tauri 初始化实现 | 相同模型/用量因宿主初始化顺序得到不同费用，首次登录后成本与本地限额失真；日志/CostRecorded 的 None 未进入 summary 完整性 |
| 备用模型与已计费用量归属 | fallback 已更新实际模型，但结算只收到初始 model_name 和无 route 的累计 RunUsage（H-226）；有价完整 usage 仍按错误模型价格快照入账 | Claude fallback 更新 currentModel 并重进 callModel，usage 按本次 resolvedModel 计价；DeepSeek Harness 的 attempt 保留 message.source route，但公开聚合 routes 只是去重集合，不冒充按模型人民币账本 | 便宜→贵低估后误放行，贵→便宜高估后误拒绝；混合模型不能用单一初始/最终模型价还原，费用事件和历史 snapshot 同样错误 |
| 有效配置与信任 | 请求覆盖、恢复、项目来源和实际 executor 条件分裂（H-03/H-04/H-27/H-28/H-54） | Codex 对恢复有效配置做校核、禁用未信任项目配置层；Claude hooks 信任检查 | 配置静默变化、用户限制未落实到实际执行 |
| 事件与恢复 | 慢消费者丢通知、订阅未就绪、恢复缺规范终态、恢复原因未按 run/task 隔离，以及最终钩子失败前已通知完成、审批由两条路径重复通知（H-01/H-43/H-44/H-83/H-84/H-95/H-214） | Codex 明确背压/断连、持续消费、按 turn 身份恢复 stale 状态及统一携带错误的 turn 完成通知；DeepSeek Harness 高层 run 在发 prompt 前建立带队列的订阅；审批从单个权限请求发出带关联身份的 server request | UI/SDK 长期等待、提前结束、恢复诊断归错 run、重复提示或审批请求无法关联 |
| 工具副作用与结果 | 不确定结果重发、写屏障被跨越、结果资产过早回收（H-06/H-35/H-36） | Codex 限定通用可重试方法与串行执行边界；Claude 按会话保留完整结果 | 重复外部操作、读取到错误时序数据、后续请求失去证据 |
| 文件操作的实际资源边界 | “读过”未绑定所读版本，搜索遍历未继承直接读取的敏感路径拒绝（H-76/H-77） | Claude 在写入执行时复查读取版本，将 Read deny 规则同时投影为 grep/glob 排除项 | 外部已保存修改被覆盖；被禁止的文件内容仍可经搜索进入模型请求 |
| 物理路径与沙箱边界 | WorkspaceRoot/PathGuard 只按词法路径判断，Windows junction 后的真实读取目标未重新校验（H-78） | Claude 权限检查同时检查原路径和 realpath/symlink 链；Codex 依赖其 sandbox/backend 边界而非模型工具的词法字符串 | WorkspaceWrite 可读取项目外内容，配置的工作区隔离失效；FullAccess/普通外部读取语义仍需单独区分 |
| 运行及附属工作的归属 | 子运行、输出 reader、复核和真实构建验证没有一致的取消/退出 owner（H-45/H-60/H-61/H-64）；原生摘要取消已修复 | Codex turn token、有限 grace、任务 abort 和专属清理，输出 reader 截止后显式处理句柄 | 停止等待、父终态后附属工作仍存活、取消后错误落成功终态 |
| 内容身份与生命周期 | 去重名称没有对应正文、共享可变项目 scope、旧计时器作用到新调用（H-51/H-53/H-57/H-59） | Claude transcript 内容驱动去重及局部 timer cleanup；DeepSeek connection-owned system section；Wasmtime 的 Store deadline 语义 | 能力或记忆漏注入、跨项目污染、调用被无关计时器提前中断 |
| 文件回退的失败边界 | 备份不可用尚未检查，当前文件已经删除；多文件已部分恢复而历史未变（H-72） | Claude fileHistory 先 stat 备份，已知缺失时不复制或删除当前文件；该机制不等于整个回退事务原子化 | 回退/分叉失败仍丢失当前文件，磁盘与会话处于不同恢复阶段 |
| 回退后的派生数据归属 | 重用 sequence 后，丢弃分支的笔记覆盖和文件检查点仍可能作用于新分支（H-23/H-73） | Claude 按目标 messageId 定位文件快照；Codex 对有效历史应用 rollback 标记后计算分支位置 | 新分支历史与文件/摘要不一致，已经丢弃的决策或文件内容重新出现 |

表中采用的是已查证的机制级比较，不表示参考项目整体没有其他缺陷；本系统的特定宿主、移动/工作流/WASM 集成，也不能直接以 Codex 同名能力代替验收。后文按问题给出触发、证据、影响及收敛方向，未覆盖项继续保持待审计。

## 实测基线：Windows 桌面与真实 DeepSeek

### 运行方法

1. `cargo build --manifest-path apps/desktop/src-tauri/Cargo.toml --offline`：通过。
2. `apps/desktop` 中运行 `pnpm dev`；启动上述构建的 `deepagent-desktop.exe`，使用独立 `DEEPAGENT_DATA_ROOT=target/live-desktop-test-20261021`。
3. 真实 WebView2 中完成连接引导；发现并选择 `deepseek-flash`。在该桌面窗口输入、发送和停止；通过本机 CDP 观察 IPC，并读取测试数据库 `runs` / `run_events`。
4. 独立运行 `cargo test -p deepagent-app-core --features keychain,web --test real_session_file_responses real_responses_session_replays_tools_skills_files_and_logs -- --ignored --nocapture`，密钥通过进程环境注入。

普通浏览器中的 `api.ts` 会回退模拟数据（参见 `getInvoke()` / `mockChatStream()`），因此浏览器预览成功不能证明真实后端通过。此次测试连接的是实际 Tauri WebView。

### 已确认结果

| 场景 | 结果及证据范围 |
|---|---|
| 新会话简短回复 | 正确回复“测试成功”；provider 输入约 16.8k、输出 4；数据库从接纳到模型请求 804ms、接纳到终态 2170ms |
| 同会话继续 | 正确复述上一轮内容；接纳到模型请求 497ms、到终态 1637ms；证明此输入的历史继续链路可用 |
| 其余已发起模型请求的桌面运行 | 6 次接纳到模型请求分别为 804 / 497 / 568 / 625 / 507 / 615ms；样本不足以声称达到 P95 性能目标 |
| 输出中停止 | 一轮发送后约 2 秒点击停止；终态 `cancelled`，事件有 `cancel.observed`、`run_cancelled`、`cancel.requested`、`cancel.propagated` 和单个 `run_terminal` |
| 接纳后立即停止 | 记录到 `start_chat_v2` 响应后点击；`cancel_run` 请求至 HTTP 响应约 4ms；数据库运行总时长 210ms，终态 `cancelled`，`model_request_started` 数为 0 |
| 模拟挂起模型的取消回归 | `cargo test -p deepagent-app-core --test golden_trace cancel_request_acknowledged_within_200ms -- --nocapture`：1 passed |
| 真实只读工具调用 | 工作区内放入未在提示中透露内容的随机标记文件；模型调用 `read_file` 后回复与文件一致。文件放在工作区外时被权限钩子拦截；未修改权限绕过 |
| 桌面进程重启与继续对话 | 正常关闭并重启同一隔离数据目录的桌面进程；展开折叠的“最近对话”后打开原会话，历史与标记仍可见；发起新一轮要求复述标记且不调用工具，模型正确复述 |
| 真实三轮 AppCore 用例 | `real_session_file_responses` 真实 provider 用例通过；三轮主 Agent 请求分别为 12469 / 13059 / 13926 输入 tokens，cache hit 为 12288 / 2944 / 13184；另有 3 次笔记维护请求。笔记更新，持久化 `last_summarized_sequence` 有效且 `tokens_at_last_extraction > 0` |
| MCP 初始化未完成时的普通对话 | 新增 `pending_host_mcp_initialization_does_not_block_conversations`：真实 Node MCP 子进程将 initialize 持有至 180 秒或显式释放；确认子进程已收到 initialize 后运行两轮真实 ChatService、mock 模型请求。两轮合计 384ms、initialize 仅 1 次、待就绪工具未发布且未进入任何模型请求；释放后工具正常发布。1 passed |
| 运行包准备等待真实磁盘锁时的普通对话 | `blocked_host_payload_preparation_does_not_block_conversations`：宿主 worker 确认进入生产磁盘准备路径并被真实 OS 文件锁阻塞；两轮 ChatService + mock 模型合计 349ms，运行包尚未解压、技能未发布；释放后成功解压，第三个新会话实际请求含就绪技能目录。1 passed |

### 已澄清项

- **笔记请求次数断言失败属于测试假设错误，不是生产故障证据。** 笔记代理一次提取可执行可变数量的模型/工具循环，生产契约没有要求请求数是 3 的倍数。移除 `note_requests % 3 == 0` 后，使用授权测试 Key 的真实三轮 AppCore 用例通过：6 次主 Agent 请求、3 次笔记维护请求；笔记已更新，持久化 `last_summarized_sequence` 有效且 `tokens_at_last_extraction > 0`。具体命令及实际 provider usage 见下方“真实三轮 AppCore 用例”和缓存核查记录。此结果验证本次实际提取及状态落账，不评价笔记内容质量或所有运行配置下的请求次数。

### 尚未定性的现象

1. **第一次与第二次上下文总量下降**：桌面两轮投影分别约 16.8k → 15.5k。分项中动态环境约 3723 → 1969 tokens，而对话分项 26 → 41 tokens；不能据总量下降认定历史丢失。需要继续验证动态环境变化来源和 wire history。
2. **立即点击停止的自动化不一致**：两轮 agent-browser 批处理显示点击成功但最终运行成功；这些轮次数据库无取消审计。另一次通过 Playwright 记录真实 IPC 的立即停止成功。当前仅证明自动化点击记录不足以证明取消请求送达，尚不能认定系统存在同样的取消缺陷。
3. **检查点捕获失败后的同对象重试契约**：`capture_before()` 在真正复制备份前就向 captured 集合插入路径；复制失败后标记未撤销。`checkpoint_audit.rs::audit_capture_failure_does_not_skip_backup_on_write_retry` 通过正式 ToolExecutionPipeline 和共享 read-before-write 缓存的真实文件工具复现：以测试 storage 的空 `files/0` 目录注入首次复制失败，文件保持 before-retry；只移除该空目录后显式再次调用同一 pipeline，write_file 成功，但最终 manifest entries=0、restore 返回 0 条，文件仍为 after-retry。对应正常备份/真实写入对照恢复 before-retry。执行 `cargo test -p deepagent-app-core --test checkpoint_audit capture --offline -- --include-ignored --nocapture`：1 passed / 1 failed，约 0.06s。这是底层同对象重试的不变量缺口，**尚不据此新增常规聊天缺陷编号**：当前 `loop_engine.rs::execute_tool_with_before_and_speculative()` 对 execute_prepared 的错误使用 `.await?` 传播，并非自动再调用同一 checkpoint；探针的第二次调用是测试显式发起，不能写成模型在真实普通 turn 中已经越过备份继续写入。后续需要证明生产恢复/复用入口的可达性，或明确该公共 API 禁止失败后复用的契约。Claude `fileHistoryTrackEdit()` 先完成 createBackup，成功后才提交 trackedFileBackups；其备份失败可能按可选能力处理，不据此声称其所有写入都必须因备份失败终止。本轮保留默认 ignored 探针及正常对照，生产行为未改。

## 已确认问题

### H-01：stdio 事件队列满时丢弃关键通知，连接仍表现正常

**级别：高（P1）；范围：stdio Harness 连接及其 SDK 消费者。** 尚无证据表明 Tauri 事件通道使用同一个队列。

#### 触发与复现

- 消费者暂时停止读取、输出管道阻塞或事件突发，令连接积压达到 256 条。
- `apps/cli/src/app_server.rs` 的 `EventOutbox::emit()` 在 `try_send()` 之前分配序号；队列满只向 stderr 输出错误，不关闭连接、不反馈生产者、不保留该通知等待重发。
- 默认忽略的审查探针 `audit_stalled_outbox_preserves_terminal_notification` 调用生产 `emit()`，将消费者固定在不读取状态，连续产生 256 条事件后发出终态，再恢复读取。
- 执行：`cargo test -p deepagent-cli audit_stalled_outbox_preserves_terminal_notification --offline -- --ignored --nocapture`。
- 实际输出：`dropped sequence 257`、`queued=256, terminal_received=false, next_sequence=258`；“必须收到终态”断言失败。该探针是确定性队列复现，并非声称此次真实 DeepSeek 桌面对话发生过管道阻塞。
- 随后新增真实 OS 管道子进程探针 `app_server::terminal_audit::audit_stdio_slow_output_drops_terminal_on_real_pipe`：子进程用生产 `EventOutbox` writer 排入 1,023 条约 4 KiB 的 `turn.completed` 通知及最后一条 `critical-terminal`；父进程确认队列生成后继续不读 stdout 250 ms，再开始排空真实 Windows 子进程管道。执行 `cargo test -p deepagent-cli --offline audit_stdio_slow_output_drops_terminal_on_real_pipe -- --ignored --nocapture`，结果 `emitted=1024, observed_notifications=272, sequenced_notifications=272, critical_terminal_seen=false, stdout_bytes=1157606`；审计断言通过，表示缺陷已复现而非已修复。
- 此端到端探针覆盖生产 outbox 与 Windows OS stdout 管道，不含正式 ChatService、RunStore 或 SDK 客户端；因此它确认慢读端可以令 outbox 丢掉终态类型事件，但没有验证同一次真实 run 的数据库终态与客户端状态出现不一致，也没有声称桌面 Tauri 使用该队列。

#### 恢复链核对

- `event_ack()` 只保存进程内 `last_event_ack`，检查单调性；没有重发、缺口恢复或映射逻辑。
- Rust SDK `client.rs::spawn_reader()` 只把通知的 `params` 转为 `HarnessEvent`，丢掉 envelope 上的 `event_sequence`。订阅端不能据此直接检测服务器丢通知；SDK 自己的 broadcast 慢消费者可能收到 `Lagged`，那是另一层行为，不能混同。
- `thread/read` **可以**按 session/run 的持久化游标恢复事件及运行状态。因此不能称持久化历史已经丢失；但它不是 connection `event_sequence` 的重放接口，SDK 未自动触发或转换这条恢复链。
- 后续直接调用生产 `ServerState::dispatch()` 做边界验证：在本连接尚未发出任何通知时提交 `event/ack(eventSequence=u64::MAX)`，服务端仍返回 `status=accepted` 并回显该值。执行 `cargo test -p deepagent-cli app_server::tests::audit_event_ack_rejects_unemitted_sequence --offline -- --ignored --exact --nocapture` 得到 **0 passed / 1 failed（预期复现）**，失败响应为 `acknowledgedEventSequence=18446744073709551615`。这不是新的 H 编号，而是 H-01 已记录的 ACK 无绑定缺口的更强证据：ACK 没有校验已分配/已交付范围，也没有把确认游标接入 `thread/read` 或 outbox；探针未启动模型、工具或公网服务。

#### 成熟实现对照

仓库内 Codex 的 `借鉴/codex/codex-rs/app-server/src/transport.rs::send_message_to_connection()`：可断开的慢连接在队列满时调用 `disconnect_connection()`；不能断开的 writer 使用 `send(...).await` 施加背压。`transport_tests.rs::broadcast_does_not_block_on_slow_connection` 明确验证慢连接被断开、快连接继续收到通知。

`outgoing_message.rs::send_server_notification_to_connections()` 使用异步 `send()`。同文件也有允许丢失的 `try_send_server_notification()`，所以不能概括成“Codex 所有通知都绝不丢失”；可借鉴的是明确区分传递保证，并把慢连接故障显式化。

#### 后果和修复方向

- 终态丢失：只等通知的调用方可能一直认为运行中，即使数据库已终结。
- 审批通知丢失：客户端可能不知道需要批准，执行侧等待直到其他超时/取消机制介入。
- 增量丢失：客户端内容和真实结果不一致；没有自动恢复时必须主动读取历史。
- 修复应复用既有 `run_events`：定义可丢弃增量与必达控制事件；选用有界背压或显式断连/恢复提示；让 SDK 保留传输序号、区分传输和持久化游标，并测试恢复到终态。不能仅扩大通道容量。

### H-02：Harness ACK 与运行接纳分离，立即中断可能被虚假确认

**级别：高（P1）；范围：stdio `turn/start` / `turn/interrupt`；桌面 `start_chat_v2` 已采用更早的接纳，不能据此认定桌面同样失败。**

#### 触发与复现

- `ServerState::turn_start()` 只创建内存 `TurnState { active: true }`，返回 `started` 和待启动 `TurnLaunch`。
- `run_stdio()` 先写 start 响应，再 `spawn_turn()`；直到后台调用 AppCore 才进入运行接纳。因此客户端拿到 turn ID 时，运行取消注册可能尚不存在。
- `turn_interrupt()` 调用 `request_cancel()` 后只检查 `Result`，**忽略 `CancelRequest.accepted`**，无条件返回 `cancelling`。
- 探针 `audit_interrupt_after_start_ack_reaches_cancellation` 使用生产分发器，保留返回的 launch 以固定上述调度窗口，随后分发 interrupt 并核对生产取消注册和 RunStore。
- 执行：`cargo test -p deepagent-cli audit_interrupt_after_start_ack_reaches_cancellation --offline -- --ignored --nocapture`。
- 实际输出：`status="cancelling", active_registration=false, durable_runs=0`；取消已送达注册对象的断言失败。

这证明合法的 ACK 后/执行前交错中，控制请求可被确认但未送达取消对象。探针没有调用真实模型，也不证明每次真实快速点击都必现；窗口频率仍需端到端调度压力测试。现有 `turn_start_returns_turn_id_and_interrupt_routes_to_chat_cancellation` 只断言响应文字为 `cancelling`，未检验信号送达，因而无法覆盖该缺陷。

#### 成熟实现对照

Codex `借鉴/codex/codex-rs/app-server/src/request_processors/turn_processor.rs::turn_interrupt_inner()`：验证活动 turn，将请求登记为 pending interrupt，向核心提交 `Op::Interrupt`；普通 turn 的响应等待 `TurnAborted`，提交失败清理 pending 并返回错误。`turn_start_inner()` 将输入交给核心后才构造返回的 turn。

本系统不必照搬其 ACK 时机：桌面已经有 `ChatService::admit_run()`，在返回接纳响应前创建 durable run 和取消树。这条既有边界可供 Harness 复用，避免再建一套取消机制。

#### 后果和修复方向

- 用户或 SDK 相信停止已受理，但后台稍后仍可能发起模型请求、执行工具及产生费用/副作用。
- 只改响应文案或增加前端延时不能消除窗口。
- start 返回前完成统一接纳；执行端消费同一 admission；interrupt 必须依据 `accepted` 返回实际受理状态或保留可交付的取消意图，并覆盖立即取消、排队取消和完成竞争。
- 测试需检验“取消后没有模型/工具启动”与唯一终态，不能只验证 JSON 字段。

以上探针以默认 `#[ignore]` 保存，因为本轮仅审查，未修改生产行为；显式执行时预期为红色。修复对应业务问题时应改为默认运行的回归测试。

### H-03：Harness 的逐次沙箱 backend 选择未进入实际执行路由

**级别：高（P1）；范围：Harness `thread/start` / `turn/start` 的 `sandboxBackend` 覆盖。** 不等于所有桌面沙箱均无效，也不证明文件权限钩子失效。

#### 触发与复现

- `apps/cli/src/app_server.rs::turn_start()` 把请求或线程上的 `sandbox_backend` 放进 `HarnessRunOverrides`。
- `run_assembler.rs` 将 overrides 序列化给 `RunEnvironment::resolve()`；其 `RunConfigOverlay::apply_permission_profile()` 消费权限 profile 和 sandbox mode，却没有消费 backend 选择。
- CLI 的 `build_chat_service()` 在构造宿主时固定 `SandboxBackendCommandExecutor`。stdio 默认使用 `Direct`；`for_workspace()` 克隆已有 executor，没有依据逐次覆盖重新选择 backend。
- 审查探针在 `kernel_v2_e2e.rs::audit_sandbox_override_does_not_route_to_default_direct_backend` 中建立真实 ChatService 和 mock 模型工具循环，并注入记录型 Direct backend：**只记录调用并返回固定结果，不执行任何系统进程**。
- 设置 `WorkspaceWrite`、无交互审批策略以进入现有 executor 路由；逐次请求指定 `sandbox_backend="windows_sandbox"`。正确行为应选择指定 backend，或者在不可用时明确拒绝，不能继续交给 Direct。
- 执行：`cargo test -p deepagent-app-core --test kernel_v2_e2e audit_sandbox_override --offline -- --ignored --nocapture`。
- 实际输出：`run_ok=true, direct_backend_calls=1`；“Direct 调用次数为零”断言失败。此为确定性 AppCore 路由复现，没有启动 Windows Sandbox，也没有进行真实主机逃逸操作。

#### 成熟实现对照

Codex 的 `app-server/src/request_processors/thread_processor.rs` 将请求 sandbox 转换进 `ConfigOverrides.sandbox_mode`，并从实际 config snapshot 提取 `sandbox_policy()` 返回；文件内 `collect_resume_override_mismatches()` 比较恢复请求与实际生效的沙箱配置。`core/src/exec.rs` 的执行路径继续携带 permission profile 和 sandbox 信息。可借鉴的是请求、有效策略和执行边界的一致性；不能把 Codex 的沙箱实现直接视作本系统的 Microsoft `.wsb` 后端。

#### 后果和修复方向

- 调用方明确要求隔离 backend，实际仍可能走宿主默认 Direct；属于安全选择与执行不一致。独立的审批、文件边界和命令规则仍然存在，不能据此推导它们全被绕过。
- 必须在现有 `SandboxBackend` 边界解析并验证逐次选择；不可用时显式失败，不能静默降级。
- 将生效 backend、工作目录、mode 和 network policy 形成逐次不可变执行上下文；保证工具注册和执行使用同一份结果。不要依赖修改全局共享 backend 来切换并发会话。
- 补充不同会话并行、继续会话、显式 backend 不可用及实际返回配置一致性测试。此探针同样默认忽略，待修复后转常规回归。

### H-04：Harness 恢复与 steer 替换运行会清空显式模型和权限配置

**级别：高（P1）；范围：stdio `thread/resume` 后未重新指定覆盖值的运行。** 与 H-03 区分：本项影响权限 profile、模型和 provider 的生命周期，甚至同一宿主内重新附着也会触发。

#### 触发与复现

- 通过 `thread/start` 显式选择 `permissionProfile=read_only`、provider、model 和 sandbox backend；这些值仅存于进程内 `ThreadState`。
- `thread_resume()` 在确认会话存在后，无条件重新插入 `ThreadState`，将上述四项设为 `None`，包括线程已经加载在同一进程内的情况。
- `turn_start()` 只从本次参数或此 ThreadState 取值。恢复请求类型只有 `threadId`，也没有机会在恢复时重申或查看生效配置；启动响应只返回 `ready/cwd`。
- 探针 `audit_resume_preserves_explicit_thread_overrides` 使用生产请求分发器启动、恢复、再构造下一次 turn，检查交给 AppCore 的 overrides；不调用模型、不执行工具。
- 执行：`cargo test -p deepagent-cli audit_resume_preserves --offline -- --ignored --nocapture`。
- 实际输出：`before={"model":"deepseek-flash","permission_profile":"read_only","provider":"deepseek","sandbox_backend":"windows_sandbox"}, after={}`；配置保留断言失败。

#### 后续复现：不经 resume 的 steer 同样丢失配置

- `turn_steer()` 会取消旧运行、登记新 turn，并直接构造 `HarnessRunOverrides::default()`，既没有承接线程配置，也没有承接旧 turn 的有效覆盖值。
- `audit_steer_preserves_effective_turn_overrides` 通过生产 thread/start 与 turn/start 建立配置；显式调用既有 `admit_run()` 使取消注册和 durable run 确实存在，排除 H-02 的接纳窗口后再调用 steer。
- 执行：`cargo test -p deepagent-cli audit_steer_preserves --offline -- --ignored --nocapture`。实际 before 包含 provider/model/read_only/windows_sandbox 及 reasoning_effort=high，after 为 `{}`；保留断言失败。没有执行真实模型或工具。
- Codex `turn_processor.rs::turn_steer_inner()` 把新增输入交给现有线程的 `steer_turn()` 并验证 expected_turn_id；它不会通过创建默认配置的新运行实现此操作。本系统可以保留自己的“取消后替换 turn”语义，但必须让替换运行继承已解析的有效配置。
- 因此本项修复应覆盖 resume、steer、fork 等生命周期边界；只保存线程元数据仍不足以保留上一 turn 的逐次覆盖。

#### 成熟实现对照

Codex `app-server/src/request_processors/thread_processor.rs::collect_resume_override_mismatches()` 对已加载线程的恢复请求比较 model、provider、cwd、approval 与 sandbox 的请求值及实际 config snapshot；恢复响应包含实际配置。其 `ThreadResumeParams` 提供显式覆盖入口。本系统目前既无完整恢复配置、也无生效值回传。可借鉴的是恢复配置必须有可验证的语义，不要求无条件冻结所有历史设置。

#### 后果和修复方向

- 恢复后若宿主默认权限比 `read_only` 更宽，后续运行将回退到更宽配置；具体工具副作用还受宿主默认值与其他审批/钩子约束，此探针没有执行真实写操作。
- 模型或 provider 可变为宿主默认选择，影响能力、费用及数据发送目标；不能承诺同一 thread ID 仍代表相同执行条件。
- 在现有会话元数据中明确持久化可恢复的配置，或要求显式重新选择并返回有效配置；已加载线程不能在普通 resume 中悄悄丢弃配置。若希望始终采用最新全局设置，应在协议和响应中明确体现，并禁止静默放宽安全约束。
- 覆盖同进程重新附着、宿主重启、默认设置改变与更严格权限恢复；探针默认忽略，修复时转常规回归。

### H-05：Chat Completions 往返投影拆散同一次 assistant 工具响应

**级别：中（P2；启用严格 reasoning 校验的供应商可能升级为请求失败）；范围：`WireMode::ChatCompletions` 下经原生 ResponseItem 再发出的工具续轮。** 默认 Responses 路由的真实桌面测试不能证明本路径无误。

#### 触发与复现

- 模型同一条 assistant 响应同时返回 reasoning、说明文本和 tool call。这是正常工具协议形态。
- `ChatCompletionAccumulator::finish()` 将其转成 `Reasoning → FunctionCall → Message`；运行时再追加工具结果。
- 下次 `ResponseRequest::from_response_items()` 的配对修复把工具结果搬到 call 后，`client.rs::response_to_chat_completion()` 经兼容 `Message` 投影重新序列化，最终产生：

```text
assistant(reasoning + tool_calls, content="")
tool(result)
assistant(content="I will read the file.", 无 reasoning)
```

- 原本在执行前给出的说明变成执行后的独立 assistant 消息，且其 reasoning 不再跟随。工具 call_id 和 arguments 在本例仍完整，不能称它们也丢失。
- 探针 `client::tests::audit_chat_projection_preserves_tool_turn_narration_and_reasoning` 使用生产流式 accumulator、生产 request 构造器、生产 Chat 投影和 serializer；输入为合成但合法的单个工具响应，不访问供应商。
- 执行：`cargo test -p deepagent-models audit_chat_projection --offline -- --ignored --nocapture`。
- 实际 wire 显示上述四条消息（含最初 user）；assistant 数量为 2，期望保持同一响应的断言失败。

#### 官方规则与成熟实现对照

[DeepSeek Thinking Mode 官方文档](https://api-docs.deepseek.com/guides/thinking_mode/)（2026-10-21 核对）要求携带 tools 的后续请求完整回传历史 reasoning，包括没有调用工具的 assistant 回答；示例将同一 response 的 content、reasoning 和 tool_calls 保留在同一个 assistant message 中。这里已证明 wire 关联被改变，**尚未用此特定请求实测 HTTP 400，不能把潜在拒绝当作已发生的网络结果**。

DeepSeek Harness `packages/llm/llm-deepseek/src/serialize.ts::assistant()` 将同一 durable assistant message 中的 text、reasoning、tool-call 块统一映射为内容；`replay.ts` 以块索引校验模型和原生 metadata 的对应关系。它当前走 Messages 协议；借鉴点是保持 assistant turn 的边界与块归属，不是复制字段名。

#### 后果和修复方向

- 续轮上下文含额外、时间顺序错误的 assistant 发言，破坏模型对工具执行前后语义的理解；严格 reasoning 规则下存在请求被拒绝风险。
- native Surface 与 Chat adapter 之间不能把有损 UI 投影当成协议回放。应由 provider seam 保留或恢复同一响应的块归属，在一次 serializer 内生成正确的 assistant 原子消息。
- 补充含文本的单/多工具响应、reasoning 多块、重启回放、压缩边界和取消中断测试，并核对真实 wire；不能通过填造 reasoning 来满足校验。

### H-06：MCP 断线恢复自动重发结果未知的工具调用，可能重复产生副作用

**级别：高（P1）；范围：使用 `ReconnectingTransport` 的 MCP 调用。** 不是一般的“连接重建有问题”，而是连接恢复与业务操作重试没有分开。

#### 触发与复现

- `reconnect.rs::ReconnectingTransport::send()` 遇到字符串匹配的 `broken pipe`、`connection lost` 等错误后，重新连接并对新 transport 调用 `send(request)`；既不区分 `tools/list` 与 `tools/call`，也不知道旧调用是否已在服务端执行。
- `mcp_service.rs::connect_one_resilient()` 将生产连接包装为此 transport；`McpClient::call_tool()` 没有额外幂等键或结果去重。JSON-RPC request ID 是响应关联标识，不能保证重启后的外部服务不重复执行。
- 探针 `reconnect::tests::audit_disconnect_after_tool_effect_does_not_replay_mutation` 通过真实 McpClient 调用合成 `append_record` 工具。第一个 mock transport 先增加副作用计数，再返回 `broken pipe` 模拟响应丢失；重连后的 transport 增加同一计数并成功返回。没有执行真实外部写操作。
- 执行：`cargo test -p deepagent-mcp audit_disconnect_after_tool_effect --offline -- --ignored --nocapture`。
- 实际输出：`result_ok=true, applied_effects=2`；期望只执行一次的断言失败。客户端看到成功，但逻辑操作已经执行两次。现有重连测试只模拟执行前断线，未覆盖执行成功但回包丢失。

#### 成熟实现对照

Grok 的 `xai-grok-shell/src/session/mcp_restart.rs::auto_restart_stdio()` 使用 1/4/16 秒有界退避重建并发布连接；该重启任务不持有原工具请求，也不负责重发它。本系统文件注释虽称对齐此机制，却增加了语义不同的业务请求重试。

Codex `rmcp-client/src/rmcp_client.rs::run_service_operation_with_transient_retries()` 明确只对 `tools/list` 的可重试传输错误做通用重试，`is_retryable_tools_list_error()` 对其他方法返回 false。它也支持显式 HTTP session-expired 404 的恢复，不能概括成“Codex 从不重试任何工具”；关键差别是没有把任意断线都解释成可以重发副作用。`tool_input.rs::call_tool()` 对已提交 proof 的续轮失败还明确禁止重启原操作。

#### 后果和修复方向

- 非幂等工具可能重复新增记录、发送消息或创建外部任务；这些是影响示例，不表示此次测试操作了这些真实服务。重试全部耗尽时，还可能出现客户端报错、服务端已执行多次的状态。
- 将连接恢复与操作重试分离：为后续调用恢复连接；已发出且结果未知的调用返回结构化 unknown-outcome，让运行知道需要核实结果。
- 只有明确未发送、操作被证明幂等或服务端支持持久化幂等键时才允许自动重发。不能仅依据错误字符串或只读名称推断安全。
- 补充执行前断线、执行后回包丢失、并发重连、取消与多次断线测试，并验证调用次数和最终外部状态。此审查探针默认忽略，生产逻辑尚未修复。

### H-07：MCP 工具发现忽略分页，只发布首页工具且不报不完整

**级别：中（P2）；范围：返回 `nextCursor` 的 MCP 服务。** 不分页的服务不受此项影响。

#### 触发与复现

- `McpClient::list_tools()` 只发一次不带 cursor 的 `tools/list`。`protocol.rs::ToolsListResult` 只有 tools 字段，反序列化直接丢弃 `nextCursor`。
- `McpRegistry::register_with_instructions()` 及 AppCore 工具发现继续使用该结果，成功注册并发布首页集合，没有第二条生产路径补拉后续页。
- 探针 `client::tests::audit_tool_discovery_reads_all_pages` 使用两页合规 mock server：第一页工具 first，并返回不透明 cursor；携带同一 cursor 才返回 second。通过真实 McpClient 发现工具。
- 执行：`cargo test -p deepagent-mcp audit_tool_discovery_reads_all_pages --offline -- --ignored --nocapture`。
- 实际输出：`names=["first"], requests=[None]`；期望 `["first","second"]` 的断言失败。不是目录展示截断，而是后续工具根本没有被发现和注册。

#### 官方规则与成熟实现对照

[MCP 2024-11-05 分页规范](https://modelcontextprotocol.io/specification/2024-11-05/server/utilities/pagination)明确将 tools/list 列为支持分页的操作，以 nextCursor 继续、缺省表示结束；cursor 必须作为不透明值传回。

Codex `codex-mcp/src/rmcp_client.rs::list_tools_for_client_uncached()` 用 `pagination.rs::collect_paginated_with_limit()` 汇总全部页，同时限制页数、条目数、cursor 大小和整体耗时，并拒绝重复 cursor。可以借鉴其“完整发现且有界”的边界，不能仅增加一次无界循环。

#### 后果和修复方向

- 模型会误认为插件没有后续页的能力，任务无法完成或选择替代工具；设置界面还可能显示连接正常，增加定位难度。
- 在现有 McpClient 统一实现有界分页，供 registry、状态查询和就绪快照共用；分页失败不能把半个工具集当作完整成功发布。
- 补充无分页、多页、空页但有 cursor、重复 cursor、超限和取消测试。此探针默认忽略，生产发现逻辑尚未修复。

### H-08：MCP 初始化缺少 initialized 通知，严格服务端无法进入就绪状态

**级别：中（P2；对要求完整握手的服务为连接阻断）；范围：当前 McpClient、stdio 及使用同一握手的重连路径。** 宽松服务端可能仍工作，不能概括成所有 MCP 都无法连接。

#### 触发与复现

- `McpClient::initialize()` 仅发送 initialize 请求并直接返回其 result；没有发送 notifications/initialized。
- `McpTransport` 只提供带 u64 ID 的 request/response `send()` 与 close，没有无 ID 通知接口；stdio 和 HTTP 实现都没有在初始化响应后补发通知。AppCore 的初连及 ConfigReconnectFactory 也未补足此步。
- 真实子进程探针 `tests/protocol_audit.rs::audit_strict_stdio_server_completes_initialization` 启动本机 Node JSON-RPC fixture。它接受本系统声明的协议版本，initialize 成功，但在 initialized 通知前拒绝 tools/list。探针使用生产 StdioTransport 与 McpClient，并在断言前关闭子进程。
- 执行：`cargo test -p deepagent-mcp --test protocol_audit audit_strict_stdio --offline -- --ignored --nocapture`。
- 实际服务端记录：`received=initialize,tools/list`，返回 `not initialized`；发现成功断言失败。该失败来自有意严格的本机 fixture，未声称所有用户插件已返回此错误。

#### 官方规则与成熟实现对照

[MCP 2024-11-05 生命周期规范](https://modelcontextprotocol.io/specification/2024-11-05/basic/lifecycle)要求 initialize 成功后客户端发送 initialized 通知，表示可以开始正常操作。本系统声明的正是这一协议版本，因此不是新版本功能缺失。

Codex `core/tests/suite/codex_apps_protocol.rs` 将 legacy 握手次序断言为 `initialize → notifications/initialized → tools/list`；`rmcp-client` 通过服务生命周期驱动握手，不用裸 request 代替完整初始化。其现代协议有独立协商分支，不能据此跳过本系统所选 legacy 协议的必需步骤。

#### 后果和修复方向

- 严格服务端无法发布工具，插件可能被标记连接/发现失败；仅重试会重复不完整握手，增加等待而不能恢复。
- 在现有 transport/client 边界补齐 JSON-RPC notification 和初始化状态机，只有协议协商及通知完成后才能发布 Ready。初连、显式重试和断线恢复共用这一实现。
- 补充 wire 次序、通知无 ID、不等待通知响应、初始化失败不发通知、协商版本不支持及重连握手测试。此探针默认忽略，生产握手尚未修改。

### H-09：stdio MCP 并发回包按读取者筛选，其他请求的响应会被永久丢弃

**级别：高（P1）；范围：同一个 StdioTransport 上存在两个未完成请求且服务端乱序响应。** 单请求顺序测试无法发现此问题。

#### 触发与复现

- `StdioTransport::send()` 分别锁 stdin 和 stdout：请求 A 写入后持有 stdout 等待；请求 B 仍能写入，但要等待 stdout。A 的读取循环遇到 ID 不等于 A 的响应就直接跳过，没有按 ID 缓存或分派。
- 服务端先回复 B、再回复 A 时，A 丢弃 B 的有效响应后完成；B 获得 reader 后再也读不到自己的响应。无外层超时则持续等待；有超时则把已成功的调用报为超时。
- 现有 registry/liveness 共享 Arc<McpClient>，后台 `client.ping()` 能与工具请求重叠；共享服务的多会话调用也需要此 transport 正确分派。此缺陷不以“模型一定并行调用 MCP 工具”为前提。
- 真实 Node 子进程探针 `stdio::tests::audit_out_of_order_concurrent_replies_reach_both_callers` 使用生产 StdioTransport。先观察请求 A 已持有 stdout，再发送 B；fixture 收齐两请求后逆序写回两个结果。用锁状态固定交错，不以任意 sleep 猜时序。
- 执行：`cargo test -p deepagent-mcp audit_out_of_order --offline -- --ignored --nocapture`。
- 实际输出：A 的 id=10 响应正确，B 为 `Err(Elapsed(()))`，尽管服务端已写出 id=11。探针在断言前关闭子进程；没有访问真实插件或执行副作用。

#### 成熟实现对照

Codex `rmcp-client/src/rmcp_client.rs::call_tool()` 从 RunningService peer 提交请求，得到关联的 request handle 后单独 await_response；连接服务与每个调用的结果等待分离。Codex 的另一条 JSON-RPC 连接 `app-server-client/src/remote.rs` 明确用 pending_requests 将 response.id 分发给对应的 oneshot sender。后者不是 MCP transport，借鉴的是相同 JSON-RPC 关联机制，不能混称为 MCP 的实现文件。

#### 后果和修复方向

- 正常成功的工具看起来挂起/超时；活性探针也可能误判健康连接，进而触发不必要重连。与 H-06 叠加时存在未知结果被再次执行的风险；本探针只验证丢响应，没有实测这条组合故障。
- 在现有 transport 内收敛成单 reader 驱动，按 ID 投递各 pending request；通知走独立处理，取消移除对应 waiter，断线使全部未完成请求得到明确错误。所有连接用户共用该分派器。
- 只锁住整个 send 可以避免此示例丢包，但会让慢工具阻塞活性检查和其他调用，不能当作完整并发设计；不要简单延长 timeout。
- 覆盖乱序、交错通知、超时后迟到响应、单请求取消、断线与并发 close。此探针默认忽略，生产 transport 尚未修改。

### H-10：运行结束后预取压缩任务仍继续执行，结果及 usage 无处归集

**级别：中（P2）；范围：进入 prefire 提前压缩区间、主运行先结束而 pass-1 尚未返回的场景。** 短上下文未触发 prefire 时不受此项影响；不同于有明确独立生命周期的后台 sub-agent。

#### 触发与复现

- `ModelAgent::maybe_schedule_prefire()` 直接 tokio::spawn，保存在普通 JoinHandle 中；任务不接收运行取消树，也没有 Drop/终态收尾取消或转交宿主的路径。
- AppCore `run_assembler.rs` 为生产 agent 配置 prefire_start，并在运行收尾读取 `agent.side_usage()`。该方法只包含已归集的 side_usage 和 prefire_cache，不包含尚未完成的 handle。
- 主运行完成或取消后丢弃 agent，只丢弃 JoinHandle，并不会取消已 spawn 的任务。它后来产生的 PrefireNote 只作为孤立 task 的返回值，不会进入后续运行、持久化缓存或已结束的计费记录。
- 探针 `model_agent::tests::audit_completed_agent_does_not_leave_prefire_running` 使用生产 ModelAgent 调度、mock 主模型和可控的 compactor：将提前阈值设为 1、硬阈值设为 1,000,000，确保进入 lead band；主模型正常结束，确认 pass-1 已开始，再读取 side_usage 并释放 agent，最后允许 pass-1 完成。没有请求真实供应商。
- 执行：`cargo test -p deepagent-runtime audit_completed_agent_does_not_leave_prefire_running --offline -- --ignored --nocapture`。
- 实际输出：`main_done=true, continued_after_owner_end=true, side_usage_at_end=None`；要求释放 agent 后不继续执行推测请求的断言失败。该探针直接复现正常结束，不是一次真实桌面点击取消；取消路径同样未向此 task 传递信号的判断来自生产调用链。

#### 成熟实现对照

Grok `session/compaction_config.rs::CompactCancelGate` 让 compact/prefire 共享取消 token；`compaction.rs` 将 token 传给 `generate_session_compact()`，`acp_session_impl/cancel.rs` 在停止时调用 `compaction.cancel.request_cancel()`。模型切换/记忆控制失效时，`model_switch.rs::abort_and_clear_prefire()` 还会取出 handle、abort 并 await，再清理缓存。

Grok 可以让部分预取跨 turn 保留，因为它的 handle/cache 属于 SessionActor；本系统却放在逐运行的 ModelAgent 中。不能只复制 spawn 和两段压缩策略，遗漏任务所属生命周期及费用归集。

#### 后果和修复方向

- 主界面已经完成或停止后，推测摘要仍可能占用网络、模型配额和费用；本地账单无法归集晚到 usage。取消本地请求也不能保证供应商已产生的费用为零，修复不能作此承诺。
- 运行反复进入提前区间后立即结束，会重复启动无消费者的摘要；本次未做真实供应商批量压力或费用测量。
- 在既有取消树中给运行所属的摘要/预取派生子信号，终态收尾有界取消并回收；若要跨 turn 缓存，应由现有会话服务明确接管其身份、有效性和计费，不能丢弃 handle 后默许继续。
- 已获得的 usage 必须在有主人的收尾路径归集；覆盖正常完成、用户取消、模型切换、摘要已完成但未收取、任务失败及会话重启。此缺陷探针默认忽略，生产行为尚未修改。

### H-11：以消息内容反查压缩边界，重复回答会让恢复重新带回已摘要的历史

**级别：中（P2）；范围：原生压缩保留尾部的首条可定位 item 是重复 assistant 文本等非唯一内容。** 用户消息已有标签，不能概括为所有重复用户输入都会触发；本项与 H-05 的 Chat 投影拆分无关。

#### 触发与复现

- `context_runtime.rs::record_native_boundary()` 用 tail 中第一条可解析 item 查找事件序号；`input_runtime.rs::sequence_of_response_item()` 对完整事件日志从前向后执行内容相等匹配，没有保留该 live item 的事件身份。
- 相同的 assistant `Done` 可以出现在早期与近期，查找会选中早期事件。`ContextCompacted.first_kept_sequence` 因而指向已经被摘要的前缀；恢复按该序号重放，把前缀与新摘要同时放回 Surface。
- 新增探针 `context_runtime::tests::audit_native_summary_replay_preserves_repeated_assistant_boundary`：使用真实内存数据库、Session 原生事件、生产 keep-window 选择器、HookedReactiveContextCompactor 和恢复函数，只有摘要模型使用 mock。24 轮旧对话及近期尾部中包含两条相同的 assistant `Done`；通过断言确认生产选择器确实从近期的 `Done` 开始保留 5 条，而非人为指定一个无法出现的切点。
- 执行：`cargo test -p deepagent-app-core audit_native_summary_replay_preserves_repeated_assistant_boundary --offline -- --ignored --nocapture`。
- 实际输出：`expected=50, saved=Some(2), original_items=54, live_items=6, replay_items=54`；live 与 replay 相等的断言失败。压缩后应只剩摘要 + 5 条尾部，恢复却多出 48 条已被摘要的旧内容。日志原始事件未删除，问题是模型视图的恢复边界错误。
- 这是生产压缩及数据库回放的确定性复现；尚未做真实供应商大窗口压缩后重启桌面的端到端复测。既有 `native_summary_keeps_exact_tool_metadata_across_replay` 使用可区分的用户尾部，无法覆盖这个非唯一边界。

#### 成熟实现对照

Claude Code `services/compact/sessionMemoryCompact.ts` 通过 `lastSummarizedMessageId` 与 `msg.uuid` 找到已总结位置；已知 UUID 不存在则返回 null，转入其他压缩路径，不拿相同文本猜边界。随后统一选择尾部并保护工具配对。

Codex `core/src/session/mod.rs::replace_compacted_history()` 先为缺失身份的 item 分配 ID，再从同一份 items 建立 live history 与持久化 `CompactedItem.replacement_history`；`core/src/compact.rs::CompactedHistoryMetadata` 的注释明确要求二者一致。它持久化的是实际替换结果，不依赖重启时凭文本猜原事件。

#### 后果和修复方向

- 跨运行或重启后，模型看到“摘要 + 被摘要的原文 + 近期尾部”，token 和费用增加，可能很快再次进入压缩或 overflow；摘要已更新的状态还可能与恢复的旧叙述冲突。这些是错误 Surface 的推论，本次没有测实际收费或真实模型误判。
- 原生 Surface 应携带既有事件 sequence/稳定 item 身份或旁路 provenance，压缩尾部必须定位到确切事件；不能简单把 first match 改成 last match，因为尾部内部也可能重复。
- 沿用现有 `ContextCompacted` / `run_events` 持久化边界，必要时兼容扩展实际 replacement/provenance，不能新建第二套上下文存储。来源不可验证时明确失败或采用可验证的保守边界，不写一个猜测的成功切点。
- 补充相同 assistant 文本、相同 reasoning、连续多次压缩、snip/patch 后压缩及重启后的 live/replay/wire 等价测试。此探针默认忽略，生产边界尚未修复。

### H-12：Streamable HTTP 未保存初始化分配的 session header，有状态远程服务后续请求失败

**级别：中（P2）；范围：启用 http feature、使用初始化分配 Mcp-Session-Id 的 Streamable HTTP 服务。** 无状态 HTTP 服务及 stdio 不受这项 header 缺失影响。

#### 触发与复现

- `connect.rs::connect_network()` 的生产 HTTP/SSE 路由进入 `http.rs::HttpTransport`；该结构只保存静态 URL、配置 headers 和 reqwest Client。send 读取响应状态及 Content-Type 后消费正文，丢弃初始化返回的会话标识，下一请求只能发送静态配置头。
- 新增 `tests/protocol_audit.rs::audit_http_initialization_retains_session_header`：在 127.0.0.1 的随机端口启动真实 TCP/HTTP fixture，第一次 initialize 返回 JSON 及 `Mcp-Session-Id: audit-session`，第二次 tools/list 只有带回该头才成功，否则返回规范建议的 400。请求均经过真实 reqwest / HttpTransport，不是 mock send。
- 执行：`cargo test -p deepagent-mcp --features http --test protocol_audit audit_http_initialization_retains_session_header --offline -- --ignored --nocapture`。
- 实际输出：`received=[("initialize", false), ("tools/list", false)]`，`discovery=Err(...400 Bad Request: missing Mcp-Session-Id)`；后续请求携带初始化 session 的断言失败。
- 最终探针直接驱动生产 HttpTransport，initialize 的请求及响应版本均为 2025-03-26，排除 McpClient 仍固定声明 2024-11-05 的版本协商混杂因素。fixture 仅检查 session header，不检查 initialized 通知，以隔离 H-08；不是一次完整 MCP 协议符合性或外部服务验收。

#### 官方规则与成熟实现对照

[MCP 2025-03-26 Streamable HTTP 规范的 Session Management](https://modelcontextprotocol.io/specification/2025-03-26/basic/transports#session-management)允许服务器在初始化时分配 Mcp-Session-Id，随后客户端必须带回；缺失时服务可返回 400，过期会话返回 404 并要求重新初始化。此处依据 Streamable HTTP 规则，不能把这些字段说成 2024-11-05 legacy HTTP+SSE 的原有要求。

Codex `rmcp-client/src/http_client_adapter.rs` 从 HTTP response headers 读取 HEADER_SESSION_ID，并将 session_id 与 Json/Sse 响应交给 rmcp StreamableHttpClientTransport；后续 post_message 接收该 session_id，并通过同一适配器插入 mcp-session-id 请求头。遇到过期会话另有恢复边界，不是要求用户手填一次性标识。

#### 后果和修复方向

- 初始化 HTTP 本身成功，但后续工具发现或调用被拒，远程工具无法进入就绪快照；不能把错误仅归因于 API Key 或服务宕机。本次只测试本机 fixture，没有使用用户凭据或外部 MCP。
- 在现有 transport/client 边界拥有动态会话状态，安全读取并带回标识；配置凭据与 session header 分开，错误及日志不得泄漏真实会话标识。
- 同时明确支持的协议版本、协商/拒绝规则及 legacy SSE 的不同连接流程；当前固定的 2024-11-05 版本不应让代码注释中的 Streamable HTTP 支持声明替代真实兼容测试。
- 补初始化、普通调用、ping、并发请求、过期 404、重建及关闭测试；恢复会话不能绕过 H-06 的副作用未知结果限制。此探针仅在 http feature 下编译，默认忽略，生产 transport 尚未修复。

### H-13：AppCore 插件就绪快照与共享技能注册表不同步，新请求可继续声明已禁用技能

**级别：中（P2）；直接复现范围：使用公共 PluginService + ChatService 生命周期的宿主，在禁用后立即创建新会话。** 桌面 `set_plugin_enabled` command 额外同步 skills，该命令返回后的路径不能据此判定同样失败。

#### 触发与复现

- `PluginService::set_enabled()` 保存禁用设置并执行 `PluginPreparation::invalidate()`，立即清空 ready snapshot；但 SkillsService 是另外一份共享注册表，不随此次失效立即更新。
- `ChatService::start_plugin_preparation()` 仅在后台 changed 回调时用 ready skill_roots 更新 SkillsService。`build_skill_catalog_blocks()` 与 `tool_runtime::register_skill_tool()` 直接从后者读目录/复制 registry，没有核对该请求捕获的插件 ready generation。
- 探针 `chat_service::tests::audit_disabled_plugin_skills_do_not_enter_new_model_request`：创建真实临时插件和 SKILL.md，启动生产宿主准备，等待首次就绪及技能发布；调用公共 set_enabled(false)，确认 ready skill_roots 已为空，再通过真实 ChatService/Kernel 发起全新会话，截获模型的 TransportRequest。模型只有离线固定回复，未使用外部 API。
- 执行：`cargo test -p deepagent-app-core --lib audit_disabled_plugin_skills_do_not_enter_new_model_request --offline -- --ignored --nocapture`。
- 实际输出：`enabled=false, ready_skill_roots=0, new_request_has_disabled_skill=true`；新请求不应包含禁用技能的断言失败。该会话没有历史，不能用“保留过去的技能调用记录”解释本次新注入。
- 后续收紧探针：插件 fixture 放在独立聊天工作目录之外，仅匹配完整目录条目 `- audit-plugin-skill: Audit plugin readiness`，再次运行仍得到相同失败。排除动态环境中的普通文件路径命中技能名造成误报；不是仅据 body 中出现一个文件夹名称定性。
- 桌面 `lib.rs::set_plugin_enabled()` 在返回前调用 `sync_plugin_runtime_after_change()` / `sync_plugin_skill_roots()`，是一个宿主特有的补同步路径。本探针明确没有调用这层 Tauri handler；文件 watcher 失效也可能经过共享注册表更新窗口，但尚未独立做 watcher → 桌面 wire 复现，不能扩大结论。
- 2026-10-23 在 Windows 工作区重跑上述 ignored 探针，仍得到 `enabled=false, ready_skill_roots=0, new_request_has_disabled_skill=true`，断言失败；确认 AppCore 直连边界的问题仍存在，不改变桌面 handler 尚未动态验证的范围。

#### 成熟实现对照

Claude Code `services/plugins/pluginOperations.ts::setPluginEnabledOp()` 在写 enabledPlugins 后统一调用 `utils/plugins/cacheUtils.ts::clearAllCaches()`，后者清插件、命令、Agent 及 skill prompt cache，并 resetSentSkillNames。它把失效组织在插件操作边界，避免要求每个 UI/CLI 调用方自行记住一套附加同步。

该参考实现对 hook pruning 仍有异步生命周期，源码注释明确暂时保留旧 hooks 直到 prune 完成；这里不声称其所有能力在所有时刻都是原子替换。采用的边界是统一失效入口和后续请求重新读取有效能力。

#### 后果和修复方向

- 新请求向模型声明已经不就绪/禁用的技能，造成模型选错能力；register_skill_tool 也可能复制旧 registry。探针直接证明新目录注入，没有调用禁用技能或验证任何权限绕过。
- 禁用与能力失效属于 AppCore 插件生命周期，不能只在 Tauri command 补同步。使该请求的 skills catalog、SkillTool registry、MCP/hook/Agent 投影来自同一份可验证就绪代次，或在统一边界进行同步失效/快照读取。
- 不为解决竞态重新把插件校验和连接放回会话路径；未就绪能力应缺省不可见。补 AppCore/CLI/Desktop、watcher、禁用与新请求交错、多插件及准备失败用例。此探针默认忽略，生产逻辑未修改。

### H-14：无效压缩保留原上下文时，已完成摘要请求的 usage 一同被丢弃

**级别：中（P2）；范围：同步原生压缩已取得 provider usage，但替换结果不比原文更小。** 与 H-10 的后台任务脱离生命周期不同，本项摘要已经完成并返回；prefire note 未采纳时另有记账分支，不能概括为所有失败压缩都漏记。

#### 触发与复现

- `HookedReactiveContextCompactor::compact_response_items()` 取得 `(summary_block, summary_usage)` 后比较 tokens；若 `tokens_after >= tokens_before`，直接返回 `Ok(None)`，usage 没有返回通道。
- `ModelAgent::maybe_proactive_compact()` 仅在 `Some(ReactiveCompaction)` 的 adoption 中归集同步摘要 usage；`None` 只增加失败计数。生产 `run_assembler.rs` 收尾读取 `agent.cumulative_usage()` / `agent.side_usage()`，后者无法看到已经丢掉的摘要用量。
- 探针 `context_runtime::tests::audit_ineffective_native_compaction_still_accounts_summary_usage` 使用真实 ModelAgent、生产 HookedReactiveContextCompactor、keep-window 选择器和内存 Session 数据库；主模型和摘要模型分别使用可观察离线 transport。历史为一个很短的前缀加足够大的近期尾部，断言生产选择器确实只压缩短前缀；摘要是正常短 JSON，没有超过 1024 输出上限的大型假回复。
- 执行：`cargo test -p deepagent-app-core --lib audit_ineffective_native_compaction_still_accounts_summary_usage --offline -- --ignored --nocapture`。
- 实际输出：`model_calls=2, summary_usage=120+18, main_billed=120, side_usage=None, adopted=false`。摘要及主模型均成功返回 120 输入 / 18 输出，下一主请求仍包含原文，且未写入 ContextCompacted；要求 side_usage 收到摘要用量的断言失败。
- 这直接证明 Agent 对运行收尾提供的用量缺失；没有请求真实供应商，也没有把测试的固定 token 数当作实际收费金额或验证桌面账单截图。

#### 成熟实现对照

Codex `core/src/compact.rs` 在压缩流收到 `ResponseEvent::Completed` 时，先调用 `record_observed_response_completed()` 及 `update_token_usage_info()`，然后才返回 CompactionResponse 给后续替换路径。已观察到的用量归集与是否最终采用 replacement 分离。

本系统底层 `ModelCompactor::summarize_accounted()` 也已经在 JSON 解析或 heuristic fallback 之前归集 usage；缺口在上层把“没有可采用的替换结果”和“没有发生模型消耗”都表达成了 None。

#### 后果和修复方向

- 运行及会话的模型用量少计已经完成的摘要请求；反复无效压缩会继续消耗而本地统计不完整。外部供应商仍可能正常收费，不能把保留原文解释为请求免费。
- 在既有 compactor outcome / 运行计费边界里分别携带“已观察用量”和“可选 replacement”，或通过同一运行所属的用量收集器归集；不能只在 adoption 成功时记账，也不能新增第二套账本。
- 保留摘要必须更小的保护，不通过强行采用更大的摘要修复记账；同一请求用量只能归集一次。后续覆盖无效摘要、post-hook 拒绝/失败、持久化失败、fallback、prefire stale、用户取消和重试。
- 此缺陷探针默认忽略，生产行为未修改；hook/持久化失败的漏记属于相邻待验证分支，本次没有据此独立定性。

### H-15：审批等待被取消后未清理，运行终态后仍接受迟到回答并授予跨调用权限

**级别：高（P1）；范围：工具已进入人工审批，用户取消运行，然后对旧审批提交 tool_session 等扩大作用域回答。** 本次验证取消本身有效、命令没有执行；问题是已结束运行的审批生命周期及迟到授权。

#### 触发与复现

- `ToolPipeline::request_approval()` 用 select 等待审批或运行取消。取消分支把 action 记为 Cancelled，但丢弃 ChannelApprovalGate future 时，没有删除 PendingApprovals 中保留的 sender。
- `RunStore::finish_with_event()` 写 cancelled 终态，没有同时使该 run 的待处理审批失效。NewRunApproval 的 expires_at 为 None，记录仍是 Pending。
- `RunCoordinator::resolve_approval_scoped()` 只验证审批记录及 offered scopes，没有拒绝所属 run 已终结的回答。数据库写入成功后，`PendingApprovals::resolve_scoped()` 即使 send 失败（receiver 已随取消丢弃），仍记录 tool_session grant；协调器因存在 durable record 返回 Ok(true)。
- 探针 `tests/kernel_v2_e2e.rs::audit_cancelled_run_rejects_late_approval_and_releases_waiter` 使用真实 ChatService 接纳、Kernel、审批 gate、运行取消树和内存 SQLite；离线模型发出 bash 调用，命令 executor 为记录型拒绝 fixture，绝不创建系统进程。等待真实审批通知后取消，确认唯一 cancelled 终态及 action=Cancelled，再通过公共 resolve_approval_scoped 提交旧回答。
- 执行：`cargo test -p deepagent-app-core --test kernel_v2_e2e audit_cancelled_run_rejects_late_approval_and_releases_waiter --offline -- --ignored --nocapture`。
- 实际输出：`terminal=cancelled, action=cancelled, command_calls=0, pending_after_terminal=1, durable_state=Pending, late=Ok(true), granted_after_terminal=true, last_event=approval.responded`。要求释放审批并拒绝迟到权限的断言失败；还在 run_terminal 之后追加了旧审批的 answered 事件。
- 这是 AppCore 真实审批/取消/持久化链复现，没有点击真实桌面旧弹窗，也没有执行后续真实命令。不能据此声称已经停止的命令被重新启动。

#### 成熟实现对照

Codex `core/src/state/turn.rs` 把 pending_approvals 放在 TurnState；`clear_pending_waiters()` 清审批、用户输入等 waiter。`core/src/tasks/mod.rs` 取消任务后通过 `session/input_queue.rs::clear_pending()` 清理该 turn 的等待对象；注释强调先让任务观察取消，再清 waiter，避免把中断误投影成模型可见的用户拒绝。

可以借鉴运行/turn 所属的审批生命周期及有序清理，不能通过全局 clear 影响其他并发会话。持久化审批过期还需本系统沿用现有 run_approvals/run_events 完成。

#### 后果和修复方向

- 停止后旧审批仍占用共享注册表和数据库 Pending 状态；迟到回答被报告受理，能留下跨调用授权。ChannelApprovalGate 后续对相同 tool/risk 会读取该 grant，可能减少下一调用的人工提示；本次没有独立执行下一调用验证 UI，也不等于其他文件/沙箱权限被绕过。
- 在现有终态收尾中按 run 失效/清理待处理审批，与接收迟到回答共享一个可验证状态边界；answer 处理应核对 run/action 仍允许等待，不能仅检查 approval_id 存在。
- waiter 需要携带 run/turn 身份并具备取消/drop 清理，持久化投影同步过期；范围授权只能由仍有效的审批产生。不要用全局清空、静默吞 send 失败或延迟按钮遮盖问题。
- 覆盖审批期间停止、回答/取消竞态、重复回答、工具作用域与命令前缀持久化作用域、两会话隔离、进程重启及审计写失败。本项只验证 tool_session；command_prefix 采用同一路径但尚未独立复现，不把它当作已经执行的测试。
- 此缺陷探针默认忽略，生产审批逻辑未修改。

### H-16：审批直接以 provider call_id 为全库身份，不同运行之间产生幂等冲突

**级别：中（P2）；触发条件：两个独立运行收到相同的 provider tool-call id，且都需要审批。** 不声称真实供应商在本次桌面样本中返回了重复 ID；这是宿主在缺少全库唯一保证时的身份边界缺陷，与 H-15 的终态清理不同。

#### 触发与复现

- ModelAgent 将 provider 的调用标识原样放入 ToolInvocation.id；`ToolActionPersistence::register_invocations()` 正确使用 `(run_id, call_id)` 检查和持久化 action，仅缺失 id 时生成新值。
- 同一结构的 `request_approval()` 却直接令 `approval_id=call_id`；`run_approvals` 以 approval_id 唯一。第二个 run 的 action 能正常登记，审批则撞到第一条记录；RunControlStore 检查 run 不同后返回 idempotency conflict，不会建立第二条待审批记录。
- 探针 `tool_pipeline::tests::audit_distinct_runs_can_approve_the_same_provider_call_id_independently` 使用真实内存数据库和生产 ToolActionPersistence：第一项审批拒绝后，另一独立 session/run 使用相同 provider id 请求自己的审批。两个会话没有共享模型历史，排除同一请求内重复 tool id 的混杂因素；工具仅为计数 fixture，测试没有 invoke 或模型网络调用。
- 执行：`cargo test -p deepagent-runtime audit_distinct_runs_can_approve_the_same_provider_call_id_independently --offline -- --ignored --nocapture`。
- 实际输出：`first_state=Denied, second_action=Received, second_approval=Err(Invalid("idempotency conflict for approval provider-call-reused")), first_approvals=1, second_approvals=0`；第二运行应有独立审批的断言失败。
- 存储冲突检查在这里正确地阻止了覆盖/复用旧授权；本探针证明合法新 action 无法进入审批，不证明旧会话的一次批准已错误放行另一个会话。

#### 成熟实现对照与修复方向

- Codex `core/src/session/mod.rs::request_command_approval()` 将 effective_approval_id 的 waiter 放进当前 active turn 的 TurnState，并说明匹配必须投递到正确的 in-flight turn；`core/src/state/turn.rs` 的 map 是 turn 所属，避免拿 provider 字符串直接建立全进程的请求身份。
- 沿用现有 run_approvals，给审批独立的宿主身份，或采用明确编码的 run/turn + call 复合身份；保留原始 provider call_id 关联模型调用，不能为修审批而改坏请求中的 tool call/result 配对。
- 内存 waiter、持久化、RuntimeEvent、Tauri/CLI/SDK 回答 DTO 必须使用同一审批身份，并兼容读取旧数据；覆盖两会话相同 ID、同一 run 的幂等重试、同一调用多次审批及重启后的回答。
- 此缺陷探针默认忽略，生产审批身份及协议未修改。真实重复 ID 频率、桌面表现和迁移策略仍需后续验证。

### H-17：桌面收到取消后的保存失败，却因对话已 settled 而静默丢弃错误

**级别：中（P2）；触发条件：取消已被模型执行循环观察，但取消审计或终态保存失败。** 此项不是取消信号被数据库挡住；停止本身已经生效，缺陷在执行状态与保存状态的 UI 投影。

#### 真实桌面复现

- 重新执行 `cargo build --manifest-path apps/desktop/src-tauri/Cargo.toml --offline` 成功（约 1m26s），直接启动 Vite 与本次桌面产物，使用独立 `target/live-desktop-test-20261021` 数据目录和现有专用测试供应商配置。通过 CDP 操作实际 Tauri WebView，先验证 `__TAURI_INTERNALS__` 存在，排除普通浏览器 mock。
- 首次探索及随后提交的可重跑探针均复现。探针 `apps/desktop/scripts/audit-cancel-storage.cjs` 不在默认测试/build 中执行；需要 Node 的 `node:sqlite`、Playwright、已启动的隔离桌面以及真实测试模型，会发起一轮付费请求。`AUDIT_DATA_ROOT` 必须解析到本仓库 target/ 的子目录，不能指向用户生产数据。
- 操作：发送不调用工具的长文请求，等真实 `model_first_token`，在隔离 SQLite 中为该 run 安装仅拒绝 `cancel.requested` 插入的 trigger，随后点击“停止”。finally 撤销 trigger 并断开 CDP；不改生产取消代码，不模拟 UI 事件。
- 第二次实测 `cancel_run` 请求到 HTTP 200 约 **2ms**。事件按顺序为 `run_cancelled → session://completed {status:failed,error:"persistence error: audit injected cancel audit storage failure"}`；前者比点击时间晚约 38ms，后者晚约 59ms。这是单次样本，不能声明 P95 达标或所有资源已回收。
- 等待 1.2 秒后的 DOM 和实际截图都无保存失败提示，发送按钮可见、停止按钮消失；`visibleStorageError=false`。探针在“界面必须显示保存错误”的断言明确失败。数据库该 run 保持 `state=finalizing, terminal_kind=NULL, finished_at=NULL`，有 `run_cancelled` 观察事件，但没有成功的 `cancel.requested` 或 `run_terminal`；未把事务回滚误说为终态保存成功。
- 截图与仅含事件元数据的结果：忽略提交的 `target/live-desktop-test-20261021/cancel-storage-failure.png`、`cancel-storage-failure.json`。没有真实制造磁盘写满或损坏生产数据库，故障类型是确定性的审计插入失败。

复跑前启动隔离桌面并配置测试模型，设置 `AUDIT_DATA_ROOT`、`AUDIT_PLAYWRIGHT_MODULE`（本机 Playwright 模块绝对路径）、`AUDIT_CDP_URL` 后执行：

```text
node apps/desktop/scripts/audit-cancel-storage.cjs
```

#### 根因与成熟实现对照

- Kernel 在转发 `RunCancelled` 之后才执行 `persist_terminal()`；保存失败沿 Result 返回。Tauri `start_chat_v2` 正确将这个 Err 转成 `session://completed` 的 failed/error，`api.ts::runChat()` 也正确 reject，错误没有在这两层丢失。
- `App.tsx::finalizeCancelled()` 已将 assistant turn 置为 settled；随后 runChat 的 catch 仅在 `last.turn.phase !== "settled"` 时调用 error finalize。因此合法的迟到保存错误直接被跳过。`onStopRun` 的 cancel IPC catch 处理的是另一条 Promise，无法覆盖这个错误；finally 继续清理 running 状态。
- Codex `tui/src/chatwidget/turn_runtime.rs::on_error()` 先 finalize_turn，再将独立 error event 加入历史并请求重绘；错误展示不依赖 turn 仍处于运行中。它提供可借鉴的“执行结束与错误展示分离”边界。
- 同时核查 Codex `core/src/tasks/mod.rs`：中断标记在 TurnAborted 前尝试 flush，终态后另做 flush；相应 flush 失败在该路径只是 warn。因此不声称 Codex 已通过本系统相同的数据库故障和 UI 验收，也不照搬其日志策略作为完整修复。

#### 影响与修复方向

- 用户无法区分“执行已停止”和“停止结果未保存”，也没有管理/恢复提示；重新打开或重启时可能看到未收尾记录。本次没有据此断言下一次启动一定重复执行工具或恢复成何种终态。
- 在现有运行与通知链路中独立表达执行结果和保存诊断，允许 settled 后追加一次、按 run/session 隔离的保存错误；不能把 cancelled 再改成“执行未停止”，也不能为避免错误而提前谎报 durable terminal。
- 覆盖正常取消与取消错误去重、错误晚于 settled、切换会话后错误、终态保存重试以及重启恢复。若改事件/DTO，沿用同一 run_events/RunStore 边界同步消费方。本轮只增加审计探针和报告，未修生产 UI。

### H-18：输出上限重试后仍截断时，未完成工具调用被补参数并提交

**级别：中（P2），若工具接受空参数并使用有副作用的默认行为，影响可升高；触发条件：首次输出上限恢复已用尽或无法提高，响应仍包含未完成工具调用。** 本项验证到真实 ModelAgent 的决策及 Surface 提交，没有执行真实文件修改或系统命令。

#### 复现与生产路径

- `ChatCompletionAccumulator` 收到 `[DONE]` 会无条件 complete_tool_calls；`finish()` 把空 arguments 改为 `{}`。Responses accumulator 在 `response.incomplete` 后的 `finish()` 也把尚未收到 item.done/arguments.done 的 builder 转成原生 FunctionCall，空参数同样补 `{}`。
- `ModelAgent::think_inner()` 第一次 length 会尝试把输出上限升至 65,536 并 abort 原 attempt，这一步正常。但再次 length 后，只有 `tool_invocations_from_items().is_empty()` 才进入续写恢复；非空时直接 commit，然后在最终 length 错误判定之前返回 CallTool/CallTools，并将原生调用加入历史。
- 默认忽略探针 `model_agent::tests::audit_output_limit_does_not_dispatch_unfinished_tool_calls` 使用生产 ModelClient、两种 accumulator、ModelAgent 与记录型 ToolAttemptController。每种协议分别输入“只开始调用，尚无参数”和“不完整 JSON 参数”，连续两次明确 max-token 终态，另留一份成功响应用于确认是否实际进入恢复。Responses fixture 没有伪造 item.done 或 arguments.done。
- 执行 `cargo test -p deepagent-runtime audit_output_limit_does_not_dispatch_unfinished_tool_calls --offline -- --ignored --nocapture`：四组都仅请求两次，均返回 `Ok(CallTool(...))`，原生 Surface 均保留一个 truncated-call，断言明确失败。Chat 的记录为 `begin:1 → prepare → abort:1 → begin:2 → prepare → commit:2`；Responses 为 `begin:1 → abort:1 → begin:2 → commit:2`。第三份正常响应没有消费。
- 空参数两组决策为 `arguments={}`；半截 JSON 两组带 `__invalid_tool_arguments__` 哨兵。生产 `ToolPipeline::prepare()` 确实会在 hooks/权限/执行之前拦截该哨兵，因此**不声称半截 JSON 能绕过校验执行**。空 `{}` 没有哨兵，后续是否执行仍取决于工具 schema、权限和审批，不等于这些边界被绕过。

#### 官方语义与成熟实现

- [DeepSeek Chat API](https://api-docs.deepseek.com/api/create-chat-completion/) 将 length 与 tool_calls 区分，并要求宿主验证工具参数；length 表示输出达到指定上限，不能仅依据已有工具名字推断调用完整。
- DeepSeek Harness `packages/llm/llm/src/assembler.ts::assembled()` 在 `finish.kind === 'max-tokens'` 时统一去掉 tool-call blocks，并同步过滤 replay metadata；不会把被丢弃的调用在下一请求重放。
- 它的 `llm-pi-ai/tests/loader-composition.spec.ts` 明确用“partial 文本 + 半截工具参数 + length”检查：保留文字，去掉 tool-call，续写请求里没有 tool_calls。这里引用仓库实现/测试作为设计证据，没有在本机执行其完整测试套件。

#### 后果与修复方向

- 未完成调用会成为执行候选、触发一次校验失败/工具循环，并污染原生 replay；空参数还可能改变工具的默认操作范围。此次 fixture 未执行工具，不能将潜在默认副作用说成已发生。
- 在同一模型响应提交边界统一处理终态与调用完整性；截断恢复的保留集合应同时决定决策、原生 Surface、stream attempt commit 及 durable replay，避免仅 UI 隐藏调用。借鉴 Harness 保留可安全续写文本、剔除截断调用并同步 replay 的方式。
- 保留正常完成的无参调用、既有权限/审批和调用配对；新增多调用中途截断、第二次 length、无法升级输出上限、Responses 未闭合 item、取消与截断交错及重启测试。本轮未改生产模型或工具逻辑。

### H-19：子 Agent 的工具限制只过滤 schema，实际执行仍可调用被排除的工具

**级别：高（P1）；范围：ChatSubagentRunner 的内置只读角色、请求 allowlist 和角色 denylist。** 本项实际写入了临时文件，不只是观察模型 schema；没有触碰用户文件，也没有调用真实供应商或系统命令。

#### 触发与复现

- `subagent_runner.rs::run_active()` 克隆父 ToolRegistry；schema callback 通过 `apply_runtime_agent_tool_filter()` 及 request.allowed_tools 过滤模型工具定义，但 RuntimeEngine 接收的仍是完整 sub_registry。`permission_set_for_agent()` 对只读角色也返回 developer 权限，因此工具 schema 限制没有形成执行边界。
- 默认忽略探针 `subagent_runner::tests::audit_read_only_subagent_rejects_tools_hidden_by_its_profile` 使用真实 ChatSubagentRunner、RuntimeEngine、文件工具实现及内存数据库。模型 transport 固定返回一个完整的 write_file 调用，随后正常完成；未安装自动批准 gate。
- 四个独立临时目录分别测试内置 explore、内置 plan、request.allowed_tools=[Read]、自定义角色 disallowedTools=[Write]。逐次检查实际 wire request 均没有 write_file schema；前三组列表分别是只读四工具、只读四工具、仅 read_file，第四组保留其他文件工具但没有 write_file。
- 执行：`cargo test -p deepagent-app-core --lib audit_read_only_subagent_rejects_tools_hidden_by_its_profile --offline -- --ignored --nocapture`。
- 实测四组都 `outcome=Ok, tool_use_count=1`，并在各自目录创建 `audit-child.txt`，内容为 `hidden tool executed`。最终要求零越界写入的断言失败：`[true, true, true, true]`。单组运行约 91–108ms，不是性能统计。
- 当前 WriteFileTool 属于 Low risk，要求 WorkspaceWrite；developer 权限满足该要求。因此本次没有绕过审批程序的证据，缺口是角色/请求声明的工具限制未进入实际 dispatch。测试调用真实子运行，但没有经过完整父会话接纳或桌面 UI，也不声称供应商在自然对话中已返回过清单外工具。

#### 成熟实现对照与修复方向

- Claude Code `tools/AgentTool/runAgent.ts` 先得到 resolvedTools，再组装 allTools 并写入 agentOptions.tools；`services/tools/toolExecution.ts` 从 `toolUseContext.options.tools` 查找实际调用，找不到时返回结构化 `No such tool available`。该实现另有废弃别名兼容分支，不能概括成完全没有 fallback。
- 应在现有 ToolRegistry/ToolPipeline 边界生成同一份有效工具集合，让 schema 与 dispatch 共享其身份和限制；模型返回隐藏工具时应拒绝并补配对失败结果，不能仅用提示词劝模型遵守。
- 角色限制需与父权限取交集，避免只读角色获得更宽的写权限；保留原有文件边界、审批及沙箱校验，不建立第二套工具执行器。覆盖 alias、deferred/MCP 工具、角色 denylist、空 allowlist、后台子运行和恢复后的限制一致性。
- 本轮仅增加审计测试和报告，未修改生产权限或工具路由。

### H-20：DAG 成功节点的工作树被强制删除，文件未交付给主目录或下游

**级别：高（P1）；范围：plan_execute 的 DAG 工作树调度。** 本项用真实临时 Git 仓库复现未提交文件和修改丢失；没有删除本项目或用户工作树。

#### 触发与复现

- `ChatPlanExecutor::execute_plan()` 使用生产 DagScheduler + GitWorktrees。`DagSubagentExecutor` 将子运行重定根到调度器工作树，并指定 shared isolation，因此子运行自己的成功工作树保留逻辑不会接管该目录。
- `DagScheduler::run()` 每个节点执行完后，无条件调用 worktrees.remove，并忽略返回错误；实际 `GitWorktrees::remove()` 执行 `git worktree remove --force`。在这之前没有合并、补丁或资产交付。后继节点从主仓库 HEAD 另建 detached worktree，只收到上游 summary。
- 默认忽略探针 `scheduler::tests::audit_successful_dag_preserves_files_for_handoff` 创建一次性真实 Git 仓库和初始提交，然后使用生产调度器与工作树实现。记录型 executor 的 produce 节点修改 tracked README.md 并创建 untracked generated.txt，现场读回校验；consume 节点确认收到了摘要并检查自己的目录是否有该文件。
- 执行：`cargo test -p deepagent-subagents audit_successful_dag_preserves_files_for_handoff --offline -- --ignored --nocapture`。
- 实测 `all_succeeded=true, producer_retained=false, consumer_saw_file=false, main_has_file=false, main_readme="original"`；要求成功输出至少保留或集成一处的断言失败。上游文件确实写出，却在下游启动前被强制删除；主工作区没有获得 tracked 修改或新文件。
- 此处两节点 executor 为确定性文件 fixture，没有调用模型或完整 ChatPlanExecutor。`all_succeeded` 是 fixture 两次返回 success 的聚合，不代表真实 review 遇到缺文件也必定成功。测试只覆盖未提交修改，不声称已提交 Git 对象不可恢复；没有验证清理错误被吞掉的另一分支。

#### 成熟实现对照与修复方向

- Claude Code `tools/AgentTool/AgentTool.tsx::cleanupWorktreeIfNeeded()` 调用 `utils/worktree.ts::hasWorktreeChanges()`；仅确认无变化时删除，否则返回保留的工作树路径/分支。变化判据包含 dirty tree、新 commits；检查失败也按有变化处理，避免误删。hook 创建的未知 VCS 工作树同样保留。
- 本系统普通 task 子运行成功时已经保留隔离工作树，DAG 所属目录却走了不同的清理语义。应在现有工作树所有权边界统一“产出交付后才清理”，沿用现有子运行记录与结果投影，不新增另一套工作树注册或存储。
- 明确输出交付契约：至少保留可定位、可恢复的工作树/提交；有依赖的下游须得到可读取的产物或受控集成后的基线。仅保留摘要或把路径写进文字，不能让已经删除的文件可用；自动集成涉及冲突和权限，应作为明确策略验证。
- 覆盖修改/无修改、未提交/已提交、失败与取消、合并冲突、清理失败、重启定位和并发 DAG 的工作树身份。此次未改生产清理逻辑或引入自动合并。

### H-21：子 Agent 恢复遗漏原始输出契约，schema、phase、label 全部丢失

**级别：中（P2）；范围：task operation=resume 的原始请求持久化和重新投影。** 本项不是 H-04 的主会话恢复覆盖值问题，而是子运行已有字段在 writer/reader 之间不对称。

#### 触发与复现

- `run_tracked()` 保存 request 时写了 description/prompt/type/allowed_tools/model/effort/skills/isolation，却没有 phase、label、schema；`resume_request()` 明确读取这三个字段，并注释为保留原 frontmatter，但缺失时全部为 None。
- `subagent_system_prompt()` 在 schema 存在时注入原 schema 和必须返回匹配 JSON 的要求。恢复后 schema=None，连同该输出要求一起消失；不是只有管理列表少显示一个标签。
- 默认忽略探针 `subagent_runner::tests::audit_child_resume_preserves_schema_phase_and_label` 通过生产 runner 完成首次子运行、读取真实 SubagentRunStore 和生成的 transcript，然后调用正式 SubagentRunner::resume。两个模型响应是离线 fixture，但两次实际请求都由生产 ModelAgent 构造并记录。
- 执行：`cargo test -p deepagent-app-core --lib audit_child_resume_preserves_schema_phase_and_label --offline -- --ignored --nocapture`。
- 首次实际 wire instructions 包含 audit-contract-phase、audit-contract-label 和 audit_resume_value schema；子运行 succeeded。恢复后仍 succeeded，resume_count=1，但 `restored_phase=None, restored_label=None, restored_schema=None`，恢复的 wire instructions 对应三项均 false；契约相等断言失败。没有伪造 transcript 或手工删除字段。
- 这证明恢复请求不再要求原输出格式，不证明真实模型一定返回无效 JSON。本次未模拟宿主重启，也未验证长历史丢失；原恢复实现另以原任务+2000 字符结果摘要重建请求，完整子历史是否应持久化仍需单独定义和测试。

#### 成熟实现对照与修复方向

- Claude Code `tools/AgentTool/resumeAgent.ts` 并行读取 getAgentTranscript 与 readAgentMetadata，恢复消息/replacement 状态、角色和工作目录后重新运行；`sessionStorage.ts::AgentMetadata` 特别说明缺角色元数据会使 fork 恢复静默退化。Codex `core/src/agent/control/resume.rs` 恢复后返回实际 ThreadConfigSnapshot。可借鉴的是恢复有效状态并验证其完整性，不是照搬它们的字段或存储。
- 本系统应在现有子运行 request/transcript 边界统一序列化与恢复契约，避免 writer/reader 手工字段清单不同步；兼容旧数据，对字段缺失给出明确默认或恢复诊断，不凭空猜出旧 schema。
- 覆盖首次运行→继续、再次继续、跨父 turn、宿主重启、旧 transcript、工具限制和模型配置；完整历史与压缩 replacement 如需恢复，应复用现有事件及 Surface 能力，不能另造子 Agent 运行内核。
- 本轮仅增加测试与报告，并把测试 runner fixture 提取复用；未修生产保存或恢复行为。

### H-22：DAG 子运行只改变声明的工作目录，文件工具仍绑定父工作区

**级别：高（P1）；范围：DagSubagentExecutor 到 ChatSubagentRunner 的工作区绑定。** 本项与 H-20 分开：H-20 是调度器删除已写入工作树的产出；本项是实际工具可能根本没有写入所分配的工作树。

#### 触发与复现

- `DagSubagentExecutor::execute()` 调用 runner.with_root(assigned_worktree)，其实现仅修改 self.root，没有重建 parent registry 中已绑定根目录的工具。请求使用 shared isolation，execution_root() 返回这个新 self.root。
- `run_active()` 仅在 execution_root != self.root 时重绑工具；DAG 路径两者已相等，因此跳过重绑并克隆父工作区 registry。system prompt/checkpoint 的根却是新目录，形成声明与实际读写目标不一致。
- 默认忽略探针 `subagent_runner::tests::audit_dag_child_rebinds_file_tools_to_assigned_worktree` 创建临时 parent 与 assigned-worktree 两个目录，在 parent 上构建真实文件工具 registry；将 assigned-worktree 作为真实 SubAgentContext 传给生产 DagSubagentExecutor。固定模型返回完整 write_file 相对路径调用，之后正常结束。
- 执行：`cargo test -p deepagent-app-core --lib audit_dag_child_rebinds_file_tools_to_assigned_worktree --offline -- --ignored --nocapture`。
- 实测子运行 `ok=true`，每次实际 wire instructions 都声明子目录，但 `parent_output=Some("hidden tool executed"), child_output=None`；要求只写入分配目录的断言失败。真实 WriteFileTool 执行，没有跳过权限或虚构 tool result。
- 本探针验证生产执行边界，用目录模拟调度器已经分配的上下文，没有在该测试再启动 Git、模型网络或完整 plan_execute。与 H-20 的真实 Git 调度器探针互补，但不能将二者拼成已经做过完整端到端模型编排。只验证 write_file，其他绑定工具需独立覆盖。

#### 成熟实现对照与修复方向

- Claude Code `AgentTool.tsx` 将完整同步执行包在 wrapWithCwd/runWithCwdOverride 内；`utils/cwd.ts` 使用 AsyncLocalStorage 为并发子运行派生有效 cwd。FileWriteTool 使用 expandPath/getCwd，路径展开以该执行上下文为依据；提示词和文件执行共享有效目录，不只修改提示文字。
- 本系统已有 run_active() 的 registry 重绑分支，应让工作区绑定与实际 registry 来源保持明确关系，避免 with_root 修改比较基准后跳过重绑。文件、命令、权限、checkpoint、项目查询和模型环境需核对同一有效工作区；保留已有 session_search 会话项目约束，不全局 chdir 或修改共享父 registry。
- 覆盖两个并行子目录写同名文件、父目录不变、相对和绝对路径、sandbox/审批、工具搜索、恢复及普通 task 与 DAG 一致性。H-19、H-20、H-22 显示同一子运行上下文在 schema、工具根、产物所有权处分裂，后续应围绕既有 runner 的有效执行配置统一治理，不逐个增加路径特判。
- 本轮只增加审计证据，没有修生产工具根或删除逻辑。

### H-23：回退会话后旧笔记仍按复用序号宣称覆盖，压缩候选会替换新分支

**级别：高（P1）；触发条件：已有会话笔记，回退到其覆盖点之前，再继续到足够的新事件，且笔记尚未重新生成。** 这是“历史分支身份”缺失，不是简单的重复消息内容匹配问题 H-11。

#### 触发与复现

- `AppService::rewind_session()` 恢复 checkpoint、截断事件日志并重新打开会话，没有同步失效 SessionMemoryState。`Session::rewind()` 允许新事件从保留尾部连续追加，所以被删掉的 sequence 会被新分支重新占用。
- 笔记覆盖状态只记录 last_summarized_sequence，没有事件 ID 或分支代次；`compaction_from_note()` 把当前消息 sequence <= 旧覆盖值视为已总结。native `summarize_response_item_zone()` 也有同样的只比较 sequence 的覆盖判断，后者本次只做代码核对，未独立动态验证。
- 默认忽略探针 `service::tests::audit_rewind_rejects_session_note_from_discarded_branch` 建立真实 Session，先写稳定前缀，再写旧分支 RED 决策；为其保存有效的 note 状态 fixture，覆盖到 sequence=8。调用正式 AppService::rewind_session 保留到 4，然后通过 Session::recover 追加新的 BLUE 分支，事件到 12。
- 执行：`cargo test -p deepagent-app-core --lib audit_rewind_rejects_session_note_from_discarded_branch --offline -- --ignored --nocapture`。
- 先确认当前模型历史已没有 RED，说明 rewind 本身确实删掉旧分支；随后调用生产 compaction_from_note，实际得到 `Some((4,"Discarded decision: use RED"))`，且将被替换的前缀已经包含 BLUE 新决策。要求拒绝旧分支 note 的断言失败：`keep=4, old_covered=8, current_tail=12, drops_new_branch=true`。
- 本次验证到真实 rewind/恢复及压缩候选选择，没有向真实供应商发送该候选，也没有实际提交 ContextCompacted。笔记是确定性持久化 fixture，不是自然模型提取，故不涉及摘要质量；首次编译探针因缺少测试 import 失败，补齐后才得到上述产品行为证据。

#### 成熟实现对照与修复方向

- Claude Code `services/SessionMemory/sessionMemoryUtils.ts` 用 lastSummarizedMessageId 跟踪覆盖端点；`services/compact/sessionMemoryCompact.ts` 在有端点时按 message.uuid 查找，当前历史找不到该 ID 则 return null，回退到模型压缩。它另有无端点的 resumed-session 策略，不能概括为所有恢复分支都使用这一保护。
- 本系统现有 session_file_state 和 session files 已有 generation，事件也有稳定 EventId。覆盖证明应绑定实际事件身份/分支代次，回退后失效旧 note 覆盖及 extraction 水位；只对 sequence 做数值截断不能证明旧摘要属于新历史。
- 笔记文本本身包含被撤销的决策，不能仅把 last_summarized_sequence 改小而继续使用旧 summary。采用现有存储里的失效/重新提取策略，或可证明对应保留分支的旧快照；native 与 Message 两条压缩入口应共用同一覆盖校验。
- 覆盖回退后序号复用、回退期间后台 extraction 晚完成、fork、宿主重启、未达提取阈值、覆盖端点保留/删除以及工具配对。未在本轮修生产 rewind、笔记状态或压缩实现。

### H-24：较长的非 ASCII 供应商错误触发 panic，结构化错误处理被中断（已修复）

**状态：已修复。级别：中（P2）；范围：启用 http feature 的 ReqwestTransport 非成功响应错误解析。** 修复前确认函数 panic，但没有证明整个桌面进程退出。

#### 触发与复现

- 修复前，`crates/deepagent-models/src/reqwest_transport.rs::stream_inner()` 对非成功 HTTP 状态读取 body 后调用 `provider_error()`；后者统一调用 `sanitize_provider_message()`。错误文本超过 2000 字节时，直接 `String::truncate(2000)`，未检查 UTF-8 字符边界。
- 回归 `audit_long_unicode_provider_error_remains_structured` 继续分别给生产 `provider_error` 输入 HTTP 429 的 JSON error.message 和纯文本 body。文本为 800 个“错”（2400 字节），无密钥或用户数据。
- 修复：统一按字符边界截断到 2000 个字符，并保留 provider status/code；同时把 Bearer 后继词和嵌入的 `sk-` token 纳入同一脱敏函数。
- 修复后执行：`cargo test -p deepagent-models --features http reqwest_transport::tests:: --offline -- --nocapture`，JSON 与纯文本两组均 `parser_panicked=false`，回归通过。测试没有调用外部供应商。

#### 后果、成熟实现与修复方向

- 正常应返回包含 HTTP status/code/message 的 CoreError，此时却发生 panic，调用方无法按该返回值分类限流、上下文超限或服务端故障。上层是否捕获任务 panic、如何收尾运行及 UI 具体表现尚未独立验证，不据此宣称桌面崩溃或取消失效。
- 仓库 Codex `codex-rs/protocol/src/error.rs::truncate_with_ellipsis()` 在截断前向前查找 `is_char_boundary`；UnexpectedResponseError::display_body 的非结构化 body 分支使用它。其 JSON error message 分支另有处理，不能声称该 helper 覆盖它的所有错误文本。
- 该修复保留既有统一错误清理边界，安全处理 Unicode 长度、脱敏和 status/code；真实 HTTP GET 错误现在也复用该 provider 错误路径。完整 Kernel/Tauri 终态仍不由这个单元测试代替。

### H-25：丢弃供应商 Retry-After，在服务恢复前耗尽限流重试（已修复）

**级别：中（P2）；范围：真实 HTTP 非成功响应到 ModelAgent 的自动重试链路。状态：已修复并转为常规回归。** 触发条件是供应商实际返回有界 Retry-After；不声称所有 DeepSeek 请求都会返回这个头。

#### 生产路径与实测

- 修复前，`ReqwestTransport::stream_inner()` 只取 status/body 后构造 provider error，`CoreError::Provider` 也只有 status/code/message；`ModelAgent` 因此只能用 500ms 基数的本地抖动退避。真实回环探针得到 `provider_wait_ms=3000, actual_retry_gap_ms=455, provider_recovered=false, agent_ok=false`，在供应商恢复前已经耗尽限流尝试。该数值是单次本机样本，不是 P95 或实际供应商故障率。
- 修复后，流式 POST 和管理 GET 在消费 response 前统一提取 `Retry-After` 及 `request-id` / `x-request-id` / `x-deepseek-request-id`，经既有 provider error 边界保存为 `retry_after_ms`、`request_id`；错误正文仍走 H-29 的有界、可取消读取和既有脱敏。秒数及 RFC 9110 要求接收的三种 HTTP-date 形式有单元回归，零值、非法值和已过期日期不会伪造等待。
- `ModelAgent` 在现有重试链采用不超过 10,000ms 的供应商建议；更长建议直接把原错误交还调用方，避免忽略恢复窗口后提前重试。没有建议时继续使用原有本地指数退避；实际等待仍在现有取消 `select!` 中，未新增第二套重试或取消机制。重试日志记录经过长度限制的 request ID，用户可见错误文本不回显它。
- 原 ignored 探针已改为常规回归。执行 `cargo test -p deepagent-app-core --features web --test audit_provider_retry --offline -- --nocapture`，正式经过 ReqwestTransport→ModelClient→ModelAgent，记录 `provider_wait_ms=3000, actual_retry_gap_ms=3018, provider_recovered=true, agent_ok=true`，**1 passed / 0 failed**。fixture 完整读取请求体后才响应，使用独立回环地址及无代理客户端，没有外部密钥或付费请求。
- 执行 `cargo test -p deepagent-models --features http reqwest_transport::tests:: --offline -- --nocapture`，包括真实 GET 429 元数据、HTTP-date/秒数、凭据脱敏及 H-29 取消回归在内 **12 passed / 0 failed**。本轮没有运行公网供应商、完整 Kernel/Tauri，也没有验证 request ID 的 UI 展示。

#### 成熟实现、影响与修复边界

- DeepSeek Harness `packages/llm/llm-deepseek/src/transport.ts::providerError()` 将 Retry-After 秒数/日期映射为 providerRetryAfterMs，保留三种 request-id 响应头；`llm-retry/src/index.ts` 在 normal policy 中原样采用有界建议，超过默认 10,000ms 上限时交还错误处理，不盲目等待。其 always mode 另有本地退避策略；本系统没有复制该模式，只对齐当前有界自动重试语义。
- [RFC 9110 §10.2.3](https://www.rfc-editor.org/rfc/rfc9110.html#section-10.2.3) 说明该头表达客户端应等待多久，值可为秒数或 HTTP-date。这里不是把建议性等待误写成无条件 MUST 协议违规；缺陷是丢失已知恢复信息，导致可恢复请求过早失败。
- 修复保持 provider adapter→`CoreError`→ModelAgent 的单一链路；request ID 丢失造成的跨供应商排障缺口已在重试诊断路径补齐。取消等待由原有 ModelAgent 回归覆盖，但完整 Kernel 停止终态和所有非 Reqwest HTTP 客户端不由本节测试代替。

### H-26：标准 PreToolUse 输出中的 deny/ask 被忽略，实际工具照常执行

**级别：高（P1）；范围：外部 hooks 的 Claude 兼容输出协议。** 不等同于绕过独立的文件边界、ToolRegistry 权限或高风险工具审批。

#### 生产链路与复现

- `deepagent-hooks/src/external_hooks.rs::parse_structured_hook_output()` 只读取顶层 permissionDecision/decision/continue/ok，没有识别标准 `hookSpecificOutput: { hookEventName: "PreToolUse", permissionDecision: "deny|ask" }`。退出码为 0 且解析无结果时，interpret_result 返回 Continue。
- 新增默认忽略的 `tool_pipeline::tests::audit_standard_external_hook_decisions_gate_actual_tool_dispatch`。通过正式 HookDefinitions 解析配置并注册；SystemHookRunner 真实启动 Windows cmd，打印固定 JSON；经 ExternalCommandHook、HookRegistry 和 ToolExecutionPipeline 调用计数型 Safe 工具。没有模型调用、文件副作用或替换生产 parser。
- 执行：`cargo test -p deepagent-runtime --lib audit_standard_external_hook_decisions_gate_actual_tool_dispatch --offline -- --ignored --nocapture`。
- 对照组顶层 deny：`tool_ok=false, tool_calls=0, approval_calls=0`，确认进程与配置链路有效。标准嵌套 deny：`tool_ok=true, tool_calls=1, approval_calls=0`；标准嵌套 ask 同样为 `true/1/0`。两组安全断言失败。审批 fixture 在被调用时会批准，但 ask 组根本没有请求审批。
- 探针含 POSIX sh 分支，本次仅在 Windows 执行，不能宣称 Linux/macOS 已验证。工具为计数 fixture，证据是实际 dispatch，没有执行危险命令或证明额外权限逃逸。

#### 成熟实现、后果与修复方向

- Claude Code `restored-src/src/types/hooks.ts` 定义 hookSpecificOutput 的按事件类型契约；`utils/hooks.ts` 验证 hookEventName 与当前事件一致，再提取 PreToolUse 的 permissionDecision、updatedInput、additionalContext。`services/tools/toolExecution.ts` 使用其权限结果和更新输入继续审批/执行。
- 迁入的兼容 hook 可以成功启动并输出合法 JSON，用户却失去它承诺的禁止/询问策略，且工具成功返回。适用插件/用户依赖该 hook 作为额外审批或策略门禁的情形；现有独立权限仍然有效。
- 在既有统一外部 hook 输出解析器中建立按事件类型的契约，兼容已有顶层格式，明确未知/不匹配事件、无效 JSON 和退出码优先级。权限决策、输入修改和追加上下文应独立承载，避免用单一枚举互相覆盖；后两项本次仅做代码核查，未扩展为独立已复现问题。
- 验收应包含真实进程和实际工具调度、顶层旧格式、标准嵌套 deny/ask/allow、不同事件及错误提示，不只测 JSON 能反序列化。未修改生产 hooks 行为。

### H-27：项目 hooks 信任开关只覆盖独立文件，settings.json 内联 hook 仍执行宿主命令

**级别：高（P1）；范围：项目配置来源到外部 hook 执行的信任边界。** 触发需要打开含相关项目配置的目录并开始会话，本次不是“仅查看目录即自动执行”的证明。

#### 两条加载路径及真实执行证据

- `run_assembler.rs::project_hook_definitions()` 在读取 `.deepagent/hooks.json` 等独立 hook 文件前检查 `ProjectService::hooks_trusted()`，未信任时跳过。这条保护确实存在。
- 另一条 `DualConfigLoader::load()` 读取项目 `.claude/.deepagent/settings.json` 及 local 文件；`RunConfigOverlay::hook_definitions()` 汇总所有 layers 的内联 hooks/hooks_json；`hook_assembly.rs::register_run_config_hooks()` 注册这些 hook 时没有同样的项目来源信任判断。最终与独立 hook 共用 SystemHookRunner 执行宿主命令。
- 新增 Windows 默认忽略探针 `chat_service::tests::audit_untrusted_project_settings_hooks_do_not_execute`。每组使用全新临时项目、内存数据库和离线模型，通过正式 ProjectService 打开项目并显式 set_hooks_trusted(false)，同时确认 TrustService 的 workspace trust 为 false。
- 在第一组 `.deepagent/hooks.json`、第二组 `.deepagent/settings.json` 中分别写入相同 SessionStart 配置：真实 PowerShell 仅向该临时项目写固定 `audit` 标记。运行正式 ChatService→组装→Kernel 会话，不提供模型工具调用，也不自动批准 hook 命令。
- 执行：`cargo test -p deepagent-app-core --lib audit_untrusted_project_settings_hooks_do_not_execute --offline -- --ignored --nocapture`。独立文件 `marker_present=false`；内联 settings 文件 `marker_present=true`，文件内容核对为 audit，最终安全断言失败。证明后者实际执行了 OS 命令，非仅进入内存注册表。
- 本次直接复现 `.deepagent/settings.json`，其他 overlay 项目来源只做代码链核对；未在 Linux/macOS 或真实桌面启动该 fixture。没有读取或修改用户私有文件、凭据、真实项目配置，也没有模型网络请求。

#### 后果、成熟实现与修复方向

- 一个仓库把相同 hook 放在不同受支持配置文件，就能改变信任开关是否生效。执行发生在宿主 hook runner，不依赖模型生成 shell 调用；模型工具层的工作区/审批不能替代配置加载前的来源信任检查。本次只验证写临时标记，不扩大为已发生凭据泄露或系统破坏。
- Claude Code `utils/hooks.ts::shouldSkipHookDueToTrust()` 明确为交互模式的全部 hook 检查 workspace trust，涵盖 SessionEnd/SubagentStop 等路径；非交互 SDK 有显式的隐含信任规则。参考点是按宿主与来源定义统一门禁，而不是声称其非交互模式也拒绝未确认项目。
- 收敛独立 hooks.json 与 overlay 内联 hooks 的来源解析：保留 ConfigLayer 作用域并在注册/执行前应用一致授权；项目、本地、用户、管理员、插件和 run override 的信任来源应可追踪，不把所有配置一概拒绝或全部默认允许。复用现有 ProjectService/TrustService，明确两个开关的关系。
- 验收覆盖相同 payload 跨配置文件、撤销信任、SessionStart/结束/子 Agent、迟到任务、显式非交互授权和插件来源，保证切换文件不能绕过授权。不应只给 `.deepagent/settings.json` 再加一个路径特判。

### H-28：未信任项目的权限配置覆盖用户只读设置，实际写文件没有请求审批

**级别：高（P1）；范围：项目配置来源到有效执行权限的解析边界。** 需要打开项目并开始包含写工具调用的会话；本次证明工作区内未经批准的写入，不声称已证明任意工作区外写入或绕过管理员策略。

#### 生产路径与对照复现

- `RunEnvironment::resolve()` 加载 `RunConfigOverlay`，再用 `apply_permission_profile(settings.effective_permission_profile()?)` 解析本轮权限。项目 overlay 的 `permission_preset: full_access` 会替换用户 profile，并令本轮审批和文件访问权限变为 FullAccess；这条链没有依据项目来源应用 TrustService 或 hooks_trusted 门禁。
- 新增默认忽略探针 `chat_service::tests::audit_untrusted_project_cannot_escalate_read_only_file_access`。两组各自建立全新临时项目和内存数据库，通过正式 SettingsService 将用户设置为 ReadOnly + AlwaysAsk，通过 ProjectService 打开项目，并确认 workspace trust 与 hooks_trusted 均为 false。
- 模型使用本地 Transport fixture，先返回完整原生 write_file 调用，再返回结束文本；实际执行正式 ChatService、Kernel、工具注册和 write_file。审批回调只计数，不批准。目标及固定内容完全位于临时项目；无外部模型请求、凭据或真实用户文件。
- 执行：`cargo test -p deepagent-app-core --lib audit_untrusted_project_cannot_escalate_read_only_file_access --offline -- --ignored --nocapture`。无项目覆盖时：`wrote_file=false, approval_count=0`；加入 `.deepagent/settings.json` 的 `{"permission_preset":"full_access"}` 后：`wrote_file=true, approval_count=0`，并核对文件内容。两组结束后，持久化用户设置仍为 ReadOnly + AlwaysAsk。最终禁止提权断言明确失败，测试耗时约 0.45 秒。
- 最初尝试工作区外目标时，实验组返回 `checkpoint target escapes workspace`，测试的成功返回断言失败，未完成该目标的写入后核查；因此不能据此报告工作区外写入成功。最终探针改为工作区内目标，独立验证用户只读限制是否真正作用于工具。最初的 fixture 编译错误也不是产品缺陷。

#### 后果、成熟实现与修复方向

- 用户在设置中选择的只读和审批策略，没有成为未信任项目配置的权限上限。相同模型调用因项目文件存在而获得写权限，界面保存的用户设置却未变。这与 H-27 共用配置来源信任缺口，但有独立的实际文件工具执行证据；H-27 不需要模型工具调用。
- Codex `config/src/loader/mod.rs::ProjectTrustContext::disabled_reason_for_decision()` 将 project-local config、hooks 和 exec policies 纳入项目授权；`project_layer_entry()` 对未信任来源创建 disabled layer。`config/src/state.rs::layers_low_to_high()` 过滤 disabled layers，未信任配置不会参与有效配置合并。参考机制是先确定来源是否被允许，再解析执行配置，不能直接照搬其全部权限语义。
- 在现有配置加载及有效 profile 解析边界统一来源授权和权限上限：明确用户主动授权、受信任项目偏好、插件默认与管理员约束各自能修改什么。项目、本地配置不应仅凭更高 merge 优先级自动放宽用户权限；复用现有配置层和信任服务，不另建权限表或执行链。
- 验收覆盖只读/工作区写入、审批策略、不同项目配置来源、显式授权及撤销、运行覆盖和管理员约束，并同时断言实际工具副作用与审批调用。当前只运行 Windows 离线 AppCore 探针，没有验证所有宿主、平台或配置来源，也未修改生产权限行为。

### H-29：HTTP 错误正文挂起时取消检查缺失，停止等待退化到正文完成或总超时（已修复）

**状态：已修复。级别：中（P2）；范围：真实 HTTP 模型 transport 的非 2xx 响应读取。** 需要供应商或网关先返回错误状态/响应头，再使正文持续未完成。正常 2xx SSE 的停止保护仍有效。

#### 生产路径与真实回环复现

- 修复前，`ReqwestTransport::stream_inner()` 对请求发送及成功 SSE 的每次 body.next 应用 `await_cancelable_timeout()`，但非成功状态分支直接执行 `response.text().await.unwrap_or_default()`，没有取消检查或正文 idle 超时。外层 stream_cancelled 的 total deadline 仍然存在，默认 300 秒，因此不是无限期无界等待。
- `RuntimeEngine` 直接 await agent.think_streaming_cancelled；ModelAgent 再 await ModelClient 的取消感知流。代码核查未发现这一 await 外另有引擎级取消竞争，因此不能依赖“信号已写入 flag”自动中断任意内部等待。本项动态证据限于实际 HTTP transport，没有运行 Tauri 或宣称已复现某次用户截图中的长等待。
- 回归 `reqwest_transport::tests::audit_cancel_interrupts_stalled_success_and_error_bodies` 使用真实 localhost TCP 服务完整消费请求，分别返回 200/429/503 响应头及正文首字节；保持剩余正文不发。客户端使用正式 ReqwestTransport，idle=2 秒、total=4 秒，确认操作仍 pending 后设置同一取消 flag，观察 500ms，再主动释放服务器正文并回收任务。无外部请求、密钥或真实付费调用。
- 修复：非 2xx 响应改用 `read_provider_error_body()`，按块读取并在每次 `body.next()` 上复用 `await_cancelable_timeout()`；错误正文限制为 64 KiB，读取失败保留 transport 错误，上层取消仍返回 `request cancelled`。成功和错误响应因此共享取消、idle 及外层 total 边界。
- 修复后执行：`cargo test -p deepagent-models --features http audit_cancel_interrupts_stalled_success_and_error_bodies --offline -- --nocapture`。200/429/503 三组均在 500ms 内返回；本次实测分别约 0ms、21ms、17ms，结果均为 `Other("request cancelled")`。测试使用同一 localhost TCP fixture，无外部请求、密钥或真实付费调用。
- 初次编译因测试闭包的 &str 生命周期推断失败；显式标注参数后才执行上述实测。编译问题不列为生产缺陷。

#### 成熟实现、后果与修复方向

- DeepSeek Harness `llm-deepseek/src/adapter.ts::generate()` 合并调用者及 consumer AbortSignal，再经 idleWatchdog 传给 request；fetch 的 signal 覆盖请求和返回 body 的消费，非 2xx response.text 也处于同一取消生命周期。finally 取消 consumer；调用者取消映射为 ABORTED。`tests/runtime.spec.ts` 有 outstanding fetch、mid-stream abort 和 idle watchdog 回归；本轮只核查这些源码，没有声称已运行参考项目测试，也不声称它已有完全相同的非 2xx 半正文用例。
- 本系统收到停止信号后，该分支仍可能继续等待错误正文，UI 的“正在停止”和运行终态随之拖延，且 transport 最终可能上交 HTTP 错误而非取消。它与 H-17 的保存错误 UI 投影、H-25 的 Retry-After 丢失是不同故障点。
- 这消除了停止等待错误正文时的取消缺口；同一轮后续 H-85 已为 `get_json()` 的发现/余额 GET 链路接入 header、body、取消与总时限边界，但其他 HTTP 客户端仍不能因此被视为自动可取消。
- 后续验收仍覆盖 4xx/5xx 半正文、慢速错误正文、正文中断、停止与 total timeout 竞争，以及经过 ModelAgent/Kernel 的终态收尾；本次已完成 transport 层 4xx/5xx 半正文回归，未据此宣称 Tauri 终态全链路已完成。

### H-30：SSE 实际按 data 行解析，合法多行事件失败，未闭合终态帧却被接受

**级别：中（P2）；范围：共用 SSE 分帧与模型响应组装边界。** 触发需要供应商/兼容网关输出多行 data、CR-only 行结束符，或连接在终态事件空行前断开；未声称当前真实 DeepSeek 成功样本采用了这些格式。

#### 标准依据与可重跑证据

- [WHATWG SSE 解析与解释规则](https://html.spec.whatwg.org/multipage/server-sent-events.html#parsing-an-event-stream) 要求同一事件的多个 data 字段以换行拼接，接受 CRLF/LF/CR 行结束符，并由空行派发事件；EOF 时未闭合事件应丢弃。这里区分 SSE 事件与网络字节 chunk，后者任意拆分并不改变前者。
- `deepagent-models/src/sse.rs::SseParser::feed()` 每见 LF 就直接返回该行 data，不维护事件级 data buffer，也不等待空行；flush 把无终止符的剩余 data 当成事件发送。`ReqwestTransport::stream_inner()` 把这些返回值直接交给模型 accumulator，并在 EOF 调用 flush，因此是生产解析边界的问题，非仅注释不准确。
- 新增默认忽略探针 `audit_sse_frames_preserve_multiline_json_and_require_terminal_boundary`，直接组合正式 SseParser 与 ChatCompletionAccumulator，按 transport 相同顺序 feed/flush/finish；终态提前返回规则也相同。执行：`cargo test -p deepagent-models audit_sse_frames_preserve_multiline_json_and_require_terminal_boundary --offline -- --ignored --nocapture`。
- 单行完整事件对照成功；完全等价的多行 JSON 事件报 `EOF while parsing a value`；CR-only 合法事件报 `trailing characters`；正文完整但最后 `data: [DONE]` 没有事件结束空行时，反而成功返回。三组协议断言失败。测试使用本地字符串，没有 HTTP 网络或真实供应商调用；没有据此宣称所有流式请求都失败。

#### 成熟实现、后果与修复方向

- Codex `codex-client/src/sse.rs::sse_stream()` 使用 eventsource_stream::Eventsource，在事件级取得 ev.data 后才交给消费方。DeepSeek Harness `llm-deepseek/src/sse.ts::parseSse()` 使用 TextDecoderStream + EventSourceParserStream，明确不把 unterminated tail 派发成事件，再解析 JSON；错误以结构化模型失败上交。本轮核查参考源码，未执行参考项目的全部解析器测试。
- 兼容网关改变合法 SSE 格式就可能使普通文本或工具调用失败，即使 JSON 语义未变；连接半帧结束又可能被误判成功。这里只证明模型组装结果，未额外运行实际工具执行或桌面终态回放，不能把它扩大为已发生工具副作用。
- 修复应统一字节 UTF-8 解码、SSE 事件分帧、provider JSON 解析的三个边界。可评估现有依赖或标准解析器，保持单一 transport 链路；兼容无空行的非标准供应商若确有需求，需显式定义策略，不能默认把半帧等同于完成。
- 验收应覆盖多 data 行、CR/LF/CRLF、分隔符跨网络 chunk、注释心跳、EOF 半帧和完整终态，并检查真实 HTTP→ModelClient→运行终态。现有默认测试把连续 data 行直接视为两个 payload，恰好固化了行级假设，常规通过不能证明 SSE 协议完整。本轮未修改生产 parser。

### H-31：同步原生压缩的摘要请求没有运行取消信号，停止必须等摘要返回（已修复）

**状态：已修复。级别：高（P1）；范围：ModelAgent 请求前的同步完整摘要压缩。** 需要历史超过压缩阈值并进入模型摘要阶段。与 H-10 的后台 prefire 生命周期及 H-29 的错误正文等待不同：本项使用正常成功摘要，也能拖住运行取消。

#### 生产调用与动态复现

- 修复前：`ModelAgent::think_inner()` 检查 cancel 后 await maybe_proactive_compact，再检查 cancel；压缩调用的 ReactiveContextCompactor 契约没有传递取消上下文。生产 HookedReactiveContextCompactor 调用 ModelCompactor::summarize_accounted，最终使用 `client.stream_response(request)`，而非取消感知入口。因此前后检查不能中断正在等待的摘要请求。
- 审计探针 `context_runtime::tests::audit_cancel_interrupts_native_summary_before_next_model_request` 使用真实内存数据库与 Session 持久化 24 条唯一原生消息；正式 ModelAgent 使用生产 HookedReactiveContextCompactor，设置本地测试阈值 2000，触发完整摘要。无历史工具结果，因此不是便宜的工具结果清理。
- 只有摘要 transport 为可控 fixture：确认收到生产摘要请求后保持响应 pending；fixture 同时提供正确响应取消 flag 的 stream_cancelled 实现并计数。此时设置正式 think_cancelled 的同一个运行 flag，观察 500ms，再主动释放正常结构化摘要并回收任务。没有模型网络请求、真实用户历史或人为修改生产 compactor。
- 修复后：`ReactiveContextCompactor` 增加原生 Surface 的取消感知入口；`HookedReactiveContextCompactor` 将摘要模型请求、压缩 hooks、prefire 和 durable boundary 前后的检查接入同一运行取消标志；`ModelCompactor` 使用 `stream_response_cancelled`。默认回归 `cargo test -p deepagent-app-core --lib context_runtime::tests::audit_cancel_interrupts_native_summary_before_next_model_request --offline -- --nocapture` 输出 `stopped_within_500ms=true, total_requests=1, cancel_aware_requests=1` 并通过，同时确认没有 `ContextCompacted` 边界或后续主模型请求。
- 修复前的复现数据仍保留在本节作为根因证据；本轮另运行了 `cargo test -p deepagent-context -p deepagent-runtime --offline`，54 + 268 个测试通过。真实桌面超大上下文/真实供应商仍未作为本项验收条件。

#### 成熟实现、后果与修复方向

- DeepSeek Harness `compaction-basic/src/summarizer.ts::summarizeWithLlm()` 接收 signal 并放入 GenerateOptions，透传至模型 adapter；aborted finish 转为 LlmError。`region.ts` 在摘要和提交边界检查取消；manual-compaction 回归明确验证取消落在摘要之后时不写 compaction/summary，同时保留已经进入 durability checkpoint 的收尾语义。
- Claude Code `services/compact/compact.ts` 的 fork 路径复用 context.abortController，streaming fallback 传同一 signal，重试 sleep 也接收它。可借鉴的是压缩属于同一取消树，并区分正在请求与已进入持久化收尾的阶段；不是简单无条件 drop 所有压缩 Future。
- 修复前用户在“正在压缩上下文”期间点击停止，取消 flag 虽已设置，执行仍等待摘要响应/超时才退出。修复后摘要请求、prefire 和 hooks 等待均可被取消；取消发生在 durable boundary 提交前不会采用部分摘要，已发生的 summary usage 仍归入运行侧账本。
- 修复复用了现有运行取消树及 Session 事件日志，没有新增存储或第二套取消机制。已覆盖主动摘要中取消、无后续主模型请求和无部分压缩边界；错误正文、SSE 分帧等其他等待边界仍由各自 H 项跟踪。

### H-32：备用宿主打开数据库先执行无所有权恢复，误终结主宿主的工具动作和子 Agent

**级别：高（P1）；范围：两个进程共用同一数据目录且先运行者存在 running 工具动作或子 Agent。** 不要求崩溃、数据库损坏或并发执行相同工具；第二个 CLI/桌面打开数据库即可触发。单宿主正常重启的恢复目标本身合理，问题是恢复执行早于宿主所有权判定。

#### 根因与真实双进程复现

- `crates/deepagent-persistence/src/lib.rs::Database::open()` 在返回数据库前调用 RunControlStore::recover 与 SubagentRunStore::cancel_orphaned。前者无 owner 条件地把所有 running 动作标为 failed 并追加 action.failed，原因为 process_restarted_while_running；后者把所有 running 子运行改为 cancelled、设置 finished_at。两者都没有证明原进程已退出。
- `AppService::open()` 先 Database::open，再 claim_host；from_shared 接收的数据库同样已执行上述维护。虽然 `AppService::recover_unfinished_runs()` 明确保护 Standby 并返回空列表，但此时子表已经改变。原有跨进程测试仅验证 runs 仍为 running，因而没有覆盖这条子表恢复路径。open_in_memory 不执行这些恢复，也容易让单元测试漏掉问题。
- 新增默认忽略探针 `crates/deepagent-app-core/tests/host_lock.rs::audit_standby_open_preserves_live_actions_and_subagents`。真实临时 SQLite 文件、父进程持有生产 OS host.lock，按正式 store API 建立 running run、Received→Prepared→Running 动作和 running 子 Agent。复用原有 subprocess fixture，让真实第二进程 claim、Database::open、AppService::from_shared 并调用 recover_unfinished_runs；没有 mock 所有权、修改生产恢复函数或访问用户数据库。
- 执行 `cargo test -p deepagent-app-core --test host_lock audit_standby_open_preserves_live_actions_and_subagents --offline -- --ignored --nocapture`。对照成立：第二进程确为 standby、recovered:0、父 run 仍 running；但父进程读回 action=Failed、reason=process_restarted_while_running、child=cancelled。父进程随后调用正式成功落盘 API，动作得到 `IllegalTransition { from: "failed", to: "completed" }`，子运行得到 `NotFound("active subagent run not found: audit-live-child")`。保持现状的不变式断言明确失败。
- 这是仍存活且持锁的 owner 的控制记录复现，没有真实启动工具/子 Agent 或调用供应商。已证明状态被误改及正常成功写入被拒绝；没有据此声称实际文件写入被中断、重复执行或全部父运行已失败。探针退出前释放锁，数据只位于自动清理的独立临时目录。

#### 成熟实现、影响与修复方向

- Claude Code `restored-src/src/utils/cronTasksLock.ts::tryAcquireSchedulerLock()` 对另一个存活 PID 返回 false，只有 stale owner 才恢复；释放也核对 sessionId。该代码是调度器锁，不是相同数据库结构，但可直接参考“先证明所有权或失活，再做恢复”的边界。
- Codex `codex-rs/state/src/runtime/memories.rs::try_claim_stage1_job()` 在事务内记录 worker_id、ownership_token、lease_until，冲突更新要求原作业不在运行或租约失效；有效 running 租约不因另一连接出现而被覆盖。这是记忆后台作业示例，不能泛化为 Codex 全部 turn 都采用同一租约；本轮仅核查参考源码，未运行参考项目测试。
- 触发后可能出现父 run 仍显示运行、工具控制记录却已失败、子运行已取消的矛盾。工具正常成功路径在 `tool_pipeline.rs` 调用持久化 Completed 转移并传播错误；SubagentRunStore::finish 也拒绝已终结记录。即使实际工作继续，正式结果也无法按原状态机正常收尾。第二宿主的“备用模式不恢复”日志不能证明数据未被改变。
- 根因修复应把“打开数据库/迁移”和“有所有权的启动恢复”分离，统一收敛到现有宿主生命周期。取得实际主宿主资格后才能恢复其失效记录；单纯把此处函数再搬进 AppService 而不核对记录 owner，也不足以覆盖允许 Standby 正常执行工作的现有设计。若支持多宿主各自运行，需要在现有运行/动作/子运行中表达可验证 owner/租约，按失效 owner 定向恢复；若不支持，则应明确限定运行入口。该选择为本系统本地设计，不新增第二套 RunStore 或控制中心。
- 验收需包括两进程持锁运行、次宿主仅查询、次宿主也运行、主宿主真正退出后接管、同进程重复打开，以及运行/动作/审批/子运行的共同恢复边界。本轮没有修改生产恢复行为，完整崩溃与多宿主压力测试仍未完成。

### H-33：输入租约仅限 ChatService 内存，两宿主可同时推进同一会话

**级别：高（P1）；范围：独立 CLI/桌面宿主共用数据目录，并同时继续同一个 session。** 与 H-32 不同，本项不需要工具或子 Agent；即使父动作表为空，父运行也未被启动恢复终结，两个模型循环仍可并发执行。

#### 生产路径与双进程模型请求复现

- `ChatService::new()` 每实例创建自己的 Arc<InputLeaseRegistry> 和 RunCoordinator；`input_runtime::accept_input_turn()` 调用本实例 registry.acquire/wait_for_turn。`deepagent-runtime/src/input.rs` 的 registry 由进程内 Mutex<HashMap>、队列和 Notify 组成，没有跨宿主会话 owner。RunCoordinator 的取消注册也在本实例内存。host.lock 当前仅限制部分启动恢复，Standby 仍可正常运行 ChatService。
- 现有 session 文件 OS 锁保护单次追加/投影提交，可以保证事件文件不被同时写坏，但不覆盖“恢复上下文→请求模型→执行工具→提交输出”的整个 turn。持久化执行租约也不能仅因存在表和查询就视作聊天入口已接入；本轮调用链没有以 session 为资源取得该租约。
- 新增独立默认忽略集成探针 `crates/deepagent-app-core/tests/host_session_audit.rs::audit_two_hosts_cannot_dispatch_the_same_session_concurrently`。父进程用正式 AppService、SettingsService、ChatService 建立真实文件会话，实际进入 ModelClient transport 后以文件 barrier 保持响应 pending。随后启动同一测试二进制的独立子进程；子进程通过 AppService::open 确认为 Standby，recover_unfinished_runs 返回空，再以自己的 ChatService 继续父进程同一个 session。
- 执行 `cargo test -p deepagent-app-core --test host_session_audit --offline -- --ignored --nocapture`：`child_role=standby, parent_pending=true, child_reached_model=true`，两条 run 在同一观察点均为 running_turn；释放响应后父调用成功、子进程正常退出且子调用成功返回同一 session。不变式断言明确失败。该测试实际执行接纳、输入恢复、内核、请求序列化和文件持久化，仅模型 transport 是离线固定响应；没有公网、真实密钥、工具副作用或用户数据。
- 再以不同的父/子输入与输出标记复测，结果相同；子请求正文包含父输入标记，却没有仍被 barrier 持有的父回答标记。这直接确认第二请求在上一轮尚未完成的历史上构造，而非另一个 session 的请求或上一轮已经结束后的正常续聊。
- 两个响应均在断言前主动释放并回收，子进程有超时 kill/wait 清理。当前证据证明会话派发互斥失效，不证明事件文件损坏、已发生文件覆盖或重复扣费；这些是需要进一步工具/供应商压力验证的后果，不能当作本次已发生事实。

#### 成熟实现、后果与修复方向

- Codex `codex-rs/rollout/src/writer_lock.rs::WriterLockCoordinator::acquire()` 按 thread_id 取得 OS writer lock，竞争 owner 得到 WouldBlock；WriterLockGuard 持有文件句柄，直到相关写工作完成才释放，清理另有协调锁。`writer_lock_tests.rs::writer_locks_reject_competing_owners_and_release_their_files` 明确验证同 thread 冲突、不同 thread 可用、释放后重新取得。该参考体现 thread writer 所有权，不把单次文件追加锁等同于长期 owner；本轮只核查参考源码，未运行其测试。
- 参考的生产接入也已核对：`codex-rs/thread-store/src/local/live_writer.rs::create_thread/resume_thread` 先 acquire_writer_lock，再创建/恢复 recorder 并将 guard 存入 live recorder；`local/mod.rs::acquire_writer_lock` 把 WouldBlock 映射成 ThreadStoreError::Conflict。它不是只有一个未使用的锁工具函数或测试。
- 用户同时从两个入口继续同一会话，两边都可能根据未完成的历史做决策，当前实例的排队/打断策略看不到另一个宿主。输入、回答与工具结果可以交错；若两边操作同一工作区，文件副作用可能竞争。实际工具冲突、跨进程停止与供应商计费本轮未动态复现，需保留验证范围。
- 应与 H-32 共同确定现有宿主运行契约：要么统一向单一 owner 路由同会话执行，要么在既有接纳/租约层增加跨进程会话 owner 与 fencing，覆盖历史恢复、模型、工具、提交及取消路由；进程内 registry 保留为本地队列投影。不能仅在 UI 禁止双击，也不能只加单次 append 锁或把备用宿主日志改成警告。
- 本地设计的验收应覆盖相同 session 两宿主派发/取消、不同 session 并行、owner 崩溃释放与接管、持有者正常完成/租约失效的竞争，并检查单条连续 history 和工具副作用。此处只新增审计探针和报告，未修生产执行链。

### H-34：并行工具绕过统一执行流水线，单工具超时保护在批量调用时失效

**级别：高（P1）；范围：同一步至少两个通过校验和 PreToolUse 的 concurrency-safe 工具。** 需工具执行没有主动响应 ToolExecutionContext，例如仅实现 Tool::invoke 的合法兼容工具；不代表每个内置工具都会挂起。这个问题与模型取消缺口 H-29/H-31 位于不同等待边界。

#### 分叉路径与对照复现

- 串行路径 `ToolPipeline::execute_prepared()` 对 bounded_by_run_timeout=true 的工具使用 tokio::time::timeout，强制执行 RuntimeConfig.tool_timeout；Tool trait 默认 bounded_by_run_timeout=true，默认 invoke_with_context 则直接 await invoke，不强制响应取消/超时。因此外层保护是明确契约的一部分。
- `RuntimeEngine::execute_tools()` 在至少两个可并行项时走自己的 FuturesUnordered 分支，直接 registry.invoke_with_context，没有串行流水线的外层 timeout，也没有围绕 futs.next 的运行取消选择。传 context.with_timeout 只是数据，不会自动取消 Future。单个相同工具仍走正式 pipeline，所以常规单工具超时测试无法覆盖。
- 新增默认忽略探针 `loop_engine::tests::audit_parallel_tools_preserve_serial_deadline_and_cancellation`。正式 RuntimeEngine、真实内存 Session/EventStore、ScriptedAgent 请求固定工具并完成；同一种 Safe、只读、无副作用的 GatedRead 只实现合法 Tool::invoke，等待显式 release。分别请求一个和两个，tool_timeout=80ms，未开启流式推测执行，也未直接调用私有分支替代 agent loop。
- 执行 `cargo test -p deepagent-runtime --lib audit_parallel_tools_preserve_serial_deadline_and_cancellation --offline -- --ignored --nocapture`：单个 started=1、400ms 内完成、持久化一条 exceeded its 80 ms budget 失败结果；两个 started=2、400ms 后仍 pending，随后设置实际运行 cancel flag，再观察 200ms 仍 pending、timeout 结果为零。主动 release 后才退出，最后断言失败。全部 Future 在断言前回收，没有真实工具、模型网络或用户数据。第一次探针编译的借用错误已调整 fixture 取 session id 的时机，不列为产品缺陷。
- 动态证据限于给定观察期和兼容工具，不声称运行了默认 120 秒或无限等待；生产并行分支缺少同等期限是代码证据。当前没有实际复现某个内置网络工具或后台进程泄漏，不能将其写成既成后果。

#### 成熟实现、影响与修复方向

- Codex `codex-rs/core/src/tools/parallel.rs::ToolCallRuntime::handle_tool_call_with_source()` 让串行/并行都经过统一 dispatch，使用 RwLock 控制 overlap；外层 select 等待 dispatch_handle 或 cancellation_token，取消时按 terminal_outcome_reached 判断是否等已完成结果，否则 abort 并 await 子任务、生成配对 aborted response。这里参考的是统一派发与有 owner 的取消收尾，不是宣称 Codex 给每种工具套同一个固定超时。
- DeepSeek Harness `packages/guard/timeout-policy/src/index.ts` 把声明 timeoutMs 的工具 deadline 放在通用 tools/execute wrapper 内，`core/agent-loop/src/tool-calls.ts` 的串/并行调度共用工具运行时。必须同时说明其策略是 cooperative：工具承诺遵守 exec.signal，wrapper 等底层达到 quiescence，不会靠 Promise.race 硬杀不协作代码；不能据此声称其能直接解决本探针的任意不协作 Future。两家参考源码已查，未执行参考测试。
- 影响是同一个工具单独调用会按时返回，模型一次发出两个却可能长期占住整个会话，停止也必须等工具自行返回；同会话后续输入等待其租约。批量请求增多时可能把预期的性能优化变成可用性回退。后续输入阻塞及实际工具资源清理需独立验证，本次仅确认运行等待和超时差异。
- 修复应让并行调度复用既有 ToolPipeline 的 prepare/dispatch/complete 边界，调度器只管理并发与模型顺序；统一 deadline、取消、工具 opt-out、结果装饰、artifact、动作投影与 hooks。取消时停止补充派发、清理本调用拥有的异步/进程资源并记录配对结果；不能把所有工具的 Future 直接丢弃当作外部工作已经停止，也不能在另一个分支再复制一套超时逻辑。
- 验收同一工具的单个/两个/混合串并行，覆盖超时、取消、失败、不同完成顺序和 opt-out 子运行/人工等待；确认结果配对、动作终态和输入租约释放。本轮未修改生产调度行为。

### H-35：并行调度跨越串行写屏障，新增无关读操作会改变已有调用的执行语义

**级别：高（P1）；范围：同一模型响应混合串行工具与至少两个通过检查的并行安全工具。** 与 H-34 的期限缺口不同，本项确认真实内置文件工具的执行顺序和下一请求结果不一致；不是仅凭事件显示次序推断。

#### 根因与正式链路复现

- `RuntimeEngine::execute_tools()` 先遍历整个 batch 收集 `parallel_idx`。只有零或一个并行候选时按模型顺序串行执行；至少两个时，先执行所有并行 I/O，再遍历原始 invocations，把已算出的结果放回原顺序，并执行剩余串行项。事件落盘顺序不能恢复已越过写操作的读取语义。
- 新增默认忽略探针 `audit_unrelated_read_does_not_move_reads_before_write_barrier`，位于 `crates/deepagent-app-core/tests/kernel_v2_e2e.rs`。正式 ChatService、ModelAgent、工具注册和内置 write_file/read_file，在临时工作区运行；只有模型 transport 离线脚本化。捕获真实下一请求的序列化原生 `function_call_output`、实时 ToolStarted 和实际磁盘结果。
- 对照请求 `write_file(source.txt, NEW) → read_file(source.txt)`：实时启动顺序为 write、read；磁盘为 NEW，下一模型请求中读结果包含 NEW，没有错误。
- 加入无关的第三个调用 `read_file(other.txt)`：实时启动顺序变为 read-source、read-other、write；最终磁盘仍为 NEW，但下一模型请求收到 source 的读失败结果，不含 NEW。所有调用均已完成才检查断言，没有遗留工作或真实供应商请求。
- 执行 `cargo test -p deepagent-app-core --test kernel_v2_e2e audit_unrelated_read_does_not_move_reads_before_write_barrier --offline -- --ignored --nocapture`：对照 `read_saw_new=true read_failed=false`，三调用 `read_saw_new=false read_failed=true`；“无关读取不改变写读屏障”断言失败，测试运行约 0.57 秒。
- 初版 fixture 预先建立 source.txt，却没有先读再覆盖，触发了正式内置工具的 read-before-edit 保护；那次失败不是本缺陷证据。现使用新文件创建流程，符合 `WriteFileTool::invoke()` 对新文件无需先读的现有契约，两个 case 均确认实际写入成功。未关闭保护或放宽权限。

#### 成熟实现、影响与修复方向

- DeepSeek Harness `packages/core/agent-loop/src/tool-calls.ts::executeToolCalls()` 明确将 exclusive 调用作为 barrier：首项为 exclusive 时仅运行该项；首项为 parallel 时进入 rolling pool，`runGroup::fillPool()` 遇到后续非 parallel 项停止补充派发，等待当前 inFlight 清空，再交还外层推进下一组。工具结果仍按模型顺序提交。参考源码已核对，未执行参考项目测试。
- 已证实的后果：增加一个无关只读调用，会让原来成功的“创建后读取”向模型返回错误，尽管最终文件创建成功。对“编辑后验证”存在读到旧状态、模型误判失败并重复操作的风险，但这两种后果尚未在本探针复现，不写成已发生的事实。
- 修复应与 H-34 一起收敛到既有统一 ToolPipeline：按原调用序列切分连续并行组及串行屏障，只有组内 I/O 重叠；前一组完全收尾后才能执行屏障，屏障结束后才能派发后一组。复用现有权限、hooks、deadline、取消和配对结果，不能通过对工具名增加临时特判或只重排日志修复。
- 验收覆盖 write→read、read→write→read、多个串行屏障、审批阻塞及取消；加入或删除无关并行工具不能改变跨屏障的先后关系。本轮只增加审计测试和文档，未修改生产行为。

### H-36：完整工具结果在 turn 结束时被删除，持久化历史与 artifact 索引留下失效引用

**级别：高（P1）；范围：超过工具结果预算、被替换成 preview + saved_path 的工具输出。** 原始工作区文件不因此删除；本项删除的是系统自己保存的完整结果副本。对于可重新读取的来源可再次执行工具，但一次性命令输出、远程响应等未必可重建，本探针没有实际执行这些不可重建来源。

#### 生命周期与真实文件数据库复现

- `tool_budget.rs::apply_tool_result_budget()` 将完整 JSON 写盘，向模型返回短 preview、saved_path 和“full output saved”说明。默认 `cleanup_on_run_end=true`；生产 `kernel_runtime.rs::build_kernel_runtime_config()` 只覆盖目录和单工具限额，继续采用该默认值。
- 串行 `ToolExecutionPipeline::complete_prepared_inner()` 将完整结果记录到现有 ArtifactStore，并将 saved_path 加入运行的 created_paths。`RuntimeEngine::run()` 在最终收尾时调用 `cleanup_tool_result_paths()` 删除这些文件，却没有使历史引用失效、移除 artifact 索引或保存可恢复的完整副本。并行分支也加入同一清理列表，但该路径的索引差异尚未由本探针覆盖。
- 新增默认忽略集成探针 `audit_saved_tool_result_survives_turn_and_database_reopen`，使用正式 ChatService、ModelAgent、内置 read_file、默认生产预算及真实临时 SQLite / session 文件。输入 201 行、约 40k 字符的本地测试文本，不触发内置 2000 行限制；仅模型 transport 离线脚本化。
- 在发送下一次模型请求时，从正式序列化 function_call_output 的 `status/result` 包装中取得 saved_path，确认文件此时存在且含 preview 外的尾标记。模型返回完成后，该路径已经不存在。释放服务和数据库再重新打开，artifact 索引仍有一条相同路径；原始 ToolCallCompleted 仍含 truncated=true 和 saved_path，但无尾标记，重新读完整结果得到 NotFound。
- 执行 `cargo test -p deepagent-app-core --test kernel_v2_e2e audit_saved_tool_result_survives_turn_and_database_reopen --offline -- --ignored --nocapture`：`during_tool=true after_turn=false reopened_artifacts=1 replay_has_reference=true reread_ok=false`；要求跨 turn 可读的断言失败，测试约 0.28 秒。没有修改用户文件、调用真实模型或手工删除结果来制造失败。
- 前三次 fixture 检查分别有尾标记重复进入 preview、误把异步桌面事件回调当作同步执行观测点、未解开 `status/result` 包装的问题，均不算生产故障证据。停止微调后核对正式 Session 与 ModelAgent 投影，改在 transport 请求边界检查真实路径；最终失败点才是产品文件生命周期断言。

#### 成熟实现、影响与修复方向

- Claude Code `restored-src/src/utils/toolResultStorage.ts::getToolResultsDir/getToolResultPath/persistToolResult()` 按项目和 session 保存完整结果，用 `writeFile(..., flag:'wx')` 避免对同一引用反复覆盖；preview 内保留完整结果路径。`cleanup.ts::cleanupOldMessageFiles()` 对 session 下 tool-results 文件调用 `unlinkIfOld(..., cutoffDate)`，采用保留期清理，不在单次 turn 结束时直接删除。参考源码已查，未运行参考清理测试；不声称文件永久保留。
- 已确认的后果是会话持久化了一个结束即失效的模型可见引用，artifact 元数据与字节文件不一致。下轮或重启后若根据该路径追问完整结果，会无法读取；本次通过实际文件读取确认 NotFound，尚未另发一轮真实供应商追问。对于原文件仍在的本样本，可重读来源，不能概括为全部原始数据永久丢失。
- 修复应复用现有 ArtifactStore 和 session/run 归属，明确临时中间文件与被历史引用的结果资产的不同生命周期。后者按引用与保留策略清理；清理需同步索引、可见状态和恢复语义，失败需可诊断。目录身份、原子写入和同 call_id 跨运行的边界也需覆盖，不能只在某个调用方改一个 cleanup 开关后宣称完整解决。
- 验收应覆盖 turn 结束、继续对话、重启、取消、并行结果、保存失败和受控保留期回收；同一历史引用在允许保留期内必须指向相同完整内容。本轮未修改生产生命周期。

### H-37：定时任务丢弃审批通知却仍等待人工回答，无人值守运行停在不可达审批上

**级别：高（P1）；范围：Cron 触发的后台会话发生需要人工决策的工具调用。** 用户已有授权或自动策略允许的操作不一定触发；本项不是绕过审批执行命令，而是审批无人接收导致运行无法继续。与 H-15 的“终态后迟到授权”是不同生命周期阶段。

#### 正式定时链路复现

- `cron_service.rs::CronService::run_tick_loop()` 在 fire 中 spawn 正式 `ChatService::run_in_session()`，两个回调均为空，审批回调注释称 auto-deny，实际没有调用任何拒绝接口。
- AppCore 仍组装 `PolicyGate + ChannelApprovalGate`；AlwaysAsk 和 AutoReview 需要人处理的分支会进入 `approval_bridge.rs::ChannelApprovalGate::request()`：注册 oneshot、调用空 notifier，然后 await rx。PendingApprovals 继续持有 sender，空闭包返回不会关闭 channel，也不等于 ApprovalDecision::Deny。生产 ToolPipeline 只和运行取消竞争，没有“没有人工消费者”的结果。
- 新增默认忽略探针 `cron_service::audit_tests::audit_unattended_cron_does_not_leave_an_unreachable_approval`：真实 CronService tick、正式磁盘 scheduled-tasks.json、ChatService、AlwaysAsk、模型脚本产生 bash 调用。命令执行器仅计数并拒绝执行，没有真实命令、网络或用户数据。
- 创建已到期 one-shot，启动实际 tick loop；等待正式 pending 审批出现，再停止 scheduler 并观察 300ms。结果：`cron_task_removed=true scheduler_stopped=true pending=1 run_finished_before_cancel=false command_calls=0`。随后通过正式 request_cancel(run_id) 清理，并等待数据库终态后才断言，未遗留执行中的后台运行。
- 执行 `cargo test -p deepagent-app-core --lib audit_unattended_cron_does_not_leave_an_unreachable_approval --offline -- --ignored --nocapture`：要求无人值守调用不能留下不可达审批的断言失败，测试约 0.55 秒。观察期不能证明数学意义无限等待，但正式代码中该等待只有回答、sender 关闭或运行取消的出口；本路径没有投递回答者。没有声称用户桌面已经发生同一 cron 故障。

#### 成熟实现、影响与修复方向

- Claude Code `restored-src/src/hooks/useScheduledTasks.ts` 将定时提示放进现有 REPL 队列，并追加 scheduled fire 消息；有 agentId 的任务路由到存活 teammate，owner 不存在则删除孤儿任务。可借鉴的是复用真实可交互宿主与队列，而非对后台会话提供一个空 UI 回调。
- Codex `codex-rs/exec/src/lib.rs` 为 headless 入口显式设置 `approval_policy=AskForApproval::Never`，并说明有效 reviewer 为 AutoReview 时另行解析；`core/src/tools/network_approval.rs::allows_network_approval_flow()` 排除 Never。可借鉴的是无人值守入口显式声明不能提问，而不是把无交互误当作完全访问，也不能宣称 Never 代表所有工具都被拒绝。这些参考源码已查，未运行参考测试。
- 已确认的后果：一次性计划已从调度列表移除，执行却仍停在没有通知接收者的审批上，用户看不到本路径的实时事件；关闭 tick loop 本身也不结束已派生运行。周期任务在后续时点可能积累多个等待运行，但本探针仅运行一次，未声称已验证规模和资源上限。
- 修复应在现有 AppCore 接纳/审批边界明确 unattended 模式：要么投递到有真实 owner 和通知路由的既有会话，要么对必须人工批准的动作立即给出结构化拒绝/需要交互的终态。沿用既有权限、审批、RunStore、取消树和事件通知；调度触发结果关联 run_id，让任务管理可发现并停止。禁止为避免等待改成 FullAccess，也不能只加超时掩盖无消费者的问题。
- 验收覆盖 AlwaysAsk、AutoReview 的 Allow/Deny/Ask、已有有效授权、调度停止、显式取消和重复触发；拒绝必须配对工具结果且命令执行计数为零，有交互模式的通知必须到达正确会话。本轮仅增加 cfg(test) 审计探针，未修改生产调度行为。

### H-38：正式工作流工具注册表为空，工具失败被包装成节点完成并令整次运行成功

**级别：高（P1）；范围：通过 ChatService / Tauri start_workflow 执行的 tool 节点。** 两个可独立核对的边界共同造成假成功：生产组装没有注册可调用工具；执行器报告失败后，WorkflowAgent 仍将其当作正常节点输出。不是模型自行决定忽略错误，也没有显式配置容错策略。

#### 生产链路与集成复现

- `run_assembler.rs::run_workflow_inner()` 创建 `Arc::new(ToolRegistry::new())`，之后直接构造 WorkflowToolExecutor 并交给 WorkflowAgent，没有调用普通对话的工具组装。registry 因而连内置 read_file 都没有；请求仍能通过版本化 workflow graph 的 tool 契约校验。
- `tool_runtime.rs::WorkflowToolExecutor::execute()` 将 registry.invoke 的 Err 包成 `Ok(ToolExecutionResult {success:false, output:null, error:Some(...)})`。`WorkflowAgent::execute_tool()` 再将它包成 `Ok(JSON)`；`think()` 仅按 Rust Result 的 Ok/Err 决定 Completed/Failed，未判别内层 success。后续 end 正常执行，AgentKernel 的终态成为 succeeded。
- 新增默认忽略集成探针 `tests/workflow_audit.rs::audit_workflow_unknown_tool_propagates_node_and_run_failure`。实际临时 input.txt 存在，正式 SettingsService、ChatService、输入接纳、图编译、WorkflowAgent、AgentKernel、事件泵和 RunStore；图为 start→read_file tool→end，不调用模型或自造节点完成事件。
- 执行 `cargo test -p deepagent-app-core --test workflow_audit --offline -- --ignored --nocapture`：`api_ok=true tool_status=Completed tool_outputs={error:"invalid input: unknown tool: read_file",output:null,success:false} node_error=None end_completed=true run_terminal=Some("succeeded")`。要求工具节点失败的断言失败，约 0.11 秒。证明正式 AppCore 事件和持久化终态，未启动 Tauri 手动点击，不能将推断的 UI 表现写成实际截图证据。
- 前端 `workflowExecutor.ts` 按事件 status 生成外层 `{success: mapped === "completed", outputs: ...}`，未从内层工具结果纠正状态；因此该投影的外层 success 会与内部 success=false 冲突。这里是消费方代码证据；不是声称生产环境已经执行了真实副作用工具。

#### 成熟实现、影响与修复方向

- DeepSeek Harness `packages/core/agent-loop/src/tool-calls.ts::appendToolResult()` 明确保留 `result.isError` 进入模型工具消息，同时保留 result.error.info 和调用事件关联。其共享 tool runtime 交付结构化执行结果，而非只看调度 Future 是否 resolve。参考的是错误语义在边界间保真；模型 Agent 可以读错误继续修复，不代表每次工具失败都必须终结整个 Agent。
- 本系统既有 `DagScheduler::run()` 也已经有 result.ok → failed 集合 → 跳过依赖节点的正式机制，说明不需另造第二套运行存储来表达失败。参考 DeepSeek 源码已查，未执行参考测试；本项只动态验证当前工作流入口。
- 已确认的影响是配置了有效节点、现有文件与合法参数的工具工作流实际没有读取成功，却得到节点 Completed 和运行 succeeded；依赖节点仍继续。需要真实工具的流程无法按预期执行，自动化消费者按终态判断会得到错误的成功信号。若将来只补 registry 注册，现有错误包装仍会隐藏未知工具、权限和执行错误，不能把二者视为一个初始化特判。
- 修复应复用已有工具组装、授权及 ToolPipeline，并在 WorkflowAgent 边界定义明确的 NodeOutcome。区分“工具执行失败”与“允许作为数据处理的错误”；默认按失败传播，只有显式容错契约才能继续且保留错误状态。同步 Rust 事件、RunStore 终态、前端外层 success 和下游依赖，不能靠前端改红色或把失败 JSON 当成功结果解决。
- 验收覆盖实际只读成功、工具返回 ok=false、未知工具、参数错误、权限拒绝、审批不可用、显式容错及下游跳过；节点状态、结果和运行终态必须一致。本轮仅增加审计测试和文档，未修复生产装配。

### H-39：条件分支只生成标签，未选中的互斥路径仍被实际派发

**级别：高（P1）；范围：正式工作流的 if-else 控制流。** 图编译校验了 source_handle 的有效性，但运行时没有根据已选分支筛选可执行边，拓扑可排序不等于路径可以执行。

#### 生产链路与集成复现

- `workflow/graph.rs::compile()` 保留边的 source_handle，并计算全图拓扑 order 和 ancestors。桌面 `WorkflowNodeShell.tsx::getNodeSourceHandles()` 为单组条件提供 true/false 出口，节点注册表将其标为 enabled/control-flow。
- `WorkflowAgent::execute_if_else()` 返回 `__branch`，`node_output_value_inline()` 将其投影成 branch。`think()` 逐项执行 compiled.order，只有 target_node_id 的局部过滤，没有按条件出口判断节点是否可达；即使已经输出 false，true 专属后继仍运行。
- 新增默认忽略的 `workflow_audit.rs::audit_workflow_unselected_branch_is_not_dispatched`，使用正式 ChatService、图编译、Kernel、节点事件和 RunStore；图为 start→条件→互斥模板 A/B→共同 end。条件刻意使用后端实际读取的 `logicalOperator/conditions/left/right` 结构，先隔离桌面字段映射问题；不调用模型、命令、网络或媒体供应商。
- 执行 `cargo test -p deepagent-app-core --test workflow_audit audit_workflow_unselected_branch_is_not_dispatched --offline -- --ignored --nocapture`：两组约 0.21 秒，true 时未选中的 FALSE_PATH 进入 Running 并产生 Completed 输出；false 时未选中的 TRUE_PATH 同样执行。两次持久化终态均为 succeeded；断言在收集完两组结果后明确失败。已确认条件本身在这两组后端结构中正确输出 true/false，排除了“仅条件比较算错”的解释。

#### 参考机制、影响与修复方向

- DeepSeek Harness `packages/workflow/workflow-ptc/src/runtime.ts::WorkflowExecution` 将工作流 body 编译为 async 脚本，由 `drive()` 的 runInContext 实际执行；child 仅在脚本调用 agent() 时创建。其控制流由程序的实际执行决定，不是提前枚举所有可能分支并执行。它是程序式工作流，不存在与本系统 source_handle 一一对应的图 API，不能称其采用了同款图调度器；这里只参考“只有实际选中的路径才启动工作”的机制。本轮读取参考源码，未运行该参考项目。
- 当前证据直接证明两条互斥模板路径都会产生输出。若路径接入 LLM、HTTP 或媒体作业，按相同派发机制存在多余请求、费用及不该发生的外部动作风险；本探针没有实际发起这些操作，不能把推断写成已发生的扣费或权限绕过。条件节点本身也不能取代权限控制。
- 修复应在既有 WorkflowAgent/编译图中统一“边已激活、边未激活、前驱未决”和节点派发状态；未选中的独占后继应跳过，不能仅隐藏 UI 输出。需要明确合流语义：来自未激活路径的边不应永远挡住公共下游，且不可读取被跳过分支不存在的值。复用原有 Kernel、节点事件和 RunStore，不新增另一套执行中心。
- 验收应覆盖 true/false、多组条件、嵌套分支、共同后继、分支内失败及显式 run-from-here；以真正的调用计数和持久化状态验证未选路径未执行。本轮仅补审计测试与记录，未改生产调度。

### H-40：专业工作流配置在前端序列化被删除或被后端按另一套字段读取

**级别：高（P1）；范围：桌面专业工作流的输入声明、条件、输出映射和模板。** 图结构使用版本化 DTO，但节点 config 是开放 Map；前端配置 schema、发送过滤器和执行器没有同一份字段契约。H-39 证明的是有效后端条件也不控制派发；本项另行证明用户配置根本没有完整到达执行器，两者不能互相代替。

#### 跨语言真实链路复现

- `workflowExecutor.ts::UI_META_KEYS` 把 `inputVariables`、`conditions` 与 status 等界面属性列在一起；专业模式的 `serializeNodes()` 直接过滤它们。`collectInputs()` 虽把 customer 的值发到 inputs，但 start 的声明已经丢失；Rust `graph.rs::output_contract()` 依赖声明识别合法引用，因此后继引用会在 compile 阶段被拒绝。条件变成空 config 后，后端 execute_if_else 默认返回 false。
- 另外两处执行字段不一致：桌面 schema/registry 的 end 使用 `outputVariables[].name`，execute_end 却读取 variable；模板配置为 `templateScript/templateInputVariables`，execute_template_transform 却读取 `template/variables`。这些配置在开放 Map 中可进入后端，执行器静默采用空缺默认值，结果为成功的空输出。
- 新增仅供显式审计使用的 `apps/desktop/scripts/audit-workflow-contract.cjs --emit-fixtures`。沿用现有 check-workflow.test.cjs 的 TypeScript 模块加载方法，实际调用生产 createDefaultNodeData、配置 schema 校验和 runWorkflow，截取真正传给 start_workflow 的 workflow；仅替换 store 和 Tauri transport，不手写/复制 serializeNodes。四组 fixture 均通过现有桌面配置校验。
- Rust 默认忽略测试 `workflow_audit.rs::audit_desktop_workflow_config_survives_backend_execution` 启动本机 Node 获取上述 JSON，再逐组送入正式 ChatService、图编译、WorkflowAgent、Kernel、节点事件和 RunStore。执行 `cargo test -p deepagent-app-core --test workflow_audit audit_desktop_workflow_config_survives_backend_execution --offline -- --ignored --nocapture`，约 0.66 秒，四组完成观察后断言明确失败：

| 有效桌面配置与期望 | 实际序列化 / 正式后端结果 |
|---|---|
| start 声明 customer=BLUE，end 引用该输出 | inputs 中值还在，但 start.config={}；compile 报 `references an undeclared output`，run=failed |
| BLUE 等于 BLUE，应选 true | inspect.config={}；节点实际输出 branch=false，run=succeeded |
| end 映射 report=EXPECTED_REPORT | name 字段保留，但 execute_end 没读到 variable；节点输出 {}，run=succeeded |
| 模板正文 EXPECTED_TEMPLATE | templateScript 保留但未被读取；节点输出 `output:""`，run=succeeded |

- 这是生产前端序列化与真实 Rust 后端之间的集成探针，不是 Tauri WebView 手动截图验收；供应商、网络和命令均未调用。本项没有遍历每一种专业节点，也没有将其他节点字段差异一概列为动态确认。

#### 成熟实现、影响与修复方向

- DeepSeek Harness `workflow-ptc/src/meta.ts::validateMeta()` 对边界数据校验已知字段、类型和嵌套结构，错误逐项命名；同目录 runtime.ts 的 readAgentOptions() 拒绝不认识或暂不支持的选项，而非以空值假装接受。其接口是程序式工作流元数据及 agent 参数，并非本系统的画布节点；可借鉴的是在消费边界拒绝契约漂移，不能声称它实现了这里的专业节点 schema。
- 本系统自己已经有统一的 values 解析、版本化 graph 和声明校验，修复应收敛到这几个既有边界：只过滤真正的 UI 属性；为每种可执行节点明确唯一的版本化 config/input/output 契约，前端编辑/序列化、后端验证/执行及字段迁移共用规则。无效或旧字段必须显式迁移或报错；不能只把三项从过滤器删掉，因为 end/template 的执行字段仍不一致，if-else 内层的桌面 `logic/items/variable/value` 与后端 `logicalOperator/conditions/left/right` 也仍需核对和统一。
- 已确认后果：合法输入引用被拒绝；条件值改变；配置的模板和输出映射静默失效，并且后三组仍报告运行成功。这会使用户难以区分“空结果是业务结果”与“配置根本没被执行”。没有把这些结果夸大为数据泄漏或真实付费损失。
- 验收必须从生产前端 schema 合法配置生成请求，再经正式 Rust 执行对比具体输出；覆盖每种 enabled 专业节点、输入声明与引用、真实 false 条件、输出映射和旧工程迁移。分别测试 UI schema 和 Rust 图结构不能替代跨语言执行测试。本轮仅增加 fixture 桥、审计测试和文档，未修改生产配置过滤或执行器。

### H-41：其他会话完成会清掉当前画布运行监听和停止入口

**级别：高（P1）；范围：画布运行与普通会话等共享 Tauri 全局 session://completed 通知时的前端运行归属。** 这与 H-39/H-40 的后端图执行无关，不需要错误的节点配置即可触发。

#### 生产链路与前端行为复现

- Tauri `SessionCompletedPayload` 已携带 run_id、session_id、status、error；普通 chat 和 start_workflow 都用全局 `completion_emitter.emit("session://completed", completed)` 发布。后端具有区分归属的信息，不是缺少 run 标识。
- `workflowExecutor.ts::runBackendWorkflow()` 对 chat://event 的 envelope.run_id 有过滤，但 session://completed 的监听不读取任何 payload，任意通知都 cleanup、resolve。runWorkflow 的 finally 随后将 `_abortController`、`_currentRunId` 清空；cancelWorkflow 因而失去待停止的 run id，不再发送 cancel_run。
- 新增显式启用的 `audit-workflow-isolation.test.cjs`，复用审计 TS loader，实际调用生产 runWorkflow/isWorkflowRunning/cancelWorkflow，替换 store、Tauri invoke/event bus。先投递 A 的 Running，再投递 B 的节点 Completed，确认已有节点过滤有效；之后投递带完整 B 归属的 session://completed，再检查 A 的监听、终态节点投递及取消请求。
- 命令：PowerShell 先设 `$env:RUN_HARNESS_AUDIT='1'`，执行 `node --test apps/desktop/scripts/audit-workflow-isolation.test.cjs`，再删除该临时环境变量。显式探针约 106ms 失败：其他会话完成后，A 的 Promise 已结束，isWorkflowRunning=false、监听数=0、A 后续节点终态未交付，点击生产 cancelWorkflow 的 cancel_run 请求数=0。断言发生前已清理本地运行；该测试没有实际启动后台模型/媒体任务。
- 复现边界：这里动态证明前端过早结算、丢弃后续事件并不发取消，不声称已经在真实桌面观察到了付费任务一直运行。后台是否仍在执行取决于 A 自身生命周期；B 的完成不会替 A 构成后端取消，潜在后果是 UI 已失去控制而实际作业还未结束。

#### 成熟实现、影响与修复方向

- Codex `tui/src/app/app_server_event_targets.rs` 将 TurnCompleted 等通知映射到所属 thread_id，`app_server_events.rs` 按线程通道 enqueue，未知/无效归属被拒绝。它不是本系统 Tauri 的同名事件协议；参考的是完成事件同样必须具有明确 owner。
- 本系统自己的 `api.ts` 普通聊天监听已经使用 `if (e.payload.run_id !== actualRunId) return`，并按 status 判断 resolve/reject；这证明可以复用现有事件契约，不需要额外全局完成中心。读取了 Codex 源码，未执行参考项目测试。
- 修复应把所有画布事件和终态订阅绑定到既有 run_id / listener owner，并让旧运行的 finally 不能清除新运行的资源；完成失败/取消的投影也读取已有 status/error。这里最后两项是应覆盖的生命周期验收方向，未另行宣称动态确认全部旧运行竞态。
- 验收：A 运行期间，B 完成/失败/取消均不能结束 A、移除 A 监听或阻断停止；只有 A 自己的终态结算；两个运行交错收尾、快速重跑及监听注册前完成也应测试。实际节点、会话终态和取消都走现有链路，不新增执行中心。本轮未修改生产订阅行为。

### H-42：定时调度没有跨进程所有权，同一触发时段可重复派发一次性和循环任务

**级别：高（P1）；范围：多个调度实例共享同一 workspace 的持久化计划文件。** 与 H-33 的同一会话输入租约不同：CronService 每次 fire 新建会话，即使修复同会话租约，也挡不住同一计划生成两个不同运行。

#### 生产链路与双进程复现

- Tauri 启动处无条件构造 CronService 并启动 run_tick_loop；该路径没有查询 AppService 的 Primary/Standby 角色，也没有取得 workspace 调度锁。系统现有 host.lock 作用域是 data root，不能覆盖“两个数据目录打开同一 workspace 的计划文件”。
- `CronScheduler::poll()` 读取完整 JSON，判断 due，先调用 fire，之后才批量 remove/mark_fired；ScheduledTaskStore 的这些操作也是无锁 read→write。两个进程在前一方落盘前均能看到相同 due 任务；CronService 的 fire 随即 spawn 正式 ChatService，执行开始不需要等待计划标记更新。
- 新增默认忽略集成探针 `crates/deepagent-runtime/tests/schedule_audit.rs::audit_two_scheduler_processes_cannot_fire_the_same_occurrence`。父进程真正进入生产 poll 的 fire callback 后，启动同一个测试二进制的独立子进程，共享同一实际 scheduled-tasks.json 和同一确定的 now；子进程也运行生产 poll。父回调的暂停固定一个合法交错，没有替换 read/write 或直接伪造重复回调。
- 执行 `cargo test -p deepagent-runtime --test schedule_audit --offline -- --ignored --nocapture`，约 0.07 秒，one-shot 和 recurring 两组都观察到父、子进程 fire 相同 task id；one-shot 最后已删除，recurring 最后只剩一条且已更新，说明最终文件看起来正确不能反证重复派发。再以同一 now 单进程 poll，两组都不重触发，作为顺序账本有效的对照。收集两组并回收子进程后，唯一派发断言明确失败。
- 这是两个真实调度进程和正式持久化的复现，没有同时开启两个桌面窗口，没有发模型/工具请求；只记录重复 fire，不把潜在的重复付费、文件写入或多次发送通知说成已发生。桌面装配允许多个调度实例的证据来自代码。

#### 成熟实现、影响与修复方向

- Claude Code `utils/cronTasksLock.ts::tryAcquireSchedulerLock()` 使用项目级锁文件、独占 `wx` 创建、owner/session/PID 与存活检查；活 owner 阻止另一会话成为 scheduler，失效时重新竞争独占创建，release 只释放自己拥有的锁。
- `utils/cronScheduler.ts` 只在 isOwner 时处理 file-backed tasks，未取得锁的会话定期尝试接管；session-only 任务是进程私有，不受这把共享锁限制。它还用 inFlight 防止文件 watcher 重载期间再次派发。这里只核对参考源码，未执行 Claude 测试，也不声称该锁解决了任意崩溃下的 exactly-once 外部副作用。
- 触发后果是相同计划可能创建多个新会话/运行；涉及模型、编辑或外部操作时会带来重复费用或动作风险，事件日志中不同 run id 也难以反推出它们来自同一个 occurrence。该触发需要重叠调度，频率未做压力统计；单实例顺序控制通过。
- 修复应在现有调度服务上明确 workspace 所有权、主备接管及 occurrence 身份；复用现有 RunStore 做计划→运行的可审计关联，不另建运行中心。计划锁不能等同于每个外部动作 exactly-once；崩溃时应区分“未派发、已接纳、执行结果未知”，按业务幂等性恢复。单纯提前删除 one-shot 或增加进程内 Mutex 都不覆盖跨进程和崩溃窗口。
- 验收覆盖两个 data root 打开同一项目、one-shot/recurring 并发触发、owner 正常退出及崩溃接管、落盘失败和已接纳运行的恢复；普通会话输入租约修复不能代替此项。本轮仅增加默认忽略审计测试和文档，未修改生产调度。

### H-43：运行启动先于事件订阅就绪，提前终态丢失后前端持续等待

**级别：高（P1）；范围：桌面普通聊天和画布客户端的启动/订阅顺序。** 与 H-01 的 stdio 队列溢出和 H-41 的外部会话终态串话不同：本项只发送当前 run 的事件，且没有队列拥塞。

#### 生产路径及可复现时序

- `workflowExecutor.ts::runBackendWorkflow()` 先 await `start_workflow` ACK，之后才注册 chat://event 和 session://completed。Tauri `start_workflow` 在返回 ACK 前已经 spawn 后台运行，并在运行结束时 emit 一次终态；没有等待前端订阅的握手。快速工作流、立即失败或 IPC 返回延迟时，节点结果及终态可以发生在订阅前。
- `api.ts::runChat()` 虽然 await 了普通事件和审批监听，但只启动完成监听的 Promise，没有 await 注册完成，就调用 start_chat_v2。当前锁定的 Tauri 2.11.2 中，JS listen 返回 plugin:event|listen 的 invoke Promise；Rust event/plugin.rs 的 listen 本身也是 async command。发出注册请求不等于注册已完成，不能把调用顺序当成已建立的订阅屏障。
- 两个包装器都依赖一次性的完成回调结算，没有在此等待路径查询持久化 run 终态或自动补读。普通聊天保留已有的 100ms 回调延迟，也无法恢复从未收到的事件。这里只确定包装器的等待结果，不把它扩大为应用中全部其他全局监听都没有收到通知。
- 新增 `apps/desktop/scripts/audit-start-subscription.test.cjs`，用既有 TS loader 加载实际 api.ts/runChat 和 workflowExecutor.ts/runWorkflow，只替换 Tauri 边界和画布 store。保持完成订阅尚未就绪，启动回调发送所属 run 的节点结果/终态后返回 ACK，再放行订阅。这是固定一个允许的异步交错，没有复制客户端实现。
- PowerShell 设置 `$env:RUN_HARNESS_AUDIT='1'` 后执行 `node --test apps/desktop/scripts/audit-start-subscription.test.cjs`，随后移除该临时环境变量：两个不丢完成信号的断言均明确失败。chat 与 canvas 均 `startedBeforeSubscriptionReady=true`、`earlyTerminalDelivered=false`、`pendingAfterReadiness=true`；canvas 的 nodeCompleted=false。放行后等待 150ms，覆盖 chat 自带的 100ms 结算延迟。测试再发同一所属 run 的晚终态作为清理对照，两者正常结算、监听归零；此对照不是声称 Tauri 会重放已丢事件。
- 探针没有启动真实后端、模型、工具或云视频任务；动态证据是生产前端在受控 IPC 时序下丢完成且继续等待，不是实际桌面发生频率或付费任务损失统计。原始后端历史没有因此被删除。它也不能解释 Codex 宿主的 continuation_history_incomplete/cipher_incompatible 报错，两者属于不同系统。

#### 成熟实现、后果与修复方向

- Codex `app-server-client/src/lib.rs::InProcessAppServerClient::start()` 在返回客户端之前建立 event_rx 并 spawn 持续读取 runtime 事件的 worker；请求通过 command 通道进入同一个活跃 worker，事件可先进入客户端队列，不依赖某次请求返回后才开始监听。它的队列取舍不能照搬，本项参考的是稳定事件消费生命周期及请求/事件并行处理。
- Codex `app-server/src/request_processors/thread_lifecycle.rs::ensure_conversation_listener()` 先将 connection 订阅到 thread，再确保监听任务运行；失败会撤销订阅。`turn_processor.rs::prepare_realtime_conversation_thread()` await 该过程，确认 Attached 后才提交实时会话操作。这里明确引用实时入口而非声称所有 turn/start 原样调用该函数；参考源码已阅读，未运行 Codex 测试。
- 本系统聊天已有两个 await listen 的本地先例。应在现有 run_id 及事件契约上建立完整就绪屏障，画布在启动前可缓存早到的 envelope，再按 ACK 归属筛选，或复用现有接纳/回放能力消除身份窗口；订阅失败不得先启动无法跟踪的工作。再用现有 run_events/状态读取补偿终态漏通知、窗口重载与断连，不能只加固定延时假定监听已经完成。
- 潜在后果：后端已结束，调用方仍等待；画布节点可能缺少结果、UI 保持运行态或阻塞下一操作。普通聊天的其他状态处理是否会部分纠正 UI，要另外实测，不把包装器 Promise 悬挂直接等同所有界面都完全冻结。由于后端实际已经终态，不能把这项笼统描述成后台工作无法停止。
- 验收包括注册延迟、ACK 前完成、快速校验失败、当前 run 失败/取消、订阅失败、窗口重连及持久化补读；必须保证无丢终态且监听可释放。此阶段只增加默认跳过的故障探针和文档，没有修改生产订阅或另建事件中心。

### H-44：启动恢复绕过统一终态边界，运行表已失败但会话重放仍没有终态

**级别：中（P2）；范围：非终态运行的启动恢复、恢复重试与重新打开的会话投影。** 与 H-32 的备用宿主越权恢复不同，本项由确认持有 Primary 角色的单宿主执行，不依赖多进程竞争。

#### 生产路径与真实持久化复现

- 正常 `kernel::persist_terminal()` 复用 RunStore::finish_with_event，在一个 SQL 事务中更新 runs、追加取消审计及规范 run_terminal，重复收尾不会再次落终态。既有失败回滚和重试测试提供了正确路径的对照。
- `AppService::recover_unfinished_runs()` 未使用该边界：它先修复 task，再独立 append_event(run_recovered_after_startup)，最后调用仅更新 runs 的 finish；既不写规范 run_terminal，也不将 EventPayload::RunTerminal 投影到会话文件。session_conversation_from_events 只凭会话 RunTerminal 设置 turn.terminal_kind，不会用 runs.terminal_kind 补齐。
- 新增默认忽略集成探针 `crates/deepagent-app-core/tests/recovery_terminal_audit.rs::audit_startup_recovery_preserves_one_replayable_terminal`：实际临时 SQLite 与生产 Session 文件保存用户输入、Running task 和 running_turn run；通过正式 AppService 执行恢复。对照不注入故障；实验组用 SQLite trigger 仅拒绝该 run 的 finished_at 更新，确认恢复失败且 run 仍未终结后移除 trigger，再调用原恢复入口。没有替换恢复函数、直接伪造恢复事件或使用用户数据库。
- 执行 `cargo test -p deepagent-app-core --test recovery_terminal_audit --offline -- --ignored --nocapture`：探针约 0.28 秒明确失败。正常恢复为 recovery_markers=1、run_kind=failed、run_terminals=0、session_terminals=0、conversation_turn_kind=null；注入后重试为 recovery_markers=2，其他值相同。两组 task 均确已 Failed，后续恢复返回空列表，说明这不是根本没执行恢复。
- 每组都先释放原 Session、Database、AppService 和 host lock，再用 AppService::open 打开相同实际文件后读取上述结果。新宿主仍不补写缺失终态；恢复入口认为运行已经完成，不再处理。这里只模拟崩溃遗留状态及一次 SQL 更新故障并实际重开存储，没有强杀真实 Tauri 或模拟 OS 断电。

#### 对照、影响与修复方向

- Codex `app-server/src/request_processors/thread_lifecycle.rs::set_thread_status_and_interrupt_stale_turns()` 在没有 live active turn 时，将遗留 InProgress turn 投影为 Interrupted；`thread_resume_and_read_interrupt_incomplete_rollout_turn_when_thread_is_idle` 测试要求首次 resume、再次 resume 和 thread/read 均保留同一 turn 的 Interrupted 状态。正常 TurnAborted 的增量投影也按 turn_id 更新终态；见 app-server-protocol 的 thread_history_projection.rs 和 thread_history.rs。这里只阅读源码和测试，未运行参考项目；不声称 Codex 采用本系统相同的 SQL/会话文件事务。
- 已实证的影响是两个恢复视图矛盾：run 查询明确失败，而 session_conversation 给同一 run 的 turn 返回空终态；仅重放规范 run_terminal 的消费者也不会收到该终态。故障重试还产生重复恢复标记。当前 thread/read 同时包含 runs 状态，能据此判定失败，不能声称所有 SDK 都永久挂起；本轮也未验证桌面是否显示错误、是否自动重跑任务，不把这些写成已发生后果。
- 修复应让启动恢复接回现有规范终态写入，并明确 runs/run_events 与文件会话投影之间可重试、按 run 身份去重的补偿规则。两种存储不是同一 SQL 事务，不能简单宣称包一个事务就全部原子；应让已终态但缺会话投影的运行仍能修复，并按真实归属写到对应 turn，避免只追加在会话尾部误标后续 turn。可从现有正常终态边界收敛，不新增恢复执行器或事件存储。
- 验收需要覆盖正常遗留恢复、恢复事件后更新失败、规范终态后会话投影失败、反复重开、同会话多个遗留运行和随后已完成 turn。以上后几项是后续修复必须验证的边界，本探针未全部动态覆盖。本轮仅增加显式运行的失败探针和文档，未修改生产恢复行为。

### H-45：父 Kernel 总时限直接丢弃执行 Future，前台子 Agent 没有取消和持久化收尾

**级别：中（P2）；范围：前台 task 子 Agent 仍在等待时，父运行触发 task_timeout。** 本项不是后台子 Agent 的独立生命周期设计，也不将正常用户取消归为同样失效。

#### 生产机制与父子运行复现

- `AgentKernel::start_prepared()` 用 tokio::time::timeout 包住 engine.run；elapsed 分支只生成预算错误并写父终态，没有先取消现有 CancellationTree 或等待所属前台子工作收尾。timeout 丢弃内层 Future，不等于执行了该 Future 的异步退出路径。
- 正式 TaskTool 的 foreground run 调用 ChatSubagentRunner::run_controlled→run_inner_with_controls→run_tracked。begin_tracking 先写 state=running 和 SubagentStarted；run_tracked 在 await run_active **之后**才写 transcript、SubagentRunStore::finish 和子终态事件。没有为 await 被外层丢弃安排等效的收尾。
- 新增默认忽略测试 `subagent_runner::tests::audit_parent_deadline_finalizes_foreground_child`，复用既有审计 runner fixture、正式 TaskTool/ToolRegistry、父 AgentKernel 和子 ModelAgent；父决策仅为离线脚本，子 transport 在真正进入 ModelClient 后保持 pending 并支持取消。先跑“子模型已启动后主动 cancel”的对照，再跑相同子任务触发父总时限。总时限只在测试配置缩短为 1 秒；生产默认 30 分钟，不声称真实用户每秒触发。
- 执行 `cargo test -p deepagent-app-core --lib audit_parent_deadline_finalizes_foreground_child --offline -- --ignored --nocapture`：首次约 1.31 秒，补充 cleanup 检查后再次执行约 1.25 秒，断言明确失败。主动取消组：parent=cancelled、root_cancelled=true、child=cancelled、finished_at 存在、transcript 存在、子终态事件=1、cleanup 成功；总时限组：parent=budget_exceeded、root_cancelled=false、child=running、finished_at 为空、transcript 不存在、子终态事件=0，正式 cleanup 入口因子记录仍 running 而拒绝。两组先等待 transport 的启动通知，排除只验证“尚未启动就取消”的假阳性。此处没有创建隔离工作树，cleanup 在检查是否有工作树之前就被运行状态拦截，不据此声称真实工作树删除失败。
- 这是生产父子执行及真实临时子运行记录的证据，没有外网、真实子工具命令或隔离工作树；不据此声称 OS 子进程或付费网络任务一定在父终态后继续。动态证明的是执行 Future 已退出后，durable 子记录仍 running 且缺收尾；实际外部工作是否停取决于具体工具的 drop/cancel 行为。

#### 成熟实现、影响与修复方向

- Codex `core/src/tasks/mod.rs` 的 abort_task 路径先取消 task token，等待 done 通知或有限的 graceful timeout，再 abort handle 并显式调用 SessionTask::abort；取消传播、等待和任务专属清理是不同步骤。这里只借鉴退出结构，没有声称参考路径是同名 task_timeout 或相同期限策略。
- Claude Code `tools/AgentTool/AgentTool.tsx` 为前台 agent 使用 finally 清理 foreground 注册、skills 状态、定时器和工作树，并向 SDK 投递 stopped/failed/completed；backgrounded 分支由其独立 finally 负责。TypeScript finally 不能直接替代 Rust 被 drop Future 的异步清理，但说明子运行的资源与终态必须有可完成的生命周期 owner。参考源码已阅读，未运行这两个项目测试。
- 后果是已 budget_exceeded 的父运行仍关联永远 running 的前台子记录，缺少 transcript 和终态通知。status/恢复/清理入口可能受其影响；当前数据库重启会将 running 子记录当 orphan 取消，但那不能补回本轮本应产生的 transcript/子终态，也不能作为运行中收尾的替代。此处未新增真实桌面卡片或重启验收。
- 修复应把父总时限与用户停止统一接回现有取消树及受控收尾，保留预算超限这个终态原因，先给前台子工作有界退出机会，再由 owner 保证丢弃路径仍能终结持久化子记录并发布一次结果。不能在父终态前无期限等待，也不能因为子工作本应独立的后台模式就把全部后台任务一并停止。测试应继续保留正常取消对照，并覆盖超时发生于子模型、工具、审批和工作树准备的边界。本轮只添加测试和文档，未修改生产 deadline 或子任务执行。

### H-46：Windows MCP transport 只终止直接进程，非 libuv 服务的普通后代仍继续运行

**级别：中（P2）；范围：Windows 本地 stdio MCP 的显式 close 或最后一个 transport 被释放，服务曾通过没有额外 Job 管理的进程启动机制创建后代。** 不是所有 MCP 都残留进程；不是要求停止一轮对话时关闭仍由其他会话共享的 MCP。

#### 正反对照与生产路径

- `deepagent-mcp/src/stdio.rs::StdioTransport::spawn` 使用 tokio Command，Windows 只设置 CREATE_NO_WINDOW；没有 Job Object 或统一进程树 owner。`close()` 调用直接 Child 的 start_kill/wait；Drop 中 abort stderr reader 并对同一 Child 调用 start_kill。kill_on_drop 同样针对该 Child，不能证明后代已结束。
- 新增 Windows 专用、默认忽略的 `tests/process_lifecycle_audit.rs::audit_stdio_close_and_drop_terminate_owned_descendants`，使用正式 StdioTransport 和真实本地进程。四组分别为 Node/.NET 根服务 × close/Drop。工作进程均为相同 Node 脚本，只写临时心跳，并自带 8 秒兜底退出；.NET 根服务由 PowerShell 执行 ProcessStartInfo + Process.Start，没有 detached/breakaway 标志、没有外网或模型调用。worker 报告实际 parent PID，正式 RPC 响应中的 server PID 必须与其相等；两进程均确认为已启动后才进行清理。
- 执行 `cargo test -p deepagent-mcp --test process_lifecycle_audit --offline -- --ignored --nocapture`：约 3.39 秒，断言明确失败。Node 两组均 parent_exited=true、descendant_alive=false、父退出后心跳增量=0；.NET 两组均 parent_exited=true、descendant_alive=true，父退出后 250ms 的观察区间内各新增 11 次心跳。这不只是 PID 可能残留：工作进程在父退出后仍实际执行文件写入。探针在失败断言之前按自身刚创建的 PID 清理后代，并等待确认退出；没有留下测试进程。
- 第一版仅 Node 的两组曾全部通过，因此没有直接把源码疑点登记为缺陷。本机 Node 22.22.1 / libuv 1.51.0；与运行库版本相符的 [libuv 1.51.0 Windows 源码](https://github.com/libuv/libuv/blob/v1.51.0/src/win/process.c)在 uv__init_global_job_handle / uv_spawn 中对非 detached 子进程设置 KILL_ON_JOB_CLOSE 并加入自己的 Job。该机制解释此处 Node 组的正对照，但不是本系统对任意 MCP 程序提供的保证。.NET fixture 的启动字段已对照 [Microsoft ProcessStartInfo 文档](https://learn.microsoft.com/en-us/dotnet/api/system.diagnostics.processstartinfo)及 Process.Start 的对应重载。

#### 成熟实现、影响和修复边界

- Codex `rmcp-client/src/stdio_server_launcher.rs::LocalStdioServerLauncher::launch_server` 创建属于该服务的进程归属：Windows 在可用时使用无 breakaway 的 Job、挂起启动后 assign/resume，Unix 使用 NewGroup。LocalProcessTerminator 用 Job 终止或组 SIGTERM 后有界升级；ProcessHandle 的显式 terminate 和 Drop 共用该归属。其 Windows Job 建立失败有日志并回退到直接进程 handle，不能据此声称 Codex 的任何环境都绝对不会残留。
- 对应 Codex `rmcp-client/tests/stdio_message_limits.rs::local_stdio_shutdown_terminates_descendants_after_server_exit` 验证 Windows 根服务已退出后 shutdown 仍终止后代，并检查 breakaway 被拒；`tests/process_group_cleanup.rs::drop_kills_wrapper_process_group` 则验证 Unix wrapper 后代。这里只阅读参考源码，没有运行参考项目测试。Claude Code 的 MCP cleanup 此次阅读到的是按直接 PID 发送 SIGINT/SIGTERM/SIGKILL 的有界升级，不能把它描述成已证明的任意树清理机制。
- 后果是 MCP 连接已经关闭或替换后，服务创建的浏览器、worker 等资源可能继续工作并占用资源；反复重建可积累旧工作进程。本探针动态证明的是 .NET fixture 的普通后代仍运行，不代表任意真实浏览器、远程付费任务或平台都已复现；尚未做插件管理 UI、真实宿主退出和 Linux/macOS 验收。
- 修复应在既有 stdio transport/服务 owner 边界收敛树归属，参考 Codex 区分管道关闭、服务有界退出、归属范围内后代强制清理和 stderr drain。Windows 应处理 spawn→assign 的竞态并明确记录 containment 降级；不能仅在设置按钮调用 taskkill，也不能把共享 MCP 进程树挂到单轮运行取消上。验收覆盖非 Node 根服务、正常关闭、初始化中断、父服务先崩溃、配置替换及 shared snapshot 最后引用释放；Unix 机制需单独动态验证。本轮只新增审计测试和文档，未修改生产进程管理。

### H-47：新增无关 MCP 配置会取消未变服务的初始化，并将宿主取消缓存为该服务失败

**级别：中（P2）；范围：服务 A 正在初始化，用户新增 B 等导致全局配置 fingerprint 改变，但 A 自身配置未变。** 本项不把“已 ready 的 A 被重启”或“每轮聊天重新初始化”当作已复现事实。

#### 根因与宿主级正反对照

- 桌面 `save_mcp_server` 执行 McpService::upsert 后调用 start_preparation；后者以全部服务配置 fingerprint 管理单个全局 generation，变化时对上一代 active_cancel 调用 cancel，而不是仅取消被替换服务的初始化。
- `mcp_preparation.rs::prepare_server_with_cancel` 持有按服务器 name 共享的 slot，在取消分支把 `MCP preparation cancelled` 作为 failures 写入同配置 fingerprint 的 CachedRegistry。新一代针对同一 A 等到该 slot 后，由于 fingerprint 未变且 retry_failed=false，直接返回这个宿主取消错误。发布处的 generation 检查能阻止旧 ready 发布，但不能阻止旧任务写坏仍被新代使用的 slot。
- 默认忽略探针 `mcp_service::preparation::tests::audit_unrelated_config_change_preserves_pending_startup` 使用正式内存数据库、McpService 的 upsert/start_preparation/cached_status/ready_registry 和真实 Node MCP 进程；A 已接收 initialize 后才改变配置，文件 gate 控制响应，不靠 sleep 猜初始化位置。两组只在是否新增 B 上不同，均最终开放 A gate；比较 upsert 规范化之后的前后 A 配置，排除 None→stdio 的 DTO 规范化被误认为业务配置变化。fixture 使用宽松 legacy 握手，隔离已登记的 H-08。
- 执行 `cargo test -p deepagent-app-core --lib audit_unrelated_config_change_preserves_pending_startup --offline -- --ignored --nocapture`：约 0.25 秒，断言明确失败。无 B 对照：A 配置未变、只启动 1 次、status=connected、ready 工具含 mcp__a__echo。新增 B：A 配置同样未变且只启动 1 次，但 status=failed/error=MCP preparation cancelled，ready 只含 B/mcp__b__echo。随后再次 start_preparation，两个场景均因全局配置相同而 no-op，失败组不会因此重新启动 A。
- 2026-10-23 重跑同一探针，结果相同：无关配置 B 加入后 A 的配置未变、进程启动数仍为 1，但 A 变为 `failed`、错误为 `MCP preparation cancelled`，快照仅含 B；普通并发共享启动与同配置 no-op 两项基线通过。
- 断言前主动关闭本地 ready registry；没有公网或真实模型调用。动态证据到宿主状态和实际工具快照为止，未做 Tauri 设置界面操作或下一请求的真实 wire 抓取。正式 attach_mcp_tools 消费该 ready snapshot，所以 A 的能力不会因单纯再发聊天自动回到新请求，这是调用链判断。

#### 参考机制、后果与修复方向

- Codex `codex-mcp/src/connection_manager.rs` 的 reusable_pending_startup 比较该服务的 connection_identity、startup_timeout、protocol mode、取消状态及 catalog 策略，复用未变化且未取消的 AsyncManagedClient；重新建立视图不要求终止它。对应 `connection_manager_tests.rs::reconciliation_reuses_an_unchanged_pending_server_without_waiting` 用 gate 验证 reconciliation 返回后原初始化仍可继续并产出工具；`reconciliation_cancels_a_reused_pending_server_when_disabled` 则验证真正禁用时才取消过时初始化。本轮只阅读参考源码，未执行其测试。
- Claude Code `services/mcp/client.ts::connectToServer` 按 getServerCacheKey(name, config) memoize 初始化 Promise；clearServerCache 接受具体服务及配置，定向删除连接和发现缓存。这是按服务身份复用/失效的参考，不声称其全局配置处理与本系统完全同构。
- 后果是在管理另一台 MCP 时，仍然合法、原本正在加载的 A 被误报为失败，新请求缺失 A 的工具；仅重复聊天或相同配置的宿主准备不会修复。初始化越慢，越容易遇到该窗口；不是模型网络慢，也不是 A 自身握手失败。重启或显式定向重试等路径可能恢复，本探针没有动态覆盖这些恢复入口。
- 修复应把初始化 owner/cancel 和失败缓存归到同一服务连接身份及配置代次；全局 generation 只控制当前视图发布，不能撤销未变服务的共享工作。取消的旧身份不应成为新身份可复用的稳定失败；同时保留真实初始化失败的有界重试策略，避免每轮聊天重试。验收增加 A 初始化中新增 B、修改 B、禁用 A、修改 A、迟到旧任务及共享插件 owner 的对照，确认只失效目标服务且新视图无过期能力。本轮仅添加测试和文档，未修改生产准备调度。

### H-48：stdio MCP 探活耗尽后只停止探针，旧能力仍可发现，后续每次调用重新启动完整恢复预算

**级别：中（P2）；范围：本地 stdio MCP 曾正常发现工具，随后断线且重启一直失败。** 本项不是 H-06 的未知结果自动重放，也不是 H-47 的配置变更取消；只审查恢复耗尽的稳定状态及后续调用。

#### 生产边界与真实进程复现

- `deepagent-mcp/src/liveness.rs::probe_loop` 达到 failure_limit 后记录“parking probe (server unavailable)”并 return，没有向 registry/宿主发布状态；McpRegistry 的工具/说明仍为初始化发现结果。`ReconnectingTransport::send` 的重试计数是每次 send 内部局部循环，没有共享的耗尽状态或服务级恢复 owner；下一次工具调用会重走全部 backoff/factory。
- 新增默认忽略探针 `registry::tests::audit_exhausted_liveness_retires_tools_and_stops_request_reconnects` 使用正式 StdioTransport、ConfigReconnectFactory、McpClient、McpRegistry、LivenessProbe 与真实 Node 进程。第 1 代能 initialize/tools/list，首次 ping 时退出；以后每代都在启动后、initialize 前退出。实际进程每次 append 一条临时启动记录，因此计数不是 mock factory 调用次数。fixture 不要求 initialized 通知，隔离 H-08。
- 测试只把三次退避改成每次 5ms、探活间隔改成 5ms，失败上限仍为 3；使用 probe.is_finished 明确等待真实耗尽，而不是等任意时长后假定停用。执行 `cargo test -p deepagent-mcp --lib audit_exhausted_liveness_retires_tools_and_stops_request_reconnects --offline -- --ignored --nocapture`：约 0.98 秒，断言明确失败。
- 实际 probe_finished=true、starts_after_probe=10（首代 + 3 次探活各自再启动 3 次）；仍有 discoverable_tools=1、adapter_count=1、instruction_entries=1。随后对同一 registry 做两次 invoke，均失败，进程累计启动数分别为 13、16。探针最后执行 close_all；没有公网、模型或持久副作用工具。
- 动态证据覆盖 registry、adapter 投影及连续调用的真实重启，没有冒称已执行完整 AppCore/UI 故障场景。源码中 AppCore cached_status 仍只检查初始化 CachedRegistry.failures，ready_registry 不读取 probe 退出状态，attach_mcp_tools 消费该快照；因此其状态/模型发现可能继续沿用旧能力，这是调用链判断，尚未通过真实桌面截图或 wire 抓取验收。

#### 参考差异、后果与修复方向

- 本系统源码声称借鉴 Grok 的 `[1s, 4s, 16s]` 及停用理念；实际只复制退避数字，缺少耗尽后的能力退役。Grok `xai-grok-shell/src/session/mcp_restart.rs::auto_restart_stdio` 三次失败后发布 Unavailable/RestartFailed，调用 unregister_server_tools；dispatcher 的 ShutdownState 有 per-server in_flight_restart，避免重复恢复任务。`exhaustion_unregisters_server_tools` 验证三次 respawn 后只移除一次，`all_three_attempts_fail_emits_exhausted_telemetry` 验证耗尽状态推送。本轮只读参考实现与测试，未运行其 workspace。
- Grok 的 HTTP 恢复路径明确允许 parked until next tool call，不能将该策略混称为其 stdio 行为，或宣称所有类型都应永久断路。这里需要修复的是本系统宣称的本地服务级耗尽语义与实际行为不一致，且不可用工具继续被发现。
- 后果是模型仍会发现并尝试失效工具，连续调用重复承担启动/握手负担，用户可能反复经历长等待。生产默认一轮完整退避合计 21 秒，另加每次启动和初始化；本探针实际使用短退避，没有实测每轮 21 秒，外层运行/工具 deadline 也可能提前截断。不能把本结果概括为所有普通聊天都等待或所有连接都失效。
- 修复应在现有 MCP 服务生命周期内维护同一连接身份的 ready/recovering/unavailable 状态和恢复预算，探活/工具调用共用恢复 owner；耗尽时在下一请求边界退役该服务新发现的工具及说明，并让既有请求收到结构化失败。明确显式重试、配置变更、宿主重启或受控冷却何时重新开放预算，避免 request 层自行无限重启；保留历史调用配对。应覆盖恢复成功、持续失败、并发调用/探活、旧代迟到、用户禁用和全局快照投影。本轮只新增测试与文档，未修改生产恢复策略。

### H-49：备用模型切换只更新模型和预算，实际请求仍携带主模型的输出上限

**级别：中（P2）；范围：主模型过载后自动切换到输出容量较小的备用模型。** 容量相同的切换对照成功；不声称当前官方 flash/pro 默认组合一定触发，也不涉及跨供应商/账号切换。

#### 生产链路与确定性对照

- `deepagent-app-core/src/model_runtime.rs::select_run_model` 从同一 catalog 选择主模型和 fallback_role，RunAssembler 将两者及 context_policy 传给正式 ModelAgent。`model_agent.rs::think_inner` 的过载恢复分支只修改 request.model/self.model，保留已由 ThinkingDepth 生成的 max_output_tokens；下一轮 pressure check 能查到备用能力，却只通过 `ContextPolicy::for_capability_with_output` 对预算中的 reserved_output 做 min，没有修改 wire 字段。ModelClient 的两种序列化路径也没有按新能力重建该字段。
- 新增默认忽略探针 `audit_fallback_rebuilds_output_limit_from_active_capability`。使用正式 ModelAgent、ModelClient、两种 serializer/accumulator 和 ContextUsage 通道；只有底层 HttpTransport 为离线 CapabilityGate。主模型能力 32,768，Deep 深度生成合法的 32,768 输出上限；首请求固定模拟 529。备用模型上下文均为 128,000，分别声明 32,768（成功对照）和 8,192（故障组）输出上限。fixture 在读到实际序列化 body 后，仅按声明上限决定成功或模拟 400，没有人为改 request 或 agent 状态。
- 执行 `cargo test -p deepagent-runtime --lib audit_fallback_rebuilds_output_limit_from_active_capability --offline -- --ignored --nocapture`：整体断言明确失败；四组都恰好发出一次主请求和一次备用请求。每组的 ContextUsage 已显示 audit-fallback/128,000 窗口，reserved_output 也等于其能力声明：

| 协议 | 备用输出上限 | 快照 reserved_output | 实际 wire 上限 | 结果 |
|---|---:|---:|---:|---|
| Responses | 32,768 | 32,768 | 32,768 | 成功 |
| Responses | 8,192 | 8,192 | 32,768 | fixture 拒绝，运行返回 400 错误 |
| Chat Completions | 32,768 | 32,768 | 32,768 | 成功 |
| Chat Completions | 8,192 | 8,192 | 32,768 | fixture 拒绝，运行返回 400 错误 |

- 这是已声明能力与正式 wire 的确定性不一致；400 来自严格离线 fixture，不是本轮真实 DeepSeek 请求结果。未执行桌面降级、真实供应商小容量模型、reasoning 不兼容或历史迁移测试，不能据此声称所有备用调用失败。

#### 成熟实现对照、后果与修复方向

- Claude Code `services/api/withRetry.ts` 达到 529 切换条件后抛 FallbackTriggeredError；`query.ts` 更新 currentModel、清理失败 attempt 的工具执行状态，再以新的 options.model 重新进入 callModel。`services/api/claude.ts::getMaxOutputTokensForModel` 根据该模型的 default/upperLimit 计算默认值并校验环境覆盖，paramsFromContext 重新组装 max_tokens。这里借鉴的是重新解析有效请求配置；不能概括为 Claude 的任意显式 maxOutputTokensOverride 都会自动被 clamp，本轮也未运行其测试。
- 后果是本可由备用模型接续的短任务，因沿用不兼容输出参数从暂时过载变成请求失败；日志/面板可能显示已按较小输出预留，实际却发送较大上限。若 gateway 自行裁剪，该 gateway 可能掩盖错误，不能据此认为本系统配置已经一致。
- 修复应在既有模型请求边界形成同一份有效模型/能力/输出上限，再供 policy、meter 和 wire 使用；模型切换、截断上调及每步请求都重新验证这份配置。对不支持的显式配置明确报错或按已定义策略收敛，不能只在预算层取 min，也不把特定模型名/8,192 特判散落在 fallback 分支。补足不同窗口、不同输出能力、显式覆盖、截断后再切换和两协议对照。本轮仅新增审计测试与文档，生产行为未修复。

### H-50：移动输入未校验界面快照，已过期和未知引用仍下发设备命令

**级别：高（P1）；范围：移动设备 MobileService → Android 输入。** 动态证据为正式服务和后端配合离线 ADB runner；没有向真实手机发送输入，也未证明 iOS 后端具备相同行为。

#### 触发、生产路径与复现

- `deepagent-mobile-protocol/src/ui.rs::UiSnapshot` 声明下一快照产生后旧引用失效，并应返回 StaleUiNode。`InputRequest.snapshot_id` 实际为 Option；本项只验证显式携带的引用，不把人工坐标输入的 None 擅自定义为必须拒绝。
- `deepagent-mobile-runtime/src/mobile_service.rs::ui_snapshot` 返回 backend 的 UUID 快照，但另行调用 SnapshotStore.register 生成递增 ID 并丢弃该返回值；该 store 因此不认识调用方刚收到的最新 ID。`input` 没有调用 validate，Android `AdbBackend::input` 也不读取 snapshot_id，直接构造并执行 input argv。
- 桌面 `mobile_ui_action` 与 AppMobileService.input 按原值转交这个请求。模型工具 MobileInputTool.invoke 也读取并转交 snapshot_id，但其 descriptor schema 没有声明这个字段，这是源码观察，尚未通过完整模型请求验证。本项不扩展为权限或工具取消结论。
- 新增默认忽略探针 `audit_mobile_service_fences_input_to_the_returned_snapshot`：正式设备发现、DeviceRegistry、MobileService、SnapshotStore、AdbBackend 全部执行；仅 AdbCommandRunner 替换为记录器，使用不可执行的假 ADB 路径。先返回两份不同 XML/快照并断言两者内容和 ID 确实不同，再依次携带最新、已被替代和未知 ID 点击同一坐标，核对返回结果和实际发给 runner 的 argv。
- 执行：`cargo test -p deepagent-mobile-android --lib audit_mobile_service_fences_input_to_the_returned_snapshot --offline -- --ignored --nocapture`。明确失败：`tracked_matches_returned=false latest_validates=false`；current、superseded、unknown 三组全部 accepted=true、StaleUiNode=false，`adb_input_commands=3`。预期仅最新引用能执行，旧/未知引用应在下发命令前拒绝。
- 首次 fixture 缺少生产 uiautomator 输出所带的 XML 声明，parser 返回固定 fallback root，测试在前置“内容不同”断言处停止；补齐正常 XML 声明后才获得上述输入缺陷证据。没有修改生产 parser，也没有把 fixture 错误列为产品缺陷。

#### 成熟实现对照、后果与修复方向

- 在本轮已查参考中没有找到直接等价的 Android 快照实现。可借鉴 Codex `codex-mcp/src/client_tool_catalog.rs::run_with_snapshot` 的版本栅栏：检查预期 revision 与当前定义，再在读保护下完成准备/执行。其 `apps_catalog_broadcast_preserves_running_calls_and_rejects_stale_calls` 测试令旧版本调用的 preparation 闭包不能进入，同时保留已开始调用；不是声称 Codex 用该代码操控手机。
- 本系统的具体移动方案属于**本地设计**：收敛在现有 MobileService/SnapshotStore，统一发布/登记的快照身份，在同一设备操作边界检查显式引用；定义快照捕获、输入、主动失效的先后关系，避免检查通过后另一个操作已改变界面。不要只在 UI 检查，或只加 validate 却保留两套不一致的 ID。None 的人工输入语义需显式定义，与模型基于快照的输入区分。
- 影响是模型根据旧界面定位后，新的界面已出现但旧坐标仍能被执行，可能点到另一个按钮、输入到错误页面，造成任务错误或非预期操作。实际后果取决于设备当时界面；本轮没有执行付款、删除等破坏动作，也没有证明所有旧输入都会点错。
- 应回归最新/旧/未知 ID、不同设备隔离、并发捕获/输入、重连失效、人工 None、工具 schema 到服务的真实传递，以及 iOS/远程 backend 的一致性。已有 SnapshotStore 单元测试只验证 store 自身，无法证明生产输入链调用了它。本轮只增加审计测试与文档，未修复生产行为。
- 后续注册链核对：当前 crates/apps Rust 源码搜索 mobile_tools、MobileInputTool 只找到定义/导出/测试，未找到生产注册入口。因此不把移动模型工具的权限/取消源码疑点作为已复现线上缺陷；本项服务与桌面输入路径仍然成立。

### H-51：项目激活改写共享知识服务，既有运行的记忆读写会被重定向到另一个项目

**级别：高（P1）；范围：项目级 memory_write / memory_recall 与相关知识后台检索。** 用户明确选择 global 的跨项目共享属于预期；本项只针对 project 作用域和本应绑定原运行的检索。

#### 触发、生产路径与复现

- 桌面 AppState 与 ChatService 共享同一 Arc<KnowledgeService>；`set_active_project` / `add_project` 调用 activate_project。`RunAssembler` 也先对该共享服务 activate_project(root)，然后把它 clone 到 MemoryService、KnowledgeServiceBackend、KnowledgeMemoryProvider。这些 wrapper 没有保存原项目根。
- `KnowledgeService::activate_project` 在互斥锁内整体替换唯一 KnowledgeState.project_root/base；锁保证单次方法不并发读坏，却不保证已捕获后端继续使用原项目。因此 A 请求等待模型/工具时切换到 B，或 B 会话开始组装后，A 后续读写会跟随 B 的当前 base。工作目录和 session 即使仍为 A，也不会给这些方法重新传回 A 的根。
- 新增默认忽略探针 `audit_existing_memory_handles_keep_their_project_after_activation`。每组使用真实临时项目 A/B、独立 global 目录，磁盘各写一条同主题的 project/workspace 记忆且 owner 标记不同；在 A 活跃时捕获正式 MemoryService、MemoryRecallTool、MemoryWriteTool、KnowledgeMemoryProvider，并将两个工具注册到正式 ToolRegistry。对照不切项目；实验组随后调用生产 activate_project(B)，再经 schema/权限校验执行原工具，调用原后台检索 provider，并重新打开磁盘核对写入位置。没有 fake memory backend、网络或模型。
- 执行：`cargo test -p deepagent-app-core --lib audit_existing_memory_handles_keep_their_project_after_activation --offline -- --ignored --nocapture`。整体断言明确失败：

| 交错 | 原工具 recall | 原 provider fetch | 原工具 scope=project 的写入位置 |
|---|---|---|---|
| A 捕获后不切换 | 只有 PROJECT_A | 只有 PROJECT_A | A=true、B=false |
| A 捕获后激活 B | 只有 PROJECT_B | 只有 PROJECT_B | A=false、B=true |

- 该结果证明生产后端/正式工具对项目切换的作用域漂移；尚未通过真实双会话模型请求、Tauri 点击或后台自动捕获动态复现。组装/桌面源码表明存在可达交错，不能把本探针描述为完整桌面端到端测试。

#### 成熟实现对照、后果与修复方向

- Claude Code `memdir/paths.ts::getAutoMemPath` 根据项目根解析项目记忆目录，缓存按项目根区分；`utils/attachments.ts::getRelevantMemoryAttachments` 先形成本次操作的 dirs，再把每个目录显式传给 `findRelevantMemories(query, memoryDir, signal, ...)`。后者扫描的是传入目录，异步工作不会逐步重新读取一个桌面共享“当前知识 base”。它通常按会话进程运行，不能据此声称已原样解决本系统多会话桌面问题；借鉴的是显式操作作用域和目录身份。
- 直接后果是 A 运行获得 B 项目的约定/错误经验，降低 DeepSeek 任务准确性；A 的 project 记忆被实际落到 B 目录，污染后续 B 会话，也使 A 重开后看不到自己刚保存的内容。若项目存放不同敏感资料，还可能把 B 的资料带入 A 后续请求；本轮只验证 provider 返回值，未发真实供应商证明实际上传。
- 修复应收敛在现有知识/记忆存储：共享 global vault，项目 vault/index 按规范化项目身份缓存；运行接纳/组装时获得绑定项目的稳定服务视图，并供工具、检索、压缩/自动捕获和收尾一致使用。UI 激活只切换 UI 查询视图，不能改变已发运行的目标。此多会话服务视图属于**本地设计**，不新增独立记忆存储；不能靠全程持有全局锁、禁止切项目或工具执行前临时切回补救。
- 还需完整双会话交错、异项目并行/同项目共享、项目移除与规范化、global 按预期共享、后台工作迟到、重启磁盘恢复及来源引用身份测试。本轮只增加审计测试和报告，未修改生产项目激活或存储行为。

### H-52：知识库锁仅保护最终文件写入，独立宿主的旧索引可把不同笔记静默覆盖

**级别：高（P1）；范围：多个知识服务/宿主共享同一 vault，或写入前磁盘已被其他作者改变。** 与 H-51 的项目切换无关；本项两个服务从始至终使用完全相同的项目/global 根。

#### 触发、生产路径与复现

- `KnowledgeService::save` 只取本服务 Mutex，调用 `KnowledgeBase::write`。后者从本地 entries 决定同标题更新及 existing_ids，再用 Vault.unique_slug 分配文件名；直到 `Vault::write` 才获取 vault 的跨进程文件锁。锁内只执行完整文件替换/read-back，不重新扫描或核对磁盘上的条目身份和版本。
- 因此两个服务都先加载旧索引，即使写入完全串行、从未争锁，也会基于旧 existing_ids 选择同一 slug。`Audit retry: policy` 与 `Audit retry policy` 是不同 normalized title，但 slug 都是 audit-retry-policy；第二次写入不报冲突，原文件被替换，首服务的缓存仍展示第一条。
- 新增默认忽略探针 `audit_independent_vault_writers_preserve_distinct_notes`：使用两个正式 KnowledgeService 和真实临时磁盘目录，均在首写前打开；首服务保存 A 后，第二服务保存 B。对照组在第二次保存前主动 reload，故障组保持生产 save 默认行为。两次 save 都必须成功，再新开第三服务读取磁盘并比对首服务缓存。未用 fake 存储、模型或网络，也未把单进程两实例称为已运行两个桌面进程。
- 执行：`cargo test -p deepagent-app-core --lib audit_independent_vault_writers_preserve_distinct_notes --offline -- --ignored --nocapture`。整体断言明确失败：

| 第二服务写入前 | 两次返回 ID 相同 | 重开磁盘条数 | A 保留 | B 保留 | 首服务缓存与磁盘一致 |
|---|---|---:|---|---|---|
| 显式 reload 对照 | false | 2 | true | true | true |
| 默认不 reload | true | 1 | false | true | false |

- 已存在的 Vault 文件锁/临时文件/原子 rename 真实执行；它们确保文件不会半写，不能证明读→选 ID→写的事务一致性。桌面有手动 kb_reload，但 save 和同项目 activate_project 不会自动刷新；把一次 reload 放到锁前也仍存在其他写入插入的窗口。

#### 成熟实现对照、后果与修复方向

- Claude Code 的记忆使用普通文件工具；`FileWriteTool` 在写前读取当前内容/修改时间，与 readFileState 比较，内容确已变化或没有读过就拒绝覆盖。`FileEditTool` 同样检查，并刻意禁止在检查和同步写入之间 await。其代码还保留 edit 前 fileHistoryTrackEdit 备份。借鉴的是“验证当前版本，冲突不能假成功”；本轮没有运行 Claude 测试，也不把单进程同步区段声称为跨进程原子事务。
- 直接后果是不同笔记被静默丢弃，且保存端/首宿主/重启后的视图互相矛盾。丢失的知识可能不再给 DeepSeek 提供纠错经验，旧宿主还可能继续检索到磁盘已不存在的 A。触发需同一 vault 的独立旧视图及 slug 冲突；单服务串行写入的对照正常，不声称任意写入都会丢失。没有确认外部备份存在，因此也不承诺可恢复。
- 修复应在现有 KnowledgeBase/Vault 边界内让锁覆盖读取当前条目身份、去重/分配 ID、版本核对和最终落盘；操作完成再一致更新或失效派生索引。对真正同一条目的并发修改采用明确版本冲突或合并策略，不能以同文件名认定同笔记；读缓存需要可诊断的刷新机制。此跨宿主事务实现为**本地设计**，复用既有锁/vault，不另建数据库，不只扩大锁等待或追加一次无锁 reload。
- 还应覆盖两个真实宿主、外部编辑、同条目冲突、中文/截断 slug、写失败重试及 consolidation/archive 使用旧快照的交错。后者源码同样在分组后才加锁，但本轮未动态复现，不把其后果当作已确认事实。本轮只新增审计测试和报告，未修改生产写入行为。

### H-53：原生压缩移除相关记忆后，后台检索去重与预算仍保留旧记录

**级别：中（P2）；范围：同一 ModelAgent 内完成压缩后，再次需要已经从 Surface 移除的后台相关记忆。** 本项与宿主 Codex 的 `continuation_history_incomplete/cipher_incompatible` 无关。

- `ModelAgent::maybe_schedule_memory_prefetch` 使用独立 `surfaced_memory_ids` 排除已经展示的 ID，达到 `MAX_SURFACED_MEMORIES_PER_RUN = 12` 后直接停止预取；`collect_finished_memory_prefetch` 也按该集合过滤返回值。`adopt_compaction` / `adopt_compacted_history` 更换模型历史，没有根据保留内容重建该集合。因此旧摘录被摘要合法省略后，其 ID 仍占用去重集合与配额。
- 新增默认忽略探针 `audit_compaction_releases_removed_memories_for_retrieval`：正式 ModelAgent、ModelClient 和请求序列化，底层响应与记忆 provider 使用既有离线 fixture。先分别注入 1 条、12 条记忆，核对实际第二次 wire input 含摘录；通过生产 adoption 边界采纳一个明确省略摘录的原生摘要，再输入不同的新查询，检查实际第四次 wire input。测试没有手改去重集合，也没有触发真实容量阈值或真实摘要模型，不能称作自动压缩端到端测试。
- 执行 `cargo test -p deepagent-runtime --lib audit_compaction_releases_removed_memories_for_retrieval --offline -- --ignored --nocapture`，断言明确失败：

| 压缩前已注入数量 | provider 总调用次数 | 压缩后去重数量 | 后续 wire 恢复旧摘录 |
|---:|---:|---:|---|
| 1 | 2 | 1 | false |
| 12 | 1 | 12 | false |

- 第一组再次检索但旧 ID 被过滤；第二组已耗尽旧配额，后续检索没有启动。直接后果是压缩后同一运行无法通过这个后台通道重新取得相关摘录。系统另有 `KnowledgeReminderProvider::passive_block` 和显式记忆工具，可能补偿；不能据此声称所有记忆从此永久不可访问。重启回放侧的 `collect_surfaced_memory_ids_from_events` 也累计历史注入 ID，但本轮未新增该路径的动态探针，不扩大复现范围。
- Claude Code `借鉴/claudecode/restored-src/src/utils/attachments.ts:2251` 的 `collectSurfacedMemories(messages)` 扫描当前保留的 relevant_memories 附件，注释明确说明：压缩移除旧附件后，去重和累计字节预算自然释放。这里只对照源码，未运行参考项目。
- 修复方向：在既有 Surface 的记忆附件身份上统一 live/replay 的当前可见集合与预算；摘要若保留某条记忆，应明确保留其身份，否则允许重新检索。不能无条件清空全部去重，导致仍在尾部的摘录反复注入；同样不能以全量历史事件替代当前可见状态。具体身份与回放映射属于本地设计，应覆盖保留尾部、重复压缩、snip、在途预取和恢复场景。本轮只新增审计测试与报告，没有修改生产检索行为。

### H-54：有效模型参数被桌面深度覆盖，显式思考强度与输出上限没有进入真实请求

**级别：中（P2）；范围：CLI/app-server 调用共享 ChatService 时指定 `reasoning_effort` / `reasoningEffort`，且与持久化 ThinkingDepth 不同。** 本项不是 H-04 的 resume/steer 参数丢失：这里新运行直接携带参数仍失效。

- CLI 的 `--reasoning-effort` 和 app-server `turn/start.reasoningEffort` 都进入 `HarnessRunOverrides`；`RunConfigOverlay::reasoning_effort_override` 按配置优先级读取，`select_run_model` 将其放进 `ResponseDefaults.reasoning_effort`。但 `RunAssembler` 继续把持久化的 `thinking_depth` 传给 ModelAgent。后者每次 `think_inner` 调用 `with_thinking_depth` 写入显式 reasoning_effort；`ModelClient::apply_defaults` 只填 None，故不会再应用调用方覆盖。
- 新增默认忽略探针 `kernel_v2_e2e::audit_harness_reasoning_override_reaches_provider_request`。四组分别使用正式 SettingsService、临时工作区/内存数据库、ChatService→RunAssembler→Kernel→ModelAgent→ModelClient 请求序列化。仅最底层 HttpTransport 使用离线回复并记录实际 body；没有绕过组装器手工造 request，也未修改生产深度或参数解析。每次运行成功，再检查第一个主请求的 `reasoning.effort`。
- 执行 `cargo test -p deepagent-app-core --test kernel_v2_e2e audit_harness_reasoning_override_reaches_provider_request --offline -- --ignored --nocapture`，最终断言明确失败，四组输出如下：

| 持久化深度 | 本次显式覆盖 | 应发 effort | 实际 wire effort |
|---|---|---|---|
| Medium | 无（对照） | high | high |
| Medium | max | max | high |
| Simple | max | max | none |
| Deep | low | low | max |

- 已确认后果：调用方请求更强思考时可能仍关闭思考；请求较低强度时仍使用最大强度。影响 DeepSeek 的任务推理策略和 headless 接口的可控性；延迟、费用和答案质量可能受影响，但本轮未使用真实供应商测量这些差异。没有启动真实 SDK/stdio 客户端；生产字段传递由源码核对，动态证据覆盖它们共用的 AppCore 执行入口。
- DeepSeek Harness `packages/llm/llm-deepseek/src/serialize.ts:146` 在最终序列化明确使用本次 `options.reasoningEffort`，缺省才回退 connection 默认；部署禁用与 effort 冲突则报错，不静默改成另一档。`tests/serialize.spec.ts:282` 逐一断言 off/low/high/max 的映射。其协议为 Messages，借鉴的是配置解析与优先级，不能直接复制 wire 字段。Codex `core/src/client.rs` 使用本次 effort 构造 reasoning，`core/tests/suite/client.rs::includes_configured_effort_in_request` 对实际请求断言配置值；本轮未执行两个参考项目的测试。
- 修复应在既有模型配置边界把桌面深度作为缺省，先解析显式运行配置、有效模型能力与部署限制，再供 Agent、meter 和 serializer 使用同一结果；合法覆盖实际生效，不合法值在请求前明确拒绝。不能只在 CLI 映射一次，后续重试/备用模型/摘要/子运行又重新套桌面默认。还需验证 run/project/managed 配置优先级、缺省对照、无效 effort、两协议及重试后的保持。本轮只新增测试和报告，未修复生产配置行为。

#### 后续复现：持久化输出上限也被相同默认填充顺序覆盖

- 此补充扩大 H-54 的已证实范围到普通桌面共用的持久化 Responses 参数，不增加重复缺陷编号。`SettingsService::set_responses_settings` 和 `set_responses_developer_json` 正确保存并解析 max_output_tokens，`select_run_model` 正确放入 ResponseDefaults。但 Agent 先对尚未应用这些 defaults 的空请求调用 with_thinking_depth，填入 8,192/16,384/32,768，ModelClient 之后只填 None，用户上限无法再生效。与 H-49 的备用模型切换不同，此处第一轮、没有任何供应商错误就失效。
- 新增默认忽略探针 `kernel_v2_e2e::audit_configured_output_limit_reaches_provider_request`，使用正式 SettingsService/ChatService/Kernel/ModelAgent 与请求序列化，四组独立内存数据库/临时工作区，模型底层为记录型离线 HttpTransport。发送前断言正式 effective_max_output_tokens 等于用户配置；每组运行均完成且只发送一次请求。执行 `cargo test -p deepagent-app-core --test kernel_v2_e2e audit_configured_output_limit_reaches_provider_request --offline -- --ignored --nocapture`，最终配置保持断言明确失败：

| 深度 | 普通设置上限 | 开发者 JSON 上限 | 应发上限 | 实际 wire | 按收到顺序记录的 ContextUsage 输出预留 |
|---|---:|---:|---:|---:|---|
| Medium | 无（对照） | 无 | 16,384 | 16,384 | 16,384 → 384,000 |
| Medium | 512 | 无 | 512 | 16,384 | 16,384 → 512 |
| Medium | 512 | 768 | 768 | 16,384 | 16,384 → 768 |
| Deep | 65,536 | 无 | 65,536 | 32,768 | 32,768 → 65,536 |

- ContextUsage 差异也有明确源码来源：请求中 `request_max_output_override` 保存 profile 上限，`current_context_policy` 优先使用它；回答写入前又将该字段清空，随后的 publish 回到 client defaults，未配置时则使用 capability.max_output_tokens（本 fixture 为 384,000）。下一次实际请求又重新套 profile。不是根据数值不同猜测事件乱序；本次捕获的是正式事件回调顺序，没有验收 Tauri 渲染或磁盘重开。不能宣称模型真的生成了超出 512 的文本，因为离线回答很短；已证明的是用户限制和运行后预算与实际请求不一致。
- 本系统 `ChatService::run_review_streaming` 已有同类正确对照：先调用 with_max_output_tokens，再应用深度，因此显式上限不会被该 helper 填掉。DeepSeek Harness `packages/llm/llm-deepseek/src/serialize.ts` 使用 options.maxTokens 优先于 model/connection 缺省；`llm-pi-ai/src/adapter.ts` 将 options.maxTokens 作为本次流式调用参数，其 modelInfo 明确区分模型能力与部署配置的请求缺省。这里只借鉴优先级及能力/请求上限分离，没有把 Messages 的 max_tokens 字段套到 Responses，也未运行参考项目。
- 修复与 H-54 原方向共用一次有效参数解析，应进一步要求发送前、回答写入后的下一请求预算及实际 wire 使用同一输出上限。覆盖低于/高于深度缺省、开发者 JSON 优先、无配置对照和多轮工具调用；本轮没有修改任何生产参数或计量策略。

### H-55：简单思考模式直接携带 Responses 的 none 进入 Chat 适配器，本地校验拒绝整个运行

**级别：中（P2）；范围：使用公开 `ModelConfig::deepseek_chat` / `WireMode::ChatCompletions` 的 ModelAgent，且选择 ThinkingDepth::Simple。** 当前生产 `select_run_model` 和 `from_catalog` 默认使用 Responses，搜索未发现桌面/CLI 正式组装器选择 ChatCompletions；因此不能据此解释当前默认桌面简单模式失败。本项是已提供适配器的条件性兼容缺陷。

- 2026-10-24 边界复核：`model_runtime.rs::select_run_model()` 明确设置 `WireMode::Responses`；对生产代码排除 tests 后搜索，没有发现 Harness 使用 `ModelConfig::deepseek_chat` 或将 ChatCompletions 设为运行协议的调用点。系统视觉在 `vision_provider_service.rs` 自行构造 `/chat/completions` 请求；无限画布在 `canvas_provider_service.rs`/`canvas_model_gateway.rs` 使用自己的 provider 协议与 serializer。这两条 Chat 链路不经过 Harness 的 `ModelClient`。因此 H-55 只描述公开 Chat adapter 的条件性缺陷，不要求也不应据此改写 Harness 的 Responses wire。Harness 的禁用思考值应按 DeepSeek Responses endpoint 的 `reasoning.effort: "none"` 语义核验。

- `ThinkingConfig::for_depth(Simple)` 生成 `reasoning_effort="none"`，这是本系统 Responses serializer 的关思考表达。`ModelClient::response_to_chat_completion` 未进行语义转换，直接调用 ChatCompletionRequest.with_reasoning_effort("none")；后者只对 off 映射 disabled，对 none 留下原始 effort，接着 validate 拒绝它。错误发生在发送前，供应商没有收到请求。
- 新增默认忽略探针 `model_agent::tests::audit_simple_depth_maps_to_chat_thinking_disabled`：正式 ModelAgent→ModelClient→选定适配器，使用既有 AttemptTransport fixture；分别运行两协议 × Simple/Medium/Deep 六组。transport 对每种协议只返回一条正常结束的离线响应，同时记录请求，不手工调用私有转换函数。
- 执行 `cargo test -p deepagent-runtime --lib audit_simple_depth_maps_to_chat_thinking_disabled --offline -- --ignored --nocapture`，断言明确失败：Responses 三档均完成且各发送 1 次；Chat Medium/Deep 均完成，分别发出 enabled + high/max；Chat Simple 返回 `invalid input: unsupported reasoning effort: none`，transport 次数为 0。测试未调用真实供应商，不能据此宣称供应商拒绝了 none，已证明的是系统自身的转换与校验相互冲突。
- 已确认后果：使用这个公开 adapter 的调用方无法运行简单模式，连最短普通回复也不会发出；中/深档正常不能证明该 route 完整可用。这与 H-05 的历史工具投影分裂不同：此处无历史工具，仅第一轮请求就失败。也不同于 H-54 的配置覆盖：此处没有任何显式覆盖或客户端默认值。
- DeepSeek Harness `packages/llm/llm-pi-ai/tests/adapter.spec.ts` 的 DeepSeek thinkingFormat 用例对 high→off 连续发请求，明确要求 off 产生 `thinking: { type: 'disabled' }` 并不携带 reasoning_effort；其其他 provider 映射可声明 off→none，体现关闭语义按适配器转换，而非把所有协议的字符串视为相同。`llm-deepseek/src/serialize.ts` 的 Messages route 也将统一 effort off 映射为 disabled，但字段布局不同，不可直接复制。这里只对照参考源码/测试，未运行参考项目。
- 修复应让既有 thinking 配置携带明确的启用状态与有效 effort，由 provider adapter 生成自己支持的字段并在同一边界验证；关闭模式、缺省和显式禁用不能都退化为一个 Option::None。覆盖两协议三档、显式 off 与默认高强度并存、运行取消及重试；后一个默认值覆盖疑点本轮未动态验证，不列为独立已确认结论。本轮只新增审计测试与报告，没有切换生产默认协议或修改 serializer。

### H-56：宿主就绪快照合并未检查工具重名，已连接 MCP 可让普通聊天在请求前整体失败

**级别：高（P1）；范围：两个已启用 MCP 的 server/local-tool 组合拼接成同一个模型可见名称。** 不需要工具实际执行，也不要求任意命令或恶意代码。

- `McpRegistry::namespaced_name` 直接拼接 `mcp__{server}__{tool}`。单个 registry 的 register_with_instructions 会检查完整工具名冲突，这是已有保护；但宿主 `prepare_server_with_cancel` 为各服务独立创建 registry，`start_preparation` 通过 `extend_ready` 合并。后者只拒绝重复 server key，不检查两个独立 registry 的工具完整名是否相同，故两者都能进入 Ready。
- 聊天 `tool_runtime::build_run_tool_registry` 无条件调用 `attach_mcp_tools`；后者把所有 adapter 注册进正式 ToolRegistry，第二个同名工具在这里才报 already registered，错误沿组装链传播。虽然 MCP 设为 deferred，延迟的是 schema 展示，不是 ToolRegistry 注册，因此普通不使用工具的对话也失败。
- 新增默认忽略探针 `kernel_v2_e2e::audit_ready_mcp_name_collision_does_not_block_chat`。两组均使用两个真实本地 Node stdio MCP 子进程、正式 upsert/start_preparation 和 ChatService 全执行入口；等待正式 ready_registry 已发布两个 server 后才聊天。唯一差别是第二个 server 名：对照为 b，故障为 a__b；第一个为 a，提供 b__echo，第二个提供 echo。没有注入损坏快照、手工追加同名工具或关闭已有保护。底层模型 transport 仅记录次数并返回离线正常回答。
- 执行 `cargo test -p deepagent-app-core --test kernel_v2_e2e audit_ready_mcp_name_collision_does_not_block_chat --offline -- --ignored --nocapture`，最终断言明确失败：

| 第二 server | 两个 Ready 工具名 | 普通聊天 | 模型请求数 |
|---|---|---|---:|
| b（对照） | mcp__a__b__echo / mcp__b__echo | 成功 | 1 |
| a__b | mcp__a__b__echo / mcp__a__b__echo | already registered | 0 |

- 两组均在断言前调用正式 close_all 关闭本测试连接，测试共约 0.46 秒。没有调用公网、实际工具或真实供应商，也没有验收 Tauri 错误展示。直接后果是这个就绪集合被每轮复用时，普通对话持续在组装阶段失败，破坏“插件失败不阻塞主体运行”的边界。当前 ToolRegistry 拒绝重名，故本轮没有复现错误工具执行或静默跨 server 路由，不能声称发生了这些后果。
- Codex `借鉴/codex/codex-rs/codex-mcp/src/tools.rs::normalize_tools_for_model_with_prefix` 保留 raw server/tool identity，统一生成模型可见名，并由 unique_callable_parts 对最终全名集合检查唯一性、必要时加稳定身份 hash；不是只对 server key 去重。`connection_manager_tests.rs` 有 namespace/tool 清洗冲突的独立测试。本轮只核对源码，没有执行 Codex 测试，也不将其 128 字节上限当成 DeepSeek 的协议限制。
- 修复应在既有就绪快照发布边界统一名称分配与反向路由，确保模型 schema、ToolRegistry、审批/权限来源和 MCP 原始调用身份一致；冲突能力必须在发布前消歧或隔离并报管理状态，不能到普通聊天组装时才使整轮失败。名称规则变化还需兼容已有历史工具调用与权限匹配。应补跨用户/插件快照、不同发布顺序、刷新/恢复以及同名配置替换测试；不通过吞掉 ToolRegistry 错误、选择第一个或禁止所有双下划线临时掩盖。本轮只新增测试和报告，没有修改生产工具命名或快照发布。

### H-57：MCP 说明与技能目录仅按已发送身份去重，正文未进入可回放模型历史，后续无状态请求丢失内容

**级别：高（P1）；范围：MCP 返回 initialize.instructions，且使用默认开启的 MCP instructions delta，同会话继续普通对话。** 无需压缩、工具调用、供应商故障或修改服务配置。

- `context_runtime::build_run_context` 调用 `mcp_instructions_block_for_run`，将说明放进本轮 system manifest；默认 delta 开启时，从全部 prior_events 收集已经 announced 的 server 名称，只渲染尚未出现的服务。新一轮会重新构建 system prompt，上一轮的 system header 不会自动成为原生模型历史。
- `run_assembler` 收尾写入 `EventPayload::McpInstructionsDelta { added, removed }`，事件只保存 server 名，没有说明正文。`collect_announced_mcp_servers_from_events` 据此恢复已告知集合，但本轮 user 附件及历史恢复没有把说明正文补回模型 Surface。因此第二轮同时省掉当前 header 和历史正文；这是内容丢失，不能当作不改变模型可见信息的 token 优化。
- 新增默认忽略探针 `kernel_v2_e2e::audit_mcp_instructions_survive_continuation_on_wire`。真实本地 Node stdio MCP 的 initialize 返回独有说明标记，tools/list 描述不含该标记；正式 McpService.upsert/start_preparation 发布 Ready 后，经正式 ChatService→RunAssembler→Kernel→ModelAgent→ModelClient 连续运行三个普通 turn：新会话、同会话继续、另一新会话。仅最底层 HttpTransport 返回离线普通回答并记录完整序列化请求；回答不复述说明，没有手工追加 announced 事件。
- 执行 `cargo test -p deepagent-app-core --test kernel_v2_e2e audit_mcp_instructions_survive_continuation_on_wire --offline -- --ignored --nocapture`：三轮运行均成功且共发出三次模型请求；最终保留性断言明确失败。重新执行仍得到相同结果，单次执行约 0.52 秒：

| 请求 | 完整 wire body 中含 MCP 说明标记 |
|---|---|
| 首个新会话 | 是 |
| 同会话第二轮 | 否 |
| 另一新会话（对照） | 是 |

- 已确认后果：仍向模型提供 MCP 工具时，后续请求不再含服务声明的使用约定、注意事项或资源操作说明；无状态模型不能从上一次请求的 header 自动找回它们。探针没有实际调用工具、发生账户误操作或产生费用，不能把这些潜在后果写成已发生。使用内存 SQLite 和临时工作区，断言前正式 close_all；未进行磁盘重启、压缩、Tauri 或公网供应商验证。本项不同于 H-53 的压缩后记忆预算未释放，也没有证据关联宿主 Codex 的 continuation/cipher 错误。
- Claude Code `借鉴/claudecode/restored-src/src/utils/mcpInstructionsDelta.ts` 的 delta 同时包含 addedNames 和实际 addedBlocks，从当前 messages 的 attachment 重建 announced；`attachments.ts` 生成 mcp_instructions_delta 附件，`messages.ts` 的对应分支把 addedBlocks 渲染为带 MCP Server Instructions 的模型 user 内容。其设计是持久化正文后减少重复注入，不是只保存名字并删正文。该源码也按 server name 去重，本轮不声称它解决所有同名配置更新问题。
- DeepSeek Harness `借鉴/deepseek-harness/packages/mcp/mcp-client/src/server-context.ts::registerServerContext` 将连接拥有的 instructions 注册为 systemPrompt.section 的 text 回调；说明有连接与提示词服务的明确内容来源。本轮仅核对这些参考源码，没有运行参考项目，也未单凭该注册函数断言其全部压缩/重启行为。
- 修复应在既有原生 Surface/事件边界保留模型实际收到的说明正文，再基于当前保留内容去重；或者在每次请求按就绪快照重新注入。二者的选择及删除、压缩、重连和配置更新语义属于本地映射设计，需以完整 wire、回放与就绪身份一致性验收，不能另建说明历史存储。现有 `mcp_instructions_delta_slashes_per_turn_token_pressure` 只证明 block 变短或变为 None，未检查后续实际请求是否保留内容。本轮仅增加审计测试和报告，尚未修复生产行为。

#### 后续复现：技能目录同样只进入当轮系统提示，续聊后无法从请求发现技能

- `build_skill_catalog_blocks` 通过 session-keyed `SkillCatalogSendState::next_delta` 取得目录并放入 system manifest；next_delta 将所有已渲染 id 加进内存 sent_ids，后续仅生成新增项。目录正文没有变成模型历史附件，下一轮重建 system header 后不含旧目录；`SkillTool` 虽仍注册，schema/固定说明只指向 available-skills 提醒，不携带具体完整目录。该函数中“sent_ids 仅用于诊断、每轮完整渲染”的局部注释与后面的真实差集代码不一致；本结论依据执行结果，不以注释为实现保证。
- 新增默认忽略探针 `kernel_v2_e2e::audit_skill_catalog_survives_continuation_on_wire`：通过正式 SkillManager 注册含独有描述的用户技能，经 SkillsService/ChatService 全运行入口连续发四轮，不调用 skill 工具，离线模型固定回复不复述目录。所有完整请求均含可调用的 skill schema；发送前没有手工删改历史。执行 `cargo test -p deepagent-app-core --test kernel_v2_e2e audit_skill_catalog_survives_continuation_on_wire --offline -- --ignored --nocapture`，四轮均完成，最终保留性断言明确失败：

| 请求 | 完整 wire 中含技能 id 与独有描述 |
|---|---|
| 首轮新会话 | 是 |
| 同会话第二轮 | 否 |
| 调用正式 reset_sent_skills 后继续 | 是 |
| 另一新会话 | 是 |

- 首次显式运行约 0.57 秒，三个正对照将原因收敛到发送去重状态，而不是技能未加载或不可见。后续模型仍有 skill 工具，却没有当前目录中的 id/适用描述，普通请求无法按这份目录选择尚未调用过的技能。已调用技能的历史结果、用户直接点名、猜测 id 后的错误建议仍可能提供其他线索，故不能声称所有技能调用都失效。未测试真实模型选择质量、磁盘重开或压缩；不把该情况扩大为执行权限绕过，也不重复计为新 H 编号。
- Claude Code `借鉴/claudecode/restored-src/src/utils/attachments.ts::getSkillListingAttachments` 同样维护 sentSkillNames，但实际返回带格式化 content 的 skill_listing 附件；`messages.ts` 的 skill_listing 分支把 content 变为模型 user message。其 resume suppression 注释明确以 transcript 已经有 listing 为前提。本系统仅复制 send-once 状态语义，却没有与之匹配的正文保留边界。参考项目只做源码对照，未运行。
- 应将技能目录一并纳入 H-57 的内容保留修复：当前 Surface 的目录正文与已展示身份必须一致，失败接纳、压缩/snip、目录更新及重启都需有明确规则。仅在 UI 安装/重载时 reset 或延长发送间隔，不能修复普通第二轮丢目录；本轮仍只添加测试和文档。

### H-58：附件清理把字符替换当作身份验证，空 ID/点目录可删除整桶，父目录 session 可删除附件 root 外数据

**级别：高（P1，数据完整性）；触发条件：调用附件删除接口时提供异常 id 或 session_id。** 正常 Composer 使用随机 ID，不会自行生成本探针的异常值；本轮未证明普通点击会触发，也未证明远程网页能调用桌面 IPC，不能表述为已发现公网攻击入口。

- `AttachmentService::remove` 对 session_id/id 调用 sanitize_segment 后，直接 `root.join(bucket).join(id)` 并 remove_dir_all。字符替换保留点号，不拒绝空 id、单点或双点。空 id 与单点将目标变为整个 session 目录；session_id 为双点则指向 root 的父目录。Tauri `attachment_remove` 只转发这两个字符串，没有类型化身份或目标边界校验；前端 api.ts 的 attachmentRemove 是该命令消费方。路径可达性由源码核对，未启动 Tauri 发送异常删除。
- 新增默认忽略探针 `attachment_service::tests::audit_attachment_removal_stays_inside_one_attachment`：四组各创建独立 TempDir，内含 attachments/session-a/first、second 两个附件目录及 attachments root 外、同一临时根下的 unrelated 目录，均写测试 canary。调用生产 remove 前对实际目标 canonicalize，断言其位于本次临时根内且不等于临时根；所有被删除内容均由本测试创建，没有触碰用户数据。
- 执行 `cargo test -p deepagent-app-core --lib audit_attachment_removal_stays_inside_one_attachment --offline -- --ignored --nocapture`，约 0.02 秒，最终隔离性断言明确失败：

| 输入 | 返回 | first | 同桶 second | root 外 unrelated |
|---|---|---|---|---|
| session-a / first（正常对照） | Ok(true) | 删除 | 保留 | 保留 |
| session-a / 空 id | Ok(true) | 删除 | 删除 | 保留 |
| session-a / 单点 id | Ok(true) | 删除 | 删除 | 保留 |
| 双点 session / unrelated | Ok(true) | 保留 | 保留 | 删除 |

- 已确认后果：异常清理请求能扩大删除范围，既可能丢失同会话其他附件，也可删除附件存储根外可解析到的目录。权限仍限于宿主进程可访问的路径，探针没有测试 ACL 绕过、符号链接、junction、竞态或系统文件；不依据该结果声称任意多级路径都能传入。删除是真实文件系统结果，不是只比较路径字符串。此处与 H-36 的正常 turn 结束时工具结果被回收不同，是附件删除接口自身的身份/路径验证缺口。
- 本系统 `install_ledger::resolve_relative` 已有可复用边界：拒绝 has_root 或 Component::ParentDir，相关默认测试验证逃逸路径拒绝。这不单独解决空 id 或整桶授权，但说明仓库已有组件级验证模式。Claude Code `借鉴/claudecode/restored-src/src/bridge/inboundAttachments.ts` 将文件 basename 清洗后与不含点号的 UUID 前缀拼成最终单文件名，并由 uploadsDir 使用宿主 session 身份；其 dxt/zip.ts::isPathSafe 在 normalize 前调用 containsPathTraversal 拒绝独立双点组件，再拒绝绝对路径。只借鉴结构化身份与完整组件验证，不声称参考项目有完全相同删除 API，也未运行参考项目。
- 修复应在现有 AttachmentService 建立单一有效身份解析，要求 id/session_bucket 为非空普通单组件，拒绝当前/父目录和 Windows 特殊路径形式；删除前确认精确附件归属，目标不得等于 session 或 root，也不得逃出拥有者目录。对 junction/符号链接及检查后替换需单独定义和验证；不通过简单禁止字符串里所有点号破坏合法文件名，也不再新增另一套附件存储。普通缺失附件可幂等，非法身份应有明确错误和诊断。
- 本项使用 code-audit 的数据流/可达性/真实文件验证；未运行 Semgrep/CodeQL（本机未发现命令，未安装新依赖），不能据此宣称完成全仓库 SAST。只添加 cfg(test) 探针和报告，生产删除行为尚未修复。

#### 补充验证：同一身份解析缺口也允许导入覆盖附件目录外文件

- `AttachmentService::ingest` 同样把外部 id/session_id 经 sanitize_segment 后用于 storage_dir；`persist_original` 对该目录直接 fs::write/copy，随后写 extracted.txt 和 metadata.json。Tauri attachment_ingest 将 AttachmentIngestDto 直接交给服务；DTO 只声明字符串，没有拒绝点目录的反序列化校验。未启动桌面发送异常输入，也未证明远程站点有 IPC 权限。
- 新增默认忽略探针 `audit_attachment_import_stays_inside_one_attachment`。四组独立 TempDir 均先创建附件 root、会话目录与兄弟 unrelated，并放置测试 canary；在调用生产 ingest 前 canonicalize 实际写入目录，确认始终在本 fixture 内且不等于 fixture 根。全程只覆盖测试创建的文本，不引用真实用户数据，不使用符号链接或 junction。
- 执行 `cargo test -p deepagent-app-core --lib audit_attachment_import_stays_inside_one_attachment --offline -- --ignored --nocapture`：明确失败，约 0.04 秒。普通 session-a/first 对照按预期写入自己的附件目录；session-a/单点、session-a/双点、双点/unrelated 均返回成功、覆盖 canary 且写入 metadata.json。三组异常分别落在会话桶、整个附件 root 和 root 外兄弟目录，最后一组 outside_root=true。
- 因而 H-58 的影响还包括覆盖可解析到的现有文件及向错误目录写入附件元数据。该项与前述删除共享同一身份验证根因，合并记录，不新增问题编号。修复必须在创建目录/写文件之前共用有效身份与精确归属解析；只限制 remove 不能关闭此缺口。空 id 在 ingest 中会生成新 ID，未把 remove 的空 id 行为套到 ingest；任意多级路径、系统文件、junction 和竞态均未验证。
- 成熟实现对照沿用本节 Claude inboundAttachments：宿主 session 确定上传目录，经过清洗的不含点号的 prefix 与文件名拼接成最终单文件身份；本仓库 install_ledger 的 ParentDir 检查可提供组件级边界参考。它们与本系统的附件 DTO 结构不同，需在现有 AttachmentService 内按其真实存储契约设计和验证。

### H-59：WASM 调用完成后遗留的 watchdog 中断共享引擎上的后续调用

**级别：中（P2，启用可选功能时影响执行正确性；本机 Windows trap 路径另有宿主退出风险）。** `deepagent-tools` / `deepagent-app-core` 的 `wasm` feature 默认关闭，当前桌面与 CLI manifest 未启用它；不能据此解释默认桌面聊天卡顿或取消。触发需共享同一 WasmSandbox、后续 guest 跨过旧 watchdog 的期限，且尚未被自己的 fuel/timeout 限制结束。

- 生产路径：`deepagent-app-core/src/wasm_tools.rs::register_wasm_tools` 为发现的工具共享一个 WasmSandbox；`SandboxedTool::invoke` 在 blocking 任务中调用正式 `run_json`。`deepagent-tools/src/wasm.rs` 每次创建新 Store，却都设置 `set_epoch_deadline(1)`，再启动独立线程 sleep 后 `engine.increment_epoch()`；成功返回仅 drop JoinHandle，使旧线程继续运行。后续 Store 的一跳期限会被前一个调用的线程满足；并发调用也有相同的共享时钟风险，但本轮只动态验证顺序调用。
- 直接 API 依据为本地 Cargo.lock 锁定的 Wasmtime 27.0.0 源码：`runtime/store.rs::set_epoch_deadline` 的 tick 相对同 Engine 的当前 epoch，默认到期 trap；`engine.rs::increment_epoch` 增加 Engine 内的单一 AtomicU64。已联网核对上游同版本 [Store 源码](https://raw.githubusercontent.com/bytecodealliance/wasmtime/v27.0.0/crates/wasmtime/src/runtime/store.rs) 与 [Engine 源码](https://raw.githubusercontent.com/bytecodealliance/wasmtime/v27.0.0/crates/wasmtime/src/engine.rs)。最新在线 API 文档为 51 开发版，未拿它替代本仓库 27 版本证据。
- 默认忽略探针 `wasm::tests::audit_completed_call_watchdog_does_not_interrupt_next_call`：先正式执行 echo，设置 1 秒 timeout 并确认成功；随后正式执行无限循环 guest，给足 fuel 并设置 6 秒 timeout。对照使用独立 Engine，实验使用同一 Engine。两组都在测试可执行文件的独立子进程执行，父进程设 12 秒外层上限并回收子进程；不向 guest 开放 host import，也不修改生产超时机制。
- 执行 `cargo test -p deepagent-tools --features wasm --lib audit_completed_call_watchdog_does_not_interrupt_next_call --offline -- --ignored --nocapture` 明确失败。补充进程内存活标记后：独立引擎组打印 2 秒与 5.5 秒标记，退出观察耗时 8,952ms；共享组两个标记均未出现，退出观察耗时 3,905ms。两子进程均为 `0xc0000409`，没有正常返回 typed timeout；这是与既有 fuel trap 测试 ignore 说明一致的本机 Windows trap 退出行为。结合短 watchdog、共享 epoch 源码与引擎对照，确认后续调用被提前中断；退出观察耗时包含崩溃处理，存活标记也受线程调度影响，均不冒充 guest 精确 trap 时刻。此前未加标记的第一次探针同样失败（9,037ms 对 3,766ms），没有以默认忽略测试的绿结果宣称修复。
- 后果：正常共享工具可能得到属于旧调用的提前超时；在本机 trap 异常环境还会提前终结承载进程。当前生产注册使用默认 policy，探针显式扩大 fuel 以隔离 epoch 原因，没有证明默认 fuel 下任意 guest 都能触发，亦未验证 Linux/macOS 或真实桌面启用该 feature。
- 成熟实现对照：Claude Code `utils/combinedAbortSignal.ts::createCombinedAbortSignal` 把 timer 放在调用局部 controller 内，并在 cleanup 清除；`utils/hooks/execHttpHook.ts` 成功和失败均 cleanup。Codex `core/src/exec.rs::ExecExpiration::wait_with_outcome` 与执行 select 把 timeout future 绑定本次 child wait。它们不是同构 Wasmtime 实现；借鉴的是完成时回收局部定时工作、不同调用互不取消。WASM 的直接实现依据来自上游 API，不宣称成熟 Agent 已采用本仓库相同后端。
- 修复方向（尚未实施）：在既有 WasmSandbox 内统一 epoch 时钟与每 Store 的真实截止时间，可参考上游 Store deadline callback 按该 Store 的 deadline 决定继续或 trap；共享 Engine 的 tick 不应由任意一个调用独占解释为“自身超时”。回收调用局部工作，不能只在成功路径加取消而遗漏并发 Store、错误返回或取消路径。另需独立验证 Windows trap 能正常返回错误，或使用有界隔离 worker 防止宿主被带走；不能仅增加超时或全局串行化掩盖。

### H-60：命令输出管道的等待超时仅丢弃句柄，读取任务仍保留缓冲并继续收集迟到输出

**级别：中（P2）；范围：SystemExecutor 的命令结束后仍有进程持有 stdout/stderr 写端，超过输出 drain 截止时间。** 正常 EOF 不触发；不将“允许命令正常留下后台服务”本身定为缺陷，也不据此解释 Codex 宿主的续接错误。

#### 生产路径、对照与复现

- `crates/deepagent-builtins/src/bash_tool.rs::run_process_controlled` 为 stdout/stderr 各启动 `tokio::spawn(drain_pipe(...))`；命令结束后分别调用 `join_drain`，生产 grace 为 2 秒。`join_drain` 把 JoinHandle 按值移入 `tokio::time::timeout`，elapsed 时返回 false，没有保留句柄并 abort。当前锁定的 Tokio 1.52.3 `runtime/task/join.rs` 明确说明 drop JoinHandle 会 detach，而非取消任务。
- 调用方读取一次 `take_buffer` 并返回后，原有读取任务仍拥有 pipe 和 `Arc<Mutex<Vec<u8>>>`。`drain_pipe` 持续 `extend_from_slice`，没有读入容量上限；迟到字节进入一个已不属于任何工具结果的缓冲。返回文案称后续输出不再捕获，实际仍在内存读取。模型结果预算在执行器返回之后生效，不能限制这段孤立缓冲。
- 新增 Windows 专用、默认忽略的 `bash_tool::tests::audit_drain_deadline_releases_reader_and_buffer`：通过正式 `prepare_process_command` 创建两个真实、隐藏的 PowerShell 子进程，stdout 使用正式 `drain_pipe` / `join_drain`。正常组写 READY 并退出；实验组写 READY 后保持管道开放，只在收到测试 stdin 后再写 65,536 字节。两组均先确认读取 READY，使用未缩短的生产 2 秒 grace；没有创建后代、联网或访问用户文件。
- 执行 `cargo test -p deepagent-builtins --lib audit_drain_deadline_releases_reader_and_buffer --offline -- --ignored --nocapture`：约 2.98 秒，明确失败。正常 EOF：drained=true、retained=false、late_bytes=0；开放管道：drained=false、retained=true，**在 join_drain 返回并 take_buffer、释放调用方强引用之后**才发送输出指令，仍读入 late_bytes=65536。观测使用 Weak，不会自行维持缓冲存活。失败断言前显式 abort 测试读取任务并确认结束、回收两个子进程；两组 reaped=true，清理后 Weak 强引用数为 0。
- 这是实际进程管道与正式清理 helper 的资源生命周期复现，不是完整 SystemExecutor/ToolPipeline/UI 的端到端内存测试。生产路径可达性由上述调用点以及已有 `a_detached_child_holding_the_pipe_does_not_hang_the_call` / `a_normal_exit_leaves_the_detached_child_running` 对照：它们验证返回有界、后台服务保留，但原有断言没有验证 reader 已释放。没有运行高内存/OOM 压力，也没有动态验证 Linux/macOS。

#### 成熟实现、后果与收敛方向

- Codex `借鉴/codex/codex-rs/core/src/exec.rs::await_output` 通过 `&mut JoinHandle` 等待；drain 超时显式 `handle.abort()`。同文件 full-buffer 分支会 abort 未完成 reader 并等待其结束；不能把这两个 capture policy 的输出保留规则混成一个保证。本项直接借鉴的是“等待截止后处理所属读取任务”，不是要求无条件杀死合法后台服务。
- Claude Code `借鉴/claudecode/restored-src/src/utils/ShellCommand.ts::cleanup` 清理两个 StreamWrapper 并释放 TaskOutput；后台 file mode 有文件大小 watchdog，pipe mode 会 spillToDisk。这里仅核对已有清理/输出所有权设计，未运行参考项目，也不声称与本系统 pipe helper 的策略完全一致。
- 影响：后台服务持续输出时，已完成工具调用之后仍存在没有结果消费者的 reader 与无上限缓冲；重复调用可累积这种资源。持续时间取决于写端什么时候关闭、宿主 runtime 何时退出；有限输出可能只形成暂时占用，大量持续输出可能导致内存压力乃至宿主内存耗尽。动态只证明迟到 64 KiB 的保留与增长，没有把 OOM 或每次命令都泄漏当作实测事实。H-46 是 MCP 关闭后进程树残留；本项是命令工具已返回后的输出读取归属，触发、对象和修复边界不同。
- 修复建议（尚未实施）：在现有 SystemExecutor 中统一两个 reader 的生命周期，drain 截止后 abort 并有界确认 reader 已结束，再读取/释放最终缓冲；取消/外层 Future 被丢弃也必须有 owner 清理。为仍需保留的后台输出使用已有受控文件/任务归属，而非孤立内存 Vec；有效输出设保留预算并保持截断提示，不要求持续后台服务随普通命令返回而退出。验收覆盖正常 EOF、单/双写端开放、迟到输出、取消、外层 drop 和重复执行；同时核实关闭 pipe 对合法后台服务的影响，不能仅让测试缓冲变小或提高等待时限。

### H-61：对抗复核脱离主运行取消，取消后仍可落成功终态，总时限也不清理复核请求

**级别：高（P1）；范围：显式开启 adversarial_verify 或 DEEPAGENT_ADVERSARIAL_VERIFY，且当前运行有创建/修改文件证据、复核重试预算尚未耗尽。** 设置默认关闭。没有证据将该条件缺陷视为所有普通对话变慢的原因；与当前 Codex 宿主的加密历史续接错误无关。

#### 生产路径与确定性对照

- `crates/deepagent-app-core/src/run_assembler.rs` 在上述 opt-in 条件下，为正式 Kernel 装配 `ModelAdversarialVerifier`、`ModelSkepticSpawner` 和默认三个 skeptics。`crates/deepagent-runtime/src/loop_engine.rs` 将原生 CompleteItems 归一为 CompleteMessage，完成事实门禁后直接 `verifier.verify(...).await`；此等待没有运行取消参数或取消 select，Accepted 后也没有再次核对取消，继续转为 TaskState::Completed、发 RunCompleted。
- `verification_panel.rs::run_skeptic_panel` 用 `tokio::spawn` 创建三个任务，逐个 await 普通 JoinHandle，没有取消 owner 或 Drop 清理。`ModelSkepticSpawner::spawn` 调用非取消版 `ModelClient::stream_response`；现有 cancel-aware 入口没有被使用。Kernel 的 Ok(Completed) 分支直接映射 Succeeded，仅 Err 分支按 handle.is_cancelled 改写终态。因此取消树正确触发也不能阻止这条成功提交路径。
- 在 `crates/deepagent-app-core/src/verification_panel/audit_tests.rs` 新增默认忽略的 `audit_cancel_during_model_panel_does_not_complete_successfully`：真实内存 SQLite Session/RunStore、Kernel、ModelAgent、ModelAdversarialVerifier、ModelSkepticSpawner 和正式请求序列化；仅 HttpTransport 离线受控。先持久化 run，用正式 CheckpointManager 捕获临时 answer.txt 写前状态并实际创建该文件，确认 Created，排除“无 checkpoint 时默认认为修改”的测试兼容分支。文件变更由 fixture 在运行前准备，不是完整 AppCore 工具写入验收。
- 正常组和取消组都等三个正式复核请求真正进入 transport，再作观察。正常组等待响应，释放后一个 succeeded 终态，任务 Completed；取消组 `RunHandle::cancel()` 返回 true 且 is_cancelled=true，100ms 观察窗口后仍未退出、active=3、数据库无终态；释放相同 PASS 响应后仍一个 succeeded 终态，任务 Completed。两组 plain_calls=3、cancellable_calls=0。受控 transport 同时实现可取消入口，若实际传入信号可中止；没有注入一个故意无视取消的自制 verifier。
- 执行 `cargo test -p deepagent-app-core --lib audit_cancel_during_model_panel_does_not_complete_successfully --offline -- --ignored --nocapture`，约 0.39 秒明确失败：正常对照成立，取消组 exited_before_release=false、terminal=Succeeded。100ms 是已执行的观察窗口，不将其外推为真实网络的最长等待；等待复核释放才完成的确定性交错与无取消生产路径共同确定问题。
- 第二探针 `audit_kernel_deadline_releases_model_panel_requests` 用同一链路、300ms task_timeout；450ms 观察时 Kernel 已返回并落盘 budget_exceeded，任务 Failed，三个复核 transport 仍 active=3。执行 `cargo test -p deepagent-app-core --lib audit_kernel_deadline_releases_model_panel_requests --offline -- --ignored --nocapture`，约 0.53 秒明确失败。释放后 active=0；两个探针均在缺陷断言前释放并等所有 fixture 请求退出，没有留下测试任务。

#### 成熟实现、影响与修复边界

- Codex `借鉴/codex/codex-rs/core/src/tasks/mod.rs::handle_task_abort` 先 cancel 运行 token，等待 done 或明确的 100ms grace，随后 abort 所属任务并调用任务级 abort 清理；相同文件的 SessionTask::run 接收 cancellation_token。参考的是“协作取消、有界退出和专属清理”共同属于 turn 生命周期，不能仅抄 token 字段而让完成后的复核另开无归属模型任务。
- Grok `借鉴/grok-build/crates/codegen/xai-grok-shell/src/session/acp_session_impl/cancel.rs::cancel_running_task` 在适用的用户 Stop 路径通过 abort_turn_task 中止当前 AgentTask；`goal_classifier.rs::run_verification_stage` 位于 turn 的 handle_prompt 内，源码也明确说明 turn abort 会丢弃该 future。这里仅确认 turn 级取消边界，不据注释承诺其所有 coordinator/后台 skeptic 必然随 Drop 停止，也没有动态运行参考项目。
- 用户后果：明明已经停止，复核仍等待供应商，可能继续产生请求成本；返回 PASS 时还把已取消的工作标记为成功。总时限只终结主运行时，已发复核请求仍消耗资源直到自己结束。动态证据是三个受控请求的 future 活跃与真实 SQLite 终态；没有测公网实际计费、OS 网络 socket 或真实 Tauri 界面。复核本身没有文件写工具，不能据此声称它会继续改用户文件。H-31 针对同步摘要取消，H-45 针对子 Agent 持久化收尾，本项针对复核面板的模型任务归属及取消后的成功提交。
- 修复建议（尚未实施）：复用现有取消树，让复核入口、模型流及每个 skeptic 派生同一运行的子 token；复核任务由能取消并 join 的 owner 管理，父 future 结束时不能仅 detach JoinHandle。运行取消应作为控制结果传播，不能转成普通复核失败或 fail-open PASS；在完成提交边界重新确认取消，并经现有唯一终态入口落盘。总时限、手动取消及异常退出共用有界清理；不新增第二套取消器。验收覆盖正常 PASS、拒绝重试、单请求卡住、取消与 PASS 竞争、task_timeout、调用方 drop，结合实际模型请求计数、session/task/RunStore 状态和资源退出检查。

### H-62：停滞分类等待没有运行取消，停止要等分类结束，但最终取消映射正确

**级别：中（P2）；范围：显式开启 execution_features.stall_detector 或 DEEPAGENT_STALL_DETECTOR，主模型已经给出无工具调用的最终答案、分类预算尚未用完，正在等待分类响应。** 默认不开启。与 H-61 的取消后错误成功不同，本项没有复现终态误记成功。

- 正式装配位于 `crates/deepagent-app-core/src/run_assembler.rs`：给 ModelAgent 设置 `ModelStallClassifier`。`model_agent.rs::think_inner` 在主模型返回最终答案之后同步 `maybe_stall_nudge(step).await`；后者直接 await trait 的 classify，未传递已经可用的运行 cancel。`stall_classifier.rs` 只给 `client.stream_response` 包 20 秒 timeout，没有使用取消版客户端入口。取消在 RuntimeEngine 收到 AgentDecision 后才再检查，因此分类等待没有被主运行取消树打断。
- 复用并扩展正式链路探针 `crates/deepagent-app-core/src/verification_panel/audit_tests.rs`，通过明确 CheckKind 区分面板和停滞检测，避免复制第二套 Kernel 夹具。新默认忽略项 `audit_cancel_during_stall_classifier_exits_before_reply` 先执行正常组，再执行取消组；使用真实 Session/RunStore、ModelAgent、ModelStallClassifier 和序列化请求，只有 transport 受控。等分类请求确实进入 transport 后调用 RunHandle::cancel，验证 true 和 is_cancelled=true；transport 同时实现可取消接口，不是注入无视信号的自制 classifier。
- 执行 `cargo test -p deepagent-app-core --lib audit_cancel_during_stall_classifier_exits_before_reply --offline -- --ignored --nocapture`：约 0.37 秒明确失败。正常组 100ms 窗口内等待一个分类请求，释放 not_stalled_complete 后 succeeded/Task Completed；取消组在相同窗口仍未退出，active=1、无数据库终态，释放后 cancelled/Task Failed。两组 plain_calls=1、cancellable_calls=0，最终各仅一个 run_terminal，active=0，缺陷断言前已收尾。
- 这证明的是“已触发运行取消仍等待分类响应”的具体交错；没有真实等待 20 秒、没有计量网络取消延迟，也没有真实供应商或桌面验收。20 秒上限来自生产 CLASSIFY_TIMEOUT，不将 100ms 观察外推为已测最坏延迟。与 H-61 不同，分类位于主 think future 内，没有这里复现的额外 tokio::spawn 面板；不能据本测试声称 Kernel 超时后分类一定游离或最终误记成功。
- 成熟参考：Grok `借鉴/grok-build/crates/codegen/xai-grok-shell/src/session/acp_session_impl/laziness.rs` 对 sampler future 同时 select 超时和 generation 轮询，用户新输入/模型变化使分类作废，注入前又核对当前 generation 和 idle/notifications_suppressed 状态。它是 idle classifier，不能把其用户新输入机制等同于本系统 turn token；可借鉴“等待及采纳均验证所属生命周期”。Codex `core/src/tasks/mod.rs::handle_task_abort` 的 turn token、有界 grace 和 task abort 给出运行级后备边界；Claude `query.ts` 给附属摘要传同一 abortController.signal，并在后续推进前检查 aborted。这里均为源码比对，未运行参考项目。
- 后果与修复建议（未实施）：用户已看到主答案、点击停止仍可能等待额外分类请求完成或生产 timeout，延长输入租约占用与下一轮推进；不据此宣称所有对话都会慢。复用当前取消树和 ModelClient 取消版入口，分类返回取消应中止本次推进，不能把取消吞成 fail-open 无分类再继续补发模型。分类返回后、nudge 注入和递归采样前核对运行有效性。验收覆盖分类头等待/流式中/解析后取消、超时和 nudge 重入，检查无延迟新请求且最终状态仍是同一 Cancelled。

### H-63：对抗复核和停滞分类丢弃已返回的 usage，现有会话成本账本漏记附属请求

**级别：中（P2）；范围：开启上述可选复核/分类，附属模型请求成功并返回 usage。** 默认关闭开关不能代替开启后的成本正确性；不把这两条链路外推为所有后台模型请求都漏计。

- `verification_panel.rs::ModelSkepticSpawner::spawn` 得到 Response 后只返回 output_text_projection；`stall_classifier.rs::ModelStallClassifier::classify` 只解析 output_text_projection，均未转交 response.usage。对应 traits 仅提供文本/判定，不能把 usage 带回 RuntimeEngine/ModelAgent。ModelClient 的非取消 stream_response 只组装 Response，本身没有数据库/会话费用记录。`run_assembler.rs` 向正式 AppRunFinalizer 传递的 usage 和 side_usage 均来自主 agent，未取得上述附属响应。
- 扩展共享离线 transport，让主模型的真实 response.completed 带 usage=input_tokens 11、output_tokens 3、total_tokens 14；每个检查请求真实终帧带 101/7/108。通过生产 ModelClient/累积器，而非直接给 finalizer 填想要的统计数字。保留主回答、正常判定和生命周期对照；在 Kernel 返回后按生产组装的同样来源传 `agent.cumulative_usage()` / `agent.side_usage()`，调用真实 AppRunFinalizer、CostService 和同一真实 SQLite costs 表，读取 CostSummary。
- 新默认忽略项 `verification_panel::audit_tests::audit_completion_check_usage_reaches_cost_ledger` 分别跑三个 skeptics 和一个 stall classifier。两组正式检查均正常完成、唯一 succeeded 终态且无残留请求；main 为 11/3、side=None。实际账本分别是 input/output=(11,3)、(11,3)，应有的 provider 返回量为 (314,24)、(112,10)。执行 `cargo test -p deepagent-app-core --lib audit_completion_check_usage_reaches_cost_ledger --offline -- --ignored --nocapture`：约 0.50 秒明确失败，差异完全对应 3×101/7 与 1×101/7。没有创建另一个成本存储。
- 边界：这是正式模型、Kernel 与 Finalizer 的集成探针，没有运行完整 ChatService/桌面，也未实际调用公网、设置真实价格或推算漏计人民币；CostService 未配置价格时仍保存 token，价格缺失不是本测试漏 token 的来源。fixture 的 usage 用于可计算对照，不代表模型真实 tokenizer 数量。H-14 是未采用的摘要费用丢失；本项是成功完成的复核/分类正常返回后没有费用归属。
- 正向对照：主调用的 11/3 正确进入现有账本；既有 `run_finalizer::tests::compaction_side_usage_is_booked_as_its_own_row` 使用同一 Finalizer 验证主调用和摘要 side_usage 分别写行。因此本系统已具备可复用的旁路计费边界，不能把本项描述为成本系统完全未实现。正常用量测试并不证明所有附属请求自动接入它。
- 成熟参考：Claude `借鉴/claudecode/restored-src/src/services/api/claude.ts` 在收到 message_delta usage 后更新当前 message 及调用 addToTotalSessionCost；返回量进入会话累计的动作发生于公共模型调用路径，独立于主回答文本是否被采用。这里借鉴“每个已完成调用都有用量归属”的边界，不主张直接照搬其进程级状态或 API 字段；未运行 Claude，也不声称其全部辅助客户端都被这一个调用点覆盖。
- 后果：开启检查后，会话累计 tokens/成本统计低于已收到的供应商 usage，用户基于该账本查看消费与预算时缺少这部分消耗。没有实测月/日限额绕过或大额消费，因此该后果不升级为已复现。修复建议（未实施）：在既有 ModelClient/调用方 seam 传递带实际模型身份、调用归属和 usage 的结果，复用现有 CostOrigin::SideCall/CostService 幂等记录；文本/判定可丢，用量不可因 PASS、FAIL、解析错误而丢；不要将附属请求量并入上下文占用 gauge，也不要另建私有账本。验收覆盖成功/不采用/解析失败、已返回 usage 后取消、模型身份、缓存字段、缺价格保存与重复终帧去重。

### H-64：自动完成验证没有运行取消与进程归属，总时限终态后真实 build.rs 仍能写文件

**级别：高（P1）；范围：AppCore 在识别到构建系统的工程中自动装配 VerificationPlan，且本轮真实创建/修改文件，模型声明完成后进入构建验证。** 该计划与可选对抗复核/停滞分类不同，不以开启这两个实验功能为前提。本项是执行生命周期缺口，不把它推断为用户此前每次准备慢或宿主 Codex 续接失败的原因。

- 生产路径：`deepagent-app-core/src/completion_plan.rs::discover_verification_plan()` 根据工程结构返回真实 `SystemCommandRunner`；Rust 步骤为 `cargo check --offline --quiet`。`run_assembler.rs` 将该计划和 hooks 一起挂到既有 Kernel。`deepagent-runtime/src/loop_engine.rs` 的 Complete/CompleteMessage 分支在 `run_mutated_files()` 成立时调用 `verify_after_completion()`；后者直接等待 `Verifier::run_suite()`。`deepagent-verification/src/verifier.rs` 逐项等待 runner，没有运行 token 或步骤前取消检查；`runner.rs::SystemCommandRunner::run()` 在 `spawn_blocking` 内使用 `std::process::Command::output()`，没有可取消进程句柄、进程树 guard 或命令截止时间。
- 根因：运行的取消树与构建进程没有关联。外层 Kernel 的 `tokio::time::timeout(engine.run(...))` 只能丢弃等待该 blocking task 的 future，不能停止已经执行的 `cmd.output()` 或其子进程。不能将 `spawn_blocking` 视为取消机制，也不能仅在构建之后检查一次 flag 来修复终态后的副作用。
- 动态证据：新增 `completion_plan/audit_tests.rs`，创建无外部依赖的临时 Cargo 工程、真实 `build.rs` 和 answer.txt 的正式 checkpoint Created 证据。使用正式计划发现、SystemCommandRunner、ModelAgent、Kernel、SQLite RunStore；模型 transport 为离线完成帧，文件变更由测试在进入 Kernel 前准备，**不是完整工具写入或桌面验收**。发现的命令仅增加 `--target-dir` 隔离构建锁/产物，未替换执行器或构建步骤。build.rs 先落 `build-started`，等待测试放行后再写 `build-wrote-after-release`；测试看到真实启动标记后才取消，避免“命令尚未启动”的伪通过。
- 正常对照：观察约 215ms 时 Kernel 未退出、无终态、无迟到文件；放行后构建成功、任务 Completed、run=succeeded、终态事件 1 条。用户取消组：`handle.cancel()` 成功且 flag=true，约 215ms 后仍未退出、无终态；放行后构建仍写文件，随后该样本最终为 cancelled、任务 Failed、终态 1 条。**本项没有证明取消总会误成功，也未实测任意长度构建的最坏停止耗时；已证明取消无法中断正在运行的构建。**
- 总时限对照：配置 5 秒 task_timeout，真实 build.rs 已启动；从启动标记后约 4,430ms 观察到 Kernel 已退出，SQLite=budget_exceeded、任务 Failed，迟到文件尚不存在。随后只放行已启动的 build.rs，真实文件出现；证明终态提交后原构建仍在执行并产生副作用。没有假造外部残留状态，也未触碰真实用户工程。每组在缺陷断言前放行、等待文件，并用同 target-dir 的第二次真实 Cargo check 等待原 Cargo 锁释放，清理成功后才释放临时目录；build.rs 另设 15 秒防悬挂上限。
- 可重跑：`cargo test -p deepagent-app-core --lib completion_plan::audit_tests::audit_ --offline -- --ignored --nocapture`。两项均明确失败，约 6.28s；正常对照各自通过。默认 `completion_plan::audit_tests::` 为 1 passed / 2 ignored，约 1.51s；忽略项必须显式运行，不能把默认绿测当修复。未动态验证 npm/tsc/go/maven/dotnet、macOS/Linux 或 Tauri 操作。
- 影响：模型已给出答案，界面仍可能长时间停在完成验证；停止无法阻止构建脚本继续操作文件。达到运行总时限也只停止主等待，旧构建可与下一轮编辑/构建交错、持有构建锁或使用 CPU。实际已发生的是测试文件在终态后写入；覆盖真实代码、重复发布或外部副作用属于依赖工程脚本的潜在风险，未在用户环境制造。
- 成熟实现对照：Codex `core/src/exec.rs::ExecExpiration` 显式统一 Timeout/Cancellation/TimeoutOrCancellation；执行等待在同一个 select 中处理进程退出与 expiration，取消分支对归属进程组先请求终止、有限等待再强制清理，超时分支同样清理子进程/进程组。`core/src/tasks/mod.rs::handle_task_abort()` 先发 turn token，等待 done 或有界 grace，再 abort 任务并调用专属清理。本轮只阅读其源码，未运行参考项目；不能据此声称所有内部 helper 都已覆盖取消。
- 本系统已有可复用能力：`deepagent-builtins/src/bash_tool.rs::SystemExecutor::run_controlled_with_environment()` 已接收取消与 timeout，实际进程使用 kill_on_drop、Windows Job/Unix 进程组及等待分支。另一个外部 hooks runner `deepagent-hooks/src/external_hooks.rs::run_hook_process()` 也已有 kill_on_drop 和 ProcessTreeGuard，runtime `fire_hook()` 已 select 运行取消；因此不列“全部 hooks 都不响应取消”的错误结论。这些源码保护是否覆盖所有 hooks 边界还需独立动态验证，不能用本次构建探针证明。
- 修复方向（尚未实施）：让既有验证计划接入运行命令执行/权限及进程 owner 边界，传递同一取消树和有效 deadline，每一步开始前与结果提交前均核对取消。无论主动取消、总时限、验证错误或 future drop，均需在有界时间内停止归属进程树并处理输出 reader；不得另造验证执行系统，也不能仅在 Kernel 完成后发 kill。回归需覆盖真实 build.rs/脚本、连续步骤、子孙进程、停止与完成竞态、父进程先退出但管道开放及单终态，并确保正常工程构建仍可完成。本轮只新增审计测试和文档，没有修改生产运行行为。

### H-65：两处自动验证绕过已配置命令后端，显式构建被拒绝后仍可在宿主执行项目脚本并写出工作区

**级别：高（P1）；范围：已识别构建系统的工程，本轮任一创建/修改触发完成验证；不要求用户修改 build.rs 或源码。** 本项与 H-64 的取消归属不同，修好取消也不能修复执行位置；与 H-03 的每轮 backend override 丢失也不同，本次没有任何 backend override，始终使用同一个已配置、确实能拒绝显式命令的后端。

- 生产边界：桌面 `src-tauri/src/lib.rs` 以 `SandboxBackendCommandExecutor(SandboxieBackend, WorkspaceWrite, network=Disabled)` 注入 `ChatService::with_local_command_executor()`，`run_assembler.rs::base_registry_request()` 将其交给普通工具，并可包装 RuntimeCommandExecutor 供应项目环境。但 `discover_verification_plan()` 无条件另建 `SystemCommandRunner`，不接收该执行器、SandboxBackend、环境 broker 或审批对象；`SystemCommandRunner` 直接在宿主 cwd 执行进程。`cargo check --offline` 仍会执行本地 build.rs，`--offline` 不是文件系统/进程隔离。
- 第二处生产入口：`kernel_runtime.rs::build_kernel_runtime_config()` 装配 `VerificationDecorator`，传入 `VerificationDispatcher::standard()`；后者另建裸 `SystemExecutor`。工具结果在 `ToolPipeline::complete_prepared_inner()` 调用 decorator 后才返回模型；对新建 `.rs` 文件，RustVerifier 直接通过该默认执行器运行 `cargo check --offline --manifest-path ...`，同样没有使用宿主配置的 executor。该路径可发生在模型最终声明完成之前。
- 真实可达复现：新增 `tests/kernel_v2_e2e/build_boundary.rs`，通过正式 SettingsService、ChatService、ToolRegistry、ModelAgent/Kernel 和 RunStore 执行。只有模型 transport 为离线序列帧、SandboxBackend 为记录并拒绝每个命令的测试实现；该 backend 不启动任何进程，外围仍是正式 SandboxBackendCommandExecutor，断言请求模式为 WorkspaceWrite。临时 Cargo 工程没有外部依赖，build.rs 仅向同一 TempDir 内的 workspace 兄弟文件写 canary；不访问用户目录、不联网、不测试真实 OS 容器。
- 对照 A：模型显式调用 bash 的 `cargo check --offline --quiet`，backend 被调用 1 次并返回唯一拒绝标记；下一次真实序列化模型请求包含该标记。模型随后结束，本轮没有文件修改；run=succeeded、终态 1 条、canary 不存在。此成功只是模型接受失败结果后结束，不表示显式构建通过。
- 对照 B：同样的显式命令被拒绝且回到模型；随后使用正式 write_file 写入 `note.txt`，模型声明结束。`.txt` 不触发单文件 Rust verifier，排除另一路校验干扰。backend 仍只收到最初那 1 次调用；真实 note.txt 存在、run=succeeded、终态 1 条，build.rs 的 canary 却出现在 workspace 外。说明完成验证没有使用配置的拒绝后端，而实际执行了宿主构建。每组均使用独立 TempDir；有限 Cargo 构建已返回后才删除目录。
- 对照 C：显式构建仍由相同 backend 拒绝，然后正式 write_file 新建 `src/added.rs`。transport 在每次真实模型请求发出前检查 canary；此组在第三次请求（文件写入后的下一次采样、最终回答尚未返回）已观察到 canary，backend 仍只调用 1 次，run 最终成功。B 组在同一采样边界 canary=false、最终完成后=true；A 组始终=false。三组区分了单文件 decorator 与最终完成计划两处真实执行入口，不能仅修复其中一处。
- fixture 修正：最初 C 样本直接覆盖现有 src/lib.rs，未事先 read_file，正式工具的读前改保护正确拒绝，原文件不变。该失败没有作为漏洞证据；改为创建新文件，并严格核对新文件内容，才得到以上 C 对照。没有取消或放宽读前改保护。
- 配置界限：测试采用 FullAccess **审批策略** 避免额外人工等待，同时显式保留 WorkspaceWrite **sandbox mode** 并在 backend 请求处断言；二者在本系统是独立设置。已证明“配置后端拒绝显式命令却被自动验证绕开”，没有证明“用户点击拒绝审批后系统仍执行”，也没有证明突破实际已建立的 Sandboxie/Windows Sandbox。桌面源码对注入 seam 的使用已核对，但真实桌面隔离软件的验收尚未执行。
- 可重跑：`cargo test -p deepagent-app-core --test kernel_v2_e2e build_boundary::audit_auto_build_respects_configured_rejecting_backend --offline -- --ignored --nocapture`。初版 A/B 明确失败、约 1.30s；扩展 A/B/C 后再次明确失败、约 2.65s。三组 `backend_calls=1`、`explicit_rejection_in_wire=true`、run_succeeded=true；A `(outside,before_final)=(false,false)`，B `(true,false)`，C `(true,true)`。默认 `build_boundary::` 为 1 passed / 1 ignored；显式探针失败不能描述为修复或通过。
- 已确认影响：自动完成验证的执行约束与普通命令不同，项目脚本可在主体命令后端拒绝执行时仍进入宿主执行。仅修改说明文件也能触发，因此不能称这些步骤天然“无副作用”。依赖项目脚本，可能读写宿主文件、访问网络、执行任意本地程序；实际复现限于受控 canary 写出 workspace，没有进行凭据读取、联网或破坏性操作。`npm run build` 等其他结构发现命令共享执行器，但尚未逐一动态验证。
- 成熟实现对照：Codex `core/src/tools/orchestrator.rs::ToolOrchestrator` 将审批、sandbox 选择和尝试封装在统一路径，传递同一 ToolCtx 的 cancellation 和有效权限；受限 sandbox 不可满足时拒绝，扩大权限路径也要核对允许条件。Claude `tools/BashTool/BashTool.tsx` 将执行权限交给 `bashToolHasPermission` 并由受控 shell 路径执行；不能因为命令是测试/构建就绕过工具权限。本轮只阅读参考源码，不主张它们具有与本系统相同的自动完成计划，也不声称其所有内部 helper 都受同一边界保护。
- 修复方向（未实施）：自动验证与普通命令应共享已经解析的执行目标、有效环境、权限/审批和 SandboxBackend seam；配置的 backend 不可用或拒绝时明确返回验证不可执行/失败，不回退为宿主直接运行。自动触发不是额外授权，`--offline`、命令名白名单或输出静默都不代替隔离。复用既有 RuntimeCommandExecutor/受控进程实现及 `VerificationDispatcher::standard_with_executor()` seam，不另造“可信验证执行器”；完成计划与单文件 decorator 都应绑定同一运行的执行配置。回归需覆盖 backend 拒绝/不可用、工作区外写入、网络禁用、远程工程、单文件验证和完整验证、取消/超时，并确保无构建系统或未修改文件的普通对话不启动构建。本批只增加审计测试与文档，没有修复这两处生产入口。

### H-66：Flash 开启 DeepSeek 原生搜索后，本地搜索函数被替换为该模型会忽略的原生工具

**级别：中（P2）；范围：启用 web feature、搜索 enabled 且 provider=DeepSeekFirst、有效模型为 deepseek-flash 的 Responses 请求。** 桌面启用了 web feature；搜索设置默认 enabled=true、provider=DeepSeekFirst。本项是能力选择与实际请求契约不一致，不声称所有模型或所有搜索配置均失效。

- 官方依据（2026-10-22 核对）：[DeepSeek Responses 指南](https://api-docs.deepseek.com/guides/responses_api/) 的 Tools 表明确：服务端 web_search 支持 deepseek-v4-pro，在 deepseek-flash 上忽略。这不是返回 400 的承诺；忽略可能不产生显式错误，不能把模型请求成功当作搜索可用。
- 生产链：`model_runtime.rs::select_run_model()` 和 `build_model_client()` 仅按全局搜索设置计算 native_web_search，没有核对所选模型能力；`tool_runtime.rs::register_web_tools()` 原本注册了可调用的本地 WebSearchTool。`deepagent-models/src/client.rs::apply_defaults()` 随后按名称将所有 web_search schema 的 kind 改成 web_search；Responses serializer 因而发送原生 `{type:web_search}`，不再发送 function 的 name/parameters。当前客户端仍有外部搜索链，但没有作为可调用函数出现在该次请求里。
- 复现新增 `tests/kernel_v2_e2e/search_capability.rs`。使用正式 SettingsService、ChatService、Kernel、ToolRegistry、ModelClient 及实际请求序列化；模型 transport 为记录请求并立即返回答案的离线 fixture。Chat/Reasoner 两个角色均固定为所测模型；禁用 deferred tool search，排除 schema 按需隐藏。每组严格核对实际 wire 的 model、仅一次请求与唯一 succeeded 终态。没有调用搜索工具、没有公网请求，也没有使用真实凭据。
- 正常对照：Pro+DeepSeekFirst 开启为 `(native,callable_function)=(true,false)`；Flash+DuckDuckGo 开启为 `(false,true)`；Flash+DeepSeekFirst 关闭为 `(false,false)`。默认测试 `supported_native_and_external_search_keep_their_wire_contracts` 通过。这只验证请求契约，不代表已在线验收搜索结果。
- 显式探针 `audit_flash_keeps_callable_search_when_native_is_unsupported` 先执行上述三组，再执行 Flash+DeepSeekFirst 开启；实际 `(true,false)`，期望不发送不支持的原生工具并保留现有外部函数入口 `(false,true)`，断言明确失败。执行 `cargo test -p deepagent-app-core --features web --test kernel_v2_e2e search_capability::audit_flash_keeps_callable_search_when_native_is_unsupported --offline -- --ignored --nocapture`：0 passed / 1 failed，约 0.66s。期望的 fallback 是本系统现有能力的修复选择；若产品选择不回退，也必须明确报告该模型不支持，不能把两种策略都说成参考实现原样行为。
- 测试夹具修正：首次编译时 ignored 测试直接 await 带 tokio::test 宏的函数，返回值不是 Future；改为两个测试共用普通 async helper 后编译及默认对照通过。该编译错误不是产品缺陷，没有修改生产功能来使探针失败。
- 已确认后果：在上述默认配置组合中，模型没有本地函数搜索入口，而发送的唯一原生搜索声明又超出官方支持范围；用户选择开启搜索与实际可用能力不一致。涉及实时资料的任务可能因此无法检索或缺少依据。未调用真实 Flash，不能宣称已观察到搜索请求数为零、回答造假或联网质量下降；还需在线任务和搜索计数验收。工具 schema 被其他已有权限/发现策略移除的场景不在本次覆盖内。
- 成熟实现对照：DeepSeek Harness `packages/web/tool-web/src/search.ts::runSearchQueries()` 将模型可调用工具交给独立 `ctx.web.search(request,signal)`；`packages/web/web/src/index.ts::WebRuntime.search()` 在明确注册/选择的 provider 上执行，无法解析可用 provider 时返回显式错误。该机制使主模型不具备内置搜索时仍可使用外部搜索 seam。本轮未运行参考项目，其 DeepSeek 搜索插件另用辅助模型/协议，不能照搬其默认模型并宣称与当前官方 Responses 限制相同。Codex `core/src/tools/hosted_spec.rs::create_web_search_tool()` 则接收独立的搜索模式与模型工具类型构造声明；本轮只核对该 seam，不据此声称 Codex 自动拦截所有不支持模型。
- 修复方向（尚未实施）：在既有有效模型/协议能力解析与 provider adapter 边界决定 hosted search 或 callable search，注册定义、路由和设置状态共用同一决定。对不支持原生工具的模型保留显式外部 provider，或明确提示不可用；模型切换、备用模型与子运行均需重新解析。测试应覆盖默认 Flash、Pro、搜索禁用、外部搜索、模型切换，以及实际搜索计数和失败反馈；不新增第二套搜索注册表，不通过删除开关或提示模型“假装联网”绕过问题。

### H-67：当前轮图片未进入实时模型 Surface，原生镜像也丢附件，已有会话的图片在下一轮仍不出现

**级别：高（P1）；范围：ChatService 的结构化图片输入，尤其桌面关闭第三方系统视觉、直接把原图发送给主模型的路径。** 系统视觉开启时可能有额外的前置识图文字，该独立路径不能据此判为完全不可用；本项不涉及供应商不支持图片、图片过大或附件丢失。

- 第一个错误边界：`run_assembler.rs` 将 `message_attachments(&self.attachments)` 写入 `session.append(MessageAppended)`，但 `ModelAgent::new(client, model, system_prompt, final_user_prompt, tools)` 只传纯文字 goal，没有把同一当前输入的附件交给实时 Surface。`ModelAgent::new` 以纯文字 Message::user 和 ResponseHistory::from_seed 创建当前目标；with_history/with_response_history 加的是接纳阶段读取的历史，不包含刚写入的当前事件。随后 FileAttachmentResolver 只遍历 Surface 已存在的 InputMessage，无法补回根本没有引用的本轮图片。
- 新增 `tests/kernel_v2_e2e/image_preparation.rs`：通过正式 AttachmentService 和桌面 AttachmentIngestDto 导入 68 字节的 PNG，SettingsService、ChatService → Kernel → ModelAgent → ModelClient 与序列化均真实执行。模型只替换底层 HttpTransport，记录每次实际 input_image 数并返回中性的离线答案；没有公网识图调用或真实凭据。所选 Chat/Reasoner 均为官方支持图片的 deepseek-flash，不依赖 fallback。
- 两轮对照：第一轮用 run_in_session_with_attachments 提交新图；第二轮沿同一 session 普通 run_in_session，不重新上传。图片文件始终存在，两轮各一条 succeeded 终态，各发一次模型请求；实际图片数为 `[0,1]`，应为 `[1,1]`。同一文件在第二轮成功转为 data:image/png，排除了文件解析器、图片尺寸、权限拒绝或 fixture 没有设置 data URL 的解释。第二轮看到图片不能弥补第一轮在缺图前提下已经完成的回答。
- 已执行显式探针 `cargo test -p deepagent-app-core --test kernel_v2_e2e image_preparation::audit_new_image_reaches_first_model_request --offline -- --ignored --nocapture`：约 0.32s 明确失败，实际 `[0,1]`、期望 `[1,1]`。正常历史对照 `retained_imported_image_reaches_continuation_request` 通过；该默认测试只确认第二轮，不以默认绿测宣称新图片发送正确。随后补强原生视觉 Off 设置与第一轮事件的附件引用断言，按下面验证记录重跑。
- 补强结果：显式设置 VisionMode::Off、读取真实 EventStore 并严格检查第一轮用户事件含导入图片的 id/path；将 transport 从仅文本 delta 改为完整 message 的 output_item.done 和 response.completed（含 output），避免只走无原生输出的兼容回放。显式探针再次约 0.37s 失败，仍为 `[0,1]`；共享入口默认回归 14 passed / 12 ignored，约 1.67s。没有因此宣称所有后续用户输入都能通过原生历史恢复，那个边界需另测。
- 后续输入的独立扩展（同根因，不另增编号）：先用纯文字建立会话，再在第二轮首次上传同一合法 PNG，第三轮仅追问。增强探针先跑前述初始图片组，再跑此组；第二轮 MessageAppended 仍严格含正确附件 id/path，文件始终可读，但三次实际 wire 图片数为 `[0,0,0]`，应为 `[0,1,1]`。两组分别有 2/3 个唯一 succeeded 终态、模型请求也恰为 2/3 次。2026-10-22 执行同一显式命令约 0.80s 再次失败：`([0,1],[0,0,0]) != ([1,1],[0,1,1])`。
- 第二个附件保真断点：`deepagent-session/src/lib.rs::response_items_for_payload()` 对 MessageAppended 自动创建 ResponseItem::Message，无条件只复制 role/content，不读取 message.attachments。两组上传后正式日志里的原生 InputMessage 图片引用数均为 0，而原始 MessageAppended 已含附件。`input_runtime.rs::indexed_response_items()` 在出现首个原生事件后优先使用原生镜像，不再转换后来的兼容 MessageAppended；只有首个原生边界前的带图源消息，因不等于纯文字镜像，才被兼容逻辑再次带入。这解释了初始图片组下一轮出现、已有会话组下一轮仍消失的差异，不能把 `[0,1]` 概括为图片总能延迟一轮恢复。原始事件未删除，属于实时/原生投影丢失，不是磁盘图片被清除。
- 调查与 fixture 边界：最初用完整 82 字节 PNG 检查正常图片时先发现首次 wire 图片为 0，因此停止把“不可读/超限附件静默省略”直接定性，转而追当前输入与历史的不一致。最终采用 68 字节样本；修正其 IDAT CRC 后，用独立 Python zlib/CRC 以及 Pillow 实际解码确认为 1×1 PNG，再重跑仍为 `[0,1]`。没有通过删除生产校验或手工把图片塞进 request 造出故障。
- 影响：用户在本轮问截图、图表或图片内容时，主模型实际只得到文字；运行仍可正常成功结束，难以仅从运行状态发现缺图。初始图片组延迟出现，已有会话的新图在后续请求中仍不可见，因此追问不能保证绕过。这里确认的是实际模型输入缺失，没有调用真实模型，不能声称已测到幻觉率、特定答案错误或费用损失。默认系统视觉为 System，本项不能扩大为所有默认图片请求都无图/无识图文本；前端原生视觉路径由 Off 开关明确提供。
- 成熟实现对照：DeepSeek Harness `attachment/src/index.ts::admitPromptContent()` 将上传内容替换为有序 durable refs；`api/session-controller/src/commands.ts` 在 prompt 命令中完成该接纳再转给会话。`llm-deepseek/src/images.ts::prepareImages()` 针对将要请求的 messages 准备引用，serialize.ts 从这些同一 message 的 image blocks 构造 wire。本轮仅源码对照，它使用 Messages 适配，不能把其字段直接复制为本系统 Responses 字段。可借鉴的是“当前消息的文字、图片和持久化归属是同一份输入”，不是在下一轮回放才补充图片。
- 修复方向（尚未实施）：在既有 RunAssembler/ModelAgent 原生 Surface 边界，用同一份已接纳当前消息创建实时输入和持久化事件；Session 的同一原生镜像必须保留附件，不能只修实时 goal。保留最终 prompt decorations、附件引用及 snip 标签，避免再次由纯文字重建。无需新增附件存储或重读整段事件来重复追加当前消息。回归需覆盖首轮、新图续轮、多图、工具循环、重启恢复、压缩后保留图像、系统视觉文字路径及 ContextMeter 与实际 wire 一致；单测手工写入 attachment_data_urls 不足以验收这条跨层链路。

### H-68：历史图片文件丢失时静默退化为纯文字请求，续聊仍成功

**级别：中（P2）；范围：保留在有效模型历史中的图片引用，在后续请求读取前对应文件已丢失。** 动态样本显式使用 VisionMode::Off、原生 Responses 图片路径；未证明文件缺失的实际发生频率，不扩大为所有图片请求失败。与 H-67 的当前输入/原生镜像缺图独立，前者修复后仍需处理本项。

- 生产链：`attachment_service.rs::FileAttachmentResolver::is_resolvable()` 将 metadata 失败转为 false；`data_url()` 在不满足条件时返回 None，读取错误也由 `.ok()?` 消失。`model_agent.rs::request_for_current_history()` 只插入 Some URL，不记录缺失引用的错误；`chat.rs::response_input_message_to_wire()` 在没有 URL 时直接返回纯文字 message。`AttachmentDataResolver` 的 Option 接口把“无须解析”和“解析失败”合并，无法向上层保留原因。这不是供应商拒绝图片或网络错误。
- 动态复现：复用 H-67 的真实 AttachmentService 导入、68 字节合法 PNG、持久化用户事件及正式 ChatService/Kernel/序列化。两组都从首轮带图开始；正常组保留文件，续聊 wire 图片数为 1；故障组只在首轮结束、图片引用已正式落盘后，删除本测试 tempdir 内的原图，再用同一句普通追问续聊。没有修改生产解析器、手工填充 data URL 或伪造原生历史。
- 2026-10-22 显式执行 `cargo test -p deepagent-app-core --test kernel_v2_e2e image_preparation::audit_missing_retained_image_is_not_silently_dropped --offline -- --ignored --nocapture`，约 0.64s 明确失败。正常组 `wire_image_counts=[0,1], continuation_succeeded=true`；故障组 `[0,0], continuation_succeeded=true`，期望故障组在采样前显式失败且仅保留首轮请求。首轮的 0 是已知 H-67；正常组同一续聊路径的 1 排除了“历史图片本来就没进入 Surface”的解释。每组每轮均检查唯一终态，故障组公开 ChatService 调用实际返回 Ok。
- 影响：用户引用之前的图表/截图继续提问时，附件文件丢失不会阻止文字请求，也不会通过调用结果提示需要恢复附件，模型只能在缺图输入下继续回答。运行成功状态不能证明所需图片已送达；没有调用真实模型，不宣称已测到幻觉率、答案错误率或费用损失。原始事件引用仍在，测试不涉及删除事件或实际用户附件。读权限拒绝、超限、空文件及并发读写仅有同接口的静态线索，尚未动态复现，不能当成本项已测场景。
- 成熟实现对照：DeepSeek Harness `attachment-local/src/store.ts::readImageFile()` 对 ENOENT 抛 `ATTACHMENT_NOT_FOUND`，其他读取失败抛 `ATTACHMENT_READ_FAILED` 并保留 cause，同时检查 digest/metadata。`attachment-local/src/index.ts::readImageRequest()` 经 `readImage()` 读取原图；`llm-deepseek/src/images.ts::prepareImages()` await 该调用再序列化，不把缺失图片映射为无图。它也提供显式 offload 投影，但不能把 offload 与任意读失败等同。本轮只源码核对参考项目，没有运行其测试，协议字段不直接照搬。
- 修复方向（尚未实施，本地边界设计）：在既有附件解析 seam 返回能区分不适用与失败的结构化结果；有效历史中需要的图片无法读取时，在真实模型请求前通过现有运行错误/终态通知反馈，保留原始事件并提供恢复或显式移除附件的路径。若产品选择文字降级，应由明确策略和可见说明决定，不能用 None 隐式执行。禁止另建附件存储或静默改写用户历史。回归至少包括正常读取、丢失、权限拒绝、空文件/超限、取消和恢复后重试；以实际模型请求计数、错误原因和单终态为准。

### H-69：只限制单张图片，缺少多图序列化后的请求体总预算

**级别：中（P2）；范围：原生 Responses 图片请求包含多个合法本地图片，base64 后的完整请求体超过供应商上限。** 不要求达到上下文 token 压缩阈值；本次只确认本地发送边界缺少总字节保护，未调用真实供应商。

- 官方契约（2026-10-22 实际打开 [DeepSeek Vision](https://api-docs.deepseek.com/guides/vision/)）：inline/base64 图片计入 48 MiB 请求体；单张 inline 图片最多 32 MiB；总原图最多 64 MiB，最多 600 张。Responses 采用相同限制。官方同时提供 Files API 引用，不能据此宣称本系统已经接入该路由。
- 生产链：`FileAttachmentResolver` 逐图允许非空且不超过 32 MiB 的文件，然后直接读取原始字节并 base64。`request_for_current_history()` 汇集所有可解析 URL；`ResponseRequest::serialize()` 输出全部图片；`ModelClient::stream_response_observed_cancelled()` 对完整请求做 serde_json 序列化后直接构造 TransportRequest，真实 reqwest transport 接收该 body。此链没有根据序列化体积校验 inline 总预算；token pressure 测量与字节传输上限不是同一约束。
- 动态证据：新增 runtimes feature 下的 ignored 探针，用仓库已依赖的 image 0.25.10 PngEncoder 生成真实 2048×2560 RGBA PNG（Uncompressed、NoFilter），再实际 image::open 解码并核对尺寸。文件大于 20 MiB、小于 32 MiB；不存在损坏 CRC、虚构 data URL 或超大像素尺寸。分别导入一份和两份独立 attachment id，经正式 ChatService 第一轮持久化，再普通续聊。两图低于 Composer 的每次 5 个附件限制，原图合计小于 64 MiB；没有靠超量上传绕过产品限制。
- 执行 `cargo test -p deepagent-app-core --features runtimes --test kernel_v2_e2e image_preparation::audit_multi_image_request_respects_inline_body_budget --offline -- --ignored --nocapture`：约 4.51s 明确失败。单图实际 body `[41733,28009830]` 字节、wire 图片 `[0,1]`；双图 body `[41787,55977624]`、图片 `[0,2]`。续聊均收到离线回答并写出唯一 succeeded 终态。双图续聊体积约 53.4 MiB，大于 48 MiB（50,331,648 字节）。首轮无图是 H-67，续聊实际出现全部图片才用于本项判断。
- 影响：符合单图限制的正常多图输入仍可能生成违反 provider 契约的请求，引发失败及重复传输开销；长历史中媒体字节也可能先于 token 容量达到上限。实际确认的是超限 body 已交给 transport，没有测真实 HTTP 状态、自动重试次数、UI 提示、实际计费或内存峰值，不能声称供应商接受了超限请求。600 张限制、64 MiB 原图总量及尺寸分支尚未动态验证。
- 成熟实现对照：DeepSeek Harness `llm-deepseek/src/request-pricing.ts::resolveRequestImageTarget()` 独立选择请求图片尺寸与编码目标（默认编码目标 2 MiB，不承诺所有图片一定压到目标）；`images.ts::prepareImages()/inlineImages()/assertImagesFit()` 按 raw/base64 表示计算累计大小和数量，超界抛 IMAGE_OFFLOAD_REQUIRED。`adapter.ts` 在 Files 解析与 inline fallback 后、fetch 前执行该准备/校验链；过量媒体需显式 offload，而非默默丢图。它的图片预算并不等同于对整个 JSON body 的严格测量，本系统仍需为文字/工具等 body 部分留余量。本轮未运行参考实现，不把参考的内部默认预算视为官方上限。
- 修复方向（尚未实施，本地设计）：复用现有附件存储和 provider seam，在请求图像准备时应用明确的编码/尺寸策略，再按目标路由检查实际 inline 字节与完整 body 总量；有已配置 Files 能力时可选择文件引用，否则在发送前明确拒绝或按显式策略 offload，保留原始图片与历史归属。处理重试/备用模型时重算目标路由，不能仅增加 token reserve、静默删除图片或无限重试相同 body。测试覆盖单图/多图、文字与工具 schema 大头、历史累积、边界前后、取消、Files 不可用和恢复重试。

### H-70：导入哈希未绑定模型历史引用，图片替换后同一附件 ID 静默发送新内容

**级别：中（P2）；范围：有效历史图片对应的持久化原图文件在后续请求前被修改或替换。** 本项要求已有本地写入行为；测试没有证明攻击者可获得该权限，也不把它描述为任意文件写入或沙箱逃逸。H-58 的路径边界和 H-68 的缺失文件降级是其他独立断点。

- 生产链：`AttachmentService::ingest()` 对落盘原图计算 SHA-256，写入 DTO/metadata.json；但 `InputAttachment::Image` 和 `MessageAttachment` 只保留 id/path/media_type（后者另有 kind），`run_assembler.rs::message_attachments()` 没有携带内容摘要。`FileAttachmentResolver` 只检查当前路径的类型、非空及 32 MiB 上限，再读取/base64，不查询或比较 metadata 中的哈希。因此同一事件引用并不保证后续请求读取原先接纳的图片。
- 动态证据：复用正式 AttachmentService、ChatService、Kernel 和真实 Responses 序列化，增加可选的 transport 图片哈希观测，不改生产请求。正常组中，续聊实际 input_image 解码字节的 SHA-256 与导入 DTO 相等。替换组先正式导入同一合法 68 字节 PNG、完成首轮和用户事件落盘，再仅将该测试 tempdir 内的原图换成 image crate 生成的另一张合法 1×1 红色 RGBA PNG；实际解码核对像素为 `[255,0,0,255]`，并严格断言新旧摘要不同、metadata.json 仍保持原摘要。
- 2026-10-22 执行 `cargo test -p deepagent-app-core --features runtimes --test kernel_v2_e2e image_preparation::audit_retained_image_verifies_ingested_digest --offline -- --ignored --nocapture`，约 0.69s 明确失败：`metadata_still_original=true replacement_sent=true continuation_succeeded=true`。两组 wire 图片数均为 `[0,1]`；两组续聊均写出唯一 succeeded 终态。首轮 0 是已知 H-67，判断依据是后续已包含图片的实际 wire 字节，不由“缺图”推断内容错误。未重传新附件、改写事件、更新 metadata 或使用不合法文件。
- 影响：历史附件身份与实际模型输入可以随磁盘内容漂移，续聊/重放难以证明引用的是用户原先提供的图；受影响的分析和审计依据可能变化而运行不报错。这里只确认输入保真缺口，没有真实模型回答、隐私泄露、攻击成功率、外部可写路径或非授权修改证据；本次替换由测试明确执行，不是系统自发改图。
- 成熟实现对照：DeepSeek Harness `attachment-local/src/store.ts::readImageFile()` 从 `sha256:...` 引用取得期望摘要，读取后比较 digest，并核对 mediaType/bytes/width/height；不匹配抛 `ATTACHMENT_CORRUPT`。相同文件的不同编码/尺寸请求版本由 `readImageRequest()` 在受控转换中派生，不能将任意路径变化视为合法转换。本轮只源码对照，未运行参考测试。
- 修复方向（尚未实施，本地设计）：复用现有附件存储和导入摘要，将稳定内容身份带入既有持久化引用或可核对的元数据解析边界；读取请求原图时校验身份，不一致时显式失败并保留源事件。受控重新编码应有明确派生关系，合法更新以新资产/新消息接纳，不能静默借用旧 ID。旧历史缺少摘要时需显式兼容/迁移策略，不能伪造已经验证；覆盖正常、缺失、损坏、合法替换、旧记录、重启和取消，并检查实际发送字节与单终态。

### H-71：发现目录丢弃官方输入模态，文字模型仍收到原生图片请求

**级别：中（P2）；范围：已发现的模型明确不支持 image，却构造了含图片的原生 Responses 请求。** 动态验证集中在模型发现/目录恢复/provider seam，不把它扩大为所有默认 Flash 图片都失败，也不声称完整桌面必定到达相同分支；H-67 的当前图片装配缺陷可能先阻止图片进入请求。

- 官方依据：2026-10-22 实际打开 [Lists Models](https://api-docs.deepseek.com/api/list-models/)，其 input_modalities 表示模型可接受的输入类型，示例 Flash 为 `[text,image]`、deepseek-v4-pro 为 `[text]`。本项使用这些公开字段构造离线 fixture，不把模型身份或能力从本地名字猜出；没有查询用户账号的实时 model 列表。
- 生产链：`discovery.rs::ModelInfo` 仅保留 id/object/owned_by/context_window/max_output_tokens，serde 忽略未知 input_modalities；持久化的 ModelCatalog 无法恢复该字段。`ModelConfig::from_catalog()` 生成的 ModelCapability 也无模态字段；`ModelClient::stream_response_observed_cancelled()` 在 defaults/序列化后直接调用 transport，没有根据目标模型检查原生 input_image。容量 metadata 数字确实保留，不可把本项说成整个官方元数据均丢失。
- 动态证据：新增 `deepagent-models/tests/modalities_audit.rs`，使用正式 ModelDiscovery 解析公开两模型目录，再实际序列化/恢复 ModelCatalog 并构造正式 ModelClient。三个请求经过同一 cancel-aware Responses 入口、真实请求序列化和离线 transport：Flash+图片发送 1 张图且客户端完成；Pro+纯文字发送 0 图且完成；Pro+图片也发送 1 张图且完成。三组解析后的 context_window 均严格等于 1,048,576，input_modalities 是否保留则均为 false。图片为已经验证过的合法 1×1 PNG data URL，在 provider seam 显式提供已解析内容，不经过 H-67 的缺图路径；不能把此夹具称为真实桌面上传验收。
- 执行 `cargo test -p deepagent-models --test modalities_audit --offline -- --include-ignored --nocapture`：1 passed / 1 failed，测试体小于 0.01s；失败探针 `audit_declared_input_modalities_constrain_requests` 明确观察 `modalities_retained=false transport_requests=[(deepseek-v4-pro,1)] accepted=true`。这里 accepted 仅表示本地客户端收到离线回答后返回 Ok，不表示实际供应商接受不受支持的图片。
- 影响：用户切换至不支持图片的模型，或运行选择这类备用模型时，有效历史图片可能形成与 provider 模态契约不符的请求；模型目录也不足以驱动准确的 UI 提示与请求路由。实际确认的是正式客户端边界缺失，未测试真实 HTTP 状态、UI 行为、自动 fallback、供应商计费或全部其他模型。不以这项推断 DeepSeek 官方当前不支持图片，Flash 支持图片已有公开依据和正向对照。
- 成熟实现对照：DeepSeek Harness `llm-deepseek/src/images.ts::prepareImages()` 从连接的 model entry 读取 inputModalities；有图但模型未声明 image 或 attachment service 不可用时，在读图/序列化/fetch 前抛 UNSUPPORTED_CONTENT，随后才准备图片。该参考使用 Messages 适配，借鉴的是按有效模型能力检查输入，不能照搬其协议字段或断言参考的所有模型切换路径已验收。本轮未运行其测试。
- 修复方向（尚未实施，本地设计）：扩展既有发现/能力解析边界保留公开输入模态并保持旧目录可读；在 provider 请求准备时按本次有效模型检查整份原生输入，工具输出图片等形态也应归同一检查。明确“不支持”和“未知”的政策，选择显式错误、用户确认的文字投影或具备能力的路由；不得静默去图或仅修改 UI。切模型/fallback/子运行共用该能力快照，回归涵盖发现、序列化恢复、旧目录、图片/文字对照和真实发送计数，不新增第二套模型注册表。

### H-72：检查点备份缺失时，失败的回退或分叉仍删除当前文件

**级别：高（P1）；范围：AppService 的 rewind_session / fork_session 共用文件恢复路径。** 触发条件是某份已经记录的检查点备份在恢复时不可用；本轮明确复现的是备份文件丢失，不代表普通回退每次都会失败。

- 根因：`deepagent-runtime/src/checkpoint.rs::CheckpointManager::restore()` 逆序遍历条目，对每个 target 先执行 `remove_existing()`，然后才检查 backup 字段、备份路径边界并复制内容。源备份不存在时，复制返回错误，当前 target 已被删除。`deepagent-app-core/src/service.rs::restore_checkpoints_after()` 没有恢复已处理文件的补偿；rewind 在文件恢复成功后才截断事件，fork 在成功后才创建分支。因而 Err 不代表工作区未变。
- 动态证据：新增 `deepagent-app-core/tests/checkpoint_audit.rs`，使用真实 Session、RunStore、CheckpointManager、CheckpointStore 和正式 AppService；两个临时文件分别从 first-before / second-before 修改为 first-after / second-after。按正式 capture 顺序记录后，仅删除测试自己 storage 下第一项的备份文件，不修改 manifest 或生产函数。逆序恢复先处理健康第二项，再遇到第一项缺失。Rewind 和 Fork 各使用全新的内存数据库及临时目录，均返回 Err；first.txt 已不存在，second.txt 已变回 second-before；源会话的完整事件 JSON 与检查点记录保持相等，session 数仍为 1，未创建 fork。补充存在性断言后重跑约 0.19s 仍为 1 passed / 1 failed，两个入口均明确输出 current_file_exists=false，排除单纯读取失败。Tauri 同名 command 确实调用这些 AppService 方法；本轮只查证桥接，未点击桌面执行回退。
- 执行：`cargo test -p deepagent-app-core --test checkpoint_audit --offline -- --include-ignored --nocapture`，1 passed / 1 failed，测试体约 0.22s。正常对照覆盖两个入口：完整备份均恢复两份 before 内容；rewind 改变原历史但不增会话，fork 保留原历史并新建会话。缺陷探针 `audit_missing_backup_preserves_current_file_on_failed_rewind_and_fork` 在同次执行中观察两个入口后，因“当前文件必须保留”断言失败。该探针默认 ignored，不能把默认基线绿测说成缺陷已修复。
- 后果：备份被外部清理、丢失或不可访问时，用户点击回退/分叉可能收到失败却失去当前工作内容；已有健康文件又可能回到过去版本，原历史仍描述回退前状态。若该文件没有其他副本，当前新内容不能靠已丢失的备份恢复。本轮没有测试磁盘写满、Windows 文件占用、目录备份缺损、并发写入或恶意 manifest，不把它概括成所有恢复错误都具有相同后果，也不宣称数据库已损坏。
- 成熟实现对照：`借鉴/claudecode/restored-src/src/utils/fileHistory.ts::restoreBackup()` 先 `stat(backupPath)`；ENOENT 时记录错误并返回，尚未尝试复制，更不先 unlink 当前文件。`applySnapshot()` 仅在明确的 null 备份标记（文件在目标版本不存在）时删除文件；无法解析备份时不触碰文件。其多文件循环也有逐项错误处理，且缺失备份返回后的 changed 标记仍需另行审计；因此这里只借鉴“恢复来源已知不可用时先保留当前文件”的边界，不声称 Claude 提供多文件/历史全局事务或本轮已运行它的测试。当前 Codex 参考树中的 ghost_snapshot 已标记兼容字段，未用旧版快照机制替代当前实现作证。
- 修复方向（尚未实施，本地设计）：在既有 CheckpointManager 内先解析并验证整个恢复计划的目标、备份边界及源可读性，缺失备份在任何目标删除前明确失败；恢复既存文件/目录时先在受限临时位置构建完整替换，再按平台能力提交，不能先删目标再读源。对中途失败保留逐项结果与可补偿的当前副本，明确 AppService 的文件恢复与事件截断/分支创建提交顺序和重试语义。跨文件、数据库和崩溃恢复的一致性属于本系统需要定义的本地契约，不从 stat 检查推导全局原子性。回归覆盖完整备份、缺失/不合法来源、部分复制失败、DB 截断失败、重试及 Windows 占用，不新建第二套事件或运行存储。

### H-73：回退后的旧分支检查点仍可覆盖新分支文件，调用却返回成功

**级别：高（P1）；范围：同一 session 原地回退后继续，再次调用文件回退。** 本项在所有备份完整、无数据库写入失败、无并发操作的正常流程中复现；不同于 H-72 的缺失备份故障，也不同于 H-23 的摘要水位，实际后果是工作区文件被旧分支覆盖。

- 根因：`AppService::rewind_session()` 恢复检查点后截断会话事件，`Session::rewind()` 允许新事件继续占用旧 sequence；旧 run/checkpoint 元数据仍保留。`CheckpointStore::list_for_session_after_sequence()` 仅按 runs.session_id 和 `c.session_sequence > target` 查询，没有判断 checkpoint 所属事件是否仍在当前分支。后续回退因而可以选中已经丢弃分支的 checkpoint，且所有恢复源有效时不会报错。保留旧记录本身不是缺陷，缺的是恢复查询中的有效分支归属。
- 动态证据：在 `checkpoint_audit.rs` 新增 `audit_rewind_does_not_restore_discarded_branch_checkpoints`，通过正式 Session 自动生成原生镜像、RunStore、CheckpointManager 和 AppService，在真实临时文件上执行：base → 旧分支 first 修改 old-first → 旧分支 second 修改 old-second → 正式 rewind 到 0 恢复 base → 新分支修改 current-branch，并保留到 assistant 答复的 sequence 4 → 再追加一轮纯文字对话，不再改文件 → 正式 rewind 到 4。两次 rewind 均 Ok，最终文本事件只含保留的新分支、没有旧分支或新尾部，但文件从 current-branch 变成 old-first，restored_paths=1。被误用的旧 second 检查点仍锚定 sequence 6；它的备份是 old-first，当前新分支在 sequence 6 处却是另一轮纯文字输入。各检查点均由正式 capture/commit 创建，旧 run 已显式标为 succeeded；没有伪造 manifest、重写检查点序号或删除备份。
- 执行：`cargo test -p deepagent-app-core --test checkpoint_audit branch --offline -- --include-ignored --nocapture`：1 passed / 1 failed，测试体约 0.35s。正常对照为没有旧分支的相同新分支流程：纯文字尾部回退保留 current-branch，restored_paths=0；实验组明确输出 `content=old-first, restored_paths=1, kept_sequence=4, discarded_checkpoint_sequence=Some(6)`，保留当前文件断言失败。默认 ignored 探针用于记录缺陷，默认测试通过不表示 H-73 已修复。
- 影响：用户撤销旧方案、继续编辑后，再回退一轮未修改文件的对话，也可能无错误地把文件恢复为旧方案。模型下一次读工作区时看到的文件与当前事件历史不同，继续编辑、验证或生成产物可能建立在错误版本上；若无其他副本，当前新内容可能丢失。本轮只验证原地 rewind 的顺序调用、单文件和服务入口，没有真实桌面、跨进程重启、fork 继承或并发写入验收；不外推为所有回退都必定误恢复。
- 成熟实现对照：Claude `fileHistory.ts::fileHistoryRewind()` 通过目标 messageId 使用 `findLast()` 定位 FileHistorySnapshot，再由该快照的 trackedFileBackups 决定每个文件版本；不是把同一会话中所有数字位置更大的历史备份依次恢复。该实现注释明确 rewind 是文件副作用，不修改 FileHistoryState，所以不能声称 Claude 靠删除全部旧快照解决。Codex `core/src/thread_rollout_truncation.rs::user_message_positions_in_rollout()` / `fork_turn_positions_in_rollout()` 先应用 ThreadRolledBack 对有效历史位置的影响，并清除失效后缀，再计算分支位置；这里借鉴的是当前分支身份与历史有效性的处理，不声称当前 Codex 提供同样的文件检查点或本轮已运行参考测试。
- 修复方向（尚未实施，本地设计）：让现有检查点元数据绑定稳定事件/分支来源，并让回退提交时退役被丢弃后缀的恢复资格；保留原始备份和审计记录，不依赖删除 runs 解决。当前有效前缀的检查点仍需支持继续向前回退，不能简单按“最新 generation”一刀切排除所有旧记录。文件恢复计划、事件截断、分支来源及 H-23 的笔记覆盖应共用同一分支变更事实，在既有持久化边界上定义失败和重试语义。验证包括两次/多次回退、同序号新事件、保留前缀、重启、fork、没有文件修改的尾部及真正的新分支修改，不新增第二套 store。

### H-74：分支边界截在源消息与原生镜像之间，保留的消息不进入下一请求

**级别：中（P2）；范围：fork_session / rewind_session 接受原始事件 sequence 作为 inclusive 保留点，且该点是已有原生历史后的 MessageAppended 源事件。** Harness 的 thread/fork 可显式指定 at_sequence；桌面当前默认使用最新 timeline sequence，本轮未证明该默认选点会落在同一断点，不能外推为所有正常分叉都会丢消息。

- 根因：`Session::append_with_response_projection()` 将 MessageAppended 与随后生成的 ResponseItemAppended 分别追加；前者返回的 sequence 可以成为合法 API 锚点。AppService/Session/EventStore 的分支与截断直接按 `<= target` 保存，既不扩大到完整逻辑消息组，也不拒绝这个边界。恢复时 `input_runtime.rs::indexed_response_items()` 在 first_native 之后只采纳原生事件，保留的源 MessageAppended 不再进入原生模型历史。因而“源日志保留”并不意味着请求 Surface 保留；该问题发生在纯文字上，区别于 H-67 的附件字段丢失。
- 动态证据：新增 `kernel_v2_e2e/branch_boundary.rs`。以正式 Session 依次追加较早 user/assistant、目标用户消息和待丢弃 assistant；目标源事件 sequence=5，自动镜像=6。通过正式 AppService fork/rewind 分别保留到 5 或 6，严格断言目标 MessageAppended 在保留事件中、尾部 assistant 已不在、实际尾 sequence 等于指定点。随后用真实 ChatService 接纳和恢复、ModelAgent 及正式 Responses 序列化发起下一轮；仅 transport 提供离线原生回答。四组请求均收到较早用户输入和本次继续指令；keep=5 的 Fork/Rewind 都没有目标用户内容，keep=6 的两组都包含目标内容。来源历史由正式 Session 创建，不是前三轮真实模型对话，也未伪造原生镜像。
- 执行：`cargo test -p deepagent-app-core --test kernel_v2_e2e branch_boundary --offline -- --include-ignored --nocapture`：1 passed / 1 failed，测试体约 0.66s。默认 ignored 的 `audit_branch_at_source_message_preserves_that_message_in_request` 在观察两入口后断言失败，均为 retained_user_sent=false；正常边界对照 retained_user_sent=true。所有继续调用返回 Ok；这不表示真实供应商能凭缺失内容正确回答，未发送公网请求。
- 影响：机器客户端在 thread/read 提供的源消息 sequence 上建立分支，或调用原始 rewind API 保留至该消息后继续，可能静默丢掉最近保留的用户约束/需求。源记录仍在，检查单纯持久化是否存在不能发现；后续自动镜像也不能补回已经错过的旧消息。未覆盖工具调用/结果切割、压缩边界、运行中分叉、UI 手动选择、跨进程重启及其他协议；不据此解释 Codex 宿主的 cipher_incompatible。
- 成熟实现对照：Codex `core/src/thread_rollout_truncation.rs` 通过解释用户/指令 turn 边界来确定有效分支位置，并先应用 rollback 对边界的影响；`truncate_rollout_before_nth_user_message_from_start()` 明确按 user message 位置裁剪。`truncate_rollout_after_turn_id()` 还要求目标 turn 位于当前有效历史、有持久化 TurnStarted 身份且状态不是 InProgress，裁剪到下一个 turn 起点之前；找不到稳定身份或目标仍在进行则明确拒绝。这些机制避免把任意原始记录位置直接等同于完整 turn。Claude 文件快照用稳定 messageId 定位目标（H-73 已述），只能作为语义身份参考，不能视为本轮已验证它的模型历史切割。需要借鉴的是明确逻辑消息/turn 的边界；本系统仍可保留现有 sequence 协议并定义它如何映射到完整提交组，不必照搬参考的 wire 字段。本轮未运行参考测试。
- 修复方向（尚未实施，本地设计）：在既有事件/恢复入口定义一个逻辑提交组，保存或解析源消息与全部原生镜像的关联；fork/rewind 统一把原始锚点解析为完整语义边界，或在执行前明确拒绝会拆开提交组的锚点。不能只让前端 sequence+1，因为 reasoning、多工具和不同消息的镜像数并不固定。恢复还需区分合法原生边界与不完整镜像，不将原始源事件静默丢弃；完整事务、失败补偿、旧日志兼容及 cursor 协议属于本地设计，复用现有 EventStore。回归应覆盖纯文字/附件/reasoning/tool call/result、旧日志、故障中断、压缩和 raw sequence 用户，检查完整 wire 与持久化一致，而不是只验证 UI 气泡。

### H-75：运行中回退没有取消或提交栅栏，已丢弃 turn 的回答仍写入新历史

**级别：高（P1）；范围：同一会话的 ChatService turn 尚在等待模型响应时，通过正式 AppService rewind_session 回退至该 turn 之前。** 这不是 H-74 的源消息/镜像边界，本项使用已完整落盘的前一组 user/assistant 作为保留点；也没有 H-73 的旧分支文件检查点参与。

- 根因与生产路径：桌面 `App.tsx::onRewindSession()` 清除本地 transcript 后直接调用 rewind，没有先请求取消；Tauri `rewind_session` 转发给 AppService。`AppService::rewind_session()` 恢复一个独立 Session、恢复检查点并执行 truncate，不检查 ChatService 持有的输入租约或活动运行。`Session::rewind()` 只重建该对象的投影，正在执行的 turn 仍持有原内存历史和原取消状态。后续 loop_engine 将 provider 原生 items 和回答通过该旧 Session 追加；底层 `session_files::append()` 读取当前 generation，cursor 不匹配时重载尾部并分配新 sequence，没有要求调用者持有回退前同一 generation 的提交资格。文件锁保障单次磁盘操作，却没有将“此 turn 仍属于当前分支”纳入提交条件。
- 动态证据：新增 `tests/kernel_v2_e2e/active_rewind.rs`。先用正式 Session 建立保留的 user/assistant；启动真实 ChatService、RunAssembler、Kernel 和 ModelAgent。transport 确认实际请求包含本轮测试用户文字后发出 Notify，然后等待另一 Notify，尚不返回回答。此时从正式 RunStore 断言运行已存在且没有 finished_at，原用户事件存在、目标回答不存在。通过正式 AppService 回退到完整保留尾部，返回 Ok，并在释放响应前确认本轮用户文字已不在真实持久化事件中。随后释放包含原生输出 item 的离线响应，原调用返回 Ok；最终用户文字仍不存在，原生/会话回答却出现在保留历史，RunStore 为 succeeded 且只有一个 run_terminal。较早保留的回答仍在。正常对照不回退，使用相同屏障和响应，原输入与回答都保留并成功终结。
- 执行：`cargo test -p deepagent-app-core --test kernel_v2_e2e active_rewind --offline -- --include-ignored --nocapture`：1 passed / 1 failed，测试体约 0.29s。实验组观测为 `accepted=true, removed_user_before_release=true, old_user_after=false, late_answer_after=true, run_ok=true, terminal=Some("succeeded")`。缺陷测试允许忙时明确拒绝，或协调终止后成功回退；它要求“回退成功丢弃的 turn 不再提交回答”，不强制选用某一种修复策略。该探针默认 ignored，显式失败记录尚未修复的生产缺陷。
- 影响：回退返回成功后，历史仍会受到已经丢弃的旧运行写入，出现没有对应用户指令的回答和旧运行成功终态；用户看到的“已回退”边界不可靠，后续恢复会读取被迟到结果污染的历史。该边界与模型供应商无关，DeepSeek 也会继承错误上下文。本轮没有执行真实文件修改工具，也没有证明回退后一定会继续发起工具/模型请求，不能把这些更大副作用当作已复现事实。未覆盖运行中 fork、排队输入、多进程、暂停/审批、压缩和真实桌面可达性，不能由本项解释宿主 Codex 的 cipher_incompatible。
- 成熟实现对照：Claude Code `restored-src/src/screens/REPL.tsx` 的消息编辑快速恢复路径明确先 `onCancel()` 再 `handleRestoreMessage(raw)`，注释直接说明 `rewindConversationTo` 的 setMessages 会与 stream append 竞争。`onCancel()` 终结 query guard 并对本地请求调用 `abortController.abort('user-cancel')`；确认弹窗通过 `MessageSelector` 的 `onPreRestore={onCancel}` 在确认恢复时触发，而取消弹窗不触发。随后 rewind 切消息数组、更新 conversationId、重置 microcompact 等派生状态。已读取这些生产函数，没有运行参考项目；这支持“恢复操作必须协调活动查询”的设计，不能推论该参考已具备本系统所需的跨进程持久化事务或完整迟到结果栅栏。Codex 的有效 turn 截断边界见 H-74，不能将它等同于本轮已查明整个 thread 回退忙态政策。
- 修复方向（尚未实施，本地设计）：在既有 AppCore/输入租约/RunCoordinator 统一管理同会话的破坏性控制操作，先选择明确的忙态拒绝或“取消并确认退出后回退”政策；控制操作与新输入接纳必须共用所有权边界，避免检查后再被新 turn 抢占。现有事件存储追加应校验运行绑定的分支身份/提交代次，回退后拒绝旧运行的迟到模型、工具和终态投影；运行审计仍保存在既有 RunStore/run_events，不能简单删除审计或只清空 UI。与 H-23/H-73 的派生状态分支身份一并收敛，保留正常顺序追加。回归覆盖请求中、工具/审批/排队中、完成竞争、取消失败及重启，检查真实日志/请求数/副作用与单终态；本轮仅增加证据，不修改生产实现。

### H-76：文件写入只验证“曾经读过”，外部已保存的改动仍被旧内容成功覆盖

**级别：高（P1）；范围：同一 ChatService 运行内通过 read_file 读取已有文件后，用户编辑器、格式化器或其他本地写入者保存该文件，随后模型使用旧版本调用 write_file 全量替换。** 不依赖多个 Agent 同时运行、保持文件长度/mtime 不变或低概率的写入瞬间竞争；本轮不是 H-52 的知识 vault 多实例去重与存储锁问题，也不是 H-35 的批量工具顺序问题。

- 根因与生产路径：AppCore `tool_runtime.rs::build_base_tool_registry()` 经 register_builtins 装配生产文件工具，`file_tools()` 为 read_file/write_file/edit_file/multi_edit 共享 FileStateCache。read_file 保存完整内容、hash、mtime 和长度；但 `WriteFileTool::invoke()` 对已有文件只调用 `cache_has_been_read()`，最终只是 `FileStateCache::has_been_read()` 的 entries.contains_key。之后读取磁盘内容仅用于旧行数统计，没有与模型实际读取版本比较；atomic_write 只保证替换文件的写入方式，不提供版本冲突检查。ToolExecutionPipeline 执行检查点捕获、权限和正式工具调用，也没有把“此前读过”升级为“当前版本仍等于所读版本”。因此外部更新不会自动撤销写资格。虽然 EditFileTool/MultiEditTool 源码也使用该布尔入口，本节动态缺陷只确认 write_file，不把未执行的编辑样本当作已复现数据丢失。
- 动态证据：新增 `tests/kernel_v2_e2e/file_version.rs`。临时 shared.txt 初始为 owner=audit-initial、value=old；真实 ChatService/ModelAgent 接收离线模型生成的 read_file 调用。第二次实际请求中的 function_call_output 必须含初始 owner，证明旧内容已经返回给模型。随后 transport fixture 作为外部编辑器执行真实 fs::write，保存更长且 owner 不同的新文件，并读取磁盘确认写入完成；该动作发生在模型发出 write_file 之前，不是延迟或并发概率注入。模型按旧内容提交整文件替换，只把 value 改为 new。生产 write_file 返回成功，下一次真实模型请求中包含其配对结果，正式 run_events 的 tool_completed.ok=true；最终磁盘 owner 恢复为 initial、外部 owner 消失，value=new，整个运行单 succeeded 终态。没有绕过生产 read cache 或直接伪造 ToolOutput。
- 对照与执行：`cargo test -p deepagent-app-core --test kernel_v2_e2e file_version --offline -- --include-ignored --nocapture`：1 passed / 1 failed，测试体约 0.66s。正常测试分别执行无外部改动、外部改动后再调用正式 read_file 两组，均写入成功；后者实际新请求包含 external owner，最终保留 external owner 且 value=new。缺陷组 `external=true, refreshed=false, write_ok=true, external_owner_preserved=false, model_value_written=true`，安全断言“拒绝过期写且保留外部内容”失败。默认忽略探针记录待修缺陷，不把正常对照通过当作该缺陷已修。
- 影响：外部工具或用户已保存的新内容可被模型基于旧版本的写操作覆盖，而系统仍向模型和运行日志报告成功。即使模型有“保留其他修改”的用户要求，仅依赖模型自行再次读取也不是可靠执行边界；对于 DeepSeek/Codex 等模型都会削弱编辑安全性。生产可能存在写前检查点，因此不将当前磁盘覆盖描述为所有备份也被永久销毁；本轮没有验收恢复此冲突文件、真实编辑器进程、真实桌面或公网模型，也未模拟文件写到一半、同 mtime/同长度或跨进程检查后写入竞争。
- 成熟实现对照：Claude Code `FileWriteTool.ts::validateInput()` 使用 readFileState 和修改时间拒绝未读/过期读；实际 call 在异步 mkdir/fileHistoryTrackEdit 之后重新同步读取当前 metadata/content，修改时间较新或缺少读取状态时再核对完整读取内容，不一致抛 FILE_UNEXPECTEDLY_MODIFIED_ERROR。其源码明确要求从检查到 writeTextContent 之间不 await，避免本进程其他异步编辑插入；Windows 修改时间变化但完整内容相同时有内容比较以避免误报。`FileEditTool.ts::call()` 也保留相同读后版本校验及同步临界段。这里已阅读参考代码，没有运行其测试；不声称该 mtime 条件覆盖同时间戳内容变更，也不能将 Node 同步区段当作跨进程原子比较与交换。借鉴的是版本身份与写资格、执行时复查，而非复制工具文案。
- 修复方向（尚未实施，本地设计）：在既有 FileStateCache/文件工具入口保留模型实际看见的版本，并在授权写入时核对当前内容身份；过期读明确返回冲突，保持磁盘内容，要求重新读取后生成操作。统一 write/edit/multi_edit 的版本语义，区分部分读取与允许替换的范围，明确工具成功写入后的新版本。现有原子替换还需与每路径操作所有权及外部变更检测配合，避免在异步检查与真正写入之间再次丢改动；跨进程事务约束为本地设计，不能声称仅加一个 mutex 即可解决所有写入者。复用既有检查点、权限和审计，不新增第二套文件工具。回归覆盖外部编辑/格式化、单文件工具之间交错、相同内容更新时间、部分读取、换行与编码、失败/取消、多个宿主及重启，并以实际磁盘和模型收到的结构化结果共同验收。

### H-77：grep 未继承敏感文件读取拒绝，普通搜索可将被禁止的内容送入模型请求

**级别：高（P1）；范围：工作区内存在本系统 fs_guard 明确禁止读取的文件，且 grep 的 query 匹配其中某行、glob 覆盖该文件。** 已确认默认递归搜索的 .env/credentials，以及显式 **/*.key 搜索的 certs/server.key；不依赖 shell、MCP、额外授权、符号链接或跳出工作区。

- 根因与生产路径：`PathGuardHook` 在正式 BeforeToolUse 阶段只检查 path/file_path/filename 三类参数；grep 接收的是 query/glob，glob 接收 pattern，因此中央 guard 不会解析它们实际遍历到的资源。`GrepTool::invoke()` 用 `walk_collect()` 从 WorkspaceRoot.path 取得候选文件，再直接 `tokio::fs::read_to_string(root.join(rel))`，没有逐候选调用 resolve_read 或 is_sensitive_path。walk_collect 只跳过部分目录和点开头的目录，不过滤敏感文件；所以根目录的 .env、非隐藏 credentials、key 文件都能进入内容扫描。工具只要求 ReadOnly 且风险 Safe，通用权限检查无法替代文件级拒绝。本系统对 Full/ReadAnywhere 模式也声明敏感文件始终禁止，不能用允许常规读取解释这个跨入口差异。
- 动态证据：新增 `tests/kernel_v2_e2e/file_search_policy.rs`。每组在独立临时工作区创建普通 safe.txt 和一个敏感文件，均只含虚构 AUDIT_SEARCH canary；先让离线模型通过真实 ChatService 发出 read_file(敏感路径)。中央 path_guard 产生唯一 tool_blocked、needs_approval=false，实际后续请求的配对结果含 sensitive 拒绝且无受保护 canary。然后同一运行让模型发出 grep；实际下一请求的 function_call_output 包含该敏感 canary，正式 run_events 中 grep 的 tool_completed.ok=true，运行单 succeeded 终态。三组分别为 `.env + 默认 glob`、`credentials + 默认 glob`、`certs/server.key + **/*.key`，均 protected_sent=true；前两组也包含正常 public canary。原生请求体在测试内解析，不打印完整请求或读取真实配置。
- 对照与执行：`cargo test -p deepagent-app-core --test kernel_v2_e2e file_search_policy --offline -- --include-ignored --nocapture`：1 passed / 1 failed，测试体约 0.85s。正常对照保持 .env 拒绝，但 grep 指定 *.txt，只发送普通文件 canary、不发送敏感 canary。默认 ignored 缺陷探针在完成三组后统一断言失败。初版 fixture 错误要求 hook 拒绝也产生 tool_completed，经实际事件摘要和 blocked() 源码核对后改为断言 tool_blocked；前两次夹具断言失败不计为产品缺陷，当前结论基于正常对照通过且内容泄漏断言失败的最终运行。
- 影响：常规搜索可以把用户以为被读取策略保护的配置、密钥或凭据行带入模型上下文；真实部署若继续调用供应商，这些内容会随模型请求上传。已验证边界是虚构敏感内容进入正式待发送请求，而不是已经向公网泄漏真实凭据。当前 grep 每行最多返回 200 字符、总 hits 有上限，这限制数量，不能使被禁止内容变成允许读取；并非每一次搜索都必定泄漏。本轮未将 glob/list_dir 仅返回文件名判为同等内容泄密，也未验证 junction、符号链接、外部目录或真实 Tauri 页面。
- 成熟实现对照：Claude Code `GrepTool.ts::checkPermissions()` 使用统一 checkReadPermissionForTool；实际 call 还从 `getFileReadIgnorePatterns(toolPermissionContext)` 获取 Read deny 规则，并在用户 glob 参数之后生成否定 rg --glob，过滤搜索候选。`utils/glob.ts` 同样使用这些 deny patterns 生成排除项，再执行文件枚举；`filesystem.ts::getFileReadIgnorePatterns()` 明确说明它用于隐藏 Read deny 拒绝的文件。该参考说明拒绝规则需要作用于隐式遍历资源，而非仅校验工具顶层参数；不声称 Claude 默认硬编码了本系统相同的敏感文件表，也没有运行参考项目或证明其覆盖所有符号链接/规则组合。
- 修复方向（尚未实施，本地设计）：在既有 WorkspaceRoot/文件访问策略内定义候选路径的读取资格，让 grep/glob/代码搜索等隐式文件发现入口复用，按当前访问模式和拒绝规则在读取内容之前过滤或明确拒绝。中央 hook 的入参校验继续存在，但不能承担它无法看见的实际文件 I/O 权限；不得只在 query/glob 字符串上查 .env，也不能先读取敏感内容再只裁 UI。目录枚举的名称可见性与内容读取应有明确契约；物理路径/junction 和运行中的策略变更需要另外验证。回归覆盖默认/显式 glob、隐藏文件、嵌套目录、敏感名称/后缀、普通可读文件、模式切换及下一 wire 内容，复用既有 registry、权限与工具结果审计，不另建搜索工具链。

### H-78：Windows 工作区目录联接绕过词法路径限制，read_file 读取根目录外文件

**级别：高（P1）；范围：Windows `WorkspaceWrite` 或默认工作区模式中，工作区内存在指向外部目录的 NTFS junction，模型向 `read_file` 提供联接下的相对路径。** 本项只确认 Windows 目录联接；没有把 Unix symlink、文件 symlink、junction 创建权限或 FullAccess 允许外部读取外推为已验证事实。

- 根因与生产路径：`WorkspaceRoot::resolve()` 拒绝 `..` 和词法 workspace 外绝对路径，然后用 `root.join(input)` 做 `starts_with(root)` 检查；它不调用 canonicalize、GetFinalPathNameByHandle 或等价的物理路径解析。`PathGuardHook` 对 `read_file` 再次调用同一个 `resolve_read`，所以中央拒绝也只看到 `workspace\linked\outside.txt`。`ReadFileTool` 之后直接对该路径执行 metadata/read_to_string，Windows junction 由文件系统跟随到 outside 目录。现有权限/审批只决定调用是否可执行，不会把联接目标纳入工作区身份。
- 动态证据：新增 Windows 专用 `tests/kernel_v2_e2e/path_boundary.rs`，用两个独立 TempDir 创建 workspace 和 outside，在 workspace 内通过真实 `cmd /C mklink /J` 建立 `linked`；junction 有 RAII 清理。outside 文件只写虚构 `AUDIT_JUNCTION_OUTSIDE_6c1e`。通过同一正式 ChatService fixture 做两个独立策略对照：WorkspaceWrite 用外部绝对路径调用 read_file，观察为硬拒绝、没有 tool_completed、配对结果无 canary；FullAccess 用同一外部文件的绝对路径调用则成功，配对结果包含 canary。然后在 WorkspaceWrite 下只向模型提供 `linked/outside.txt`，解析真实下一请求：读结果仍含 canary，RunStore 的 tool_completed.ok=true 且无 tool_blocked。由直接拒绝和 FullAccess 允许的对照确认，联接组不是测试误用了 FullAccess，也不是拒绝路径误报。该测试没有替换 PathGuard、WorkspaceRoot 或文件工具，也没有把 outside 绝对路径交给联接组的模型。
- 执行：`cargo test -p deepagent-app-core --test kernel_v2_e2e path_boundary --offline -- --include-ignored --nocapture`（Windows）：1 passed / 1 failed，测试体约 0.46s。非 ignored 权限对照通过；ignored junction 缺陷探针明确失败，观察为 `content_returned=true, tool_completed_ok=Some(true), tool_blocked=false`。由于该缺陷测试在 `cfg(windows)` 下，不能以 Linux 结果代替；本机本次就是 Windows。此前首次 probe 已产生同样泄漏信号，但本条证据以本轮加入的显式权限对照为准。
- 影响：配置为只允许工作区写入的普通会话仍可能把项目外文件内容读入 DeepSeek 请求，工作区隔离和用户对“当前项目”的理解失效。当前证据是外部临时文本进入离线待发送请求，不是已经向公网发送真实敏感文件；也没有验证通过联接写入、删除、grep 遍历或 junction 被替换后的竞态。敏感文件名过滤仍可能阻止某些目标，但不能替代物理路径边界。
- 成熟实现对照：Claude Code `fsOperations.ts::getPathsForPermissionCheck()` 明确沿 symlink 链收集中间目标与最终 realpath；`filesystem.ts` 的 `pathInAllowedWorkingPath()` 对原始及解析路径都做工作目录判断，并在权限规则上同时检查两种形式。该机制与本系统当前词法 `starts_with` 有直接差异。Codex 的 `SandboxBackend`/平台沙箱是独立执行边界，不能据此声称其文件工具在相同 junction fixture 下的具体行为；本轮没有运行参考测试。
- 修复方向（尚未实施，本地设计）：在既有 WorkspaceRoot/PathGuard 边界增加物理路径解析，按操作类型处理不存在的最终文件、已存在的父目录联接、悬空联接和 Windows 特殊路径；原始输入、每个中间目标及最终句柄路径都必须满足当前 `FsAccess`。对读取可先解析所有已存在父级，写入必须防止检查后联接替换，必要时采用不跟随联接的句柄/原子创建策略；错误需保留路径诊断而不泄漏外部内容。grep/glob/list_dir 的隐式遍历应复用同一解析策略，与 H-77 一并验收。回归覆盖正常目录、junction、file symlink、悬空目标、FullAccess 对照、Windows/Unix 及真实工具下一 wire，不能只增加字符串黑名单。

### H-79：glob/list_dir 返回被直接读取策略拒绝的敏感文件名

**级别：中（P2）；范围：工作区内存在被 `fs_guard` 禁止直接读取的 `.env`，模型调用 `glob("**")` 或 `list_dir`，并继续消费工具结果。** 本项限定为文件名/目录结构泄漏；敏感文件内容经搜索进入模型请求的独立证据见 H-77。

- 根因与生产路径：`PathGuardHook` 只从工具参数中的 `path`、`file_path`、`filename` 读取候选路径；`glob` 使用 `pattern`，因此 hook 不会看到实际匹配文件。`ListDirTool::invoke()` 直接枚举目录 entry 名称，`GlobTool::invoke()` 通过 `walk_collect()` 枚举工作区；该遍历只跳过少量构建/点开头目录，不调用 `WorkspaceRoot::resolve_read()` 或 `is_sensitive_path()`。结果因此包含 `.env`，即使同一运行中 `read_file(".env")` 已被中央策略拒绝。
- 动态证据：新增 `tests/kernel_v2_e2e/file_listing_policy.rs`。临时工作区包含虚构 `.env` 与 `visible.txt`；正式 ChatService 先发出 `read_file(".env")`，事件确认 `tool_blocked` 且结果含敏感拒绝。随后同一正式运行分别发出 `list_dir` 和 `glob("**")`，解析下一次真实序列化请求中的配对 `function_call_output`：两者都 `tool_completed.ok=true`，同时包含 `.env` 和 `visible.txt` 文件名。默认普通文件控制通过；缺陷测试显式失败，未读取真实凭据或向公网发送请求。
- 对照与执行：`cargo test -p deepagent-app-core --test kernel_v2_e2e file_listing_policy --offline -- --include-ignored --nocapture`：1 passed / 1 failed，约 0.59s；两次直接读取均被拒绝、两次发现均成功且 `hidden_name_sent=true, visible_name_sent=true`。失败断言为“拒绝的 `.env` 名称不得进入下一 wire”。`cargo fmt --all` 已执行；本批没有修改生产行为。
- 影响：模型可获知用户明确禁止读取的配置文件存在性和目录结构，进而推断环境、凭据或私有资源布局；文件名本身不等于内容泄漏，不能把本项与 H-77 合并。实际敏感内容仍需通过另一个工具路径进入请求才构成 H-77 的内容边界。本轮没有真实用户目录、真实密钥或公网传输。
- 成熟实现对照：Claude Code 的 `GlobTool.ts`、`GrepTool.ts` 均调用 `checkReadPermissionForTool()`，并通过 `getFileReadIgnorePatterns()` 将 Read deny 规则转成搜索排除模式；`utils/glob.ts` 在枚举阶段应用这些排除项。该机制说明隐式发现必须复用读取策略，而不是只检查顶层 `pattern`/`path`。本轮未运行参考项目，不声称其默认敏感文件规则与本系统完全相同。
- 修复方向（尚未实施，本地设计）：在现有 `WorkspaceRoot`/文件读取策略中提供候选路径可见性判断，让 `list_dir`、`glob`、`grep` 共用同一过滤边界；先确定“文件名可见”和“内容可读”是否同一策略，再按访问模式、显式授权和物理路径解析统一实现。过滤应发生在遍历结果生成前，不能先返回敏感名称再由 UI 删除；与 H-77、H-78 共用真实路径和重启/配置变更回归，不新增第二套搜索工具。

### H-80：Responses repair 对外部混合工具类型缺少不变量校验（条件性）

**级别：低至中（P2/P3，取决于坏历史是否可达）；范围：调用方直接把 `custom_tool_call` 与 `function_call_output`（或反向组合）以同一 `call_id` 交给公共 Responses 构造器。** 本轮没有证明正常生产会话、崩溃恢复或旧迁移会写出这种混合事件；因此不能把它描述成当前桌面必现的会话缺陷。它是一个需要在公共边界拒绝或校正的输入不变量。

- 根因与公共入口：`deepagent_models::responses::repair_tool_call_pairs()` 建立全局 `call_id → output` 映射，只验证 call_id 是否存在，没有保存每个 call 的 provider-native kind。重建时若 call 是 `CustomToolCall`，但映射中的首个 output 是 `FunctionCallOutput`，函数会原样推回错误类型；相反方向同样成立。`ResponseRequest::from_response_items()` 会调用该 repair，但 `Serialize` 直接把保留下来的 item 类型写入 wire，未再做 kind 校验或转换。
- 正常生产路径对照：`deepagent-session::tests::native_custom_tool_completion_keeps_custom_pairing` 通过 `Session::append_response_item(CustomToolCall)` 后再追加正式 `ToolCallCompleted`，实际持久化的 output 为 `CustomToolCallOutput`；现有 `appending_tool_trajectory_persists_responses_items` 也证明普通 function call 使用 `FunctionCallOutput`。因此当前证据支持“正式入口按 kind 配对”，不支持“正常生产已生成混型历史”。
- 2026-10-24 复核：重新核对 DeepSeek [Create Response 参数页](https://api-docs.deepseek.com/zh-cn/api/create-response/)中 `call_id`/配对说明与 [Responses 工具调用指南](https://api-docs.deepseek.com/zh-cn/guides/tool_calls/)，再运行上述混型探针，结果仍为 **0 passed / 1 failed（预期）**；`cargo test -p deepagent-session --offline` 为 **14 passed**，其中 custom/function 正常配对对照通过。没有新增证据证明坏数据能由正常 Session 生产路径产生，故维持条件性结论，不把该边界缺陷升级为默认生产故障。
- 动态证据：新增 `responses::tests::audit_repairs_mismatched_custom_output_kind_before_serialization`。测试通过正式 `ResponseRequest::from_response_items()` 构造 `custom_tool_call(call-patch)` + `function_call_output(call-patch)`，再序列化真实请求；断言期望 `custom_tool_call_output`，实际 wire 为 `function_call_output`，显式失败。该输入是人为构造的公共 API 边界样本，未修改生产逻辑、未调用公网供应商、未使用真实会话数据。
- 成熟实现对照：Codex `core/tests/common/responses.rs::validate_request_body_invariants()` 同时按类型收集调用和结果，明确断言每个 `custom_tool_call_output` 匹配 `custom_tool_call`，并对 function/custom 两组分别要求对称配对。该参考说明仅匹配 call_id 不足以保证 Responses 历史合法；本轮没有把 Codex 测试代码当作可直接复制的业务实现。
- 影响：如果坏历史确实通过迁移、第三方调用或损坏回放进入 DeepSeek Responses 请求，供应商可能拒绝整个下一轮输入，导致会话续聊失败、工具结果无法被模型消费或恢复链路持续携带坏历史。当前证据只证明公共构造器会接受人为混型输入并生成错误 wire；不证明正常生产会话已产生该状态，也不证明 DeepSeek 在每种部署上返回的具体 HTTP 错误码，更不把它解释为 Codex 桌面出现的 `Continuation requires complete conversation input`。
- 修复方向（尚未实施，本地设计）：repair 时为每个 call_id 保存 `Function`/`Custom` 类型，并在输出重建时强制生成匹配的 output variant；遇到同一 call_id 同时出现两种 call 类型时明确记录诊断并拒绝/选择可验证的首个类型，不能静默覆盖。恢复、snip、compaction 和旧事件迁移都应复用同一 invariant；在请求发送前增加无敏感内容的类型配对摘要和正式回归，复用现有 Responses seam，不新增第二套历史修复链。

### H-81：Responses 完成文本 item 被错误补充全局 fallback（含空 assistant）

**级别：中（P2）；触发条件：Responses 响应的完成 item 与全局文本增量状态不一致。** 已确认两种表现：同一响应生成至少两个非空 assistant message item、既有文本 delta、也有对应 `response.output_item.done` 时会重复追加整次全文；只有完整 `output_item.done` 而没有文本 delta 时会额外追加空 assistant item。单 message item 且带 delta 的对照正常。该接口使用列表承载输出，不限定为一个 message；本项没有统计真实 DeepSeek Flash/Pro 当前生成这些序列的频率，也不假设 OpenAI/Codex 特有的 phase 字段在 DeepSeek 上可用。

- 根因与生产路径：`deepagent-models/src/stream.rs::ResponseAccumulator` 把所有 `response.output_text.delta` 写进一个全局 `content`，`record_output_item` 又逐项保存 done message。`finish()` 在“全局 content 非空或还没有 fallback item”时，再为整次响应构造一个 message fallback；随后 `response_item_already_present` 只与单个已有 item 做字符串相等比较。两段不同文本都已完整保存时，拼接全文不等于任一段，因而被作为第三项追加；done-only 时全局 content 为空且还没有其他 fallback item，于是产生空 message。`Response::output_text_projection` 将重复内容拼接到最终输出，`ModelAgent` 同时把相关 item 加入 Surface/待持久化队列；RuntimeEngine 使用正式 `Session::append_response_item` 全部落盘。错误发生在正常解析/组装，不需要篡改已存历史或制造不配对工具结果。
- 可控复现：新增 `tests/kernel_v2_e2e/native_output_items.rs`，由底层离线 HttpTransport 提供完整 created、每个 item 的 added/content-part added/text delta/text done/content-part done/item done，以及带同一 output 列表与 usage 的 completed。各事件带唯一 sequence_number，item_id/output_index/content_index 对应；无缺终态、重复发送或手工修改会话事件。通过正式 ChatService→Kernel→ModelClient→ResponseAccumulator 先完成一轮，再释放全部原 ChatService/Database owner，从磁盘 `state.sqlite` 和既有文件会话存储重开，续聊时检查正式序列化 input。
- 2026-10-22 执行 `cargo test -p deepagent-app-core --test kernel_v2_e2e native_output_items --offline -- --include-ignored --nocapture`：1 passed / 2 failed，测试体约 0.42s。单 item 内容 `AUDIT_FIRST|AUDIT_SECOND`：provider_items=1 / persisted_items=1 / resumed_wire_items=1，流式与最终均 24 字节。两 item 内容分别为 `AUDIT_FIRST|`、`AUDIT_SECOND`：provider_items=2 / persisted_items=3 / resumed_wire_items=3，流式 24 字节、最终 48 字节；第三项恰为两段拼接全文，重开后的实际 wire 与错误持久化三项完全一致。done-only 内容 `AUDIT_DONE_ONLY`：provider_items=1 / persisted_items=2 / resumed_wire_items=2，流式无 delta、最终 15 字节；第二项为空字符串，且空项随重开后的实际 wire 保留。默认正常对照通过，两个缺陷探针独立 ignored 并已显式执行失败。
- 一手契约与成熟实现：本轮通过 HTTP 实际读取 [DeepSeek Responses API](https://api-docs.deepseek.com/api/create-response/)，输出定义为 item 列表，message content 为 output_text parts，SSE 示例包含 item_id/output_index/content_index。参考 Codex `codex-api/src/sse/responses.rs::parses_items_and_completed` 明确测试两个 message done 后 completed；`process_responses_event` 分别发出 OutputItemDone 与 OutputTextDelta，`core/src/session/turn.rs` 用活动 item 的身份处理展示增量，并把完成 item 交给完成路径，不在终态用全部展示文字再造一个完成 message。fixture 只采用本系统已支持的公共 message/文本事件，不使用 Codex phase、加密内容或私有字段；未运行参考项目。
- 影响：多 item 样本已证明最终保存的回答比流式展示多一份全文，且重复内容随下一轮请求保留；done-only 样本证明空 assistant item 也随恢复进入下一轮 wire。它们会增加上下文内容并可能干扰后续推理。没有公网请求、真实费用、tokenizer 或 UI 截图证据，因此不能给出实际计费增量、所有响应必然翻倍或桌面重复绘制的发生率。会话恢复忠实保留了错误结果，并未修正它。
- 修复方向（尚未实施，本地设计）：在既有 ResponseAccumulator 内按 provider item_id/output_index（及 content_index）持有增量归属；以完成 item 作为该项规范结果，只为确实缺少 done 的对应项补 fallback。不要直接禁用所有 fallback，已有 delta-only 兼容样本仍需要显式策略；也不能把全局 contains/字符串去重作为替代，否则相同文本的不同合法 item 会被误删。回归覆盖单/多 item、相同内容不同身份、工具/搜索穿插、流式展示与最终结果一致、截断/取消及持久化重开后的 wire，复用同一原生 Surface，不新增第二套历史。

### H-82：Responses 内容过滤被误判为输出到限，触发预算升级和长度续写

**级别：中（P2）；范围：DeepSeek Responses 返回 `response.incomplete`，其中 `response.incomplete_details.reason=content_filter`。** 普通 completed 不受这条原因映射影响；本项不同于 H-18 的截断工具调用提交，也不同于 H-81 的 delta/item 重复。

- 官方契约：2026-10-22 通过 HTTP 实际读取 [DeepSeek Responses API](https://api-docs.deepseek.com/api/create-response)，`incomplete_details.reason` 明确列出 `max_output_tokens` 和 `content_filter` 两个原因，流式终态为 completed/incomplete/failed。在线 web 工具两次返回检索服务 HTTP 500，随后用 PowerShell Invoke-WebRequest 成功读取官方原文并去标签核对；未从第三方推导协议字段。
- 根因及可达链：`deepagent-models/src/stream.rs::ResponseAccumulator::push_sse_data_observed` 对任何 `response.incomplete` 都设置 `FinishReason::Length`，完全不读取原因；usage 仍解析。正式 `ModelAgent::think_inner` 见 Length 后先将输出上限至少提到 65,536；再次 Length 且没有工具调用时，把部分输出加入原生 Surface，再插入 `MAX_OUTPUT_RECOVERY_PROMPT` 并重新请求。虽已有 `FinishReason::ContentFilter` 分支，此入口无法产生该值。Chat adapter 的现有测试则将 content_filter 显式报告为 provider failure，不能用该测试替代 Responses 验收。
- 动态证据：新增 `tests/kernel_v2_e2e/incomplete_reason.rs`，通过生产 ChatService→RunAssembler→Kernel→ModelClient/累加器→ModelAgent，在真实临时 SQLite 上读取 run_events。只有底层 transport 提供离线 Responses 事件：先后两次 incomplete，第三次 completed 用于有界结束；每次使用独立 response/item 身份和唯一 sequence，带实际文本 delta、item.done、output 与 usage。两组只改变 incomplete reason，不使用真实危险提示、用户历史或外部工具。
- 实测：`cargo test -p deepagent-app-core --test kernel_v2_e2e incomplete_reason --offline -- --include-ignored --nocapture` 为 1 passed / 1 failed，首次约 0.50s。长度对照和 content_filter 组均记录 3 次真实序列化请求，上限为 `[16384, 65536, 16384]`；第三次 input 已包含 `AUDIT_PARTIAL`。两组均返回 Ok，且 run_events 各只有一个 `succeeded` 终态。初版过滤探针在请求数应为 1 处失败；随后将断言收紧到“不得走预算升级/长度续写”，避免把本地不重试策略当成通用标准，并严格检查第三请求含 `Output token limit hit. Resume directly` 的实际 user item。确定性缺陷是原因丢失及错误进入长度恢复，不声称所有成熟 Agent 都禁止任何内容过滤重试。
- 成熟实现对照：Codex `codex-api/src/sse/responses.rs::process_responses_event` 读取 incomplete_details.reason，将 content_filter 映射为独立 `ApiError::ContentFilter`；`api_bridge.rs` 继续保留 `CodexErrorDetails::ContentFilter`。`core/src/responses_retry.rs::handle_response_stream_error` 在 sampling 场景写入专门的 ContentFilterGuidance，再依据独立错误和重试预算判断；不是把该原因当 Length 或提高 max_output_tokens。DeepSeek Harness 的默认 `llm-deepseek/translate.ts::stopReason` 走 Messages 协议，只把明确的 max_tokens 转为 max-tokens，其他不支持原因报 malformed；这是原因保真原则的对照，不是同一 Responses wire 实现。未执行参考项目。
- 影响：会增加不合适的重试和等待，把“输出长度不足”的错误解释写入续轮上下文；原始内容过滤原因不再到达上层分类与用户诊断。此样本第三次由离线 fixture 主动返回 completed，成功终态仅证明原原因没有成为独立运行失败，不能推断真实供应商会接受续写、过滤必被绕过或存在实际额外收费。本轮没有真实过滤请求、工具副作用、桌面截图或真实 token 账单。
- 收敛方向（尚未实施，本地设计）：在既有 provider seam 保留 incomplete 原因及已返回 usage，仅明确 max_output_tokens 进入当前长度恢复；内容过滤走独立、有限、可诊断的策略，未知原因不得默认为长度。复用现有 ModelFailure/终态事件，不另造重试器。回归覆盖两种官方原因、未知/缺失原因、部分文本、完整/截断工具调用、取消、重试上限和单终态；是否允许过滤恢复应另定显式产品策略，不能靠换提示词假装输出预算耗尽。

### H-83：最终钩子报错前已发布完成，stdio 同一 turn 收到相互冲突的两个终态

**级别：中（P2）；范围：stdio Harness 正式运行已生成答案，随后 TaskCompleted 生命周期钩子执行报错。** 本轮用已支持的用户 `hooks_json` 配置 prompt hook；主回答文本合法，但同一离线模型为 hook 返回普通文本，触发既有结构化决策解析错误。正常钩子和普通 provider error 的对照各只有一个匹配的终态。没有证明普通 provider error 会重复失败，也没有将本项外推为所有对话都会触发。

- 根因与生产路径：`deepagent-runtime/src/loop_engine.rs::RuntimeEngine::run` 在 `Complete`/`CompleteMessage` 分支先把 task 置为 Completed 并 emit `RuntimeEvent::RunCompleted`，退出循环后才 `fire_task_hook(..., HookPoint::TaskCompleted).await?`。正式 `ExternalHostHook` → `AppHookActionExecutor::execute_prompt` 的模型响应若无法由 `parse_model_hook_decision` 解析，错误穿过 HookRegistry 回到 Kernel；Kernel 随后保存唯一 `failed` 终态。`apps/cli/src/app_server.rs::spawn_turn` 的 on_event 已将早到的 RunCompleted 投影为 `turn.completed`，而 AppCore 返回 Err 后又发送 `turn.failed`。持久化 `run_terminal` 的幂等机制仍有效，实时“完成”的含义却早于实际收尾边界。
- 动态复现：新增 `apps/cli/src/app_server/terminal_audit.rs`，通过生产 `initialize`/`thread/start`/`turn/start` dispatcher，执行真实 `spawn_turn`、ChatService、RunAssembler、Kernel、prompt hook 和 ResponseRequest 序列化。只在底层使用 MockTransport，设置保存在临时磁盘 SQLite，使用 MemorySecretStore 和虚构占位凭据，没有公网请求或真实用户数据。收集正式 HarnessEvent，等待内存 active=false 后再判断，避免只看到首个通知便结束测试。没有人工 emit RunCompleted/RunFailed。
- 实测：缺陷探针实际收到 `["turn.completed", "turn.failed"]`；`thread/read` 的 durable run events 保留 `run_completed`，其后只有一个 `run_terminal=failed`。TaskCompleted 错误通过正式 live HookCompleted 事件观察，不属于 `run_events` 事件类型，探针不再错误断言它应出现在该表。钩子正常返回 `{"ok":true}`、普通 `response.failed`、两轮独立游标补读三个对照通过。缺陷探针保持 ignored 并显式执行，未修改生产行为。
- 成熟实现对照：Codex `app-server/src/bespoke_event_handling.rs::handle_error_notification` 记录错误并发送独立诊断，`handle_turn_complete` 从 turn_summary 读取 last_error，再统一调用 `emit_turn_completed_with_status` 选择 Completed/Failed；中断也通过该函数携带 Interrupted 状态。其错误诊断与 turn 完成边界有明确区分。Claude `tools/TaskUpdateTool/TaskUpdateTool.ts` 在设置 completed 前先执行 TaskCompleted hooks，并在 blockingErrors 非空时返回更新失败；`query/stopHooks.ts` 在继续/结束决策前汇集相关钩子反馈并传入 abort signal。Claude 此处是任务状态更新/teammate 收尾，不与本系统 run 协议等同；这些代码只提供边界参考，本轮未执行参考项目。
- 影响及限制：消费端若将首个 `turn.completed` 当作运行结束，可在错误通知前停止等待或开始下一步；后续 `thread/read` 又显示 failed，实时与恢复视图发生冲突。失败原因仍然可从正式 hook 错误和持久化终态获得，未发现此样本的日志缺失或重复持久化终态。此轮没有真实 stdin/stdout 背压、SDK 进程订阅、Tauri UI、钩子网络故障或公开模型请求，不能宣称这些路径都已验收。
- 收敛方向（尚未实施，本地设计）：让既有 Kernel 终态提交成为机器协议的唯一完成事实；答案生成、钩子进度和诊断继续作为非终态事件。先明确所有 final hook、usage 和必要清理的失败策略，再将最终状态投影一次，复用现有 RunStore/run_terminal 和事件体系；不在客户端靠忽略第二个通知决定结果。回归覆盖正常钩子、钩子失败/超时/取消、provider error、完成与停止竞争、数据库失败、重连补读和不同宿主入口。

### H-84：审批请求在 gate 决策前投影，造成重复发送或自动放行误报

**级别：中（P2）；范围：stdio Harness 正式 `turn/start` 的工具审批事件。** 默认 `AlwaysAsk` 下，同一次 `delete_path` 请求会向客户端发出两条 `approval.requested`：一条带真实 `approvalId`，另一条没有 ID。`AutoReview` 下，外部 `BeforeToolUse` hook 对 `read_file` 返回 `ask` 时，也会先发出一条没有 ID 的审批请求；随后默认分类器将该 `read_file` 自动放行，工具实际成功执行。后者不是重复通知，而是 gate 决策前的误报。

- 根因与生产路径：`deepagent-runtime/src/tool_pipeline.rs::ToolExecutionPipeline::request_approval` 在调用 gate 前发 `RuntimeEvent::ToolBlocked { needs_approval: true }`。`deepagent-harness-protocol/src/events.rs::project_runtime_event` 无条件把它投影成 `HarnessEvent::ApprovalRequested { approval_id: None }`。若 gate 随后进入 `ChannelApprovalGate::request`，其通知回调又会由 `apps/cli/src/app_server.rs::spawn_turn::on_approval` 投影出带 `approval_id: Some(call_id)` 的第二条事件，因此 `AlwaysAsk` 下会重复。`AutoReview` 的 `PolicyGate` 可能在调用 channel gate 前直接自动放行或拒绝；此时早发的无 ID 事件仍已到达客户端。默认分类器对 `read_file` 为 Allow，因此 hook Ask 会出现“审批提示已发出、实际没有待决审批且工具照常执行”的误报。
- 动态复现：新增 ignored 探针 `apps/cli/src/app_server/terminal_audit.rs::audit_tool_approval_emits_one_correlated_event`。它通过真实 app-server dispatcher、`spawn_turn`、ChatService、RunAssembler、Kernel、ToolPipeline 和 `ChannelApprovalGate`，用离线 Responses fixture 返回正式 `delete_path` 调用；事件经正式 `project_runtime_event`/审批回调收集。临时 SQLite 与虚构凭据均在测试目录，不访问公网。探针收到两条相同 turn/tool/reason 的 `approval.requested`，一条为 `approvalId=audit-approval-call`，另一条省略 `approvalId`；随后取消审批等待，并断言目标路径不存在。
- AutoReview 补充复现：ignored 探针 `apps/cli/src/app_server/terminal_audit.rs::audit_auto_review_hook_ask_emits_before_auto_allow` 使用外部 `PreToolUse` command hook 对 `read_file` 返回顶层 `permissionDecision=ask`，再由生产 `PolicyGate` 和默认 `SafetyClassifier` 决策。两段离线 Responses 响应确保一次工具调用后正常结束 turn。实测恰有一条 `approval.requested`，没有 `approvalId`；reason 与 hook Ask 一致；对应 `item.completed` 的工具结果 `ok=true`，且 turn 正常完成。证明通知发生在策略自动放行之前，客户端收到的不是可响应审批。
- 实测：`cargo test -p deepagent-cli terminal_audit::audit_tool_approval_emits_one_correlated_event --offline -- --ignored --nocapture` 为 **0 passed / 1 failed**，测试体约 **0.27s**；失败断言为期望 1 条、实际 2 条。默认 `terminal_audit` 对照为 **3 passed / 2 ignored**，普通文本 turn 无审批通知，普通成功、provider error 和独立游标补读通过。失败探针保持 ignored，本轮没有修改生产行为。
- 成熟实现对照：Codex `借鉴/codex/codex-rs/app-server/src/bespoke_event_handling.rs` 对 `EventMsg::ExecApprovalRequest` / `ApplyPatchApprovalRequest` 建立权限等待保护后，分别通过单一 `send_request` 发出带 thread、turn、item/call 身份的审批请求并等待对应响应；`protocol/src/approvals.rs` 保留对应 call/approval 标识。本系统也应由现有 gate 作为通知所有者，不再把一个“被阻塞”运行事件解释成另一份待审批请求。参考仅用于事件所有权和关联身份设计，不声称 Codex 没有其他审批问题。
- 影响及限制：客户端可能显示重复审批，或在工具已自动放行时显示一个无法响应的审批提示；无 ID 事件都不能与 `approval/respond` 的 pending approval 对应。本次已复现 `AlwaysAsk` 高风险调用和 `AutoReview` hook Ask/read_file 自动放行；尚未覆盖 AutoReview deny、已有 grant、子 Agent、桌面宿主及其他工具审批路径。
- 收敛方向（尚未实施，本地设计）：协议层的 `approval.requested` 只由成功注册 pending 的现有 ApprovalGate 通知回调发布，并必须携带可响应的 call ID。`ToolBlocked(needs_approval=true)` 保留为运行时诊断/工具状态，不再被映射成第二个审批请求。回归覆盖 AlwaysAsk 一请求一 ID、自动放行/拒绝不伪造人工请求、session grant 不弹窗、hook Ask、取消清理、响应只解除对应 pending call，以及 stdio/桌面共享 AppCore 后的事件一致性。

### H-85：模型目录与余额 GET 没有总时限，响应头或响应体停滞时请求可无限 pending（已修复）

**状态：已修复。级别：中（P2）；范围：模型初始化/刷新、凭据诊断和账户余额查询。** 这不是普通聊天每轮准备路径的问题，本轮没有证据表明它造成用户报告的聊天前等待。

- 修复前根因与生产路径：`deepagent-models/src/reqwest_transport.rs::ReqwestTransport::get_json` 对 `.send().await` 和随后读取的 `.text().await` 都没有 timeout，也没有取消参数。`TransportTimeouts` 只包裹 `stream`/`stream_cancelled`，不会应用到 GET。项目锁定的 reqwest 0.12.28 异步 `ClientBuilder` 文档明确写明总请求 timeout 默认关闭；本系统 `ReqwestTransport::new` 只设置 connect timeout。`deepagent-models/src/discovery.rs::ModelDiscovery::list_models` 和 `balance.rs::fetch_balance` 均经由该 GET。AppCore 调用方包括 `SettingsService::initialize`/`refresh_models`、`validate_api_key` 和 `query_balance`；桌面 `refresh_models` 的同步 command 用 `state.rt.block_on` 等待刷新，`get_balance` 则是未附加截止时间的 async command。
- 动态复现：回归 `reqwest_transport::tests::audit_get_json_is_bounded_while_waiting_for_headers_and_body` 使用本机 loopback HTTP 服务；服务分别停在响应头之前、200 成功响应体中间、503 错误响应体中间。传输配置的 first-byte、idle、total 都设为 50ms，测试等待 150ms 后记录请求是否仍未完成，再释放响应并回收任务。没有使用真实凭据、DeepSeek 网络或用户数据。
- 修复：`get_json` 和新增的 `get_json_cancelled` 统一经过正式 transport 的首字节 timeout、逐块 body idle timeout 和总 deadline；默认 GET 总时限取模型 transport `total` 与 30 秒管理请求上限的较小值，显式设置的更短预算保持不变。成功 body 限制为 16 MiB，避免管理响应无界累积；取消版复用同一取消树，现有 `HttpTransport` 实现仍可通过默认方法兼容。
- 修复后实测：`cargo test -p deepagent-models --features http reqwest_transport::tests:: --offline -- --nocapture` 中，头部停滞、成功半截 body、503 半截 body 的 50ms 预算均按时返回具体 timeout；`get_json_cancelled_interrupts_stalled_body` 在约 50ms 后设置取消，500ms 内返回 `request cancelled`。本机完整 HTTP models 测试随后通过，未访问真实供应商或用户数据。
- 成熟实现对照：DeepSeek Harness `packages/credentials/deepseek-account-platform/src/index.ts` 对账户详情请求使用 `AbortSignal.any([lifetime.signal, AbortSignal.timeout(...)])`；`requestTimeoutMs` 与 `balanceTimeoutMs` 默认均为 30 秒，凭据生命周期取消和请求时限共同生效。Codex `codex-rs/agent-identity/src/lib.rs` 对 JWKS 和 registration 请求分别设置 `.timeout(...)`。这些只作为管理类 HTTP 请求应有独立时限/取消边界的参考，不代表它们与模型流式请求的预算相同。
- 影响及限制：这消除了管理 GET 无限 pending 的 transport 缺口，并为已有生命周期拥有者提供取消入口；当前 `SettingsService` 的公开刷新/余额 API 尚未接收运行取消 token，因此它们仍受明确的 30 秒管理上限约束，而不是承诺点击停止立即中断设置操作。本轮没有运行 Tauri，不能据此确定主 UI 的具体线程占用；也没有证据解释本轮讨论的 Codex `Continuation ...` 错误，该错误按用户最新指示暂不追查。
- 后续收敛方向（本地设计）：让需要更短 UI 交互边界的 discovery、余额和 doctor 调用显式传入宿主取消 token，并在命令层投影“已超时/已取消”状态；继续复用 `HttpTransport`，不新增第二套网络客户端或取消机制。

### 后续独立核查：模型 API 跨 origin 重定向不会转发 Bearer key

- 候选风险：`ReqwestTransport::get_json` 默认跟随重定向且附带 Bearer key，需确认跳转到另一个 origin 时是否会把凭据交给目标服务器。
- 源码依据：锁定的 reqwest 0.12.28 默认重定向策略在每次 follow 前调用 `redirect::remove_sensitive_headers`；host 或有效端口变化时删除 `Authorization`、Cookie 等敏感头。DeepSeek Harness `packages/credentials/deepseek-account-platform/src/protocol.ts::fetchProtocol` 对授权请求配置 `redirect: 'error'`，会直接拒绝任何重定向，策略更严格。
- 动态验证：新增非 ignored 测试 `reqwest_transport::tests::cross_origin_redirect_does_not_forward_bearer_authorization`。本机第一个 listener 收到 `Authorization: Bearer audit-secret-placeholder` 并将请求 302 到第二个 listener；第二个 listener 收到实际跟随请求，但没有 `Authorization`，返回 JSON 后调用成功。只使用回环地址和占位 key。
- 结论与边界：该候选不构成已复现泄露，不新增 H 编号。验证覆盖模型发现/余额共用的 GET transport 与跨端口 origin；没有覆盖自定义 redirect policy 的 `with_client`、任意自定义传输实现或供应商 POST 的其他 transport 行为，不据此宣称所有 provider/网络重定向都安全。

### H-86：模型发现 GET 的错误正文可把反射的 API Key 写入 `/doctor` 会话历史（已修复）

**状态：已修复。级别：条件性低（P3）；范围：API Key 校验失败且响应正文包含授权值时。** 不能据此声称 DeepSeek 正常错误响应会回显 API Key；漏洞前置条件是供应商或错误网关的正文确实含有该值。DeepSeek 当前模型发现地址固定，不支持这里切换到任意代理 URL，故不把恶意自定义 provider 当作常规触发条件。

- 修复前根因与生产路径：`ReqwestTransport::get_json` 对非成功状态读取完整 `response.text()`，再把原文直接格式化进 `CoreError::Other`。同 crate 的流式错误路径会用 `provider_error`/`sanitize_provider_message` 对消息做清理，GET 路径没有复用。`doctor::check_api_key` 又将 transport 错误原样插入 `DiagnosticResult.detail`；正式 `/doctor` 把该 detail 编入回复，而 `slash_runtime::maybe_handle_slash_command` 将回复作为 assistant `MessageAppended` 持久化到 session event log。该会话后续恢复时可能把内容重新带入模型请求。
- 修复：GET 非成功响应改为复用结构化 `provider_error`，保留 HTTP status/code 但不原样暴露正文；provider message 统一限制长度并脱敏 Bearer 后继词、常见 `sk-` token 及已知配置 key。`doctor::check_api_key` 在诊断边界再次按当前 key 脱敏，覆盖自定义 `HttpTransport` 返回的原始错误。
- 修复后动态回归：`reqwest_transport::tests::audit_get_error_body_does_not_echo_bearer_credential` 通过本机 401 服务反射请求 Authorization，测试通过且错误不含完整占位 key；`doctor::tests::audit_api_key_check_redacts_credentials_reflected_by_get_errors` 通过正式 `check_api_key` 也通过。两项均无真实密钥、公网请求或用户数据。
- 成熟实现对照：DeepSeek Harness `packages/credentials/deepseek-account-platform/src/protocol.ts` 明确将协议错误限制为稳定错误码，不暴露响应正文；该账户平台使用不同 endpoint 和 token 类型，此处只借鉴“授权请求错误不转发原始响应正文”的错误边界。当前模型 stream 已有本地 `sanitize_provider_message`，可复用其结构化 provider message/敏感值清理方向，但需对任意 bearer 格式而不只 `sk-` 前缀验证。
- 影响及限制：已阻断本项已复现的“反射 key 进入 GET 错误和 doctor 诊断”路径；上游本身仍会收到用户主动发送的 Authorization，不能据此声称供应商侧不记录密钥。回归覆盖 provider GET 和 doctor 诊断，但没有启动完整 slash session 去验证持久化事件和下一轮 wire，只能把后两者标为代码路径上的间接保护。
- 后续收敛方向（本地设计）：继续让新诊断入口复用同一脱敏边界；若增加新的凭据类型，应补充其明确 token 形状和持久化/导出回归，不在 UI 层自行拼接原始 provider body。

### H-87：模型刷新用宿主本地旧快照覆盖另一宿主新设置

**级别：中（P2）；触发条件：同一数据目录由多个宿主打开，模型初始化/刷新与另一宿主修改设置并发。** 单进程、单 `SettingsService` 的顺序设置操作不构成本项触发条件；本轮没有做实际双桌面进程验收。

- 根因与生产路径：`SettingsService` 在每个实例内缓存完整 `AppSettings`（`settings_cache`）；`initialize` 先等待模型目录网络发现，再通过 `load()` 取得 prior 设置并整体 `save()`。`refresh_models` 复用 `initialize`。另一个独立服务/宿主在网络等待期间提交权限等设置时，当前服务仍可能用本地缓存的旧 prior 组装完整对象并覆盖共享设置文档。`DocumentStore::put` 的 upsert 直接替换 body，没有读取版本条件；SQLite 串行化单次写入不能阻止这类旧快照覆盖。`HostLock` 明确允许 Standby 对同一 SQLite 数据库继续正常读写；桌面创建各自的 `SettingsService`，`initialize_project`、`refresh_models` 和 `set_sandbox_mode` 命令均未要求 Primary，因此生产拓扑允许该竞争。
- 动态复现：ignored 探针 `settings::tests::audit_refresh_models_preserves_other_host_sandbox_mode` 使用两个分别打开同一个临时文件 SQLite 的数据库连接。第一服务先缓存 `WorkspaceWrite`，然后刷新在受控网络 transport 处暂停；第二服务将沙箱收紧为 `ReadOnly` 并收到成功 view；恢复刷新后，通过第三个新连接重新读库。探针显式以 **0 passed / 1 failed** 退出，精确实测持久化沙箱值回到 `WorkspaceWrite`。初始探针同时观察到模型选择回到 Flash，但目录刷新本身会重新 auto-select，不能据此把选型变化归因于旧缓存；最终探针仅断言与模型自动选型无关的沙箱限制。没有调用真实供应商、用户数据或真实多进程桌面。
- 成熟实现对照：Codex `core/src/session/mod.rs::update_settings_if` 在同一 Session 的状态锁内从当前配置派生并发布更新；`core/src/session/thread_settings.rs::apply_update` 与 `checkpoint_thread_settings` 另以同一个持久化信号量串行化设置更新、快照捕获和落盘。这说明设置变更与持久化快照应由共同所有者串行提交。该锁/信号量属于 Codex 的单 Session 进程内机制，并不直接解决本系统多个进程共享 SQLite 的竞争，不能把它作为跨进程修复的现成答案。
- 影响及限制：在触发竞争时，沙箱模式会被旧值覆盖；`initialize` 从 prior 携带的其他 `AppSettings` 字段同样处于该整文档覆盖边界，但本探针没有逐字段动态验证。将 `ReadOnly` 覆回 `WorkspaceWrite` 会改变新宿主/刷新宿主后续读取到的默认运行限制；运行专属配置、显式权限 profile 或独立沙箱后端仍可能另有限制，本项没有执行工具以证明完整权限提升。当前证据证明的是两个独立 SQLite 连接下的可达覆盖，不证明用户的单进程日常操作已经遇到，也没有验证实际多个 Tauri/CLI 进程间的时序与锁行为。
- 收敛方向（尚未实施，本地设计）：把已发现模型目录等刷新结果作为字段级变更应用到数据库当前值，禁止网络 await 后将旧缓存整份写回；共享 SQLite 的读改写使用可检测冲突的版本字段/条件更新或等效原子事务，并在冲突时重新基于最新文档合并/重试。进程内互斥只能减少同一宿主竞争，不能取代跨宿主数据库提交边界。回归需使用两个独立文件连接及真实 Primary/Standby 进程，覆盖刷新与模型、沙箱、permission rules、hooks 等字段并发更新、冲突重试、进程退出恢复和普通单宿主刷新。

### H-88：旧模型刷新在退出登录或换 Key 后重新写入旧凭据

**级别：中（P2）；触发条件：模型刷新已经读取旧 Key 并等待网络，另一个可写宿主随后退出登录或完成新 Key 初始化，而旧刷新较晚收到成功的目录响应。** 本项不是加密被破解，也不证明已退出的远端账户能够继续使用；只有旧 Key 在供应商侧仍有效，后续使用它的请求才可能成功。

- 根因与生产路径：`SettingsService::refresh_models` 读取当前凭据后调用 `initialize(&key)`；后者等待目录发现成功，再无条件 `secrets.set(API_KEY_NAME, api_key)`。`clear_key` 只删除存储值，没有撤销在途刷新的提交资格；新 Key 初始化也没有阻止旧刷新晚提交。该写入不依赖 `AppSettings` 缓存，因此与 H-87 不同，单纯刷新设置缓存不能解决。桌面启动时 `App.tsx` 会自动调用 `refreshModels`，退出经 `clear_api_key`，新 Key 经 `initialize_project`；共享数据目录允许 Standby 正常读写，所以不同宿主可以形成此交错。尚未证明同一 Tauri 窗口的同步 command 也能并发执行这组操作。
- 动态复现：ignored 探针 `settings::tests::audit_refresh_models_does_not_restore_a_superseded_api_key` 分别覆盖 `sign_out` 和 `replace_key`。每组用两个独立文件 SQLite 连接及正式 `SqliteSecretStore`，只有设备包装 Key 的后端使用临时 `MemorySecretStore`，不接触系统凭据。第一服务的刷新在 GateTransport 暂停；第二服务退出或用新占位 Key 成功初始化，并立即核对存储值。释放旧刷新后，第三条新连接重新解密读取。两组均得到 `refresh_succeeded=true credential_preserved=false previous_restored=true configured=true`，严格确认恢复的是旧凭据，而非仅得到任意错误值；探针以 **0 passed / 1 failed** 退出。没有调用公网或使用真实 Key。
- 成熟实现对照：DeepSeek Harness `packages/credentials/deepseek-account-platform/src/index.ts::signOut` 先 `cancelSignIn` 并等待 attempt.done，再删除凭据；已经进入 committing 的 attempt 则等待其提交完成后再删除。`startSignIn` 等待进行中的 removing；账户查询另捕获 `detailsLifetime`，凭据更新时 abort 旧 lifetime，返回前检查当前 lifetime。该实现采用账户授权流程，与本系统原始 API Key 初始化协议不同，此处只借鉴“撤销与提交共用生命周期所有者、迟到结果无发布资格”的边界；这些进程内对象不直接提供跨宿主数据库栅栏。
- 影响及限制：已成功退出的本地凭据状态会被恢复，或用户刚保存的新 Key 被旧值覆盖；新连接读到 configured=true。若旧 Key 仍有效，后续运行可能使用旧账户/凭据，影响用户对退出与计费归属的判断；本轮没有发出后续模型请求、测试供应商撤销或验证真实桌面重启。当前 UI 本地 onboarding 标记与后端状态如何表现，仍需 Tauri 验收，不能把本探针写成已自动重新进入工作台。
- 收敛方向（尚未实施，本地设计）：在现有 SettingsService/SecretStore 边界区分“目录刷新”和“用户显式更换凭据”；刷新不得因为复用初始化就重新保存捕获的旧 Key。撤销/更换与在途操作使用共同凭据代次/提交资格，网络结果提交前对照数据库当前版本；退出也必须推进代次，避免删除后重设相同字符串的 ABA 问题。宿主内取消旧等待可以减少浪费，但跨宿主仍需原子条件提交。回归覆盖退出、换 Key、同值重新登录、两次初始化乱序完成、网络失败及正常顺序操作，验证数据库凭据、配置 view 和真实后续请求所用身份一致；不新增第二套凭据存储。

### H-89：Harness 显式选择未知模型时静默回退到默认模型

**级别：中（P2）；触发条件：Harness `turn/start` 显式提供的 model 不在当前模型目录中。** 默认模型和目录内显式模型正常；本项不表示普通桌面对话已遇到此问题。

- 根因与生产路径：公开 `TurnStartRequest.model` 由 CLI app-server 优先于 thread 默认模型，并进入 `HarnessRunOverrides`。`RunConfigOverlay` 将其作为显式 `model_override` 传给 `select_run_model`；当前实现先以 `catalog.has_model(id)` 过滤，未知值被丢弃，随后无提示选择该角色的 catalog 默认值。该兼容注释针对 `~/.claude/settings.json` 的 `sonnet/opus` 别名，但 Harness 显式选择也经过同一过滤分支，没有来源区分。正式 `ChatService` 集成探针捕获模型请求序列化 body：请求 `deepseek-audit-not-in-catalog`，实际 wire 为 `deepseek-v4-flash`，没有错误且 durable terminal 为 `succeeded`。
- 动态复现与对照：`model_selection::audit_unknown_explicit_model_is_rejected_or_sent_without_substitution` 以 **0 passed / 1 failed** 退出，输出 `requested=deepseek-audit-not-in-catalog wire_models=["deepseek-v4-flash"] error_present=false terminal=succeeded`。同一夹具的默认模型与已知显式 Pro 均到达预期 wire；不支持的显式 provider 在任何模型请求前明确失败。测试使用临时文件 SQLite、正式 SettingsService/ChatService/Kernel 与离线传输，没有调用公网、真实模型或完整 stdio server。
- 成熟实现对照：DeepSeek Harness `packages/llm/llm-pi-ai/src/adapter.ts::modelOf` 对未知 provider/model 返回 `UNKNOWN_MODEL`，请求准备与流式请求共用解析；Codex `models-manager/src/model_info.rs::model_info_from_slug` 的未知 slug 回退仅补能力元数据且保留请求 slug。Codex 的 manager 接口也允许特定实现按明示策略做 provider fallback，因此这里的结论限定为：显式模型不能在没有策略提示时静默变成另一个模型。Claude 别名兼容应在来源明确的设置导入入口解析，而不是对所有显式请求做默认回退。
- 影响与限制：调用方指定的模型没有被使用，用户或自动化客户端可能误以为请求使用了指定能力，实际却以默认模型完成；预算、能力和预期响应可能不同。探针没有请求真实供应商，因此不报告实际费用、质量或账户侧日志；也未启动 CLI stdio 端到端或 Tauri 桌面。当前证据证明正式 AppCore 请求构造和持久化终态中的确定行为，不证明特定 Harness 客户端已经触发。
- 收敛方向（尚未实施，本地设计）：保留兼容别名，但只在配置来源明确的适配入口映射；Harness/CLI/SDK 的显式 model 应在本地明确拒绝未知值，或按原值发送并让 provider 决定，不能静默换成目录默认项。错误需说明请求模型及可选目录信息，并维持单一 durable failed 终态。补充 stdio `turn/start` 回归验证指定模型、未知模型、thread 默认和别名导入四条路径。

### H-90：Harness SDK 关闭 writer 后的失败请求残留 pending reply

**级别：低（P3）；触发条件：调用 `HarnessClient::close()` 后，仍保留的克隆继续调用请求 API；或关闭和请求提交发生竞争。** 正常未关闭连接的请求/响应路径不在本项范围内。

- 根因与生产路径：`HarnessClient::call_unchecked` 在写入前先把 request id 和 oneshot sender 插入共享 `pending` 表，然后才取得 stdin mutex。若 `close()` 已把共享 writer 取走，调用会在 `guard.as_mut()` 的 `None` 分支返回 `BrokenPipe`，但该分支没有从 `pending` 删除刚登记的 sender。调用方 future 返回后 receiver 被丢弃，sender 仍保存在 HashMap，直到 stdout reader 收到该 ID 的响应或 EOF 才会移除/清空。后续重复失败调用可以继续堆积。`write_all(line)` 错误路径有显式清理，但 newline 写入和 flush 的 `?` 错误路径同样未清理；本轮动态探针只验证了 writer 已关闭分支。
- 动态复现与正常基线：新增 Windows ignored 单测 `client::audit_tests::audit_request_after_close_does_not_leak_pending_reply`。使用真实 `cmd.exe` 子进程保持 stdin 管道所有者存活，构造正式 `HarnessClient`、调用其公开 `close()`，再通过公开 `thread_list()` 发请求。返回值确为 BrokenPipe，pending 长度实际为 1；期望值为 0 的探针以 **0 passed / 1 failed** 退出。子进程在测试收尾被杀掉，没有模型、文件副作用、真实 CLI server 或公网。`cargo test -p deepagent-harness-sdk --lib --offline` 的其他 4 项 wire envelope 测试通过；该测试不验证关闭期间真实 server stdout EOF 与该分支的竞态窗口。
- 成熟实现对照：Codex `app-server-client/src/remote.rs::RemoteAppServerRequestHandle::request_json_rpc` 将请求交给 worker command channel；worker 在注册 pending 后若 websocket 写入失败，会按 request id 删除该项，并在 worker 退出时为所有剩余请求发送错误。其请求所有权在连接 worker 中集中管理。本项只对照请求写失败后的 pending 清理和 worker 生命周期，不声称两者 transport 或协议完全相同。
- 影响与限制：关闭后请求仍会按预期失败，不会进入 CLI/AppCore 或调用模型；问题限于该 SDK client 实例及其克隆的待处理表，若调用方持续误用关闭后的句柄且 reader 尚未 EOF，内存会随失败次数增加。没有测量生产客户端长期内存影响，也没有证明正常 `HarnessProcess::wait()` 后仍会持续累积；server 通常会在 stdin EOF 后退出并由 reader 清表。
- 收敛方向（尚未实施，本地设计）：在共享 writer 锁内先验证句柄仍开放，再登记/发送，或在统一发送错误出口无条件按 ID 清理 pending；newline 和 flush 失败也走相同清理路径。回归覆盖 close 后请求、close 与并发调用、JSON 行已部分写入后断管、flush 失败和 reader EOF 收尾，并保持原有 id 匹配与快速响应竞态保护。

### H-91：Harness 并发事件生产可使实时 eventSequence 倒退

**级别：低（P3）；触发条件：同一 stdio 连接有多个并发事件生产者，并在某个生产者分配序号后、入队前发生调度切换。** 单生产者或分配与入队没有交错时，FIFO 通知顺序正常。本项不是持久化事件丢失。

- 根因与生产路径：`EventOutbox::emit` 先以 `AtomicU64::fetch_add` 分配序号，再调用 Tokio mpsc 的 `try_send`。原子操作只保证序号唯一，不会把序号分配和发送入队做成同一临界区；多个生产者可先后取得 1、2，却按 2、1 的顺序入队。writer 按队列顺序写出，因此真实 wire 上可能先看到 `eventSequence=2`，再看到 `eventSequence=1`。服务端 `event_ack` 明确拒绝小于已确认值的序号，消费者若逐事件确认，在确认 2 后确认迟到的 1 会得到 invalid params。
- 动态复现：ignored 探针 `app_server::tests::audit_concurrent_event_producers_keep_sequences_in_wire_order` 将生产步骤确定性地暂停在“已分配、尚未入队”边界，再模拟另一生产者完成入队。实际输出 `reserved=[1,2] enqueued=[2, 1]`，期望 `[1,2]` 的断言以 **0 passed / 1 failed** 退出。该探针精确复现了生产代码允许的交错，但没有启动完整 stdio server、真实并发线程或客户端，也没有观察事件 ack 的端到端失败。
- 成熟实现对照：Codex app-server 将连接通知交由每连接 worker/writer 串行发送；其对应实现没有本系统的 `eventSequence`/`event_ack` 契约，因此这里只能对照“单连接拥有一个 outbound writer”的结构，不能声称 Codex 直接证明了本系统序号规则。
- 影响及限制：实时消费者可能收到不单调的事件序号；使用序号作确认游标的消费者会遇到确认回退错误，或需要额外容错处理。当前证据没有表明底层 `run_events`/session 事件被删除、持久化补读丢失，亦没有证明某个 SDK 已因此漏处理事件。H-01 仍单独描述 outbox 满队列丢通知及其恢复边界，不能与本项合并。
- 收敛方向（尚未实施，本地设计）：将未编号事件送入现有有界队列，由唯一 writer 在实际串行化输出时分配 `eventSequence`；或在同一短临界区内完成序号分配与入队。保留现有 writer 和协议，不新增队列/存储。补充两个生产者的确定性交错、完整 wire 顺序及 ack 递增回归；队列满导致的丢通知和序号缺口仍由 H-01 独立验收。

### H-92：stdio EOF 直接退出运行时，活动 turn 没有有界收尾

**级别：中（P2）；触发条件：CLI Harness 的 stdio 输入在至少一个 turn 仍运行时收到 EOF。** 没有活动 turn 时 EOF 正常结束；本项只描述连接关闭时仍在执行的 turn。

- 根因与生产路径：`run_stdio` 的 `while let Some(line) = next_line().await` 在 stdin EOF 时直接返回 `Ok(())`；`spawn_turn` 使用 `tokio::spawn`，不保存或 join `JoinHandle`。CLI `main` 的 `#[tokio::main]` runtime 随主 Future 返回而关闭。项目锁定的 Tokio 1.52.3 runtime 文档说明，runtime shutdown 时 spawned async tasks 在 yield 后会被 drop，并不保证完成。`ChatService` 写唯一 durable terminal 的代码位于 run Future 正常返回之后；`ActiveRunRegistration::drop` 只移除取消映射，不替代终态提交。因此正常 EOF 没有先请求现有取消树、等待 run 收尾或报告超时的步骤。
- 动态复现：runtime 层 ignored 探针 `app_server::terminal_audit::audit_runtime_shutdown_finalizes_in_flight_stdio_turn` 使用正式 `ServerState`/`ChatService`/`spawn_turn`、临时 SQLite 与离线阻塞 transport；确认模型请求已进入 transport 后关闭 Tokio runtime，关闭后 run 仍为 `running_turn`、`finished_at=None`、`run_terminal=0`。另新增子进程管道探针 `app_server::terminal_audit::audit_stdio_eof_drops_active_turn_in_child_process`：父进程与独立子进程通过真实 stdin/stdout 管道完成 `initialize`、`thread/start`、`turn/start`；等子进程 transport 写出“模型请求已进入”的临时标记后关闭 stdin。子进程正常退出（exit code 0），但父进程重开临时 SQLite 读到 `run_state=running_turn`、`finished_at=None`、`run_terminal=0`；期望单一持久化终态的探针以 **0 passed / 1 failed** 退出。这一子进程由同 crate 测试可执行文件启动并直接调用生产 `run_stdio`，transport 是离线阻塞 fixture；没有启动发行版 `deepagent server` 二进制、Harness SDK 客户端或公网供应商。runtime 层探针第一次因测试自身在 runtime 外调用 `spawn_turn` 而失败，修正驱动后才获得产品路径证据。
- 成熟实现对照：Codex `core/src/thread_manager.rs::shutdown_all_threads_bounded` 对 manager 当前跟踪的 threads 提交 `shutdown_and_wait()`，并按完成、提交失败、超时分类；其 `thread_manager_tests.rs::shutdown_all_threads_bounded_submits_shutdown_to_every_thread` 验证所有受管 thread 都收到 shutdown。这里借鉴的是运行 owner 的显式、有界退出，不推断该 API 被 Codex 每一种 stdio 断开路径直接调用。
- 影响及限制：客户端已收到 turn start ACK，但该连接关闭会使 run 留在非终态，直到后续宿主启动恢复接手；关闭时没有该 turn 的完成/失败/中断通知。CLI 下次启动确实会执行 unfinished-run recovery，但 H-44 已单独证明其恢复终态与 session projection 仍有缺陷，本项不重复归因。探针没有证明 OS 子进程在 runtime drop 后继续运行；普通 `SystemExecutor` 配置了 `kill_on_drop(true)`，MCP 和其他子进程不由该测试覆盖。
- 收敛方向（尚未实施，本地设计）：沿用现有 RunCoordinator/取消树，为 app-server 已接纳 turn 保存任务 owner；EOF 时停止接受新请求、向每个活动 run 发出现有取消信号、在有界 deadline 内等待任务和 durable terminal，再 flush 有界 outbox。超时任务需要记录可诊断结果并交给既有恢复机制，不可静默丢弃；不能为 stdio server 新建第二套 run 或取消系统。回归需覆盖空闲 EOF、模型等待、工具执行、审批等待、多个并行 turn、终态与 EOF 竞争、outbox flush，以及恢复后的 thread/read 一致性。

### H-93：Chat Completions 路由未转换 Responses 命名工具选择的字段形状

**级别：低（P3，条件性适配器缺陷）；触发条件：调用方显式使用 `WireMode::ChatCompletions`，并为 `ResponseRequest.tool_choice` 或客户端默认值设置指定函数的对象形式。** 字符串形式 `auto`、`none`、`required` 不受该形状问题影响。当前桌面正式模型组装固定使用 Responses，因此本项不能解释默认桌面对话故障。

- 根因与生产路径：`ResponseRequest.tool_choice` 明确使用 Responses API 对象形状。`response_to_chat_completion` 将 `request.tool_choice` 原值传入 `ChatCompletionRequest`；随后 Chat request serializer 也原样写入 `tool_choice`。默认值由 `apply_chat_defaults` 同样不经协议转换地复制。DeepSeek 官方 [Chat Completions API](https://api-docs.deepseek.com/api/create-chat-completion) 指定函数形状为 `{"type":"function","function":{"name":"my_function"}}`；官方 [Responses API](https://api-docs.deepseek.com/api/create-response) 使用 `{"type":"function","name":"my_function"}`。因此命名工具选择在两种协议间需要显式映射。
- 动态复现：ignored 探针 `client::tests::audit_chat_tool_choice_translates_request_shape` 直接走 `ResponseRequest → response_to_chat_completion → JSON serializer`；`client::tests::audit_chat_tool_choice_translates_default_shape` 走配置默认值 → `apply_chat_defaults` → JSON serializer。两者均期望 Chat 形状，实际都序列化为 `{"type":"function","name":"read_file"}`，因此以 **0 passed / 2 failed** 退出。它证明适配器产生的 JSON 不符合官方 Chat 文档形状；未发送公网请求，也未实测 provider 对错误形状的具体错误响应。
- 成熟实现对照：DeepSeek Harness `packages/llm/llm-deepseek/README.zh.md` 明确声明不映射 `tool_choice`，因为它不属于其核心词汇。该实现是选择不支持，而不是证明两协议形状可以直通。本系统已有公开 Chat adapter 且暴露该字段，应在 adapter 边界转换受支持的值，或在发送前明确拒绝无法表达的 Responses-only 选项。
- 影响及限制：选定函数调用模式时，Chat wire body 包含不匹配的对象形状，可能导致请求被 provider 拒绝或不按调用方预期执行；这两种 provider 行为均未在本轮实测。桌面 `select_run_model` 强制 `WireMode::Responses`，`ModelConfig::from_catalog` 也默认 Responses；`build_chat_model_client` 同样沿用该默认。仓库搜索到的 ChatCompletions 切换点是显式公开 adapter 及测试代码，故当前证据不支持把它外推为默认桌面主链路故障。
- 收敛方向（尚未实施，本地设计）：在现有 `response_to_chat_completion` 边界将 Responses `{type:function,name}` 映射成 Chat `{type:function,function:{name}}`；字符串形式原样保留；Responses-only 的 `custom`、`web_search` 等取值若 Chat 不支持，应在本地明确拒绝，不能发出未经验证的形状。配置默认值与请求级显式值共用同一转换函数。补充序列化线级断言及受控 transport 捕获，覆盖显式对象、默认对象、字符串、unsupported 类型与 Chat 原生直接请求；再以真实 provider 仅验证已确认支持的调用选择。

### H-94：Chat 兼容投影覆盖相邻 reasoning item，只保留最后一段

**级别：低（P3，条件性适配器缺陷）；触发条件：显式使用 `WireMode::ChatCompletions`，并传入两个或更多相邻 Responses reasoning items，随后跟 assistant message 或 function call。** 单段 reasoning 和桌面默认 Responses 路由不在本项范围内。

- 根因与生产路径：`response_to_chat_completion` 通过 `messages_from_response_items` 把原生历史投影为 Chat 的 `Message`。该函数遇到每个 `ResponseItem::Reasoning` 都执行 `pending_reasoning = Some(content.clone())`；连续 item 后一项覆盖前一项，再将只剩的值放入 assistant `reasoning_content`。DeepSeek 官方 [Responses API 输入定义](https://api-docs.deepseek.com/api/create-response) 将 `reasoning` 列为可用 input item，并支持有序 item list。Chat 仅提供单个 `reasoning_content` 字段，转换应合并相邻文本或明确拒绝不能表达的历史，不能静默丢段。
- 动态复现：ignored 探针 `client::tests::audit_chat_projection_preserves_all_reasoning_items` 向正式转换函数传入 `reasoning("first ")`、`reasoning("second")`、assistant `"answer"`，再序列化 Chat JSON。断言 assistant reasoning 应为 `first second`，实际只有 `second`，探针以 **0 passed / 1 failed** 退出。仅测试本地投影/序列化，没有请求真实 provider。
- 成熟实现对照：DeepSeek Harness `packages/llm/llm-deepseek/src/translate.ts::startBlock` 将每个 native `thinking` block 建成独立 reasoning block，`deltaChunk` 保留各自 block index；`serialize.ts::assistant` 按顺序遍历所有 assistant content blocks 并逐块发出 thinking。该代码走 DeepSeek Messages，而非本系统 Chat Completions，因此只佐证成熟 Harness 保留分段 reasoning 的内部历史，不作为 Chat provider wire 格式依据。
- 影响及限制：Chat 路由的下一请求会遗漏较早的 reasoning 文本，模型收到的内部续轮上下文变短；其对最终回答、工具选择或 provider 接受性的具体影响没有经过真实模型验证。Responses 输出通常是单个 reasoning item 的路径已被覆盖，但当前类型、累加器和输入协议均允许多个 item；本项仍不能外推为用户桌面默认路径已触发。
- 收敛方向（尚未实施，本地设计）：将尚未 flush 的相邻 reasoning 文本按输入顺序追加到一个 pending buffer，并在 assistant message / tool-call grouping 处一次消费；不要跨 user/tool 边界携带。补充一段、多段、reasoning 后工具调用、reasoning 被非 assistant item 隔断及重开后相同投影的回归，并保持现有 provider-native Surface 不变。

### H-95：启动恢复把同一 session 的最新中断原因套用到所有未完成 run

**级别：低（P3，诊断分类缺陷）；触发条件：同一 session 中存在多个未完成 run，较早 run 的中断位置与较新 run 不同，且较新的 session 事件改变了全局日志尾部分类。** run 的终态处理仍按各自记录进行；本项只确认 interruption 原因没有按 run/task 隔离。

- 根因与生产路径：`AppService::recover_unfinished_runs()` 逐个处理 `RunStore::unfinished()` 返回的 run，但每次都加载该 session 的完整 `EventStore` 日志，并调用 `classify_interruption(&events)`。分类函数从 session 尾部反向扫描，签名不接收 run ID 或 `task_id`。同一日志中的 `TaskCreated` 和 `runs.task_id` 已标识各 task；正式会话投影也能将两个 run 分成独立 turn，但恢复诊断没有使用这些身份。
- 动态复现：显式 ignored 探针 `crates/deepagent-app-core/tests/recovery_terminal_audit.rs::audit_recovery_classifies_each_run_from_its_own_task_turn` 使用正式 `Session`、`RunStore`、`AppService::session_conversation()` 和 `recover_unfinished_runs()`。同一 session 中先创建停在 `ToolCallRequested` 后的旧 task/run，再创建停在新 user message 后的新 task/run；投影确认两个 run ID 都存在。恢复结果实际为 `old=Some("interrupted_prompt"), latest=Some("interrupted_prompt")`，而旧 run 应为 `interrupted_turn`，因此探针以 **0 passed / 1 failed** 退出。没有启动真实双宿主或模拟进程崩溃；此探针只验证正式恢复分类路径。
- 成熟实现对照：Codex `app-server/tests/suite/v2/thread_resume.rs::thread_resume_and_read_interrupt_incomplete_rollout_turn_when_thread_is_idle` 以具体 `turn_id` 将不完整 rollout turn 恢复为 `Interrupted`；`app-server/src/request_processors/thread_lifecycle.rs::set_thread_status_and_interrupt_stale_turns()` 仅改变仍处于 `InProgress` 的 turn。可借鉴的是以 turn 身份更新恢复状态，不是对整份 thread 历史只做一次尾部判断。
- 影响及限制：错误分类被写入 `run_recovered_after_startup` 事件并返回 `RunRecoveryDto.interruption`。当前源码搜索没有发现该字段驱动续跑、模型请求或 run/task 终态；因此已证实影响是旧 run 的恢复原因诊断可能归错，不能据此声称会改变执行行为或用户可见 UI。终态写入和任务失败处理属于既有恢复路径，本探针不评估 H-44 的终态投影问题。
- 收敛方向（尚未实施，本地设计）：在现有事件流中用明确的 run/task 身份确定各 turn 的分类范围；若需要把位于 `TaskCreated` 前的 user prompt 归属到 task，应由统一投影按现有 task/run 关系处理，不能简单截断后遗漏 prompt。补充同 session 多 run 的旧工具中断、新 prompt 中断、已完成旧 turn、无决定性事件，以及重放后身份稳定的测试；保留现有 RunStore、session event log 和恢复入口。

### H-96：定时任务已派发但落账失败，重启后会再次派发且调度循环退出

**级别：中（P2，条件性调度可靠性缺陷）；触发条件：桌面调度器发现到期任务并调用 fire 后，one-shot 删除或 recurring 的 `last_fired_at` 写入失败，之后应用重启。** 本项确认重复派发和 tick loop 退出；没有执行模型或工具副作用。

- 根因与生产路径：桌面启动时创建 `CronService` 并启动 `run_tick_loop()`。其 fire callback 对每个任务立即 `tokio::spawn` `ChatService::run_in_session()`，然后返回。`CronScheduler::poll()` 在 callback 执行后才调用 `ScheduledTaskStore::remove()` 或 `mark_fired()`；若写入错误，due task 的持久状态仍在。`run_tick_loop()` 对 `poll()` 使用 `?`，因此错误会结束循环；Tauri 调用处只记录错误并结束该后台任务，没有重试 owner。应用重启重新读取未变化的 due 文件后会再次触发。
- 动态复现：Windows ignored 探针 `crates/deepagent-runtime/tests/schedule_audit.rs::audit_post_fire_bookkeeping_failure_replays_same_occurrence` 让正式 poll 读取临时计划后，在 fire callback 中将计划文件设为只读，注入 callback 之后的持久化错误。one-shot 删除和 recurring 标记都失败，计划仍保持 due；释放权限并重建 `CronScheduler` 后，同一 `now` 再 poll，两个模式的 callback 计数都为 2。探针 **1 passed** 表示预期的重复已复现；callback 仅记录次数，没有执行 `ChatService`、模型、工具或公网请求，也没有重启桌面应用。
- 成熟实现对照：Claude Code `utils/cronScheduler.ts` 用 `inFlight` 阻止异步文件删除/`lastFiredAt` 更新尚未完成时重复处理，并在 recurring 文件持久化之前先推进进程内 `nextFireAt`；失败会记录日志并清理 in-flight 状态。该机制提供进程内去重和明确错误路径，不证明跨重启 exactly-once；本系统需要额外定义 durable occurrence 与恢复语义。
- 影响及限制：故障后当前桌面 cron tick loop 停止，直至应用重启；重启后 one-shot 仍在文件中、recurring 的 fire anchor 仍旧时，同一到期 occurrence 可重新创建会话并再次请求模型。具体费用、工具副作用和重复操作未在探针中发生或量化。该条件需要计划记账写入失败，不能外推为每轮计划都会重复。
- 收敛方向（尚未实施，本地设计）：以稳定的 task+scheduled-time occurrence identity 接入现有 AppCore 接纳与 RunStore，区分待派发、已接纳、结果未知；同一 occurrence 的重放必须关联或发现既有 run，不能因计划文件写失败而盲目再建会话。计划文件与 SQLite 不能原子提交，必须明确崩溃窗口和人工/自动恢复策略，不宣称外部动作 exactly-once。tick owner 对可恢复 poll 错误要有有界重试和可观测状态；恢复策略需同时验证失败发生在 fire 前、接纳后及标记持久化后。

### H-97：调度器停止不取消或等待已派发的定时运行

**级别：中（P2，生命周期归属缺口）；范围：CronService 已派发 ChatService run 后，scheduler shutdown 而宿主进程仍在运行的窗口。** 不覆盖宿主进程被操作系统终止后的任务恢复语义，也不说明模型供应商会在本探针中产生副作用。

#### 生产路径与可重跑复现

- `CronService::run_tick_loop()` 将 shutdown token 传给 `deepagent_runtime::schedule::run_tick_loop()`。tick loop 只在轮询边界选择 shutdown；到期 callback 在 `CronService::run_tick_loop()` 内对每条任务 `tokio::spawn(ChatService::run_in_session(...))`，不保存 join handle，也未将 scheduler shutdown 派生/连接到该 run 的取消 token。
- 桌面启动代码创建一个新的 `CancellationToken` 并仅交给 scheduler loop。没有生命周期 owner 保留该 token 来取消或等待独立的 fire 任务。CLI build_chat_service 也装配 CronService，但本结论只验证 AppCore 的 scheduler 与 run 生命周期，不外推 CLI 进程退出表现。
- 新增默认忽略探针 `cron_service::audit_tests::audit_scheduler_shutdown_leaves_started_cron_run_active`：正式 CronService 读取真实临时 `scheduled-tasks.json` 并触发到期 one-shot；正式 ChatService 使用可控 HttpTransport，等待它确实进入模型流后再停止 tick loop。模型 transport 只等待取消信号，不访问公网或执行工具。
- 执行 `cargo test -p deepagent-app-core --lib audit_scheduler_shutdown_leaves_started_cron_run_active --offline -- --ignored --nocapture`：**1 passed**，观察到 `scheduler_returned=true run_finished=false model_cancel_observed=false`。这表示 scheduler shutdown 返回后，运行仍未终结且模型取消信号未抵达。随后测试通过正式 `request_cancel(run_id)` 取消 run，并等待 durable terminal，未留下活跃测试任务。

#### 成熟实现对照、影响与边界

- Claude Code `utils/cronScheduler.ts` 暴露 `start()`/`stop()`，`useScheduledTasks.ts` 的 effect cleanup 调用 `scheduler.stop()`；CLI 在输入关闭时也停止 scheduler。它将 fire prompt 放入当前会话的队列，由该会话的 run/进程生命周期管理执行，不等价于本系统为每次 fire 独立创建 ChatService run 的模式。参考证据支持 scheduler 自身必须有显式停止边界，不能据此声称 Claude API 覆盖本系统独立 run 的取消/等待。
- 当前证据只证明 stopping tick loop 不影响已经派发的模型请求；并未关闭 Tauri 整个 runtime、杀进程、验证启动恢复或真实供应商调用。桌面宿主退出时后台 future 如何被 runtime drop 处理，以及数据库记录如何重启恢复，仍需单独验证。
- 收敛方向（尚未实施，本地设计）：为 CronService 的 fire 工作建立明确 owner。宿主 shutdown 时停止接纳新 occurrence，再按既有 RunCoordinator 取消/等待已运行项并记录终态；如果产品语义要求 fire 独立于调度器继续运行，则至少要由宿主级任务 supervisor 持有并在重启时恢复/呈现。运行 ID 与稳定 occurrence identity 关联应复用现有 RunStore，不另建任务 store。验收需覆盖 fire 前 shutdown、模型请求中、工具/审批等待、取消审计失败、正常完成竞争、真实桌面退出与重启，且一个 scheduler shutdown 不能影响其他会话或插件宿主任务。

### H-98：DeepSeek Chat Completions 的 JSON 输出模式被拒绝或静默丢弃

**级别：低（P3，条件性适配器缺陷）；触发条件：显式使用公开 `WireMode::ChatCompletions`，并通过 `ResponseRequest.text` 或客户端默认值请求 `json_object`。** 当前桌面模型组装固定使用 Responses，因此本项不覆盖默认桌面对话。DeepSeek Chat 文档支持 `text` 和 `json_object`；不把 Responses 的 JSON Schema 格式假定为 Chat 协议能力。

- 根因与生产路径：`ModelClient::stream_response_observed()` 将 Chat 路由交给 `response_to_chat_completion()`。该函数遇到任何 `request.text` 都在发送前返回 `text response format is not supported by DeepSeek Chat Completions`，没有区分官方支持的 `json_object`。若调用方将格式放在 `ModelConfig.defaults.text`，Chat 路径调用的 `apply_chat_defaults()` 不读取该字段；`ChatCompletionRequest` 与自定义 serializer 也没有 `response_format`，所以该默认值不会进入 wire，且不会报错。
- 动态复现：`client::tests::audit_chat_json_object_format_reaches_provider_request` 走请求级值 → 转换器 → serializer，实际在 transport 前被上述错误拒绝（**0 passed / 1 failed**）。`client::tests::audit_chat_default_json_object_format_reaches_provider_request` 走正式 `ModelClient::stream_response()`、Chat 默认值、受控 transport 和 wire 捕获；模型请求成功到达 transport，但 wire 的 `response_format` 为缺失（**0 passed / 1 failed**）。两个测试都没有调用公网供应商。
- 官方依据（2026-10-22 核对）：DeepSeek [Chat Completions API](https://api-docs.deepseek.com/api/create-chat-completion) 的 `response_format` 描述声明 `{ "type": "json_object" }` 开启 JSON Output，允许值为 `text`、`json_object`；该 schema 未声明 `json_schema`。JSON Output 还要求提示词指示模型输出 JSON，否则文档提示可能持续生成空白直至 token 上限。
- 成熟实现对照：Claude Code SDK 在 `entrypoints/sdk/coreSchemas.ts` 显式定义 `json_schema` 输出格式；Grok `acp_session_impl/turn.rs` 校验 schema，并根据 backend 能力选择原生 schema 或 `StructuredOutput` 工具回退。两者使用各自协议，不能据此将 JSON Schema wire 形状套到 DeepSeek Chat。DeepSeek Harness 的 `llm-deepseek` 使用 Messages 协议，本次代码搜索未发现同构 Chat `response_format` serializer，因此以 DeepSeek 官方 Chat 字段定义为本项的协议依据。可借鉴的工程边界是结构化格式需要显式校验、按 adapter 能力映射或拒绝，不能静默丢弃。
- 影响及限制：显式请求 JSON 输出的 Chat 调用会在本地失败；仅设置默认值时调用表面成功但模型没有收到 JSON 模式约束。未验证真实供应商下游表现、最终回答格式或实际 Chat 生产调用频率。`select_run_model()` 固定选择 Responses，故不外推为桌面默认请求故障。
- 收敛方向（尚未实施，本地设计）：在现有 Chat adapter 增加 provider 明确支持的 `response_format` 表达，将 `ResponseRequest.text.format.type=json_object` 和配置默认值统一解析并序列化为 DeepSeek Chat `{ "type": "json_object" }`；对 `json_schema`、未知类型或其余 Responses-only 形状在发送前给出明确的不支持错误。补充请求级、默认值、`text` 模式和不支持模式的 wire/拒绝回归，不改 Messages/Responses 协议。

### H-99：Chat Completions 静默忽略配置中的 top_logprobs 默认值

**级别：低（P3，条件性配置映射缺陷）；触发条件：显式使用 `WireMode::ChatCompletions`，且 `ModelConfig.defaults.top_logprobs` 已配置。** 请求级 `ResponseRequest.top_logprobs` 会在 Chat 转换前被明确拒绝；问题只发生在该值来自客户端默认配置时。当前桌面固定使用 Responses，不覆盖默认桌面运行。

- 根因与生产路径：Chat 分支先将 `ResponseRequest` 交给 `response_to_chat_completion()`，之后才调用 `apply_chat_defaults()`。后者没有处理 `ResponseDefaults.top_logprobs`；前者仅检查请求对象自己的 `top_logprobs`，不会读取客户端 defaults。Chat request 结构及 serializer 也没有 `logprobs` / `top_logprobs` 字段，因此默认配置被绕过校验，正常请求可继续发送。
- 动态复现：ignored 探针 `client::tests::audit_chat_top_logprobs_default_is_rejected_before_transport` 配置 `top_logprobs=5`，使用合法的离线 SSE 响应运行正式 `ModelClient::stream_response()`。审计断言要求不支持的默认值像请求级值一样在 provider transport 前拒绝；实际请求成功完成，且 transport 调用一次，因此探针以 **0 passed / 1 failed** 退出。没有向公网发送请求。
- 官方依据（2026-10-22 核对）：DeepSeek [Chat Completions API](https://api-docs.deepseek.com/api/create-chat-completion) 列出 `logprobs` 和 `top_logprobs`；`top_logprobs` 范围为 0–20，且必须同时设置 `logprobs=true`。所以不能只把 Responses 的 `top_logprobs` 字段原样塞进 Chat 请求来声称语义兼容。
- 成熟实现对照：DeepSeek Harness 的 `llm-deepseek` 是 Messages serializer，本次搜索未发现对应 Chat `logprobs` 路由；Codex 参考树里关于 OpenAI 模型移除 `top_logprobs` 的说明属于其他模型/provider，不能当作 DeepSeek Chat 限制。就本项而言，DeepSeek 官方字段依赖是唯一直接协议判据；适配器应根据完整能力映射这组参数，或明确拒绝配置。
- 影响及限制：当调用者期望该配置在 Chat 请求中生效时，系统实际不发字段且仍报告调用成功。当前响应类型也未证明能保留/暴露 token logprob 结果，因此本项不把“应当支持 logprobs”作为结论，只确认默认值被静默忽略。公开 Chat adapter 的生产使用频率和真实设置组合尚未验证。
- 收敛方向（尚未实施，本地设计）：对 defaults 和 request 先经过同一协议能力校验。第一版若 Chat adapter 尚未支持 `logprobs` 请求与返回数据，就在发送前对 `top_logprobs` 默认值明确报不支持；完整支持时则成对设置 `logprobs=true` 和范围合法的 `top_logprobs`，并验证响应信息的保留。补充请求级拒绝、默认值拒绝及完整支持时的线级形状测试。

### H-100：Harness SDK initialize 接受缺少 result/error 的 JSON-RPC response

**级别：低（P3，条件性 SDK 协议校验缺陷）；触发条件：Harness SDK 连接的服务端对 initialize 请求返回一个 JSON-RPC response object，但同时省略 `result` 和 `error`。** 正常的本系统 CLI app-server 通过 `RpcResponse::success/error` 创建响应，本项不证明默认 CLI/Tauri 服务会发出畸形响应。

- 根因与生产路径：协议 DTO 将 `RpcResponse.result` 和 `RpcResponse.error` 都表示为可选字段；`HarnessClient::call_unchecked()` 只在 `error` 存在时返回错误，否则返回 response。`initialize()` 收到该成功结果后直接设置共享 `initialized=true`。因此只含 `jsonrpc` 和 `id` 的非法 response 会被当作握手成功，后续 SDK 调用也会通过初始化门禁。
- 动态复现：Windows ignored 探针 `client::audit_tests::audit_initialize_rejects_response_without_result_or_error` 使用本机 PowerShell 子进程和真实 stdin/stdout 管道返回 `{"jsonrpc":"2.0","id":1}`，再调用正式 `HarnessClient::initialize()`。实际为 `Ok(RpcResponse { jsonrpc: "2.0", id: Number(1), result: None, error: None })` 且 `initialized=true`；要求拒绝并保持未初始化的探针以 **0 passed / 1 failed** 退出。fixture 不启动真实 CLI server、第三方服务或公网供应商。
- 协议依据（2026-10-22 核对）：[JSON-RPC 2.0 response object](https://www.jsonrpc.org/specification#response_object) 要求 response 包含 `result` 或 `error`，且必须恰好包含其中一个。成功响应必须带 `result`；错误响应必须带 `error`。
- 成熟实现对照：Codex `codex-rs/app-server-protocol/src/rpc.rs` 将 `JSONRPCResponse.result` 定义为必需字段；其 remote initialize 路径会解析握手返回的数据再完成初始化。本系统 CLI 正常构造路径使用 `RpcResponse::success/error`，目前没有证据表明它会生成该非法 envelope。因此缺陷边界是 SDK 对不符合规范的外部/替代 server 响应校验不足，不外推为默认 Harness server 的常规运行故障。
- 影响及限制：当服务端返回该畸形 envelope 时，SDK 会错误地报告初始化成功，随后请求可能继续发送并以超时或远端错误失败，令握手兼容性问题难以在边界处诊断。没有验证生产环境有服务端会返回此响应，也没有测得真实调用频率。
- 收敛方向（尚未实施，本地设计）：在现有 RPC response 解码/校验边界拒绝同时缺失 `result` 与 `error`、或同时出现二者的 envelope，并确保 initialize 只在匹配请求 ID、协议版本及有效成功结果后标记已初始化。补充非法 envelope、错误 response、正常 initialize 和错配 ID 的 SDK 回归；复用单一 protocol DTO 与既有错误类型，不增加第二套协议层。

### H-101：Harness SDK 忽略畸形 stdout 行并让请求持续 pending

**级别：低（P3，条件性 SDK 传输错误处理缺陷）；触发条件：SDK 子进程向 stdout 写出非 JSON-RPC 行，且之后不发送当前请求的有效 response、也不关闭 stdout。** 本系统 CLI 正常 stdio server 输出 JSON-RPC 行；本项针对进程输出污染、协议损坏或不兼容 server 的故障路径。

- 根因与生产路径：`spawn_reader()` 对 `decode_line()` 失败只写 `debug` 日志并继续读取，不通知对应请求，也不终止 reader。`call_unchecked()` 没有请求超时；该 request id 仍留在 `pending`，调用一直等待 oneshot，直到同 ID 的后续 response 或 stdout EOF。EOF 才统一清空 pending 并令等待方得到 `ResponseChannelClosed`。
- 动态复现：Windows ignored 探针 `client::audit_tests::audit_malformed_server_line_does_not_leave_request_pending` 使用 PowerShell 子进程读取真实 SDK 请求，向真实 stdout 管道输出 `not-json`，写入就绪标记后保持进程存活。就绪标记确认畸形行已发出，随后观测到调用仍未完成、pending 长度为 1；探针以 **0 passed / 1 failed** 退出。测试终止 fixture 后 EOF 释放等待任务，没有模型、工具或公网调用。
- 成熟实现对照：Codex `借鉴/codex/codex-rs/app-server-client/src/remote.rs` 在 `serde_json::from_str::<JSONRPCMessage>` 失败时发出 `Disconnected`，设置 `InvalidData` worker 错误并退出读取循环；worker 收尾会将该错误交付给所有 pending request。它的 remote transport 与本系统 Windows stdio 传输不同，本项只借鉴“协议解码失败必须终止并收束在途请求”的生命周期边界。
- 影响及限制：畸形输出之后若子进程仍存活，当前 SDK 调用可能永久 pending，且没有结构化协议错误可供上层展示。未证明本系统发布的 CLI 会输出非 JSON 到 stdout，也未测量外部集成的触发频率；真实 CLI 健康路径未受此探针覆盖。
- 收敛方向（尚未实施，本地设计）：沿用现有 reader 与 pending 表；遇到无法解码的 stdout 行时保存脱敏的 decode error，结束 reader 并以同一错误收束全部 pending 请求。增加畸形 JSON、未知 envelope、正常 notification/response 和 EOF 竞争回归；若产品确实允许非协议 stdout，再由 server 将诊断统一写入 stderr，而不是让 reader 猜测并跳过。

### H-102：InstructionsLoaded 外部 hook 缺少兼容输入必需字段

**级别：低（P3，条件性 hooks 兼容缺口）；触发条件：项目声明依赖 Claude Code `InstructionsLoaded` 输入字段的外部 hook，并在会话装配时成功加载指令文件。** hook 仍会被调用；本项是已触发事件的输入 DTO 不兼容，不是该生命周期事件完全未实现。

- 根因与生产路径：`RunAssembler` 以 `HookData::Instructions { paths }` 派发 `HookPoint::InstructionsLoaded`；该类型仅保存路径。`ExternalCommandHook::build_payload()` 因此只序列化 `paths`，加上公共的 `session_id`、`hook_event_name`。正式 ChatService 路径也已触发该 dispatch，但没有传递单文件路径对应的 memory type 或加载原因。
- 动态复现：Windows ignored 探针 `chat_service::tests::audit_instructions_loaded_hook_receives_required_file_metadata` 建立临时项目和 `CLAUDE.md`，将该项目标记为 hooks trusted，在 `.deepagent/hooks.json` 注册真实 PowerShell command hook，由其把 stdin 写入临时文件。运行正式 ChatService/RunAssembler 并检查 payload，实际为 `{"hook_event_name":"InstructionsLoaded","paths":[".../CLAUDE.md"],"session_id":"..."}`；断言 `file_path` 存在失败（**0 passed / 1 failed**），同样没有 `memory_type`、`load_reason`。没有读取用户配置或发起公网模型请求。
- 成熟实现对照：Claude Code `entrypoints/sdk/coreSchemas.ts::InstructionsLoadedHookInputSchema` 将 `file_path`、`memory_type`、`load_reason` 定义为必需字段，并允许 `globs`、`trigger_file_path`、`parent_file_path`；`utils/claudemd.ts` 在单个 instruction 文件加载时传递路径、memory type 和 load reason。当前系统聚合成一个 paths 数组，语义也不同；借鉴重点是保留事件所需来源元数据，不要求无条件复制 Claude 的所有记忆加载类别。
- 影响及限制：按 Claude schema 读取 `file_path` 或按 `load_reason`/`memory_type` 筛选的兼容 hook 会拿不到所需字段，可能跳过处理或由其自身报错。仅证明缺字段，没有测试某个第三方脚本的最终行为，也未测试 compaction reload、nested instructions 和条件规则分别对应何种原因。
- 收敛方向（尚未实施，本地设计）：扩展现有 `HookData::Instructions` 表达每个来源文件及本系统可确认的类型/原因；在 hook 输入 serializer 内对 Claude 兼容字段做明确映射。若本系统无法确定某个来源语义，不伪造 Claude enum，应定义版本化的本地事件字段或在兼容模式下明确拒绝不完整映射。覆盖 root instructions、nested instructions、重载、多个文件和未知来源，并验证真实 hook stdin。

### H-103：工具生命周期 hook 缺少调用 ID 与真实工具结果

**级别：低（P3，条件性 hooks 输入契约缺陷）；触发条件：配置的 `PreToolUse` 或 `PostToolUse` hook 依赖标准 `tool_use_id`，或 Post hook 需要读取工具返回内容。** hook 本身仍执行；本项与 H-26 的标准权限决策输出解析缺陷独立。

- 根因与生产路径：`ToolExecutionPipeline::prepare()` 已持有 provider `call_id`，却通过 `HookData::before_tool(name, arguments)` 派发而丢弃 ID。工具执行后虽有完整 `ToolOutput`，流水线只调用 `HookData::after_tool(name, arguments, ok)`，既丢弃 call ID，也丢弃 `output.value`；`ExternalCommandHook::build_payload()` 最终只发 `tool_name`、`tool_input`，Post hook 的 `tool_response` 被压成 `{"ok":true|false}`。
- 动态复现：Windows ignored 探针 `tool_pipeline::tests::audit_external_tool_hooks_include_call_id_and_result` 使用正式 `ToolExecutionPipeline`、`HookDefinitions::register_into()`、`SystemHookRunner` 和计数工具。调用明确携带 `id="audit-tool-use-42"`，Pre/Post 命令 hook 分别把真实 stdin 存入临时目录。实际 Pre payload 缺 `tool_use_id`；Post payload 同样缺 ID，且 `tool_response` 为 `{"ok":true}`，实际工具返回 `{"value":"audit"}`。要求保留身份及结果的探针以 **0 passed / 1 failed** 退出；没有真实模型、文件写入或公网调用。
- 失败分支动态复现：Windows ignored 探针 `tool_pipeline::tests::audit_external_tool_failure_hook_includes_call_id_and_error` 注册返回固定错误的正式 `Tool`，调用携带 `id="audit-failed-tool-use-9"`。真实 `PostToolUseFailure` command hook 收到的 payload 为 `{"hook_event_name":"PostToolUseFailure","session_id":"...","tool_input":{},"tool_name":"failure_probe","tool_response":{"ok":false}}`；`tool_use_id` 与标准 `error` 字段均缺失，断言以 **0 passed / 1 failed** 退出。无模型、公网或外部副作用。
- 成熟实现对照：Claude Code `entrypoints/sdk/coreSchemas.ts` 的 `PreToolUseHookInputSchema` 与 `PostToolUseHookInputSchema` 都要求 `tool_use_id`；成功 Post 还要求 `tool_response`。`PostToolUseFailureHookInputSchema` 同样要求 `tool_use_id`，并要求 `error` 字符串。本系统的 HookData 需要跨 runtime 事件和 Claude 兼容 serializer 传递这些值，不能只在 JSON parser 层补默认值。
- 影响及限制：需要把 hook 输入与模型原始调用对应、或基于实际工具输出做后处理的兼容脚本，拿到的身份和结果不足。探针确认字段缺失，但没有运行某个用户脚本来测量其实际误判或副作用；失败细节实测只覆盖 `ToolOutput::failure` 的固定 message。
- 收敛方向（尚未实施，本地设计）：在现有 `HookData::Tool` 增加可选 call ID 和按事件携带成功结果/失败详情，由 ToolExecutionPipeline 在执行前后传入；外部 serializer 按事件构造 `tool_use_id`、真实 `tool_response` 或 `error`，不把调用信息散落到独立全局映射。覆盖有/无 provider ID、工具成功/失败、并行工具、工具参数被 hook 修改及调用结果截断后的 payload，并保持 H-26 权限决策语义独立测试。

### H-104：Claude 兼容 hook 公共输入缺少 transcript_path 和 cwd

**级别：低（P3，条件性 hooks 基础契约缺陷）；触发条件：外部 hook 依赖 Claude Code 公共输入中的工作目录或 transcript 路径。** 当前所有命令 hook 都会执行；本项说明其 payload 不满足所宣称的 Claude Code 基础字段契约。

- 根因与生产路径：`ExternalCommandHook::build_payload()` 和 `build_http_payload()` 分别从 HookContext 自行构造输入，但共享基础部分只序列化 `session_id`、`hook_event_name`；HookContext 没有 cwd 或 transcript 路径字段。Claude `BaseHookInputSchema` 将 `session_id`、`transcript_path`、`cwd` 声明为必需字符串，`permission_mode` 为可选。
- 动态复现：Windows ignored 探针 `chat_service::tests::audit_external_hook_base_payload_includes_cwd_and_transcript_path` 在临时、已信任项目的 `.deepagent/hooks.json` 注册 SessionStart PowerShell hook，将真实 stdin 保存到临时文件。正式 ChatService/RunAssembler 执行后捕获 `{"hook_event_name":"SessionStart","session_id":"..."}`；`transcript_path` 断言失败（**0 passed / 1 failed**），`cwd` 也缺失。没有使用用户会话、磁盘 transcript 或公网模型请求。
- 成熟实现对照：Claude Code `entrypoints/sdk/coreSchemas.ts::BaseHookInputSchema` 定义这三个公共必需字段；`utils/hooks.ts::createBaseHookInput()` 从 `getTranscriptPathForSession()` 与 `getCwd()` 构造每个 hook 的基础 payload。与 H-102 的 InstructionsLoaded 专属元数据、H-103 的工具调用 ID/结果不同，本项影响所有依赖公共上下文的兼容 hook。
- 影响及限制：使用 schema 校验的 hook 会拒绝输入；依赖 cwd 或 transcript 的脚本无法按项目路径或会话记录定位工作。本轮只证明字段缺失，未运行第三方 hook validator 或证明发生了文件误读/错误写入；也没有指定本系统应公开的 transcript 文件格式或路径。
- 收敛方向（尚未实施，本地设计）：在现有 hook 调用上下文边界提供实际 cwd、可用的 transcript 引用和 permission mode；外部命令、HTTP 及 host hook 共用一份基础字段构造，避免各自复制 serializer。若本系统没有等价 transcript，应明确定义稳定、只读的兼容视图或版本化本地字段，不能伪造空路径。验证不同 workspace、subagent、无会话 transcript 和 HTTP/command hook 的公共字段一致性。

### H-105：Stop hook 缺少必需的 stop_hook_active 字段

**级别：低（P3，条件性 hooks 事件契约缺陷）；触发条件：已授权的项目配置 `Stop` 外部 hook，并在模型生成最终答复时触发 completion gate。** 未配置 `Stop` hook 的普通会话不受此项影响；这与 H-104 的所有事件公共字段缺失是不同层次的问题。

- 根因与生产路径：`RuntimeEngine::completion_gate()` 在候选答复通过 `BeforeResponse` 后，以 `HookPoint::Stop` 和 `HookData::Response { content }` 派发事件。`ExternalCommandHook::build_payload()` 将其序列化为 `content`，但没有生成 Stop 事件专属状态 `stop_hook_active`。
- 动态复现：Windows ignored 探针 `chat_service::tests::audit_stop_hook_payload_includes_stop_hook_active` 在临时且已信任的项目中注册 PowerShell `Stop` hook，执行正式 ChatService 完成一轮离线模型请求，并从命令 hook 的真实 stdin 捕获 payload。观察到 `{"content":"Hello from the agent.","hook_event_name":"Stop","session_id":"…"}`；`stop_hook_active` 不是布尔值，要求该字段存在的断言以 **0 passed / 1 failed** 退出。该探针验证的是 Stop 事件专属字段，不以 H-104 的 `cwd`/`transcript_path` 缺失重复计数。
- 成熟实现对照：Claude Code `entrypoints/sdk/coreSchemas.ts::StopHookInputSchema` 要求 `stop_hook_active: boolean`，`last_assistant_message` 为可选；`utils/hooks.ts` 构造 Stop 输入时会传递 `stop_hook_active` 和候选答复文本。本系统已有 Stop veto 流程，但兼容 payload 未携带必需的防递归状态。
- 影响及限制：对 `Stop` 输入使用 Claude schema 校验或依据 `stop_hook_active` 避免递归阻断的脚本，会收到不完整 payload，无法区分当前是否由 Stop hook 触发的重入。没有运行第三方脚本，也没有证明本系统会递归执行 Stop hook；该探针不代表默认会话故障。
- 收敛方向（尚未实施，本地设计）：在现有完成门状态中明确维护 Stop hook 重入标记，按 Claude 兼容字段传递该布尔值，并把候选答复映射到 `last_assistant_message`；若产品不支持重入，也应以稳定的 false/受控状态表达，而不是省略必需字段。通过首次 Stop、Stop hook 要求重试、重试后再次 Stop、StopFailure 及 schema 验证覆盖，确保字段语义与重入次数一致。

### H-106：StopFailure hook 缺少必需的 error 分类

**级别：低（P3，条件性 hooks 事件契约缺陷）；触发条件：已授权项目配置 `Stop` hook 拒绝候选答复，且同时配置 `StopFailure` hook。** 未配置这两个 hook 或 Stop 未拒绝时不触发。

- 根因与生产路径：`RuntimeEngine::completion_gate()` 收到 Stop hook 的 `Deny`/`Ask` 后调用 `fire_stop_failure()`；该函数将候选答复和拒绝原因拼成一段 `content` 并通过 `HookData::Response` 派发。`ExternalCommandHook::build_payload()` 只序列化 `content`，没有生成错误分类字段。
- 动态复现：Windows ignored 探针 `chat_service::tests::audit_stop_failure_hook_payload_includes_error` 通过正式 ChatService 触发真实交错：临时、已信任项目的 Stop PowerShell hook 首次以退出码 `2` 拒绝，第二次放行；StopFailure hook 将 stdin 写入临时文件。正式运行随后重试模型并成功完成。捕获 payload 为 `{"content":"Hello from the agent.\\n\\nStop failure reason: blocked by external hook","hook_event_name":"StopFailure","session_id":"…"}`；必需 `error` 缺失，断言以 **0 passed / 1 failed** 退出。全程使用离线模型 transport 和临时目录。
- 成熟实现对照：Claude Code `entrypoints/sdk/coreSchemas.ts::StopFailureHookInputSchema` 要求 `error`，类型为 `SDKAssistantMessageErrorSchema` 的枚举（`authentication_failed`、`billing_error`、`rate_limit`、`invalid_request`、`server_error`、`unknown`、`max_output_tokens`）；候选答复和 Stop hook 拒绝原因不是这些模型错误类别，不能把文本直接冒充 enum。Claude 的 hook 构造器从 API 失败上下文构造该事件。
- 影响及限制：依赖该结构化字段的兼容脚本会因 payload 校验失败，或无法区分 API 失败与 Stop hook 拒绝。本探针证明的是 Stop veto 产生的失败事件没有 typed error；没有测试 API 请求失败时是否会派发 StopFailure，也没有证明第三方脚本的实际运行结果。
- 收敛方向（尚未实施，本地设计）：先划清本系统 `StopFailure` 的触发语义。若仅表示候选答复被 hook 拒绝，应扩展事件输入保留拒绝原因，并在版本化的本地字段表达，而不是伪造 Claude API error enum；若宣称完整 Claude 兼容，则需按其实际 API failure 触发条件及错误类型构造 payload。补测 Stop veto、模型失败、重试成功/耗尽和外部 schema 校验，避免混淆 hook 决策拒绝与供应商请求失败。

### 复核但未登记：Responses `tool_search_call`

DeepSeek 官方 [Create Response](https://api-docs.deepseek.com/api/create-response) 将 `tool_search_call` 列为可能的输出 item，但同页请求工具类型只列 `function`、`custom`、`web_search`，输入 item 也不接受 `tool_search_call`；页面说明其他内置工具类型会被忽略，且只明确 `web_search` 对 `deepseek-v4-pro` 可用。官方 [Responses API Guide](https://api-docs.deepseek.com/guides/responses_api) 的事件列表也只把它列为 output item，没有列出对应生命周期事件或当前可启用条件。

本系统的 `tool_search` 是 AppCore 延迟工具注册的普通 function schema，模型以 `function_call(name="tool_search")` 调用，由现有函数调用解析和工具执行链处理。Codex 的 `tool_search_call` 则使用其独有的 `execution="client"`、结构化 `arguments` 与配对的 `tool_search_output`，不能据此把 Codex item 当成 DeepSeek 的等价协议。当前 Responses 累加器会忽略未识别的 `tool_search_call`，但本次未找到 DeepSeek 官方支持的请求配置可触发它，也未取得真实 provider 输出证据；因此不把这个差异登记为生产缺陷，保留为 provider 行为出现或官方补充启用条件后的复核项。

### H-107：续聊的每个 turn 结束都会触发 SessionEnd hook

**级别：低（P3，条件性生命周期语义缺陷）；触发条件：配置了 `SessionEnd` hook，并在同一个持久会话中完成一轮或多轮续聊。** 未配置该 hook 时不影响模型运行和任务终态。

- 根因与生产路径：`RunAssembler` 只在 `continue_session.is_none()` 时设置 `fire_session_start`；而 `RuntimeEngine::run()` 在每次 run 收尾都无条件派发 `HookPoint::SessionEnd`。同一持久 session 的第二轮仍走 `run_in_session(..., Some(session_id), ...)`，不会触发 `SessionStart`，却会再次收到 `SessionEnd`。`HookPoint` 自身文档定义为“会话结束时触发一次”，与按 turn 派发不符。
- 动态复现：Windows ignored 探针 `chat_service::tests::audit_session_end_hook_does_not_fire_after_each_continued_turn` 用正式 `ChatService::run_in_session` 对同一 session 连续执行两轮，通过真实 PowerShell command hooks 记录事件名。断言前检查 session id 相同；实际日志为 `["SessionStart", "SessionEnd", "SessionEnd"]`，期望仍打开的会话只有 `["SessionStart"]`，以 **0 passed / 1 failed** 复现。使用临时 workspace、内存 SQLite 和固定离线模型事件，没有真实用户数据或公网请求。
- 成熟实现对照：Claude Code 的 `processSessionStartHooks()` 由 startup、resume、clear、compact 等会话生命周期路径调用；`executeSessionEndHooks()` 在显式 clear 与 graceful shutdown 路径执行，不在每次 assistant turn 后执行。
- 影响及限制：为关闭对话而配置的清理、同步或汇总 hook 会在每个 assistant turn 后提前执行，并可能重复产生副作用。探针确认事件频率错误，没有运行用户配置的实际清理逻辑；宿主退出时跨多个打开会话的 hook 调度仍需单独核查。
- 收敛方向（尚未实施，本地设计）：将 `SessionEnd` 从 Kernel run 收尾移到现有会话关闭/清理或宿主关闭生命周期，并确保每个 session 最多派发一次；每 turn 的任务终态继续由 `TaskCompleted`/`RunCompleted` 表达。补测新建、续聊、显式清理、窗口关闭、取消/失败和重复关闭，验证 Start/End 配对与可重放状态一致。

### H-108：SessionStart/SessionEnd payload 缺少必需的事件元数据

**级别：低（P3，条件性 hook 输入契约缺陷）；触发条件：配置依赖标准 `SessionStart.source` 或 `SessionEnd.reason` 的外部 hook。** 不依赖这些字段的 hook 仍会被调用；本项与 H-104 的公共 `cwd`/`transcript_path` 字段缺失及 H-107 的派发频率错误分别记录。

- 根因与生产路径：RuntimeEngine 对 `SessionStart`、`SessionEnd` 都传入 `HookData::None`；`ExternalCommandHook::build_payload()` 只写公共 `session_id`、`hook_event_name`，没有事件专属来源或结束原因。
- 动态复现：Windows ignored 探针 `chat_service::tests::audit_session_lifecycle_hooks_include_required_event_metadata` 通过正式 ChatService 运行并执行两个 PowerShell command hooks，捕获的 Start 与 End payload 都只有 `hook_event_name`、`session_id`；`source` 和 `reason` 均缺失，联合断言以 **0 passed / 1 failed** 退出。使用临时 workspace、内存 SQLite 与离线 transport，没有公网调用。
- 成熟实现对照：Claude Code `SessionStartHookInputSchema` 要求 `source` 为 `startup`/`resume`/`clear`/`compact` 之一，`SessionEndHookInputSchema` 要求 `reason` 为 `clear`/`resume`/`logout`/`prompt_input_exit`/`other`/`bypass_permissions_disabled` 之一；实际构造器分别接收并序列化 source 与 reason。
- 影响及限制：校验兼容 schema 的 hook 会拒绝 payload；不校验的脚本无法按启动/恢复来源或退出原因分支。本轮只证明字段缺失，没有运行第三方校验器或用户脚本；H-107 的频率问题也意味着当前 SessionEnd 不能诚实映射成一次真实会话退出原因。
- 收敛方向（尚未实施，本地设计）：在现有 hook 生命周期边界使用有类型的 Start source / End reason 数据，不从当前任务结束推造退出原因；待 H-107 的真实会话关闭 owner 确定后再为 End 选择合法 reason。覆盖新建、恢复、清理、压缩、退出以及不触发 End 的普通续聊，并使用官方 schema 做 payload 回归。

### H-109：SubagentStop hook 缺少必需的停止状态与 transcript 路径

**级别：低（P3，条件性 hook 输入契约缺陷）；触发条件：项目或用户配置了 `SubagentStop` command hook，并有子 Agent 正常或异常结束。** 未配置该 hook 时不影响子运行结果。

- 根因与生产路径：`ChatSubagentRunner::run_tracked()` 结束子运行后只将 `agent_id`、`agent_type` 和可选 summary 放入 `HookData::Subagent`。`ExternalCommandHook::build_payload()` 对该类型只序列化这些值与 `last_assistant_message`，没有传入该子运行的 transcript 文件路径，也没有停止 hook 的重入状态。
- 动态复现：Windows ignored 探针 `subagent_runner::tests::audit_subagent_stop_hook_includes_required_fields` 经正式 `ChatSubagentRunner::run_inner()` 同时触发 Start/Stop command hooks，并从 PowerShell stdin 捕获 JSON。Start 收到 `agent_id`、`agent_type`；Stop 实际为 `{"agent_id":"…","agent_type":"review","hook_event_name":"SubagentStop","last_assistant_message":"…","session_id":"…"}`，`stop_hook_active` 与 `agent_transcript_path` 缺失，schema 断言以 **0 passed / 1 failed** 退出。使用临时目录、离线模型 transport，无公网请求。
- 成熟实现对照：Claude Code `entrypoints/sdk/coreSchemas.ts::SubagentStopHookInputSchema` 将 `stop_hook_active`、`agent_id`、`agent_transcript_path`、`agent_type` 定义为必需事件字段；`utils/hooks/hooksConfigManager.ts` 也向 hook 用户说明提供 `agent_transcript_path`。其公共 `cwd` / `transcript_path` 另属 H-104，本项只统计 SubagentStop 的专属字段。
- 影响及限制：依赖 schema 校验或读取 agent transcript 的兼容 hook 会拒绝输入或无法定位子运行记录。本探针证明字段缺失，没有运行第三方 schema validator 或用户 hook 的实际逻辑；没有据此判断 `stop_hook_active` 的正确取值语义。
- 收敛方向（尚未实施，本地设计）：让子运行完成上下文携带稳定 transcript path 与有明确定义的 Stop hook 重入状态，在现有 typed hook data / serializer 边界构造事件 payload；验证成功、失败、取消、Stop hook 要求继续及再次触发 Stop 的路径，并对照官方 schema 校验。

### H-110：PreCompact/PostCompact payload 不符合事件输入 schema

**级别：低（P3，条件性 hook 输入契约缺陷）；触发条件：配置 `PreCompact` 或 `PostCompact` command hook，且发生自动阈值或 provider overflow 压缩。** 未配置这些 hook 时不影响压缩。

- 根因与生产路径：`HookedReactiveContextCompactor` 在消息与原生 ResponseItem 压缩路径都复用 `HookData::Compact` 和固定 serializer。PreCompact 写出 `trigger` 与 `compact_summary: null`；PostCompact 写出 `trigger` 与 summary。`CompactionTrigger::as_str()` 的 `auto_compact_threshold` / `context_overflow`，以及 micro-compact 的 `micro_compact`，均不属于参考 schema 的 `manual|auto`。
- 动态复现：Windows ignored 探针 `context_runtime::tests::audit_precompact_payload_includes_required_fields` 经正式 Message compactor 捕获 PreCompact payload `{"compact_summary":null,"hook_event_name":"PreCompact","session_id":"…","trigger":"context_overflow"}`；`audit_postcompact_payload_includes_required_fields` 捕获 PostCompact payload，含真实 `compact_summary` 但仍是 `trigger=context_overflow`。两个 schema 断言均失败（合跑 **0 passed / 2 failed**）；compaction 与离线摘要请求实际执行。
- 成熟实现对照：Claude Code `entrypoints/sdk/coreSchemas.ts::PreCompactHookInputSchema` 要求 `trigger` 为 `manual|auto` 且 `custom_instructions` 存在（可为 null）；`PostCompactHookInputSchema` 同样要求 `manual|auto` 并要求字符串 `compact_summary`。`hooksConfigManager.ts` 对两事件声明同一 trigger 集合。
- 影响及限制：依赖字段校验或按触发类型过滤的 PreCompact hook 无法处理输入；PostCompact hook 的 summary 可读，但严格校验仍会因 trigger 被拒。本次动态覆盖 Message compactor 的 overflow；ResponseItem 路径在源码上使用相同 DTO，但未单独运行，也未启动第三方 schema validator。
- 收敛方向（尚未实施，本地设计）：区分 hook 协议 trigger (`auto`/`manual`) 与内部压缩原因，只在现有 hook DTO 边界映射；为 PreCompact 使用 `custom_instructions`，为 PostCompact 保留字符串 `compact_summary`。分别覆盖自动阈值、provider overflow、micro-compact、原生 Responses compaction 及手动路径（若产品提供）。

### H-111：PreCompact hook 的 stdout 指令没有进入摘要请求

**级别：中（P2，条件性 hook 功能缺陷）；触发条件：配置 `PreCompact` command hook，并从成功退出的 hook stdout 返回摘要指令。** 不返回指令或使用退出码拒绝压缩时不受此项影响。

- 根因与生产路径：外部命令结果解释器将普通文本 stdout 映射为 `HookOutcome::Continue`；`HookedReactiveContextCompactor::compact()` 对 `BeforeCompact` 只检查 `Deny`/`Ask`，随后直接调用 `summarize_zone()`，没有把 hook stdout 或修改结果传给摘要模型。
- 动态复现：Windows ignored 探针 `context_runtime::tests::audit_precompact_stdout_is_used_as_compaction_instructions` 让正式 PowerShell PreCompact hook 输出唯一标记 `AUDIT_CUSTOM_COMPACTION_INSTRUCTIONS`，随后检查 `ScriptedTransport` 捕获的真实 summary request body；标记不存在，断言以 **0 passed / 1 failed** 退出。hook 与模型请求均为本地离线执行。
- 成熟实现对照：Claude Code `utils/hooks/hooksConfigManager.ts` 对 `PreCompact` 明确规定：exit code 0 时 stdout 追加为 custom compact instructions，exit code 2 阻止压缩。本系统保留阻止路径，但未实现成功 stdout 的指令路径。
- 影响及限制：用户配置的压缩保留/强调指令会被静默忽略，摘要可能丢掉 hook 指定的重要信息。本探针确认了 compactor 发出的请求体不含指令，没有尝试证明具体摘要质量变化或真实模型输出。
- 收敛方向（尚未实施，本地设计）：为 `BeforeCompact` 明确一个有类型的 hook result，区分 veto、审批和 custom instructions；在已有 compaction pipeline 将指令纳入 summary prompt，不把通用 `Modify` 猜测性地套用到所有 hook。覆盖空 stdout、多 hook 合并、拒绝、超时、Message/ResponseItem 和取消路径。

### H-112：工作树生命周期 hook 缺少事件专属字段

**级别：低（P3，条件性 hook 输入契约缺陷）；触发条件：配置 `WorktreeCreate`、`WorktreeRemove` 或 `CwdChanged` command hook，并执行隔离子 Agent 的工作树创建、移除或工作目录切换。** 普通共享目录子运行不触发这组事件。

- 根因与生产路径：`ChatSubagentRunner::execution_root()`、`remove_worktree()` 与 `run_tracked()` 分别派发三个 hook，但统一使用 `HookData::Path`；外部 command/HTTP serializer 只生成 `path`，无法表达工作树名称、删除路径或切换前后的目录。
- 动态复现：Windows ignored 探针 `subagent_runner::tests::audit_worktree_hook_payloads_include_required_fields` 通过正式 `dispatch_child_hook()` 和真实 PowerShell command hooks 分别捕获三种 payload，实际均为 `hook_event_name`、`session_id`、`path`。`name`、`worktree_path`、`old_cwd`、`new_cwd` 联合断言以 **0 passed / 1 failed** 退出。该探针直接验证生产 dispatcher 和 serializer，传入代表性事件数据；未创建真实 Git worktree。
- 成熟实现对照：Claude Code `entrypoints/sdk/coreSchemas.ts` 的 `WorktreeCreateHookInputSchema` 要求 `name`，`WorktreeRemoveHookInputSchema` 要求 `worktree_path`，`CwdChangedHookInputSchema` 要求 `old_cwd` 与 `new_cwd`。
- 影响及限制：依赖 schema 校验、按目录切换同步状态或清理特定 worktree 的 hook 会因必需字段缺失而无法工作。本探针未测试真实 Git create/remove 的执行顺序、hook 拒绝或隔离目录路径的安全边界。
- 收敛方向（尚未实施，本地设计）：使用事件专属 typed hook data；创建事件携带稳定名称，删除事件携带实际 worktree 路径，目录切换事件在同一切换点传递旧/新绝对路径。通过真实隔离子 Agent、创建拒绝、子运行失败清理和 Windows 路径 schema 回归。

### H-113：FileChanged hook 字段名与变化类型不符合 schema

**级别：低（P3，条件性 hook 输入契约缺陷）；触发条件：配置 `FileChanged` command hook，且工具通过正式 checkpoint 变更工作区文件。** 未配置该 hook 时不影响文件写入。

- 根因与生产路径：`ToolExecutionPipeline::fire_file_changed_hooks()` 根据 checkpoint mutation evidence 派发 `HookData::FileChange`；serializer 输出 `path` / `change_kind`，且类型值是 `created`、`modified`、`deleted`。参考 schema 要求字段 `file_path` 和枚举 `event: add|change|unlink`，本系统没有相应映射。
- 动态复现：Windows ignored 探针 `tool_pipeline::tests::audit_file_changed_hook_payload_includes_required_fields` 经正式 ToolExecutionPipeline、CheckpointManager 和 `write_file` 工具创建临时文件，真实 PowerShell hook 捕获 `{"change_kind":"created","hook_event_name":"FileChanged","path":"…/created.txt","session_id":"…"}`。schema 字段及枚举断言以 **0 passed / 1 failed** 退出；没有访问用户文件或网络。
- 成熟实现对照：Claude Code `entrypoints/sdk/coreSchemas.ts::FileChangedHookInputSchema` 要求 `file_path`，`event` 只能是 `change`、`add`、`unlink`。
- 影响及限制：按文件路径或 add/change/unlink 类型处理的兼容 hook 无法通过字段校验，也不能区分创建、修改和删除。本探针动态覆盖创建；其它映射值由生产 `mutation_kind_label()` 代码确认，未逐个执行编辑和删除工具。
- 收敛方向（尚未实施，本地设计）：在既有 `MutationKind` 到 hook DTO 的唯一边界映射标准字段和值，同时保留内部事件名；以真实 write/edit/delete pipeline 和 checkpoint evidence 验证 add/change/unlink，并覆盖失败、未变化和多路径工具。

### H-114：后台子 Agent Notification hook 缺少必需的通知类别

**级别：低（P3，条件性 hook 输入契约缺陷）；触发条件：配置 `Notification` command hook，且后台子 Agent 完成并向父会话发布终态结果。** 未配置该 hook 时不影响子运行结果。

- 根因与生产路径：`ChatSubagentRunner::run_tracked()` 在后台子 Agent 终态时以 `HookData::Notification { message }` 派发通知；当前类型和 `ExternalCommandHook::build_payload()` serializer 只支持 `message`，没有通知类别字段。
- 动态复现：Windows ignored 探针 `subagent_runner::tests::audit_background_subagent_notification_includes_required_fields` 经正式 `ChatSubagentRunner::run_tracked()` 执行离线子 Agent，再由真实 PowerShell command hook 捕获 stdin。payload 为 `{"hook_event_name":"Notification","message":"background sub-agent … finished (succeeded): …","session_id":"…"}`；`message` 存在但 schema 必需的 `notification_type` 缺失，断言以 **0 passed / 1 failed** 退出。使用临时目录与离线 transport，无公网调用。
- 成熟实现对照：Claude Code `entrypoints/sdk/coreSchemas.ts::NotificationHookInputSchema` 要求 `hook_event_name="Notification"`、`message` 和 `notification_type`，`title` 可选；`utils/hooks/hooks.ts` 还按 `notification_type` 进行 matcher 匹配。
- 影响及限制：依赖 schema 校验或按通知类别筛选的兼容 hook 会拒绝该 payload，无法区分后台 Agent 完成等通知类型。本探针证明字段缺失，没有运行第三方 schema validator，也未验证 Claude 对 `notification_type` 的具体枚举约束；schema 将它定义为字符串。
- 收敛方向（尚未实施，本地设计）：在现有 typed `HookData::Notification` 中定义通知类别来源，并由唯一 serializer 输出 `notification_type`；确认各通知调用方能提供稳定类别后，再覆盖后台子 Agent 成功、失败、取消及其它通知来源，避免从 message 文本推断类别。

### H-115：PermissionDenied hook 丢失工具调用关联 ID

**级别：低（P3，条件性 hook 输入契约缺陷）；触发条件：配置 `PermissionDenied` command hook，且工具调用被权限等执行前检查拒绝。** 本次测试中的权限检查实际拒绝执行，没有发现因缺字段而绕过权限。

- 根因与生产路径：`RuntimeEngine::execute_tool_with_before()` 将 `ToolPipelineResult.call_id` 写入 `ToolCallCompleted` 和模型 Observation，但派发 `HookData::Permission` 时只保留工具名、参数、原因。当前 HookData 类型及 command/HTTP serializer 均没有 `tool_use_id`，所以原始调用 ID 无法进入 hook 输入。
- 动态复现：Windows ignored 探针 `loop_engine::tests::audit_permission_denied_hook_includes_tool_call_id` 经正式 RuntimeEngine → ToolExecutionPipeline，使用带固定 ID 的 ScriptedAgent、空权限集和带调用计数的只读工具。工具执行次数为 **0**，失败 Observation 与 SQLite 中唯一匹配的工具完成事件都保留 `audit-permission-denied-call`；真实 PowerShell hook 却只收到 `{"hook_event_name":"PermissionDenied","reason":"missing permissions: [ReadOnly]","session_id":"…","tool_input":{"path":"audit.txt"},"tool_name":"counting_read"}`。精确 ID 断言以 **0 passed / 1 failed** 退出，约 0.65s；临时目录、内存数据库，无网络或实际文件读取。
- 成熟实现对照：Claude Code `entrypoints/sdk/coreSchemas.ts::PermissionDeniedHookInputSchema` 要求 `tool_name`、`tool_input`、`tool_use_id` 和 `reason`。同文件的 `PermissionRequestHookInputSchema` 没有将调用 ID 定义为必需字段；本项不将 Request 的同类字段缺失额外登记为缺陷。
- 影响及限制：严格校验的兼容 hook 会拒绝输入；相同工具和参数被多次调用时，hook 不能可靠关联具体被拒绝的调用及其审计记录。动态验证覆盖权限能力不足的串行正式路径，没有覆盖并行批次、人工拒绝和 HTTP hook；这些生产调用方使用相同不带 ID 的 DTO，但不冒充已经逐项实测。
- 收敛方向（尚未实施，本地设计）：让现有 PermissionDenied 事件 DTO 接收流水线已确定的调用身份，保留 permission/request 与 denied 的语义差异；验证能力不足、hook 拒绝、人工拒绝、批量调用及重试中 ID 和工具结果保持一一对应，不能从工具名或参数反查身份。

### H-116：步数上限终止的失败任务仍触发 TaskCompleted hook

**级别：中（P2，条件性生命周期缺陷）；触发条件：配置 `TaskCompleted` hook，且显式设置的运行步数上限耗尽。** 默认运行没有步数上限；未配置完成 hook 时没有本项外部脚本副作用。

- 根因与生产路径：`RuntimeEngine::run()` 在循环耗尽时已将任务改为 `Failed` 并发出 `RunFailed`，随后无条件调用 `fire_task_hook(..., TaskCompleted)`。`HookData::Task` 只包含 ID 和 subject，不携带结果状态；外部 hook 无法从这个 payload 判断“失败退出”还是“成功完成”。该调用也位于预算超限和部分取消分支之后，但它们受取消派发条件影响，本次不将源码位置视为已经动态覆盖。
- 动态复现：Windows ignored 探针 `loop_engine::tests::audit_failed_task_does_not_fire_task_completed_hook` 使用正式 RuntimeEngine、Session 和真实 PowerShell command hook，运行正常完成与显式 `max_steps=1` 两组。成功对照返回 `Completed("done")`、任务 Completed 且脚本收到对应 task_id；上限组只作一次 Continue，返回 `StepLimitReached`、任务 Failed，脚本仍收到 `hook_event_name="TaskCompleted"` 及失败任务的 ID。两组状态断言均通过，“失败不触发完成 hook”断言以 **0 passed / 1 failed** 退出，约 1.09s。仅临时目录/内存 SQLite，无模型或外部服务调用。
- 成熟实现对照：Claude Code `tools/TaskUpdateTool/TaskUpdateTool.ts` 在 `status !== existingTask.status` 且 `status === 'completed'` 时调用 `executeTaskCompletedHooks`，并在无 blockingErrors 后才写入 completed；`utils/hooks.ts` 将其定义为任务将被标记完成时的事件。本系统的 kernel task 与 Claude 的任务清单不是同一对象，借鉴的是按真实状态转换选择事件的边界，不将两者 ID 或存储强行等同。
- 影响及限制：按“任务完成”触发通知、归档或后续操作的脚本可能在任务失败时启动。探针没有部署、发送通知或产生真实外部副作用；也没有证明普通 provider error、用户取消或默认无步数上限的运行会触发相同行为。H-83 是完成 hook 报错后的实时/持久化终态冲突，本项是 hook 被派发给已失败任务，两者应分别验收。
- 收敛方向（尚未实施，本地设计）：在既有 RuntimeEngine/Kernel 收尾边界按真实任务结果定义完成事件，仅成功转换走 TaskCompleted；若需要通用退出通知，应明确现有事件的状态语义及兼容策略，不能继续借完成事件表示任意退出。覆盖成功、显式步数/费用上限、取消、模型失败与 hook 失败，同时核对 task state、规范终态和实际脚本调用数。

### H-117：API 错误终止未派发 StopFailure，反而只在完成被拒绝时触发

**级别：中（P2，条件性生命周期缺陷）；触发条件：配置 `StopFailure` hook，并依赖它处理结束本轮的模型 API 错误。** 未配置该 hook 时不改变模型错误本身及规范终态。

- 根因与生产路径：`RuntimeEngine::run()` 对模型思考返回的普通错误直接 `return Err(e)`；Kernel 负责最终失败落盘，但没有派发 StopFailure。全仓库的生产 StopFailure 派发点仅在 `completion_gate()` → `fire_stop_failure()`，当 BeforeResponse/Stop 返回 Deny 或 Ask 时调用。这是把“完成 gate 要求继续”与“API 错误结束”映射到了同一个事件名。
- 动态复现：Windows ignored 探针 `chat_service::tests::audit_provider_error_dispatches_stop_failure_hook` 经正式 ChatService → ModelClient/Responses accumulator → RuntimeEngine/Kernel，配置实际 SessionStart、Stop、StopFailure PowerShell command hooks。成功对照终态为 succeeded、事件 `SessionStart → Stop`；供应商失败组输入 `response.failed` / `invalid_request_error` / `audit invalid request`，调用方收到对应错误，数据库终态为 failed 且 `run_terminal` 恰好一条，但实际 hook 只有 `SessionStart`。两组已捕获 payload 的 session_id 均对应各自 run；生命周期断言以 **0 passed / 1 failed** 退出，约 1.89s。使用临时目录、内存 SQLite、离线 MockTransport，无公网或真实供应商故障。
- 成熟实现对照：Claude Code `utils/hooks/hooksConfigManager.ts` 明确把 StopFailure 定义为 API 错误（限流、鉴权等）结束 turn 时替代 Stop 的通知，输出与退出码被忽略；`query.ts` 在错误恢复耗尽并退出前调用 `executeStopFailureHooks`，`utils/hooks.ts` 从 API error message 构造 `error` / `error_details`。不把 Claude 的内部错误枚举直接套到 DeepSeek，本系统仍需在自己的 provider seam 映射分类。
- 影响及限制：依赖该事件的失败通知、诊断收集和外部自动化在本次真实 API 失败路径没有运行；H-106 已动态证明正常回答被 Stop 拒绝后反而派发 StopFailure，可能导致误报。H-106 的 payload 缺 `error` 与本项触发时机错误是两条独立验收项。本探针未验证限流重试耗尽、鉴权 HTTP 状态、取消或进程总时限，也没有证明数据库把 API 失败写成成功——本次失败落盘正确。
- 收敛方向（尚未实施，本地设计）：在既有运行失败收尾入口按最终 provider 错误原因派发一次有类型的 StopFailure；可恢复重试和 Stop 要求继续不触发终结性失败通知。明确 hook 的时限、取消和异常处理不得破坏原失败及单一终态，验证错误恢复成功、恢复耗尽、鉴权/限流、普通成功及 Stop 拒绝重试。

### H-118：Responses 流式失败丢弃供应商错误码，上下文溢出恢复依赖消息措辞（已修复）

**状态：已修复。级别：中（P2，条件性模型恢复缺陷）；触发条件：供应商通过 `response.failed` 返回本系统已支持的 `context_length_exceeded` code，但 message 没有现有分类器识别的英文溢出关键词。** HTTP 错误和包含关键词的消息不能用来替代该事件路径验收。

- 根因与生产路径：`deepagent-models/src/stream.rs::ResponseAccumulator::push_sse_data_observed` 处理 `response.failed` 时只取 `response.error.message`，把真实 code 固定替换为 `response_failed`。`ModelClient` 原样向上返回该 CoreError；`classify_model_error` 本可单凭 `context_length_exceeded` 识别 ContextOverflow，但元数据丢失后只能匹配 message，最终返回 Unknown。`ModelAgent::think_inner` 的工具结果剪枝与原生压缩恢复只在 ContextOverflow 分支进入，Unknown 直接返回失败。
- 修复前动态复现：ignored 探针 `model_agent::tests::audit_streamed_overflow_recovers_from_provider_code_without_message_keywords` 用同一 `response.failed` / `context_length_exceeded` fixture 仅替换 message，经正式 ModelClient/ResponseAccumulator/ModelAgent 执行。英文 `maximum context window exceeded` 对照实际请求 **2 次、压缩 1 次**，第二次请求的 input 从超过 3 项缩到 2 项并正常回答；中文 `输入超出限制，请缩短内容` 组实际请求 **1 次、压缩 0 次**，返回 `Provider { status: None, code: Some("response_failed"), ... }`，分类为 Unknown，原历史保持未修改。两组先验证完整 code 可被当前分类器正确识别；最终断言以 **0 passed / 1 failed** 复现。摘要为既有计数型 fixture，模型传输离线，没有真实超长请求或公网供应商错误。
- 修复实现：在 `deepagent-models` 内新增 HTTP body 与流内失败共用的 provider error 归一化边界；优先保留实际 `error.code`，缺失时读取 `error.type`，两者都缺失才使用调用方明确提供的 fallback。HTTP 状态、`Retry-After` 与 request ID 仍只由 HTTP transport 提供，流内错误不伪造这些字段；message 继续经过同一 Bearer/API Key 脱敏和 Unicode 安全长度限制。`response.failed` 直接把 `response.error` 交给该边界，不在 Runtime 或 UI 增加本地化关键词特判。
- 修复后动态验证：同一探针移除 ignored 并成为常规回归；英文关键词对照与中文 code-only 组均为 **请求 2 次、压缩 1 次、最终成功**，第二次请求继续验证为压缩后的 2 项 input。精确命令 `cargo test -p deepagent-runtime --lib model_agent::tests::audit_streamed_overflow_recovers_from_provider_code_without_message_keywords --offline -- --exact --nocapture` 为 **1 passed / 0 failed**。`deepagent-models` 共用归一化单测另验证真实 code 保留及消息脱敏。
- 官方与成熟实现对照：DeepSeek 官方 [Create Response](https://api-docs.deepseek.com/api/create-response/) 定义失败 response 的 `error.code` / `error.message`，[Responses 指南](https://api-docs.deepseek.com/guides/responses_api/) 明确 `response.failed` 携带包含 error 的完整 response；该页未列具体 code 枚举，本项不声称 DeepSeek 必然以该 code 返回每次溢出。Codex `codex-api/src/sse/responses_error.rs::parse_failed_response` 按实际 code 将 `context_length_exceeded` 映射到 ContextWindowExceeded；DeepSeek Harness `llm-deepseek/src/transport.ts::providerError` 同时保留 type/code/message 的分类信息，后者是 Messages 协议，仅参考错误归一化边界，不复制其事件结构。
- 影响及限制：已消除本项中“流内真实 code 被固定 fallback 覆盖”的分类缺口；具有已支持机器错误码的流式上下文溢出不再依赖英文 message 才能进入现有压缩恢复。本次只动态验证 ContextOverflow，没有把限流、鉴权、过载的分类/重试/备用模型效果一并视为已验证，也没有实测供应商何时在 HTTP 与流内错误之间选择。缺失或未知 code 仍按明确 fallback/既有 Unknown 语义处理；取消和压缩后再次失败的组合边界继续由后续审计覆盖。

### H-119：视频状态 HTTP 等待不响应运行取消，停止已受理仍需等待供应商返回（已修复）

**级别：高（P1，长任务可用性）；范围：画布 `video-gen` 已提交云任务、正在等待 `/video/status` 响应。** 与 H-29 的聊天错误正文、H-31 的摘要等待不同，本项通过正式画布运行和成功形状的真实 HTTP 网关定位等待边界；不是 UI 没有发送停止信号。

- 生产路径（修复前）：`run_assembler::run_workflow_inner` 给 WorkflowAgent 注入现有取消树的 flag；`execute_video` 将该 flag 交给 CanvasVideoRequest。`CanvasModelGateway::generate_video` 只在每轮 poll **之前**检查 flag，随后直接 await `poll_media_job(..., 60_000)`；`post_json` 的请求/正文读取没有取消选择。WorkflowAgent 只实现 `Agent::think`，继承的 `think_cancelled` 也是启动前检查；RuntimeEngine await 此决策，没有另外中断正在等待的节点。故信号已经到达仍不能打断此 HTTP 等待。
- 可重跑探针：`cargo test -p deepagent-app-core --test workflow_audit media:: --offline -- --include-ignored --nocapture`，本次 **1 passed / 1 failed**，约 0.48s。新增 `tests/workflow_audit/media.rs` 复用生产 SettingsService、CanvasProviderService、CanvasModelGateway、ChatService、Kernel、取消树、文件 SQLite 与 artifact 存储；只有供应商为本机 loopback HTTP。必须收到真实 `POST /v1/video/status` 后才调用 `request_cancel`，保持响应 pending 观察 300ms，再释放 `Failed` 响应并等待、回收工作流与服务器任务；最终断言在清理后执行。
- 实际取消组：`accepted=true, signal_ms=0, stopped_within_300ms=false, unfinished_before_response=true`，从取消到退出合计约 316ms（含 300ms 的响应前观察）。最初测试输出将这个总耗时误标为 `exit_after_release_ms`，后更正为 `cancel_to_exit_ms`，并非放行响应后还等了 316ms。释放响应后得到唯一规范 `cancelled` 终态，视频节点为 Cancelled，end 未完成，总计仅 submit/status 两个请求，没有额外提交或下载。**最终取消语义成立、当前等待无法及时退出**；不能把 0ms 的本地受理耗时写成执行已停止或 P95，也未实际等待生产 60 秒上限。
- 正常对照 `real_video_workflow_stores_the_download_and_one_success_terminal` 通过：同一生产链路执行 submit→status(Succeed)→download，状态请求携带原 task ID，产出 artifact，读回测试视频字节与下载内容相同；实时节点带正确 job ID，RunStore 恰好一个 succeeded 终态。使用的是占位 MP4 字节以验证传输与存储，没有解码/播放或真实生成质量验收。
- 修复实现：继续使用既有运行取消 flag 和 `wait_for_legacy_flag`，没有新增取消树。统一 `await_video_operation` 包装提交、状态 HTTP 请求及响应正文、轮询退避、视频下载 headers/chunk；biased select 让已经到达的取消信号优先结束本地等待。取消下载时丢弃未完成的响应读取与 artifact stream，利用现有 staging drop 清除 `.part`，未发布半成品。错误文本明确本地停止不代表供应商任务或已发生副作用被撤销；续跑仍可使用现有 task ID。
- 修复后动态验证：状态接口探针先观察到真实本机 `POST /v1/video/status`，受理取消后保持响应未释放；修复前 300ms 后仍未退出，修复后 300ms 窗口内退出，约 50ms 完成取消到运行退出，RunStore 只有一个 `cancelled` 终态且没有后续请求。下载单测让 loopback 服务已返回响应头、body 持续 pending；取消在 300ms 内结束等待，错误为 `Cancelled`，artifact 根目录没有 `.part` 或成品残留。成功对照仍完成 submit→status→download 并保存 artifact。以上是本机受控 HTTP，不是远程 provider 撤销验证。
- 成熟实现对照：Codex `codex-rs/core/src/tasks/mod.rs::handle_task_abort` 先 cancel token，再等待任务完成通知或 `GRACEFULL_INTERRUPTION_TIMEOUT_MS=100`，随后 `task.handle.abort()` 并 await `session_task.abort(...)` 的专属清理，最后发布中断生命周期并刷新 rollout。参考的是有 owner 的取消、有限等待和提交顺序，**不是**宣称 Codex 实现本系统的视频协议或能撤销云端任务；只核查参考源码，未运行其全套测试。
- 影响与边界：本地 submit/poll/download future 现在都响应现有运行取消信号；本地等待停止不等于远程任务停止，供应商若已接受 submit，仍可能继续生成并产生费用。动态覆盖包括状态 HTTP 等待和下载 body 等待；没有逐项故障注入 submit 的响应头/正文及 poll 正文的每一种停滞形态，也未实等生产 60 秒轮询时限、真实云端生成或桌面按钮。

### H-120：WorkflowAgent 的节点 publisher 绕过持久化事件通道，实时状态与输出无法从 run_events 重放（已修复）

**级别：高（P1，运行恢复与机器消费）；范围：正式 `ChatService::run_workflow` 的节点生命周期、输出与媒体来源。** 与 H-41/H-43 的前端监听归属/早到事件不同，本项在监听完整且运行正常成功时，规范日志里仍没有节点事件；订阅时序修复本身不能补回不存在的记录。

- 生产根因：`run_workflow_inner` 先用原始 ChannelSink 构建 NodeEventPublisher 并交给 WorkflowAgent；AgentKernel 随后包装 PersistentEventSink，并调用 `agent.set_event_sink(persistent.clone())`。ModelAgent 接收此通道，WorkflowAgent 却继承 trait 的默认 no-op，原 publisher 始终直达实时 ChannelSink，绕开 run_events。既有 PersistentEventSink 已能序列化 `RuntimeEvent::WorkflowNode`，问题是事件没有经过它，不是缺表或缺少新 event store。
- 动态复现：`cargo test -p deepagent-app-core --test workflow_audit audit_video_node_events_survive_database_reopen --offline -- --ignored --nocapture`，**0 passed / 1 failed**，约 0.18s。复用 H-119 的真实本机 HTTP 与正式网关，运行 start→video-gen→end；正常 submit/status/download 完成，完整收集实时 NodeExecutionEvent，再 drop ChatService、CanvasArtifactService 和原 Database，使用新连接重开临时 SQLite，调用生产 RunStore::events_after 补读。
- 实际 `live=6, durable_before=0, durable_after=0, terminal=succeeded, artifact_readable=true`。实时视频 Completed 携带 provider/model/job ID、artifact URI 和输出；新连接只保留唯一 succeeded 运行终态及可读取的原视频资产，规范 replay 的节点序列为空，最后与实时序列比较失败。无需模拟断线、故意丢事件、损坏数据库或触发 provider 错误。
- 成熟实现对照：本系统自己的 Kernel/ModelAgent 已提供“同一事件经持久化后再交实时 delegate”的可复用边界，既有 `kernel_injects_durable_sink_into_streaming_agent` 对照本次亦通过。媒体参考 ArtCraft `generate_video_command.rs::handle_request` 在 enqueue 成功后调用 `insert_into_task_database_with_frontend_payload`；`TaskEnqueueSuccess` 将 provider/provider_job_id/frontend subscriber 等写入 TaskDatabase；Grok 视频后台 polling 按数据库 pending 状态恢复任务。参考的是明确保存任务身份和恢复依据；本系统应接回已有 RunStore/run_events，**不引入 ArtCraft 的另一套任务数据库**。参考源码已查，未运行其桌面或恢复测试。
- 影响与限制：以 run_events 为依据的断线补读/机器消费者不能重建各节点的状态、结果与媒体来源；UI 即使完整收到事件，也不使规范日志自动完整。CanvasWorkflowStore 是独立的图文档保存入口，界面成功回写并保存的 graph 可能仍持有输出；可选诊断日志也不等于规范 replay。本项没有证明所有画布节点在重启后消失，没有删除媒体资产，也未验证真实桌面崩溃或云端任务重连。
- 修复实现：`WorkflowAgent::set_event_sink` 接收 Kernel 已包装为 durable 的规范 sink，并更新既有 `NodeEventPublisher`。publisher 通过共享 sink slot 替换目标，先前捕获 publisher clone 的长任务进度 callback 也自动切换到同一规范事件链；无第二套事件存储或日志。
- 修复后动态验证：解除 `audit_video_node_events_survive_database_reopen` 的 ignored 标记，正式 ChatService/Kernel/Gateway、本机 HTTP 与文件 SQLite 执行同一成功路径，断开所有旧数据库 owner 后由新连接回放，输出 `live=6, durable_before=6, durable_after=6, terminal=succeeded, artifact_readable=true`，6 条事件与实时对象完全相同，包含视频 task ID、artifact provenance 和节点输出。`workflow::node_events::tests` 的 5 项（含 publisher clone 在替换 sink 后正确投递）全部通过。
- 影响及限制：规范 run_events 现在能回放该正式视频工作流的实时节点状态、结果和来源字段；本项没有证明桌面重启后所有画布视觉状态必然恢复，没有覆盖数据库写入失败、取消轨迹及同时发生的 publisher 更新，也未执行真实 Tauri 崩溃/断线重连。

### H-121：前端忽略视频进度事件的 job_id，完成前停止后重跑丢失续跑身份（已修复）

**级别：高（P1，媒体任务恢复与重复提交风险）；范围：视频已提交并收到 Queued/Running 进度，但尚未完成。** 与 H-120 的规范日志缺口独立：即使实时事件准确到达前端，当前节点仍不保存 task ID；仅接回持久化 sink 不能自动修复消费方投影。

- 生产契约（修复前）：`WorkflowAgent::progress_sink` 调用 `NodeEventPublisher::emit_job_progress`，发出 `status=running, job_id=<id>, updates.jobPhase=queued/running`，此时没有 outputs。`workflowExecutor.ts` 的事件处理只读取 status/elapsed/error/outputs；`applyNodeOutputs` 只从 outputs.videoTaskId 提取任务 ID，未读取顶层 job_id/updates，故运行中 task ID 被忽略。成功后 `execute_video` 的 outputs 包含 videoTaskId，恰好绕过缺陷；后端 resume 分支则明确依赖下一请求 config.videoTaskId。
- 可重跑动态探针：`cargo test -p deepagent-app-core --test workflow_audit audit_desktop_video_progress_keeps_task_id_for_resume --offline -- --ignored --nocapture`，**0 passed / 1 failed**，约 0.61s。Rust 用真实 NodeEventPublisher 分别生成 Running/queued 与 Completed 对照事件，通过 JSON stdin 传入 `scripts/audit-video-task-progress.cjs`；脚本加载生产 workflowExecutor 和真实 creativeStore，替换 Tauri invoke/event 为内存桥。最初每组均完整启动→收到事件→停止→再次启动；后续 H-122 复用夹具时将 Completed 对照改为收到本运行完成通知后正常结束，Running 组仍停止，缺陷结论不变。捕获实际生产 TS serializer 输出的下一份 WorkflowRequest，最后释放所有监听和运行 Promise 再断言。
- 实际输出：进度组 `receivedTaskId=audit-video-job, storedTaskId=null, resumedTaskId=null`；完成组以上三项均为 `audit-video-job`。同时确认对应节点 status 与真实事件一致，证明事件确已交付，失败不来自 H-41 的外部完成串扰或 H-43 的启动订阅竞态。此次没有运行真实 WebView/云端请求，不能把下一请求缺 ID 的证据写成真实供应商已经多收费。
- 成熟实现对照：ArtCraft 在 enqueue 后将 provider/provider_job_id 写入 TaskDatabase，后台基于 pending 任务取回；前端 `frontend/libs/tauri-api/src/lib/artcraft/task_queue/GetTaskQueue.ts` 的 TaskQueueItem 与投影保留 provider_job_id，身份不依赖 completed_item。参考的是运行中身份与最终产物分离，而非复制其状态表/协议；其视频命令的数据库插入失败仅记日志再继续返回，也不能被当作无故障的完整范本。
- 影响与边界：用户停止正在排队/生成的视频，再按现有入口重跑，序列化 config 不带恢复 ID；按当前 `generate_video` 分支会再次 submit，旧云端任务可能仍执行，带来重复任务/费用风险。已保存成功输出的 ID 对照正常；本探针不证明云端远程取消、真实重复请求、实际收费、所有画布持久化或账号切换行为。
- 修复实现：生产 executor 增加窄映射，只允许 `video-gen` 节点把非空顶层 `job_id` 写成现有 `videoTaskId`；不把后端任意 `updates` 展开进 Zustand。取消时现有状态复位只改 status，不清除此任务身份，因此下一次 serialize 继续携带相同 ID。完成事件仍从 outputs 投影同一字段，兼容旧事件和已有成功路径。
- 修复后动态验证：原 Rust/TS 跨层探针显式重跑为 **1 passed**，输出 `progress stored=job/resumed=job`，Completed 对照也为同一 ID；生产 Rust publisher、真实 workflowExecutor/creativeStore/serializer 均参与，仅 Tauri event/invoke 用内存桥。桌面默认 `pnpm test:workflow` 新增生产 executor 回归后为 **78 passed**，实际执行 running event→停止→再次提交并断言第二份 workflow config 保留 provider ID。
- 影响及限制：完成前停止再运行不再因前端丢失 ID而重复 submit；本项没有解决已完成任务修改输入后仍错误续跑（H-122），也没有验证节点图重开、账号切换、真实 WebView 或云端计费。旧 attempt 迟到事件已由 H-134 单独复现；provider/model/account 归属仍需和 H-122 一并收敛。

### H-122：视频生成成功后修改提示词仍隐式续跑旧任务，新提示词没有发给供应商却再次成功

**级别：高（P1，输出语义正确性）；状态：已修复并通过本机供应商往返探针。** 范围是视频节点输入或 provider 配置变化后再执行，以及运行中任务停止后以相同输入续跑。与 H-121 运行中 ID 丢失相反，本项原因为完成后的旧 ID 无条件保留。

- 生产路径：VideoGenForm 的提示词 onChange 只传 `{videoPrompt: ...}`；NodeConfigForm 的 handleUpdate 直接交给 creativeStore.updateNodeData 合并，旧 videoTaskId 不失效。workflowExecutor.resetAllStatus 也只改状态，serializer 将该 ID 与新提示词同时发出。`WorkflowAgent::execute_video` 把任意非空 config.videoTaskId 设为 resume_task_id；`CanvasModelGateway::generate_video` 只看是否有 ID 来跳过 submit，未确认任务是否已完成、输入是否改变或本次是否要求续跑。
- 动态复现：`cargo test -p deepagent-app-core --test workflow_audit audit_edited_video_prompt_submits_a_new_task --offline -- --ignored --nocapture`，**0 passed / 1 failed**，约 0.72s。先用正式 ChatService/Kernel/Gateway 与本机 HTTP 完成第一段视频；把该次真实 Completed 事件交给生产 TS executor/真实 creativeStore，再经同一个 updateNodeData 入口修改 videoPrompt。捕获真实 TS 下一 WorkflowRequest，直接 serde 回正式 Rust 类型并通过同一 ChatService 执行第二次，两个 run 均核对唯一终态。服务器与两个运行在断言前全部回收，没有手工给第二份请求补旧 task ID。
- 实际 `new_prompt_in_ts_request=true, carried_task_id=audit-video-job, submits=1, polls=2, new_prompt_sent=false, both_terminals=succeeded`。新提示词确实穿过前端序列化并进入后端，供应商却只收到首次 submit；第二次 status 仍查询原任务并再次下载旧任务结果。失败断言要求修改后的输入产生第二次 submit；没有把“参数根本没进入后端”当成根因，也没有把运行成功说成新视频生成成功。
- 成熟实现对照：ArtCraft `handle_artcraft_video_via_router` 依据本次 prompt/model/reference/resolution 等构造新的 GenerateVideoRequestBuilder，生成入口得到 job token 后交给已保存的 task；独立后台 polling 根据 provider_job_id 查询既有任务。参考的是“提交本次生成意图”和“恢复已提交任务”有各自身份与入口；参考代码未提供可直接照搬到本系统 DAG 的输入失效规则，该规则应作为本地设计验收，不声称参考项目所有编辑/幂等边界都正确。
- 影响与限制：用户看见新提示词仍得到旧任务结果与 succeeded，无法据状态分辨新指令是否执行；依赖该节点的下游可能继续使用过期产物。样本仅改提示词、用占位视频验证 task/request/asset 轨迹，不评价实际视频内容、模型质量或 UI 截图；供应商/账号、参考图和尺寸变化仍需独立动态验证，不能一并计为已覆盖。
- 修复实现：在既有 `CanvasProviderConfig` 增加不含凭据的随机 `credential_version`，API Key 实际变化时轮换版本；`CanvasModelGateway` 对有效 prompt、model、protocol、endpoint、credential version、size、已解析参考图内容做带长度边界的 HMAC-SHA-256 指纹，密钥只取后端当前 provider 凭据，随后再与 provider task ID 组合为 opaque resume fingerprint。任务 ID 和指纹经现有 `CanvasJobProgress`、`WorkflowNodeEvent`/`run_events`、桌面 executor 与现有画布节点字段成对保存。只有本次后端重算指纹与节点携带值相同才复用 task ID；缺少旧版指纹、输入变化或来源变化时提交新任务，并在 status 前发布新的 id+fingerprint。旧会话无需迁移，旧节点将安全地重新提交，不会把旧 task ID 发给当前供应商。没有新增 task store、节点表或持久化凭据哈希。
- 修复后真实往返探针：
  - `cargo test -p deepagent-app-core --test workflow_audit audit_edited_video_prompt_submits_a_new_task --offline -- --ignored --nocapture`：**1 passed**；生产 TS executor/creativeStore 修改提示词并序列化后，正式 Rust WorkflowRequest、ChatService、Kernel、Gateway 运行；供应商收到 `submits=2, polls=2, new_prompt_sent=true`，两次 run 均唯一 `succeeded`，第二次 poll 使用新 task ID。
  - `cargo test -p deepagent-app-core --test workflow_audit audit_video_resume_submits_new_task_for_replaced_provider_identity --offline -- --ignored --nocapture`：**1 passed**；同一 provider/config 未变化的显式续跑总 submit 仍为 1；更换 endpoint 与占位 Key 后，新 endpoint 收到一次新的 submit 和一次新 task status，`old_task_polled=false`，最终唯一 succeeded。两端均为本机 loopback HTTP，不涉及真实账号。
  - 输入指纹单元测试覆盖 prompt、model、protocol、endpoint、credential generation/key、size、reference image 和 task ID；含 RFC 4231 HMAC-SHA-256 固定向量校验。持久化值不包含明文 API Key、prompt 或图片内容。H-121 的 NodeEventPublisher→TS store→serializer 配对指纹探针已重跑并通过。
- 成熟实现对照：ArtCraft `handle_artcraft_video_via_router` 依据本次 prompt/model/reference/resolution 等构造新的 GenerateVideoRequestBuilder，生成入口得到 job token 后交给已保存的 task；独立后台 polling 根据 provider_job_id 查询既有任务。参考的是“提交本次生成意图”和“恢复已提交任务”有各自身份与入口。本系统以现有事件日志和节点投影实现绑定，是本地设计；不声称 ArtCraft 自身实现了本系统的输入失效规则。
- 影响与剩余边界：修改提示词、模型、尺寸、可解析参考图，或 provider protocol/endpoint/凭据版本后，不会再误用已完成任务；provider 变化也不会向新端点发送旧 ID。兼容旧节点的代价是无法无损续跑其历史远端任务，首次执行会重新提交，原远端任务可能仍在运行并计费。远程 HTTP 图片引用按 URL 字符串参与指纹，若同一 URL 的内容在远端被替换而 URL 不变，本地无法检测内容变化；图像供应商可能在 HTTP 请求后再抓取它。本轮未验证供应商远程取消、旧 attempt 迟到事件覆盖策略、真实云端账户权限或生成质量，也未完成 H-122 之外的全系统审计。

#### 修复前历史复现：同一 provider ID 的地址/凭据替换

- 后续 ignored 探针 `audit_video_resume_rejects_replaced_provider_identity` 复用正式 ChatService/Gateway、SQLite 与两台独立 loopback HTTP。先在 A 完成 submit/status/download，从实际 Completed 取得 ID；显式带 ID 在未变的 A 上续跑，对照确认总 submit 仍为 1。随后通过生产 save_provider 保留原配置 ID，将 base URL 和占位 API Key 改成 B；resolve_model 读回新地址/新凭据，再用相同模型引用与原 task ID 经正式入口恢复。
- 修复前 `audit_video_resume_rejects_replaced_provider_identity`：**0 passed / 1 failed**，约 0.41s；`replacement_polled=true, same_old_task=true, api_ok=true, terminal=succeeded`。A 不再收到请求，B 收到原 ID 并返回成功。该失败记录保留作修复前证据，不代表当前状态。
- 代码原因：generate_video 在每次恢复时按当前 model_ref 重新 resolve provider，resume_task_id 只有字符串，没有提交时的 endpoint/account 版本；“同一次调用内不切候选供应商”的保证无法覆盖两次运行间配置替换。参考 ArtCraft 保存任务 provider/provider_job_id 并据此选择 polling；对自定义 endpoint/凭据版本的绑定与变更处置仍需本系统本地设计，不能声称参考项目已验证任意可编辑网关配置。
- 修复后探针 `audit_video_resume_submits_new_task_for_replaced_provider_identity`：**1 passed**；原 provider 上未变输入续跑时总 submit=1。修改 base URL 与占位 Key 后，新 provider 收到 submit 新任务、随后 status 只带新 task ID；旧 provider 请求数不增加，`old_task_polled=false`，最终 run 唯一 succeeded。使用两台 loopback fixture，无真实云端账号。
- 影响及限制：旧节点缺少 fingerprint 时不会自动续跑，第一次再次执行将新建 provider task；原远端任务可能仍运行并产生费用。探针验证了地址与 Key 同时变化，Key generation 另有 provider-service 单测，但未单独以两次完整 HTTP 运行验证“同地址只换 Key”；也未验证真实云端账户撤销、服务商计费、远程取消或数据访问边界。

### H-123：本地与 SSH 终端的分片读取推进到缓存末尾，未返回的输出被游标跳过

**级别：中（P2，游标接口的数据完整性）；范围：单次待返回历史超过 64 KiB、仍在 256 KiB 保留窗口内的终端读取或重新附着。** 本项不覆盖普通模型流或命令工具的输出；桌面 `TerminalPlugin` 当前调用旧版无游标的 `localPtyRead` / `sshPtyRead`，不能据本探针宣称当前 xterm 面板已发生相同丢失。

- 生产路径（修复前）：`terminal_service.rs::TerminalService::pty_read_with_cursor` 与 `deepagent-ssh/src/session.rs::SshSession::pty_read_with_cursor` 都先取得全部已接收字节的 `current`，将返回 `data` 截至 64 KiB，最后却返回 `cursor: current`。`truncated` 只检查旧历史是否被淘汰，没有反映此处尚未返回的尾部。Direct 与 SSH 的 `TerminalSessionBackend::read` 又把该 cursor 写入现有 SQLite 游标存储；Tauri 的 `local_pty_read_cursor` / `terminal_session_read` 直接暴露本地生产路径，TypeScript 也导出对应 API。
- 修复实现：两端先计算本次读取的有效起点（请求游标与保留历史起点的较大值），将返回游标设为实际 `data` 的末尾；只有没有返回数据时才保留当前可用生产游标。响应上限造成的未返回后缀不会再被确认，历史起点被淘汰仍仅通过 `truncated` 报告。未新增输出存储或第二套游标。
- 本地动态验证（修复前历史）：`crates/deepagent-app-core/tests/terminal_audit.rs::audit_direct_terminal_cursor_preserves_unreturned_output` 通过正式 TerminalService、Direct backend、实际 Windows ConPTY / cmd.exe 和临时文件 SQLite 运行。受控 `type payload.txt` 产生输出；先用正式旧版读取排空有界 transport 队列并保留历史，再让游标消费者附着。执行 `cargo test -p deepagent-app-core --test terminal_audit --offline -- --ignored --nocapture`，**0 passed / 1 failed**，约 0.45s：小输出对照 `first_bytes=3832, cursor=3832, next_bytes=0, tail_received=true`；大输出 `first_bytes=65536, cursor=107992, persisted_cursor=107992, next_bytes=0, truncated=false`。消费者按返回 cursor 继续读时收不到尾标记，而独立从实际返回长度 65536 读取仍得到 **42456 字节和完整尾标记**。所有 shell 均在缺陷断言前通过正式 close 关闭；精确总字节数包含本机控制序列和提示符，不作为跨机器固定常数。
- SSH 动态验证：`cargo test -p deepagent-ssh audit_ssh_terminal_cursor_preserves_unreturned_output --offline -- --ignored --nocapture`，**0 passed / 1 failed**，约 0.00s（首次依赖编译约 28.77s）。测试将 4 KiB transport chunks 放入正式 SshSession 的 PTY receiver，调用真实 reader，不替换读取算法。16 KiB 对照完整；96 KiB 组为 `source=98304, first=65536, cursor=98304, next=0, suffix=32768, truncated=false`，未返回后缀的每个字节均与来源相符。这里只验证会话缓冲/分页，无远程 SSH 登录或实际网络丢包结论。
- 探针限制与排错：本地夹具最初不读取 transport 就等待命令完成标记，分别在 10s / 3s 等待期失败；读取诊断显示命令确已输出但未执行后续标记。该夹具会让按 chunk 数限长的 transport 队列阻塞大量输出，不能当成产品缺陷。本次改为通过生产旧读取持续排空队列、完成后验证保留历史；没有增加生产超时或改动读取实现。未运行真实桌面游标重连、跨进程恢复、Linux/macOS PTY；未动态验证 SSH adapter 的 SQLite 写入，该分支仅由源码核对。
- 成熟机制对照：Codex `core/src/unified_exec/process.rs::OutputBuffers` 在同一锁内记录 pending 与 transcript；`process_manager.rs::collect_output_until_deadline` 取走的 pending 完整进入本次收集，`head_tail_buffer.rs::HeadTailBuffer` 对预算淘汰独立累计 `omitted_bytes`，在输出路径提供 omission marker。它不是本系统相同的字节游标协议，不能直接照搬 DTO；可借鉴的是**消费位置对应实际交付范围、预算淘汰有明确损失信息**，不把缓存生产进度等同于已交付进度。
- 修复后验证：`cargo test -p deepagent-ssh audit_ssh_terminal_cursor_preserves_unreturned_output --offline -- --nocapture` **1 passed**，96 KiB 输入首包 `first=65536,cursor=65536`，后包 `next=32768,cursor=98304`，完整内容可还原；`cargo test -p deepagent-app-core --test terminal_audit audit_direct_terminal_cursor_preserves_unreturned_output --offline -- --nocapture` **1 passed**，Windows ConPTY 大输出首包 `first=65536,cursor=65536`，后包 `next=42456`，持久化游标为 65536 且尾标记在后包收到。原先两个失败探针现已改为默认回归测试。
- 影响与剩余边界：修复前附着消费者会静默跳过仍在内存中的输出，持久化 cursor 也可能让恢复误以为已读取；现在下一读取位置只推进到本次实际返回末尾，历史淘汰仍单独通过 `truncated` 报告。若要证明网络客户端已经收到数据，仍需在现有接口中另行定义客户端 ACK，不能把服务端 IPC 返回写入 SQLite 等同于网络交付成功。当前尚未验证真实桌面游标重连、跨进程恢复、Linux/macOS PTY、SSH adapter 的 SQLite 动态写入、重复/未来 cursor、并发生产和断线恢复。

### H-124：系统视觉预处理未纳入运行生命周期，点击停止后仍处理附件并提交主模型（已修复）

**级别：高（P1，可见的停止操作无效并造成后续模型调用）；范围：系统视觉开启且有图片尚未识别，正式 `onSubmit` 在调用 `runChat` / `start_chat_v2` 前等待 Vision API 的新会话与续聊。** 纯文本正常停止路径作为正对照通过；本项不是后端已接纳 run 的 cancellation tree 缺陷，也不表示所有图片识别失败。

- 生产路径：React `App.tsx::onSubmit` 先等 `getVisionSettings()`，随后同步创建运行 ID、写入 `activeRunIdsRef` 并让 `runningSessionIds` / `activePendingRunKey` 表现为运行中；构造 transcript 后调用 `recognizeImageAttachmentsOnSend()`，它又逐张 await `visionRecognizeImage()`。直到全部预处理完成后，`runChat()` 才调用 `start_chat_v2` 创建真正的 AppCore admission/cancellation registration。停止按钮此时读到有效的前端 run ID，并调用真实 `cancelRun(runId)`；Tauri `cancel_run` 最终查询 `RunCoordinator` 的活动注册。尚未 admission 的 ID 返回 `{accepted:false,state:"not_found"}`，UI 提示取消失败；原 `onSubmit` 在图像 promise 完成后没有重新检查该 run 的取消/失效状态，继续处理余下图片并调用 `start_chat_v2`。后端取消注册从未覆盖此前的识别等待。
- 动态复现：新增 opt-in `apps/desktop/scripts/audit-vision-preflight-cancel.test.cjs`。它从生产 `App.tsx` AST 加载并执行原 `onSubmit`、`onStopRun` 和视觉/附件 helper；直接加载生产 `api.ts::visionRecognizeImage/runChat/cancelRun`，受控 Tauri IPC 暂停 Vision 响应，并以与正式 RunCoordinator 一致的 accepted 语义按是否 `start_chat_v2` admission 返回取消 ACK。调用原 UI callback，分别在续聊和新会话的第一张图片请求挂起时点击停止，随后释放原图片请求，等待所有订阅、定时器和 run promise 收尾。
- 修复前执行 `$env:RUN_HARNESS_AUDIT='1'; node --test scripts/audit-vision-preflight-cancel.test.cjs`（工作目录 `apps/desktop`），**1 passed / 2 failed**：普通文本对照 `startsBeforeStop=1, cancelsBeforeRelease=1, cancelAccepted=true, stoppingBeforeRelease=true, startsAfterStop=0`；续聊和新会话都为 `startsBeforeStop=0, cancelsBeforeRelease=1, cancelAccepted=false, stoppingBeforeRelease=false, cancelNotFoundVisible=true, startsAfterStop=1, recognizedTextSubmitted=true, preflightToolsSubmitted=1, listenersAfterFinish=0`。停止后仍多发的唯一识别请求就是第二张图片；第一张识别结果连同后续图片处理正常结束后，生产 API 收到一次新 `start_chat_v2`。失败断言在 completion 和生产 TS listener 清理之后触发，因此没有留下后台 run/测试进程。
- 修复后执行同一命令，**3 passed / 0 failed**：续聊和新会话在视觉预处理期间停止均为 `cancelsBeforeRelease=0`、`cancelAccepted=undefined`、`visionCallsAfterStop=0`、`startsAfterStop=0`、`cancelNotFoundVisible=false`；已接纳文本运行继续使用原后端取消路径并完成清理。`apps/desktop/scripts/check-process-collapse.test.cjs` 同步回归 **12 passed / 0 failed**，覆盖旧 transport 清理不覆盖新运行。
- 本探针没有执行真实图片读取、视觉公网请求、真实 Tauri 窗口或 SQLite admission；`visionRecognizeImage` IPC 使用本机受控延迟，没有推断其服务商延迟/计费。本地 handler 不模拟 stop 前已进入取消表的后端 run，但相邻纯文本正对照证实同一个实际 stop callback 可取消这种已接纳运行。
- 成熟实现对照：Codex `core/src/tasks/mod.rs::handle_task_abort` 将活动 turn 的取消交给任务 token；Claude Code `restored-src/src/utils/attachments.ts` 的相关 prefetch 从 turn AbortController 创建 child controller，并在 turn abort 时同步中止，`query.ts` 在处理中断流和逐工具派发前主动查同一信号；DeepSeek Harness `core/agent/src/runtime-types.ts` 定义 `agent/pre-step` 的 turn signal，`api/session-controller/src/index.ts::prompt()` 在 prompt admission 前 `throwIfAborted()`。这些机制共同说明预处理要么属于一个有 owner、能取消的准备阶段，要么 admission 前就拒绝已取消请求；不能让 UI 先登记“运行中”而后端没有取消目标。本系统在提交前的 UI preflight 状态应标为单一 run 生命周期的一部分，不需要另建第二个运行表或取消机制。
- 修复实现：复用现有前端 run ID，增加 `preparing`、`starting`、`admitted` 阶段和每个运行的 `AbortController`。视觉预处理在每张图片前、视觉响应后检查同一信号；停止发生在 admission 前时只中止该运行的本地准备链，不调用尚不存在的后端 run。`runChat` 在 `start_chat_v2` 返回后绑定同一信号，若停止与 admission 竞态则立即调用同一 `cancel_run`，并在最终清理时移除监听和阶段状态。已接纳运行保持原 AppCore 取消树，不新增存储或取消机制。
- 剩余边界：视觉 IPC 的底层 HTTP 请求仍由服务端决定是否可中止；本修复保证取消后的迟到结果不会继续识别后续图片或提交主模型。真实 Tauri 窗口、真实视觉供应商、SQLite admission 和网络级取消仍需单独桌面验收，不能由本地隔离 harness 推导。

### H-125：SSH 客户端不验证远端主机密钥，连接到未信任服务器仍成功

**级别：高（P1，网络中间人或错误地址映射触发时破坏 SSH 对端身份、机密性与完整性；密码认证还可能泄露密码）；范围：桌面/应用通过正式 `SshService` 建立的 SSH 连接及复用该连接的命令、PTY、文件和远程安装功能。** 本项不涉及 DeepSeek 模型协议或模型请求，是独立的远端执行身份边界。

- 生产路径：`crates/deepagent-ssh/src/service.rs::connect_client`（约第 1358 行）将 `ServerCheckMethod::NoCheck` 传给 `async-ssh2-tokio 0.12.2`。已核对该版本 registry 源码 `src/client.rs::ClientHandler::check_server_key`（约第 964 行）：`NoCheck => Ok(true)`；库还提供 `KnownHostsFile`、`DefaultKnownHostsFile` 和 pin 公钥等验证选项。`SshConnectionConfig` 没有服务端 host key/fingerprint 字段，创建/更新 SSH 连接的 DTO 与表单也没有对应信任输入。`connect`、连接测试和定期状态刷新都复用 `connect_client`，因此不是只影响一条诊断路径。
- 动态复现：新增 opt-in `crates/deepagent-ssh/tests/host_key_audit.rs`。测试仅绑定 `127.0.0.1` 随机端口，在进程内启动 russh 临时服务器并随机生成 host key；使用正式 `SshService::create_connection` 和 `SshService::connect`，密码是测试专用临时值。实测 `production_connect_accepts_untrusted_key=true`。随后为同一 host/port 写入另一把随机公钥，使用依赖正式 `KnownHostsFile` 验证路径连接，得到 `known_hosts_rejects_mismatched_key=true`，证明拒绝能力可用且该测试对照能区分验证与 `NoCheck`。
- 本轮复核（2026-10-23）：重新执行同一 ignored 探针，仍得到 `production_connect_accepts_untrusted_key=true`、`known_hosts_rejects_mismatched_key=true`，最终 **0 passed / 1 failed**；失败稳定发生在生产 `SshService` 入口断言，未修改生产实现或测试信任材料。
- 显式探针命令 `cargo test -p deepagent-ssh --test host_key_audit --offline -- --ignored --nocapture` 以 **0 passed / 1 failed** 结束；这是预期的审计失败，最后断言要求生产入口拒绝未信任密钥，但当前入口成功。输出中仅有 loopback、临时端口和布尔结果。第一次探针版本把 `check_known_hosts_path` 的预期 `KeyChanged` 当成异常而在对照阶段 panic；修正为将其作为拒绝证据后，正式入口断言稳定失败。第一次运行中服务器也因客户端主动断开而得到 session close 错误；夹具现已将正常关闭/握手拒绝视为 fixture 生命周期，不把它们算作产品故障。
- 成熟实现对照：DeepSeek Harness `packages/ssh/ssh/src/index.ts` 启动本地 OpenSSH 时明确传 `StrictHostKeyChecking=yes`，其 SSH README 要求连接前配置 known-host 项，启动测试检查实际参数；Claude Code `restored-src/src/utils/plugins/marketplaceManager.ts` 对 Git SSH/submodule 使用同一 strict 选项，并针对未知/变更主机密钥给出失败诊断。Codex 本次核查到的 `windows-sandbox-rs/src/ssh_config_dependencies.rs` 是将 OpenSSH 的 known-host/config 文件纳入沙箱依赖追踪；没有据此推断 Codex 独立实现了通用 SSH 主机密钥校验。
- 影响与触发边界：攻击者需要能中间人拦截/重定向 SSH 网络，或连接配置中的主机地址错误地指向攻击者；单纯持有正常 SSH 账户并不能令可信网络下的真实服务器密钥替换。触发后，使用密码认证时客户端可能把口令交给伪装服务器；使用密钥文件时不表示私钥会被发送，但客户端会把该连接视作目标并可能向错误对端发送命令、路径和文件内容，接收伪造输出，破坏远程操作的机密性与完整性。此次只使用本机随机服务器和测试凭据，没有访问公网、用户服务器或真实凭据，也没有证明现实网络中已有攻击发生。
- 收敛方向（尚未实施，本地设计）：在 SSH 连接配置中持久化可审计的 host key 信任来源，首次连接通过用户核验 fingerprint 后写入现有配置存储，后续按 host+port 校验并在密钥变化时失败关闭；至少复用现有库的 known_hosts 支持，并明确其路径、hashed-host/非默认端口兼容和 Windows 路径行为。禁止在未知密钥时静默自动接受。信任配置要贯穿 `connect`、test、status refresh、重连以及更新后的连接身份；UI 显示首次核验和 key changed 的可操作错误。回归覆盖 unknown / matching / changed key、非默认端口、IPv6、连接复用、密码与 key-file auth、临时 known_hosts、重启恢复以及服务端密钥轮换；只有失败关闭和兼容路径均验证后才算修复。

### H-126：MCP stdio 与 HTTP transport 在完整读取/解析前没有响应字节上限

**级别：中（P2；仅当已配置的 MCP server 返回异常大响应，或并发 MCP 请求同时返回大响应时，才会形成显著的宿主内存与解析 CPU 压力；本探针证明缺少上限，不声称 2 MiB 本身会导致进程失效）。** 本项关注 transport 的资源边界，不是 MCP 协议要求的通用最大响应大小。

- 生产路径：stdio 的 `crates/deepagent-mcp/src/stdio.rs::StdioTransport::send` 使用 `AsyncBufReadExt::read_line(&mut String)` 读取整条 JSON-RPC 消息，再调用 `serde_json::from_str`；读取前没有累计字节计数或行长度上限。HTTP 的 `crates/deepagent-mcp/src/http.rs::HttpTransport::send` 对成功响应调用 `response.text().await`，随后才解析 JSON 或 SSE；总请求 timeout 为 60 秒，但没有 body 字节上限。非成功 HTTP 状态的错误正文也通过 `response.text()` 全量读取。
- 工具结果预算不能保护该边界：`crates/deepagent-runtime/src/tool_budget.rs::apply_tool_result_budget` 的默认模型可见预算为 4,000 tokens，但它位于工具结果已由 MCP transport 收到并反序列化之后；因此只限制后续进入模型的结果投影，不能限制 transport 的缓冲、JSON 解析和中间对象分配。
- 动态复现：新增 opt-in `crates/deepagent-mcp/tests/response_limit_audit.rs`，使用正式 `StdioTransport::spawn/send` 启动本机临时 Node MCP server，并使用正式 `HttpTransport::with_url/send` 对接 loopback HTTP fixture。两条路径都先发送 16 字节 payload 的小响应对照，再发送 2 MiB payload。实测 stdio `accepted_payload_bytes=2097152`；HTTP `accepted_payload_bytes=2097152`，本机 fixture 观测到完整 `Content-Length=2097200`。两边均在 10 秒探针截止内完整解析并返回。探针使用的 1 MiB 仅是审计对照线，**不是 MCP 标准上限或既有产品配置**；证明点是 transport 未施加任何读取上限。
- 显式复现命令：
  - `cargo test -p deepagent-mcp --test response_limit_audit audit_stdio_transport_accepts_oversized_response_line --offline -- --ignored --nocapture`：**0 passed / 1 failed（预期）**；16 字节对照通过，2 MiB 正常返回后被审计断言捕获。
  - `cargo test -p deepagent-mcp --features http --test response_limit_audit audit_http_transport_accepts_oversized_response_body --offline -- --ignored --nocapture`：**0 passed / 1 failed（预期）**；16 字节对照通过，HTTP fixture 确认向客户端发送完整 2 MiB body，transport 正常返回并解析。
- 本轮复核（2026-10-23）：上述两个命令均再次得到 **0 passed / 1 failed**；stdio 输出 `accepted_payload_bytes=2097152`，HTTP 输出 `accepted_payload_bytes=2097152` 且 fixture 观测 response body `2097200` 字节。失败都发生在审计阈值断言，说明当前生产 transport 仍未在读取/解析前限制响应大小。
- 成熟实现对照与边界：DeepSeek Harness `packages/mcp/mcp-client/src/connection.ts` 对 MCP attributed server instructions 提供 32 KiB 上限；这是语义字段的大小约束，不是任意 JSON-RPC 响应 body 限制。Codex `codex-mcp/src/trusted_access.rs` 对特定 verified-access HTTP 响应采用 1 MiB 流式字节上限；这是另一个受控 API 的局部做法，不能据此声称 Codex 已给所有 MCP transport 加统一上限。两者只支持“在解码/解析前设置有界读取”的工程模式；通用 MCP 响应预算仍应由本系统按工具目录、工具调用和资源内容制定本地策略。
- 影响与触发边界：stdio server 是本机已配置子进程，HTTP server 是显式配置的 MCP 端点；需要相应 server 返回大型内容。现实现对 JSONL 整行和 HTTP 正文均先完整缓冲，再构建 JSON 值；多请求/多连接并发会叠加内存和解析成本。探针只确认 2 MiB 可完整通过，没有制造极端 payload、压力测试或证明实际 OOM。MCP server 的可信级别、单次调用并发度和允许的大型资源使用场景需要在修复设计中区分，避免把文件/图像资源误伤为过小的固定限制。
- 收敛方向（尚未实施，本地设计）：在 MCP transport 边界引入明确、可配置且有合理默认值的字节预算。stdio 采用有上限的增量行读取，超过上限即返回结构化 transport 错误并关闭/隔离该连接；HTTP 按流式 chunk 计数后再解析，不能只依赖可伪造或缺失的 `Content-Length`，成功正文、SSE 和错误正文都要有界。错误信息只保留状态、字节数和预算，不回显完整远端 body。再为 MCP 工具目录、单次工具结果和资源/图片分别定义策略；大型合法内容走现有 artifact/file reference 或分块读取，不让无界 JSON 进入内存。验收需包含小响应成功、刚好到界成功、超界拒绝、chunked 无 Content-Length、SSE 超界、畸形/多行 stdio 输出、并发请求、取消/超时及不回显远端正文；预算值作为本地设计，经真实插件兼容测试确定。

### H-127：MCP image 工具结果被降格成 Responses 普通 JSON 字符串，模型收不到图像输入

**级别：中（P2；仅当 MCP 工具返回 `type: image` 内容且后续模型需要检查图像时触发；调用本身仍可成功，但模型无法按图像语义观察结果）。** 当前跨层证明限定在 DeepSeek Responses 请求路径，不外推所有 provider adapter。

- 规范能力依据：DeepSeek 官方 [Responses API 指南](https://api-docs.deepseek.com/guides/responses_api/)明确允许 `input_image` 出现在 `function_call_output` 与 `custom_tool_call_output` 的 `output` 列表中，并说明 `deepseek-flash` 会将其作为真实图像处理；[Vision 指南](https://api-docs.deepseek.com/guides/vision/)也列出工具输出图像形式。也就是说，工具调用配对不要求把图片压成文本，Responses 有原生多模态结果形状。
- 生产路径：MCP `ContentBlock::Other` 保留 image JSON；`McpToolAdapter::invoke` 将整个 `ToolCallResult` 用 `serde_json::to_value` 转成 JSON，并附加拼出的文本。随后 `ModelAgent::record_observation` 把 observation 包为 `{"status":"ok","result":...}` 并用 `serde_json::to_string` 生成 tool output；`ResponseItem::FunctionCallOutput` 的 `output` 类型是 `String`，`deepagent-models/src/chat.rs` 再将这个字符串直接放进 wire 的 `function_call_output.output`。该链路没有把 image data 保存成 attachment reference，也没有把结果转换成 `[{"type":"input_image",...}]`。
- 动态复现：新增 opt-in `crates/deepagent-app-core/tests/kernel_v2_e2e/mcp_image_output.rs`，经真实本机 Node stdio MCP 子进程返回一个 PNG `type: image` block；正式 `McpService` 准备、`McpRegistry`、adapter、ChatService、Kernel、ModelAgent 和 Responses 请求序列化均参与。模型使用离线 EventSink transport，仅回放 tool call/assistant answer 并捕获实际 provider request JSON。下一次请求中，正式 `function_call_output.output` 为字符串；重新解析该字符串可见完整 `result.content[0].type == "image"`，原 base64 仍在请求正文；没有 `input_image` content-part 数组。`typed_input_image=false`、`raw_json_string=true`、`raw_image_data_present=true`。测试只有最后的类型断言故意失败。
- 显式探针命令 `cargo test -p deepagent-app-core --test kernel_v2_e2e mcp_image_output::audit_mcp_image_result_is_not_serialized_as_text_json --offline -- --ignored --nocapture`：**0 passed / 1 failed（预期）**，约 0.44s；两个正式模型请求都发生，第一轮 MCP 工具成功，第二轮 wire 请求仍匹配工具调用 ID，但图像 JSON 被当作普通字符串。没有公网供应商、用户文件或真实 UI。
- 成熟实现对照：DeepSeek Harness `packages/mcp/mcp-client/src/tools.ts::createMcpToolDefinition` 在 MCP result 含 image 时通过 `prepareImageProjection` 解析 MIME/base64、按当前模型检查图片能力、将图片保存到 attachment store，再由 `projectContent` 输出带持久引用的图像 content block；`packages/llm/llm-deepseek/src/images.ts` 在请求准备时解析 attachment 并物化 provider image input。其内容投影与图片读取均由现有 ToolRuntime/LLM adapter 所有，不直接把远端 base64 JSON 当普通文本喂回模型。此处只借鉴边界，不复制实现代码。
- 影响与触发边界：模型会看到一段 JSON 字符串和 base64，而不是图像视觉输入；回答无法依赖该图像的实际视觉内容。图像字节还进入模型可见字符串、上下文计量和持久化 tool result；较大的图片可能先被 4,000-token 工具结果预算截断，令字符串既不能作为图像理解，也不再是完整原始 JSON。探针使用小图片只验证形状，不证明真实供应商是否会自行解码字符串里的 base64，也不测账单、视觉质量或大图截断。
- 收敛方向（尚未实施，本地设计）：复用本系统现有附件存储和 provider image materialization seam，为工具结果新增可持久化的有类型 image reference；在工具执行结果提交到原生 Surface 前，将 MCP image block 经 MIME/base64 校验、大小/数量/模型模态检查后保存为附件引用，并以 DeepSeek Responses 支持的 `input_image` 输出块构造 `function_call_output.output`。raw data 不写入 assistant/tool 文本、事件 JSON 或模型上下文；图片引用需要随工具结果 event/replay 可恢复，遵守附件清理、来源不可信标记与 token/字节预算。明确不支持的模型应返回可见、配对正确的文字诊断，不静默丢图。回归覆盖 function/custom 输出、混合文本和多图顺序、无图对照、错误 MIME/base64、超限、非视觉模型、工具失败、预算、压缩/replay/restart 和实际完整 wire；保持本 provider adapter 边界，不把 DeepSeek 字段散落到 MCP DTO/UI。

### H-128：停止运行后活动 MCP 工具仍等待响应，取消信号没有到达 MCP adapter/transport

**状态：已修复。级别：高（P1）；触发条件：用户停止发生在已发出的 MCP `tools/call` 尚未返回期间。** 修复前如果 MCP server 本身快速返回，延迟很短；若服务端不响应，运行会一直停留在工具等待，直到工具期限或服务端响应。

- 生产路径（修复前）：AppCore `RunCoordinator::request_cancel` 对活动 run 立即调用取消树并返回 accepted；`RuntimeEngine` 在普通工具路径把派生 `ToolExecutionContext` 交给 `ToolRegistry::invoke_with_context`。但 `McpToolAdapter` 继承兼容默认实现忽略 context；`McpClient` 和 `McpTransport` 没有取消路由；因此停止信号不会放弃当前 MCP future，也不会发送 `notifications/cancelled`。
- 动态复现（修复前）：本机 Node stdio MCP server 收到 `tools/call` 后以文件 gate 暂停响应；正式 `request_cancel(session_id)` 返回 `accepted=true`，再等 250ms，Chat run 仍未结束且 server 没收到 `notifications/cancelled`。释放 server 后才持久化 `terminal_kind=cancelled`。
- 显式回归命令 `cargo test -p deepagent-app-core --test kernel_v2_e2e mcp_cancellation::stopping_run_interrupts_inflight_stdio_mcp_call --offline -- --nocapture`：**1 passed**，约 0.54s；正式 AppCore/Kernel/McpService/stdio 子进程中，取消信号结束活动工具等待并发送协议通知；工具调用产生配对失败结果，运行持久化为 `cancelled`，模型请求数仍为 1。使用无副作用本机 fixture 和离线 provider。
- 规范与成熟实现对照：MCP 2024-11-05 [Cancellation](https://modelcontextprotocol.io/specification/2024-11-05/basic/utilities/cancellation)定义按 request ID 发送 `notifications/cancelled`，接收端 SHOULD 停止处理并释放资源；该扩展是 optional，所以此处不将“未发送通知”单独称作 MCP 协议必违，而是产品运行取消链未贯通。DeepSeek Harness `packages/core/tools/src/index.ts::ToolExecutionInput` 将每次工具的 `AbortSignal` 设为必需字段，`packages/mcp/mcp-client/src/tools.ts::syncTools` 把 `execution.signal` 与 per-call timeout 一并传入 `client.callTool`。Codex `core/src/tools/parallel.rs` 在工具 dispatch 与 run cancellation 间使用 `tokio::select!`，取消获胜时中止活动 dispatch 并生成 paired aborted result。参考的是取消归属和执行等待边界；不同 MCP SDK 的底层通知实现不作未经验证的推断。
- 修复实现：沿用现有每工具 child cancellation token，从 `McpToolAdapter::invoke_with_context` 传入 MCP client；stdio 取消时在同一 stdin 写锁内发送无 ID 的 `notifications/cancelled`，写入最多等待 50ms；HTTP 中止当前响应等待，后台按同一 endpoint 和认证 headers 发送取消通知，发送最多等待 100ms；重连等待和重连握手也监听该工具的 child token。Runtime 顺序及并行工具 action 终态统一检查 token 与兼容取消标志，保证取消原因落为 `Cancelled`。配对失败结果说明远端操作结果可能未知，不将通知视作远端副作用回滚。
- 影响与边界：本机 stdio 与 loopback HTTP 测试证明停止可终止本地工具等待并发送通知。MCP 取消是 optional，远端 server 可以忽略；远程写入等副作用仍可能已经发生。并发 stdio 响应路由继续由现有独立探针跟踪，本次回归未宣称已修复。
- 后续分阶段复核：上述修复和既有回归覆盖请求已经发完的响应等待阶段；stdio 写入在背压下仍不监听取消，正式运行复现见 H-136。不能将响应阶段通过扩展成整个 transport 生命周期均可及时取消。

### H-129：设置页的一次性 MCP 测试连接没有启动截止时间（已修复）

**状态：已修复。级别：中（P2，管理界面可用性）；触发条件：用户运行 MCP“测试连接”，stdio 服务在 `initialize` 或 `tools/list` 任一阶段不返回匹配响应。** 原一次性测试入口绕过正常宿主托管准备的 30 秒启动截止时间；该发现仅针对管理界面诊断入口，不代表普通聊天会等待 30 秒或无限期。

- 生产路径：桌面 Tauri `test_mcp_server` 调用 `McpService::test_server`；该入口验证配置后依次调用 MCP 初始化与工具发现。原路径没有总截止时间，stdio `StdioTransport::send` 会等待 stdout 中与 request ID 匹配的 JSON-RPC 响应。普通 `mcp_connection_status` 读取缓存状态，不会启动连接。
- 修复实现：`McpService::test_server` 现在与宿主托管准备共用 `MCP_STARTUP_TIMEOUT`（30 秒），覆盖 `initialize` + `tools/list` 整个协议握手；超时后显式关闭临时 transport，并最多等待 2 秒完成关闭。超时、握手失败或关闭失败都返回 `status=failed` 和原因，不发布到长期 ready registry。deadline 包含协议握手，但不包含同步 `connect_transport` 创建客户端/子进程之前的执行时间。
- 动态验证：两个默认运行回归测试通过同一探测实现，以 2 秒测试期限启动本机 Node stdio server。`test_server_deadlines_stdio_initialize` 让 initialize 挂起；`test_server_deadlines_stdio_tools_list` 正常完成 initialize 后让 tools/list 挂起。两者均收到对应阶段请求，期限到达后返回 `status=failed`、错误包含 `MCP startup timed out`，随后 transport close 等待子进程退出；无需手动放行 fixture。使用相同公开 `McpService::test_server` 代码路径，仅通过私有测试辅助入口缩短期限。
- 成熟实现对照：Codex `codex-mcp/src/connection_manager.rs` 为每个连接解析 `startup_timeout_sec`，缺省使用 `rmcp_client.rs::DEFAULT_STARTUP_TIMEOUT = 30s`；`async_managed_client.client().await` 的启动由 connection manager 管理。DeepSeek Harness MCP supervisor 提供可等待的 startup `ready` 与在卸载时 dispose negotiating transport，但其配置不声明与本探针等价的统一 startup deadline；本次复用本系统已有有界准备规则，不声称参考实现拥有相同的超时配置。
- 影响及限制：修复后协议握手超时不会无限占用管理界面请求。尚未覆盖 HTTP 半截响应下的实际桌面 spinner、重复点击的子进程数量，以及 UI 是否展示完整 `failed` 详情；同步 spawn 阶段也未单独限制。未访问公网、真实插件或用户数据。

### H-130：Harness `turn/steer` 可从同一个已替代 turn 重复派生多个运行

**级别：中（P2）；触发条件：客户端在第一次 `turn/steer` 已返回、旧 turn 的 `spawn_turn` 尚未结束前，再次使用同一个旧 `turnId` 发送 steer。** 这是 stdio app-server 入口的运行身份问题；本项没有证明桌面 `start_chat_v2` 存在同样行为。

- 生产路径：`apps/cli/src/app_server.rs::turn_steer()` 检查 `TurnState.active`，调用 `ChatService::request_cancel(old_turn_id)`，记录 `continuation.created`，创建新的 turn ID 并返回新的 `TurnLaunch`。它没有在第一次接受替代请求时把旧 `TurnState` 标记为 cancelling/replaced，也没有记录已创建的 replacement ID。旧状态只由异步 `spawn_turn()` 在 `run_in_session_with_overrides()` 完成之后经 `finish_turn()` 改成 inactive；因此取消信号已设置但旧 run 仍在工具、hook 或终态收尾期间时，旧 turn 继续满足 `active=true`。
- 动态复现：新增 opt-in 单测 `app_server::tests::audit_repeated_steer_does_not_launch_multiple_replacements`。通过正式 `ServerState::dispatch()` 创建 thread/start 与 turn/start，并用正式 `ChatService::admit_run()` 建立旧 turn 的 durable run/cancellation registration；在不让后台 `spawn_turn()` 收尾的情况下，对同一旧 turn ID 连续 dispatch 两个 `turn/steer`。执行 `cargo test -p deepagent-cli audit_repeated_steer_does_not_launch_multiple_replacements --offline -- --ignored --nocapture`，实测 `first_status=success, second_status=success, first_launch=true, second_launch=true`，两个不同 replacement turn ID 均生成；期望第二个过期 steer 不再产生 launch 的断言失败，**0 passed / 1 failed（预期暴露缺陷）**。
- 生产输入租约核对：两个 `TurnLaunch` 都会由 `run_stdio()` 分别交给 `spawn_turn()`。AppCore `input_runtime::accept_input_turn()` 对同一 session 使用 `InputLeaseRegistry` 串行化输入，因此它们不会同时持有 session 执行租约；但 registry 不会合并这些已接受的 run。后来的 replacement 仍排在队列并可在先前运行后获得租约。该路径足以使重复请求成为多个顺序运行，而不是对当前 replacement 的单一纠偏。此次探针停在 app-server dispatch 层，没有启动这两个任务、模型请求或工具副作用；费用与副作用是触发后的风险，不是本次已观察到的结果。
- 成熟实现对照：Codex `app-server/src/request_processors/turn_processor.rs::turn_steer_inner()` 传递 `expected_turn_id` 给 `CodexThread::steer_turn()`；`core/src/codex_thread.rs::steer_turn()` 文档明确只在所期待 turn 仍是活动 turn 时接收，并由 core input queue 检查当前 turn 身份。它将 steering 输入投递到同一个 active turn，不通过取消旧 turn 并重复创建独立替代 turn。DeepSeek Harness 的 `core/agent/src/runtime-types.ts` 把 `steer()` 定义为提交给最近 step 的当前 Agent 输入；其 agent loop 将输入放进当前 session inbox，由下一 step 消费。两者语义不同，但都有明确的活动执行 owner；不能把多条 steer 入 inbox 与本系统多次启动 replacement 混为一谈。
- 影响与边界：客户端超时重试、重复提交或仍引用旧 `turnId` 时，可接受多个独立替代输入；这些 run 经同一 session 租约依次执行，可能产生额外模型请求、重复工具动作或多份用户未预期的回答。普通情况下每个请求只 steer 一次时不触发；本次没有测量模型费用，也没有启动副作用工具。
- 收敛方向（尚未实施，本地设计）：把 turn 生命周期从单一 `active: bool` 收敛为显式 active/cancelling/replaced/terminal 状态。在同一 app-server state 临界区内对旧 turn 做一次性替代 claim，并关联唯一 replacement turn；同一旧 ID 的后续请求必须拒绝为 stale，或返回已创建 replacement 的身份但不得再次启动。状态变更与现有 `continuation.created` 持久化失败之间需定义一致顺序，重连查询沿用 `thread/read`/`run_events`，不增加第二套 run/event store。回归覆盖重复请求、响应丢失后重试、旧任务完成与 steer 竞争、旧取消审计落盘失败、排队 replacement、服务进程重启和不同 thread 隔离。

### H-131：固定总时限会截断仍在持续输出的 provider stream

**级别：条件性中（P2）；触发条件：单次流式请求持续超过 300 秒，但每个 SSE 间隔仍小于 45 秒 idle timeout。** 普通聊天生成预算由 `ThinkingDepth` 设为 8,192 / 16,384 / 32,768 tokens；因此该风险只在慢速、长输出请求中触发，不代表 DeepSeek 的 384K provider 上限直接进入普通聊天。

- 生产路径：`ReqwestTransport::stream()` 和 `stream_cancelled()` 都在 `tokio::time::timeout(self.timeouts.total, stream_inner(...))` 内运行；默认 `total=300s`、`idle=45s`。即使收到进展事件，外层总计时器也不会重置，最终返回 `model stream total deadline exceeded`。ModelAgent 每次请求经 `with_thinking_depth()` 设置输出上限；Medium 默认 16,384 tokens，Deep 为 32,768 tokens。超过 300 秒所需的平均速率分别约为 54.6 和 109.2 tokens/秒。
- 动态复现：opt-in loopback 探针 `reqwest_transport::tests::audit_active_provider_stream_is_not_cut_off_by_total_deadline` 用短 deadline 缩放相同生产 transport 逻辑。本机 fixture 每 50ms 发出合法 SSE progress，正式 transport 收到 4 个事件后仍在 `[DONE]` 前返回 `model stream total deadline exceeded`。后续 ModelAgent 探针 `audit_partial_response_is_streamed_but_not_committed_after_total_deadline` 用同一错误消息覆盖三次 attempt，观察到三次 `ContentDelta` 后最终失败，`response_history` 仍只有 seed user item、`pending_response_items` 为空。两项均为本地 fixture，不证明真实 DeepSeek provider 在用户部署中会以该速率持续生成，也未运行真实桌面。
- 成熟实现对照：DeepSeek Harness `packages/llm/llm-deepseek/src/adapter.ts` 与 `packages/llm/llm-pi-ai/src/adapter.ts` 使用可取消的 idle watchdog 包裹 async iterator，每次 `next()` 期间监测无活动时限，不因累计生成时间到期而主动中止活跃流。此处只引用其流生命周期实现，不声称请求预算或所有 provider adapter 完全相同。
- 桌面显示与恢复边界：`App.tsx::appendDelta()` 会把 `content_delta` 立即写入内存 transcript；`run_failed` 走 `finalize(..., "error")`，保留已有 text parts 并在尾部追加错误文本。已完成失败运行的 transcript 留在 `liveTranscripts`，同进程切换会话再返回仍显示半截内容；同进程下一次发送也以这份 live transcript 作为 UI seed，但 provider 的模型历史仍由后端 session Surface 构建。`AgentKernel::PersistentEventSink` 将流文本按 batch 写入 run-local `run_events`，但 `session_detail()` / `session_conversation_from_events()` 只从 session `EventStore` 重建；失败 attempt 未提交的 assistant item 不会成为 `MessageAppended`，因此整个应用重启后半截文本不会作为回答复活。取消路径不同：`finalizeCancelled()` 会移除 reasoning/output text。新增 AppCore SQLite 审计回归 `service::tests::audit_failed_stream_delta_is_not_replayed_as_an_assistant_answer`，证明 run ledger 中有 partial delta，session replay 仍只有用户消息和失败终态。
- 影响与边界：若慢速长输出越过 300 秒，请求会以 transport timeout 失败；用户当前运行时看到“半截回答 + 错误”，但这一内容只是进程内 UI 投影，不是后端 session transcript 或模型历史。退出应用后该半截回答消失，形成实时界面与重开会话不一致；同进程缓存期间后续 UI 也会继续保留它。尚未用真实 Tauri 窗口截图/手动操作验证视觉样式，本轮通过生产前端路径检查和 AppCore 持久化重放测试确认行为。建议将总时限收敛为可取消 idle watchdog；另需在 provider 失败 UX 上明确决定保留为“未完成片段”还是删除，不能把片段呈现成完整 assistant answer。

### H-132：Chat Completions 的合法内容过滤终态被当作未知 provider 错误，usage 未进入运行账本

**级别：低（P3，条件性）；触发条件：调用方使用公开 `WireMode::ChatCompletions` 路由，DeepSeek 对该请求返回合法 `finish_reason=content_filter`，且响应包含 usage。** 桌面 `model_runtime::select_run_model()` 默认使用 Responses，因此该项不解释桌面默认协议的过滤行为。

- 官方契约：DeepSeek [Chat Completions API](https://api-docs.deepseek.com/api/create-chat-completion) 的 `finish_reason` 枚举包含 `content_filter`，定义为内容因安全过滤被省略；同一接口定义 stream final usage chunk 可携带请求用量。官方说明允许过滤结果没有 assistant content，不能要求一定存在文本块。
- 生产路径：公开 `ModelClient::stream_response()` 在 `WireMode::ChatCompletions` 下进入 `response_to_chat_completion()` 和 `ChatCompletionAccumulator`。`map_finish_reason()` 只接受 `stop`、`tool_calls`、`length`，把合法的 `content_filter` 转为 provider `Err`；即便补上该映射，`[DONE]` 路径还会把无内容的过滤响应作为 `empty_response` 拒绝。`ModelAgent` 只在拿到 `Ok(Response)` 后调用 `record_provider_response_usage()`；`SinkObserver` 对流式 `Usage` event 不做处理，所以 adapter 返回 `Err` 后这份用量无法进入运行账本。
- 动态复现：新增 opt-in 测试 `client::tests::audit_chat_content_filter_preserves_typed_finish_reason_and_usage`，经正式 `ModelClient::stream_response()` 和 Chat Completions 路由送入官方形状的无内容过滤终态及 9 prompt tokens usage。预期是返回 `FinishReason::ContentFilter` 且保留 usage；当前会在 `content_filter` 映射处以 provider error 退出，`[DONE]` 后续路径也未被到达。使用 MockTransport，无公网或真实 provider 响应。
- 成熟实现对照：Codex 的 Responses adapter 将官方 `incomplete_details.reason=content_filter` 映射成独立 `ApiError::ContentFilter`，Runtime 能区分过滤与 transport failure；这是 H-82 中已经核对的 Responses 同协议处理，不等同于 DeepSeek Chat Completions。DeepSeek Harness 的默认 DeepSeek LLM adapter 使用 Messages 协议；其 pi-ai adapter 将终态 stop/error 分类保留到统一 finish event，本轮没有找到其对 DeepSeek Chat `content_filter` wire 值的同协议处理，不把它作为直接实现证据。
- 影响与边界：触发时用户看到 provider failure 而不是明确的内容过滤终态；尽管 provider 返回了 usage，当前 ModelAgent 路径无法记账，导致该请求的上下文/计费统计少记。此项限于显式 Chat Completions client；没有真实 DeepSeek 账号或桌面 Chat 路由复测。修复需让 Chat adapter 映射 `content_filter` 到已有 `FinishReason::ContentFilter`，允许该终态为空内容，并继续将 Response/usage 交给既有运行层处理。

### H-133：Responses `response.failed` 携带 usage 时未进入运行用量

**级别：低（P3，条件性）；触发条件：Responses provider 以 `response.failed` 结束，但完整 response 同时包含非空 usage。** 默认桌面运行使用 Responses 路由；该项不改变失败终态，只影响 provider 已返回用量的保留。

- 官方契约：DeepSeek [Responses API](https://api-docs.deepseek.com/api/create-response/) 将 `response.failed` 定义为携带完整 response 和 error details 的终态；同一 Response schema 定义 `usage` 及 input/output/cache/reasoning 统计。该契约允许失败终态携带 usage；本轮没有真实供应商失败样本证明每类失败都会返回非空用量。
- 生产路径：`ResponseAccumulator::push_sse_data_observed()` 对 `response.completed` 和 `response.incomplete` 解析并发出 `ModelStreamEvent::Usage`，但 `response.failed` 分支只取 `response.error` 并立即构造 `Err`。`ModelAgent::think_inner()` 仅在 `stream_model_attempt()` 返回 `Ok(Response)` 后调用 `record_provider_response_usage()`；其 `SinkObserver` 也忽略流式 Usage，因此错误终态中 provider 已提供的数值没有入口写入运行用量和 `RuntimeEvent::Usage`。
- 动态复现：ignored 探针 `client::tests::audit_failed_responses_terminal_emits_included_usage` 经正式 `ModelClient::stream_response_observed()` 和 Responses wire 路由发送 schema 允许的 `response.failed + usage`，断言保留 provider error 的同时 observer 收到 9 input、2 output、4 cached 和 1 reasoning tokens。当前调用正确返回 error，但没有发 Usage event，探针在 usage 断言处失败。使用 MockTransport，无公网或真实 provider 错误。
- 成熟实现对照：DeepSeek Harness `llm-pi-ai/src/stream.ts` 在 pi-ai `error` event 路径先发 usage，再发 error finish，`tests/convert.spec.ts` 覆盖 usage(1,0) 的错误终态；这是另一 provider adapter 的统一流事件生命周期模式，不声称其处理 DeepSeek Responses wire。Codex Responses adapter 将 `response.failed` 映射为独立 API error，而其 completed/incomplete 分支读取 `resp.usage`；本轮未找到 Codex failed event 将 usage 纳入错误结果的直接证据。
- 影响与边界：当失败响应确实包含非空 usage，当前运行事件和累计运行 token 统计会少记这部分 provider 报告值；不据此推断所有失败请求都会计费。修复需在既有错误返回边界保留可选 provider usage，并由运行层统一记账，同时维持失败分类、取消和单一终态语义。

### H-134：被替代的画布 run 迟到节点事件可覆盖当前状态

**级别：高（P1，工作流结果与任务身份错误）；触发条件：同一画布运行尚未完成时再次调用 `runWorkflow()`，且旧 run 的异步 Tauri 监听注册晚于新 run 完成。** 新 run 的节点状态已经更新后，旧 run 的合法事件仍可能覆盖它。

- 生产路径：`runWorkflow()` 遇到已有 `_abortController` 时只调用前端 `abort()`，没有按旧 `_currentRunId` 调用 `cancel_run`。`runBackendWorkflow()` 在 `start_workflow` ACK 后才注册 `chat://event` 与 `session://completed`；`listen()` Promise 尚未完成时，abort cleanup 拿不到对应 unlisten。Promise 随后完成仍无条件把旧 listener 写入模块级 `_eventUnlisten` / `_completionUnlisten`。节点回调只验证事件 run_id 等于它闭包中自己的 `ack.run_id`，没有再验证这个 run 仍是当前 owner，所以旧 run 的事件仍会通过过滤并写入共享节点 store。`session://completed` 对 run_id 不作过滤的问题另见 H-41。
- 动态复现：显式命令 `$env:RUN_HARNESS_AUDIT='1'; pnpm exec node --test --test-name-pattern='a superseded workflow attempt' scripts/audit-workflow-isolation.test.cjs`。探针延迟第一次运行的两条 `listen()` Promise，启动第二次运行并投递其 Running 事件，再放行第一次注册并投递旧 run 的 Completed 事件。实际输出 `staleAttemptOverwroteCurrent=true, cancelRequestsOnSupersede=0`，对应断言 **0 passed / 1 failed（预期暴露缺陷）**。它加载生产 `workflowExecutor.ts`，只替换 Tauri/store 边界；没有启动真实 Kernel、供应商或媒体任务。
- 成熟实现对照：Codex `app-server-protocol/src/protocol/v2/item.rs` 的 `ItemStartedNotification` / `ItemCompletedNotification` 同时带 `thread_id`、`turn_id` 和 item；`turn.rs` 的开始/完成通知也带 `thread_id` 与 turn。可借鉴在 item 与 turn 两级按 owner identity 归属事件。ArtCraft `frontend/libs/tauri-events/src/lib/events/functional/VideoGenerationCompleteEvent.ts` 将异步 `listen()` Promise 保存在 hook 所有者内，并在组件提前卸载后解除迟到注册的监听；这是 listener 生命周期模式，不负责同一组件内的新旧任务仲裁。
- 已直接证明：新 run 的节点状态从 Running 被旧 run 的 Completed 改写；重跑路径没有调用 `cancel_run`。`applyNodeOutputs()` 和 `applyNodeEventIdentity()` 也在同一个未按当前 owner 隔离的回调内更新结果及媒体身份字段，但本探针没有单独断言这些字段被旧输出覆盖；没有运行后端任务，不能断言供应商仍在后台生成或产生重复费用。
- 修复方向：将 controller、run_id、完成状态和两条 unlisten Promise 收进每次运行自己的 owner 对象；开始替代 run 前以旧 run_id 取消旧内核运行；每次异步注册完成时若 owner 已失效就立即 unlisten；节点和完成回调必须同时匹配当前 owner 与事件 run_id，清理只允许释放本 owner 的资源。该边界与 H-41 的外部完成串话、H-43 的订阅就绪竞态独立。本轮只增审计探针和文档，没有改生产行为。

### H-135：stdio MCP 隐式继承宿主凭据，配置环境不是授权边界

**级别：高（P1，有条件的凭据暴露风险）；触发条件：宿主进程环境实际含凭据，并启动了不应获得这些凭据的本地 stdio MCP。** 这是与成熟实现相比的默认环境授权缺口，不是 MCP 协议要求所有实现必须清理环境。

#### 修复前的生产路径与动态证据

- 正式宿主准备：`McpService::connect_one_resilient()` → `connect_transport()` → `StdioTransport::spawn()`；测试连接 `probe_with_timeout()` 和 `ConfigReconnectFactory::reconnect()` 也复用后两层。`crates/deepagent-mcp/src/stdio.rs:50` 创建普通 `tokio::process::Command`，仅 `.envs(&config.env)`，没有 `.env_clear()` 或统一环境过滤。显式配置覆盖同名值，并不限制其他宿主变量继承。
- `stdio.rs` 收集宿主/config 中的敏感值并在 stderr 诊断中脱敏，这是日志保护；该列表没有作为子进程环境策略使用，不能阻止 MCP 在自己的进程中读取凭据。
- 新增显式探针 `crates/deepagent-mcp/tests/env_inheritance_audit.rs`。外层通过当前测试 exe 启动独立宿主，先清空环境，仅保留运行基础变量，再注入三条随机临时目录 nonce 派生的虚构 canary；不修改父进程环境。隔离宿主通过正式 `connect_transport` / `McpClient` 启动真实 Node stdio fixture，完成 initialize 和 tools/list；fixture 只返回存在性/匹配布尔值，不返回环境原值。
- 缺陷命令：`cargo test -p deepagent-mcp --test env_inheritance_audit audit_stdio_does_not_inherit_ambient_credentials --offline -- --ignored --exact --nocapture`。实际 **0 passed / 1 failed（预期暴露缺口）**，运行约 0.85s：空 `config.env` 组 `ambient_secret_present=true, ambient_secret_matches=true, lowercase_token_present=true, lowercase_token_matches=true`；普通变量对照及显式覆盖控制断言先通过，随后在“未声明 secret 不应进入 MCP”处失败。小写 token 同样继承由 fixture 观察值直接确认，不能把尚未执行到的后续断言说成独立失败。
- 正向对照：`cargo test -p deepagent-mcp --test env_inheritance_audit audit_stdio_preserves_explicit_environment_overrides --offline -- --ignored --exact --nocapture`，**1 passed**，运行约 0.34s；显式 secret/普通变量覆盖正确，旧 secret 值不匹配。两组子进程均正常关闭；单次 MCP 请求、Node fixture 和隔离宿主分别有 5s/15s/20s 上限。

#### 成熟实现对照

- **Codex**：`借鉴/codex/codex-rs/rmcp-client/src/utils.rs::create_env_for_mcp_server()` 从基础变量白名单、显式 env_vars、CA 配置及 extra_env 构建最终环境，并过滤不可继承变量；Windows 同名覆盖考虑大小写。`stdio_server_launcher.rs:276` 使用该 builder；这里的 `Command` 是 `codex_utils_pty::Command`，其 `utils/pty/src/child_command.rs:96` 构造器明确 `.env_clear()`，因此白名单不会被普通 Command 的默认继承绕过。只核对参考源码，没有运行 Codex 测试。
- **DeepSeek Harness**：`packages/mcp/mcp-client/src/transport.ts::buildChildEnv()` 将 `scrubbedParentEnv()` 与显式配置合并；`packages/subprocess/subprocess/src/index.ts` 的基础环境过滤大小写不敏感的 `KEY|PASSWORD|SECRET|TOKEN` 及 `DSH_*`，保留运行/代理所需环境，显式配置在过滤后叠加。这提供了共享启动边界，不是每个 MCP 各写一份过滤。
- **Claude Code 的差异**：`restored-src/src/services/mcp/client.ts` 同样从 `subprocessEnv()` 再叠加 server env；但 `utils/subprocessEnv.ts` 默认仍继承环境，仅当 `CLAUDE_CODE_SUBPROCESS_ENV_SCRUB` 启用时清理列明凭据及 `INPUT_*` 副本，并保留其明确允许的工具凭据。不能声称所有成熟 Agent 默认全面清理，或把该条件策略描述成默认白名单。

#### 影响、修复方向与限制

- 已直接证明：MCP 在未通过 `config.env` 声明时，也能读取宿主实际存在的凭据形态变量。若本地插件/依赖不可信或被替换，可能把这些凭据用于自身请求、日志或外传；这些是触发后的风险，未在本探针中执行。凭据原本不在宿主环境时，本路径不会凭空读取数据库或 keychain；不能据此宣称所有供应商 Key 已泄漏。
- 在现有统一子进程启动边界定义可解释的继承策略：基础环境、显式授权转发和配置覆盖分层，Windows 名称大小写一致；启动、测试连接和重连共用同一结果。默认白名单或敏感变量清理的取舍属于本地设计，应考虑代理、CA、包管理器及需要明确凭据的 MCP，不能直接把日志 `is_secret_key` 当作授权规则，也不能只在一个调用方加过滤。
- 后续验收应覆盖空 env、显式转发/覆盖、大小写、代理/CA、重连及普通工具兼容；审计记录和错误中不输出环境值。清理环境并不实现 OS 沙箱，也不能阻止同一 OS 用户下未隔离进程使用其他渠道访问资源。当前仅 Windows 本机生产 transport 复现；未做 macOS/Linux、完整 PluginService/AppCore 到模型 wire、真实用户凭据或公网外传验证。探针默认 ignored，本轮不修改生产环境策略。

### H-136：stdio MCP 请求写入受背压阻塞时，停止信号无法结束发送等待

**级别：高（P1，停止延迟与取消后的迟到投递）；触发条件：本地 MCP 暂停或足够慢地读取 stdin，请求大小足以让管道写入持续等待，用户在此时停止。** 本探针以 512 KiB 虚构参数确定性制造背压，初轮 2 MiB 同样复现；不代表普通短参数都会触发。

#### 生产路径与动态复现

- 正式路径：`ChatService::run_in_session()` → Kernel/RuntimeEngine → `ToolPipeline::execute_prepared()` → `ToolRegistry::invoke_with_context()` → `McpToolAdapter` / `McpRegistry` → `McpClient::call_tool_with_context()` → 当前连接 transport → `StdioTransport::send_with_context()`。既有 H-128 已贯通 context，但 stdio 的专用实现先 `self.write_request(request).await?`，随后才检查取消并进入监听取消的响应 `select!`。
- `write_request()` 的 stdin 锁等待、`write_all()` 及 `flush()` 均未监听工具 context。通用 `McpTransport::send_with_context()` 原本有覆盖整个 `send()` 的取消 select，但 stdio override 不经过该默认实现；Runtime 的顺序 ToolPipeline 外面只有工具预算 timeout，没有独立的取消竞速。因此树上的信号已触发，当前执行仍可能等到写入恢复或工具预算到期。默认预算来源为 `ToolExecutionContext::DEFAULT_TIMEOUT`（120s），这里不称其必然永久挂起。
- 在既有 `kernel_v2_e2e/mcp_cancellation.rs` 共用正式运行夹具增加 ignored 探针，原响应等待测试仍保留。Node 正常完成 initialize/tools/list；接收工具请求首个数据块后暂停 stdin，并原子发布 gate 证明运行已进入实际发送阶段。模型为离线 fixture，工具参数是 512 KiB 重复字符，SQLite/Kernel/McpService/stdio 都使用正式实现。
- 命令：`cargo test -p deepagent-app-core --test kernel_v2_e2e mcp_cancellation::audit_stopping_run_interrupts_backpressured_stdio_write --offline -- --ignored --exact --nocapture`。实际 **0 passed / 1 failed（预期暴露缺口）**，运行约 1.68s：`argument_bytes=524288, bytes_read_before_pause=65536, accepted=true, run_still_waiting_after_250ms=true, cancellation_notification_received=false`。恢复读取前服务端未收到完整请求；恢复后 `complete_request_received_after_release=true`，最终终态 `cancelled`、模型请求数 1。初轮使用 2 MiB 参数时也在同一停止断言失败（约 1.32s），其余观察相同。先释放 gate、join 运行并关闭 MCP，随后断言停止等待失败，不留测试子进程。

#### 成熟实现对照

- Codex `借鉴/codex/codex-rs/core/src/tools/parallel.rs::handle_tool_call_with_source()` 将整个 dispatch handle 与 run cancellation 放入 `tokio::select!`；取消时中止并等待活动 dispatch，并生成 paired aborted result。这个外层执行边界不依赖某个具体 transport 是否已经进入响应读取。这里只核对源码，不据此宣称远端操作被回滚或不会收到已经发出的字节。
- DeepSeek Harness `packages/mcp/mcp-client/src/tools.ts::syncTools()` 把每次工具的 `execution.signal` 与调用 timeout 一起交给 `client.callTool()`；`packages/core/tools/src/index.ts` 要求异步实现观察/转发 signal 并在取消后达到静止状态。对照的是整次调用拥有同一取消上下文；未运行其 SDK 背压测试，也不推断其具体管道中断实现。

#### 后果、修复方向与限制

- 已证明停止受理后本地发送等待仍未退出；释放背压才继续投递完整调用。这会造成 UI 长时间停在停止中，并让服务端在用户停止后仍首次拿到完整工具请求。fixture 只返回文本并写临时标记，没有真实文件修改/网络写入；不能将迟到投递说成已发生真实业务副作用，也没有等满默认 120 秒。
- 在现有执行 owner 内覆盖锁等待、发送和读取的取消边界；本地停止不能以协议取消通知写入成功为前提。还必须明确部分帧已写入时连接的状态：简单丢弃 `write_all` future 后继续复用同一 newline JSON 管道，可能留下半帧；要按既有连接生命周期决定退役/重建或有界完成，并保留结果未知语义。不能为单个工具另造取消树，也不能取消其他会话的运行。
- 后续验收需同时覆盖请求未发送、部分写入、stdin 锁排队、响应等待、重复停止、共享连接及重连；保证终态一次、配对失败结果和无新模型步骤。当前仅 Windows 本机故障注入；未覆盖 macOS/Linux、真实 Tauri UI、真实 MCP 副作用、共享服务取消隔离及自然背压发生频率。本轮仅增加测试和报告，生产行为未改。

### H-137：MCP 工具目录变更通知被丢弃，后续运行继续发布旧工具

**级别：中（P2，动态能力陈旧）；触发条件：已连接的 stdio MCP 在运行期间新增、删除或更新工具，并通过声明的 `tools.listChanged` 能力发出变更通知。** 静态工具目录不触发本项；不能据此宣称所有 MCP 调用不可用。

#### 生产路径与动态证据

- 正式路径：宿主 `McpService::start_preparation()` → `prepare_server_with_cancel()` → `McpRegistry::register_with_instructions()` 首次 `tools/list` → 发布 Ready 快照；聊天 `mcp_runtime::attach_mcp_tools()` 克隆该快照，并将 adapters 注册进现有 ToolRegistry。`ConnectedServer.tools` 保存首次发现的 Vec，`extend_ready()` 克隆这份目录；`mcp_preparation.rs` 只有配置/运行包变更触发重新准备，没有工具变更通知处理入口。
- `crates/deepagent-mcp/src/stdio.rs::read_response()` 只尝试反序列化 `JsonRpcResponse`，仅将 ID 等于当前请求的响应返回。标准 `notifications/tools/list_changed` 没有 ID，因缺少必需 ID 而反序列化失败，随后被跳过；这次观察与 H-09 的并发响应错配不同，没有并发工具调用。
- 新增 `crates/deepagent-app-core/tests/kernel_v2_e2e/mcp_catalog_change.rs`。真实 Node 完成握手并声明 `tools.listChanged=true`，首次 tools/list 只有 initial_tool。正式 ChatService/Kernel 调用该工具时，服务端先把目录改为 updated_tool，在同一 stdout 管道上先写通知、再写当前调用成功响应。第一轮正常结束证明生产读取循环已越过通知；随后最多观察 Ready 快照 1 秒，再开始同会话中的另一独立 run，捕获正式 provider 序列化请求。模型和目录查询为离线 fixture，不访问公网。
- 命令：`cargo test -p deepagent-app-core --test kernel_v2_e2e mcp_catalog_change::audit_mcp_list_changed_reaches_next_run --offline -- --ignored --exact --nocapture`，实际 **0 passed / 1 failed（预期暴露缺口）**，运行约 1.70s。服务端报告 `initialize_requests=1, list_requests=1, notifications_sent=1, current_tools=[updated_tool]`；宿主 Ready、首轮和下一 run 的 wire schema 均仍为 `mcp__catalog_audit__initial_tool`，模型请求数 3，两次运行均正常结束。先关闭真实子进程再在 Ready 目录断言处失败；下一 run 的旧 schema 由捕获结果直接证明，不能把尚未执行到的最后一条断言描述为另一次失败。
- 不变目录正向对照 `unchanged_mcp_catalog_is_preserved_between_runs` 实际通过（约 0.63s）：初始化和 tools/list 各一次、通知零次、两个 run 的真实 schema 都保留 initial_tool。两轮运行各有 5 秒上限，子进程有 15 秒兜底；1 秒仅是本地刷新观察窗口，不能推断协议要求在 1 秒内更新。

#### 规范与成熟实现对照

- 当前客户端声明的协议版本为 2024-11-05。[MCP 工具规范](https://modelcontextprotocol.io/specification/2024-11-05/server/tools)定义 `tools.listChanged` 与 `notifications/tools/list_changed`，声明该能力的服务器在目录变化时 **SHOULD** 发送通知；这不是“所有客户端 MUST 自动刷新”的强制要求。本项作为本系统动态能力管理缺口记录。
- **Claude Code**：`借鉴/claudecode/restored-src/src/services/mcp/useManageMCPConnections.ts:616` 在服务端声明能力后注册 `ToolListChangedNotificationSchema` handler；收到通知删除 `fetchToolsForClient` 的该服务缓存，重新发现并 `updateServer({...client, tools:newTools})`。失败会记录具体服务错误，没有要求下一轮用户对话承担初始化工作。
- **DeepSeek Harness**：`借鉴/deepseek-harness/packages/mcp/mcp-client/src/connection.ts:263` 配置 listChanged 的 onChanged → refreshTools → enqueueSync；同步队列串行处理初始和通知后的工具同步，`isCurrent(generation)` 防止卸载/替换连接的旧任务继续生效。这里对照的是连接拥有通知和工具注册的生命周期，没有运行参考项目测试。
- **Codex 的差异**：`借鉴/codex/codex-rs/rmcp-client/src/logging_client_handler.rs:86` 的 `on_tool_list_changed()` 仅记录日志；不能把该 handler 描述为已经刷新通用工具目录，也不能据此声称所有成熟 Agent 都实现相同策略。本项已核对到的明确刷新实现来自 Claude Code 和 DeepSeek Harness。

#### 影响、修复方向与限制

- 已证明服务端目录已经替换，而下一独立 run 仍向模型公布旧工具。若删除工具，模型可能选择已不存在的能力并收到失败；若新增工具，模型看不到；若同名 schema 改变，旧参数定义可能导致 validation/远端拒绝。这些后果由目录陈旧推导，当前 fixture 没有在第二轮实际调用旧/新工具，也未验证真实供应商的选择或重试成本。不能将未知发生频率说成所有请求持续失败。
- 在现有 transport/client 的消息分发及 McpService 生命周期内接收通知，按服务有界地重发 tools/list，并经现有权限、名称唯一性和配置代次检查发布新快照。就绪工具定义与调用路由仍引用同一快照，已发出请求保持其版本，下一边界采用新目录；刷新串行化、重复通知合并、失败保留/退役策略及连接替换后的旧结果拒绝属于本地设计，需要明确并验证。不要让会话请求重新建连，也不要另建工具注册表或只刷新设置页面。
- 后续验收需覆盖新增、删除、同名 schema 更新、通知风暴、刷新失败/超时、禁用及连接代次竞争，并同时检查 schema 和实际调用路由。当前仅 Windows 本机 stdio、同会话连续两个正式 run 与离线模型，未覆盖 HTTP/SSE、真实 Tauri、macOS/Linux、真实 DeepSeek 或其他服务端；默认 ignored 探针保留缺陷，生产行为未改。

### H-138：MCP 结果整对象进入模型上下文，客户端元数据没有独立可见性边界

**级别：中（P2，有条件的数据最小化与上下文隔离风险）；触发条件：MCP 把仅供客户端/UI 使用的数据放在结果顶层 `_meta`，并通过本系统执行工具。** 这不是任意 MCP 元数据天然包含秘密，也不代表本系统已经支持完整 MCP Apps 协议。

#### 生产链与实测

- `ToolCallResult.extra` 以 serde flatten 保留 `structuredContent`、`_meta` 等额外字段；这是必要的无损接收能力。`McpToolAdapter::invoke()` 与 `invoke_with_context()` 随后直接 `serde_json::to_value(result)`，连同这些字段形成 ToolOutput；`ModelAgent::observe()` 将 observation.output 全部包入 `{status,result}` 并序列化为 `FunctionCallOutput`。没有从原始结果到模型可见内容的投影步骤，保存/审计元数据和公开给模型使用了同一个值。
- 复用 `kernel_v2_e2e/mcp_image_output.rs` 的真实 Node → McpService → ChatService/Kernel → provider wire 夹具，抽取共用结果 fixture，保留 H-127 原图像探针。新增 metadata 探针仅向 Node 返回结果的 `_meta.audit.example/clientState.privateDetail` 写入固定虚构 canary；公开 `content` 文本和 `structuredContent` 各有独立标记，提示、工具参数和首次模型请求没有 canary。
- 命令：`cargo test -p deepagent-app-core --test kernel_v2_e2e mcp_image_output::audit_mcp_client_metadata_is_not_sent_to_model --offline -- --ignored --exact --nocapture`，实际 **0 passed / 1 failed（预期暴露缺口）**，运行约 0.35s。输出 `model_requests=2, public_text_present=true, structured_content_present=true, private_before_tool=false, private_after_tool=true`。正式序列化的下一请求带入客户端 canary；缺陷断言前已经关闭真实 MCP 子进程，没有对公网发送或读取真实凭据。

#### 成熟实现与契约边界

- **DeepSeek Harness**：`借鉴/deepseek-harness/packages/mcp/mcp-client/src/tools.ts::createExecutor()` 在校验上游 CallToolResult 后，显式构造只含 content 与可选 structuredContent 的 McpResult；原始调用由调用方提供的 `options.call` 控制，canonical 模型结果不默认包含顶层 `_meta`。这里对照的是原始协议对象与模型结果的职责分离，没有运行参考实现。
- **Codex**：`借鉴/codex/codex-rs/core/src/tools/context.rs::McpToolOutput` 分别实现 `to_response_item()` 和 `post_tool_use_response()`；前者通过 `response_payload()` 调用 `protocol/src/models.rs::CallToolResult::as_function_call_output_payload()`，从 content 或 structured_content 构造模型输出，不序列化顶层 meta。后者仍能把完整结果交给 hook，metadata capture 另有资格标记。不能说它删除了原始元数据，也不能把正文中受识别的图像/encryptedContent 元信息与顶层 metadata 混为一谈。
- **Claude Code**：`借鉴/claudecode/restored-src/src/services/mcp/client.ts:1897` 将处理后的 `mcpResult.content` 放入 data，将 `_meta` 与 structuredContent 放入单独 mcpMeta 通道；`McpToolCallError` 同样单独保留 error result 的 `_meta`。这证明其有专门元数据通道，不能把所有消费者的行为都归结为删除元数据；本次仅按已核对源码描述。
- [OpenAI 插件工具结果文档](https://developers.openai.com/plugins/reference#tool-results)明确区分 content/structuredContent 的模型可见性与 `_meta` 的 component-only 用途；[MCP Apps 模式](https://apps.extensions.modelcontextprotocol.io/api/documents/patterns.html#persisting-view-state)展示用 CallToolResult._meta.viewUUID 恢复 UI 状态。这些属于应用/客户端使用语义，**不能套用为当前客户端声明的 2024-11-05 基础 MCP 对所有元数据的通用强制保密规则**。本项按成熟宿主的数据边界差异记录，不将未来扩展未实现本身算作协议违规。

#### 后果、收敛方向与限制

- 已直接证明：只出现在客户端 metadata 中的内容会进入模型 wire。若实际服务把 UI 明细、内部关联数据或大表放在该通道，将扩大供应商可见数据范围和上下文占用；可能的敏感数据披露和额外 token 成本依赖实际内容，本探针只用短虚构 canary，没有量化计费或证明真实秘密泄漏。
- 在既有 MCP adapter/工具 observation 边界明确原始结果、宿主元数据和模型内容的投影契约；保留可审计原始数据与所需 provenance，不以“修复”为由丢失本地诊断。模型结果从明确字段构造，typed image/resource、公开 structuredContent、isError 与调用配对均需保留；允许的元数据如何显式进入模型属于本地设计，不应默认序列化整个协议对象，也不另建事件/工具存储。
- 当前仅 Windows 本机 stdio 与离线模型，未验错误结果、嵌套 metadata、HTTP/SSE、重启回放、真实 MCP Apps UI、真实供应商或 macOS/Linux；不将此探针视为完整数据流安全审查。生产行为未改。

### H-139：后置 hook 报错跳过已执行工具的结果提交，重开后成功写入变成结果缺失

**级别：高（P1，条件性的副作用与历史不一致）；触发条件：串行工具已经实际执行成功，随后配置的 PostToolUse hook 返回执行错误。** 本次用正式 prompt hook 返回无效决策 JSON 触发；不将普通 command hook 的非零退出、所有 hook 或默认无 hook 对话视为已覆盖。

#### 生产链与动态复现

- `ToolExecutionPipeline::complete_prepared_inner()` 先处理实际工具输出并发出成功 `RuntimeEvent::ToolCompleted`，随后 `fire(AfterToolUse).await?`，之后才转换 action 的 Completed 状态并返回结果。`HookRegistry::dispatch()` 在检查该 point 是否可 veto 之前执行 `result?`；因此即使 AfterToolUse 是观察点，执行错误仍能提前结束流水线。这与明确返回 Deny 后被非 veto 点忽略的行为不同。
- 正式 `AppHookActionExecutor::execute_prompt()` 通过 ModelClient 获取回复，`parse_model_hook_decision()` 对普通文字返回 `model-backed hook must return JSON` 错误；错误经 registry/pipeline 一直传播。`RuntimeEngine::execute_tool_with_before_and_speculative()` 没有拿到 ToolPipelineResult，后面的 `session.append(ToolCallCompleted)` 和模型 observe 均未执行。Kernel 可以正常保存唯一 failed 运行终态，但它不能据此复原已经成功的工具结果。
- 新增 `kernel_v2_e2e/post_tool_hook_commit.rs`：正式 ChatService → Kernel → write_file 创建临时 `side-effect.txt`；配置匹配 write_file 的 PostToolUse prompt hook。离线 transport 分别给主请求和 hook 请求返回工具调用、有效 `{"ok":true}` 或非 JSON 文字，捕获真实序列化请求；不伪造 RuntimeEvent、action 状态或会话事件。第一轮结束后确认真实文件内容，关闭全部第一轮服务和数据库，重开同一个数据库并调用正式启动恢复，再继续原会话。
- 实跑 `cargo test -p deepagent-app-core --test kernel_v2_e2e post_tool_hook_commit --offline -- --include-ignored --nocapture`：**1 passed / 1 failed（缺陷探针预期失败）**；初轮约 4.73s，补充关闭前 action 观察后的复测约 0.82s。正常组第一轮为 succeeded、工具成功事件 1 条、session 工具结果 1 条；重开 action 保持 Completed，下一 wire 的 status=ok、result 与实际工具输出完全一致。错误组第一轮为 failed、成功工具事件同样 1 条，但 session 工具结果为 0，关闭前 action 仍为 Running；重开 action=Failed，原因是 `process_restarted_while_running`，下一 wire 的同 call_id 输出为 `status=error` 和 `tool result missing; the previous tool execution was interrupted or its result was not persisted`。`Database::open()` 已调用 RunControlStore::recover，所以错误动作在正式 AppService 恢复之前便已被归类；运行本身早已终结，AppService 没有待恢复的 run。
- 错误组主模型请求 1 次、hook 请求 1 次，正常组主模型 2 次、hook 1 次；两组重开续聊各新增一次主请求，没有重执行写入。两组第一轮均只有一个规范终态。HookCompleted 的 continued/error 从正式实时回调观察，**没有错误声称它属于当前 run_events 事件流**；run_events 中保留的成功 ToolCompleted 是独立的实际结果证据。

#### 成熟实现对照

- **Claude Code**：`借鉴/claudecode/restored-src/src/services/tools/toolExecution.ts` 先对非 MCP 工具调用 `addToolResult(toolOutput, mappedToolResultBlock)`，随后消费 `runPostToolUseHooks()` 产生的附加消息；`services/tools/toolHooks.ts::runPostToolUseHooks()` 将已识别的 hook 错误/阻断/取消作为附加消息，外层捕获并记录执行异常，不让异常抹掉已产生的普通工具结果。MCP 因支持结果修改而另有顺序，本项不把非 MCP 顺序强行套到所有工具。
- **Codex**：`借鉴/codex/codex-rs/hooks/src/events/post_tool_use.rs` 把执行错误记录为 HookRunStatus::Failed 的 hook event，并返回结构化 PostToolUseOutcome；`core/src/tools/registry.rs` 分开处理原工具 lifecycle outcome、hook 的 should_block 和反馈。源码明确说明 PostToolUse block 拒绝的是结果，不是已经完成的执行；需要拒绝模型结果时返回 RespondToModel，而不是留下未收尾的执行动作。这里对照职责分离，不声称 Codex 无条件把被 hook 阻断的原始结果提供给模型，也没有运行参考项目。

#### 修复前后果与修复方向（历史记录）

- 已证明：实际磁盘变更和成功 run event 仍存在，但模型历史失去真实工具结果，重开后又被标为进程重启失败。模型可能把已完成的修改、命令或外部操作当作未完成并选择重试；**本探针没有执行第二次写入或真实外部副作用**，不能把这种风险写成已发生的重复操作。它也不等于原始结果在所有持久化源中彻底丢失，成功 ToolCompleted 仍保存在 run_events。
- 应在既有 ToolPipeline / action / Session 边界明确“实际执行完成”“hook 反馈或拒绝后续使用”“模型可见结果”的分别收尾契约：已有执行事实及真实结果先获得可回放记录，再运行可失败的后置扩展；若 hook 要阻断继续，仍以同 call_id 返回可解释的配对反馈，并保留实际副作用与原结果。明确观察 hook 错误的继续/停止策略属于本地设计，不能靠吞掉所有 hook 错误、删除原调用、制造未知工具或重执行来修复，也不能另建结果存储。
- 与 H-83 区分：H-83 是最终完成 hook 报错导致同一 turn 的实时终态冲突；本项两组均唯一终态，缺陷是**工具已执行到 session/action 提交之间**的边界。与 H-103 的 hook 输入字段缺失也不同；即使补齐输入字段，后置执行错误仍须正确收尾。
- 当前只动态覆盖 Windows、单个串行 write_file、PostToolUse prompt 的有效/无效 JSON、文件数据库重开、正式恢复与离线 provider wire；未覆盖并行组、PostToolUseFailure、FileChanged、取消、HTTP/MCP hook、DB 写入故障、真实 Tauri、macOS/Linux 或真实 DeepSeek。生产代码未改，缺陷探针保持 ignored，默认测试通过不表示该缺口已修复。

### H-140：输入 hook 成功输出的上下文被丢弃，主模型只收到原始输入

**级别：中（P2，条件性的模型上下文缺失）；触发条件：配置的 UserPromptSubmit command hook 以退出码 0 返回普通文本上下文，或标准 `hookSpecificOutput.additionalContext`。** 直接用户输入及顶层拒绝决定的正常对照均成立，不将默认无 hook 对话判定为受影响。

#### 生产链与动态证据

- `hook_assembly.rs::register_user_hooks()` 通过现有 HookDefinitions、ObservableHookRunner 和 SystemHookRunner 注册真实命令。`run_assembler.rs` 在建立当前用户 turn 前调用 `submit_user_prompt()`；后者进入 `RuntimeEngine::submit_prompt()` 和 HookRegistry。该入口的输入改写分支确实会把 effective_prompt 同时交给后续模型装配与记录，不应仅凭代码片段声称改写只进入 task；本轮转而验证参考实现具有明确消费路径的追加上下文契约。
- `external_hooks.rs::interpret_result()` 将退出码 0 且没有识别到决定的输出变为 `HookOutcome::Continue`。`parse_structured_hook_output()` 没有解析 additionalContext，普通非 JSON 文本也没有上下文承载；HookOutcome 只有决定/输入改写/异步等待，没有与决定并存的追加内容。`submit_prompt()` 对 Continue 返回原始 prompt。ObservableHookRunner 虽然在实时 HookCompleted 中保留 stdout，模型装配并不消费它，所以“命令执行成功、输出可见”不能证明模型收到了内容。
- 新增 `kernel_v2_e2e/prompt_hook_context.rs`：通过正式 SettingsService 配置用户作用域 hook，真实 PowerShell 读取 stdin 并保存到临时文件，然后输出固定虚构 canary 或标准 JSON。正式 ChatService/RunStore/Kernel 与离线 recording transport 驱动下一实际请求；严格核对 hook stdin 中的事件、session 与 prompt，实时完成事件的唯一性、退出码、完整 stdout、决定，运行终态唯一性，以及实际序列化 wire 和会话 MessageAppended/ResponseItemAppended 中的 canary。
- 实跑 `cargo test -p deepagent-app-core --test kernel_v2_e2e prompt_hook_context --offline -- --include-ignored --nocapture`：**1 passed / 1 failed（缺陷探针预期失败）**，测试执行约 2.59s。直接输入对照：hook 退出 0、continued、模型请求 1、wire_context=true、history_context=true。顶层 block 对照：退出 0、blocked、模型请求 0。普通 stdout 和标准 additionalContext 两组：退出 0、continued、模型请求均为 1，但 wire_context=false、history_context=false；四组都记录唯一 succeeded 终态。block 组的 succeeded 描述的是当前已有终结行为，本项不另据此认定新的终态缺陷。
- HookCompleted 来自正式 live 回调，**没有把它当作已落入 run_events 的 hook 审计记录**。history_context 只检查会话 MessageAppended/ResponseItemAppended，未把日志或 hook 配置中的 canary 冒充模型历史；没有伪造事件、替换生产 parser 或调用公网模型。

#### 成熟实现对照

- **Claude Code**：`借鉴/claudecode/restored-src/src/types/hooks.ts` 明确定义 UserPromptSubmit 的 additionalContext；`utils/hooks.ts` 按 hookEventName 提取该字段。`utils/processUserInput/processUserInput.ts` 先处理 blockingError/preventContinuation，再将 additionalContexts 创建为带 hook 来源的 `hook_additional_context` 消息加入待查询内容。决定与模型可见上下文分别处理，没有用 Continue 抹掉允许后的内容。
- **Codex**：`借鉴/codex/codex-rs/hooks/src/events/user_prompt_submit.rs` 的 outcome 同时包含 should_stop、stop_reason、additional_contexts 和 hook_events；成功非 JSON stdout 与解析出的 additional_context 均有独立的模型上下文通道，输出过大时经过既有 spiller。`core/src/session/turn.rs` 把该上下文与接纳输入一起记录；`core/src/hook_runtime.rs::record_additional_contexts()` 将其转换为原生 ResponseItem 并经现有 conversation 记录路径写入。此处仅对照仓库源码，未运行参考项目，也不把具体角色或权限策略直接套用到 DeepSeek。

#### 后果、修复方向与验证限制

- 依赖输入 hook 自动补充的项目约定、运行环境或动态检索内容可能在 hook 显示成功的同时对模型不可见。本轮直接证明内容缺失，**没有评测实际回答质量、证明模型忽略了其他已有规则，或证明权限/沙箱被绕过**。用户通过原始输入显式提供同样内容时能够进入请求；原始用户消息未因此被删减。
- 应在既有 HookOutcome/dispatch/PromptDecision 边界分开承载决定、输入修改、附加上下文与诊断，验证事件类型后有界收集上下文，在同一次用户输入接纳中写入既有原生 Surface 和会话事件，再由统一 meter 计量。多个 hook 的上下文不能因决定优先级而互相覆盖；需要明确来源、长度及允许的信任等级。这是本地收敛设计，不新增提示词存储，不以改写原始用户消息或直接把所有 stdout 提升为 system 指令代替上下文通道，也不能从 UI 日志反向拼接历史。
- 与 H-26 共享外部输出解析层，但本项补充的是它尚未动态覆盖的**允许后追加上下文的交付**：顶层 block 正常不能修复内容丢失；仅补嵌套 permissionDecision 同样不能修复普通 stdout 或为非 veto 观察点承载附加内容。应在同一输出契约收敛，避免为每个事件复制 parser。它也与 H-111 的 PreCompact 指令未进入摘要请求不同，本项针对正式主模型输入。
- 当前仅 Windows、用户作用域的同步 PowerShell UserPromptSubmit、短文本/标准 JSON、单 hook、单轮离线 Responses wire。未动态验证 SessionStart/SubagentStart/工具 hooks、多 hook 合并、HTTP/MCP hooks、取消、长输出、续聊、压缩、数据库重开、真实 Tauri 或真实 DeepSeek，也不据此宣称上述路径同样失败。生产行为未改；缺陷测试保持 ignored，正常对照为默认测试。

### H-141：周期 todo reminder 在独立用户回合之间被重置，长期会话永不触发

**级别：中（P2，长期任务的计划提示缺失）；触发条件：同一 session 通过多个独立用户输入继续对话，期间模型没有调用 `todo_write`。** 单个 `ModelAgent` 内部连续步数的现有单元测试仍可通过；缺陷发生在正式 `ChatService::run_in_session` 为每个用户回合重新组装 agent 的边界。

#### 生产链与动态证据

- `run_assembler.rs` 每次组装正式运行都会调用 `ModelAgent::new(...)`，随后只通过 `with_history(...)`/`with_response_history(...)` 注入持久化会话历史；这两个恢复入口只重建 Surface 和 Message 投影，没有恢复 todo reminder 的两个计数器。
- `ModelAgent::new()` 将 `turns_since_todo_write` 设为 `0`，将 `turns_since_todo_reminder` 设为阈值；`maybe_inject_todo_reminder()` 只在当前 agent 的模型请求前递增计数。一次普通用户回合通常只经过一次该检查，因此跨回合历史不会累计。
- 新增 `kernel_v2_e2e/todo_reminder_lifecycle.rs`：使用正式 `ChatService`、`RunAssembler`、`AgentKernel`、同一 session 的 11 个独立输入和离线 Responses transport，捕获每次实际序列化的请求 body。模型每轮都返回成功文本，不调用 todo 工具。
- 显式执行 `cargo test -p deepagent-app-core --test kernel_v2_e2e todo_reminder_lifecycle::audit_todo_reminder_counts_survive_independent_session_turns --offline -- --ignored --exact --nocapture`：**0 passed / 1 failed（缺陷探针预期失败）**，约 1.47 秒；输出 `independent_turns=11 requests=11 reminder_requests=[]`。这证明周期提醒没有从前 10 个已持久化 assistant 回合恢复计数。

#### 成熟实现对照

- **Claude Code**：`借鉴/claudecode/restored-src/src/utils/attachments.ts::getTodoReminderTurnCounts` 每次构造附件都从完整 `messages` 逆向扫描，独立计算最近 `TodoWrite` 和最近 reminder 之后的 assistant 回合数；计数不绑定某个 agent 实例的内存寿命。
- 本系统现有 `TodoReminderAdapter` 正确读取当前 TodoStore，但它只提供列表快照；计数状态仍在 `ModelAgent` 内存字段中，导致会话恢复能保留计划内容却不能保留 reminder 节奏。这里采用参考实现的生命周期原则，不复制其 TypeScript 结构。

#### 后果、修复方向与验证限制

- 长任务如果每个用户输入都触发一次独立 run，会持续得到新的模型请求却永远不到提醒阈值；用户仍可能看到 todo 列表工具，但不会收到“长期未追踪计划”的周期性提示。本探针没有评估模型最终任务质量，也没有把提醒缺失扩大为 todo 数据丢失。
- 应在既有 session event/history replay 边界计算两个计数：从最近一次 `todo_write` 对应的原生 tool call 和最近一次周期提醒的原生 reminder item 之后，统计 assistant turns；每次压缩、snip、重启和分支恢复都应使用同一历史过滤规则。运行中的计数可作为缓存，但不能成为唯一事实来源。
- reminder 注入必须继续走现有原生 Surface 和消息投影，计数恢复不能新建第二个 TodoStore 或独立会话状态表；`todo_write` 仍只重置 write 轴，提醒注入后只重置 reminder 轴。
- 当前探针使用离线模型和空 Todo 列表，仅验证提醒正文是否进入真实 wire；未覆盖 `todo_write` 后重置、压缩后事件边界、分支回退、并行工具、CLI/SDK 入口及真实 DeepSeek。生产代码未改，缺陷测试保持 ignored。

### H-142：运行事件持久化失败被吞掉，实时事件与回放事件发生分叉

**级别：高（P1，事件回放不完整）；触发条件：`run_events` 的单条运行事件写入失败，而运行仍继续向实时 sink 分发事件。** 运行可能显示为成功，但重新连接、重启或 `thread/read` 只能看到持久化成功的前缀，无法重建实时 UI 曾经看到的完整工具生命周期。

#### 生产链与动态证据

- `crates/deepagent-runtime/src/kernel.rs::PersistentEventSink::append()` 调用现有 `RunStore::append_event()`。写入失败时只 `tracing::warn!`，没有把失败返回给 `emit()`，也没有进入运行终态或重试状态。
- `PersistentEventSink::emit()` 在 `append()` 返回后无条件调用 `self.delegate.emit(event)`。因此同一个 `RuntimeEvent` 的实时投影和 durable `run_events` 投影没有共同的提交边界。
- 新增 ignored 探针 `kernel::tests::audit_event_persistence_failure_does_not_split_live_and_replay`：使用真实 `AgentKernel::start()`、`RunStore` 和 SQLite trigger，仅拒绝 `tool_completed` 插入；自定义 Agent 通过 Kernel 注入的规范 sink 发送 `ToolStarted`、`ToolCompleted` 后正常完成。实时标签包含 `tool_completed`，数据库事件为 `run_accepted、setup.started、capability.snapshot、setup.completed、run_started、session_registered、turn_started、tool_started、run_completed、run_terminal`，缺少 `tool_completed`。执行：`cargo test -p deepagent-runtime audit_event_persistence_failure_does_not_split_live_and_replay --offline -- --ignored --nocapture`，**0 passed / 1 failed（预期暴露缺陷）**。

#### 成熟实现对照

- **DeepSeek Harness**：`借鉴/deepseek-harness/packages/session/session-checkpoint-policy/README.md` 将 persistence flush 作为模型请求、顶层工具副作用和下一步推导前的共享 durability barrier；checkpoint rejection 采用 fail-closed，未成功落盘时不进入后续模型或工具动作。`借鉴/deepseek-harness/packages/session/session-persistence/README.md` 规定后台写失败要保留顺序、暂停自动路径，并由下一次显式 flush 重试且公开拒绝，而不是继续把失败事件当作已提交。
- **Codex**：`借鉴/codex/codex-rs/core/src/session/mod.rs::send_event_raw_with_persistence` 保持实时 reduction、规范 append、投递的固定顺序；`借鉴/codex/codex-rs/core/src/tasks/mod.rs` 在终态事件之后显式 flush rollout，再让调用方看到终态。其具体存储实现不同，但共同约束是 durable boundary 不能被实时 delivery 静默越过。
- **Claude Code**：`借鉴/claudecode/restored-src/src/bridge/replBridge.ts` 对持久化序列和高水位进行单独管理，持续失败时限制 drain 并保留诊断；它的实时 SSE/桥接状态不能替代 transcript 持久化。

#### 后果、修复方向与验证限制

- 不能通过“继续发送实时事件”来掩盖 durable 写失败；否则工具调用可能在 UI 中显示已完成，重连后缺少结果，模型下一轮可能重复或误判该调用。当前探针只证明事件序列分叉，不证明一定发生重复外部副作用。
- 应在现有 `PersistentEventSink` 边界收敛提交语义：普通事件写入失败时，实时 delegate 不得把该事件宣称为已提交；应保留失败诊断并让运行进入可观测的 persistence-error/终止或受控重试状态。对已经发出的实时事件，协议需要明确其为 provisional，不能让 `thread/read` 假设它已 durable。终态事件必须在所有必需前置事件可回放后再发布。
- 修复必须复用现有 `RunStore/run_events`、终态和协议投影，不新增 live event store 或补写第二套日志。回归需覆盖普通事件、delta flush、终态写失败、断线重读以及单个事件失败后的 sequence 连续性。
- 当前仅使用 SQLite trigger、内存数据库、离线 Agent 和真实 Kernel；未运行真实 Tauri、真实 provider、跨进程数据库锁争用或崩溃恢复。生产代码本轮未改，探针保持 ignored。

### H-143：终态持久化失败前已经发布实时完成事件

**级别：高（P1，完成状态与 durable run 状态矛盾）；触发条件：`run_terminal` 的原子终态写入失败。** 用户界面可能先收到 `run_completed`，但数据库中的 `runs.finished_at` 仍为空，重连后该运行不会被视为已完成。

#### 生产链与动态证据

- `AgentKernel::start_prepared()` 在 `RuntimeEngine` 返回成功后先由 `PersistentEventSink` 发出 `RuntimeEvent::RunCompleted`；该事件会进入实时 delegate，并尝试写入普通 `run_events` 行。
- 随后 `persist_terminal()` 调用 `RunStore::finish_with_event()`，在同一个 SQLite 事务中更新 `runs` 并写入 `run_terminal`。任一写入失败都会回滚终态，Kernel 返回错误；此前已经发出的实时 `run_completed` 没有撤回或 provisional 标记。
- 新增 ignored 探针 `kernel::tests::audit_terminal_persistence_failure_does_not_claim_live_completion`：真实 `AgentKernel`、`RunStore` 和 SQLite trigger 拦截 `run_terminal`，离线 Agent 正常返回完成。实际实时标签为 `run_started、session_registered、turn_started、run_completed`；数据库没有 `run_terminal`，`finished_at` 仍为空。执行 `cargo test -p deepagent-runtime audit_terminal_persistence_failure_does_not_claim_live_completion --offline -- --ignored --nocapture`，**0 passed / 1 failed（预期暴露缺陷）**。

#### 成熟实现对照

- **DeepSeek Harness**：`借鉴/deepseek-harness/packages/session/session-checkpoint-policy/README.md` 要求 durable checkpoint 成为下一步和工具副作用的前置屏障，拒绝时 fail-closed；`session-persistence/README.md` 将 flush 明确为 durability barrier。终态投影不能在其 durable boundary 失败后继续被当作已提交。
- **Codex**：`借鉴/codex/codex-rs/core/src/tasks/mod.rs` 在发出终态 turn 事件后显式 `flush_rollout()`，并保留 flush 错误诊断；`借鉴/codex/codex-rs/core/src/session/mod.rs::send_event_raw_with_persistence` 固定规范持久化与实时投递顺序。具体存储不同，但都要求终态可回放边界明确，不能让实时完成替代 durable completion。

#### 后果、修复方向与验证限制

- 前端可能停止 spinner 并显示成功，随后 `thread/read`、重启恢复或运行列表又显示未完成；如果调用方按完成状态允许下一轮输入，还可能并发占用同一会话或重复收尾。
- 应把 `run_completed` 与 `run_terminal` 收敛为一个可观测提交边界：终态事务成功后再发布完成事件，或明确先发布 provisional 事件并在持久化失败时发送失败/回滚事件。不能依靠 UI 自己猜测，也不能新增第二个终态存储。
- 修复需覆盖普通成功、终态写失败、终态重复调用、实时重连、进程重启和前端完成通知；当前探针只使用 SQLite trigger、内存数据库和离线 Agent，未覆盖真实 Tauri、真实 provider、崩溃时序或跨进程锁争用。生产代码本轮未改，探针保持 ignored。

### H-144：DeepSeek Chat 合法终态 `aborted` / `insufficient_system_resource` 被当作未知错误

**级别：中（P2，Chat 路由的终态语义和恢复策略错误）；触发条件：调用方使用 `WireMode::ChatCompletions`，DeepSeek 返回官方允许的 `finish_reason=aborted` 或 `finish_reason=insufficient_system_resource`。** 这与 H-132 的 `content_filter` 过滤终态不同，覆盖的是用户/服务端中断和推理资源不足两个独立原因。

#### 生产链与动态证据

- 官方 [DeepSeek Chat Completions API](https://api-docs.deepseek.com/api/create-chat-completion) 将 `finish_reason` 枚举列为 `stop`、`length`、`content_filter`、`tool_calls`、`insufficient_system_resource` 和 `aborted`；后二者不是未知值。
- 生产 `ModelClient::stream_response()` 在 Chat 模式进入 `ChatCompletionAccumulator::push_sse_data_observed()`；`map_finish_reason()` 只接受 `stop`、`tool_calls` 和 `length`，其他值统一返回 `CoreError::provider`。因此 provider 的合法终态在 `Response` 组装前被转换为错误，调用方拿不到可区分的终态。
- 新增两个 opt-in 正式客户端探针：`client::tests::audit_chat_aborted_finish_is_not_rejected_as_unknown` 与 `client::tests::audit_chat_insufficient_system_resource_finish_is_not_rejected_as_unknown`。二者都通过正式 `ModelClient → ChatCompletionAccumulator` 和 `MockTransport` 注入官方形状的终态及 usage，执行结果均为 `0 passed / 1 failed（预期）`：前者返回 `Provider { code: Some("aborted"), message: "model stopped with unsupported finish reason: aborted" }`，后者返回同形状的 `insufficient_system_resource` 错误。没有修改生产解析器，也没有调用公网。

#### 成熟实现对照

- **DeepSeek Harness** 的 `借鉴/deepseek-harness/packages/llm/llm-pi-ai/src/stream.ts::mapStopReason()` 把 `aborted` 映射成独立的 `{ kind: 'aborted', failure: ... }`，而不是与普通 provider error 混用；`token-meter` 仍在观察 usage 后接受 aborted finish，说明终态和用量是两个维度。
- **Codex** 的 Responses SSE 解析器对 `content_filter` 等供应商终态保留独立错误类别，再由上层决定是否重试；共同原则是保留 provider 原因，不把合法终态压成一个“未知错误”。这里仅采用原因保真和分层恢复原则，不把 Codex 的 Responses 枚举直接套到 DeepSeek Chat。

#### 后果、修复方向与验证限制

- `aborted` 会被误报为不支持的 provider failure，调用方无法区分用户/服务端中断与网络失败；`insufficient_system_resource` 也无法进入有界退避或换模型的专门策略。两者在当前边界都不能稳定形成 `Response`，provider 返回的 usage 也不会随该响应交给上层 `Response` 记账路径。
- 应在 `FinishReason` 中增加与协议语义对应的终态（或等价的 typed provider outcome），在 Chat adapter 中保留这两个值，并允许无文本终态继续完成 usage 解析；运行层再明确 `aborted` 的取消归属、资源不足的重试预算和最终事件映射。不能把它们误判成 `Length`，也不能无条件自动重试用户主动取消。
- 本轮只添加缺陷探针和审计记录，生产行为未修改。当前证据是正式 adapter 的离线 wire 复现，未调用真实 DeepSeek、未验证具体供应商在不同模型/非流式模式下的频率，也未覆盖默认 Responses 路由；因此不把该项扩大为桌面默认路径必现。

### H-145：Thinking 模式下不支持的 Chat `tool_choice` 组合未在请求前拒绝

**级别：中（P2，协议组合校验缺失）；触发条件：调用方使用 `WireMode::ChatCompletions`，启用 DeepSeek Thinking，并设置 `tool_choice=required` 或指定函数。** DeepSeek 官方明确说明这两种选择在 Thinking 模式不支持，应在客户端边界拒绝，而不是把必失败请求交给 provider。

#### 生产链与动态证据

- 官方 [DeepSeek Chat Completions API](https://api-docs.deepseek.com/api/create-chat-completion) 的 `tool_choice` 说明写明：Thinking 模式不支持 `required` 和 named tool choice，API 会返回 HTTP 400。
- 当前 `ChatCompletionRequest::validate()` 只检查 reasoning effort、关闭 Thinking 与 effort 的矛盾，以及工具 kind；没有检查 Thinking enabled 与 `tool_choice` 的组合。
- 新增两个 opt-in 正式客户端探针：`client::tests::audit_chat_thinking_rejects_required_tool_choice_before_transport` 与 `client::tests::audit_chat_thinking_rejects_named_tool_choice_before_transport`。二者都构造真实 `ModelClient`、合法 function schema、`reasoning_effort=high` 与官方不支持的选择，配合调用计数 transport。执行结果均为 `0 passed / 1 failed（预期）`：断言要求请求前错误失败，但当前 `result.is_ok()`，说明 transport 已被调用（调用计数为 1）。没有调用公网。

#### 成熟实现对照

- **DeepSeek Harness** 的 provider adapter 将统一模型选项先转换为供应商可用的 request shape，并把不适用的 Thinking/tool 组合留在适配器边界处理；其 `llm-deepseek` 测试覆盖 thinking/tool round-trip 和 replay，而不是把所有 OpenAI 选项直接透传。
- **Codex** 的模型 capability/catalog 在请求装配前提供支持矩阵，运行层依据有效能力决定 tool choice 与 reasoning 组合；共同原则是把供应商不支持的组合在本地变成可解释配置错误，避免浪费一次请求。这里采用边界校验原则，不把 Codex 的能力枚举直接复制到 DeepSeek。

#### 后果、修复方向与验证限制

- 触发时会把确定性 HTTP 400 交给 provider，浪费一次请求并将配置错误混入普通模型失败；如果上层把该错误当 transient provider error 重试，还会重复发送同一个必失败请求。当前 `ModelClient` 的本地校验和 transport 调用计数已经证明请求越过了应有的拒绝边界。
- 应在 Chat request validation 中根据 `thinking.type=enabled`（或有效 reasoning effort）解析 `tool_choice`：字符串 `required`、对象指定函数都返回结构化 invalid-input，并保证 transport 调用数为零；`auto`、`none` 保留。Responses 路由按其自己的官方支持矩阵独立校验，不能用 Chat 规则污染默认 Responses。
- 本轮只添加缺陷探针和审计记录，生产行为未修改。当前证据为正式 adapter 的离线调用计数，未向真实 DeepSeek 发送 400 请求，也未覆盖非流式 Chat、其他模型版本或默认 Responses 路径。

### H-146：DeepSeek Chat 用户隔离字段被序列化为 Responses 的 `user`

**级别：中（P2/P3，条件性供应商身份与隔离语义缺失）；触发条件：调用方使用 `WireMode::ChatCompletions` 并为模型客户端配置 `ResponseDefaults.user`。** DeepSeek Chat Completions 要求请求字段为 `user_id`；Responses API 使用 `user`。当前两个协议共用一个默认字段，但 Chat serializer 没有做协议级映射。

#### 生产链与动态证据

- 官方 [DeepSeek Chat Completions API](https://api-docs.deepseek.com/api/create-chat-completion) 将用户区分字段定义为 `user_id`，用途包括用户区分、内容安全审查、KV cache 隔离和调度隔离；官方 [Responses API](https://api-docs.deepseek.com/api/create-response) 使用的是 `user`，两者不能按同名字段直接透传。
- 当前生产路径是 `ModelConfig::deepseek_chat()` → `ResponseDefaults.user` → `ModelClient::apply_chat_defaults()` → `response_to_chat_completion()` → `ChatCompletionRequest::Serialize`。其中 `ChatCompletionRequest` 的序列化实现固定执行 `map.serialize_entry("user", value)`，因此配置值会以 Chat 不识别的 `user` 发出；Responses 路径的 `ResponseRequest` 序列化仍使用 `user`，不属于本项缺陷。
- 新增 opt-in 正式客户端探针 `client::tests::audit_chat_user_identity_uses_provider_user_id_field`。它通过真实 `ModelClient::stream_response()`、Chat 路由、默认值应用和受控 SSE transport 捕获最终请求体。执行 `cargo test -p deepagent-models audit_chat_user_identity_uses_provider_user_id_field --offline -- --ignored --nocapture` 得到 **0 passed / 1 failed（预期）**；捕获 wire 为 `{"messages":[...],"model":"deepseek-flash","stream":true,"stream_options":{"include_usage":true},"user":"audit-user-123"}`，`user_id` 缺失。没有调用公网。

#### 成熟实现对照

- **DeepSeek Harness** 的 provider adapter 先把统一运行时选项映射到当前协议的 request shape，再由各协议 serializer 输出；其 token-meter、session 和 provider 选项不把不同 provider 的同名/近义字段当成可直接透传的公共 wire。这里应沿用“适配器按协议显式映射”的边界。
- **Codex** 的后端身份类型也把运行身份作为明确的 `user_id` 语义保存，而不是依赖 UI 文本或一个跨协议的模糊字段。具体字段名和 DeepSeek 协议不同，但共同原则是身份字段必须在 provider 边界保真、可审计、可按宿主隔离。

#### 后果、修复方向与验证限制

- 当前 Chat 请求未必会因此立刻返回错误，但 provider 可能忽略 `user`，导致配置的用户身份没有参与 Chat 路由的用户区分、内容安全审查身份、KV cache 隔离、调度隔离或多用户追踪。不能据此宣称多用户隔离已经生效。
- 修复应在 Chat adapter 中显式把内部用户身份序列化为 `user_id`，并让 Responses adapter 保持 `user`；不要简单把公共字段改名后同时影响两种协议。需要补充两条协议的 wire 回归、空值省略、Unicode/长度边界和默认值与请求级值优先级测试；若 provider 对身份字段有字符集或长度限制，应在适配器边界给出结构化 invalid-input。
- 本轮只添加缺陷探针和审计记录，生产行为未修改。证据是正式本地 adapter 的最终请求体捕获，未调用真实 DeepSeek、未验证 provider 对错误字段的具体响应，也未覆盖 Chat 非流式请求、不同账号或多宿主调度；严重级别保持条件性。

### H-147：DeepSeek 工具 schema 名称未按 provider 约束校验，非法、超长和重复名称直接越过请求边界

**级别：高（P1，条件性工具目录/请求阻断）；触发条件：可见工具来自 MCP、插件或其它动态注册源，名称为空、含不允许字符、超过 128 个字符，或在同一次请求中重复。** 普通内置工具名称通常满足约束；本项不是说每个默认桌面会话都会失败，而是一个进入可见快照的异常工具 schema 就可能让整次模型请求被 provider 拒绝。

#### 生产路径与动态证据

- DeepSeek 官方 [Responses API](https://api-docs.deepseek.com/api/create-response) 规定 function 工具名必须非空、最长 128 个字符、匹配 `^[a-zA-Z0-9_-]+$`，并在同一请求中唯一；官方 [Chat Completions API](https://api-docs.deepseek.com/api/create-chat-completion) 对 function 名称使用同样约束。MCP 的服务器名和本地工具名来自外部配置/服务，不能默认视为已经满足该协议约束。
- 当前生产路径是 `ToolDescriptor.name`/`RemoteTool.namespaced_name` → `ToolSchema::function` → `ResponseRequest.tools` → `ModelClient::stream_response()`；Responses serializer 和 Chat serializer 都直接写入 `tool.function.name`。`McpRegistry::namespaced_name()` 只做 `mcp__{server}__{tool}` 字符串拼接，`ToolRegistry::register()` 只拒绝重复注册名，没有做 provider 字符集、长度或请求内唯一性校验。动态快照合并也不能替代最后的 provider boundary validation。
- 新增 opt-in 正式客户端探针 `client::tests::audit_deepseek_tool_schema_names_are_validated_before_transport`。它使用真实 `ModelClient`、两种 `WireMode`、正式 request serializer 和计数型 transport，分别构造空名、含点号的 `read.file`、129 个字符名称以及重复 `same_tool`。没有手工调用私有 serializer，也没有公网请求。
- 执行 `cargo test -p deepagent-models audit_deepseek_tool_schema_names_are_validated_before_transport --offline -- --ignored --nocapture`：**0 passed / 1 failed（预期）**。四种 Responses 输入均返回受控的 `Ok(Response)` 且 `transport_calls=1`；四种 Chat 输入也均 `transport_calls=1`（本 fixture 没有 `[DONE]`，因此在 accumulator 处返回 `stream ended without [DONE]`，但请求已经到达 transport）。这证明非法 schema 没有在请求前被拒绝；不把受控响应误写成 provider 已接受非法名称。

#### 成熟实现对照

- **Codex** 在 `借鉴/codex/codex-rs/codex-mcp/src/mcp/mod.rs::sanitize_responses_api_tool_name` 明确把 MCP 名称中不符合 `^[a-zA-Z0-9_-]+$` 的字符替换为 `_`，空名变为 `_`；`codex-mcp/src/tools.rs` 随后以 128 字节上限截断并给冲突名称追加稳定 hash，同时保留 raw server/tool identity 用于实际路由。它把模型可见名和原始路由名分开，不把外部 MCP 名称原样当作 provider function 名称。
- **DeepSeek Harness** 的 LLM serializer 通过已装配的工具 registry 生成 provider 工具列表，并把动态 tool addition/removal 作为独立的模型上下文块；这支持“工具身份和模型可见投影分离”的边界。本轮没有把其 Messages serializer 的名称行为夸大成 DeepSeek Responses 的 128 字符验证，也没有运行参考项目测试。

#### 后果、修复方向与验证限制

- 一个非法或重复工具名进入可见集合时，Responses/Chat 请求可能收到确定性的 400；由于工具 schema 在普通文本请求也会随请求发送，用户即使不打算调用该工具也可能无法开始对话。若上层把该错误当作可重试 provider 错误，可能重复发送相同必失败请求；如果只在 `ToolRegistry` 注册阶段拒绝，还可能让整个 MCP 快照在请求组装时才失败，无法定位具体来源。
- 应在现有 MCP/插件工具快照发布前建立 provider-neutral raw identity 与按协议生成的 model-visible name，但仍在 `ModelClient` 发送前做最终 schema validation。Responses 和 Chat 可共享字符/长度/唯一性规则，却不能共享未经协议确认的字段映射；名称消歧必须同步 schema、调用路由、审批/权限、历史 call name 和诊断来源。对无法安全消歧的项，应让该能力进入管理失败状态并从新请求快照排除，不能静默选择第一个或吞掉注册错误。
- 回归应覆盖正常名称、空/非法/超长、大小写、跨 server 冲突、同一请求重复、快照刷新/重启、延迟工具发现、Responses/Chat 两路以及真实 MCP `tools/list` 到最终 wire 的完整链路；同时断言原始 MCP call 仍使用 raw name。不能只测 `ToolSchema` JSON 能序列化。
- 本轮只增加审计探针和文档，生产名称生成与校验未修改。证据来自正式模型客户端和受控 transport，未调用真实 DeepSeek、未启动带非法名称的完整桌面 MCP 配置，也未验证 provider 对每种错误的具体 HTTP body；Chat 组的 accumulator fixture 只用于证明请求越过 transport 边界。

### H-148：Responses 工具调用的空 `call_id` 未在请求边界拒绝

**级别：高（P1，条件性工具调用阻断）；触发条件：持久化历史、旧迁移或 provider/插件投影产生 `function_call` 与对应 output 的空 `call_id`。** 正常 DeepSeek 响应通常会返回非空 ID；本项证明的是公共 Responses 构造和请求发送边界没有守住官方必需字段，不能把它扩大为每次普通对话都会出现空 ID。

#### 生产路径与动态证据

- DeepSeek 官方 [Responses API](https://api-docs.deepseek.com/api/create-response) 规定 `function_call` / `function_call_output` 的 `call_id` 必须非空且唯一；工具调用和结果必须通过该 ID 配对。
- 当前生产路径是 `ResponseItem` → `ResponseRequest::from_response_items()` → `repair_tool_call_pairs()` → `ModelClient::stream_response()` → `ResponseRequest` 序列化。`repair_tool_call_pairs()` 只按字符串是否出现来收集和配对，不检查 `call_id.is_empty()`；Responses 路由没有对应的 `validate()`，因此空 ID 会原样进入 JSON wire。
- 新增 opt-in 正式客户端探针 `client::tests::audit_responses_call_ids_are_validated_before_transport`。它通过真实 `ResponseRequest` 构造、生产 `ModelClient` 和计数型 `HttpTransport`，提交空 `function_call.call_id` 及同 ID 的 `function_call_output`。执行 `cargo test -p deepagent-models audit_responses_call_ids_are_validated_before_transport --offline -- --ignored --nocapture` 得到 **0 passed / 1 failed（预期）**：结果为 `Ok(Response)`，`transport_calls=1`。这证明空调用身份越过了 provider boundary；没有调用公网。

#### 成熟实现对照

- **Codex** 的 Responses 请求不把 call/result 当作只按字符串查找的松散列表；其 `core/tests/common/responses.rs::validate_request_body_invariants` 按原生调用类型和身份检查配对不变量，测试夹具要求工具调用身份可关联且结果类型对称。这里借鉴的是“请求发送前验证完整原生不变量”，不是复制 Codex 的协议代码。
- **DeepSeek Harness** 的 tool-call/session 记录把 `ToolCallId` 作为运行时关联身份贯穿工具执行、结果和事件投影；其 headless JSON stream 在投影工具结果时使用已建立的 call identity，而不是遇到空值再猜测。这里采用身份必须在执行和持久化边界被拒绝或显式生成的原则。

#### 后果、修复方向与验证限制

- 真实 provider 可能直接返回 400，导致当前请求失败；更危险的是，空 ID 会让多个未关联调用共用同一个键，`repair_tool_call_pairs()` 可能把结果合并、补错或把历史恢复成模型无法解释的工具配对。工具执行本身可能已经发生，下一轮才在 provider 边界暴露问题，形成“动作完成但会话无法继续”的状态。
- 应在现有 Responses provider seam 增加统一输入不变量校验：`call_id` 必须非空、每次请求唯一、每个 call 必须有同 native kind 的 output；校验应在 repair 后、序列化前执行，错误保留 item index、call kind 和脱敏后的身份诊断。对旧历史不能静默丢调用或改成 `unknown_tool`；应沿用现有运行终态/恢复契约记录可回放的配对失败，或在可证明安全时生成一次稳定的本地 ID，并保留原始事件。
- 回归需覆盖空/空白/重复 ID、跨 function/custom 混用、孤立 output、恢复/压缩/取消补结果、Responses/Chat 分路及真实 MCP/provider 输出到最终 wire 的链路；不能只测试 JSON 可序列化。
- 本轮只增加审计探针和文档，生产校验未修改。证据来自正式客户端和受控 transport；未向真实 DeepSeek 发送非法请求，也未证明当前默认桌面历史已经产生空 ID。重复 native kind 配对另由 H-80 记录，H-148 关注的是官方 `call_id` 非空/唯一不变量未在请求边界执行。

### H-149：Responses 重复 `call_id` 被静默去重并跨过 provider boundary

**级别：高（P1，条件性工具历史丢失）；触发条件：同一 Responses 输入中出现两个不同的 function call，却复用同一个非空 `call_id`。** 这是持久化历史合并、恢复/压缩边界或错误的工具投影可能产生的身份冲突；本项不把它扩大为正常 provider 输出的必然行为。

#### 生产路径与动态证据

- DeepSeek 官方 [Responses API](https://api-docs.deepseek.com/api/create-response) 规定 `function_call` / `function_call_output` 的 `call_id` 必须非空且唯一，并要求每个 function call 有对应结果。唯一性是调用历史可关联的协议不变量，不是可由客户端静默修复的展示字段。
- 当前生产路径是 `ResponseItem` → `ResponseRequest::from_response_items()` → `repair_tool_call_pairs()` → `ModelClient::stream_response()` → `ResponseRequest` 序列化。`repair_tool_call_pairs()` 使用全局 `HashSet` 和 `call_id → output` 映射：第二个相同 ID 的调用被跳过，第二个结果也被丢弃，首个结果被保留；函数随后继续发送修复后的请求，没有返回校验错误。
- 新增 opt-in 正式客户端探针 `client::tests::audit_responses_duplicate_call_ids_are_rejected_before_transport`。它通过真实 `ResponseRequest`、生产 `ModelClient` 和受控计数型 `HttpTransport` 提交两个不同的 function call（`read_file` / `write_file`）及两个不同结果，但共用 `duplicate-call`。执行 `cargo test -p deepagent-models audit_responses_duplicate_call_ids_are_rejected_before_transport --offline -- --ignored --nocapture` 得到 **0 passed / 1 failed（预期）**：结果为 `Ok(Response)`、`transport_calls=1`；捕获的 wire `input` 只有第一组 `read_file` + `first result`，第二次调用和 `second result` 已不可见。没有调用公网。

#### 成熟实现对照

- **Codex** 的 Responses 测试夹具在发送后按 `call_id` 建立调用/结果不变量，并把缺失、空身份和未配对结果作为请求失败条件；它不会把不同原生调用先合并成一条再继续发送。这里借鉴的是请求边界保持调用身份唯一且可回放的原则，不复制其协议类型。
- **DeepSeek Harness** 的工具执行记录以每次 `ToolCallId` 贯穿 dispatch、结果和事件流；工具结果根据已建立的调用身份回填，而不是把同 ID 的兄弟调用折叠为一个 map entry。该边界保证运行时事件、模型历史和工具执行对象仍能互相定位。

#### 后果、修复方向与验证限制

- 两个不同工具若复用 ID，第二个调用可能已经在上游事件或运行时执行过，但下一次模型请求只看到第一组调用和结果；模型会丢失一次动作及其结果，后续可能重复执行、误判状态或因历史与实际工具副作用不一致而继续错误操作。由于当前修复发生在请求构造阶段，日志只会看到正常 `Ok` 请求，难以从 provider 错误定位历史已被裁剪。
- 应在现有 Responses provider seam 增加不可变输入校验：同一请求内 function/custom call 的 `call_id` 必须非空、全局唯一，并按原生 kind 与对应 output 对称；冲突应在 repair 前或 repair 后、序列化前返回带 item index/kind 的诊断，不能静默保留第一条。旧历史的冲突应进入现有恢复/终态错误契约，不能删除原始事件或自动把第二个调用改成未知工具；若未来生成稳定本地 ID，必须保留原始身份映射并写入可回放事件。
- 回归需覆盖同 kind 重复、function/custom 跨 kind 重复、重复 output、恢复/压缩/取消补结果、Responses/Chat 分路以及真实 MCP 工具快照到 wire 的完整路径；同时验证冲突失败不触发 transport，正常唯一 ID 保持原顺序和调用结果配对。
- 本轮只增加审计探针和报告，生产校验未修改。证据来自正式模型客户端和受控 transport，未向真实 DeepSeek 发送非法请求，也未验证用户历史中实际出现重复 ID 的频率；H-80 记录的是 function/custom output 原生 kind 混配，H-148 记录的是空 `call_id`，本项专门记录非空但重复 ID 导致的静默历史丢失。

### H-150：Responses `input` 与 `instructions` 同时为空仍发送请求

**级别：中高（P1，条件性无效请求）；触发条件：公共请求构造器收到空消息列表且未设置 instructions。** 正常 ChatService turn 会提供用户输入或系统上下文；本项证明的是 Responses adapter 的公共边界允许构造并发送协议上无效的请求，不能把它扩大为每次正常对话都会出现空 turn。

#### 生产路径与动态证据

- DeepSeek 官方 [Responses API](https://api-docs.deepseek.com/api/create-response) 规定 `input` 与 `instructions` 至少提供一个；`input` 可以是字符串或 item 数组，但不能与 instructions 一起缺失。
- 当前生产路径是 `ResponseRequest::new(model, messages)` → `response_items_from_messages()` → `ModelClient::stream_response()` → `ResponseRequest` 序列化。传入空 `messages` 时，构造器得到 `instructions=None`、`input=[]`；Responses 流式入口只设置默认值和 `stream=true`，没有调用等价的请求校验。
- 新增 opt-in 正式客户端探针 `client::tests::audit_responses_require_input_or_instructions_before_transport`。它使用真实 `ResponseRequest::new("deepseek-flash", Vec::new())`、生产 `ModelClient` 和受控计数型 transport。执行 `cargo test -p deepagent-models audit_responses_require_input_or_instructions_before_transport --offline -- --ignored --nocapture` 得到 **0 passed / 1 failed（预期）**：结果为 `Ok(Response)`、`transport_calls=1`，捕获 wire 为 `{"model":"deepseek-flash","input":[],"stream":true}`。没有调用公网。

#### 成熟实现对照

- **DeepSeek Harness** 的 AgentLoop 在 `借鉴/deepseek-harness/packages/core/agent-loop/src/agent.ts` 中先经过 `preStep` 决定是否存在有效 messages；初始 decision 为空时结束 turn，不启动 model step。它把空输入当作 loop admission 状态，而不是发送一个空 provider request。
- **Codex** 的 app-server turn 入口对空输入返回 `input must not be empty`；空用户回合在协议入口被拒绝，避免将无效 payload 推迟到模型服务才暴露。

#### 后果、修复方向与验证限制

- 当前 adapter 会为无有效输入的 turn 消耗一次 provider 请求配额，并可能得到 provider 400；若上层把它当作可重试错误，会重复发送同一个必失败请求。更隐蔽的是，离线/代理 transport 可能返回成功，使运行记录出现没有用户输入的 assistant turn，破坏会话语义和 usage 统计。
- 应在现有 Responses provider seam 增加请求级校验：`instructions` 非空或 `input` 至少含一个可发送 item；空白 instructions、只有被 repair 丢弃的工具 item、以及取消/恢复产生的空 turn 都应在进入 transport 前得到统一的可回放错误或 no-op 终态。校验不能依赖 provider 返回，也不能用隐式占位文本制造模型输入。
- 回归需覆盖空消息、空白 instructions、仅工具输出被修复为空、正常 system-only、正常 user-only、Chat/Responses 分路以及取消/重试/恢复的空 turn；正确失败必须断言 transport 调用数为零。
- 本轮只增加审计探针和报告，生产校验未修改。证据来自正式模型客户端和受控 transport，未向真实 DeepSeek 发送空请求，也未在完整桌面接纳链路中制造空用户回合；Harness/Codex 的空 turn 处理用于边界对照，不代表两者内部协议完全相同。

### H-151：Responses malformed function call 被填充默认身份并继续提交

**级别：高（P1，条件性错误工具派发/历史污染）；触发条件：provider 流中的 `function_call` 缺失必需的 `call_id` 或 `name`。** 正常 DeepSeek 响应会提供这些字段；本项证明 adapter 对 malformed output 的处理会制造一个看似合法的工具调用，不能把它扩大为正常 provider 输出的常态。

#### 生产路径与动态证据

- DeepSeek 官方 [Responses API](https://api-docs.deepseek.com/api/create-response) 的输出 schema 要求 `function_call` 携带 `call_id`、`name` 和 `arguments`；`call_id` 是后续 `function_call_output` 的关联身份，`name` 是实际工具名。缺失字段不是可用的默认工具调用。
- 当前生产路径是 provider SSE → `ResponseAccumulator::record_output_item()` / `finish()` → `ModelClient::stream_response()` → `Response` → `ModelAgent` 的工具决策和原生 Surface。`record_output_item()` 对缺失 `call_id`/`id` 使用空字符串，对缺失 `name` 使用 `"tool"`；`finish()` 随后又从同一 builder 生成 `call_stream_0` 的 fallback，导致同一 malformed item 可出现两次，且至少有一项身份非法。
- 新增 opt-in 正式客户端探针 `client::tests::audit_responses_reject_malformed_function_call_output`。它通过生产 `ModelClient`、真实 `ResponseAccumulator` 和受控 SSE fixture 发送不含 `call_id`、`id`、`name` 的 `function_call` item，再发送正常 `response.completed`。执行 `cargo test -p deepagent-models audit_responses_reject_malformed_function_call_output --offline -- --ignored --nocapture` 得到 **0 passed / 1 failed（预期）**：结果为 `Ok(Response)`，`output_items` 同时包含空 `call_id`/`tool` 和 `call_stream_0`/`tool` 两个 function call。没有调用公网。

#### 成熟实现对照

- **DeepSeek Harness** 的 `借鉴/deepseek-harness/packages/llm/llm-deepseek/src/translate.ts` 在解析工具块时要求 `id` 和 `name` 都是非空字符串，缺失即返回 `malformed('empty tool identity')`；它不会用默认工具名或本地生成的新身份掩盖 provider 输出错误。其共享 assembler 也以带身份的 tool-call block 作为后续 dispatch 的前置条件。
- **Codex** 的 Responses/session 类型保留调用身份为可选只用于明确的非模型控制消息；正式模型 function call 与 output 的配对校验按身份进行，不把缺失模型字段转换成任意可执行工具。这里借鉴的是“解析失败先终止，不能生成可执行默认调用”的边界，不复制其内部 item 类型。

#### 后果、修复方向与验证限制

- 当前 malformed 响应会被上层当作正常 `CallTool` 候选。空 ID 可能无法匹配任何工具结果，`tool` 默认名可能被权限/路由层当成未知工具；同时 fallback 重复项会污染响应投影、事件和下一轮 repair，造成工具调用数、usage/终态以及模型历史不一致。探针没有执行真实工具，因此只确认错误 item 已进入决策输入，不声称已经产生文件或命令副作用。
- 应在 `ResponseAccumulator` 的 provider response boundary 统一校验 item：function/custom tool call 必须有非空身份、非空名称和可解析的 arguments 字段；缺失或重复的 item 应返回结构化 malformed provider error，禁止写入 `Response`、ModelAgent Surface 或工具 pipeline。流式 delta 与 `output_item.done` 两条入口必须共享同一校验，不再各自生成 fallback ID；仅对明确的 max-token 截断沿用 H-18 的安全剔除规则。
- 回归需覆盖缺失/空白 `call_id`、缺失/空白 `name`、只有 item `id`、重复 item.done、增量与 done 混合、合法无参调用、custom tool、取消/截断终态以及错误后的 session replay；断言 malformed response 不产生任何工具决策或持久化 output item。
- 本轮只增加审计探针和报告，生产 parser 未修改。证据来自正式 adapter 和受控 SSE；未向真实 DeepSeek 发送 malformed response，也未执行实际工具或完整桌面 UI。H-18 专门记录 max-token 截断调用被提交，本项专门记录 provider 字段缺失时 parser 主动制造默认身份。

### H-152：官方 `tool_search_call` 在 Responses adapter 中被静默丢弃

**级别：中（P2，条件性协议能力缺口）；触发条件：provider 的 Responses 流返回原生 `tool_search_call` output item。** DeepSeek 官方输出 schema 和流事件表都声明了该类型，但当前公开请求工具 schema 没有说明普通 function/custom/web-search 配置如何启用它，因此本项不能扩大为默认请求必现。

#### 生产路径与动态证据

- DeepSeek 官方 [Responses API](https://api-docs.deepseek.com/api/create-response) 将 `tool_search_call` 列为输出 item 类型，并说明其 `call_id` 用于把工具输出传回 API；官方 [Responses 指南](https://api-docs.deepseek.com/guides/responses_api) 同样明确 `response.output_item.added/done` 可以携带 `tool_search_call`。这是一项正式协议类型，不是第三方网关私有扩展。
- 当前 `deepagent-core::ResponseItem` 只有 message/reasoning/function/custom/web-search 变体，没有 `ToolSearchCall` 或对应 output；`ResponseAccumulator::add_item()` 只把 function/custom call 建成工具调用，`record_output_item()` 也只保留 message、reasoning、function/custom 和 web-search。未知 item 直接返回，不产生错误、原生 Surface item 或工具决策；`finish()` 在没有文本和已识别调用时补一个空 assistant message。
- 新增 opt-in 正式客户端探针 `client::tests::audit_responses_tool_search_call_is_not_silently_dropped`。它通过生产 `ModelClient → ResponseAccumulator` 注入官方形状的 `response.output_item.done`：`call_id=search-1`、`execution=client`、查询参数为 `calendar create`，随后发送正常 `response.completed`。执行 `cargo test -p deepagent-models audit_responses_tool_search_call_is_not_silently_dropped --offline -- --ignored --nocapture` 得到 **0 passed / 1 failed（预期）**：正式调用返回 `Ok(Response)`，唯一 output item 是内容为空的 assistant message；调用身份、执行位置和查询参数全部消失。fixture 没有访问公网或执行任何工具。
- 本系统现有 deferred tools 和名为 `tool_search` 的本地函数仍走普通 `function_call`/ToolRegistry 链路。它们不能解析原生 `tool_search_call`，也不能作为本项已经支持的证据。

#### 成熟实现对照

- **Codex** 在 `protocol/src/models.rs` 为 `ToolSearchCall` 和 `ToolSearchOutput` 建立独立原生类型，保留 `call_id`、`execution`、结构化 `arguments`、状态和返回工具目录；对应 roundtrip 测试覆盖客户端执行与服务端执行的可空 ID 差异。
- Codex 的 `codex-api/src/sse/responses.rs::parses_tool_search_call_items` 直接验证 `response.output_item.done` 的解析，`core/src/client_common.rs` 和远程压缩逻辑继续保留 call/output，集成测试还断言搜索调用在下一请求中具有配对 output。这里借鉴的是把原生协议项纳入统一 Response Surface、回放和压缩，而不是新建第二套工具注册表。

#### 后果、修复方向与验证限制

- 一旦 provider 返回该 item，本系统会把“模型请求发现/装载工具”误判为一次正常空回答：不会执行搜索、不会回传 `tool_search_output`，也不会留下可重放的调用身份。运行可能以成功结束，用户只看到空输出；若上层继续请求，模型所期待的工具目录和本地历史已分叉。
- 应扩展现有 `ResponseItem`/原生 Surface，加入 tool-search call/output 的 typed variant，并同步 provider parser、请求 serializer、历史回放、Context Meter、snip/compaction、工具配对和事件投影。客户端执行应复用既有 deferred-tool 索引与 ToolRegistry 生成 output；服务端执行应保留 provider 返回状态。若当前运行环境没有执行能力，adapter 必须返回结构化 unsupported-provider-item 错误，禁止静默成功。
- 回归需覆盖 added/done 去重、client/server execution、结构化 arguments、可空 server call ID、call/output 配对、压缩与重启回放、工具目录变化、取消和 malformed item；真实 provider 验收还需先找到官方可达的启用方式。本轮只确认“收到该官方 item 后会丢失”，没有证明当前 DeepSeek 普通请求能主动触发它，也没有发送真实请求或测量回答质量。
- 本轮只增加审计探针和报告，未修改生产 parser、Surface 或工具执行行为；H-57/H-137 记录的是已有目录在后续请求中的保留和动态刷新，本项记录的是 provider 原生工具检索协议项根本未进入系统。

### H-153：Responses 规范思考档位贯通缺陷（规范值链路已修复；别名见 H-155）

**级别：中（P2，配置契约缺口，已修复）；修复前触发条件：通过桌面 Responses 高级设置关闭思考，或在开发者 JSON 中使用 DeepSeek 官方列出的 `none`。** 普通 `ThinkingDepth::Simple` 曾直接构造 `none`，不经过保存校验；H-54 另行记录了已保存 Responses 默认值被桌面深度覆盖的问题。

#### 修复前的生产路径与动态证据

- DeepSeek 官方 [Responses API](https://api-docs.deepseek.com/api/create-response) 定义 `reasoning.effort` 的规范值为 `none / low / high / max`，其中 `none` 关闭思考；同页明确 `minimal` 会映射为 `low`，`medium / xhigh` 会映射为 `high`。官方 [Thinking Mode](https://api-docs.deepseek.com/guides/thinking_mode/) 也把实际模型档位列为 low/high/max，并说明兼容名称的映射。因此 `medium` 不是独立的模型强度，`none` 也不是非法输入。
- 桌面 `ConfigSettings.tsx` 的 Responses 高级下拉只提供 `low / medium / high / max`：缺少关闭项，却把服务端会映射到 high 的 `medium` 展示成独立档位。普通 DTO 的 `ResponsesApiSettings::validate()` 同样只接受这四个字符串。开发者 JSON 虽直接表示 provider 的 `reasoning.effort`，`validate_developer_responses_fields()` 仍使用同一错误白名单。
- 新增默认忽略探针 `kernel_v2_e2e::audit_responses_reasoning_effort_settings_match_deepseek_contract`。每个值都新建正式 `SettingsService`、内存数据库和现有模型发现 mock，调用桌面共用的 `set_responses_settings()` 或开发者入口 `set_responses_developer_json()`，再从正式持久化设置读取 effective effort；失败项不会阻止其余值执行。没有绕过 service 直接调用 validator。
- 执行 `cargo test -p deepagent-app-core --test kernel_v2_e2e audit_responses_reasoning_effort_settings_match_deepseek_contract --offline -- --ignored --nocapture`，得到 **0 passed / 1 failed（预期）**：

| 设置入口 | effort | 保存结果 | 保存后的 effective effort |
|---|---|---|---|
| 普通 Responses 设置 | none | 拒绝：只允许 low/medium/high/max | 无 |
| 普通 Responses 设置 | low / high / max | 成功 | 原值 |
| 开发者 JSON | none | 拒绝：只允许 low/medium/high/max | 无 |
| 开发者 JSON | minimal | 拒绝 | 无 |
| 开发者 JSON | medium | 成功 | medium |
| 开发者 JSON | xhigh | 拒绝 | 无 |
| 开发者 JSON | low / high / max | 成功 | 原值 |

#### 成熟实现对照

- **DeepSeek Harness** 的 provider 配置在 `packages/llm/llm-deepseek/src/config.ts` 只暴露语义档位 `off / low / high / max`；`model-info.ts` 用同一组档位描述模型能力，`serialize.ts` 在最终 adapter 边界将 off 映射为协议的禁用表达，并拒绝没有声明的语义档位。它不会把 provider 为兼容客户端接受的 `medium` 再包装成一个实际不存在的模型档位。
- 其通用 `llm-pi-ai` adapter 允许模型声明 `reasoningEfforts` 映射，例如 `off: "none"`，并有实际请求断言证明 UI/Agent 的 off 最终变为 provider wire 的 none。这里借鉴的是“用户语义、模型能力与 wire 拼写分层”，不是复制 Messages API 的 `thinking/output_config` 字段。

#### 修复前后果与修复复测

- 修复前，合法的 `none` 无法从设置入口保存，桌面高级控件也不能表达关闭思考；`medium` 会伪装成独立档位。本节前述发现是修复前基线，H-54 仍是独立的运行时优先级缺陷。
- 当前以模型层 `ReasoningEffort::{None, Low, High, Max}` 表达四个规范档位。Harness 运行选择固定使用 `WireMode::Responses`，并将这些值序列化为官方 endpoint schema 的 `reasoning.effort`；Chat adapter 是独立消费者，不应被描述成 Harness 的请求路由。
- 旧 `none` 保存/透传缺陷及 UI 档位已修复。当前仍拒绝 `minimal / medium / xhigh` 的普通设置和开发者 JSON 输入；DeepSeek endpoint 文档明确接受并映射这些别名，这一现存缺口单列为 H-155。旧持久化 `medium` 映射为 `high` 的迁移不能代替新输入兼容。
- 新增端到端回归 `responses_reasoning_effort_settings_reach_provider_wire`：正式 SettingsService 保存四个规范值，经 ChatService、Kernel、ModelAgent 和 ModelClient 序列化到离线 HttpTransport；每档检查实际 Responses body。测试特意把全局档位设为 `low`，确认 Responses 专项设置进入请求。
- 设置迁移、四个规范值 wire 和 Chat adapter 投影均有独立测试；测试不访问公网，只能证明本地行为，不能证明真实供应商兼容或思考强度实际生效。DeepSeek Thinking Mode 指南另将 `output_config.effort` 列为 Responses 强度控制，而 Create Response 参数页只列 `reasoning.effort` 且定义其同时控制模式和强度；当前 Harness 遵从 endpoint 参数页，未发送 `output_config`。文档差异及其限制记录在 H-155，不把 Chat/Anthropic 参数移植到 Harness。

### H-154：Responses `user` 只检查非空，非法标识符进入真实 provider 请求（已修复）

**级别：中（P2，条件性请求拒绝与隔离标识错误，已修复）；修复前触发条件：开发者 JSON 或其他 SettingsService 调用方配置含空格、斜杠、非 ASCII 字符或超过 512 字符的 Responses `user`。** 未配置该可选字段的普通用户不受影响。本项是默认 Responses 路由的值约束；H-146 记录的是 Chat adapter 使用了错误字段名，两者不是同一个缺陷。

#### 修复前的生产路径与动态证据

- DeepSeek 官方 [Responses API](https://api-docs.deepseek.com/api/create-response) 要求 `user` 只能使用 `[a-zA-Z0-9\-_]`，最大长度 512，并说明该值用于安全审查、KV cache 隔离和调度隔离，且不得包含用户隐私信息。这个约束属于 provider 请求契约，不只是 UI 格式建议。
- `ResponsesApiSettings::validate()` 对普通字段只拒绝空字符串；`validate_developer_responses_fields()` 对原始 JSON 同样只检查“非空字符串”。值随后经 `effective_user()` 进入 `ResponseDefaults.user`，`ModelClient::apply_defaults()` 直接写入 `ResponseRequest.user`；发送边界没有第二次字符集或长度校验。
- 新增默认忽略探针 `kernel_v2_e2e::audit_responses_user_identifier_is_validated_before_transport`。每组使用正式 SettingsService/内存数据库、开发者 JSON 保存入口、ChatService→Kernel→ModelAgent→ModelClient 和真实 Responses JSON serializer；最底层 transport 只记录 body 并返回受控离线文本。探针不直接构造 `ResponseRequest`，且逐组使用新服务以避免设置残留。
- 执行 `cargo test -p deepagent-app-core --test kernel_v2_e2e audit_responses_user_identifier_is_validated_before_transport --offline -- --ignored --nocapture`，得到 **0 passed / 1 failed（预期）**：

| 用例 | 字节数 | 设置保存 | 模型请求数 | wire 保留原值 |
|---|---:|---|---:|---|
| `account_1-2` | 11 | 成功 | 1 | 是 |
| 512 个 ASCII 字符 | 512 | 成功 | 1 | 是 |
| 空字符串 | 0 | 拒绝 | 0 | 不适用 |
| 含空格 | 9 | **错误地成功** | 1 | 是 |
| 含斜杠 | 9 | **错误地成功** | 1 | 是 |
| 含中文 | 8 | **错误地成功** | 1 | 是 |
| 513 个 ASCII 字符 | 513 | **错误地成功** | 1 | 是 |

#### 成熟实现对照

- **DeepSeek Harness** 的 Messages adapter 不向用户暴露同名 Responses body 字段。它在 `packages/identity/anonymous-user-id` 生成并持久化 UUID，读取时用完整 UUID 正则验证，随后由 `llm-deepseek/src/host.ts` 以 branded `AnonymousUserId` 提供给 adapter；session identity 另行传递。协议字段不同，不能复制其 header，但其边界证明身份由专门组件生成/验证，而不是让任意设置文本直接进入模型请求。
- **Grok Build** 的 computer-hub 身份由凭据在服务端解析出 `user_id`，transport 再绑定 `(user_id, session_id)`；客户端不以任意请求文本替代已认证 principal。这里同样只借鉴“身份和普通提示配置分离”的原则，不把其认证协议套到 DeepSeek `user`。

#### 修复前的后果与建议（历史记录）

- 当前本地运行会把不符合官方 schema 的请求交给 provider；按文档契约它可能导致整轮请求被 400 拒绝，使模型和工具均不开始。探针没有访问公网，因此不声称具体错误码或所有网关都会同样拒绝。若兼容网关宽松接收，无效或不稳定的值也可能破坏预期的 cache/调度隔离；本轮没有测量 provider 内部隔离结果。
- 应在既有 Responses 设置/adapter 边界提供一个共享 validator：ASCII、1～512 字符、完整匹配 `[A-Za-z0-9_-]+`。开发者 JSON和普通 DTO 共用它，ModelClient 发送前仍做最后防线，避免其他调用方绕过设置服务。若产品需要自动身份，复用现有设置/身份存储生成不含隐私的稳定 opaque ID；否则保持字段缺省，不新增第二套账户系统。请求预览和错误日志不得泄露完整 ID。
- 回归需覆盖 1/512/513 边界、各允许字符、空白/Unicode/控制字符、普通设置与开发者 JSON、旧非法设置读取、Chat H-146 转换及实际 transport 调用数。旧非法值应在读取或下次保存时给出明确迁移错误，不能静默截断后改变 cache identity。
- 上述结论属于修复前记录；实际实现与复测结果见下一节。

#### 修复实现与复测结果

- 设置 DTO 与 developer JSON 共用 `deepagent_models::chat::validate_responses_user_identifier()`，要求 1～512 个 ASCII 字母、数字、连字符或下划线；拒绝错误不包含用户输入。已有持久化值不会被截断或静默改写。
- `ResponseRequest::validate()` 在 ModelClient 应用 defaults 后、序列化和 transport 前验证最终值，因此手工构造的非法请求和无效默认值也会在发送前失败。
- 普通设置和 developer JSON 的边界测试覆盖有效字符、1/512/513 长度、空值、空格、斜杠、Unicode 和控制字符；SettingsService → ChatService → Kernel → ModelAgent → ModelClient 集成测试确认非法值 transport_calls=0，合法边界各发送一次。
- `cargo test -p deepagent-models rejects_invalid_responses_user_before_transport --offline -- --nocapture`、`cargo test -p deepagent-app-core --lib responses_user_identifier_validates_both_settings_surfaces --offline -- --nocapture` 与 `cargo test -p deepagent-app-core --test kernel_v2_e2e responses_user_identifier_is_validated_before_transport --offline -- --nocapture` 均通过。仅使用离线 transport；H-146 的 Chat 字段映射仍是独立缺陷。

### H-155：Responses 设置拒绝官方接受的 reasoning effort 兼容别名

**级别：低（P3，开发者配置兼容性）；触发条件：用户在普通 Responses 设置或开发者 JSON 使用 DeepSeek 明确接受的 `minimal`、`medium` 或 `xhigh`。** 规范档位 `none / low / high / max` 可用；当前问题是本地拒绝官方兼容输入，不是已证明规范 wire 值错误。

#### 生产路径与动态证据

- 2026-10-24 复核 DeepSeek [Create Response 参数文档](https://api-docs.deepseek.com/zh-cn/api/create-response/)：`reasoning.effort` 的规范值为 `none / low / high / max`，并明确接受兼容别名 `minimal → low`、`medium / xhigh → high`。endpoint schema 是本系统 Responses wire 的直接依据；这些别名不是 UI 自定义档位。
- 当前 `ReasoningEffort::parse()` 只接受四个规范值；普通 `ResponsesApiSettings::validate()` 和开发者 JSON 的 `validate_developer_responses_fields()` 共用该收窄规则。开发者 JSON 会被本地解析、抽取为类型化设置，再由 `ResponseRequest` 重新序列化，不是原样透传。
- 运行 `cargo test -p deepagent-app-core --test kernel_v2_e2e responses_reasoning_effort_settings_accept_only_canonical_values --offline -- --nocapture`，结果 **1 passed**。这项通过意味着现有实现确实拒绝 `minimal / medium / xhigh`，测试当前把这种拒绝当作预期；它不证明 DeepSeek 会拒绝这些别名。
- 运行 `cargo test -p deepagent-app-core --test kernel_v2_e2e responses_reasoning_effort_settings_reach_provider_wire --offline -- --nocapture`，结果 **1 passed**。正式 SettingsService → ChatService → Kernel → ModelAgent → ModelClient 将四个规范值放在 `reasoning.effort` 中发送到离线 transport。未访问真实 provider。

#### 成熟实现对照与后果

- DeepSeek Harness 用自己的受限 `ReasoningEffortId` 契约表示 `off / low / high / max`，并在 provider adapter 边界序列化；它接的是 Messages 协议，README 中的 `output_config.effort` / `thinking.type` 不能直接充当本系统 Responses wire 的依据。这里可借鉴的是“内部规范档位与 provider serializer 分层”，不能用内部 enum 的白名单覆盖官方 Responses 输入兼容性。
- 影响限于用户粘贴官方文档/SDK 中合法 alias 到本系统设置时保存失败；不影响当前 UI 发出的四个规范值，也没有证据表明已存在请求会因该问题被 DeepSeek 拒绝。建议在输入边界将官方 alias 归一化成规范 enum，wire 继续只发送规范值，并把测试改成验证 alias 接受且规范化。

#### 官方文档差异与验证限制

- DeepSeek [Thinking Mode 指南](https://api-docs.deepseek.com/zh-cn/guides/thinking_mode/) 的 Responses 栏还出现 `output_config.effort`，但 2026-10-24 复核的 [Create Response 参数页](https://api-docs.deepseek.com/zh-cn/api/create-response/) 将该 endpoint 的请求 schema 定义为 `reasoning.effort`，并明确列出上述兼容别名；Responses 兼容性表也只确认 `reasoning.effort`。本系统 Harness 生产路由明确为 Responses，当前请求遵循 endpoint 参数页，只发 `reasoning.effort`。官方两页仍有字段表述差异；没有 provider 实测可消解差异，也没有依据把指南中的 `output_config` 当作 endpoint 请求字段。
- 本次仅核对文档和本地序列化，不证明 `low/high/max` 在当前具体模型上的实际推理档位或 UI 展示；模型能力应以实时 `/models` 元数据另行核验。

### H-156：同名扩展工具可能误入 `apply_patch` custom 序列化分支

**级别：低（P3，条件性扩展兼容风险）；触发条件：WASM 或程序化扩展把工具精确命名为 `apply_patch`，使它进入当前 Responses 工具集合。** 普通 AppCore 内置工具及 MCP 工具不会触发此分支；确认的是 adapter 对精确名称的处理与 DeepSeek 文档 shape 不同，尚未证明服务端拒绝或忽略额外字段。

#### 生产路径与证据

- `ModelClient::apply_defaults()` 将名为 `apply_patch` 的工具设为 `custom`；`ResponseRequest::serialize()` 随后发送 `type=custom`、`name=apply_patch`，并额外发送 `description` 和 `format={type:text}`，见 `crates/deepagent-models/src/client.rs` 与 `crates/deepagent-models/src/chat.rs`。
- DeepSeek [Responses 工具兼容性文档](https://api-docs.deepseek.com/zh-cn/guides/responses_api/) 对 custom 工具只列 `{"type":"custom","name":"apply_patch"}`；Create Response schema 将 `description` 说明为 function 工具描述，没有为 custom 声明 `format`。因此当前 serializer 超出了已明确记录的 DeepSeek custom shape。
- `register_builtins()` 及 `build_base_tool_registry()` 没有注册一等 `apply_patch` 工具；MCP 统一使用 `mcp__<server>__<tool>` 命名空间，不会与该保留名冲突。工作区 WASM 工具则可由文件名定义名称，因此 `apply_patch.wasm` 是一个可能触发该分支的可选扩展配置。
- 新增完整 AppCore 请求探针，正式 SettingsService/ChatService/Kernel/ModelAgent/ModelClient 经离线 transport 捕获默认 Responses JSON，并确认正常工具集合非空但不含 `apply_patch`。执行 `cargo test -p deepagent-app-core --test kernel_v2_e2e responses_custom_tool::default_harness_request_does_not_advertise_apply_patch --offline -- --exact --nocapture`：**1 passed**。最初按“默认请求应含 apply_patch”的错误假设写探针；真实请求捕获否定了该假设，随后检查注册边界并修正探针。这是审计夹具假设错误，不是产品请求失败。
- 因默认完整 AppCore 请求没有该工具，本轮没有声称在默认路由中动态复现 custom 字段差异，也没有向 DeepSeek 发出真实请求。仍需在可控的同名扩展配置中捕获该 request body；当前影响局限于这种可选扩展，服务端对额外字段的处理未知。

#### 成熟实现对照、影响与下一步

- DeepSeek Harness 参考实现走 Messages 协议，不适合作为 Responses `custom` 字段的证据；OpenAI/Codex 的 custom tool 扩展也不能覆盖 DeepSeek 自己公布的兼容子集。此处以 DeepSeek Responses 文档为准。
- 触发范围只限于运行时确实广告了精确名称 `apply_patch` 的可选扩展；默认 AppCore 请求已由完整链路离线捕获并确认不广告该工具。因此不能将本项描述成普通会话每轮会失败。
- DeepSeek 文档的“未知参数静默忽略”说明位于顶层参数兼容性表，没有承诺嵌套 `tools[]` 对象里的额外键也会被忽略。当前证据只能确认本地 wire 超出文档列出的 custom 最小 shape，不能推断 provider 必然拒绝或接受；条件触发时可能被忽略，也可能导致该请求报错。
- 本轮验证 `cargo test -p deepagent-app-core --test kernel_v2_e2e responses_custom_tool::default_harness_request_does_not_advertise_apply_patch --offline -- --exact --nocapture`：**1 passed**。这验证默认生产组装下没有该风险触发项；没有可用的默认生产扩展路径来形成 provider 请求，也未发公网请求。若未来某扩展需要精确注册 `apply_patch`，应先在那条正式装配路径捕获 body，再比较文档最小 shape 与当前 shape，并在有明确测试预算和授权时做受控 provider 验证。
- 在能触发此分支的扩展配置中，若 provider 对额外嵌套字段严格拒绝，整轮请求会失败；若被忽略则无直接运行影响。故目前影响定为低（P3）、条件性、服务端行为未知，不扩展到默认 Harness 请求。

### H-157：模型目录丢弃 model-level reasoning effort，Responses 请求不校验所选模型支持档位

**级别：低（P3，条件性模型扩展兼容风险）；触发条件：DeepSeek `/models` 返回某模型的 `effort.supported_levels` 不含当前全局或 turn override 所选的 effort。** 2026-10-24 官方模型列表示例中的 Flash 与 Pro 都列出 `low/high/max`、默认 `high`，因此当前公开示例没有触发证据；本项确认的是公开模型目录契约未被保留，以及合法目录形状遇到能力收窄时会构造不匹配 wire。未证明任何当前线上模型已经拒绝请求。

#### 生产路径与离线复现

- DeepSeek [获取模型列表文档](https://api-docs.deepseek.com/zh-cn/api/list-models/)明确返回每个模型的 `effort.supported_levels` 和 `default_level`，说明 supported levels 是 `reasoning_effort` 可接受的档位；用于关闭思考的 `none` 不在该列表中。当前 `ModelInfo` 仅定义 `id/object/owned_by/context_window/max_output_tokens`，Serde 忽略其余字段；`ModelCatalog` 因而不能持久化这些模型级档位或默认值。
- `ModelConfig::from_catalog()` 将目录转换为 `ModelCapability`，但 capability 只有 `supports_thinking: bool`，没有可选档位或模型默认档位。`model_runtime.rs::resolve_reasoning_effort()` 从全局/Responses 设置或 turn override 得到单一 enum，不看所选模型元数据；`ResponseDefaults` 随后把该值传给正式 Responses serializer。
- 新增 `crates/deepagent-models/tests/reasoning_effort_audit.rs`。测试以 DeepSeek `/models` 公布字段形状提供单一离线模型 `supported_levels=[low]`、`default_level=low`，经过正式 `ModelDiscovery.discover()`、`ModelCatalog` serde roundtrip、`ModelConfig::from_catalog()` 和 `ModelClient::stream_response()`。运行 `cargo test -p deepagent-models --test reasoning_effort_audit --offline -- --ignored --nocapture`，得到 **0 passed / 1 failed（预期）**，并记录 `metadata_retained=false`、wire `reasoning.effort=high`。这证明客户端把目录能力丢掉并越过请求边界；fixture 是公开 schema 的能力收窄场景，不是 DeepSeek 当前模型实测或真实 provider 拒绝证据。
- 2026-10-24 复跑单项探针 `audit_model_effort_metadata_constrains_responses_request`：**0 passed / 1 failed（预期）**，输出仍为 `metadata_retained=false advertised=[low] default=low wire_effort=Some("high")`。同时核对 DeepSeek [获取模型列表](https://api-docs.deepseek.com/zh-cn/api/list-models/)：`effort.supported_levels` 是模型可接受的 reasoning 档位，`default_level` 必须属于该集合；因此本地缺口仍是模型能力目录未贯通，而不是官方字段定义不确定。
- 同期重跑规范档位基线：`cargo test -p deepagent-app-core --lib reasoning_effort --offline -- --nocapture` **3 passed**；`cargo test -p deepagent-app-core --test kernel_v2_e2e responses_reasoning_effort_settings --offline -- --nocapture` **2 passed**；`cargo test -p deepagent-models --lib reasoning_effort_serializes_canonical_wire_values --offline -- --nocapture` **1 passed**。这说明 wire 的 `reasoning.effort` 形状和规范枚举当前通过，不能证明模型级支持范围被遵守。

#### 成熟实现对照、影响与下一步

- DeepSeek Harness `packages/llm/llm-deepseek/src/model-info.ts::modelInfo()` 为解析模型生成 reasoning metadata；`serialize.ts` 会在 fetch 前拒绝不在该 adapter 声明集合中的 effort。这可对照“模型信息与请求选项共用边界、unsupported 值不发网络”的做法。但该实现使用自身静态 `REASONING_EFFORTS`，本轮没有证据表明它动态消费 DeepSeek `/models` 的新 `supported_levels` 字段，因此不把它描述成此目录能力缺口的完整现成修复。
- 对当前文档列出的模型，`high` 是已公布默认档位，所以没有观察到用户当前请求会因此不兼容。若后续目录出现仅支持 `low` 的模型，或某模型能力收窄而用户仍选择 `high/max`，本系统会继续发送该档位；provider 可能返回参数错误，也可能忽略/降级。真实服务端响应、是否计费、自动 fallback 行为均未验证。
- 收敛方向（本地设计，未实施）：在现有 `ModelInfo`/`ModelCapability` 中保留可选 model-level effort levels/default，并保持旧 catalog 可反序列化；按本次实际主模型与 fallback model 解析有效 effort。显式 unsupported 值应在网络前给出可定位错误；未显式配置时使用该模型公开 default 或有明确来源的兼容默认。DeepSeek endpoint 文档把 `none` 单独定义为关闭，不应错误要求它必须出现在 `supported_levels` 中。测试应覆盖发现、存储恢复、全局/turn override、主备模型切换及真实 wire；只有受控 provider 验证后才可声明服务器错误行为。

### H-158：Responses named `tool_choice` 接受官方不支持的 `custom` / `web_search` 类型并进入 wire

**级别：中（P2，条件性请求阻断或工具强制失效）；触发条件：用户通过 Responses 高级设置或 developer JSON 将 `tool_choice` 配置成 `{"type":"custom","name":"apply_patch"}` 或 `{"type":"web_search"}`。** 普通默认值为省略/`auto`，不会触发；本项不等同于 H-93 的 Chat 形状转换，也不否定 `tools[]` 可以声明 custom `apply_patch` 或 Pro 的 `web_search`。

#### 官方契约、生产路径与动态复现

- DeepSeek [Responses API 参数页](https://api-docs.deepseek.com/zh-cn/api/create-response/) 将 `tool_choice` 定义为 `none / auto / required` 或指定函数的扁平对象 `{"type":"function","name":"my_function"}`，named choice 的 `type` 枚举只有 `function`。同一文档允许 `tools[]` 出现 `custom` 和 `web_search`，但“工具可声明”不等于“可用相同类型强制选择”。
- 当前 `ResponsesApiSettings::validate()` 与 `from_developer_json()` 最终调用 `validate_developer_responses_fields()`；该函数却把对象 `type` 白名单写成 `function | custom | web_search`。随后 `model_runtime.rs` 把原始 `serde_json::Value` 放入 `ResponseDefaults.tool_choice`，`ModelClient::apply_defaults()` 直接复制，`ResponseRequest::validate()` 只检查 `user`，serializer 原样发送。这是设置与 provider boundary 共用错误协议模型，不是单一 UI 展示问题。
- 新增 opt-in 探针 `responses_tool_choice::unsupported_responses_named_tool_choices_are_rejected_before_transport`，分别创建与 choice 匹配的 custom `apply_patch` 和 Pro `web_search` 工具，经过正式 settings parser、`ModelClient` 默认转换、`ResponseRequest` serializer 与计数 transport。执行 `cargo test -p deepagent-app-core --test kernel_v2_e2e responses_tool_choice::unsupported_responses_named_tool_choices_are_rejected_before_transport --offline -- --ignored --exact --nocapture`：**0 passed / 1 failed（预期）**。两组均 `accepted=true` 且各发送一次；捕获 wire 分别含 `tool_choice={type:custom,name:apply_patch}` 与 `{type:web_search}`，并同时含相应工具声明。没有调用公网，因此只证明非法 shape 越过本地边界，不声称 DeepSeek 实际返回 400。

#### 成熟实现对照、后果与修复方向

- **Codex** 的 `ResponsesApiRequest.tool_choice` 是字符串，正式请求组装固定发送 `auto`，公共请求类型无法塞入任意 JSON 对象；这不是 DeepSeek named function 的完整实现，但体现了“只表达当前 adapter 明确支持的 choice 形状”。**DeepSeek Harness** 的 provider-neutral `GenerateOptions` 也不暴露任意 `tool_choice` JSON，Messages serializer 只序列化其适配器支持的字段。两者共同避免把上游未经类型化的 provider 对象直接透传；本系统仍需按 DeepSeek Responses 自己的兼容子集实现，不能照抄其他协议字段。
- 触发后 provider 可能拒绝整次请求，也可能忽略不支持的 choice 并退化成自动选择；前者会让启用该配置后的所有普通对话在采样前失败，后者会让用户以为已强制 `apply_patch`/搜索但模型可以返回文字或调用别的工具。具体 HTTP 状态、计费和忽略行为未经真实 API 验证，不写成既成事实。
- 根因修复应把 `serde_json::Value` 收敛为协议级 typed choice（`Auto / None / Required / Function { name }`），输入边界只接受官方形状，并在 defaults 应用后的 `ResponseRequest::validate()` 再做最终校验；指定函数还应验证名称合法且存在于本次最终工具快照。Chat adapter 若继续支持 named function，必须在自己的 serializer 显式转换为 Chat 的嵌套 `function` 形状，不能与 H-93 共用原始 JSON。回归覆盖 request/default/developer JSON、缺失工具、custom/web_search 拒绝、Responses/Chat 分路及 transport 调用数为零。

### H-159：MCP 非法 `inputSchema` 在注册与发布阶段未校验，作为 function `parameters` 进入每次模型请求

**级别：高（P1，条件性工具目录/请求阻断）；触发条件：MCP、WASM 或程序化工具提供缺失、非对象或不可编译的参数 schema，并进入本轮可见工具快照。** 普通内置工具通常使用对象 schema，不代表默认会话必现；但可见集合中的一个异常工具会随普通文本请求一起发送，无需模型先选择调用它。

#### 官方契约、生产路径与动态复现

- DeepSeek [Create Response 参数页](https://api-docs.deepseek.com/zh-cn/api/create-response/)把 function `parameters` 定义为“以 JSON Schema **对象**描述”的输入参数；省略该字段表示无参数函数，不等同于显式发送 `null`、字符串或数组。官方 [Tool Calls 指南](https://api-docs.deepseek.com/zh-cn/guides/tool_calls/)的工具示例也使用 `type=object`，并明确说明 strict 模式会校验传入 schema、非法或不支持的类型会返回错误。这里用 Responses endpoint 的 `object` 类型作为本系统 wire 直接依据；不把 Chat strict beta 的全部 schema 子集无条件套到普通 Responses。
- 本系统当前协商 `MCP_PROTOCOL_VERSION=2024-11-05`。该版 MCP [Tools 规范](https://modelcontextprotocol.io/specification/2024-11-05/server/tools)把 `inputSchema` 列为工具定义的一部分，并定义为参数 JSON Schema，示例根是对象。`McpToolDef.input_schema` 却使用 `#[serde(default)] serde_json::Value`：服务省略字段时会成功解析成 `null`，非对象 JSON 也会成功解析。
- `McpRegistry::register_with_instructions()` 原样保存 schema，`McpToolAdapter::descriptor()` 原样复制到 `ToolDescriptor.parameters`，`ToolRegistry::register()` 只检查重名；`build_visible_tool_schemas()` / `ToolSchema::function()` 再原样发布到 provider。现有 `ToolRegistry::validate_invocation()` 虽会编译 schema，但它只在模型已经返回工具调用、即将执行时运行，不能保护更早的模型请求边界。`ResponseRequest::validate()` 也没有检查最终工具 schema。
- 新增 opt-in 完整链探针 `tool_schema_parameters::invalid_mcp_tool_schemas_are_rejected_before_provider_transport`，分别让正式 `McpClient` 的 `tools/list` 返回：省略 `inputSchema`、字符串 schema、数组 schema；再经过 `McpRegistry`、`McpToolAdapter`、`ToolRegistry`、`ToolSchema` 和正式 `ModelClient` serializer，由计数 transport 捕获请求。执行 `cargo test -p deepagent-app-core --test kernel_v2_e2e tool_schema_parameters::invalid_mcp_tool_schemas_are_rejected_before_provider_transport --offline -- --ignored --exact --nocapture`：**0 passed / 1 failed（预期）**。三组均为 `mcp_registered=true`、`tool_registered=true`、`transport_calls=1`，wire 的 `parameters` 依次是 `null`、`"not-a-schema"`、`[]`。这证明非法形状已越过本地 provider 边界；没有调用公网 DeepSeek，因此不声称服务端实际返回了某个 HTTP 状态。

#### 成熟实现对照、后果与修复方向

- **DeepSeek Harness** 的类型化 `defineTool()` 通过 `parameterSchemaSpecToJsonSchema()` 把参数编译为 object-rooted JSON Schema，并调用 `assertSupportedJsonSchema()`；其 JSON Schema 模块还提供 `assertObjectJsonSchema()`，在需要对象根时明确拒绝其它根类型。它的模型投影 `schemaOf()` 会拒绝非无损 JSON。需要如实限定：raw `tools.register()` 当前明确校验 output schema，却没有对任意 `definition.parameters` 做同等完整的 schema 语义校验，因此这是“类型化工具路径已有更强边界”，不是参考实现所有动态工具入口都已完全解决。
- **Codex** 的 MCP 链使用 `rmcp::model::Tool` 保存原始 schema，工具准备阶段主要处理名称、插件来源和文件参数；本轮没有找到其对所有 MCP `input_schema` 执行完整 JSON Schema 编译的证据，不能宣称 Codex 已完整解决本项。可借鉴的是它把模型可见名称/来源投影集中在工具准备边界；本系统仍需按自身 DeepSeek/MCP 契约增加验证，而不是以参考项目缺口为放行理由。
- 如果 DeepSeek 严格拒绝非法工具声明，同一工具进入可见快照后，即使用户只发普通文本，整轮也可能在采样前失败；deferred MCP 工具尚未发布时，故障可能在发现后的下一轮才出现。若 provider 宽松接收，模型稍后真正调用该工具时才会由 `validate_invocation()` 报“invalid schema”，造成目录可见但永远不可执行。具体服务端拒绝、忽略、计费和错误码均未经真实 API 验证。
- 根因修复应落在统一能力快照发布边界：MCP 协议反序列化不能把缺失必需 schema 静默等价成 `null`；`McpRegistry` 或统一 `ToolRegistry::register/replace` 应在能力可见前要求对象根并编译 schema，错误需携带 server/tool 来源且只隔离该能力，不能让一个坏工具永久阻断所有普通对话；最终 provider request validation 再做 defense-in-depth。不要仅在调用时保留现有 `validate_invocation()`，也不要为 MCP、WASM、插件分别复制校验。回归应覆盖初始/动态刷新、deferred discovery、replace、Chat/Responses 两路、坏工具隔离、合法复杂 schema 和 transport 调用数为零。

### H-160：Canvas gateway 把非法显式模型引用解释为自动路由，修通专业节点后可静默改用另一供应商

**级别：高（P1，潜在模型/供应商身份与数据目的地错误）；触发条件：调用 Canvas text bridge 时传入非空裸模型名，并至少配置一个可服务 text 场景的候选模型。** 前端四类专业 AI 节点都生成同样的非法默认值，但 H-162 进一步证明它们当前在更早的 executor/prompt-contract 阶段失败；因此本项是已动态确认的 gateway 缺口和修通节点后的直接风险，不能再声称现版本新建专业节点已经通过完整工作流向错误供应商发出请求。

#### 官方契约、生产路径与动态复现

- 截至本轮查证，DeepSeek [Responses API 参数页](https://api-docs.deepseek.com/zh-cn/api/create-response/)把 `model` 的可能值限定为 `deepseek-flash` / `deepseek-v4-pro`；[Responses 指南](https://api-docs.deepseek.com/zh-cn/guides/responses_api/)给出的调用和兼容表也只列这两个值；[获取模型列表](https://api-docs.deepseek.com/zh-cn/api/list-models/)示例返回的 ID 同样只有这两项。因此 `deepseek-chat` 既不是当前 Responses 文档中的合法模型 ID，也不携带本系统画布要求的 provider 身份。这里不把该结论外推到旧版 API 或第三方兼容端点。
- 前端 `nodeRegistry.ts` 的 `llmModel`、`classifierModel`、`extractorModel`、`agentV2Model` 和 `nodeSchemas.ts::CHAT_MODEL` 都写死 `deepseek-chat`。新增 opt-in Node 探针真实加载、转译并调用生产 `createDefaultNodeData()` / `getNodeDefinition()`，观察到四种新节点的运行默认和 schema 默认全是该裸值；要求模型引用为空（明确自动选择）或满足 `providerId::modelId` 的断言失败。首次探针把节点 kind 误写为 `classifier`，只得到 fixture 错误；改成生产注册名 `question-classifier` 后才得到上述产品行为，未把第一次失败算作缺陷证据。
- 正式前端契约 `MODEL_REF_SEPARATOR="::"`，`scenarioModelOptions()` 只为已配置模型生成 `providerId::modelId`；但只要 `current` 不在目录内，它就把裸值原样插回选项，标签仍显示 `deepseek-chat`。这让新默认和历史遗留值在选择器中看起来仍可选，没有“失效/待迁移”状态。
- `WorkflowAgent::execute_llm()` 可从 LLM/分类/提取字段得到非空 `model_ref`，而正式 AppCore 工作流挂载 `CanvasModelGateway`。但 H-162 证明分类/提取当前没有 executor，LLM 又在 `assemble_system_prompt()` 先被拒绝；这里的静态字段流不能冒充完整可达性。网关本身仍存在确定缺口：`decode_model_ref()` 对任何没有 `::` 的值返回 `None`，`CanvasModelGateway::complete()` 把它与真正空引用合并，都调用 `execute(..., None)`，后者从全局 text 候选中选模型，绕过显式 `execute_on(provider, model)` 边界。
- 新增默认忽略的 Rust 探针 `canvas_model_gateway::tests::audit_invalid_explicit_model_ref_is_rejected_before_provider_transport`，只配置一个名为 `other-text-model` 的受控非 DeepSeek text 候选，通过正式 provider persistence、`CanvasModelBridge::complete()`、候选解析、OpenAI-compatible serializer 和本机 HTTP listener 发送 `model_ref="deepseek-chat"`。执行 `cargo test -p deepagent-app-core --lib audit_invalid_explicit_model_ref_is_rejected_before_provider_transport --offline -- --ignored --nocapture`：**0 passed / 1 failed（预期）**；日志为 `decoded=None`、`wire_model=Some("other-text-model")`，正式调用返回成功且 provenance 的实际 `model_id` 也是 `other-text-model`。断言要求非法非空身份在网络前失败，所以红色。没有连接真实供应商，也没有证明真实费用或数据泄露已经发生。

#### 成熟实现对照、影响与修复方向

- **DeepSeek Harness** 的 workflow child options 从入口到 subagent 使用结构化 `{ provider, model }`，而 `llm-pi-ai` 的 `PiAiAdapter::modelOf()` 对同一配置快照解析精确 route/model；模型不存在时在 provider I/O 前抛出稳定 `UNKNOWN_MODEL`，测试同时覆盖未知 provider 的 `NO_ADAPTER`。它没有把非空但无法解析的身份重新解释成默认路由，适合作为本项主要对照。
- **Codex** 也把 `model_provider_id` 与 model slug 分开保存；例如恢复子 Agent 的 provider 时从 `model_providers` 精确查找，缺失就返回 `Model provider ... not found`。本轮没有证明 Codex 对所有任意 model slug 都做目录拒绝，所以只把它作为“供应商身份不与自动选择混用”的有限对照，不宣称是完整现成修复。
- 对直接 bridge 调用或 H-162 修复后的专业节点，用户界面显示/保存的是 `deepseek-chat`，实际请求却可发送给任一 text 候选；模型能力、上下文、价格、授权凭据和数据接收方都可能改变。当前完整专业工作流的已确认结果是提前失败而不是误发；若只修 executor 而不同时修本项，才会把潜在身份错误变成可达生产行为。`WorkflowAgent` 的结果还会在原引用非空时把 `model` 写回旧引用，即使 `providerId/modelId` 是实际值。真实供应商请求、计费和数据处理未验证。
- 根因修复应建立单一类型化选择：`Auto` 只能由空值或显式枚举表达，`Explicit { provider_id, model_id }` 必须完整解码并在本轮不可变 provider snapshot 中精确存在；非空非法/未知值在 transport 前失败并指出节点及原值。新节点默认应改为明确 Auto，不能把某个易过期的供应商模型 ID 当成跨用户默认；历史裸值需要一次版本化迁移或明确失效提示，不能由 `scenarioModelOptions()` 伪装成合法选项。画布 text/image/video/audio/embedding 共用同一解析器，避免只修 `complete()`；H-40 的字段统一需同步处理 Agent V2。验收应覆盖空值自动路由、合法精确引用、裸值/畸形/未知 provider/未知 model、目录刷新、多个供应商、旧图迁移、真实 workflow outcome provenance 和 transport 调用数。

### H-161：画布 LLM 节点的温度和输出上限在 provider adapter 前被丢弃

**级别：高（P1，修通节点后的显式输出/成本边界失效）；触发条件：携带 `temperature/max_tokens` 的 completion request 进入已配置 Canvas provider。** 默认专业 LLM 节点也带 `temperature=0.7`、`maxTokens=4096`，但 H-162 证明它当前先在 prompt contract 失败；所以本项动态证明 bridge 到 wire 的参数缺口，不把现版本完整专业节点描述为已经发生超额生成。DeepSeek 当前默认开启思考模式且 temperature 在该模式下不生效，真正的直接高影响项仍是输出上限。

#### 官方契约、生产路径与动态复现

- DeepSeek [Chat Completions 参数页](https://api-docs.deepseek.com/zh-cn/api/create-chat-completion/)定义 `temperature` 为 0–2 的可选采样参数，并定义 `max_tokens` 为 1–393216 的输出上限；未传 `max_tokens` 时，非思考模式默认 8K，思考模式默认 64K，`reasoning_effort=max` 时默认 128K。该页同时说明思考模式下 temperature 不生效。因此不能把“DeepSeek 默认可能忽略 temperature”误写成“网关可以丢弃这两个不同语义的字段”。
- 前端 `nodeRegistry.ts` / `nodeSchemas.ts` 的新 LLM 节点默认写入 `llmTemperature=0.7`、`llmMaxTokens=4096`，schema 允许用户分别选择 0–2 和 1–128000。`WorkflowAgent::execute_llm()` 从正式 config 读取它们，并放入 `CanvasCompletionRequest.temperature/max_tokens`。本轮在已有 bridge 单元测试中加入 0.2/17 对照，执行 `cargo test -p deepagent-runtime --lib llm_node_with_explicit_canvas_model_uses_the_bridge --offline -- --nocapture`：**1 passed**，证明前端字段对应的后端节点层没有丢失。
- 丢失发生在下一层：`CanvasModelGateway::complete()` 将 bridge request 重建为 `CanvasModelRequest`，后者的类型根本没有 temperature/max token 字段；OpenAI-compatible Chat body 固定只组装 `model/messages/stream=false`，Anthropic body 硬编码 `max_tokens=4096`，Gemini Chat body 也没有从节点参数生成对应 generation config。三条 adapter 的静态事实说明这是共享内部 DTO 边界缺失，不是 DeepSeek 名称特判。
- 新增默认忽略探针 `canvas_model_gateway::tests::audit_canvas_completion_honors_node_temperature_and_output_limit`：精确选择本机 provider 的 `deepseek-flash`，经正式 `CanvasModelBridge::complete()`、provider resolve、OpenAI-compatible serializer 与 HTTP transport 发送 `temperature=0.2/max_tokens=17`。执行 `cargo test -p deepagent-app-core --lib audit_canvas_completion_honors_node_temperature_and_output_limit --offline -- --ignored --nocapture`：**0 passed / 1 failed（预期）**；捕获 body 为 `{"messages":[...],"model":"deepseek-flash","stream":false}`，两个字段都不存在，provider 仍返回成功。这里验证的是本地 wire，不声称真实 DeepSeek 一定生成超过 17 tokens 或产生特定费用。

#### 成熟实现对照、影响与修复方向

- **DeepSeek Harness** 在 provider-neutral generate options 中保留 `maxTokens` / `temperature`，其 DeepSeek serializer 明确使用 `options.maxTokens ?? model.maxTokens ?? connection.maxTokens` 生成 `max_tokens`，并在显式 temperature 存在时发送；model info 的 `defaultMaxTokens` 与同一 connection/model 配置一致。这是“调用配置、能力预算和 wire 在同一 adapter seam 汇合”的成熟对照。
- **Codex** 没有与本画布一一对应的逐节点温度/上限 UI，本轮不以其普通 turn 配置强行类比，也没有找到可证明其所有派生模型调用都保留这两个字段的单一证据。可借鉴的仍是本系统 H-54 已指出的统一有效请求配置原则，而不是声称 Codex 已完整解决本画布契约。
- 对 DeepSeek 默认思考路径，用户配置 4096 上限却可能按 provider 默认最多 64K/128K 生成；这会扩大最坏延迟、token 消耗和节点输出体积，并使用户依赖短 JSON/分类结果的工作流得到意外长内容。实际回答可以自然提前结束，故不把每次运行写成必然超额。对关闭思考或其他兼容 provider，temperature 的丢失还会改变用户预期的确定性/随机性。Anthropic 固定 4096 则会反向截断用户显式请求的更大上限。
- 根因修复应把生成控制加入现有 `CanvasModelRequest`（或统一的 typed generation options），由每个协议 adapter 显式映射、验证或拒绝；不能在 `complete()` 为 DeepSeek 单独拼 JSON。有效 model capability 应限制上限，明确区分“未指定”和显式值；不支持某字段的协议应给出可诊断语义，不能静默丢弃。预算/usage/截断恢复要读取同一有效值。验收覆盖 OpenAI/DeepSeek、Anthropic、Gemini 的实际 body，边界值、思考模式提示、默认/显式优先级、候选 fallback 和节点运行 provenance。

### H-162：前端标为可用的四类专业 AI 节点没有可达的正式模型执行链

**级别：阻断（P0，核心工作流能力不可用）；范围：专业画布中的 LLM、Agent V2、问题分类和参数提取节点。** 这不是 H-40 已记录的若干配置字段被删/错读：即使直接构造满足 Rust 图契约的节点配置并使用合法 `providerId::modelId`，四类节点仍在 provider transport 前失败。它也改变 H-160/H-161 的现状判断：当前完整专业工作流会先失败，不能把 gateway 级红色探针写成这些节点已经发生误路由或超额生成。

#### 生产契约与完整链复现

- 前端 `nodeRegistry.ts` 把四类节点全部声明为 `availability: "enabled"`、`executor: "model"`，配置 schema 也允许用户选择模型并填写任务；`graph.rs::output_contract()` 同时接受四种 kind 并声明输出，因此图可正常编译。这两层共同向用户和调用方表达“可执行”，不是灰置的占位能力。
- 实际 dispatcher `WorkflowAgent::execute_node_inline()` 只把 `llm` / `agent-v2` 送入 `execute_llm()`，完全没有 `question-classifier` / `parameter-extractor` 分支；仓库现有非忽略测试 `registered_kinds_without_an_executor_report_unsupported_node` 甚至把后二者列为预期 `UnsupportedNode`。这证明声明 registry 与执行能力 registry 已分裂。
- `llm` / `agent-v2` 虽进入 `execute_llm()`，却始终把原 node kind 写入 `CanvasCompletionRequest.node_kind`。`CanvasModelGateway::complete()` 随后调用 `assemble_system_prompt()`；该函数把非空 kind 当作创作节点 profile 查询，而 `canvas_node_contracts()` 只注册创作节点，没有 `llm` 或 `agent-v2`，于是分别返回 `UnsupportedNode: canvas node ... is not registered`。代码注释说专业节点应以空 kind 保留自己的 system prompt，但生产调用方没有建立这个边界。Agent V2 还使用 `agentV2Model/Task/SystemPrompt`，`execute_llm()` 读取的却是 `llmModel/model`、`agentTask/prompt` 和少量 system 字段，字段契约也没有闭合。
- 新增默认忽略的完整链探针 `canvas_model_gateway::tests::audit_enabled_professional_ai_nodes_reach_the_canvas_provider`：每组使用正式图编译、WorkflowAgent 两个 step、真实 `CanvasModelGateway`、内存 provider 持久化、合法精确模型引用及本机 HTTP listener。执行 `cargo test -p deepagent-app-core --lib audit_enabled_professional_ai_nodes_reach_the_canvas_provider --offline -- --ignored --nocapture`：**0 passed / 1 failed（预期）**。收集完四组再断言，实际为：LLM/Agent V2 均报 gateway `UnsupportedNode`，问题分类/参数提取均报 kernel `UnsupportedNode`，四组 `provider_called=false`。没有公网供应商、Tauri WebView 或真实费用。

#### 成熟实现对照、后果与修复方向

- **DeepSeek Harness** 的程序式 workflow 只在脚本实际执行 `agent()` 时创建 child；`workflow-ptc::readAgentOptions()` 在执行边界解析并拒绝未知/不支持选项，随后通过已有 adapter registry 解析可用模型。它没有与本画布一一对应的四类 UI 节点，但“声明可运行的 agent 调用必须有真实执行路径，未知能力在入口明确拒绝”可作为主要对照。
- **Codex** 没有本系统这种专业画布节点目录，本轮未找到可诚实声称为现成分类/提取节点的实现，不能强行类比。可对照的是其 thread/turn/model 调用都收敛到同一核心模型 client，而不是由 UI 单独声明一套运行时没有的能力。本项修复仍应复用现有 AgentKernel、Canvas bridge、provider registry、usage 和事件链，不能为四个节点再造第二套运行中心。
- 已确认后果是四类专业 AI 节点无法完成任何模型请求：用户配置合法模型也只会得到运行失败，分类与提取不可能产生其 schema 承诺的结构化输出，Agent V2 也不具备名称所暗示的工具循环/记忆行为。当前四组 transport 为零，所以不产生本探针所测调用的 provider token 消耗；不能把失败夸大成真实扣费或数据外传。反过来，H-160/H-161 目前是被此阻断缺口遮蔽的后续风险，修复顺序必须联动。
- 根因修复应先统一“节点声明、配置 schema、可执行 capability、prompt policy、输出 contract”这一份版本化事实：前端 enabled 只能来自后端可执行能力快照，或在实现前明确标为 unsupported；专业一次性 LLM 可复用 Canvas text bridge，但应显式传 `Professional` prompt policy，而不是用空字符串暗号。分类/提取需要在同一 bridge 上组装明确 prompt、校验结构化结果并映射既有 output contract。Agent V2 若承诺工具与记忆，应复用 ModelAgent/ToolPipeline/审批/取消链；若当前仅一轮 completion，就不应冒充 Agent V2。修复必须同时处理 H-160 的模型身份、H-161 的生成参数和后续 usage 归集，避免“修通即暴露”级联缺陷。
- 验收应从生产 `createDefaultNodeData()` 和 schema 生成四类配置，经 `runWorkflow`/Tauri DTO、图编译、Kernel、真实 bridge 到受控 provider，逐项断言：provider 调用次数、精确模型、prompt/参数、结构化输出、节点事件、终态、usage；Agent V2 另需工具调用、审批、取消和记忆边界。无 executor 的节点必须在添加到画布前可见地禁用，而不是运行时才报错。

### H-163：工作流模型调用丢失 DeepSeek usage，成本账本、缓存观测和 token budget 与实际请求分裂

**级别：高（P1，计量/预算/成本事实错误）；范围：能够到达 Canvas text provider 的创作节点，以及未走 Canvas bridge 的 run-level WorkflowAgent 兼容分支。** H-63 是对抗复核与停滞分类两条附属调用丢账，本项是工作流自身的主要节点调用；H-162 当前阻断四类专业 AI 节点，但 `text-gen` 等创作节点已经可触发本项，且修通专业节点后会立即继承同一缺口。

#### 官方字段、两条生产路径与动态复现

- DeepSeek [Chat Completions 参数页](https://api-docs.deepseek.com/zh-cn/api/create-chat-completion/)的完成响应包含 `prompt_tokens`、`completion_tokens`、`total_tokens`、`prompt_tokens_details.cached_tokens`、`prompt_cache_hit_tokens`、`prompt_cache_miss_tokens` 以及 `completion_tokens_details.reasoning_tokens`；其中 prompt 等于 hit+miss，reasoning 是 completion 的子集。官方 [KV Cache 指南](https://api-docs.deepseek.com/zh-cn/guides/kv_cache/)明确要求用 hit/miss 字段观察自动缓存效果。Responses 对应字段则是 `input_tokens_details.cached_tokens` 与 `output_tokens_details.reasoning_tokens`，不能把两套命名混用。
- Canvas OpenAI-compatible adapter 已能从同一 JSON 提取 `choices[].message.reasoning_content`，但 `parse_output()` 构造的 `CanvasModelOutput::Text` 只有 text/reasoning/provider/model；`CanvasCompletionResponse` 同样没有 usage 或 raw usage。`WorkflowAgent::execute_llm()` 的 bridge 分支收到结果后立即返回节点 outcome，只写 text/reasoning/provenance，不调用 `RunUsage::record_call()`。这不是 provider 没有返回 usage，而是 app-core bridge DTO 在解析边界主动丢弃。
- 新增默认忽略探针 `audit_canvas_completion_preserves_provider_usage_and_cache_details`：本机 HTTP 返回 text、reasoning 和完整 100 prompt / 20 completion / 80 hit / 20 miss / 12 reasoning usage，经正式 provider resolve、Chat parser 与 Canvas bridge。执行后 text/reasoning 正向对照均通过，但序列化 bridge response 只有 `text/reasoning/providerId/modelId`，`usage.prompt_tokens` 为 `None`，断言失败。由此把“整份响应解析失败”和“只丢计量”区分开。
- 新增完整归集探针 `audit_canvas_provider_usage_reaches_workflow_run_accounting`，改用 H-162 不阻断的正式 `text-gen` 图，执行 start 与模型节点；随后按 `run_assembler.rs` 的正式来源读取 `agent.cumulative_usage()`，交给真实 `AppRunFinalizer` 与同一 SQLite `CostService`。执行 `cargo test -p deepagent-app-core --lib audit_canvas_provider_usage_reaches_workflow_run_accounting --offline -- --ignored --nocapture`：**0 passed / 1 failed（预期）**；实际 `RunUsage` 九个字段全为 0，账本 input/output/hit/miss 也全为 0，而本机 provider 明确返回 100/20/80/20 与 reasoning=12。测试没有运行完整 Kernel；Kernel 的 `UsageRecorded` 和 token budget 均从同一个 `cumulative_usage()` 读取，因此其后果来自静态生产调用链，不冒充本探针已动态读取事件表。
- 不经 Canvas bridge 的旧兼容分支也不是正确对照。它从正式 `ModelClient::stream_chat_completion()` 得到已解析的 `deepagent_models::Usage` 后，手工只赋 `prompt_tokens`、累加 `completion_tokens`、赋 `total_tokens`，没有调用项目唯一语义 `RunUsage::record_call()`。默认忽略探针 `audit_run_level_workflow_model_preserves_deepseek_usage_buckets` 使用真实 Chat accumulator 返回同样 usage；实际结果为 prompt=100、completion=20、total=120，但 `billed_prompt_tokens=0`，reasoning/cache/billed cache 全为 0。`AppRunFinalizer` 按 billed 字段记成本，因此该分支会把输入记为 0、只保留输出；这与 bridge 分支的整行跳过不同，但同属重复实现 usage 规则导致的事实分裂。
- 正式 `run_assembler.rs::run_workflow()` 在 Kernel 后只把 `agent.cumulative_usage()` / `side_usage()` 交给 `AppRunFinalizer`；没有另一个 provider usage 来源可以补救。Kernel/RuntimeEngine 的 `UsageRecorded` 和 `max_total_tokens` gate 也读取该值；WorkflowAgent 没有覆盖 `take_pending_raw_usage()`，原始 DeepSeek usage 同样不会进入持久化诊断。桥接路径因此显示/记账为零，兼容路径则少计 prompt、cache 与 reasoning。没有调用公网、配置真实价格或推算人民币损失。

#### 成熟实现对照、影响与修复方向

- **DeepSeek Harness** 的 adapter 将 provider usage 作为独立 `usage` stream chunk 输出，durable `assistant/message` 同时可携带 usage；`token-meter/src/turn-usage.ts::deriveTurnTokenUsage()` 按每次 attempt 生命周期聚合 input/output/cache-read/cache-write/reasoning，并在字段不安全、矛盾或生命周期不完整时拒绝给出伪精确值，还保留 provider/model route 归属。它不是本系统的 CostService，但展示了内容投影和计量事实不应共用一个“只返回文本”的 DTO。
- **Codex** 没有本画布 bridge；其 core client 在 `ResponseEvent::Completed` 上继续传递 `token_usage`，协议 `TokenUsage` 明确保留 cached input 与 reasoning output，`TokenUsageInfo::append_last_usage()` 再统一累计。这里只对照“usage 随完成事件跨边界保留并由一个累计器处理”，不声称 Codex 有相同成本表或 DeepSeek Chat 字段。
- 已动态确认的后果是：成功模型请求可以在节点输出可见的同时，工作流累计用量和会话成本账本显示为零；兼容分支则把 100 输入 tokens 记为 0 且看不到 80 cache hit 与 12 reasoning tokens。缓存优化无法用工作流数据评估，预算/成本 UI 与 provider 事实不一致。`max_total_tokens` 使用 billed prompt+completion；由生产代码可知 bridge 会把本次消耗当 0、兼容分支只看到 20 output，因此配置的 token budget 可能晚停，但本轮没有专门跑超预算 Kernel，不能声称已观察到具体多发次数。
- 根因修复应让 Canvas text adapter 在解析 provider JSON 时生成现有 `deepagent_models::Usage`（以及必要的 raw usage），经 `CanvasModelOutput`、`CanvasCompletionResponse` 传回 WorkflowAgent；Agent 只调用唯一的 `RunUsage::record_call()`，不得再维护手写三字段规则。节点 outcome 可投影必要 usage 供声明了 usage 输出的专业节点使用，但成本、预算和持久化必须消费同一 canonical 对象，不能从 UI JSON 反解析。Chat/Anthropic/Gemini 各自 adapter 显式映射支持字段；不存在的 bucket 保持“未知”语义时需评估现有零值类型是否足够，不能把未知伪装成真实 0。
- 验收应覆盖 bridge 与 run-level 两路、单/多节点、多 attempt/fallback、失败/截断/取消、无 usage 响应、DeepSeek hit+miss/reasoning、Anthropic/Gemini 对应字段、`UsageRecorded.raw_responses_usage`、CostService、UI 和 `max_total_tokens`。每个 provider 已返回的调用只记一次；prompt gauge 与 billed sum、reasoning 子集和 cache bucket 不能重复相加。修 H-162 时必须同时跑本项回归，防止“节点终于可用但预算完全失真”。

### H-164：画布文本请求不在运行取消树内，节点完成态可与运行取消终态分裂

**级别：高（P1，停止可用性与状态一致性）；动态范围：正式 `text-gen` 的 provider 响应等待。** image/audio/embedding 的请求 DTO 和共用 HTTP 边界存在同样的静态缺口，但本轮没有逐项故障注入，不能写成四种模态均已动态复现。视频链已经通过 H-119 单独接入取消，不能用该修复推断其他 Canvas 请求也已覆盖。

#### 生产路径与动态复现

- `CanvasCompletionRequest`、`CanvasImageRequest`、`CanvasAudioRequest` 和 `CanvasEmbeddingRequest` 都没有运行取消字段；它们在 gateway 内重建的 `CanvasModelRequest` 也只有 operation、prompt、输入、size 和 timeout。与视频专用的 `CanvasVideoRequest.cancel` 不同，这四条调用链无法把 `run_assembler` 已有取消 flag 传到 provider wait。
- `WorkflowAgent` 只覆盖 `Agent::think()`：开始一个 step 时调用 `is_cancelled()`，随后直接 await `bridge.complete()` / `generate_image()` / `run_audio()` / `embed()`。它没有覆盖 `think_cancelled()`；trait 默认实现也只在调用旧 `think()` **之前**检查一次 flag。RuntimeEngine 正在 await 该 decision 时没有另一个 owner 中断 Canvas future。
- `CanvasModelGateway::call()` 是 text/image/audio/embedding 的共同 provider 边界。生产代码对 `req.body(body).send().await` 和 `resp.bytes().await` 直接等待，只设置 provider timeout；没有像修复后的视频 `await_video_operation` 一样对运行取消做 select。text/embedding 固定 120 秒，image 固定 300 秒，audio 默认 300 秒，因此停止延迟不是只有网络瞬时窗口。
- 新增默认忽略的完整链探针 `audit_text_cancel_does_not_wait_for_provider_response`：用正式 ChatService、WorkflowAgent、Kernel/RuntimeEngine、运行取消树、CanvasModelGateway、provider 持久化、文件 SQLite 与 RunStore 执行 start→text-gen→end；本机 HTTP listener 在收到真实 `POST /v1/chat/completions` 后记录请求但保持成功响应 pending。只有确认请求已经进入 provider 后才调用 `request_cancel()`，观察 300ms，再释放响应并回收 run/server。
- 精确命令 `cargo test -p deepagent-app-core --test workflow_audit audit_text_cancel_does_not_wait_for_provider_response --offline -- --ignored --nocapture` 为 **0 passed / 1 failed（预期）**。复核实际 `accepted=true, stopped_within_300ms=false, unfinished_before_response=true, cancel_to_exit_ms=332, requests=1`；332ms 包含 300ms 的响应前观察期，不是释放响应后单独等待 332ms。释放后 run 最终只有一个 `cancelled` 终态，但 text 节点已经发布 `Completed`，没有发布 `Cancelled`，最终断言失败。
- 这组交错证明的不是“取消请求没有送达”：RunCoordinator 明确返回 accepted，且最终运行确实映射为 cancelled。缺口是节点在入场前的单次取消检查之后失去运行取消身份；provider 返回时它仍提交成功节点结果，RuntimeEngine 后续才看到取消并终结 run，形成节点与运行终态分裂。测试使用成功 JSON 和本机 HTTP，不包含公网 DeepSeek、真实费用或远程任务撤销。

#### 成熟实现对照、影响与修复方向

- **DeepSeek Harness** 的 `llm-deepseek/src/adapter.ts` 明确把一次模型请求定义为“one cancellable lifecycle”：将 consumer signal 与调用者 `options.signal` 合并，同一 signal 传给图片准备、Files API、请求扩展、fetch 和 stream watchdog；generator 收尾还 abort reader 并 await iterator cleanup。该参考实现走 Messages 协议而非本系统的 Canvas/OpenAI-compatible 路径，不能复制字段形状；可对照的是一次模型调用从输入准备到正文消费共享同一取消身份。
- **Codex** `core/src/tasks/mod.rs::handle_task_abort` 先 cancel turn token，等待任务完成通知或 100ms grace，随后 abort task handle 并调用任务专属 cleanup。它没有本系统的 Canvas 节点，也不证明远端 provider 已取消；这里仅对照“运行 owner 能在协作取消失效时有界收回任务，并在清理后发布中断生命周期”。
- 动态确认的后果是停止按钮已经显示受理，工作流仍至少等待当前 provider 响应；若 provider 只在 timeout 返回，text 可等至 120 秒，image/audio 静态上限可到 300 秒，但本轮没有实等这些上限。请求已经到达 provider 后，本地取消也不能保证撤回已发生的 token 消耗或服务端副作用。响应随后到达时节点先发布 Completed、run 再标 cancelled，会让实时 UI、run_events replay 和依赖节点状态的机器消费者得到互相矛盾的事实；本探针没有证明 end 或后继节点继续执行，也没有推算真实费用。
- 根因修复不应给四种模态各写一套 flag 特判。应在现有 Canvas bridge/request 汇合边界携带运行取消身份，使输入引用解析、上传/准备、provider send、响应 headers/body 和必要的 artifact commit 共用一个 cancel-aware helper；video 的现有 helper 可提炼语义，但不同协议的外部副作用与 cleanup 仍需显式处理。取消必须返回可识别的 cancelled outcome，让 WorkflowAgent 发布节点 Cancelled，且不能在 cancel 已发生后提交 Completed，再由 RuntimeEngine 事后改写 run。
- 验收需要为 text/image/audio/embedding 分别覆盖：请求发送前、headers pending、body pending、输入引用下载/上传、artifact staging、正常成功、provider error、timeout 与取消/响应竞争；检查取消受理后有界退出、无迟到 Completed、唯一运行终态、规范节点事件可 replay、没有半成品，并明确“本地停止”不冒充“远端 provider 已撤销”。H-119 的 video submit/poll/download 回归必须同批保留，防止统一 helper 改造倒退。

### H-165：图片供应商可令宿主下载任意私网 URL，并把响应作为成功 artifact 保存

**级别：高（P1，条件性 SSRF / 宿主网络边界缺失）；触发条件：已配置的 image-gen 供应商或兼容网关在成功响应中返回指向 loopback、私网、链路本地或云元数据地址的 `data[0].url`。** 本项审计测试本身是正常且安全的本机故障注入：只绑定两台 `127.0.0.1` 临时服务和虚构 canary，不访问公网、真实内网、云元数据或用户服务。

#### 生产路径与完整工作流复现

- `CanvasModelGateway::parse_output()` 处理 image generation/edit 成功响应时，若没有 `b64_json` / inline data，就直接读取 provider 给出的 `url` 并调用 `fetch_binary(url, None)`。该 URL 不要求与已配置 provider 同源，也没有由用户配置的 artifact host allowlist。
- `fetch_binary()` 新建一个宿主 `reqwest::Client`，对字符串执行 GET；只检查最终 HTTP status，然后 `resp.bytes()` 整体读入。它没有拒绝 loopback、RFC1918、链路本地/metadata 地址，没有把 DNS 解析结果绑定到已验证地址，也没有为每次 redirect 重验目标；仓库锁定的 `reqwest 0.12.28` 在未设置 redirect policy 时默认最多跟随 10 跳，当前 builder 正是这个默认配置。该调用不经过 ToolRegistry、PermissionSet、ChannelApprovalGate 或沙箱网络代理。
- 返回字节只需让 `sniff_image_mime()` 识别，随后 `generate_image()` 就通过现有 CanvasArtifactService 保存并发布 Completed；下载来源身份没有进入 artifact policy。H-164 已覆盖此同步请求链的取消缺口，本项独立聚焦目的地址授权，不用“加取消”代替 SSRF 修复。
- 新增默认忽略探针 `audit_provider_image_url_cannot_fetch_loopback_resource`：正式 ChatService、WorkflowAgent、Kernel/RuntimeEngine、CanvasModelGateway、provider 持久化、文件 SQLite、RunStore、节点事件与 artifact store 执行 start→image-gen→end。第一台 loopback 服务是正常 OpenAI-compatible image provider，收到正式 `POST /v1/images/generations` 后返回第二台 loopback 服务的 `/host-private.png`；第二台只记录请求并返回带 PNG 魔数的虚构 canary。
- 精确命令 `cargo test -p deepagent-app-core --test workflow_audit audit_provider_image_url_cannot_fetch_loopback_resource --offline -- --ignored --nocapture` 为 **0 passed / 1 failed（预期）**。实际 `provider_requests=1, private_requests=["GET /host-private.png HTTP/1.1"], approvals=0, terminal="succeeded", artifact_bytes=36, canary_stored=true`。测试在完整收尾、读回 RunStore 与 artifact 后才以“私网服务不得被访问、canary 不得被保存”断言失败。
- 这证明正常可达的供应商响应能够驱动宿主发出第二跳私网 GET 并持久化响应，不是测试代码直接调用 `fetch_binary()`。测试没有模拟 DNS rebinding/redirect、没有读取真实秘密，也没有证明 canary 被再次发给供应商。潜在外传需要后续节点或其他消费方把 artifact 再发送出去；本轮只把已动态确认的内部访问与持久化作为事实。

#### 官方边界、成熟实现与后果

- DeepSeek 官方 [Responses 参数文档](https://api-docs.deepseek.com/zh-cn/api/create-response/)和[使用指南](https://api-docs.deepseek.com/zh-cn/guides/responses_api/)允许**请求输入**中的 `input_image.image_url` 由 provider 获取；这不是让本地客户端信任**响应输出**中的任意 URL。Canvas image generation 是本系统额外的 provider 集成，不能用 DeepSeek 输入协议为宿主第二跳下载背书。
- **DeepSeek Harness** 的 `llm-deepseek/src/images.ts::prepareImages()` 从已有 AttachmentStore 按 durable attachment identity 读取请求图片，并执行模型模态、角色、字节和数量预算；adapter 的 `imageAccess` 也只解析宿主已有附件。它没有本系统的图片生成输出下载，不能称其已通过同款 SSRF 测试；可借鉴的是 provider 请求只消费有 owner 的资产身份，不把 provider 返回的字符串自动升级为宿主网络授权。
- **Codex** 没有本系统的 Canvas image-gen 下载器；其 network proxy 把目标域、managed allowlist、阻断观察和 network approval 组织为同一策略边界，在需要审批且未硬拒绝时由 policy decider 决定。这里对照的是“外部数据不能绕过运行的网络策略直接使用宿主网络”，不是复制 Codex 的进程代理实现。
- [OWASP SSRF Prevention Cheat Sheet](https://cheatsheetseries.owasp.org/cheatsheets/Server_Side_Request_Forgery_Prevention_Cheat_Sheet.html)将服务端根据不可信 URL 下载图片列为典型 SSRF 场景，建议优先 allowlist；必须支持任意公网地址时，至少在连接时拒绝 loopback、私网、链路本地和 metadata 地址，并禁用 redirect 或对每个 redirect 重新执行同一验证。单纯检查 `http(s)` scheme 或最初 hostname 不足以抵抗 redirect 与 DNS rebinding。
- 已确认后果是供应商能使桌面宿主访问用户自己未配置的本机/私网 HTTP 目标，并将响应保存为成功图片 artifact。实际环境中这可能用于探测内部服务、访问 metadata、触发不应由外部供应商决定的 GET，或把私网响应交给后续节点；但本轮没有执行这些真实动作，因此不声称已经取得凭据、触发真实副作用或完成外传。

#### 根因修复与验收

- 在现有 CanvasArtifactService/provider seam 内建立唯一的 `ProviderArtifactFetcher`，不要让 image/video/audio 各自新建裸 reqwest client。下载策略由可信本地配置决定：优先使用 provider 配置的 HTTPS artifact host allowlist/CDN 列表；若业务必须接受任意公网 URL，则在连接时校验所有解析地址为全局可路由地址，拒绝 userinfo、loopback、私网、链路本地、multicast 与 metadata 主机，并对每次 redirect、重试和备用地址重复校验。开发环境确需 loopback 时使用显式、可审计的本地 provider profile，不把例外设为默认。
- 该边界同时继承现有运行取消树、连接/首字节/idle/总时限、响应字节上限和流式 artifact staging；验证 Content-Type 与实际魔数，不在完整 body 读入后才发现超限。下载日志只能记录脱敏来源身份，URL query/userinfo 不进入普通事件。若选择审批未知 host，复用既有 ChannelApprovalGate 与 run scope，不能另造第二套授权中心。
- 验收覆盖 IPv4/IPv6 loopback、RFC1918、link-local/metadata、十进制/十六进制等地址表示、DNS 多地址与 rebinding、公开地址重定向到私网、跨 scheme/port、provider/CDN allowlist、正常 inline base64、合法公网 artifact、取消和超限清理。断言未授权目标请求数为零、无 artifact/Completed、唯一失败终态；显式本地开发例外和已批准公网目标则可成功。视频 H-119/H-122 及 H-164 的取消回归需同批保留。

### H-166：DeepSeek Responses 思考模式下 top_p 会被忽略或规范化，设置值不等于有效采样值

**级别：中（P2，条件性采样配置不生效）；触发条件：选择 `deepseek-flash` 并在思考模式配置 `top_p < 0.95`，或在非思考模式配置 `top_p != 1.0`；也包括选择 `deepseek-v4-pro` 并在思考模式显式配置任意 `top_p`。** Flash 思考模式对 `0.95–1.0` 使用传入值、低于 0.95 按 0.95 处理；Flash 非思考模式有效值恒为 1.0；Pro 思考模式无工具/有工具分别固定为 1.0/0.95；Pro 非思考模式使用传入值。该项不表示 API 请求失败，也不声称答案必然不同。

#### 官方规则与生产 wire 复现

- DeepSeek 官方 [Responses 指南兼容性表](https://api-docs.deepseek.com/zh-cn/guides/responses_api/)明确：`deepseek-flash` 的 `top_p` 仅在思考模式生效，有效范围 0.95–1.0，低于 0.95 按 0.95 处理；非思考模式恒为 1.0，传入值被忽略。对 `deepseek-v4-pro`，思考模式会忽略传入值：无工具时有效值为 1.0，有工具时固定为 0.95；非思考模式才使用传入值。官方 [Create Response 参数页](https://api-docs.deepseek.com/zh-cn/api/create-response/)也说明 Responses 默认 reasoning 行为开启，`low/high/max` 开启思考，`none` 关闭思考。
- 本系统设置层 `ResponsesApiSettings::validate()` 只检查 `top_p ∈ [0,1]`；`model_runtime::select_run_model()` 将用户有效 top_p 与 reasoning effort 分别注入 `ResponseDefaults`，`ModelClient::apply_defaults()` 不按模型/effort/工具集合校验或规范化，Responses serializer 原样发出这些字段。因此 UI/设置中显示的 0.2 并不总是 DeepSeek Flash/Pro 实际采用的有效 top_p。
- 新增 opt-in `crates/deepagent-models/tests/reasoning_effort_audit.rs::audit_flash_thinking_mode_top_p_below_floor_reaches_wire_unchanged`，走正式 `ModelClient::stream_response()` 与 Responses serializer，使用离线 `AuditTransport` 捕获请求。精确命令 `cargo test -p deepagent-models --test reasoning_effort_audit audit_flash_thinking_mode_top_p_below_floor_reaches_wire_unchanged --offline -- --ignored --nocapture`：**0 passed / 1 failed（预期）**；实测 `wire_effort=high`、配置 `top_p=0.2`、wire `top_p=0.2`，与官方有效值 0.95 冲突。此测试验证本地配置与实际发出的 wire，不伪造服务端采样结果。
- 新增同文件 `audit_model_specific_top_p_rules_reach_wire`，仍只走正式 `ModelClient::stream_response()` 与离线 `AuditTransport`，覆盖 Pro thinking 无工具/有 function 工具、Pro `none` 和 Flash `none` 四组对照。精确命令 `cargo test -p deepagent-models --test reasoning_effort_audit audit_model_specific_top_p_rules_reach_wire --offline -- --ignored --exact --nocapture`：**0 passed / 1 failed（预期）**；实测序列为 `[(Pro, no tools, high, 0.2), (Pro, tools, high, 0.2), (Pro, no tools, none, 0.2), (Flash, no tools, none, 0.2)]`。与官方有效值 `[1.0, 0.95, 0.2, 1.0]` 比较，前两项 Pro thinking 和最后一项 Flash none 不一致，Pro none 对照一致。仅请求 body 在本地 transport 被捕获；没有调用公网 API，也没有把服务端规范化伪装成离线 transport 的事实。

#### 成熟实现对照、影响和修复方向

- DeepSeek Harness `packages/llm/llm-pi-ai/src/adapter.ts` 显式映射 reasoning effort，并在请求构造中只按 adapter/模型 profile 提供的参数发送；其 profile 还会在未声明 effort 时拒绝选择，而不是把不适用配置直接交给 provider。这里是参数适配边界的设计对照；未找到其对 DeepSeek Flash `top_p < 0.95` 或 Pro 思考模式 `top_p` 忽略规则作同款钳位/拒绝的实现，不能称其已解决同一规则。
- Codex 的模型目录带每模型 supported/default reasoning effort，turn 请求使用所选模型能力构造有效参数；这是模型能力随选择流动的架构对照，不是 DeepSeek top_p 规则的同协议处理证据。
- 触发时 Flash 会按 0.95 处理；Pro 思考模式则按工具集合分叉为 1.0/0.95，而 UI、保存配置和请求日志仍呈现 0.2。用户无法凭本地回显判断真实有效设置，实验复现性和参数预期失真；工具目录变化还会让同一设置在 Pro 思考模式下产生不同有效采样值。未证明该规范化造成 API 错误、额外费用或特定回答质量变化。
- 收敛方向：在现有模型能力/Responses adapter 边界计算一次 `EffectiveSamplingConfig`，依据所选模型、最终 reasoning effort 和最终工具集合明确拒绝、提示并规范化，或在 UI 展示 Flash 的 0.95 下限及 Pro 思考模式的 1.0/0.95 规则；不得悄悄将 wire 变成另一数值而不反馈。fallback 切换、关闭思考、developer JSON 覆盖、工具目录变化、重试与运行设置展示都必须使用同一解析结果。回归覆盖 Flash thinking 的 0.94/0.95/1.0、Flash none、Pro thinking 无工具/有工具、Pro none 及备用模型；只用受控 wire 测试证明本地契约，服务端有效行为仍以官方文档为依据。

### H-167：画布图片下载无响应字节上限，超预算数据被完整缓冲并持久化

**级别：中（P2，条件性内存/磁盘资源消耗）；触发条件：image-gen 供应商返回可达 URL，且该 URL 响应体显著大于合理图片预算。** 这与 H-165 的“目的地址是否获授权”是独立控制：即使目标是允许访问的公网 CDN，也仍需限制响应体大小；反过来，设置大小上限不会使 loopback/私网访问因此变得安全。

#### 生产路径与本机容量复现

- `CanvasModelGateway::parse_output()` 对 provider `url` 调用 `fetch_binary()`；该 helper 只检查 HTTP status，随后 `resp.bytes()` 将整个响应物化为 `Vec<u8>`，没有依据 `Content-Length` 提前拒绝，也没有增量读取计数/硬上限。结果经 `generate_image()` 调用 `CanvasArtifactService::import_bytes()`；通用 `ArtifactStore` 不按 Image 类型限制字节数。图片嗅探只需要前缀签名，不能形成大小边界。
- 本系统 `FileAttachmentResolver` 为用户输入图片设置了 32 MiB 单图上限，但这只是输入请求预算，不是供应商生成图片的正式输出契约；本项用它作本地可比较的现存边界，不推导 DeepSeek 或所有图片供应商都规定 32 MiB 输出限制。Canvas artifact/import 与磁盘配额没有发现等价硬限制。
- 扩展 H-165 的双 loopback fixture 后新增 opt-in `audit_provider_image_download_rejects_oversized_artifact`。供应商服务返回 32 MiB+1 字节、前 8 字节具有 PNG signature 的受控响应，完整经过真实 image-gen workflow、Canvas gateway、ArtifactService、SQLite 和 RunStore。精确命令 `cargo test -p deepagent-app-core --test workflow_audit audit_provider_image_download_rejects_oversized_artifact --offline -- --ignored --nocapture`：**0 passed / 1 failed（预期）**；记录 `private_requests=1, configured_input_limit=33554432, response_bytes=33554433, artifact_bytes=33554433, terminal="succeeded"`。断言在完整读取并回放 artifact 后失败，说明不是测试本身直接写文件。
- 该实验只证明 32 MiB+1 的完整缓冲和持久化，未测较大 payload 下的峰值 RSS、系统 OOM、磁盘耗尽、并发放大或长期累积。潜在后果是多个大响应/并发生成造成额外内存拷贝与工作区 artifact 膨胀，桌面宿主变慢、写盘失败或被操作系统终止；这些后果的阈值尚未实测，不能声称已触发。

#### 无限画布对照与收敛方向

- 本地参考项目 Penguin-Magic 的 `backend-nodejs/src/routes/assets.js::downloadRemoteImageToUserAssets()` 先检查响应 `Content-Length`，再检查实际 `arrayBuffer` 长度，超过 `config.MAX_FILE_SIZE` 返回 413；`backend-nodejs/src/config.js` 将其设为 10 MiB。它展示了“响应声明与实际字节都要受限”的资产导入契约，但同样先完整读取 arrayBuffer，因此不是内存有界流式下载的证明。该路径属于用户资产保存路由，不宣称 NodeCanvas 每个模型下载器都复用它。
- Codex / DeepSeek Harness 没有本系统 Canvas image-gen 输出下载器，不能伪称存在同款实现。可迁移的原则是由 artifact/adapter 边界统一实施有类型的响应预算，并将限制结果明确反馈给节点运行，而不是让一个模型 URL 决定宿主资源消耗。
- 在现有 CanvasArtifactService/provider seam 定义明确的 image output byte budget；下载优先检查声明长度，但不能信任或依赖 `Content-Length`，必须按流累计并在超过预算的第一个 chunk 时中止、清理 staging、不得创建 artifact/Completed。inline base64 JSON 同样在解码前估算并在解码后硬性校验，避免只约束 URL 下载。单文件限制与 workspace 总配额/并发预算分开设计；budget 需按真实供应商图像格式和工作流确定，不直接把用户输入附件 32 MiB 或 Penguin 的 10 MiB 照搬为通用规格。
- 验收覆盖长度缺失/谎报/超限、chunked 超限、刚好等于预算、inline base64 超限、网络取消、写盘失败及多节点并发；检查内存行为为流式有界、无超限 artifact、staging 清理、节点失败和唯一 run 终态。H-165 地址授权与 H-164 取消回归需分别保留，互不替代。

### H-168：易变 instructions 位于历史之前，动态变化截断多轮 KV cache 前缀

**级别：中（P2，持续性性能/费用问题）；触发条件：同一 Responses 会话连续请求之间，工作区结构、日期、权限、插件/MCP/skills 或其他 leading dynamic instructions 发生变化。** 该缺口不改变本地保存的会话历史，也没有证据表明它会直接令回答错误；影响是上一轮完整历史不能继续作为本轮的相同请求前缀，因而降低 DeepSeek 自动缓存可复用范围，并可能增加首 token 延迟与未命中输入费用。

#### 官方规则、生产路径与动态复现

- DeepSeek 官方 [KV Cache 指南](https://api-docs.deepseek.com/zh-cn/guides/kv_cache/)说明缓存默认自动构建，后续请求只有完整匹配已落盘的前缀单元才可命中；`A+B` 后请求 `A+B+C` 可复用前者，而 `A+B` 后请求 `A+C` 不能命中旧单元。官方 [Responses 参数页](https://api-docs.deepseek.com/zh-cn/api/create-response/)又明确：`instructions` 是模型上下文中的**第一条 system 消息**，API 无状态，多轮必须在每次 `input` 回传完整历史；[Responses 指南](https://api-docs.deepseek.com/zh-cn/guides/responses_api/)还明确不支持 `prompt_cache_key` / `prompt_cache_retention`，缓存由服务端自动管理。因此 wire JSON 中字段书写顺序不是本项依据，语义提示序列是 `instructions → input history`。官方 [Tool Calls 指南](https://api-docs.deepseek.com/zh-cn/guides/tool_calls/)进一步明确 Responses 支持在对话中间插入工具调用和 `system` 消息，因此 adapter 有官方允许的 append-only 表达，不必把每次更新都改写为 leading instructions。
- 当前 `build_system_manifest()` 把基础 system prompt 与内置输出风格放在 `<<<DYNAMIC>>>` 前，把日期、工作目录、Git/项目结构、权限、插件风格、延迟工具目录、MCP instructions 与 skills 放在同一个 `instructions` 字符串的边界后。该 marker 只能让基础 prompt 自身拥有稳定公共前缀；它不会把后半段移到历史后面。更底层的 `response_items_from_messages()` 还会无条件收集**所有位置**的 `Role::System`，拼成顶层 `instructions`，其余消息才成为 `input`；这使上层即使构造官方允许的中途 system，也会在 provider adapter 被重新提升到历史之前。
- 新增 opt-in serializer 探针 `responses::tests::audit_preserves_mid_history_system_messages_as_input_items`：输入 `leading system → user → assistant → updated system → user`，精确命令 `cargo test -p deepagent-models responses::tests::audit_preserves_mid_history_system_messages_as_input_items --offline -- --ignored --exact --nocapture` 为 **0 passed / 1 failed（预期）**。实际 `instructions=Some("stable leading rules\n\nupdated runtime context")`，`items` 只剩 user/assistant/user，正式转换器完全删除了中途 system 的原位置。第一次用不完整测试名加 `--exact` 得到 0 tests，不计为验证，已改用完整模块路径重跑。
- 先保留正常对照 `consecutive_session_requests_preserve_responses_prefix_on_wire`：在两轮之间环境未变化时，正式 SettingsService、ChatService、Kernel、ModelAgent、ModelClient 的第二次 wire 确实以第一次完整 `input` 为前缀，instructions 与工具 schema 也保持一致；精确命令 **1 passed**。这只证明静态环境下可缓存形状正确。
- 新增 opt-in `audit_dynamic_instructions_do_not_cut_off_conversation_cache_prefix`，沿相同正式链路在临时 Rust workspace 完成第一轮后新增 `src/new_after_first_turn.rs`，再记录第二轮 Responses JSON。精确命令 `cargo test -p deepagent-app-core --test kernel_v2_e2e cache_prefix::audit_dynamic_instructions_do_not_cut_off_conversation_cache_prefix --offline -- --ignored --exact --nocapture`：**0 passed / 1 failed（预期）**。实测 `stable_instruction_bytes=12934, first_difference=12987, first_instructions_bytes=14238, second_instructions_bytes=14264, retained_first_input_items=1, second_input_items=3`；第二轮 `input` 的确保留第一轮前缀，但 leading instructions 已在它之前发生首个差异，最终相等断言失败。
- 该离线探针证明的是请求构造的前缀关系，不是服务端缓存观测。DeepSeek 缓存为尽力而为，而且公共前缀可能在后续请求才另行落盘；不能由一次字节差异声称真实请求“必然零命中”。线上只能以 Responses `usage.input_tokens_details.cached_tokens` 观察本次命中。本系统对该 Responses 字段的解析及 miss 推导已有测试通过，没有把 Chat 的 `prompt_cache_hit_tokens` 字段名套入 Responses，这部分不是缺陷。

#### 成熟实现对照、影响边界与收敛方向

- DeepSeek Harness 的 DeepSeek 模型目录默认把 `deepseek-flash` 声明为 `systemPromptUpdate: 'in-history'`；其 serializer 保留最初 leading system，并将后续完整 system 快照排在对应 user/tool-result turn 后。其架构文档和 README 明确说明，这样变化发生在已缓存历史之后，历史末尾之前的前缀仍有资格复用。DeepSeek 官方 `/models` 当前只在 `api_capabilities.anthropic_messages.system_prompt_update` 下为 Flash 声明 `in-history`、Pro 声明 `leading-only`，所以 Harness 的能力开关和替换语义不能原样移植；但 Responses 自身允许中途 system 已由 Tool Calls 指南独立确认。
- Codex `core/tests/suite/prompt_caching.rs::overrides_turn_context_but_keeps_cached_prefix_and_key_constant` 保留第一请求全部 `input` 作为第二请求前缀，然后追加更新后的 permissions/environment 与新用户消息。可借鉴的是“易变上下文追加在旧历史之后”的序列设计；Codex 的 `prompt_cache_key` 不能移植到 DeepSeek，因为 DeepSeek Responses 明确不支持该参数。
- 触发越频繁、历史越长，损失越大：文件创建/删除、跨日运行、沙箱切换、MCP 或 skill 目录变化都可能使后续请求只能复用较短的基础 instructions 前缀，而不能复用上一轮结束位置对应的完整历史单元。可能后果是 cached tokens 减少、输入账单上升和响应延迟增加；本轮没有真实服务端 A/B、费用样本或延迟测量，不量化百分比，也不声称回答质量下降。
- 根因收敛不能简单删除动态上下文。Responses adapter 应保留第一条稳定 system 为 `instructions`，把之后的 workspace/权限/能力快照作为官方允许的中途 system input item 维持在原始历史位置；快照仍需版本化、持久化、可 replay，meter、恢复、压缩、权限更新和工具目录变化必须读取同一个 context version。需真实验证“新 system 是增量还是完整替换”的模型语义，不能仅因字段可发送就照搬 Anthropic `in-history` 的替换规则。若某 provider/model 不能安全表达所需更新，再显式开启新的 request series，并让 usage/诊断显示缓存序列已重启。
- 验收覆盖：无变化、工作区变化、跨日、权限变化、工具/MCP/skills 增删、压缩前后、进程重开恢复及 provider fallback；逐项断言旧 request 的完整语义输入仍是下一 request 的前缀，动态快照只追加一次且 replay 一致。真实 DeepSeek 集成测试再用 `input_tokens_details.cached_tokens` 对照同长度静态/动态序列；缓存尽力而为，单次 miss 不能直接判定实现失败，应以重复样本和 wire 前缀共同归因。

### H-169：桌面接受任意 `image/*`，不支持的 SVG 被原样发送给 DeepSeek Responses

**级别：中（P2，请求可用性与历史持续失败）；触发条件：用户在桌面 Composer 选择浏览器标记为 `image/*`、但 DeepSeek 不支持的格式，例如 `image/svg+xml`。** 当前普通入口没有格式 allowlist 或真实字节解码；附件进入历史后，后续轮次会继续尝试发送同一无效图片。H-67 会令当前轮图片未进入首次请求，但不能当作保护：本项在第二轮 retained-history 路径已独立复现。

#### 生产路径与完整链复现

- DeepSeek 官方 [Responses 参数页](https://api-docs.deepseek.com/zh-cn/api/create-response/)把 `input_image.image_url` 支持格式明确限定为 JPEG、PNG、GIF、WebP，值可为 HTTP(S) URL 或相应 base64 data URL；“浏览器认为它是 image”不等同于 provider 支持。官方 [Responses 指南](https://api-docs.deepseek.com/zh-cn/guides/responses_api/)说明图片限制沿用视觉输入契约。
- 桌面 `Composer.tsx::addFiles()` 仅用 `file.type.startsWith("image/")` 决定 `kind: "image"`，并把浏览器 MIME 原样交给 `attachmentIngest`。因此 SVG、BMP、TIFF、AVIF 等都走图片分支；`accept` 也未在这个 Composer input 上形成四格式限制。
- `AttachmentService::ingest()` 原样持久化 `kind`/`mime`；`kind == "image"` 时不解码图片，直接标记 ready。`FileAttachmentResolver::is_resolvable()` 只检查 kind、文件存在及 32 MiB 大小；`data_url()` 优先信任持久化 `media_type`，只有缺失时才按扩展名嗅探四种格式，也没有核对声明 MIME 与真实字节。最终 Responses serializer 把所得字符串作为 `input_image.image_url` 发出。
- 新增 opt-in `image_preparation::audit_unsupported_image_mime_is_rejected_before_transport`：通过正式 AttachmentService 和桌面同形 `AttachmentIngestDto` 保存一个语法完整的 1×1 SVG，逐字节回读确认落盘内容，再经 Vision Off、ChatService、Kernel、ModelAgent 和 ModelClient 执行首轮带附件及第二轮普通续聊。精确命令 `cargo test -p deepagent-app-core --test kernel_v2_e2e image_preparation::audit_unsupported_image_mime_is_rejected_before_transport --offline -- --ignored --exact --nocapture`：**0 passed / 1 failed（预期）**；记录 `urls_per_request=[[], ["data:image/svg+xml;base64,..."]]`。第一组空数组是已知 H-67 控制，第二轮完整 SVG 到达离线 transport 后运行成功，最终“不允许 SVG 到达 transport”断言失败。
- 已证事实是本地 provider boundary 发送了官方不支持的格式；fixture 的离线 transport 有意返回成功，故不能声称真实 DeepSeek 已返回某个 HTTP 状态。真实服务可能拒绝请求；若拒绝，因为无效附件仍在持久历史中，普通续聊可能反复失败或需要用户显式移除/分支。未测自动重试次数、桌面错误展示、SVG 大小/复杂度、BMP/TIFF/AVIF 或真实费用。

#### 成熟实现对照与收敛方向

- DeepSeek Harness 的 `attachment-local` 把允许媒体类型固定为 `image/png`、`image/jpeg`、`image/webp`、`image/gif`；保存前 `detectImage()` 解码真实字节，随后比较检测 MIME 与声明 MIME，不一致抛 `IMAGE_TYPE_MISMATCH`，再进行规范化和容量验证。它不是依赖文件扩展名或调用方 MIME。
- Codex `protocol/src/models.rs::local_image_unsupported_image_format_adds_placeholder` 用真实 SVG 文件验证输入投影：不生成 image item，而是产生明确的“unsupported image `image/svg+xml`”文字占位，保留路径和可诊断原因。这里借鉴的是显式、可见的降级，不要求本系统采用相同文案。
- Penguin-Magic 的 `NodeCanvasAssetPanel` 对画布资产入口同时用四格式 MIME allowlist、`accept="image/png,image/jpeg,image/webp,image/gif"` 和大小检查；这只覆盖该资产面板，不证明其所有图片入口都安全，但说明无限画布参考项目没有把任意 `image/*` 当成同一 provider 输入格式。
- 根因修复应在既有 AttachmentService/provider image preparation seam 建立唯一的图片身份与格式验证：后端按真实字节检测格式并与声明 MIME 对照，再按本次有效 provider/model 的支持集决定保留、受控栅格化或明确拒绝；UI `accept` 只作提前反馈，不能替代后端边界。SVG 若提供转换，必须在有尺寸/资源上限的隔离解码器中栅格化为受支持格式，不能仅把 MIME 改成 PNG。错误需要进入现有运行/附件诊断，不能像 H-68 一样静默退化为纯文字成功。
- 验收覆盖四种支持格式的真实解码与 MIME 一致性、SVG/BMP/TIFF/AVIF、伪造 MIME、扩展名错配、空/损坏/超大图片、当前轮与 retained history、工具结果图片、模型切换/fallback、重启和压缩；断言非法格式 transport 调用数为零、合法格式字节身份稳定。H-67 当前轮投递、H-69 总体积预算、H-70 内容摘要和 H-127 MCP 图像投影需同批保留，不能相互替代。

### H-170：历史图片每轮重发完整 base64，未接入 DeepSeek Files API 的持久引用

**级别：中（P2，请求体、延迟、带宽与官方能力利用）；触发条件：合法图片进入 retained history 后，同一会话继续发起一个或多个模型请求。** 小图只造成固定冗余；真实截图、多图和长会话会在每轮重新编码并上传相同字节，还会更快逼近 48 MiB 请求体边界。H-69 是“超限 inline 请求仍进入 transport”的边界校验缺陷；本项即使图片和请求均未超限也会发生，是 provider 文件身份与复用能力缺失，不能互相替代。

#### 官方契约、生产路径与完整链复现

- 2026-10-24 实际打开 DeepSeek 官方 [Responses 指南](https://api-docs.deepseek.com/zh-cn/guides/responses_api/)与 [Responses 参数页](https://api-docs.deepseek.com/zh-cn/api/create-response/)：`input_image` 必须在 `image_url` 与 `file_id` 中二选一；后者引用 Files API 上传的图片。Responses 是无状态 API，多轮请求需回传完整历史，因此历史里的 inline data URL 也会随完整 `input` 再次发送。
- 官方 [图像理解](https://api-docs.deepseek.com/zh-cn/guides/vision/)明确把“多个请求复用同一张图片，希望避免每次重复上传”列为 Files API 的适用场景；inline/base64 计入 48 MiB 请求体，单张最多 32 MiB，`file_id` 单张可到 64 MiB；不含 `file_id` 的图片总量最多 64 MiB，含 `file_id` 时最高 200 MiB，单请求最多 600 张。这是传输与容量能力，不据此推断视觉 token 会减少。
- 当前 `ModelAgent::request_for_current_history()` 每轮遍历历史 `InputMessage.attachments`，调用 `AttachmentDataResolver::data_url()` 后写入 `ResponseRequest.attachment_data_urls: BTreeMap<attachment_id, data_url>`；重试还会重新复制同一 map。`ResponseInputItem::InputMessage` 只保留通用 `MessageAttachment`，没有 provider 文件引用；`response_input_message_to_wire()` 对每个可解析图片固定输出 `{"type":"input_image","image_url":...,"detail":"auto"}`。
- 对生产 `crates/` 与 `apps/` 排除测试、文档、参考目录后的全局检索没有 DeepSeek `/v1/files` 上传、`file_id` 请求类型、上传索引、过期刷新或远端失效重传实现；命中的 `file_id` 只属于 codegraph 内部文件节点，不是 provider Files API。故这不是“上传存在但测试没走到”，而是当前 provider seam 无法表达该生命周期。
- 新增 opt-in `image_preparation::audit_retained_image_reuses_deepseek_file_id`：通过正式 AttachmentService 导入可解码的 68 字节 PNG，经 ChatService、Kernel、ModelAgent 和 ModelClient 在同一 session 完成首轮及两次普通续聊，离线 transport 只解析最终 JSON 并返回正常 Responses 事件。精确命令 `cargo test -p deepagent-app-core --test kernel_v2_e2e image_preparation::audit_retained_image_reuses_deepseek_file_id --offline -- --ignored --exact --nocapture`：**0 passed / 1 failed（预期）**；记录 `inline_counts=[0,1,1]`、`file_id_counts=[0,0,0]`、`request_bytes=[41721,42259,42483]`、`repeated_inline_chars=228`、`same_inline_payload=true`。第一轮零图片是已知 H-67 控制；判断依据是后两轮均发送同一个完整 data URL，且没有任何 `file_id`。
- 同一文件中的正常对照 `retained_imported_image_reaches_continuation_request` 为 **1 passed**，证明失败不是附件未持久化或续聊未读取。探针没有访问公网、没有模拟虚假的 Files 成功，也不把 1×1 样本的 228 字符外推为真实费用、延迟或 token 数字。

#### 成熟实现对照、影响边界与收敛方向

- DeepSeek Harness 的 `llm-deepseek` 把 request-image 版本映射到用户/连接作用域内的 `file_id`：`runtime.spec.ts::uploads a durable image once and sends only its Files API id` 验证只 POST 一次 `/v1/files`，模型请求只携带引用；`DeepSeekFileStore` 以 endpoint/认证作用域和确定性图片 variant 建索引、合并并发上传，默认上传寿命 7 天、提前 1 小时刷新。Files 解析失败时整次请求统一回退为受独立预算约束的 all-base64；provider 报 file ID 过期/删除时只失效对应映射并重传一次，避免永久坏引用和无界重试。
- Codex `protocol/src/models.rs` 用 `ImageReference::Inline { image_url } | File { file_id }` 明确区分两种 wire 身份；`file_image_user_input_serializes_file_id` 验证 file-backed 用户图片直接序列化为 `input_image.file_id`。这只证明成熟协议层没有把图片来源压成 data URL；本轮不宣称 Codex 已实现 DeepSeek 专属上传、过期和配额生命周期。
- 影响随图片字节、历史轮数、重试次数和并行分支线性放大。可能后果包括重复 base64 编码与内存分配、更多上行字节、更大的请求解析延迟、48 MiB body 提前失败，以及本可由 64 MiB `file_id` 承载的单图无法使用。图片仍会进入模型并按尺寸计视觉 token；没有官方证据表明把同一图片改为 `file_id` 会降低视觉 token，亦没有证据证明它必然提高 KV cache 命中，因此不作这些宣传。
- 根因修复应留在既有 `deepagent-models` provider adapter / 附件准备 seam，不把 Files 状态塞进 UI DTO、通用 RunStore 或另造附件库：为 provider request image 建立 typed `Inline | FileId` 投影；用实际字节摘要、规范化 variant、provider endpoint、账户/凭据作用域和有效模型形成稳定 key；上传索引只保存可审计的远端 ID、字节身份与过期信息，不保存 API key。上传、fallback、重试和失效刷新必须共用模型请求的取消 owner，并继续服从 H-69 的 inline/总量预算、H-70 的内容身份及 H-169 的格式验证。
- 验收覆盖：同图同 variant 跨轮/重试/并发只上传一次，不同转换或账户不能串 ID；过期前刷新、provider 删除/无效 ID、Files 超时/限流/配额、上传成功但索引提交失败、取消和进程重开均有界；fallback 必须是一次请求内一致的 all-base64 表示并重新核算完整 body，不混合半新半旧状态。真实 DeepSeek 集成再核对上传、引用、删除与 usage；离线探针只负责 wire 与生命周期状态机，不冒充线上性能结论。

### H-171：Responses 图片角色沿用 Chat 规则，合法 `developer` 图丢失且非法 `assistant` 图静默成功

**级别：中（P2，模型输入内容完整性；当前桌面常规触发概率低）；触发条件：公共 provider seam、未来 Harness/SDK 或导入的原生历史构造一个带图片附件的 `developer`、`system` 或 `assistant` message item。** 普通桌面附件目前由 `Role::User` 产生，因此本项不是现有 Composer 每次发图都会触发；但 `ResponseInputItem` 和 `ResponseRequest::from_response_items()` 是公开模型接口，当前适配层对可表达输入给出了成功却改变语义的结果。

#### 官方契约、生产路径与离线复现

- 2026-10-24 实际打开 DeepSeek 官方 [Responses 指南](https://api-docs.deepseek.com/zh-cn/guides/responses_api/)和 [Responses 参数页](https://api-docs.deepseek.com/zh-cn/api/create-response/)：message 角色支持 `user` / `assistant` / `system` / `developer`，其中 `developer` 视同 `user`；图片只允许出现在 `user` / `developer`，以及 function/custom tool output 中。`system` / `assistant` 图片会返回 `400`。这是 Responses 专属角色矩阵，不能直接套用只允许 user 图片的 Chat/Vision 规则。
- 当前 `response_input_message_to_wire()` 只有 `if role == "user"` 才解析附件；其注释明确以 Vision guide 推导“only permitted on user messages”。所有其他角色都会把 `image_urls` 置空，再返回原 role 和纯字符串 content。该函数没有 `Result`，所以既不能保留合法 developer 图片，也不能对非法 system/assistant 图片产生显式错误。
- `deepagent-models` 公开导出字符串角色的 `ResponseInputItem::InputMessage`、`ResponseRequest::from_response_items()` 与 ModelClient，故此输入在 provider seam 可直接到达。正式 ModelAgent 从当前 `Message` 重建 history 时，带附件消息同样不限制 role；但普通 `Message::with_attachments` 的生产入口当前只用于 user，核心 `Role` 也没有独立 Developer 变体。因此本轮把现有桌面路径列为未触发，不把公共适配层缺陷夸大成高频 UI 故障。
- 新增 opt-in `modalities_audit::audit_responses_image_roles_preserve_or_reject_content`：使用真实 68 字节 PNG data URL，分别构造 developer/assistant `InputMessage`，经公开 ResponseRequest、正式 ModelClient 与 serializer 到离线 transport；fixture 仅记录最终 JSON 并返回正常 Responses terminal。精确命令 `cargo test -p deepagent-models --test modalities_audit audit_responses_image_roles_preserve_or_reject_content --offline -- --ignored --exact --nocapture`：**0 passed / 1 failed（预期）**。developer 实测 `accepted=true, transport_calls=1, image_parts=0, content_was_string=true`；assistant 也是同一结果。也就是说，前者丢失官方支持的模型输入，后者绕过官方错误成为文本成功。
- 默认 `modalities_audit` 对照为 **1 passed / 2 ignored**，正常 Flash user 图片仍含一个 `input_image`，Pro 纯文本仍正常。未调用真实 DeepSeek；assistant 的 `400` 是官方契约，不是本地 fixture 伪造的线上观测。未动态覆盖 system、Chat Completions、持久化导入或真实 Harness 协议入口。

#### 成熟实现对照、后果与收敛方向

- DeepSeek Harness 的 Messages adapter 在图片准备阶段显式检查角色：除了该协议允许的 user/tool 外，含图消息直接返回 `UNSUPPORTED_CONTENT`，不会删除图片后继续采样。它走 Anthropic Messages，角色集合不能复制到 Responses；可借鉴的是 provider 路由拥有自己的角色矩阵，并对不支持内容失败可见。
- Codex 的 app-server v2 把图片定义为 `UserInput::Image` / `LocalImage`，转换后由核心生成 user message；wire 内容则用 typed `ContentItem::InputImage` 保存 image reference 和 detail，而不是在字符串 role 旁挂附件、到 serializer 再决定是否悄悄丢弃。该入口并不证明 Codex 支持 developer 图片；它证明成熟边界限制可构造状态并保持内容类型。
- developer 图片触发时，模型只看到提示文本，看不到用户/上层指令依赖的图表或截图，调用者却收到成功答案；assistant/system 图片触发时，原本应显式失败的非法历史被改写为另一条合法请求，可能掩盖恢复、协议迁移或第三方 SDK 的数据错误。后果是答案语义不可信和审计记录与真实模型输入不一致，不是权限提升，也没有证据表明当前桌面用户路径已受影响。
- 根因修复应让 provider adapter 拥有显式、可测试的 image-role policy：Responses 允许 user/developer，拒绝 system/assistant；Chat Completions、Anthropic Messages 分别按各自官方契约处理。`response_input_message_to_wire()` 应返回 `Result` 或接收已验证的 typed content，不能用空 vector 表示“附件非法/不适用/读取失败”三种不同状态。若通用核心继续没有 Developer 角色，Harness DTO 到 Responses DTO 的映射也必须保留 developer 身份，不把它提前混成 system 或 user。
- 验收覆盖 user/developer/system/assistant、纯图/图文、多图、inline/Files、function/custom output、Chat/Responses 路由、重试/fallback、压缩与恢复；合法角色逐字保留图片顺序，非法角色 transport 调用数为零且运行只落一个可见失败终态。与 H-67 当前轮投递、H-68 读取失败、H-127 工具结果图、H-169 格式及 H-170 Files 生命周期分别回归，不能用任一通过替代角色契约。

### H-172：Responses 数值参数只在设置层校验，公开 ModelClient 仍发送越界值

**级别：低（P3，条件性 provider-contract 缺口）；触发条件：调用者绕过 AppCore 设置入口，直接向公开 `ModelClient` 传入越界 `ResponseRequest`，或在公开 `ModelConfig::with_defaults()` 中配置越界值。** 当前 `ResponsesApiSettings` 对普通/开发者设置已有范围验证，因此没有证据表明常规桌面设置界面能触发；风险面是公共模型 crate、未来 Harness/SDK 接入和内部直接构造配置。此项与 H-166 不重复：H-166 的 `top_p=0.2` 位于 Responses 的顶层取值范围内，只是 Flash thinking 下不符合模型有效下限；H-172 使用超过顶层参数边界的值。

#### 官方契约、生产路径与离线复现

- 2026-10-24 实际打开 DeepSeek 官方 [Responses 参数页](https://api-docs.deepseek.com/zh-cn/api/create-response/)与 [Responses 指南](https://api-docs.deepseek.com/zh-cn/guides/responses_api/)：`temperature` 范围为 0–2；`top_p` 上限为 1（思考模式下 Flash 的模型有效范围另见 H-166）；`top_logprobs` 范围为 0–20。验证仅依据协议允许范围，不推断越界请求在真实服务端一定报错或静默归一化。
- `ResponseRequest::with_temperature()`、`with_top_p()`、`with_top_logprobs()` 只赋值；`ResponseRequest::validate()`（`crates/deepagent-models/src/chat.rs`）目前只校验 `user`。Responses dispatch 先调用 `apply_defaults()`，再调用该 `validate()`，最后序列化并调用 transport；因此 request 字段和 defaults 都处在最终校验缺口内。`ResponseDefaults` 与 `ModelConfig::with_defaults()` 是公共 API，不受 AppCore settings parser 自动约束。
- AppCore 的 `ResponsesApiSettings::validate()` 和开发者 JSON parser 已分别拒绝 temperature 超过 2、top_p 超过 1、top_logprobs 超过 20；本项不是这些路径失效，而是相同协议 invariant 没有在 provider seam 重验。若未来新入口绕过 AppCore，设置层正确性无法保护线级请求。
- 新增 ignored 探针 `responses_parameter_bounds_audit::audit_out_of_range_responses_parameters_are_rejected_before_transport` 经正式 `ModelClient::stream_response()` 验证 request 字段 `2.1 / 1.1 / 21`；运行结果为 **0 passed / 1 failed（预期）**，三次均 `error=false`、`transport_calls=1`，wire 分别携带 `temperature=2.1`、`top_p=1.1`、`top_logprobs=21`。`audit_out_of_range_response_defaults_are_rejected_before_transport` 再分别把同值放入 `ResponseDefaults`，同样 **0 passed / 1 failed（预期）**，三个 defaults 各触发一次 transport。所有请求由离线 recording transport 接收，无公网或真实 provider。
- 正常边界对照 `audit_documented_responses_parameter_boundaries_reach_transport`：`temperature=0/2`、`top_p=0/1`、`top_logprobs=0/20` 均成功进入离线 wire，**1 passed**。这确认探针并非把合法边界一概拒绝，也不等价于线上回答或服务端接受性验证。

#### 成熟实现对照、后果与收敛方向

- DeepSeek Harness 的 `packages/llm/llm-deepseek/src/config.ts` 在 schema 与动态配置 resolver 两层校验 provider 的 `maxTokens` 为正数/安全整数；`packages/core/agent-loop/src/index.ts` 和 `packages/sdk/server/src/server.ts` 又分别在 agent 创建与 SDK `initialize` 边界校验 maxTokens；`serialize.ts` 只从解析后的调用 options/model/connection 优先级生成 `max_tokens`。这是“每个公开入口拒绝非法值、序列化消费已校验配置”的可核代码模式，不是其对本项三种采样参数已有校验的证据。
- 同一 DeepSeek Harness serializer 将显式 `temperature` 直接映射到 Messages wire；本地搜索没有找到它对 `temperature` 或 `topP` 做同等范围验证。Codex 本地参考树也没有为 DeepSeek Responses 的这些字段提供可比较的 provider validator，因此不声称它解决了相同协议问题。此处协议范围仍以 DeepSeek 官方文档为准。
- 越界值触发时，系统会把违反官方参数范围的请求交给供应商；供应商可能拒绝请求或按自身行为处理，但本次离线验证无法确认真实结果。最直接的后果是该 turn 可能变成 provider 错误，或实际采样行为与本地配置预期不一致；没有证据表明它会绕过权限、导致必然扣费或对合法桌面设置产生影响。条件性和低频触发使其定为 P3，而非安全漏洞。
- 收敛方向（尚未实施，本地设计）：让 `ResponseRequest::validate()` 在 `ModelClient` 应用 defaults 后验证同一套 Responses 数值边界，确保 request 与 `ResponseDefaults` 共用唯一 validator；Chat 路由继续通过其协议 capability 明确支持/拒绝，而不是把 Responses 限制误用于不同协议。保留 UI/AppCore 提前反馈，但 provider seam 必须是最终可信边界。加入 request/defaults 两种来源的 transport-zero 负例及合法边界正例，并覆盖普通、cancel-aware 和未来 Harness/SDK 入口。

### H-173：Responses 工具输出图片无法保留，公共消息边界静默降为纯文本

**级别：中（P2，条件性多模态能力丢失）；触发条件：工具结果以 `Message::Tool` 携带图片附件，或未来 Harness/SDK 将图片工具结果投影到现有 `Message`/`ResponseItem` 边界。** 当前默认 `input_runtime` 主要把工具结果 JSON 转成字符串，因此本项没有证明现有桌面工具已经产生可达图片附件；它证明一旦上游已有工具图片，Responses 请求会把模型可见内容改写成文本而不是保留图片，也没有显式失败。

#### 官方契约、生产路径与离线复现

- DeepSeek 官方 [Responses 参数页](https://api-docs.deepseek.com/zh-cn/api/create-response/)规定 `function_call_output` / `custom_tool_call_output` 的 `output` 可以是字符串，也可以是 `input_text` / `input_image` 内容块；[Responses 指南](https://api-docs.deepseek.com/zh-cn/guides/responses_api/)进一步明确工具输出中允许 `input_image`，图片只能出现在 `user` / `developer` 消息或工具输出中，`system` / `assistant` 图片才是 400 错误。这里的验证依据是官方输入类型，不推断真实服务端对本地错误 wire 的具体错误码。
- 当前生产路径是 `Message::Tool` → `response_items_from_messages()` → `ResponseItem::FunctionCallOutput` / `CustomToolCallOutput` → `ResponseRequest::from_response_items()` → `ModelClient::stream_response()` → `response_input_items_to_wire()`。`ResponseItem` 中两个工具输出 variant 的 `output` 都是 `String`；`response_items_from_messages()` 对 `Role::Tool` 只复制 `message.content`，完全忽略 `message.attachments`；serializer 因此只能发字符串。
- 新增 ignored 正式探针 `modalities_audit::audit_responses_tool_output_images_are_preserved_before_transport`：构造真实 assistant `ToolCall` 与 `Message::tool_result(...).with_attachments(image)`，经 `ResponseRequest::new`、正式 `ModelClient` 和 recording transport 捕获最终 JSON。执行 `cargo test -p deepagent-models --test modalities_audit audit_responses_tool_output_images_are_preserved_before_transport --offline -- --ignored --exact --nocapture` 得到 **0 passed / 1 failed（预期）**；捕获 `output_image_parts=0`，wire 为 `function_call_output.output = "screenshot ready"`。没有网络请求、真实图片文件或用户数据。

#### 成熟实现对照、后果与收敛方向

- Codex `codex_protocol::models::FunctionCallOutputBody` / `ContentItem`（参考 `借鉴/codex/codex-rs/code-mode-protocol/src/response.rs` 的同类 `FunctionCallOutputContentItem`）把工具结果内容建模为 typed text/image/audio 项，并在测试中实际构造 `InputImage`；工具输出图片有独立身份，不依赖把附件挂在仅面向 user 的 `Message` 上。这里借鉴的是内容类型和来源身份保持，不声称 DeepSeek 与 Codex 的所有字段完全相同。
- DeepSeek Harness 的 `packages/llm/llm/src/content.ts` 与 `packages/llm/llm/README.md` 明确遍历并处理 tool-role 结果中的图片：视觉路由保留图片引用，纯文本路由生成确定性占位符，文件引用不会被静默送入 adapter。它把“模型可见内容可由会话日志重建”作为不变量；本系统当前字符串化工具结果既丢失图像语义，也没有记录一个可复原的 typed image occurrence。
- 触发后，截图、图表、OCR 结果或 MCP image tool 的视觉证据会被模型当作一段文字（通常是 JSON/base64/描述字符串）；模型可能继续生成看似成功但没有看过图片的答案。对需要视觉判断的工具链，这是功能性错误和上下文/计量失真；它不是权限提升，也没有证据表明当前默认桌面工具必然触发。
- 收敛方向（尚未实施，本地设计）：在现有 `ResponseItem`/session event 边界新增可序列化的 typed tool-output content（至少 text/image，携带 `attachment_id`/`file_id`/受控 data URL 引用及 detail），由 Responses adapter 映射为 `input_image`，由 Chat/Anthropic adapter 按各自矩阵显式转换或拒绝。图片读取失败、不可支持格式和超预算必须保留结构化错误，不能退化为字符串；压缩、恢复、Files offload、usage 与日志要能重建同一 occurrence。回归需覆盖 function/custom、inline/file_id、正常文本对照、历史重放和无图片工具路径。

### H-174：公开 `replace_messages` 绕过 Responses 工具调用配对修复

**级别：中（P2，条件性 Harness/SDK 会话续接阻断）；触发条件：外部调用者先构造 `ResponseRequest`，再通过公开 `replace_messages()` 写入含未完成 `function_call` 的消息投影。** 当前 `ModelAgent::ResponseHistory::request()` 主要走 `from_response_items()`，因此本项不是默认桌面路径已必现的结论；风险在公开模型客户端、SDK 或未来 Harness 适配器复用该 mutator 时暴露。

#### 官方契约、生产路径与正式复现

- DeepSeek 官方 [Responses API 参数文档](https://api-docs.deepseek.com/zh-cn/api/create-response/)规定 `function_call` / `function_call_output` 的 `call_id` 必须非空且唯一，并且每个 `function_call` 都必须有对应的 `function_call_output`；Responses 是无状态接口，客户端必须在下一次 `input` 中回传可配对的历史。未配对的调用不是可安全忽略的兼容输入。
- `ResponseRequest::from_response_items()` 会调用 `repair_tool_call_pairs()`，对崩溃、取消或旧历史中的未完成调用补一个明确的失败 output。可是公开 `ResponseRequest::replace_messages()` 直接执行 `response_items_from_messages()` 后赋值 `self.input = input`，没有经过同一 repair，也没有在 `ModelClient::stream_response()` 的 Responses 分支再次校验配对。
- 新增正式 ignored 探针 `crates/deepagent-models/tests/response_repair_audit.rs::audit_replace_messages_repairs_dangling_responses_calls_before_transport`。它使用真实 `ResponseRequest`、公开 `replace_messages()`、正式 `ModelClient` 和 recording `HttpTransport`；消息包含一个没有结果的 `read_file` 调用，fixture 只返回离线 `response.completed`，不调用公网或执行工具。
- 执行：`cargo test -p deepagent-models --test response_repair_audit audit_replace_messages_repairs_dangling_responses_calls_before_transport --offline -- --ignored --exact --nocapture`。实际输出：`has_call=true has_output=false`，最终 wire `input` 只有 `function_call(call_id=audit-dangling-call)`，没有同 ID 的 `function_call_output`；断言以 `0 passed / 1 failed（预期）` 结束。失败发生在 transport 已被调用之后，受控 fixture 的成功响应不代表真实 DeepSeek 接受该请求。

#### 成熟实现对照

- **Codex** 将 Responses 历史保持为 typed `ResponseItem`，并在 `借鉴/codex/codex-rs/protocol/src/models.rs` 中把 `function_call_output` 建模为独立 payload；其 `core/tests/common/responses.rs::validate_request_body_invariants` 对请求中的原生工具调用和结果不变量做集中校验。调用方不能通过重建一个旧的字符串消息视图绕过该边界。
- **DeepSeek Harness** 的 `借鉴/deepseek-harness/packages/llm/llm-deepseek/src/translate.ts` 在工具块解析时要求非空 tool identity，session/agent-loop 继续沿用带身份的 tool-call block；它没有把“消息投影替换”当成可以跳过 provider 输入不变量的捷径。本轮只核对源码，没有运行参考项目测试，也不把其 Messages 协议字段直接套到 Responses wire。

#### 后果、修复方向与限制

- 触发后，真正的 DeepSeek 请求可能因缺少配对结果返回 400；如果上层把它作为可重试 provider 错误，重复失败会发生在每次续聊/重试。更严重的是，工具可能已经在宿主执行过，但模型历史只留下调用没有结果，形成“副作用已发生、下一轮无法继续”的状态。当前探针没有执行真实工具，也没有向公网发送非法请求，不能声称已观测真实 HTTP 错误或重复副作用。
- 修复应收敛到一个 provider-boundary invariant：`replace_messages()` 复用 `from_response_items()` 的 repair，或改为返回 `Result` 并在 `ModelClient` 发送前统一校验；不能只在一个公开构造器修补。校验应保留 call/item 位置和 native kind，不能静默删除调用、改成 `unknown_tool` 或仅依赖 provider 400。若运行时需要保留未完成调用，应写入既有持久化/恢复边界的结构化失败 output，并让所有重试、压缩、恢复和 SDK 路径复用同一实现。
- 回归需覆盖正常成对调用、未完成调用、孤立 output、空/重复 `call_id`、function/custom 混用、`replace_messages` 与 `from_response_items` 两入口、Chat/Responses 分路及 transport 调用数；H-148/H-149 关注空/重复身份，本项关注**公开消息替换入口绕过 repair**，不与其重复。
- 本轮只新增审计测试和文档，未修改生产修复；默认桌面 `ResponseHistory::request()` 的正常路径仍由 `from_response_items()` 负责 repair，触发面限定为公开 mutator/未来 Harness 或 SDK 复用。

### H-175：工作流 `http-request` 节点绕过统一网络权限与审批边界

**级别：高（P1，工作流网络能力越权）；触发条件：能够提交专业工作流定义的调用者放入 `http-request` 节点，并将 `httpUrl` 指向宿主可达的 loopback、私网或其他未授权网络目标。** 本项与 H-165 的供应商图片 URL 第二跳不同：H-165 是 provider 成功响应驱动的 artifact 下载，本项是工作流定义本身直接触发裸 HTTP 请求；两条链路都必须独立收敛到统一网络策略。

#### 生产路径与正式本机复现

- `ChatService::run_workflow()` 通过既有 `RunAssembler`、`AgentKernel`、取消树、事件 sink 和 `RunStore` 执行 `WorkflowAgent`，但 `run_workflow_inner()` 只构造空的 `ToolRegistry` 与 `WorkflowToolExecutor`。`WorkflowAgent::execute_node_inline()` 对 `http-request` 直接进入 `execute_http_request()`，在那里按节点字段新建 `reqwest::Client`，设置普通 timeout 后调用 `send().await`，读取响应并把 body/status/headers 返回为节点输出。
- 这条路径没有调用普通聊天使用的 `ToolRegistry`/`PermissionSet`，没有 `ChannelApprovalGate`、`NetworkPolicyController`、`SandboxBackend` 或宿主网络代理；`on_approval` 回调也不会被触发。节点 schema 只校验字段形状，不能把“URL 可写入配置”误认为“网络访问已授权”。
- 新增正式 ignored 探针 `crates/deepagent-app-core/tests/workflow_audit.rs::audit_workflow_http_node_does_not_bypass_network_approval`。它通过正式 `ChatService::run_workflow()` 提交 `start → http-request → end`，在临时 `127.0.0.1` 监听一个只返回 `CANARY` 的服务，记录实际 TCP 请求与审批回调次数；没有公网、真实内网、凭据或用户服务。
- 精确命令：`cargo test -p deepagent-app-core --test workflow_audit audit_workflow_http_node_does_not_bypass_network_approval --offline -- --ignored --exact --nocapture`。
- 实际输出：`api_ok=true error=None requests=1 approvals=0 run_terminal=Some("succeeded")`，随后断言 `requests == 0` 失败，结果为 **0 passed / 1 failed（预期）**。这证明正式工作流已成功向 loopback 发出请求、未进入审批回调，并且运行仍记为 `succeeded`；不是测试代码直接调用 `reqwest`。本探针只验证访问与授权缺口，不把虚构响应扩展解释为真实敏感数据读取。

#### 官方契约与成熟实现对照

- DeepSeek 官方 [Tool Calls 指南](https://api-docs.deepseek.com/zh-cn/guides/tool_calls/)明确说明：模型返回的是工具调用，**具体 function 由用户提供，模型本身不执行函数**；客户端应执行函数，再把 `tool` 结果回传模型。官方 [Responses 参数文档](https://api-docs.deepseek.com/zh-cn/api/create-response/)也把 `input`、工具 item 和输出 item 定义为模型协议数据，并不授予任意宿主 URL 的网络权限。因此把 workflow JSON 的 `httpUrl` 直接升级为宿主出站能力，不是 DeepSeek Responses/Tool Calls 的必然语义，而是本系统新增的执行边界。
- **DeepSeek Harness** 的 `借鉴/deepseek-harness/packages/web/web-fetch-http/src/provider.ts` 将 Web 抓取封装为显式 provider；`requestOnce()` 先走 `validateFetchUrl()`、解析并校验所有地址，再用 pinned dispatcher 发起请求，redirect 每一跳重新校验且只跟随同源地址。其测试 `packages/web/web-fetch-http/tests/fetch-http.spec.ts` 明确覆盖 loopback 在连接前被拒绝、跨源 redirect 被拒绝、超限与取消。参考项目 README 也将该能力描述为“仅限公网的 HTTP fetch provider”，不是在任意节点里散落 `fetch`。
- **Codex** 将应用网络策略挂到 HTTP client factory 和 sandbox/approval profile；参考 `借鉴/codex/codex-rs/app-server/src/config_manager.rs` 的 `application_network_policy`、`借鉴/codex/codex-rs/app-server/src/in_process_bootstrap.rs` 的 policy 注入，以及 app-server protocol 中的 network approval amendment。可借鉴的架构是不让单个工作流节点自行新建无策略 client，而是让所有出站请求继承一次运行的有效网络策略；本轮没有运行 Codex 参考测试，也不声称其策略与本系统权限模型字段一一相同。

#### 影响、后果与修复方向

- 触发后，工作流提交者可以让宿主探测或读取本机服务、RFC1918/链路本地/云 metadata 地址，或访问当前网络策略本应阻断的公网目标；请求结果还会进入节点输出、事件与后续模型上下文。当前动态样本只确认 loopback GET 和成功终态，没有读取真实秘密、没有外传、没有模拟 DNS rebinding 或 redirect，因此不把这些潜在后果写成已发生事实。
- 该缺口还破坏 Harness 的可审计性：UI/CLI/SDK 看到的是一个普通 `WorkflowNode Completed` 和 `run succeeded`，审批中心没有对应记录，网络日志也不带统一 run/permission scope。即使未来只允许“受信 workflow”，当前公开 `WorkflowRequest` 的 admission 并没有把网络 profile 与定义绑定，恢复、重放和远程执行入口会继续继承不明确的权限语义。
- 根因修复应复用现有工具/权限边界，不能在 `execute_http_request()` 增加几个 hostname 特判。建议在 `run_workflow_inner()` 解析一次不可变的 `RunNetworkContext`（profile、allowlist、approval owner、取消 signal），由统一 `NetworkFetcher`/HTTP tool adapter 执行 URL 校验、DNS 地址 pinning、redirect 重校验、超时/大小/取消和审计；`http-request` 节点只提交 typed request，不持有裸 reqwest client。默认 profile 应拒绝 loopback、私网、link-local、metadata 和未批准 redirect；显式本地开发例外必须是可审计配置，而不是默认放行。
- 验收应覆盖：无网络 profile、已批准 host、loopback/IPv6/RFC1918/link-local/metadata、DNS 多地址与 rebinding、redirect 到私网、请求/响应超限、取消与重试、工作流恢复/重放、CLI/SDK/Tauri 三种入口。断言未授权目标请求数为零、无 `Completed`/成功终态、审批事件与实际请求一一对应；正常公网例外必须使用同一 fetcher。H-165 的 provider artifact fetch、H-164 的 Canvas 取消和 H-38 的工具失败传播需保持独立回归。

本轮只新增审计探针和文档，未修改生产实现；下一步应在统一网络 fetcher 设计明确后，再决定是否将工作流 HTTP 节点改造成现有 ToolRegistry 的 typed tool，避免出现第三套网络执行链。

### H-176：工作流 `knowledgeBaseId` 是死字段，知识检索节点无条件混合 project/global vault

**级别：中（P2，工作流上下文越权与配置失效）；触发条件：工作流定义使用 `knowledge-retrieval` 节点并声明某个 `knowledgeBaseId`，同时当前运行的 `KnowledgeService` 中存在 project 与 global 两个 vault 的相似命中。** 如果 global vault 被当作共享上下文，这仍然是“选择 project 但返回 global”的明确契约违约；如果调用者把该字段当作隔离边界，混入内容还可能造成跨作用域知识泄露。当前动态样本只证明两类文档同时进入节点输出，不把文档内容扩展解释为真实机密。

#### 生产调用链与正式复现

- 桌面节点 schema `apps/desktop/src/canvas/workflow/utils/nodeSchemas.ts` 暴露 `knowledgeBaseId`、`knowledgeTopK`、`queryVariable` 与 `scoreThreshold`。该字段会随工作流 JSON 进入后端，但没有对应的枚举、owner 或 scope 约束。
- `WorkflowAgent::execute_knowledge_retrieval()` 只读取 `queryVariable`、`knowledgeTopK` 和 `scoreThreshold`，随后调用 `KnowledgeRetriever::search(query, limit)`；trait 没有 base/scope 参数。
- `RunAssembler::run_workflow_inner()` 将共享 `KnowledgeService` 包装成 `WorkflowKnowledgeRetriever`。其 `search()` 固定调用 `self.service.search(query, None, limit)`；`KnowledgeService::search()` 再把 project/global vault 载入的统一索引一起查询。`KnowledgeBase::search()` 只支持 `kind` 过滤，不支持 `Scope` 过滤。因此 `knowledgeBaseId` 在生产调用链上没有任何消费方。
- 新增正式 ignored 探针 `crates/deepagent-app-core/tests/workflow_audit.rs::audit_workflow_knowledge_base_id_is_honored`。探针在临时 project/global vault 各写入一条包含同一查询词的文档，通过正式 `ChatService::run_workflow()` 执行 `start → knowledge-retrieval → end`，设置 `knowledgeBaseId=project`，捕获真实 `WorkflowNode::Completed` 输出。
- 精确命令：`cargo test -p deepagent-app-core --test workflow_audit audit_workflow_knowledge_base_id_is_honored --offline -- --ignored --exact --nocapture`。
- 实际输出：`api_ok=true error=None knowledge_base_id=project titles=["Global-only runbook", "Project-only runbook"]`；随后断言 `knowledgeBaseId=project` 不应返回 global 文档，测试以 **0 passed / 1 failed（预期）** 结束。运行成功且节点完成，说明这是静默的作用域失效，不是后端不可用。

#### DeepSeek 官方契约边界与成熟 Agent 对照

- DeepSeek 官方 [Responses API 参数文档](https://api-docs.deepseek.com/zh-cn/api/create-response/)说明 Responses 是无状态接口，客户端必须在每次 `input` 中回传完整历史；[Responses 指南](https://api-docs.deepseek.com/zh-cn/guides/responses_api/)描述的是输入 item、工具调用和模型输出，不定义本地 knowledge-base、vault 或项目 scope。官方 [Tool Calls 指南](https://api-docs.deepseek.com/zh-cn/guides/tool_calls/)明确具体 function 由用户提供并由客户端执行。因此 H-176 是本系统 workflow contract 的缺口，不能把 DeepSeek API 的字段语义臆造为本地知识库权限。
- **DeepSeek Harness** 的 `借鉴/deepseek-harness/packages/mcp/mcp-resources` 为 server/resource 解析保留调用 agent 的 scope；工具在调用者 scope 中解析 provider，缺失或不可见 server 在 dispatch 前失败，不能把所有 server 合并进一个全局列表。该设计可借鉴的是“声明的 scope 必须贯穿发现、解析和执行”，不是把 MCP 字段直接复制为本系统的 knowledge schema。
- **Codex** 的 app-server/workspace 与 permission profile 将运行身份、工作区和能力策略作为一次运行的显式上下文；memory 相关入口按 thread/workspace 选择，而不是让一个共享可变服务在节点执行时自行决定可见集合。这里的可比点是执行上下文绑定和统一的 policy seam，本轮没有把 Codex 的字段当作本系统兼容协议，也没有运行其参考测试。
- **Claude Code** 的记忆目录按项目根解析，项目记忆与用户级记忆的来源边界在装配阶段明确。它解决的是“当前项目身份决定哪些记忆可见”，不是通过提示词让模型自行过滤。本系统当前只在 `KnowledgeService::activate_project()` 处理项目身份，workflow retrieval 没有等价的显式 scope 入口。

#### 影响、后果与根因修复方向

- 触发后，画布用户选择的 knowledge base 不决定检索集合；模型上下文可能同时收到未选择的 global/project 文档，导致回答引用错误项目的 runbook、把不同环境的配置混在一起，或在 global 预期共享但 workflow 期望隔离时暴露不应进入该节点的内容。当前探针只观测到标题和 excerpt 混合、run 成功，不声称已读取真实敏感信息。
- 这会破坏 Harness 的可审计性：UI/CLI/SDK 看到的是配置中明确写入的 `knowledgeBaseId`，但 `WorkflowNode::Completed` 没有返回实际 scope/base 身份，重放、恢复和跨宿主运行也无法证明同一知识集合被使用。H-51 不覆盖此项：H-51 的根因是共享 `KnowledgeService` 激活后运行身份漂移；H-176 的根因是节点公开配置到 backend retriever trait 完全没有 scope 参数，即使服务身份稳定也会混合两个 vault。
- 根因修复应在现有 runtime/app-core seam 收敛：将 `knowledgeBaseId` 解析为一次运行不可变的 `KnowledgeScope`（至少 project、global，或显式 base ID），扩展 `KnowledgeRetriever::search` 为带 scope/base 的 typed 请求，由 `KnowledgeService`/`KnowledgeBase` 在候选检索前过滤 scope；节点输出和审计事件携带实际 scope、base revision 与命中来源。未知/无权 base 必须在 dispatch 前失败，不能静默回退到混合索引。
- 恢复、重放、被取消的重试以及 CLI/SDK/Tauri 入口必须复用同一 scope snapshot；passive memory、`knowledge_search` 工具和 workflow 节点是否共享集合要显式定义，不能因为共用 `KnowledgeService` 就隐式获得全量可见性。回归需覆盖 project-only、global-only、explicit mixed、未知 base、空/旧字段迁移、跨项目运行、重启和命中排序，断言节点输出、事件与实际磁盘来源一致。

本轮 H-176 只新增正式审计探针和文档，未修改生产检索行为；在 scope DTO、权限归属和兼容迁移规则明确前，不应把忽略 `knowledgeBaseId` 的现状标记为兼容行为。

### H-177：未知 `target_node_id` 被当作空工作流成功，重跑/恢复目标没有 admission 校验

**级别：中（P2，工作流控制完整性与错误终态）；触发条件：调用者通过 `WorkflowRequest.target_node_id` 指定不存在、已删除或版本不匹配的节点。** 该字段是公开 JSON 请求的一部分，桌面序列化器也会发送它；触发后不会执行目标节点及其下游，但运行仍报告 `succeeded`。它与 H-39 的区别是：H-39 讨论已存在条件图的分支可达性，H-177 讨论目标身份根本不在当前图中的 admission 失败。

#### 生产调用链与正式复现

- `apps/desktop/src/canvas/workflow/utils/workflowExecutor.ts` 将 `targetNodeId` 直接序列化为 `WorkflowRequest.target_node_id`。`ChatService::run_workflow()` → `RunAssembler::run_workflow_inner()` 将其原样传入 `WorkflowAgent::new()`。
- `deepagent-runtime::workflow::compile()` 校验版本、节点/边 ID、拓扑与输出引用，但不接收或验证 `target_node_id`。`WorkflowAgent::think()` 仅用 `target_node_id` 做运行时过滤：节点不是目标且不是 `start` 时，只有在其 `ancestors` 包含目标 ID 时才执行；未知目标不会命中任何祖先集合，因而 `end` 也被标记 `Skipped`。遍历结束后，`think()` 返回 `AgentDecision::Complete("workflow completed")`，AppCore 继续写入成功终态。
- 新增正式 ignored 探针 `crates/deepagent-app-core/tests/workflow_audit.rs::audit_workflow_unknown_target_node_is_rejected`。它通过正式 `ChatService::run_workflow()` 执行两节点 `start → end` 图，提交 `target_node_id=missing-node`，收集 `WorkflowNode` 事件并从真实 `RunStore` 读取终态。
- 精确命令：`cargo test -p deepagent-app-core --test workflow_audit audit_workflow_unknown_target_node_is_rejected --offline -- --ignored --exact --nocapture`。
- 实际输出：`api_ok=true error=None completed_nodes=["start"] run_terminal=Some("succeeded")`；随后断言未知目标必须拒绝，测试以 **0 passed / 1 failed（预期）** 结束。没有模型请求、工具副作用或公网访问；失败来自正式工作流状态机本身。

#### DeepSeek 官方边界与成熟 Agent 对照

- DeepSeek 官方 [Responses API 参数文档](https://api-docs.deepseek.com/zh-cn/api/create-response/)只定义 Responses 的输入 item、模型响应和工具调用配对；[Responses 指南](https://api-docs.deepseek.com/zh-cn/guides/responses_api/)也没有本地 DAG、节点重跑或 `target_node_id` 语义。因此这里不能把 DeepSeek 的 provider 成功响应当作本地 workflow target 已校验的证据；这是宿主 Harness 的控制面契约。
- **Codex** 在 `借鉴/codex/codex-rs/app-server/src/request_processors/turn_processor.rs` 的 `load_thread()` 中先解析 ID，再从 `thread_manager` 查找；无效格式或不存在的 thread 直接返回 `invalid_request` / `thread not found`，不会创建一个空 turn。工作流目标虽然不是同一字段，但“控制面引用必须在 admission 阶段解析成功”是可迁移的架构原则。
- **Claude Code** 的 `--resume` 入口先验证 session ID 形状并检查 transcript 是否存在；恢复失败会返回明确错误，而不是启动一个没有历史的成功会话。参考实现的具体错误文本不作为本系统协议，只作为失败语义对照。
- **DeepSeek Harness** 的 agent loop 没有把任意字符串当作可执行节点；其 scope、provider 和 server 句柄在装配时由 typed identity 绑定，缺失 provider 在调用前失败。它提供的可比点是“运行上下文引用先解析、再执行”，不是为本系统引入新的 DAG 引擎。

#### 影响、后果与根因修复方向

- 触发后，用户界面、CLI 或 SDK 可能看到一次 `succeeded`，但目标节点、下游输出和 end 输出均不存在。对可重跑的媒体/工具工作流，这会把“节点已删除、图版本不匹配、恢复游标过期”误报为成功；自动化调度器可能据此跳过补偿或继续读取空结果。
- 该行为也破坏事件审计：事件流只有 `start Completed` 与后继 `Skipped`，没有一个明确的 `target_not_found` 失败；RunStore 只保留成功终态，重放方无法区分真正完成和空执行。当前样本只证明错误终态与节点输出缺失，没有声称已经造成外部副作用。
- 根因修复应在 `compile`/`run_workflow_inner` 的 admission 边界校验目标 ID：目标为空保持全图执行；目标存在才生成不可变的 `TargetPlan`（目标、祖先路径、允许的下游）；未知、重复/版本不匹配目标返回结构化 `InvalidTargetNode`，不启动 `AgentKernel`。运行时仍需以同一计划判断条件边，避免重新按字符串过滤。
- 验收应覆盖：未知目标、目标为 start/end、目标在条件分支两侧、目标节点已从持久化图删除、恢复时 revision 不匹配、取消/失败/重试和 CLI/SDK/Tauri 三入口。无效目标必须没有成功终态、没有模型/工具/媒体请求，并在节点事件与 RunStore 中保留可诊断错误；有效目标的祖先执行和下游语义要保持现有工作流回归。

本轮 H-177 只新增正式审计探针和文档，未修改生产工作流执行逻辑。

### H-178：专业画布 `code` 节点标记为 enabled，但脚本从未执行且错误被伪造成成功

**级别：高（P1，工作流语义与结果完整性）；触发条件：使用桌面专业画布的 `code` 节点，提交任意 `codeScript`（包括明确无效脚本）和 `codeInputVariables`。** 节点注册表和编辑器把它展示为可用的 JavaScript/Python 执行能力，但运行时没有对应解释器或受控执行边界；触发后节点仍为 `Completed`，输出只是声明变量的默认值或空对象。

#### 生产调用链与正式复现

- `apps/desktop/src/canvas/workflow/utils/nodeRegistry.ts` 将 `code` 注册为 `availability: "enabled"`、`executor: "code"`，并公开 `codeLanguage`、`codeScript`、`codeInputVariables` 与 `codeOutputVariables`。`nodeSchemas.ts` 同样把脚本标成 JavaScript/Python 编辑器字段，因此这是面向用户的已承诺能力，不是隐藏实验字段。
- `deepagent-runtime::workflow::graph::compile()` 对 `code` 节点的绑定契约使用 `codeInputVariables`，输出契约使用 `codeOutputVariables`；但 `WorkflowAgent::execute_code()` 读取的是 `code` 和 `codeVariables`，把这两个前端字段完全忽略。该函数只把已读取的变量复制到结果，并为声明输出填充空字符串、0、空数组或空对象，没有任何 JavaScript/Python 执行调用、脚本语法校验或失败路径。
- 新增正式 ignored 探针 `crates/deepagent-app-core/tests/workflow_audit.rs::audit_workflow_code_node_executes_script_or_fails_loudly`。它通过正式 `ChatService::run_workflow()` 执行 `start → code → end`，提交 `codeLanguage=javascript`、`codeScript="this is intentionally invalid JavaScript; throw new Error('CODE_CANARY')"`、`codeInputVariables=[{name: input, value: SOURCE}]` 和 `codeOutputVariables=[{name: result, type: string}]`，只捕获节点事件和 `RunStore`，不执行系统命令、网络或文件写入。
- 精确命令：`cargo test -p deepagent-app-core --test workflow_audit audit_workflow_code_node_executes_script_or_fails_loudly --offline -- --ignored --exact --nocapture`。
- 实际输出：`api_ok=true code_status=Completed code_outputs=Some(Object {}) code_error=None run_terminal=Some("succeeded")`；断言“无效脚本必须失败或不能成功完成”以 **0 passed / 1 failed（预期）** 结束。换言之，脚本中的 `CODE_CANARY` 没有被解析，也没有进入错误消息；这是正式工作流的静默伪成功，不是测试夹具直接调用内部函数。

#### DeepSeek 官方契约边界与成熟 Agent 对照

- DeepSeek 官方 [Responses API 参数文档](https://api-docs.deepseek.com/zh-cn/api/create-response/)和 [Tool Calls 指南](https://api-docs.deepseek.com/zh-cn/guides/tool_calls/)定义的是模型请求、工具 schema、function call 与客户端回传结果；它们不提供本地 JavaScript/Python sandbox，也不会替宿主执行 `codeScript`。因此本问题属于 Harness 的能力声明与执行器契约，不能把一次 DeepSeek 请求成功当成代码节点成功。
- **Codex** 将命令执行作为显式工具/沙箱能力，由工具注册、权限 profile、审批和执行结果共同定义；没有一个只凭 UI 字段就把任意脚本当作已执行的伪 executor。可迁移的做法是：能力目录必须与实际注册 executor 一致，执行前通过统一权限边界，执行后持久化真实 stdout/stderr/退出码，失败不可映射为正常完成。
- **Claude Code** 的可执行扩展通过受控工具和 hook 命令注册，命令返回码、输出和 hook 决策是生命周期输入；未安装或不可执行的命令会暴露失败，不会用配置回显替代执行结果。其隔离重点是“声明、授权、实际进程”三者同一条链，而不是在 schema 中承诺语言执行器后静默跳过。
- **DeepSeek Harness** 的工具/插件边界把 provider、tool schema 和调用者 scope 在装配阶段绑定；未注册的能力在 dispatch 前失败。它可以借鉴“typed capability + fail closed”，但不能直接推断出本系统应新增一个任意代码解释器。
- `借鉴/better-harness` 中的 JavaScript/Python 代码主要用于静态分析、测试映射和报告生成，不构成本系统这种工作流节点运行时；因此不能把静态解析器依赖误认为已有安全执行环境。

#### 影响、后果与根因修复方向

- 触发后，用户会看到节点成功、工作流成功和空/默认输出。下游节点可能继续使用空值，把错误结果写入 artifact、知识库或外部工具；自动化调度器会跳过重试和告警。更严重的是，用户可能据此误以为脚本已经在沙箱中执行，实际却没有任何计算或审计记录。
- 该缺口还破坏跨入口一致性：桌面 schema、CLI/SDK 发送的同一工作流声称有脚本能力，但 `WorkflowAgent` 的事件没有 language、script digest、执行器版本、stdout/stderr 或 exit code；恢复/重放无法判断是脚本成功、未执行还是默认值填充。它与 H-162 的“AI 节点标 enabled 但没有 executor”同属能力注册问题，但 H-178 更危险之处是节点明确发布 `Completed` 和成功终态，并把错误结果继续交给下游。
- 根因修复应先做能力边界决策：如果当前不准备提供受控代码运行时，就把 registry/schema 改为 `unavailable`，并在 compile/admission 返回结构化 `UnsupportedOperation`，不能保留“代码执行”假象。如果要支持，则必须在现有 `ToolRegistry`/权限/`SandboxBackend` 边界下实现语言级执行器：固定 runtime 版本、workspace/网络/时间/资源限制、输入输出 schema、超时/取消、stdout/stderr 截断与审计；禁止把脚本直接交给宿主 shell。
- 无论选择哪条路径，`codeInputVariables`、`codeScript`、`codeLanguage` 和 `codeOutputVariables` 必须在一个版本化 DTO 中贯通；成功只能来自真实 executor 的结构化结果，语法错误、运行时异常、超时、取消和资源限制都必须产生失败节点与非成功 run 终态。回归需覆盖空脚本、语法错误、输入绑定、类型转换、输出声明、超时、取消、拒绝网络/文件、跨入口序列化以及恢复/重放。

本轮 H-178 只新增正式审计探针和文档，未修改生产代码或启用任意脚本执行器。

### H-179：专业画布 `document-extractor` 标记为 enabled，但正式运行没有 executor

**级别：中（P2，公开能力与工作流可用性）；触发条件：在专业画布选择“文档提取”节点并提交工作流。** 前端提供文件变量、数组模式和 `text` 输出，用户可以把它接入图；后端图编译接受该 kind，但运行时没有文档解析器，节点进入 `Failed`，工作流不能完成。

#### 生产调用链与正式复现

- `apps/desktop/src/canvas/workflow/utils/nodeRegistry.ts` 将 `document-extractor` 注册为 `availability: "enabled"`、`executor: "retrieval"`，并声明 `text` 输出；`nodeSchemas.ts` 提供 `docExtractorFileVariable` 和 `docExtractorIsArray` 配置。这是用户可见、可配置的已启用节点，不是 `reserved` 占位节点。
- `deepagent-runtime::workflow::graph::output_contract()` 为该 kind 接受 `text` 输出，但 `WorkflowAgent::execute_node_inline()` 没有 `document-extractor` 分支；它落入统一的 `UnsupportedNode` 错误。现有 runtime 单测只验证内部错误字符串，未覆盖 AppCore 的真实 run 终态，故本轮补充正式链探针。
- 新增正式 ignored 探针 `crates/deepagent-app-core/tests/workflow_audit.rs::audit_workflow_enabled_document_extractor_has_executor`。它通过 `ChatService::run_workflow()` 执行 `start → document-extractor → end`，提交完整的文件变量配置，捕获 `WorkflowNode` 终态并读取 `RunStore`。
- 精确命令：`cargo test -p deepagent-app-core --test workflow_audit audit_workflow_enabled_document_extractor_has_executor --offline -- --ignored --exact --nocapture`。
- 实际输出：`api_ok=false extractor_status=Failed extractor_outputs=None extractor_error=Some("invalid input: UnsupportedNode: kernel has no executor for node kind \`document-extractor\`") run_terminal=Some("failed")`；探针以 **0 passed / 1 failed（预期）** 结束。没有模型请求、文件解析副作用或公网访问；失败由正式运行链报告。

#### DeepSeek 官方契约边界与成熟 Agent 对照

- DeepSeek 官方 [Responses API 指南](https://api-docs.deepseek.com/zh-cn/guides/responses_api/)和 [参数文档](https://api-docs.deepseek.com/zh-cn/api/create-response/)不定义本地文档解析器或工作流节点。即使后续用 DeepSeek 做 OCR/摘要，也必须先有明确的文件读取、格式识别、大小限制和输出 DTO；一次模型请求成功不能证明 `document-extractor` 已实现。
- **Codex** 的文件读取/命令执行能力以已注册工具、sandbox policy 和权限 profile 为边界；不可用能力在 tool catalog 或 dispatch 阶段失败，不会同时出现在“enabled”目录中。对本系统可迁移的原则是能力目录必须由后端 executable capability 投影生成，或至少在启动时做 registry/executor 一致性校验。
- **Claude Code** 把文档/代码处理拆成明确的工具或 hook 命令，命令不存在、退出失败或输入不支持都会进入错误路径；它没有把一个尚未接入的解析器作为正常可用节点交给用户。
- **DeepSeek Harness** 的 provider/tool 绑定也是 typed capability：没有可用 provider 或 tool handler 时在 dispatch 前失败。这里应借鉴 fail-closed 和 capability discovery，而不是把 `executor: "retrieval"` 当作已有实现。

#### 影响、后果与根因修复方向

- 用户会在图设计阶段认为可以上传文件并提取文本，实际运行才收到通用 `UnsupportedNode`；自动化工作流因此失败，后续 LLM、知识入库或回答节点不会运行。由于错误发生在执行中而不是保存/编译时，问题会在生产任务触发后才暴露，浪费排队、审批和模型准备成本。
- 公开 schema 与运行时 output contract 还会误导 CLI/SDK：同一 JSON 在一个入口能通过 compile，在另一个入口才失败；事件只有 generic error，没有声明“节点尚未启用”或替代能力，恢复/重试不会改变结果。
- 根因修复有两条合法路径：若尚未准备解析器，把 registry/schema 改为 `reserved` 并在保存或 compile 阶段返回结构化 `UnsupportedOperation`；若确实要支持，新增受控 `DocumentExtractor` trait/adapter，明确 MIME allowlist、最大字节数、页数/CPU/超时/取消、临时文件边界和解析器版本，再由 `WorkflowAgent` 统一执行并输出 `text`、来源和诊断元数据。不要只把 kind 加入 match 后回显 config。
- 验收需覆盖 PDF、纯文本、图片/OCR、数组输入、空/损坏文件、超限、取消、解析器异常、跨宿主重放和权限拒绝；能力目录、compile、事件和终态必须一致。若由 DeepSeek 参与提取，还需单独验证 Responses 输入的文件/图片格式、usage、缓存和失败重试，不能把 provider adapter 的成功当作本地 extractor 成功。

本轮 H-179 只新增正式审计探针和文档，未修改生产代码或宣称文档提取已实现。

### H-180：专业画布 `human-input` 节点不进入统一人工输入/审批生命周期

**级别：高（P1，交互式工作流可恢复性）；触发条件：在专业画布使用标记为 `enabled` 的“人工审批”节点，等待用户填写表单后继续下游。** 前端公开了字段、提示和超时配置，但运行时没有等待、通知、提交、取消或恢复协议；节点直接失败为未实现。

#### 生产调用链与正式复现

- `apps/desktop/src/canvas/workflow/utils/nodeRegistry.ts` 将 `human-input` 注册为 `availability: "enabled"`、`executor: "human"`，输出 `action`；`nodeSchemas.ts` 提供 `humanInputFields`、`humanInputPrompt` 和 `timeoutSeconds`。UI 组件也按这些字段渲染人工审批节点。
- `deepagent-runtime::workflow::graph::source_handles()` 为人工输入生成 `action-0..N` 与 `timeout` 分支，`output_contract()` 声明 `action`，所以工作流图的控制流契约向调用者表达了“等待用户决定后继续”。但 `WorkflowAgent::execute_node_inline()` 没有 `human-input` 分支，实际只返回 `UnsupportedNode`。
- 这条缺口尤其明显，因为系统已有 `AgentDecision::NeedsApproval`、`InputLeaseRegistry`、`input_runtime` 和统一 approval bridge；工作流节点没有复用这些边界，也没有产生可被桌面/CLI/SDK 消费的等待事件。
- 新增正式 ignored 探针 `crates/deepagent-app-core/tests/workflow_audit.rs::audit_workflow_human_input_uses_shared_waiting_lifecycle`。它用合法的 `action-0` 后继边执行 `start → human-input → end`，提交一个必填 boolean 字段和 30 秒超时，通过正式 `ChatService::run_workflow()` 捕获节点事件并读取 `RunStore`。
- 精确命令：`cargo test -p deepagent-app-core --test workflow_audit audit_workflow_human_input_uses_shared_waiting_lifecycle --offline -- --ignored --exact --nocapture`。
- 实际输出：`api_ok=false api_error=Some("invalid input: UnsupportedNode: kernel has no executor for node kind \`human-input\`") review_events=[(Running, None, None), (Failed, None, Some("invalid input: UnsupportedNode: kernel has no executor for node kind \`human-input\`"))] run_terminal=Some("failed")`；探针以 **0 passed / 1 failed（预期）** 结束。第一次探针遗漏 `source_handle`，先被图 admission 拒绝；补成合法 `action-0` 后才得到上述执行器证据，未把构造错误算入缺陷。

#### DeepSeek 官方契约边界与成熟 Agent 对照

- DeepSeek 官方 [Responses API 指南](https://api-docs.deepseek.com/zh-cn/guides/responses_api/)只描述模型响应和客户端继续提交输入；它没有本地表单、审批或等待节点语义。若模型需要人工确认，宿主必须持久化自己的 pending state，不能把“请求成功”当成用户已回答。
- **Codex** 的 app-server/turn 体系把 approval request 绑定到具体 turn/call，并有 `AskForApproval`、permission profile 和可恢复的等待事件；未得到用户决定时工具不执行，晚到/跨 run 的决定会被拒绝。这个模式比直接把节点标记 failed 更适合工作流，但需要沿用本系统已有审批作用域和事件 store，而不是新建第二套表单中心。
- **Claude Code** 通过 `AskUserQuestion`/hook 交互让 agent 停在需要用户回答的阶段，回答成为下一次 agent 输入；用户拒绝、超时和取消都进入显式分支。具体 UI 协议不能直接复制，但“pending → answer/deny/timeout → resume”状态机是本系统缺少的。
- **DeepSeek Harness** 的 provider loop 只负责模型与工具调用，人工状态由宿主 scope/turn 管理；这再次说明人工输入是 Harness 控制面能力，而非 DeepSeek Responses 字段。

#### 影响、后果与根因修复方向

- 触发后，用户在画布上配置的人工审批无法出现表单或通知，工作流直接失败；自动化任务不会等待用户、不会在回答后恢复下游节点，且 `timeoutSeconds` 完全无效。对需要人工确认的发布、外部发送和高风险工具流程，这会让用户改用绕过审批的替代路径，或把工作流误判为基础设施故障。
- 事件审计也不完整：只有 `Running → Failed`，没有字段定义、请求 ID、作用域、过期时间、用户回答或拒绝原因。CLI/SDK/Tauri 不能统一展示 pending 状态，重启后无法恢复等待中的工作流。
- 根因修复应复用现有 `AgentDecision::NeedsApproval`/`InputLeaseRegistry`/RunStore：在节点进入时生成不可变 `HumanInputRequest`（run、node、revision、字段 schema、timeout、scope），持久化并发出唯一 `RunAwaitingInput` 事件；UI、CLI、SDK 通过统一 `input/respond` 路由提交答案，按 run/node/revision 校验后恢复同一 agent。超时、取消、拒绝和重复回答必须是幂等终态，并撤销待处理租约。
- 如果短期不实现该状态机，应把 registry 改成 `reserved`，在保存/compile 阶段明确拒绝，而不是继续向用户承诺可用。回归需覆盖多字段校验、action 分支、timeout、取消、重启恢复、跨 run 迟到回答、并发输入租约和下游事件顺序。

本轮 H-180 只新增正式审计探针和文档，未修改生产人工输入或审批逻辑。

### H-181：专业画布 `iteration` 不消费前端输入数组，也没有真正调度迭代体

**级别：高（P1，工作流数据完整性与控制流）；触发条件：使用专业画布的迭代节点，给 `inputVariable` 绑定一个非空数组并连接下游。** 节点在 UI 中标记为 enabled，提供并行、并发数、错误处理和扁平化选项；运行时却读取另一套内部字段，非空输入被静默当成空集合，工作流仍成功。

#### 生产调用链与正式复现

- `apps/desktop/src/canvas/workflow/utils/nodeRegistry.ts` 与 `nodeSchemas.ts` 暴露 `inputVariable`、`outputVariable`、`parallel`、`maxConcurrency`、`errorHandling`、`flatten`；节点输出声明为 `output` 数组，控制流出口为 `item`/`done`。这向用户承诺的是“对输入数组逐项执行并产出结果”的图节点。
- `WorkflowAgent::execute_iteration()` 实际只读取 `iterationCollection`、`iteratorVariable`、`iterationOperation` 和 `iterationTemplate`。这些字段不在前端 schema 中，也没有按 `item` 分支执行子图、并发调度、`outputVariable` 绑定、错误策略或 `flatten` 处理；缺少 `iterationCollection` 时直接返回 `output=[]` 和 `__note=empty collection`。
- 新增正式 ignored 探针 `crates/deepagent-app-core/tests/workflow_audit.rs::audit_workflow_iteration_processes_frontend_input_contract`。它使用合法的 `start` 输入声明（`items=["ONE","TWO"]`）、规范的 `iteration` `item` 后继边和正式 `ChatService::run_workflow()`，捕获节点事件并读取 `RunStore`。为隔离既有 start 声明字段差异，fixture 同时提供 `name`/`variable`，不把该兼容字段问题算入本项。
- 精确命令：`cargo test -p deepagent-app-core --test workflow_audit audit_workflow_iteration_processes_frontend_input_contract --offline -- --ignored --exact --nocapture`。
- 实际输出：`api_ok=true api_error=None iteration_events=[(Running, None, None), (Completed, Some(Object {"__note": String("empty collection"), "output": Array []}), None)] run_terminal=Some("succeeded")`；断言应得到 `{"output":["ONE","TWO"]}`，实际以 **0 passed / 1 failed（预期）** 结束。前两轮探针分别因缺少 start 声明兼容字段和 `source_handle=item` 被 admission 拒绝，补齐后才得到本条 executor 证据；不把构造错误计入缺陷。

#### DeepSeek 官方契约边界与成熟 Agent 对照

- DeepSeek 官方 [Responses API 指南](https://api-docs.deepseek.com/zh-cn/guides/responses_api/)描述的是一次无状态模型请求和客户端后续输入，不定义本地 DAG 的迭代、并发或下游合流语义。即使迭代体内调用 DeepSeek，也必须由 Harness 管理每个 item 的请求、取消、usage、重试和结果顺序。
- **Codex** 的 agent loop 对一个 turn 中的并行工具调用有明确的调用组/独占 barrier、取消与结果关联；它不会先把“计划调用 N 次”显示成完成，再把输入集合悄悄变成空。这里可借鉴的是每个 item 都有 owner/call identity、并发策略和最终聚合事实。
- **Claude Code** 的批量/并行工具调用仍由工具运行时交付每个调用结果，失败、取消和输出顺序进入下一轮上下文；它不是本系统图迭代的同一 API，但说明“循环/并发配置必须落到实际调度器”，不能只保存在 schema。
- **DeepSeek Harness** 的 tool-call runtime 将并行安全工具分组、保留调用顺序并把每项错误回传 agent；这比当前 `execute_iteration()` 的单次空数组返回更接近可审计的迭代执行，但不等于本系统可以直接复制其 tool API。

#### 影响、后果与根因修复方向

- 触发后，数组输入全部丢失，下游看到空结果却收到成功终态；若迭代体原本用于批量 LLM、HTTP、文件或媒体操作，系统会漏处理项目但不会告警，自动化消费者会把漏处理当作完成。`parallel`、`maxConcurrency` 和 `errorHandling` 看似可配置，实际完全不生效。
- 事件流没有 item 级 run/node 身份、尝试次数、聚合规则或跳过/失败原因，恢复/重放无法知道哪些元素已处理；重复运行可能既不会补齐缺失元素，也无法安全去重已完成元素。当前探针只使用字符串数组和空下游，不声称已造成真实 provider 费用或外部副作用。
- 根因修复应先定义 typed `IterationPlan`：在 compile/admission 解析 `inputVariable` 为不可变数组引用、迭代体边界、并发/错误/flatten 策略和 output schema；运行时为每个 item 建立子执行上下文与取消 token，复用现有 `AgentKernel`、权限/审批、事件和 RunStore，不另造运行中心。每个 item 的成功、失败、取消、超时和重试必须可重放并在合流时保持定义的顺序。
- 如果短期不实现子图迭代，应把节点改成 `reserved`，或在 compile 阶段明确 `UnsupportedOperation`；不能用内部测试字段驱动一个与前端配置无关的简化 map。回归需覆盖空/非空数组、对象和文件项、串行/并行上限、item 级取消、部分失败三种策略、flatten、下游合流、重启恢复及 CLI/SDK/Tauri 一致性。

本轮 H-181 只新增正式审计探针和文档，未修改生产迭代调度逻辑。

### H-182：专业画布 `answer` 节点忽略 `answerTemplate`，成功运行却输出空答案

**级别：高（P1，用户可见结果完整性）；触发条件：使用专业画布的“直接回答”节点，配置非空 `answerTemplate`，可选地绑定 `answerVariables`。** 节点会完成并让下游继续，但最终答案不是用户配置的模板，而是执行器对缺失字段的空字符串回退。

#### 生产调用链与正式复现

- `apps/desktop/src/canvas/workflow/utils/nodeRegistry.ts` 将 `answer` 注册为 `availability: "enabled"`，默认字段为 `answerTemplate` 和 `answerVariables`；`nodeSchemas.ts` 也以这两个字段向用户提供模板编辑器和变量绑定。
- `WorkflowAgent::execute_answer()` 却读取 `text` 和 `variables`。当生产桌面 JSON 带有 `answerTemplate` 时，`text` 缺失，执行器用 `Value::String("")` 作为默认值并返回 `{"answer":""}`；没有 schema mismatch 错误或完成门禁。
- 新增正式 ignored 探针 `crates/deepagent-app-core/tests/workflow_audit.rs::audit_workflow_answer_node_renders_frontend_contract`。它通过正式 `ChatService::run_workflow()` 执行 `start → answer → end`，提交 `answerTemplate="EXPECTED_ANSWER"` 和空变量绑定，捕获完成事件并读取 `RunStore`。
- 精确命令：`cargo test -p deepagent-app-core --test workflow_audit audit_workflow_answer_node_renders_frontend_contract --offline -- --ignored --exact --nocapture`。
- 实际输出：`api_ok=true answer_outputs=Some(Object {"answer": String("")}) run_terminal=Some("succeeded")`；断言应返回 `EXPECTED_ANSWER`，以 **0 passed / 1 failed（预期）** 结束。没有模型、工具、网络或文件副作用；失败是正式配置到 executor 的消费不一致。

#### DeepSeek 官方契约边界与成熟 Agent 对照

- DeepSeek 官方 [Responses API 参数文档](https://api-docs.deepseek.com/zh-cn/api/create-response/)定义模型输出 item，不定义本地 canvas 的“直接回答”节点或模板字段。模型请求成功不能证明宿主已经渲染了用户要求的最终答案；最终文本属于本地工作流 DTO 和事件终态。
- **Codex** 的最终 turn 输出来自实际 assistant message/response item，并通过所属 thread/turn 事件交付；不会把缺失的显示字段当成成功答案。可借鉴的是完成事件必须携带事实输出，UI 投影不能替代核心结果。
- **Claude Code** 的最终回答由实际 agent message 和 hook/工具生命周期共同决定；配置或命令输出缺失时进入错误/继续询问路径，不会以空字符串伪装已回答。具体文本协议不能直接复制到本系统。
- **DeepSeek Harness** 保留 provider-native response items，再生成 UI 文本投影；这比当前 answer executor 直接读取错误字段更能避免“模型/模板已有内容但投影为空”的静默丢失。这里借鉴的是事实结果与展示投影分离，不是 Responses 字段映射。

#### 影响、后果与根因修复方向

- 触发后，画布 UI、CLI/SDK 和下游节点会看到空答案；用户难以区分“模板本来就是空”与“配置被执行器忽略”。如果该节点用于最终回复、通知正文、审批说明或 API 响应，任务虽标成功却交付空内容，可能触发错误自动化动作。
- 该问题与 H-40 的模板转换/end 输出映射缺口相邻但不重复：H-40 证明跨语言序列化和多个节点字段漂移；H-182 进一步证明直接回答这个最终用户出口在正式运行中具体静默空成功。
- 根因修复应统一 answer config DTO：前端 `answerTemplate`/`answerVariables`、Rust validator、`execute_answer` 和事件 output 使用同一版本化字段；旧字段 `text`/`variables` 只能通过显式迁移处理。缺失必填模板、未知字段或变量引用错误应在 compile/admission 失败；完成门禁应拒绝没有实际 answer payload 的成功事件。
- 回归需覆盖纯文本、变量引用、非字符串 JSON 值、空模板策略、模板语法错误、结束节点映射、重放/恢复和 CLI/SDK/Tauri 一致性，断言 wire/事件/RunStore 三者输出一致。本轮只新增审计，不修改生产执行器。

本轮 H-182 只新增正式审计探针和文档，未修改生产回答逻辑。

### H-183：专业画布 `variable-aggregator` 忽略 `aggregatorVariables`，成功运行却输出空数组

**级别：高（P1，工作流数据完整性）；触发条件：使用专业画布的变量聚合节点，绑定一个或多个上游变量。** 前端配置能够通过 schema 和图编译，但运行时读取另一套字段，合法输入不会进入结果，且没有错误终态。

#### 生产调用链与正式复现

- `apps/desktop/src/canvas/workflow/utils/nodeRegistry.ts` 将 `variable-aggregator` 注册为 `availability: "enabled"`，默认字段为 `aggregatorOutputType` 与 `aggregatorVariables`；`nodeSchemas.ts` 用变量引用控件让用户选择多个上游值，输出声明为 `output`。
- `WorkflowAgent::execute_variable_aggregator()` 只读取 `variables` 数组并逐项解析；它不读取公开的 `aggregatorVariables`。当生产配置使用前端字段时，函数拿到空数组，返回 `{"output":[]}` 并发布 `Completed`。
- 新增正式 ignored 探针 `crates/deepagent-app-core/tests/workflow_audit.rs::audit_workflow_variable_aggregator_consumes_frontend_bindings`。它通过正式 `ChatService::run_workflow()` 执行 `start → variable-aggregator → end`，将 `FIRST`/`SECOND` 绑定到两个前端变量引用，捕获节点输出和 `RunStore`。
- 精确命令：`cargo test -p deepagent-app-core --test workflow_audit audit_workflow_variable_aggregator_consumes_frontend_bindings --offline -- --ignored --exact --nocapture`。
- 实际输出：`api_ok=true aggregate_outputs=Some(Object {"output": Array []}) run_terminal=Some("succeeded")`；断言应为 `{"output":["FIRST","SECOND"]}`，以 **0 passed / 1 failed（预期）** 结束。没有模型、工具、网络或文件副作用。

#### DeepSeek 官方契约边界与成熟 Agent 对照

- DeepSeek 官方 [Responses API 指南](https://api-docs.deepseek.com/zh-cn/guides/responses_api/)只定义 provider input/output，不定义本地变量聚合器。模型请求成功不能证明上游数据已经进入下游 prompt 或最终输出；聚合属于 Harness 的 typed workflow contract。
- **Codex** 的工具调用结果按 call ID 逐项保留并在下一轮输入中提交，缺少结果会成为明确的工具错误/不完整状态，而不是把列表变成空集合后成功。可借鉴的是输入项、聚合结果与 owner 一一对应。
- **Claude Code** 的工具/子任务结果由运行时以结构化结果交付，失败和空结果有不同状态；它不会把未解析的配置字段当作“无输入”的正常业务结果。具体工具协议不是本系统 DTO。
- **DeepSeek Harness** 的 agent loop 保留每个 tool result 的 `isError`、调用身份和顺序，再决定下一步；本系统的数据节点也应保留绑定来源与解析结果，而不是在字段不匹配时返回默认空数组。

#### 影响、后果与根因修复方向

- 触发后，所有上游绑定值被丢掉，下游 LLM、回答、HTTP 或工具节点收到空列表却看到成功终态；批量提示、权限名单、收件人列表或文件集合可能变成空操作，系统不会告警。当前样本只证明两个临时字符串丢失，不声称已执行外部动作。
- 事件输出没有记录绑定字段、解析后的来源和聚合版本；重放、恢复和跨入口运行无法区分“用户真的选择了空列表”与“字段被执行器忽略”。该点与 H-40 的跨语言配置总括相关，但本条提供了一个尚未覆盖的聚合节点正式动态证据。
- 根因修复应把 `aggregatorVariables` 固化为版本化 typed binding DTO，compile 阶段验证每个引用的上游输出和类型，executor 只消费该 DTO；成功输出必须包含来源/计数元数据，未知或旧字段必须显式迁移或拒绝。若暂不实现，应将节点改为 `reserved`，不能默认空集合成功。
- 回归需覆盖字符串/数字/对象/数组、空绑定、重复绑定、上游失败/跳过、类型声明、顺序稳定性、下游引用、重启恢复以及 CLI/SDK/Tauri 入口，断言实际输出与事件/RunStore 一致。本轮只新增审计，不修改生产变换逻辑。

本轮 H-183 只新增正式审计探针和文档，未修改生产变量聚合逻辑。

### H-184：工作流 `start` 的必填输入缺少 admission 校验，缺失值被静默填成 null

**级别：高（P1，输入完整性与自动化可靠性）；触发条件：工作流声明 `start.inputVariables[].required=true`，调用者通过 CLI/SDK/Tauri 提交的 `WorkflowRequest.inputs` 缺少该字段。** 该约束在桌面 schema 中明确存在，但后端不拒绝、不等待，也不产生输入错误。

#### 生产调用链与正式复现

- `apps/desktop/src/canvas/workflow/utils/nodeSchemas.ts` 的 `VARIABLE_DECLARATION_PROPERTIES` 提供 `required` 字段，start 节点允许字符串、数字、布尔、对象、数组和文件输入；因此必填语义是公开 workflow contract 的一部分。
- `deepagent-runtime::workflow::graph::output_names()` 只校验声明名称和类型，不读取 `required`；`WorkflowAgent::execute_start()` 对每个声明执行 `self.inputs.get(name).cloned().unwrap_or(Value::Null)`，缺失值直接变成 null。`WorkflowRequest.inputs` 的 serde 默认空 Map 也让调用者可以省略整个输入对象。
- 新增正式 ignored 探针 `crates/deepagent-app-core/tests/workflow_audit.rs::audit_workflow_missing_required_input_is_rejected`。它通过正式 `ChatService::run_workflow()` 执行包含 `required=true` 的 `start → end` 图，提交 `inputs={}`，捕获 start 事件并读取 `RunStore`。
- 精确命令：`cargo test -p deepagent-app-core --test workflow_audit audit_workflow_missing_required_input_is_rejected --offline -- --ignored --exact --nocapture`。
- 实际输出：`api_ok=true start_outputs=Some(Object {"customer": Null}) run_terminal=Some("succeeded")`；断言缺失必填值必须拒绝，测试以 **0 passed / 1 failed（预期）** 结束。没有模型、工具、网络或文件副作用。

#### DeepSeek 官方契约边界与成熟 Agent 对照

- DeepSeek 官方 [Responses API 参数文档](https://api-docs.deepseek.com/zh-cn/api/create-response/)把 `input` 作为每次请求的显式输入，但不定义本地工作流变量的 required/default 语义。宿主必须在请求模型前完成业务输入校验；把 null 发给模型不能视为满足必填约束。
- **Codex** 的 app-server 在创建 thread/turn 时先解析并校验请求 DTO，未知/缺失控制面引用返回 `invalid_request`，不会创建一个带空值的“成功”运行。其具体字段不是本系统兼容协议，但 admission-first 原则适用。
- **Claude Code** 的命令/工具输入在边界按 schema 和 required 参数校验，缺少必需参数进入工具错误或再次询问，不把空值当作用户确认。其交互 UI 不应直接复制到本系统。
- **DeepSeek Harness** 在 provider/config resolver 边界对公开 options 做 typed validation；可借鉴的是每个公开入口都执行同一套校验，而不是只依赖桌面 schema。

#### 影响、后果与根因修复方向

- 触发后，下游节点会把 null 当作真实业务值继续运行；条件判断可能走错误分支，模板/模型提示会出现空字段，工具或 HTTP 节点可能构造错误请求。run 仍是 `succeeded`，调度器和 SDK 无法区分“用户明确提供 null”和“输入根本缺失”。
- 该问题也破坏跨入口一致性：桌面表单可能阻止缺失字段，但公开 JSON、恢复、定时任务和 SDK 可以直接绕过 UI；事件只记录 null，不记录缺失原因、schema revision 或默认值来源。
- 根因修复应在 `run_workflow_inner()` 的 admission 边界将 `inputVariables` 编译为 typed `InputPlan`，对 required/default/options/min/max/类型做一次不可变校验；缺失 required 返回结构化 `MissingWorkflowInput`，可选字段才应用显式 default。start executor 只消费已经校验的输入，不再自行 `unwrap_or(Value::Null)`。
- 回归需覆盖字符串空值与缺失的区别、显式 null、默认值、枚举/长度/类型、文件引用、恢复/重放、定时任务以及 CLI/SDK/Tauri 三入口；错误必须在模型、工具和媒体调用前发生，并在事件/RunStore 保留可诊断字段。本轮只新增审计，不修改生产 admission。

本轮 H-184 只新增正式审计探针和文档，未修改生产输入校验逻辑。

### H-185：工作流 `start` 不校验输入类型，声明 number 却接受任意字符串

**级别：高（P1，类型安全与下游行为）；触发条件：工作流输入声明 `type=number`、`required=true`，调用者提交字符串或其他不兼容 JSON 类型。** 后端不会在工作流 admission 或 start executor 处拒绝，错误值继续进入条件、模板、模型和工具节点。

#### 生产调用链与正式复现

- 桌面 start schema 允许声明 `string/number/boolean/object/array/file`，`WorkflowNodeSpec` 和 `WorkflowRequest.inputs` 也能完整携带这些字段；因此本项不依赖 UI 丢字段或旧 DTO。
- `graph::output_names()` 只读取声明名和类型做静态输出集合校验，不对 `WorkflowRequest.inputs` 做 JSON 类型检查；`WorkflowAgent::execute_start()` 直接 clone 输入值，不读取声明的 `type`、`required`、`options`、长度或数值边界。
- 新增正式 ignored 探针 `crates/deepagent-app-core/tests/workflow_audit.rs::audit_workflow_input_type_mismatch_is_rejected`。它通过正式 `ChatService::run_workflow()` 执行完整 `start → end` 图，声明 `count` 为 number，提交 `inputs.count="not-a-number"`，捕获 start 输出并读取 `RunStore`。
- 精确命令：`cargo test -p deepagent-app-core --test workflow_audit audit_workflow_input_type_mismatch_is_rejected --offline -- --ignored --exact --nocapture`。
- 实际输出：`api_ok=true start_outputs=Some(Object {"count": String("not-a-number")}) run_terminal=Some("succeeded")`；断言类型不匹配必须拒绝，测试以 **0 passed / 1 failed（预期）** 结束。没有模型、工具、网络或文件副作用。

#### DeepSeek 官方契约边界与成熟 Agent 对照

- DeepSeek 官方 [Responses API 参数文档](https://api-docs.deepseek.com/zh-cn/api/create-response/)要求请求字段满足 provider 的 JSON 形状，但不会知道本地 workflow 变量的业务类型。宿主应在进入 provider/tool loop 之前完成类型校验，不能把字符串 number 当作合法模型输入。
- **Codex** 的工具 schema/参数验证在工具执行前进行，参数类型错误会形成工具错误并阻止执行；app-server 的请求边界也拒绝非法控制面字段。可迁移的是“schema 是可执行约束，不是仅 UI 提示”。
- **Claude Code** 的工具输入由 JSON schema 和命令参数边界校验，缺少/错误类型进入错误或重新询问，不以原值继续执行；其 schema 名称和 UI 不属于本系统协议。
- **DeepSeek Harness** 在 provider 配置与工具 resolver 边界保留 typed validation，公开 SDK、agent 创建和 server initialize 都不能绕过同一校验。这个原则比只在桌面表单校验更符合多宿主 Harness。

#### 影响、后果与根因修复方向

- 触发后，条件比较可能因字符串/数字差异走错分支，模板渲染得到错误文本，模型收到不符合业务 schema 的上下文，工具/HTTP 节点可能构造错误请求；run 仍成功，调用者无法定位输入错误。当前样本只证明字符串进入 start，不声称已造成外部副作用。
- 该缺口与 H-184 共同表明 start schema 没有形成后端 `InputPlan`；H-184 是缺失值，H-185 是已存在但错误类型，两个不变量需分别回归。显式 JSON `null`、可选默认值和字符串空值也不能混用。
- 根因修复应在 `run_workflow_inner()` admission 编译 typed input plan，严格校验类型、required/default、枚举、长度、数字范围和 file/artifact 引用，返回结构化 `InvalidWorkflowInput`；start executor 只消费已验证的值，不再直接 clone 任意 Map。
- 回归需覆盖所有类型、整数/浮点边界、null、数组元素类型、对象字段、文件引用、默认值、恢复/重放和 CLI/SDK/Tauri 三入口，确认错误发生在模型/工具/媒体调用前并持久化可诊断原因。本轮只新增审计，不修改生产输入校验。

本轮 H-185 只新增正式审计探针和文档，未修改生产输入类型处理逻辑。

### H-186：工作流 `start` 没有输入 allowlist，未声明字段进入执行上下文并成功传播

**级别：高（P1，输入边界与数据隔离）；触发条件：调用者在 `WorkflowRequest.inputs` 中提交未出现在 `start.inputVariables` 的额外字段。** 后端既不拒绝，也不忽略；它把额外字段直接放入 start 节点输出，使下游节点和模型能够读取未声明数据。

#### 生产调用链与正式复现

- `WorkflowDefinition` 的 start 节点通过 `inputVariables` 声明公开输入，桌面 schema 为每个变量提供名称、类型和必填属性；按 typed workflow contract，未声明字段应在 admission 失败或至少不进入节点输出。
- `WorkflowAgent::execute_start()` 先处理声明变量，随后遍历 `self.inputs` 并执行 `result.entry(key).or_insert_with(...)`。这一段把所有额外 key 复制到输出，完全绕过声明的 allowlist、类型、required 和 scope 校验。
- 新增正式 ignored 探针 `crates/deepagent-app-core/tests/workflow_audit.rs::audit_workflow_undeclared_input_is_rejected_or_ignored`。它通过正式 `ChatService::run_workflow()` 执行只声明 `known` 的 `start → end` 图，提交 `known="KNOWN"` 和 `unexpected="UNDECLARED_CANARY"`，捕获 start 输出并读取 `RunStore`。
- 精确命令：`cargo test -p deepagent-app-core --test workflow_audit audit_workflow_undeclared_input_is_rejected_or_ignored --offline -- --ignored --exact --nocapture`。
- 实际输出：`api_ok=true has_undeclared=true start_outputs=Some(Object {"known": String("KNOWN"), "unexpected": String("UNDECLARED_CANARY")}) run_terminal=Some("succeeded")`；断言未声明字段必须拒绝或排除，测试以 **0 passed / 1 failed（预期）** 结束。没有模型、工具、网络或文件副作用。

#### DeepSeek 官方契约边界与成熟 Agent 对照

- DeepSeek 官方 [Responses API 参数文档](https://api-docs.deepseek.com/zh-cn/api/create-response/)只定义 provider 接收的 `input` item，不知道本地工作流有哪些合法变量。宿主把未声明字段放入 `input` 后，模型会把它当作正常上下文；这不是 provider 可以替宿主修复的业务边界。
- **Codex** 的 tool schema 和 permission profile 按公开参数/能力 allowlist 构造调用，未声明参数不会自动成为工具输入；请求控制面引用在 app-server admission 失败，而不是被复制进 thread 上下文。可迁移的原则是“公开 schema 是 allowlist”。
- **Claude Code** 的工具输入以 schema 定义的字段为边界，额外字段通常在命令/工具适配层被拒绝或丢弃；用户提示中出现的任意文本也不会自动升级为工具参数。
- **DeepSeek Harness** 的 agent/provider scope 在装配时绑定，resolver 只发布声明且可见的能力；其 typed boundary 可作为本系统输入 allowlist 的参考，不能把所有 JSON map 当作可见上下文。

#### 影响、后果与根因修复方向

- 触发后，调用者可以把未声明的任意字段注入工作流上下文，绕过 UI 中的输入声明和类型/required 校验；下游条件、模板、模型、HTTP 或工具节点可能读取这些隐藏值。当前样本只证明 canary 进入 start 输出，没有执行外部副作用或宣称数据泄露。
- 该问题与 H-184/H-185 分别覆盖缺失值和错误类型：即使所有已声明字段都满足 required/type，未声明字段仍能越过第三条 admission 不变量。事件也没有记录输入 schema revision、被拒绝字段或实际 allowlist，重放无法判断上下文来源。
- 根因修复应在 `run_workflow_inner()` 编译 typed `InputPlan`：对输入 key 做 exact allowlist，未知字段返回结构化 `UnknownWorkflowInput`；start executor 只读取计划中的字段，不再遍历原始 Map。若需要透传动态 payload，应在 schema 中声明一个明确的 object/map 字段并对其 scope、大小和敏感字段策略单独校验。
- 回归需覆盖未知字段、大小写/Unicode 近似名、显式动态 object、默认值、恢复/重放、压缩、CLI/SDK/Tauri 三入口，并断言未知字段在模型/工具/媒体请求前已被拒绝或明确隔离。本轮只新增审计，不修改生产输入 allowlist。

本轮 H-186 只新增正式审计探针和文档，未修改生产工作流输入边界。

### H-187：工作流声明的输入默认值被静默丢弃，缺失字段变成 `null` 仍成功

**级别：中高（P1，输入语义与可复现性）；触发条件：`start.inputVariables` 声明某个非必填输入并设置 `default`，调用方不在 `WorkflowRequest.inputs` 中提供该字段。** 后端不采用声明的默认值，而是把输入写成 `null`，随后继续执行并记录成功终态。

#### 生产调用链与正式复现

- 桌面 `InputVariableDefinition` 明确公开 `default`，并允许输入变量只声明 `type` 而不设置 `required`；`workflowExecutor.collectInputs()` 也按 `v.default ?? ""` 构造桌面入口的初始请求。这个字段因此是公开工作流契约，不是仅供渲染的注释。
- Rust `WorkflowDefinition::compile()` 只检查声明名和 `type`，没有校验或编译默认值；`WorkflowAgent::execute_start()` 对每个声明变量执行 `self.inputs.get(name).cloned().unwrap_or(Value::Null)`，完全忽略同一声明中的 `default`。
- 新增正式 ignored 探针 `crates/deepagent-app-core/tests/workflow_audit.rs::audit_workflow_declared_default_is_applied_when_input_is_missing`。它通过正式 `ChatService::run_workflow()` 执行 `start → end`，声明 `greeting` 为 `string`、`default="FALLBACK"`，传入空 `inputs`，捕获 start 输出并读取 `RunStore`。
- 精确命令：`cargo test -p deepagent-app-core --test workflow_audit audit_workflow_declared_default_is_applied_when_input_is_missing --offline -- --ignored --exact --nocapture`。
- 实际输出：`api_ok=true greeting=Some(Null) run_terminal=Some("succeeded")`；断言默认值必须进入输出，测试以 **0 passed / 1 failed（预期）** 结束。没有模型、工具、网络或文件副作用。

#### DeepSeek 官方契约边界与成熟 Agent 对照

- DeepSeek 官方 [Responses API 参数文档](https://api-docs.deepseek.com/zh-cn/api/create-response/)只把 `input` 定义为字符串或输入 item 列表，并说明 `input` 与 `instructions` 至少传一个；它没有本地 workflow 变量的 `default`、`required` 或字段类型语义。因此默认值必须在宿主 admission/执行计划中解析，不能指望 DeepSeek provider 补齐。
- **DeepSeek Harness** 的 `vendor/schemastery/src/index.ts` 在 `Schema.resolve()` 中区分 `required` 和 `meta.default`：缺失值遇到非空 default 会先用 clone 后的默认值，再进入类型/范围 resolver；只有没有默认且非必填时才保留空值。这是本系统应采用的 typed input plan 语义。
- **Codex** 将配置和工具参数的 schema/默认值保留在宿主侧，并在请求前解析有效配置；公开 Responses 工具 schema 将参数边界作为声明契约，不把缺失字段任意复制到上下文。可迁移原则是“缺省值在 admission 解析一次，之后请求、事件和恢复都使用同一个规范化对象”。
- **Claude Code** 的成熟工具/插件链以 schema 校验结果作为执行前边界，验证失败需要返回结构化错误而不是继续成功；本系统当前把声明默认值静默替换成 `null`，比显式失败更难发现，也会让下游模型/工具看到错误但类型合法的输入。

#### 影响、后果与根因修复方向

- 触发后，依赖默认值的模板、条件、HTTP body、工具参数或 DeepSeek prompt 会收到 `null`，而运行记录却是 `succeeded`。这会造成错误分支选择、空请求参数、模型输出漂移和重放不可复现；若默认值用于权限/路由/供应商选择，还可能把调用导向错误能力。
- H-184 只证明缺失 required 没有被拒绝，H-187 进一步证明即使 schema 明确给出合法 fallback，执行器仍丢失语义；H-185 的类型检查也无法弥补这种“值缺失但不失败”的静默错误。
- 根因修复应在 workflow admission 阶段编译 typed `InputPlan`：逐变量按 `provided → default → required error → typed null` 的明确优先级规范化，并对 default 本身执行声明类型、options、长度/数值范围校验。`execute_start()` 只消费该计划，禁止直接读取原始 `inputs` 或自行产生 `null`。
- 回归需覆盖显式值优先于 default、显式 `null` 是否允许、default 类型不匹配、default 不在 options、字符串/数值边界、恢复/重放以及 CLI/SDK/Tauri 三入口，并在事件中保留规范化输入来源（provided/default）以便审计。本轮只新增审计，不修改生产输入处理逻辑。

本轮 H-187 只新增正式审计探针和报告，未修改生产工作流输入边界。

### H-188：工作流字符串 `options` 不参与 admission，非法枚举值成功进入执行上下文

**级别：中高（P1，输入约束与路由安全）；触发条件：`start.inputVariables` 声明 `type="string"` 与 `options=["safe","fast"]`，调用方提交列表之外的值。** 后端不拒绝，也不标记输入无效，而是将任意字符串继续传播并记录成功终态。

#### 生产调用链与正式复现

- 桌面 `InputVariableDefinition.options?: string[]` 与 start schema 的 `options` 字段明确表达字符串枚举，UI 会用它生成可选值控件；该字段不是未知扩展字段。
- Rust `WorkflowDefinition::compile()` 的 `validate_type()` 只接受 `string/number/boolean/object/array/file`，没有读取 `options`；`WorkflowAgent::execute_start()` 直接 clone `self.inputs` 中的值，没有枚举检查。
- 新增正式 ignored 探针 `crates/deepagent-app-core/tests/workflow_audit.rs::audit_workflow_string_options_are_enforced`。它通过 `ChatService::run_workflow()` 执行 `start → end`，声明 `mode` 的选项为 `safe/fast`，提交 `mode="UNDECLARED_MODE"`，捕获 start 输出并读取 `RunStore`。
- 精确命令：`cargo test -p deepagent-app-core --test workflow_audit audit_workflow_string_options_are_enforced --offline -- --ignored --exact --nocapture`。
- 实际输出：`api_ok=true start_outputs=Some(Object {"mode": String("UNDECLARED_MODE")}) run_terminal=Some("succeeded")`；断言非法枚举必须拒绝或使 run 失败，测试以 **0 passed / 1 failed（预期）** 结束。没有模型、工具、网络或文件副作用。

#### DeepSeek 官方契约边界与成熟 Agent 对照

- DeepSeek 官方 [Responses API 参数文档](https://api-docs.deepseek.com/zh-cn/api/create-response/)把本地工作流值最终当作 `input` 内容；文档没有声明宿主自定义的 `options` 语义。provider 不会知道 `safe/fast` 是业务 allowlist，必须在本地 admission 先做 exact match。
- **DeepSeek Harness** 的 schemastery 将约束放在 schema resolver，而不是只用于 UI：字符串/数字 resolver 会检查元数据的边界，`const`/union 等 schema 会对不匹配值抛 `ValidationError`。迁移到本系统应把 `options` 编译为字符串 enum/union，并在 provider、工具和 HTTP 节点前复用同一结果。
- **Codex** 的工具参数通过 JSON Schema 发布，模型可见字段和允许值属于工具输入契约；宿主在执行工具前依据 schema/类型做边界处理，而不是把任意字符串当作已验证参数。相同原则适用于 workflow start 的业务枚举。
- **Claude Code** 的工具/插件输入以 schema 校验为执行门槛，参数不符合声明时应产生结构化 validation error；本系统当前把非法 option 作为成功数据继续下游，缺少成熟 agent 的 fail-closed 行为。

#### 影响、后果与根因修复方向

- 触发后，非法模式会进入条件节点、模板、HTTP 请求、工具参数或 DeepSeek prompt；若 `options` 控制 provider、权限、执行策略或业务分支，调用者可以绕过 UI 选择器注入未声明模式。当前样本只证明值传播和成功终态，没有执行外部副作用。
- H-185 证明“类型错误”可成功，H-188 进一步证明即使值的 JSON 类型正确，枚举语义仍完全未校验；H-187 的 default 处理也不能替代 options 检查。
- 根因修复应在同一个 typed `InputPlan` 中编译 `options` 为 exact string set，定义大小写、Unicode 规范化和空字符串策略；提供值和 default 都必须先过 enum 检查，失败返回结构化 `InvalidWorkflowInput`，不得降级成 `null` 或继续成功。
- 回归需覆盖合法首尾值、非法值、大小写/Unicode 近似、空数组/重复 options、default 不在 options、恢复/重放和 CLI/SDK/Tauri 三入口，并断言 provider/工具调用前没有非法值。本轮只新增审计，不修改生产输入边界。

本轮 H-188 只新增正式审计探针和报告，未修改生产工作流输入边界。

### H-189：工作流 `minimum/maximum` 不参与 admission，越界数值成功进入执行上下文

**级别：中高（P1，输入约束与资源/策略边界）；触发条件：`start.inputVariables` 声明 `type="number"` 且设置 `minimum`/`maximum`，调用方提交范围外的 number。** 后端不拒绝越界值，仍传播并记录成功。

#### 生产调用链与正式复现

- 桌面 `InputVariableDefinition` 与 start schema 公开 `minimum`、`maximum`，用于配置数值输入约束；前端 schema 对字段类型也标成 number。
- Rust `WorkflowDefinition::compile()` 的 `validate_type()` 只校验类型名字，忽略 `minimum`/`maximum`；`WorkflowAgent::execute_start()` 直接把输入 map 中的 JSON number 复制到输出，没有范围判断。
- 新增正式 ignored 探针 `crates/deepagent-app-core/tests/workflow_audit.rs::audit_workflow_numeric_bounds_are_enforced`。它通过正式 `ChatService::run_workflow()` 执行 `start → end`，声明 `retries` 范围 `[10,20]`，提交 `retries=3`，捕获 start 输出并读取 `RunStore`。
- 精确命令：`cargo test -p deepagent-app-core --test workflow_audit audit_workflow_numeric_bounds_are_enforced --offline -- --ignored --exact --nocapture`。
- 实际输出：`api_ok=true start_outputs=Some(Object {"retries": Number(3)}) run_terminal=Some("succeeded")`；断言越界数值必须拒绝或使 run 失败，测试以 **0 passed / 1 failed（预期）** 结束。没有模型、工具、网络或文件副作用。

#### DeepSeek 官方契约边界与成熟 Agent 对照

- DeepSeek 官方 [Responses API 参数文档](https://api-docs.deepseek.com/zh-cn/api/create-response/)描述 provider 自身的参数范围（例如 temperature/top_p），但不认识本地 workflow 变量的 `minimum/maximum`。这些业务范围必须由宿主在发送模型、工具或 HTTP 请求前执行。
- **DeepSeek Harness** 的 schemastery `checkWithinRange()` 会对 number/string/array 等 schema 元数据执行上下界检查，超界抛 `ValidationError`；这比当前只在 UI 生成控件、后端完全不判定的行为更接近可复现的 typed admission。
- **Codex** 以 JSON Schema 表达工具参数和配置的 minimum/maximum，并在宿主解析/执行边界保留 schema 约束；合法 JSON 类型不等于合法业务值，不能只做 Rust `Value` 类型判断。
- **Claude Code** 的工具参数同样依赖 schema 约束并在执行前报告 validation error；本系统把越界值当作成功输入，导致错误边界无法被审批、日志和重放发现。

#### 影响、后果与根因修复方向

- 触发后，越界重试次数、并发数、top-k、文件大小或路由阈值可以进入条件、工具、HTTP 和 DeepSeek prompt；具体后果取决于下游，但当前 run 成功会掩盖配置错误和资源预算失控。样本未执行外部副作用。
- H-185 只覆盖 JSON 类型错误，H-188 覆盖字符串枚举；H-189 证明合法 number 仍可绕过业务范围，三者需要独立 admission 不变量。
- 根因修复应在 typed `InputPlan` 中解析有限、可比较的 number，定义 inclusive/exclusive（当前 UI 语义为 inclusive）和 NaN/Infinity 不可表示策略；provided 与 default 都必须先过范围检查，错误返回结构化 `InvalidWorkflowInput`。
- 回归需覆盖下界/上界、刚好边界、两侧越界、负数/小数、maximum 小于 minimum、default 越界、恢复/重放和 CLI/SDK/Tauri 三入口，并断言非法值不会到达 provider/工具。本轮只新增审计，不修改生产输入边界。

本轮 H-189 只新增正式审计探针和报告，未修改生产工作流输入边界。

### H-190：工作流 `minLength/maxLength` 不参与 admission，越界字符串成功传播

**级别：中高（P1，输入约束与资源/提示词边界）；触发条件：`start.inputVariables` 声明字符串长度范围，调用方提交范围外的字符串。** 后端不拒绝越界值，仍继续执行并记录成功。

#### 生产调用链与正式复现

- 桌面 `InputVariableDefinition` 与 start schema 公开 `minLength`、`maxLength`，并将二者限定为字符串输入的长度约束。
- Rust `WorkflowDefinition::compile()` 不解析这两个字段，`WorkflowAgent::execute_start()` 直接复制 `self.inputs`；没有字符数/Unicode scalar 数量检查，也没有超限错误。
- 新增正式 ignored 探针 `crates/deepagent-app-core/tests/workflow_audit.rs::audit_workflow_string_length_bounds_are_enforced`。它通过 `ChatService::run_workflow()` 执行 `start → end`，声明 `token` 长度 `[5,8]`，提交单字符字符串，捕获节点事件并读取 `RunStore`。
- 精确命令：`cargo test -p deepagent-app-core --test workflow_audit audit_workflow_string_length_bounds_are_enforced --offline -- --ignored --exact --nocapture`。
- 实际输出：`api_ok=true start_outputs=Some(Object {"token": String("<redacted>")}) run_terminal=Some("succeeded")`；事件层对字符串做了既有脱敏，终态仍证明越界输入未被拒绝，测试以 **0 passed / 1 failed（预期）** 结束。没有模型、工具、网络或文件副作用。

#### DeepSeek 官方契约边界与成熟 Agent 对照

- DeepSeek 官方 Responses 文档只规定 provider 输入 item 与模型参数，不知道本地 workflow 的字符串长度；长度预算、上下文占用和业务字段限制必须由宿主执行。
- **DeepSeek Harness** 的 schemastery `checkWithinRange()` 对 string length 使用 schema metadata 的 min/max，并在 resolver 中抛出 `ValidationError`；长度不是只供表单展示的提示。
- **Codex** 对用户输入和工具 schema 设置明确字节/字符预算，并在宿主边界限制工具输入大小；成熟 harness 会在模型或工具调用前拒绝/截断并留下结构化原因，而不是成功后让下游自行猜测。
- **Claude Code** 的工具和插件 schema 由执行前校验消费，越界参数进入工具前应产生 validation error；本系统目前把单字符值作为成功输入，缺少可观测的 fail-closed 行为。

#### 影响、后果与根因修复方向

- 触发后，过短或过长值可以进入模板、条件、HTTP、工具和 DeepSeek prompt；过长输入会扩大 token/成本，过短输入可能绕过业务标识最小长度。当前样本只证明成功传播，没有执行外部副作用。
- H-190 与 H-189 都是合法 JSON 类型但越过业务范围；H-188 是枚举边界，三类约束应分别在同一 typed plan 中验证，不能只依赖前端控件。
- 根因修复应定义长度单位（建议 Unicode scalar 值，同时另设 UTF-8 byte budget），在 provided/default 解析后统一检查 min/max；超界返回结构化 `InvalidWorkflowInput`，并在日志中只记录长度和字段名，不能回显敏感值。
- 回归需覆盖刚好边界、两侧越界、Unicode 多字节/组合字符、默认值越界、空字符串、恢复/重放和 CLI/SDK/Tauri 三入口，并断言 provider/工具调用前没有非法值。本轮只新增审计，不修改生产输入边界。

本轮 H-190 只新增正式审计探针和报告，未修改生产工作流输入边界。

### H-191：工作流图没有入口节点仍可成功完成，结构 admission 缺少 start/end 不变量

**级别：高（P1，执行语义与成功终态可信度）；触发条件：提交一个没有 `start` 节点的合法 DAG（本次样本只有一个 `end` 节点）。** `compile()` 接受该图，`WorkflowAgent` 直接执行可达的拓扑节点，run 最终记录 `succeeded`。

#### 生产调用链与正式复现

- `WorkflowDefinition` 的图契约把 `start` 作为输入声明节点、`end` 作为完成节点；桌面 registry 也把二者作为工作流入口和出口，而不是普通可选业务节点。
- `compile()` 在遍历节点时只用 `has_start` 检测“最多一个 start”，没有在循环后检查 `has_start == true`，也没有要求至少一个 `end`。没有 start 时，拓扑排序仍然给单节点图生成 order。
- `WorkflowAgent::think()` 以 `compiled.order[self.step]` 执行任意图节点；遇到 `end` 立即返回 `Complete`，即使从未建立输入上下文。若图既没有 end，也可能在 order 用尽后走通用 `workflow completed` 成功分支。
- 新增正式 ignored 探针 `crates/deepagent-app-core/tests/workflow_audit.rs::audit_workflow_requires_a_start_node`。它通过 `ChatService::run_workflow()` 提交只有 `end` 节点的定义，捕获节点事件并读取 `RunStore`。
- 精确命令：`cargo test -p deepagent-app-core --test workflow_audit audit_workflow_requires_a_start_node --offline -- --ignored --exact --nocapture`。
- 实际输出：`api_ok=true node_events=2 run_terminal=Some("succeeded")`；断言无 start 的图必须在 admission 失败，测试以 **0 passed / 1 failed（预期）** 结束。没有模型、工具、网络或文件副作用。

#### DeepSeek 官方契约边界与成熟 Agent 对照

- DeepSeek 官方 [Responses API](https://api-docs.deepseek.com/zh-cn/api/create-response/)只接收已构造的 `input`/`instructions`，不定义本地 DAG 的入口、出口或成功语义；“请求发出并返回 response”不能证明宿主工作流结构有效。
- **Codex** 的 app-server 把 thread/turn 生命周期作为显式 admission：turn 依附已存在 thread，不能凭空把没有上下文的请求标成成功。这里可迁移的原则是先验证运行图的 owner/entry，再允许执行和发布终态。
- **DeepSeek Harness ACP** 在 prompt/close 入口先完成 admission，并要求 owned activity、更新、持久化和 descendants 达到 quiescence 后才 settlement；本系统的图编译却允许缺失入口并立即成功，缺少同等级的结构和收尾门槛。
- **Penguin-Magic** 的 canvas-agent 计划、runtime 和 contract tests 将 agent plan、node run started/completed/failed 作为显式契约；本系统不能把任意拓扑遍历完成解释为有入口的工作流成功。

#### 影响、后果与根因修复方向

- 触发后，调用者可以提交没有输入入口的图，得到 `succeeded` 和完成事件；CLI、SDK、桌面和调度器会把没有真正输入/入口的空执行当作成功，跳过告警、重试和人工检查。当前样本只证明成功终态，不包含外部副作用。
- 缺失 start 会掩盖 H-184 至 H-190 的所有输入校验，因为根本没有 start schema；缺失 end 则会让 `think()` 用“order 用尽”成功分支替代明确的完成节点，重放无法区分正常完成和结构错误。
- 根因修复应在 `compile()` 形成不可变 `WorkflowShape`：强制恰好一个 start、至少一个 end，要求 start 是入度 0、end 是出度 0，并检查从 start 可达且至少有一条路径到 end；不满足时返回结构化 `InvalidWorkflowGraph`，不能让 agent 进入执行。
- 回归需覆盖无 start、无 end、多个 start、多个 end、孤立节点、start 不可达、end 不可达、仅循环/空入口以及 target-node 重跑；所有结构错误必须在模型/工具/网络前失败且不发布 `succeeded`。本轮只新增审计，不修改生产图编译逻辑。

本轮 H-191 只新增正式审计探针和报告，未修改生产工作流图结构。

### H-192：工作流图没有 `end` 节点仍以成功终态结束

**级别：高（P1，终态可信度与调度语义）；触发条件：提交包含 `start` 但没有 `end` 的 DAG。** `WorkflowAgent::think()` 在拓扑 order 用尽后走通用 `workflow completed` 分支，run 仍记录 `succeeded`，没有明确的终点节点或输出契约。

#### 生产调用链与正式复现

- `compile()` 不统计或要求 `end` 节点；只要节点 ID、边和引用合法，只有 start 的图也会生成完整 order。
- `WorkflowAgent::think()` 的 `if self.step >= self.compiled.order.len()` 分支把“所有节点已遍历”直接映射为 `AgentDecision::Complete`，只有在实际遇到 `end` 时才使用 end 输出生成摘要。因此缺 end 的执行与正常 end 完成共用成功终态。
- 新增正式 ignored 探针 `crates/deepagent-app-core/tests/workflow_audit.rs::audit_workflow_requires_an_end_node`。它通过 `ChatService::run_workflow()` 提交只有 `start` 节点的定义，捕获节点事件并读取 `RunStore`。
- 精确命令：`cargo test -p deepagent-app-core --test workflow_audit audit_workflow_requires_an_end_node --offline -- --ignored --exact --nocapture`。
- 实际输出：`api_ok=true node_events=2 run_terminal=Some("succeeded")`；断言缺 end 的图必须在 admission 或终态映射失败，测试以 **0 passed / 1 failed（预期）** 结束。没有模型、工具、网络或文件副作用。

#### DeepSeek 官方契约边界与成熟 Agent 对照

- DeepSeek Responses 只定义 provider response 的完成/失败事件，不知道宿主 DAG 的 end 节点；把所有拓扑节点跑完不能替代工作流自己的输出 contract。参考 [Responses 流式终态说明](https://api-docs.deepseek.com/zh-cn/guides/responses_api)。
- **Codex** 的 turn settlement 由明确的 turn 状态和终态事件驱动，不能仅因某个内部列表耗尽就把一次没有最终结果的 turn 当成成功；本系统应把 end 作为同样明确的 settlement boundary。
- **DeepSeek Harness ACP** 的 prompt/close 只有在 owned activity、更新、持久化和 descendants 达到 quiescence 后 settlement；它不会把“队列空了”直接等价于业务成功。这里可迁移的是终态必须由显式契约决定。
- **Penguin-Magic** 的 node-canvas agent 事件契约区分 node run started/completed/failed 与 agent 终态，并以 agent plan/runtime 的显式阶段收束；本系统缺 end 时却复用普通成功 summary，导致事件流无法说明是否生成了最终输出。

#### 影响、后果与根因修复方向

- 触发后，只有初始化输入、没有业务输出的图会显示成功；调度器可能把它视为已完成并跳过重试、告警或后续依赖，SDK/CLI 也无法区分“正常 end 输出”和“拓扑遍历结束”。
- H-191 证明缺 start 的图可成功，H-192 证明缺 end 的图也可成功；两者合在一起说明现有成功条件是“能遍历 order”，不是“满足工作流协议”。
- 根因修复应在 `compile()` 强制至少一个 end，并要求从唯一 start 可达的 end；运行时移除或改为结构错误的 order-exhausted success 分支，只有 end executor 成功后才能发布 succeeded。没有 end 的重放应落 `failed`/`invalid_graph`，而不是伪造答案。
- 回归需覆盖 start→end 正常图、start-only、end-only、孤立 end、多个 end、不可达 end、target-node 重跑和恢复/重放，并断言唯一成功终态都由 end 输出触发。本轮只新增审计，不修改生产图编译逻辑。

本轮 H-192 只新增正式审计探针和报告，未修改生产工作流图终态。

### H-193：start 与 end 彼此不可达时仍按全图拓扑执行并成功

**级别：高（P1，控制流隔离与成功语义）；触发条件：工作流同时包含合法的 `start` 和 `end`，但 end 不在 start 的可达子图中。** 当前编译器把所有入度为零的节点放入 ready 集合，执行器遍历整个 DAG，而不是只执行从 start 出发的节点。

#### 生产调用链与正式复现

- `compile()` 为所有节点计算拓扑 order：初始 ready 是全图所有 indegree=0 的节点；没有基于唯一 start 做可达性筛选，也不拒绝孤立组件。
- `WorkflowAgent::think()` 完全依赖该 order。由于 input order 先放 start、后放 end，本次两个无边根节点依次执行；end 即使没有 start ancestor，仍触发 `AgentDecision::Complete` 和成功终态。
- 新增正式 ignored 探针 `crates/deepagent-app-core/tests/workflow_audit.rs::audit_workflow_requires_a_reachable_end_node`。它通过 `ChatService::run_workflow()` 提交两个节点、零边的图，捕获节点事件并读取 `RunStore`。
- 精确命令：`cargo test -p deepagent-app-core --test workflow_audit audit_workflow_requires_a_reachable_end_node --offline -- --ignored --exact --nocapture`。
- 实际输出：`api_ok=true node_events=4 run_terminal=Some("succeeded")`；start/end 各发布 running/completed，断言不可达 end 不能产生成功终态，测试以 **0 passed / 1 failed（预期）** 结束。没有模型、工具、网络或文件副作用。

#### DeepSeek 官方契约边界与成熟 Agent 对照

- DeepSeek Responses 的 provider 完成事件只说明一次模型请求结束，不能证明宿主 DAG 中 end 是由 start 输入驱动；可达性必须在本地图 admission 建立。
- **DeepSeek Harness workflow** 不把任意节点图交给通用拓扑遍历：它解析一个具有明确顶层脚本入口和最终 `return` 的运行，格式错误在 run publication 前失败；`workflow/start`/`workflow/end` 配对且 stop reason 明确。可迁移原则是“入口拥有整个执行范围，终态来自该入口的 settlement”。
- **Codex** 的 agent tree 使用显式 root/parent identity 注册和恢复，子运行必须属于 root tree；不属于当前 root 的活动不会因为存在于共享 registry 就自动参与当前 turn。这里可比的是 root reachability/ownership，而不是照搬多 Agent API。
- **Penguin-Magic** 的 canvas-agent plan/runtime 明确把计划步骤和 node run 事件绑定到一次 agent plan；孤立画布节点的存在不应自动变成当前运行的业务步骤。本系统当前把 definition 中所有根都视为可执行，扩大了运行范围。

#### 影响、后果与根因修复方向

- 触发后，孤立的 end 可以让未连接主流程的图成功；更危险的是孤立工具、HTTP、模型或媒体节点也可能被全图遍历执行，即使用户没有从 start 连到它们。当前样本未包含外部副作用，只证明执行范围扩张和成功终态。
- H-191/H-192 是节点缺失；H-193 在节点齐全时仍失败，根因是编译计划没有 start-rooted reachability。仅补“必须存在 start/end”不能修复本项。
- 根因修复应从唯一 start 做 DFS/BFS，编译 `reachable_order`；所有 end 必须属于该集合，定义中的不可达节点应明确拒绝（推荐）或被版本化标为 disabled，不能静默执行。成功只允许来自 reachable end。
- 回归需覆盖两个独立组件、孤立副作用节点、start→end 正常图、多个可达 end、不可达 end、条件分支、target-node 重跑和恢复/重放；断言不可达节点没有 running/completed 事件、没有 provider/工具/网络调用。本轮只新增审计，不修改生产图规划逻辑。

本轮 H-193 只新增正式审计探针和报告，未修改生产工作流可达性处理。

### H-194：拓扑等价图仅改变节点数组顺序，执行范围便发生变化

**级别：高（P1，确定性、重放与副作用边界）；触发条件：定义同时包含可达的 `start → end` 主路径和至少一个孤立根节点，并且编辑器保存、导入或迁移改变节点数组顺序。** 当前稳定拓扑排序以节点数组下标解决 ready 并列；`WorkflowAgent::think()` 一执行到任何 `end` 就立即返回 `Complete`，不会结算 order 中剩余节点或发布 skipped 事件。因此数组中的 `end` 相对位置决定孤立节点是被执行，还是完全没有事件。

#### 生产调用链与正式复现

- `compile()` 用 `BTreeSet<usize>` 保存 ready 节点，始终取最小数组下标；当 `start` 完成后，可达 `end` 与孤立根同时 ready，谁的下标小谁先执行。
- `WorkflowAgent::think()` 在 end 节点 completed 后立刻返回 `AgentDecision::Complete`。order 后续条目不会再被调用，也不会转换为 skipped/cancelled，因此持久化事件无法说明它们为何没有运行。
- 新增正式 ignored 探针 `crates/deepagent-app-core/tests/workflow_audit.rs::audit_workflow_execution_scope_is_not_node_order_dependent`。两份定义具有完全相同的节点 ID、kind、config 和边 `start → end`，唯一差异是 `end` 与孤立 `variable-assigner` 在 `nodes` 数组中的位置。
- 精确命令：`cargo test -p deepagent-app-core --test workflow_audit audit_workflow_execution_scope_is_not_node_order_dependent --offline -- --ignored --exact --nocapture`。
- 实际输出：第一份定义 `end_first=["start", "end"] terminal=Some("succeeded")`；第二份定义 `orphan_first=["start", "orphan", "end"] terminal=Some("succeeded")`；完成事件集合不相等，测试以 **0 passed / 1 failed（预期）** 结束。探针只执行 start/end/空 assignments 节点，没有模型、工具、网络、命令或文件副作用。

#### 成熟 Agent 对照与 DeepSeek 边界

- DeepSeek Responses 只结算单次 provider response，不能为宿主工作流定义“还有哪些节点应当执行”。DeepSeek Harness 的 PTC workflow 用显式脚本控制流和最终 return/throw 结算；其 `guest.spec.ts` 还验证最后 progress 未确认前不能发布脚本 settlement。也就是说，终态由同一程序控制流和已确认事件产生，不由无关容器顺序抢跑。
- Codex `agent-graph-store/src/store.rs` 要求 list 结果有稳定顺序，但 descendant 查询仍从明确 `root_thread_id` 沿有向 parent/child 边遍历，且 status filter 应用于每条 traversed edge；稳定排序只影响展示/合并，不能扩大或缩小 root 的成员集合。本系统反而让 tie-break 顺序改变执行成员集合。
- Penguin-Magic 的 canvas agent 先把模型结果验证成带 step/operation 的计划，`collectOperations()` 按步骤展开明确操作，再发布同一次 `canvas.patch` 后才发 `session.completed`。它的数组顺序是显式计划语义；DeepAgent 当前节点数组只是图的存储表示，却被隐式升级成终态与执行范围语义。

#### 影响、后果与根因修复方向

- 同一拓扑图在导入导出、前端重新序列化或迁移后可能执行不同节点；若孤立节点将来是工具、HTTP、模型或媒体节点，保存顺序可能决定是否发生外部调用。动态样本只证明无副作用节点的执行差异，外部副作用风险来自同一正式 dispatch 分支的代码推导，未冒充动态证据。
- `end` 之后的节点没有 terminal/skipped 事件，但 run 为 succeeded；CLI/SDK/UI 的 replay 无法区分“有意不在执行计划”“被 end 提前截断”和“事件丢失”，破坏可审计性。
- H-193 证明不可达节点被错误纳入 order；H-194 进一步证明即便错误 order 已生成，实际执行集合还会被 end 的相对下标二次裁剪。仅做 reachability 能消除本样本，但执行器仍应建立不依赖序列化顺序的 settlement 不变量，防止多 end、分支 merge 和 target-node 模式复发。
- 根因修复应在编译阶段生成从唯一 start 出发的 typed execution plan，拒绝不可达节点；定义一个可验证的唯一成功终点，或明确规定多个终点的 join/choice 语义。运行终态必须在计划内所有必需节点均 settled 后发布，未运行节点必须有带原因的 skipped 事件。回归至少对节点数组排列做 property test，并覆盖多 end、并行分支、条件分支、target-node、恢复与 replay。

本轮 H-194 只新增正式审计探针和报告，未修改生产工作流调度或终态逻辑。

### H-195：多个可达 end 没有 reject/join/choice 语义，第一个 end 截断正常分支

**级别：高（P1，并行/分支工作流完整性与错误成功）；触发条件：从唯一 start 出发存在两条或更多静态可达路径，并且至少两条路径各自终止于 end。** 编译器只限制 start 至多一个，不限制 end 数量；执行器也没有 join 或“条件互斥终点”计划，任一 end completed 就结束整个 run。

#### 生产调用链与正式复现

- `graph::compile()` 用 `has_start` 拒绝第二个 start，但没有 `has_end`/end count 校验；只限制 end 不能有出边。因此多个 end 会进入同一个 `CompiledWorkflow.order`。
- `WorkflowAgent::think()` 不检查剩余 reachable 节点、分支状态或 end 身份；节点 kind 为 `end` 时立即返回 `AgentDecision::Complete`。AgentKernel 收到该决策后写入成功终态，order 中其余可达节点不再获得运行机会。
- 新增正式 ignored 探针 `crates/deepagent-app-core/tests/workflow_audit.rs::audit_workflow_does_not_succeed_with_a_reachable_branch_unsettled`。图为 `start → left → left-end` 与 `start → right → right-end` 两条普通默认边，两个分支都从 start 静态可达；节点只用空 assignments，无外部能力。
- 精确命令：`cargo test -p deepagent-app-core --test workflow_audit audit_workflow_does_not_succeed_with_a_reachable_branch_unsettled --offline -- --ignored --exact --nocapture`。
- 实际输出：`api_ok=true terminal_events=[("start", Completed), ("left", Completed), ("left-end", Completed)] run_terminal=Some("succeeded")`；`right` 和 `right-end` 没有 completed/skipped/failed/cancelled，断言“成功 run 不能留下静态可达且未结算的分支”以 **0 passed / 1 failed（预期）** 结束。没有模型、工具、网络、命令或文件副作用。

#### 成熟 Agent 对照与 DeepSeek 边界

- DeepSeek Responses 的 `completed` 只能表示一次模型响应完成，不能替宿主确认所有 workflow branch 已结算。DeepSeek Harness PTC 的 `parallel()` 是显式组合子；`guest.spec.ts` 在并发上限为 1 时仍等待三个 child 依次完成，最终才以完整结果数组和 `stopReason=completed` 结算，不会把第一个 child 完成当作整个 parallel 完成。
- Codex 的持久化 agent graph 从 root 沿 parent/child 边列举 descendants，并把 status filter 应用于每条 traversed edge；子任务的归属和生命周期是显式边。它没有用“稳定排序中第一个完成的 descendant”替代整棵 root tree 的状态。可迁移原则是先定义聚合语义，再从成员状态推导父级终态。
- Penguin-Magic 的 canvas agent 计划把所有 step operations 展开并作为一个 `canvas.patch` 发布，之后才发 `session.completed`；计划中的操作集合由 validated plan 决定。它不是通用 DAG join 引擎，但至少不会因为某个画布节点名为 end 就让同计划后续 operation 从事件流中消失。

#### 影响、后果与根因修复方向

- 触发后，批量处理、并行检索、多个媒体生成、左右审阅或“成功/失败各自终点”的画布可能只跑数组顺序靠前的一支，却整体显示成功。未运行分支没有终态事件，自动化无法补偿，用户也无法从 replay 识别截断。
- 若前一分支只生成草稿而后一分支负责校验、保存或发布，系统会在校验/落盘前宣布完成；若前一分支有外部副作用而后一分支负责清理，清理也不会执行。当前样本仅动态证明空节点分支被截断，没有执行真实外部副作用。
- H-194 的后续节点是孤立根；H-195 的右分支由 start 明确连接并静态可达，因此不能只用“拒绝孤立节点”修复。必须决定协议：v1 最小安全方案是要求恰好一个 end，且所有可执行路径汇入它；若支持多个 end，则图计划必须把它们标成显式 choice 或 join，choice 由条件边选择且未选分支产生带原因 skipped，join 则等待所有必需 predecessor/branch settled。
- 成功终态需要从 typed plan 的 settlement 计算，不能在节点 executor 内看到 `kind=end` 就直接返回全局 Complete。回归应覆盖双分支单 end join、双 end 并行、条件互斥双 end、某分支失败/取消、节点数组排列、target-node 与恢复重放，并断言每个计划成员恰好一个 terminal event。

本轮 H-195 只新增正式审计探针和报告，未修改生产工作流终点或分支语义。

### H-196：合法 target-node 运行跳过必需祖先，正常节点因缺少中间输出失败

**级别：高（P1，节点重跑/恢复可用性与数据依赖完整性）；触发条件：用户点击任一依赖非 start 中间节点输出的节点运行按钮。** 桌面把该节点 ID 作为 `target_node_id` 提交完整图；运行时保留 start、目标及目标的后代，却把目标的其他祖先全部跳过。请求中没有上一轮节点输出快照，因此目标引用必然缺失。

#### 生产调用链与正式复现

- `ConfigForms.tsx` 等节点面板直接调用 `runWorkflow(nodeId)`；`workflowExecutor.ts` 重置所有节点状态，把当前图、start 默认输入和 `target_node_id` 发送给 `start_workflow`，没有附带已完成节点输出、对应 workflow revision 或 artifact snapshot。
- `WorkflowAgent::think()` 的过滤条件方向相反于依赖闭包：对非 target、非 start 节点，只有“当前节点的 ancestors 包含 target”才执行。因此 target 的 descendants 被保留，而 target 自己依赖的 intermediate ancestors 被标记 `Skipped`。
- 编译器已经验证 target 对祖先输出的引用合法，但执行时 `values::resolve()` 只查本次 `self.outputs`。被跳过祖先没有输出，target 以 `workflow reference: required output is unavailable` 失败。
- 新增正式 ignored 探针 `crates/deepagent-app-core/tests/workflow_audit.rs::audit_workflow_target_node_keeps_required_ancestor_outputs`。控制图为 `start → source(variable-assigner) → target(list-operator) → end`，target 显式引用 `{{#source.output#}}`；先整图运行，再用同一定义指定 target。
- 精确命令：`cargo test -p deepagent-app-core --test workflow_audit audit_workflow_target_node_keeps_required_ancestor_outputs --offline -- --ignored --exact --nocapture`。
- 实际结果：控制组 `full_ok=true`，target 输出 `result=["SOURCE_VALUE"]`，run=`succeeded`；目标组将 source 标记 `Skipped`，target 标记 `Failed`，错误为 `required output is unavailable`，run=`failed`；断言合法 target 运行必须保留所需 ancestor 数据，以 **0 passed / 1 failed（预期）** 结束。没有模型、工具、网络、命令或文件副作用。

#### 成熟 Agent 对照与 DeepSeek 边界

- DeepSeek Responses 的响应续接能力不能替代本地 DAG 节点输出、artifact 与 revision；本项目若调用 provider，也必须先由 Harness 构造完整目标输入。把缺失依赖留到 provider/节点执行时报错不是 DeepSeek 兼容策略。
- DeepSeek Harness 的 continuation 基准先持久化 session history，再以同一 durable session identity resume，并验证恢复后的 history/tool continuation；它选择“恢复已验证状态”。其 workflow 文档又明确当前脚本没有 journaling/resume，而不是暴露一个看似可重跑、实际丢上下文的入口。
- Codex 的 resume 会 materialize/读取 thread rollout；app-server 对不存在的 thread 直接报错。其核心原则是恢复输入来自对应持久化身份，不能仅凭一个目标 ID 构造缺历史的“续跑”。对本系统可迁移为 `(workflow_revision, node_id, dependency_snapshot)` 的显式身份。
- Penguin-Magic 将 `runNodeIntent` 定义成独立画布操作，并为该节点发布 `node.run.started/completed/failed`；计划架构说明它延迟交给既有节点执行链。它不是本系统的 DAG 重跑实现，但至少把“运行一个节点的 intent/draft”与“裁剪整图后假设输入仍存在”分开建模。

#### 影响、后果与根因修复方向

- 所有消费中间 LLM、工具、检索、模板、媒体或变量节点输出的下游节点，单节点运行都可能稳定失败；若节点恰好只依赖 start 或常量则成功，造成看似随机的功能差异。用户会误以为节点本身、provider 或 artifact 损坏。
- source 被发布为 Skipped、target 为 Failed，整次 run 为 failed，但 UI 的节点按钮没有说明这是缺少依赖闭包；反复点击只会重复同一失败。若未来静默用 null/default 补齐，则风险会从明显失败转为错误成功。
- H-177 是不存在 target 未拒绝；H-196 是 target 确实存在、编译引用也合法，却因执行计划方向错误而失败。校验 ID 不能修复依赖缺失。
- 根因修复需先明确两种模式并版本化：`recompute` 从 start 执行 target 的全部必需 ancestors 后在 target 处停止（或按产品定义继续 descendants）；`resume` 则必须加载同一 workflow revision、节点配置 digest、上游 outputs/artifacts 和权限快照，任何 stale/missing dependency fail closed。当前请求没有这些字段，最小可行方案是生成 ancestor closure，而不是当前 descendant filter。
- 回归需覆盖 target 依赖 start、单层/多层祖先、分支祖先、工具/模型/artifact 输出、祖先失败或取消、revision 变化、输出已过期、target 为 start/end、恢复重放及 UI/CLI/SDK 一致性；断言每个跳过原因和数据来源均可审计。

本轮 H-196 只新增正式审计探针和报告，未修改生产 target-node 执行或状态恢复逻辑。

### H-197：end 的输出类型声明只校验类型名称，成功结果可违反公开契约

**级别：高（P1，机器协议可靠性与下游数据完整性）；触发条件：end 的输出引用解析为与 `outputVariables[].type` 不一致的 JSON 值。** 工作流定义向 UI、持久化数据和未来 Harness/SDK 暴露类型声明，但运行时把它当作目录元数据：即使声明 `number`，实际 string 仍以成功终态交付。

#### 生产调用链与正式复现

- `deepagent-runtime::workflow::graph::output_names()` 在编译图时调用 `validate_type()`，只确认声明的类型字符串属于支持集合并收集输出名；它既不保存可供运行时执行的输出 schema，也不检查实际值。
- `WorkflowAgent::execute_end()` 读取 `outputVariables[].variable/value`，调用 `values::resolve()` 后直接插入结果。该路径不读取同一声明的 `type`，节点完成与 run 终态也没有后置 schema gate。
- 新增正式 ignored 探针 `crates/deepagent-app-core/tests/workflow_audit.rs::audit_workflow_end_output_matches_declared_type`。定义同时提供 `name` 与 `variable`，绕开 H-40 已记录的跨层字段漂移，仅测试类型约束：start 输入 `raw:string`，end 声明 `count:number`，值引用 `{{#start.raw#}}`，请求输入为 `"NOT_A_NUMBER"`。
- 精确命令：`cargo test -p deepagent-app-core --test workflow_audit audit_workflow_end_output_matches_declared_type --offline -- --ignored --exact --nocapture`。
- 实际结果：`api_ok=true`、`end_status=Some(Completed)`、`end_outputs={"count":"NOT_A_NUMBER"}`、`run_terminal=Some("succeeded")`；断言要求拒绝运行、end 失败或 `count` 为 JSON number，以 **0 passed / 1 failed（预期）** 结束。探针没有模型、工具、网络、命令或文件副作用。

#### DeepSeek 官方语义与成熟 Agent 对照

- DeepSeek Responses 官方参数文档把 `text.format.type=json_schema` 描述为按提供的 JSON Schema 生成结构化响应；本项的 end 是宿主本地工作流输出，不会由 provider 替宿主校验。因此可迁移原则不是把 Responses 字段生搬到 DAG，而是在宿主承诺结构化契约的最终交付边界执行等价的值/schema 验证。
- DeepSeek Harness 的 `packages/core/tools/src/index.ts` 为每个成功工具值调用 `validateJsonSchemaValue(tool.output.schema, detached, 'value')`；有 violation 时拒绝规范化成功结果。`packages/core/tools/src/json-schema.ts` 提供实际 schema/value 验证器。其直接对象是工具输出而非工作流 end，但证明同一成熟 Harness 将输出 schema 视为运行时约束，不是只验证 schema 本身语法。
- Codex `hooks/src/engine/output_parser.rs::structured_output_rejects_invalid_shapes_and_types` 覆盖错误字段类型和形状，`command_runner_tests.rs::async_hook_marks_invalid_structured_output_as_failed` 进一步断言异步结构化输出不匹配时状态为 Failed。其 hook 协议与本系统 workflow 不同；可迁移不变量是机器消费的结构化结果必须先验证，再发布成功。
- Penguin-Magic 的计划执行会先验证计划对象和 operation 支持集合，但未发现其提供可直接引用的通用 DAG end 输出类型验证；因此不把它写成已解决本项，只作为“机器对象不可无验证进入执行链”的弱对照。

#### 影响、后果与根因修复方向

- CLI JSONL、未来 stdio Harness/TypeScript SDK、工作流嵌套调用和自动化消费者会按声明生成类型或反序列化；一次类型漂移即可造成客户端解码失败、错误分支、数值计算异常或把本应失败的 run 继续传给后续副作用节点。
- UI 若把输出声明用于组件选择或格式化，错误成功可能表现为渲染异常；更严重的是持久化 replay 仍会忠实重放这个“成功的错误值”，使在线与恢复后都稳定错误，而非瞬时展示问题。
- H-185 检查 start 输入的实际类型；H-197 检查 end 对外输出的实际类型。两者共享“声明未成为 runtime contract”的设计根因，但生产边界、受影响消费者和失败后果不同；只修输入 admission 不能修复输出漂移。
- 根因修复应把工作流变量类型编译成单一 `ValueContract`/JSON Schema 表示，由 start admission、节点输出和 end publication 共用；end 在发布 Completed 和 run succeeded **之前** 验证完整对象，错误必须带 node ID、变量名、expected/actual type 并进入唯一 Failed 终态。不要只在 UI 或 SDK 解码处补校验。
- 若产品希望字符串到数字等隐式转换，必须在定义中版本化并形成确定的 coercion 规则；默认应 fail closed，避免不同宿主各自转换。回归需覆盖全部标量、array/object/null、引用值、常量值、缺失 required、额外输出、嵌套 schema、重放/恢复，以及 UI/CLI/SDK 对同一失败的统一投影。

本轮 H-197 只新增正式审计探针和报告，未修改生产工作流类型检查或终态逻辑。

### H-198：OR 条件组从 true 开始累加，所有子条件为 false 仍选择 true 分支

**级别：高（P1，确定性控制流正确性；当前执行范围后果被 H-39 部分掩盖）；触发条件：if-else 使用后端当前支持的 `logicalOperator="or"`，且同组所有比较结果均为 false。** 条件节点不是模型判断，而是宿主确定性逻辑；相同输入必然错误输出 true，不存在采样或供应商差异。

#### 生产调用链与正式复现

- `WorkflowAgent::execute_if_else()` 对每组条件统一执行 `let mut group_result = true`；AND 使用 `group_result && cmp` 时 true 是正确单位元，但 OR 随后执行 `group_result || cmp`，第一次比较前已经为 true，因此任何合法 OR 组都无法变回 false。
- 函数还读取了顶层 `logicalOperator` 却只赋给未使用变量；本项只登记已经动态证明的组内 OR 单位元错误，不把多组组合语义一并写成已复现问题。
- 新增正式 ignored 探针 `crates/deepagent-app-core/tests/workflow_audit.rs::audit_workflow_or_condition_does_not_start_from_true`。为与 H-40 去重，fixture 不使用桌面的 `logic/items/variable` 字段，而直接使用后端当前实际消费的 `logicalOperator/conditions/left/right`；两个比较分别为 `false is true` 与 `0 > 1`，都应为 false。测试只读取条件节点自己的 Completed 输出，不以 H-39 已失效的边派发作为判据。
- 精确命令：`cargo test -p deepagent-app-core --test workflow_audit audit_workflow_or_condition_does_not_start_from_true --offline -- --ignored --exact --nocapture`。
- 实际结果：`api_ok=true`、`decision=Some(String("true"))`、`run_terminal=Some("succeeded")`；断言期望 `branch="false"`，以 **0 passed / 1 failed（预期）** 结束。无模型、工具、网络、命令或文件副作用。

#### 成熟 Agent/工作流实现对照与 DeepSeek 边界

- Dify 的公开 if-else 节点定义把逻辑操作符收敛为 `and | or` 枚举，并在节点校验中拒绝空 case、空 conditions、缺变量或缺比较操作符；其 workflow builder 也生成明确的 `logical_operator`、`conditions` 与分支 handle。这里引用的是成熟 Agent 工作流的配置/admission 做法，不据此前端代码臆称其后端求值实现细节。
- DeepSeek Harness 没有与本画布同构的 if-else DAG 节点；其可迁移原则来自 Schemastery 和工具 runtime：有限集合字段先在统一 schema 中解析，成功结果再经过可执行验证器。不能让 `or` 等已声明枚举落到静默默认或未覆盖分支。DeepSeek Responses 也不参与本项，本地控制流必须在调用模型之前确定并可测试。
- Codex 同样没有可直接对照的通用画布条件节点，因此本项不伪造“Codex 已用某函数解决”。可迁移的成熟工程做法是把条件求值器做成独立纯函数，以 AND 全真、OR 任一真为真，并对 0/1/N 条件和所有操作符做表驱动测试；调度器只消费已验证的 branch ID。

#### 影响、后果与根因修复方向

- 当前 H-39 会无视 branch 并实际派发未选路径，所以本样本的错误 decision 没有额外减少或增加执行节点；它仍污染节点事件、调试视图和任何直接读取该结果的消费者。不能据此声称本轮已观察到仅 true 路径产生网络/工具副作用。
- 修复 H-39 后若遗漏本项，任意全 false 的 OR 条件都会稳定进入 true 分支，可能调用错误模型、工具、HTTP 或人工审批路径；run 仍成功，审计记录还会把错误决策当作正常业务选择。两个问题必须各自有回归，避免修复调度后才暴露条件求值缺陷。
- H-40 是 UI 与 Rust 条件字段漂移；H-39 是正确 decision 未约束派发；H-198 是即使直接提供后端原生字段，decision 本身仍算错。修复任意一项都不能替代另两项。
- 根因修复应提取版本化 `ConditionGroup` DTO 和纯 evaluator：`and => comparisons.all(...)`、`or => comparisons.any(...)`，明确空组是 admission error 而不是借助布尔单位元默认为真/假；拒绝未知 logical operator，不再使用 `_ => and`。删除当前未使用的顶层字段或定义多 case 的 first-match 语义，并统一前端字段迁移。
- 回归需覆盖 OR 的 FF/FT/TF/TT、AND 的同表、空组、单条件、多组 first-match/else、未知操作符、比较错误、引用缺失，以及在 H-39 修复后的真实分支节点执行集合；事件、UI、CLI/SDK 和 replay 必须报告同一 branch ID。

本轮 H-198 只新增正式审计探针和报告，未修改生产条件求值或分支调度。

### H-199：多 case 条件删除 false 端口，但 executor 无匹配时仍返回 false

**级别：高（P1，控制流完整性与协议 admission）；触发条件：if-else 定义两个以上 case，并需要在所有 case 均不匹配时连接 ELSE 路径。** UI 不渲染 `false` source handle，Rust 编译器也拒绝协议调用者显式提交该边；executor 自己却把 `false` 作为无匹配时的规范结果，生产方与消费者的有限集合不闭合。

#### 生产调用链与正式复现

- `WorkflowNodeShell.tsx::getNodeSourceHandles()` 在 conditions 数量不超过 1 时返回 `true/false`，超过 1 时只返回每个 condition ID；因此桌面用户无法从多 case 节点连接 ELSE。
- `workflow::graph::source_handles()` 复制同一规则：`conditions.len() > 1` 时只返回 group IDs。任何导入、CLI 或未来 Harness 请求即使手写 `source_handle="false"`，也会在编译期被 `requires a valid source handle` 拒绝。
- `WorkflowAgent::execute_if_else()` 遍历所有组后如果没有命中，明确返回 `{"__branch":"false"}`；`node_output_value_inline()` 再公开为 `{"branch":"false"}`。这是 executor 可产生但图协议无法消费的状态。
- 新增正式 ignored 探针 `crates/deepagent-app-core/tests/workflow_audit.rs::audit_workflow_multi_case_condition_keeps_else_handle`：两个 AND case 分别检查 `green == red` 与 `green == blue`，均为 false；图显式把 `false` handle 连到 end，经正式 `ChatService::run_workflow` 提交。
- 精确命令：`cargo test -p deepagent-app-core --test workflow_audit audit_workflow_multi_case_condition_keeps_else_handle --offline -- --ignored --exact --nocapture`。
- 实际结果：`api_ok=false`、错误 `edge e2 requires a valid source handle for node decide`、`run_terminal=Some("failed")`；断言多 case 必须允许 executor 的 false fallback，以 **0 passed / 1 failed（预期）** 结束。失败发生在 admission，无模型、工具、网络、命令或文件副作用。

#### 成熟 Agent/工作流实现对照与 DeepSeek 边界

- Dify 的公开 if-else 节点结构把可连接 `_targetBranches` 与求值 `cases` 分开；默认显式保留 `true` 与 `false` 分支，builder 也把 case ID 作为 source handle 公开。其 `checkValid` 拒绝空 case/conditions。可迁移原则是从统一的 branch descriptor 同时生成编辑器 handle、图校验与 executor 结果，且 fallback 是一等分支；本报告不从前端源码推断其后端的全部调度语义。
- DeepSeek Harness 与 Codex 都没有同构画布节点，因此不伪造直接实现对照。它们可迁移的协议原则是有限集合由单一 typed source 定义，生产方不能发出 consumer schema/enum 不接受的值；本项在调用 DeepSeek 前已经失败，和 Responses 参数、缓存或模型推理无关。
- 本系统已经有 `source_handles()` 作为静态集合，问题是 executor 另写了一套 branch 产生规则。成熟设计应让编译后的 node contract 同时携带合法输出 IDs 与 fallback，executor 只能返回该集合成员，UI 也从同一版本化描述投影。

#### 影响、后果与根因修复方向

- 当前桌面用户无法表达标准 IF/ELIF/ELSE；协议调用者尝试补上 ELSE 会让整个 run 在执行前失败。若省略 ELSE，无匹配决策没有合法下游语义；同时 H-39 目前还会错误执行其他路径，进一步掩盖缺口，不能把“图仍有输出”当作 fallback 已工作。
- H-40 会先删除桌面 conditions，H-39 会忽略选中 handle，H-198 会算错 OR，H-199 则是即使字段、求值和派发都修复，多 case 的可产生结果仍无法连线。四项需要分层回归，不能用一次条件节点重写后只测 happy path。
- 若未来调度器严格按 handle 派发而不修本项，无匹配时可能没有 successor、end 不可达或被错误结算成功；若改为默认取最后一个 case，又会把显式条件 case 偷偷当 ELSE。fallback 必须在协议中有独立 ID、显示标签和 replay 事件。
- 根因修复应定义唯一 `BranchContract { cases, fallback }`：UI、serializer、compiler 和 evaluator 全部投影它。最小兼容方案是在任意 case 数量下始终加入保留的 `false` handle，并禁止 case ID 与 `false/true` 保留字冲突；若采用 Dify 风格动态 case IDs，则迁移旧 `true/false` 图并版本化。
- 回归需覆盖 0/1/N case、全部不匹配、首个/中间/末个匹配、fallback 接线、case ID 重复或保留字冲突、导入旧图、H-39 修复后的执行集合、节点事件与重放，以及 UI/CLI/SDK 对同一 branch 目录的一致投影。

本轮 H-199 只新增正式审计探针和报告，未修改生产 source handle、条件执行或 UI。

### H-200：Responses 生命周期事件丢弃响应身份、服务端模型与序号，durable run 无法关联完整流

**级别：中（P2，运行可观测性/流完整性诊断缺口）；触发条件：Responses provider 发出正常 lifecycle 事件，调用方需要在断线补读、问题排查或 provider 行为核对时把同一 response 的 created、item 和 completed 事件关联起来。** 本项没有证明当前模型回答或工具执行错误；它证明正式持久化协议无法恢复 provider 已提供的 correlation/progression 元数据。

#### 生产路径与动态证据

- DeepSeek 官方 [Responses API 指南](https://api-docs.deepseek.com/zh-cn/guides/responses_api/) 规定每个 SSE 事件都有单调递增的 `sequence_number`，流以 `response.completed`、`response.incomplete` 或 `response.failed` 结束；response 对象有唯一 `id`。官方 [Responses 参数文档](https://api-docs.deepseek.com/zh-cn/api/create-response/) 在 response schema 中定义 `id`、`model` 与终态。本探针没有断言必须校验序号连续性，只检查公开元数据是否被保留。
- 当前链路为 provider SSE → `ResponseAccumulator::push_sse_data_observed()` → `ModelStreamEvent::ResponseStreamEvent` → `SinkObserver` → `RuntimeEvent::ResponsesStreamEvent` → `PersistentEventSink` → `run_events`。从 Models 到 Runtime 的两个 DTO 只承载 `event_type/item_id/item_type/delta_chars`；累加器构造事件时没有读取顶层 `sequence_number`，也没有读取 `response.created/completed` 的嵌套 `response.id` 与 `response.model`。持久层对 `delta_chars` 存在的 metadata 事件还会跳过落盘；created/completed 因没有 `delta_chars`，持久化后仅有 `type=responses_stream_event` 与 `event_type`。
- 新增 opt-in 正式 AppCore 探针 `native_output_items::audit_responses_lifecycle_retains_provider_identity_and_sequence`。底层受控 transport 给同一个正常响应提供 `response.created`（`id=audit-response`、`model=deepseek-flash`、序号 0）、完整 message item 与连续事件序号，以及 `response.completed`（同 ID/model、序号 7、usage）；随后走正式 `ChatService → ModelClient → ResponseAccumulator → ModelAgent → Kernel → PersistentEventSink → RunStore`，从真实 `run_events` 读取 lifecycle 事件。执行 `cargo test -p deepagent-app-core --test kernel_v2_e2e native_output_items::audit_responses_lifecycle_retains_provider_identity_and_sequence --offline -- --ignored --exact --nocapture` 得到 **0 passed / 1 failed（预期）**：实际 durable lifecycle 为 `[{"event_type":"response.created","type":"responses_stream_event"},{"event_type":"response.completed","type":"responses_stream_event"}]`，两条都缺 `response_id`、`provider_model` 和 `sequence_number`；run 本身正常完成。没有公网请求、网络故障或工具副作用。

#### 成熟实现对照

- **Codex** `codex-api/src/sse/responses.rs` 将 `response.created` 解析为 `ResponseEvent::Created { response_id }`，completed/incomplete 解析为 `ResponseEvent::Completed { response_id, ... }`；stream event parser 还单独读取服务端 model metadata 并向下游发 `ServerModel`。因此其运行事件/response 边界保留了响应身份和实际模型，不把协议终态压成一个无身份的 event type。本次源码比对没有找到 Codex 对 DeepSeek `sequence_number` 做严格递增验证的证据，故不将该点记为已解决。
- **DeepSeek Harness** 中与本次 Responses SSE DTO 同构的可验证实现/测试不足，未据此声称它已持久化 provider response identity。对 DeepSeek wire 的判据以官方文档为准，成熟实现对照只引用 Codex 已检查的代码。
- 这是可观测性/协议投影原则的差异，不代表 DeepSeek 与 OpenAI/Codex 的其它响应字段或事件语义相同。

#### 后果、修复方向与验证限制

- 正常流的 UI 文本、native output item 和 usage 仍可成功交付；因此当前没有证据表明本项单独改变模型答案。实际后果是在 `run_events` 断线补读、重放或故障报告中，无法判断多个 lifecycle 是否属于同一 provider response，也无法把本地 attempt 与供应商响应 ID/模型快照对齐。若将来诊断重复/缺失/乱序 SSE，当前 durable 事件只能显示事件类型，无法核对官方 sequence 或关联 provider 侧请求；该限制会扩大定位时间，并使自动化流完整性诊断缺少依据。
- 修复应在现有 Responses semantic event DTO 增加可选、脱敏且版本兼容的 `sequence_number/response_id/provider_model`（必要时只在 created/终态保存 ID/model），由 accumulator 捕获并通过既有 RuntimeEvent 与 run_events 投影；不另建 provider event store。需为既有 JSON 消费者使用缺省兼容字段。若后续实现 sequence 校验，应明确按单 response 生命周期管理，并将缺号、重复、逆序和重试新 response 分别作为可观察的完整性状态；不应把官方“单调递增”外推成未证实的重连 resume 协议。
- 回归至少包括正常 sequence 0..N、跨 item 的同 response ID、重试产生新 response ID、字段缺失兼容、重复/跳号/倒序诊断，以及 `run_events` 断线补读保真。正常文本、reasoning、工具调用、usage 和唯一终态必须保持现有行为。真实 DeepSeek sequence 的实际发送一致性和服务端 model 值是否总会出现，仍需真实 provider 样本验证；本次离线 fixture 仅证明宿主收到合法形状后丢弃元数据。

本轮 H-200 只新增 `#[ignore]` 审计探针和报告，未修改生产事件 DTO 或解析行为。

### H-201：工作流画布缩放控件改变 zoom 却保留平移偏移，视口焦点随每次缩放跳动

**级别：低（P3，桌面画布导航一致性）；触发条件：画布已平移到非原点后，使用底栏放大/缩小、底栏比例滑杆或小地图放大/缩小。** 问题仅影响视图定位；没有证据显示节点坐标、工作流定义、运行事件或模型请求因此变化。

#### 生产路径与动态证据

- `workflow/components/BottomBar.tsx` 的 `handleZoomIn`、`handleZoomOut` 与 `handleSlider` 对现有 `viewport` 展开后只替换 `zoom`，再分别调用 `onViewportChange` 与 `rfInstance.setViewport()`；`workflow/components/MiniMap.tsx::changeZoom` 同样读取当前 viewport 并只替换 `zoom`。这几条交互都保留旧的 `x/y` 平移量。
- React Flow 使用 `screen = world * zoom + translation` 这一 viewport 仿射变换；本仓库锁定的 `@xyflow/system` 发行源码将 viewport 映射为 translate/scale transform，反算世界坐标为 `(screen - translation) / zoom`。因此 zoom 改变而 translation 不变时，中心看到的世界点会变化。`CanvasApp.tsx` 的 Ctrl/Meta+wheel 路径另有按鼠标锚点重算 x/y 的代码；该路径不是本项复现范围，表明现有画布已经存在正确锚定模式。
- 新增 opt-in 探针 `apps/desktop/scripts/audit-workflow-zoom-anchor.test.cjs`，用 TypeScript AST 提取并执行当前生产 TSX 中的六个原始 handler，只替换它们的闭包依赖与 React Flow 实例，不手写生产 zoom handler。尺寸为 1000×600、起始 viewport `{x:-500,y:-240,zoom:1}`，使常规缩放前中心世界坐标为 `(1000,540)`。执行 `$env:RUN_HARNESS_AUDIT='1'; node --test apps/desktop/scripts/audit-workflow-zoom-anchor.test.cjs` 得到 **0 passed / 1 failed（预期）**：底栏与小地图 zoom-in/滑杆后中心变为 `(909.09,490.91)`；zoom-out 后变为 `(1111.11,600)`；底栏 reset 以 zoom=1.5 的非默认 viewport 为输入，同样未保留 `(766.67,440)` 的中心点。六条生产 handler 均存在焦点漂移。该测试验证的是实际 handler 参数与最终 viewport，不是只对 source 文本做字符串匹配；但没有启动 Tauri/WebView 或进行像素截图检查。

#### 无限画布参考对照

- **Penguin-Magic NodeCanvas** 的 `components/NodeCanvas/index.tsx::zoomAtViewportCenter()` 先以视口中心和旧 `offset/scale` 求出中心世界点，再用新 scale 反算新 offset；代码注释明确该函数供底部工具栏与小地图共用。**Penguin-Magic InfiniteCanvas** 的滚轮缩放以鼠标世界点为锚，重新计算 offset 后提交 viewport。两种交互策略分别对应底栏/小地图固定中心与滚轮指针位置，本系统现有 Ctrl/Meta+wheel 与其设计一致，但按钮路径没有对应的中心锚定。
- Codex 与 DeepSeek Harness 不提供本系统这种桌面无限画布控件，故不把它们声称为直接 UI 对照；这项属于桌面交互实现质量，不是 DeepSeek API/缓存/工具协议差异。Penguin 的实现用于本项已明确限定的本地参考对照，不意味着其代码可以直接复制。

#### 后果与修复方向

- 用户在非原点视区点一次放大，视口中心立即移动约 90.91 个世界单位；缩小一次则向相反方向移动约 111.11 个单位。连续调整滑杆或放大/缩小会让当前关注节点从屏幕中心漂开，需要再次平移或调用 fitView。图数据和运行不变，影响是可重复的导航跳动而非数据损坏；鼠标锚定滚轮、重新定位或 fitView 是当前可用替代路径。
- 根因是缩放控制在组件里各自只更新标量 zoom，而没有共同的 camera/viewport 转换策略。建议在 Canvas 父层提供单一 `zoomAtViewportCenter(nextZoom)`，底栏和 MiniMap 共用它；在父层依据容器尺寸求旧世界中心并重算 x/y，一次同时更新 React Flow 和 CanvasStore，避免出现 UI/React Flow 两份行为。若以后给小地图本身加入点击导航，应另明确其点击位置锚点，不能与本项中心缩放混为一谈。
- 回归覆盖底栏 +/-、比例滑杆、小地图 +/- 在正负平移及不同当前 zoom 下保持世界中心不变，并确保 Ctrl/Meta+wheel 仍保持鼠标下世界点不动；兼顾 zoom clamp、尺寸变化和空节点 fitView。当前未做生产修复，未启动 WebView 做视觉验证。

本轮 H-201 只新增 opt-in 生产 handler 审计探针和报告，未修改生产画布行为。

### H-202：工作流图持久化不包含视口，重启后用户的画布导航状态无法恢复

**级别：低（P3，画布编辑连续性）；触发条件：用户平移/缩放创作或专业画布、修改图并等待保存，然后重启/重新挂载桌面应用。** 图节点与连线仍会恢复；丢失的是相机平移/缩放状态，ReactFlow 会按当前 `fitView` 设置重新计算初始视图。本项不影响工作流执行输入或已保存图数据。

#### 生产路径与动态证据

- 正式 `workflowPersistence.ts::persist()` 只从 creative/professional store 生成 `{nodes, edges}` 并调用 `canvas_workflow_state_write`；`normalizeGraph()` 也只返回这两项。`useWorkflowPersistence()` 在数据库 hydration 完成后才挂载 `WorkflowCanvasInner`，但不载入 viewport。`canvasStore.ts` 的 viewport 是模块内 Zustand 初始值 `{x:0,y:0,zoom:1}`；`CanvasApp.tsx` 的 `onMove` 只更新该内存 state，没有找到它进入 graph persistence 或独立 preference 的其它生产写入路径。
- 正式 `<ReactFlow>` 配置 `fitView`、没有 `defaultViewport`。当前锁定的 `@xyflow/react` 12.11.2 源码将该 prop 作为初始 `fitViewQueued` 交给 store，按 nodes bounds 与容器尺寸计算 viewport；这与恢复“上次镜头”不同。没有声称真实像素位置或 WebView 启动时序已做 UI 验收。
- 新增 opt-in `apps/desktop/scripts/audit-workflow-viewport-persistence.test.cjs`。它直接加载正式 `workflowPersistence.ts` 和生产 CanvasStore，在受控 Tauri boundary 记录实际 `canvas_workflow_state_write` 数据；在写入前将正式 viewport 设为 `{x:-975,y:-430,zoom:1.65}`，同时创建一个真实 store 图节点，再 flush。之后创建第二个生产 store 实例模拟全新 WebView，调用正式 hydration，并从生产 `CanvasApp.tsx` AST 读取 ReactFlow 的实际 props。执行 `$env:RUN_HARNESS_AUDIT='1'; node --test apps/desktop/scripts/audit-workflow-viewport-persistence.test.cjs` 得到 **0 passed / 1 failed（预期）**：写入图键为 `['edges','nodes']`、`persistedViewport=null`；新 store viewport 为 `{x:0,y:0,zoom:1}`，但图节点 `audit-node` 正常恢复；ReactFlow props 为 `{fitView:true,defaultViewport:false}`。因此失败只针对导航状态恢复；没有 Tauri 窗口、文件数据库或节点图数据外部副作用。

#### 无限画布参考对照

- **Penguin-Magic NodeCanvas** 在 `services/api/nodeCanvas.ts::NodeCanvasDocument` 将 viewport 纳入每个画布文档，`nodeCanvasStorage.js::normalizeDocument()` 持久化并归一化 viewport，`applyCanvasDocument()` 读回时通过 `commitViewport(source.viewport.canvasOffset, source.viewport.scale)` 应用；另有 viewport 变化后的 debounce 写入。也就是说，同一用户有多个画布时视口有明确的文档身份，而不是只有图内容持久化。
- Codex 与 DeepSeek Harness 没有可直接比较的无限画布 camera contract；不将它们虚构成 viewport 保存方案。本项是桌面工作流画布的状态持久化选择，不是 DeepSeek Responses/缓存/工具协议问题。

#### 后果与修复方向

- 图被正确恢复，所以不存在数据丢失；但用户重开后无法回到上次聚焦的区域，ReactFlow 重新 fit 整张图时，散布较远的大图可能以较小比例显示，仍需再次 zoom/pan。创作与专业两个 mode 当前只共享一份内存 viewport，也没有各自的 per-mode camera snapshot；当前探针证明重启丢失，不单独定性 mode 切换的视觉效果。
- 根因是持久化契约把可编辑图数据当成完整画布文档，camera 被当作临时 UI state。建议在现有每 mode 的 canvas-workflow document 中增加可选且向后兼容的 viewport，保存时随 `onMove` debounce、恢复时在 ReactFlow fitView 完成/尺寸确定后应用；ReactFlow `setViewport` 和 Zustand state 必须同步，旧记录缺 viewport 时继续保留现有 fitView 首开行为。不得因为相机位置变化而触发图节点历史或改变 workflow revision/语义。
- 回归覆盖 creative/professional 分别保存、关闭/重启、旧数据无 viewport 迁移、尺寸变化时保持合理的 world center、空图与 fitView、窗口关闭前 flush 以及用户持续操作时的 debounce 写入顺序。当前未改生产逻辑，未执行完整桌面 UI/屏幕验证。

本轮 H-202 只新增 opt-in 生产持久化探针和报告，未修改生产 viewport 或 graph contract。

### H-203：Responses `message` 的公开字符串角色缺少 provider 边界校验

**级别：低（P3，条件性公共 API 协议缺口）；触发条件：调用者、历史导入器或未来 Harness/SDK 直接构造 `ResponseInputItem::Message` / `InputMessage`，并提供 Responses 不支持的 `role` 字符串。** 当前桌面普通消息由 `deepagent_core::message::Role` 的四个变体产生，通常路径不会生成任意 role；本项不表示日常聊天已发送非法角色。

#### 官方契约、生产路径与离线复现

- 2026-10-24 实际打开 DeepSeek 官方 [Responses API 参数文档](https://api-docs.deepseek.com/zh-cn/api/create-response/)：`message` 支持的角色仅为 `user`、`assistant`、`system`、`developer`，其中 `developer` 视同 `user`；[Responses 指南](https://api-docs.deepseek.com/zh-cn/guides/responses_api/)亦将同一集合列入输入兼容性。工具结果使用独立 `function_call_output` / `custom_tool_call_output` item，不应借用一个任意 message role 表示。
- `deepagent_core::response_item::ResponseInputItem::{Message, InputMessage}` 将 `role` 定义为 `String`。`ResponseRequest::validate()` 仅验证可选 `user` 标识，没有验证 message role；`response_input_items_to_wire()` 把字符串原样写入 JSON。因此通过公开 `deepagent_models::ResponseInputItem` / `ResponseRequest::from_response_items()` 构造时，非法值不会在 serializer 前失败。普通应用消息经 `deepagent_core::message::Role::as_str()` 转换，正常桌面生产路径只有四种本地角色；风险在 provider DTO、外部反序列化或未来复用。
- 新增 opt-in 正式 ModelClient 探针 `crates/deepagent-models/tests/responses_input_role_audit.rs::audit_invalid_responses_message_roles_are_rejected_before_transport`，分别为 `Message` 与 `InputMessage` 提供 `role="moderator"`，经过公开 request constructor、正式 ModelClient、最终 serializer 到 recording transport。执行 `cargo test -p deepagent-models --test responses_input_role_audit --offline -- --include-ignored --nocapture` 得到 **1 passed / 1 failed（预期）**：非法组为 `message_error=false, input_message_error=false, transport_calls=2, wire_roles=["moderator","moderator"]`；同一命令的官方角色控制为 **1 passed**，按顺序观察到 `user/assistant/system/developer`。离线 fixture 的成功只证明调用抵达传输边界，不证明 DeepSeek 服务端会接受或以何种错误拒绝该请求。

#### 成熟实现对照、后果与收敛方向

- DeepSeek Harness 的 `packages/llm/llm-deepseek/src/wire-types.ts` 把其 Messages 路由的 `WireMessage.role` 限定为 TypeScript 字面量联合，而不是任意字符串。这是 provider adapter 持有角色集合的实现模式；该文件对应的 Messages 协议没有 Responses 的 `developer` 角色，不能照抄它的集合到 DeepSeek Responses。
- Codex 参考树的 `protocol/src/models.rs::ResponseInputItem::Message` 同样以 `String` 表示 wire role，因此没有证据表明 Codex 的该协议 DTO 已经消除此处校验缺口。Codex 对内容块和会话项使用更丰富的 typed model，但不把它误报为 role validator。
- 若无效 role 触发，供应商可能拒绝整个请求，也可能忽略/改变这条消息的输入语义；本轮没有真实 API 结果，故不能断言具体响应码或输入处理方式。可确定的宿主后果是调用者无法在 provider boundary 得到本地明确错误，协议错误被延迟到网络之后；对未来 SDK/导入路径可能造成一次无效请求或上下文缺项。没有证据表明普通桌面用户会触发、会发生权限绕过或必然产生费用。
- 修复应留在现有 Responses adapter：对 `Message` / `InputMessage` 仅接受官方四种角色，其他 item 类型继续按各自结构验证；在 `ModelClient` transport 前给出不回显敏感内容的协议错误。不要把 core 持久化类型立即改成封闭枚举而破坏历史兼容；provider 边界可先校验，若之后需要 typed DTO，再以版本化迁移解决。验收覆盖四种合法角色、空值/未知值、两种 message variant、Chat 与 Responses 路由分离、历史反序列化和 transport 调用数为零。

本轮 H-203 仅新增 opt-in 审计探针与本报告，没有改生产校验行为；服务端接受性仍待真实 DeepSeek 请求验证。

### H-204：Harness SDK 对无响应但未断开的 RPC 无期限等待，调用取消后 pending 不回收

**级别：中（P2，条件性 SDK 可用性与请求所有权缺口）；触发条件：SDK 已写出一个合法 RPC 请求，对端保持 stdout/连接打开但长期不回对应 JSON-RPC response；调用方通过 abort/drop 取消等待，或未设置外部 deadline。** 这不是 CLI 默认路径必然故障的证据；若对端及时响应或关闭 stdout，现有行为可正常返回/报错。

#### 正式 SDK 路径与本机复现

- `HarnessClient::call_unchecked()` 在 `rx.await` 上直接等待，没有 per-request deadline、取消 token 或取消时的 pending 清理；`spawn_reader()` 只会在匹配响应到达时 remove request id，或 stdout EOF 时统一 clear。SDK 的公开 `thread_list()`/`config_read()` 等调用走同一路径。请求如果已写出后调用 future 被 drop，oneshot receiver 会消失，但共享 `pending` 中的 sender 仍然保留；Harness protocol 没有在该 client seam 提供取消这条服务器操作的消息。
- 新增 opt-in Windows 子进程探针 `client::audit_tests::audit_unresponsive_server_request_has_a_bounded_wait`。PowerShell fixture 从真实 stdin 管道读到合法请求后先写临时 marker，再保持 stdout 打开 30 秒、不发送 response。测试确认 SDK `thread_list()` 在 300ms 外层观察 deadline 内仍未完成；取消 request task 后 `pending_after_caller_cancel=1`。随后 fixture 被测试明确终止，stdout EOF 才使等待结束为 `ResponseChannelClosed { id: 1 }`。命令 `cargo test -p deepagent-harness-sdk --lib client::audit_tests::audit_unresponsive_server_request_has_a_bounded_wait --offline -- --ignored --exact --nocapture` 为 **0 passed / 1 failed（预期复现）**；失败观测为 `completed_before_release=false, pending_after_caller_cancel=1`。该 fixture 不运行真实 CLI server、模型、工具、用户文件或公网服务。
- H-101 的触发输入是 reader 收到不可解码 stdout 行后继续忽略；H-204 的 fixture 没有发任何非法/非协议字节，问题在请求成功写出但一直没有响应时 SDK 缺少等待期限和 waiter abandonment 清理。两者可共存，但不把同一 malformed-line 现象重复登记。

#### 成熟实现对照、后果与收敛方向

- DeepSeek Harness `packages/sdk/client/src/client.ts::request()` 支持 client-wide 或 per-call `requestTimeoutMs`；期限到达时通过 `AbortController` 中止等待，并由 `packages/sdk/protocol/src/transport.ts::JsonRpcLineTransport.request()` 的 abort handler 删除 pending id。README 明确 initialize 有单独 deadline，request timeout 为可选配置；这证明其支持显式有界请求和本地 pending 回收，不代表它会自动取消 server 端已接纳的工作。
- Codex `app-server-client/src/remote.rs::RemoteAppServerRequestHandle::request_json_rpc()` 也直接 await oneshot，没有据此声称 Codex 已解决每条 RPC 的超时。该文件确实给 connect、initialize、shutdown 设置 deadline，并在 worker 退出时 settle 全部 pending；此处只借鉴其连接生命周期收束边界。比较结果是本系统至少应提供可配置 request deadline/cancel ownership，而不应虚称每个成熟 agent 都使用相同的请求超时设计。
- 触发时，依赖 SDK promise 的 Harness 调用方可能无限挂起；若上层反复用外层 timeout/drop 处理，且 server 保持连接并永不响应，pending sender 会按被放弃请求数量增长直到同 ID response 或 EOF。超时发生在请求写出之后时，服务端是否已接纳操作未知，不能安全自动重试 `thread/start`、`turn/start` 等潜在有副作用请求；应把“结果未知”交给调用方并允许 `thread/read` 等读回查询。当前探针仅以 `thread/list` 验证 SDK 等待，不证明实际 CLI server 有此死锁或内存持续增长量。
- 收敛方向（尚未实施，本地设计）：在现有 SDK call 层引入 request options / timeout，给 initialize 与普通 RPC 可分别配置 deadline；使用 RAII pending registration，成功匹配、写失败、deadline、调用 future drop、reader EOF 均由一个 owner 按 ID 只移除/完成一次。超时错误应携带安全的 method/id 和“请求结果可能未知”语义，不自动重放已写请求；协议若以后需要远端取消，应定义独立版本化 cancel contract，不能把丢弃 Rust future 误当成服务器取消。测试覆盖发送前取消、写入中失败、已发送未答、超时后迟到回复、EOF 竞争、重复取消以及副作用请求的读回恢复。

本轮 H-204 仅新增 opt-in 审计探针与报告，没有改生产 SDK 行为；deadline 用例的 300ms 仅为测试观察窗，不是产品超时配置。

### H-205：Rust Harness SDK 丢弃通知外层 `eventSequence`，无法可靠做 ACK 或缺口恢复

**级别：中（P2，条件性事件完整性与恢复契约缺口）；触发条件：客户端通过公开 `HarnessClient::subscribe()` 消费通知，并需要确认实际消费到哪个 wire 序号、发现序号缺口或从 `thread/read` 续读。** 服务端通知的 `eventSequence` 仍然存在于 JSON-RPC 外层，但 SDK 的公开事件流只返回 `HarnessEvent`，因此这个问题与 H-01 的“服务端队列已丢通知”及 H-91 的“服务端并发分配/入队顺序倒退”是不同根因。

#### 正式 SDK 路径与本机复现

- `apps/cli/src/app_server.rs::EventOutbox` 为每个通知分配单调 `eventSequence`，并在 `RpcNotification.event_sequence` 写出；`crates/deepagent-harness-protocol/src/requests.rs::RpcNotification` 也明确反序列化该字段。
- 但 `crates/deepagent-harness-sdk/src/client.rs::spawn_reader()` 在收到 `WireMessage::Notification(notification)` 后只执行 `serde_json::from_value::<HarnessEvent>(notification.params)`，随后 `events.send(event)`；`notification.event_sequence` 没有进入任何公开 DTO、订阅项或客户端游标。`HarnessClient::event_ack()` 又要求调用者自己传入 `u64`，SDK 没有把“最后实际交付的序号”与事件绑定起来。
- 新增 opt-in Windows 子进程探针 `crates/deepagent-harness-sdk/src/client.rs::audit_tests::audit_sdk_notification_preserves_event_sequence_for_ack_and_gap_detection`。PowerShell fixture 通过真实 stdout 输出两条合法 `harness/event` 通知，外层序号分别为 7、9（中间故意缺 8），再经过生产 `spawn_reader()` 和 `broadcast` 订阅读取。执行：

  `cargo test -p deepagent-harness-sdk --lib client::audit_tests::audit_sdk_notification_preserves_event_sequence_for_ack_and_gap_detection --offline -- --ignored --exact --nocapture`

  结果为 **0 passed / 1 failed（预期复现）**。断言前捕获的公开事件序列化结果为：

  `[{"type":"thread.started","threadId":"t-7","title":null,"protocolVersion":1},{"type":"thread.started","threadId":"t-9","title":null,"protocolVersion":1}]`

  可观察到的 `eventSequence` 为 `[None, None]`，而 wire 实际为 `[Some(7), Some(9)]`。本探针没有访问模型、工具、用户文件或公网，也没有修改生产 SDK。

#### 成熟实现对照、后果与收敛方向

- DeepSeek Harness 的 `packages/sdk/client/src/types.ts` 将完整通知对象作为订阅值，`client.ts` 的 `subscribe()`、`subscribeSessionTree()` 和 `HarnessSession` 均在通知对象上工作；其实现虽然没有本系统同名的 `eventSequence` 契约，但保留了 transport envelope，未来可在同一位置增加序号/游标，不会先被 SDK 丢掉。
- Codex `app-server-client/src/lib.rs` 将有界广播溢出显式映射为 `AppServerEvent::Lagged { skipped }`，而不是把事件当成无元数据的业务枚举静默交付。该对照不能证明 Codex 有本系统的 `event/ack` 机制，只能说明成熟客户端把传输完整性状态暴露给调用者。
- 触发后，SDK 使用者无法知道收到的 `thread.started` 是序号 7 还是 9，也无法从订阅流判断是否漏掉序号 8；若按“已消费事件数”猜测并调用 `event/ack`，服务端确认游标可能落后、错误前进或与 `thread/read` 的 `afterSequence` 错配。断线重连时，调用者只能盲读全量或自行另建原始 JSON-RPC reader，破坏 SDK 的单一入口边界；对终态、审批和工具结果等不可重复事件，后果是缺口无法自动定位，可能表现为 UI/SDK 永久等待或错误地把不完整流当作完整。
- 收敛应留在现有协议/SDK seam：将公开订阅项改为携带 `event_sequence` 的通知信封（或增加等价的 `HarnessNotification` DTO），保留现有 `HarnessEvent` 作为 `params` 业务载荷；`event_ack` 应提供“按已交付通知确认”的辅助 API，或由订阅项暴露 sequence 后由调用者显式 ACK。收到非连续序号时，SDK 至少返回可识别的 gap/lag 状态，交由已有 `thread/read` durable cursor 补读，不应自动重放有副作用的请求。回归需覆盖连续序号、7→9 缺口、broadcast lag、迟到响应、ACK 回退、断线后 `thread/read` 补读，以及旧客户端 DTO 的兼容反序列化。

本轮 H-205 仅新增 opt-in 审计探针与本报告，没有修改生产协议或 SDK API；缺口恢复的最终策略和版本兼容方案尚未实施。

### H-206：Harness SDK 接受协议版本不兼容的 initialize 结果并进入已初始化状态

**级别：中（P2，条件性协议兼容与错误隔离缺口）；触发条件：SDK 对端返回合法 JSON-RPC response，`result` 存在且 `error` 为空，但结果中的 `protocolVersion` 与客户端支持版本不一致，或缺少本系统 initialize 结果所需的身份字段。** 正常的本系统 CLI app-server 返回版本 1，本项不是默认服务必然错误；风险在版本漂移、错误二进制、代理转发到其他 Harness 服务或第三方实现时。

#### 正式 SDK 路径与本机复现

- 本地协议在 `crates/deepagent-harness-protocol/src/requests.rs` 将 `PROTOCOL_VERSION` 固定为 `1`；生产 `apps/cli/src/app_server.rs::initialize()` 的成功结果同时发布 `protocolVersion`、`serverName`、`serverVersion` 和 capabilities。也就是说，initialize 的 result 不是任意 JSON，而是后续请求能够安全解释的版本/身份协商边界。
- `crates/deepagent-harness-sdk/src/client.rs::HarnessClient::initialize()` 只检查 `RpcResponse.error` 是否存在：`call_unchecked()` 返回任何无 error 的 `RpcResponse` 后，函数直接写入 `initialized=true`，没有读取或校验 result 的 `protocolVersion`、`serverName`、`serverVersion`，也没有将不兼容结果转换为 `SdkError`。随后所有公开 `call()` 方法都会因为该布尔值而放行。
- 新增 Windows ignored 探针 `client::audit_tests::audit_initialize_rejects_incompatible_protocol_result`。它启动本机 PowerShell 子进程，通过真实 stdin/stdout 管道读取合法 initialize 请求，再返回合法 JSON-RPC：`{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":999,"serverName":"future-server","serverVersion":"9.9.9"}}`。该响应没有 malformed JSON、没有缺少 `result`、也没有 RPC error。执行 `cargo test -p deepagent-harness-sdk --lib client::audit_tests::audit_initialize_rejects_incompatible_protocol_result --offline -- --ignored --exact --nocapture` 得到 **0 passed / 1 failed（预期复现）**；生产观测为 `result=Ok(RpcResponse { ... protocolVersion: 999 ... })` 且 `initialized=true`。没有启动真实第三方服务、模型、工具、用户文件或公网。
- H-100 的触发器是 response 同时缺少 `result` 和 `error`，重点是 JSON-RPC envelope 合法性；H-206 的 fixture 具有完整 result，只是协议版本不兼容，故不合并为同一问题。H-204 的静默连接也不参与本项，fixture 已立即返回 response。

#### 成熟实现对照、后果与收敛方向

- DeepSeek Harness `packages/sdk/client/src/client.ts::initialize()` 在 request 成功后校验 result 是对象，并要求 `serverInfo.name` 与 `serverInfo.version` 都是字符串；不满足就抛出 `SdkProtocolError`，不会把客户端当作可用。其后续 API 还对 method-specific result（例如 prompt 的 `messageId`）做结构校验，说明握手和方法结果都属于 SDK 协议边界，而不是把 raw JSON 直接当成功。
- Codex `app-server-client` 使用 typed request/response 和初始化后的 server metadata；本次没有找到与本系统同名的 `protocolVersion` 校验，因此不声称 Codex 已实现同一数字版本策略。可迁移的架构原则是：握手必须产生可验证的连接能力快照，方法结果必须按对应 DTO 解码，不能只看 JSON-RPC error 位。
- 触发后，SDK 会把不兼容服务标记为已初始化，后续 `thread/start`、`turn/start`、`approval/respond` 等请求可能进入一个字段/终态语义不同的对端。可确定的后果是兼容性错误被延迟到业务调用后，且 SDK 无法在连接建立处给出明确的版本错误；如果对端恰好复用相同方法名但改变参数或事件含义，调用方可能把错误 payload 当作正常 Harness 状态。该探针没有继续向 fixture 发送副作用请求，因此不把具体数据破坏或模型调用写成已发生事实。
- 根因修复应留在现有 SDK handshake seam：解析 initialize result 的版本化 typed DTO，至少要求 `protocolVersion == PROTOCOL_VERSION`、非空 server identity 和必要 capability 字段；不兼容时保持 `initialized=false`，清理 pending，并返回带 expected/actual 的结构化协议错误。旧服务若需要兼容，应在协议版本协商中明确向后兼容范围，不应静默接受任意数字。回归需覆盖正确版本、未知更高/更低版本、缺字段、错误类型、RPC error、迟到 response，以及 initialize 失败后公开调用仍被 `NotInitialized` 拒绝；不能把此校验分散到每个业务方法。

本轮 H-206 仅新增 opt-in SDK 审计探针与本报告，没有修改生产握手行为；版本协商和 typed result 解码尚未实施。

### H-207：Harness SDK 对 `thread/start` 成功结果缺少必填 `threadId` 校验

**级别：中（P2，条件性 SDK 协议完整性与错误隔离缺口）；触发条件：** 对端对合法 `thread/start` 请求返回 JSON-RPC 成功响应，但 `result` 不是本方法约定的包含非空 `threadId` 的对象，例如返回 `{}`。当前 SDK 会把该响应交给调用方并报告成功；正常的本系统 CLI server 不会主动生成该结果，风险来自版本漂移、代理/第三方 Harness 实现、错误响应映射或未来方法扩展。

#### 正式 SDK 路径与本机复现

- `crates/deepagent-harness-protocol/src/requests.rs` 定义 `thread/start` 请求，但没有与之对应的 typed result DTO；`apps/cli/src/app_server.rs::thread_start()` 的正式成功结果包含 `threadId`，并立即发布 `thread.started` 事件。`crates/deepagent-harness-sdk/src/client.rs::HarnessClient::thread_start()` 只调用通用 `call()`，返回未解码的 `RpcResponse`。
- 通用 `call_unchecked()` 仅在 `response.error` 非空时返回 `SdkError::Rpc`，只要 JSON-RPC `error` 为空就返回 `Ok(response)`；它不会按 method 检查 `result` 的对象形状、`threadId` 字符串或非空约束。现有 SDK e2e 测试也由调用方自行执行 `response.result()["threadId"].as_str().expect(...)`，说明协议校验已经泄漏到每个消费者。
- 新增 Windows ignored 探针 `client::audit_tests::audit_thread_start_rejects_success_result_without_thread_id`。探针启动本机 PowerShell 子进程和真实 stdin/stdout 管道：第一次请求返回合法 initialize result（`protocolVersion=1`），第二次对正式 `thread_start()` 返回合法 JSON-RPC `result={}`。执行：

  `cargo test -p deepagent-harness-sdk --lib client::audit_tests::audit_thread_start_rejects_success_result_without_thread_id --offline -- --ignored --exact --nocapture`

  实际结果为 **0 passed / 1 failed（预期复现）**，生产观测：`thread/start ... got Ok(RpcResponse { jsonrpc: "2.0", id: Number(2), result: Some(Object {}), error: None })`。fixture 没有模型、工具、用户文件、真实 CLI server 或公网副作用。
- H-206 的输入是 initialize 结果中的协议版本/身份不兼容，重点是连接握手是否进入 Ready；H-207 的 initialize 已合法通过，缺陷发生在后续方法成功结果的 schema 边界。H-100 的输入同时缺少 JSON-RPC `result` 与 `error`，而 H-207 明确具有 `result={}`，因此不合并。H-204 的静默连接不参与本项，fixture 对每个请求都立即返回。

#### 成熟实现对照、后果与收敛方向

- DeepSeek Harness 的 `packages/sdk/client/src/client.ts::initialize()` 会校验握手对象；其 `session/prompt` 等方法还校验 method-specific result（例如必须存在字符串 `messageId`），错误结果会转为 `SdkProtocolError`，不会把 raw JSON 的存在当作业务成功。其协议 README 也将各方法的 result 类型列为 wire contract，而不是把校验责任交给每个调用者。
- Codex app-server 使用 typed protocol notification/request models，测试辅助层按具体通知方法反序列化后才交给调用方；本次没有找到与本系统 `thread/start` 同名的 Rust SDK 方法，因此不声称 Codex 已实现相同字段名校验。可迁移的成熟原则是：方法结果和请求一样属于协议边界，不能只由 JSON-RPC envelope 的 error 位决定成功。
- 触发后，调用方会拿到一个“成功但没有线程身份”的响应。如果它把 `Ok` 当作 admission receipt，可能把空/默认 ID 继续传给 `turn/start`、`thread/read` 或 UI/SDK 状态；常见后果是后续请求以错误线程运行、立即收到无效线程错误，或在客户端先写入已接纳状态后等待永远不会到来的 `thread.started`。本探针没有继续发送副作用请求，因此不把具体数据破坏或模型调用写成已发生事实。
- 根因修复应留在现有 SDK/protocol seam：为每个公开方法定义版本化 typed result（至少先覆盖 `initialize`、`thread/start`、`thread/resume`、`turn/start`、`approval/respond`、`event/ack`），在 `call()` 返回前按 method 解码并拒绝缺字段、错误类型和空 ID；错误需带 method、expected shape 和实际摘要，同时保持 `RpcResponse` 原始诊断信息。调用方不应各自调用 `result()[...]` 猜 schema。回归需覆盖正确结果、缺字段、错误类型、空字符串、RPC error、迟到 response 和旧版本兼容策略；不能把校验复制到 CLI、桌面和 SDK 三套入口。

本轮 H-207 仅新增 opt-in SDK 审计探针与本报告，没有修改生产 SDK/protocol 行为；方法级 result DTO 与统一校验尚未实施。

## 本次验证与交付边界

| 命令或实测 | 结果 |
|---|---|
| H-142 运行事件持久化失败显式探针 | `cargo test -p deepagent-runtime audit_event_persistence_failure_does_not_split_live_and_replay --offline -- --ignored --nocapture`：0 passed / 1 failed（预期）；真实 `AgentKernel` + SQLite trigger 让 `tool_completed` durable append 失败，但实时 sink 仍收到该事件；仅临时内存数据库和离线 Agent |
| H-142 Kernel 默认回归与静态检查 | `cargo test -p deepagent-runtime --lib kernel::tests --offline`：10 passed / 1 ignored / 0 failed；`cargo fmt --all -- --check` 与 `git diff --check` 通过；未运行真实 Tauri、provider 或跨进程故障注入 |
| H-143 终态持久化失败显式探针 | `cargo test -p deepagent-runtime audit_terminal_persistence_failure_does_not_claim_live_completion --offline -- --ignored --nocapture`：0 passed / 1 failed（预期）；实时收到 `run_completed`，但 `run_terminal` 事务回滚且 `finished_at` 为空 |
| H-144 Chat `aborted` 合法终态显式探针 | `cargo test -p deepagent-models audit_chat_aborted_finish_is_not_rejected_as_unknown --offline -- --ignored --nocapture`：0 passed / 1 failed（预期）；正式 `ModelClient` 将官方 `finish_reason=aborted` 返回为 `Provider` 未知终态错误；MockTransport，无公网 |
| H-144 Chat `insufficient_system_resource` 合法终态显式探针 | `cargo test -p deepagent-models audit_chat_insufficient_system_resource_finish_is_not_rejected_as_unknown --offline -- --ignored --nocapture`：0 passed / 1 failed（预期）；正式 `ModelClient` 将官方 `finish_reason=insufficient_system_resource` 返回为 `Provider` 未知终态错误；MockTransport，无公网 |
| H-145 Chat Thinking + `required` tool choice 组合显式探针 | `cargo test -p deepagent-models audit_chat_thinking_rejects_required_tool_choice_before_transport --offline -- --ignored --nocapture`：0 passed / 1 failed（预期）；正式 adapter 返回 Ok 且计数 transport 被调用，未在请求前拒绝；MockTransport，无公网 |
| H-145 Chat Thinking + named tool choice 组合显式探针 | `cargo test -p deepagent-models audit_chat_thinking_rejects_named_tool_choice_before_transport --offline -- --ignored --nocapture`：0 passed / 1 failed（预期）；正式 adapter 返回 Ok 且计数 transport 被调用，未在请求前拒绝；MockTransport，无公网 |
| H-146 Chat 用户身份字段显式探针 | `cargo test -p deepagent-models audit_chat_user_identity_uses_provider_user_id_field --offline -- --ignored --nocapture`：0 passed / 1 failed（预期）；正式 Chat wire 发出 `user` 且缺少 DeepSeek 要求的 `user_id`；受控 transport，无公网 |
| H-147 DeepSeek 工具 schema 名称显式探针 | `cargo test -p deepagent-models audit_deepseek_tool_schema_names_are_validated_before_transport --offline -- --ignored --nocapture`：0 passed / 1 failed（预期）；Responses 的空名、非法字符、129 字符和重复名均以 `transport_calls=1` 越过正式 adapter，Chat 四组同样到达 transport 后因 fixture 缺 `[DONE]` 结束；受控 transport，无公网 |
| H-148 Responses 空 `call_id` 显式探针 | `cargo test -p deepagent-models audit_responses_call_ids_are_validated_before_transport --offline -- --ignored --nocapture`：0 passed / 1 failed（预期）；正式 `ResponseRequest`/`ModelClient` 将空 function call/result ID 送达 transport，`transport_calls=1`；受控 transport，无公网 |
| H-149 Responses 重复 `call_id` 显式探针 | `cargo test -p deepagent-models audit_responses_duplicate_call_ids_are_rejected_before_transport --offline -- --ignored --nocapture`：0 passed / 1 failed（预期）；两个不同 function call 和结果共用一个 ID，正式 adapter 仍返回 `Ok`、`transport_calls=1`，wire 只剩第一组调用/结果；受控 transport，无公网 |
| H-150 Responses 空 `input`/`instructions` 显式探针 | `cargo test -p deepagent-models audit_responses_require_input_or_instructions_before_transport --offline -- --ignored --nocapture`：0 passed / 1 failed（预期）；正式 adapter 将 `input=[]` 且无 instructions 的请求送达 transport，返回 `Ok`、`transport_calls=1`；受控 transport，无公网 |
| H-151 Responses malformed `function_call` 显式探针 | `cargo test -p deepagent-models audit_responses_reject_malformed_function_call_output --offline -- --ignored --nocapture`：0 passed / 1 failed（预期）；正式 adapter 将缺少 `call_id`/`name` 的 item 解析为两个 function call（空 ID/默认 `tool` 与 fallback `call_stream_0`），返回 `Ok(Response)`；受控 SSE，无公网 |
| H-152 Responses `tool_search_call` 显式探针 | `cargo test -p deepagent-models audit_responses_tool_search_call_is_not_silently_dropped --offline -- --ignored --nocapture`：0 passed / 1 failed（预期）；正式 adapter 返回 `Ok(Response)`，唯一 output item 是空 assistant message，原生调用的 `call_id`、`execution` 和结构化 arguments 全部丢失；受控 SSE，无公网 |
| H-153 修复前 Responses 思考强度探针 | `cargo test -p deepagent-app-core --test kernel_v2_e2e audit_responses_reasoning_effort_settings_match_deepseek_contract --offline -- --ignored --nocapture`：修复前 0 passed / 1 failed（预期）；普通设置拒绝 canonical `none`，开发者 JSON 拒绝 `none/minimal/xhigh`，`medium` 原样保存；正式 SettingsService 与内存数据库，无公网或模型请求 |
| H-153/H-155 规范值输入边界及旧设置迁移 | `cargo test -p deepagent-app-core --test kernel_v2_e2e responses_reasoning_effort_settings_accept_only_canonical_values --offline -- --nocapture` 与 `cargo test -p deepagent-app-core --lib load_migrates_legacy_reasoning_and_removes_noncanonical_values --offline -- --nocapture`：均通过；证明当前新输入只接受四个规范值、旧持久化 medium 映射为 high；与官方接受别名的差异见 H-155 |
| H-153 设置到 provider wire 端到端 | `cargo test -p deepagent-app-core --test kernel_v2_e2e responses_reasoning_effort_settings_reach_provider_wire --offline -- --nocapture`：1 passed；四档均通过正式 SettingsService → ChatService → Kernel → ModelAgent → ModelClient 发到离线 transport，检查实际请求 body，无公网请求 |
| H-153 模型适配器及桌面构建 | `cargo test -p deepagent-models --offline`：83 passed / 22 ignored，集成测试 1 passed / 1 ignored；`pnpm build`：通过完整脚本、TypeScript 检查和 Vite build，有依赖 externalized-module 与大 chunk 警告 |
| H-153 AppCore 默认集成回归 | `cargo test -p deepagent-app-core --test kernel_v2_e2e --offline`：33 passed / 30 ignored；更新两处旧断言，明确推理强度不再隐式提供输出 token 上限。无网络请求 |
| H-153 AppCore 单元测试 | `cargo test -p deepagent-app-core --lib --offline`：1091 passed / 44 ignored；覆盖 reasoning 设置迁移及 AppCore 默认回归 |
| H-155 官方 Responses effort 别名拒绝复现 | `cargo test -p deepagent-app-core --test kernel_v2_e2e responses_reasoning_effort_settings_accept_only_canonical_values --offline -- --nocapture`：1 passed；该测试断言 SettingsService 拒绝 `minimal`、`medium`、`xhigh`，与 DeepSeek Create Response 文档明示的兼容别名相矛盾；未发 provider 请求 |
| H-155 规范值实际请求 JSON | `cargo test -p deepagent-app-core --test kernel_v2_e2e responses_reasoning_effort_settings_reach_provider_wire --offline -- --nocapture`：1 passed；正式 AppCore/Kernel/ModelClient 发到记录 JSON 的离线 transport，检查四个规范值位于 `reasoning.effort`；未验证真实供应商行为 |
| H-156 默认请求 custom tool 范围探针 | `cargo test -p deepagent-app-core --test kernel_v2_e2e responses_custom_tool::default_harness_request_does_not_advertise_apply_patch --offline -- --exact --nocapture`：1 passed；正式 AppCore 请求携带普通工具但没有精确名 `apply_patch`。因此只确认可选同名扩展的 serializer shape 风险；未触发该分支、未捕获含扩展的完整 body，也未发公网请求 |
| H-157 模型级 effort 元数据探针 | `cargo test -p deepagent-models --test reasoning_effort_audit audit_model_effort_metadata_constrains_responses_request --offline -- --ignored --nocapture`（2026-10-24）：0 passed / 1 failed（预期）；正式 ModelDiscovery/ModelCatalog 恢复丢弃 fixture 的 `effort.supported_levels=[low]`，正式 ModelClient 仍序列化 `reasoning.effort=high`；仅离线新模型 fixture，不代表当前 DeepSeek 模型或真实 provider 已拒绝 |
| H-158 Responses named `tool_choice` 显式探针 | `cargo test -p deepagent-app-core --test kernel_v2_e2e responses_tool_choice::unsupported_responses_named_tool_choices_are_rejected_before_transport --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；settings parser 接受 custom `apply_patch` 与 `web_search` 两种对象，正式 ModelClient 各发送一次匹配工具和原样 choice；仅受控 transport，无公网 provider |
| H-159 MCP 工具参数 schema 发布边界探针 | `cargo test -p deepagent-app-core --test kernel_v2_e2e tool_schema_parameters::invalid_mcp_tool_schemas_are_rejected_before_provider_transport --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；省略、字符串、数组三种 `inputSchema` 均通过正式 MCP/工具注册链，各触发一次模型 transport，wire `parameters` 分别为 `null`、字符串、数组；仅受控 transport，无公网 provider |
| H-160 前端工作流模型默认探针 | `cd apps/desktop && node --test scripts/audit-workflow-model-identity.test.cjs`：0 passed / 1 failed（预期）；动态执行生产 registry 后，LLM/问题分类/参数提取/Agent V2 的节点默认与 schema 默认均为裸 `deepseek-chat`；首次错误 kind 的 fixture 失败不计入结果，无 Tauri 或公网 |
| H-160 后端模型身份与 transport 探针 | `cargo test -p deepagent-app-core --lib audit_invalid_explicit_model_ref_is_rejected_before_provider_transport --offline -- --ignored --nocapture`：0 passed / 1 failed（预期）；请求 `deepseek-chat` 被解码为 `None`，正式候选路由向本机 HTTP 发送 `model=other-text-model` 并成功返回；无真实供应商 |
| H-160 隔离与相关回归 | `cargo test -p deepagent-app-core --lib canvas_model_gateway::tests --offline`：41 passed / 1 ignored；`cd apps/desktop && pnpm test:workflow`：78 passed；`cargo fmt --all -- --check` 与限定文件 `git diff --check` 通过。未运行 workspace 全量、Clippy、桌面构建/手动 UI、真实 Tauri 或公网供应商；默认忽略的缺陷未修复 |
| H-161 工作流节点到 bridge 参数对照 | `cargo test -p deepagent-runtime --lib llm_node_with_explicit_canvas_model_uses_the_bridge --offline -- --nocapture`：1 passed；正式节点 config 的 temperature=0.2/maxTokens=17 原样进入 CanvasCompletionRequest；第一次误加 `--exact` 只得到 0 tests，不算产品验证 |
| H-161 Canvas provider wire 探针 | `cargo test -p deepagent-app-core --lib audit_canvas_completion_honors_node_temperature_and_output_limit --offline -- --ignored --nocapture`：0 passed / 1 failed（预期）；精确 provider/model 请求成功，但本机 HTTP 捕获 body 不含 `temperature` / `max_tokens`；无公网供应商 |
| H-161 隔离与相关回归 | `cargo test -p deepagent-app-core --lib canvas_model_gateway::tests --offline`：41 passed / 2 ignored；`cargo test -p deepagent-runtime --lib workflow::agent::tests --offline`：28 passed；`cargo fmt --all -- --check` 与限定文件 `git diff --check` 通过。未运行 workspace 全量、Clippy、桌面 build/手动 UI、真实 Tauri 或公网供应商；默认忽略的 H-160/H-161 均未修复 |
| H-162 专业 AI 节点完整链探针 | `cargo test -p deepagent-app-core --lib audit_enabled_professional_ai_nodes_reach_the_canvas_provider --offline -- --ignored --nocapture`：0 passed / 1 failed（预期）；正式图编译与 WorkflowAgent 下，LLM/Agent V2 在 Canvas prompt contract 报 UnsupportedNode，问题分类/参数提取在 kernel dispatcher 报 UnsupportedNode，四组本机 provider 调用数均为 0 |
| H-162 隔离与相关回归 | `cargo test -p deepagent-runtime --lib registered_kinds_without_an_executor_report_unsupported_node --offline -- --nocapture`：1 passed，确认后二者是当前显式预期；`cargo test -p deepagent-app-core --lib canvas_model_gateway::tests --offline`：41 passed / 3 ignored；`cargo fmt --all -- --check` 与限定文件 `git diff --check` 通过。未运行 workspace 全量、Clippy、桌面 build/手动 UI 或公网供应商；缺陷未修复 |
| H-163 Canvas bridge usage 契约探针 | `cargo test -p deepagent-app-core --lib audit_canvas_completion_preserves_provider_usage_and_cache_details --offline -- --ignored --nocapture`：0 passed / 1 failed（预期）；本机 Chat 响应的 text/reasoning 正常保留，但 bridge response 没有 usage，100/20/80/20/12 全部无返回通道 |
| H-163 WorkflowAgent 与成本账本探针 | `cargo test -p deepagent-app-core --lib audit_canvas_provider_usage_reaches_workflow_run_accounting --offline -- --ignored --nocapture`：0 passed / 1 failed（预期）；可执行 text-gen 完成后 RunUsage 九项全 0，正式 AppRunFinalizer/SQLite CostService 的 input/output/hit/miss 也全 0 |
| H-163 run-level 兼容分支探针 | `cargo test -p deepagent-app-core --lib audit_run_level_workflow_model_preserves_deepseek_usage_buckets --offline -- --ignored --nocapture`：0 passed / 1 failed（预期）；Chat accumulator 已返回完整 usage，WorkflowAgent 仅留下 prompt=100/completion=20/total=120，billed prompt、cache 与 reasoning 全为 0 |
| H-163 隔离与相关回归 | `cargo test -p deepagent-app-core --lib canvas_model_gateway::tests --offline`：41 passed / 6 ignored；`cargo test -p deepagent-runtime --lib workflow::agent::tests --offline`：28 passed；`cargo fmt --all -- --check` 与限定文件 `git diff --check` 通过。未运行 workspace 全量、Clippy、桌面 build/手动 UI、完整 Kernel token-budget 探针或公网供应商；缺陷未修复 |
| H-164 Canvas text 运行取消显式探针 | `cargo test -p deepagent-app-core --test workflow_audit audit_text_cancel_does_not_wait_for_provider_response --offline -- --ignored --nocapture`：0 passed / 1 failed（预期）；正式取消 accepted 后 300ms 内未退出且运行记录未完成，释放本机 HTTP 响应后 text 节点 Completed、run 唯一终态 cancelled；动态只覆盖 text |
| H-164 边界与相关回归 | `CanvasCompletionRequest`/image/audio/embedding DTO 及共用 `CanvasModelGateway::call()` 均无取消输入；H-119 的 video 专用 cancel helper 不经过该边界。`cargo test -p deepagent-app-core --test workflow_audit --offline`：3 passed / 7 ignored；H-119 视频取消精确回归：1 passed，约 49ms 退出。未逐项动态复现 image/audio/embedding、真实 Tauri 或公网 provider |
| H-165 provider 返回 URL 的宿主第二跳显式探针 | `cargo test -p deepagent-app-core --test workflow_audit audit_provider_image_url_cannot_fetch_loopback_resource --offline -- --ignored --nocapture`：0 passed / 1 failed（预期）；正式 image-gen provider 返回第二台 loopback URL 后，宿主 GET `/host-private.png`，零审批，36 字节 canary 完整入库且 run succeeded；仅本机临时服务 |
| H-165 边界与相关回归 | 生产 `parse_output → fetch_binary → CanvasArtifactService` 直接消费 provider URL；`Cargo.lock` 与本地依赖源码确认 `reqwest 0.12.28` 默认最多跟随 10 跳。`cargo test -p deepagent-app-core --test workflow_audit --offline`：3 passed / 8 ignored；H-119 视频取消精确回归：1 passed，约 58ms 退出；H-164 文本取消显式探针仍为 0 passed / 1 failed（预期）。未动态覆盖 redirect/DNS rebinding、IPv6、真实私网/metadata、后续 artifact 外传或真实恶意供应商 |
| H-166 Flash/Pro `top_p` 显式探针 | Flash thinking：`cargo test -p deepagent-models --test reasoning_effort_audit audit_flash_thinking_mode_top_p_below_floor_reaches_wire_unchanged --offline -- --ignored --nocapture`；模型/模式对照：`cargo test -p deepagent-models --test reasoning_effort_audit audit_model_specific_top_p_rules_reach_wire --offline -- --ignored --exact --nocapture`；两项均 0 passed / 1 failed（预期），正式 ModelClient 捕获到配置 0.2 原样进入 wire。四组结果 `[(Pro, no tools, high, 0.2), (Pro, tools, high, 0.2), (Pro, no tools, none, 0.2), (Flash, no tools, none, 0.2)]`；官方有效值为 `[1.0, 0.95, 0.2, 1.0]`，Pro thinking 两组与 Flash none 不匹配，Pro none 对照匹配。仅离线 transport，未调用真实 API，未测回答质量 |
| H-167 provider image 超预算显式探针 | `cargo test -p deepagent-app-core --test workflow_audit audit_provider_image_download_rejects_oversized_artifact --offline -- --ignored --nocapture`：0 passed / 1 failed（预期）；双本机 loopback fixture 返回 32 MiB+1，宿主 `resp.bytes()` 后保存等长 artifact，终态 succeeded。只证明该样本，不宣称实测 OOM/磁盘耗尽；Penguin-Magic 10 MiB 对照也先整体物化 |
| H-168 动态 instructions 截断历史缓存前缀显式探针 | `cargo test -p deepagent-app-core --test kernel_v2_e2e cache_prefix::audit_dynamic_instructions_do_not_cut_off_conversation_cache_prefix --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；第一轮后新增 workspace 文件，第二轮仍保留第一轮 `input`，但 leading instructions 在 byte 12987 已变化，早于历史。仅离线 wire，不证明真实 provider 本次必然 miss |
| H-168 中途 system serializer 探针 | `cargo test -p deepagent-models responses::tests::audit_preserves_mid_history_system_messages_as_input_items --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；中途 system 被拼入 leading instructions，input 原位置消失。首次不完整测试名配 `--exact` 为 0 tests，不计入证据 |
| H-169 不支持图片格式显式探针 | `cargo test -p deepagent-app-core --test kernel_v2_e2e image_preparation::audit_unsupported_image_mime_is_rejected_before_transport --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；语法完整 SVG 经正式桌面同形 ingest/历史/Responses 链在第二轮成为 `data:image/svg+xml`，离线 transport 返回成功；未访问公网或伪造 provider 拒绝 |
| H-170 Files API 图片复用显式探针与正常对照 | `cargo test -p deepagent-app-core --test kernel_v2_e2e image_preparation::audit_retained_image_reuses_deepseek_file_id --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；三轮正式 wire 为 `inline=[0,1,1]`、`file_id=[0,0,0]`，后两轮重复同一 data URL，body 为 41,721 / 42,259 / 42,483 字节。`... image_preparation::retained_imported_image_reaches_continuation_request ... --exact --nocapture`：1 passed；真实 PNG/AttachmentService/AppCore，离线 transport，无 Files 或公网请求 |
| H-171 Responses 图片角色显式探针与正常对照 | `cargo test -p deepagent-models --test modalities_audit audit_responses_image_roles_preserve_or_reject_content --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；developer 与 assistant 均 `accepted=true, transport_calls=1, image_parts=0, content_was_string=true`。`cargo test -p deepagent-models --offline`：84 passed / 27 ignored；其中 modalities 对照 1 passed / 2 ignored，Flash user 图片和 Pro 文本正常。正式 ModelClient/serializer、离线 transport，无公网 |
| H-172 越界 Responses request 字段显式探针 | `cargo test -p deepagent-models --test responses_parameter_bounds_audit audit_out_of_range_responses_parameters_are_rejected_before_transport --offline -- --ignored --nocapture`：0 passed / 1 failed（预期）；temperature 2.1、top_p 1.1、top_logprobs 21 均 `error=false`，transport 调用 3 次且 wire 原样携带越界值；正式 ModelClient、离线 transport，无公网 |
| H-172 越界 ResponseDefaults 显式探针 | `cargo test -p deepagent-models --test responses_parameter_bounds_audit audit_out_of_range_response_defaults_are_rejected_before_transport --offline -- --ignored --nocapture`：0 passed / 1 failed（预期）；三种非法 default 各 `error=false, transport_calls=1`；正式 ModelClient defaults 合并路径、离线 transport，无公网 |
| H-172 官方范围边界对照 | `cargo test -p deepagent-models --test responses_parameter_bounds_audit audit_documented_responses_parameter_boundaries_reach_transport --offline -- --ignored --nocapture`：1 passed；temperature 0/2、top_p 0/1、top_logprobs 0/20 均进入离线 wire；未调用真实 DeepSeek |
| H-173 Responses 工具输出图片显式探针 | `cargo test -p deepagent-models --test modalities_audit audit_responses_tool_output_images_are_preserved_before_transport --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；正式 `Message::Tool`→`ResponseRequest`→`ModelClient`→recording transport 捕获 `output_image_parts=0`，`function_call_output.output` 为纯字符串；未调用公网或真实图片服务 |
| H-174 `replace_messages` 工具配对显式探针 | `cargo test -p deepagent-models --test response_repair_audit audit_replace_messages_repairs_dangling_responses_calls_before_transport --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；公开 mutator 后正式 Responses wire 为 `has_call=true, has_output=false`，transport 已被调用；仅离线 recording transport，无公网或工具副作用 |
| H-175 工作流 HTTP 节点网络边界显式探针 | `cargo test -p deepagent-app-core --test workflow_audit audit_workflow_http_node_does_not_bypass_network_approval --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；正式 `ChatService`/`WorkflowAgent`/`AgentKernel` 向临时 loopback 发出 1 次 GET，审批回调 0 次，run 为 `succeeded`；仅本机临时服务，无公网或用户数据 |
| H-176 工作流知识库作用域显式探针 | `cargo test -p deepagent-app-core --test workflow_audit audit_workflow_knowledge_base_id_is_honored --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；正式 `ChatService`/`WorkflowAgent`/`KnowledgeService` 在 `knowledgeBaseId=project` 下同时返回 project/global 两条临时文档，run 成功；仅临时 vault 与离线模型，无用户知识或公网 |
| H-177 工作流未知目标显式探针 | `cargo test -p deepagent-app-core --test workflow_audit audit_workflow_unknown_target_node_is_rejected --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；正式 `ChatService`/`WorkflowAgent`/`RunStore` 对 `target_node_id=missing-node` 只完成 `start` 却记录 `succeeded`；无模型请求、工具副作用或公网 |
| H-178 工作流代码节点执行显式探针 | `cargo test -p deepagent-app-core --test workflow_audit audit_workflow_code_node_executes_script_or_fails_loudly --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；正式 `ChatService`/`WorkflowAgent` 对明确无效 `codeScript` 发布 `code_status=Completed`、空对象输出且 run=`succeeded`；无脚本解释器、系统命令、网络或文件写入 |
| H-179 文档提取节点执行显式探针 | `cargo test -p deepagent-app-core --test workflow_audit audit_workflow_enabled_document_extractor_has_executor --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；正式 `ChatService`/`WorkflowAgent` 对 enabled `document-extractor` 返回 `UnsupportedNode`，节点 failed、run=`failed`；无模型请求或文件解析副作用 |
| H-180 人工输入生命周期显式探针 | `cargo test -p deepagent-app-core --test workflow_audit audit_workflow_human_input_uses_shared_waiting_lifecycle --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；正式 `ChatService`/`WorkflowAgent` 对合法 `action-0` 图边仍返回 `UnsupportedNode`，节点 failed、run=`failed`，没有等待/输入事件 |
| H-181 迭代节点输入契约显式探针 | `cargo test -p deepagent-app-core --test workflow_audit audit_workflow_iteration_processes_frontend_input_contract --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；正式 `WorkflowAgent` 对 `items=["ONE","TWO"]` 发布 `Completed` 但输出 `[]`，run=`succeeded`；无模型、工具、网络或文件副作用 |
| H-182 直接回答节点输出显式探针 | `cargo test -p deepagent-app-core --test workflow_audit audit_workflow_answer_node_renders_frontend_contract --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；正式 `WorkflowAgent` 对 `answerTemplate="EXPECTED_ANSWER"` 发布 `answer=""` 且 run=`succeeded`；无模型、工具、网络或文件副作用 |
| H-183 变量聚合节点输出显式探针 | `cargo test -p deepagent-app-core --test workflow_audit audit_workflow_variable_aggregator_consumes_frontend_bindings --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；正式 `WorkflowAgent` 对两个前端绑定发布 `output=[]` 且 run=`succeeded`；无模型、工具、网络或文件副作用 |
| H-184 必填工作流输入显式探针 | `cargo test -p deepagent-app-core --test workflow_audit audit_workflow_missing_required_input_is_rejected --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；正式 `WorkflowAgent` 对 `required=true` 缺失字段发布 `customer=null` 且 run=`succeeded`；无模型、工具、网络或文件副作用 |
| H-185 输入类型不匹配显式探针 | `cargo test -p deepagent-app-core --test workflow_audit audit_workflow_input_type_mismatch_is_rejected --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；正式 `WorkflowAgent` 对声明 number 的 `count="not-a-number"` 发布字符串值且 run=`succeeded`；无模型、工具、网络或文件副作用 |
| H-186 未声明输入显式探针 | `cargo test -p deepagent-app-core --test workflow_audit audit_workflow_undeclared_input_is_rejected_or_ignored --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；正式 `WorkflowAgent` 将 `unexpected="UNDECLARED_CANARY"` 复制进 start 输出且 run=`succeeded`；无模型、工具、网络或文件副作用 |
| H-187 工作流默认值显式探针 | `cargo test -p deepagent-app-core --test workflow_audit audit_workflow_declared_default_is_applied_when_input_is_missing --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；正式 `WorkflowAgent` 对声明 `default="FALLBACK"` 的缺失输入发布 `greeting=null` 且 run=`succeeded`；无模型、工具、网络或文件副作用 |
| H-188 工作流字符串选项显式探针 | `cargo test -p deepagent-app-core --test workflow_audit audit_workflow_string_options_are_enforced --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；正式 `WorkflowAgent` 对 `options=["safe","fast"]` 接受 `mode="UNDECLARED_MODE"` 且 run=`succeeded`；无模型、工具、网络或文件副作用 |
| H-189 工作流数值范围显式探针 | `cargo test -p deepagent-app-core --test workflow_audit audit_workflow_numeric_bounds_are_enforced --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；正式 `WorkflowAgent` 对范围 `[10,20]` 接受 `retries=3` 且 run=`succeeded`；无模型、工具、网络或文件副作用 |
| H-190 工作流字符串长度显式探针 | `cargo test -p deepagent-app-core --test workflow_audit audit_workflow_string_length_bounds_are_enforced --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；正式 `WorkflowAgent` 对长度 `[5,8]` 接受单字符 `token` 且 run=`succeeded`；节点值按既有规则脱敏，无模型、工具、网络或文件副作用 |
| H-191 工作流缺失 start 显式探针 | `cargo test -p deepagent-app-core --test workflow_audit audit_workflow_requires_a_start_node --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；正式 `WorkflowAgent` 对只有 `end` 的图发布 2 条节点事件并记录 `succeeded`；无模型、工具、网络或文件副作用 |
| H-192 工作流缺失 end 显式探针 | `cargo test -p deepagent-app-core --test workflow_audit audit_workflow_requires_an_end_node --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；正式 `WorkflowAgent` 对只有 `start` 的图发布 2 条节点事件并记录 `succeeded`；无模型、工具、网络或文件副作用 |
| H-193 工作流不可达 end 显式探针 | `cargo test -p deepagent-app-core --test workflow_audit audit_workflow_requires_a_reachable_end_node --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；start/end 均存在但零边，正式 `WorkflowAgent` 仍发布 4 条节点事件并记录 `succeeded`；无模型、工具、网络或文件副作用 |
| H-194 工作流声明顺序稳定性显式探针 | `cargo test -p deepagent-app-core --test workflow_audit audit_workflow_execution_scope_is_not_node_order_dependent --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；相同节点/边仅交换数组位置，completed 集合分别为 `start,end` 与 `start,orphan,end`，两个 run 都为 `succeeded`；无模型、工具、网络、命令或文件副作用 |
| H-195 工作流多个可达终点显式探针 | `cargo test -p deepagent-app-core --test workflow_audit audit_workflow_does_not_succeed_with_a_reachable_branch_unsettled --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；左右两条 start 可达路径各有 end，实际仅左侧三节点 completed 后 run 即 `succeeded`，右侧分支无任何终态事件；无模型、工具、网络、命令或文件副作用 |
| H-196 工作流合法目标依赖闭包显式探针 | `cargo test -p deepagent-app-core --test workflow_audit audit_workflow_target_node_keeps_required_ancestor_outputs --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；整图控制组 target 得到 `SOURCE_VALUE` 并成功，指定合法 target 后 source=Skipped、target=Failed、run=`failed`，错误为 required output unavailable；无模型、工具、网络、命令或文件副作用 |
| H-197 工作流输出类型契约显式探针 | `cargo test -p deepagent-app-core --test workflow_audit audit_workflow_end_output_matches_declared_type --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；end 声明 `count:number`，实际发布 `{"count":"NOT_A_NUMBER"}`、end=Completed、run=`succeeded`；无模型、工具、网络、命令或文件副作用 |
| H-198 工作流 OR 条件显式探针 | `cargo test -p deepagent-app-core --test workflow_audit audit_workflow_or_condition_does_not_start_from_true --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；两个原生 OR 子条件均为 false，节点却输出 `branch=true`、run=`succeeded`；只验证 decision，不复用 H-39 的错误派发作为判据；无模型、工具、网络、命令或文件副作用 |
| H-199 工作流多 case ELSE 端口显式探针 | `cargo test -p deepagent-app-core --test workflow_audit audit_workflow_multi_case_condition_keeps_else_handle --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；两个 case 都不匹配且显式连接 executor 定义的 `false` fallback，正式编译以 invalid source handle 拒绝，run=`failed`；无模型、工具、网络、命令或文件副作用 |
| H-200 Responses lifecycle identity/sequence 显式探针 | `cargo test -p deepagent-app-core --test kernel_v2_e2e native_output_items::audit_responses_lifecycle_retains_provider_identity_and_sequence --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；正常 `response.created`/`response.completed` fixture 含同一 response ID、provider model 与序号 0/7，但正式 `run_events` 仅保留 Runtime envelope 和两个 `event_type`，provider 字段全部丢失；run 仍成功，无公网/工具副作用 |
| H-201 无限画布缩放焦点显式探针 | `$env:RUN_HARNESS_AUDIT='1'; node --test apps/desktop/scripts/audit-workflow-zoom-anchor.test.cjs`：0 passed / 1 failed（预期）；动态执行底栏 zoom-in/out/reset/滑杆与小地图 zoom-in/out 六个生产 handlers；平移后中心世界坐标随缩放漂移；无 Tauri、图数据写入或模型请求 |
| H-202 无限画布视口重启恢复显式探针 | `$env:RUN_HARNESS_AUDIT='1'; node --test apps/desktop/scripts/audit-workflow-viewport-persistence.test.cjs`：0 passed / 1 failed（预期）；真实生产 persistence 写入只含 `nodes/edges`，全新 store viewport 回到 `{0,0,1}`，节点 `audit-node` 恢复、ReactFlow 启用 fitView 且未设置 defaultViewport；Tauri transport 为内存替身，无真实 WebView |
| H-203 Responses 输入 role 显式探针及合法对照 | `cargo test -p deepagent-models --test responses_input_role_audit --offline -- --include-ignored --nocapture`：1 passed / 1 failed（预期）；`Message` 和 `InputMessage` 的 `moderator` 均通过正式 ModelClient 到达 recording transport；`user/assistant/system/developer` 四种官方角色对照通过；无公网，未观察真实 provider 错误行为 |
| DeepSeek reasoning serializer 及静态 prompt cache 前缀对照 | `cargo test -p deepagent-models --lib reasoning_effort_serializes_canonical_wire_values --offline -- --nocapture`：1 passed；`cargo test -p deepagent-app-core --lib cacheable_prefix --offline -- --nocapture`：3 passed；`cargo test -p deepagent-app-core --test kernel_v2_e2e cache_prefix::consecutive_session_requests_preserve_responses_prefix_on_wire --offline -- --exact --nocapture`：1 passed。本地同会话、环境不变时，双轮 wire 的 input 前缀、instructions 与工具顺序断言通过；该对照不覆盖 H-168 的动态变化，也不保证真实 provider cache 命中。 |
| H-154 修复前 Responses `user` 探针 | `cargo test -p deepagent-app-core --test kernel_v2_e2e audit_responses_user_identifier_is_validated_before_transport --offline -- --ignored --nocapture`：修复前 0 passed / 1 failed（预期）；非法值越过设置校验并进入离线 wire |
| H-154 设置和最终 provider 边界验证 | settings DTO/developer JSON 测试、ModelClient 直达 transport 测试及 ChatService→ModelClient 集成测试均通过；非法 identifier 的 transport 调用数为 0，合法 1/512 字符边界各发送一次；未访问公网 |
| H-141 todo reminder 生命周期显式探针 | `cargo test -p deepagent-app-core --test kernel_v2_e2e todo_reminder_lifecycle::audit_todo_reminder_counts_survive_independent_session_turns --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期），约 1.47s；正式同一 session 连续 11 次独立用户回合、实际请求 11 次，`reminder_requests=[]`；仅临时内存数据库与离线 transport |
| H-140 输入 hook 上下文显式探针及对照 | `cargo test -p deepagent-app-core --test kernel_v2_e2e prompt_hook_context --offline -- --include-ignored --nocapture`：1 passed / 1 failed（预期），执行约 2.59s；真实 PowerShell 成功输出普通文字/标准 additionalContext，两组实际 wire 与会话消息仍无内容；直接输入可见、顶层 block 不发模型。仅临时文件、离线 transport |
| H-140 默认 Kernel 集成及静态检查 | `cargo test -p deepagent-app-core --test kernel_v2_e2e --offline`：30 passed / 29 ignored / 0 failed，约 4.59s；`cargo fmt --all -- --check` 与 `git diff --check` 通过。仅测试和报告，未运行 workspace 全量、Clippy、桌面 build、真实 Tauri、其他平台或真实供应商；ignored 缺陷并未修复 |
| H-139 后置 hook 失败与重开显式探针 | `cargo test -p deepagent-app-core --test kernel_v2_e2e post_tool_hook_commit --offline -- --include-ignored --nocapture`：1 passed / 1 failed（预期），最终复测约 0.82s；真实 write_file 成功但无 session 结果、action 留在 Running；重开标为进程重启失败，正式下一 wire 只含同 call_id 的补造失败；正常 hook 保留真实结果。仅临时文件与离线模型 |
| H-139 默认 Kernel 集成及静态检查 | `cargo test -p deepagent-app-core --test kernel_v2_e2e --offline`：29 passed / 28 ignored / 0 failed，约 4.00s；`cargo fmt --all -- --check`、`git diff --check` 通过。仅测试与报告；未跑 workspace 全量、Clippy、桌面 build、真实 Tauri、macOS/Linux 或真实供应商，默认忽略的缺陷仍未修复 |
| H-138 MCP 客户端 metadata 显式探针 | `cargo test -p deepagent-app-core --test kernel_v2_e2e mcp_image_output::audit_mcp_client_metadata_is_not_sent_to_model --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期），约 0.35s；仅位于 MCP 结果顶层 _meta 的虚构 canary 在下一正式 wire 出现，公开 content/structuredContent 仍存在；本机真实 Node，无公网发送 |
| H-138 公开内容对照与原图像探针复核 | `cargo test -p deepagent-app-core --test kernel_v2_e2e mcp_image_output --offline -- --include-ignored --nocapture`：1 passed / 2 failed（两项预期缺陷），约 0.47s；无 _meta 组公开文本/结构化内容保留、canary 不出现；H-138 仍失败；共享夹具的 H-127 仍复现 string output、typed_input_image=false、raw_json_string=true。不能将旧图像缺口当作本轮新增回归 |
| H-138 默认 Kernel 集成与静态检查 | `cargo test -p deepagent-app-core --test kernel_v2_e2e --offline`：28 passed / 27 ignored，约 4.01s；`cargo fmt --all -- --check`、`git diff --check` 通过。仅测试和报告；未跑 workspace 全量、Clippy、桌面 build、真实 Tauri、其他平台或公网供应商 |
| H-137 MCP 目录变更显式探针 | `cargo test -p deepagent-app-core --test kernel_v2_e2e mcp_catalog_change::audit_mcp_list_changed_reaches_next_run --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期），约 1.70s；服务端确已替换目录并发通知，tools/list 仍仅 1 次，下一独立 run 的正式 wire 继续携带旧工具；真实本机 Node、离线模型，无公网请求 |
| H-137 不变目录对照及默认集成基线 | `cargo test -p deepagent-app-core --test kernel_v2_e2e mcp_catalog_change --offline -- --nocapture`：1 passed / 1 ignored，约 0.63s；`cargo test -p deepagent-app-core --test kernel_v2_e2e --offline`：27 passed / 26 ignored，约 2.67s。`cargo fmt --all -- --check`、`git diff --check` 通过。仅测试与文档，未跑 workspace 全量、Clippy、桌面 build、真实 Tauri、macOS/Linux 或真实供应商 |
| H-136 MCP stdin 背压取消显式探针 | `cargo test -p deepagent-app-core --test kernel_v2_e2e mcp_cancellation::audit_stopping_run_interrupts_backpressured_stdio_write --offline -- --ignored --exact --nocapture`：512 KiB 当前探针 0 passed / 1 failed（预期暴露缺口），约 1.68s；初轮 2 MiB 也同样失败，约 1.32s。正式运行中取消 accepted，但 250ms 后仍等待发送；释放 gate 后完整请求到达，读回终态 cancelled、模型请求 1 次；本机 Node，无公网副作用 |
| H-136 原响应等待取消对照 | `cargo test -p deepagent-app-core --test kernel_v2_e2e mcp_cancellation::stopping_run_interrupts_inflight_stdio_mcp_call --offline -- --exact --nocapture`：1 passed，约 0.53s；小参数完整发出后停止可退出等待并发送取消通知，终态 cancelled、模型请求 1 次；用于区分 H-128 已修复的阶段与新增写入阶段缺口 |
| H-136 Kernel 集成默认基线与静态检查 | `cargo test -p deepagent-app-core --test kernel_v2_e2e --offline`：26 passed / 25 ignored，约 2.67s；共用夹具的原 MCP 取消回归通过。`cargo fmt --all -- --check`、`git diff --check` 通过。仅审计测试和报告，未运行 workspace 全量、Clippy、桌面 build、真实 Tauri、macOS/Linux 或真实供应商 |
| H-135 stdio MCP 环境继承显式探针 | `cargo test -p deepagent-mcp --test env_inheritance_audit audit_stdio_does_not_inherit_ambient_credentials --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期暴露缺口），约 0.85s；显式覆盖对照命令 `... audit_stdio_preserves_explicit_environment_overrides ...`：1 passed，约 0.34s。隔离宿主只使用虚构 canary，真实 Node 经正式 transport 握手/列工具；未读取真实凭据、未联网外传 |
| H-135 默认 MCP 基线与静态检查 | `cargo test -p deepagent-mcp --features http --offline`：61 单测、2 真实进程和 1 stderr 诊断测试通过，共 64 passed / 12 ignored；`cargo fmt --all -- --check`、`git diff --check` 通过。本轮仅增加审计测试和报告，未跑 workspace 全量、Clippy、桌面 build、真实 Tauri 或其他平台 |
| H-120 durable sink 修复复核 | `cargo test -p deepagent-app-core --test workflow_audit --offline`：3 passed / 6 ignored（取消、成功对照、节点事件重开回放）；`cargo test -p deepagent-runtime --lib workflow::node_events::tests --offline`：5 passed；格式与 diff 检查通过。Runtime Clippy `--no-deps -D warnings` 被未改动的 `model_agent.rs:225 clone_on_copy` 阻断；未运行真实 Tauri/桌面崩溃与断线重连 |
| H-119 视频运行取消修复 | `cargo test -p deepagent-app-core --test workflow_audit --offline`：2 passed / 7 ignored，包含停滞状态 HTTP 的正式运行取消和正常视频成功对照；`cargo test -p deepagent-app-core --lib canvas_model_gateway::tests --offline`：39 passed，包含下载 body 停滞取消后无临时 artifact；`cargo fmt --all -- --check`、`git diff --check` 通过。完整 AppCore Clippy 受既有 `deepagent-persistence::run_store` explicit-counter-loop 阻断；`--no-deps` 下新增 `post_json` 参数告警已通过 `VideoCancellation` options struct 消除，仍被 context_runtime 的 clone-on-copy、plugin_preparation type-complexity 与 run_assembler too-many-arguments 等既有告警阻断。未执行真实 Tauri、云端视频生成或桌面按钮验证；本地取消不代表撤销云端任务 |
| H-129 一次性 MCP 测试连接 deadline 修复 | `cargo test -p deepagent-app-core --lib mcp_service::tests::test_server_deadlines_stdio_initialize --offline -- --nocapture` 与 `...test_server_deadlines_stdio_tools_list...`：均 1 passed；正式 MCP 探测路径 + 本机 Node stdio server 分别挂起 initialize、tools/list 至 2 秒 deadline，返回 failed/timeout 并关闭子进程。`cargo test -p deepagent-app-core --lib mcp_service::tests --offline`：26 passed / 1 ignored；`cargo fmt --all -- --check` 通过。未访问公网或用户数据 |
| H-130 app-server 重复 steer 显式审计探针 | `cargo test -p deepagent-cli audit_repeated_steer_does_not_launch_multiple_replacements --offline -- --ignored --nocapture`：0 passed / 1 failed（预期）；正式 `ServerState::dispatch` 对同一旧 turn 两次返回成功并各生成一个 replacement launch。没有执行 spawn_turn、provider、工具或真实 stdio 子进程 |
| H-131 活跃模型流固定总期限显式探针 | `cargo test -p deepagent-models --features http reqwest_transport::tests::audit_active_provider_stream_is_not_cut_off_by_total_deadline --offline -- --ignored --nocapture`：0 passed / 1 failed（预期）；loopback fixture 每 50ms 发 progress，正式 transport 收到 4 个事件后于 `[DONE]` 前返回总时限错误。`cargo test -p deepagent-runtime audit_partial_response_is_streamed_but_not_committed_after_total_deadline --offline -- --ignored --nocapture`：1 passed；ModelAgent 对三次 deadline failure 都发出 partial delta，最终失败且不提交 assistant item 到 history。均无公网或真实 DeepSeek 请求 |
| H-132 Chat Completions 内容过滤和 usage 显式探针 | `cargo test -p deepagent-models audit_chat_content_filter_preserves_typed_finish_reason_and_usage --offline -- --ignored --nocapture`：预期 0 passed / 1 failed；正式 `ModelClient::stream_response()` Chat 路由收到无内容 `content_filter` + 9 prompt tokens usage 后，在 finish reason 映射处返回 provider error。`cargo test -p deepagent-models chat_completions::tests --offline`：9 passed；`cargo fmt --all -- --check`、`git diff --check` 通过。MockTransport，无公网 |
| H-133 Responses failed usage 显式探针 | `cargo test -p deepagent-models audit_failed_responses_terminal_emits_included_usage --offline -- --ignored --nocapture`：0 passed / 1 failed（预期）；正式 ModelClient 返回 provider error，但 observer 未收到该终态中 9 input、2 output、4 cached、1 reasoning usage。`cargo test -p deepagent-models --offline`：83 passed / 11 ignored；`tests/modalities_audit.rs`：1 passed / 1 ignored；`cargo fmt --all -- --check`、`git diff --check` 通过。MockTransport，无公网 |
| H-134 旧画布 run 迟到事件显式探针 | `$env:RUN_HARNESS_AUDIT='1'; pnpm exec node --test --test-name-pattern='a superseded workflow attempt' scripts/audit-workflow-isolation.test.cjs`：预期 0 passed / 1 failed，输出 `staleAttemptOverwroteCurrent=true, cancelRequestsOnSupersede=0`；生产 `workflowExecutor.ts` + 延迟 Tauri listener mock，新 run Running 后旧 run Completed 改写节点；无真实 Kernel/provider/云端任务 |
| H-128 运行取消中的 MCP 工具回归 | `cargo test -p deepagent-app-core --test kernel_v2_e2e mcp_cancellation::stopping_run_interrupts_inflight_stdio_mcp_call --offline -- --nocapture`：1 passed；正式 AppCore/Kernel/McpService/stdio 子进程中，取消 accepted=true 后 250ms 内退出、服务端收到 `notifications/cancelled`、工具调用有配对失败结果且模型请求数仍为 1；另有 `cargo test -p deepagent-mcp --features http --offline` 覆盖 HTTP 取消通知，`cargo test -p deepagent-runtime --offline --lib` 覆盖 token 终态 |
| H-127 MCP 图像工具结果显式审计探针 | `cargo test -p deepagent-app-core --test kernel_v2_e2e mcp_image_output::audit_mcp_image_result_is_not_serialized_as_text_json --offline -- --ignored --nocapture`：0 passed / 1 failed（预期）；真实本机 Node MCP stdio + 正式 AppCore/Kernel/Responses serializer，下一请求 output 为包含原始图片 JSON/base64 的字符串，无 `input_image` block；离线 provider，无公网 |
| H-126 stdio MCP 响应大小显式审计探针（本轮复核） | `cargo test -p deepagent-mcp --test response_limit_audit audit_stdio_transport_accepts_oversized_response_line --offline -- --ignored --nocapture`：0 passed / 1 failed（预期）；正式 `StdioTransport` 接受并解析 2 MiB JSONL payload，小响应对照通过；本机临时 Node 子进程，无真实插件副作用 |
| H-126 HTTP MCP 响应大小显式审计探针（本轮复核） | `cargo test -p deepagent-mcp --features http --test response_limit_audit audit_http_transport_accepts_oversized_response_body --offline -- --ignored --nocapture`：0 passed / 1 failed（预期）；正式 `HttpTransport` 接受并解析 2 MiB JSON body，小响应对照通过；仅 loopback fixture，无公网服务 |
| H-125 SSH 主机密钥显式审计探针（本轮复核） | `cargo test -p deepagent-ssh --test host_key_audit --offline -- --ignored --nocapture`：0 passed / 1 failed（预期暴露缺陷）；真实生产 `SshService.connect` 接受本机临时随机 host key，库的 `KnownHostsFile` 对照拒绝随机错误 key。仅 loopback、临时服务与测试口令；没有公网/用户 SSH 连接。本轮未修改生产实现 |
| H-123 修复前本地终端与 SSH 游标探针 | 两个正式 reader 在 64 KiB 上限时均为 0 passed / 1 failed；保留本行作为缺陷证据，不能当作当前状态 |
| H-123 修复后本地终端与 SSH 回归 | `cargo test -p deepagent-app-core --test terminal_audit audit_direct_terminal_cursor_preserves_unreturned_output --offline -- --nocapture`：1 passed，真实 Windows ConPTY/SQLite；`cargo test -p deepagent-ssh audit_ssh_terminal_cursor_preserves_unreturned_output --offline -- --nocapture`：1 passed，正式 SSH session reader/受控 transport；两端第一页游标均等于实际返回长度，后页取回剩余输出 |
| H-123 相关正常基线 | `cargo test -p deepagent-ssh -p deepagent-terminal --offline`：SSH 1 passed / 2 ignored（含需真实服务器的既有集成测试），terminal 4 passed；`cargo test -p deepagent-app-core --lib terminal_ --offline`：9 passed，含终端服务、SQLite 租约和取消存储故障回归。未执行本批 workspace 全量、Clippy、桌面 build 或真实 Tauri 点击 |
| H-124 视觉发送前停止跨层探针（修复前） | `RUN_HARNESS_AUDIT=1 node --test scripts/audit-vision-preflight-cancel.test.cjs`：文本对照通过，继续会话/新会话的视觉等待组各 0 passed / 1 failed；释放和 listener 清理完成后，停止失败且第二张图片和主 `start_chat_v2` 仍发出。无真实图片内容、供应商、Tauri 窗口或数据库 |
| H-124 视觉发送前停止跨层探针（修复后） | 同一命令 **3 passed / 0 failed**：续聊、新会话视觉预处理停止均无后续视觉请求、无 `start_chat_v2`、无 `cancelNotFound`；已接纳文本对照仍可取消。`check-process-collapse.test.cjs` **12 passed / 0 failed**；真实 Tauri/供应商未执行 |
| H-122 修复前 provider 来源替换探针 | `audit_video_resume_rejects_replaced_provider_identity`：0 passed / 1 failed，约 0.41s；替换地址后 B 收到旧 task ID。保留为缺陷证据 |
| H-122 修复后 provider 来源替换探针 | `cargo test -p deepagent-app-core --test workflow_audit audit_video_resume_submits_new_task_for_replaced_provider_identity --offline -- --ignored --nocapture`：**1 passed**；同来源续跑 submit=1，替换来源后新端点 submit=1/status=1，`old_task_polled=false`，终态 succeeded；仅本机 HTTP |
| H-122 来源探针默认基线与 fixture 所有权复核 | `cargo test -p deepagent-app-core --test workflow_audit --offline`：1 passed / 8 ignored，约 0.17s；fixture 新增 provider service 保活后，H-120 重开测试显式 drop 该 owner，再按原命令复测仍 0 passed / 1 failed，约 0.17s，实时 6/落盘 0 的结论不变。fmt/diff 检查通过；未跑 workspace、Clippy、桌面构建或真实账号 |
| H-122 修复前提示词修改探针 | `audit_edited_video_prompt_submits_a_new_task`：0 passed / 1 failed，约 0.72s；只 submit 一次且第二次 poll 旧 ID。保留为缺陷证据 |
| H-122 修复后提示词修改往返 | `cargo test -p deepagent-app-core --test workflow_audit audit_edited_video_prompt_submits_a_new_task --offline -- --ignored --nocapture`：**1 passed**；真实生产 TS 更新/serializer→正式 Rust 工作流，`submits=2, polls=2, new_prompt_sent=true`，第二次 poll 新 task ID |
| H-122 输入/provider 指纹单测 | `cargo test -p deepagent-app-core --lib video_resume_identity_covers_effective_inputs_and_provider_generation --offline` 与 `...hmac_sha256_matches_rfc_4231_test_case...`：均 **1 passed**；覆盖 prompt/model/protocol/endpoint/credential version/key/size/reference/task ID，固定 HMAC 向量通过，持久化值不含明文 API Key |
| H-122 运行中 task/fingerprint 跨层探针 | `cargo test -p deepagent-app-core --test workflow_audit media::audit_desktop_video_progress_keeps_task_id_for_resume --offline -- --ignored --nocapture`：**1 passed**；Running 和 Completed 的 task ID + fingerprint 均通过生产 TS store/serializer 续传 |
| H-122 默认基线及旧探针复核 | 提取跨语言测试 helper 后，`cargo test -p deepagent-app-core --test workflow_audit --offline`：1 passed / 7 ignored，约 0.17s；H-121 原命令显式复测仍 0 passed / 1 failed，约 0.47s，完成对照保留 ID、进度组仍丢失；未将 helper 重构说成生产修复 |
| H-122 完成通知对照调整后的复核 | 脚本的 Completed 分支改用本运行完成通知正常结束后，H-122 与 H-121 各自原命令仍均为 0 passed / 1 failed，分别约 0.65s / 0.42s；同一新提示词轨迹及 ID 丢失结论不变。仅审计测试/报告，未跑 workspace 全量、Clippy、完整桌面 build 或真实云端视频生成 |
| H-119 耗时标注更正后的复核与 H-122 检查 | H-119 原命令再次显式复现，0 passed / 1 failed，约 0.47s；`cancel_to_exit_ms=338` 含 300ms 观察期，最终仍唯一 cancelled。`cargo fmt --all -- --check` 与 `git diff --check` 通过，耗时输出更正没有改变生产行为 |
| H-121 视频进度跨语言显式探针 | `cargo test -p deepagent-app-core --test workflow_audit media::audit_desktop_video_progress_keeps_task_id_for_resume --offline -- --ignored --nocapture`：1 passed；真实 Rust producer 与 TS executor/Zustand/下一请求 serializer 中，Running 和 Completed 两组均保存并续传同一 task ID；未发真实云端视频请求 |
| H-121 默认前端回归与构建 | 桌面目录 `pnpm test:workflow`：78 passed；新增回归实际运行生产 executor 的 running event→停止→再提交链路，第二份 config 保留 provider ID。`pnpm build` 全部脚本、TypeScript 检查和 Vite production build 通过；Vite 给出既有 browser externalization 与大 chunk 警告。未执行真实 Tauri/WebView 或云端视频生成 |
| H-120 节点事件重开显式探针 | `cargo test -p deepagent-app-core --test workflow_audit media::audit_video_node_events_survive_database_reopen --offline -- --exact --nocapture`：1 passed；修复后实时 6 条、规范日志重开前后均 6 条，task ID、artifact 来源和 succeeded 终态保持一致 |
| H-120 默认基线与持久化 sink 对照 | `cargo test -p deepagent-app-core --test workflow_audit --offline`：1 passed / 5 ignored，约 0.16s；`cargo test -p deepagent-runtime --lib kernel::tests::kernel_injects_durable_sink_into_streaming_agent --offline`：1 passed，约 0.13s；`cargo fmt --all -- --check` 与 `git diff --check` 通过。仅测试/报告，未跑 workspace 全量、Clippy、桌面构建或真实崩溃恢复 |
| H-119 视频 HTTP 取消显式探针与成功对照 | `cargo test -p deepagent-app-core --test workflow_audit media:: --offline -- --include-ignored --nocapture`：1 passed / 1 failed，约 0.48s；取消已受理但 300ms 内未退出，释放响应后唯一 cancelled；成功对照完成下载入库。首次编译有仅用于临时目录保活的字段未读取警告，后改为 `_root`；不作为产品缺陷 |
| H-119 默认基线与检查 | `cargo test -p deepagent-app-core --test workflow_audit --offline`：1 passed / 4 ignored，约 0.20s；重新编译无上述警告，`cargo fmt --all -- --check` 与 `git diff --check` 通过。本批仅审计测试与报告，未跑 workspace 全量、Clippy、桌面构建、真实 Tauri 或云端视频 API |
| H-88 在途刷新凭据覆盖显式探针 | `cargo test -p deepagent-app-core --lib settings::tests::audit_refresh_models_does_not_restore_a_superseded_api_key --offline -- --ignored --nocapture`：0 passed / 1 failed，约 0.19s；sign_out、replace_key 两组均未保持新状态，第三连接读回 configured=true；正式文件 SQLite 加密存储、临时包装 Key、离线 transport，无公网/真实 Tauri |
| H-87/H-88 强化凭据身份断言后合跑 | `cargo test -p deepagent-app-core --lib settings::tests::audit_refresh_models_ --offline -- --ignored --nocapture`：0 passed / 2 failed，约 0.22s；H-87 严格落盘沙箱断言失败，H-88 两组均 previous_restored=true；没有将“凭据缺失或任意不同值”误当作恢复旧凭据 |
| H-88 默认回归与检查 | `cargo test -p deepagent-app-core --lib settings::tests --offline`：30 passed / 2 ignored，约 0.50s；`cargo test -p deepagent-app-core --lib secret_store::tests --offline`：5 passed，约 0.02s；`cargo fmt --all -- --check`、`git diff --check` 通过。本批只增加审计探针及报告，未运行全 workspace、Clippy、前端构建、真实 Tauri 或跨进程复测；两项缺陷保持未修复 |
| H-87 多宿主设置覆盖显式探针 | `cargo test -p deepagent-app-core --lib settings::tests::audit_refresh_models_preserves_other_host_sandbox_mode --offline -- --ignored --nocapture`：0 passed / 1 failed；两个独立 SQLite 文件连接交错写入后，刷新将另一连接成功保存的 `ReadOnly` 覆为旧 `WorkspaceWrite`；第三连接读回确认落盘结果；临时数据库与离线 GateTransport，未启动真实多进程桌面或调用公网 |
| H-87 默认设置基线与检查 | `cargo test -p deepagent-app-core --lib settings::tests --offline`：30 passed / 1 ignored，约 0.54s；单服务沙箱设置刷新对照通过；`cargo fmt --all -- --check`、`git diff --check` 通过。缺陷探针需显式执行，默认通过不代表已修复；本批仅测试与文档，未跑全 workspace、Clippy、前端构建、真实 Tauri 或多进程测试 |
| H-89 显式模型选择探针 | `cargo test -p deepagent-app-core --test kernel_v2_e2e model_selection --offline -- --include-ignored --nocapture`：2 passed / 1 failed；未知显式模型实际发送为 `deepseek-v4-flash` 并成功终止，默认与已知显式模型以及非法 provider 对照均通过；临时 SQLite/离线 transport，无公网或完整 CLI stdio/Tauri |
| H-90 关闭 writer 后 pending 泄漏探针 | `cargo test -p deepagent-harness-sdk --lib audit_request_after_close_does_not_leak_pending_reply --offline -- --ignored --nocapture`：0 passed / 1 failed；BrokenPipe 后 pending=1；Windows 临时 `cmd.exe` 管道 fixture，无真实 CLI server；同 crate 默认单测 4 passed / 1 ignored |
| H-91 并发事件序号倒退显式探针 | `cargo test -p deepagent-cli app_server::tests::audit_concurrent_event_producers_keep_sequences_in_wire_order --offline -- --ignored --nocapture`：0 passed / 1 failed；确定性调度得到 `reserved=[1,2] enqueued=[2, 1]`。未启动真实 stdio server/客户端；这不是持久化丢事件测试 |
| H-91 app-server 默认测试基线 | `cargo test -p deepagent-cli app_server::tests --offline`：8 passed / 0 failed / 5 ignored；H-91 为显式 ignored 审计探针，默认通过不代表该交错已修复 |
| H-92 runtime shutdown 活动 turn 显式探针 | `cargo test -p deepagent-cli terminal_audit::audit_runtime_shutdown_finalizes_in_flight_stdio_turn --offline -- --ignored --nocapture`：0 passed / 1 failed；模型请求确已进入 transport，runtime drop 后 `running_turn` 未终结、`finished_at=None`、`run_terminal=0`；正式 AppCore run 与临时 SQLite |
| H-92 stdio 子进程 EOF 显式探针 | `cargo test -p deepagent-cli app_server::terminal_audit::audit_stdio_eof_drops_active_turn_in_child_process --offline -- --ignored --nocapture`：0 passed / 1 failed；测试可执行文件子进程运行生产 `run_stdio`，父进程经真实管道完成三次 RPC 并等到 transport 入场标记再关闭 stdin；子进程 exit code 0，SQLite 仍为 `running_turn`、`finished_at=None`、`run_terminal=0`。无发行版 CLI 子进程、Harness SDK 或公网供应商 |
| H-01 慢消费者真实 stdout 管道显式探针 | `cargo test -p deepagent-cli --offline audit_stdio_slow_output_drops_terminal_on_real_pipe -- --ignored --nocapture`：审计断言通过，表示复现成立；生产 outbox 在真实 Windows 子进程 stdout 管道未读 250 ms 时，1,024 条终态类型通知只收到 272 条，`critical-terminal` 丢失，读取后输出共 1,157,606 字节。探针未运行正式 ChatService/RunStore/SDK；未测试 Tauri |
| H-01 ACK 边界后续显式探针 | `cargo test -p deepagent-cli app_server::tests::audit_event_ack_rejects_unemitted_sequence --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；无任何已发通知时 ACK `u64::MAX` 仍返回 accepted，证明 ACK 未绑定已分配事件或 durable replay cursor；生产 ServerState/SQLite 内存数据库，无模型、工具或公网 |
| H-93 Chat 命名工具选择请求/默认值显式探针 | `cargo test -p deepagent-models audit_chat_tool_choice --offline -- --ignored --nocapture`：0 passed / 2 failed；request 与 defaults 两路径均序列化为 Responses 形状而非 DeepSeek Chat 形状；仅 adapter/serializer 离线复现，无公网 provider 请求 |
| H-94 Chat 连续 reasoning item 显式探针 | `cargo test -p deepagent-models audit_chat_projection_preserves_all_reasoning_items --offline -- --ignored --nocapture`：0 passed / 1 failed；输入 `first ` 与 `second`，输出仅含 `second`；仅 adapter/serializer 离线复现，无公网 provider 请求 |
| H-95 同一 session 多 run 的恢复分类显式探针 | `cargo test -p deepagent-app-core --test recovery_terminal_audit audit_recovery_classifies_each_run_from_its_own_task_turn --offline -- --ignored --nocapture`：0 passed / 1 failed；旧 run 的 tool interruption 被分类为 `interrupted_prompt`，最新 run 的 `interrupted_prompt` 分类正确；正式 AppService/SQLite 内存数据库与 session projection，无真实进程崩溃或桌面验收 |
| H-95 默认探针基线与检查 | `cargo test -p deepagent-app-core --test recovery_terminal_audit --offline`：0 passed / 2 ignored；两条均为显式审计探针，默认通过不代表问题已修复。`cargo fmt --all -- --check`、`git diff --check` 通过；本批未跑全 workspace、Clippy、桌面构建或真实 Tauri |
| H-96 派发后记账失败/重启重放显式探针 | `cargo test -p deepagent-runtime --test schedule_audit audit_post_fire_bookkeeping_failure_replays_same_occurrence --offline -- --ignored --nocapture`：1 passed；one-shot 与 recurring 均在 callback 后触发存储错误，重建 scheduler 后 callback count=2。正式 `CronScheduler` 与 Windows 文件权限、临时 workspace；未执行 `ChatService`/模型/工具，也未启动真实桌面 |
| H-96 默认探针基线与检查 | `cargo test -p deepagent-runtime --test schedule_audit --offline`：0 passed / 2 ignored；H-42/H-96 均是显式审计探针。`cargo fmt --all -- --check`、`git diff --check` 通过；本批未跑全 workspace、Clippy、桌面构建或真实 Tauri |
| H-97 scheduler shutdown 与在途 run 显式探针 | `cargo test -p deepagent-app-core --lib audit_scheduler_shutdown_leaves_started_cron_run_active --offline -- --ignored --nocapture`：1 passed，预期复现为 scheduler 已返回、run 未终结、模型取消未观察到；之后通过正式 run cancel 完成清理。真实 CronService/ChatService/RunStore、挂起型离线 transport；未关闭整个 Tauri/CLI 进程、未调用公网或工具 |
| H-98 Chat JSON 输出请求级与默认值显式探针 | `cargo test -p deepagent-models audit_chat_json_object_format_reaches_provider_request --offline -- --ignored --nocapture`：0 passed / 1 failed，显式 `json_object` 在 transport 前被拒绝；`cargo test -p deepagent-models audit_chat_default_json_object_format_reaches_provider_request --offline -- --ignored --nocapture`：0 passed / 1 failed，正式 Chat 请求到达受控 transport，但 wire 未含 `response_format`。官方 DeepSeek Chat 文档核对支持 `text/json_object`；无公网调用，桌面默认 Responses 路径不在本项范围 |
| H-99 Chat `top_logprobs` 默认值显式探针 | `cargo test -p deepagent-models audit_chat_top_logprobs_default_is_rejected_before_transport --offline -- --ignored --nocapture`：0 passed / 1 failed；配置 `top_logprobs=5` 后正式 Chat 运行使用合法离线响应完成，transport 被调用一次，证明配置默认未与请求级值共用不支持校验。官方文档要求 `logprobs=true`；无公网调用，桌面默认 Responses 路径不在本项范围 |
| H-100 SDK initialize 畸形 response 显式探针 | `cargo test -p deepagent-harness-sdk --lib audit_initialize_rejects_response_without_result_or_error --offline -- --ignored --nocapture`：0 passed / 1 failed；实际 `result=None, error=None` 仍返回 Ok 并设置 `initialized=true`。Windows PowerShell 子进程和本地管道 fixture；未启动真实 CLI server、第三方 server 或公网 |
| H-101 SDK 畸形 stdout 行显式探针 | `cargo test -p deepagent-harness-sdk --lib audit_malformed_server_line_does_not_leave_request_pending --offline -- --ignored --nocapture`：0 passed / 1 failed；畸形行发出并由标记确认后，调用仍 pending、表内保留 1 项，直到 fixture EOF 才释放。Windows PowerShell 子进程和真实本地管道，无 CLI server 或公网 |
| H-204 SDK 对端静默时的请求 deadline / abandonment 显式探针 | `cargo test -p deepagent-harness-sdk --lib client::audit_tests::audit_unresponsive_server_request_has_a_bounded_wait --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；对端收到合法请求并保持 stdout 打开，300ms 内 SDK 未完成；取消调用 task 后 pending=1；仅 fixture EOF 才释放。Windows PowerShell 子进程，无真实 CLI server 或公网 |
| H-205 SDK 通知外层 eventSequence 丢失显式探针 | `cargo test -p deepagent-harness-sdk --lib client::audit_tests::audit_sdk_notification_preserves_event_sequence_for_ack_and_gap_detection --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；真实 PowerShell stdout 输出 eventSequence=7、9，经生产 `spawn_reader` 后公开事件序列化结果均无 eventSequence，无法观察 7→9 缺口；无模型、工具、用户文件或公网 |
| H-207 SDK `thread/start` 成功结果 schema 显式探针 | `cargo test -p deepagent-harness-sdk --lib client::audit_tests::audit_thread_start_rejects_success_result_without_thread_id --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；initialize 合法通过后，正式 `thread_start()` 对 `result={}` 返回 Ok，缺少 `threadId` 未被拒绝；Windows PowerShell 子进程与本地管道，无真实 CLI server、模型、工具或公网 |
| H-208 Responses `response.completed` 终态 schema 显式探针 | `cargo test -p deepagent-models --test responses_terminal_contract_audit audit_completed_event_without_required_response_fields_is_rejected --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；正式 `ModelClient → ResponseAccumulator` 对 `response={}` 仍返回 `Ok(Response)`、`finish_reason=Stop`、`usage=None`，缺少 `response.id/status` 未被拒绝；受控离线 transport，无真实供应商、工具、文件或公网 |
| H-209 Responses 输出 message role 显式探针 | `cargo test -p deepagent-models --test responses_output_item_contract_audit audit_output_message_without_role_is_rejected --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；完整终态和文本 delta 对照下，`output_item.done` 缺少 `role` 仍被正式 `ResponseAccumulator` 默认补成 `assistant` 并返回 `Ok(Response)`；受控离线 transport，无真实供应商、工具、文件或公网 |
| H-210 Responses 非单调 sequence 显式探针 | `cargo test -p deepagent-models --test responses_sequence_audit audit_non_monotonic_responses_sequence_is_rejected --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；同一响应发送 `sequence_number=0,0,4,2`（重复、跳号、逆序），正式 `ModelClient → ResponseAccumulator` 仍返回 `Ok(Response)`、`finish_reason=Stop`，未拒绝或标记流损坏；受控离线 transport，无真实供应商、工具、文件或公网 |
| H-211 Harness SDK 字符串 response ID 显式探针 | `cargo test -p deepagent-harness-sdk --lib client::audit_tests::audit_string_response_id_does_not_leave_request_pending --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；正式 Windows PowerShell fixture 返回 `{"jsonrpc":"2.0","id":"1","result":{}}`，`spawn_reader` 将字符串 ID 变为整数 0，`thread/list` 在 fixture 保持连接期间未完成且 pending=1；无真实 CLI server、模型、工具、文件或公网 |
| H-212 Harness SDK JSON-RPC 版本显式探针 | `cargo test -p deepagent-harness-sdk --lib client::audit_tests::audit_wrong_jsonrpc_version_does_not_complete_request --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；正式 Windows PowerShell fixture 返回 `{"jsonrpc":"1.0","id":1,"result":{}}`，`thread/list` 经 `spawn_reader` 直接返回 `Ok(RpcResponse)`，响应 `jsonrpc` 保持为 `1.0`；无真实 CLI server、模型、工具、文件或公网 |
| H-213 Harness SDK notification method 路由显式探针 | `cargo test -p deepagent-harness-sdk --lib client::audit_tests::audit_non_harness_notification_method_is_not_emitted_as_event --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；正式 Windows PowerShell fixture 返回 `method:"other/event"` 但合法 `HarnessEvent` params，reader 忽略 method 并广播为 `ThreadStarted { thread_id: "t-wrong-method" }`；无真实 CLI server、模型、工具、文件或公网 |
| H-214 Harness SDK 请求后订阅竞态显式探针 | `cargo test -p deepagent-harness-sdk --lib client::audit_tests::audit_subscribe_after_request_preserves_pre_response_event --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；正式 Windows PowerShell stdio fixture 对两次 `thread/start` 都先发合法 `harness/event` 再发匹配 response，迟订阅组 response 成功但 300ms 内事件为 `None`，预订阅对照收到 `t-early`；无真实 CLI server、模型、工具、文件或公网 |
| H-215 Harness 归档后运行接纳显式探针 | `cargo test -p deepagent-cli app_server::tests::audit_archived_thread_does_not_accept_new_turn --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；正式 `ServerState` dispatcher 依次完成 `thread/start`、`thread/archive`、`thread/list`、`turn/start`，输出 `archive_success=true durable_archived=true still_listed=true turn_success=true launch=true`；只创建 `TurnLaunch`，未 spawn turn、调用模型、工具或公网 |
| H-215 CLI 默认测试基线 | `cargo test -p deepagent-cli --offline`：85 passed / 16 ignored / 0 failed；H-215 探针默认忽略，绿色基线不表示缺陷已修复；本轮仅新增审计测试与文档，未修改生产行为或协议 |
| H-216 Harness 冷分叉 readiness 显式探针 | `cargo test -p deepagent-cli app_server::tests::audit_cold_thread_fork_is_loaded_for_the_next_turn --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；文件 SQLite 经新 `ServerState` 冷启动后，输出 `fork_success=true fork_loaded=false turn_success=false launch=false`；显式 resume 正向对照为 `resume_success=true resumed_turn_success=true resumed_launch=true`；未 spawn turn、调用模型、工具或公网 |
| H-216 CLI 默认测试基线 | `cargo test -p deepagent-cli --offline`：85 passed / 17 ignored / 0 failed；H-215/H-216 审计探针默认忽略，绿色基线不表示任一缺陷已修复；本轮仅新增 H-216 测试与文档 |
| H-217 Harness JSON-RPC 方法错误分类显式探针 | `cargo test -p deepagent-cli app_server::tests::audit_unknown_namespaced_method_is_not_reported_as_invalid_params --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；`thread/does-not-exist` 返回 `-32602`，未知无斜杠 method 返回 `-32601`，已知 `thread/start` 坏参数返回 `-32602`；无模型、工具、文件或公网 |
| H-218 Harness thread/read 历史分页显式探针 | `cargo test -p deepagent-cli app_server::tests::audit_thread_read_has_bounded_history_page --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；600 条消息样本返回 `events=1201`、`serialized_bytes=1570066`、`has_continuation=false`；正式 SQLite/dispatcher，无模型、工具、文件、子进程或公网 |
| H-219 Harness tool/list 作用域显式探针 | `cargo test -p deepagent-cli app_server::tests::audit_tool_list_is_not_scoped_to_thread_capabilities --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；`read_only/direct` 与 `full_access/windows_sandbox` 两个 thread 均返回 `28` 个相同工具，`first_has_snapshot=false`、`second_has_snapshot=false`；无模型、工具执行、子进程或公网 |
| H-220 Harness thread/list 分页显式探针 | `cargo test -p deepagent-cli app_server::tests::audit_thread_list_has_bounded_page --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；创建 120 个真实 thread 后返回 `threads=120`、`serialized_bytes=25009`、`has_continuation=false`；正式 SQLite/dispatcher，无模型、工具、子进程或公网 |
| H-221 Harness config/read 有效配置显式探针 | `cargo test -p deepagent-cli app_server::tests::audit_config_read_is_not_a_static_protocol_banner --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；thread/start 指定 `deepseek-v4-pro`、`high`、`read_only` 与 `windows_sandbox` 后，正式 `config/read` 仍只返回 `protocolVersion`、`provider`、`transport`，`model/reasoningEffort/permissionProfile/sandboxBackend` 均缺失；正式 SQLite/dispatcher，无模型、工具、子进程或公网 |
| H-222 Harness thread/start reasoning 贯通显式探针 | `cargo test -p deepagent-cli app_server::tests::audit_thread_start_reasoning_effort_is_not_silently_dropped --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期）；正式 `thread/start` 携带 `reasoningEffort=high` 后无覆盖 `turn/start` 的 `TurnLaunch.overrides.reasoning_effort=None`，但 `turn/start` response 仍成功；正式 SQLite/dispatcher，无模型、工具、子进程或公网 |
| H-223 模型目录刷新保留选型显式探针 | `cargo test -p deepagent-app-core --test kernel_v2_e2e model_selection::audit_catalog_refresh_preserves_selected_model_on_provider_wire --offline -- --ignored --exact --nocapture`：0 passed / 1 failed（预期），约 0.69s；默认组 wire `[Flash,Flash]`，显式 Pro 组 `[Pro,Flash]`，重开 SQLite 后分别为 Flash/Flash；四次 run 各有唯一 succeeded，目录两次完全相同且 Pro 仍存在；只有 discovery/模型 transport 是离线夹具 |
| H-224 成本日历与预算接纳探针 | `cargo test -p deepagent-app-core --test kernel_v2_e2e cost_calendar:: --offline -- --include-ignored --nocapture`：1 passed / 2 failed（预期），约 0.48s；日/月账本各为 13、周期成本各误为 1、限额 5 仍各 dispatch 一次且唯一 succeeded；当前时刻 12 元日/月对照均在 dispatch 前拒绝。真实系统时间与文件 SQLite；账单为合成历史数据，模型为离线 transport |
| H-225 首次登录计价激活探针 | `cargo test -p deepagent-app-core --test kernel_v2_e2e pricing_activation:: --offline -- --include-ignored --nocapture`：1 passed / 1 failed（预期），约 3.15s；首次登录费用 NULL/汇总 0、两次 wire 均 Flash、两次 succeeded，usage/cache 未丢；已配置启动与刷新同步对照各计 0.000616 元、第二轮预算拒绝且总 wire 数 1。临时文件 SQLite，离线模型用量，无真实扣费 |
| H-226 fallback 成本路由探针 | `cargo test -p deepagent-app-core --test kernel_v2_e2e fallback_cost:: --offline -- --include-ignored --nocapture`：1 passed / 3 failed（预期），约 1.28s；四组同模型对照正确；Flash→Pro 实计 0.000616/应计 0.002370 且下一轮放行，反向实计 0.002370/应计 0.000616 且误拦截；Flash 成功工具请求后再切 Pro，累计 input=2000/cache hit=1600，实计 0.001232/应计 0.002986，下一轮仍放行。完整服务与临时文件 SQLite、离线 transport，无真实费用 |
| H-102 InstructionsLoaded 外部 hook payload 显式探针 | `cargo test -p deepagent-app-core --lib audit_instructions_loaded_hook_receives_required_file_metadata --offline -- --ignored --nocapture`：0 passed / 1 failed；正式 ChatService 装配实际运行 Windows PowerShell hook，payload 有 `paths` 但缺 `file_path`、`memory_type`、`load_reason`；临时项目和离线模型 transport |
| H-103 Pre/Post tool hook 输入显式探针 | `cargo test -p deepagent-runtime --lib audit_external_tool_hooks_include_call_id_and_result --offline -- --ignored --nocapture`：0 passed / 1 failed；正式 pipeline 发出 provider ID，但 hook stdin 缺 `tool_use_id`；Post `tool_response={"ok":true}`，真实 fixture 结果 `{"value":"audit"}` 未传递；Windows PowerShell hook、无模型/公网 |
| H-103 PostToolUseFailure hook 输入显式探针 | `cargo test -p deepagent-runtime --lib audit_external_tool_failure_hook_includes_call_id_and_error --offline -- --ignored --nocapture`：0 passed / 1 failed；正式失败工具执行后收到 hook，stdin 缺 `tool_use_id` 与标准 `error`，只含 `tool_response={"ok":false}`；Windows PowerShell hook，无模型/公网 |
| H-104 hook 公共输入显式探针 | `cargo test -p deepagent-app-core --lib audit_external_hook_base_payload_includes_cwd_and_transcript_path --offline -- --ignored --nocapture`：0 passed / 1 failed；正式 SessionStart hook 捕获到的 payload 只有 `hook_event_name` 和 `session_id`，缺 Claude schema 必需的 `cwd`、`transcript_path`；Windows 临时项目、离线 transport |
| H-105 Stop hook 事件字段显式探针 | `cargo test -p deepagent-app-core --lib chat_service::tests::audit_stop_hook_payload_includes_stop_hook_active --offline -- --ignored --nocapture`：0 passed / 1 failed；正式 ChatService/RuntimeEngine 完成门触发已授权项目的 PowerShell Stop hook，捕获 payload 有 `content` 但缺 Claude schema 必需的 `stop_hook_active`；Windows 临时项目、离线 transport |
| H-105 默认基线与检查 | `cargo test -p deepagent-app-core --lib chat_service::tests --offline`：74 passed / 6 ignored；`cargo fmt --all -- --check` 与 `git diff --check` 通过。本轮仅新增 ignored 审计探针与报告，未跑 workspace 全量测试、Clippy、桌面构建、真实 Tauri 或公网 hook |
| H-106 StopFailure 失败分支显式探针 | `cargo test -p deepagent-app-core --lib chat_service::tests::audit_stop_failure_hook_payload_includes_error --offline -- --ignored --nocapture`：0 passed / 1 failed；Stop 首次拒绝、StopFailure 真实派发、模型重试后成功；payload 缺 Claude schema 必需的 `error`，仅有文本 `content`；Windows 临时项目、离线 transport |
| H-106 默认基线与检查 | `cargo test -p deepagent-app-core --lib chat_service::tests --offline`：74 passed / 7 ignored；`cargo fmt --all -- --check` 与 `git diff --check` 通过。本轮仅新增 ignored 审计探针与报告，未跑 workspace 全量测试、Clippy、桌面构建、真实 Tauri 或公网 hook |
| H-107 SessionEnd 生命周期显式探针 | `cargo test -p deepagent-app-core --lib chat_service::tests::audit_session_end_hook_does_not_fire_after_each_continued_turn --offline -- --ignored --nocapture`：0 passed / 1 failed；两个 turn 共用同一 session id，真实 PowerShell hooks 实际记录 `SessionStart, SessionEnd, SessionEnd`。临时 workspace、内存 SQLite、离线模型 transport |
| H-107 默认基线与检查 | `cargo test -p deepagent-app-core --lib chat_service::tests --offline`：74 passed / 8 ignored；`cargo fmt --all -- --check` 与 `git diff --check` 通过。本批仅增加 ignored 审计探针与报告，未修改生产逻辑、未跑 workspace 全量测试、Clippy、桌面构建或真实 Tauri |
| H-108 Session lifecycle metadata 显式探针 | `cargo test -p deepagent-app-core --lib chat_service::tests::audit_session_lifecycle_hooks_include_required_event_metadata --offline -- --ignored --nocapture`：0 passed / 1 failed；捕获的 SessionStart/SessionEnd payload 分别缺官方 schema 必需的 `source`/`reason`；真实 Windows PowerShell hooks、临时 workspace、内存 SQLite 与离线 transport |
| H-108 默认基线与检查 | `cargo test -p deepagent-app-core --lib chat_service::tests --offline`：74 passed / 9 ignored；`cargo fmt --all -- --check` 与 `git diff --check` 通过。本批仅增加 ignored 审计探针与报告，未修改生产逻辑、未跑 workspace 全量测试、Clippy、桌面构建或真实 Tauri |
| H-109 SubagentStop payload 显式探针 | `cargo test -p deepagent-app-core --lib subagent_runner::tests::audit_subagent_stop_hook_includes_required_fields --offline -- --ignored --nocapture`：0 passed / 1 failed，约 1.09s；正式 ChatSubagentRunner 的 PowerShell hook 捕获到缺少 `stop_hook_active` 与 `agent_transcript_path` 的 Stop payload，Start/Stop 均真实触发，离线模型 transport |
| H-109 默认基线 | `cargo test -p deepagent-app-core --lib subagent_runner::tests --offline`：6 passed / 5 ignored；5 个 ignored 包含现存审计项和 H-109，默认通过不表示 ignored 项已修复 |
| H-110 Pre/PostCompact schema 显式探针 | `cargo test -p deepagent-app-core --lib compact_payload_includes_required_fields --offline -- --ignored --nocapture`：0 passed / 2 failed；两个正式 hook payload 均泄露 `trigger=context_overflow`，PreCompact 另缺 `custom_instructions` |
| H-111 PreCompact stdout 指令显式探针 | `cargo test -p deepagent-app-core --lib audit_precompact_stdout_is_used_as_compaction_instructions --offline -- --ignored --nocapture`：0 passed / 1 failed；hook stdout 的唯一标记未进入 ScriptedTransport 捕获的 summary request body |
| H-110/H-111 默认基线与检查 | `cargo test -p deepagent-app-core --lib context_runtime::tests --offline`：23 passed / 6 ignored；`cargo fmt --all -- --check` 与 `git diff --check` 通过。本批仅增加 ignored 审计探针与报告，未修改生产逻辑、未跑 workspace 全量测试、Clippy、桌面构建或真实 Tauri |
| H-112 工作树 hook payload 显式探针 | `cargo test -p deepagent-app-core --lib audit_worktree_hook_payloads_include_required_fields --offline -- --ignored --nocapture`：0 passed / 1 failed；WorktreeCreate/WorktreeRemove/CwdChanged 均实际仅发 `path` |
| H-113 文件变更 hook payload 显式探针 | `cargo test -p deepagent-runtime --lib audit_file_changed_hook_payload_includes_required_fields --offline -- --ignored --nocapture`：0 passed / 1 failed；真实工具写入触发 hook，捕获 `path` / `change_kind=created`，缺 schema 要求的 `file_path` / `event=add` |
| H-112 默认基线 | `cargo test -p deepagent-app-core --lib subagent_runner::tests --offline`：6 passed / 6 ignored；ignored 包含 H-109、H-112 和既有子 Agent 审计项 |
| H-113 默认基线 | `cargo test -p deepagent-runtime --lib tool_pipeline::tests --offline`：11 passed / 5 ignored；ignored 包含 H-103、H-113 和既有审批审计项 |
| H-114 后台子 Agent Notification payload 显式探针 | `cargo test -p deepagent-app-core --lib audit_background_subagent_notification_includes_required_fields --offline -- --ignored --nocapture`：0 passed / 1 failed；真实 PowerShell hook 捕获正式后台子 Agent 的 Notification payload，缺少 schema 必需的 `notification_type`；离线模型 transport、临时目录，无公网 |
| H-114 默认基线与格式检查 | `cargo test -p deepagent-app-core --lib subagent_runner::tests --offline`：6 passed / 7 ignored；ignored 包含 H-109、H-112、H-114 和既有子 Agent 审计项。`cargo fmt --all -- --check` 通过；本批仅增加 ignored 审计探针与报告，未修改生产行为、未跑 workspace 全量测试、Clippy、桌面构建或真实 Git worktree 子 Agent |
| H-115 PermissionDenied payload 显式探针 | `cargo test -p deepagent-runtime --lib audit_permission_denied_hook_includes_tool_call_id --offline -- --ignored --nocapture`：0 passed / 1 failed，约 0.65s；权限拦截、工具零执行、Observation/SQLite 的固定 ID 关联全部通过，真实 PowerShell hook 独缺 `tool_use_id` |
| H-115 默认基线 | `cargo test -p deepagent-runtime --lib loop_engine::tests --offline`：36 passed / 2 ignored，约 3.54s；ignored 包含 H-34、H-115，默认通过不代表缺陷已修复。本批仅测试和报告，未跑 workspace 全量、Clippy、桌面构建、真实 Tauri 或公网供应商 |
| H-116 TaskCompleted 派发显式探针 | `cargo test -p deepagent-runtime --lib audit_failed_task_does_not_fire_task_completed_hook --offline -- --ignored --nocapture`：0 passed / 1 failed，约 1.09s；正常完成对照与步数上限失败均执行了真实 PowerShell 完成 hook，失败 task_id 与 payload 一致；未验证默认无上限对话或完整桌面 |
| H-116 默认基线与检查 | `cargo test -p deepagent-runtime --lib loop_engine::tests --offline`：36 passed / 3 ignored，约 5.11s；`cargo fmt --all -- --check` 通过。本批只增加审计测试和报告，未修改生产行为，未跑 workspace 全量、Clippy、桌面构建或真实 Tauri |
| H-117 API 失败 hook 显式探针 | `cargo test -p deepagent-app-core --lib audit_provider_error_dispatches_stop_failure_hook --offline -- --ignored --nocapture`：0 passed / 1 failed，约 1.89s；成功组实际有 SessionStart/Stop，API 失败组只有 SessionStart，双方各一条正确规范终态；临时 SQLite 与真实 PowerShell hooks，模型传输离线 |
| H-117 默认基线与检查 | `cargo test -p deepagent-app-core --lib chat_service::tests --offline`：74 passed / 10 ignored，约 1.39s；`cargo fmt --all -- --check` 与 `git diff --check` 通过。本批只增加 ignored 审计探针与报告，未修改生产行为，未跑 workspace 全量、Clippy、桌面构建、真实 Tauri 或公网模型故障 |
| H-118 流式溢出恢复显式探针 | 修复前同一 code 的英文对照请求 2 次/压缩 1 次，中文组请求 1 次/压缩 0 次并返回 Unknown，ignored 断言为 0 passed / 1 failed；修复后 `cargo test -p deepagent-runtime --lib model_agent::tests::audit_streamed_overflow_recovers_from_provider_code_without_message_keywords --offline -- --exact --nocapture`：1 passed / 0 failed，英文与中文组均请求 2 次/压缩 1 次并成功；正式 parser/ModelAgent、离线 transport、计数型摘要 fixture |
| H-118 完整测试与检查 | `cargo test -p deepagent-models --features http --offline`：96 passed / 10 ignored；`cargo test -p deepagent-runtime --offline`：275 passed / 15 ignored；`cargo clippy -p deepagent-models --all-targets --features http --offline -- -D warnings`、`cargo fmt --all -- --check` 与 `git diff --check` 通过。Runtime Clippy 被既有 `deepagent-persistence::run_store` explicit-counter-loop 阻断；加 `--no-deps` 后另被既有 `model_agent.rs:225` clone-on-copy 阻断，均不在本次 H-118 diff。`cargo test --workspace --offline` 在 AppCore 阶段为 1080 passed / 1 failed / 50 ignored 后停止，失败项是无关的 real-build fixture 未在期限内创建 `build-started`；同项精确重跑 1 passed。workspace 未完整通过；未跑桌面构建、真实 Tauri 或公网供应商流内溢出 |
| 长度恢复用量与重开正向验证 | `cargo test -p deepagent-app-core --test kernel_v2_e2e incomplete_reason --offline -- --nocapture`：2 passed / 1 ignored，最终约 0.37s；成功/耗尽两组实际分别 3/5 次请求，逐条 usage 与落盘总量对应，重开不丢失。`cargo fmt --all -- --check` 与 `git diff --check` 通过；仅测试与报告，未跑 workspace、Clippy 或桌面构建 |
| 修改长度 fixture 后复核旧 H-82 | `cargo test -p deepagent-app-core --test kernel_v2_e2e audit_content_filter_does_not_retry_as_output_limit --offline -- --ignored --nocapture`：0 passed / 1 failed，约 0.32s；过滤组仍实际发出 3 次请求并升级/续写，账本断言通过但原因恢复断言失败，不将默认测试通过报告成 H-82 已修复 |
| H-112/H-113 格式与差异检查 | `cargo fmt --all -- --check` 与 `git diff --check` 通过；本批仅增加 ignored 审计探针与报告，未修改生产行为、未跑 workspace 全量测试、Clippy、桌面构建或真实 Git worktree 子 Agent |
| Responses `tool_search_call` 候选差异核查 | DeepSeek 官方请求 schema 不提供可触发该 item 的工具类型；AppCore `tool_search` 以普通 function schema 注册。`cargo test -p deepagent-app-core --lib tool_manifest::tests --offline`：4 passed；`cargo test -p deepagent-models --lib stream::tests --offline`：21 passed。未新增缺陷或测试，未调用公网 provider |
| H-86 GET 反射凭据回归 | `cargo test -p deepagent-models --features http reqwest_transport::tests:: --offline -- --nocapture`：通过；本机 401 服务反射 Authorization 后，结构化错误不含完整占位 key |
| H-86 doctor 诊断传播回归 | `cargo test -p deepagent-app-core --lib doctor::tests::audit_api_key_check_redacts_credentials_reflected_by_get_errors --offline -- --nocapture`：1 passed；正式 `check_api_key` 的 detail 不含完整占位 key，临时内存 DB/secret store，无公网 |
| 模型 API 跨 origin 重定向凭据对照 | `cargo test -p deepagent-models --features http --offline reqwest_transport::tests::cross_origin_redirect_does_not_forward_bearer_authorization -- --nocapture`：1 passed；首跳带占位 Bearer key、302 到另一端口后的第二跳无 Authorization，目标响应成功；仅本机 loopback |
| H-85 GET 响应停滞回归 | `cargo test -p deepagent-models --features http reqwest_transport::tests:: --offline -- --nocapture`：8 passed / 2 ignored；头部停滞、200 半截 body、503 半截 body 均在 50ms 预算内收尾，取消版半截 body 在 500ms 内返回 `request cancelled`；仅本机 HTTP，无公网或真实凭据 |
| H-83 完成后钩子失败显式探针 | `cargo test -p deepagent-cli terminal_audit::audit_final_hook_error_has_one_matching_live_and_durable_terminal --offline -- --ignored --nocapture`：0 passed / 1 failed，约 0.32s；实际收到 `turn.completed → turn.failed`，durable 唯一终态为 failed；失败断言准确指出实时多发了 completed |
| H-83/H-84 默认测试基线 | `cargo test -p deepagent-cli terminal_audit --offline`：3 passed / 0 failed / 3 ignored；H-83、H-84 与 AutoReview 补充缺陷探针均需显式运行，默认通过不代表问题已修复 |
| H-84 工具审批重复事件显式探针 | `cargo test -p deepagent-cli terminal_audit::audit_tool_approval_emits_one_correlated_event --offline -- --ignored --nocapture`：0 passed / 1 failed，约 0.27s；同一 turn 实际收到 2 条审批请求，一条带 call ID、一条无 ID；取消后目标路径不存在。离线 Responses fixture 与临时 SQLite |
| H-84 AutoReview hook Ask 显式探针 | `cargo test -p deepagent-cli terminal_audit::audit_auto_review_hook_ask_emits_before_auto_allow --offline -- --ignored --nocapture`：1 passed / 0 failed，约 0.92s；观察到一条无 ID 的 `approval.requested`，随后 `read_file` 成功、turn 正常完成。离线脚本 hook、两段 Responses fixture 与临时 SQLite |
| H-83/H-84 终态与审批显式探针合跑 | `cargo test -p deepagent-cli terminal_audit --offline -- --include-ignored --nocapture --test-threads=1`：4 passed / 2 failed，约 2.35s；H-83 和 AlwaysAsk H-84 缺陷探针分别复现实时终态冲突和审批事件重复，AutoReview 补充探针与三个正常对照通过 |
| H-82 incomplete 原因显式探针及长度对照 | `cargo test -p deepagent-app-core --test kernel_v2_e2e incomplete_reason --offline -- --include-ignored --nocapture`：1 passed / 1 failed，约 0.50s；两组请求数/预算/部分文本续写及成功终态相同；过滤组不应走长度恢复的断言失败，仅离线 transport 和临时 SQLite |
| H-82 收紧断言后的复测及默认基线 | 最终显式探针仍 1 passed / 1 failed，约 0.37s；严格确认预算升级和实际长度续写 user item，未强制过滤一律不能重试。`cargo test -p deepagent-app-core --test kernel_v2_e2e --offline`：22 passed / 22 ignored，约 2.06s；`cargo test -p deepagent-models --offline`：单元 82 passed / 3 ignored、集成 1 passed / 1 ignored。fmt、diff 检查通过；本批仅审计测试与报告，未跑全 workspace、Clippy、桌面构建、真实 Tauri 或公网过滤请求；默认通过不代表忽略问题已修复 |
| H-77 搜索敏感文件显式探针与正常对照 | `cargo test -p deepagent-app-core --test kernel_v2_e2e file_search_policy --offline -- --include-ignored --nocapture`：1 passed / 1 failed，约 0.85s；三组 direct read 均硬拒绝、grep 均成功且敏感 canary 进入下一 wire，普通 *.txt 对照无泄漏；虚构临时文件和离线 transport |
| H-77 共享集成入口基线 | `cargo test -p deepagent-app-core --test kernel_v2_e2e --offline`：18 passed / 17 ignored，约 1.71s；fmt、diff 检查通过。缺陷探针另行显式失败；本批仅新增测试和报告，未跑全 workspace、Clippy、前端构建、真实 Tauri 或公网模型 |
| H-78 Windows junction 显式探针与权限对照 | `cargo test -p deepagent-app-core --test kernel_v2_e2e path_boundary --offline -- --include-ignored --nocapture`：1 passed / 1 failed，约 0.46s；WorkspaceWrite 直接外部路径拒绝、FullAccess 直接外部路径允许，junction 组仍将 canary 带入下一 wire 且 read_file/tool_completed 成功；Windows 临时文件、离线 transport，无用户路径 |
| H-79 文件发现名称显式探针与普通文件对照 | `cargo test -p deepagent-app-core --test kernel_v2_e2e file_listing_policy --offline -- --include-ignored --nocapture`：1 passed / 1 failed，约 0.59s；read_file 被拒绝，glob/list_dir 均成功并将 `.env` 与 `visible.txt` 带入下一 wire；普通文件控制通过；仅临时 fixture 和离线 transport |
| H-80 Responses 工具类型配对显式探针 | `cargo test -p deepagent-models --lib audit_repairs_mismatched_custom_output_kind_before_serialization --offline -- --include-ignored --nocapture`：0 passed / 1 failed，约 0.00s；人为混型输入的正式 ResponseRequest 序列化结果保留 `function_call_output`，未校正为 `custom_tool_call_output`；无公网调用 |
| H-80 正常 Session 配对对照 | `cargo test -p deepagent-session --offline`：14 passed，测试体约 0.36s，包括 `native_custom_tool_completion_keeps_custom_pairing` 与 `appending_tool_trajectory_persists_responses_items`；本批 fmt、diff 检查通过；该对照不把公共边界探针解释成生产坏历史。未执行全 workspace、前端构建、真实 Tauri 或公网供应商验证 |
| H-81 多文本/done-only item 与磁盘恢复显式探针 | `cargo test -p deepagent-app-core --test kernel_v2_e2e native_output_items --offline -- --include-ignored --nocapture`：1 passed / 2 failed，约 0.42s；多 item 变三个持久化/续轮 wire item，done-only 多出一个空 item，单 item 对照通过；仅离线 provider fixture 与临时磁盘 |
| H-81 单终态补强及共享集成基线 | 增加 done-only 探针和续聊唯一 succeeded 终态断言后显式重跑仍 1 passed / 2 failed；`cargo test -p deepagent-app-core --test kernel_v2_e2e --offline`：21 passed / 21 ignored，约 1.95s。fmt、diff 检查通过；本批仅测试与文档，没有全 workspace、Clippy、前端构建、真实 Tauri 或公网供应商验证；默认通过不代表忽略缺陷已修复 |
| H-78 共享集成入口基线 | `cargo test -p deepagent-app-core --test kernel_v2_e2e --offline`：19 passed / 18 ignored，约 1.83s；非 ignored 物理路径权限对照通过，junction 缺陷探针另行显式失败；fmt、diff 检查通过，本批未跑全 workspace、Clippy、前端构建、真实 Tauri 或公网模型 |
| H-76 文件版本显式探针与正常对照 | `cargo test -p deepagent-app-core --test kernel_v2_e2e file_version --offline -- --include-ignored --nocapture`：1 passed / 1 failed，约 0.66s；不改文件及外部保存后重读均正常，过期读的 write_file 仍成功覆盖外部内容；真实临时磁盘/生产流水线/下一请求配对结果，transport 离线 |
| H-76 共享集成入口基线 | `cargo test -p deepagent-app-core --test kernel_v2_e2e --offline`：17 passed / 16 ignored，约 1.85s；新增缺陷探针默认忽略，不能把该结果说成过期写已修复。fmt、diff 检查通过；本批未跑全 workspace、Clippy、前端构建、真实 Tauri 或公网模型 |
| H-76 现有文件工具测试基线 | `cargo test -p deepagent-builtins --lib file_tools::tests --offline`：36 passed / 0 ignored，约 0.11s；含未读拒绝、已读可改、写后缓存失效及重复读取等，不代替本轮已显式失败的外部保存后过期覆盖场景 |
| H-75 活跃 turn 回退显式探针与正常对照 | `cargo test -p deepagent-app-core --test kernel_v2_e2e active_rewind --offline -- --include-ignored --nocapture`：1 passed / 1 failed，约 0.29s；回退已删除 user，释放响应后旧 answer 仍落盘，运行 succeeded/单终态；仅离线 transport 与临时会话文件 |
| H-75 消息断言补强与共享入口基线 | 将内容存在检查限定为 MessageAppended/ResponseItemAppended，排除 task 等元数据后显式重跑仍为 1 passed / 1 failed，约 0.28s。`cargo test -p deepagent-app-core --test kernel_v2_e2e --offline`：16 passed / 15 ignored，约 1.59s；fmt、diff 检查通过。本批仅测试和报告，未跑全 workspace、Clippy、前端构建、真实 Tauri 或公网模型 |
| H-74 源消息分支边界显式探针与对照 | `cargo test -p deepagent-app-core --test kernel_v2_e2e branch_boundary --offline -- --include-ignored --nocapture`：1 passed / 1 failed，约 0.66s；两入口 keep=5 丢目标消息、keep=6 保留，正式续聊均 Ok；transport 离线，不含真实供应商或桌面 UI |
| H-74 终态补强与共享入口基线 | 补充四组续聊均为单个 succeeded 终态、保留日志无待丢弃 assistant 后，显式探针约 0.65s 仍为 1 passed / 1 failed。`cargo test -p deepagent-app-core --test kernel_v2_e2e --offline`：15 passed / 14 ignored，约 1.67s；fmt、diff 检查通过。本批只增加审计测试和文档，未跑全 workspace、Clippy、前端构建、真实桌面或公网模型 |
| H-73 回退分支检查点显式探针与正常对照 | `cargo test -p deepagent-app-core --test checkpoint_audit branch --offline -- --include-ignored --nocapture`：1 passed / 1 failed，约 0.35s；正常组文件保持 current-branch，旧分支组被恢复为 old-first，两次回退均成功、当前历史不含旧分支。仅临时文件和正式服务，不含真实桌面或模型请求 |
| H-73 共享检查点入口默认基线 | `cargo test -p deepagent-app-core --test checkpoint_audit --offline`：3 passed / 3 ignored，约 0.21s；fmt、diff 检查通过。新缺陷探针另行显式失败；本批仅测试与文档，未重跑全 workspace、Clippy、桌面构建、真实 Tauri 或公网模型 |
| H-72 检查点恢复显式探针与正常对照（见该节命令） | 1 passed / 1 failed，约 0.22s；rewind/fork 缺失备份均导致当前文件不可读、健康文件已恢复，原历史和检查点未变；完整备份两入口对照通过。未操作用户文件、未运行真实桌面 |
| H-72 存在性补强及默认基线 | 补强后显式探针 1 passed / 1 failed，约 0.19s，两个入口 current_file_exists=false。默认 `cargo test -p deepagent-app-core --test checkpoint_audit --offline`：1 passed / 1 ignored；`cargo test -p deepagent-app-core --test checkpoint_audit --lib service::tests --offline`：库测试 405 passed / 10 ignored / 688 filtered，集成因同一名字过滤运行 0 项，已另跑前述集成命令；`cargo test -p deepagent-runtime --lib checkpoint::tests --offline`：5 passed。fmt、diff 检查通过；未跑本批 workspace、Clippy、前端构建、公网模型或真实桌面，生产行为未修改 |
| 检查点捕获同对象重试的待定证据 | 显式 capture 探针与对照 1 passed / 1 failed，约 0.06s；正式 pipeline/真实文件工具的第二次调用由测试发起，生产普通 loop 同对象重试尚未证明，不新增 H 编号。补充后默认 `cargo test -p deepagent-app-core --test checkpoint_audit --offline`：2 passed / 2 ignored，约 0.21s；fmt、diff 检查通过。仅测试与文档，未再次扩大到 workspace、Clippy、桌面或公网验证 |
| H-71 模型输入模态显式探针及对照（见该节命令） | 1 passed / 1 failed，测试体 <0.01s；Flash 图片、Pro 文字对照完成，Pro 图片也到达 transport，目录未保留 input_modalities；无公网调用或完整桌面图片测试 |
| H-71 模型 crate 默认基线 | `cargo test -p deepagent-models --offline`：单元测试 81 passed / 2 ignored，新增集成测试 1 passed / 1 ignored；fmt、diff 检查通过。缺陷探针已另行显式执行失败；未运行全 workspace、Clippy、前端构建、完整 AppCore 图片路由或公网供应商测试，本批仅审计测试与文档 |
| H-70 历史图片内容身份显式探针（见该节命令） | 约 0.69s 明确失败；正常 wire 摘要等于导入值，替换组 metadata 保持原值但 wire 含替换 PNG 的摘要且续聊成功；仅改测试临时图片，无公网调用 |
| H-70 共享入口与条件编译基线 | runtimes feature 下 `cargo test -p deepagent-app-core --features runtimes --test kernel_v2_e2e --offline`：14 passed / 15 ignored，约 1.80s；默认同入口 14 passed / 13 ignored，约 1.76s；fmt、diff 检查通过。未跑全 workspace、Clippy、前端构建、真实 Tauri 或公网视觉请求；本批只补审计测试与文档 |
| H-69 多图 inline 请求体显式探针（见该节命令） | 约 4.51s 明确失败；单图 28,009,830 字节，双图 55,977,624 字节并含全部两图，均到达离线 transport；本地合法 PNG 解码通过，未发公网请求 |
| H-69 共享入口与条件编译基线 | `cargo test -p deepagent-app-core --features runtimes --test kernel_v2_e2e --offline`：14 passed / 14 ignored，约 1.64s；不带 feature 的同入口：14 passed / 13 ignored，约 1.62s。fmt、diff 检查通过。超限缺陷探针为 ignored 且已单独执行失败；未运行全 workspace、Clippy、前端构建、真实 Tauri 或公网图片模型 |
| H-68 历史图片丢失显式探针（见该节命令） | 约 0.64s 明确失败；正常组 `[0,1]`、故障组 `[0,0]`，两组续聊均返回 Ok；只删除临时导入原图，未调用真实模型或桌面 UI |
| H-68 单终态补强与共享入口基线 | 增加对成功调用实际写出 succeeded 单终态的严格断言后，显式探针约 0.73s 再次失败，仍为 `[0,0]` 且成功。`cargo test -p deepagent-app-core --test kernel_v2_e2e --offline`：14 passed / 13 ignored，约 1.61s；fmt、diff 检查通过。未跑全 workspace、Clippy、前端构建、真实 Tauri 或公网视觉模型；默认绿测不代表 H-68 已修复 |
| H-67 当前图片进入首个请求显式探针（见该节命令） | 约 0.32s 明确失败；两轮 wire 图片数 `[0,1]`，正常历史对照通过；真实图片文件始终存在，两运行均成功。未调用真实视觉模型或桌面 UI |
| H-67 原生响应、持久化输入及共享入口补强 | 原生 output_item.done 与 completed/output fixture 下显式探针约 0.37s 再次失败；已断言 Off 设置和正式首轮事件附件引用。`cargo test -p deepagent-app-core --test kernel_v2_e2e --offline`：14 passed / 12 ignored；fmt、diff 检查通过。本批只测试与文档，未跑全 workspace、Clippy、桌面构建或公网识图 |
| H-67 已有会话新增图片与原生镜像补充 | 同一显式探针约 0.80s 再次失败；初始图组 `[0,1]`、纯文字会话再发图组 `[0,0,0]`，两组原始用户事件均有附件而 native 图片引用均为 0；五轮运行均成功。没有直接篡改事件、删除文件或调用公网模型 |
| H-66 模型搜索能力显式探针（见该节命令） | 约 0.66s 明确失败；三组正常对照成立，Flash+DeepSeekFirst 实际 native=true/callable_function=false；只验证实际请求形状与官方限制，不含真实联网和回答质量 |
| H-66 搜索契约与共享入口基线 | `cargo test -p deepagent-app-core --features web --test kernel_v2_e2e --offline`：14 passed / 12 ignored，约 1.69s；缺陷探针另行显式失败。`cargo test -p deepagent-models --offline`：81 passed / 2 ignored；包含新增的官方容量元数据发现/目录恢复对照。仅测试与文档，未做公网搜索、真实桌面、全 workspace、Clippy 或前端构建 |
| H-65 配置后端拒绝与真实宿主构建显式探针（见该节命令） | 初版 A/B 约 1.30s、扩展 A/B/C 约 2.65s，均明确失败；无修改无 canary，.txt 在完成后出现，.rs 在最终采样前已出现；backend 均仅收到并拒绝 1 次显式命令，三组 run=succeeded；未使用真实 OS sandbox |
| H-65 共享入口回归 | `cargo test -p deepagent-app-core --test kernel_v2_e2e --offline`：13 passed / 11 ignored，约 1.84s；包含默认执行的后端拒绝且无修改对照，新增 ignored 已显式失败。fmt、diff 检查通过；本批仅测试与文档，未执行全 workspace、Clippy、真实桌面 sandbox、前端构建或公网供应商测试 |
| H-64 真实自动构建取消/总时限显式探针（见该节命令） | 两项明确失败、约 6.28s：取消后等待真实 build；5 秒总时限落 budget_exceeded 后原 build.rs 仍写文件。正常对照成功，全部临时构建在断言前放行并完成清理 |
| H-64 计划发现与已有受控命令对照 | `cargo test -p deepagent-app-core --lib completion_plan:: --offline -- --nocapture`：8 passed / 2 ignored，约 1.22s；两项 ignored 已显式执行失败。`cargo test -p deepagent-hooks --lib system_runner_timeout_terminates_process_tree_promptly --offline -- --nocapture`：1 passed；`cargo test -p deepagent-builtins --lib system_executor_cancels_running_process_within_two_seconds --offline -- --nocapture`：1 passed，真实 shell 启动后约 11ms 取消退出。fmt、diff 检查通过；本批仅测试/文档，未跑全 workspace、Clippy、桌面构建或真实供应商 |
| H-63 附属检查用量进入真实成本账本显式探针（见该节命令） | 明确失败：panel 应有 314/24、stall 应有 112/10，实际均仅 11/3；检查完成且无残留，未测真实价格或公网费用 |
| H-63 正向账本对照及检查基线 | `cargo test -p deepagent-app-core --lib run_finalizer::tests::compaction_side_usage_is_booked_as_its_own_row --offline -- --nocapture`：1 passed；`cargo test -p deepagent-app-core --lib verification_panel:: --offline -- --nocapture`：8 passed / 4 ignored，约 0.21s；fmt、diff 通过。四个忽略探针均已分别显式执行失败；本轮仅测试及文档，未执行全 workspace、UI 构建或公网测试 |
| H-62 停滞检测取消显式探针（见该节命令） | 明确失败：取消后分类仍活跃；响应释放后正确落 cancelled，正常对照成功；没有把最终状态正确说成停止及时 |
| `cargo test -p deepagent-app-core --lib verification_panel:: --offline -- --nocapture`（停滞检测核查时） | 8 passed / 3 ignored，约 0.21s；新增停滞检测完整 Kernel 正常对照通过；合并显式执行 `verification_panel::audit_tests::audit_` 三项均按既有缺陷失败、约 0.53s，H-61 两项行为保持；fmt 与 diff 检查通过。仅测试和文档变更，未执行全 workspace、桌面构建或真实供应商 |
| H-61 对抗复核取消与总时限显式探针（见该节两条命令） | 两项均明确失败；正常对照成功，取消后仍 succeeded；主 Kernel 超时后仍有三个活跃复核请求。断言前请求均已释放，未修生产行为 |
| `cargo test -p deepagent-app-core --lib verification_panel:: --offline -- --nocapture` | 7 passed / 2 ignored，约 0.18s；包含正式 Kernel/ModelAgent 正常完成对照；两项忽略缺陷已单独及合并显式执行失败，不能视为已修；本轮 fmt、diff 检查通过，未运行全 workspace 或桌面构建，因为仅新增测试/审计文档、无生产或 UI 契约变更 |
| `cargo test -p deepagent-runtime --lib loop_engine::tests --offline`（双完成事件与门禁核查时） | 36 passed / 1 ignored，约 3.22s；新增完整语义流三组对照通过，忽略的是既有 H-34 探针，不代表缺陷修复；同批 fmt 与 diff 检查通过 |
| H-60 Windows 输出 drain 显式探针（见该节命令） | 明确失败：正常 EOF 释放缓冲，开放管道在 grace 结束、调用方释放后仍保留缓冲并接收迟到 65,536 字节；断言前 reader/子进程均已回收 |
| `cargo test -p deepagent-builtins --lib bash_tool::tests --offline`（H-60 核查时） | 23 passed / 1 ignored，约 26.39s；忽略的是已显式执行失败的新缺陷探针；同批 fmt 与 diff 检查通过，未修改生产行为 |
| `cargo test -p deepagent-app-core --lib tool_manifest::tests --offline`（权限验证补充时） | 4 passed；搜索后的 schema/执行权限、动态加载、父子发现集合隔离均通过；同批 fmt 和 diff 检查通过 |
| H-58 附件导入异常身份显式探针（见补充命令） | 明确失败：三组异常均成功覆盖 canary 并写 metadata，分别落在会话桶、附件 root 及 root 外兄弟目录；正常对照成立 |
| `cargo test -p deepagent-app-core --lib attachment_service::tests --offline`（导入补充时） | 4 passed / 2 ignored；导入本轮显式执行失败，删除此前已显式失败；同批 fmt 与 diff 检查通过 |
| H-59 WASM 共享 watchdog 显式探针（见该节命令） | 明确失败；独立组有 2 秒/5.5 秒标记，共享组均无且更早退出；Windows 两组 trap 均异常退出，父测试进程正常保留证据 |
| `cargo test -p deepagent-tools --features wasm --lib --offline` | 27 passed / 2 ignored；H-59 显式执行失败，另一个忽略项是既有 Windows fuel trap；同批 fmt 与 diff 检查通过 |
| `cargo test -p deepagent-app-core --test kernel_v2_e2e --offline`（跨运行上下文验证补充时） | 12 passed / 10 ignored；新增跨两轮运行及数据库重开的上下文正向测试默认执行；忽略的缺陷探针不代表通过 |
| H-58 附件清理异常身份显式探针（见该节命令） | 明确失败：正常对照只删目标，空/点号 id 删除同桶两附件，父目录 session 实际删除 root 外测试目录 |
| `cargo test -p deepagent-app-core --lib attachment_service::tests --offline` | 4 passed / 1 ignored；新缺陷探针另行显式执行失败；同批 cargo fmt --all -- --check 通过 |
| H-57 技能目录续聊显式探针（见补充命令） | 明确失败：first=true / continued=false / reset=true / fresh=true；四轮均成功且 skill 工具仍在请求中 |
| `cargo test -p deepagent-app-core --test kernel_v2_e2e --offline`（H-57 技能目录补充时） | 11 passed / 10 ignored；技能探针显式重跑仍失败；同批 cargo fmt --all -- --check 通过 |
| H-54 持久化输出上限显式探针（见补充命令） | 明确失败：设置 512/768 后 wire 均为 16,384；设置 65,536 后 wire 为 32,768；无覆盖对照正常，四组均完成 |
| `cargo test -p deepagent-app-core --test kernel_v2_e2e --offline`（H-54 输出上限补充时） | 11 passed / 9 ignored；缺陷探针已另行显式执行失败，默认绿测不代表修复；同批 cargo fmt --all -- --check 通过 |
| H-57 真实 MCP 说明跨 turn 显式探针（见该节命令） | 明确失败且重跑一致：完整请求标记 first=true / continued=false / fresh=true，三轮模型请求均正常完成 |
| `cargo test -p deepagent-app-core --test kernel_v2_e2e --offline`（H-57 核查时） | 11 passed / 8 ignored；新增缺陷探针已单独显式执行并失败，默认通过不代表修复 |
| `cargo test -p deepagent-app-core --lib mcp_instructions --offline` | 6 passed；现有去重测试未验证第二轮完整请求仍含正文 |
| `cargo fmt --all -- --check`（H-57 核查时） | 通过；首次指出新探针格式差异，格式化该测试文件后重跑通过 |
| H-56 两真实 MCP 的名称冲突显式探针（见该节命令） | 明确失败：两服务器均 Ready；正常命名对照发 1 次模型并完成，冲突组普通聊天在工具注册失败、模型请求为 0 |
| `cargo test -p deepagent-app-core --test kernel_v2_e2e --offline`（H-56 核查时） | 11 passed / 7 ignored；H-56 探针已另行显式执行并失败 |
| `cargo test -p deepagent-mcp --offline`（H-56 核查时） | 58 passed / 6 ignored；直接 register 的重名拒绝回归通过，不覆盖宿主 extend_ready 发布后的聊天可用性 |
| H-55 两协议三档思考显式探针（见该节命令） | 明确失败：5 组完成；Chat Simple 在本地校验返回 unsupported reasoning effort: none，transport 次数为 0 |
| `cargo test -p deepagent-models --offline`（H-55 核查时） | 80 passed / 2 ignored；各 adapter 的独立单测不覆盖 Simple 的跨层转换，新探针已单独执行失败 |
| `cargo test -p deepagent-runtime --lib --offline`（H-55 核查时） | 266 passed / 8 ignored；包括 H-55 在内的审计缺陷探针默认忽略，不能以此宣称问题已修复 |
| H-54 Harness 思考覆盖显式探针（见该节命令） | 明确失败：无覆盖对照正确；max/low 覆盖被 Medium/Simple/Deep 持久化档位分别改成 high/none/max |
| `cargo test -p deepagent-app-core --test kernel_v2_e2e --offline`（H-54 核查时） | 11 passed / 6 ignored；新缺陷探针另行显式执行并失败 |
| H-53 压缩后后台记忆显式探针（见该节命令） | 明确失败：1 条组再次取回但被旧 ID 过滤；12 条组不再调用 provider；两组后续 wire 均缺少摘录 |
| `cargo test -p deepagent-runtime --lib memor --offline`（H-53 核查时） | 5 passed / 1 ignored；新缺陷探针另行显式执行并失败，不视为已修复 |
| H-52 独立知识服务写入显式探针（见该节命令） | 明确失败：两次保存均成功，不刷新时 ID 相同、磁盘只剩 B、首缓存仍为 A；刷新对照两条均保留 |
| `cargo test -p deepagent-app-core --lib knowledge_service::tests --offline`（H-52 核查时） | 18 passed / 2 ignored；H-51/H-52 缺陷探针各自另行显式执行，不能用默认绿测视为已修复 |
| H-51 项目记忆作用域显式探针（见该节命令） | 明确失败：不切换对照仅访问 A；激活 B 后，原工具/provider 改读 B 且 project 写入实际落在 B，磁盘重开确认 |
| `cargo test -p deepagent-app-core --lib knowledge_service::tests --offline`（H-51 核查时） | 18 passed / 1 ignored；缺陷探针另行显式执行并失败 |
| `cargo test -p deepagent-app-core --lib memory_service::tests --offline`（H-51 核查时） | 5 passed；单项目读写/检索基线通过，不能覆盖项目切换交错 |
| `cargo test -p deepagent-knowledge -p deepagent-memory --offline`（H-51 核查时） | knowledge 63 passed、memory 40 passed；不包含 H-51 的跨服务作用域探针 |
| H-50 移动输入快照显式探针（见该节命令） | 明确失败：登记/返回 ID 不同，最新/旧/未知引用全部接受，记录到 3 条 ADB 输入 argv；未连接真实设备 |
| `cargo test -p deepagent-mobile-runtime -p deepagent-mobile-android --offline`（H-50 核查时） | runtime 67 passed；Android 63 passed / 1 ignored，另有 7 个真机测试 ignored；缺陷探针另行显式执行并失败 |
| H-49 备用能力/实际请求显式探针（见该节命令） | 明确失败：两协议的 32,768→8,192 备用组，快照预留已为 8,192，wire 仍为 32,768；容量相同的两组成功 |
| `cargo test -p deepagent-runtime --lib fallback --offline`（H-49 核查时） | 1 passed / 1 ignored；普通切换基线通过，新缺陷探针已另以 --ignored 显式执行并失败 |
| H-48 stdio 探活耗尽/连续调用显式探针（见该节命令） | 明确失败：探活已结束仍保留 1 工具/adapter/说明；实际启动数 10→13→16，随后两调用各重开一轮恢复且失败 |
| `cargo test -p deepagent-mcp --offline`（H-48 核查时） | 58 passed / 6 ignored；真实崩溃后恢复成功对照仍通过，本次持续失败耗尽探针已另用显式命令执行 |
| H-47 宿主准备配置变更显式探针（见该节命令） | 明确失败：无变更 A 正常 ready；仅新增 B 后 A 的相同配置被缓存为 preparation cancelled，ready 只保留 B，重复同配置准备为 no-op |
| `cargo test -p deepagent-app-core --lib mcp_service:: --offline`（H-47 核查时） | 26 passed / 2 ignored；网络参考服务与本次缺陷探针默认忽略，本次探针已另用显式命令执行 |
| H-46 Windows MCP 进程归属显式探针（见该节命令） | 明确失败：Node close/Drop 两对照后代均退出；.NET close/Drop 两组父已退出但后代仍运行并各新增 11 次心跳，测试主动清理并确认退出 |
| `cargo test -p deepagent-mcp --offline`（H-46 核查时） | 58 passed / 5 ignored；包括真实崩溃重连、诊断脱敏基线；本次进程树探针已另用显式命令执行 |
| `cargo test -p deepagent-app-core --lib subagent_runner::tests --offline`（H-45 核查时） | 6 passed / 4 ignored；包括本次失败探针在内的审计用例默认忽略，须使用各节命令显式执行 |
| `cargo test -p deepagent-runtime --lib kernel_deadline_finalizes_and_fails_active_task --offline` | 1 passed；父运行正常写出预算终态的既有回归通过，未覆盖前台子运行收尾 |
| H-45 父总时限/前台子 Agent 显式探针（见该节命令） | 明确失败：主动取消对照能收尾；总时限退出后父=budget_exceeded、根未取消、子记录仍 running、无 transcript/子终态事件 |
| H-44 文件数据库启动恢复显式探针（见该节命令） | 明确失败：正常与故障重试均 run=failed、两日志终态数=0、会话 turn 终态为空；故障重试额外产生两条恢复标记，真实重开后未自愈 |
| `cargo test -p deepagent-persistence --lib run_store::tests --offline`（H-44 核查时） | 3 passed；正常终态幂等和事务回滚通过，不能替代恢复路径的投影一致性 |
| `cargo test -p deepagent-app-core --lib startup_recovery_finalizes_unfinished_runs_and_active_tasks --offline` | 1 passed；原有测试只检查运行表/task/恢复标记等，没有检查规范终态和会话 turn 投影 |
| `cargo test -p deepagent-app-core --test recovery_terminal_audit --offline` | 1 ignored；H-44 已另行显式执行失败 |
| H-43 前端启动/订阅显式探针（见该节命令） | 两个断言明确失败：聊天/画布均先启动后就绪，早到终态未交付且继续 pending；画布节点结果亦未收到，晚终态对照可正常清理 |
| `node --test apps/desktop/scripts/audit-start-subscription.test.cjs apps/desktop/scripts/audit-workflow-isolation.test.cjs apps/desktop/scripts/check-workflow.test.cjs apps/desktop/scripts/check-canvas-ipc.test.cjs` | 常规入口 79 passed / 3 skipped，0 failed；三项为 H-41/H-43 显式启用探针，不代表其生产缺陷已修复 |
| H-42 双进程调度忽略探针（见该节命令） | 明确失败：one-shot/recurring 同一 occurrence 均被两个进程 fire；顺序再次 poll 的两组对照均无重触发 |
| `cargo test -p deepagent-runtime --lib schedule:: --offline`（新增 H-42 后） | 19 passed；基本时间与单实例持久化通过，不覆盖共享调度所有权 |
| `cargo test -p deepagent-runtime --test schedule_audit --offline` | 1 ignored；缺陷探针已另行显式执行失败 |
| `cargo test -p deepagent-builtins --lib system_executor_cancels_running_process_within_two_seconds --offline -- --nocapture`（加强真实启动判据后） | 1 passed；PowerShell 实际写出 ready 标记后取消，cancel→命令退出约 13ms；总用例约 0.55s |
| `cargo test -p deepagent-builtins --lib a_timed_out_command_takes_its_detached_child_with_it --offline -- --nocapture` | 1 passed，约 12.21s；既有超时/后代延迟标记回归通过，未独立枚举 PID |
| `cargo test -p deepagent-builtins --lib bash_tool::tests --offline`（加强真实启动取消判据后） | 23 passed，约 26.14s；包括启动后取消、超时、长脚本、后台进程日志与输出管道边界 |
| H-41 前端事件归属显式探针（见该节命令） | 明确失败：B 完成令 A 提前结算，监听归零，A 后续节点终态未交付且停止不发送 cancel_run；没有启动真实云作业 |
| `node --test apps/desktop/scripts/audit-workflow-isolation.test.cjs`（未启用审计环境变量） | 1 skipped；显式启用后两次复现同一故障，约 106ms/96ms；导出共用 TS loader 后重跑 H-40，四组仍复现 |
| H-40 真实前端序列化→正式后端执行忽略探针（见该节命令） | 明确失败：四组均未兑现配置；输入引用被拒绝，条件 false、end 空对象、模板空文本；后三组仍 succeeded |
| `node --test apps/desktop/scripts/check-workflow.test.cjs`（新增 H-40 后） | 77 passed；现有配置、归一化与前端行为测试通过，不验证这些字段在 Rust 正式执行中生效 |
| `cargo test -p deepagent-app-core --test workflow_audit --offline`（新增 H-40 后） | 3 ignored；均为已有缺陷的 opt-in 失败探针，已经分别显式执行，不把默认无失败解释为问题修复 |
| H-39 条件派发忽略探针（见该节命令） | 明确失败：条件正确输出 true/false，两组未选中的互斥模板都进入 Running/Completed，运行均 succeeded |
| `cargo test -p deepagent-runtime --lib workflow:: --offline`（新增 H-39 后） | 66 passed；尚无条件派发回归，不能用基础图与值测试替代真实路径验证 |
| `cargo test -p deepagent-app-core --test workflow_audit --offline`（新增 H-39 后） | 2 ignored；H-38 与 H-39 均已单独显式运行并失败；抽取共用宿主 fixture 后 H-38 仍复现同样的 unknown tool/Completed/succeeded |
| H-38 正式工作流工具失败忽略探针（见该节命令） | 明确失败：read_file 未注册；输出含 success=false/unknown tool，但节点 Completed、end 继续、持久化 run=succeeded |
| `cargo test -p deepagent-runtime --lib workflow:: --offline`（新增 H-38 后） | 66 passed；图/值/媒体等单元回归，不覆盖正式工作流的空 registry 装配 |
| `cargo test -p deepagent-app-core --lib session_search_tests --offline`（新增 H-38 后） | 2 passed；手动注册工具的适配器权限/校验测试通过，不能证明 run_workflow 实际注册了工具 |
| H-37 无人值守 Cron 忽略探针（见该节命令） | 明确失败：one-shot 已移除，scheduler 停止后 pending=1、run 未终结、命令执行数为 0；显式取消并等待终态后结束测试 |
| `cargo test -p deepagent-app-core --lib approval_bridge::tests --offline`（新增 H-37 后） | 19 passed；验证现有审批桥接，不代表无人值守装配正确 |
| `cargo test -p deepagent-runtime --lib schedule:: --offline`（新增 H-37 后） | 19 passed；验证 cron 时间计算与 store/poll，不代表 fire 后台会话可以获得人工答复 |
| H-36 完整结果生命周期忽略探针（见该节命令） | 明确失败：模型下一请求时文件存在且完整；turn 完成即删除；真实数据库重开后 artifact 索引与历史引用仍在，文件不可读 |
| `cargo test -p deepagent-app-core --test kernel_v2_e2e --offline`（新增 H-36 后） | 11 passed / 5 ignored；H-36 缺陷探针另行显式执行失败 |
| H-35 内置写读屏障忽略探针（见该节命令） | 明确失败：两调用先写后读且读到 NEW；增加无关读取后，两次读均先于写启动，下一请求的 source 读结果报错，最终磁盘写入成功 |
| `cargo test -p deepagent-app-core --test kernel_v2_e2e --offline`（新增 H-35 后） | 11 passed / 4 ignored；缺陷探针另行显式执行失败，不代表生产问题已修复 |
| H-34 串并行对照忽略探针（见该节命令） | 明确失败：单工具 80ms 超时并落失败结果；两个相同工具越过 400ms，取消后 200ms 仍 pending，主动 release 后才退出 |
| `cargo test -p deepagent-runtime --lib loop_engine::tests --offline`（新增 H-34 后） | 35 passed / 1 ignored；单工具、常规并行与取消前派发等已有回归通过，缺陷探针另行显式执行失败 |
| H-33 双宿主会话忽略探针（见该节命令） | 明确失败：同一 session 的父响应仍 pending 时，Standby 子宿主已发出第二条模型请求；两条 run 同时 running_turn，释放后均返回成功 |
| `cargo test -p deepagent-app-core --test host_lock --test host_session_audit --offline`（新增 H-33 后） | 原有跨进程测试 1 passed，新缺陷探针合计 2 ignored；两个探针另行显式运行均失败，不代表缺陷已修复 |
| H-32 双进程忽略探针（见该节命令） | 明确失败：备用宿主恢复数为零且父 run 仍 running，但动作已 failed、子运行已 cancelled，父宿主正常成功落盘均被拒绝 |
| `cargo test -p deepagent-app-core --test host_lock --offline`（新增 H-32 后） | 1 passed / 1 ignored；原有主备运行恢复及双进程写入测试通过，未覆盖动作和子运行被误终结 |
| `cargo test -p deepagent-persistence --offline`（核查 H-32 时） | 67 passed；既有单宿主恢复及文件会话并发追加测试通过，不代表恢复具有跨宿主所有权判断 |
| `cargo fmt --all -- --check` | 通过 |
| `cargo test -p deepagent-app-core --lib context_runtime::tests --offline`（新增 H-31 及阈值压缩恢复验证后） | 23 passed / 3 ignored，0 failed；包含真正经过阈值的原生摘要请求与数据库重开验证 |
| H-31 摘要取消回归 | `cargo test -p deepagent-app-core --lib context_runtime::tests::audit_cancel_interrupts_native_summary_before_next_model_request --offline -- --nocapture`：1 passed；500ms 内取消成功，summary request 通过 cancel-aware transport，主模型请求数仍为 1，且没有写入部分 `ContextCompacted` 边界 |
| H-30 单项忽略探针（见该节命令） | 明确失败：多行 JSON、CR-only 和未闭合终态帧三组不符预期，单行完整帧对照通过 |
| `cargo test -p deepagent-models --features http --offline`（新增 H-30 后） | 84 passed / 4 ignored，0 failed；新缺陷探针默认忽略 |
| H-29 单项回归（见该节命令） | 通过：200/429/503 半正文均在 500ms 内响应取消，实测约 0/21/17ms；不再释放正文后才返回 |
| `cargo test -p deepagent-models --features http --offline`（H-29 修复后） | 单元测试 88 passed / 12 ignored，集成测试 1 passed / 1 ignored；0 failed，H-29 不再是 ignored 探针 |
| H-28 单项忽略探针（见该节命令） | 明确失败：只读对照无文件，未信任项目 full_access 覆盖后实际写入，审批数为零；没有修复生产行为 |
| `cargo test -p deepagent-app-core --lib chat_service::tests --offline`（新增 H-28 后） | 74 passed / 3 ignored，0 failed；新增探针默认忽略，不代表其缺陷修复 |
| `cargo test -p deepagent-cli -p deepagent-models -p deepagent-mcp --offline` | CLI 82 passed / 3 ignored；models 84 passed / 1 ignored；MCP 单元 55 passed / 3 ignored、子进程集成 3 passed、协议审查 1 ignored；均无常规失败。组合命令的 feature 合并启用了额外 HTTP 测试 |
| `cargo test -p deepagent-mcp --features http --offline`（新增 H-12 后） | 单元 60 passed / 3 ignored、子进程集成 3 passed、协议审查 2 ignored；无常规失败 |
| `cargo test -p deepagent-cli --offline`（新增 steer 探针后） | 82 passed / 4 ignored，无常规失败 |
| `cargo test -p deepagent-runtime --offline`（新增 H-10 探针后） | 265 单元 + 5 稳定性集成测试 passed，1 ignored；含 500 次以上工具调用测试 |
| `cargo test -p deepagent-app-core --test kernel_v2_e2e --offline`（新增 H-15 后） | 11 passed / 3 ignored，无常规失败 |
| `cargo test -p deepagent-app-core --lib --offline approval_bridge::tests` | 19 passed；常规审批测试未覆盖运行取消后的迟到回答 |
| `cargo test -p deepagent-app-core --lib --offline runtime_payload::tests` | 3 passed / 1 ignored；磁盘锁隔离集成探针另行显式执行通过 |
| `cargo test -p deepagent-runtime --lib --offline tool_pipeline::tests`（新增 H-16 后） | 11 passed / 1 ignored，无常规失败；审批身份缺陷探针另行显式执行复现失败 |
| `cargo test -p deepagent-app-core --lib --offline input_runtime::tests`（新增排队取消测试后） | 28 passed；含取得租约前/后取消及后继输入推进；不代表真实大上下文端到端压缩或完整宿主并发压力已验收 |
| `cargo test -p deepagent-app-core --lib --offline context_runtime::tests`（新增 H-14 后） | 22 passed / 2 ignored；重复内容和无效压缩计费缺陷探针默认忽略，常规测试无失败 |
| `cargo test -p deepagent-app-core --lib --offline run_coordinator::tests`（新增存储故障注入后） | 7 passed；包括审计插入失败后的信号送达、事务回滚、显式重试及唯一终态 |
| `cargo test -p deepagent-cli audit_ --offline -- --ignored --nocapture`（首批执行时） | 当时的 2 个探针明确失败，分别复现 H-01、H-02；后来新增的 resume/steer 探针分别单独执行，见 H-04 |
| `cargo test -p deepagent-app-core --test kernel_v2_e2e audit_sandbox_override --offline -- --ignored --nocapture` | 探针明确失败，复现 H-03 |
| H-04 至 H-09 各节列出的单项命令 | 7 个探针分别明确失败（H-04 包含 resume、steer 两个），证据见对应章节；不是常规测试通过 |
| H-10 节单项命令 | 1 个探针明确失败，复现结束后预取仍继续及收尾 usage 缺失 |
| H-11 节单项命令 | 1 个探针明确失败：压缩保留 6 items，数据库恢复得到 54 items |
| H-12 节 http feature 单项命令 | 1 个探针明确失败：真实本机 HTTP 请求丢失 session header，后续 tools/list 收到 400 |
| H-13 节单项命令 | 1 个探针明确失败：已禁用、ready roots 为空，全新会话的实际模型请求仍声明该插件技能 |
| H-14 节单项命令 | 1 个探针明确失败：摘要与主模型均已返回 usage，原上下文保留，但 side_usage 为空 |
| H-15 节单项命令 | 1 个探针明确失败：取消终态后审批仍 Pending，迟到回答受理并产生 tool_session grant；命令执行数为零 |
| H-16 节单项命令 | 1 个探针明确失败：第二 run 的 action 独立登记，但相同 provider id 的审批遭全库幂等冲突 |
| H-17 真实 Tauri 探针 | 当前源码桌面编译通过；真实模型首片段后停止约 2ms ACK；保存失败已送达 UI，但展示断言明确失败 |
| H-18 节单项命令 | 1 个探针四组均复现：Chat/Responses、空参数/半截 JSON 在第二次 length 后仍返回工具决策并保留原生调用 |
| H-19 节单项命令 | 1 个探针四组均复现：wire 无 write_file schema，真实子运行仍成功写入各自临时目录 |
| H-20 节单项命令 | 真实临时 Git 仓库探针失败：上游工作树已删除、下游无产物、主目录仍是初始内容 |
| H-21 节单项命令 | 真实首次运行及 resume 后契约断言失败；恢复计数为 1，但 schema/phase/label 和 wire 指令均丢失 |
| H-22 节单项命令 | 真实 DAG 子执行器及文件工具探针失败；wire 声明子目录，产出实际落到临时父目录 |
| H-23 节单项命令 | 真实 rewind/恢复后探针失败：旧覆盖点 8 被新分支复用，旧 RED 摘要被选为包含新 BLUE 决策前缀的替代候选 |
| H-24 provider error 回归 | `cargo test -p deepagent-models --features http reqwest_transport::tests:: --offline -- --nocapture`：JSON 和纯文本两种较长中文错误均 `parser_panicked=false`，回归通过 |
| `cargo test -p deepagent-models --features http --offline`（H-24/H-25/H-85/H-86 修复后） | 单元测试 94 passed / 9 ignored，集成测试 1 passed / 1 ignored；0 failed |
| H-25 修复前探针 | 真实回环 HTTP 指定等待 3000ms，455ms 后已重试并再次 429，模型循环失败；安全断言失败 |
| `cargo test -p deepagent-app-core --features web --test audit_provider_retry --offline -- --nocapture`（H-25 修复后） | 真实回环记录 `actual_retry_gap_ms=3018`，供应商恢复且 Agent 成功；1 passed / 0 failed，已是常规回归 |
| `cargo test -p deepagent-models --features http reqwest_transport::tests:: --offline -- --nocapture`（H-25/H-29/H-85/H-86） | 真实 GET 429 元数据、Retry-After 秒数/日期、凭据脱敏、重定向及成功/错误正文取消/时限等 12 passed / 0 failed |
| H-26 节单项命令 | 真实 Windows hook 进程的顶层 deny 正确拦截；标准嵌套 deny/ask 两组均执行工具且无审批，安全断言失败 |
| `cargo test -p deepagent-runtime --lib tool_pipeline::tests --offline`（新增 H-26 后） | 11 passed / 2 ignored；H-16、H-26 已分别显式复现，未修生产行为 |
| `cargo test -p deepagent-hooks --offline`（H-26 核查时） | 62 passed，无 ignored 或失败；原有 stdout 测试仅覆盖扁平格式等，未覆盖标准嵌套 PreToolUse 决策 |
| H-27 节单项命令 | 正式 ChatService 中，未信任的独立 hooks 文件不执行，同样的内联 settings hook 实际写出临时标记，安全断言失败 |
| `cargo test -p deepagent-app-core --lib chat_service::tests --offline`（新增 H-27 后） | 74 passed / 2 ignored，无常规失败；H-13、H-27 已分别显式复现 |
| `cargo test -p deepagent-hooks --offline`（新增大输入时限回归后） | 63 passed；新增 Windows 大输入 deadline 测试通过，无 ignored 或失败 |
| `cargo test -p deepagent-app-core --lib service::tests --offline`（新增 H-23 后） | 405 passed / 4 ignored；该 substring filter 也匹配其他 *_service::tests，并非仅 AppService |
| `cargo test -p deepagent-session -p deepagent-persistence --offline`（新增 H-23 后） | session 14 passed，persistence 67 passed，均无 ignored 或失败；原有 rewind 测试覆盖事件截断及恢复，没有验证笔记覆盖身份 |
| `cargo test -p deepagent-app-core --lib subagent_runner::tests --offline`（新增 H-21、H-22 后） | 6 passed / 3 ignored；H-19 提取共享 fixture 后另行显式复跑，四组仍复现同一缺陷 |
| `cargo test -p deepagent-subagents --offline`（新增 H-20 后） | 13 passed / 1 ignored；原有 DAG 测试检查清理和摘要，未校验文件产出保留 |
| `cargo test -p deepagent-memory -p deepagent-knowledge --offline` | memory 40 passed，knowledge 63 passed；均无 ignored 或失败 |
| `cargo test -p deepagent-app-core --lib session_memory::tests --offline` | 31 passed；含空窗口不请求、笔记未改不推进覆盖游标、越界读写拒绝及异步 pass 收尾 |
| `cargo test -p deepagent-app-core --lib memory_service::tests --offline` | 5 passed；vault 写入、主动 recall 与知识检索可见性边界通过 |
| `cargo test -p deepagent-app-core --lib knowledge_service::tests --offline` | 18 passed；项目切换、磁盘重载、draft、检索及离线捕获测试通过 |
| `cargo test -p deepagent-app-core --lib --offline`（新增 H-22 后） | 1072 passed / 9 ignored，0 failed；忽略项含已知缺陷探针及 opt-in 集成探针，不能将常规通过解释为这些缺陷已修复。输出保存在忽略提交的 target/audit-app-core-20261021.log |
| `cargo test -p deepagent-app-core --lib subagent_runner::tests --offline`（新增 H-19 后） | 6 passed / 1 ignored；常规角色测试只验证 schema 过滤，没有验证清单外实际调用被拒绝 |
| `cargo test -p deepagent-runtime --lib model_agent::tests --offline`（新增 H-18 后，修复前历史） | 56 passed / 1 failed / 2 ignored；原有 `time_based_microcompact_clears_stale_tool_results_after_idle` 在 `Instant::now() - 3601s` 处溢出，尚未进入生产 microcompact；单独重跑同样失败，不能据此认定生产压缩故障 |
| `cargo test -p deepagent-runtime --offline`（空闲微压缩时钟回归修正后） | unit 269 passed / 14 ignored，稳定性集成 5 passed，schedule audit 2 ignored；0 failed。完整长运行用例超过 60 秒后正常完成 |
| `cargo test -p deepagent-app-core --lib --offline chat_service::tests`（新增 H-13 后） | 74 passed / 1 ignored；新缺陷探针默认忽略，常规会话服务测试无失败 |
| `git diff --check` | 通过 |

### 验证基础设施修正：空闲微压缩测试不再依赖系统已运行时长

- 失败测试原先通过 `Instant::now() - 3601s` 伪造空闲时间；本机单调时钟基点不足 3601 秒时，减法在进入生产逻辑前直接 overflow panic。单独精确重跑稳定得到相同 panic，证明这不是 H-25 改动造成的随机失败，也不能用该失败推断生产 micro-compact 已损坏。
- `maybe_time_based_microcompact()` 现在把当前单调时刻与解析后的阈值传入一个确定性内部边界；该边界使用 `saturating_duration_since` 计算空闲时长。测试以“基准时刻 + 阈值”构造前后两组，不修改进程环境变量，也不依赖机器 uptime。生产配置名、默认 3600 秒阈值、持久化及 Surface replacement 均未改变。
- 精确执行 `cargo test -p deepagent-runtime --lib model_agent::tests::time_based_microcompact --offline -- --nocapture`：2 passed / 0 failed；随后完整 `cargo test -p deepagent-runtime --offline` 得到上表结果。本项是测试可重复性和时钟边界治理，没有新增 H 缺陷编号，也没有把离线测试扩大为真实长时间桌面缓存命中验收。

审查累计新增内容包括报告、各 H 节列出的默认忽略缺陷探针、显式桌面/宿主隔离探针、正常回归，以及标题明确标记“已修复”项目对应的生产改动；没有把仍标记为“已确认”或“尚未实施”的项目写成已修复。默认测试包括取消存储故障、排队取消、Windows hook 大输入时限、上下文恢复、真实构建完成与后端拒绝等；完整清单和实际命令以各节及上表为准。没有在每批重复运行全 workspace、Clippy 或前端构建，不能据单项回归宣称全系统通过。桌面使用当前源码真实编译产物测试，截图保存在忽略提交的 `target/live-desktop-test-20261021/`（`tool-result.png`、`restart-recall.png`、`cancel-storage-failure.png`）。

### 后续独立验证：后台 MCP 准备不阻塞聊天

- 执行：`cargo test -p deepagent-app-core --test kernel_v2_e2e pending_host_mcp_initialization --offline -- --ignored --nocapture`，1 passed。此集成探针需要本机 Node，因此默认忽略；它与 H 系列预期失败的缺陷探针性质不同。
- 180 秒是 fixture 的自然完成时间；本次在两轮对话完成后主动释放，没有实际等待 180 秒。生产 MCP 启动本身还有 30 秒整体上限，测试证明的是“确认握手正在进行时，对话不等待它”，不是承诺握手可运行 180 秒。
- 两轮模型均为离线 replay；真实执行了宿主启动、MCP 进程、ChatService 接纳、组装、Kernel 及模型请求构造。MCP fixture 在释放后接受宽松 legacy 握手，以单独验证生命周期隔离；不覆盖 H-08 的严格协议兼容性。
- 该结果支持当前聊天读取 ready snapshot 的边界；不能由此证明磁盘运行包全量校验也已通过 180 秒延迟注入，更不能证明失败插件的 skills/hooks/说明全部隔离。

### 后续独立验证：运行包磁盘准备等待不阻塞聊天

- 执行：`cargo test -p deepagent-app-core --lib blocked_host_payload_preparation_does_not_block_conversations --offline -- --ignored --nocapture`，1 passed。该探针独占生产全局磁盘准备 gate，应单独运行，默认忽略。
- 创建真实临时 runtime.zip 与插件技能，在启动宿主前对其 `.runtime.lock` 加 OS 排他锁；随后观察生产 `DISK_PREPARATION` 已被 worker 持有，确认它已经进入运行包准备并等待文件锁。没有替换生产准备函数或虚构 Ready 状态。
- 保持锁期间两轮真实 ChatService/Kernel + 离线模型在 349ms 内完成；runtime/value.txt 不存在、ready skill_roots 为空、两次实际请求均没有该技能的完整目录条目。释放后校验/解压完成，技能注册表更新；第三个全新会话的实际请求包含该目录条目，且解压内容与原包相同。
- 2026-10-23 重跑得到相同生命周期结果：两轮对话 384ms，锁持有期间技能未发布、runtime 未解压；释放后后台准备完成并发布技能。该数据是本次单机样本，不作为 P95 指标。
- 新增默认忽略的 `runtime_payload::tests::cancelling_blocked_payload_preparation_exits_without_extracting`，使用真实 OS 排他文件锁阻塞生产准备函数；取消后在 14ms 退出，返回 `plugin preparation cancelled`，没有解压文件或遗留 `.runtime-stage-*`。执行 `cargo test -p deepagent-app-core --lib --offline cancelling_blocked_payload_preparation_exits_without_extracting -- --ignored --nocapture`：1 passed。该结果验证 Windows 当前样本的锁等待取消响应，不代表大包解压期间、其他平台或整个桌面停止链的 P95。
- 新增默认忽略的 `runtime_payload::tests::workspace_switch_does_not_publish_stale_plugin_preparation`：真实 A/B 项目插件均预先启用，A 的 runtime payload 被 OS 文件锁阻塞后切换到 B，再释放 A；正式 `PluginService` 的 ready snapshot 最终只含 B 的技能根，A 的 runtime 未解压。执行 `cargo test -p deepagent-app-core --lib --offline workspace_switch_does_not_publish_stale_plugin_preparation -- --ignored --nocapture`：1 passed，约 2.11 秒。它验证当前 AppCore 项目作用域切换的代次栅栏；没有启动 Tauri 窗口，也不覆盖跨进程并发。
- 首次探针把 fixture 放在聊天工作目录内，单纯匹配技能名会命中动态环境的文件路径；已把工作目录与 fixture 分开，并改为匹配目录条目后重跑通过。该早期断言失败不认定为产品能力泄漏；H-13 也收紧相同判据并独立重跑，缺陷仍存在。
- 这是实际文件锁等待边界，没有真的等待 180 秒，也没有对逐文件校验人为注入 180 秒延迟、统计 P95 或验证跨进程安装并发。它支持磁盘准备在宿主后台执行的隔离设计，不能代替完整运行包性能及损坏恢复验收。

### 后续独立验证：取消审计写入失败不阻止信号

- 新增默认运行的 `run_coordinator::tests::cancellation_survives_audit_write_failure_and_terminal_retry_is_idempotent`，通过真正的 RunCoordinator 接纳并取得运行注册，在内存数据库的 run_events 表安装仅拦截 `cancel.requested` 的 SQLite trigger，使用 `RAISE(ABORT, ...)` 注入写失败。
- 重复 request_cancel 均 accepted；根 token、派生子 token 和 legacy flag 均已取消。调用生产 `kernel::persist_terminal()` 明确返回注入错误；数据库只保留 run_accepted，finished_at 仍为空，说明审计及终态未部分提交，也未把停止落盘说成成功。
- 撤销 trigger 后显式重试，同一取消意图成功写入 `cancel.requested → cancel.propagated → run_terminal`；再次写终态返回 false，最终仅一个 cancelled 终态。释放注册后不再接受该 run 的取消。
- 执行 `cargo test -p deepagent-app-core --lib --offline run_coordinator::tests`：7 passed，其中包括本项故障注入。此项验证已存在实现的正确边界，本轮未修生产代码。
- 边界：没有真实损坏用户数据库，没有实际模拟磁盘写满；显式重试成功不代表所有生产失败都会自动重试。本测试本身不覆盖 UI 或全部模型/进程资源清理；后来真实桌面的错误展示验证见 H-17，结果为缺陷。

### 后续独立验证：排队取消不使后继输入失去租约

- 新增默认测试 `input_runtime::tests::cancelling_queued_input_before_or_after_promotion_does_not_strand_follower`，使用真实 Session 数据库、生产 accept_input_turn、InputLeaseRegistry 与 InputLeaseGuard；固定前驱 owner、待取消输入及 follower 的顺序。
- 分别执行“待取消输入仍在队列”和“前驱已释放、待取消输入刚被提升为 active”两种交错。两者都在有界时间返回取消，待取消项从队列消失；owner 不被错误释放，follower 随后取得租约，最后新输入仍能正常 acquire。
- `cargo test -p deepagent-app-core --lib --offline input_runtime::tests`：28 passed。该测试只验证输入接纳/租约层，前驱的取消回调为记录型 fixture，没有运行并发模型、Tauri UI、完整终态或多进程压力，不能据此覆盖所有会话调度竞态。

后续仍需覆盖逐文件运行包校验长延迟注入、完整宿主并发租约/取消压力、完整压缩的真实 wire 及重启、DeepSeek 流式异常与其他宿主入口。保存失败的 UI 通知现已通过真实故障注入确定为 H-17。此次样本不能用来宣称全系统验收完成，或所有准备与取消问题均已修复。

### 后续独立验证：记忆与知识库已有保护边界

- memory/knowledge 两个 crate 和 AppCore 的 session_memory、memory_service、knowledge_service 测试合计 157 项通过，分别为 40+63+31+5+18。没有因为发现其他模块缺陷而把这部分也定性为失效。
- 核查笔记提取实际使用仅含 pinned note 工具的 registry；这与 H-19 的“完整执行表、仅过滤 schema”不同。原有测试实际验证 off-note 访问拒绝、未编辑不宣称覆盖、放弃的 pass 不保存 watermark。
- 上述是现有离线回归，不覆盖真实模型笔记内容质量或跨进程崩溃瞬间。随后进行的真实三轮 session-memory 用例验证了本次笔记更新和游标落账；会话 rewind/笔记一致性仍已确认为 H-23。这说明原有 157 项通过及单次 live 提取均不能替代分支恢复测试，不能概括成记忆系统全面验收通过。

### 后续独立验证：Windows hook 大输入仍能触发命令超时

- 代码中 stdin 写入发生在 child.wait_with_output 的 timeout 之前，最初怀疑大输入可能挡住命令超时；实际验证没有支持这个假设，因此不列为 H 系列缺陷。
- `system_runner_deadline_handles_large_stdin_payload` 使用生产 SystemHookRunner，向 Windows cmd/ping 回环命令提供 1MiB JSON stdin，配置 100ms 命令超时。实测约 119ms 返回 exit_code=124；先前 PowerShell 延迟读取的 fixture 也在约 287ms 返回超时。没有据此认定所有平台/管道背压情况都安全。
- 保留 Windows 专属默认回归，只使用临时内存数据和回环地址；没有调用公网。进程树清理由生产 ProcessTreeGuard 负责，本次未单独枚举子进程验证完整树退出。
- 本项补充证明大输入样本的 deadline 可用，不证明 H-26 的输出协议正确，也不是对 stdin 全生命周期时限的形式保证。

### 后续独立验证：唯一历史边界的主动原生摘要进入请求并在数据库重开后保持一致

- 新增默认测试 `native_threshold_summary_changes_request_and_survives_database_reopen`；执行 `cargo test -p deepagent-app-core --lib native_threshold_summary_changes_request_and_survives_database_reopen --offline -- --nocapture`：1 passed。
- 真实文件数据库持久化 24 条内容唯一的原生消息、近期 function_call/result 及当前输入。使用正式 ModelAgent、HookedReactiveContextCompactor、keep-window 和 2000 的本地强制阈值；模型仅为离线 ScriptedTransport，先返回短摘要再返回主模型答案，不手工调用 compactor 绕过阈值入口。
- 从正式 ModelClient 发出的序列化请求 body 反序列化原生 input，确认包含摘要标记、近期工具的原始 name/call_id/arguments/output 及当前输入，保留 instructions，早期完整正文已经消失。只有一个 ContextCompacted boundary，估算 68759→11564 tokens，wire input 为 8 items。
- 释放 agent、session 和数据库后重新打开相同 SQLite 文件；按生产 response_items_from_events 恢复，8 items 与刚发送的请求 input 完全相等，原始早期事件仍保留。这是请求序列化/原生边界/实际磁盘重开的正向证据，没有真实供应商、桌面 UI 或真实 1M 容量压力测试；强制阈值低于受保护尾部，本项不要求压缩结果低于 2000。
- 初次 fixture 没有启用生产组装器使用的 snip tagging，导致 live/replay 只在 user 的 `[id:uN]` 标签上不等；按 RunAssembler 实际顺序启用同一 with_snip_tool 后通过。该 fixture 不一致未列为新缺陷，也未通过删除标签来放宽最终逐 item 比较。
- 此处刻意使用唯一内容的边界，不能用它否定已复现 H-11 的重复内容匹配故障；也不覆盖 H-31 的压缩中取消。真实供应商大窗口压缩和完整 Tauri 重启仍未验收。

### 后续独立验证：Windows 命令真正启动后取消

- 审阅现有 `system_executor_cancels_running_process_within_two_seconds` 时发现：它创建并 pin Future 后先 sleep 和设置取消，再首次 await 命令 Future；Rust async 的该用例实际上只验证启动前取消。其最初约 0.10s 的通过不能算运行中取消已验证。此前 commentary 已及时更正，不把测试标题当行为证据。
- 本轮只加强 cfg(test) 测试：使用 tokio::spawn 真实驱动生产 SystemExecutor；临时 PowerShell 向环境变量指定的临时文件写 PID，然后等待 30 秒；父测试必须看见子进程写出的 ready 文件才发取消。取消等待有 3 秒上限，超限先 abort/join 回收测试工作再报错；不用用户目录或网络命令。
- 实际执行 `cargo test -p deepagent-builtins --lib system_executor_cancels_running_process_within_two_seconds --offline -- --nocapture`：`shell_started=true cancel_to_exit_ms=13`，1 passed，总约 0.55 秒；返回包含 cancelled 且不包含 before start。生产 run_process_controlled 在取消后等待 child 退出再返回，这是已启动 shell 的退出证据，未覆盖整个 Agent/Tauri 的取消链。
- 另外执行既有 `a_timed_out_command_takes_its_detached_child_with_it`，1 passed，约 12.21 秒；断言是后代计划写出的延迟标记未出现，没有单独枚举所有子孙 PID。因此只说明该 timeout fixture 通过，不能把它扩大为任意进程树都清理正确。H-34、H-29 等其他取消缺口仍成立；H-31 已在本轮修复并通过回归。
- 没有修改生产命令、权限或沙箱行为；此处保留为正向保护边界与测试质量修正，不新增 H 缺陷编号。

### 后续独立验证：保留历史、工具参数和结果进入同一上下文投影及实际请求

- 新增默认测试 `context_projection_tracks_retained_history_output_and_tool_result_on_wire`；执行 `cargo test -p deepagent-runtime --lib context_projection_tracks_retained_history_output_and_tool_result_on_wire --offline -- --nocapture`：1 passed。正式 ModelAgent、ModelClient 与请求序列化均实际执行，provider 响应来自离线 AttemptTransport，没有调用真实供应商或工具。
- 使用内容唯一于本测试的两条较长历史消息、当前输入、1,200 字符的调用参数及约 8,000 字符的工具结果。初始投影为 8,571；第一步 provider 输入 fixture 为 10,000，调用写入后投影 10,345；工具结果写入后的快照仍以 10,000 为锚点且已增长；第二步 provider 输入 fixture 为 12,000，最终回答写入后投影为 12,508。累计计费输入独立为 22,000，所有快照分项之和等于各自投影。
- 对正式客户端实际发出的第二次请求 body 反序列化原生 input，逐项核对旧历史，以及调用的 name/call_id/原始 arguments 和完整结果均存在。此样本证明运行内的当前投影不是只保留最近一轮消耗，也没有把两次重复发送的输入计费相加当作占用。
- 首次断言误要求工具结果净增量至少 2,000，实际为 1,977；核查发现 `ResponseRequest::from_response_items` 会给尚未返回的调用补临时失败输出供请求投影使用，真实结果到来后替换该占位。已将断言改为明确容纳占位扣除的 1,900 下限，并保留真实请求中完整结果的严格断言；这是测试预期修正，不列为产品缺陷。
- 对照 `借鉴/deepseek-harness/packages/llm/token-meter/src/estimate.ts` 和 `projection.ts`：参考实现同样使用固定字符密度估算增量，并明确区分 provider 输入锚点、下一请求投影和累计计费；其文档也承认启发式对 CJK/schema 的偏差。本系统的 `HeuristicTokenizer` 注释所称“不会低估”不能当作真实 tokenizer 的保证。本轮没有精确 tokenizer 或真实供应商的增量误差测量，因此不据此新增缺陷。
- 同批执行 `cargo test -p deepagent-context --offline`：54 passed；`cargo test -p deepagent-runtime --lib context_ --offline`：5 passed。这里只覆盖原生 Responses 离线路径，没有新增 Chat Completions、图片真实 token 成本、跨用户 turn 的 AppCore 重新组装或桌面显示验证；H-05 等已确认问题仍未修复。

### 后续独立验证：跨用户轮次与数据库重开后仍保留完整上下文

- 新增默认测试 `context_projection_retains_turns_across_runs_and_database_reopen`，通过正式文件 SQLite、SettingsService、ChatService → Kernel → ModelAgent → ModelClient 执行三轮。仅底层 HttpTransport 使用记录请求并返回合成 provider usage 的离线 fixture；未调用公网供应商。
- 前两轮沿用同一 session：第一轮 provider 输入 4,000、回答写入后投影 5,217；第二轮 provider 输入 6,000、回答写入后投影 6,023。第一轮较长的完整用户输入与完整 assistant 回答均存在于第二次实际序列化请求中。两轮 CostService 独立账目为 `(4,000, 600)`、`(6,000, 25)`，没有把累计输入 10,000 当作当前占用。
- 第一宿主的 ChatService 与所有数据库 owner 离开作用域后，以 `AppService::open` 重开同一数据库；`session_detail.context_usage` 与重开前最后发出的快照逐字段一致。第三轮实际序列化请求同时保留前两轮的完整输入和完整回答；provider 输入 fixture 为 7,500，最终投影 7,513。所有捕获快照的 blocks token 总和均等于 occupancy；第三轮未接入 CostService，不能据此声称三轮计费都已验证。
- 首次 fixture 的长输入/回答以空格结尾，严格 substring 断言未通过；改为带明确 END 后缀的内容后，仍对完整输入/回答进行严格比较并通过。尚未精确定位末尾空白处理来源，不将此测试数据调整列为产品缺陷。
- 执行 `cargo test -p deepagent-app-core --test kernel_v2_e2e --offline`：12 passed / 10 ignored；`cargo fmt --all -- --check` 与 `git diff --check` 通过。本项扩展了上节的运行内证据，证明该样本的跨轮次组装与数据库恢复没有把上下文缩减为最近一轮消耗；不证明真实 tokenizer/供应商误差、工具循环/压缩边界、Tauri 面板或完整桌面进程重启已通过。H-05、H-54、H-57 等已确认缺陷仍然独立存在。

### 后续独立验证：延迟工具被发现后仍受 schema 与执行权限约束

- 新增默认测试 `tool_manifest::tests::deferred_discovery_does_not_grant_missing_execution_permissions`。正式 ToolRegistry 注册一个需要 WorkspaceWrite 的 deferred 工具，其实际行为仅增加内存计数；通过正式 register_tool_search_into、live_tool_schema_source 与 ToolExecutionPipeline，使用 read_only 权限执行工具搜索，再显式尝试调用该工具。没有执行文件写入或公网调用。
- 执行 `cargo test -p deepagent-app-core --lib deferred_discovery_does_not_grant_missing_execution_permissions --offline -- --nocapture`：1 passed。当前搜索确实返回一个匹配并写入 discovered（matches=1 / discovered=1），但只读 live schema 没有该工具；即使 pipeline 设置 auto_approve=true，真实调用仍在 Permission 阶段拒绝，计数为 0。developer 权限对照能够执行，计数变为 1。
- 这证明该样本下的 discovery 不会增加 PermissionSet 能力，auto approve 也不绕过缺失能力。搜索候选快照本身尚未按 granted 过滤，会显示当前不可执行的名称/描述；本轮没有证据证明这导致真实模型循环失败或泄露敏感内容，不将其等同于执行越权，也不据此新增缺陷编号。该测试没有覆盖自定义角色按工具名限制：H-19 的 schema allowlist 与实际 registry 分裂仍是独立已复现问题。
- Claude Code `tools/ToolSearchTool/ToolSearchTool.ts::call` 使用 invocation options.tools 作为发现池，`utils/toolSearch.ts` 决定 deferral；Codex `core/src/tools/handlers/tool_search.rs::ToolSearchHandlerCache::get_or_build` 从当前 registry 的 deferred entries 构建搜索视图。这里只对照发现池归属，不声称它们所有权限策略均等价于本系统 PermissionSet。已有 `large_plugin_advertises_one_source_and_loads_only_matches_in_same_run` 与 `cloned_registry_rebinds_search_to_child_discoveries` 还分别验证动态 schema 加载与父子 discovered 集合隔离；本轮执行 `cargo test -p deepagent-app-core --lib tool_manifest::tests --offline`：4 passed。

### 后续独立验证：Responses 双完成事件不重复执行工具，流式预执行保留 hooks 和权限门禁

- 核查 `ResponseAccumulator::emit_tool_completed` 在 builder 上记录 completed_emitted，参数完成和 output_item 完成共用一次完成通知；`RuntimeEngine` 仅在没有挂载 hooks 时允许 speculation，提前执行仍经过 registry.check。最初的“双完成事件可能启动两次预执行”的疑点没有得到证据支持，不新增 H 系列缺陷。
- 新增默认执行测试 `loop_engine::tests::responses_dual_completion_keeps_tool_execution_and_gates_consistent`，贯穿正式 ModelClient/ResponseAccumulator → ModelAgent/SinkObserver → StreamingToolAttempt → ToolExecutionPipeline/ToolRegistry → SQLite session events → 下一次真实序列化请求。离线 transport 依次提供 item.added、arguments.delta、arguments.done、item.done 和 response.completed；工具只增加内存计数，未访问用户文件、网络或真实供应商。
- 执行 `cargo test -p deepagent-runtime --lib responses_dual_completion_keeps_tool_execution_and_gates_consistent --offline -- --nocapture`：1 passed，约 0.45 秒。可执行组在 response.completed 之前已实际执行一次，完整模型/工具循环结束后仍为一次；BeforeToolUse allowlist 拒绝组、PermissionSet 为空组，提前执行和最终执行计数均为零。测试显式设置 auto_approve=true，仍不能绕过这两项拒绝。
- 三组均为两个模型请求、同一 call_id 的一条工具完成持久化事件，第二请求各只有一个原生 FunctionCall 和一个对应 FunctionCallOutput；成功组 ok=true，拒绝组 ok=false，模型最终正常结束。它验证了完整语义流的去重、真实执行次数和配对，不仅是手工调用 ToolAttemptController 的模拟。
- 参考 DeepSeek Harness 的 `packages/llm/llm-pi-ai/tests/tool-argument-streaming.spec.ts`：其 OpenAI Responses 路由 fixture 同时发送 arguments.done 和 item.done，并核查参数完成行为，说明两事件共存应被正常处理。该参考属于其 pi-ai 的兼容路由，不是官方 DeepSeek Messages 字段；没有把它当成默认 DeepSeek provider 的同构实现，也未运行参考项目测试。
- 此测试没有装配完整 AppCore/桌面、durable run_actions、真实供应商或网络 SSE 分帧，不证明所有工具幂等/并发取消、异常矛盾参数、截断输出均正确。尤其不关闭 H-18 的截断提交、H-19 的子角色限制或 H-34 的批量超时缺口；也与 Codex 宿主的 cipher_incompatible 无关。本轮仅补正向审计证据，未修改生产行为。

### 后续独立核查：DeepSeek 原生 Responses 合法，当前模型目录容量字段可以被读取

- 2026-10-22 核对[官方 Responses 指南](https://api-docs.deepseek.com/guides/responses_api/)：DeepSeek 原生提供 Responses 入口，支持 Flash/Pro、无状态历史回传和相应工具项。因此不能因本系统默认走 Responses 就认定其错误套用 OpenAI 协议；需要按各字段和模型限制核对，H-05/H-55 的 Chat 问题不能外推成默认 Responses 完全不可用。
- 同日[官方模型列表](https://api-docs.deepseek.com/api/list-models/) 已公开 context_window、max_output_tokens、modalities、effort 和按协议能力元数据。本仓库 capability.rs/discovery.rs 的“当前只返回 ID”注释已经陈旧，但 ModelInfo 确实反序列化容量与输出上限，resolver 也优先使用 provider 数字；不能仅凭注释列出“容量数字必然丢失”的缺陷。
- 新增默认测试 `discovery::tests::published_capacity_metadata_survives_discovery_and_catalog_restore`：按官方公开形状提供离线 MockTransport，经生产 ModelDiscovery.discover、ModelCatalog 序列化/反序列化，再交 ModelCapabilityResolver；两项数字保持 1,048,576 / 393,216，source=ProviderMetadata。额外 modality/effort 字段不妨碍容量解析。该测试不证明这些额外字段被本系统保存或使用，也未进行真实网络发现；暂未复现其丢失引起的具体行为，不另立缺陷编号。
- `cargo test -p deepagent-models --offline`：81 passed / 2 ignored；既有 metadata 优先、Responses reasoning 序列化、图片构造和工具流回归均通过。它们不是全链路图片识别、真实模型能力选择或全部协议兼容性的验收。

### 后续独立验证：多个完整 reasoning item 不被全局 delta 额外追加

- 新增 `stream::tests::completed_reasoning_items_do_not_receive_joined_global_fallback`，为两个不同 reasoning 身份分别提供 added、带 item_id/output_index/content_index 的 delta 和完整 done；正式 ResponseAccumulator 保留两个原 ID/内容，没有补出第三份拼接 reasoning。本候选不构成已复现缺陷，不新增 H 编号；不能把文本分支 H-81 自动外推到完整 reasoning 分支。
- 首次以 `cargo test -p deepagent-models --lib reasoning --offline -- --include-ignored --nocapture` 执行，12 passed / 1 failed；新增候选通过，失败是同时匹配到的旧 H-05 `audit_chat_projection_preserves_tool_turn_narration_and_reasoning`，不能解释为 reasoning item 探针失败。此对照未覆盖空 reasoning done、缺失 done、公开网络或 AppCore 续轮。
- 将夹具收紧为逐 item 的 added→delta→done 顺序后，默认 `cargo test -p deepagent-models --offline` 的 82 个通过单测包含本对照；只新增测试，生产累加器未修改。

### 后续独立验证：stdio 补读的 session 与各 run 游标互相独立

- `terminal_audit::reopened_thread_read_keeps_session_and_each_run_cursor_independent` 经正式 dispatcher/spawn_turn 完成同一 session 两轮运行，两轮都只有一个实时完成及一个 durable succeeded；用独立 Database/ServerState reader 重新读取同一磁盘，完整 `thread/read` 与原结果相等。
- 同一请求中显式设置 sessionAfterSequence 和不同的 runAfterSequences，另将旧 afterSequence/runAfterSequence 设为 u64::MAX 验证优先级；返回结果与各条 stream 单独 `sequence > after` 筛选逐项相等。first run 到末尾后为空、second run 从中段仍可取到终态，runId/stream/streamSequence 关联一致。第一次显式运行通过，测试体共 0.74s（与另外三项合计），不能当作该探针单独耗时。
- 这是持久化读取和游标优先级的正常证据，不覆盖真实管道断连/背压和 SDK 自动恢复；H-01 的实时丢通知及 ACK 缺口仍保留。独立 reader 建立时原宿主对象仍在，未宣称完成真实进程退出重启验收。

### 后续独立核查：DeepSeek system prompt 保留稳定前缀布局

- `system_context.rs::build_system_manifest()` 将内置 system prompt 与用户选择的 output style 放在 `DYNAMIC_INSTRUCTIONS_BOUNDARY` 之前；cwd、日期及运行环境等易变内容位于边界之后。没有从这一设计直接推断真实供应商必然命中缓存。
- 执行三项既有生产组装测试：`volatile_context_never_enters_the_cacheable_prefix`、`the_cacheable_prefix_is_byte_identical_between_turns`、`output_style_stays_in_cacheable_prefix_before_boundary`，均为 **1 passed**。这证明测试覆盖的 system prompt 前缀跨样本稳定，不证明整份 Responses wire（包括 input、tools、配置默认值）每轮都相同，也不证明 DeepSeek provider cache 的命中率。
- 成熟实现对照：DeepSeek Harness `packages/core/agent-loop/tests/request-cache.e2e.ts` 有 key-gated 真实 API 用例，构造多步工具调用和下一轮追问，并断言首个请求之后每个请求均返回 `cacheReadTokens > 0`。本轮只检查该测试的源码，没有运行它，也没有调用 DeepSeek API。
- 后续执行真实用例：`cargo test -p deepagent-app-core --features keychain,web --test real_session_file_responses real_responses_session_replays_tools_skills_files_and_logs --offline -- --ignored --nocapture`。三轮主 Agent provider usage 分别为 12469 / 13059 / 13926 输入 tokens，cache hit 分别为 12288 / 2944 / 13184；另有 3 次笔记维护请求，持久化笔记已更新且 `last_summarized_sequence`、`tokens_at_last_extraction` 断言通过。该记录证明本次真实供应商响应返回了 cache-hit usage，不代表稳定前缀必然命中，也不能由三轮样本推导普遍 cache hit 比例。
- 2026-10-24 复跑 `cargo test -p deepagent-app-core --lib cacheable_prefix --offline -- --nocapture`：3 passed；`volatile_context_never_enters_the_cacheable_prefix` 单测：1 passed。另查 `ToolRegistry` 内部使用 `BTreeMap<String, ToolSpec>`，相同可见工具集按名称稳定排序后进入 schema；工具权限、deferred discovery、MCP/plugin 就绪集合变化仍会改变工具集，这属于需要单独观察的 cache boundary，不等于不稳定迭代顺序。
- 边界补充：这些测试不捕获连续两轮的完整 `ResponseRequest` JSON；当前没有证明 `input`、工具 schema、effort/defaults、附件解析和请求级身份在同一真实 session 的完整字节前缀都稳定。也没有把供应商 cache hit 归因到其中某一项。
- 结论：当前证据仍不支持“system prompt 每轮重写导致稳定前缀丢失”的缺陷，不新增 H 编号。实际前缀字节测试证明的是组装布局；live usage 是供应商对本次请求的计量。要归因 cache hit 仍需结合请求正文、provider 计费语义及更多重复样本，本次结果不做该推断。

### 后续独立验证：输出到限恢复的全部响应保留用量并写入账本

- 扩展既有 `tests/kernel_v2_e2e/incomplete_reason.rs` 的离线 provider fixture，复用正式 ChatService → ModelClient/ResponseAccumulator → ModelAgent → Kernel → AppRunFinalizer/CostService。每次请求返回不同的完整 usage：第 N 次输入 `12N`、输出 `6N`、cache hit `4N`；逐条核对 RuntimeEvent::Usage，并从真实 SQLite 费用表读取总量，释放原 ChatService/Database 后再打开同一文件读取。
- 成功恢复组两次 incomplete 后第三次 completed，**3 次请求、3 条 usage，累计输入/输出 72/36，cache hit/miss 24/48**，唯一终态 succeeded。耗尽组连续五次 incomplete 后停止，**5 次请求、5 条 usage，累计输入/输出 180/90，cache hit/miss 60/120**，唯一终态 context_exhausted；这两个样本的累计值均超过最后一次请求量。数据库重开后四项总量完全一致，未重复记账，也未因最终未完成而丢弃已回答请求的费用。
- 成熟实现对照：Claude Code `services/api/claude.ts` 收到 usage 后更新总费用，再处理 `max_tokens` 的恢复消息；DeepSeek Harness `token-meter/src/turn-usage.ts` 及其测试按尝试归集用量，避免 streaming sample 与最终 message 重复相加。本系统 `record_provider_response_usage()` 在是否升级/续写前计入 usage，AppRunFinalizer 在非成功终态也记录已发生量；本次是对这条已有行为的验证，没有修复生产逻辑或新增 H 编号。
- 首次耗尽组的测试预期误写为通用 failed，账本断言均已通过；核对 `kernel.rs::classify_terminal_error` 发现既有 `max_tokens` 映射为 context_exhausted，改为该约定后重跑。这里不据此声称该终态命名已完成语义审查，也不将测试预期修正计为产品缺陷。
- 本项不关闭 H-82 的过滤原因丢失、H-14 的未采纳摘要费用遗漏或 H-63 的附属模型调用费用归属问题。仅使用合成 provider usage 与临时数据库，没有真实供应商账单、Tauri UI、在途取消或数据库写入失败验证。

### 后续独立核查：插件文件 watcher 覆盖相同大小与时间戳的变更

- 在 `plugin_preparation::tests::audit_same_size_edits_and_replacements_invalidate_prepared_projection` 中创建临时插件，先通过正式 `prepare_candidate()` 建立运行投影缓存，再启动生产 `RecommendedWatcher`。分别测试了原地编辑和临时文件替换；新旧内容字节长度相等，并将新文件修改时间恢复到旧值，使 `PathSnapshot` 的长度和毫秒级时间戳检查保持匹配。
- Windows 探针通过。两种改动均由 watcher 事件使插件投影缓存失效，随后 `prepare_candidate()` 重新读取文件并得到新输出样式内容。没有发现“大小及时间戳相同便稳定复用旧能力”的复现证据，因此不新增 Harness 缺陷项。
- 成熟实现对照：Claude Code `utils/skills/skillChangeDetector.ts` 对技能目录监听 add/change/unlink，并通过 `useSkillsChange.ts` 清除 command/skill 缓存后重新扫描；Grok `prompt/skills.rs::collect_skill_config_dirs()` 明确由技能发现和 watcher 共用同一目录来源。两者支持“变更通知负责失效、统一发现范围”的设计依据；本次只核对源码，不声称它们能消除任意平台文件通知丢失。
- 验证边界：探针直接观察本系统插件准备投影缓存及重新读取结果，没有组装正式模型请求；AppCore 禁用插件技能残留由 H-13 单独记录且仍未修复。也没有对 macOS/Linux watcher、断开的监视句柄、进程崩溃恢复或真实 Tauri 桌面进行验证。此前的陈旧缓存疑点仍不能扩展到这些未验证条件。
- 执行：`cargo test -p deepagent-app-core --lib audit_same_size_edits_and_replacements_invalidate_prepared_projection --offline -- --ignored --nocapture`，**1 passed**。该审计探针默认忽略，生产逻辑未修改。

## 当前工程收敛方向与实施依赖（审计结论，尚未实施）

下表依据已复现的边界归并；优先级不等于所有问题具有相同触发概率。设计收敛在本系统既有内核，不复制参考项目建立第二套运行链。

| 修复批次 | 关联问题 | 应统一的现有边界 | 首要验收 |
|---|---|---|---|
| 执行权限与子运行上下文 | H-03、H-19、H-22、H-65 | 有效工具集合、sandbox 选择、执行根、checkpoint 与两处自动验证必须来自同一已解析执行配置；参考 Claude 的有效工具池与子 cwd、Codex 的统一命令编排 | 清单外调用不能执行；两个子目录同名写入隔离；声明 sandbox 与实际 backend 一致；配置后端拒绝时单文件/完成验证均不得转为宿主执行 |
| 可选 WASM 的时钟与调用归属 | H-59 | 既有 WasmSandbox 统一 Engine tick 与 per-Store deadline；参考上游 Wasmtime 及成熟 Agent 的局部 timer 清理 | 顺序与并发调用互不提前超时；成功/失败/取消不遗留误伤；Windows trap 错误不终结宿主；默认未启用路径保持不受影响 |
| 接纳、终态与审批生命周期 | H-02、H-15、H-16、H-17、H-84 | Harness 复用 AppCore admission；审批身份带运行作用域；执行停止与保存错误分别投影；审批通知只由一个 gate owner 发布并携带 call ID；参考 Codex 的 turn 所属 waiter 和单一权限请求 | ACK 后立即取消不发模型；终态后旧审批无效；两 run 同 call_id 独立；一项人工审批只发一条可响应请求；保存失败可见 |
| 归档状态与运行接纳 | H-215 | 复用现有 `ArchiveService`、`ServerState` 与运行协调边界，把唯一 durable archive 状态投影到 `thread/list`、`thread/resume`、`turn/start`、`turn/steer` 等 admission；归档活动 turn 时明确拒绝、先取消并有界等待或 archived-pending 的单一契约；参考 DeepSeek Harness pre-step gate 与 Codex archive teardown，不新增第二套归档或运行中心 | archive 成功后默认列表不再公开 thread，所有新运行入口拒绝；活动 turn 的归档结果可审计且终态唯一；显式 unarchive/restore 后才可恢复；进程重启、迟到 follow-up、subagent lineage 与并发 archive/start 均不能绕过 |
| Thread create/resume/fork 的就绪提交 | H-216 | `thread/fork` 复用现有 `ThreadState` 构造与 AppCore fork 结果，在成功 response 前原子安装可接纳的 child；从 durable cold source 重建有效 workspace/config 时共用 resume 的单一解析边界；参考 DeepSeek Harness await agent creation 与 Codex live fork/listener，不增加第二份 thread store | fork response 后无需隐式补一轮 resume 即可 start；持久化失败与内存安装失败不返回假成功或遗留无主 child；热/冷 source、重启、配置、并发 fork/start 与失败补偿一致 |
| Thread/read 历史分页与预算 | H-218 | 在现有 EventStore/RunStore 投影上增加 message-aligned page、明确 page size/字节预算和 opaque continuation；保留 after-sequence 作为增量读取，不把全量聚合塞进单个 response；参考 DeepSeek Harness `SessionHistoryController.page()` 的 500 条上限与 Codex 的 turns/items 分页 | 长历史读取不会一次性构造无界 JSON；调用方能按 cursor 续读、只恢复所需窗口；超预算能以明确 continuation/错误结束，避免 SDK/UI 内存峰值和断线重试放大 |
| Run-scoped capability snapshot | H-219 | 保留兼容 `tool/list`，在现有 run assembler/ToolRegistry/MCP lifecycle 边界生成带 thread/run、workspace、permission profile、sandbox backend、MCP/config 代次、tool schema hash 和风险/来源的 snapshot；在 `turn/start` 或独立 resolve response 中返回并与 run_events 关联；参考 Warp 动态 capability negotiation、Codex thread/turn 配置快照，不新增第二套 registry | 客户端和模型能知道本次 run 的实际工具面；能力变化可审计、重放可复原；sandbox/权限/MCP 失败显示为明确 degradation，不把全局存在误认为当前可执行 |
| Thread/list 分页与过滤 | H-220 | 沿用 EventStore 的 session 元数据查询，在现有协议增加 limit、排序/过滤和 opaque cursor；默认有界，cursor 绑定排序快照并拒绝重复/篡改；参考 Codex `thread/list` 默认 25、最大 100 与 nextCursor，不把 thread/read 全量历史混入列表 | 列表刷新只扫描/序列化当前页；历史规模增长不会阻塞客户端；归档、workspace、provider、更新时间等筛选可在服务端完成，断线可从 cursor 续读 |
| 有效配置读取与来源可观测性 | H-221 | `config/read` 在现有 Settings/ThreadConfig/ServerState 汇合边界读取实际生效的 model、reasoning、permission、sandbox 与来源层；保留全局协议 banner 兼容字段，不把配置解析复制到第二套控制面；参考 Codex `ConfigReadParams` 的 `includeLayers`/`cwd` 与恢复响应中的实际 config snapshot | 客户端能确认下一 turn 使用的模型和执行边界；thread override、恢复、权限/沙箱变化不会静默脱离控制面；诊断和审计可将 wire、run_events 与配置来源对应起来；DeepSeek Responses 参数仍由 provider adapter 负责，不把宿主配置误写成官方 API 字段 |
| Thread 级 reasoning effort 贯通 | H-222 | 扩展现有 `ThreadStartRequest`/`ThreadState`/`HarnessRunOverrides` 的同一配置边界，未知字段不再静默丢弃；`turn/start` 无覆盖时继承线程快照，并在实际 Responses wire 前复用 provider effort 校验；参考 DeepSeek Responses `reasoning.effort`（含兼容别名）与 DeepSeek Harness ACP 的 session config snapshot/prompt pinning、Codex turn 事件中的 reasoning effort | 客户端在 thread 创建时选择的思考模式真正作用于后续 turn；没有“成功但仍用默认 effort”的假成功；每次 turn、重试和工具循环的 effort 可回放、可计费且与 wire 一致；显式 turn 覆盖仍有清晰优先级 |
| 模型目录与用户选择分离 | H-223 | 在现有 SettingsService/ModelCatalog 边界区分首次自动选型、目录元数据刷新与显式角色选择；刷新保留仍可用的选定 ID，失效模型走可观测的拒绝/迁移策略；参考 Codex remote catalog refresh 与 requested model 分离、DeepSeek Harness ACP 独立 selected/choices 与 prompt snapshot | 相同目录刷新不改变用户选型、下一 wire 或持久化角色；模型下架、别名变更、启动刷新与显式选择竞争有明确语义；复用 H-87 的字段级原子提交和 H-88 的凭据代次，不再由 refresh 调用整份初始化 |
| 费用周期与日/月预算边界 | H-224 | 在既有 CostService/CostStore 定义唯一 period resolver，明确本地预算时区、自然日/月或显式账期及起止点；summary 与 admission 复用同一周期；参考 Grok 显式 period 和 Codex reset 身份，不复制供应商账单逻辑 | 本地午夜/月初、UTC、跨年/闰月/夏令时均有可注入时钟的边界回归；历史账本不被重写，周期费用与限额决策一致；不因整数取模无声重置。保留总额、session 用量与既有唯一成本库 |
| Settings 激活与实时计价一致性 | H-225 | 在既有初始化/设置生效边界统一提交有效模型与价格版本，令 CostService 消费同一 ready snapshot；summary 保留未定价完整性，有限预算采用明确的未知费用策略；参考 Claude 请求时公共计价/不准确标记，不复制其默认模型猜价 | 首次登录、已配置启动、显式刷新和登录切换同样生效；初始化成功后首次调用即可计价；NULL 费用不冒充真实 0；保留原始 usage 和价格来源，历史补算需版本/幂等审计 |
| 每调用模型身份与成本结算 | H-226 | 在既有 ModelAgent/provider usage→finalizer→CostService 链保留 run/step/attempt 与实际 provider/model、时间和价格版本，按调用或同价 route 分桶结算；总 RunUsage 保持上下文 gauge/运行累计投影，不新增第二成本库；参考 Claude 按本次 resolvedModel 计价和 DeepSeek Harness attempt route | 双向 fallback、混合模型工具循环、重试/截断/取消均按实际有用量请求记一次；wire、费用事件、价格 snapshot、summary 和下轮准入一致；不能只把初始 model_name 改成最终 model_name，未知 route/费用明确标记 |
| JSON-RPC 方法注册与错误分类 | H-217 | 现有 `HarnessRequest` 解析/dispatch seam 维护唯一方法注册表；未知 method 统一返回 `ERR_METHOD_NOT_FOUND`，已知 method 的 payload 解码失败才返回 `ERR_INVALID_PARAMS`；参考 JSON-RPC 2.0 与 Codex `method_not_found()`，不再用 method 字符串形状推断语义 | 未知 `thread/*`、未来扩展和拼写错误均可被 SDK 明确识别；参数错误与能力不存在分离；错误码、日志和重试策略一致；新增方法只需更新一个注册/dispatch seam |
| Harness SDK 请求期限与 waiter 所有权 | H-90、H-100、H-101、H-204 | 现有 SDK request/pending 表共用 RAII owner 和可配置 deadline；响应、写失败、future drop、超时、EOF 均按 id 收束一次；参考 DeepSeek Harness 可配置 requestTimeout + AbortSignal pending removal，并保持对 Codex per-RPC timeout 能力的诚实边界 | 对端静默时调用按配置期限返回；调用取消不遗留 pending；迟到响应/EOF 竞态不误配；请求已写后的结果未知不自动重放有副作用操作 |
| Harness SDK 通知身份、订阅时序与缺口恢复 | H-01、H-91、H-205、H-214 | 现有 `RpcNotification` 外层序号、SDK 公开订阅项和 `thread/read` durable cursor 统一为带 sequence 的通知信封；提供先建订阅再发请求的 owned run/stream API，参考 DeepSeek Harness `HarnessSession.run()` 的 subscribe-before-prompt 队列与 Codex 的先附加监听再启动操作；不建立无界第二事件库 | 请求响应前通知仍可由该次操作消费；连续事件可按实际序号 ACK；7→9、broadcast lag 和迟订阅均可识别并转为 durable 补读；不把裸业务事件或事件计数冒充已交付游标；旧 DTO 兼容反序列化 |
| 启动恢复的 run/task 诊断归属 | H-44、H-95 | 现有恢复投影按 run 的 task 身份界定历史范围；参考 Codex 按 turn ID 标记 stale turn | 同 session 多个未完成 run 的中断原因各自匹配其 task turn；较新的 user prompt 不改写旧 run 的工具中断原因；重放稳定，且不改变唯一终态 |
| 机器终态与最终钩子的提交边界 | H-83 | 复用既有 Kernel 终态提交和协议投影；答案/钩子进度与最终结果分开，参考 Codex 的统一完成状态与 Claude 更新前执行钩子 | hook 正常/失败/取消、provider error、DB 故障及补读一致；同一 turn 只发布一个与 durable 结果相符的最终状态 |
| 工具实际执行与后置扩展的提交边界 | H-139 | 既有 ToolPipeline/action/Session 保存已执行事实和结果；后置 hook 的反馈、拒绝继续及失败分别投影；参考 Claude 普通工具结果先保留、Codex 区分执行 lifecycle 与 hook outcome | hook 有效/无效/执行错误及重开后，成功副作用不会变成未完成操作；原结果可回放、动作有准确终态、模型配对反馈可解释；不盲目重复执行 |
| 输入 hook 的附加上下文交付 | H-140、H-26 | 既有 hook 输出结果分开携带决定、改写与上下文，接回当前输入的原生 Surface/会话记录和统一 meter；参考 Claude hook_additional_context 与 Codex 独立 additional_contexts 通道 | 普通 stdout 与合法 additionalContext 在实际 wire 中可见且有来源；拒绝仍不发模型；多 hook 有界合并、恢复可回放，不把 UI 日志或任意输出升级为系统权限 |
| 运行取消覆盖所有等待与统一工具调度 | H-10、H-29、H-31、H-34、H-35、H-45、H-119、H-164 | 摘要、prefire、正文、重试、工具、画布视频 HTTP、其他 Canvas 同步模型请求和前台子任务属于现有取消树；共用 ToolPipeline、deadline 与串行屏障；区分执行 Future 丢弃、外部工作清理与持久化收尾 | 各等待中停止及时退出；父超时保留 budget_exceeded 且前台子工作有界收尾；串并行超时一致；Canvas 节点不先 Completed 再把 run 标 cancelled；跨写屏障的读取不提前；任务有 owner；取消不假成功或吞保存错误；本地停止不冒充云任务已撤销 |
| 命令输出读取与缓冲生命周期 | H-60 | 既有 SystemExecutor 共用 reader owner、drain 截止和输出保留预算；参考 Codex 显式 abort reader 及 Claude 清理输出监听/受控文件 | 正常 EOF 释放；单/双管道开放时有界停止 reader，返回后迟到输出不增长孤立缓冲；取消/drop 清理，合法后台服务的输出策略明确 |
| 交互终端的分页范围与恢复游标 | H-123 | 既有 TerminalReadChunk / local 与 SSH reader 统一返回终点及淘汰语义；参考 Codex 将 pending 消费与明确 omission 信息分开，字节游标投影为本地设计 | 每页 cursor 不跳过未返回字节；大输出分页可还原；只有真实历史缺口才报告丢失；持久化/恢复不得将生产位置冒充已交付位置 |
| 可选模型复核与主运行终态一致性 | H-61 | 既有取消树派生复核子 token，面板任务 owner 管理取消与 join，完成提交重查运行取消；参考 Codex 的有界 turn abort 和任务清理 | 取消后不提交成功；总时限/drop 不遗留复核请求；正常 PASS 仍完成，失败反馈有界，取消不当作普通复核失败 |
| 停滞检测的取消与结果采纳 | H-62 | 既有模型取消入口及运行 token 覆盖分类等待、nudge 注入与重入；参考 Grok 分类 generation/idle 核对和 Codex turn abort | 分类中停止及时退出；取消不被 fail-open 吞成继续采样；无延迟新请求；最终取消映射保持正确 |
| 附属模型调用的费用归属 | H-63 | 复用现有 SideCall/CostService，模型响应归属传递保留真实模型和 usage，独立于文本/判定采纳；参考 Claude 公共调用路径记录 usage | panel/classifier 成功或不采纳均记已返回量；缓存/模型准确；缺价保留 tokens；不误加上下文 gauge、不重复记账 |
| 自动构建验证的执行归属 | H-64 | 既有 VerificationPlan 接回受控命令、运行取消和有效截止；参考 Codex ExecExpiration 与进程组清理，不靠 spawn_blocking 丢 future 停进程 | 真实 build.rs 在取消/总时限/drop 后有界退出，终态后无迟到写入；后续步骤不启动；正常验证和单终态保持 |
| 子运行产物与恢复 | H-20、H-21 | 现有 SubagentRunStore / transcript / 工作树所有权的交付契约；参考 Claude 先检查变化再清理、恢复有效 metadata | 文件可定位并交给下游后才清理；恢复不丢原输出契约；重启后仍能定位 |
| 模型可见工具结果资产的生命周期 | H-36 | 现有 ArtifactStore 与 session/run 归属统一完整副本、preview 引用和保留期；参考 Claude 会话结果目录与按年龄回收 | 正常结束、取消和重启后引用在保留期内仍可读；索引与文件一致；回收可诊断 |
| 附件存储身份与写入/删除范围 | H-58 | 既有 AttachmentService 统一有效单组件身份、会话归属和精确附件读写/删除；参考本仓库 ledger 组件验证及 Claude 宿主会话/UUID 文件身份 | 异常 id/session 在写入前拒绝；不覆盖或删除桶/root/兄弟数据；正常导入/幂等删除、junction/竞态和重开分别验证 |
| 定时任务的运行归属与交互能力 | H-37、H-96 | 现有 CronService → AppCore 接纳、审批和通知共用明确的 unattended 契约；参考 Claude 队列归属与 Codex headless 策略 | 需要人工时明确拒绝或投递到真实 owner；计划可关联运行，停止入口可发现；不能留下无消费者审批；poll 故障后循环状态可见且可恢复 |
| 定时调度所有权与触发身份 | H-42、H-96 | workspace 调度 owner、主备接管及稳定 occurrence→现有 RunStore 关联；参考 Claude 项目 scheduler lock 与 inFlight | 多宿主同一时段只派发一次；owner 失效可接管；已接纳 occurrence 重放时命中同一 run，结果未知时不盲目重试外部动作 |
| 工作流工具能力与失败传播 | H-38 | 复用现有工具组装和 ToolPipeline，NodeOutcome 保留结构化失败并定义显式容错；参考 DeepSeek isError 保真及现有 DAG 依赖失败传播 | 只读工具能实际运行；未知工具/拒绝/执行失败不冒充成功；节点、下游及 RunStore 语义一致 |
| 工作流条件可达性与派发 | H-39 | 在现有编译图/WorkflowAgent 统一激活边、跳过边和合流；参考 DeepSeek 程序式工作流只启动实际执行路径的工作 | 未选分支真实执行数为零，嵌套与公共后继正确，不把跳过分支的输出当作已有值 |
| 工作流节点跨语言配置契约 | H-40 | 现有版本化 graph、配置 schema、序列化和执行器统一字段及兼容迁移；参考 DeepSeek 在消费边界拒绝未知/不支持的参数 | 生产前端 schema 合法配置逐项进入正式后端，具体输出符合设置；无效配置拒绝，不能成功返回空默认值 |
| 工作流模型身份与自动路由边界 | H-160 | 现有 provider/model snapshot 上统一 `Auto` 与精确身份类型；参考 DeepSeek Harness 结构化 pair 和 UNKNOWN_MODEL/NO_ADAPTER，Codex 的独立 provider 身份 | 新节点不写死过期模型；非空非法/未知身份不发网络；空值才自动选择；多供应商、旧图迁移和 outcome provenance 显示实际同一身份 |
| 工作流模型生成参数贯通 | H-161 | 在现有 CanvasModelRequest 汇合 typed generation options，各协议 adapter 显式映射/拒绝；参考 DeepSeek Harness 的 request→model→connection 优先级 | 节点显式 temperature/output cap 与有效预算及实际 wire 一致；未指定保留 provider/model 默认；DeepSeek/Anthropic/Gemini、fallback 和截断分别验证 |
| 专业工作流 AI 节点能力闭环 | H-162 | 前端节点目录由后端版本化 executable capability 投影；一次性 LLM/分类/提取复用 Canvas text bridge，Agent V2 复用 ModelAgent/ToolPipeline/审批/取消，不新增运行中心 | enabled 节点从生产前端配置到 provider、结构化输出、事件、终态和 usage 全链可用；未实现能力在添加前明确禁用；修通时同时关闭 H-160/H-161 后续风险 |
| 工作流 provider usage 与预算/成本统一 | H-163 | Canvas adapter 保留 canonical Usage/raw usage，WorkflowAgent 两路只用 RunUsage::record_call；参考 DeepSeek Harness 的 usage chunk/attempt meter 与 Codex completed usage 累计 | text 与 usage 同次提交；prompt gauge/billed sum、cache/reasoning、route 均正确；UsageRecorded、CostService、节点输出和 token budget 一致且不重复计账 |
| Provider 返回资产的下载授权与 SSRF 边界 | H-165 | CanvasArtifactService/provider 配置下统一受控 fetcher，可信配置决定 artifact host allowlist；参考 Codex network policy/approval、DeepSeek Harness owner-bound attachment 与 OWASP 连接时地址校验 | 未授权 loopback/私网/link-local/metadata 及 redirect 请求数为零；DNS/多地址、大小、类型、取消、staging 与日志脱敏一致；合法 CDN/显式本地开发 profile 可审计成功 |
| 按模型能力解析有效采样参数 | H-166 | 既有 ModelCapability/Responses adapter 将模型、reasoning effort 与 top_p 解析为同一个有效设置；参考 Codex 模型目录能力随 turn 选择流动，DeepSeek Harness provider profile 明确 adapter 参数边界 | Flash thinking 的 0.94/0.95 边界及 Pro/none/fallback 均与 UI、设置和 wire 一致；规范化需可见，不声称离线 wire 能证明服务端输出 |
| 生成媒体下载的字节预算与 artifact 配额 | H-167 | 现有 provider/artifact 边界统一流式计数、取消与 staging 清理；参考 Penguin-Magic assets route 的 Content-Length + 实际长度双检查（该实现仍整体缓冲） | image URL 与 inline data 超限均在采满/提交前拒绝；chunked/缺失或谎报长度、并发与总配额有边界；H-165 SSRF 与 H-164 cancellation 独立验收 |
| 动态上下文与 KV cache 历史前缀 | H-168 | 保持稳定 leading instructions，在 Responses adapter 保留官方允许的中途 system input item；变化快照版本化并追加在已缓存历史之后，参考 DeepSeek Harness 的 in-history 更新与 Codex 的 append-only turn context | 静态与各类动态变化下 wire 前缀、持久化、压缩和恢复一致；不提升/重复旧快照；验证中途 system 语义；真实 DeepSeek 用 cached tokens 重复采样，不把尽力而为缓存当强保证 |
| Provider 图片格式与内容身份验证 | H-169、H-70 | 既有 AttachmentService/provider preparation 统一真实字节检测、声明 MIME 对照及模型格式能力；参考 DeepSeek Harness decode/normalize、Codex 显式 unsupported projection、Penguin 画布四格式入口 | 四种支持格式正常；SVG/BMP/TIFF/AVIF、伪 MIME/错扩展在 transport 前拒绝或受控转换；当前/历史/工具图片一致，错误可见且不静默去图 |
| DeepSeek Files 图片身份与跨请求复用 | H-170、H-69、H-70 | 在既有 provider image preparation seam 建立 scoped variant→`file_id` 索引和 typed `Inline | FileId` wire 投影；参考 DeepSeek Harness 的上传复用/刷新/单次失效重传与 Codex 的 `ImageReference`，不新增第二套附件库 | 同账户同 variant 跨轮/重试/并发只上传一次；账户、endpoint、转换版本隔离；过期/删除/限流/配额/取消/重启有界；Files 失败的 all-base64 fallback 重新核算完整请求预算和内容身份 |
| Provider 图片角色与内容保真 | H-171、H-68、H-127 | provider adapter 按 Responses/Chat/Messages 分别拥有 typed image-role policy；参考 DeepSeek Harness 的显式 UNSUPPORTED_CONTENT 与 Codex typed UserInput/ContentItem | Responses user/developer 保留图像，system/assistant 网络前失败；其他协议按自身角色矩阵；压缩/恢复/SDK 导入不静默去图，错误与实际 wire 可审计 |
| Responses 参数的最终 provider 校验 | H-172 | `ModelClient` 在 defaults 合并后复用单一 Responses 参数 validator；参考 DeepSeek Harness 对 maxTokens 在配置、SDK 和 agent 创建边界的多层拒绝模式（不外推为它已验证 temperature/top_p） | request 与 defaults 的 temperature/top_p/top_logprobs 越界均在 transport 前拒绝；官方合法端点仍正常；Chat 与 Responses 按各自协议能力校验；未来 Harness/SDK 复用同一最终边界 |
| Responses 工具输出多模态内容保真 | H-173、H-127 | 现有 ResponseItem/Message 边界增加 typed tool-output text/image 引用，Responses adapter 映射 `input_image`，其他协议显式转换或拒绝；参考 Codex `FunctionCallOutputBody`/typed `InputImage` 与 DeepSeek Harness tool-role 图片投影 | function/custom 工具输出中的图片不被字符串化；内容身份、detail、Files/inline、压缩恢复、计量和错误可重建；无图片工具路径保持原行为 |
| Responses 历史替换与工具配对不变量 | H-148、H-149、H-174 | 所有公开历史构造/替换入口在 provider boundary 复用同一 typed call/result validator；参考 Codex typed `ResponseItem` 与 DeepSeek Harness 带身份的 tool-call block | `function_call` 始终有同 native kind、非空且唯一的 output；未完成历史被结构化修复或明确拒绝，任何 mutator、重试、压缩、恢复和 SDK 都不能绕过校验 |
| 工作流节点事件与规范回放 | H-120 | WorkflowAgent 的 publisher 接回既有 Kernel 持久化 sink；参考现有 ModelAgent 通道与 ArtCraft 保存媒体任务身份，复用 run_events | 各节点实时事件与重开补读一致，失败/取消/进度也可重放；保留媒体来源和任务身份，不靠图文档偶然自动保存代替运行日志 |
| 媒体任务身份与节点投影/续跑 | H-121、H-122 | 既有节点事件携带的运行中身份统一投影，后端以 provider/account/task 和输入身份决定新生成或恢复；参考 ArtCraft 的提交与 pending task/provider_job_id 分离，DAG 输入失效为本地设计 | 收到 Running 即可安全保留续跑身份；有效续跑不重复 submit，修改输入的新生成实际提交新请求；旧 attempt 不覆盖新任务，已完成资产保留来源 |
| 桌面运行事件与订阅生命周期 | H-41、H-43、H-134 | 现有 run_id、完成 payload、运行 owner、订阅就绪屏障和持久化补读统一；参考 Codex thread/turn/item identity 与 ArtCraft 异步 listener owner | 外部会话终态不影响画布；ACK 前完成/快速失败不丢终态及节点结果；监听失败不先派发；停止仍能发送对应取消；旧回调不能改写节点或清理新 run |
| 多宿主所有权、会话派发与启动恢复 | H-32、H-33 | 数据库打开与有所有权的恢复分离；现有会话租约及运行/动作/子运行共同定义 owner，参考 Claude 存活 owner 保护、Codex thread writer 锁与后台作业有效租约保护 | 第二宿主打开不改变活跃记录；同会话不同时派发，异会话可并行；真正失效 owner 才恢复；原宿主仍可正常完成 |
| 共享设置与凭据的提交资格 | H-87、H-88 | 现有 SettingsService/SecretStore 在数据库当前版本上提交字段变更；参考 Codex Session 设置持久化许可与 DeepSeek 账户撤销/登录生命周期，跨宿主版本栅栏为本地设计 | 刷新不覆盖已收紧限制；退出/换 Key 后旧刷新不能写回旧凭据；独立连接、真实主备进程、失败和乱序完成一致 |
| 启动恢复的规范终态、会话投影与诊断归属 | H-44、H-95 | 恢复共用现有幂等终态边界，文件会话投影与 interruption 分类均具备按 run/task 身份的处理；参考 Codex 对 stale turn 的一致 Interrupted 投影 | 正常/故障重试/重开后各视图同一终态；一条规范终态；缺失投影可补；多个 unfinished run 的原因各自对应目标 turn，不能改错后续 turn |
| MCP 连接协议与投递 | H-06、H-07、H-08、H-09、H-12 | 现有 MCP manager/transport 内统一握手、会话头、分页、按 request id 分发；连接恢复与业务重试分离 | 严格 Node/HTTP fixture、乱序双请求、模糊成功写调用不自动重放 |
| MCP 运行中的工具目录同步 | H-137 | 既有 client/transport 分发列表变更通知，McpService 在有效配置代次内重新发现并发布同版本 schema/路由；参考 Claude 定向缓存失效和 DeepSeek 串行同步/连接身份检查 | 下一运行看到新增、删除及 schema 更新；刷新失败、重复通知、禁用/替换竞争有界；不在对话路径建连，不只改 UI |
| MCP 原始结果与模型内容的投影 | H-127、H-138 | 既有 adapter/observation 保留宿主原始结果、独立元数据与 canonical 模型内容；参考 DeepSeek 显式 content/structuredContent 和 Claude mcpMeta 通道 | 客户端 _meta 不默认进入 wire，公开结构化内容和多模态不丢；成功/失败、持久化/回放、provenance 与诊断有明确契约 |
| MCP 整次调用取消与部分写入状态 | H-128、H-136 | 既有执行 owner 覆盖发送锁、请求写入和响应读取；参考 Codex 外层 dispatch 取消和 DeepSeek 每次调用的 signal，半帧后的连接处置须有明确本地契约 | 背压及锁排队时停止不依赖远端读取；不会复用损坏帧连接；保留结果未知、单终态和工具配对，不影响其他会话取消树 |
| 本地 MCP 进程归属与退出 | H-46 | 参考 Codex 服务级 Job/进程组 owner，显式关闭与最后引用释放共用有界清理；不依赖被启动程序自行管理后代 | Node 与非 libuv 根服务均覆盖；close/Drop、父先退出、初始化中断后后代不继续工作；containment 降级有诊断 |
| 本地 MCP 子进程环境授权 | H-135 | 现有统一进程启动边界管理基础环境、显式转发/覆盖和不可继承变量；参考 Codex 白名单、DeepSeek 共享清理及 Claude 条件策略，具体默认值为本地设计 | 空配置不隐式获得未授权宿主凭据；显式授权、大小写、代理/CA、首次启动/测试/重连一致；不以环境过滤代替 OS 沙箱 |
| MCP 初始化复用与配置代次 | H-47 | 按服务连接身份复用 pending startup，定向取消/失效；全局视图代次不能写坏仍有效的共享 slot | 新增或修改 B 不取消未变 A；禁用/替换 A 才终止旧任务；迟到取消结果不变成新代缓存失败 |
| MCP 模型可见名称与快照发布 | H-56 | 现有 registry 在发布前统一名称唯一性和 raw owner 路由；参考 Codex 对最终可见名的消歧 | 多快照合并不发布重名；能力冲突不阻塞普通对话；历史和权限匹配保持可解释 |
| MCP/扩展工具参数 schema 发布资格 | H-159 | 在现有统一能力快照边界校验对象根并编译 JSON Schema，保留来源并隔离坏能力；参考 DeepSeek Harness 的类型化 schema 编译/断言，provider boundary 再防御 | 缺失、非对象和不可编译 schema 均不进入 Chat/Responses wire；一个坏工具不阻断普通对话；动态刷新、deferred、WASM/插件和 replace 共用同一规则 |
| 能力目录和使用说明的内容保留与去重 | H-57 | 既有原生 Surface/事件保存实际正文并基于当前内容去重，或每请求从就绪快照注入；参考 Claude 的 MCP/skill 正文附件及 DeepSeek 连接拥有的 system section | 连续 turn 的完整 wire 均保留有效目录/说明；压缩/恢复不因旧身份标记丢正文；断线与配置更新语义一致 |
| MCP 断线恢复耗尽与能力退役 | H-48 | 探活与工具共用连接身份的恢复 owner/预算，状态驱动下一请求就绪快照；参考 Grok stdio 耗尽后 Unavailable 与注销工具 | 失败耗尽后新发现不含旧工具/说明；普通连续调用不重开启动风暴；显式重试可恢复；不影响其他服务 |
| 项目配置来源、权限与 hooks 决策 | H-26、H-27、H-28 | 配置 overlay 和独立 hook 文件共用来源授权；有效 profile 明确用户权限上限；参考 Codex 禁用未信任配置层、Claude 的 hook trust gate 与 hookSpecificOutput | 未授权项目配置不能放宽用户只读或审批；更换配置文件不能执行脚本；标准 deny/ask 能实际拦截/审批 |
| Hook 生命周期与输入/输出契约 | H-102 至 H-117 | 现有 HookData 按事件定义所需上下文，command/HTTP 共用 serializer；参考 Claude 按事件声明输入 schema 和输出处理。真实 session owner、调用 ID、transcript 与结果在派发点传入，压缩 stdout 由 compactor 明确消费 | 正式入口捕获的 payload 可校验且身份正确；普通续聊不误发 SessionEnd；失败不误发 TaskCompleted；API 错误终止派发一次 StopFailure，Stop 拒绝重试不误报；PreCompact 指令进入实际摘要请求；缺字段不能靠任意占位值掩盖 |
| Provider 流式协议、错误和请求恢复 | H-24、H-25、H-29、H-30、H-118 | 既有 transport 内分清字节解码、SSE 帧及 provider JSON；统一错误元数据、重试及正文取消边界；参考 Codex/DeepSeek 的事件解析器及全请求取消 | 标准 SSE 格式等价；半帧不能假完成；Unicode 不 panic；成功/错误正文和重试等待均可取消；流内错误保留实际 code，已支持的溢出分类不依赖 message 措辞 |
| 模型切换的有效请求参数 | H-49 | 有效模型能力、输出上限、budget、meter 和 wire 共用同一次配置解析；参考 Claude 的 fallback 重新进入 callModel 并按新模型重组默认参数 | 小输出容量备用模型可接续；快照预留与 wire 一致；显式覆盖及截断恢复不得重新引入超限字段 |
| 有效模型参数与输出预算 | H-54 | 既有配置 overlay、模型能力、桌面深度缺省统一解析到最终请求；参考 DeepSeek 本次 effort/maxTokens 优先及 Codex wire 断言 | 显式 effort/输出上限生效；发送前及下一请求预算与 wire 一致；无覆盖保持默认；不支持值明确拒绝；重试和子运行不重新覆盖 |
| 思考模式的协议转换 | H-55 | 既有 thinking 配置表达开关/强度，由 adapter 显式生成和校验自己的 wire 字段；参考 DeepSeek 分 provider 的 off 映射 | Simple/Medium/Deep 经两协议完整 Agent 路径均可发送；禁用不被默认强度重新启用 |
| Responses 思考强度设置契约 | H-153、H-155 | Harness 固定 Responses 路由；官方 alias 在输入边界规范化为 enum 后由 Responses serializer 输出；Chat/Anthropic 字段不能混入 | 规范档位和官方 alias 均可保存；alias wire 归一为规范值；`none` 和 override 优先级正确；真实 API 参数页与指南差异保持显式未决，不能无依据发送 `output_config` |
| Responses `apply_patch` custom tool schema | H-156 | 使用 DeepSeek Responses 文档明示的最小 custom shape；配对 item 沿用原生 Surface | 捕获真实装配的 JSON，核对 provider 对当前额外字段的反应；固定 request shape，并通过一次受控真实调用和 function/custom 配对回归 |
| Responses 工具选择协议 | H-93、H-145、H-158 | 现有 provider adapter 分别拥有 typed choice，设置与最终请求复用同一校验；参考 Codex/DeepSeek Harness 只暴露 adapter 支持的形状 | Responses 只接受字符串模式或扁平 named function；custom/web_search 在网络前拒绝；Chat 显式映射自己的嵌套形状；Thinking 组合与最终工具存在性均验证 |
| 模型与搜索能力的有效选择 | H-66 | 既有模型/协议能力解析统一 hosted 与 callable search；参考 DeepSeek Harness 的独立 web provider seam，原生能力以当前官方限制为准 | Flash 不被注入会忽略的唯一搜索入口；Pro 原生及外部搜索正常；禁用、切换、备用模型和子运行策略一致，真实搜索数与错误可验证 |
| 当前多模态用户输入与实时 Surface | H-67 | RunAssembler/ModelAgent 共用同一已接纳消息的文字、附件与持久化归属；参考 DeepSeek Harness 在创建 user message 前完成有序附件接纳 | 首轮与续轮新图均进入本次实际请求；无重复 user item；历史/压缩/重启、系统视觉文本及 meter/wire 一致；不可读附件的行为独立验证 |
| 附件请求准备的失败语义 | H-68 | 既有 AttachmentDataResolver 区分不适用与读取失败，并在采样前保留结构化错误；参考 DeepSeek Harness 的附件错误和 await 准备链 | 正常图可用；丢失/权限/空文件/超限有明确结果；不静默删除模型图片；恢复、取消、单终态及实际请求数可验证 |
| 多图路由的传输预算 | H-69 | 复用附件/provider seam，独立控制请求图像编码、inline/Files 字节及数量；参考 Harness 的请求版本和显式 offload，完整 JSON 边界为本地设计 | 合法单图可发；多图和文字/schema 累积不交出超限 body；重试/备用模型、取消、恢复及 Files 降级可验证 |
| 历史附件的内容身份 | H-70 | 复用导入摘要与附件存储，把期望身份贯穿引用与请求读取；参考 Harness 的 digest/metadata 校验，旧记录迁移为本地设计 | 正常读取身份一致；合法替换/损坏不借旧 ID 发送；受控转换可追踪；旧会话、重启、取消、实际字节和单终态可验证 |
| 发现能力与输入模态 | H-71 | 既有 ModelInfo/ModelCapability/provider seam 共用本次有效模型的模态声明；参考 Harness 在准备图片前检查 model inputModalities | 目录/恢复保留能力；文字模型不交出图片请求；未知能力、切模型/fallback/子运行、旧目录和真实请求计数可验证 |
| 检查点恢复的失败一致性 | H-72 | 既有 CheckpointManager 先验证来源与计划，完成临时替换后提交；参考 Claude 的缺失备份前置检查，跨文件/历史补偿契约为本地设计 | 缺失备份不删除当前文件；部分失败、DB 截断失败及重试可诊断和恢复；rewind/fork、目录与 Windows 文件占用分别验证 |
| 回退后的派生状态分支身份 | H-23、H-73 | 现有回退事实同时决定事件、检查点恢复资格和笔记覆盖；参考 Claude 的 messageId 快照与 Codex 的有效历史位置，持久化提交/补偿为本地设计 | 丢弃分支不覆盖新文件/摘要；保留前缀仍可回退；同序号替换、重启、fork、多次回退与失败重试可验证 |
| 源事件与原生镜像的语义提交边界 | H-74 | 现有 Session/EventStore 关联逻辑消息与完整镜像组，分支/回退统一解析语义锚点；参考 Codex 按 turn/message 计算边界，原始 sequence 兼容为本地设计 | 保留的消息完整进入下一 wire；raw 锚点、旧日志、崩溃、附件/reasoning/工具和压缩不拆组，不在 UI 散落 +1 特判 |
| 活跃运行与回退的提交所有权 | H-75 | 现有输入租约/RunCoordinator 与回退共用同会话所有权，明确拒绝或取消并等待；参考 Claude 恢复前 onCancel，持久化分支提交栅栏为本地设计 | 成功回退不再接收被丢弃 turn 的迟到回答；新接纳、排队、工具/审批、终态及重启保持一致，原审计不删除 |
| 文件读取版本与后续修改资格 | H-76 | 现有 FileStateCache/文件工具统一已读版本、执行时冲突校验和实际写入；参考 Claude 的 stale read 拒绝与检查后不 await，跨进程提交约束为本地设计 | 过期写不覆盖外部改动，重读后正常修改；整文件/部分读取、write/edit/multi_edit、外部编辑与格式化、失败/取消和审计一致 |
| 搜索遍历与读取拒绝规则 | H-77 | 隐式候选文件共用既有 WorkspaceRoot/读取策略，读内容前判定；参考 Claude Read deny → grep/glob 排除项 | 直接拒绝的内容不经搜索进入下一 wire；默认/显式 glob、敏感名称/后缀和普通文件对照一致；路径别名和名称可见性另有明确契约 |
| 物理路径解析与工作区隔离 | H-78 | WorkspaceRoot/PathGuard 解析真实父级/目标路径，文件工具和隐式搜索共用；参考 Claude 原始路径+realpath 检查，Codex 平台沙箱为独立对照 | WorkspaceWrite 不能跟随 junction 读取或写到根外；正常目录、悬空目标、FullAccess、平台差异和竞态语义可验证 |
| 文件发现名称与读取策略 | H-79 | 现有 WorkspaceRoot/读取策略为 list_dir、glob、grep 提供统一候选可见性；参考 Claude 将 Read deny 转为 glob/grep 排除项 | 被拒绝文件的名称不进入下一 wire；普通文件仍可发现；默认/显式模式、物理路径、配置变更和恢复语义一致 |
| Responses 工具调用与结果类型不变量 | H-80 | 现有 repair_tool_call_pairs 按 call_id 和原生 kind 共同配对；参考 Codex 分别验证 function/custom 两组并要求对称 | custom/function 结果类型不混用；旧事件、恢复、压缩和取消后的补结果都能在 wire 前明确拒绝或校正 |
| Responses 增量与完成 item 的内容归属 | H-81 | 现有累加器按 item 身份拥有 delta/completed 状态；参考 Codex 活动 item 的展示增量与完成项提交分离 | 多文本 item 不额外追加全文；相同文本不同 item 不误删；流式、最终、Surface、重开后 wire 一致；兼容 fallback 只覆盖未完成对应项 |
| Responses incomplete 原因与恢复策略 | H-82 | 既有累加器/ModelFailure 保留原因，长度与过滤分别决策；参考 Codex 独立 ContentFilter 及专门指引 | 仅真实输出到限走预算升级/长度续写；过滤/未知原因有明确有界策略、usage 不丢、单终态及取消成立 |
| 移动设备快照与操作关联 | H-50 | 在现有 MobileService/SnapshotStore 统一快照身份与设备操作顺序；参考 Codex 的版本栅栏原则，移动实现为本地设计 | 最新引用可执行；旧/未知引用不得下发命令；并发快照/输入和不同设备隔离，None 语义明确 |
| 项目记忆与检索的运行作用域 | H-51 | 现有 vault/index 按项目绑定稳定运行视图，global 共享；参考 Claude 对目录身份的显式传递，多会话视图为本地设计 | UI/其他会话切项目不改运行内工具、检索和后台工作的目标；不同项目磁盘读写隔离，global 按配置共享 |
| 知识 vault 的多宿主事务与索引一致性 | H-52 | 既有锁覆盖读取、身份/版本核对与写入，派生索引按提交失效/更新；参考 Claude 写前版本校验，多宿主事务为本地设计 | 不同标题同 slug 不覆盖；同条目冲突可见；真实多宿主、外部编辑和重开磁盘一致 |
| 压缩后的记忆可见性与检索预算 | H-53 | 当前保留 Surface 的附件身份统一去重、配额与回放；参考 Claude 从当前 transcript 收集已展示记忆 | 旧摘录移除可再次检索；尾部仍保留的不重复注入；在途结果、snip 和恢复一致 |
| 模型 Surface、压缩与用量 | H-05、H-10、H-11、H-14、H-18、H-23 | 同一原生提交边界决定完整调用、截断续写及回放；覆盖点绑定真实分支身份；预取有 owner；压缩采用与已发生消耗分离；参考 DeepSeek Harness 的 max-tokens 过滤 | 未闭合调用不提交；live/replay 一致；回退后旧笔记不可覆盖新分支；无效摘要仍记已返回 usage；结束后无游离预取 |
| 能力快照与传输恢复 | H-01、H-04、H-13 | 现有快照代次、宿主配置和持久化游标；参考 Codex 的慢连接处理与 Grok 的只读 registry 快照 | 禁用后新请求无残留技能；SDK 感知缺口并可恢复；resume/steer 不丢有效配置 |

其中子运行已经出现三类独立执行上下文分裂（H-19/H-20/H-22）。后续修复应先确定同一配置/工作树/工具注册表的所有权和生命周期，再按这些探针逐项关闭；不能只隐藏工具、修改提示路径或延迟删目录来宣称隔离已成立。

工作流同样已确认能力装配/失败传播（H-38）、路径派发（H-39）、跨语言配置契约（H-40）三个独立断点。不能继续用单个字段别名、前端状态改色或无条件跳过节点修补。应先明确现有版本化节点契约及执行状态机，再逐项接回同一工具注册/权限/Kernel 链路，以跨语言真实请求、节点输出、实际调用数和 RunStore 终态共同验收。本轮未获生产修复授权，以上仍为审计建议。

画布媒体已分别确认并修复 HTTP 等待取消（H-119）、节点 publisher 绕开规范事件日志（H-120）、运行中 task ID 被前端丢弃（H-121）和修改输入却复用已完成任务（H-122）四个边界。H-122 修复后通过现有 run_events 和画布节点传递 task/fingerprint 配对身份；provider task 只有在后端重算的输入与 provider 指纹匹配时才恢复。H-134 已确认旧 attempt 的迟到节点事件可以覆盖新 run 状态；仍需审计真实云端取消和其他未修复项。

Hooks 的连续输入契约缺口（H-102 至 H-115）也指向同一工程边界：当前多个不同事件复用过于粗粒度的 HookData，再由两套 serializer 逐项拼字段。后续修复应先建立事件级契约与正式派发点的上下文传递，并统一 command/HTTP 投影；不能逐个补空字符串、固定布尔值或从 message 文本反推 metadata。探针只证明各自记录的路径，不把字段审计当成全部 hook 生命周期已经验收。

### H-208：Responses `response.completed` 缺少必填 response 字段仍被当作成功

**级别：中高（P1，供应商流协议完整性与运行终态污染）；触发条件：** DeepSeek Responses 流发送 `response.completed`，但事件中的 `response` 对象缺少官方 schema 要求的 `id`、`status` 等字段（本次最小样本为 `response={}`）。当前客户端仍会发出 `Finished(Stop)` 并返回成功 `Response`，因此畸形、截断或代理改写的终态可能被写入正常完成路径。

#### 官方契约、生产路径与正式复现

- DeepSeek 官方 [Responses API 指南](https://api-docs.deepseek.com/zh-cn/guides/responses_api)规定流以 `response.completed`、`response.incomplete` 或 `response.failed` 结束，`response.completed` 携带包含 `usage` 的完整 `response` 对象。官方 [Responses 参数文档](https://api-docs.deepseek.com/zh-cn/api/create-response)的 Responses schema 将 `id`、`object`、`created_at`、`status` 标为 required，`status` 的合法值包括 `completed`、`incomplete` 和 `failed`。
- 当前生产链路是 `HttpTransport::stream()` → `ModelClient::stream_response_observed()` → `ResponseAccumulator::push_sse_data_observed()` → `ResponseAccumulator::finish()`。累加器对 `response.completed` 只设置 `terminal=FinishReason::Stop`、`saw_terminal=true` 并可选读取 usage；它不要求 `response` 存在，也不校验 `response.id`、`response.status`、`object` 或终态值与事件类型一致。
- 新增正式 ignored 探针 `crates/deepagent-models/tests/responses_terminal_contract_audit.rs::audit_completed_event_without_required_response_fields_is_rejected`。探针通过正式 `ModelClient` 和受控 `HttpTransport`，先发送一个文本 delta，再发送 `{"type":"response.completed","response":{}}`；没有真实供应商、工具、文件或公网请求。
- 执行：`cargo test -p deepagent-models --test responses_terminal_contract_audit audit_completed_event_without_required_response_fields_is_rejected --offline -- --ignored --exact --nocapture`。
- 实际输出：`Responses terminal contract audit: error=false transport_calls=1 result=Ok(Response { output_items: [Message { role: "assistant", content: "offline" }], finish_reason: Some(Stop), usage: None, raw_usage: None })`，随后断言 `response.completed without required response.id/status must be rejected` 失败，结果为 **0 passed / 1 failed（预期复现）**。这证明缺口在正式模型客户端边界，不是测试绕过生产解析器；`transport_calls=1` 仅表示受控 fixture 已被消费，不是公网发送。

#### 成熟 Agent 对照

- **Codex** 的 `借鉴/codex/codex-rs/codex-api/src/sse/responses.rs` 为完成事件定义 typed `ResponseCompleted { id: String, ... }`，`response.completed` 的 `response` 缺失或无法反序列化时返回 stream error，不会生成 `ResponseEvent::Completed`。它没有在该结构中把 DeepSeek 的 `status` 作为字段保留，因此本系统不能声称 Codex 已覆盖所有 DeepSeek 专属字段；可确认的成熟边界是至少不接受缺失 response/id 的终态。
- **DeepSeek Harness** 的 Responses 兼容路由沿用 typed provider event/adapter 边界，测试 fixture 始终提供完整 response 对象后才产生最终 assistant/tool 事件；本轮核对其 `packages/llm/llm-pi-ai/tests/tool-argument-streaming.spec.ts` 的完整 `response.completed.response` fixture，没有把其 OpenAI 兼容测试字段直接当作 DeepSeek schema 的替代证明。DeepSeek 官方自己的 schema 是本项判断必需字段的依据。
- 这些成熟实现的共同点是：终态先经过 provider event 解码和身份校验，再进入 turn/run 完成 reducer；不会仅凭字符串事件类型把任意 JSON 当作成功完成。

#### 影响、后果与根因修复方向

- 触发后，缺失/截断 `id` 或错误 `status` 的上游响应会被记录为正常 Stop；上层可能发布 succeeded/completed、写入空 usage，随后继续持久化 assistant 内容或允许下一轮。这样会把 provider 协议损坏伪装成业务成功，掩盖中间代理、供应商回归或传输截断，并让重试/诊断选择错误的分支。
- 本次样本没有模拟真实网络截断，也没有声称供应商会主动发送该非法对象；它证明的是当前客户端对任何能到达 SSE parser 的畸形终态缺少拒绝门禁。H-200 记录的是合法 response 的 `id/model/sequence` 在 `run_events` 落盘时丢失，属于**已解析元数据的持久化保真问题**；H-208 是**解析前终态 schema 未校验**，二者根因和修复位置不同。H-133 记录 `response.failed` usage 丢失，也不是同一问题。
- 根因修复应收敛到 Responses provider adapter 的单一终态校验：`response.completed`/`response.incomplete`/`response.failed` 必须有 object、非空 `id`、合法 `status`，并校验事件类型与 `status` 一致；usage、error 和 incomplete reason 按各终态的官方可选性解析。校验失败应返回带 provider event/type/字段路径的结构化 stream error，不得补默认 id、默认为 completed 或继续发 Finished。
- 回归应覆盖：缺少 response、缺少/空 id、缺少 status、`response.completed` 携带 `status=incomplete/failed`、incomplete 的合法 reason、failed 的 error/usage、正常完整 response，以及 Chat Completions 独立分路。终态拒绝前不能发布 `Finished`、运行成功或 durable terminal；正常 DeepSeek response 仍需保留 usage 和既有 H-200 的身份/顺序审计。

本轮只新增审计探针和文档，未修改生产实现；正式单元/集成回归与修复仍待授权进入实现阶段。

### H-209：Responses 输出 `message` item 缺少 `role` 仍被静默补成 assistant

**级别：中（P2，输出协议保真与异常响应隔离）；触发条件：** Responses 流的 `response.output_item.done` 返回 `type=message`，但缺少官方输出 message 的 `role` 字段。当前 parser 将缺失值默认成 `assistant`，继续生成成功的 `ResponseOutputItem::Message`，没有保留“供应商输出不完整”的诊断。

#### 官方契约、生产路径与正式复现

- DeepSeek 官方 [Responses 参数文档](https://api-docs.deepseek.com/zh-cn/api/create-response)的 response output item schema 将 `role` 定义为 message item 的字段，合法值只有 `assistant`，示例完整 item 也同时带有 `id`、`status`、`role` 和 `content`；官方流式事件示例同样在 message `output_item.added` 中携带 `role=assistant`。
- 当前生产链路是 `HttpTransport::stream()` → `ModelClient::stream_response_observed()` → `ResponseAccumulator::push_sse_data_observed()` → `record_output_item()`。`record_output_item()` 对 `message` 使用 `.get("role").and_then(as_str).unwrap_or("assistant")`，缺失、空值和非字符串都被转换为合法-looking 的 `ResponseOutputItem::Message`；它也没有验证输出 message 的 role 是否确实为 `assistant`。
- 新增正式 ignored 探针 `crates/deepagent-models/tests/responses_output_item_contract_audit.rs::audit_output_message_without_role_is_rejected`。受控 transport 发送一个 `output_text.delta`，随后发送同文本的 `response.output_item.done`，item 为 `{"type":"message","content":[{"type":"output_text","text":"offline"}]}`，再发送包含完整 `response.id/object/created_at/status/model/output` 的合法终态。这样排除了 H-81 的 done-only 空 message fallback。
- 执行：`cargo test -p deepagent-models --test responses_output_item_contract_audit audit_output_message_without_role_is_rejected --offline -- --ignored --exact --nocapture`。
- 实际输出：`Responses output item contract audit: error=false transport_calls=1 result=Ok(Response { output_items: [Message { role: "assistant", content: "offline" }], finish_reason: Some(Stop), usage: None, raw_usage: None })`，断言 `message output items without the required assistant role must be rejected` 失败，结果为 **0 passed / 1 failed（预期复现）**。第一次 done-only 夹具曾同时出现一个空 assistant fallback；已改为 delta+done 同内容并重跑，最终证据只保留 H-209 的 role 默认化行为。

#### 成熟 Agent 对照

- **Codex** 的 `借鉴/codex/codex-rs/protocol/src/models.rs::ResponseItem::Message` 将 `role: String` 作为非 Option 字段；`codex-api/src/sse/responses.rs::process_responses_event` 通过 `serde_json::from_value::<ResponseItem>` 解析 `output_item.done`，缺少 role 会在 typed decode 边界失败并记录 stream error，而不是默认写入 assistant。
- **DeepSeek Harness** 的 provider/LLM 层将消息角色保持为受约束的模型消息结构，测试与转换链不会把缺少角色的 provider output 任意提升成 assistant；本轮以官方 DeepSeek Responses schema 和 Codex typed decode 作为可核验对照，没有把 Messages 协议字段直接当成 Responses 的替代实现。
- 共同边界是：provider 输出的身份字段先被 typed parser 验证，再交给会话/展示 reducer；缺失字段应成为可诊断协议错误，不应由 UI 语义默认值掩盖。

#### 影响、后果与根因修复方向

- 触发后，格式损坏、代理截断或未来 provider 变更导致的缺失 role 会被当作正常 assistant 输出。当前样本仍能显示文本，但错误响应的来源、字段完整性和跨 item 身份不可审计；若后续允许其他 output message role 或依赖 item identity 做恢复，默认化还会把错误归属到错误角色。
- 本项与 H-81 不同：H-81 是合法 message item 与全局 delta/完成归属不一致导致重复或空 item；H-209 是单个 message item 的必填身份字段缺失。与 H-203 也不同：H-203 校验的是输入 `Message`/`InputMessage` role 越过 provider 边界，H-209 校验的是 provider 输出 item 的角色缺失。
- 根因修复应在 Responses parser 使用 typed output-item DTO 或显式字段校验：`message.role` 必须是非空 `assistant`，缺失、空值和其他角色均返回带 item type/字段路径的 stream error；不要在解析层以 `unwrap_or("assistant")` 补默认值。`id/status/content` 的必要性也应按官方 output item schema 分 item 类型明确建模，不能把所有未知字段静默降级。
- 回归应覆盖：合法 assistant message、缺失/空/非字符串 role、非法 role、缺失 content、delta+done、done-only（与 H-81 分开断言）、多个 message item、function/custom tool item 和正常完整终态；错误发生前不得发布错误的 `Finished` 或 durable 成功事件。

本轮只新增审计探针和文档，未修改生产实现；H-81 的已有证据保持独立，正式 parser 修复和全链路回归尚未实施。

### H-210：Responses 流非单调 `sequence_number` 被静默接受

**级别：中（P2，流完整性与恢复诊断）；触发条件：** DeepSeek Responses SSE 事件携带重复、跳号或逆序的 `sequence_number`。官方协议把该字段定义为递增序列，当前 parser 却完全忽略它，继续把异常事件折叠成成功响应。

#### 官方契约、生产路径与正式复现

- DeepSeek 官方 [Responses API 指南](https://api-docs.deepseek.com/zh-cn/guides/responses_api)说明：流式响应的每个事件都带递增 `sequence_number`，并以 `response.completed`、`response.incomplete` 或 `response.failed` 结束；这使序号成为流顺序和缺口检测的 provider 契约，而不仅是展示字段。
- 当前生产路径是 `HttpTransport::stream()` → `ModelClient::stream_response_observed()` → `ResponseAccumulator::push_sse_data_observed()`。累加器读取 `type`、文本/工具字段和终态，但不读取或保存顶层 `sequence_number`，因此重复、跳号、逆序和缺失序号不会改变状态，也不会生成诊断错误。
- 新增正式 ignored 探针 `crates/deepagent-models/tests/responses_sequence_audit.rs::audit_non_monotonic_responses_sequence_is_rejected`。受控 transport 给同一响应依次发送 `response.created` 序号 0、`response.output_text.delta` 序号 0、`response.output_text.done` 序号 4、`response.completed` 序号 2；这同时覆盖重复、跳号和逆序，并保持合法终态字段，排除了 H-208 的终态缺字段因素。
- 执行：`cargo test -p deepagent-models --test responses_sequence_audit audit_non_monotonic_responses_sequence_is_rejected --offline -- --ignored --exact --nocapture`。
- 实际输出：`Responses sequence audit: error=false transport_calls=1 result=Ok(Response { output_items: [Message { role: "assistant", content: "offline" }], finish_reason: Some(Stop), usage: None, raw_usage: None })`，断言 `duplicate, skipped, or descending sequence_number values must be rejected` 失败，结果为 **0 passed / 1 failed（预期复现）**。

#### 成熟 Agent 对照

- **Claude Code** 的 `借鉴/claudecode/restored-src/src/cli/transports/SSETransport.ts` 对传输层 SSE frame id 维护 `lastSequenceNum`，记录已见序号并检测 duplicate；断线重连会通过 `from_sequence_num` 与 `Last-Event-ID` 请求续传。它的 frame id 不等同于 DeepSeek JSON `sequence_number`，但证明成熟 Harness 不把可恢复流的顺序信息静默丢掉。
- **DeepSeek Harness** 的 `借鉴/deepseek-harness/packages/core/session` 将持久事件日志定义为序号连续，`seq` 始终等于日志长度，并提供按精确序号读取和范围快照；异常序号会在统一 append/恢复边界暴露，而不是由各个 UI reducer 猜测。
- **Codex** 的 Responses SSE 结构体当前也未发现对 provider `sequence_number` 做严格递增校验的证据；因此本项不是声称 Codex 已经完整解决 DeepSeek 序列校验，而是记录本系统在官方 DeepSeek 契约上的明确缺口。Codex 仍使用 typed event/parser 和可观察的 stream error 边界，异常事件不应被当作正常完整流无声吞掉。

#### 影响、后果与根因修复方向

- 触发后，代理截断、重复重放、服务端乱序、拼接错误或未来断线恢复实现都可能被当成正常完成。当前回答文本可能仍正确，但 Harness 无法判断中间事件是否缺失或重复，后续工具参数、reasoning、输出 item 和 durable event 的顺序可能不可回放；重试/续传若以后接入，还可能重复执行同一工具调用。
- 本项与 H-200 不同：H-200 证明合法 provider `response.id/model/sequence_number` 没有进入 `run_events`，属于可观测元数据丢失；H-210 证明非法序列在 provider parser 边界就被接受，属于流完整性校验缺失。与 H-205 也不同：H-205 是 SDK 外层通知序列字段被 reader 丢失，而 H-210 是 DeepSeek Responses payload 内的序列从未被读取。
- 根因修复应在 Responses parser 为单次 response 生命周期维护 `last_sequence_number` 与已见集合：首事件应为合法非负序号；后续序号必须严格递增，重复、跳号、逆序、类型错误和缺失按策略返回带事件类型/序号的 stream error。若服务端确实允许非连续但只要求递增，应至少拒绝重复/逆序并记录 gap；不能继续静默成功。序号应通过既有 `ModelStreamEvent`/`RuntimeEvent` 以可选、兼容字段保留，避免另建 event store。
- 回归应覆盖：连续序号正常流、缺失字段、null/负数/字符串、重复、跳号、逆序、终态序号、错误后不发布 `Finished`、新 response 重试时状态重置，以及 Chat Completions 独立分路。正式修复前不能把“官方每事件递增”扩展为未经证实的 provider resume 协议。

本轮只新增审计探针和文档，未修改生产实现；H-200、H-205 的既有证据保持独立，正式 sequence 校验、事件投影和重连策略尚未实施。

### H-211：Harness SDK 将字符串 JSON-RPC response ID 强制折叠为整数，导致请求无法关联

**级别：中高（P1，协议互操作与请求生命周期）；触发条件：** JSON-RPC server 对合法请求返回字符串形式的 response `id`（例如请求 ID 为字符串 `"1"`）。当前 SDK reader 使用 `as_i64().unwrap_or_default()`，把该 ID 转成整数 `0`，因此找不到原 pending 请求；调用不会在响应到达时完成或失败，而是一直占用 pending，直到对端 EOF/关闭。

#### JSON-RPC 契约、生产路径与正式复现

- JSON-RPC 2.0 的 request/response `id` 可以是 String、Number 或 Null（Null 不应作为请求 ID）；客户端不能把合法字符串 ID 当作缺省整数。Harness 作为通用 stdio 协议入口，应保留 wire ID 的类型和值，才能与不同 server、代理和 SDK 实现互操作。
- 当前生产链路是 `HarnessClient::request()` 将 ID 写入共享 `pending` 表，随后 `spawn_reader()` 读取 stdout 并解码 `WireMessage::Response`。reader 在 `crates/deepagent-harness-sdk/src/client.rs` 第 369 行执行 `response.id.as_i64().unwrap_or_default()`，再以该整数从 `HashMap<i64, oneshot::Sender<_>>` 删除 pending。字符串 `"1"` 因 `as_i64()` 返回 `None` 被映射为 `0`，不会命中真实 ID `1`。
- 新增正式 Windows ignored 探针 `crates/deepagent-harness-sdk/src/client.rs::client::audit_tests::audit_string_response_id_does_not_leave_request_pending`。PowerShell fixture 通过真实 stdin/stdout 管道读取请求，返回 `{"jsonrpc":"2.0","id":"1","result":{}}`，写入 marker 后保持进程 30 秒，排除 EOF 释放造成的假成功。探针调用正式 `HarnessClient::thread_list()`，并在 marker 出现后观察 300ms 内是否完成及 pending 表长度。
- 执行：`cargo fmt --all -- --check`；`cargo test -p deepagent-harness-sdk --lib client::audit_tests::audit_string_response_id_does_not_leave_request_pending --offline -- --ignored --exact --nocapture`。
- 实际结果：格式检查通过；探针为 **0 passed / 1 failed（预期复现）**，断言输出 `completed=false, pending=1`。响应已经被正式 decoder 接收，失败点是 response ID 关联，而非 fixture 未发送、JSON 解码失败或 EOF 清理。

#### 成熟 Agent 对照

- **Codex** 的 `借鉴/codex/codex-rs/app-server-protocol/src/rpc.rs` 明确定义 `RequestId` 为 `String | Integer`，请求、响应和 pending 关联均围绕该类型工作，不把 ID 先投影成单一整数。这是本项最直接的成熟实现对照。
- **DeepSeek Harness** 的 `借鉴/deepseek-harness/packages/sdk/protocol/src/transport.ts` 使用 `Map<JsonRpcId, PendingRequest>` 保存 pending，并在关闭时统一 reject/清理；`JsonRpcId` 同时保留字符串和数字。其设计说明 SDK transport 的职责是维护 wire identity，而不是把协议 ID 转成宿主方便的内部整数。
- **Claude Code** 的 stdio/JSON-RPC transport 同样以请求 envelope 的原始 ID 做关联；本轮不把其内部实现字段当作 JSON-RPC 规范的额外约束，只采用其“类型保真、响应关联失败可诊断”的成熟边界作为参考。

#### 影响、后果与根因修复方向

- 触发后，合法 server 响应会被静默丢弃，调用方看不到成功结果，也收不到协议错误；每个受影响请求会在 `pending` 表中滞留，直到 server 关闭 stdout 才被统一清空。持续运行的 stdio server 会因此积累悬挂请求、泄漏 oneshot sender 和请求上下文，最终表现为 SDK 超时、并发能力下降或后续恢复无法判断哪些请求已经完成。
- 对 `thread/list` 这类只读方法，表面后果是调用卡住；对 `turn/start`、`approval/respond` 或未来工具/配置请求，可能进一步阻塞用户工作流、把已完成的服务端动作误判为未执行，并诱发客户端重试。若动作具有副作用，重试还可能造成重复执行，因此这是协议互操作和生命周期边界的 P1，而不是单纯类型风格问题。
- 本项与 H-204 不同：H-204 是 server 已收到合法请求但没有任何 response 时没有 deadline；H-211 是 server 已返回合法 response，但客户端因 ID 类型丢失无法关联。与 H-101 也不同：H-101 的输入行不可解析而被忽略；H-211 的 JSON 已成功 decode，错误发生在 ID 映射。
- 根因修复应把 `JsonRpcId` 提升为共享协议类型（至少 `String | Integer`，明确拒绝 Null 作为 request ID），并让 `pending`、request 生成器、response decoder、错误/关闭清理全链路使用同一类型和值。禁止 `unwrap_or_default()` 这类把类型错误转成有效-looking ID 的降级。若内部仍需整数序列，只能在 wire ID 与内部 key 之间建立无损、可比较的枚举映射，不能用 `as_i64` 代替。
- 回归应覆盖：数字 ID、字符串数字 ID、普通字符串 ID、负数/零、超出 i64 的 JSON number、null ID、响应 ID 类型不匹配、未知 ID、重复响应、server EOF/关闭及并发请求；每个路径都需验证完成/错误结果与 pending 表在结束后的大小。还要验证 stdout 关闭时所有 `Sender` 收到结构化 transport error，而不是只 `clear()` 丢弃。

本轮只新增正式失败探针和审计文档，未修改生产实现；H-90、H-100、H-101、H-204、H-205、H-207 的既有证据保持独立。生产修复应先统一 JSON-RPC ID 类型和 pending 生命周期，再补 SDK/CLI/stdio 的跨实现互操作回归。

### H-212：Harness SDK 未校验 JSON-RPC response 的协议版本

**级别：中高（P1，协议边界与错误隔离）；触发条件：** server 返回一个带有合法请求 ID 和成功 `result` 的 response，但 `jsonrpc` 字段不是 JSON-RPC 2.0（本次为 `"1.0"`）。当前 SDK 只把该字段反序列化为普通 `String`，随后按 ID 交付给调用方；错误版本不会被拒绝，调用方会得到看似成功的 `RpcResponse`。

#### JSON-RPC 契约、生产路径与正式复现

- Harness stdio 协议声明使用 JSON-RPC 2.0，生产请求由 `request_line()` 固定发送 `jsonrpc: "2.0"`。因此 response 也必须在交付前验证同一协议版本；不能把版本字段当作任意透传元数据。错误版本可能代表旧 server、错误 endpoint、代理串线或非 JSON-RPC 输出，继续执行会掩盖真正的互操作故障。
- 当前生产路径是 `HarnessClient::call_unchecked()` → `spawn_reader()` → `decode_line()` → `RpcResponse`。`crates/deepagent-harness-protocol/src/requests.rs::RpcResponse` 将 `jsonrpc` 定义为 `String`，`crates/deepagent-harness-sdk/src/transport.rs::decode_line()` 不校验值，reader 也没有版本判断；只要 ID 命中 pending，就通过 `oneshot` 交给 `call_unchecked()`。因此 `thread/list()` 对 `jsonrpc: "1.0"` 直接返回成功。
- 新增正式 Windows ignored 探针 `crates/deepagent-harness-sdk/src/client.rs::client::audit_tests::audit_wrong_jsonrpc_version_does_not_complete_request`。PowerShell fixture 通过真实 stdin/stdout 管道返回 `{"jsonrpc":"1.0","id":1,"result":{}}`，写入 marker 后保持进程 30 秒，排除 EOF 清理造成的假失败；探针调用正式 `HarnessClient::thread_list()`，并在 marker 出现后观察 300ms 内是否得到成功结果。
- 执行：`cargo fmt --all -- --check`；`cargo test -p deepagent-harness-sdk --lib client::audit_tests::audit_wrong_jsonrpc_version_does_not_complete_request --offline -- --ignored --exact --nocapture`。
- 实际结果：rustfmt 在排版修正后通过；探针为 **0 passed / 1 failed（预期复现）**，失败输出为 `accepted=true, pending_before_eof=0, observed=Ok(Ok(Ok(RpcResponse { jsonrpc: "1.0", id: Number(1), result: Some(Object({})), error: None })))`。这证明错误版本已经穿过正式 decoder、reader 和 pending 关联边界，而不是 fixture 未发送或 JSON 无法解析。

#### 成熟 Agent 对照

- **Codex** 的 `借鉴/codex/codex-rs/app-server-protocol/src/rpc.rs` 明确定义 `JSONRPC_VERSION: &str = "2.0"`，并把 request/response envelope 建模为受约束的协议类型。Codex 的 app-server 注释说明其内部某些消息不完全依赖 JSON-RPC 字段，因此本轮不声称它对任意外部 response 都做了同样的运行时拒绝；可以确认的成熟边界是协议版本不是任意业务字符串，且由统一 rpc 模块集中表达。
- **DeepSeek Harness** 的 `借鉴/deepseek-harness/packages/sdk/protocol/src/transport.ts` 在所有 request、notification 和 response 写入处固定 `jsonrpc: '2.0'`，测试 fixture 也统一使用 `2.0`。其 transport 没有本项所需的显式入站版本拒绝证据，因此不能把它夸大为已解决；但其协议生成集中化说明版本应归 transport/protocol 层所有，而不是由调用方忽略。
- 两个成熟实现共同避免把协议 envelope 的关键字段交给业务方法自行解释。本系统的问题是“生成端固定 2.0、消费端接受任意版本”，形成单向约束，导致错误 server 仍能伪装成兼容 server。

#### 影响、后果与根因修复方向

- 触发后，旧版/错误版 server、错误管道或中间代理返回的数据会被当成当前 Harness 的成功响应。初始化之后的 `thread/list`、`config/read` 等只读调用会报告成功但实际不代表同一协议；`turn/start`、`approval/respond`、`turn/interrupt` 等状态改变请求则可能在不兼容端点上被误判为已接收，错误定位和重试选择都会被污染。
- 在多宿主或版本滚动部署中，这会把“协议版本不兼容”降级成业务层空结果、缺字段或后续挂起，延迟发现真实配置错误；与 H-211 的 ID 丢失不同，本项响应已正确命中 pending；与 H-100 的 envelope 缺少 `result/error` 不同，本项有完整成功结果但协议版本错误。
- 根因修复应在唯一 `decode_line()`/协议 adapter 边界验证 `jsonrpc == "2.0"`，失败返回带实际版本和 envelope 类型的 `SdkError::Decode` 或新的结构化协议错误；不要在 `initialize()` 或各个业务方法中分别补检查。`RpcRequest`、`RpcResponse`、`RpcNotification` 应共享同一版本常量/typed field，避免生成与消费出现两套规则。
- 回归应覆盖：正确 `2.0` response/notification、`1.0`、缺失字段、空字符串、数字版本、大小写变体、版本错误的 error response，以及错误版本不应移除/完成 pending、EOF 清理仍能释放请求。还需验证 server 发送错误版本后，SDK 不应把 `initialized` 标记为 true，也不应允许后续请求伪装成功。

本轮只新增正式失败探针和审计文档，未修改生产实现；H-100、H-207、H-211 的既有证据保持独立。正式修复应把版本校验、ID 类型、result/error 互斥和 pending 清理收敛到同一个 Harness JSON-RPC transport 边界。

### H-213：Harness SDK 忽略 notification method，任意通知可伪装成 Harness 事件

**级别：高（P1，事件路由、状态污染与控制面可信边界）；触发条件：** stdio server 或同一 stdout 上的中间组件发送一个 JSON-RPC notification，其 `method` 不是 Harness 协议规定的 `harness/event`，但 `params` 恰好符合任一 `HarnessEvent` 结构。当前 SDK reader 忽略 method，只反序列化 params，因此会把该通知广播为正式 Harness 事件。

#### 生产路径与正式复现

- 当前协议 DTO `RpcNotification` 同时保留 `jsonrpc`、`method`、`eventSequence` 和 `params`，说明 method 本来是 wire identity 的一部分；`transport.rs::WireMessage::Notification` 的注释也明确它应是 `harness/event`。但是 `client.rs::spawn_reader()` 在匹配 notification 后直接执行 `serde_json::from_value::<HarnessEvent>(notification.params)`，没有检查 `notification.method == "harness/event"`，随后把事件发送到公开 broadcast channel。
- 这不是 serde untagged 分类造成的偶然现象：notification 已正确解码为 `RpcNotification`，错误发生在 reader 丢弃 method 后的事件投影边界。任何未来控制通知、诊断通知、插件通知或意外 stdout 数据，只要 params 与当前事件 enum 同形，就会进入同一订阅流。
- 新增正式 Windows ignored 探针 `crates/deepagent-harness-sdk/src/client.rs::client::audit_tests::audit_non_harness_notification_method_is_not_emitted_as_event`。PowerShell fixture 通过真实 stdout 输出 `{"jsonrpc":"2.0","method":"other/event","params":{"type":"thread.started","threadId":"t-wrong-method","title":null,"protocolVersion":1}}`，保持进程存活 30 秒；测试订阅正式 `HarnessClient` 使用的 broadcast reader，并观察 300ms。
- 执行：`cargo test -p deepagent-harness-sdk --lib client::audit_tests::audit_non_harness_notification_method_is_not_emitted_as_event --offline -- --ignored --exact --nocapture`。
- 实际结果：探针为 **0 passed / 1 failed（预期复现）**，SDK 收到 `observed=Ok(Ok(ThreadStarted { thread_id: "t-wrong-method", title: None, protocol_version: 1 }))`。fixture 没有使用 `harness/event`，所以该事件只能来自生产 reader 忽略 method 的行为；没有真实 CLI server、模型、工具、用户文件或公网。

#### 成熟 Agent 对照

- **DeepSeek Harness** 的 `packages/sdk/protocol/src/transport.ts` 把 notification handler 明确定义为 `(method, params)`，入站处理调用 `notificationHandler?.(method, objectParams(frame.params))`，不会在 transport 层丢掉 method。其 `HarnessSdkNotificationMap` 再用 `session.event`、`session.status`、`subagent.started`、`subagent.finished` 等方法名索引各自 payload，README 也把 method→type 映射列为协议契约。
- **Codex** 的 app-server protocol 将 `ServerNotification` 建模为按 method 判别的 typed enum/envelope，生产分发以 `ServerNotification::TurnStarted`、`ItemCompleted` 等具体变体工作；调用方不会只看 params 的内部 shape 猜通知类型。这里引用的是其 method-owned typed dispatch，不声称 Codex 的每个通知都与本系统 `harness/event` 同形。
- 成熟实现共同保留“方法名决定 payload schema”这一 JSON-RPC 路由边界。DeepAgent 当前把 method 和 params 解耦，等于允许 payload 的内部 `type` 字段绕过外层协议路由。

#### 影响、后果与根因修复方向

- 触发后，调用方可能收到并处理并不存在的 `thread.started`、`turn.completed`、审批、工具、终态或配置相关事件。即使当前 fixture 只伪造只读的 ThreadStarted，前端/SDK 消费者通常依据事件流更新本地状态、结束 spinner、提示审批、ACK 游标或启动补读；错误事件会造成状态污染、错误 UI 和恢复游标偏移。
- 若未来在同一 stdout 增加独立 notification 方法，当前实现会产生确定性的命名空间冲突：一个新方法的 payload 只要含有与 HarnessEvent 相同的 `type` 字段，就会被旧 SDK 误解释。这会破坏协议可扩展性，使新增通知无法做到向后兼容隔离。
- 本项与 H-205 不同：H-205 是合法 `harness/event` notification 的外层 `eventSequence` 被丢失；H-213 是非法/其他 method 的 notification 被错误接纳。与 H-212 也不同：H-212 检查 response 的 JSON-RPC 版本，本项检查 notification 的 method 路由，即使版本为正确 `2.0` 仍会触发。
- 根因修复应在单一 notification decode/dispatch seam 中先验证 `jsonrpc == "2.0"` 和 method，再按 method 选择 typed payload decoder。当前只支持 `harness/event` 时，其他 method 应明确忽略或生成可观察的 UnknownNotification 诊断，不得进入 `HarnessEvent` channel。若未来支持多方法，应定义版本化 typed notification enum，而不是继续只反序列化 params。
- 回归应覆盖：合法 `harness/event`、未知 method 同形 params、空/缺失/非字符串 method、错误 JSON-RPC 版本、未知 HarnessEvent type、错误 params schema、eventSequence 保留、多个不同方法交错，以及 broadcast lag/重连后不会把未知通知计入 ACK 游标。

本轮只新增正式失败探针和审计文档，未修改生产实现；H-205 与 H-212 的证据保持独立。生产修复应把 method 路由、版本校验、eventSequence 和 typed payload 解码统一在 Harness transport 层，不在 UI/SDK 消费者中各自过滤。

### H-214：Harness SDK 请求返回后才订阅会永久丢失先于 response 到达的合法通知

**级别：高（P1，SDK 事件生命周期与终态可达性）；触发条件：** 调用方先 `await thread_start()` / `await turn_start()`，再调用公开 `HarnessClient::subscribe()`，而 server 在匹配 response 之前已经发送合法 `harness/event`。这不是慢消费者溢出：通知到达时尚无 receiver，SDK 的 broadcast 不提供历史重放，随后创建的 receiver 永远看不到该通知。

#### 生产路径与正式复现

- `HarnessProcess::spawn()` 创建 `tokio::sync::broadcast::channel(256)` 后立即丢弃初始 receiver；`spawn_reader()` 收到合法 notification 后调用 `events.send(event)`，并忽略返回值。Tokio broadcast 只向**当时已经存在**的 receiver 交付；没有 receiver 时发送失败，之后 `subscribe()` 从当前尾部开始，不能回读之前的事件。
- 公开 API 将 `thread_start()`、`turn_start()` 与 `subscribe()` 暴露为互不约束的独立操作，rustdoc 示例按 initialize → thread_start → turn_start 调用，既没有先订阅，也没有 high-level owned turn/stream helper。API 文档没有告知“必须在任何可能产生通知的请求之前订阅”，类型系统也无法阻止错误顺序。
- server 本身确实允许这种次序：`apps/cli/src/app_server.rs::thread_start()` 先调用 emitter 排队 `ThreadStarted`，再返回 response；事件 writer 与 response writer共享 stdout 锁但由不同任务竞争。`turn/start` 虽先写 response 再 spawn turn，快速失败/完成仍可在调用方恢复调度并执行 `subscribe()` 前发出事件。因此通知先于 response 不是伪造的非法 wire 顺序，SDK 必须能处理。
- 新增 Windows ignored 探针 `crates/deepagent-harness-sdk/src/client.rs::client::audit_tests::audit_subscribe_after_request_preserves_pre_response_event`。PowerShell fixture 通过真实 stdin/stdout 处理 initialize 和两次正式 `HarnessClient::thread_start()`；两次都严格按“合法 `harness/event` notification → 同 ID 成功 response”发送。第一组在 request 返回后才订阅，第二组在 request 前先订阅，二者使用相同事件形状和 reader。
- 执行：`cargo test -p deepagent-harness-sdk --lib client::audit_tests::audit_subscribe_after_request_preserves_pre_response_event --offline -- --ignored --exact --nocapture`。
- 实际结果为 **0 passed / 1 failed（预期复现）**：两次 response 均成功，迟订阅组输出 `late_event=None`，预订阅对照输出 `early_event=Some("t-early")`。这排除了 notification 解码失败、method 错误、响应未完成和 broadcast 本身完全失效；失败只由订阅建立时序造成。fixture 不启动真实 CLI server、模型、工具、文件访问或公网。

#### 成熟 Agent 对照

- **DeepSeek Harness** 的 `packages/sdk/client/src/api.ts::HarnessSession.run()` 在调用 `client.prompt()` **之前**先执行 `client.subscribeSessionTree(this.id)`，然后由订阅对象自己的 queue 保存已经到达但尚未 `next()` 消费的通知；`finally` 中统一关闭订阅。它把“订阅就绪 → 发起会产生事件的请求 → 消费到 idle”做成一个高层 owned operation，而不是要求每个调用方自行拼出无竞态顺序。
- 其 `NotificationSubscriptionImpl.push()` 在没有 waiter 时把通知追加到该订阅的 queue，`next()` 优先消费队列；`sdk-client.spec.ts` 还以 HarnessSession.run 的完整通知收集作为契约。这里引用的是 subscribe-before-request 和 per-subscription queue，不声称 DeepSeek Harness 具备本系统 `eventSequence/thread_read` 的全部恢复语义。
- **Codex** 的 app-server 路径在执行会话操作前先附加 conversation listener；H-43 已记录其 `ensure_conversation_listener()` 顺序。本项不把 H-43 重复计数为 SDK 缺陷证据：H-43 发生在本系统桌面 chat/canvas 前端，H-214 发生在独立 Rust Harness SDK 的公开 API 与 broadcast 生命周期，修复位置和调用者不同。

#### 影响、边界与根因修复方向

- 丢失 `thread.started` 会让调用方的实时状态与成功 response 不一致；丢失 `turn.started`、审批或 `turn.completed/failed` 时，等待事件终态的 SDK/UI 可能永久等待、漏掉人工审批或错误保留 running 状态。随后 `thread/read` 可能已经显示完成，但 H-205 又使 SDK 缺少 wire sequence，调用方无法准确知道从何处补读。
- 本次动态样本只直接证明 `ThreadStarted` 丢失，不声称已经在真实 DeepSeek turn 中复现终态或审批丢失；这些后果来自相同无 receiver 丢弃机制和事件类型共享通道。预先订阅是当前可行规避方式，但公开 API/示例没有强制或封装该不变量，因此不能把责任留给每个 SDK 使用者。
- H-01 是 server outbox 已有消费者但队列满时丢通知；H-205 是已收到合法通知后丢 `eventSequence`；H-214 是 SDK notification 到达时尚无订阅者。扩大 channel 容量、保存 sequence 或修 server 背压都不会单独修复“零 receiver”窗口。
- 根因修复应在现有 SDK 上提供 owned `start_turn_stream` / `run_turn`（名称可按本地 API 定稿）：内部先创建 receiver，再发送 request；request 成功后按返回的 thread/turn ID 过滤已排队事件，直到唯一终态，并在 drop/错误/取消时释放订阅。底层 `subscribe()` 可以保留，但文档必须明确其 only-from-now 语义。与 H-205 一并接入带 sequence 的通知信封和 `thread/read` durable 补读，不能建立无界的第二事件库或靠全局 Vec 永久缓存所有通知。
- 回归需覆盖：notification 先于/后于 response、响应与通知同批到达、零 receiver、两个并行 turn、快速成功/失败、审批、broadcast lag、SDK future drop、EOF、重连及 `thread/read` 补读；确保每个 owned operation 只消费自己的 thread/turn 事件，终态一次，且不把较早无关通知错误归属给新请求。

本轮只新增正式失败探针和审计文档，未修改生产 SDK 行为；H-01、H-43、H-205 的既有证据保持独立。

### H-215：Harness 归档状态未进入活动列表与运行接纳，已归档 thread 仍可创建 turn

**级别：中（P2，Harness 控制面与运行接纳）；触发条件：** 客户端对已加载 thread 成功调用 `thread/archive`，随后在同一 app-server 生命周期中执行 `thread/list` 或 `turn/start`。归档 marker 已持久化，但 server 内存中的 thread 与运行入口没有观察该状态。

#### 生产路径与正式复现

- `apps/cli/src/app_server.rs::thread_archive()` 只调用 `ArchiveService::archive_session()` 并返回 `archived=true`。它没有从 `ServerState.threads` 移除 thread，没有给内存状态附加 archived 标记，也没有拒绝或停止该 thread 的活动 turn。
- `ArchiveService` 将归档实现为 document-store 中独立的 visibility index；`archive_session()` 不删除原事件流，`is_archived()` 可稳定读回 marker。这种保留事件的存储设计本身不是问题，缺口在于 Harness 的控制面没有把同一个 marker 纳入接纳判定。
- `thread_list()` 直接枚举 `EventStore::list_sessions()`，未读取 `ArchiveService::archived_ids()`，所以 archive 成功后 thread 仍出现在普通活动列表。`turn_start()` 只检查 `ServerState.threads` 是否仍有条目，随后登记 `TurnState` 并构造 `TurnLaunch`；archive 没有删除该条目，因此新 turn 被继续接纳。
- `thread_resume()` 的静态生产路径同样只从 `EventStore` 查 session/event 并重新插入 `ServerState.threads`，未检查 archive marker。由于本次动态探针复用同一已加载 dispatcher，没有覆盖进程重启后的 resume，故这里只记录静态可达缺口，不把它写成动态结果。
- 新增默认忽略探针 `apps/cli/src/app_server.rs::app_server::tests::audit_archived_thread_does_not_accept_new_turn`，通过正式 dispatcher 依次执行 `thread/start`、`thread/archive`、`thread/list` 与 `turn/start`；并另用 `ArchiveService::is_archived()` 排除归档写入失败。
- 执行：`cargo test -p deepagent-cli app_server::tests::audit_archived_thread_does_not_accept_new_turn --offline -- --ignored --exact --nocapture`。
- 实际结果为 **0 passed / 1 failed（预期复现）**，输出 `archive_success=true durable_archived=true still_listed=true turn_success=true launch=true`。失败首先断言普通列表不应继续返回已归档 thread；同一输出同时证明 `turn/start` 返回成功且创建了 `TurnLaunch`。探针没有 spawn 该 launch、调用模型、执行工具、访问网络或制造外部副作用。

#### 成熟 Agent 对照

- **DeepSeek Harness** 的 `packages/api/session-controller/src/archived-session-gate.ts` 注册 `agent/pre-step` gate；当前 session 或 subagent lineage 中任一 session 已归档时返回 `{ kind: 'reject' }`，不发起模型 request。源码还明确覆盖归档后的迟到 settlement 与 queued follow-up；活动工作由各 owner 停止，gate 负责防止再次进入。`archived-session-gate.host.spec.ts` 验证归档时 reject、gate 卸载后 enter。这里借鉴的是“durable archive 状态进入每步 admission”，不是照搬其插件结构。
- **Codex** 的 `thread_archive_response()` 在落持久化归档前调用 `prepare_thread_for_archive()`；后者通过 `remove_thread_for_client()` 卸载活动 thread，提交并等待 Shutdown，再执行 teardown。其 resume 读取到 `archived_at` 后明确报错，要求先执行 unarchive；协议同时提供 `thread/unarchive`。这使列表可见性、内存加载状态、运行停止与恢复资格由同一生命周期收束。
- DeepSeek 官方 Responses API 不定义 thread archive/list/resume；H-215 是宿主 Harness 的控制面与运行接纳缺陷，不能归因于 provider，也不应以调整模型参数修复。

#### 影响、边界与根因修复方向

- 已动态证明的直接后果是：调用方收到 archive 成功后，普通活动列表仍可见该 thread，且新 `turn/start` 仍被接受并创建可运行 launch。归档因此只是旁路标签，不构成执行边界；依赖归档来隐藏、冻结或阻止自动 follow-up 的客户端会得到错误保证。
- 本次没有实际运行新 turn，故不声称模型已经产生额外费用、工具已经执行副作用，也不声称归档活动 turn 后它必然继续。桌面 UI、真实 stdio 子进程、SDK 和重启后 resume 尚未动态覆盖；这些必须作为后续回归，而不是从本探针外推为事实。
- 当前 Harness 协议暴露 `thread/archive`，却没有对应 `thread/unarchive`；如果修复只是在所有入口拒绝 archived thread，客户端将缺少正式恢复路径。根因修复必须先定义显式 restore/unarchive 契约，再把现有 `ArchiveService` 作为唯一状态源接入 list、resume、start、steer 及其他能触发 step 的入口。
- 对已有活动 turn，归档必须选择并统一一种可验证语义：拒绝归档、先取消并有界等待后归档，或记录 archived-pending 并在清理完成后提交。不能继续“marker 成功但运行 owner 不知情”，也不能在 Harness 内新建第二套取消、run store 或 archive registry。
- 回归应覆盖：空闲/活动 thread 归档、普通列表与显式 archived 列表、同连接及重启后 resume、start/steer、归档与 start 并发、迟到 follow-up、subagent lineage、取消超时/保存失败、幂等 archive、显式 unarchive 后恢复，以及事件/终态只提交一次。

本轮只新增正式失败探针和审计文档，未修改生产归档行为。该项不与 H-92 的 server EOF 活动 turn 丢弃、H-130 的 steer 替代 turn 重复派生或 H-214 的 SDK 订阅竞态合并计数；三者的触发条件、状态所有者和修复边界均不同。

### H-216：Harness 冷分叉返回成功但新 thread 未加载，首个 `turn/start` 被拒绝

**级别：中（P2，thread 生命周期响应原子性）；触发条件：** source thread 已持久化但不在当前 `ServerState.threads` 中，例如 app-server 重启后客户端直接调用 `thread/fork`，没有先对 source 执行 `thread/resume`。分叉写入成功，响应也返回新 ID，但 child 没有成为可运行的已加载 thread。

#### 生产路径与正式复现

- `thread_fork()` 先通过 `EventStore::event_count()` 和 `AppService::fork_session()` 从 durable history 创建 child；这条持久化路径不要求 source 已在内存加载，因此冷 source 可以成功分叉。
- 持久化成功后，代码才执行 `self.threads.get(&params.thread_id).cloned()`。只有 source 已加载时才复制其 `ChatService`、workspace、provider/model、permission profile 与 sandbox backend，并把 child 插入 `self.threads`；source 冷启动未加载时，这个分支静默跳过。
- 随后的成功 response 不反映安装是否发生，无条件返回新 `threadId`。`turn_start()` 则只接受 `self.threads` 中已加载的 ID，所以客户端按自然顺序执行 `thread/fork` → `turn/start` 会得到 `thread is not loaded`，且没有 `TurnLaunch`。
- 新增默认忽略探针 `app_server::tests::audit_cold_thread_fork_is_loaded_for_the_next_turn`：先用文件 SQLite 和第一个正式 `ServerState` 创建 source，销毁该宿主；再以同一数据库构造新的 `ServerState`，确认 source 不在活动 map 后执行正式 fork 与 start。随后显式 `thread/resume` child 并再次 start，作为持久化 child 可用的正向对照。
- 执行：`cargo test -p deepagent-cli app_server::tests::audit_cold_thread_fork_is_loaded_for_the_next_turn --offline -- --ignored --exact --nocapture`。
- 实际结果为 **0 passed / 1 failed（预期复现）**：`fork_success=true fork_loaded=false turn_success=false launch=false`；显式恢复后 `resume_success=true resumed_turn_success=true resumed_launch=true`。这将根因收窄为 fork 成功提交与活动 thread 安装分裂，而不是 AppService 没有生成有效历史。探针只检查 launch，不 spawn turn，不调用模型、工具或网络。

#### 成熟 Agent 对照

- **DeepSeek Harness** 的 `SessionCommandController.fork()` 读取 source prefix 后，先 `await agents.create({ sessionId: childId, seed, ... })`，创建失败就返回错误；只有 agent 已创建并完成可选 workspace attach 后才返回 child ID。客户端 `ClientSessions.fork()` 的文档明确继承 create 的“synchronous-addressability guarantee”，返回后 child 已进入 catalog 且可 retain；UI 随后直接打开 child session。这里借鉴的是创建 readiness 与成功响应同属一次提交，不要求采用相同的 Agent registry。
- **Codex** 的 `thread/fork` 在返回前由 thread manager 创建 `forked_thread`，自动附加 conversation listener，再发送 response；源码明确注释“the new thread is usable as soon as the response arrives”。Codex 还把 `thread/fork followed by turn/start` 作为公开建议流程，说明额外 resume 不是该成功响应的隐含前置步骤。
- DeepSeek 官方 Responses API 不定义 thread/fork；本项属于宿主 Harness 的线程生命周期契约，与 provider 的 reasoning、cache 或工具调用格式无关。

#### 影响、边界与根因修复方向

- 已动态证明的直接后果是：冷 source 的 fork response 为成功并产生 durable child，但客户端无法直接在返回的新 ID 上开始 turn。若 SDK/UI 把成功视为可用并立即提交输入，首个输入会失败；若调用方把失败解释为 fork 未发生而重试，则每次都可能再创建一个 durable child，造成重复分支与会话列表污染。
- 当前规避方式是先显式 resume child；探针对该路径的正向控制已经通过。但协议和成功响应没有声明这一步，热 source 又不需要它，形成依赖进程内加载历史的非确定行为，不能把规避步骤当作稳定契约。
- 本次没有通过真实 stdio/SDK 执行、没有并发请求，也没有断言 child 的 provider/permission/sandbox 是否正确恢复。H-04 已单独证明 resume 丢覆盖配置，因此“自动调用现有 resume”也不是完整根因修复；必须共用一套可验证的有效配置重建边界。
- 根因修复应让 start/resume/fork 的成功语义统一为“返回的 thread 已可接纳后续协议操作”。冷 fork 应从 durable source/result 构造并安装 child `ThreadState`，在 response 前完成；若持久化已成功但内存安装失败，应执行明确补偿或返回带 child identity 的部分失败，不能无声成功，也不能靠客户端猜测并重复 fork。
- 回归应覆盖：热/冷 source、真实进程重启、默认/显式 workspace 与有效配置、fork 后立即 start、显式 resume 幂等、安装失败补偿、并发 fork/start、同请求重试与 child 唯一性、归档 source（与 H-215 协同）以及 source/child 各自独立后续运行。

本轮只新增正式失败探针和审计文档，未修改生产 fork 行为。H-73 关注 inclusive sequence 断点导致的历史内容错误；H-216 的 child history 正向对照可恢复并运行，问题仅是成功响应后的 readiness，因此两项不重复。

### H-217：未知 namespaced JSON-RPC method 被误报为 Invalid params

**级别：中（P2，Harness 协议错误分类）；触发条件：** 客户端发送不存在的、包含命名空间斜杠的 method，例如 `thread/does-not-exist`。当前 dispatcher 在 `HarnessRequest` 反序列化失败后，仅凭原始 method 是否包含 `/` 选择 `ERR_INVALID_PARAMS` 或 `ERR_METHOD_NOT_FOUND`。

#### 生产路径与正式复现

- `apps/cli/src/app_server.rs::ServerState::dispatch()` 先把 `{method, params}` 解析为 `HarnessRequest`。解析失败时，代码以 `request.method.contains('/')` 为条件：含 `/` 返回 `ERR_INVALID_PARAMS`（-32602），不含 `/` 返回 `ERR_METHOD_NOT_FOUND`（-32601）。这把“方法不存在”和“已知方法参数坏”混成了字符串形状判断。
- 该路径不是已知方法参数校验的正常结果：`thread/start` 携带 `{"cwd":42}` 仍返回 -32602；未知无斜杠 method 返回 -32601。只有未知 namespaced method 被分错。
- 新增默认忽略探针 `app_server::tests::audit_unknown_namespaced_method_is_not_reported_as_invalid_params`，通过正式 `ServerState` dispatcher 发送未知 namespaced method、未知 plain method 和已知方法坏参数三组输入。
- 执行：`cargo test -p deepagent-cli app_server::tests::audit_unknown_namespaced_method_is_not_reported_as_invalid_params --offline -- --ignored --exact --nocapture`。
- 实际结果为 **0 passed / 1 failed（预期复现）**，输出 `namespaced=Some(-32602) plain=Some(-32601) known_bad_params=Some(-32602)`；失败只在要求未知 namespaced method 应为 -32601 的断言。没有启动模型、工具、文件写入、子进程或公网请求。

#### 规范与成熟实现对照

- [JSON-RPC 2.0 官方规范](https://www.jsonrpc.org/specification)明确区分：`-32601` 是“Method not found（方法不存在/不可用）”，`-32602` 是“Invalid params（方法参数无效）”；规范示例对不存在的 `foobar` 返回 -32601。方法名是否包含 `/` 不改变这个分类。
- **Codex** 在 `app-server/src/error_code.rs` 提供独立 `method_not_found()`，在 unsupported operation、未实现方法和配置后端不支持的操作路径显式使用它；参数结构错误则使用 `invalid_params()`。它没有通过 method 字符串是否含命名空间字符推断错误类别。
- DeepSeek Harness 的远端 API 通过 typed method/command seam 返回结构化错误；本项不把其所有 gateway 错误码强行等同 JSON-RPC 数字码，只借鉴“方法路由与参数校验分层”的边界。

#### 影响、边界与根因修复方向

- 直接后果是客户端、SDK、代理或测试夹具无法可靠区分“服务端不支持/拼写错误的 namespaced method”和“服务端支持该 method 但参数不合法”。兼容层可能错误地重试参数、把未知扩展当成调用者输入错误，或跳过应有的 capability fallback；日志和故障统计也会把协议演进问题归为参数质量问题。
- 本次没有证明任何真实 SDK 已因该错误码产生错误重试、费用或工具副作用；动态证据只覆盖正式 dispatcher 的 response code。未知 method 的通知路由、批处理与无 id notification 仍需独立回归，不能从本项外推。
- 根因修复应收敛到现有 `HarnessRequest` 注册/dispatch seam：先按 method 注册表判断是否存在；存在时再解码 params，解码失败返回 -32602；不存在统一返回 -32601，并保留稳定 method 名称和可审计数据。不能继续以 `/`、`.` 或其他命名约定作为方法存在性的代理。
- 回归应覆盖：已知 method 合法/坏参数、未知 plain/namespaced method、未来扩展命名、空/大小写变体、`rpc.` 保留前缀、notification 无 id、错误 JSON-RPC 版本、批处理（若支持）以及 SDK 对两类错误的映射。新增协议 method 只能更新一个注册/dispatch seam。

本轮只新增正式失败探针和审计文档，未修改生产协议分派。H-213 关注合法 `HarnessEvent` 被错误 notification method 接纳；H-217 关注 request method 的错误码分类，触发层和修复边界不同。

### H-218：`thread/read` 无界全量物化历史，缺少分页与响应预算

**级别：中高（P1/P2 之间，长历史与多客户端恢复稳定性）；触发条件：** thread 的 durable session history 超过一个正常对话窗口，客户端调用公开 `thread/read` 获取状态或断线恢复。当前请求没有 page size、字节预算或 continuation 字段；`afterSequence` 只能过滤起点，不能限制本次返回的大小。

#### 生产路径与正式复现

- `apps/cli/src/app_server.rs::thread_read()` 先调用 `EventStore::load_session()`，把过滤后的全部 session events 收集为 `Vec<Value>`；随后对该 thread 的每个 run 调用 `RunStore::events_after()`，并继续加载 actions、approvals、artifacts 和 graph，最后在一个 `RpcResponse` 中聚合返回。生产 DTO `ThreadReadRequest` 只有 `afterSequence`、`sessionAfterSequence`、`runAfterSequence` 和 per-run cursors，没有 limit、page size 或 opaque continuation。
- 新增默认忽略探针 `app_server::tests::audit_thread_read_has_bounded_history_page`：使用真实文件 SQLite、`ServerState`、`Session::recover()` 和 `EventStore`，向同一 thread 追加 600 条约 1 KiB 的用户消息，再经正式 dispatcher 调用 `thread/read`。探针不启动模型、工具、子进程或网络。
- 执行：`cargo test -p deepagent-cli app_server::tests::audit_thread_read_has_bounded_history_page --offline -- --ignored --exact --nocapture`。
- 实际结果为 **0 passed / 1 failed（预期复现）**，输出 `events=1201, serialized_bytes=1570066, has_continuation=false`。600 条消息对应的持久化事件连同初始/边界事件全部进入单一 response；响应没有 `nextCursor` 或 `hasMore`。失败断言只要求历史页不超过 500 条且有 continuation，未把 500 条当成本系统既有契约，而是采用成熟参考的安全上限作为审计基线。

#### 成熟 Agent 与 DeepSeek 对照

- **DeepSeek Harness** 的 `packages/api/session-controller/src/history.ts` / `SessionHistoryController.page()` 将历史读取定义为 message-aligned page，按 `maxMessages`、`maxEvents` 等预算向后扩展，并返回 `hasMore`、`nextBeforeSeq` 等继续读取信息；`types.ts` 的普通历史窗口将 500 条消息作为上限。其 `follow()` 负责实时尾部，不用一次全量 hydration 代替分页。这里借鉴的是“冷历史读取有界且可继续”的契约，不把其内部 cursor 编码照搬到本系统。
- **Codex** 的 app-server protocol 将完整 `thread/read(includeTurns=true)` 标记为 paginated threads 的 deprecated full-history hydration，并把读取拆为 `thread/turns/list`、`thread/items/list` 等带 cursor/limit 的接口；`thread/read` 主要返回 thread metadata 与可选的受控视图，而不是无条件把所有 item、工具结果和控制投影拼进一个 JSON。它还在 list 类接口上暴露 limit/cursor，允许客户端按需恢复窗口。
- **DeepSeek 官方 Responses API** 只规定模型请求/响应，不规定 Harness thread history 的读取方式；因此 H-218 是宿主控制面内存与恢复契约问题，不应把调整 DeepSeek cache、reasoning 或 tool 参数当作修复。

#### 影响、边界与根因修复方向

- 已动态证明的直接后果是：历史规模增长会同时放大服务端 `Vec`、serde JSON 和客户端 response 缓冲；在本机 600 条消息样本中，一次 `thread/read` 已构造约 1.57 MiB JSON，且没有 continuation。更长 session、run events、工具结果或 graph 会按同一路径继续放大，并可能让 stdio 单行、SDK 解析、桌面状态更新和断线重试都成为峰值放大器。
- 本探针没有制造 OOM、超时或真实 UI 崩溃，也没有证明任一客户端已经因 1.57 MiB 样本失败；不能把线性放大外推成“必然崩溃”。已确认的是协议没有给调用方一个在有限预算内只取窗口、暂停并续读或明确拒绝的机制。
- `afterSequence` 仍有价值：它可做持久化游标过滤，不能替代分页，因为首次读取、游标落后、跨 run 聚合和单个超大 event 仍可能一次返回无界数据。修复应在现有 EventStore/RunStore 投影边界增加 message-aligned page 与 opaque continuation，并为 session events、run events、控制投影和单条 payload 定义字节/条目预算；不得另建第二套 history store。
- 需要先确定协议兼容策略：旧 `thread/read` 客户端可保留明确的受控上限并返回 continuation/error；新客户端使用分页读取 session 与 run/item 视图。分页 cursor 必须绑定 thread、stream、排序方向和快照/sequence，拒绝跨 thread、重复 cursor、超长 cursor 与无界反复请求；实时通知仍走现有 outbox，不把 follow/live 流混入冷历史页。
- 回归应覆盖：空/小历史、恰好上限、超上限、单个超大 event、多个 run 的独立 cursor、actions/approvals/artifacts/graph 的预算、断线续读、重启后 cursor、历史追加与 cursor 稳定性、重复/篡改 cursor、旧客户端兼容以及 SDK/CLI/Tauri 只渲染当前页的内存上限。

本轮只新增正式失败探针和审计文档，未修改生产 `thread/read` 行为。H-01/H-205 关注通知序号与 ACK/恢复身份，H-73 关注分叉 sequence 边界；它们不覆盖本项“冷历史 response 无界聚合”的内存与分页契约。

### H-219：`tool/list` 是无作用域的静态目录，无法描述当前 run 的实际能力

**级别：中高（P1/P2 之间，能力发现与审计契约）；触发条件：** 同一 Harness 进程中存在多个 workspace、权限 profile、sandbox backend 或动态 MCP/config 代次，客户端调用 `tool/list` 作为下一次 `turn/start` 的能力依据。当前 `ToolListRequest` 没有 thread/run 参数，响应也没有能力快照身份。

#### 生产路径与正式复现

- `apps/cli/src/app_server.rs::tool_list()` 从 `self.threads.values().next()` 任意取一个已加载 thread 的 `ChatService`，否则使用 `base_chat`，然后直接调用 `chat.tool_descriptors()`。HashMap 迭代顺序不是协议稳定的选择，返回值没有 workspace、permission profile、sandbox backend、MCP/config 代次、schema hash、风险或来源。
- `ChatService::tool_descriptors()` 本身只构造基础 registry 并以 `PermissionSet::developer()` 生成静态 descriptors。正式 run 则由 `run_assembler` 结合 workspace、运行配置、MCP/plugin snapshot、tool search 和 policy gate 组装本次工具集；两者不是同一个 run-scoped snapshot。
- 新增默认忽略探针 `app_server::tests::audit_tool_list_is_not_scoped_to_thread_capabilities`：在真实文件 workspace-a/workspace-b 上启动两个 thread，分别指定 `read_only/direct` 与 `full_access/windows_sandbox`，在第一个和第二个 thread 建立后各调用一次正式 `tool/list`。
- 执行：`cargo test -p deepagent-cli app_server::tests::audit_tool_list_is_not_scoped_to_thread_capabilities --offline -- --ignored --exact --nocapture`。
- 实际结果为 **0 passed / 1 failed（预期复现）**，输出 `first_tools=28, second_tools=28, first_has_snapshot=false, second_has_snapshot=false, identical_tools=true`，并记录了两个不同 thread ID。探针没有启动模型、执行工具、创建子进程或访问网络；失败断言只要求不同运行上下文产生可识别的能力快照，不把“工具数组必须不同”当作唯一证据。

#### 成熟 Agent 与架构对照

- **Warp 参考实现**按 execution mode、session type、feature flag 和 Computer Use 开关动态生成 `supported_tools`。能力是一次运行的协商结果，不是全局 registry 的静态列表。项目已有《基于 Warp 证据的底层升级技术方案》3.3 节也明确指出当前 `tool/list` 缺少 run-scoped snapshot、schema hash、风险、并发安全、sandbox capability、来源和版本。
- **Codex** 将 permission profile、sandbox/root、thread/turn 配置和 MCP/plugin 相关能力放在 thread/turn 控制面，并在恢复、fork、turn 启动时保存可解释的配置快照。其 app-server 协议把工具能力和执行上下文拆成可查询、可回放的控制面字段，客户端不需要从一个无上下文工具数组猜当前运行边界。
- **DeepSeek Harness** 的 agent/session controller 将工具注册、MCP 连接代次和 per-turn signal 绑定到 session/agent owner；工具目录变化会更新对应连接/目录状态。DeepSeek Responses API 本身不定义 `tool/list`，本项属于宿主 Harness 的能力协商和执行一致性问题。

#### 影响、边界与根因修复方向

- 已动态证明的后果是：客户端无法从 `tool/list` 判断返回的 28 个工具属于哪个 workspace、哪个权限 profile 或哪个 sandbox；当多个 thread 并存时，服务端甚至通过 HashMap 任意选择一个 ChatService。目录没有 snapshot identity，无法把一次模型请求的工具面与后续 `run_events`、重放或失败诊断关联起来。
- 本探针没有证明模型已经调用了不可用工具，也没有证明某个具体 MCP 工具一定会被错误执行。实际后果取决于 run assembler、policy gate 和动态 MCP/plugin 状态；已确认的是协议缺少让客户端避免这种误判所需的机器可读信息。
- 根因修复应沿现有 `RunAssembler`、`ToolRegistry`、MCP lifecycle 和 `run_events` 边界生成唯一 capability snapshot：至少包含 thread/run、canonical workspace、permission profile/hash、sandbox backend/capabilities、MCP/config 代次、工具 schema hash、风险、来源和是否需要审批。`tool/list` 可保持兼容作为全局目录，但 `turn/start` response 或新增 `capabilities/resolve` 必须返回本次 run 的 snapshot ID 与工具投影。
- 快照需在运行接纳前确定并和实际 registry、policy gate、工具 schema 一致；动态目录变化、权限/沙箱切换、MCP 降级和 fallback 要生成新代次或明确拒绝，不能继续复用静态全局数组。历史重放读取保存的 snapshot，不能用当前 registry 重新解释过去的工具面。
- 回归应覆盖：单 thread、多 workspace、多 permission profile、多 sandbox backend、MCP 初始/刷新/失败、plugin/skill enablement、tool search、fork/resume、重启、fallback、schema hash 变化、snapshot 过期、并发 `tool/list` 与 `turn/start`，以及客户端看到 snapshot 后实际模型 wire 与 ToolRegistry 的一一对应。

本轮只新增正式失败探针和审计文档，未修改 `tool/list` 或运行装配行为。H-07/H-137 关注 MCP 目录分页与动态刷新，H-57 关注目录正文在后续模型请求中的保留；H-219 关注 Harness 控制面没有把实际能力投影为有身份、有版本的 run-scoped snapshot。

### H-220：`thread/list` 无界返回全部 session，缺少列表分页与服务端过滤

**级别：中（P2，控制面规模与客户端刷新稳定性）；触发条件：** app-server 的 durable session 数量超过一个普通列表窗口，客户端周期性调用 `thread/list` 刷新历史、恢复入口或归档视图。当前 `ThreadListRequest` 是空结构，服务端没有 limit、cursor、排序、workspace/provider/archived 过滤或快照语义。

#### 生产路径与正式复现

- `apps/cli/src/app_server.rs::thread_list()` 直接调用 `EventStore::list_sessions()`，将查询返回的全部 session 映射成 JSON，再一次性放入 `threads` 数组。没有服务端 page size，也没有把“是否还有下一页”写入 response。
- 新增默认忽略探针 `app_server::tests::audit_thread_list_has_bounded_page`：在真实文件 workspace 和 SQLite 数据库中，经正式 `ServerState` dispatcher 连续执行 120 次 `thread/start`，再调用正式 `thread/list`；每个 thread 都有真实持久化 session row 和 session 文件。
- 执行：`cargo test -p deepagent-cli app_server::tests::audit_thread_list_has_bounded_page --offline -- --ignored --exact --nocapture`。
- 实际结果为 **0 passed / 1 failed（预期复现）**，输出 `threads=120, serialized_bytes=25009, has_continuation=false`。没有启动模型、工具、子进程或网络；失败断言只要求默认列表有界并提供 continuation，不把某个具体页大小冒充本系统已存在的协议契约。

#### 成熟 Agent 对照

- **Codex** 的 `ThreadListParams` 明确支持 opaque `cursor`、可选 `limit`、排序键/方向、provider/source/archived/section/project/cwd/search 等过滤；实现将默认页大小设为 25、最大 100，并从 thread store 返回 `next_cursor` 与 backwards cursor。列表只承担 thread metadata，详情和 turns/items 另行分页读取。
- **DeepSeek Harness** 的 session controller 将 session catalog、history page 和 live follow 分开；其客户端 session manager 接受列表变化和分页历史，避免把所有冷历史内容当作一次列表响应。这里采用的是控制面和历史读取分离的架构原则；本轮没有把未找到的 DeepSeek session catalog 默认页大小写成事实。
- **DeepSeek Responses API** 不定义宿主 session list；H-220 只属于 Harness 控制面，不应通过模型请求参数或缓存设置修复。

#### 影响、边界与根因修复方向

- 已动态证明的直接后果是：每次列表刷新都会扫描和序列化全部 session metadata；session 数量增加后，服务端 response、SDK 解析和 UI 列表 diff 都按总量增长。25 KB 样本没有造成超时，因此不声称当前规模已导致崩溃；问题是协议没有提供有界刷新机制。
- 归档、workspace、provider 和更新时间筛选目前只能由客户端拿到全量后自行处理；这会增加本地内存和跨进程传输，也使不同客户端的排序/过滤语义不一致。H-215 的归档可见性错误仍独立存在，本项不把它重复计数。
- 修复应在现有 `EventStore::list_sessions()` 查询边界增加 limit/cursor、稳定排序键和可选服务端 filters，响应返回 opaque `nextCursor`；cursor 绑定排序快照/最后一行键、过滤条件和协议版本，拒绝跨查询复用、重复 cursor 和无界循环。默认页大小与最大值需要作为本地协议常量并在 SDK/CLI/Tauri DTO 中同步。
- 列表分页不能替代 H-218 的 thread history 分页，也不能让客户端为恢复而下载所有 run events；列表只返回 metadata，详情、事件和控制投影继续沿现有 `thread/read` 或新的分页视图读取。回归覆盖空列表、恰好边界、超页、插入/归档并发、排序稳定性、cursor 重启、过滤组合和旧客户端兼容。

本轮只新增正式失败探针和审计文档，未修改 `thread/list` 生产行为。H-218 关注单 thread 的历史 response 无界，H-220 关注跨 thread 的 session catalog 无界；两者查询对象和修复接口不同。

### H-221：`config/read` 只返回静态协议 banner，无法读取有效执行配置

**级别：中高（P1/P2 之间，控制面可观测性、恢复与权限/沙箱诊断）；触发条件：** 客户端先用 `thread/start` 指定模型、reasoning effort、permission profile 和 sandbox backend，再调用公开 `config/read` 以确认后续 turn 的实际配置。当前返回值只描述协议/宿主 banner，不包含有效执行配置或配置来源。

#### 生产路径与正式复现

- `apps/cli/src/app_server.rs::config_read()` 通过正式 `ServerState` dispatcher 返回固定的 `protocolVersion`、`provider` 和 `transport`。它没有接收 thread/cwd 参数，也没有从活动 thread 的有效 `ThreadConfig`、Settings overlay、permission profile 或 sandbox backend 读取值。
- 新增默认忽略探针 `app_server::tests::audit_config_read_is_not_a_static_protocol_banner`：使用真实文件 workspace、内存 SQLite、`ServerState::new_for_test_with_database()` 和正式初始化，先调用 `thread/start`，请求明确指定 `deepseek-v4-pro`、`reasoningEffort=high`、`permissionProfile=read_only`、`sandboxBackend=windows_sandbox`，随后调用正式 `config/read`。
- 执行：`cargo test -p deepagent-cli app_server::tests::audit_config_read_is_not_a_static_protocol_banner --offline -- --ignored --exact --nocapture`。
- 实际结果：**0 passed / 1 failed（预期复现）**。输出为 `keys=Some(["protocolVersion", "provider", "transport"]), provider=Some(String("deepseek-official")), model=None, reasoning=None, permission=None, sandbox=None`。断言只要求有效配置字段可被读取，没有把 thread override 必须直接混入全局 response 当作既有协议契约。探针未启动模型、工具、子进程或网络。

#### 成熟 Agent 与 DeepSeek 对照

- **Codex** 的 app-server `ConfigReadParams` 支持 `includeLayers` 和可选 `cwd`，配置读取路径返回实际解析后的配置/来源层；`thread_processor` 的恢复路径也会校核请求覆盖与实际 `ThreadConfigSnapshot`，并把有效配置带回控制面。可借鉴的是“读取有效配置必须与解析/执行使用同一 snapshot”，不是照搬 Codex 的字段名或权限语义。
- **DeepSeek Harness** 的 Responses/session controller 不定义宿主 `config/read`；其 provider profile、model adapter 和 agent/session owner 仍把模型参数、工具/信号与连接上下文作为运行时状态。因而 H-221 属于本系统 Harness 控制面可观测性缺口，不是 DeepSeek API 要求 `config/read` 的证据。
- **DeepSeek Responses API** 的 `model`、`reasoning`、工具和其他 request 参数仍应由 provider adapter 按官方 schema 投影；`config/read` 应报告宿主解析出的有效值/来源，不能把宿主字段伪装成 Responses 请求字段，也不能用它替代最终 wire 校验。

#### 影响、边界与根因修复方向

- 已动态证明的直接后果是：客户端收到成功的 `config/read` 后，无法知道下一次 turn 是否会使用 `deepseek-v4-pro`、`high`、`read_only` 或 `windows_sandbox`。恢复、SDK/CLI 展示、审计日志和权限/沙箱故障诊断只能依赖本地请求记忆或猜测；当 settings、thread override、项目来源或宿主 backend 变化时，控制面没有机器可读的生效值确认。
- 本探针没有证明 `thread/start` 的指定值已经在执行器中被错误应用，也没有证明当前请求一定会发往错误模型或越过权限。它证明的是读取接口无论请求配置为何都返回同一静态形状；H-03/H-04/H-27/H-28/H-54 分别覆盖配置实际应用、恢复和信任边界，不能用 H-221 重复计数。
- 根因修复应复用现有 Settings、ThreadConfig、permission/sandbox resolver 和 run admission，在 `config/read` 增加明确的 scope（全局、thread 或 cwd）以及有效配置 snapshot/version、来源层和受限的可序列化字段；不得在 app-server 再实现一套配置合并。若保留无参数兼容调用，应至少返回全局有效默认与“需要 thread scope”的可解释结果，而不是静态 banner 冒充完整配置。
- `config/read` 与 `turn/start` 必须读取同一解析 snapshot，或返回 snapshot ID 并让 turn admission 校验其仍有效；敏感 secret、完整 prompt 和机器路径应脱敏。变更、恢复、fork、归档、权限/沙箱降级和 provider fallback 都应留下来源/版本及可审计诊断。DeepSeek `model`、reasoning、tool choice 和 usage/cache 信息仍在 provider seam 保持原生结构，不应被 UI/CLI DTO 压平。
- 回归应覆盖：全局与 thread scope、cwd/project layer、显式/默认模型、reasoning effort、permission profile、sandbox backend、MCP/plugin 代次、恢复/fork、未信任项目配置、fallback、重启、并发 config/read 与 turn/start、旧客户端字段兼容，以及实际 provider wire 与返回 snapshot 的一致性。应同时验证不泄露 API key、token、完整 instructions 或敏感 host path。

本轮只新增正式失败探针和审计文档，未修改 `config/read` 生产行为。H-217 关注未知方法错误分类，H-218/H-220 关注无界历史/列表响应；H-221 关注有效执行配置没有进入控制面可观测性，触发对象和修复边界不同。

### H-222：`thread/start` 的 `reasoningEffort` 被静默丢弃，后续 turn 回退到默认思考配置

**级别：中高（P1/P2 之间，DeepSeek 思考模式控制与成本/延迟契约）；触发条件：** Harness 客户端在创建 thread 时携带 `reasoningEffort=high`（或 `none`、`low`、`max`），随后调用不带 reasoning 覆盖的 `turn/start`。当前线程请求类型没有该字段，Serde 默认忽略未知 JSON；`ThreadState` 也不保存 reasoning effort，导致 turn 的 `HarnessRunOverrides` 为 `None`，后续解析使用 Settings/模型默认值。

#### 生产路径与正式复现

- `crates/deepagent-harness-protocol/src/requests.rs::ThreadStartRequest` 定义了 `cwd`、`provider`、`model`、`permissionProfile`、`sandboxBackend`，没有 `reasoningEffort`；该结构没有 `deny_unknown_fields`，所以请求中的未知 `reasoningEffort` 不报错。
- `apps/cli/src/app_server.rs::thread_start()` 将已解析字段保存进 `ThreadState`，但没有 reasoning 槽位；`turn_start()` 只把 `TurnStartRequest.reasoning_effort` 写入 `HarnessRunOverrides`，无 turn 覆盖时只能得到 `None`。这条路径绕过了真实模型，直接暴露“配置是否进入 launch”的事实。
- 新增默认忽略探针 `app_server::tests::audit_thread_start_reasoning_effort_is_not_silently_dropped`：使用真实文件 workspace、内存 SQLite、`ServerState` 和正式 dispatcher；发送 `thread/start` 的 `reasoningEffort=high`，取得返回的 `threadId`，再发送无覆盖 `turn/start`，检查正式 `TurnLaunch.overrides.reasoning_effort`。
- 执行：`cargo test -p deepagent-cli app_server::tests::audit_thread_start_reasoning_effort_is_not_silently_dropped --offline -- --ignored --exact --nocapture`。
- 实际结果：**0 passed / 1 failed（预期复现）**。输出 `thread/start reasoning audit: response_success=true, reasoning_effort=None`。`turn/start` response 仍报告成功并生成 launch；没有启动模型、工具、子进程或网络。该探针证明的是字段静默丢失，不声称真实供应商已经收到某个错误 effort。

#### 官方 DeepSeek、成熟 Agent 对照

- **DeepSeek Responses 参数文档**定义 `reasoning.effort` 为 `none`、`low`、`high`、`max`，并明确 `minimal` 映射到 `low`、`medium`/`xhigh` 映射到 `high`；不传时使用模型默认思考行为。[官方 Create Response 参数页](https://api-docs.deepseek.com/zh-cn/api/create-response) 的 `reasoning` 段同时说明该字段控制思考模式和强度。因此 thread 级选择不能在入口被静默丢弃后假装成功。
- **DeepSeek Harness** 的 ACP `session/set_config_option` 将 model/reasoning 作为会话配置并返回完整结果；其 README 说明 prompt admission 会先 snapshot 选项，再在该 turn 的每个模型步骤固定同一 provider/model/reasoning effort，并让并发配置变更从下一轮生效。可借鉴的是“会话配置有身份、prompt 前快照并贯穿整轮”，不是照搬 ACP 字段。
- **Codex** 的 app-server protocol 将 reasoning effort 作为 turn/model configuration 与事件字段（如 `reasoning_effort` / `model_reasoning_effort`），模型目录也声明支持档位和默认档位；事件映射保留实际 effort，便于客户端和回放确认。这里采用的是同一有效配置快照原则，不声称 Codex 的具体模型档位等于 DeepSeek 的四个值。

#### 影响、边界与根因修复方向

- 已动态证明的后果是：客户端可收到“thread 创建成功、turn 启动成功”，但模型实际会使用全局设置或 provider default，而不是 thread 请求的思考强度。对 DeepSeek 而言，这会改变是否启用思考、推理 token 数、延迟、输出预算和费用；`none` 可能意外启用默认思考，`max` 可能退回较低档位。依赖 thread 配置做可重复评测、重试、审计或成本比较的调用方会得到不可解释的结果。
- 本探针没有启动 `ModelAgent`，所以没有测量真实 wire 的 `reasoning.effort`、回答质量或 token usage；不能写成“每个真实请求必然使用 high 的反值”。H-54 是桌面深度覆盖持久化参数，H-153/H-155 是 Responses effort 值/别名输入契约，H-221 是 `config/read` 读取缺失；H-222 的独立事实是 Harness thread DTO 到 turn launch 的字段丢失。
- 根因修复应扩展现有 typed 协议和唯一配置边界：`ThreadStartRequest` 明确加入 `reasoningEffort`（或在版本化协议中明确不支持并返回 invalid params，不能静默忽略），`ThreadState` 保存已解析/原始值及来源，`thread/start` 成功响应返回脱敏的有效配置 snapshot；`turn/start` 无覆盖继承该 snapshot，有覆盖按明确优先级合并，并在 run admission 前调用现有 provider/model capability validator。不要在 app-server 另写一套 effort 解析或绕开 `RunAssembler`。
- 每次 turn 应把最终 effort/snapshot ID 与 run_events、provider request metadata 和 usage/cost 关联；工具循环、截断恢复、steer/fork/resume 必须继续使用同一 turn snapshot。并发改变 thread 默认值只影响下一次尚未接纳的 turn，不能改变已接纳 turn；显式 `none` 必须与“未提供”区分。
- 回归应覆盖四个规范值、四个兼容别名、非法/空值、模型能力不支持、thread 默认与 turn 覆盖优先级、resume/fork/restart、并发 config change、工具循环/重试/截断、Responses 与 Chat adapter 分离，以及 recording transport 对最终 `reasoning.effort` 的严格断言。真实 DeepSeek A/B 只用于确认 provider 行为，不替代本地字段贯通测试。

本轮只新增正式失败探针和审计文档，未修改 Harness 生产行为。H-54/H-155/H-221 已分别记录其他层级；H-222 关注 `thread/start` 这一协议入口是否把 DeepSeek reasoning 选择传入后续 turn。

### H-223：刷新相同模型目录会覆盖仍有效的用户选型，下一次 Responses 请求静默换模型

**级别：中（P2，模型选择与能力/成本预期一致性）；触发条件：** 用户用正式 `set_model(Chat, Pro)` 选择非自动默认模型，随后模型目录刷新成功，即使返回的目录完全相同、选中的 Pro 仍存在。无需多宿主、并发、网络故障、未知模型或恢复/steer。

#### 生产路径与动态复现

- `crates/deepagent-app-core/src/settings.rs::set_model()` 校验 ID 在已发现目录中后，修改对应角色并持久化。`refresh_models()` 读取现有 Key 后直接调用 `initialize()`；后者经 `ModelDiscovery::discover()` / `ModelCatalog::auto_select()` 重建整个 catalog，把它原样放入新 `AppSettings.catalog`，没有从 prior 保留角色选择。其他许多设置显式保留 prior，模型选型不在其中。
- `apps/desktop/src/App.tsx` 的启动 effect 在 `getSettings()` 返回 configured 后调用 `refreshModels()`；Tauri `refresh_models` command 转交同一共享服务。因此正常启动存在自动触发路径；本轮没有启动真实 WebView，不能声称实际桌面重启已被动态验收。
- 新增默认忽略探针 `model_selection::audit_catalog_refresh_preserves_selected_model_on_provider_wire`，复用既有 `SelectionModel` 记录器。两组均用临时文件 SQLite、MemorySecretStore 占位凭据和同一份两模型 discovery JSON；默认组保持 Flash，实验组调用正式 `set_model(Chat, deepseek-v4-pro)`。每组刷新前后各执行一次完整 ChatService→RunAssembler→Kernel→ModelAgent→ModelClient→Responses serializer，不传逐次 model override；最后释放服务并用新数据库连接读取持久化 catalog。只在 transport 层用固定离线回答，不直接修改 catalog、不伪造运行终态。
- 2026-10-24 执行：`cargo test -p deepagent-app-core --test kernel_v2_e2e model_selection::audit_catalog_refresh_preserves_selected_model_on_provider_wire --offline -- --ignored --exact --nocapture`。实测 **0 passed / 1 failed（预期暴露缺陷），约 0.69s**：

| 组别 | 刷新前 wire model | 刷新后 wire model | 新连接读取的 chat_model |
|---|---|---|---|
| 未手动选型对照 | deepseek-v4-flash | deepseek-v4-flash | deepseek-v4-flash |
| 显式 Pro | deepseek-v4-pro | deepseek-v4-flash | deepseek-v4-flash |

- 四次运行分别验证唯一 `run_terminal.kind=succeeded`；刷新结果明确仍包含原选定模型。失败断言是“目录刷新不替换仍有效的选型”，不是因为模型请求失败。fixture 中 Flash 的 ID 沿用仓库既有目录样本，不用它宣称当前公网模型 ID 一定相同。

#### 官方与成熟实现对照

- **DeepSeek 官方** [Responses 参数页](https://api-docs.deepseek.com/zh-cn/api/create-response)将 `model` 定义为显式必填请求参数；[Responses 指南](https://api-docs.deepseek.com/zh-cn/guides/responses_api)又分别限制图片与原生搜索的支持模型。这说明静默换模型不只是下拉框展示差异，也可能改变可用能力；官方没有规定宿主的目录刷新应重置用户偏好。此问题属于本系统配置生命周期，不是 provider 错误或 KV cache 实现缺陷。
- **Codex** `借鉴/codex/codex-rs/models-manager/src/manager.rs` 将 `refresh_available_models()` / `apply_remote_models()` 的远端目录更新与 `get_default_model(model, policy, ...)` 的用户选择分离；后者有显式 model 时保留，`StaticModelsManager` 即使允许 fallback 也优先保留仍可用的 requested model。`manager_tests.rs::static_manager_preserves_supported_requested_model_when_fallback_is_allowed` 为此提供针对性断言。这里只核对源码/测试，未运行参考项目，也不把 Codex 的未知模型策略移植为 DeepSeek 契约。
- **DeepSeek Harness** `借鉴/deepseek-harness/packages/acp/acp/src/model-control.ts::AcpModelControl` 把 `selected` 与列出的 `choices` 分开；`state()/options()` 重列目录时使用当前 selection，`set()` 才应用客户端选型。`session.ts::prompt()` 捕获 snapshot 并 pin 到整轮。它不是本系统全局 Settings 的现成实现，可借鉴的是“目录信息、用户意图和本轮快照有不同所有者”，不能因为重列可选项就重新自动选择。

#### 影响、边界与收敛方向

- 已证明的是：用户成功选定 Pro、刷新成功、随后成功运行的真实序列化请求却变成 Flash，且改变已经落盘；单进程正常顺序即可触发。用户可能失去选择的模型能力、质量/延迟/费用预期，后台维护或依赖角色默认值的下次运行也可能受影响；本轮没有测量真实回答质量、费用、cache hit 或其他角色调用，所以这些后果只能作为条件性风险，不写成实测损失。
- 本探针动态覆盖 Chat 角色、相同目录、无逐次覆盖的独立新 session；未验证 Reasoner 角色、真实桌面、下架/改名、真实 DeepSeek API 或已接纳运行中途被改变。H-87 的跨宿主 stale-write、H-88 的旧凭据恢复、H-89 的未知显式模型回退、H-222 的 thread reasoning 丢失具有不同触发边界，本项不重复计数。
- 根因修复方向（尚未实施，本地设计）：保留现有 SettingsService/ModelCatalog 与 provider seam，分离首次初始化的自动默认选择、只读目录发现/字段级刷新、用户显式选择。刷新时在原子提交边界保留仍存在的角色 ID；消失/改名时按明确策略拒绝、标记 unavailable 或经用户确认迁移，记录 requested/effective/source/reason，不能无声 auto-select。并发选择与刷新需复用 H-87 的版本/条件更新，不以保存网络等待前的旧偏好替代；目录刷新也不应重新保存凭据（H-88）。
- 回归应覆盖默认与两角色手动选择、同目录/重排/新增/删除/别名、初始化与显式重新初始化的不同意图、自动启动刷新、刷新失败保留、refresh/set_model 竞争、多宿主、重启读回、逐次覆盖优先级，以及实际 wire/事件/能力元数据与生效选择一致。先定义失效选择的可观测策略，不为每个模型名增加临时特判。

本轮只新增离线正式失败探针与审计记录，未修复生产选型行为，没有协议、权限、沙箱或数据迁移变更。

本轮验证：`cargo fmt --all -- --check` 与 `git diff --check` 通过（首次格式检查提示新增探针两处布局，调整后重跑通过）；上述 H-223 显式探针按预期失败。既有 model-selection 两项正常对照单独执行均为 1 passed；`cargo test -p deepagent-app-core --lib --offline` 为 **1091 passed / 50 ignored**，`cargo test -p deepagent-app-core --test kernel_v2_e2e --offline` 为 **35 passed / 37 ignored**。默认忽略的已知缺陷不因常规测试为绿而关闭。本轮没有修改生产行为或前端契约，未执行全 workspace/clippy/桌面构建、真实 UI 或公网模型验收。

### H-224：日/月费用使用错误时间桶，真实预算接纳漏计本期历史费用后继续请求模型

**级别：中高（P1/P2 之间，本地费用上限的决策完整性）；触发条件：** 已附加 CostService 的入口设置有限 daily/monthly budget，并且既有费用位于真实日历边界和错误取模边界之间。日统计声明从本地午夜开始，月统计声明本月；月缺陷在 UTC 时区也存在，不以本地时区歧义为成立前提。默认无限预算不会拦截，也不能代表有限预算正确。

#### 生产路径与动态复现

- `crates/deepagent-app-core/src/cost_service.rs::summary()` 对 Unix 毫秒做 `now - now % 86_400_000` 和 `now - now % (86_400_000 * 30)`，然后交给 `CostStore::total_since()`。前者是 UTC 日界，后者是从 epoch 起固定 30 天桶，既不是自然月，也不是滚动最近 30 天。`CostSummary.today_cost` 注释明确写的是 local midnight。`check_budget()` 直接使用上述 today/month；`run_assembler.rs` 在普通聊天接纳、模型 dispatch 前调用它，没有重新计算周期。
- 正式桌面 `get_cost_summary`/`set_budget` command 与 `apps/desktop/src/api.ts`/`types.ts` 暴露相同日/月契约；桌面组装对 ChatService 调用 `with_cost(cost.clone())`。本轮未运行 WebView，也没有证明存在实际预算设置 UI；动态验证从这两个 command 共用的服务边界进入，不能写成已经点击过桌面按钮。
- 新增 `tests/kernel_v2_e2e/cost_calendar.rs`：临时文件 SQLite、正式 `create_session()`、`CostStore::insert()` 注入合成历史账单，正式 `set_budget()`/`summary()`/`check_budget()`，随后完整执行 ChatService→RunAssembler→Kernel→ModelAgent→ModelClient→Responses serializer。只替换 discovery 和模型 transport；占位凭据、固定短回答、无工具副作用，不修改 SystemClock、主机时区或用户数据库。
- 每个边界组先放一笔 1 元在两个边界之后作为低于限额的成功对照，再用独立数据库增加一笔 12 元于两个边界之间。全部账单时间不晚于实际 now。每笔 token=1 与人民币值均为明确标识的合成 fixture，**不是模型真实 tokenizer/价格计算结果，也不是实际供应商扣款**。测试隔离周期聚合与接纳，不把 fixture 金额当作当前 DeepSeek 单价验证。
- 2026-10-24 18:41:25 +08:00 执行 `cargo test -p deepagent-app-core --test kernel_v2_e2e cost_calendar:: --offline -- --include-ignored --nocapture`，**1 passed / 2 failed（预期暴露缺陷），约 0.48s**；两个失败均发生在最终契约断言，不是编译、fixture 或网络错误：

| 实验组 | 应用的日历起点（北京时间） | 实际取模起点（北京时间） | 12 元历史账单时间（北京时间） | 正式 session/总成本 | 期内应计 / 实计 | 限额 / 实际接纳 |
|---|---|---|---|---|---|---|
| 日预算 | 10-24 00:00 | 10-24 08:00 | 10-24 04:00 | 13 / 13 元 | 13 / 1 元 | 5 元；放行、1 次模型调用、唯一 succeeded |
| 月预算 | 10-01 00:00 | 10-04 08:00 | 10-02 16:00 | 13 / 13 元 | 13 / 1 元 | 5 元；放行、1 次模型调用、唯一 succeeded |

- 正常对照 `current_cost_blocks_daily_and_monthly_model_dispatch` 为 **1 passed**：独立日/月组各把 12 元写在当前时刻，真实运行分别返回 `daily budget exhausted` / `monthly budget exhausted`，模型调用均为 0。两个边界组各自只有 1 元的低费用对照也均唯一 succeeded。因此不是“预算代码完全没接线”，而是时间范围错误使有效拦截条件看不到已存在的费用。session_cost/total_cost=13 同时证明历史行已进入真实账本，没有插入失败或全局漏存。
- 两个缺陷探针默认 `#[ignore]`，须显式运行；探针根据当前时钟选择边界间历史时间。若复跑环境恰逢日历边界与取模边界重合（例如 UTC 日界），会明确报 precondition，而不是声称当时复现日界缺陷。当前宿主为 UTC+08 且无本月 DST 切换；使用当前 offset 的探针不是所有 IANA 时区/DST 的正确性证明。全部原始观察立即记入本节，后续常规验证另补。

#### 官方与成熟实现对照

- **DeepSeek 官方** [价格与扣费规则](https://api-docs.deepseek.com/zh-cn/quick_start/pricing/)描述按用量扣费，且当前价格存在北京时间高峰/空闲时段。它没有定义本系统的本地 daily/monthly budget，也没有允许用 epoch 固定 30 天桶替代月度预算。本项与真实报价是否及时更新、usage 是否漏计独立；即使每行价格完全正确，聚合错周期仍会导致错误决策。不能从价格页的北京时间推导所有用户本地预算都必须采用北京时间。
- **Grok Build** `借鉴/grok-build/crates/codegen/xai-grok-shell/src/extensions/billing.rs` 的 `UsagePeriod` 显式保留 period type 与 RFC3339 start/end；`BillingConfig.current_period` 优先于旧 `billing_period_start/end`，历史 `BillingCycle` 也明确 year/month。其数据来自远端 billing，不是同类本地成本库；可借鉴的是周期有明确身份、起止与来源，而不是客户端用 30 天取模猜账期。本轮未运行参考项目或访问其 billing 服务。
- **Codex** `借鉴/codex/codex-rs/protocol/src/protocol.rs::RateLimitWindow` 区分 rolling window duration 和显式 `resets_at`；TUI `status/tests.rs::status_snapshot_includes_monthly_limit` 的月限额样本也带 reset 时间。它展示供应商 limit，不等于本系统自建人民币预算；本轮没有找到可直接移植的同类本地日/月费用聚合，不能写成“Codex 已有本项完全相同实现”。
- **Claude Code** `restored-src/src/utils/stats.ts` 将 timestamp 解析为 Date 后通过 `statsCache.ts::toDateString()` 做日历日期 key。追到 helper 后确认它用 `toISOString()`，即 UTC 日期，而不是本地午夜；`calculateStreaks()` 虽有本地 `setHours(0,0,0,0)`，不能据此夸大为本地费用预算范例。这里仅借鉴按日历日期建模及日期过滤，时区仍需本系统明确设计。
- 现有依赖已锁定 `time 0.3.47`，官方 [Date](https://docs.rs/time/0.3.47/time/struct.Date.html#method.replace_day) 与 [PrimitiveDateTime](https://docs.rs/time/0.3.47/time/struct.PrimitiveDateTime.html#method.assume_offset) 提供日历月初、午夜与 offset 映射；不需要新增依赖或手写“每月 30 天”规则。跨 DST 的完整时区选择仍需另行定义，不把固定 offset 当时区数据库。

#### 影响、边界与收敛方向

- 已动态确认：本地周期内合成费用已经达到 13 元、预算为 5 元时，服务仍判断可运行，并完成一次真实序列化的离线模型请求；没有产生实际扣费，不能宣称已造成 8 元或其他具体财务损失。真实 provider 场景下同一错误准入可继续消耗费用；放大幅度取决于后续运行、定价和费用分布，未量化。
- 固定 30 天桶的重置会随日期漂移，可能在月中漏掉本月前半段，或在月初纳入上月费用。非 UTC 本地日界也可能漏计本日凌晨或带入昨日。**本轮只动态复现漏计后放行**，其他日期的误拒绝为生产公式推导，未冒充已跨时钟运行；没有为测试改系统时间。
- 范围限于实际附加 CostService 的普通聊天接纳及公开 summary/check_budget。未动态覆盖 CLI 是否挂载成本服务、工作流/后台附属调用、并发预算保留、运行中硬停、未知价格/NULL cost、预算重启持久化、真实供应商账期或桌面 UI。H-14/H-63/H-163 是 usage/归属漏计，本项在历史成本行完整存在的前提下仍独立成立；不能重复登记那些问题。
- 根因收敛（尚未实施，**本地设计**）：在现有 CostService 建立唯一费用周期解析边界，明确预算的 timezone、自然日/月或显式账期，计算 start/end，并让 summary 与 admission 消费同一周期快照；供应商账期/rolling limit 与用户本地预算分别标识，不改写历史费用时间，不另造第二成本库。需要可注入 clock 的确定性边界测试，不把 host SystemClock 散落到多个汇总函数，且不能把本项修复扩张为并发预算保证。
- 回归需覆盖日/月界前后 1ms、UTC/正负偏移、跨年、28/29/30/31 日、DST gap/fold、主机时区变化策略、期内/期外历史行、等于限额、无预算、未知费用、真实接纳零 dispatch 对照与唯一终态。历史总额/session 用量保持不变；周期与时区应可观测，避免 UI 误称“本月”。

本轮仅增加审计探针和证据文档，尚未修复生产预算行为；不涉及协议、权限、沙箱或数据迁移。

本轮验证：`cargo fmt --all -- --check`、`git diff --check` 通过（首次格式检查提示新增探针布局，格式化后重跑通过）；H-224 显式探针为上述 **1 passed / 2 failed**。`cargo test -p deepagent-app-core --lib cost_service::tests:: --offline`：**6 passed**；`cargo test -p deepagent-persistence cost_store::tests:: --offline`：**4 passed**；`cargo test -p deepagent-app-core --lib --offline`：**1091 passed / 50 ignored**；`cargo test -p deepagent-app-core --test kernel_v2_e2e --offline`：**36 passed / 39 ignored**。默认忽略的已知缺陷不因常规测试为绿而关闭。未跑全 workspace、Clippy、桌面 build/真实 UI、公网模型或真实扣费验收；本轮没有生产行为/跨层契约改动，不据此宣称全系统已通过。

### H-225：首次登录只持久化报价，未激活既有 CostService，正常模型有用量却无费用且预算继续放行

**级别：中高（P1/P2 之间，首次配置的计价与本地预算完整性）；触发条件：** 全新未配置宿主已构造 CostService，启动的一次性价格刷新在用户完成登录前结束，随后 `initialize_project` 成功，但没有再执行会同步 CostService 的显式模型刷新。无需未知模型、别名、异常 provider usage、网络断连或日历边界；在向导等待用户输入期间结束启动刷新即满足前置顺序。不是声称每一种并发登录时序都必然触发。

#### 生产路径与动态复现

- `apps/desktop/src-tauri/src/lib.rs` 启动时构造共享 `CostService::new()`，仅在 `settings.view()` 已有值时加载报价，并 spawn 一次 `refresh_pricing()`→`cost.set_pricing()`；无 settings 时 `refresh_pricing()` 返回 `settings not initialized`，`if let Ok` 没有后续重试/激活订阅。`initialize_project` 随后只调用 `settings.initialize()`，再初始化 sandboxie，最后返回 view；没有更新 state.cost。
- `SettingsService::initialize()` 已调用 `refresh_pricing_inner()` 并写入包含 Flash 价的持久化 settings，所以此时不是 settings 中缺少模型价格；缺的是持久化配置到已存活计价 owner 的生效连接。这里的 refresh 使用仓库现有内置闲时快照，**没有从官网动态取回报价**；本项只检验这份已存在报价的激活，不冒充验证了报价获取/更新的真实性。正常 `refresh_models` command 则有显式后台 `refresh_pricing()`→`set_pricing()`，因此是另一个行为不同的激活入口。
- 前端 `OnboardingWizard.tsx::connect` 调用 `initializeProject()`；成功完成向导后 `App.tsx` 的 `onComplete` 只关闭向导、`refreshSessions()`。App 的 configured→`refreshModels()` effect 依赖为 `[]`，未配置启动时走 false 分支，不因完成登录自动重跑。上述为生产源码核查，未进行真实 WebView 点击或时间竞争测试。
- 新增 `tests/kernel_v2_e2e/pricing_activation.rs`。文件 SQLite、MemorySecretStore 占位凭据、正式 SettingsService/CostService/ChatService；先按宿主顺序执行 view/load 与启动 refresh，明确等待“settings 尚未初始化”返回，再 initialize。测试不手工清空价格、不篡改已配置状态、不直接写成本行。discovery 夹具只列官方 canonical `deepseek-flash`，每次正式 Responses wire 保持该模型。
- 离线 transport 返回合法 `response.completed.usage`：input=1000、output=100、total=1100、cached=800；生产 parser/RunUsage/finalizer 自行推导 cache miss=200 并写真实 costs 表。测试确认 settings 中价格为 hit=0.02、miss=1、output=4 元/百万 tokens，对应本次闲时样本应计 **0.000616 元**。此为合成用量按快照计算，不是真实 tokenization、账单或供应商扣款；当前测试时刻为周六空闲时段，不用它验证高峰切换。
- 三组分别设置 daily_limit=0.0005 元并尝试两轮完整聊天；金额刻意很小，用于无真实消耗的确定性边界。首轮放行符合“运行前检查”的现有契约，检验点是其消费是否计入下一轮。2026-10-24 执行 `cargo test -p deepagent-app-core --test kernel_v2_e2e pricing_activation:: --offline -- --include-ignored --nocapture`：**1 passed / 1 failed（预期暴露缺陷），约 3.15s**：

| 场景 | 首轮费用行 / summary | 首轮 input/output/hit/miss | CostRecorded | 第二轮 / 实际 wire 模型 |
|---|---|---|---|---|
| initialize 在宿主成本服务构造前已完成 | 0.000616 / 0.000616 | 1000/100/800/200 | Some(0.000616) | daily budget exhausted；仅 1 次 Flash |
| 全新宿主先结束启动刷新，再首次登录 | NULL / 0 | 1000/100/800/200 | None；第二轮亦 None | 无错误、成功；共 2 次 Flash |
| 首次登录后显式刷新并同步 CostService | 0.000616 / 0.000616 | 1000/100/800/200 | Some(0.000616) | daily budget exhausted；仅 1 次 Flash |

- 每次实际完成的调用均验证唯一 `run_terminal.kind=succeeded`；所有首轮恰有一条历史成本行。首次登录组只有人民币值及价格 snapshot 缺失，provider usage/cache 全部存在；两个正常对照证明相同模型/用量/价格的 parser、finalizer、定价公式及预算门本可工作。没有读取用户凭据或发公网请求。
- `CostService::record()` 在内存报价空时故意保留 usage 并写 `cost_yuan=None`，还发出 `CostRecorded(None)` 与告警，因此不能说底层完全隐瞒了未知费用。信息在汇总决策层丢失：`CostStore` 使用 `COALESCE(SUM(cost_yuan),0)`，summary 没有未知费用完整性标记，`check_budget()` 因 0<limit 继续放行。拒绝硬编码猜价是合理边界，NULL 在预算里被当作 0 则放大了初始化接线缺口。

#### 官方与成熟实现对照

- **DeepSeek 官方** [价格页](https://api-docs.deepseek.com/zh-cn/quick_start/pricing/)提供 Flash 的缓存命中/未命中/输出分桶价格，[Responses 指南](https://api-docs.deepseek.com/zh-cn/guides/responses_api/)与[参数页](https://api-docs.deepseek.com/zh-cn/api/create-response/)规定响应 usage。H-225 没有要求 provider 额外返回人民币，也没有将本地预算当成 DeepSeek API 参数；当前应计值由正式配置已有报价和 provider 形状的合成用量算出。问题在宿主激活顺序，不在 DeepSeek KV cache 是否命中。
- **Claude Code** `借鉴/claudecode/restored-src/src/services/api/claude.ts` 收到 usage 后调用公共 `calculateUSDCost(resolvedModel, usage)`；`utils/modelCost.ts::getModelCosts()` 按 canonical model 查表，不依赖“先登录还是先创建独立成本服务”这一份可落后的空副本。未知模型会记录 telemetry、设置 `hasUnknownModelCost`，`cost-tracker.ts::formatTotalCost()` 明确提示成本可能不准确。它采用默认模型/默认档估价，**不能直接移植为 DeepSeek 官方准确费用**；这里只借鉴公共计价归属与不确定性可观测性，也不声称它对未知费用做了严格预算拒绝。
- **Grok Build** `借鉴/grok-build/crates/codegen/xai-grok-shell/src/extensions/billing.rs::handle_get_billing()` 从当前 auth gate 获取上下文后请求 billing；请求/状态/解析失败返回明确错误，而不是返回伪零 billing。其来源是服务端 credits，不是本系统 token×人民币单价，本轮未调用参考服务；可借鉴登录生效后的单一事实来源和不可用信息不伪装成数值成功。
- **Codex** 本地参考保留 token usage 与 provider limit/reset，但未找到与本系统完全相同的首登 Tauri 人民币 CostService 设计，不据此声称 Codex 已实现某种相同修复。该初始化 seam 的具体收敛属于本地设计，不能把其他 Agent 的认证或计价 API 硬套进来。

#### 影响、边界与收敛方向

- 已证实：即使官方模型、缓存分桶和持久化报价均正常，首次登录后的活跃宿主也可能把成功调用记为“有完整 tokens、无人民币”，汇总为 0，并在有限预算下继续 dispatch。受影响可持续至既有 `set_pricing` 桥接被执行或以已配置状态重建宿主；本轮动态覆盖显式同步可恢复下一次入账，没有做真实重启或证明旧 NULL 行被补算。
- 真实使用下，首次配置后的成本展示与费用预期不一致，本地限额不能覆盖这些调用；已保存 usage 尚在，保留了后续审计/按正确历史价格补算的可能性。不能将本轮 0.000616 的离线差额宣称为实际损失，也不能把 aggregate=0 写成真实费用为零或账本已被攻击篡改。
- 本轮动态范围是生产服务相同顺序及完整聊天链路，没有启动 Tauri、执行向导动画、跑真实凭据/公网或验收 UI。首次启动后台任务恰在 initialize 之后加载则可能正确，此为时序窗口，不把确定性服务顺序探针冒充所有启动序列。CLI、工作流、重登换账户、报价高峰/节假日、目录别名和真实价格刷新真实性均未动态验证。
- H-224 用完整有价历史行证明日/月时间桶错；H-14/H-63/H-163 证明特定请求 usage 归属丢失；本项使用当前时刻且 usage/cache 完整，根因是配置激活没有到达已存在的计价 owner，不重复计数。
- 根因收敛（未实施，**本地设计**）：在现有设置激活/Application composition seam 定义统一的模型/报价 ready snapshot 或带版本通知，使首次 initialize、已配置启动、显式刷新、登录切换都更新同一 CostService；不要在 Onboarding、App.tsx、每个 run 前分别补一次 refresh，不新建第二价格库/成本库。初始后台刷新失败不能被当作终身无需更新。费用不可用时保留 known/unknown 完整性；有限预算对未知费用如何阻断、要求确认或允许显式降级，应成为明确策略，而非 SQL NULL 的偶然结果。
- 回归需覆盖首次登录前后两种后台任务时序、初始化失败/重试、已有配置启动、旧空报价升级、显式刷新、账户/endpoint 切换、并发激活的版本顺序，以及实际 wire/usage/CostRecorded/成本 snapshot/下一次准入一致。历史补算需原始模型、时间和有效价格版本、幂等与审计，不能用当前价静默重写旧账。

本轮仅新增审计探针与文档，尚未修复生产初始化链路；没有修改权限、沙箱、协议、前端或数据库 schema。

本轮验证：`cargo fmt --all -- --check`、`git diff --check` 通过（首次格式检查提示新探针两处布局，格式化后复检通过）；H-225 显式探针为上述 **1 passed / 1 failed**。正常对照单独执行 `cargo test -p deepagent-app-core --test kernel_v2_e2e pricing_activation::configured_startup_and_explicit_pricing_refresh_book_usage_and_enforce_budget --offline -- --exact`：**1 passed**；`cargo test -p deepagent-app-core --lib --offline`：**1091 passed / 50 ignored**；`cargo test -p deepagent-app-core --test kernel_v2_e2e --offline`：**37 passed / 40 ignored**。默认忽略的缺陷未被常规绿色结果关闭。未执行全 workspace、Clippy、桌面 build/真实 UI、真实供应商用量与扣费验收；本轮没有生产行为/前端契约变更，不据此宣称已修复或全系统通过。

### H-226：备用模型切换后仍按初始模型结算，完整 usage 被错误归价且下一轮预算误放行或误拒绝

**级别：中高（P1/P2 之间，费用账本与本地预算完整性）；触发条件：** 普通聊天已挂载且正确激活 CostService，Chat 与 Reasoner 角色不同，运行内触发既有过载 fallback，并由不同价格的备用模型成功返回 usage。不是所有运行都错价，也不将离线过载夹具说成当前 DeepSeek 服务必然发送的错误格式。

#### 生产路径与即时复现记录

- `crates/deepagent-app-core/src/run_assembler.rs` 调用 `select_run_model(Chat, Reasoner, ...)`，将 fallback 交给 ModelAgent，但在 Kernel 执行前捕获 `model_name_for_cost = model.clone()`。运行后 `AppRunFinalizerRequest` 同时接收 `agent.cumulative_usage()` 和这份未更新的初始模型名。
- `crates/deepagent-runtime/src/model_agent.rs::think_inner` 的过载分支确实更新 `request.model` 与 `self.model` 后重试，因此真实序列化的请求已改为备用模型；`record_provider_response_usage()` 则只调用 `self.usage.record_call(&usage)`。`agent.rs::RunUsage` 保存最近 prompt gauge 与累计 billed/cache/output/reasoning，没有保存每次已回答调用的模型身份。不是“fallback 根本没发生”。
- `run_finalizer.rs::cost_request_for()` 将整份累计 billed usage 绑定到一个传入的 `model_name`；`record_usage_cost()` 用它选择价格并发 `CostRecorded.model`。正式 `costs` 表最终包含初始模型及其命中/未命中/输出单价 snapshot。模型身份错误一路扩散到账本、费用事件、summary 和下一次 `check_budget()`，不是只影响展示标签。
- 新增可重跑 `tests/kernel_v2_e2e/fallback_cost.rs`。临时文件 SQLite、MemorySecretStore 占位凭据、正式 SettingsService 初始化与 `set_model(Chat/Reasoner)`；显式把初始化已有报价同步到 CostService，排除 H-225。discovery 同时列 `deepseek-flash` 与 `deepseek-v4-pro`，不靠未知模型/别名/非法参数触发，不涉及 H-49 的小输出容量模型。
- 离线 HttpTransport 捕获正式 wire 的 `body.model`，仅在指定一次请求返回 `CoreError::provider(Some(503), Some("overloaded_error"), ...)`，其余请求返回带完整 usage 的正常 Responses 完成事件，每个已回答调用为 input=1000、output=100、cached=800、total=1100，正式 parser 推导 miss=200。**这是本系统现有 classifier 已支持的受控分类条件**；DeepSeek 官方错误页把普通 503 解释为繁忙并建议稍后重试，没有规定 `overloaded_error` 或 529，因此不据此声称已验证真实供应商的自动切换可达频率。
- 运行前断言正式 settings 的闲时价对应 Flash **0.000616 元/样本**、Pro **0.002370 元/样本**；报价来源仍为项目内置快照，不冒充动态抓价。每组设置日限额 **0.002 元**，执行两轮独立聊天；保留首轮成本行/模型名/三桶价格 snapshot/summary/CostRecorded，再观察第二轮实际 dispatch 数。首轮从空账本接纳符合现有“运行前检查”契约，检验点不是首轮运行中硬停。
- 混合模型扩展：Flash 第一次完成请求给出带 usage 的 `read_file("fixture.txt")`，正式工具成功读取测试自建临时文件；第二次 Flash 请求才受控过载，Pro 随后成功回答并给出另一份完整 usage。没有手工写 costs 行或更改费用。测试确认 `tool_completed.ok=true`，所有实际完成的聊天均只有一个 `run_terminal.kind=succeeded`。
- 2026-10-24，首次执行 `cargo test -p deepagent-app-core --test kernel_v2_e2e fallback_cost:: --offline -- --include-ignored --nocapture`：**1 passed / 3 failed（预期暴露缺陷），约 1.28s**；一项正常测试内部覆盖四组对照。运行后立即登记本节，不等待其他候选完成：

| 场景 | 首轮实际 wire 顺序 | 首轮账本模型 / 实计元 | 按实际已回答模型应计元 | 第二轮实际结果 |
|---|---|---|---|---|
| Flash 单次正常对照 | Flash | Flash / 0.000616 | 0.000616 | 放行，1 次 dispatch（正常） |
| Pro 单次正常对照 | Pro | Pro / 0.002370 | 0.002370 | budget exhausted，0 dispatch（正常） |
| Flash 两次工具循环对照 | Flash、Flash | Flash / 0.001232 | 0.001232 | 放行，1 次 dispatch（正常） |
| Pro 两次工具循环对照 | Pro、Pro | Pro / 0.004740 | 0.004740 | budget exhausted，0 dispatch（正常） |
| Flash 过载→Pro 回答 | Flash（失败）、Pro | Flash / 0.000616 | 0.002370 | **错误放行**，1 次 dispatch |
| Pro 过载→Flash 回答 | Pro（失败）、Flash | Pro / 0.002370 | 0.000616 | **错误拒绝**，0 dispatch |
| Flash 工具调用成功，再过载→Pro | Flash、Flash（失败）、Pro | Flash / 0.001232 | 0.002986 | **错误放行**，1 次 dispatch |

- 三组失败均到达费用/下一轮接纳的正确性断言，不是编译或夹具耗尽。所有首轮当前都只有一条成本行，`CostRecorded` 的 model/费用与错误账本一致。切换单次组 input/output/hit/miss 仍完整为 1000/100/800/200；混合组完整累加为 2000/200/1600/400。过载的失败请求不提供 usage，探针没有为它凭空计费；问题在成功返回的完整用量被错误归属，不是缺失 usage 的估算。

#### 官方与成熟 Agent 对照

- **DeepSeek 官方** [价格页](https://api-docs.deepseek.com/zh-cn/quick_start/pricing/)明确按模型及 cache hit/miss/output 单价计费，当前两模型价格不同；2026-10-24 为周六，属于页面规定的闲时。宿主 fallback 不会令 Pro 消耗自动按原 Flash 单价结算。[错误码页](https://api-docs.deepseek.com/zh-cn/quick_start/error_codes/)的普通 503 不等同本系统额外识别的 overload 条件。本项是宿主计价身份缺陷，不要求 DeepSeek 返回人民币，也不意味着 KV cache 分桶计算本身错。
- **Claude Code** `借鉴/claudecode/restored-src/src/query.ts` 捕获 `FallbackTriggeredError` 后更新 `currentModel=fallbackModel`，下一次 `deps.callModel` 的 `options.model` 取 currentModel；`services/api/claude.ts` 为本次调用设置 `resolvedModel`（一般取 options.model，Bedrock inference profile 另解析 backing model），usage 到达后调用 `calculateUSDCost(resolvedModel, usage)`，并将本次 `options.model` 交给 `addToTotalSessionCost`。因此计价位置在每次请求的模型解析边界，而不是整个多步 run 结束后用初始模型归价。这里只查源码，没有运行参考项目，也不将 Claude 的美元价/未知模型估价移植成 DeepSeek 人民币规则。
- **DeepSeek Harness** `借鉴/deepseek-harness/packages/llm/token-meter/src/turn-usage.ts` 的 `messageRoute()` 从每个 `assistant/message.source` 提取 provider/model，`NormalizedAttempt` 与本次 usage 一起保留 route，并按 durable attempt lifecycle 结算；任何缺失/矛盾用量不会假称精确。不过最终 `TurnTokenUsage.routes` 只是 unique route 集合，**不是每模型 token 分桶或人民币账本**，不能声称直接拿这个聚合结果就能修复混合模型成本。借鉴的是在合并前保存调用来源及完整性。
- **Codex** 本轮未找到与本系统本地人民币 CostService/日限额完全同构的实现，不声称 Codex 已提供相同 monetary-budget 修复。比较应落在单次调用身份与 usage 的归属边界，不能把 token usage 累计等价成精确金额结算。本轮不需要 OpenAI 专有计价 API。

#### 影响与架构收敛（未实施）

- 已证实双向价格错配和下一轮错误接纳：本轮 Flash→Pro 单次样本少计 0.001754 元，反向多计相同金额；混合样本少计 0.001754 元。这只是合成样本差额，**不是真实损失/扣款**。实际影响随调用用量、两模型价差及切换频率变化，不能从离线测试估算全体用户损失或公开服务故障频率。
- 廉价模型过载切到更贵模型时，本地成本会看起来更低，预算已被实际模型消耗越过仍可接纳新运行；反向切换会提前耗尽本地额度，阻断本可继续的任务。缓存分桶完整也不能弥补归价身份错，错误 snapshot 还会误导历史审计/导出及后续补算。没有证明供应商真实余额或账单被改变。
- **不能只将 `model_name_for_cost` 换成 `agent.model`：** 混合组由 Flash 和 Pro 各回答一次，若全部按最终 Pro 算会变成 0.004740，而正确为 0.002986。根因是运行累积前失去每调用 route，初始/最终标签都不足以描述混合计价。也不应在 UI 或单独 fallback 分支补一个差价特判。
- 本地设计方向：沿既有 ModelAgent/provider response usage→AppRunFinalizer→CostService/CostStore 维护有调用身份的 usage record，至少关联 run/step/attempt、实际 provider/model、调用时间与有效报价版本；按调用或同价 route 分桶结算，再投影总 usage/CostSummary。继续复用唯一账本与现有 CostRecorded/预算门，不另造 run store、cost store 或 parallel 计价服务；上下文 latest gauge 与累计 billed 语义保持不变。
- 价格不可用、route 缺失和多次 usage 更新需有显式完整性与幂等策略；不能将错误归属补成猜价或在失败重试中重复计费。按时间有效价结算/持久化价格版本是治理方向，本轮没有复现高峰切换错误，不单独登记。旧账若仅保存累计初始模型和价，不能自动断言可精确还原混合调用，补算应标明证据不足而非覆盖历史。
- 回归应覆盖主/备同价与不同价、双向切换、多个已成功工具步后切换、重试前后 usage 更新、截断续写、失败/取消、compaction/后台附属请求、无价/无 route、跨价格版本、重启后的账本/事件一致性及预算接纳零 dispatch 对照。本轮只动态覆盖普通 ChatService 两角色路径、一次受控切换和一次工具循环；未验证 Tauri UI、CLI 是否装配 CostService、工作流 fallback、真实供应商故障或跨价时段。
- 与既有项不重复：H-49 是切换后输出容量/请求参数没有重建；H-14/H-63/H-163 是特定路径漏 usage；H-224 是周期桶；H-225 是报价激活缺口。本项价格已激活、时间为当前、usage/cache 全部存在，仍因实际模型身份丢失独立错价。没有重复登记先前已确认问题。

本轮仅新增审计探针与文档，未修复生产模型/计价行为；未修改协议、前端、权限、沙箱或数据库 schema。`cargo fmt --all -- --check` 与 `git diff --check` 通过；H-226 显式探针初跑及加强账本/事件金额断言后复跑均为 **1 passed / 3 failed（预期），约 1.28s**。`cargo test -p deepagent-app-core --lib --offline`：**1091 passed / 50 ignored**；`cargo test -p deepagent-app-core --test kernel_v2_e2e --offline`：**38 passed / 43 ignored**。默认忽略的已知缺陷不因正常回归为绿而关闭。

实际尝试 `cargo clippy -p deepagent-app-core --test kernel_v2_e2e --offline -- -D warnings`，**未通过**：已有 `deepagent-models/src/client.rs:355` 的 `clone_on_copy` 和 `deepagent-persistence/src/run_store.rs:127` 的 `explicit_counter_loop` 被 deny warnings 拦截，本轮未修改这些生产文件，也未压制告警；不能声称新增测试已通过完整 Clippy。未执行全 workspace 测试、全 features Clippy、桌面 build/真实 UI、公网故障与真实扣费验收；这里只交付有证据的新增审计项，不宣称全系统通过。

### 本轮排除候选：Responses 工具参数增量仅含 `output_index`

- 当前 `ResponseAccumulator` 的 function/custom 参数 delta/done 只按 `item_id/call_id` 查找 builder，DeepSeek Harness 的 `tool-argument-streaming.spec.ts` fixture 则只给 `output_index`；受控事件可以证明本系统会漏掉这类 fixture 的实时参数增量。
- 但 DeepSeek 官方 Responses 指南明确把完整格式指向 OpenAI Responses 定义；该定义的 `response.function_call_arguments.delta/done` 同时包含 `item_id` 与 `output_index`，官方示例也提供二者。参考仓库的最小 fixture 不能冒充真实 DeepSeek wire，更不能证明官方服务会省略 `item_id`。
- 即使忽略该不完整 fixture 的 delta/done，合法的后续 `response.output_item.done.item.arguments` 仍会恢复最终参数并生成正确工具调用；已确认的差异仅是丢增量与完成事件延迟，不是执行 `{}` 或错误参数。因此本轮不分配 H 编号、不提升为 DeepSeek 协议缺陷。只有真实供应商样本或正式兼容目标明确允许缺 `item_id` 后，才应以低级别兼容性项重新评估。
