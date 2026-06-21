# Implementation Plan — Office Agent（录音与文件预览）

> 分 4 个阶段交付，每阶段自成可用增量。任务按依赖顺序排列；每项标注对应需求。
> 约定：不改现有命令签名；新能力以新模块/服务/工具加入；Rust 过 `cargo clippy` 无新告警并附单测；前端过类型检查。

## Phase 1 — 右侧入口 + 插件 UI + 基础预览

- [x] 1. 扩展前端插件类型与快捷入口卡片
  - 在 `ChatView.tsx` 的 `PluginType` 追加 `"recording" | "file_preview"`（不改现有成员）
  - 在 `TOOL_CARDS` 追加「录音」「文件预览」两张卡片（图标 microphone / file-lines）
  - 在底部面板与右侧边栏两处渲染分支各追加 `recording` / `file_preview` 的渲染
  - _Requirements: 1.1, 1.2, 1.3, 1.4, 1.6_

- [x] 2. 增加 i18n 文案
  - 在 `locales` 增加 `chatView.tools.recording` / `recordingDesc` / `file_preview` / `filePreviewDesc`（中英）
  - _Requirements: 1.5_

- [x] 3. 新建 `FilePreviewService`（app-core）与基础预览命令
  - `crates/deepagent-app-core/src/file_preview_service.rs`：`get_metadata`、`extract_text`（txt/md/json/csv 直读；docx/pptx 用 zip + 手写 XML run 提取；xlsx 用 calamine）
  - `dto.rs` 增 `PreviewResult` / `PreviewMetadata` / `SheetPreview`，`lib.rs` 导出
  - 单测：样例文件的文本提取与元数据
  - _Requirements: 7.1, 7.2, 7.5, 7.6, 7.7, 7.8, 12.2_

- [x] 4. 注册预览 Tauri 命令
  - `lib.rs`：`AppState` 加 `preview: Arc<FilePreviewService>`，`setup` 构造，`invoke_handler!` 注册 `preview_open_file` / `preview_extract_text` / `preview_get_metadata` / `preview_read_data_url`
  - _Requirements: 7.1, 12.1_

- [x] 5. 实现 `FilePreviewPlugin.tsx`
  - 文件选择器（复用 `dialog:allow-open`）+ 元数据条
  - 渲染分发：文本/json、csv 表格、图片（data URL 直显）、docx/pptx 文本、xlsx sheet+前 N 行
  - 解析失败/不支持类型的可读提示
  - `api.ts`/`types.ts` 增封装与类型
  - _Requirements: 6.1, 6.2, 6.5, 7.2, 7.3, 7.5, 7.6, 7.7, 12.3_

- [x] 6. 实现 `RecordingPlugin.tsx` 的 UI 骨架（无后端）
  - 状态机 `idle|recording|paused|transcribing|done|error` + 控制按钮 + 计时器 + 录音中显著指示
  - 「转写/生成纪要/导出」入口（Phase 2/3 前先置灰标注「即将支持」）
  - _Requirements: 2.1, 2.2, 2.3, 2.5_

- [x] 7. PDF 基础预览（降级优先）
  - `FilePreviewService` 增 pdf 文本提取（pdf-extract，Tier C）；缺渲染运行时时仅返回文本
  - 前端 pdf 分支：有页面图则显示，否则展示文本
  - _Requirements: 7.1, 7.4_

## Phase 2 — 录音采集 + 运行时框架 + 离线转写

- [x] 8. 新建 `RecordingService`（app-core）+ 音频采集
  - 用 `cpal` 列设备/采集，`hound` 写 16kHz 单声道 PCM WAV 到 app_data 下 `recordings/`
  - `RecordingSession` 生命周期管理；`dto.rs` 增 `RecordingSessionDto`
  - 单测：状态机迁移（start→pause→resume→stop）
  - _Requirements: 3.2, 3.3, 3.4, 3.6, 12.2_

- [x] 9. 注册录音 Tauri 命令 + 麦克风权限
  - `audio_list_input_devices` / `audio_start_recording` / `audio_pause_recording` / `audio_resume_recording` / `audio_stop_recording`
  - 首用麦克风触发系统授权；权限被拒返回明确错误；`capabilities/default.json` 按需加媒体权限
  - 录音进度用 `app.emit("recording:progress", ...)`
  - _Requirements: 3.1, 3.5, 11.4_

- [x] 10. 新建 `RuntimeService`（app-core）+ 按需下载安装框架
  - 运行时注册表（id/version/各平台 URL+SHA-256+目标子目录）；`resolve(capability) -> TierR|TierC`
  - 安装根目录：可执行文件同级 `runtimes/`，只读时回退 `app_data_dir()/runtimes/`；不写系统目录/PATH、不需管理员
  - 下载（流式 + `runtime:progress`）→ SHA-256 校验（失败即删）→ 便携包就地解压；支持取消/卸载
  - `locate_runtime_dir(install_root, sub)`（镜像 `locate_builtin_skills_dir`）+ 单测；安装根目录回退单测；校验失败单测
  - 命令：`runtime_list` / `runtime_status` / `runtime_install` / `runtime_cancel` / `runtime_uninstall`
  - _Requirements: 9.1, 9.2, 9.4, 9.5, 9.6, 9.7, 9.8, 9.9, 11.7_

