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
| stdio Harness / SDK 事件交付与恢复 | 事件队列在确定性消费者停读和真实 Windows 子进程 stdout 管道背压下均丢失通知；另有立即中断窗口、恢复丢覆盖配置、最终钩子失败时先完成后失败、审批事件重复且有一条缺少 ID、并发生产时实时事件序号倒退，以及 EOF 时运行时直接丢弃活动 turn 均已复现；两轮各自游标和独立磁盘补读对照正常 | 已确认 H-01、H-02、H-04、H-83、H-84、H-91、H-92；通知丢失尚无自动恢复，H-44 的恢复终态投影仍未修复 |
| DeepSeek 请求、reasoning、工具协议、usage | 真实请求和 usage 返回；三轮真实 session 请求观察到 cache hit 为 12288 / 2944 / 13184 tokens（仅为本次样本）；stable/dynamic system prefix 的边界与字节稳定性测试通过；Chat 工具投影拆分、MCP 图像工具结果被序列化为普通 JSON 文本、截断调用、命名工具选择协议形状不匹配、连续 reasoning item 丢段、JSON 输出模式拒绝/静默丢弃、top_logprobs 默认值静默忽略、错误解析/重试/取消缺口、流内错误丢码阻断溢出恢复、SSE 分帧不完整及备用模型输出上限分裂均已复现 | 已确认并修复 H-24、H-29；已确认 H-05、H-18、H-25、H-30、H-49、H-93、H-94、H-98、H-99、H-118、H-127；H-93/H-94/H-98/H-99 均不覆盖默认 Responses 路由 |
| 上下文、压缩、记忆 | 预取生命周期、压缩回放、压缩计费、旧笔记覆盖新分支及同步摘要取消缺口均已复现；真实笔记提取请求计数假设错误已澄清；原生摘要取消已接入运行取消树并通过回归 | 已确认并修复 H-31；已确认 H-10、H-11、H-14、H-23；其他压缩边界仍在核查 |
| 工具、审批、权限与沙箱 | 工作区拦截、backend override；迟到审批、跨运行审批冲突、审批事件重复且有一条缺少 ID、子 Agent 工具限制失效、未信任项目提权、批量工具绕过超时及读操作越过写屏障 | 已确认 H-03、H-15、H-16、H-19、H-28、H-34、H-35、H-84；其他边界未完成 |
| MCP、插件与 hooks 生命周期 | MCP 协议、stdio/HTTP 响应无 transport 字节上限、图像工具结果未转为模型多模态内容、一次性测试连接在 initialize/tools/list 阶段无界等待（已修复并验证 30 秒总握手上限）、禁用插件技能残留、标准 hooks 决策忽略、内联项目 hooks 绕过信任、hook 公共输入缺字段及事件专属字段缺失、Pre/PostCompact 输入契约/指令未生效、工作树/FileChanged 字段不兼容、后台通知缺类别、权限拒绝缺调用 ID、步数上限失败仍发完成 hook、API 失败漏发 StopFailure、Windows 后代清理、配置变更误取消、断线耗尽后能力未退役，以及续聊时 SessionEnd 按每个 turn 派发均已复现 | 已确认并修复 H-128、H-129；其他已确认 H-06 至 H-09、H-12、H-13、H-26、H-27、H-46 至 H-48、H-102 至 H-117、H-126 至 H-127；其他 hooks 和准备边界未完成 |
| 持久化、崩溃恢复、并发与输入租约 | 取消存储故障不挡信号；排队取消推进；桌面吞掉保存失败、备用宿主误终结活跃子记录、双宿主同会话同时发模型、完整工具结果引用结束即失效、启动恢复缺少终态投影，以及同一 session 多 run 的中断原因串扰均已复现 | 已确认 H-17、H-32、H-33、H-36、H-44、H-95；完整恢复及并发压力未完成 |
| 共享设置的多宿主提交 | 两条独立文件 SQLite 连接下，刷新模型用宿主缓存的旧完整设置覆盖另一服务已成功收紧的沙箱默认值 | 已确认 H-87；第三个新连接读回落盘结果，未启动真实双进程桌面 |
| 在途刷新与凭据撤销/替换 | 正式 SQLite 加密存储下，另一连接已退出或换 Key，旧模型刷新返回后仍写回旧凭据，并读回 configured=true | 已确认 H-88；两个独立数据库连接和第三连接回读，未验证真实 Tauri 多进程操作 |
| Harness 显式模型选择 | 未知模型被正式 AppCore 请求静默替换为 catalog 默认模型，运行仍以 succeeded 终止 | 已确认 H-89；实际捕获序列化 wire model；未调用公网供应商或启动真实 stdio/Tauri |
| Harness SDK 请求生命周期与协议验证 | 关闭 stdin 后，仍存活的客户端克隆调用返回 BrokenPipe，但 request id 留在 pending reply 表；initialize 接受既无 `result` 也无 `error` 的非法 JSON-RPC response 并标记已初始化；畸形 stdout 行被忽略后请求一直 pending 到 EOF | 已确认 H-90、H-100、H-101；Windows 子进程夹具；未验证真实第三方 server 对畸形响应/输出的兼容行为 |
| CLI / SDK / 桌面 / 工作流与子 Agent 一致性 | 子 Agent 工具限制、DAG 文件交付、工作流装配/调度/配置、完成事件串话、订阅竞态及父任务超时丢子运行收尾均已复现 | 已确认 H-19 至 H-22、H-38 至 H-41、H-43、H-45；记忆现有回归通过，其他图执行及重启边界未完成 |
| 定时任务与无人值守运行 | Cron 不可达审批、两进程重复触发同一任务、派发后记账失败重启重触发，以及 scheduler 停止后已派发 run 仍活跃均已复现；基础 cron/store 回归通过 | 已确认 H-37、H-42、H-96、H-97；真实模型副作用、原子接纳与宿主进程退出恢复边界未完成 |
| 移动设备操作与 UI 快照 | 正式 MobileService/Android backend 接受已被替代和未知快照，登记 ID 与返回 ID 不一致 | 已确认 H-50；底层命令 runner 为离线记录器，真机及并发输入未验收 |
| 项目记忆与知识检索作用域 | 捕获 A 项目的正式工具/检索后端后激活 B，原后端实际读取 B 并将 project 记忆写入 B 的磁盘目录 | 已确认 H-51；服务/工具级确定性交错，完整双会话桌面动态验证未执行 |
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
| 画布云端视频任务的取消等待 | 正式 ChatService/WorkflowAgent/Kernel 与真实本机 HTTP、文件 SQLite、artifact 正常对照；停止在状态接口等待中已受理，但释放响应前运行仍未退出 | 已确认 H-119；未实等生产 60 秒轮询时限，未执行真实云端生成或桌面点击 |
| 工作流节点事件的持久化通道 | 正式视频工作流实时收到 6 条节点事件；关闭全部原数据库 owner 后重开，规范 run_events 仍为 0 条节点事件；唯一运行成功终态与视频资产均保留 | 已确认 H-120；不等同于所有画布自动保存内容丢失，未验真实桌面崩溃/断线重连 |
| 视频运行中 task ID 的前端投影与续跑 | 正式 Rust publisher 发出 Running/job_id，生产 TS executor 与真实 Zustand store 收到后仍不存 ID，停止再提交时 config 缺 videoTaskId；Completed 对照正常 | 已确认 H-121；实际跨语言事件/序列化探针，Tauri 传输替换为本机内存桥，未发生真实云端重复收费 |
| 视频修改输入及配置后的执行身份 | 真实 TS/Rust 往返中，修改提示词仍轮询旧任务并 succeeded，新提示词未提交；同一 provider ID 改地址/凭据后，旧 task ID 被发到新地址并 succeeded | 已确认 H-122；本机真实 HTTP、原地址续跑对照；未验真实供应商视频、同地址仅换凭据或云端账号权限 |
| 本地与 SSH 终端的分页读取 | 真实 Windows ConPTY / SQLite 与正式 SSH session reader 均在 64 KiB 返回上限时将游标推进到全部缓存末尾；未返回后缀仍可从实际末端读出 | 已确认 H-123；小输出对照正常；当前桌面旧读取不使用该游标接口，SSH 网络与其他平台未验证 |
| 系统视觉发送前图片预处理与停止 | 正式 App submit/stop callback 与 API client 的受控图像请求中，run 尚未进入后端就显示运行；cancel 返回 not_found，释放请求后仍识别下一张并提交主模型 | 已确认 H-124；continued/new 两种会话均复现；使用实际 App 回调但 React/Tauri/provider 为隔离 harness，不是真实窗口/网络 |

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
| 有效配置与信任 | 请求覆盖、恢复、项目来源和实际 executor 条件分裂（H-03/H-04/H-27/H-28/H-54） | Codex 对恢复有效配置做校核、禁用未信任项目配置层；Claude hooks 信任检查 | 配置静默变化、用户限制未落实到实际执行 |
| 事件与恢复 | 慢消费者丢通知、订阅未就绪、恢复缺规范终态、恢复原因未按 run/task 隔离，以及最终钩子失败前已通知完成、审批由两条路径重复通知（H-01/H-43/H-44/H-83/H-84/H-95） | Codex 明确背压/断连、持续消费、按 turn 身份恢复 stale 状态及统一携带错误的 turn 完成通知；审批从单个权限请求发出带关联身份的 server request | UI/SDK 长期等待、提前结束、恢复诊断归错 run、重复提示或审批请求无法关联 |
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

