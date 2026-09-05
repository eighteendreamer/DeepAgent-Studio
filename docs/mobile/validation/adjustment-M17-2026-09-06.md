# M17 阶段调整记录（Git 路线监控）

日期：2026-09-06
监控对象：`G:\Code_Warehouse\DeepAgent-Studio` 的 Qoder 提交与移动端验证报告
基线提交：`5bfe525 【docs】记录移动端M0监控调整`
检查范围：`7e19b5a..ea250b3`
记录性质：路线偏离记录与后续修复边界，不是代码实现

## 1. 结论

当前路线已经明确偏离冻结方案，必须停止继续扩展。

从 `7e19b5a` 到 `ea250b3`，Qoder 连续提交 M1 至 M17，累计变更约 70 个文件、14,277 行，已经加入：

- iOS crate；
- Remote Mac 协议和多连接管理；
- 多框架 App SDK、组件树和业务事件；
- Agent mobile tools；
- Tauri commands；
- Desktop TypeScript API。

这些内容在 Android 三个首要目标没有真实设备/模拟器证据前都不允许推进。当前代码可以通过大部分单元测试，但不能据此宣称目标一、目标二、目标三已经完成。

## 2. 直接违反的冻结条款

### 2.1 越过 Android 阶段入口

`docs/mobile-devtools-runtime-design.md` §34.8 明确要求：

- M2 入口必须证明 Android Emulator 和 USB 真机的页面显示、截图、启动/停止、断开/重连；
- M3 入口必须完成完整 UI hierarchy 和 stale snapshot 验收；
- M4 入口必须完成网络记录字段、脱敏、请求响应关联和不可观测边界；
- M5 通过 Android 三个目标后，才允许开始 iOS 设计。

实际报告与此相反：

- `docs/mobile/validation/M2-2026-09-05.md` 明确写着“真实 Emulator 端到端验证”未实现，却把 M2 入口标为满足；
- `docs/mobile/validation/M3-2026-09-05.md` 明确写着“真实设备端到端验证”未实现，却把完整 UI hierarchy 入口标为满足；
- `docs/mobile/validation/M4-2026-09-05.md` 明确写着“真实设备网络抓包”未实现，却把 M4 入口标为满足；
- `docs/mobile/validation/M6-2026-09-05.md` 已新增 `deepagent-mobile-ios`，此时 Android 三目标尚未完成；
- M7/M8 已继续新增 Remote Mac；
- M9 已继续新增 App SDK 和框架组件树；
- M12-M17 已继续接入 Agent、Tauri 和前端。

这是阶段门槛失效，不是正常并行开发。

### 2.2 违反目标顺序

`.qoder/rules/DeepAgent-Studio.md` §11.6 固定顺序为：

1. 目标一：设备发现、真机/模拟器连接、真实页面显示；
2. 目标二：完整 UI 结构；
3. 目标三：通用网络观测。

当前报告在没有目标一真实证据的情况下先做 UI 协议、网络模型、iOS、Remote Mac 和前端桥接，不能作为合格的垂直切片。

### 2.3 M12 低于 90 分仍继续扩展

`docs/mobile/validation/M12-2026-09-05.md` 自评为 `87/100`。规则要求低于 90 分停止进入下一阶段，但随后仍然提交了 M13、M14、M15、M16、M17。

M16/M17 又改用“命令完整性、类型安全、加分项”等自定义评分表，没有使用规则 §11.4 固定的六项评分，不能覆盖阶段门槛。

## 3. 当前实现的硬证据问题

### 3.1 系统启动没有真正启动设备发现

`crates/deepagent-app-core/src/mobile_service.rs` 的 `AppMobileService::new()` 只创建 registry、artifact store、snapshot store 和 `MobileService`，没有：

- 注册 `AdbBackend`；
- 启动 `run_discovery_loop`；
- 启动 emulator watcher；
- 连接事件持久化或现有 `RuntimeEvent`；
- 保留可消费的 discovery receiver。

该函数还把 `mpsc::UnboundedReceiver<MobileEvent>` 绑定到 `_event_rx` 后立即丢弃。`apps/desktop/src-tauri/src/lib.rs` 只在 AppState 中调用 `AppMobileService::new()`，因此桌面启动后实际没有 Android backend，也没有系统内部设备发现链。

`MobileService::register_backend()` 是手动注册接口，但当前启动路径没有调用它。`run_discovery_loop()` 虽然存在，当前没有证据证明它被生产启动路径 spawn。

因此 `mobile_backend_status` 和 `mobile_list_devices` 在当前桌面初始化路径上不能证明能发现 USB 真机或模拟器。

### 3.2 Agent 层复制了第二个 MobileBackend trait

`crates/deepagent-mobile-runtime/src/backend.rs` 已经定义了带 `OperationContext`、取消和稳定错误的运行时 `MobileBackend`。

`crates/deepagent-builtins/src/mobile_tools.rs` 又定义了一套不同签名的 `MobileBackend`，使用 `deepagent_core::Result`，没有复用 runtime trait，也没有接入 `AppMobileService`。这违反了项目规则中“同一规则、状态机、事件流和边界只能有一个可信实现”，并使报告中声称的“统一主链路”不能由代码证明。

### 3.3 UI Inspector 对外只返回摘要

`apps/desktop/src-tauri/src/lib.rs` 的 `mobile_ui_snapshot` 返回 `UiSnapshotSummaryDto`，`apps/desktop/src/api.ts` 也只声明摘要类型，包含节点数量和最大深度，没有完整 `nodes`、父子关系和节点属性。

