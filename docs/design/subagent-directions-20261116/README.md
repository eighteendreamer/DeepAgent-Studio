# 子代理协作：三个 HTML 展示方向

日期：2026-11-16。设计探索，不修改正式应用。

## 打开方式

用浏览器直接打开同目录的 `index.html`。图标、CSS、JavaScript 均内嵌，不依赖 CDN、
构建、安装包或网络。页面提供 A / B / C 三种方向切换，以及运行中、全部完成、
失败阻断和重放演示。所有内容均是演示数据，不会启动代理、调用模型、执行工具或写入设置。

## 截图对应的真实链路

- `apps/desktop/src/components/chat-timeline/ProcessToolRow.tsx`：
  `plan_execute` 的工具结果解析后交给 `DagExecutionRow`。
- `apps/desktop/src/components/chat-timeline/DagExecutionRow.tsx`：
  按阶段分组展示 DAG 节点；截图中的 `Phase`、`nodes complete`、
  节点 ID 和进度条均与该组件对应。
- `crates/deepagent-app-core/src/dag_orchestration.rs`：
  生产 `ChatPlanExecutor` 使用 `DagScheduler`、`GitWorktrees` 和 `DagSubagentExecutor`。
- `crates/deepagent-app-core/src/subagent_runner.rs`：
  `DagSubagentExecutor::execute` 将每个节点交给现有 `run_tracked` 子代理链路，
  工作目录由调度器分配的 worktree 决定。

因此，这是**多子代理编排的结果汇总面板**，不是单个子代理的会话窗口。
当前代码存在真实执行链，但不能从一张截图证明该次运行、并行度或隔离实际成功。
本轮只核对了实现，没有运行真实模型或重新测试隔离。

## 三个方向

| 方向 | 信息组织 | 适用场景 |
| --- | --- | --- |
| A 对话内联（推荐） | 协作摘要 + 三名执行代理的紧凑列表，整体可折叠，任务详情按需展开 | 日常对话，不挤占主回答 |
| B 并行泳道 | 共同的规划上游 → 三条并行泳道 → 汇总验证 | 展示角色分工和并行关系 |
| C 执行账本 | 左侧任务索引 + 右侧结果、依赖与数据字段 | 失败诊断和历史复查 |

这是基于现有产品样式的本地设计：沿用 `#111827` 主色、细灰边、低饱和状态色，
控件视觉与项目 shadcn Button / Item 一致。独立 HTML 使用语义化 HTML 控件，
不引入另一套生产组件或运行时。

## 数据与交互边界

- 原型优先展示当前节点 DTO 已有的目标、角色、状态、依赖和摘要；
  将 `plan_execute`、节点 ID 等技术字段退至次级信息。
- 不虚构真实代理会话 ID、worktree 路径、模型费用、工具日志或单节点耗时。
  当前聚合器没有填写节点耗时，原型也不补假值。
- 所有角色、任务结果、测试数量都是固定演示样本，不代表本项目的实际运行结果。
- “重放演示”仅是本地状态动画，不是重试任务；没有取消、授权、重试等真实运行操作。
- “失败阻断”里的下游保持等待并注明依赖未满足；它是交互场景示意。
  生产聚合器目前会把调度器 skipped 节点映射为 failed，正式落地时须显式对齐，
  不能直接把原型的展示状态当作已有协议。
- 这些方向不代表当前 DAG 汇总 DTO 已支持实时更新。
  现有该面板从工具输出解析聚合结果；真实实时过程需要接已有事件链，不能靠动画冒充。
- A / B 中点击任务显示详情；C 中选中任务后切换右侧内容。
  方向标签支持方向键、Home / End；整体折叠保留过渡，尊重减少动态效果设置。

## 验证

交付前检查 HTML 中嵌入 JavaScript 的语法，并在浏览器验证三种布局、
状态切换、整体折叠、任务选择、依赖详情和演示重放。
不涉及生产 Rust / TypeScript 修改，不以此原型宣称完整项目构建通过。

图标来自项目已安装的 `lucide-react`，内嵌图标由库组件生成，
未手工绘制；许可文本保留在 HTML 的 `icon-license` 区域中。
