# 目标二验收报告：完整 UI 结构

> 验收日期：2026-09-06
> 验收人：程序员Eighteen + Qoder Agent
> 执行方案：`docs/mobile-devtools-runtime-execution-plan.md`

## 1. 验收环境

| 项目 | 值 |
|------|-----|
| 真实设备 | vivo PFVM10 (OP522D) |
| Android 版本 | 12 (SDK 31) |
| 连接方式 | USB, serial `NVPNM7CUWKT4NZPZ` |
| adb 路径 | `C:\Users\32734\platform-tools\adb.exe` |
| adb 版本 | v1.0.41 |
| OS | Windows 10, build 26200 |

## 2. 完成标准对照（section 6.1）

| # | 标准 | 状态 | 证据 |
|---|------|------|------|
| 1 | 一次 snapshot 返回完整 hierarchy | ✅ 通过 | `real_ui_snapshot_returns_full_tree` 测试，67 nodes, max_depth=12 |
| 2 | 节点有稳定 snapshot_id / node_id | ✅ 通过 | snapshot_id=`snap-android-NVPNM7CUWKT4NZPZ-<uuid>`, node_id=`ui-0`..`ui-66` |
| 3 | 节点属性、父子关系、数量和边界可验证 | ✅ 通过 | `parse_uiautomator_builds_hierarchy` 单元测试验证 parent_id/children/bounds |
| 4 | 节点查询可复现 | ✅ 通过 | 多次运行返回一致结构（同屏幕节点数稳定为 67） |
| 5 | snapshot 过期后安全拒绝 | ✅ 通过 | SnapshotStore 已实现 TTL 过期机制 |

## 3. 关键修复

### 3.1 UI 树层级丢失（根因修复）

**问题**：Phase B 初始实现使用 `split("<node")` 扁平化解析 uiautomator XML，所有节点 `parent_id=None`、`children=[]`，`max_depth=1`。

**根因**：`parse_uiautomator_output` 丢失了 XML 嵌套层级信息。

**修复**：改为栈追踪方式逐标签解析（commit `3162992`）：
- 维护 `parent_stack: Vec<usize>` 追踪当前嵌套路径
- `<node ...>` 入栈，`</node>` 出栈，`<node ... />` 不入栈
- 每个节点设置正确的 `parent_id` 和父节点 `children` 列表

**验证**：真机 max_depth 从 1 提升到 12。

## 4. 真实设备证据

### 4.1 UI Snapshot 完整树
```
test real_ui_snapshot_returns_full_tree ... ok
UI snapshot: id=snap-android-NVPNM7CUWKT4NZPZ-1b6d1c47-... nodes=67 max_depth=12 root=ui-0
```

### 4.2 层级结构验证
- 根节点 `ui-0`：无 parent，children 包含直接子节点
- 叶子节点：parent_id 指向正确父节点，children 为空
- 中间节点：既有 parent_id 也有 children
- 所有节点有 bounds（非零宽高）

## 5. 自动化测试汇总

| 测试套件 | 通过 | 失败 |
|----------|------|------|
| `deepagent-mobile-android` 单元测试 | 63 | 0 |
| `deepagent-mobile-android` 真实设备测试 | 1 (UI snapshot) | 0 |
| `cargo fmt --check` | 通过 | - |
| `cargo clippy -D warnings` | 通过 | - |
| `cargo test --workspace` | 2500+ | 0 |

## 6. 评分

| 维度 | 得分 | 说明 |
|------|------|------|
| 代码与架构边界 | 20/20 | 复用 uiautomator dump 通用能力，无第二套链路 |
| 功能行为 | 22/25 | 完整层级获取，节点属性/父子/边界正确；节点数受当前屏幕内容影响 |
| 跨平台通用性 | 15/15 | 零项目特判，使用 Android 公开 uiautomator 能力 |
| 测试证据 | 17/20 | 63 单元测试 + 真机测试 + 层级验证测试；模拟器未验证 |
| 安全与可恢复性 | 10/10 | 无权限变更，临时文件清理正确 |
| 复查质量 | 10/10 | 发现并修复了层级丢失根因，diff 已检查 |
| **总分** | **94/100** | |

## 7. 未验证项

- 模拟器证据缺失（无 AVD 可用）
- snapshot 过期拒绝通过单元测试覆盖，未做真实等待验证

## 8. Commit 记录

- `2b34616` — Phase B：UI snapshot从summary改为返回完整树
- `3162992` — 【fix】UI snapshot解析器建立正确父子层级（max_depth从1修复为12）

## 9. 结论

目标二在真实 USB 真机上已完整闭环：完整 hierarchy 获取、稳定 ID、正确父子关系、节点属性和边界。总分 94/100。
