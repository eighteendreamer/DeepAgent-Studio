# 已编辑文件列表功能设计文档

## 概述

在任务完成或手动停止时，UI 显示本次会话中已编辑的文件列表，包括文件路径、新增行数和删除行数。用户可以快速查看改动范围，并点击文件路径跳转到编辑器。

## 核心问题

### 1. 修改范围的定义

**问题**：什么算"本次会话的修改"？

#### 选项 A：基于 Git 工作区差异（当前实现）
- **定义**：与 Git HEAD 对比的所有未提交改动
- **优点**：
  - 实现简单，复用 `git diff --numstat HEAD` 命令
  - 与用户的 Git 工作流一致
  - 包含所有实际文件改动，不遗漏
- **缺点**：
  - 会包含会话开始前的未提交改动（如果用户工作区本来就有改动）
  - 无法区分"本次 AI 改的"和"用户自己改的"
  - 如果会话中途用户手动提交了一部分，这些改动会从列表中消失

**当前实现位置**：
- `crates/deepagent-runtime/src/loop_engine.rs:collect_edited_files()`
- 调用 `git diff --numstat HEAD` 获取工作区改动

#### 选项 B：基于会话内工具调用记录
- **定义**：只统计本次会话中 `Edit`/`Write`/`NotebookEdit` 工具成功调用的文件
- **优点**：
  - 精确归因：只记录 AI 实际修改的文件
  - 不受 Git 状态影响，即使没有 Git 仓库也能工作
  - 可以准确统计行数增删（工具调用时已知）
- **缺点**：
  - 实现复杂度高，需要在工具层记录每次修改
  - 如果用户会话中途手动编辑文件，这些改动不会被统计
  - 需要会话级持久化（目前已实现：`AgentKernel.edited_files`）

**实现成本**：
- 需要在 `Edit`/`Write`/`NotebookEdit` 工具执行成功后，计算行数差异并更新 `edited_files` HashMap
- 需要处理同一文件多次编辑的累加逻辑
- 需要处理文件删除、重命名等边缘情况

#### 选项 C：基于 Git 提交范围（未来可选）
- **定义**：会话开始时记录 HEAD，会话结束时与当前 HEAD 对比
- **优点**：
  - 只包含本次会话期间提交的改动
  - 适合"一次会话一次提交"的工作流
  - 可以准确反映已落盘的改动
- **缺点**：
  - 不包含未提交的工作区改动（用户可能还没提交）
  - 如果用户会话中提交了多次，需要累加多个 commit 的 diff
  - 需要会话开始时记录 HEAD SHA，增加状态管理复杂度

### 2. 时序与生命周期

**问题**：何时统计？何时清空？

#### 当前实现：会话级持久化
- **统计时机**：
  - 任务完成时：`RuntimeEvent::RunCompleted { edited_files }`
  - 手动停止时：`RuntimeEvent::RunCancelled { edited_files }`
  - 两者都在 `loop_engine.rs` 的终态发射点调用 `collect_edited_files()`

- **存储位置**：
  - `AgentKernel.edited_files: Arc<Mutex<HashMap<PathBuf, EditedFile>>>`
  - 会话级共享，所有 turn 累加

- **清空时机**：
  - 新会话创建时，`AgentKernel::new()` 初始化空 HashMap
  - 同一会话的多个 turn 不清空，累加修改

**已知问题**：
- 如果用户在会话中途手动提交代码，`git diff HEAD` 会丢失已提交的改动
- 如果用户在会话开始前就有未提交改动，这些改动会被错误归因到本次会话

### 3. 事件传递与 UI 同步

**问题**：如何可靠地将 `edited_files` 从后端传递到前端？

#### 当前实现：RuntimeEvent 直接传递
- **后端发射**：
  - `crates/deepagent-runtime/src/loop_engine.rs:646/691/700` 三处发射点
  - 通过 `RuntimeEventSink` 发送到 Tauri 事件总线
  - Tauri 后端：`apps/desktop/src-tauri/src/lib.rs:1960` 包装为 `RunEventEnvelope<RuntimeEvent>`