### H-25：丢弃供应商 Retry-After，在服务恢复前耗尽限流重试

**级别：中（P2）；范围：真实 HTTP 非成功响应到 ModelAgent 的自动重试链路。** 触发条件是供应商实际返回有界 Retry-After；不声称所有 DeepSeek 请求都会返回这个头。

#### 生产路径与实测

- `ReqwestTransport::stream_inner()` 仅取 status 和 body 后返回 provider_error，未保留响应头。CoreError::Provider 仅有 status/code/message，ModelAgent 无法取得建议等待时长或响应头里的 request ID。
- `ModelAgent::retry_delay()` 对限流使用 500ms 基数、指数退避及 ±20% 抖动；另有限流失败次数上限 2。这一组合使其可能在服务建议的恢复时间前用完尝试次数，而非仅丢失展示信息。
- 新增默认忽略集成探针 `crates/deepagent-app-core/tests/audit_provider_retry.rs`，真实 localhost TCP HTTP 服务先返回 429、`Retry-After: 3`、`x-request-id` 和结构化错误；第二请求若已过 3 秒返回有效 Chat SSE，否则继续 429。完整读取请求体后才响应，使用独立回环地址及无代理客户端，没有外部密钥或付费请求。
- 执行：`cargo test -p deepagent-app-core --features web --test audit_provider_retry --offline -- --ignored --nocapture`。实际经过 ReqwestTransport→ModelClient→ModelAgent，记录 `provider_wait_ms=3000, actual_retry_gap_ms=455, provider_recovered=false, agent_ok=false`，最终仍是 HTTP 429，安全断言失败。该数值是一次本机样本，不是 P95 或实际供应商故障率。
- 两次请求均已由 fixture 接收，生产限流重试确实发生；最终错误保留 status/code/message，但没有服务端响应头的 request ID。测试没有运行完整 Kernel/Tauri，也不将这次失败等同于已观测的用户长等待。

#### 成熟实现、影响与修复方向

