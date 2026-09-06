---
trigger: always_on
---

# DeepAgent Studio 项目开发规则

本文件是 `G:\Code_Warehouse\DeepAgent-Studio` 的项目专属规则。用户当前指令优先于本文档；当前代码、配置、测试和官方文档优先于猜测。`借鉴/` 目录只作为设计和实现参考，不能当作可直接复制的业务代码来源。

## 0. 最高优先级铁律

1. 每次开始回复时，先称呼用户为“程序员Eighteen”。
2. 先查证，再动手。任何 API、库、框架、系统行为、命令参数和版本行为，都必须先找到依据。
3. 禁止臆造。不要凭记忆写 API 签名、配置项、模型参数、协议字段或系统沙箱能力。
4. 要根因修复，不要补丁。重复出现的问题，优先重审设计边界，而不是继续加临时特判。
5. 完成以验证为准。没实际跑过编译、测试、lint、构建或手动验证，不算完成；未执行的验证必须如实说明。
6. 忠实执行用户意图。不擅自扩范围，不顺手改无关内容，不覆盖用户已有改动。
7. 不堆屎山。新增代码必须收敛复杂度、复用既有边界，并能被测试验证；不能为了赶进度复制粘贴出第二套链路。

## 1. 项目事实与目录边界

这是一个 Rust monorepo：

- `crates/`：运行时核心、模型、工具、MCP、存储、配置等 Rust crate。
- `apps/cli/`：无头 CLI。
- `apps/desktop/`：Tauri 桌面应用；`src-tauri/` 是 Rust 后端，`src/` 是 React/TypeScript 前端。
- `借鉴/`：参考项目目录，只用于查证设计、协议和实现方式。
- `target/`、`dist/`、`node_modules/`、本地缓存、运行日志和构建产物不要提交。

涉及架构判断时，默认认为 DeepAgent Studio 的核心价值在现有内核：`AgentKernel`、`RunStore`、`run_events`、`RunPhase`、`TerminalKind`、`InputLeaseRegistry`、审批、取消、MCP、工具注册、权限、Sandboxie/Tauri UI。升级应把这些能力平台化，而不是绕开它们另造一套。

## 2. 证据优先级

动手前按顺序核对，命中即可作为主要依据：

1. 当前代码库已有同类实现、测试、配置和提交历史。
2. `借鉴/claudecode`
3. `借鉴/codex/codex-rs`
4. `借鉴/grok-build`
5. `借鉴/open-code-review`
6. `借鉴/better-harness`
7. `借鉴/deepseek-harness`，优先用于 DeepSeek 官方模型接入、插件化、运行时编排、会话/工作流设计。
8. Rust、Tauri、DeepSeek、OpenAI、Microsoft 官方文档。

如果以上都没有覆盖，必须明确标注为“本地设计”，说明为什么需要自设计，并把边界和验证方式写清楚。涉及联网查证时，优先使用官方文档和一手资料。

## 3. 动手前检查

开始开发前必须完成：

- 明确本轮需求范围、受影响模块和不应修改的内容。
- 读取将要修改的文件，以及同类功能的实现、调用方、测试和配置。
- 检查 `Cargo.toml`、`package.json`、`rustfmt.toml`、CI/脚本和现有规则文件，不猜命令。
- 涉及跨层契约时，全局搜索生产方和消费方，包括 Rust 类型、Tauri command、前端类型、事件名、持久化字段、CLI 输出和测试。
- 涉及删除数据、改公共 API、改协议、改模型供应商、改权限/沙箱边界或外部服务时，先确认影响范围；没有明确授权时不执行不可逆操作。

## 4. 反堆屎山与渐进治理