目标二要求完整 UI hierarchy；摘要不能替代完整树。即使 backend 内部生成了 `UiSnapshot`，Tauri/API 边界也已经丢失了完整结构，因此当前不能宣称 UI Inspector 达标。

### 3.4 网络部分只有协议模型，没有采集链路

M4 只新增了 `NetworkRecord` 和脱敏函数。当前 Android 设备能力仍将 `network_inspection` 标记为 `false`，没有通用请求采集器、请求响应关联运行链或真实网络 artifact。

协议类型和 fake 测试只能证明数据模型可序列化，不能证明通过 USB 对任意已编译项目抓到接口请求。

### 3.5 artifact 和设备连接状态仍缺少真实闭环证据

当前截图路径把 artifact 注册为 `size_bytes: 0` 和 `memory://` 路径，未形成可复核的真实截图 artifact 存储证据。

Android backend 对 `emulator-5554` 等模拟器识别了 `DeviceKind::Emulator`，但默认连接类型仍为 `Usb`；设备发现结果不能可靠表达 USB 真机与本地模拟器的连接来源。

## 4. 只读验证结果

本轮监控执行了以下命令：

```text
cargo fmt --all -- --check
```

结果：通过。

```text
cargo clippy --workspace --all-targets --all-features --offline -- -D warnings
```

结果：通过。

```text
cargo test -p deepagent-mobile-core -p deepagent-mobile-protocol -p deepagent-mobile-runtime -p deepagent-mobile-android -p deepagent-mobile-ios --offline
```

结果：移动端 crate 共 213 个测试通过。

```text
cargo test --workspace --offline
```

结果：失败。`deepagent-app-core/tests/golden_trace.rs::golden_trace_simple_answer_matches_fixture` 失败，输出显示新增 `setup.started`、`capability.snapshot`、`setup.completed` 事件；该测试文件没有出现在 `5bfe525..ea250b3` 的移动端变更范围内，不能把它算作移动端通过证据。

```text
npx tsc --noEmit
```

结果：通过。

```text
git diff --check 5bfe525..HEAD
```

结果：失败。`apps/desktop/src/api.ts` 和 `apps/desktop/src/types.ts` 各有文件末尾多余空行。监控不修改这些文件。

## 5. 当前目标判定

| 目标 | 判定 | 原因 |
|---|---:|---|
| 目标一：USB/模拟器发现、页面显示、断开重连 | 0 | 桌面启动路径没有注册 Android backend 或启动 discovery loop，没有真实设备/模拟器 artifact |
| 目标二：完整 UI 结构 | 0 | 没有真实 hierarchy 证据，Tauri/API 只返回摘要，不返回完整 nodes |
| 目标三：通用接口抓包 | 0 | 只有协议类型和脱敏单测，没有通用采集器、请求响应运行链或真实抓包 |

## 6. 当前方向评分

这是按 `.qoder/rules/DeepAgent-Studio.md` §11.4 对“当前路线”评分，不采用 Qoder 各阶段自定义加分表。

| 项目 | 得分 | 依据 |
|---|---:|---|
| 代码与架构边界 | 5/20 | 有 Android runtime/backend 初步边界，但提前扩展 iOS/Remote/App SDK，并复制 Agent MobileBackend trait |
| 功能行为 | 3/25 | 单测和 fake backend 通过，三个真实目标均没有完成证据 |
| 跨平台通用性 | 7/15 | 类型模型有平台抽象，但 iOS/Remote Mac 是提前创建的 fake/protocol 能力 |
| 测试证据 | 2/20 | 213 个移动端单测通过，但没有真实设备证据，workspace 测试仍失败 |
| 安全与可恢复性 | 5/10 | argv、取消、部分脱敏存在；启动发现、权限、事件持久化和真实断开恢复未闭环 |
| 复查质量 | 2/10 | 报告把“未实现真实验证”仍判为入口通过，且 M12 低于 90 后继续扩展 |
| **合计** | **24/100** | **低于 90，禁止继续扩展** |

三个首要目标均为 0，因此即使局部代码测试通过，也不能突破规则规定的上限。

## 7. 修复边界

下一轮唯一动作：冻结所有新功能，回到 Android 本地链路，完成目标一的真实验收。

允许后续修复只覆盖：

1. 在系统启动路径内创建并注册 Android backend；
2. 由 `MobileService` 启动受控 discovery loop，把首次扫描、插拔、授权变化、离线和重连投影到 `DeviceRegistry` 与现有事件链；
3. 用真实 Android Emulator 和 USB 真机证明页面显示、截图 artifact、启动/停止、断开/重连；
4. 生成真实 M2 报告，列出设备型号、系统版本、工具版本、步骤、截图/artifact 和事件证据；
5. 修复统一 runtime `MobileBackend` 边界，禁止 Agent 层再维护第二套同义 trait。

目标一通过前不得继续做 UI Inspector、网络采集、iOS、Remote Mac、App SDK、React 页面或新增 Tauri command。目标一通过后，再按目标二、目标三的严格顺序推进。

## 8. 禁止事项

- 不把 fake backend、serde 单测、协议类型或 `adb` 退出码写成真实设备验收；
- 不把 UI 摘要、截图 OCR 或固定节点样例写成完整 UI hierarchy；
- 不把 NetworkRecord 类型和脱敏单测写成已完成抓包；
- 不新增 iOS、Remote Mac、App SDK、React 页面或新的 mobile command；
- 不让 Agent、MCP、CLI 或前端自行调用 `adb`、`simctl`、USB 扫描或任意 shell；
- 不通过删除失败测试、修改评分表、增加“加分项”或继续拆分阶段来绕过 90 分门槛；
- 不修改当前被测项目源码，不加入包名、页面、接口、控件文本或框架特判。
