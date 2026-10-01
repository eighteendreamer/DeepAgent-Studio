# Desktop UI DAG 可视化详细设计方案

> **目标**：在 Desktop 对话界面中展示 DAG 编排的实时进度，让用户直观看到多个子代理的执行状态和依赖关系。

---

## 1. 显示位置与触发方式

### 1.1 显示位置

**方案 A（推荐）：嵌入到对话流中（Inline Card）**

当用户调用 `plan_execute()` 后，在对话流中插入一个**可折叠的 DAG 卡片**，类似工具调用卡片。

```
┌─────────────────────────────────────────────────┐
│ User                                             │
│ Plan and implement the user authentication      │
└─────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────┐
│ Assistant                                        │
│ I'll break this into phases...                  │
│                                                  │
│ [Tool Call] plan_execute                        │
│ ┌─────────────────────────────────────────────┐ │
│ │ 🔄 DAG Execution: Authentication System     │ │
│ │                                             │ │
│ │ Phase: Review                    [2/3 ✓]   │ │
│ │ ├─ review-backend      [✓ Done]            │ │
│ │ ├─ review-frontend     [⚙️ Running]         │ │
│ │ └─ review-security     [⏸️ Pending]         │ │
│ │                                             │ │
│ │ Phase: Implementation            [0/2]      │ │
│ │ ├─ fix-backend         [⏸️ Pending]         │ │
│ │ └─ fix-frontend        [⏸️ Pending]         │ │
│ │                                             │ │
│ │ [View Graph] [View Logs]                   │ │
│ └─────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────┘
```

**方案 B：侧边面板（Side Panel）**

点击 "View DAG" 按钮后，右侧弹出侧边栏展示完整 DAG 拓扑图。

---

### 1.2 触发方式

**自动触发**：
- 当 `plan_execute` 工具被调用时，自动在对话流中插入 DAG 卡片
- 卡片初始状态为"折叠"，显示总体进度
- 点击展开后显示详细的 phase 分组 + 节点列表

**手动打开**：
- 卡片右上角有 "View Graph" 按钮，点击后打开侧边栏展示 Mermaid 拓扑图

---

## 2. UI 设计

### 2.1 Inline Card（嵌入式卡片）

#### 折叠状态（Collapsed）
```
┌─────────────────────────────────────────────────┐
│ 🔄 DAG Execution: Authentication System         │
│ Progress: 3/8 nodes complete (37%)              │
│ Current: review-frontend, fix-backend           │
│                                                  │
│ [▼ Expand] [View Graph] [View Logs]            │
└─────────────────────────────────────────────────┘
```

#### 展开状态（Expanded）
```
┌─────────────────────────────────────────────────┐
│ 🔄 DAG Execution: Authentication System    [▲]  │
├─────────────────────────────────────────────────┤
│ Phase: Review                        [2/3 ✓]   │
│ ├─ review-backend                    [✓ Done]   │
│ │  └─ Summary: Found 3 issues                   │
│ ├─ review-frontend                   [⚙️ Running]│
│ │  └─ Running for 23s...                        │
│ └─ review-security                   [⏸️ Pending]│
│                                                  │
│ Phase: Implementation                [0/2]      │
│ ├─ fix-backend                       [⏸️ Pending]│
│ │  └─ Depends on: review-backend               │
│ └─ fix-frontend                      [⏸️ Pending]│
│     └─ Depends on: review-frontend              │
│                                                  │
│ Phase: Testing                       [0/1]      │
│ └─ run-tests                         [⏸️ Pending]│
│     └─ Depends on: fix-backend, fix-frontend    │
│                                                  │
│ [View Graph] [View Logs] [Stop All]            │
└─────────────────────────────────────────────────┘
```

---

### 2.2 侧边栏图表视图（Graph View）

点击 "View Graph" 后，右侧打开侧边栏，展示 **Mermaid 流程图**：

```
┌─────────────────────────────────────────────────┐
│ DAG Graph: Authentication System           [×]  │
├─────────────────────────────────────────────────┤
│                                                  │
│        ┌─────────────────┐                      │
│        │ review-backend  │                      │
│        │   ✓ Done        │                      │
│        └────────┬────────┘                      │
│                 │                                │
│                 ▼                                │
│        ┌─────────────────┐                      │
│        │  fix-backend    │                      │
│        │  ⏸️ Pending      │                      │
│        └────────┬────────┘                      │
│                 │                                │
│     ┌───────────┴──────────┐                    │
│     │                      │                    │
│     ▼                      ▼                    │
│ ┌─────────┐          ┌─────────┐               │
│ │run-tests│          │ deploy  │               │
│ │⏸️ Pending│          │⏸️ Pending│               │
│ └─────────┘          └─────────┘               │
│                                                  │
│ Legend:                                          │
│ ✓ Done  ⚙️ Running  ⏸️ Pending  ❌ Failed        │
│                                                  │
│ [Zoom In] [Zoom Out] [Fit to View]             │
└─────────────────────────────────────────────────┘
```

