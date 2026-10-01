# DAG 可视化 vs 现有工具展示流程 - 关系说明

> **核心答案**：这是一个**增强/升级**，不是替换。DAG 可视化是针对 `plan_execute` 工具的**专门增强展示**，与现有的单工具展示（`ProcessToolRow`）**共存**。

---

## 1. 现有工具展示方式（保留）

### 1.1 当前实现：ProcessToolRow

**适用场景**：单个工具调用（如 `Read`, `Write`, `Bash`, `task` 等）

**展示方式**：
```
┌─────────────────────────────────────────────────┐
│ 🔧 Write  ● Done  120ms  src/main.rs           │
│   └─ (点击展开显示详细输出)                      │
└─────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────┐
│ 💻 Bash  ⚙️ Running  npm install               │
└─────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────┐
│ 🤖 task  ● Done  2.3s  review code             │
│   └─ Found 3 issues in backend                  │
└─────────────────────────────────────────────────┘
```

**特点**：
- ✅ 单行显示（折叠状态）
- ✅ 显示状态（Running / Done / Error）
- ✅ 显示耗时
- ✅ 点击展开查看详细输出
- ✅ 适用于所有工具调用

---

## 2. DAG 可视化（新增）

### 2.1 专门针对：plan_execute 工具

**适用场景**：**仅当调用 `plan_execute` 工具时**才显示 DAG 卡片

**展示方式**：
```
┌─────────────────────────────────────────────────┐
│ 🔄 DAG Execution: Authentication System    [▲]  │
├─────────────────────────────────────────────────┤
│ █████████████████░░░░░░░░░░░░░ 60% (3/5)       │
│                                                  │
│ Phase: Review                        [2/2 ✓]   │
│ ├─ review-backend                    [✓ Done]   │
│ └─ review-frontend                   [✓ Done]   │
│                                                  │
│ Phase: Implementation                [1/2 ✓]    │
│ ├─ fix-backend                       [✓ Done]   │
│ └─ fix-frontend                      [⚙️ Running]│
│                                                  │
│ [View Graph] [View Logs]                       │
└─────────────────────────────────────────────────┘
```

**特点**：
- ✅ 显示整个 DAG 的全局进度
- ✅ 按 Phase 分组展示节点
- ✅ 实时更新所有节点状态
- ✅ 显示依赖关系
- ✅ 可选查看拓扑图

---

## 3. 共存方式

### 3.1 对话流示例

```
┌─────────────────────────────────────────────────┐
│ User                                             │
│ Implement authentication with review phases     │
└─────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────┐
│ Assistant                                        │
│ I'll create a plan...                           │
│                                                  │
│ [Tool Call] plan_execute                        │  ← 触发 DAG 卡片
│ ┌─────────────────────────────────────────────┐ │
│ │ 🔄 DAG Execution: Authentication System     │ │
│ │ Phase: Review                    [2/3]      │ │
│ │ ├─ review-backend      [✓ Done]             │ │
│ │ ├─ review-frontend     [⚙️ Running]          │ │
│ │ └─ review-security     [⏸️ Pending]          │ │
│ │ [View Graph] [View Logs]                    │ │
│ └─────────────────────────────────────────────┘ │
│                                                  │
│ The plan is executing...                        │
└─────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────┐
│ User                                             │
│ Also fix the typo in README.md                  │
└─────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────┐
│ Assistant                                        │
│ Sure, let me check the file first.             │
│                                                  │
│ [Tool Call]                                     │
│ 📄 Read  ● Done  50ms  README.md               │  ← 传统单工具展示
│                                                  │
│ [Tool Call]                                     │
│ 🔧 Edit  ● Done  80ms  README.md               │  ← 传统单工具展示
│   └─ Fixed typo: "authetication" → "authentication"
│                                                  │
│ Fixed the typo!                                 │
└─────────────────────────────────────────────────┘
```

---

## 4. 判断逻辑（代码实现）

### 4.1 MessageTurn 渲染逻辑

```typescript
// apps/desktop/src/components/chat-timeline/MessageTurn.tsx

export function MessageTurn({ turn, ... }: { ... }) {
  return (
    <div className="message-turn">
      {turn.blocks.map(block => {
        // 检测工具类型
        if (block.kind === 'tool') {
          // 特殊处理：plan_execute 显示 DAG 卡片
          if (block.toolName === 'plan_execute') {
            return (
              <DagExecutionCard
                key={block.id}
                executionId={block.result.executionId}
                initialData={parseInitialDagData(block)}
              />
            );
          }
          
          // 其他工具：使用传统的 ProcessToolRow
          return <ProcessToolRow key={block.id} tool={block} />;
        }
        
        // 其他 block 类型...
        return <DefaultBlockRenderer block={block} />;
      })}
    </div>
  );
}
```

---

## 5. 为什么需要 DAG 专门展示？

### 5.1 传统工具展示的局限性

**问题**：`plan_execute` 执行一个包含 5 个节点的 DAG

**传统展示（ProcessToolRow）**：
```
┌─────────────────────────────────────────────────┐
│ 📋 plan_execute  ⚙️ Running  Authentication    │
│   └─ (展开后只能看到最终结果，看不到实时进度)   │
└─────────────────────────────────────────────────┘
```

