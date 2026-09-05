# DeepAgent Studio 移动端预览与调试底层支持方案

> 状态：架构设计稿
> 适用版本：DeepAgent Studio 当前 Rust workspace + Tauri v2 + React 架构
> 目标平台：Android 模拟器、Android USB 真机、iOS Simulator、iOS USB 真机
> 核心原则：复用现有 Agent Runtime、事件存储、权限审批和 Tauri 服务边界

## 1. 目标与范围

本子系统为 DeepAgent Studio 提供统一的移动端预览与调试能力：

- 发现和管理 Android / iOS 设备
- 支持 Android Emulator、Android USB 真机
- 支持 iOS Simulator、iOS USB 真机
- 安装、卸载、启动、停止应用
- 截图、录屏、设备信息和应用信息
- UI Hierarchy 获取、节点查询、属性检查
- 点击、长按、输入、滑动等自动化操作
- Android logcat、iOS 日志和应用内日志采集
- 应用内网络请求采集
- 将移动端状态和操作提供给 Agent、MCP、CLI 和桌面 UI
- 为 uni-app、H5、原生 Android、原生 iOS 预留 App SDK 接入点

本阶段不实现 Android Emulator 或 iOS Simulator 本体，也不重新实现 ADB、Xcode、simctl、devicectl 或 XCTest。DeepAgent Studio 只负责工具链编排、统一抽象、生命周期、权限、事件和 UI。

## 2. 平台边界

### 2.1 Windows

本地支持：

- Android SDK、ADB、AVD、Emulator
- Android USB 真机
- Android UI Automator、logcat、截图和输入

本地不支持：

- iOS Simulator
- iOS USB 真机调试
- Xcode 构建和 XCTest

Windows 上的 iOS 能力必须通过 Remote Mac Runtime 转发。

### 2.2 macOS

本地支持：

- Android 全部能力
- iOS Simulator
- iOS USB 真机
- Xcode、simctl、devicectl、xcodebuild、XCTest

### 2.3 Remote Mac Runtime

Windows 桌面端通过受认证的远程连接访问 Mac：

```
DeepAgent Studio (Windows)
        |
        | Mobile Remote Protocol
        v
Mac Mobile Agent
        |
        +-- simctl
        +-- devicectl
        +-- xcodebuild
        +-- XCTest
        |
        v
iOS Simulator / iPhone
```

远程 Mac 不是第二套业务内核。它只提供 Mobile Backend Adapter，并将能力、事件和错误映射回统一移动端协议。

## 3. 与现有项目的集成边界

### 3.1 必须复用的现有能力

| 现有能力 | 移动端用途 |
| --- | --- |
| `deepagent-app-core` | MobileService、设备服务、应用服务和设置服务的统一门面 |
| `deepagent-runtime` | Agent 的 THINK → EXECUTE → OBSERVE 循环 |
| `RuntimeEvent` | 设备、应用、日志、网络和 UI 事件投影 |
| `deepagent-tools` | Mobile Tool 注册、schema、能力和风险等级 |
| `deepagent-builtins` | 面向 Agent 的移动端内置工具实现 |
| `deepagent-mcp` | 将移动端工具注册为 MCP 工具 |
| `deepagent-persistence` | 设备配置、调试会话、事件和采集记录 |
| `deepagent-terminal` | ADB/Xcode 命令的受控执行、取消和输出流 |
| `deepagent-ssh` | Remote Mac 连接、探测、文件传输和远程命令 |
| `deepagent-security` | API key、远程凭据和敏感日志脱敏 |
| Tauri AppState | 桌面端共享 MobileService 和运行时状态 |
| React `api.ts` | Tauri invoke、事件监听和浏览器预览 mock |

### 3.2 禁止新增的重复能力

- 不新增第二个 RunStore 或事件存储
- 不新增第二个审批中心
- 不让 Agent 直接执行 `adb`、`xcrun` 或任意 shell
- 不让 UI 自己解释设备状态
- 不把 Android 和 iOS 供应商字段泄漏到 Agent 工具协议
- 不在 Remote Mac 重新实现 Agent Runtime

