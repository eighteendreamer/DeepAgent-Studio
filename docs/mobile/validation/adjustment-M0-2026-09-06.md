# M0 阶段调整记录（Git 路线监控）

日期：2026-09-06
监控对象：工作区 `G:\Code_Warehouse\DeepAgent-Studio` 的 Git 修改
基线提交：`0ca48cd 【docs】冻结移动端调试工程实施方案`
记录性质：监控发现的阶段阻断与后续修复边界，不是代码实现方案变更

## 1. 结论

当前修改仍处于冻结方案规定的 M0 范围内，暂未发现提前加入 iOS、网络抓包、React Inspector 或项目专用适配的证据。

但是，M0 当前不能视为完成，也不能进入 M1。新增移动端 crate 在当前工作树上无法通过编译，且没有提交 M0 验证报告、系统启动设备发现证据或真实设备/模拟器证据。后续执行必须先恢复 M0 的可编译、可测试状态。

## 2. 观察到的 Git 修改

当前工作树包含：

- 根 `Cargo.toml`：注册 `deepagent-mobile-core`、`deepagent-mobile-protocol`、`deepagent-mobile-runtime`、`deepagent-mobile-android`。
- 根 `Cargo.lock`：随 workspace 依赖解析产生的变更。
- 四个未提交 mobile crate，包含领域模型、协议 DTO、运行时 registry/operation、Android ADB 解析器和 fake backend。
- `.qoder/rules/DeepAgent-Studio.md`：已有用户规则改动，监控只读，不覆盖。
- `docs/公开项目实际.txt`：已有未跟踪文件，监控只读，不纳入本阶段。

## 3. 验证证据

已执行：

```text
git diff --check
```

结果：通过；仅出现 Git 关于规则文件换行格式的提示。

已执行：

```text
cargo check -p deepagent-mobile-android --offline
```

结果：失败，阻断原因包括：

- `crates/deepagent-mobile-runtime/src/backend.rs` 使用 `OperationContext`，但当前模块没有可解析的导入。
- `deepagent_mobile_protocol::InstallRequest` 在协议 crate 中未导出，导致 runtime trait 无法解析。
- `deepagent-mobile-protocol`、`deepagent-mobile-runtime` 存在未使用导入警告；在严格 lint 下会继续阻断。

这些是当前代码契约没有闭合的事实，不代表可以用临时 re-export、任意兼容分支或跳过检查来掩盖。

## 4. 与冻结方案的对照

### 已对齐

- crate 数量和首轮目录边界与“工程冻结决策（直接开工版）”一致。
- 操作模型使用结构化 `MobileOperationKind`，没有发现 `execute_mobile(command: String)` 形式的任意 shell 入口。
- 已建立 `DeviceRegistry`、`MobileBackend`、取消上下文和 Android ADB parser 的基础边界。
- 当前未发现项目包名、页面文本、接口 URL、固定坐标或被测项目源码特判。

### 当前不满足

- M0 必须可编译、可运行相关测试；当前 `cargo check` 失败。
- M0 需要完成取消、超时和错误契约的可验证测试；当前 `OperationContext` 只有 deadline 字段，尚未看到完整的超时执行约束。
- M0 需要可重复的 ADB parser/fake executable 证据；当前看到 parser 和 fake backend，但没有验证报告确认 fake executable 或受控进程边界已经闭合。
- `AdbBackend::list_devices` 及其他真实 ADB 操作仍返回 `BackendUnavailable`。这在 M0 可以作为未实现边界，但必须明确不能据此宣称目标一已完成。
- 当前没有 `docs/mobile/validation/M0-2026-09-06.md` 形式的阶段测试报告。

## 5. 允许的修复范围

下一轮唯一动作：恢复 M0 的可编译、可测试状态，并补齐 M0 证据。

允许范围：

1. 修复 mobile crate 之间已有类型和模块导出契约。
2. 让 `cargo check`、相关 crate 单元测试和严格 lint 在离线环境下可执行。
3. 用受控 fixture/fake executable 验证 `adb devices -l` 解析、设备状态映射、取消、超时和错误路径。
4. 生成 M0 阶段报告，明确真实设备/模拟器验证尚未进入本轮时必须标记为未完成。

## 6. 本阶段禁止事项

- 不进入 M1，不新增 Tauri command、React 页面、Agent/MCP 工具或 UI Inspector。
- 不新增 `deepagent-mobile-ios`、`deepagent-mobile-sdk`、`deepagent-mobile-network` 或 Remote Mac。
- 不让 Agent、MCP、CLI 或前端自行调用 `adb devices`、`emulator -list-avds`、`simctl list` 或任意 shell。
- 不添加项目包名、页面、接口、控件文本、固定坐标或测试项目专用分支。
- 不用 fake backend、截图、OCR、静态 JSON 或退出码伪造目标一、目标二、目标三的真实完成证据。
- 不通过放宽 lint、删除测试、吞掉错误或临时复制第二套链路来“修复”当前阻断。

## 7. 当前评分（监控检查分，非阶段通过分）

| 项目 | 得分 | 依据 |
|---|---:|---|
| 代码与架构边界 | 17/20 | 四个 crate 和结构化边界基本对齐，但跨 crate 契约尚未闭合 |
| 功能行为 | 4/25 | 真实 ADB 设备列表和操作尚未实现，当前不能验证目标一 |
| 跨平台通用性 | 12/15 | 领域模型保持平台无关，当前仍处于 Android 首轮范围 |
| 测试证据 | 4/20 | 有单元测试草稿，但编译失败且没有阶段报告/真实设备证据 |
| 安全与可恢复性 | 5/10 | 有取消和结构化操作意图，超时、权限和断开恢复尚未形成证据 |
| 复查质量 | 8/10 | 已完成 Git 状态、diff 和编译检查；Qoder 尚未提供本轮回顾报告 |
| **合计** | **50/100** | **低于 90，禁止进入下一阶段** |

## 8. 监控后续判定

后续 Git 修改只有在满足以下条件后，才可判定 M0 通过并允许进入下一阶段评审：

- mobile workspace 可编译；
- 相关测试和严格 lint 通过；
- 有 M0 阶段报告和命令输出证据；
- 没有 Agent 自行发现设备或项目专用适配；
- 报告明确列出尚未完成的真实设备、UI 树和网络目标，不把 fixture 结果写成真实能力。