**缺点**：
- ❌ 看不到内部子任务进度
- ❌ 看不到哪些节点正在运行
- ❌ 看不到依赖关系
- ❌ 看不到 Phase 分组
- ❌ 只能等全部完成后才能看结果

---

### 5.2 DAG 专门展示的优势

```
┌─────────────────────────────────────────────────┐
│ 🔄 DAG Execution: Authentication System         │
│ Progress: 3/5 nodes (60%)                       │
│                                                  │
│ Phase: Review                        [2/2 ✓]   │
│ ├─ review-backend      [✓ Done] (2.3s)         │
│ └─ review-frontend     [✓ Done] (1.8s)         │
│                                                  │
│ Phase: Implementation                [1/2]      │
│ ├─ fix-backend         [✓ Done] (3.1s)         │
│ └─ fix-frontend        [⚙️ Running] (45s...)    │  ← 实时显示
│                                                  │
│ Phase: Testing                       [0/1]      │
│ └─ run-tests           [⏸️ Pending]             │  ← 等待前置节点
│     └─ Depends on: fix-frontend                 │
└─────────────────────────────────────────────────┘
```

**优势**：
- ✅ 实时显示所有节点状态
- ✅ 清晰展示依赖关系
- ✅ Phase 分组，多阶段可见
- ✅ 总体进度一目了然
- ✅ 可查看拓扑图

---

## 6. 实现策略

### 6.1 渐进式增强

**Phase 1（最小可行）**：
- ✅ 保留所有现有工具的 `ProcessToolRow` 展示
- ✅ **仅**为 `plan_execute` 增加 `DagExecutionCard`
- ✅ 两者共存，互不影响

**Phase 2（可选增强）**：
- 为 `task` 工具（单个子代理）也增强展示
- 显示子代理的实时输出流

**Phase 3（可选增强）**：
- 所有长时间运行的工具都可以有进度展示

---

### 6.2 向后兼容

**保证**：
- ✅ 旧版本对话回放时，`plan_execute` 结果仍可正常显示
- ✅ 如果 DAG 数据不可用，降级为传统 `ProcessToolRow` 展示
- ✅ 不影响其他工具的展示

```typescript
// 降级逻辑
if (block.toolName === 'plan_execute' && block.result?.dagData) {
  return <DagExecutionCard ... />;
} else {
  return <ProcessToolRow tool={block} />;
}
```

---

## 7. 对比总结

| 维度 | ProcessToolRow（现有） | DagExecutionCard（新增） |
|---|---|---|
| **适用工具** | 所有工具 | 仅 `plan_execute` |
| **显示方式** | 单行 + 展开详情 | 多行卡片 + Phase 分组 |
| **实时更新** | 仅状态切换 | 所有节点实时更新 |
| **进度展示** | 无 | 总体进度条 + 节点进度 |
| **依赖关系** | 无 | 显示依赖 |
| **Phase 分组** | 无 | 支持 |
| **拓扑图** | 无 | 可选 Mermaid 图 |
| **是否保留** | ✅ 保留 | ✅ 新增 |

---

## 8. 用户体验对比

### 8.1 场景 A：简单工具调用

**用户输入**：
```
Read src/main.rs and fix the typo
```

**展示方式**：
```
[Tool Call] Read  ● Done  50ms  src/main.rs       ← ProcessToolRow（简洁）
[Tool Call] Edit  ● Done  80ms  src/main.rs       ← ProcessToolRow（简洁）
```

**结论**：简单任务用传统展示更合适，简洁清晰。

---

### 8.2 场景 B：复杂 DAG 编排

**用户输入**：
```
Plan and implement a full authentication system with review phases
```

**展示方式**：
```
[Tool Call] plan_execute                           ← 触发 DAG 卡片
┌─────────────────────────────────────────────────┐
│ 🔄 DAG Execution: Authentication System         │
│ Phase: Review           [2/2 ✓]                 │
│ Phase: Implementation   [1/2 ⚙️]                │
│ Phase: Testing          [0/1 ⏸️]                 │
│ [View Graph] [View Logs]                        │
└─────────────────────────────────────────────────┘
```

**结论**：复杂任务用 DAG 卡片更合适，信息丰富、实时更新。

---

## 9. 最终回答

### ❓ 这是替换还是升级？

**答案**：✅ **升级（增强）**

- **不是替换**：所有现有工具的 `ProcessToolRow` 展示方式**完全保留**
- **是增强**：**仅**为 `plan_execute` 工具增加专门的 DAG 可视化卡片
- **共存策略**：根据工具类型自动选择展示方式

### 🎯 实施原则

1. **最小侵入**：只修改 `MessageTurn.tsx` 的渲染逻辑，增加判断分支
2. **向后兼容**：旧对话回放不受影响
3. **渐进增强**：先实现 `plan_execute`，后续可扩展到其他工具

---

**结论**：这是一个针对特定场景（DAG 编排）的**专门增强**，不影响现有的简单工具展示流程。两者共存，各司其职。

---

**文档创建日期**：2026-10-01  
**作者**：Kiro (Claude Fable 5)