---

## 3. 技术实现

### 3.1 数据结构

#### 前端类型定义（TypeScript）

```typescript
// apps/desktop/src/types/dag.ts

export interface DagNode {
  id: string;
  goal: string;
  phase?: string;
  role?: string;
  status: 'pending' | 'running' | 'done' | 'failed';
  dependsOn: string[];
  startedAt?: number;  // timestamp
  completedAt?: number;
  summary?: string;
  error?: string;
}

export interface DagExecution {
  executionId: string;
  title: string;
  nodes: DagNode[];
  currentNodes: string[];  // 当前正在运行的节点 ID
  createdAt: number;
  updatedAt: number;
}

export interface DagStatusUpdate {
  executionId: string;
  nodeId: string;
  status: 'pending' | 'running' | 'done' | 'failed';
  summary?: string;
  error?: string;
}
```

---

### 3.2 后端 Tauri Command

```rust
// apps/desktop/src-tauri/src/commands/dag.rs

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagNodeStatus {
    pub id: String,
    pub goal: String,
    pub phase: Option<String>,
    pub status: String,  // "pending" | "running" | "done" | "failed"
    pub depends_on: Vec<String>,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagExecutionStatus {
    pub execution_id: String,
    pub title: String,
    pub nodes: Vec<DagNodeStatus>,
    pub updated_at: u64,
}

#[tauri::command]
pub async fn get_dag_status(
    execution_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<DagExecutionStatus, String> {
    // 从 AppState 中查询 DAG 执行状态
    // 实现略（需要在 AppState 中维护 DAG 状态映射）
    todo!()
}

#[tauri::command]
pub async fn subscribe_dag_updates(
    execution_id: String,
    window: tauri::Window,
) -> Result<(), String> {
    // 订阅 DAG 状态更新，通过 WebSocket/Event 推送到前端
    // 当节点状态改变时，发送事件：
    // window.emit("dag-update", DagStatusUpdate { ... })?;
    todo!()
}
```

---

### 3.3 前端组件实现

#### 3.3.1 DagExecutionCard 组件

```tsx
// apps/desktop/src/components/dag/DagExecutionCard.tsx

import { useState, useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { Accordion, AccordionContent, AccordionItem, AccordionTrigger } from "../ui/accordion";
import { Badge } from "../ui/badge";
import { Button } from "../ui/button";
import { Progress } from "../ui/progress";
import type { DagExecution, DagStatusUpdate } from "../../types/dag";

export function DagExecutionCard({ executionId, initialData }: {
  executionId: string;
  initialData: DagExecution;
}) {
  const [dag, setDag] = useState<DagExecution>(initialData);
  const [expanded, setExpanded] = useState(false);

  useEffect(() => {
    // 订阅 DAG 状态更新
    const unlisten = listen<DagStatusUpdate>('dag-update', (event) => {
      if (event.payload.executionId === executionId) {
        setDag(prev => ({
          ...prev,
          nodes: prev.nodes.map(node =>
            node.id === event.payload.nodeId
              ? { ...node, status: event.payload.status, summary: event.payload.summary }
              : node
          ),
          updatedAt: Date.now(),
        }));
      }
    });

    return () => {
      unlisten.then(fn => fn());
    };
  }, [executionId]);

  const nodesByPhase = groupNodesByPhase(dag.nodes);
  const completedNodes = dag.nodes.filter(n => n.status === 'done').length;
  const totalNodes = dag.nodes.length;
  const progress = (completedNodes / totalNodes) * 100;

  return (
    <div className="rounded-lg border bg-card p-4">
      <div className="flex items-center justify-between mb-2">
        <h3 className="text-sm font-semibold flex items-center gap-2">
          🔄 DAG Execution: {dag.title}
        </h3>
        <Button variant="ghost" size="sm" onClick={() => setExpanded(!expanded)}>
          {expanded ? "▲" : "▼"}
        </Button>
      </div>

      <div className="mb-3">
        <Progress value={progress} className="h-2" />
        <p className="text-xs text-muted-foreground mt-1">
          Progress: {completedNodes}/{totalNodes} nodes complete ({Math.round(progress)}%)
        </p>
      </div>

      {expanded && (
        <div className="space-y-4">
          {Object.entries(nodesByPhase).map(([phase, nodes]) => (
            <PhaseSection key={phase} phase={phase} nodes={nodes} />
          ))}
        </div>
      )}

      <div className="flex gap-2 mt-3">
        <Button variant="outline" size="sm" onClick={() => openGraphView(executionId)}>
          View Graph
        </Button>
        <Button variant="outline" size="sm" onClick={() => openLogs(executionId)}>
          View Logs
        </Button>
      </div>
    </div>
  );
}

function PhaseSection({ phase, nodes }: { phase: string; nodes: DagNode[] }) {
  const completed = nodes.filter(n => n.status === 'done').length;
  const total = nodes.length;

  return (
    <div>
      <div className="flex items-center justify-between mb-2">
        <h4 className="text-sm font-medium">Phase: {phase}</h4>
        <Badge variant="secondary">{completed}/{total} ✓</Badge>
      </div>
      <div className="space-y-2 pl-4">
        {nodes.map(node => (
          <NodeRow key={node.id} node={node} />
        ))}
      </div>
    </div>
  );
}

function NodeRow({ node }: { node: DagNode }) {
  const statusIcon = {
    pending: '⏸️',
    running: '⚙️',
    done: '✓',
    failed: '❌',
  }[node.status];

  return (
    <div className="flex items-start gap-2 text-sm">
      <span className="text-base">{statusIcon}</span>
      <div className="flex-1">
        <div className="flex items-center gap-2">
          <span className="font-mono text-xs">{node.id}</span>
          <Badge variant="outline" className="text-xs">{node.status}</Badge>
        </div>
        {node.summary && (
          <p className="text-xs text-muted-foreground mt-1">└─ {node.summary}</p>
        )}
        {node.dependsOn.length > 0 && node.status === 'pending' && (
          <p className="text-xs text-muted-foreground mt-1">
            └─ Depends on: {node.dependsOn.join(', ')}
          </p>
        )}
      </div>
    </div>
  );
}

function groupNodesByPhase(nodes: DagNode[]): Record<string, DagNode[]> {
  return nodes.reduce((acc, node) => {
    const phase = node.phase || 'Default';
    if (!acc[phase]) acc[phase] = [];
    acc[phase].push(node);
    return acc;
  }, {} as Record<string, DagNode[]>);
}
```

