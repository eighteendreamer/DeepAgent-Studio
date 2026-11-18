# Start → Chat 分段定位与挂载开销修复

## 范围与测量口径

本轮只定位 Start → Chat 的界面开销。没有修改模型协议、模型请求重试、权限、沙箱、远程连接或后端会话链路。测试通过 Playwright CDP 连接真实 Tauri WebView2，向隔离数据根发送本地 `/help`，经过真实桌面 IPC，但不会调用推理接口。没有使用桌面控制插件，也没有替换 Tauri API。

此前的 325.3ms 是单次 **4 倍 WebView CPU 减速**样本，从发送事件到 Chat DOM 出现；它不是纯 `setView` 调用时间，也不是模型首 token 时间。历史样本没有分段记录，不能事后给其各阶段填精确数值。本轮另行重测并明确区分诊断录制和无录制基准。

复测脚本为 `apps/desktop/scripts/profile-start-chat.cjs`。固定渲染视口 1280×800，正常动画，分别记录：

- `dispatchMs`：Enter 事件分发到同步处理返回。
- `afterDispatchToDomMs`：事件返回到 Chat DOM 被 MutationObserver 观察到，包含 React 更新、DOM 提交、同步布局钩子等工作。
- `domMs`：上述两段之和。
- `frame1Ms`、`frame2Ms`：后续两次 requestAnimationFrame 回调时刻。它们是帧边界代理，不是显示器像素完成显示的精确时间。

减速只作用于 WebView CPU，不能代表所有低配置电脑。采样与后台系统负载会影响结果；不作“任意电脑所有交互 10ms”的保证。

## 根因证据

`App.tsx` 中，普通文字发送在调用 `runChat` 前已经设置 seeded messages 和 `view="chat"`。没有图片和待完成项目选择时，这段切换前没有相关 `await`。因此不能把本轮本地命令测得的 UI 延迟归给供应商或数据库等待。

实际渲染是条件卸载整个 `StartView`、新挂载整个 `ChatView`。两个视图分别拥有 Composer；消息时间线、编辑外壳、工具栏及布局钩子也会初始化。它不是已经挂载好的两个节点之间仅切换一个可见状态。

修复前一次 4 倍减速、仅开启 timeline 的诊断录制：DOM 准备 282.3ms，Enter 同步处理 6.3ms；样式计算 52.0ms、同步布局 24.8ms，发送至 DOM 边界内共 18 次布局。其余时间还包含组件树遍历、DOM 创建/删除、钩子和调度，不能将这些类别各自的嵌套耗时重复相加。

进一步的函数计时和 CPU 调用栈定位到三个可消除的生命周期开销：

1. `useSpringPress` 在每个按钮卸载时无条件执行 `gsap.set(clearProps: "scale")`。未发生按压的按钮没有需要清理的缩放；GSAP 的样式/变换处理却仍会运行。交替诊断中，每次约 7–8 个按钮，仅这一项清理就曾占 8.8–55.3ms（4 倍减速）。总延迟同时有波动，这个区间不是整体提速比例。
2. `MorphingMenuShell` 在面板尚未打开时读取 compact 尺寸和 computed style，然后初始化 GSAP 再清理。新 Composer 中的关闭菜单也会重复做这些工作。现在关闭的静止状态直接使用 CSS 布局，打开或退出动画时才测量。
3. `useMessageEditMotion` 在只显示消息气泡时，仍写入隐藏编辑器宽度、重置 textarea 高度、读取 scrollHeight 和编辑器高度。现在关闭状态只测量当前可见气泡，编辑器在真正打开时准备；原有编辑开合动画保留。

首轮完整带 CPU 采样和几何栈追踪的录制会明显扰动耗时，只用于定位调用来源；不将其总时间用于性能对比。

## 回归与复测

先运行回归得到失败，再做修复：未按压按钮清理、关闭菜单测量、隐藏编辑器测量均在修复前失败。修复后 13 项定向测试通过，包含实际按压与释放、减少动态效果、菜单开关恢复、编辑取消与快速重开、长消息和容器尺寸适配。

`pnpm build` 通过：543 项前端测试、TypeScript 检查和 Vite 生产构建。本轮仅修改前端行为，没有改 Rust 生产逻辑。

`cargo build --manifest-path apps/desktop/src-tauri/Cargo.toml --features custom-protocol --offline -j 1` 通过，耗时 5 分 34 秒。随后实际启动这个新可执行文件，使用其内嵌生产资源验收；Rust 为 debug 构建。没有在最终测量中注入诊断源码、替换资源或使用开发服务器。

每组 7 次采样，中位数如下，各列分别求中位数，不能将不同列的中位数当作同一次采样逐项相加：

| 指标 | 修复前 1x | 修复后 1x | 修复前 4x | 修复后 4x |
| --- | ---: | ---: | ---: | ---: |
| Enter 同步处理 | 1.1ms | 1.4ms | 10.6ms | 12.1ms |
| 事件返回至 Chat DOM | 63.9ms | 25.0ms | 285.6ms | 250.0ms |
| 发送至 Chat DOM 总计 | 64.7ms | 26.4ms | 297.6ms | 260.7ms |
| 第一帧回调 | 70.8ms | 33.0ms | 351.2ms | 300.0ms |
| 第二帧回调 | 85.3ms | 48.0ms | 472.0ms | 428.8ms |

首次发送样本单列：1x 为 66.9 → 61.5ms，4x 为 261.8 → 263.3ms。首次挂载仍然存在；不能将重复切换的改善泛化为首次发送也已明显加速。

另行开启 timeline 的单次诊断中，发送至 DOM 的同步布局由 18 次降至 6 次。不过修复后的录制总时间为 335.6ms，样式计算 55.2ms、布局 37.1ms，并未比前一次录制更短。录制与机器调度有波动；布局次数减少是机制验证，无录制基准另行比较，不能选择性报告录制总时间或据此保证所有场景更快。

新程序的原生交互脚本还验证了两个菜单的展开/收起、隐藏编辑器按需打开及取消恢复、发送动画 25 个有效帧和最终气泡可见；页面异常为 0。已查看原生菜单截图。整个主区域没有出现 Suspense loading。

**仍未达到任意电脑、所有交互 10ms 的要求。** 当前 Start → Chat 仍包含完整 Chat 子树创建、必要布局和发送动画的目标测量。这个回合消除了三处确定的额外工作，没有把界面改成只需更新可见性的共享持久视图，也没有通过去掉正常动画来缩短指标。

复测可执行文件 SHA256：`7844C98B6A78F555FAC40172EFD23E9C30B01F1414B09DEF544EC548C7E1CADE`。相关三份生产源码与可执行文件哈希保存在 `validated-inputs.json`，复测后再次校验一致。本轮没有运行全仓 clippy 或 Rust workspace 测试；Rust 生产逻辑未变，已实际通过原生构建和桌面验收。

脚本调用示例：为隔离测试进程开启 WebView2 CDP 后，在仓库根目录设置 `DESKTOP_AUDIT_CDP`、`DESKTOP_AUDIT_OUTPUT`、`DESKTOP_AUDIT_CPU_RATE=1` 或 `4`，运行 `node apps/desktop/scripts/profile-start-chat.cjs`。默认采样 7 次；`DESKTOP_AUDIT_TRACE=1` 另外输出带发送边界的 timeline。Playwright 使用已提供的工作区运行时 `NODE_PATH`，未新增项目依赖。

原始诊断、基准 JSON、截图和构建日志保存在忽略目录 `target/start-chat-profile-20261118/`，不提交用户数据库或模型内容。