- 不通过复制粘贴、超长函数、深层嵌套、散落魔法值、重复分支、无边界全局状态或连续临时特判完成需求。
- 同一规则、状态机、事件流、审批逻辑、取消逻辑、权限判定只能有一个可信实现。
- 不为了“看起来抽象”而抽象。只有在真实复用、隔离边界或降低复杂度时，才新增 crate、trait、service、DTO、command 或 UI 组件。
- 遇到已有复杂代码，先补最小验证或测试，再按小步、可回滚的方式治理；不做无边界重写。
- 重构必须保持外部契约兼容，或同步更新所有消费方并明确说明破坏性影响。
- 如果同类问题连续出现三次，停止继续打补丁，先提出架构级修复方案。

## 5. DeepAgent 架构专属规则

### 5.1 Harness 平台化边界

- Harness 是统一入口，不是第二套运行链。CLI、SDK、Desktop、app-server 必须收敛到同一套 AppCore/RuntimeEngine 能力。
- 禁止新增第二套 run store、第二套 event store、第二套审批中心、第二套取消机制或第二套工具注册表。
- 优先把现有 `RuntimeEvent`、`RunStore`、`run_events`、运行终态和取消能力投影为 harness 协议事件，而不是重建持久化。
- 协议 DTO 与 Tauri UI DTO 分离：UI DTO 服务视图展示，harness DTO 服务机器协议。跨边界字段必须版本化、可序列化、可测试。
- `thread/start`、`thread/resume`、`turn/start`、`turn/stream`、`turn/interrupt`、`turn/steer`、`approval/respond`、`tool/list`、`config/read`、`sandbox/status` 等能力应共用协议层和测试夹具。
- 第一批优先闭环 stdio JSON-RPC、CLI JSONL 和 TypeScript SDK；HTTP/WebSocket/远程 daemon 只有在协议稳定后再进入。

### 5.2 事件与持久化

- 事件必须保持可 replay、可断线续读、顺序稳定。修改 `run_events`、sequence、terminal event 或 replay 语义时，必须补持久化和重连测试。
- 运行终态只能落一次。失败、取消、中断、完成的映射必须明确，不允许 UI、CLI、SDK 各自解释。
- 流式事件必须携带足够上下文，使前端、CLI JSONL、SDK stream 和日志能还原同一次 turn 的状态。

### 5.3 DeepSeek 模型适配

- 本项目默认模型方向是 DeepSeek 官方模型。涉及模型参数、reasoning、工具调用、流式响应、usage、cache 命中、错误码和限流策略时，以 DeepSeek 官方文档和 `借鉴/deepseek-harness` 为优先依据。
- 不得把 OpenAI/Codex 的字段、事件和 tool call 假设直接套到 DeepSeek 上。需要兼容多模型时，在 provider adapter 层做显式映射。
- 保留 DeepSeek 特有能力的结构化信息，例如 reasoning、cache usage、token usage、工具调用增量和供应商错误上下文；不能压平成只剩文本。
- 模型接入必须隔离在 `deepagent-models` 或既有 provider seam 内，不把供应商分支散落到 UI、CLI 或工具层。

### 5.4 工具、MCP 与审批

- 工具能力优先复用现有 `Tool` trait、`ToolRegistry`、权限集合、风险级别和 schema validation。
- MCP 能力优先复用现有 registry、adapter、deferred tools；不要在 CLI、SDK 或 app-server 内绕开 MCP 层手写工具发现。
- 审批请求必须走统一路由，能够被 UI、CLI、SDK 或自动策略处理。批准结果需要包含作用域、风险类别和可审计上下文。
- 高风险工具、文件写入、命令执行、网络访问和跨 workspace 操作都必须经过权限模型，不允许在新入口中默认放行。

### 5.5 沙箱与权限