- **前端接收**：
  - `apps/desktop/src/api.ts:1354` 注册 `"chat://event"` 监听器
  - `unwrapRunPayload()` 根据 `run_id` 过滤事件
  - `apps/desktop/src/App.tsx:1695` 处理 `run_cancelled` 事件
  - `apps/desktop/src/App.tsx:1681` 处理 `run_completed` 事件

#### 已修复的时序竞争问题（commit fb6c4f2）
- **根因**：`session://completed` 事件先到达，立即触发 `finally` 块取消监听器；随后的 `RuntimeEvent::RunCancelled` 到达时监听器已被移除
- **修复**：在 `session://completed` resolve 前延迟 100ms，给后续事件到达窗口
- **代码位置**：`apps/desktop/src/api.ts:1390`

#### CLI JSONL 输出的限制
- **问题**：CLI 的 harness 协议投影（`crates/deepagent-harness-protocol/src/events.rs:440`）将 `RunCancelled` 映射为 `TurnInterrupted`，但 `TurnInterrupted` 不包含 `edited_files` 字段
- **影响**：CLI `--json` 模式下，`edited_files` 信息丢失
- **解决方案**：需要扩展 `HarnessEvent::TurnInterrupted` 增加 `edited_files` 字段（TODO）

### 4. UI 展示规则

**当前实现**：
- `apps/desktop/src/components/chat-timeline/EditedFilesList.tsx`
- 默认显示前 3 个文件，其余折叠
- 每个文件显示：路径、新增行数（绿色）、删除行数（红色）
- 点击文件路径跳转到编辑器（如果支持）

**边界情况**：
- `edited_files` 为空数组：不显示组件
- 只有 1-3 个文件：全部展开，无折叠按钮
- 超过 3 个文件：显示前 3 个 + "还有 X 个文件"折叠按钮

## 当前架构（选项 A：Git 工作区差异）

### 数据流

```
┌─────────────────────────────────────────────────────────────┐
│ 1. 会话开始                                                   │
│    AgentKernel::new() 初始化 edited_files: HashMap          │
└─────────────────────────────────────────────────────────────┘
                         │
                         ▼
┌─────────────────────────────────────────────────────────────┐
│ 2. 每个 Turn 运行                                             │
│    RuntimeEngine 持有 Arc<Mutex<HashMap>> 引用               │
└─────────────────────────────────────────────────────────────┘
                         │
                         ▼
┌─────────────────────────────────────────────────────────────┐
│ 3. 任务完成或手动停止                                          │
│    loop_engine.rs:collect_edited_files()                    │
│    执行 `git diff --numstat HEAD`                            │
│    解析输出，构造 Vec<EditedFile>                             │
└─────────────────────────────────────────────────────────────┘
                         │
                         ▼
┌─────────────────────────────────────────────────────────────┐
│ 4. 发射 RuntimeEvent                                         │
│    RunCompleted { edited_files } 或                         │
│    RunCancelled { edited_files }                            │
└─────────────────────────────────────────────────────────────┘
                         │
                         ▼
┌─────────────────────────────────────────────────────────────┐
│ 5. Tauri 事件总线                                             │
│    包装为 RunEventEnvelope<RuntimeEvent>                     │
│    发送到 "chat://event" 频道                                │
└─────────────────────────────────────────────────────────────┘
                         │
                         ▼
┌─────────────────────────────────────────────────────────────┐
│ 6. 前端接收                                                   │
│    api.ts 监听器 unwrapRunPayload() 过滤 run_id             │
│    延迟 100ms 等待事件到达                                    │
└─────────────────────────────────────────────────────────────┘
                         │
                         ▼
┌─────────────────────────────────────────────────────────────┐
│ 7. UI 渲染                                                    │
│    App.tsx 调用 finalize()                                   │
│    TurnFooter 显示 EditedFilesList 组件                      │
└─────────────────────────────────────────────────────────────┘
```

