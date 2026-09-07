# 无限画布 UI 方案（v3 · 双模式）

> 单页面、双模式、零冗余。
>
> 参考源：Dify（专业模式交互）+ Penguin-Magic（创作模式交互）+ ComfyUI/NodeTool（视频工作流）
>
> 范围：仅前端，后端预留。
>
> 时间：2026-09-07
> 状态：方案评审中

---

## 1. 页面布局

整个页面只有画布 + 三个浮层，没有多余元素：

```
┌──────────────────────────────────────────────────────────────┐
│                                                              │
│  [创作] [专业]                                               │
│                                                              │
│                                                              │
│                      ReactFlow 画布                          │
│                                                              │
│                                                              │
│                                                              │
│                                                              │
│                                                              │
│  ┌───────┐              ┌──────────────────────────┐         │
│  │       │              │                          │         │
│  │ MiniMap│             │       底部工具栏           │         │
│  │       │              │  ↩↪ | 网格 | ──●── 100% | ▶ 运行 │ │
│  └───────┘              └──────────────────────────┘         │
│                                                              │
└──────────────────────────────────────────────────────────────┘
```

| 浮层 | 位置 | 内容 |
|---|---|---|
| 模式切换 | 左上角 | `[创作]` `[专业]` 两个按钮，当前模式高亮 |
| 小地图 | 左下角 | 画布缩略图 + 视口指示器 |
| 底部工具栏 | 底部居中 | 撤销/重做、网格/磁吸、缩放滑块、运行按钮 |

**没有顶部导航。没有侧边栏。没有固定面板。所有配置通过点击节点弹出。**

---

## 2. 双模式设计

### 2.1 创作模式

面向内容创作者，节点按**媒体类型**组织，交互参考 Penguin-Magic。

**两个创作方向**（节点库中分组显示）：

#### 图文创作

| 节点 | 图标 | 说明 |
|---|---|---|
| 文本生成 | `faPen` | LLM 文本写作（提示词 → 文本） |
| 图片生成 | `faImage` | 文生图 / 图生图 |
| 图片对比 | `faLeftRight` | 双图滑块对比 |
| 图片编辑 | `faWandMagicSparkles` | 裁剪 / 去背景 / 超分 / 重绘（预留） |

#### 视频创作

| 节点 | 图标 | 说明 |
|---|---|---|
| 脚本生成 | `faFileLines` | LLM 生成视频脚本/分镜 |
| 图片生成 | `faImage` | 生成故事板图片（复用图文的图片生成） |
| 视频生成 | `faVideo` | 文生视频 / 图生视频（Sora / Veo / Kling） |
| 视频拼接 | `faFilm` | 多段视频拼接成完整视频（预留） |

**创作模式特点**：
- 节点内直接预览（图片节点显示缩略图，视频节点显示封面帧）
- 点击节点弹出**浮动配置面板**（靠近节点，不固定在一侧）
- 面板内容简洁：提示词 + 模型选择 + 几个关键参数
- 连线表示数据流向（图片→视频 = 图生视频）

### 2.2 专业模式

面向开发者/业务人员，节点按**功能职责**组织，交互完全参考 Dify。

| 分组 | 节点 | 说明 |
|---|---|---|
| **流程控制** | 开始 | 工作流入口，定义输入变量 |
| | 结束 | 工作流出口，定义输出 |
| | 条件分支 | If-Else 路由 |
| | 迭代 | For-Each 循环 |
| **AI** | LLM | 大语言模型调用 |
| | Agent | 自主 Agent（工具调用循环） |
| | 问题分类 | LLM 分类路由 |
| | 参数提取 | LLM 结构化抽取 |
| **知识** | 知识检索 | RAG 知识库检索 |
| **数据** | 代码执行 | Python/JS 沙箱 |
| | HTTP 请求 | 外部 API 调用 |
| | 模板转换 | Jinja2 模板渲染 |
| | 变量聚合 | 多路输出合并 |
| **工具** | 工具调用 | 注册工具 / MCP 工具 |
| | 人工审批 | 暂停等待人工确认 |

**专业模式特点**：
- 点击节点 → **右侧固定面板**滑出（360px，Dify 风格）
- 面板显示完整编辑信息：输入变量选择、参数配置、输出预览、单次运行、错误处理
- 连线上显示数据类型标签
- 支持子流程/迭代容器节点

### 2.3 模式切换规则

- 切换模式时画布数据**不共享**（两套独立 store），但可以在 UI 上提示"切换到专业模式将显示专业工作流"
- 模式按钮始终在左上角可见
- 节点库内容随模式切换完全替换
- 底部工具栏、小地图、画布操作（平移/缩放/框选）两种模式一致

---

## 3. 通用交互（两种模式共享）

### 3.1 添加节点

