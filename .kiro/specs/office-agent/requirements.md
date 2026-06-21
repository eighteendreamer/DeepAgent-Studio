# Requirements — Office Agent（办公 Agent：录音与文件预览）

## Introduction

DeepAgent Studio 当前是一个面向编程的桌面 Agent。本特性把它扩展为「日常办公 + 编程」助手：**不新建复杂的 Office 工作台**，而是在现有右侧快捷入口（文件 / 侧边聊天 / 浏览器 / 终端 / 项目地图）新增两个轻量入口——**录音**与**文件预览**——并把录音、转写、预览、文档生成这些能力接回当前对话上下文。

源方案见 `工作模式-办公开发.md`。核心执行链：

```
用户对话 + 点右侧快捷入口
  → DeepAgent 操作本机文件 / 模型 / 内置运行时
  → 输出 Word / Excel / PPT / 会议纪要 / 摘要 / 预览结果
```

### 硬约束（贯穿所有需求）

- **不破坏现有功能**：现有 5 个快捷入口（files / chat / browser / terminal / project_map）行为不变；现有 90+ Tauri 命令签名不变。
- **复用现有模式**：新插件按 `BrowserPlugin` / `TerminalPlugin` 的方式接入 `ChatView` 的 `TOOL_CARDS` 与渲染分支；新命令按现有 `#[tauri::command]` + `invoke_handler!` + app-core service 模式接入。
- **三层能力解析（核心模型）**：每个办公/语音能力都按固定优先级解析——
  - **Tier R（托管运行时，最高保真）**：应用自带、**按需下载安装**的运行时（pdfium / pandoc / LibreOffice / 语音模型 / OCR）。
  - **Tier C（代码兜底，永远可用）**：无任何外部运行时时，用纯 Rust + **系统大模型 + skills 知识**完成（不是死模板、也不是模型直吐二进制，详见 Requirement 13）。
  - 解析顺序：Tier R 命中则用；否则回落 Tier C；任何一层都不得 panic。
- **运行时按需下载、装进应用自有目录**：重型运行时**一律不随安装包分发**；用户首次需要某能力且缺失时提示下载，**用户同意后自动下载+校验+解压**到**应用自身安装目录下的托管运行时目录**（不写操作系统默认程序目录、不进 PATH、不需管理员）。用户拒绝则回落 Tier C。
- **模型不直接执行命令行**：大模型只能通过受控的 `office.*` / `speech_*` / `runtime_*` 工具与服务交互，不能直接 spawn whisper / pandoc / libreoffice / python。
- **权限分级独立**：录音、文件写操作、运行时下载执行的审批级别独立控制，不被「工作模式」隐式放大。

---

## Requirements

### Requirement 1 — 右侧快捷入口扩展

**User Story:** 作为办公用户，我希望在右侧「+」快捷入口看到「录音」和「文件预览」两个卡片，以便快速触发办公任务。

#### Acceptance Criteria

1. WHEN 用户打开右侧边栏或底部面板的「+ / new」标签 THEN 系统 SHALL 在现有 5 个卡片之后额外展示「录音」和「文件预览」两个卡片，且卡片样式、悬停、点击交互与现有卡片一致。
2. THE `PluginType` 联合类型 SHALL 新增 `"recording"` 与 `"file_preview"` 两个成员，且不移除或改名任何现有成员。
3. WHEN 用户点击「录音」卡片 THEN 系统 SHALL 在当前面板（侧边栏或底部）新开一个录音标签页并渲染 `RecordingPlugin`。
4. WHEN 用户点击「文件预览」卡片 THEN 系统 SHALL 新开一个文件预览标签页并渲染 `FilePreviewPlugin`。
5. THE 两个卡片的标题与描述 SHALL 通过现有 i18n 机制（`chatView.tools.*`）提供，至少包含中文与英文文案。
6. WHERE 标签页可关闭 THE 录音与文件预览标签 SHALL 复用现有标签页关闭逻辑（`xmark` 按钮）。

### Requirement 2 — 录音面板 UI 与交互

**User Story:** 作为用户，我希望在录音面板里开始/暂停/停止录音并看到实时状态，以便录制会议或语音备忘。

#### Acceptance Criteria