### 关键代码位置

| 模块 | 文件 | 行数 | 职责 |
|------|------|------|------|
| 后端会话状态 | `crates/deepagent-runtime/src/kernel.rs` | 375 | `AgentKernel.edited_files` 字段定义 |
| 后端会话状态 | `crates/deepagent-runtime/src/kernel.rs` | 533 | 将 `edited_files` 注入到 `RuntimeEngine` |
| 后端统计逻辑 | `crates/deepagent-runtime/src/loop_engine.rs` | 1144-1176 | `collect_edited_files()` 执行 Git diff |
| 后端事件发射 | `crates/deepagent-runtime/src/loop_engine.rs` | 646/691/700 | 三处发射 `RunCancelled` 或 `RunCompleted` |
| 后端事件类型 | `crates/deepagent-runtime/src/events.rs` | 428-449 | `RuntimeEvent` 枚举定义 |
| Tauri 桥接 | `apps/desktop/src-tauri/src/lib.rs` | 1960-1966 | 包装为 `RunEventEnvelope` 并发送 |
| 前端事件监听 | `apps/desktop/src/api.ts` | 1312-1322 | `unwrapRunPayload()` 过滤逻辑 |
| 前端时序修复 | `apps/desktop/src/api.ts` | 1390 | 延迟 100ms 等待后续事件 |
| 前端事件处理 | `apps/desktop/src/App.tsx` | 1681/1695 | 处理 `run_completed` 和 `run_cancelled` |
| 前端 UI 组件 | `apps/desktop/src/components/chat-timeline/EditedFilesList.tsx` | 全文件 | 文件列表渲染 |
| 前端 UI 集成 | `apps/desktop/src/components/chat-timeline/TurnFooter.tsx` | 调用位置 | 在 footer 显示文件列表 |

## 未来改进方向

### 短期（P1）

1. **扩展 harness 协议支持 edited_files**
   - 修改 `HarnessEvent::TurnInterrupted` 增加 `edited_files: Vec<EditedFile>` 字段
   - 更新 `project_runtime_event()` 投影逻辑，保留 `edited_files`
   - CLI JSONL 输出也能显示已编辑文件

2. **增加测试覆盖**
   - 集成测试：模拟一次会话修改多个文件，验证统计准确性
   - 前端测试：验证事件到达时序，确保 100ms 延迟足够
   - 边界测试：空列表、单文件、大量文件（100+）

3. **改进 UI 体验**
   - 文件路径支持相对路径显示（相对于 workspace root）
   - 支持按文件类型或目录分组
   - 支持批量复制文件路径

### 中期（P2）

4. **切换到选项 B：基于工具调用记录**
   - 在 `Edit`/`Write`/`NotebookEdit` 工具成功后，更新 `AgentKernel.edited_files`
   - 计算精确的行数差异（不依赖 Git）
   - 处理同一文件多次编辑的累加
   - 支持非 Git 仓库的文件修改统计

5. **增加会话级统计持久化**
   - 在 `sessions` 表增加 `edited_files_json` 字段
   - 会话结束时持久化到数据库
   - 历史会话也能查看已编辑文件列表

### 长期（P3）

6. **支持选项 C：基于 Git 提交范围**
   - 会话开始时记录 `HEAD` SHA
   - 会话结束时执行 `git diff <start_sha>..HEAD`
   - 支持用户选择统计模式（工作区 vs 提交范围）

7. **支持文件内容预览**
   - 点击文件时显示 diff 预览（类似 GitHub PR）
   - 支持逐行查看新增/删除内容
   - 支持一键回滚某个文件的修改

## 已知问题与限制