- 沙箱升级必须通过 `SandboxBackend` 或等价边界适配，不能把 Sandboxie、Microsoft Windows Sandbox、无沙箱模式的判断散落到业务逻辑。
- Microsoft Windows Sandbox 适配以微软官方 `.wsb` 能力为准，包括 MappedFolders、Networking、Clipboard、Printer、MemoryInMB、LogonCommand 等；不臆造未支持的隔离能力。
- Windows Sandbox 映射目录默认最小权限，写入目录必须可审计；host path、workspace root、临时目录和日志目录都要做边界检查。
- 权限模型优先用 profile 化表达，例如 filesystem、network、command、approval reviewer、grant scope。UI 现有设置可以做映射，但不能替代协议级权限。
- 任何绕过沙箱执行命令的路径都必须有明确理由、日志和测试覆盖。

### 5.6 Tauri 桌面与前端

- 桌面端已有 `start_chat_v2`、事件通道、审批通道和会话完成通知等链路；harness 迁移应渐进并保持兼容。
- 修改 Tauri command、事件 payload 或前端类型时，必须同步 Rust 端、TypeScript 类型、调用方和构建验证。
- 所有涉及前端 UI 组件的改动，优先使用 `apps/desktop/src/components/shadcn/` 中与 `G:\Code_UZIP\ui` 或 `https://ui.shadcn.com/` 对齐的组件；不要在业务组件中直接使用原生或自设计组件，除非文件上传等语义确实要求原生控件。
- UI 改动应保持现有产品气质：工作台式、清晰、可扫描，不引入与当前设计体系冲突的孤立风格。

## 6. 通用开发规范

- 优先在现有架构内做最小、完整、可验证的修复。
- 没有必要不要新增 command、DTO、UI、协议、配置项或额外抽象。
- Rust 代码按 `rustfmt.toml` 统一格式，当前 `max_width = 100`。
- Rust 命名使用 `snake_case`，React 组件使用 `CamelCase`。
- 错误处理必须保留上下文，不吞异常，不静默降级。可以转换错误类型，但要保留原因。
- 日志走项目既有设施；核心路径的状态流转、外部调用、权限判定、配置加载、降级和恢复必须留下可追踪信息。
- 引入新依赖前确认现有依赖能否满足，检查维护状态、许可证和版本兼容性。
- 不提交密钥、API Key、token、机器专属配置、敏感日志、提示词内容或用户隐私数据。

## 7. 构建、测试与运行

常用命令以仓库当前配置为准，优先使用：

```text
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace --offline
cargo run -p deepagent-cli
cd apps/desktop && pnpm install
cd apps/desktop && pnpm build
cd apps/desktop && pnpm tauri dev
```

验证要求：

- 文档变更至少检查 diff 和 Markdown 内容一致性。
- Rust 行为变更至少运行格式检查、相关 crate 测试；跨 crate 或共享行为变更运行 workspace 测试。
- Tauri 桥接或前端类型变更至少运行 `cd apps/desktop && pnpm build`。
- UI 行为变更需要手动运行或截图验证，并报告验证方式。
- 如果环境、网络、依赖或权限导致某项验证无法执行，必须如实报告，不用“应该可以”替代结果。

## 8. 修 Bug 纪律

1. 先复现，再修复。优先用真实失败输入、日志或堆栈定位。
2. 沿调用链追到第一个错误来源，形成单一可验证假设。
3. 一次只做一个根因修复，重新运行相关验证。
4. 修复行为缺陷时，优先补真实回归测试，不只造理想化样本。
5. 同一路线连续失败三次时，停止继续微调，重新评估设计或向用户报告阻塞原因。

## 9. Git 与交付规范

- 可提交任务完成后，默认创建一个只包含本轮相关文件的 Git commit；不自动推送、不创建 PR，除非用户明确要求。
- 提交前必须检查 `git status` 和 diff，不得提交无关改动、用户未授权文件、密钥或敏感日志。
- 最近提交常用简短中文前缀，提交信息格式：

```text
【类型】中文描述
```

常用类型：

- `feat`：新增功能
- `fix`：修复缺陷
- `refactor`：不改变外部行为的重构或屎山治理
- `perf`：性能优化
- `test`：测试变更
- `docs`：文档或规则变更
- `chore`：构建、依赖或工具调整
- `migration`：数据库结构或数据迁移

