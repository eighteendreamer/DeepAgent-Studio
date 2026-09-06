# M20 监控调整记录：Phase B / 网络抓包推进过早，仍需回到目标一边界

- 日期：2026-09-06
- 记录性质：监控发现的新一轮阶段偏离，不是代码实现
- 监控范围：`14babeb..2b34616` 加上当前未提交移动端改动
- 当前结论：代码已经开始推进 UI snapshot 完整树，并且出现网络抓包入口；但按当前执行方案，目标一仍缺少真实 Emulator 证据，目标二和目标三都不应提前展开。

## 1. 观察到的新提交和未提交改动

### 新提交

```text
2b34616 【feat】Phase B：UI snapshot从summary改为返回完整树（382节点）
7e7f526 【docs】更新目标一验收报告至91分（新增全链路测试证据）
1525d16 【test】AppMobileService全链路真实设备集成测试
77b3911 【docs】目标一验收报告（USB真机闭环，模拟器AVD缺失阻塞）
4cb0006 【test】真实设备launch/terminate集成测试（启动设置→截图→强制停止）
5760734 【feat】stdout迁移为Vec<u8>支持二进制数据，截图使用exec-out避免行尾转换
38dbfce 【test】真实USB设备集成测试（probe→discovery→device_info）
8ffe595 【feat】ToolResolver支持ANDROID_HOME和常见SDK路径查找
```

### 当前未提交改动

```text
crates/deepagent-mobile-android/src/network_capture.rs
crates/deepagent-mobile-runtime/src/backend.rs
crates/deepagent-mobile-runtime/src/mobile_service.rs
crates/deepagent-mobile-protocol/src/events.rs
crates/deepagent-mobile-protocol/src/operations.rs
crates/deepagent-app-core/src/mobile_service.rs
apps/desktop/src-tauri/src/lib.rs
apps/desktop/src/api.ts
apps/desktop/src/types.ts
crates/deepagent-mobile-android/tests/real_device.rs
```

### 受影响方向

- UI snapshot 从 summary 切到完整树；
- 新增网络抓包链路与 Tauri/API 暴露；
- Android backend、runtime、protocol 同步扩展；
- 真实设备测试新增 network capture 相关用例。

## 2. 为什么这算阶段偏离

当前执行方案要求的顺序很明确：

1. 先把目标一做成真实验收闭环；
2. 目标二再做完整 UI 结构；
3. 目标三最后做通用网络观测。

而当前事实是：

- `docs/mobile-validation/goal-1-verification-report.md` 明确写着模拟器无 AVD 可用；
- 目标一虽然在 USB 真机上得到较强证据，但当前方案仍要求把真实 Emulator / USB 证据补齐后再进入后续阶段；
- 现在已经开始推进 `Phase B` 的完整 UI 树；
- 还开始新增 `mobile_start_network_capture` / `mobile_get_network_records` 这类目标三入口。

也就是说，新的工作面已经越过了当前应停留的阶段边界。

## 3. 证据摘要

### 3.1 目标一证据现状

`docs/mobile-validation/goal-1-verification-report.md` 记录了：

- 真实 USB 设备：vivo PFVM10 (OP522D)，Android 12；
- 设备发现、截图、启动/停止、AppMobileService 全链路有证据；
- 但模拟器无 AVD，可用证据缺失。

这说明目标一的证据并不完整到足以支撑“顺手进入后两阶段而不再回头检查”的程度。

### 3.2 目标二开始得过早

`2b34616` 已经把 UI snapshot 从 summary 改为完整树，甚至写到“382 节点”。

这本身不是坏事，但它属于目标二动作。按执行方案，目标二应在目标一边界收紧后再进入，而不是与目标三一起并行展开。

### 3.3 目标三已经开始露头

当前未提交改动已经出现：

- `network_capture.rs`
- `mobile_start_network_capture`
- `mobile_stop_network_capture`
- `mobile_get_network_records`

这等于直接开始目标三的通用网络观测入口，过早。

## 4. 只读验证结果

本轮监控执行了以下只读命令：

```text
git status --short --branch
git log --oneline --decorate -12
git show --stat --oneline 14babeb..HEAD
git diff --stat
git diff --name-status
git diff --check
cargo check -p deepagent-mobile-android --offline
cargo check -p deepagent-app-core --offline
cargo test -p deepagent-mobile-android --offline
cargo test -p deepagent-app-core mobile_service --offline
```

结果：

- `cargo check -p deepagent-mobile-android --offline` 通过；
- `cargo check -p deepagent-app-core --offline` 通过；
- `cargo test -p deepagent-mobile-android --offline` 通过，56 个单测通过，7 个真实设备测试被忽略；
- `cargo test -p deepagent-app-core mobile_service --offline` 通过，9 个测试里 1 个真实设备测试被忽略；
- `git diff --check` 通过；
- 当前工作区还有 `.qoder/rules/DeepAgent-Studio.md` 修改和 `docs/公开项目实际.txt` 未跟踪文件。

未执行：

- 未执行真实 Emulator 端到端验证；
- 未执行真实 USB 物理拔插自动化验证；
- 未执行 `cargo test --workspace --offline`；
- 未执行 `pnpm build`；
- 未执行对网络抓包链路的真实设备验收。

## 5. 当前阶段评分

按 `.qoder/rules/DeepAgent-Studio.md` 第 11.4 节评分：

| 项目 | 得分 | 依据 |
|---|---:|---|
| 代码与架构边界 | 15/20 | 仍沿统一 mobile runtime 前进，但目标二/三入口开始提前扩散 |
| 功能行为 | 12/25 | USB 真机链路有证据，Emulator 和网络抓包仍未完成真实验收 |
| 跨平台通用性 | 13/15 | 没看到明显项目特判 |
| 测试证据 | 10/20 | 真实 USB 测试与全链路测试存在，但后两阶段没有真实证据 |
| 安全与可恢复性 | 7/10 | 事件、artifact、取消边界持续补强，真实断开/重连和抓包恢复未完整验证 |
| 复查质量 | 8/10 | diff/check/test 已做，但阶段顺序开始失守 |
| **合计** | **65/100** | **已明显越过当前应冻结的阶段边界，但尚未达到继续扩展目标三的条件** |

## 6. 修复边界

下一轮唯一动作：

1. 停止继续推进目标二和目标三；
2. 回到目标一的缺口，补齐真实 Emulator 证据；
3. 把真实 USB / Emulator 的发现、页面显示、截图、启动/停止、断开/重连证据整理成一致的阶段报告；
4. 目标一没有完全闭环前，不再提交网络抓包入口和 UI 结构扩展。

## 7. 禁止事项

- 不把 `382` 节点的完整树当成可以跳过目标一的理由；
- 不把 network capture 的协议和测试当成已经完成通用抓包；
- 不继续加 iOS / Remote Mac / App SDK；
- 不让 Agent、前端、MCP 或 CLI 自己找设备或自己抓包；
- 不用项目日志、OCR、静态 JSON 或 fake backend 代替真实设备证据。

## 8. 下一轮唯一动作

停住扩展，先把目标一缺的 Emulator 证据补全，再决定是否允许进入目标二和目标三。