## 4. 建议新增的 crate 与目录

第一阶段建议只新增四个核心 crate，避免一次拆出过多空壳：

```
crates/
  deepagent-mobile-core/       # 平台无关领域模型、能力和错误
  deepagent-mobile-protocol/   # 稳定 DTO、命令、事件和版本
  deepagent-mobile-runtime/    # 生命周期、会话、后端选择和事件转发
  deepagent-mobile-android/    # ADB、Emulator、UI Automator、logcat
```

第二阶段再加入：

```
crates/
  deepagent-mobile-ios/        # simctl、devicectl、xcodebuild、XCTest
  deepagent-mobile-sdk/        # App SDK 消息和调试桥接
  deepagent-mobile-network/    # 应用内网络记录模型和采集接口
```

应用入口：

```
apps/
  mobile-agent/                # macOS Remote Mac Runtime，第二阶段
```

桌面端和应用门面新增模块：

```
crates/deepagent-app-core/src/mobile_service.rs
apps/desktop/src-tauri/src/mobile_commands.rs
apps/desktop/src/mobile/
```

## 5. Core 领域模型

```rust
pub enum MobilePlatform {
    Android,
    Ios,
}

pub enum DeviceKind {
    Physical,
    Emulator,
    Simulator,
}

pub enum DeviceConnection {
    Usb,
    Local,
    Remote { host_id: String },
}

pub enum DeviceState {
    Disconnected,
    Connecting,
    Ready,
    Booting,
    Busy,
    Unauthorized,
    Offline,
    Error,
}

pub struct MobileDevice {
    pub id: String,
    pub name: String,
    pub platform: MobilePlatform,
    pub kind: DeviceKind,
    pub connection: DeviceConnection,
    pub state: DeviceState,
    pub os_version: Option<String>,
    pub capabilities: DeviceCapabilities,
}

pub struct DeviceCapabilities {
    pub screenshot: bool,
    pub ui_tree: bool,
    pub input: bool,
    pub logs: bool,
    pub install: bool,
    pub network_inspection: bool,
}
```

所有 ID 必须由 Core 生成或经过后端校验。设备序列号、UDID、远程主机 ID 和应用包名必须分开建模，不能混用字符串。

## 6. Backend 抽象

Mobile Runtime 只依赖平台无关的后端 trait：

```rust
#[async_trait]
pub trait MobileBackend: Send + Sync {
    async fn probe(&self) -> MobileResult<BackendStatus>;
    async fn list_devices(&self) -> MobileResult<Vec<MobileDevice>>;
    async fn device_info(&self, device_id: &str) -> MobileResult<MobileDevice>;
    async fn install(&self, request: InstallRequest) -> MobileResult<InstallResult>;
    async fn uninstall(&self, request: AppTarget) -> MobileResult<()>;
    async fn launch(&self, request: LaunchRequest) -> MobileResult<()>;
    async fn terminate(&self, request: AppTarget) -> MobileResult<()>;
    async fn screenshot(&self, device_id: &str) -> MobileResult<ArtifactRef>;
    async fn ui_snapshot(&self, device_id: &str) -> MobileResult<UiSnapshot>;
    async fn perform_input(
        &self,
        device_id: &str,
        action: InputAction,
    ) -> MobileResult<InputResult>;
    async fn subscribe(&self, device_id: &str) -> MobileResult<MobileSubscription>;
}
```

Backend 只负责外部工具和设备协议。权限判断、审批、事件持久化、Agent 工具调用和 UI DTO 由上层完成。

## 7. Android 实现

### 7.1 工具链

第一阶段通过受控进程调用：

- `adb`
- `emulator`
- `sdkmanager`
- `avdmanager`
- UI Automator 导出的 hierarchy
- `logcat`

执行必须经过已有 Terminal/SandboxBackend 边界，使用固定参数数组，不拼接 shell 字符串。

### 7.2 Android 能力

