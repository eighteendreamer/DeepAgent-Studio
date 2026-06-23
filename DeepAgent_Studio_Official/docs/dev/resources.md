# 脚本与资源

## 32. 辅助脚本

| 脚本 | 用途 |
| --- | --- |
| `scripts/inspect_db.py` | 检查 SQLite 数据库内容 |
| `scripts/watch_db.py` | 观察数据库变化 |
| `scripts/probe_deepseek.py` | 探测 DeepSeek API/模型响应 |
| `scripts/kiro_auto_confirm.py` | Kiro 自动确认辅助 |

这些脚本是开发调试工具，不属于应用运行时主链路。

## 33. `.deepagent` 工作区资源

`.deepagent` 在仓库内承载可被本项目自身 Agent/技能系统发现的资源：

- `.deepagent/agents/rust-architect.md`：工作区 agent 定义。
- `.deepagent/commands/review.md`：动态命令文件。
- `.deepagent/skills/docx|pdf|pptx|xlsx`：Office/PDF 类技能和脚本。
- `.deepagent/skills/superpowers`：一组通用开发方法技能及测试素材。
- `.deepagent/skills/mcp-builder`：MCP 构建技能。
- `.deepagent/skills/webapp-testing`：Web app 测试技能。
- `.deepagent/skills/code-review-skill`：代码审查技能。
- `.deepagent/skills/ui-ux-pro-max-skill`：UI/UX 设计技能。
- `.deepagent/skills/agent-browser`、`planning-with-files` 等：浏览器和计划类技能。

注意：这些目录中包含大量 schema、脚本、测试 fixture、文档和素材。维护指南应关注技能入口 `SKILL.md`、脚本用途、风险和安装来源，而不是逐个展开所有 schema 文件。

## 34. `.kiro` 规格文档

`.kiro/specs` 包含阶段性规格：

- `knowledge-base`：知识库需求、设计、任务。
- `office-agent`：Office agent 的需求、设计、任务、测试计划、报告、运行时许可证、优化修复计划。
- `daily-office-mode`：日常办公模式设计与任务。
- `native-project-map`：原生项目地图需求、设计、任务和元数据。

这些规格是理解 feature 背景的补充材料；代码实现以 `crates` 和 `apps` 为准。