| 方式 | 说明 |
|---|---|
| **双击空白处** | 弹出节点选择浮层（Popover），搜索 + 分组列表 |
| **拖拽添加** | 从节点库面板拖到画布（节点库通过双击或按钮唤出） |
| **右键菜单** | 右键空白处 → "添加节点" → 弹出选择浮层 |
| **连线插入** | 两节点之间连线上出现 "+" 按钮，点击插入节点 |

**节点选择浮层**（双击弹出）：

```
┌─────────────────────┐
│ 🔍 搜索节点...       │
├─────────────────────┤
│ 图文创作             │
│  📝 文本生成          │
│  🖼 图片生成          │
│  ↔ 图片对比           │
│  ✨ 图片编辑          │
├─────────────────────┤
│ 视频创作             │
│  📄 脚本生成          │
│  🎬 视频生成          │
│  🎞 视频拼接          │
└─────────────────────┘
```

### 3.2 编辑节点

| 模式 | 编辑方式 |
|---|---|
| 创作模式 | 点击节点 → **浮动面板**（靠近节点右侧，400px 宽，可拖拽位置） |
| 专业模式 | 点击节点 → **右侧固定面板**（360px 宽，贴边滑入，Dify 风格） |

两种面板都支持：
- 节点名称编辑
- 参数配置（按节点类型渲染不同表单）
- 单次运行按钮
- 运行结果预览
- 关闭按钮 / 点击空白处关闭

### 3.3 工作流运行

| 方式 | 说明 |
|---|---|
| **底部工具栏 ▶ 按钮** | 运行整个工作流（从所有 Start 节点开始） |
| **框选 + 运行** | 框选部分节点 → 底部工具栏出现"运行选中"按钮 |
| **单节点运行** | 在节点编辑面板中点击"运行此节点" |

**运行时可视化**：
- 当前执行节点：蓝色脉冲边框
- 执行成功：绿色边框
- 执行失败：红色边框 + 面板显示错误
- 连线上：虚线流动动画表示数据传递
- 底部工具栏显示运行状态（运行中 / 成功 / 失败 + 耗时）

### 3.4 画布操作

| 操作 | 方式 |
|---|---|
| 平移 | 中键拖拽 / 空格+左键 |
| 缩放 | 滚轮 / 底部滑块 |
| 框选 | 左键拖拽空白处 |
| 多选 | Ctrl+点击 / 框选 |
| 删除 | Delete / Backspace |
| 复制粘贴 | Ctrl+C / Ctrl+V |
| 撤销重做 | Ctrl+Z / Ctrl+Shift+Z |

---

## 4. 底部工具栏

```
┌──────────────────────────────────────────────────────────┐
│  ↩  ↪  │  ▦网格  ⊕磁吸  │  ────●──── 100%  │  3/12  │  ▶ 运行  │
└──────────────────────────────────────────────────────────┘
```

| 区域 | 内容 |
|---|---|
| 左侧 | 撤销 / 重做 |
| 中左 | 网格开关 / 磁吸开关 |
| 中间 | 缩放滑块 + 百分比（点击重置 100%） |
| 中右 | 选中数/总数 |
| 右侧 | **▶ 运行** 按钮（主色高亮） |

- 玻璃态背景（`backdrop-blur-xl` + 半透明）
- 圆角 14px
- 底部居中，距底 16px
- 框选时右侧追加 **"▶ 运行选中"** 按钮

---

## 5. 目录结构

