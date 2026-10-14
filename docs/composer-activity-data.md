# 输入框上方的待办与 Git 变更

## 数据来源

生产页面在 `ChatView` 中绑定会话消息和 `useGitStatus`，不再引用固定示例数据。

- 待办：当前会话最近一次成功的 `todo_write` 或 `task_list` 完整输出。执行中、失败、被拒绝或格式错误的结果不覆盖已有列表；成功的空快照清空列表。
- 历史恢复：从既有会话工具输出重建。旧记录缺少输出时，成功的 `todo_write` 参数作为兼容兜底。运行结束不会自动把未完成项改成完成。
- Git：当前会话保存的项目路径所对应仓库的未提交差异，包含用户手动修改；新会话采用当前选中的项目。提交后相应变更条目消失。远程模式隐藏本地 Git 入口。
- 环境信息弹窗已移除待办摘要；生成文件、图片、链接和既有 Git 工作台入口保留。

## 刷新与隔离

本地设计：工具完成、运行状态变化和会话切换引起的刷新合并等待 400ms。模型文字增量不会触发 Git 查询。打开变更列表时查询变更，窗口回到前台时复查完整 Git 状态。

`useGitStatus` 每个项目只有一个刷新队列，并发触发合并成后续一次查询。项目切换或卸载后，旧请求不能写入当前状态和缓存。缓存升级为 v2，首次使用缓存后仍会检查真实仓库。查询失败显示错误和重试。

会话详情通过既有 `SessionDetailDto` 提供 `version = 1` 和 `project_path`，复用持久化的会话目录，不用显示名称反推路径。打开旧会话时，详情未返回、与当前会话不匹配、旧后端未提供路径或会话没有所属项目时隐藏 Git 入口。详情加载、回退和重命名的延迟返回也按当前会话隔离；环境信息中的 Git 工作台使用同一目录。旧详情反序列化为 v0，无数据库迁移。

## Git 统计契约

`GitChangesDto.version = 1`；逐文件新增 `line_stats_available`。旧数据反序列化为 v0，缺少可用性标记的计数按未确认处理，无数据库迁移。

路径和重命名使用 Git 的 NUL 格式；无 HEAD 的仓库使用缓存区查询，并按当前文件内容计算新文件行数。依据：[git-diff 官方文档](https://git-scm.com/docs/git-diff)、[git-status 官方文档](https://git-scm.com/docs/git-status)。

未跟踪文本文件按实际内容计数。Git 未提供行数的变更，以及非 UTF-8、符号链接、无法读取或超过 8MiB 的新文件显示“未统计”。摘要只累加已确认计数；全部行数未知时隐藏数字，部分未知时添加提示。8MiB 上限是本地资源边界。

## 验证和复核

已执行：

- 11 项前端回归测试：真实格式输出、旧记录、空快照、失败结果、未知行数展示、同名项目的会话目录隔离、详情加载的延迟返回、项目请求竞态、缓存复查、刷新合并和失败重试。
- 真实临时仓库测试：中文/空格路径、重命名、删除、逐个新文件、二进制、无 HEAD、提交后清空及错误传播。
- `cargo test --workspace --offline`：2853 通过，31 忽略。
- `cargo clippy -p deepagent-app-core --all-targets --offline -- -D warnings`、`cargo check --offline --manifest-path apps/desktop/src-tauri/Cargo.toml`、`pnpm build` 均通过。未运行全 features clippy。
- 浏览器验证：待办进度更新和历史快照重载、失败写入保持、空快照清理、Git 逐文件统计、查询失败/恢复、无项目及新会话清理。

浏览器验证页消费真实 GitService 测试生成的返回值，工具事件使用录制格式夹具。未调用付费模型，也未操作用户项目的 Git；未进行原生 Tauri WebView 的真实模型联调。

桌面应用需要重启开发后端或使用重新构建的版本才能收到新增字段；只刷新旧后端的页面不会获得新的统计信息。待办只有工具实际写入或读取快照后才显示，不从普通回复文字推测任务。

生成开发验证数据并启动预览：

```powershell
New-Item -ItemType Directory -Force apps/desktop/target
$env:DEEPAGENT_COMPOSER_GIT_FIXTURE = Join-Path (Get-Location) 'apps/desktop/target/composer-git-verification.json'
cargo test -p deepagent-app-core --offline real_git_changes_include_unicode_rename_deleted_new_and_binary_files
cd apps/desktop
pnpm exec vite --host 127.0.0.1 --port 1441 --strictPort
```

打开 `http://127.0.0.1:1441/scripts/composer-activity-preview.html`。生成数据位于被忽略的 `target` 目录，预览页不进入正式构建入口。

全量 `cargo fmt --all -- --check` 仍受既有 `deepagent-runtime/src/kernel.rs:384` 长类型格式影响；本轮修改的 Rust 文件格式检查通过。
