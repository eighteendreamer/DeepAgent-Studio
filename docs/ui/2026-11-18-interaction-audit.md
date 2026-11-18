# 模型页 B 布局与交互响应检查

日期：2026-11-18。用户选择 B：紧凑供应商侧栏，右侧模型表格。

## 修复范围

- `ModelsSettings.tsx`：供应商侧栏与模型表格；已保存的模型按供应商缓存，切换不重新请求。主动拉取单独进行，不阻止切换；请求版本隔离迟到的结果。启用状态立即反馈，持久化失败回滚。模型表格复用项目已有 `VirtualList`，仅挂载可见行；固定行高省去逐行 DOM 测量和尺寸监听，搜索重置视口。
- `CustomTitleBar.tsx`、桌面 `lib.rs`：React 直接拥有顶栏，不再等待 decorum 的页面加载注入。沿用 Tauri 窗口 API 和已授权的 decorum 贴靠命令。
- `GlobalModal.tsx`：受控关闭同步卸载，消除“内容先消失、外壳继续关闭动画”。打开时内容直接可见。
- `SettingsView.tsx`、`App.tsx`：稳定页面身份，保留访问过的设置分类和技能、知识库、插件、自动化页面；移除导航截图等待和页面淡入。按导航意图预载代码。首次访问仍需要加载代码与数据。
- `SurfaceVisibility.ts`、`KnowledgeGraph.tsx`：隐藏页面关闭其模态浮层，暂停知识图谱动画及尺寸监听。
- `RemoteSidebar.tsx`：浏览器预览没有原生 Webview，不注册原生文件拖放；此路径的异常在脚本覆盖主导航后被发现。

这是本地 UI 生命周期设计，复用现有 React、shadcn、虚拟列表及 Tauri 边界；未增加数据库、业务协议、权限或供应商请求。

## 测量方式

脚本：`apps/desktop/scripts/check-ui-responsiveness.browser.cjs`，生产构建夹具：`ui-audit-build.cjs`。

使用 Playwright 驱动无头 Edge，不使用桌面控制插件。页面是完整 App 的实际组件树；API 使用浏览器已有 mock，模型注册表使用三个供应商、每家默认 40 个模型的固定夹具。注册表请求固定等待 120ms，不读取真实数据库、密钥，也不调用供应商服务。

- `handlerMs`：程序触发点击或输入事件的同步执行耗时。不同组件可能同步提交或异步调度 React，因此不能把它单独当成界面响应时间。
- `domMs`：从触发事件到 MutationObserver 观察到指定 DOM 条件。
- `nextFrameMs`：达到 DOM 条件后的下一次 requestAnimationFrame 回调；不是物理屏幕完成显示的时间。
- `requests`：该次交互期间 API 调用数，区分缓存命中与重新请求。
- 弹窗关闭额外逐帧检查是否出现只有外壳、没有表单的状态。

覆盖 17 个设置分类的冷/热切换、供应商切换、搜索/清除搜索、启用、模型编辑开关、供应商弹窗开关、六个主页面的冷/热切换。另检查滚动至列表最后一项、虚拟列表挂载数、隐藏页面浮层和 1280px 布局。

1x 和 6x 使用 Chromium CDP 的 CPU 降速。6x 是受控减速实验，不代表所有低配置设备。首次加载、真实 Tauri IPC、文件系统、外部网络、聊天发送、SSH 连接、原生窗口操作及物理显示延迟不属于这份浏览器计时的完整覆盖范围。

复跑示例（工作目录 `apps/desktop`，环境需要可加载 Playwright）：

```powershell
node scripts/check-ui-responsiveness.browser.cjs --production --label=acceptance-production
```

脚本自动检测本机 Edge，也支持 `UI_AUDIT_BROWSER`。可用 `NODE_PATH` 指向已有 Playwright 环境，无须向项目新增运行依赖。