```
apps/desktop/src/canvas/
├── CanvasApp.tsx                    # 现有（无限画板，不动）
├── CanvasTitleBar.tsx               # 现有（共用）
│
└── workflow/                        # 新增（无限画布）
    ├── WorkflowCanvasApp.tsx        # 主入口
    ├── types.ts                     # 所有类型定义
    │
    ├── store/
    │   ├── creativeStore.ts         # 创作模式 store（Zustand）
    │   ├── professionalStore.ts     # 专业模式 store（Zustand）
    │   └── canvasStore.ts           # 共用 store（模式/视口/历史）
    │
    ├── components/
    │   ├── ModeSwitcher.tsx         # 左上角模式切换
    │   ├── MiniMap.tsx              # 左下角小地图
    │   ├── BottomBar.tsx            # 底部工具栏
    │   ├── NodePicker.tsx           # 双击弹出的节点选择器
    │   ├── ConfigPanel.tsx          # 配置面板容器（根据模式切换浮动/固定）
    │   └── RunIndicator.tsx         # 运行状态指示器
    │
    ├── nodes/                       # 节点内容渲染
    │   ├── creative/                # 创作模式节点
    │   │   ├── TextGenNode.tsx
    │   │   ├── ImageGenNode.tsx
    │   │   ├── ImageCompareNode.tsx
    │   │   ├── ImageEditNode.tsx
    │   │   ├── ScriptGenNode.tsx
    │   │   ├── VideoGenNode.tsx
    │   │   └── VideoStitchNode.tsx
    │   │
    │   └── professional/            # 专业模式节点
    │       ├── StartNode.tsx
    │       ├── EndNode.tsx
    │       ├── IfElseNode.tsx
    │       ├── IterationNode.tsx
    │       ├── LLMNode.tsx
    │       ├── AgentNode.tsx
    │       ├── QuestionClassifierNode.tsx
    │       ├── ParameterExtractorNode.tsx
    │       ├── KnowledgeRetrievalNode.tsx
    │       ├── CodeNode.tsx
    │       ├── HttpRequestNode.tsx
    │       ├── TemplateTransformNode.tsx
    │       ├── VariableAggregatorNode.tsx
    │       ├── ToolNode.tsx
    │       └── HumanInputNode.tsx
    │
    ├── panels/                      # 配置面板
    │   ├── creative/                # 创作模式面板（浮动）
    │   │   ├── TextGenPanel.tsx
    │   │   ├── ImageGenPanel.tsx
    │   │   ├── VideoGenPanel.tsx
    │   │   └── ...
    │   │
    │   └── professional/            # 专业模式面板（右侧固定）
    │       ├── StartPanel.tsx
    │       ├── LLMPanel.tsx
    │       ├── CodePanel.tsx
    │       └── ...
    │
    ├── hooks/
    │   ├── useHistory.ts            # 撤销/重做
    │   ├── useViewport.ts           # 视口持久化
    │   └── useWorkflowRunner.ts     # 工作流执行引擎（前端模拟）
    │
    └── utils/
        ├── nodeDefaults.ts          # 各节点默认配置
        ├── validation.ts            # 连接有效性检查
        └── layout.ts                # 自动布局
```

---

## 6. 类型定义

```ts
// 模式
type CanvasMode = "creative" | "professional";

// 节点状态
type NodeStatus = "idle" | "running" | "completed" | "error";

// 创作模式节点类型
type CreativeNodeKind =
  | "text-gen"          // 文本生成
  | "image-gen"         // 图片生成
  | "image-compare"     // 图片对比
  | "image-edit"        // 图片编辑
  | "script-gen"        // 脚本生成
  | "video-gen"         // 视频生成
  | "video-stitch";     // 视频拼接

// 专业模式节点类型
type ProfessionalNodeKind =
  | "start"
  | "end"
  | "if-else"
  | "iteration"
  | "llm"
  | "agent"
  | "question-classifier"
  | "parameter-extractor"
  | "knowledge-retrieval"
  | "code"
  | "http-request"
  | "template-transform"
  | "variable-aggregator"
  | "tool"
  | "human-input";

// 创作节点数据
interface CreativeNodeData {
  label: string;
  status: NodeStatus;
  errorMessage?: string;

  // 文本/脚本生成
  prompt?: string;
  model?: string;
  output?: string;

  // 图片生成
  imageModel?: string;
  imagePrompt?: string;
  imageUrl?: string;
  aspectRatio?: string;
  resolution?: string;

  // 视频生成
  videoService?: "sora" | "veo" | "kling";
  videoModel?: string;
  videoPrompt?: string;
  videoInputUrl?: string;
  videoUrl?: string;
  videoDuration?: number;
  videoProgress?: number;
  videoTaskId?: string;

  // 图片对比
  leftImageUrl?: string;
  rightImageUrl?: string;

  // 图片编辑
  editMode?: "crop" | "remove-bg" | "upscale" | "repaint";

  // 视频拼接
  inputVideoUrls?: string[];

  // 运行结果
  result?: unknown;
  executionTime?: number;
}

// 专业节点数据
interface ProfessionalNodeData {
  label: string;
  description?: string;
  status: NodeStatus;
  errorMessage?: string;

  // 开始节点
  inputVariables?: Array<{ name: string; type: string; required?: boolean }>;

  // 结束节点
  outputMapping?: Record<string, string>;

  // 条件分支
  conditions?: Array<{ variable: string; operator: string; value: string }>;

  // LLM
  llmModel?: string;
  llmPrompt?: string;
  llmSystemPrompt?: string;
  llmTemperature?: number;
  llmMaxTokens?: number;

  // Agent
  agentTools?: string[];
  agentStrategy?: "function-call" | "react";

  // 代码
  codeLanguage?: "javascript" | "python";
  codeScript?: string;

  // HTTP
  httpMethod?: "GET" | "POST" | "PUT" | "DELETE";
  httpUrl?: string;
  httpHeaders?: Record<string, string>;
  httpBody?: string;

  // 知识检索
  knowledgeBaseId?: string;
  knowledgeTopK?: number;

  // 工具
  toolId?: string;
  toolParams?: Record<string, unknown>;

  // 运行结果
  result?: unknown;
  executionTime?: number;
}
```

---

## 7. 节点视觉