```
adb devices
adb -s <serial> install <apk>
adb -s <serial> uninstall <package>
adb -s <serial> shell am start ...
adb -s <serial> shell am force-stop ...
adb -s <serial> shell input tap ...
adb -s <serial> shell input text ...
adb -s <serial> shell input swipe ...
adb -s <serial> exec-out screencap -p
adb -s <serial> logcat
```

每次调用必须记录：

- backend
- device_id
- argv（敏感参数脱敏）
- started_at / finished_at
- exit_code
- stdout/stderr 摘要
- artifact 引用
- RuntimeEvent 关联 ID

### 7.3 USB 真机

状态必须区分：

- `unauthorized`：等待用户在设备上授权 USB 调试
- `offline`：设备存在但 ADB 通道不可用
- `device`：可操作
- `no permissions`：主机权限或驱动异常

Windows 驱动检测和 Android SDK 路径属于环境诊断，不应在 Agent 工具中自动修改系统。

### 7.4 Emulator

Emulator 管理服务负责：

- 列出 AVD
- 启动和停止 AVD
- 等待 boot completed
- 将 AVD 与 ADB device 绑定
- 记录启动日志和失败原因

“内置模拟器”在产品中表示生命周期和窗口集成，不表示把 Emulator 嵌入 Rust。

## 8. iOS 实现

### 8.1 macOS Backend

通过受控命令适配：

- `xcrun simctl`
- `xcrun devicectl`
- `xcodebuild`
- XCTest/XCUITest runner
- macOS 日志采集工具

必须在运行时探测 Xcode、Command Line Tools、设备授权和 iOS Runtime 是否可用。工具版本、参数和输出格式以当前安装的 Apple 官方工具为准，不能写死未经验证的字段。

### 8.2 Simulator

支持：

- 列出设备和 Runtime
- 创建、启动、停止、重置
- 安装和卸载 App
- 启动和终止 Bundle ID
- 截图和录屏
- 打开 URL、推送测试数据
- 获取测试日志

### 8.3 USB 真机

真机调试需要：

- macOS
- Xcode 和已安装的开发组件
- 设备信任和开发者模式
- 有效签名和 provisioning profile
- 通过 `devicectl` 或 Xcode 流程验证连接

DeepAgent Studio 不绕过签名、信任或开发者模式；所有失败都必须呈现可操作的诊断信息。

### 8.4 Windows 远程 Mac

Remote Mac Agent 提供与本地 iOS Backend 相同的 Mobile Backend API。连接复用现有 SSH 配置和安全存储，首版可以采用 SSH + 长驻 agent 进程，后续再升级为双向 WebSocket。

远程请求必须包含：

- protocol_version
- request_id
- workspace_id
- device_id
- capability
- timeout_ms
- cancellation_token

远程输出必须限制大小，并把截图、录屏、日志等大对象作为 artifact 传输，不能把二进制直接塞入普通事件。

## 9. 统一 UI Tree

Android UI Automator、iOS XCTest 和 App SDK 都转换为统一节点：

```rust
pub struct UiNode {
    pub node_id: String,
    pub parent_id: Option<String>,
    pub role: UiRole,
    pub text: Option<String>,
    pub label: Option<String>,
    pub accessibility_id: Option<String>,
    pub bounds: Bounds,
    pub visible: bool,
    pub enabled: bool,
    pub clickable: bool,
    pub editable: bool,
    pub children: Vec<String>,
    pub source: UiNodeSource,
}
```

```rust
pub enum UiRole {
    Page,
    Button,
    Text,
    TextBox,
    Image,
    List,
    ListItem,
    Checkbox,
    Switch,
    Dialog,
    WebView,
    Unknown,
}
```

查询 API：

- `ui.snapshot`
- `ui.find`
- `ui.inspect`
- `ui.click`
- `ui.long_press`
- `ui.input`
- `ui.scroll`
- `ui.press_back`

节点 ID 必须只在当前 snapshot 或调试会话内有效，避免设备 UI 变化后误操作旧节点。

## 10. App SDK 设计