---

#### 3.3.2 集成到 MessageTurn

```tsx
// apps/desktop/src/components/chat-timeline/MessageTurn.tsx

import { DagExecutionCard } from '../dag/DagExecutionCard';

export function MessageTurn({ turn, ... }: { ... }) {
  return (
    <div className="message-turn">
      {turn.blocks.map(block => {
        if (block.kind === 'tool' && block.toolName === 'plan_execute') {
          // 检测到 plan_execute 工具调用，渲染 DAG 卡片
          return (
            <DagExecutionCard
              key={block.id}
              executionId={block.result.executionId}
              initialData={parseInitialDagData(block)}
            />
          );
        }
        
        // 其他 block 类型...
        return <DefaultBlockRenderer block={block} />;
      })}
    </div>
  );
}
```

---

### 3.4 Mermaid 图表渲染（可选）

```tsx
// apps/desktop/src/components/dag/DagGraphView.tsx

import { useEffect, useRef } from 'react';
import mermaid from 'mermaid';
import type { DagExecution } from '../../types/dag';

export function DagGraphView({ dag }: { dag: DagExecution }) {
  const containerRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!containerRef.current) return;

    const mermaidSyntax = generateMermaidSyntax(dag);
    mermaid.render('dag-graph', mermaidSyntax).then(({ svg }) => {
      if (containerRef.current) {
        containerRef.current.innerHTML = svg;
      }
    });
  }, [dag]);

  return <div ref={containerRef} className="w-full h-full" />;
}

function generateMermaidSyntax(dag: DagExecution): string {
  let mermaid = 'graph TD\n';
  
  dag.nodes.forEach(node => {
    const statusIcon = {
      pending: '⏸️',
      running: '⚙️',
      done: '✓',
      failed: '❌',
    }[node.status];
    
    const nodeLabel = `${node.id}<br/>${statusIcon} ${node.phase || ''}`;
    mermaid += `  ${node.id}["${nodeLabel}"]\n`;
    
    node.dependsOn.forEach(depId => {
      mermaid += `  ${depId} --> ${node.id}\n`;
    });
  });

  return mermaid;
}
```

---

## 4. 数据流

```
用户调用 plan_execute
    ↓
Rust 后端执行 DagScheduler
    ↓
每个节点状态变化时：
    ↓
window.emit("dag-update", { executionId, nodeId, status, summary })
    ↓
前端 listen('dag-update')
    ↓
更新 DagExecutionCard 状态
    ↓
重新渲染进度条 + 节点列表
```

---

## 5. 示例效果

### 5.1 真实案例：Authentication System

**用户输入**：
```
Plan and implement a complete authentication system with review phases
```