### 1. Git 状态依赖
- **问题**：如果用户会话中途提交代码，已提交的改动会从列表中消失
- **影响**：统计不准确，用户困惑
- **解决方案**：切换到选项 B（基于工具调用记录）

### 2. 非 Git 仓库支持
- **问题**：当前实现完全依赖 `git diff`，非 Git 仓库无法工作
- **影响**：限制使用场景
- **解决方案**：切换到选项 B（基于工具调用记录）

### 3. 多用户并发编辑
- **问题**：如果用户在会话运行时手动编辑文件，这些改动会被错误归因到 AI
- **影响**：统计不准确
- **解决方案**：切换到选项 B（基于工具调用记录）

### 4. CLI JSONL 输出缺失
- **问题**：harness 协议投影丢失 `edited_files` 字段
- **影响**：CLI 用户无法获取文件列表
- **解决方案**：扩展 `HarnessEvent::TurnInterrupted` 协议

### 5. 文件重命名处理
- **问题**：`git diff` 的重命名检测可能不准确，显示为"删除 + 新增"
- **影响**：行数统计虚高
- **解决方案**：
  - 短期：使用 `git diff --find-renames`
  - 长期：切换到工具调用记录，精确跟踪重命名

## 测试验证

### 手动测试场景

1. **基本场景：单文件编辑**
   - 启动会话，修改一个文件
   - 手动停止或等待完成
   - 验证：显示 1 个文件，行数正确

2. **多文件编辑**
   - 启动会话，修改 5 个文件
   - 验证：显示前 3 个 + "还有 2 个文件"折叠按钮
   - 点击展开，验证所有文件显示

3. **跨 Turn 累加**
   - 第一个 Turn 修改 2 个文件
   - 第二个 Turn 修改另外 3 个文件
   - 验证：第二个 Turn 完成时显示 5 个文件

4. **空列表**
   - 启动会话，只执行读操作（`Read`/`Glob`/`Grep`）
   - 手动停止
   - 验证：不显示文件列表组件

5. **时序竞争修复验证**
   - 启动会话，修改文件
   - 立即手动停止（不等待完成）
   - 打开浏览器控制台，查看 `[DEBUG unwrapRunPayload]` 日志
   - 验证：`run_cancelled` 事件 `matches: true`，文件列表正常显示

### 自动化测试（TODO）

```rust
#[tokio::test]
async fn edited_files_accumulate_across_turns() {
    // 模拟两个 turn，每个 turn 编辑不同文件
    // 验证第二个 turn 结束时，edited_files 包含两个 turn 的所有文件
}

#[tokio::test]
async fn edited_files_empty_for_read_only_run() {
    // 模拟一个只执行读操作的 turn
    // 验证 edited_files 为空
}

#[tokio::test]
async fn git_diff_integration() {
    // 在真实 Git 仓库中，修改文件，执行 collect_edited_files()
    // 验证返回的文件路径和行数与 `git diff --numstat HEAD` 一致
}
```

## 参考提交

- `1261172` - 【feat】任务完成时显示已编辑的文件列表
- `4c2fb5d` - 【feat】手动停止任务时显示已编辑文件列表
- `c24a30c` - 【fix】修复手动停止时 edited_files 为空的问题（会话级持久化）
- `fb6c4f2` - 【fix】修复手动停止时 edited_files 事件丢失问题（时序竞争）

## 总结

当前实现（选项 A）基于 Git 工作区差异，实现简单但存在归因不准确的问题。中期建议切换到选项 B（基于工具调用记录），实现精确归因和非 Git 仓库支持。长期可以考虑选项 C（基于 Git 提交范围）作为可选模式。

核心权衡：
- **实现复杂度 vs 准确性**：Git diff 简单但不准确，工具记录准确但复杂
- **实时性 vs 一致性**：工作区差异实时但不稳定，提交范围一致但滞后
- **通用性 vs Git 集成**：工具记录通用但失去 Git 语义，Git diff 依赖 Git 但语义清晰