黑盒模式无法访问 Compose、React Native、uni-app/Vue 组件树、业务状态和内部网络请求，因此保留 App SDK 模式。

App SDK 不直接暴露任意执行能力，而是提供明确的调试桥：

```
App
  ├── UiProvider
  ├── ConsoleProvider
  ├── NetworkProvider
  ├── StorageProvider
  └── RuntimeBridge
```

第一阶段只定义协议和 Android 示例实现：

- 应用启动/停止通知
- 应用内日志
- 网络请求摘要
- 自定义 UI 节点树
- 截图标记和业务事件

第二阶段再提供：

- Android Kotlin SDK
- iOS Swift SDK
- uni-app/Vue 插件
- React Native / Compose 适配

SDK 的数据必须经过用户显式开启的 debug profile，发布构建默认关闭，避免泄露业务数据。

## 11. 网络与日志

### 11.1 网络

首版采用应用内拦截器，不做全局 HTTPS MITM：

- Android：OkHttp/应用层拦截器
- iOS：URLSession/应用层拦截器
- uni-app：`uni.request` 包装器

统一模型：

```rust
pub struct NetworkRecord {
    pub request_id: String,
    pub method: String,
    pub url: String,
    pub status: Option<u16>,
    pub duration_ms: u64,
    pub request_headers: RedactedMap,
    pub response_headers: RedactedMap,
    pub request_body: Option<RedactedBody>,
    pub response_body: Option<RedactedBody>,
}
```

默认脱敏 Authorization、Cookie、token、密码和身份证等字段，并设置大小上限。

### 11.2 日志

统一为 `MobileLogRecord`：

- Android logcat
- iOS OSLog/XCTest 输出
- App SDK console
- 崩溃摘要

日志只进入诊断流和可选持久化，不默认写入普通聊天上下文。

## 12. 事件模型

不要新建独立 Agent Event Bus。移动端事件应作为现有 RuntimeEvent 的扩展或关联事件投影：

```rust
RuntimeEvent::Mobile {
    session_id: Option<String>,
    device_id: String,
    event: MobileEvent,
}
```

核心事件：

- DeviceDiscovered
- DeviceConnected
- DeviceDisconnected
- DeviceStateChanged
- AppInstalled
- AppStarted
- AppStopped
- UiSnapshotCaptured
- UiChanged
- InputPerformed
- ScreenshotCaptured
- LogReceived
- NetworkRequest
- NetworkResponse
- CrashDetected
- BackendDiagnostic

事件必须满足：

- 可序列化
- 可回放
- 可断线续读
- 关联 session/run/turn/device/request
- 终态只落一次
- 二进制通过 artifact 引用
- 敏感字段脱敏

## 13. Tool 与 MCP

Agent 只能调用统一移动端工具：

```
mobile_device_list
mobile_device_info
mobile_device_connect

mobile_app_install
mobile_app_uninstall
mobile_app_launch
mobile_app_stop

mobile_ui_snapshot
mobile_ui_find
mobile_ui_inspect
mobile_ui_click
mobile_ui_input
mobile_ui_swipe

mobile_screenshot
mobile_record_start
mobile_record_stop
mobile_logs
mobile_network
```

工具实现位于 `deepagent-builtins` 或专用 mobile tool 模块，注册仍走现有 `ToolRegistry`。

风险等级建议：

| 操作 | 默认策略 |
| --- | --- |
| list/info/snapshot/logs | allow |
| screenshot/find/inspect | allow |
| click/input/swipe | ask 或按项目 profile |
| install/uninstall | ask |
| launch/stop | ask |
| clear data/reset emulator | deny/ask |
| shell/任意命令 | deny，只有受控诊断路径允许 |

工具返回结构化结果和 artifact 引用，不能只返回拼接文本。

## 14. Tauri 与 React 接入

Tauri 新增命令建议：

