# 子代理

## 30. 子代理与 worktree

`deepagent-subagents` 提供 DAG 调度和 git worktree 隔离：

- `dag`/`planner` 负责任务拓扑。
- `scheduler` 按依赖调度子代理。
- `worktree` 为子任务隔离文件系统状态。
- `deepagent-builtins::TaskTool` 是运行时暴露给模型的子代理工具入口。

适合把大任务拆成并行子任务，例如调研、实现、测试、代码审查等。

