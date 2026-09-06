# M19 监控调整记录：M18 后续修复方向基本回正，但仍未完成目标一真实验收

- 日期：2026-09-06
- 记录性质：监控发现的新提交审查与阶段约束，不是代码实现
- 监控范围：`5f2088a..14babeb`
- 当前结论：本轮修改大体围绕目标一启动链路和证据链修复，没有继续扩 iOS、Remote Mac 或 App SDK；但仍缺少真实 USB 真机和真实 Android Emulator 验收，因此不能宣称目标一完成。

## 1. 观察到的新提交

```text
14babeb 【test】添加移动端全链路集成测试（discovery→event→DTO→screenshot）
d69943a 【fix】screenshot写入真实文件，不再返回memory://虚拟路径
4435176 【fix】同步前端Mobile DTO类型与Rust DTO字段
5c56124 【fix】ToolResolver.resolve()实际搜索PATH，probe不再永远返回false
6e535c4 【feat】接通移动端事件通道，MobileEvent投影到Tauri事件系统
b3e2ba8 【refactor】消除移动端backend双实例，统一为Arc共享
```

主要改动集中在：

- `crates/deepagent-app-core/src/mobile_service.rs`
- `crates/deepagent-mobile-runtime/src/mobile_service.rs`
- `crates/deepagent-mobile-android/src/backend.rs`
- `crates/deepagent-mobile-android/src/tool_resolver.rs`
- `apps/desktop/src-tauri/src/lib.rs`
- `apps/desktop/src/types.ts`
- `crates/deepagent-app-core/tests/golden_trace.rs`
- `docs/mobile-devtools-runtime-execution-plan.md`

## 2. 与当前执行方案的对齐情况

本轮比 M18 前的路线更接近正确方向：

- 修复了 Android backend 双实例问题，向统一 runtime backend 边界靠拢；
- 增加了 MobileEvent 到 Tauri 事件系统的投影；
- 修复了 `ToolResolver.resolve()` 不搜索 PATH 的问题；
- 修复了截图 artifact 不应继续使用 `memory://` 虚拟路径的问题；
- 增加了 discovery → event → DTO → screenshot 的集成测试。

这些都属于目标一前置链路修复，方向基本正确。

## 3. 仍然存在的问题

### 3.1 仍未完成真实设备验收

新增的 `full_chain_discovery_to_dto` 使用的是 `FakeAndroidBackend`，只能证明内部链路在 fake backend 下可运转。

它不能证明：

- 系统启动后能发现真实 USB 真机；
- 系统启动后能发现真实 Android Emulator；
- 真机或模拟器页面能真实显示；
- `mobile_screenshot` 产物来自真实设备；
- USB 插拔、授权变化、模拟器启动/停止能被系统内部 discovery loop 发现。

因此，目标一仍然没有完成。

### 3.2 新提交缺少阶段验证报告

M18 后连续提交了 6 个修复 / 测试提交，但 `docs/mobile/validation/` 下没有新增对应的 M19 验证报告。

按照 `.qoder/rules/DeepAgent-Studio.md` 第 11 节，每一轮必须记录本轮目标、修改文件、依据来源、实际命令、自动化测试结果、真实设备/模拟器信息、三项目标得分、失败证据和下一轮唯一动作。

当前缺少这份正式记录，后续开发容易再次把 fake 集成测试误读为真实目标一通过。

### 3.3 仍不能进入目标二和目标三

虽然本轮修复了若干目标一基础链路，但目标一真实验收尚未完成。

因此不得继续推进：

- 完整 UI Tree；
- 网络抓包；
- iOS；
- Remote Mac；
- App SDK；
- 新 React 调试页面。

## 4. 只读验证结果

本轮监控执行了以下只读命令：

```text
git status --short --branch
git log --oneline --decorate -8
git show --stat --oneline 5f2088a..HEAD
git diff --check 5f2088a..HEAD
```

结果：

- 新增 6 个移动端相关提交；
- `git diff --check 5f2088a..HEAD` 通过；
- 当前工作区仍有 `.qoder/rules/DeepAgent-Studio.md` 修改和 `docs/公开项目实际.txt` 未跟踪文件。

```text
cargo check -p deepagent-app-core --offline
```

结果：通过。

```text
cargo test -p deepagent-app-core full_chain_discovery_to_dto --offline
```

结果：通过，1 个测试通过。

```text
cargo test -p deepagent-app-core mobile_service --offline
```

结果：通过，8 个测试通过。

未执行：

- 未执行真实 USB 真机测试；
- 未执行真实 Android Emulator 测试；
- 未执行桌面端手动启动验收；
- 未执行 `pnpm build`；
- 未执行完整 `cargo test --workspace --offline`。

## 5. 当前阶段评分

按 `.qoder/rules/DeepAgent-Studio.md` 第 11.4 节评分：

| 项目 | 得分 | 依据 |
|---|---:|---|
| 代码与架构边界 | 16/20 | backend 双实例和事件投影方向已修正，但仍需确认 Agent 层是否完全收敛到统一边界 |
| 功能行为 | 13/25 | fake 集成链路通过，真实 USB/Emulator 未验收 |
| 跨平台通用性 | 13/15 | 未发现项目硬编码或业务特判 |
| 测试证据 | 8/20 | app-core 检查和相关测试通过，但没有真实设备证据 |
| 安全与可恢复性 | 5/10 | cancellation、event、artifact 有改善，断开/重连和权限闭环未实测 |
| 复查质量 | 6/10 | 监控已执行 diff/status/test 检查，但 Qoder 未提交正式阶段报告 |
| **合计** | **58/100** | **目标一真实证据缺失，最高不得超过 59 分** |

## 6. 修复边界

下一轮唯一允许动作：

1. 补正式 M19 验证报告；
2. 使用真实 Android Emulator 和真实 USB 真机跑目标一；
3. 报告设备型号、系统版本、ADB 版本、连接方式、启动步骤、截图 artifact、事件日志、断开/重连证据；
4. 明确 fake 集成测试只作为内部链路测试，不作为目标一完成证据。

## 7. 禁止事项

- 不把 `FakeAndroidBackend` 集成测试写成真实设备验收；
- 不把截图写入文件等价为真实页面显示；
- 不继续新增 UI Inspector、网络抓包、iOS、Remote Mac、App SDK；
- 不新增项目包名、页面、接口、控件文本的特判；
- 不让 Agent、前端、MCP 或 CLI 自己扫描 USB 或调用任意 shell；
- 不绕过 `MobileService -> MobileRuntime -> Backend -> DeviceRegistry -> MobileEvent` 主链路。

## 8. 下一轮唯一动作

停止继续堆代码，先做真实目标一验收。

目标一通过前，所有后续开发只能围绕真实设备发现、页面显示、截图 artifact、启动/停止、断开/重连证据补齐展开。