1. THE 录音面板 SHALL 展示控制按钮：开始、暂停、继续、停止，以及当前录音时长与状态指示。
2. THE 录音状态 SHALL 取值于 `idle | recording | paused | transcribing | done | error`，且 UI 随状态切换可用按钮（例如 `idle` 时只可「开始」）。
3. WHEN 录音进行中 THEN 面板 SHALL 显著显示「录音中」指示（如红点 + 计时），满足源方案的「UI 明显显示录音中」要求。
4. WHEN 用户点击停止 THEN 面板 SHALL 展示转写入口（「转写为文字」/「生成会议纪要」），并在转写期间显示进度/忙碌态。
5. THE 面板 SHALL 提供「导出 Word / Markdown」入口（Phase 3 落地，Phase 1 可置灰并标注「即将支持」）。
6. IF 录音或转写失败 THEN 面板 SHALL 进入 `error` 状态并展示可读错误信息，不崩溃。

### Requirement 3 — 录音后端（音频采集）

**User Story:** 作为系统，我需要从用户麦克风采集音频并保存为可转写的文件，以支撑会议纪要流程。

#### Acceptance Criteria

1. THE 系统 SHALL 提供命令：`audio_list_input_devices`、`audio_start_recording`、`audio_pause_recording`、`audio_resume_recording`、`audio_stop_recording`，并按现有 `invoke_handler!` 模式注册。
2. WHEN `audio_start_recording` 被调用 THEN 系统 SHALL 创建一个 `RecordingSession`（含 `id`、`status`、`startedAt`），开始采集，并将音频写入用户数据目录下的 `recordings/<时间戳>_<名称>.wav`。
3. THE 采集格式 SHALL 为转写引擎可接受的 PCM WAV（16kHz / 单声道优先），或在保存阶段转换为该格式。
4. WHEN `audio_stop_recording` 被调用 THEN 系统 SHALL 结束采集、刷新文件、更新 `durationMs` 与 `audioPath`，并将 session 置为可转写状态。
5. WHERE 首次使用麦克风 THE 系统 SHALL 触发操作系统麦克风权限申请；IF 权限被拒 THEN 命令 SHALL 返回明确错误而非静默失败。
6. THE 录音产物 SHALL 默认仅保存在本地，不上传任何远端。

### Requirement 4 — 语音转写（进程内引擎 + 按需下载模型）

**User Story:** 作为用户，我希望把录音离线转写成带时间戳的文字，以便后续整理。

#### Acceptance Criteria

1. THE 系统 SHALL 提供命令 `speech_transcribe_file(audio_path)`，由 app-core 的 speech 服务用**进程内引擎**（`whisper-rs`，静态链接 whisper.cpp，无需 `whisper-cli.exe` sidecar）执行转写，而非由大模型直接调用命令行。
2. THE 转写结果 SHALL 为 `TranscriptSegment[]`，每段含 `startMs`、`endMs`、`text`，可选 `speaker`、`confidence`，并写入 `recordings/<时间戳>_转写.json`。
3. THE 语音模型（`base`/`small`/`tiny` 等 ggml 模型）SHALL 一律**不随安装包分发**，而是作为托管运行时资产按需下载（见 Requirement 9）；首次转写若模型缺失，系统 SHALL 提示下载并在同意后自动获取。
4. WHEN 引擎可用但模型缺失 THEN 转写命令 SHALL 返回可操作错误（指明需下载的模型），UI 据此触发下载流程。
5. THE 已下载模型 SHALL 存放于应用托管运行时目录，并通过跨平台资源发现逻辑定位（镜像 `locate_builtin_skills_dir` 的多候选路径策略）。
6. WHERE ASR 无「纯代码无模型」兜底 THE 系统 SHALL 明确：语音转写至少需要下载一次模型；在模型完全不可用时，相关入口 SHALL 引导下载而非静默失败。
7. （可选，Phase 2+）THE 系统 MAY 提供 `speech_transcribe_stream(session_id)` 以支持边录边转写的实时状态。

### Requirement 5 — 会议纪要生成

**User Story:** 作为用户，我希望把转写整理成结构化会议纪要并导出 Word，以便分享归档。

#### Acceptance Criteria