交付时简要说明：

- 修改了哪些文件以及目的。
- 实际运行了哪些命令和结果。
- 哪些测试或环境验证未执行，以及原因。
- 是否存在兼容性、配置、数据迁移、权限或沙箱注意事项。
- 如果已提交，报告 commit id。

## 10. 安全与配置

- 把运行日志、本地缓存、凭据、模型请求和模型响应都当作敏感信息处理。
- 凡是涉及模型接入、工具调用、权限、沙箱、持久化、恢复逻辑、远程控制或 app-server 的改动，都要确认脱敏、边界检查和审计信息仍然有效。
- 不执行破坏性命令，不强推，不绕过 hook，不修改 Git 配置，除非用户明确要求并理解后果。


## 11. 移动端预览调试专项规则（用户强制要求）

本节适用于 Android/iOS 模拟器、USB 真机、移动端预览、UI Inspector、网络抓包和 Remote Mac。与本节冲突时，优先执行用户当前指令。

### 11.1 目标不可改变

移动端子系统的三个首要验收目标固定如下，不得用 mock、截图假象或项目定制适配替代：

1. 系统启动后，能够发现并通过 USB 访问已运行到 Android 真机上的项目，或接入已运行的模拟手机，打开页面并显示真实画面。
2. 能够通过 USB 或模拟器访问已编译项目的完整 UI 结构；UI Inspector 必须返回设备平台公开调试接口实际暴露的完整层级，不得只返回当前可见的一小部分节点，也不得以截图 OCR 代替 UI 树。
3. 能够通过 USB 或模拟器采集已编译项目的接口请求，返回请求方法、URL、传递参数、请求头脱敏结果、响应状态、响应头脱敏结果、响应结构、耗时和失败原因。

每个目标都必须分别提供：真实设备/模拟器记录、可复现步骤、原始证据位置、自动化测试结果和评分。没有真实证据只能标记为未完成。

### 11.2 禁止项目特判

禁止为了适配用户当前运行到真机上的某一个项目而添加：

- 项目名、包名、Bundle ID、页面名、接口 URL、控件文本的硬编码分支；
- 只识别某个框架、某个页面或某套业务协议的专用解析器；
- 项目专属坐标、固定截图模板、固定 JSON 字段映射；
- 绕过通用能力的临时 Tauri command、临时 MCP tool 或临时 UI 分支；
- 通过修改被测项目源码来制造“支持成功”的结果。

实现目标是类似浏览器 DevTools 的通用调试基础设施：对 Android/iOS 平台公开的设备、UI、日志和网络调试能力做统一抽象。若平台本身不公开某项能力，必须报告平台限制、当前可观测边界和验证方法，不得编造完整支持。

### 11.3 每轮任务的固定闭环

每一轮开发任务必须按以下顺序执行，少一步都不能宣称完成：

1. 说明本轮范围、受影响模块、不修改范围和验收目标。
2. 先查证现有代码、配置、调用方、测试和官方工具行为。
3. 只实现一个可验证的根因或完整垂直切片，不复制第二套链路。
4. 执行与本轮变更匹配的格式、编译、单元测试、集成测试或真实设备验证。
5. 回顾检查：查看 diff、状态机、错误路径、取消/超时、权限、脱敏、事件持久化和跨层契约，确认没有误改和临时特判。
6. 按 11.4 评分，并记录通过项、失败项、未执行项、证据和下一步。

测试失败、真实设备不可用或证据不足时，必须报告为未完成/阻塞，不得用“应该可以”或“代码已写好”替代结果。

### 11.4 每轮评分标准（100 分）