`UI_AUDIT_RATES=1,6` 控制降速档位，`UI_AUDIT_MODEL_COUNT=1000` 用于大列表。复用已生成的隔离构建时设置 `UI_AUDIT_SKIP_BUILD=1`、`UI_AUDIT_BUILD_LABEL=acceptance-production`，并另取结果标签，避免覆盖常规样本。

## 验证记录

- `pnpm build` 通过：525 项测试、TypeScript 检查、Vite 生产构建；构建存在既有的文件预览依赖浏览器外置模块和大 chunk 提示。
- 顶栏、模型缓存并发、插件页面生命周期、侧栏行为定向测试 23 项通过。
- `cargo check --manifest-path apps/desktop/src-tauri/Cargo.toml --offline` 通过。
- `cargo fmt --manifest-path apps/desktop/src-tauri/Cargo.toml -- --check` 通过。
- 原生窗口按钮用隔离的 API mock 验证调用、最大化状态、监听清理和贴靠定时取消；没有操作用户实际窗口。未执行完整 Rust workspace 测试。

完整脚本结果、截图和 CPU profile 写入 `apps/desktop/target/ui-responsiveness/`，不提交构建产物或本地日志。

## 最终生产计时

数据：`target/ui-responsiveness/acceptance-production/measurements-{1,6}x.json`。每家 40 个模型。下表是 DOM 条件就绪耗时，单位 ms；样本数是每个 CPU 档位的交互次数。小样本 P95 仅描述本次观测，不代表稳定的总体上限。

| 场景 | 样本数 | 1x P50 / P95 | 6x P50 / P95 |
| --- | ---: | ---: | ---: |
| 设置分类回访 | 17 | 17.3 / 22.4 | 77.5 / 273.5 |
| 供应商切换 | 8 | 16.4 / 20.0 | 220.8 / 277.9 |
| 添加供应商弹窗打开 | 3 | 12.6 / 22.3 | 127.1 / 267.7 |
| 搜索、清除、启用、编辑开关 | 5 | 12.3 / 22.2 | 128.3 / 219.2 |
| 主页面回访 | 6 | 17.6 / 32.5 | 84.0 / 207.9 |
| 主页面首轮访问 | 6 | 79.4 / 98.9 | 768.2 / 2102.8 |

首轮指本次 App 生命周期内的访问轮次，并不代表 HTTP 缓存、操作系统文件缓存全部清空。首页代码此前已在启动时加载。首页和远程连接侧栏仍在进入时读取其自身数据；未声称所有页面均零请求。

两个 CPU 档位的功能断言均通过、未捕获异常均为 0：每家供应商的已保存模型只读取一次，八次切换不新增读取；设置分类回访不重新请求；技能、知识库、插件、自动化回访复用状态；供应商弹窗关闭没有空壳；隐藏设置页没有遗留模态浮层。

大列表压力检查也已通过：使用同一份最终生产构建，每家供应商 1000 个模型，1x CPU，共三家供应商。结果位于 `target/ui-responsiveness/stress-production/measurements-1x.json`，未捕获异常为 0，模型注册表总读取 3 次，八次供应商切换均无新增请求。滚动末尾、从底部搜索与清除、虚拟列表挂载上限及弹窗空壳检查均通过。此轮供应商切换 DOM 耗时 P50 / P95 为 33.0 / 39.4ms，仍未达到 10ms 目标。

修复前开发模式的记录在 `target/ui-responsiveness/before/`，可复核每次供应商切换重复请求、顶栏缺失和弹窗空壳。该记录与上述生产构建不作耗时百分比对比。

## 验收边界

顶栏、弹窗空壳、供应商重复请求及设置分类重复挂载是可独立验证的修复项。“任何电脑、所有交互均在 10ms 内完成”不是当前已经达成的结果。应同时查看同步执行、DOM 更新和后端等待，不能只摘取较小的数字作为验收。

本轮工作依赖工作区既有的未提交多协议模型设置实现；提交前必须隔离本轮差异，不得把无关业务变更或已有删除一并提交。