1. THE 系统 SHALL 提供命令 `speech_generate_meeting_minutes(transcript)`，将转写交给现有 `ChatService` 大模型整理为结构化纪要。
2. THE 会议纪要 SHALL 至少包含字段：会议主题、会议时间、参会人、会议摘要、关键决策、待办事项、风险问题、原始转写。
3. WHEN 用户在对话中说「把刚才录音整理成会议纪要 / 把这个音频转成文字并生成 Word」THEN Agent SHALL 能依「录音 → whisper 转写 → 分段整理 → 大模型生成 → `office.docx.create` → 文件预览」链路完成，并在最终写文件前请求确认（见 Requirement 11）。
4. THE 生成的纪要 SHALL 可导出为 Markdown（Phase 2）与 `.docx`（Phase 3），保存至 `recordings/` 目录。

### Requirement 6 — 文件预览面板 UI

**User Story:** 作为用户，我希望选择并在面板中预览办公文件，并能一键把它作为上下文发给当前对话。

#### Acceptance Criteria

1. WHEN 用户打开文件预览面板 THEN 面板 SHALL 提供文件选择器（复用现有 `dialog:allow-open`），可选 `docx/xlsx/pptx/pdf/txt/md/json/csv/png/jpg/webp`。
2. WHEN 选定文件 THEN 面板 SHALL 按文件类型渲染预览（见 Requirement 7），并展示文件名、类型、大小等元数据。
3. THE 面板 SHALL 提供「发送到对话」操作，将文件作为上下文交给当前会话（见 Requirement 10）。
4. THE 面板 SHALL 提供让 Agent「摘要 / 修改 / 转换 / 生成新文件」的快捷提示入口（点击后向对话注入对应指令）。
5. IF 文件类型不支持或解析失败 THEN 面板 SHALL 显示可读提示，不崩溃。

### Requirement 7 — 文件预览后端与渲染策略

**User Story:** 作为系统，我需要按文件类型提取文本/渲染页面/读取元数据，以支撑预览与上下文注入。

#### Acceptance Criteria

1. THE 系统 SHALL 提供命令：`preview_open_file(path)`、`preview_extract_text(path)`、`preview_get_metadata(path)`，并按现有命令模式注册；`preview_render_pages(path)` 与 `preview_send_to_chat(path, mode)` 按 Phase 推进。
2. WHERE 文件为 `txt/md/json/csv` THE 前端 SHALL 直接渲染文本（csv 以表格呈现）。
3. WHERE 文件为图片 THE 前端 SHALL 直接显示。
4. WHERE 文件为 `pdf` THE 系统 SHALL 在 pdfium 运行时（Tier R）可用时将页面转图片预览；运行时缺失时降级（Tier C）为「纯 Rust 提取文本」。
5. WHERE 文件为 `docx` THE 系统 SHALL 提取文本（Rust 直接解析 ZIP/XML 优先），并在 Tier R 可用时可选转 PDF 预览。
6. WHERE 文件为 `xlsx` THE 系统 SHALL 列出 sheet 列表并展示每个表前 N 行。
7. WHERE 文件为 `pptx` THE 系统 SHALL 提取文本并尽可能提供缩略图（缩略图依赖 Tier R）。
8. THE 文本提取实现 SHALL 优先纯 Rust 处理 ZIP/XML（Tier C）；仅在需要更高保真或处理旧格式时使用 Tier R 运行时，且对调用方提供一致接口。

### Requirement 8 — 统一 Office 工具层（AI 可用工具）

**User Story:** 作为 Agent，我希望通过统一受控的 `office.*` 工具读写办公文档，而不是直接执行 skills 里的 Python 脚本。

#### Acceptance Criteria

1. THE 系统 SHALL 在 `deepagent-builtins`/工具层暴露工具：`office.docx.read`、`office.docx.create`、`office.docx.edit`、`office.xlsx.read`、`office.xlsx.create`、`office.xlsx.recalculate`、`office.pptx.read`、`office.pptx.create`、`office.pdf.render`、`office.file.preview`。
2. THE 工具 SHALL 按现有 builtin 工具模式定义（名称、JSON 参数 schema、风险级别、权限），与现有文件/终端工具一致地参与审批流。
3. WHEN 大模型请求生成/编辑文档 THEN 它 SHALL 只能通过这些工具完成，禁止直接 spawn 命令行或 python。
4. THE 工具底层实现 SHALL 按三层解析（详见 Requirement 13）：
   - **Tier R**：托管运行时存在时走高保真路径（pandoc / LibreOffice 转换、pdfium 渲染），或在运行时可用时执行对应 skill 脚本。
   - **Tier C**：无运行时时，由**系统大模型 + skill 知识**产出结构化文档规格，再由**纯 Rust OOXML writer** 物化为 `.docx/.xlsx/.pptx`（不直接吐二进制、不跑 Python）。
