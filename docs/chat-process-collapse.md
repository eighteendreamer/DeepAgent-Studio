# 执行过程自动收起

## 行为

聊天中的执行摘要只显示两类信息：运行时显示“正在处理”，结束后显示“已处理”；结束后的摘要附带本轮耗时。工具次数、步骤数和异常数量不再占用摘要行，详细信息仍保留在展开内容中。

输出总结时，执行过程会自动收起。后续又开始工具调用、重试、验证或等待审批时，过程会自动展开。成功、失败、取消、阻塞、达到最大轮数、预算耗尽和上下文耗尽都会进入已处理状态并收起。

用户手动展开后，展开状态会跨虚拟列表行的卸载和重新挂载保留。再次整体收起会重置内部思考、工具和 DAG 的展开状态。收起时如果焦点在详情里，焦点会移到摘要按钮；阅读历史时，新内容不会强制把滚动位置拉到底部。

## 实现边界

- `ProcessDisclosureStore` 由 `App` 持有，状态不放在虚拟化的行组件中。
- 运行身份沿用现有 run/event 投影；历史回放只增加可选的稳定消息 ID 和版本化 turn 元数据，不新增运行状态库或数据库迁移。
- 所有工具执行、审批、权限和取消仍走现有运行时链路。

## 验证

本轮验证包括：

- `apps/desktop/scripts/check-process-collapse.test.cjs`：12 项通过，覆盖输出收起、所有结束状态、展开状态保存、滚动锚点和旧运行尾部事件隔离。测试提取并执行实际 `App` 累积器，避免另写一套事件处理模拟实现。
- `cargo test --workspace --offline`：2850 项通过，31 项忽略。
- `cargo check --manifest-path apps/desktop/src-tauri/Cargo.toml --offline`：通过。
- `pnpm build`（`apps/desktop`）：通过。
- 相关 crate 的 `cargo clippy --all-targets --offline -- -D warnings` 和本轮 Rust 文件格式检查：通过。

浏览器验证使用实际聊天组件，已确认失败后收起、手动展开、48→49 轮虚拟列表切换、阅读锚点和收起后的焦点回收。最后的摘要文字简化后，浏览器工具因 URL 策略拒绝访问本地验证页，未重新截图。未运行原生 Tauri WebView 或真实模型联调；Tauri 此处仅做编译检查。

本地手动复核入口：在 `apps/desktop` 执行 `pnpm exec vite --host 127.0.0.1 --port 1431 --strictPort`，打开 `http://127.0.0.1:1431/scripts/timeline-verification.html`。这是固定输入的开发验证页，不进入正式产品路由、不调用模型、不写会话数据。

`cargo fmt --all -- --check` 仍会报告既有的 `crates/deepagent-runtime/src/kernel.rs` 长类型格式差异；本轮未修改该无关文件。