### 创作模式节点

```
┌───────────────────────┐
│ 🖼 图片生成            │  ← 图标 + 名称（外置标题，节点上方）
├───────────────────────┤
│                       │
│  ┌─────────────────┐  │
│  │                 │  │  ← 内容预览区（图片缩略图/文本摘要/视频封面）
│  │   预览内容       │  │
│  │                 │  │
│  └─────────────────┘  │
│                       │
│  状态: ✓ 完成 · 2.3s  │  ← 状态行
└───────────────────────┘
    ○                 ○   ← 输入端口（左）  输出端口（右）
```

- 宽度：280px
- 圆角：16px
- 背景：`bg-elevated-bg` + 玻璃态
- 状态边框：idle 默认 / running 蓝色脉冲 / completed 绿色 / error 红色

### 专业模式节点

```
┌───────────────────────┐
│ 🟣 LLM                │  ← 分组色块 + 图标 + 名称
├───────────────────────┤
│ 模型: DeepSeek-V3     │  ← 摘要信息（1-2 行关键配置）
│ 温度: 0.7             │
├───────────────────────┤
│ ✓ 完成 · 1.2s · 350t │  ← 运行结果摘要
└───────────────────────┘
    ○                 ○
```

- 宽度：240px
- 圆角：12px
- 左上角色块标识分组（蓝=流程控制 / 紫=AI / 绿=数据 / 橙=工具）
- 更紧凑，信息密度高于创作模式

---

## 8. 实施阶段

### 阶段 1：骨架 + 模式切换（3-5 天）

- [ ] 创建 `workflow/` 目录
- [ ] 实现 `types.ts`
- [ ] 实现 `canvasStore.ts`（模式/视口/历史）
- [ ] 实现 `creativeStore.ts` + `professionalStore.ts`（Zustand）
- [ ] 实现 `WorkflowCanvasApp.tsx`（ReactFlow 容器）
- [ ] 实现 `ModeSwitcher.tsx`（左上角创作/专业切换）
- [ ] 实现 `BottomBar.tsx`（底部工具栏）
- [ ] 实现 `MiniMap.tsx`（左下角小地图）
- [ ] 修改 `main.tsx` 路由 + 侧栏入口
- [ ] 实现 `useHistory.ts`（撤销/重做）

**验收**：能打开画布窗口，切换模式，底部栏可缩放，小地图显示

### 阶段 2：创作模式节点（5-7 天）

- [ ] 实现创作模式 NodeShell（预览区 + 状态行）
- [ ] 实现 7 种创作节点的内容渲染
- [ ] 实现 `NodePicker.tsx`（双击弹出节点选择器）
- [ ] 实现节点拖拽添加 + 右键菜单添加
- [ ] 实现连线（端口拖拽 + 连线插入）
- [ ] 实现创作模式浮动配置面板（3-4 个核心面板）
- [ ] 实现节点选中/删除/复制粘贴

**验收**：创作模式下能添加节点、连线、编辑参数、看到预览

### 阶段 3：专业模式节点（7-10 天）

- [ ] 实现专业模式 NodeShell（摘要行 + 色块标识）
- [ ] 实现 15 种专业节点的内容渲染
- [ ] 实现右侧固定配置面板（Dify 风格）
- [ ] 实现 15 种专业面板（完整编辑信息）
- [ ] 实现变量引用选择器（节点间数据流）
- [ ] 实现连接有效性检查（类型兼容）

**验收**：专业模式下能构建完整工作流，面板可编辑所有参数

### 阶段 4：运行引擎 + 可视化（3-5 天）

- [ ] 实现 `useWorkflowRunner.ts`（前端模拟执行）
- [ ] 实现整体运行（底部 ▶ 按钮）
- [ ] 实现框选运行
- [ ] 实现单节点运行（面板内按钮）
- [ ] 实现运行可视化（边框动画 + 连线流动 + 状态更新）
- [ ] 实现运行结果展示（面板内显示）

**验收**：点击运行能看到节点依次执行、状态变化、结果输出

### 阶段 5：打磨 + 持久化（3-5 天）

- [ ] 视口持久化（localStorage）
- [ ] 画布快照保存/加载
- [ ] 框选 + 多选交互完善
- [ ] 键盘快捷键完善
- [ ] 自动布局算法
- [ ] 小地图样式打磨

**验收**：画布关闭重开数据恢复，交互流畅

---

## 9. 变更日志

| 日期 | 版本 | 说明 |
|---|---|---|
| 2026-09-07 | v1.0 | 初始方案（含 TopDock，偏 Penguin-Magic） |
| 2026-09-07 | v2.0 | 简化版：去掉 TopDock，统一工作流画布 |
| 2026-09-07 | v3.0 | 双模式方案：创作模式 + 专业模式，参考 Dify + Penguin-Magic + ComfyUI |