5. WHERE 用户机器无 Python / 无任何托管运行时 THE 文档创建与编辑 SHALL 仍可用（走 Tier C）。
6. THE 现有 `.deepagent/skills/docx|xlsx|pptx|pdf` SHALL 保留并承担**双重角色**：(a) 作为 Tier C 注入给大模型的**生成知识**（SKILL.md + 参考文档）；(b) 作为 Tier R 在运行时可用时执行的兼容脚本后端。无论哪种角色，模型都不直接执行其脚本。

### Requirement 9 — 托管运行时：按需下载、装进应用自有目录

**User Story:** 作为用户，我希望安装包保持精简，重型运行时在我需要时由应用自己下载安装到它自己的目录里，全程不污染我的系统；我不装的话功能也能用（降级）。

#### Acceptance Criteria

1. THE 重型运行时（pdfium、pandoc、LibreOffice、语音模型、OCR/视觉）SHALL **一律不随安装包分发**；安装包仅含应用本体、现有 skills、与 Tier C 所需的纯 Rust 能力。
2. THE 系统 SHALL 提供运行时管理命令：`runtime_list`（列出可用运行时及状态）、`runtime_status(id)`、`runtime_install(id)`（下载+校验+安装，发进度事件）、`runtime_cancel(id)`、`runtime_uninstall(id)`。
3. WHEN 用户触发某能力且其 Tier R 运行时缺失 THEN 系统 SHALL 提示该能力可下载更高保真运行时，并说明体积；IF 用户同意 THEN 系统 SHALL 自动下载、校验、安装；IF 用户拒绝 THEN 系统 SHALL 回落 Tier C 完成任务。
4. THE 安装位置 SHALL 为**应用自身安装目录下的托管运行时目录**（例如随应用可执行文件同级的 `runtimes/`）；THE 系统 SHALL NOT 写入操作系统默认程序目录、SHALL NOT 修改系统 PATH/注册表、SHALL NOT 要求管理员权限。WHERE 应用安装于只读位置（如 `Program Files`）THE 系统 SHALL 回退到应用数据目录下的 `runtimes/`（仍属应用自有、可写、可一键卸载）。
5. THE 安装方式 SHALL 优先采用**便携压缩包就地解压**，而非执行会改动系统的厂商安装器（`.exe/.msi`）。
6. THE 每个运行时条目 SHALL 配置固定可信下载 URL（HTTPS）、版本、**SHA-256 校验和**与各平台目标子目录；下载完成 SHALL 校验哈希，校验失败 SHALL 删除并报错，绝不安装未校验产物。
7. THE 下载与安装 SHALL 全程可见进度（`runtime:progress` 事件）、可取消；自动下载并执行二进制属高风险动作，SHALL 需用户显式同意（见 Requirement 11）。
8. THE 运行时定位逻辑 SHALL 复用跨平台多候选路径策略（镜像 `locate_builtin_skills_dir`），缺失时返回清晰诊断而非 panic。
9. THE `tauri.conf.json` 的 `bundle.resources` SHALL 保留现有 `resources/skills/**/*` 条目；除非确有必须随包项，否则不新增重型运行时打包条目（运行时走下载安装目录而非 bundle）。

### Requirement 10 — 录音/预览结果接入聊天上下文

**User Story:** 作为用户，我希望录音与预览不是孤岛，完成后能自然回到当前对话继续让 Agent 处理。

#### Acceptance Criteria

1. WHEN 录音转写完成 THEN 系统 SHALL 在对话中生成一条上下文卡片：「已完成录音转写，是否生成会议纪要？」并提供一键操作。
2. WHEN 用户在预览面板点击「发送到对话」THEN 系统 SHALL 生成上下文卡片：「已打开 xxx.docx，可让 DeepAgent 摘要、修改、转换或生成新版本。」
3. THE 注入给模型的上下文 SHALL 采用结构化块，例如：
   ```
   <office-context>
   当前预览文件: xxx.docx
   文件类型: docx
   提取文本摘要: ...
   可用工具: office.docx.read, office.docx.edit, office.file.preview
   </office-context>
   ```