- DeepSeek Harness `packages/llm/llm-deepseek/src/transport.ts::providerError()` 将 Retry-After 秒数/日期映射为 providerRetryAfterMs，保留三种 request-id 响应头。`llm-retry/tests/retry.spec.ts` 明确验证有界 2000ms 建议被原样采用；超配置上限的建议在 normal mode 下交还错误处理，不盲目等待。always mode 另有本地退避策略。
- [RFC 9110 §10.2.3](https://www.rfc-editor.org/rfc/rfc9110.html#section-10.2.3) 说明该头表达客户端应等待多久，值可为秒数或 HTTP-date。这里不是把建议性等待误写成无条件 MUST 协议违规；缺陷是丢失已知恢复信息，导致可恢复请求过早失败。
- 在既有 provider adapter/CoreError 边界保留经过白名单处理的重试元数据和请求身份；统一策略决定采用有界 provider 等待、向上交还或本地退避。等待继续接入现有取消树，避免为这一问题新建第二套重试链。
- 增加秒数、日期、负值/非法值、过长建议、取消期间等待、不同 HTTP 状态和请求 ID 脱敏回归。request ID 丢失也削弱跨供应商排障能力，但本次没有验证 UI/日志里所有其他请求追踪来源。

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

### H-118：Responses 流式失败丢弃供应商错误码，上下文溢出恢复依赖消息措辞

**级别：中（P2，条件性模型恢复缺陷）；触发条件：供应商通过 `response.failed` 返回本系统已支持的 `context_length_exceeded` code，但 message 没有现有分类器识别的英文溢出关键词。** HTTP 错误和包含关键词的消息不能用来替代该事件路径验收。

- 根因与生产路径：`deepagent-models/src/stream.rs::ResponseAccumulator::push_sse_data_observed` 处理 `response.failed` 时只取 `response.error.message`，把真实 code 固定替换为 `response_failed`。`ModelClient` 原样向上返回该 CoreError；`classify_model_error` 本可单凭 `context_length_exceeded` 识别 ContextOverflow，但元数据丢失后只能匹配 message，最终返回 Unknown。`ModelAgent::think_inner` 的工具结果剪枝与原生压缩恢复只在 ContextOverflow 分支进入，Unknown 直接返回失败。
- 动态复现：ignored 探针 `model_agent::tests::audit_streamed_overflow_recovers_from_provider_code_without_message_keywords` 用同一 `response.failed` / `context_length_exceeded` fixture 仅替换 message，经正式 ModelClient/ResponseAccumulator/ModelAgent 执行。英文 `maximum context window exceeded` 对照实际请求 **2 次、压缩 1 次**，第二次请求的 input 从超过 3 项缩到 2 项并正常回答；中文 `输入超出限制，请缩短内容` 组实际请求 **1 次、压缩 0 次**，返回 `Provider { status: None, code: Some("response_failed"), ... }`，分类为 Unknown，原历史保持未修改。两组先验证完整 code 可被当前分类器正确识别；最终断言以 **0 passed / 1 failed** 复现。摘要为既有计数型 fixture，模型传输离线，没有真实超长请求或公网供应商错误。
- 官方与成熟实现对照：DeepSeek 官方 [Create Response](https://api-docs.deepseek.com/api/create-response/) 定义失败 response 的 `error.code` / `error.message`，[Responses 指南](https://api-docs.deepseek.com/guides/responses_api/) 明确 `response.failed` 携带包含 error 的完整 response；该页未列具体 code 枚举，本项不声称 DeepSeek 必然以该 code 返回每次溢出。Codex `codex-api/src/sse/responses_error.rs::parse_failed_response` 按实际 code 将 `context_length_exceeded` 映射到 ContextWindowExceeded；DeepSeek Harness `llm-deepseek/src/transport.ts::providerError` 同时保留 type/code/message 的分类信息，后者是 Messages 协议，仅参考错误归一化边界，不复制其事件结构。
- 影响及限制：具有已支持机器错误码的流式上下文溢出会因本地丢码而跳过恢复；依赖英文文本意味着供应商措辞变化或本地化也会改变行为。本次只动态验证 ContextOverflow，没有把限流、鉴权、过载的分类/重试/备用模型效果一并视为已验证，也没有实测供应商何时在 HTTP 与流内错误之间选择。既有直接由测试 transport 返回结构化错误的 `__ERROR_CONTEXT__` 回归绕过 accumulator，无法发现此缺口。
- 收敛方向（尚未实施，本地设计）：在现有 provider seam 统一 HTTP 与流内失败的结构化错误构造，保留可信 code 与经过现有脱敏边界处理的 message，不在 Runtime/UI 追加文本特判。缺失或未知 code 保留明确 fallback，不伪造 HTTP 状态；补测真实 code/不同 message、缺字段、各可恢复错误、取消和压缩后再次失败，核对实际请求数、压缩次数及终态。

### H-119：视频状态 HTTP 等待不响应运行取消，停止已受理仍需等待供应商返回

**级别：高（P1，长任务可用性）；范围：画布 `video-gen` 已提交云任务、正在等待 `/video/status` 响应。** 与 H-29 的聊天错误正文、H-31 的摘要等待不同，本项通过正式画布运行和成功形状的真实 HTTP 网关定位等待边界；不是 UI 没有发送停止信号。

- 生产路径：`run_assembler::run_workflow_inner` 给 WorkflowAgent 注入现有取消树的 flag；`execute_video` 将该 flag 交给 CanvasVideoRequest。`CanvasModelGateway::generate_video` 只在每轮 poll **之前**检查 flag，随后直接 await `poll_media_job(..., 60_000)`；`post_json` 的请求/正文读取没有取消选择。WorkflowAgent 只实现 `Agent::think`，继承的 `think_cancelled` 也是启动前检查；RuntimeEngine await 此决策，没有另外中断正在等待的节点。故信号已经到达仍不能打断此 HTTP 等待。
- 可重跑探针：`cargo test -p deepagent-app-core --test workflow_audit media:: --offline -- --include-ignored --nocapture`，本次 **1 passed / 1 failed**，约 0.48s。新增 `tests/workflow_audit/media.rs` 复用生产 SettingsService、CanvasProviderService、CanvasModelGateway、ChatService、Kernel、取消树、文件 SQLite 与 artifact 存储；只有供应商为本机 loopback HTTP。必须收到真实 `POST /v1/video/status` 后才调用 `request_cancel`，保持响应 pending 观察 300ms，再释放 `Failed` 响应并等待、回收工作流与服务器任务；最终断言在清理后执行。
- 实际取消组：`accepted=true, signal_ms=0, stopped_within_300ms=false, unfinished_before_response=true`，从取消到退出合计约 316ms（含 300ms 的响应前观察）。最初测试输出将这个总耗时误标为 `exit_after_release_ms`，后更正为 `cancel_to_exit_ms`，并非放行响应后还等了 316ms。释放响应后得到唯一规范 `cancelled` 终态，视频节点为 Cancelled，end 未完成，总计仅 submit/status 两个请求，没有额外提交或下载。**最终取消语义成立、当前等待无法及时退出**；不能把 0ms 的本地受理耗时写成执行已停止或 P95，也未实际等待生产 60 秒上限。
- 正常对照 `real_video_workflow_stores_the_download_and_one_success_terminal` 通过：同一生产链路执行 submit→status(Succeed)→download，状态请求携带原 task ID，产出 artifact，读回测试视频字节与下载内容相同；实时节点带正确 job ID，RunStore 恰好一个 succeeded 终态。使用的是占位 MP4 字节以验证传输与存储，没有解码/播放或真实生成质量验收。
- 成熟实现对照：Codex `codex-rs/core/src/tasks/mod.rs::handle_task_abort` 先 cancel token，再等待任务完成通知或 `GRACEFULL_INTERRUPTION_TIMEOUT_MS=100`，随后 `task.handle.abort()` 并 await `session_task.abort(...)` 的专属清理，最后发布中断生命周期并刷新 rollout。参考的是有 owner 的取消、有限等待和提交顺序，**不是**宣称 Codex 实现本系统的视频协议或能撤销云端任务；只核查参考源码，未运行其全套测试。
- 影响与边界：状态接口慢或半截正文停滞时，用户看到取消受理但同一运行/输入租约仍占用到本次 HTTP 返回或超时。视频提交（120 秒）、下载 headers/chunk（120 秒）和轮询 sleep 也存在前后检查的代码疑点，本探针仅动态覆盖状态请求等待，不将其他阶段或云端计费/撤销效果一并报为已复现。
- 收敛方向（本地设计，尚未实施）：在现有 CanvasModelBridge/取消树边界贯通可唤醒的取消上下文，覆盖 submit、poll headers/body、退避和下载，不另造运行链路；任务 owner 负责释放本地 HTTP/流式临时文件与唯一终态。明确“停止本地等待”和“供应商支持且成功执行远程取消”是两个独立结果；已提交的云任务保留 provider/account/task 身份供恢复，不能通过直接 drop Future 宣称云任务已撤销。验收需补各阶段取消、截止时间、完成竞争、无迟到请求与终态、临时文件清理、任务恢复及真实桌面。

### H-120：WorkflowAgent 的节点 publisher 绕过持久化事件通道，实时状态与输出无法从 run_events 重放

**级别：高（P1，运行恢复与机器消费）；范围：正式 `ChatService::run_workflow` 的节点生命周期、输出与媒体来源。** 与 H-41/H-43 的前端监听归属/早到事件不同，本项在监听完整且运行正常成功时，规范日志里仍没有节点事件；订阅时序修复本身不能补回不存在的记录。

- 生产根因：`run_workflow_inner` 先用原始 ChannelSink 构建 NodeEventPublisher 并交给 WorkflowAgent；AgentKernel 随后包装 PersistentEventSink，并调用 `agent.set_event_sink(persistent.clone())`。ModelAgent 接收此通道，WorkflowAgent 却继承 trait 的默认 no-op，原 publisher 始终直达实时 ChannelSink，绕开 run_events。既有 PersistentEventSink 已能序列化 `RuntimeEvent::WorkflowNode`，问题是事件没有经过它，不是缺表或缺少新 event store。
- 动态复现：`cargo test -p deepagent-app-core --test workflow_audit audit_video_node_events_survive_database_reopen --offline -- --ignored --nocapture`，**0 passed / 1 failed**，约 0.18s。复用 H-119 的真实本机 HTTP 与正式网关，运行 start→video-gen→end；正常 submit/status/download 完成，完整收集实时 NodeExecutionEvent，再 drop ChatService、CanvasArtifactService 和原 Database，使用新连接重开临时 SQLite，调用生产 RunStore::events_after 补读。
- 实际 `live=6, durable_before=0, durable_after=0, terminal=succeeded, artifact_readable=true`。实时视频 Completed 携带 provider/model/job ID、artifact URI 和输出；新连接只保留唯一 succeeded 运行终态及可读取的原视频资产，规范 replay 的节点序列为空，最后与实时序列比较失败。无需模拟断线、故意丢事件、损坏数据库或触发 provider 错误。
- 成熟实现对照：本系统自己的 Kernel/ModelAgent 已提供“同一事件经持久化后再交实时 delegate”的可复用边界，既有 `kernel_injects_durable_sink_into_streaming_agent` 对照本次亦通过。媒体参考 ArtCraft `generate_video_command.rs::handle_request` 在 enqueue 成功后调用 `insert_into_task_database_with_frontend_payload`；`TaskEnqueueSuccess` 将 provider/provider_job_id/frontend subscriber 等写入 TaskDatabase；Grok 视频后台 polling 按数据库 pending 状态恢复任务。参考的是明确保存任务身份和恢复依据；本系统应接回已有 RunStore/run_events，**不引入 ArtCraft 的另一套任务数据库**。参考源码已查，未运行其桌面或恢复测试。
- 影响与限制：以 run_events 为依据的断线补读/机器消费者不能重建各节点的状态、结果与媒体来源；UI 即使完整收到事件，也不使规范日志自动完整。CanvasWorkflowStore 是独立的图文档保存入口，界面成功回写并保存的 graph 可能仍持有输出；可选诊断日志也不等于规范 replay。本项没有证明所有画布节点在重启后消失，没有删除媒体资产，也未验证真实桌面崩溃或云端任务重连。
- 收敛方向（本地设计，尚未实施）：让 WorkflowAgent 在现有 `Agent::set_event_sink` 边界接收 Kernel 的规范 sink，publisher 与派生媒体进度 callback 使用同一运行通道；run_id/graph revision/node/attempt/task/provenance 按既有事件落盘，实时 delegate 继续由同一 owner 转发。恢复用现有游标补读和图投影，避免额外写一份节点日志或在前端拼接伪造历史。验收覆盖正常/失败/取消节点、长任务进度、数据库失败、重开补读与实时顺序及唯一终态。

### H-121：前端忽略视频进度事件的 job_id，完成前停止后重跑丢失续跑身份

**级别：高（P1，媒体任务恢复与重复提交风险）；范围：视频已提交并收到 Queued/Running 进度，但尚未完成。** 与 H-120 的规范日志缺口独立：即使实时事件准确到达前端，当前节点仍不保存 task ID；仅接回持久化 sink 不能自动修复消费方投影。

- 生产契约：`WorkflowAgent::progress_sink` 调用 `NodeEventPublisher::emit_job_progress`，发出 `status=running, job_id=<id>, updates.jobPhase=queued/running`，此时没有 outputs。`workflowExecutor.ts` 的事件处理只读取 status/elapsed/error/outputs；`applyNodeOutputs` 只从 outputs.videoTaskId 提取任务 ID，未读取顶层 job_id/updates，故运行中 task ID 被忽略。成功后 `execute_video` 的 outputs 包含 videoTaskId，恰好绕过缺陷；后端 resume 分支则明确依赖下一请求 config.videoTaskId。
- 可重跑动态探针：`cargo test -p deepagent-app-core --test workflow_audit audit_desktop_video_progress_keeps_task_id_for_resume --offline -- --ignored --nocapture`，**0 passed / 1 failed**，约 0.61s。Rust 用真实 NodeEventPublisher 分别生成 Running/queued 与 Completed 对照事件，通过 JSON stdin 传入 `scripts/audit-video-task-progress.cjs`；脚本加载生产 workflowExecutor 和真实 creativeStore，替换 Tauri invoke/event 为内存桥。最初每组均完整启动→收到事件→停止→再次启动；后续 H-122 复用夹具时将 Completed 对照改为收到本运行完成通知后正常结束，Running 组仍停止，缺陷结论不变。捕获实际生产 TS serializer 输出的下一份 WorkflowRequest，最后释放所有监听和运行 Promise 再断言。
- 实际输出：进度组 `receivedTaskId=audit-video-job, storedTaskId=null, resumedTaskId=null`；完成组以上三项均为 `audit-video-job`。同时确认对应节点 status 与真实事件一致，证明事件确已交付，失败不来自 H-41 的外部完成串扰或 H-43 的启动订阅竞态。此次没有运行真实 WebView/云端请求，不能把下一请求缺 ID 的证据写成真实供应商已经多收费。
- 成熟实现对照：ArtCraft 在 enqueue 后将 provider/provider_job_id 写入 TaskDatabase，后台基于 pending 任务取回；前端 `frontend/libs/tauri-api/src/lib/artcraft/task_queue/GetTaskQueue.ts` 的 TaskQueueItem 与投影保留 provider_job_id，身份不依赖 completed_item。参考的是运行中身份与最终产物分离，而非复制其状态表/协议；其视频命令的数据库插入失败仅记日志再继续返回，也不能被当作无故障的完整范本。
- 影响与边界：用户停止正在排队/生成的视频，再按现有入口重跑，序列化 config 不带恢复 ID；按当前 `generate_video` 分支会再次 submit，旧云端任务可能仍执行，带来重复任务/费用风险。已保存成功输出的 ID 对照正常；本探针不证明云端远程取消、真实重复请求、实际收费、所有画布持久化或账号切换行为。
- 收敛方向（本地设计，尚未实施）：在现有 run/revision/node/attempt 归属下将进度事件的任务身份与允许的 updates 投影到节点，统一字段含义并接入原有图持久化；取消/失败不能清掉尚可恢复的任务身份。身份还应绑定原 provider/model/account 版本，由后端权威记录驱动恢复，防止旧事件覆盖新任务；不能无校验地 Object.assign 任意 updates，不能把所有 rerun 都隐式当新生成。验收运行中/成功/失败、停止再跑、图重开、旧事件与新 attempt 竞争及供应商切换，核对实际 submit 次数。

### H-122：视频生成成功后修改提示词仍隐式续跑旧任务，新提示词没有发给供应商却再次成功

**级别：高（P1，输出语义正确性）；范围：已有成功视频的节点修改提示词后再点执行。** 与 H-121 运行中 ID 丢失相反，本项是完成后的旧 ID 无条件保留。两项共同说明需要明确生成意图和任务生命周期，不能用“每次都保留 ID”或“每次都清空 ID”覆盖所有情况。

- 生产路径：VideoGenForm 的提示词 onChange 只传 `{videoPrompt: ...}`；NodeConfigForm 的 handleUpdate 直接交给 creativeStore.updateNodeData 合并，旧 videoTaskId 不失效。workflowExecutor.resetAllStatus 也只改状态，serializer 将该 ID 与新提示词同时发出。`WorkflowAgent::execute_video` 把任意非空 config.videoTaskId 设为 resume_task_id；`CanvasModelGateway::generate_video` 只看是否有 ID 来跳过 submit，未确认任务是否已完成、输入是否改变或本次是否要求续跑。
- 动态复现：`cargo test -p deepagent-app-core --test workflow_audit audit_edited_video_prompt_submits_a_new_task --offline -- --ignored --nocapture`，**0 passed / 1 failed**，约 0.72s。先用正式 ChatService/Kernel/Gateway 与本机 HTTP 完成第一段视频；把该次真实 Completed 事件交给生产 TS executor/真实 creativeStore，再经同一个 updateNodeData 入口修改 videoPrompt。捕获真实 TS 下一 WorkflowRequest，直接 serde 回正式 Rust 类型并通过同一 ChatService 执行第二次，两个 run 均核对唯一终态。服务器与两个运行在断言前全部回收，没有手工给第二份请求补旧 task ID。
- 实际 `new_prompt_in_ts_request=true, carried_task_id=audit-video-job, submits=1, polls=2, new_prompt_sent=false, both_terminals=succeeded`。新提示词确实穿过前端序列化并进入后端，供应商却只收到首次 submit；第二次 status 仍查询原任务并再次下载旧任务结果。失败断言要求修改后的输入产生第二次 submit；没有把“参数根本没进入后端”当成根因，也没有把运行成功说成新视频生成成功。
- 成熟实现对照：ArtCraft `handle_artcraft_video_via_router` 依据本次 prompt/model/reference/resolution 等构造新的 GenerateVideoRequestBuilder，生成入口得到 job token 后交给已保存的 task；独立后台 polling 根据 provider_job_id 查询既有任务。参考的是“提交本次生成意图”和“恢复已提交任务”有各自身份与入口；参考代码未提供可直接照搬到本系统 DAG 的输入失效规则，该规则应作为本地设计验收，不声称参考项目所有编辑/幂等边界都正确。
- 影响与限制：用户看见新提示词仍得到旧任务结果与 succeeded，无法据状态分辨新指令是否执行；依赖该节点的下游可能继续使用过期产物。样本仅改提示词、用占位视频验证 task/request/asset 轨迹，不评价实际视频内容、模型质量或 UI 截图；供应商/账号、参考图和尺寸变化仍需独立动态验证，不能一并计为已覆盖。
- 收敛方向（本地设计，尚未实施）：在既有节点/attempt/task 关系中明确“新生成”与“恢复旧任务”的意图，保存输入身份及原 provider/model/account 归属；输入变化或主动重新生成创建新 attempt，显式续跑只使用原任务身份。已完成输出保留为来源资产，新结果提交前标注有效性，旧任务晚事件按 attempt 拒绝覆盖新结果。复用现有 run_events 与图存储，补测修改/未修改/已完成/取消/失败后重跑及实际 submit 次数，和 H-119～H-121 一起验收完整状态机。

#### 恢复归属补充：同一 provider ID 的地址/凭据替换

- 后续 ignored 探针 `audit_video_resume_rejects_replaced_provider_identity` 复用正式 ChatService/Gateway、SQLite 与两台独立 loopback HTTP。先在 A 完成 submit/status/download，从实际 Completed 取得 ID；显式带 ID 在未变的 A 上续跑，对照确认总 submit 仍为 1。随后通过生产 save_provider 保留原配置 ID，将 base URL 和占位 API Key 改成 B；resolve_model 读回新地址/新凭据，再用相同模型引用与原 task ID 经正式入口恢复。
- `cargo test -p deepagent-app-core --test workflow_audit audit_video_resume_rejects_replaced_provider_identity --offline -- --ignored --nocapture`：**0 passed / 1 failed**，约 0.41s；`replacement_polled=true, same_old_task=true, api_ok=true, terminal=succeeded`。A 不再收到请求，B 收到携带原 ID 的 status 并下载 B 返回的结果；所有运行和两个本机服务器在最终缺陷断言前回收。此处没有新增 H 编号，归于 H-122 中同一个无输入/来源绑定的 task ID 复用边界。
- 代码原因：generate_video 在每次恢复时按当前 model_ref 重新 resolve provider，resume_task_id 只有字符串，没有提交时的 endpoint/account 版本；“同一次调用内不切候选供应商”的保证无法覆盖两次运行间配置替换。参考 ArtCraft 保存任务 provider/provider_job_id 并据此选择 polling；对自定义 endpoint/凭据版本的绑定与变更处置仍需本系统本地设计，不能声称参考项目已验证任意可编辑网关配置。
- 影响与限制：新地址可能查不到旧任务，也可能把相同字符串解释成另一个任务；本机 B 接受该 ID 的 fixture 证明本系统未校验来源，不证明真实云端任务 ID 必定碰撞或已经发生数据泄漏。样本同时更换地址和占位凭据，并确认生产 resolve 的有效凭据已变化；未分别验证同地址只换 Key、真实账户切换/撤销或新主机认证规则。归属变化应明确拒绝不匹配恢复或重新核对原来源授权；不可自动复用已撤销旧凭据，也不可静默将旧 ID 交给新主机。

### H-123：本地与 SSH 终端的分片读取推进到缓存末尾，未返回的输出被游标跳过

**级别：中（P2，游标接口的数据完整性）；范围：单次待返回历史超过 64 KiB、仍在 256 KiB 保留窗口内的终端读取或重新附着。** 本项不覆盖普通模型流或命令工具的输出；桌面 `TerminalPlugin` 当前调用旧版无游标的 `localPtyRead` / `sshPtyRead`，不能据本探针宣称当前 xterm 面板已发生相同丢失。

- 生产路径：`terminal_service.rs::TerminalService::pty_read_with_cursor` 与 `deepagent-ssh/src/session.rs::SshSession::pty_read_with_cursor` 都先取得全部已接收字节的 `current`，将返回 `data` 截至 64 KiB，最后却返回 `cursor: current`。`truncated` 只检查旧历史是否被淘汰，没有反映此处尚未返回的尾部。Direct 与 SSH 的 `TerminalSessionBackend::read` 又把该 cursor 写入现有 SQLite 游标存储；Tauri 的 `local_pty_read_cursor` / `terminal_session_read` 直接暴露本地生产路径，TypeScript 也导出对应 API。
- 本地动态验证：新增默认忽略的 `crates/deepagent-app-core/tests/terminal_audit.rs::audit_direct_terminal_cursor_preserves_unreturned_output`，通过正式 TerminalService、Direct backend、实际 Windows ConPTY / cmd.exe 和临时文件 SQLite 运行。受控 `type payload.txt` 产生输出；先用正式旧版读取排空有界 transport 队列并保留历史，再让游标消费者附着。执行 `cargo test -p deepagent-app-core --test terminal_audit --offline -- --ignored --nocapture`，**0 passed / 1 failed**，约 0.45s：小输出对照 `first_bytes=3832, cursor=3832, next_bytes=0, tail_received=true`；大输出 `first_bytes=65536, cursor=107992, persisted_cursor=107992, next_bytes=0, truncated=false`。消费者按返回 cursor 继续读时收不到尾标记，而独立从实际返回长度 65536 读取仍得到 **42456 字节和完整尾标记**。所有 shell 均在缺陷断言前通过正式 close 关闭；精确总字节数包含本机控制序列和提示符，不作为跨机器固定常数。
- SSH 动态验证：`cargo test -p deepagent-ssh audit_ssh_terminal_cursor_preserves_unreturned_output --offline -- --ignored --nocapture`，**0 passed / 1 failed**，约 0.00s（首次依赖编译约 28.77s）。测试将 4 KiB transport chunks 放入正式 SshSession 的 PTY receiver，调用真实 reader，不替换读取算法。16 KiB 对照完整；96 KiB 组为 `source=98304, first=65536, cursor=98304, next=0, suffix=32768, truncated=false`，未返回后缀的每个字节均与来源相符。这里只验证会话缓冲/分页，无远程 SSH 登录或实际网络丢包结论。
- 探针限制与排错：本地夹具最初不读取 transport 就等待命令完成标记，分别在 10s / 3s 等待期失败；读取诊断显示命令确已输出但未执行后续标记。该夹具会让按 chunk 数限长的 transport 队列阻塞大量输出，不能当成产品缺陷。本次改为通过生产旧读取持续排空队列、完成后验证保留历史；没有增加生产超时或改动读取实现。未运行真实桌面游标重连、跨进程恢复、Linux/macOS PTY；未动态验证 SSH adapter 的 SQLite 写入，该分支仅由源码核对。
- 成熟机制对照：Codex `core/src/unified_exec/process.rs::OutputBuffers` 在同一锁内记录 pending 与 transcript；`process_manager.rs::collect_output_until_deadline` 取走的 pending 完整进入本次收集，`head_tail_buffer.rs::HeadTailBuffer` 对预算淘汰独立累计 `omitted_bytes`，在输出路径提供 omission marker。它不是本系统相同的字节游标协议，不能直接照搬 DTO；可借鉴的是**消费位置对应实际交付范围、预算淘汰有明确损失信息**，不把缓存生产进度等同于已交付进度。
- 影响与收敛方向（尚未实施，本地设计）：附着消费者会静默跳过尚在内存中的输出，持久化 cursor 也可能让恢复误以为已读取；例如命令错误或完成标记恰在后缀时会遗漏。复用现有终端契约，统一 local/SSH 的已生成范围、保留起点与本次返回终点，下一读取位置只能推进到本次实际返回末尾；历史淘汰仍单独显式报告。若需要客户端真正确认收到数据，还应由既有接口明确区分服务端返回位置与客户端 ACK，不能把本次已验证的返回前写库等同于网络交付成功。回归应覆盖多页、页内偏移、恰好上限、历史淘汰、重复/未来 cursor、并发生产和断线恢复；不新增第二套输出存储。

### H-124：系统视觉预处理未纳入运行生命周期，点击停止后仍处理附件并提交主模型

**级别：高（P1，可见的停止操作无效并造成后续模型调用）；范围：系统视觉开启且有图片尚未识别，正式 `onSubmit` 在调用 `runChat` / `start_chat_v2` 前等待 Vision API 的新会话与续聊。** 纯文本正常停止路径作为正对照通过；本项不是后端已接纳 run 的 cancellation tree 缺陷，也不表示所有图片识别失败。

- 生产路径：React `App.tsx::onSubmit` 先等 `getVisionSettings()`，随后同步创建运行 ID、写入 `activeRunIdsRef` 并让 `runningSessionIds` / `activePendingRunKey` 表现为运行中；构造 transcript 后调用 `recognizeImageAttachmentsOnSend()`，它又逐张 await `visionRecognizeImage()`。直到全部预处理完成后，`runChat()` 才调用 `start_chat_v2` 创建真正的 AppCore admission/cancellation registration。停止按钮此时读到有效的前端 run ID，并调用真实 `cancelRun(runId)`；Tauri `cancel_run` 最终查询 `RunCoordinator` 的活动注册。尚未 admission 的 ID 返回 `{accepted:false,state:"not_found"}`，UI 提示取消失败；原 `onSubmit` 在图像 promise 完成后没有重新检查该 run 的取消/失效状态，继续处理余下图片并调用 `start_chat_v2`。后端取消注册从未覆盖此前的识别等待。
- 动态复现：新增 opt-in `apps/desktop/scripts/audit-vision-preflight-cancel.test.cjs`。它从生产 `App.tsx` AST 加载并执行原 `onSubmit`、`onStopRun` 和视觉/附件 helper；直接加载生产 `api.ts::visionRecognizeImage/runChat/cancelRun`，受控 Tauri IPC 暂停 Vision 响应，并以与正式 RunCoordinator 一致的 accepted 语义按是否 `start_chat_v2` admission 返回取消 ACK。调用原 UI callback，分别在续聊和新会话的第一张图片请求挂起时点击停止，随后释放原图片请求，等待所有订阅、定时器和 run promise 收尾。
- 执行 `$env:RUN_HARNESS_AUDIT='1'; node --test scripts/audit-vision-preflight-cancel.test.cjs`（工作目录 `apps/desktop`），**1 passed / 2 failed**：普通文本对照 `startsBeforeStop=1, cancelsBeforeRelease=1, cancelAccepted=true, stoppingBeforeRelease=true, startsAfterStop=0`；续聊和新会话都为 `startsBeforeStop=0, cancelsBeforeRelease=1, cancelAccepted=false, stoppingBeforeRelease=false, cancelNotFoundVisible=true, startsAfterStop=1, recognizedTextSubmitted=true, preflightToolsSubmitted=1, listenersAfterFinish=0`。停止后仍多发的唯一识别请求就是第二张图片；第一张识别结果连同后续图片处理正常结束后，生产 API 收到一次新 `start_chat_v2`。失败断言在 completion 和生产 TS listener 清理之后触发，因此没有留下后台 run/测试进程。
- 本探针没有执行真实图片读取、视觉公网请求、真实 Tauri 窗口或 SQLite admission；`visionRecognizeImage` IPC 使用本机受控延迟，没有推断其服务商延迟/计费。本地 handler 不模拟 stop 前已进入取消表的后端 run，但相邻纯文本正对照证实同一个实际 stop callback 可取消这种已接纳运行。
- 成熟实现对照：Codex `core/src/tasks/mod.rs::handle_task_abort` 将活动 turn 的取消交给任务 token；Claude Code `restored-src/src/utils/attachments.ts` 的相关 prefetch 从 turn AbortController 创建 child controller，并在 turn abort 时同步中止，`query.ts` 在处理中断流和逐工具派发前主动查同一信号；DeepSeek Harness `core/agent/src/runtime-types.ts` 定义 `agent/pre-step` 的 turn signal，`api/session-controller/src/index.ts::prompt()` 在 prompt admission 前 `throwIfAborted()`。这些机制共同说明预处理要么属于一个有 owner、能取消的准备阶段，要么 admission 前就拒绝已取消请求；不能让 UI 先登记“运行中”而后端没有取消目标。本系统在提交前的 UI preflight 状态应标为单一 run 生命周期的一部分，不需要另建第二个运行表或取消机制。
- 影响与收敛方向（尚未实施，本地设计）：图片识别可能继续耗时与计费，所有剩余图片仍处理，并在用户看到“停止失败”后启动主模型。复用已有 run ID 与 AppCore admission/cancellation 树，让前端 admission 与准备状态一致；取消需要阻止未启动的下一图、后端 admission 和主请求，且对已经启动但不可中止的视觉 HTTP 至少在响应返回时检查旧 run 的取消意图并丢弃结果。要求覆盖新会话/续聊、首图/后续图、两图、切换会话、停止/正常完成竞争、视觉失败与普通文本对照，验证停止后新增视觉请求数和主模型 start 数为零，且只有一个正确终态。HTTP 级取消能否在 provider request 途中释放资源需独立验证，不从前端防迟到 admission 推导。

### H-125：SSH 客户端不验证远端主机密钥，连接到未信任服务器仍成功

**级别：高（P1，网络中间人或错误地址映射触发时破坏 SSH 对端身份、机密性与完整性；密码认证还可能泄露密码）；范围：桌面/应用通过正式 `SshService` 建立的 SSH 连接及复用该连接的命令、PTY、文件和远程安装功能。** 本项不涉及 DeepSeek 模型协议或模型请求，是独立的远端执行身份边界。

- 生产路径：`crates/deepagent-ssh/src/service.rs::connect_client`（约第 1358 行）将 `ServerCheckMethod::NoCheck` 传给 `async-ssh2-tokio 0.12.2`。已核对该版本 registry 源码 `src/client.rs::ClientHandler::check_server_key`（约第 964 行）：`NoCheck => Ok(true)`；库还提供 `KnownHostsFile`、`DefaultKnownHostsFile` 和 pin 公钥等验证选项。`SshConnectionConfig` 没有服务端 host key/fingerprint 字段，创建/更新 SSH 连接的 DTO 与表单也没有对应信任输入。`connect`、连接测试和定期状态刷新都复用 `connect_client`，因此不是只影响一条诊断路径。
- 动态复现：新增 opt-in `crates/deepagent-ssh/tests/host_key_audit.rs`。测试仅绑定 `127.0.0.1` 随机端口，在进程内启动 russh 临时服务器并随机生成 host key；使用正式 `SshService::create_connection` 和 `SshService::connect`，密码是测试专用临时值。实测 `production_connect_accepts_untrusted_key=true`。随后为同一 host/port 写入另一把随机公钥，使用依赖正式 `KnownHostsFile` 验证路径连接，得到 `known_hosts_rejects_mismatched_key=true`，证明拒绝能力可用且该测试对照能区分验证与 `NoCheck`。
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

### H-129：设置页的一次性 MCP 测试连接没有启动截止时间（已修复）

**状态：已修复。级别：中（P2，管理界面可用性）；触发条件：用户运行 MCP“测试连接”，stdio 服务在 `initialize` 或 `tools/list` 任一阶段不返回匹配响应。** 原一次性测试入口绕过正常宿主托管准备的 30 秒启动截止时间；该发现仅针对管理界面诊断入口，不代表普通聊天会等待 30 秒或无限期。

- 生产路径：桌面 Tauri `test_mcp_server` 调用 `McpService::test_server`；该入口验证配置后依次调用 MCP 初始化与工具发现。原路径没有总截止时间，stdio `StdioTransport::send` 会等待 stdout 中与 request ID 匹配的 JSON-RPC 响应。普通 `mcp_connection_status` 读取缓存状态，不会启动连接。
- 修复实现：`McpService::test_server` 现在与宿主托管准备共用 `MCP_STARTUP_TIMEOUT`（30 秒），覆盖 `initialize` + `tools/list` 整个协议握手；超时后显式关闭临时 transport，并最多等待 2 秒完成关闭。超时、握手失败或关闭失败都返回 `status=failed` 和原因，不发布到长期 ready registry。deadline 包含协议握手，但不包含同步 `connect_transport` 创建客户端/子进程之前的执行时间。
- 动态验证：两个默认运行回归测试通过同一探测实现，以 2 秒测试期限启动本机 Node stdio server。`test_server_deadlines_stdio_initialize` 让 initialize 挂起；`test_server_deadlines_stdio_tools_list` 正常完成 initialize 后让 tools/list 挂起。两者均收到对应阶段请求，期限到达后返回 `status=failed`、错误包含 `MCP startup timed out`，随后 transport close 等待子进程退出；无需手动放行 fixture。使用相同公开 `McpService::test_server` 代码路径，仅通过私有测试辅助入口缩短期限。
- 成熟实现对照：Codex `codex-mcp/src/connection_manager.rs` 为每个连接解析 `startup_timeout_sec`，缺省使用 `rmcp_client.rs::DEFAULT_STARTUP_TIMEOUT = 30s`；`async_managed_client.client().await` 的启动由 connection manager 管理。DeepSeek Harness MCP supervisor 提供可等待的 startup `ready` 与在卸载时 dispose negotiating transport，但其配置不声明与本探针等价的统一 startup deadline；本次复用本系统已有有界准备规则，不声称参考实现拥有相同的超时配置。
- 影响及限制：修复后协议握手超时不会无限占用管理界面请求。尚未覆盖 HTTP 半截响应下的实际桌面 spinner、重复点击的子进程数量，以及 UI 是否展示完整 `failed` 详情；同步 spawn 阶段也未单独限制。未访问公网、真实插件或用户数据。

## 本次验证与交付边界

| 命令或实测 | 结果 |
|---|---|
| H-129 一次性 MCP 测试连接 deadline 修复 | `cargo test -p deepagent-app-core --lib mcp_service::tests::test_server_deadlines_stdio_initialize --offline -- --nocapture` 与 `...test_server_deadlines_stdio_tools_list...`：均 1 passed；正式 MCP 探测路径 + 本机 Node stdio server 分别挂起 initialize、tools/list 至 2 秒 deadline，返回 failed/timeout 并关闭子进程。`cargo test -p deepagent-app-core --lib mcp_service::tests --offline`：26 passed / 1 ignored；`cargo fmt --all -- --check` 通过。未访问公网或用户数据 |
| H-128 运行取消中的 MCP 工具回归 | `cargo test -p deepagent-app-core --test kernel_v2_e2e mcp_cancellation::stopping_run_interrupts_inflight_stdio_mcp_call --offline -- --nocapture`：1 passed；正式 AppCore/Kernel/McpService/stdio 子进程中，取消 accepted=true 后 250ms 内退出、服务端收到 `notifications/cancelled`、工具调用有配对失败结果且模型请求数仍为 1；另有 `cargo test -p deepagent-mcp --features http --offline` 覆盖 HTTP 取消通知，`cargo test -p deepagent-runtime --offline --lib` 覆盖 token 终态 |
| H-127 MCP 图像工具结果显式审计探针 | `cargo test -p deepagent-app-core --test kernel_v2_e2e mcp_image_output::audit_mcp_image_result_is_not_serialized_as_text_json --offline -- --ignored --nocapture`：0 passed / 1 failed（预期）；真实本机 Node MCP stdio + 正式 AppCore/Kernel/Responses serializer，下一请求 output 为包含原始图片 JSON/base64 的字符串，无 `input_image` block；离线 provider，无公网 |
| H-126 stdio MCP 响应大小显式审计探针 | `cargo test -p deepagent-mcp --test response_limit_audit audit_stdio_transport_accepts_oversized_response_line --offline -- --ignored --nocapture`：0 passed / 1 failed（预期）；正式 `StdioTransport` 接受并解析 2 MiB JSONL payload，小响应对照通过；本机临时 Node 子进程，无真实插件副作用 |
| H-126 HTTP MCP 响应大小显式审计探针 | `cargo test -p deepagent-mcp --features http --test response_limit_audit audit_http_transport_accepts_oversized_response_body --offline -- --ignored --nocapture`：0 passed / 1 failed（预期）；正式 `HttpTransport` 接受并解析 2 MiB JSON body，小响应对照通过；仅 loopback fixture，无公网服务 |
| H-125 SSH 主机密钥显式审计探针 | `cargo test -p deepagent-ssh --test host_key_audit --offline -- --ignored --nocapture`：0 passed / 1 failed（预期暴露缺陷）；真实生产 `SshService.connect` 接受本机临时随机 host key，库的 `KnownHostsFile` 对照拒绝随机错误 key。仅 loopback、临时服务与测试口令；没有公网/用户 SSH 连接。未修改生产实现 |
| H-123 本地终端与 SSH 游标探针 | 本地 `cargo test -p deepagent-app-core --test terminal_audit --offline -- --ignored --nocapture`：0 passed / 1 failed，实际 ConPTY 与 SQLite；SSH `cargo test -p deepagent-ssh audit_ssh_terminal_cursor_preserves_unreturned_output --offline -- --ignored --nocapture`：0 passed / 1 failed，正式 session reader / 受控 transport。两组小输出正常，大输出均在无历史淘汰时跳过保留后缀；生产尚未修改 |
| H-123 相关正常基线 | `cargo test -p deepagent-ssh -p deepagent-terminal --offline`：SSH 1 passed / 2 ignored（含需真实服务器的既有集成测试），terminal 4 passed；`cargo test -p deepagent-app-core --lib terminal_ --offline`：9 passed，含终端服务、SQLite 租约和取消存储故障回归。未执行本批 workspace 全量、Clippy、桌面 build 或真实 Tauri 点击 |
| H-124 视觉发送前停止跨层探针 | `RUN_HARNESS_AUDIT=1 node --test scripts/audit-vision-preflight-cancel.test.cjs`：文本对照通过，继续会话/新会话的视觉等待组各 0 passed / 1 failed；释放和 listener 清理完成后，停止失败且第二张图片和主 `start_chat_v2` 仍发出。无真实图片内容、供应商、Tauri 窗口或数据库 |
| H-122 provider 来源替换补充探针 | `cargo test -p deepagent-app-core --test workflow_audit audit_video_resume_rejects_replaced_provider_identity --offline -- --ignored --nocapture`：0 passed / 1 failed，约 0.41s；原地址续跑总 submit=1，替换地址/占位凭据后 B 收到原 task ID 并返回 succeeded；仅本机 HTTP，无真实云端账号 |
| H-122 来源探针默认基线与 fixture 所有权复核 | `cargo test -p deepagent-app-core --test workflow_audit --offline`：1 passed / 8 ignored，约 0.17s；fixture 新增 provider service 保活后，H-120 重开测试显式 drop 该 owner，再按原命令复测仍 0 passed / 1 failed，约 0.17s，实时 6/落盘 0 的结论不变。fmt/diff 检查通过；未跑 workspace、Clippy、桌面构建或真实账号 |
| H-122 修改提示词后的真实往返显式探针 | `cargo test -p deepagent-app-core --test workflow_audit audit_edited_video_prompt_submits_a_new_task --offline -- --ignored --nocapture`：0 passed / 1 failed，约 0.72s；首次真实本机生成完成事件→生产 TS 修改/序列化→正式 Rust/网关第二次运行，两次 succeeded，仅 1 次 submit/2 次旧任务 poll，新提示词未发给供应商 |
| H-122 默认基线及旧探针复核 | 提取跨语言测试 helper 后，`cargo test -p deepagent-app-core --test workflow_audit --offline`：1 passed / 7 ignored，约 0.17s；H-121 原命令显式复测仍 0 passed / 1 failed，约 0.47s，完成对照保留 ID、进度组仍丢失；未将 helper 重构说成生产修复 |
| H-122 完成通知对照调整后的复核 | 脚本的 Completed 分支改用本运行完成通知正常结束后，H-122 与 H-121 各自原命令仍均为 0 passed / 1 failed，分别约 0.65s / 0.42s；同一新提示词轨迹及 ID 丢失结论不变。仅审计测试/报告，未跑 workspace 全量、Clippy、完整桌面 build 或真实云端视频生成 |
| H-119 耗时标注更正后的复核与 H-122 检查 | H-119 原命令再次显式复现，0 passed / 1 failed，约 0.47s；`cancel_to_exit_ms=338` 含 300ms 观察期，最终仍唯一 cancelled。`cargo fmt --all -- --check` 与 `git diff --check` 通过，耗时输出更正没有改变生产行为 |
| H-121 视频进度跨语言显式探针 | `cargo test -p deepagent-app-core --test workflow_audit audit_desktop_video_progress_keeps_task_id_for_resume --offline -- --ignored --nocapture`：0 passed / 1 failed，约 0.61s；真实 Rust producer 与 TS executor/Zustand/下一请求 serializer，Running 组 task ID 丢失，Completed 对照保留；未发真实云端视频请求 |
| H-121 默认基线与检查 | `cargo test -p deepagent-app-core --test workflow_audit --offline`：1 passed / 6 ignored，约 0.17s；桌面目录 `pnpm test:workflow`：77 passed，Node runner 约 3.48s；`cargo fmt --all -- --check` 与 `git diff --check` 通过。本批仅审计 Rust/脚本及报告，未改 UI/桥接生产类型，未跑完整 pnpm build、workspace、Clippy 或真实 Tauri |
| H-120 节点事件重开显式探针 | `cargo test -p deepagent-app-core --test workflow_audit audit_video_node_events_survive_database_reopen --offline -- --ignored --nocapture`：0 passed / 1 failed，约 0.18s；实时 6 条、重开前后规范日志均 0 条；唯一 succeeded 终态和 artifact 字节保留 |
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
| H-118 流式溢出恢复显式探针 | `cargo test -p deepagent-runtime --lib audit_streamed_overflow_recovers_from_provider_code_without_message_keywords --offline -- --ignored --nocapture`：0 passed / 1 failed，约 0.00s；同一 code 的英文对照请求 2 次/压缩 1 次，中文组请求 1 次/压缩 0 次并返回 Unknown；正式 parser/ModelAgent、离线 transport、计数型摘要 fixture |
| H-118 默认基线与检查 | `cargo test -p deepagent-runtime --lib model_agent::tests --offline`：58 passed / 6 ignored，约 0.98s；`cargo fmt --all -- --check` 与 `git diff --check` 通过。仅新增 ignored 测试与报告，未修改生产行为；未跑 workspace 全量、Clippy、桌面构建、真实 Tauri 或供应商流内溢出 |
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
| `cargo test -p deepagent-models --features http --offline`（H-24/H-85/H-86 修复后） | 单元测试 92 passed / 9 ignored，集成测试 1 passed / 1 ignored；0 failed |
| H-25 节单项命令 | 真实回环 HTTP 指定等待 3000ms，455ms 后已重试并再次 429，模型循环失败；安全断言失败 |
| `cargo test -p deepagent-app-core --features web --test audit_provider_retry --offline` | 常规入口 1 ignored，无失败；这是显式 opt-in 的 HTTP 故障探针，不代表限流恢复通过 |
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
| `cargo test -p deepagent-runtime --lib model_agent::tests --offline`（新增 H-18 后） | 56 passed / 1 failed / 2 ignored；原有 `time_based_microcompact_clears_stale_tool_results_after_idle` 在 `Instant::now() - 3601s` 处溢出，尚未进入生产 microcompact；单独重跑同样失败。本轮未修改该测试或生产时钟，不能报常规测试全绿，也不据此认定生产压缩故障 |
| `cargo test -p deepagent-app-core --lib --offline chat_service::tests`（新增 H-13 后） | 74 passed / 1 ignored；新缺陷探针默认忽略，常规会话服务测试无失败 |
| `git diff --check` | 通过 |

审查累计新增内容限于报告、各 H 节列出的默认忽略 Rust 缺陷探针、显式运行的桌面故障探针、宿主隔离集成探针及各节注明的正常对照/回归测试，未修改生产运行逻辑。默认测试包括取消存储故障、排队取消、Windows hook 大输入时限、上下文恢复、真实构建完成与后端拒绝等；完整清单和实际命令以各节及上表为准。没有在每批重复运行全 workspace、Clippy 或前端构建，不能据单项回归宣称全系统通过。桌面使用当前源码真实编译产物测试，截图保存在忽略提交的 `target/live-desktop-test-20261021/`（`tool-result.png`、`restart-recall.png`、`cancel-storage-failure.png`）。

### 后续独立验证：后台 MCP 准备不阻塞聊天

- 执行：`cargo test -p deepagent-app-core --test kernel_v2_e2e pending_host_mcp_initialization --offline -- --ignored --nocapture`，1 passed。此集成探针需要本机 Node，因此默认忽略；它与 H 系列预期失败的缺陷探针性质不同。
- 180 秒是 fixture 的自然完成时间；本次在两轮对话完成后主动释放，没有实际等待 180 秒。生产 MCP 启动本身还有 30 秒整体上限，测试证明的是“确认握手正在进行时，对话不等待它”，不是承诺握手可运行 180 秒。
- 两轮模型均为离线 replay；真实执行了宿主启动、MCP 进程、ChatService 接纳、组装、Kernel 及模型请求构造。MCP fixture 在释放后接受宽松 legacy 握手，以单独验证生命周期隔离；不覆盖 H-08 的严格协议兼容性。
- 该结果支持当前聊天读取 ready snapshot 的边界；不能由此证明磁盘运行包全量校验也已通过 180 秒延迟注入，更不能证明失败插件的 skills/hooks/说明全部隔离。

### 后续独立验证：运行包磁盘准备等待不阻塞聊天

- 执行：`cargo test -p deepagent-app-core --lib blocked_host_payload_preparation_does_not_block_conversations --offline -- --ignored --nocapture`，1 passed。该探针独占生产全局磁盘准备 gate，应单独运行，默认忽略。
- 创建真实临时 runtime.zip 与插件技能，在启动宿主前对其 `.runtime.lock` 加 OS 排他锁；随后观察生产 `DISK_PREPARATION` 已被 worker 持有，确认它已经进入运行包准备并等待文件锁。没有替换生产准备函数或虚构 Ready 状态。
- 保持锁期间两轮真实 ChatService/Kernel + 离线模型在 349ms 内完成；runtime/value.txt 不存在、ready skill_roots 为空、两次实际请求均没有该技能的完整目录条目。释放后校验/解压完成，技能注册表更新；第三个全新会话的实际请求包含该目录条目，且解压内容与原包相同。
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
- 结论：当前证据仍不支持“system prompt 每轮重写导致稳定前缀丢失”的缺陷，不新增 H 编号。实际前缀字节测试证明的是组装布局；live usage 是供应商对本次请求的计量。要归因 cache hit 仍需结合请求正文、provider 计费语义及更多重复样本，本次结果不做该推断。

### 后续独立验证：输出到限恢复的全部响应保留用量并写入账本

- 扩展既有 `tests/kernel_v2_e2e/incomplete_reason.rs` 的离线 provider fixture，复用正式 ChatService → ModelClient/ResponseAccumulator → ModelAgent → Kernel → AppRunFinalizer/CostService。每次请求返回不同的完整 usage：第 N 次输入 `12N`、输出 `6N`、cache hit `4N`；逐条核对 RuntimeEvent::Usage，并从真实 SQLite 费用表读取总量，释放原 ChatService/Database 后再打开同一文件读取。
- 成功恢复组两次 incomplete 后第三次 completed，**3 次请求、3 条 usage，累计输入/输出 72/36，cache hit/miss 24/48**，唯一终态 succeeded。耗尽组连续五次 incomplete 后停止，**5 次请求、5 条 usage，累计输入/输出 180/90，cache hit/miss 60/120**，唯一终态 context_exhausted；这两个样本的累计值均超过最后一次请求量。数据库重开后四项总量完全一致，未重复记账，也未因最终未完成而丢弃已回答请求的费用。
- 成熟实现对照：Claude Code `services/api/claude.ts` 收到 usage 后更新总费用，再处理 `max_tokens` 的恢复消息；DeepSeek Harness `token-meter/src/turn-usage.ts` 及其测试按尝试归集用量，避免 streaming sample 与最终 message 重复相加。本系统 `record_provider_response_usage()` 在是否升级/续写前计入 usage，AppRunFinalizer 在非成功终态也记录已发生量；本次是对这条已有行为的验证，没有修复生产逻辑或新增 H 编号。
- 首次耗尽组的测试预期误写为通用 failed，账本断言均已通过；核对 `kernel.rs::classify_terminal_error` 发现既有 `max_tokens` 映射为 context_exhausted，改为该约定后重跑。这里不据此声称该终态命名已完成语义审查，也不将测试预期修正计为产品缺陷。
- 本项不关闭 H-82 的过滤原因丢失、H-14 的未采纳摘要费用遗漏或 H-63 的附属模型调用费用归属问题。仅使用合成 provider usage 与临时数据库，没有真实供应商账单、Tauri UI、在途取消或数据库写入失败验证。

## 当前工程收敛方向与实施依赖（审计结论，尚未实施）

下表依据已复现的边界归并；优先级不等于所有问题具有相同触发概率。设计收敛在本系统既有内核，不复制参考项目建立第二套运行链。

| 修复批次 | 关联问题 | 应统一的现有边界 | 首要验收 |
|---|---|---|---|
| 执行权限与子运行上下文 | H-03、H-19、H-22、H-65 | 有效工具集合、sandbox 选择、执行根、checkpoint 与两处自动验证必须来自同一已解析执行配置；参考 Claude 的有效工具池与子 cwd、Codex 的统一命令编排 | 清单外调用不能执行；两个子目录同名写入隔离；声明 sandbox 与实际 backend 一致；配置后端拒绝时单文件/完成验证均不得转为宿主执行 |
| 可选 WASM 的时钟与调用归属 | H-59 | 既有 WasmSandbox 统一 Engine tick 与 per-Store deadline；参考上游 Wasmtime 及成熟 Agent 的局部 timer 清理 | 顺序与并发调用互不提前超时；成功/失败/取消不遗留误伤；Windows trap 错误不终结宿主；默认未启用路径保持不受影响 |
| 接纳、终态与审批生命周期 | H-02、H-15、H-16、H-17、H-84 | Harness 复用 AppCore admission；审批身份带运行作用域；执行停止与保存错误分别投影；审批通知只由一个 gate owner 发布并携带 call ID；参考 Codex 的 turn 所属 waiter 和单一权限请求 | ACK 后立即取消不发模型；终态后旧审批无效；两 run 同 call_id 独立；一项人工审批只发一条可响应请求；保存失败可见 |
| 启动恢复的 run/task 诊断归属 | H-44、H-95 | 现有恢复投影按 run 的 task 身份界定历史范围；参考 Codex 按 turn ID 标记 stale turn | 同 session 多个未完成 run 的中断原因各自匹配其 task turn；较新的 user prompt 不改写旧 run 的工具中断原因；重放稳定，且不改变唯一终态 |
| 机器终态与最终钩子的提交边界 | H-83 | 复用既有 Kernel 终态提交和协议投影；答案/钩子进度与最终结果分开，参考 Codex 的统一完成状态与 Claude 更新前执行钩子 | hook 正常/失败/取消、provider error、DB 故障及补读一致；同一 turn 只发布一个与 durable 结果相符的最终状态 |
| 运行取消覆盖所有等待与统一工具调度 | H-10、H-29、H-31、H-34、H-35、H-45、H-119 | 摘要、prefire、正文、重试、工具、画布媒体 HTTP 和前台子任务属于现有取消树；共用 ToolPipeline、deadline 与串行屏障；区分执行 Future 丢弃、外部工作清理与持久化收尾 | 各等待中停止及时退出；父超时保留 budget_exceeded 且前台子工作有界收尾；串并行超时一致；跨写屏障的读取不提前；任务有 owner；取消不假成功或吞保存错误；本地停止不冒充云任务已撤销 |
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
| 工作流节点事件与规范回放 | H-120 | WorkflowAgent 的 publisher 接回既有 Kernel 持久化 sink；参考现有 ModelAgent 通道与 ArtCraft 保存媒体任务身份，复用 run_events | 各节点实时事件与重开补读一致，失败/取消/进度也可重放；保留媒体来源和任务身份，不靠图文档偶然自动保存代替运行日志 |
| 媒体任务身份与节点投影/续跑 | H-121、H-122 | 既有节点事件携带的运行中身份统一投影，后端以 provider/account/task 和输入身份决定新生成或恢复；参考 ArtCraft 的提交与 pending task/provider_job_id 分离，DAG 输入失效为本地设计 | 收到 Running 即可安全保留续跑身份；有效续跑不重复 submit，修改输入的新生成实际提交新请求；旧 attempt 不覆盖新任务，已完成资产保留来源 |
| 桌面运行事件与订阅生命周期 | H-41、H-43 | 现有 run_id、完成 payload、订阅就绪屏障和持久化补读统一；参考 Codex thread 通道和持续消费 worker | 外部会话终态不影响画布；ACK 前完成/快速失败不丢终态及节点结果；监听失败不先派发；停止仍能发送对应取消；旧回调不能清理新 run |
| 多宿主所有权、会话派发与启动恢复 | H-32、H-33 | 数据库打开与有所有权的恢复分离；现有会话租约及运行/动作/子运行共同定义 owner，参考 Claude 存活 owner 保护、Codex thread writer 锁与后台作业有效租约保护 | 第二宿主打开不改变活跃记录；同会话不同时派发，异会话可并行；真正失效 owner 才恢复；原宿主仍可正常完成 |
| 共享设置与凭据的提交资格 | H-87、H-88 | 现有 SettingsService/SecretStore 在数据库当前版本上提交字段变更；参考 Codex Session 设置持久化许可与 DeepSeek 账户撤销/登录生命周期，跨宿主版本栅栏为本地设计 | 刷新不覆盖已收紧限制；退出/换 Key 后旧刷新不能写回旧凭据；独立连接、真实主备进程、失败和乱序完成一致 |
| 启动恢复的规范终态、会话投影与诊断归属 | H-44、H-95 | 恢复共用现有幂等终态边界，文件会话投影与 interruption 分类均具备按 run/task 身份的处理；参考 Codex 对 stale turn 的一致 Interrupted 投影 | 正常/故障重试/重开后各视图同一终态；一条规范终态；缺失投影可补；多个 unfinished run 的原因各自对应目标 turn，不能改错后续 turn |
| MCP 连接协议与投递 | H-06、H-07、H-08、H-09、H-12 | 现有 MCP manager/transport 内统一握手、会话头、分页、按 request id 分发；连接恢复与业务重试分离 | 严格 Node/HTTP fixture、乱序双请求、模糊成功写调用不自动重放 |
| 本地 MCP 进程归属与退出 | H-46 | 参考 Codex 服务级 Job/进程组 owner，显式关闭与最后引用释放共用有界清理；不依赖被启动程序自行管理后代 | Node 与非 libuv 根服务均覆盖；close/Drop、父先退出、初始化中断后后代不继续工作；containment 降级有诊断 |
| MCP 初始化复用与配置代次 | H-47 | 按服务连接身份复用 pending startup，定向取消/失效；全局视图代次不能写坏仍有效的共享 slot | 新增或修改 B 不取消未变 A；禁用/替换 A 才终止旧任务；迟到取消结果不变成新代缓存失败 |
| MCP 模型可见名称与快照发布 | H-56 | 现有 registry 在发布前统一名称唯一性和 raw owner 路由；参考 Codex 对最终可见名的消歧 | 多快照合并不发布重名；能力冲突不阻塞普通对话；历史和权限匹配保持可解释 |
| 能力目录和使用说明的内容保留与去重 | H-57 | 既有原生 Surface/事件保存实际正文并基于当前内容去重，或每请求从就绪快照注入；参考 Claude 的 MCP/skill 正文附件及 DeepSeek 连接拥有的 system section | 连续 turn 的完整 wire 均保留有效目录/说明；压缩/恢复不因旧身份标记丢正文；断线与配置更新语义一致 |
| MCP 断线恢复耗尽与能力退役 | H-48 | 探活与工具共用连接身份的恢复 owner/预算，状态驱动下一请求就绪快照；参考 Grok stdio 耗尽后 Unavailable 与注销工具 | 失败耗尽后新发现不含旧工具/说明；普通连续调用不重开启动风暴；显式重试可恢复；不影响其他服务 |
| 项目配置来源、权限与 hooks 决策 | H-26、H-27、H-28 | 配置 overlay 和独立 hook 文件共用来源授权；有效 profile 明确用户权限上限；参考 Codex 禁用未信任配置层、Claude 的 hook trust gate 与 hookSpecificOutput | 未授权项目配置不能放宽用户只读或审批；更换配置文件不能执行脚本；标准 deny/ask 能实际拦截/审批 |
| Hook 生命周期与输入/输出契约 | H-102 至 H-117 | 现有 HookData 按事件定义所需上下文，command/HTTP 共用 serializer；参考 Claude 按事件声明输入 schema 和输出处理。真实 session owner、调用 ID、transcript 与结果在派发点传入，压缩 stdout 由 compactor 明确消费 | 正式入口捕获的 payload 可校验且身份正确；普通续聊不误发 SessionEnd；失败不误发 TaskCompleted；API 错误终止派发一次 StopFailure，Stop 拒绝重试不误报；PreCompact 指令进入实际摘要请求；缺字段不能靠任意占位值掩盖 |
| Provider 流式协议、错误和请求恢复 | H-24、H-25、H-29、H-30、H-118 | 既有 transport 内分清字节解码、SSE 帧及 provider JSON；统一错误元数据、重试及正文取消边界；参考 Codex/DeepSeek 的事件解析器及全请求取消 | 标准 SSE 格式等价；半帧不能假完成；Unicode 不 panic；成功/错误正文和重试等待均可取消；流内错误保留实际 code，已支持的溢出分类不依赖 message 措辞 |
| 模型切换的有效请求参数 | H-49 | 有效模型能力、输出上限、budget、meter 和 wire 共用同一次配置解析；参考 Claude 的 fallback 重新进入 callModel 并按新模型重组默认参数 | 小输出容量备用模型可接续；快照预留与 wire 一致；显式覆盖及截断恢复不得重新引入超限字段 |
| 有效模型参数与输出预算 | H-54 | 既有配置 overlay、模型能力、桌面深度缺省统一解析到最终请求；参考 DeepSeek 本次 effort/maxTokens 优先及 Codex wire 断言 | 显式 effort/输出上限生效；发送前及下一请求预算与 wire 一致；无覆盖保持默认；不支持值明确拒绝；重试和子运行不重新覆盖 |
| 思考模式的协议转换 | H-55 | 既有 thinking 配置表达开关/强度，由 adapter 显式生成和校验自己的 wire 字段；参考 DeepSeek 分 provider 的 off 映射 | Simple/Medium/Deep 经两协议完整 Agent 路径均可发送；禁用不被默认强度重新启用 |
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

画布媒体进一步确认 HTTP 等待取消（H-119）、节点 publisher 绕开规范事件日志（H-120）、运行中 task ID 被前端丢弃（H-121）和修改输入却复用已完成任务（H-122）四个不同边界。即使调用名义上经过 Kernel，取消、事件与恢复身份仍可能在 adapter/Agent/UI 内部分裂；修复应先确定现有 run/revision/node/attempt、输入身份与 provider task 的提交和投影关系，再按四个探针分别验收，不能仅补日志、前端字段或取消后的状态颜色。

Hooks 的连续输入契约缺口（H-102 至 H-115）也指向同一工程边界：当前多个不同事件复用过于粗粒度的 HookData，再由两套 serializer 逐项拼字段。后续修复应先建立事件级契约与正式派发点的上下文传递，并统一 command/HTTP 投影；不能逐个补空字符串、固定布尔值或从 message 文本反推 metadata。探针只证明各自记录的路径，不把字段审计当成全部 hook 生命周期已经验收。