- `mobile_backend_status`
- `mobile_list_devices`
- `mobile_device_info`
- `mobile_start_emulator`
- `mobile_stop_emulator`
- `mobile_install_app`
- `mobile_launch_app`
- `mobile_stop_app`
- `mobile_ui_snapshot`
- `mobile_ui_action`
- `mobile_capture_screenshot`
- `mobile_logs_subscribe`
- `mobile_remote_mac_list`

前端新增 Mobile DevTools 页面：

```
设备栏 | 预览区 | Inspector
-------------------------
UI Tree | Screen | Properties
-------------------------
Console | Network | Timeline
```

前端只消费 Mobile DTO 和事件，不直接解析 ADB、simctl 或 XCTest 输出。

浏览器预览模式应提供静态 mock，不能假装访问真实设备。

## 15. 持久化设计

在现有 SQLite 迁移中增加：

- `mobile_backends`
- `mobile_devices`
- `mobile_debug_sessions`
- `mobile_artifacts`
- `mobile_network_records`
- `mobile_log_cursors`

不把完整截图、录屏和大日志直接塞进事件表。事件表保存 artifact 元数据、哈希、大小、mime、路径和来源。

远程 Mac 凭据、设备配对信息和 SDK token 继续走现有加密 secret store / OS keychain。

## 16. 分阶段实施计划

### Phase M0：契约和环境探测

交付：

- `deepagent-mobile-core`
- `deepagent-mobile-protocol`
- Backend status DTO
- Android/iOS 工具链探测
- 统一错误模型
- JSON fixture 和协议测试

验收：

- Windows 能报告 Android SDK/ADB/Emulator 状态
- macOS 能报告 Xcode/simctl/devicectl 状态
- 不安装工具时返回明确诊断

### Phase M1：Android 本地闭环

交付：

- ADB 设备发现
- USB 真机和 Emulator 状态
- install/launch/stop
- screenshot
- logcat
- 基础输入
- UI hierarchy snapshot
- Tauri/React Mobile DevTools 初版

验收：

- Android Emulator 完成一次安装、启动、截图、点击、日志闭环
- USB 真机能够区分 unauthorized、offline、device
- Agent 通过工具完成一次 UI 操作
- 事件可在断线后回放

### Phase M2：Android Inspector 和 App SDK

交付：

- 统一 UI Tree
- find/inspect/action
- Android App SDK 协议
- 网络和应用日志采集
- artifact 生命周期

验收：

- 同一 Agent 工具不依赖 Android 原生类名
- 节点失效时拒绝操作并要求重新 snapshot
- 敏感字段脱敏测试通过

### Phase M3：macOS iOS 本地能力

交付：

- simctl Simulator 生命周期
- Simulator app install/launch/screenshot
- devicectl 设备探测
- XCTest UI snapshot/action bridge
- iOS 日志和错误诊断

验收：

- Simulator 完成最小闭环
- 无 Xcode、未授权设备和签名错误均能分类呈现
- iOS UI Tree 映射到同一 NormalizedNode

### Phase M4：Remote Mac Runtime

交付：

- Mac agent
- SSH bootstrap/health probe
- 双向请求取消
- artifact 传输
- Windows → Mac → iPhone/Simulator 闭环

验收：

- Windows 可查看远程 Mac 的 Simulator 和 USB iPhone
- 断线后请求不会永久挂起
- 远程凭据不落明文
- 远程设备事件可回放

### Phase M5：uni-app 与多框架 SDK

交付：

- uni-app/Vue 调试桥
- React Native/Compose/SwiftUI 扩展点
- 组件树、业务事件和网络记录
- 插件化 SDK 分发

## 17. 测试与验证

必须增加：

### Rust 单元测试

- DTO 序列化兼容
- UI Tree 归一化
- selector 查询
- 设备状态机
- capability 检查
- 命令参数构造
- 敏感日志脱敏
- 取消与超时

### Backend 集成测试

使用 fake executable 或 mock transport，不要求 CI 安装 Android SDK/Xcode：

- ADB 输出解析
- simctl 输出解析
- devicectl 错误分类
- Remote Mac 请求/响应
- artifact 大小限制