- [x] 11. 新建 `SpeechService` + `speech_transcribe_file`（进程内引擎 + 模型按需下载）
  - 用 `whisper-rs`（静态链接，无 sidecar 进程）转写；模型经 `RuntimeService` 从托管目录加载
  - 模型缺失 → 结构化错误（指明需下载 `whisper-base`），UI 触发 `runtime_install`
  - 解析输出为 `TranscriptSegment[]`，写 `_转写.json`
  - 单测：whisper 输出 → segments 解析（固定样例，不真跑引擎/不真实下载）
  - _Requirements: 4.1, 4.2, 4.3, 4.4, 4.5, 4.6, 12.2_

- [x] 12. 运行时下载 UI（缺失提示 + 同意安装 + 进度）
  - 缺失运行时/模型时弹下载提示（说明体积、固定来源），用户同意走 `runtime_install` + 进度条 + 可取消
  - 拒绝则回落 Tier C（语音例外：引导下载模型）
  - _Requirements: 9.3, 9.7, 11.7, 13.6_

- [x] 13. RecordingPlugin 接通采集与转写
  - 录音→保存→转写→展示 segments；转写忙碌态与 `error` 态；模型缺失走下载 UI
  - _Requirements: 2.4, 2.6_

- [x] 14. 会议纪要（Markdown）生成
  - `speech_generate_meeting_minutes`：转写交 `ChatService` 整理为结构化纪要（主题/时间/参会人/摘要/决策/待办/风险/原始转写）
  - 导出 Markdown 到 `recordings/`
  - _Requirements: 5.1, 5.2, 5.4_

## Phase 3 — Office 三层服务、工具层与上下文接入

- [x] 15. 新建 `OfficeService`（app-core）+ 三层解析 + Tier C 生成
  - `resolve(cap)` 经 `RuntimeService` 选 Tier R/C；读取与基础生成走纯 Rust（zip/xml、calamine、OOXML writer）
  - **Tier C 文档生成**：注入对应 skill 知识 + `ChatService` 产出结构化文档规格 → 纯 Rust 物化为 `.docx/.xlsx/.pptx`（不跑 Python、不直吐二进制）
  - 单测：三层解析选择、纯 Rust 读写往返、规格→文件物化
  - _Requirements: 8.4, 8.5, 8.6, 13.1, 13.2, 13.3, 13.4, 13.7, 12.2_

- [x] 16. 新建 `office_tools.rs`（deepagent-builtins）
  - 按现有 builtin 模式注册 `office.docx.read/create/edit`、`office.xlsx.read/create/recalculate`、`office.pptx.read/create`、`office.pdf.render`、`office.file.preview`
  - 经 `OfficeBackend` 桥接到 `OfficeService`（对层级无感）；标注 RiskLevel（read/preview 低、create/export 中、overwrite/delete 高）
  - _Requirements: 8.1, 8.2, 8.3, 11.5_

- [x] 17. 会议纪要导出 docx
  - `speech_generate_meeting_minutes` 链路接 `office.docx.create` 导出 `.docx` 到 `recordings/`，写文件前走审批
  - _Requirements: 5.3, 5.4, 11.2_

- [x] 18. xlsx/pptx 预览增强
  - xlsx：多 sheet + 前 N 行；pptx：文本 + 缩略图（缩略图依赖 Tier R）
  - _Requirements: 7.6, 7.7_

- [x] 19. 录音/预览结果接入聊天上下文
  - 录音完成生成卡片「是否生成会议纪要？」；预览「发送到对话」生成卡片 + 注入 `<office-context>` 块
  - `preview_send_to_chat(path, mode)` 经 `ChatService` 上下文/工具结果机制注入（遵守 token 预算与审批）
  - _Requirements: 10.1, 10.2, 10.3, 10.4_

## Phase 4 — Tier R 高保真补全与回归

- [x] 20. 接入 pdfium（PDF 页面渲染）
  - `office.pdf.render` / `preview_render_pages` 在 pdfium 就绪时转页面图片，缺失时降级文本
  - pdfium 注册到 `RuntimeService`（各平台便携 dll/so/dylib）
  - _Requirements: 7.4, 9.1, 13.5_

- [x] 21. 接入 pandoc / LibreOffice（高保真生成与旧格式）
  - pandoc：md↔docx 高保真转换；LibreOffice：旧格式 .doc/.xls/.ppt 与高保真转 PDF
  - 注册到 `RuntimeService`（便携包就地解压）；随附 GPL/LGPL license 文本与来源说明
  - Tier R 命中时 `OfficeService` 走高保真路径，否则保持 Tier C
  - _Requirements: 8.4, 9.1, 9.3, 13.1, 13.7_

- [x] 22. 文件修改前后预览 + 审批完善
  - 高风险（覆盖/删除/外发/打印）强制审批卡；office.* 工具与运行时下载风险级别接入权限规则；与「工作模式」解耦
  - _Requirements: 11.1, 11.2, 11.3, 11.5, 11.6, 11.7_

- [x] 23. 跨平台构建与回归
  - Windows/macOS/Linux 构建；平台相关能力（录音设备、运行时二进制、安装根目录回退）降级验证
  - 全量 `cargo build` + `cargo clippy` + 前端类型检查 + 单测通过
  - _Requirements: 12.1, 12.2, 12.3, 12.4, 12.5_