**DAG 定义**（后端生成）：
```rust
PlanDag {
    nodes: [
        PlanNode {
            id: "review-backend",
            goal: "Review backend auth code",
            phase: Some("Review"),
            role: Some("reviewer"),
        },
        PlanNode {
            id: "review-frontend",
            goal: "Review frontend login UI",
            phase: Some("Review"),
            role: Some("reviewer"),
        },
        PlanNode {
            id: "fix-backend",
            goal: "Fix backend issues",
            phase: Some("Implementation"),
            depends_on: vec!["review-backend"],
        },
        PlanNode {
            id: "fix-frontend",
            goal: "Fix frontend issues",
            phase: Some("Implementation"),
            depends_on: vec!["review-frontend"],
        },
        PlanNode {
            id: "run-tests",
            goal: "Run integration tests",
            phase: Some("Testing"),
            depends_on: vec!["fix-backend", "fix-frontend"],
        },
    ]
}
```

**前端渲染效果**：

```
┌─────────────────────────────────────────────────┐
│ 🔄 DAG Execution: Authentication System    [▲]  │
├─────────────────────────────────────────────────┤
│ █████████████████░░░░░░░░░░░░░ 60% (3/5)       │
│                                                  │
│ Phase: Review                        [2/2 ✓]   │
│ ├─ review-backend                    [✓ Done]   │
│ │  └─ Found 2 SQL injection risks               │
│ └─ review-frontend                   [✓ Done]   │
│    └─ Missing CSRF token validation             │
│                                                  │
│ Phase: Implementation                [1/2 ✓]    │
│ ├─ fix-backend                       [✓ Done]   │
│ │  └─ Added parameterized queries               │
│ └─ fix-frontend                      [⚙️ Running]│
│    └─ Running for 45s...                        │
│                                                  │
│ Phase: Testing                       [0/1]      │
│ └─ run-tests                         [⏸️ Pending]│
│    └─ Depends on: fix-frontend                  │
│                                                  │
│ [View Graph] [View Logs] [Stop All]            │
└─────────────────────────────────────────────────┘
```

---

## 6. 实施计划

### 6.1 Phase 1：Inline Card（1 天）

**任务**：
1. ✅ 创建 `DagExecutionCard.tsx` 组件（~200 行）
2. ✅ 创建 TypeScript 类型定义（`dag.ts`）
3. ✅ 集成到 `MessageTurn.tsx`
4. ✅ 实现按 phase 分组展示
5. ✅ 实现实时状态更新（WebSocket/Event）

**验收标准**：
- DAG 卡片在对话流中正确显示
- 点击展开/折叠正常工作
- 节点状态实时更新

---

### 6.2 Phase 2：后端状态管理（0.5 天）

**任务**：
1. ✅ 在 `AppState` 中维护 DAG 执行状态映射
2. ✅ 实现 `get_dag_status` Tauri command
3. ✅ 实现 `subscribe_dag_updates` Tauri command
4. ✅ 在 `DagScheduler` 中触发状态更新事件

**验收标准**：
- 前端能够通过 `invoke("get_dag_status")` 查询状态
- 节点状态变化时，前端收到 `dag-update` 事件

---

### 6.3 Phase 3：Graph View（可选，0.5 天）

**任务**：
1. ✅ 创建 `DagGraphView.tsx` 组件
2. ✅ 集成 `mermaid` 库
3. ✅ 实现 Mermaid 语法生成
4. ✅ 实现侧边栏展示

**验收标准**：
- 点击 "View Graph" 打开侧边栏
- Mermaid 图表正确渲染
- 节点状态颜色正确显示

---

## 7. 替代方案（最简版本）

如果时间有限，可以实现**纯文本版本**（无 UI 组件）：

```
┌─────────────────────────────────────────────────┐
│ Assistant                                        │
│                                                  │
│ [Tool Call] plan_execute                        │
│ ┌─────────────────────────────────────────────┐ │
│ │ DAG Execution Started                       │ │
│ │                                             │ │
│ │ Phase: Review                               │ │
│ │   - review-backend [Running]                │ │
│ │   - review-frontend [Pending]               │ │
│ │                                             │ │
│ │ Phase: Implementation                       │ │
│ │   - fix-backend [Pending]                   │ │
│ └─────────────────────────────────────────────┘ │
│                                                  │
│ [System] review-backend completed               │
│ [System] review-frontend started                │
│ [System] review-frontend completed              │
│ [System] fix-backend started                    │
│ ...                                              │
└─────────────────────────────────────────────────┘
```

这种方案只需要在后端生成文本输出，无需前端 UI 改动。

---

## 8. 总结

**推荐方案**：Phase 1 + Phase 2（Inline Card + 后端状态管理）

**工期**：1.5 天

**核心价值**：
- ✅ 用户直观看到 DAG 执行进度
- ✅ 按 phase 分组，清晰展示多阶段 workflow
- ✅ 实时更新，无需刷新页面
- ✅ 与现有对话流完美集成

**可选增强**：Mermaid 图表视图（+0.5 天）

---

**文档创建日期**：2026-10-01  
**设计者**：Kiro (Claude Fable 5)