4. THE 上下文注入 SHALL 复用现有 `ChatService` 的上下文/工具结果机制，不绕过其 token 预算与审批。

### Requirement 11 — 权限与审批分级

**User Story:** 作为用户，我希望危险的办公操作（覆盖、删除、外发）必须经我确认，低风险操作顺畅执行。

#### Acceptance Criteria

1. THE 低风险操作（预览文件、提取文本、生成摘要）SHALL 无需审批直接执行。
2. THE 中风险操作（新建文件、导出文件、保存副本）SHALL 提示但可按现有策略放行。
3. THE 高风险操作（覆盖原文件、删除文件、发送邮件、打印）SHALL 弹出审批卡，必须用户显式批准。
4. THE 录音 SHALL 在首次使用时申请麦克风权限，并在录音中显著提示。
5. THE 各 `office.*` 工具 SHALL 标注对应风险级别，并接入现有权限规则（`get_permission_rules` / `set_permission_rules`）与审批对话框。
6. THE 办公操作的审批级别 SHALL 独立于「工作模式」，不因切换到办公模式而被隐式降低。
7. THE 运行时下载安装（`runtime_install`）SHALL 视为高风险动作：必须用户显式同意才下载并执行/解压二进制；下载源固定可信、HTTPS、SHA-256 校验通过方可安装；与「工作模式」解耦。

### Requirement 12 — 非功能与质量约束

#### Acceptance Criteria

1. THE 变更 SHALL 不新增对现有内核 crate 的破坏性改动；新增能力以新模块/新 service/新工具的形式加入。
2. THE Rust 代码 SHALL 通过 `cargo build` 与 `cargo clippy`（无新增告警），新增服务/工具 SHALL 附带单元测试。
3. THE 前端 SHALL 通过现有构建（`tsc` / 打包）无类型错误。
4. THE 功能 SHALL 跨 Windows / macOS / Linux 可构建；平台相关能力（录音设备、运行时二进制）按平台优雅降级。
5. THE 实现 SHALL 分 4 个阶段交付（见 design 与 tasks），每阶段自成可用增量。

### Requirement 13 — 三层能力解析与代码兜底（LLM + skills）

**User Story:** 作为用户，无论我是否安装了重型运行时，办公能力都要能用；没装时我希望结果仍然专业，而不是简陋模板。

#### Acceptance Criteria

1. THE 每个办公/语音能力 SHALL 通过统一的能力解析器选择实现路径，优先 Tier R（托管运行时），缺失则回落 Tier C（代码兜底），全程不 panic。
2. WHERE Tier C 用于**文档生成**（docx/xlsx/pptx/纪要）THE 路径 SHALL 为：
   - 将对应 skill 的知识（SKILL.md + 参考文档）作为指导注入系统大模型（`ChatService`）；
   - 由大模型按该专业方法产出**结构化文档规格**（如 JSON/中间表示），而非直接输出二进制或最终文件；
   - 由**纯 Rust OOXML writer** 将规格物化为最终 `.docx/.xlsx/.pptx`。
3. THE Tier C 路径 SHALL NOT 在运行时执行 skill 的 Python/Node 脚本（脚本执行仅属 Tier R）；THE Tier C 仅取用 skill 的**文本知识**。
4. WHERE Tier C 用于**预览/文本提取** THE 路径 SHALL 为纯 Rust 解析（zip+xml / calamine / pdf 文本提取）。
5. WHERE Tier C 用于 **PDF 渲染** THE 系统 SHALL 提供最低限度降级（如仅文本或基础位图），并提示安装 pdfium 以获得完整页面渲染。
6. THE 能力解析结果（当前走 Tier R 还是 Tier C、缺失项是什么）SHALL 可被 UI 查询，以便提示「基础模式可用，安装 X 可提升保真度」。
7. THE Tier R 与 Tier C 对同一能力 SHALL 暴露一致的服务接口，使上层（`office.*` 工具 / 命令）无需感知当前层级。