### 真实设备验收矩阵

| 场景 | Windows | macOS |
| --- | --- | --- |
| Android Emulator | 必须 | 必须 |
| Android USB | 必须 | 必须 |
| iOS Simulator | 远程 Mac | 必须 |
| iOS USB | 远程 Mac | 必须 |

### 项目验证命令

```text
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace --offline
cd apps/desktop
pnpm build
```

涉及 Tauri 命令或前端类型变更时必须执行桌面构建；涉及真实设备的验证必须另行记录操作系统、工具链版本、设备型号、系统版本和连接方式。

## 18. 安全要求

- Agent 不得直接执行任意 ADB/Xcode shell
- 所有安装、卸载、输入、清数据和远程操作经过统一权限 profile
- USB 设备身份、远程 Mac 身份和应用包名必须校验
- 截图、日志、网络正文默认按敏感数据处理
- 网络和日志正文默认脱敏并限长
- Remote Mac 使用现有密钥存储、SSH 配置和审计日志
- 设备断开、取消、超时必须终止子进程和远程请求
- 所有高风险动作都必须有可审计的 approval context

## 19. 首个可交付切片

建议第一轮只做 Android 本地闭环：

```
设备发现
 → 选择 USB/Emulator
 → 安装 APK
 → 启动 App
 → 截图
 → UI snapshot
 → Agent click/input
 → logcat
 → RuntimeEvent 回放
```

完成这个切片后，再抽象 iOS Backend 和 Remote Mac。这样可以尽早验证 Mobile Runtime 是否真正融入现有 Agent Kernel，而不是先堆积跨平台接口和 UI。

以下第 20—33 节为实施约束与防偏离规则，与前面的架构方案共同构成完整开发要求。

## 20. 唯一主链路

所有能力必须沿着下面的链路：

    React / CLI / Agent / MCP
      -> deepagent-app-core::MobileService
      -> deepagent-mobile-runtime
      -> platform backend
      -> ADB / Emulator / simctl / devicectl / XCTest

禁止从 React 或 Agent 直接执行外部命令。Tauri command 只能做参数转换和服务调用。

## 21. 四个硬边界

1. Agent 只调用 `mobile_*` 结构化工具，不能知道 ADB、simctl 参数或原生控件类名。
2. React 只消费 Mobile DTO、MobileEvent 和 ArtifactRef，不能解析 ADB/XML/plist/XCTest 原始输出。
3. Mobile Runtime 不能创建第二套 THINK/EXECUTE/OBSERVE 循环、RunStore、审批中心或工具注册表。
4. Remote Mac 只接受版本化白名单方法，禁止任意 command、shell、script 或未经校验的路径。

## 22. 先做契约，再做真实适配器

在写真实 ADB 代码前，必须先完成并测试：MobilePlatform、DeviceKind、DeviceState、MobileDevice、DeviceCapabilities、MobileOperation、MobileOperationKind、MobileErrorCode、UiSnapshot、UiNode、MobileEvent、ArtifactRef。

跨边界类型必须实现 serde；新增字段必须兼容旧客户端；枚举新增值不能让旧端崩溃；每个 ID、时间、状态和错误都要有 fixture。

## 23. 设备状态

设备状态转换必须集中在 mobile-runtime，不能由 UI 推断。Android 至少区分 device、unauthorized、offline、no permissions 和 removed；iOS 至少区分 Booting、Ready、Shutdown、未信任、未开发者模式和工具链不存在。每次转换都产生 DeviceStateChanged 事件。

只有 Ready 才能执行普通操作；Busy 时同一设备默认拒绝第二个互斥操作；断开后未完成操作必须产生终止事件。

## 24. 操作与取消

每个操作必须携带 operation_id、debug_session_id、device_id、deadline、cancel token 和审批上下文。必须处理启动前取消、运行中取消、设备断开、超时、SSH 断线和 Agent run 取消。取消后不得继续发送输入、写 artifact 或发送成功事件。

