# Office Agent 优化与修复建议

日期：2026-06-21

## 1. 必修项

### 1.1 解决 whisper 构建环境问题

现状：`cargo check -p deepagent-app-core --features whisper` 失败，原因是本机缺少 `libclang.dll`，`bindgen` 无法构建 `whisper-rs-sys`。

建议：

1. 开发机和 CI 安装 LLVM，并配置 `LIBCLANG_PATH`。
2. 在 `RUNTIME-LICENSES.md` 或开发文档中写清 whisper feature 的 Windows 构建依赖。
3. 若希望普通用户完全无 C++/clang 依赖，优先改为 `whisper.cpp` sidecar 可执行文件方案：Rust 只调用托管 runtime，不在主程序内编译 whisper-rs。

优先级：P0。没有这一步，真实本地语音转写无法完成发布级验证。

### 1.2 补齐真实端到端手测

自动化已覆盖服务层，但以下体验必须手测：

1. 麦克风授权弹窗。
2. 开始、暂停、继续、停止录音。
3. whisper-base 模型下载、校验、安装。
4. 音频转写、会议纪要生成、Word 导出。
5. 文件预览发送 `<office-context>` 后，模型是否能正确基于上下文回答。
6. 高风险动作审批卡是否正常阻断。

优先级：P0。办公 Agent 是强交互功能，只靠单测不能证明用户体验成立。

### 1.3 给 runtime registry 补真实 pin

现状：RuntimeService 已做 fail-closed，如果没有 pinned SHA-256 会拒绝安装，这是正确设计。

建议：

1. 为 `whisper-base`、`whisper-small`、`pdfium`、`pandoc`、`libreoffice` 建立正式 release manifest。
2. 每个平台记录 URL、文件名、SHA-256、体积、许可说明。
3. UI 下载卡展示体积、来源、许可和风险说明。

优先级：P0。否则用户首次下载模型/运行时时会被 fail-closed 阻断。

## 2. 高价值优化

### 2.1 录音与转写从“可用”升级为“可靠”

建议增加：

1. 录音设备选择。
2. 输入音量电平显示。
3. 录音文件损坏检测。
4. 长会议分段转写。
5. 转写失败后保留音频并给出重试入口。

优先级：P1。

### 2.2 文件预览增强

建议增加：

1. PDF 安装 pdfium 后的页面缩略图和页码。
2. xlsx 多 sheet 切换与行列上限提示。
3. docx/pptx 的结构化预览，如标题、段落、表格、幻灯片编号。
4. 大文件预览分块加载。
5. 预览失败时展示可读错误和“发送文件路径给 Agent”的降级入口。

优先级：P1。

### 2.3 Office 工具命名统一

当前底层工具名是 `office_read`、`office_docx_create`、`office_xlsx_create`，方案里多处使用 `office.docx.read`、`office.docx.create` 这类命名。

建议：

1. 对模型暴露的工具命名统一一套规范。
2. 如果保留下划线命名，在文档中同步修正。
3. 给旧命名保留 alias，避免已有 prompt 或 skill 失效。

优先级：P1。

### 2.4 审批与覆盖保护做 UI 回归

建议补测：

1. 新建文件。
2. 覆盖已有文件但未传 `overwrite=true`。
3. 覆盖已有文件且用户拒绝审批。
4. 覆盖已有文件且用户同意审批。
5. 删除、打印、外发等未来高风险动作的审批占位。

优先级：P1。

## 3. 架构后续项

### 3.1 Connections 不要混进办公面板

`DeepAgent Studio 办公与现实交互开发方案.md` 的完整愿景需要 Connections，但当前办公 MVP 不应把复杂配置塞进录音/文件预览面板。

建议：

1. 新建 `Settings -> Connections`。
2. MCP、OAuth、浏览器、视觉、设备网关都归入 Connections。
3. 右侧面板只显示当前会话用到的轻量状态。

优先级：P2。

### 3.2 视觉与 computer-use 分阶段推进

不要直接让大模型裸坐标点击。

建议阶段：

1. `vision.capture_screen`、`vision.inspect_window`、OCR。
2. 输出结构化 `control_id`。
3. `computer.click_control(control_id)` 基于控件 ID 执行。
4. 每次动作写入 action receipt，支持回放和审计。

优先级：P2。

### 3.3 办公能力是否拆成 `deepagent-office`

当前实现放在 `deepagent-app-core` 可以接受，因为 MVP 仍在收敛阶段。

建议在以下条件满足时再拆 crate：

1. OfficeService 超过 app-core 的职责边界。
2. Connections/OAuth、Office provider、文档引擎都稳定后。
3. 需要复用到 CLI、服务端或插件运行时。

优先级：P3。现在不建议急着拆。

## 4. 建议新增测试

1. UI 端到端：录音卡片打开、状态变化、停止后出现转写按钮。
2. UI 端到端：文件预览选择 txt/docx/xlsx/pdf，并发送到聊天。
3. Runtime 下载 mock：进度事件、取消下载、校验失败 UI 提示。
4. Approval e2e：`office_docx_create` 覆盖已存在文件时必须审批。
5. whisper 环境 CI：Windows 带 LLVM 的 feature check。
6. 发布构建：`npm run build` 与 Tauri 打包。

## 5. 推荐落地顺序

1. 配好 LLVM/libclang 或切换 whisper sidecar。
2. 补 runtime manifest 的真实 SHA-256。
3. 做录音到会议纪要到 Word 的人工验收。
4. 做文件预览到聊天上下文的人工验收。
5. 补高风险审批 UI 回归。
6. 再推进 Connections、Vision、computer-use。

## 6. 一句话判断

当前办公 MVP 方向是对的，基础工程也已经能通过主要自动化门禁；下一步不要急着扩平台能力，先把语音 runtime、真实设备、审批 UI 和发布构建这四个交付风险补牢。