- 代码与架构边界：20 分。复用既有 AppCore/Runtime/Tool/权限/事件边界，无第二套链路、无屎山。
- 功能行为：25 分。本轮目标在真实输入下可复现，成功和失败路径都存在。
- 跨平台通用性：15 分。没有被测项目硬编码，协议和 DTO 不泄漏供应商实现。
- 测试证据：20 分。自动化测试、真实设备/模拟器测试、日志和 artifact 证据充分。
- 安全与可恢复性：10 分。审批、权限、脱敏、取消、超时、断开和重连正确。
- 复查质量：10 分。执行了 diff/status 检查，明确列出风险、遗漏和未验证内容。

90 分以下不得进入下一阶段；三项目标任意一个为 0 分，整轮最高 59 分；出现项目特判、任意 shell 绕过、伪造 UI 树或伪造抓包结果，整轮 0 分并必须回退设计。

### 11.5 三项目标的验收硬门槛

目标一必须证明：设备发现、USB/模拟器连接、设备 Ready 状态、页面启动、真实截图 artifact、断开和重连。不能只证明 adb 命令退出码为 0。

目标二必须证明：完整层级获取、稳定 snapshot_id/node_id、节点属性、层级数量和边界、节点查询，以及 snapshot 过期后的安全拒绝。不能把“当前屏幕可见节点”写成“完整 UI 结构”。

目标三必须证明：请求和响应成对关联、方法、URL、传参、脱敏头、状态、响应结构、耗时、错误和请求序号。抓包必须来自通用平台/应用调试观测链路；不能依赖当前项目添加专用日志或修改业务代码。

### 11.6 进度顺序

严格先完成目标一，再完成目标二，最后完成目标三。目标一未通过，不得堆 UI Inspector；目标二未通过，不得宣称 Agent 能可靠操作；目标三未通过，不得宣称网络调试完成。每阶段必须形成测试报告和评分记录。

### 11.7 交付报告必填项

每轮最终报告必须包含：本轮目标、修改文件、依据来源、实际命令、自动化测试结果、真实设备/模拟器型号与系统版本、USB/模拟连接方式、三项目标得分、总分、失败证据、未执行验证、发现的偏离风险、是否存在项目特判，以及下一轮唯一建议动作。

### 11.8 设备发现必须由系统内部完成

USB 真机和模拟机的发现、状态刷新、连接、断开和重连，必须由 DeepAgent Studio 内部实现的设备发现链完成：

```text
系统启动
  -> MobileService
  -> MobileRuntime
  -> Android/iOS Backend
  -> 官方设备探测接口
  -> DeviceRegistry
  -> MobileEvent
  -> Tauri/React/Agent 消费
```

具体规则：

- 系统启动时由 `MobileService` 初始化设备后端并启动受控 discovery loop；
- USB 插拔、模拟器启动/停止、设备授权变化必须由系统内部 watcher/polling 发现并转换为 `DeviceStateChanged`；
- `DeviceRegistry` 是设备列表的唯一可信来源，UI、CLI、Agent 和 MCP 只能读取它；
- Agent 不得自行扫描 USB、猜测 serial/UDID、枚举端口、调用 `adb devices`、`emulator -list-avds`、`simctl list` 或任意 shell；
- Agent 不得要求开发者在对话中手工复制设备 ID 作为正常发现流程；
- Agent 只能调用 `mobile_device_list`、`mobile_device_info` 等结构化工具，并使用系统返回的已验证 `device_id`；
- 设备 ID 必须由系统后端验证归属、平台、连接类型和当前状态，外部传入的未知 ID 必须拒绝；
- discovery loop 必须支持首次扫描、增量更新、断开、重连、授权变化、模拟器状态变化和进程重启恢复；
- discovery 失败必须产生稳定错误和诊断事件，不能让 Agent 接管发现工作；
- 任何新增“让 Agent 自己发现设备”的工具、提示词、示例或降级路径都视为架构违规。

开发期间，Agent 可以读取系统发现链的日志、事件和测试 fixture 来定位问题，但不得替代该链路执行发现。测试必须验证：系统启动后自动发现设备、列表更新可被 UI/CLI/Agent 共同消费、设备断开后自动移除或标记、重连后恢复原设备身份。