禁止用 `execute_mobile(command: String)` 覆盖所有动作。操作必须使用结构化的 Install、Launch、Snapshot、Tap、InputText 等枚举。

## 25. UI Snapshot 防错

node_id 只在当前 snapshot 有效。点击、输入、滑动请求必须带 snapshot_id + node_id。快照变化后操作必须返回 StaleUiNode，要求重新 snapshot；不得偷偷改用当前坐标。selector 顺序为 accessibility_id、resource identifier、role+label、visible text、结构路径，坐标仅供人工调试兜底。

## 26. 外部命令

必须先实现 ToolResolver：解析路径、读取版本、判断能力、记录诊断并缓存结果。进程调用使用 argv 数组，禁止拼接 shell 字符串，禁止以 powershell/cmd/sh 作为通用逃生口。每次调用保存脱敏参数、工作目录、起止时间、退出码、输出摘要、artifact 和 operation_id，并设置大小上限、超时、取消和 UTF-8 容错。

## 27. 权限矩阵

list/info/status、snapshot/find/inspect、screenshot 默认 allow；click/input/swipe、install/launch/stop、录屏默认 ask；uninstall/clear data/reset 可 ask 或 deny；任意 shell 默认 deny；远程 Mac 管理默认 ask。审批必须关联 operation_id、device_id、tool、风险类别、作用域和操作者。

## 28. Artifact

截图、录屏、完整 logcat 和网络正文不能直接写入 RuntimeEvent。事件只保存 artifact_id、mime、size、hash、来源、生命周期和权限级别。大对象必须限长、可取消、可过期清理，并且只能写入应用数据目录或允许的 workspace 目录。

## 29. 第一阶段严格范围

第一阶段只实现 adb devices、device info、screenshot、logcat tail、install、launch、stop、UI snapshot、tap/input/swipe 和 RuntimeEvent 投影。禁止全局 HTTPS MITM、自动安装 SDK、任意 shell、root、清数据、自动签名 APK、同时实现 iOS 或 uni-app 业务组件树。

## 30. iOS 与 Remote Mac 顺序

iOS 严格按“工具链探测 → Simulator list/info → boot/shutdown → install/launch → screenshot → UI snapshot → USB discovery → USB install/launch → XCTest action → Remote Mac”推进。Windows 不能假装拥有 iOS backend，只能报告 Remote Mac 不可用或连接真实 Mac。

Remote Mac 先做 fake protocol，再做 SSH 启动，最后接真实设备。每个请求必须有 protocol_version、request_id、method、deadline、cancel_token、workspace_id、device_id。远端 agent 崩溃、SSH 断开、设备断开和命令失败必须是不同错误。

## 31. 合并硬门槛

必须具备 fake backend、ADB 状态解析、UI selector、stale node、取消、设备断开、审批、artifact 限制、事件 replay 和协议版本不匹配测试。没有真实 SDK 的 CI 也必须通过 fake backend。

出现以下任意情况应拒绝提交：Tauri 直接 `Command::new("adb")`、Agent 接收任意 shell、UI 解析 XML、没有 snapshot_id、没有取消/超时、没有权限等级、远端接受任意 shell、二进制塞进 RuntimeEvent、第二套 event bus、Windows 假设 Xcode 存在、静默 fallback。

## 32. PR 交付模板

每个 PR 必须说明：修改层次及原因、新增 DTO/事件及兼容性、外部命令如何解析/取消/审计、权限在哪里执行、断开如何收敛、大对象如何存储、fake backend 测试、真实 OS/工具链/设备验证，以及明确未实现的能力。

## 33. 最小闭环验收

发现 Android Emulator → 创建 DebugSession → 等待 Ready → 安装 APK → 启动 package/activity → 获取 screenshot artifact → 获取 UiSnapshot → 按 snapshot_id+node_id 点击 → 重新 snapshot → 读取 logcat → 断开设备 → replay RuntimeEvent。过程必须可见 operation_id、状态变化、审批决定、稳定错误码、artifact hash/size，并能被 Agent、桌面 UI、CLI 分别投影。
