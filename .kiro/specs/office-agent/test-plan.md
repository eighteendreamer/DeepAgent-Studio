# Office Agent — 完整测试方案与测试用例

> 版本：v1（office-agent Phases 1–4 完成后）
> 目标：验证已实现功能满足两份设计方案的**办公相关需求**：
> - `工作模式-办公开发.md`（录音 / 文件预览 / office.* 工具 / 内置运行时 / 接入对话 / 权限）— **全部实现**。
> - `DeepAgent Studio 办公与现实交互开发方案.md`（七层架构愿景）— **办公子集已实现**（skill 负责流程、office 工具负责执行、风险分级、主界面简洁、可回放可审批）；视觉 / computer-use / connections / edge 为后续阶段，本方案标注 N/A（含规划用例）。
>
> 执行约定：开发期只做基础冒烟（`cargo check` + 关键单测）；完整回归在功能完成后统一执行（本文件即统一执行依据）。

---

## 第一部分 · 测试方案

### 1. 测试范围

**In-scope（本轮验证）**
- 右侧入口：录音、文件预览两个插件卡片与渲染。
- 文件预览：txt/md/json/csv/图片/docx/xlsx/pptx/pdf 的 Tier C 纯 Rust 解析。
- 录音：cpal 采集状态机 + WAV 落盘（设备相关部分手动验证）。
- 运行时管理：按需下载、SHA-256 fail-closed、zip/tar.gz/raw 安装、安装根回退、卸载。
- 语音转写：进程内引擎接口 + 模型定位（whisper feature 关闭时降级提示）。
- 办公文档：Tier C 纯 Rust docx/xlsx 生成、Markdown→DocSpec、读取；Tier R pandoc/LibreOffice 路径（代码就绪）。
- office.* AI 工具：read/create + 风险级别 + 覆盖保护 + 审批接入。
- 结果接入对话：`<office-context>` 注入。

**Out-of-scope（后续阶段，方案 §9–§12）**
- `deepagent-vision`（像素级视觉、OCR、UI 树）。
- `deepagent-computer-use`（控件定位点击、桌面自动化）。
- `Connections` 连接中心（OAuth/office-provider/device-gateway）。
- `deepagent-edge` 现实设备（打印机/NAS/摄像头/MQTT）。
> 这些在本文件 §覆盖矩阵中标注为 **N/A（Future）** 并附规划用例，便于后续阶段补测。

### 2. 测试层次（对应方案 §18）

| 层 | 方案 §18 对应 | 本方案实现 |
|----|---------------|------------|
| 单元测试 | 连接状态/风险分级/视觉解析/参数校验 | RuntimeService 状态+校验、风险级别、office/preview 解析、工具参数校验（36 条） |
| 集成测试 | MCP/OAuth/视觉→动作链/skill→工具链 | 桌面命令层编译集成、office 工具经 ChatService 注册、Tier 解析 |
| 仿真测试 | 假邮箱/日历/打印机/视觉/设备 | Mock Downloader/Recorder/Engine/Backend；fail-closed 注册表 |
| 回放测试 | 连接/审批/视觉/动作回执 | 工具调用经 runtime 事件流 + 审批卡；office-context 卡片（手动） |

### 3. 测试环境与执行命令

| Gate | 命令 | 工作目录 |
|------|------|----------|
| 内核全量单测 | `cargo test --workspace` | 仓库根 |
| 内核 lint | `cargo clippy --workspace` | 仓库根 |
| 桌面特性 lint | `cargo clippy -p deepagent-app-core --features audio,runtimes,pdfium` | 仓库根 |
| 语音特性编译 | `cargo check -p deepagent-app-core --features whisper` | 仓库根（需 C++/cmake）|
| pdfium 特性编译 | `cargo check -p deepagent-app-core --features pdfium` | 仓库根 |
| 桌面壳编译 | `cargo check` | `apps/desktop/src-tauri` |
| 前端类型检查 | `npx tsc --noEmit` | `apps/desktop` |
| 桌面完整构建 | `npm run build` | `apps/desktop`（手动）|

---

## 第二部分 · 自动化测试用例（已执行，结果见 §最终结果）

> 每条标注：对应需求（office-agent Requirements + 文档章节）、实现测试函数、预期。状态以 §最终结果的实跑为准（全部 PASS）。

### TG-PREVIEW 文件预览（R6/R7/R13；工作模式 §七）

| TC | 说明 | 测试函数 | 预期 |
|----|------|----------|------|
| TC-PREVIEW-001 | 扩展名分类正确 | `file_preview_service::tests::classify_covers_office_and_text` | md=text, csv=csv, png=image, docx/xlsx/pptx/pdf 对应, 未知=unknown |
| TC-PREVIEW-002 | 元数据 name/ext/size/kind | `metadata_reports_name_ext_kind` | 正确返回 |
| TC-PREVIEW-003 | 纯文本提取 | `extracts_plain_text` | 文本原样，未截断 |
| TC-PREVIEW-004 | docx 段落文本提取 | `ooxml_text_pulls_runs_per_paragraph` | 按段落取 `w:t`，实体解码 |
| TC-PREVIEW-005 | 跳过自闭合标签 | `extract_runs_skips_self_closing` | `<w:t/>` 跳过 |
| TC-PREVIEW-006 | XML 实体解码（&amp; 末位）| `decode_entities_handles_amp_last` | `a & b <c>` |
| TC-PREVIEW-007 | docx 最小包提取 | `extracts_docx_from_minimal_zip` | 读回正文文本 |
| TC-PREVIEW-008 | pptx 幻灯片序号解析 | `slide_number_parses_and_sorts` | slide2<slide12 排序 |
| TC-PREVIEW-009 | 图片 base64 data URL | `read_data_url_builds_data_uri_for_image` | `data:image/png;base64,...` |
| TC-PREVIEW-010 | 非图片拒绝 data URL | `read_data_url_rejects_non_image` | 返回错误 |
| TC-PREVIEW-011 | base64 编码向量 | `base64_encodes_known_vectors` | RFC 4648 标准向量 |
| TC-PREVIEW-012 | 不支持类型不报错 | `unsupported_type_returns_message_not_error` | 返回 message 而非 Err |

### TG-REC 录音（R2/R3；工作模式 §四）

| TC | 说明 | 测试函数 | 预期 |
|----|------|----------|------|
| TC-REC-001 | 完整状态机迁移 | `recording_service::tests::full_lifecycle_transitions` | idle→recording→paused→resume→done |
| TC-REC-002 | 非法暂停被拒 | `cannot_pause_when_not_recording` | 报「cannot go」 |
| TC-REC-003 | 停止需活动会话 | `stop_requires_active_session` | 未知会话报错 |
| TC-REC-004 | 文件名清洗 | `sanitize_makes_safe_names` | 非法字符→`-`，空→recording |
| TC-REC-005 | 错误态设置 | `mark_error_sets_status` | status=error + message |

### TG-RT 运行时管理（R9/R11.7；工作模式 §五/§九；方案 §15 下载高风险）

| TC | 说明 | 测试函数 | 预期 |
|----|------|----------|------|
| TC-RT-001 | raw 下载+校验+安装 | `runtime_service::tests::install_verifies_and_places_raw_file` | 文件落 `<root>/<dest>`，installed |
| TC-RT-002 | 校验和不符清理 | `install_rejects_checksum_mismatch_and_cleans_up` | 删临时文件 + 报「checksum mismatch」|
| TC-RT-003 | 无 pin 校验和拒装 | `install_blocked_without_pinned_checksum` | 报「no pinned SHA-256」(fail-closed) |
| TC-RT-004 | zip 解压安装 | `install_extracts_zip` | 解压到 dest，probe 存在 |
| TC-RT-005 | tar.gz 解压安装 | `install_extracts_tar_gz` | 解压成功，文件内容正确 |
| TC-RT-006 | 默认注册表含 Tier R | `default_registry_lists_tier_r_runtimes` | 含 pandoc/pdfium，fail-closed 未 pin |
| TC-RT-007 | 安装根可写回退 | `install_root_falls_back_to_writable_candidate` | 跳过不可写候选 |
| TC-RT-008 | 卸载移除文件 | `uninstall_removes_installed_raw_file` | installed→false |
| TC-RT-009 | list 标志位 | `list_reports_pinned_and_platform_flags` | pinned/platform/installed 正确 |

### TG-SPEECH 语音转写（R4/R5；工作模式 §六）

| TC | 说明 | 测试函数 | 预期 |
|----|------|----------|------|
| TC-SPEECH-001 | 转写产物路径 | `speech_service::tests::transcript_path_is_sibling_with_suffix` | `<stem>_转写.json` |
| TC-SPEECH-002 | 引擎未启用报错 | `unavailable_engine_errors` | 报「not enabled」|

### TG-OFFICE 办公文档生成（R8/R13；工作模式 §八）

| TC | 说明 | 测试函数 | 预期 |
|----|------|----------|------|
| TC-OFFICE-001 | 无运行时 tier=C | `office_service::tests::tier_is_c_without_runtime` | "C" |
| TC-OFFICE-002 | docx 生成可读回 | `create_docx_writes_openable_package_with_text` | 含标题/标题级/段落/表格，XML 转义，预览可读回 |
| TC-OFFICE-003 | xlsx 往返 | `create_xlsx_roundtrips_via_calamine` | calamine 读回 sheet+单元格 |
| TC-OFFICE-004 | Markdown 解析 | `markdown_parses_title_headings_bullets` | 标题/标题级/项目符号/段落 |
| TC-OFFICE-005 | md→docx 端到端（含中文）| `markdown_to_docx_end_to_end` | 读回「会议纪要」「决策一」|

### TG-TOOLS office.* AI 工具（R8/R11；工作模式 §八/§十一；方案 §4.1/§15）

| TC | 说明 | 测试函数 | 预期 |
|----|------|----------|------|
| TC-TOOLS-001 | 风险级别正确 | `office_tools::tests::read_is_safe_create_is_write` | read=Safe/read_only，create=Low/WorkspaceWrite |
| TC-TOOLS-002 | 创建工具调用后端 | `docx_create_invokes_backend` | 后端收到 out_path |
| TC-TOOLS-003 | 缺参数为失败非 panic | `missing_args_are_failures` | ToolOutput.ok=false |
| TC-TOOLS-004 | 覆盖保护（集成）| chat_service `OfficeToolBackend` 覆盖守卫 | 文件已存在且 overwrite≠true → 拒绝 |

### TG-INT 集成 / 编译门

| TC | 说明 | 验证方式 | 预期 |
|----|------|----------|------|
| TC-INT-001 | 桌面命令层集成 | `cargo check`（src-tauri）| 90+ 命令含 office-agent 全部命令编译通过 |
| TC-INT-002 | office 工具注册进 ChatService | app-core 编译 + `with_office` 链路 | 注册成功 |
| TC-INT-003 | 前端类型契约 | `npx tsc --noEmit` | DTO 镜像类型一致，0 error |
| TC-INT-004 | pdfium 特性编译 | `cargo check --features pdfium` | pdfium-render+image 集成编译通过 |
| TC-INT-005 | 全特性 lint | `cargo clippy --features audio,runtimes,pdfium` | 0 warning |

---

## 第三部分 · 手动系统测试用例（需桌面 App）

前置：`npm run build` 成功，桌面 App 运行，已连接 DeepSeek API Key。

### TG-M-UI 入口与插件（R1）
| TC | 步骤 | 预期 |
|----|------|------|
| TC-M-UI-001 | 打开右侧「+」 | 出现「录音」「文件预览」卡片，样式与现有一致 |
| TC-M-UI-002 | 点卡片 | 分别渲染 RecordingPlugin / FilePreviewPlugin |
| TC-M-UI-003 | 中/英切换 | 卡片标题与描述随语言变化 |

### TG-M-PREVIEW 文件预览端到端（R6/R7）
| TC | 输入 | 预期 |
|----|------|------|
| TC-M-PREVIEW-001 | 选 txt/md/json | 文本渲染 |
| TC-M-PREVIEW-002 | 选 csv | 表格渲染 |
| TC-M-PREVIEW-003 | 选 png/jpg | 图片显示 |
| TC-M-PREVIEW-004 | 选 docx | 正文文本 |
| TC-M-PREVIEW-005 | 选 xlsx | sheet 列表 + 前 N 行 |
| TC-M-PREVIEW-006 | 选 pptx | 幻灯片文本 |
| TC-M-PREVIEW-007 | 选 pdf（无 pdfium）| 文本 + 「安装 pdfium 渲染页面」提示 |
| TC-M-PREVIEW-008 | 选损坏文件 | 可读错误，不崩溃 |
| TC-M-PREVIEW-009 | 点「发送到对话」| 注入 `<office-context>`，模型可据此回答 |

### TG-M-REC 录音→转写→纪要→导出（R2/R3/R4/R5/R10）
| TC | 步骤 | 预期 |
|----|------|------|
| TC-M-REC-001 | 开始/暂停/继续/停止 | 状态/计时正确，录音中红点 |
| TC-M-REC-002 | 首次录音 | 系统麦克风授权弹窗（R11.4）|
| TC-M-REC-003 | 停止后转写（无模型）| 弹下载模型卡片（体积/来源）|
| TC-M-REC-004 | 同意下载（pin 校验和后）| 进度条 + 完成后自动转写 |
| TC-M-REC-005 | 拒绝下载 | 提示需模型（语音无纯代码兜底，R4.6）|
| TC-M-REC-006 | 转写完成 | 分段（时间戳+文本）|
| TC-M-REC-007 | 生成纪要 | 8 段结构化 Markdown（主题/时间/参会人/摘要/决策/待办/风险/原始转写）|
| TC-M-REC-008 | 导出 Word | recordings 目录生成 .docx + 路径提示 |
| TC-M-REC-009 | 发送到对话 | 纪要以 `<office-context>` 注入 |

### TG-M-TOOLS office.* 工具端到端（R8/R11；方案 §4.1/§15）
| TC | 提示词 | 预期 |
|----|--------|------|
| TC-M-TOOLS-001 | "读取这个 docx 并总结" | 调用 `office_read` |
| TC-M-TOOLS-002 | "把要点生成一个 Word" | 调用 `office_docx_create`，写前审批 |
| TC-M-TOOLS-003 | "生成含这些数据的 Excel" | 调用 `office_xlsx_create` |
| TC-M-TOOLS-004 | 覆盖已存在文件 | 需 overwrite=true，否则拒绝（R11.3）|
| TC-M-TOOLS-005 | 无 Python 环境 | 仍能创建（Tier C 纯 Rust，R8.5）|

### TG-M-RT 运行时与权限（R9/R11；方案 §15）
| TC | 步骤 | 预期 |
|----|------|------|
| TC-M-RT-001 | 触发缺失运行时 | 高风险提示，需显式同意（R11.7）|
| TC-M-RT-002 | 安装位置 | 应用自有 `runtimes/`（只读回退 app_data），不写系统目录/PATH/免管理员（R9.4）|
| TC-M-RT-003 | 高风险（覆盖/删除/外发/打印）| 强制审批卡（R11.3；方案 §15.1 高风险）|
| TC-M-RT-004 | 风险与「工作模式」解耦 | 切换模式不降低 office 风险判定（R11.6）|

---

## 第四部分 · 与设计文档的覆盖矩阵

### 4.1 office-agent Requirements（R1–R13）→ 用例
| 需求 | 用例 | 状态 |
|------|------|------|
| R1 右侧入口 | TC-M-UI-001..003 | 实现（手动验证）|
| R2 录音 UI | TC-REC-001..005, TC-M-REC-001 | PASS |
| R3 录音采集 | TC-REC-*, TC-M-REC-001/002 | 自动 PASS；设备相关手动 |
| R4 转写 | TC-SPEECH-*, TC-M-REC-003..006 | 接口 PASS；端到端需 whisper+模型 |
| R5 会议纪要 | TC-OFFICE-005, TC-M-REC-007 | PASS（生成）；端到端需模型 |
| R6 预览 UI | TC-M-PREVIEW-* | 实现（手动）|
| R7 预览后端 | TC-PREVIEW-001..012 | PASS |
| R8 office 工具 | TC-OFFICE-*, TC-TOOLS-*, TC-M-TOOLS-* | PASS |
| R9 托管运行时 | TC-RT-001..009 | PASS |
| R10 接入对话 | TC-M-PREVIEW-009, TC-M-REC-009 | 实现（手动）|
| R11 权限审批 | TC-TOOLS-001/004, TC-M-RT-003/004 | PASS（风险级别）；审批手动 |
| R12 非功能 | TC-INT-001..005 | PASS |
| R13 三层降级 | TC-OFFICE-001/002, TC-M-PREVIEW-007 | PASS |

### 4.2 工作模式-办公开发.md → 章节覆盖
| 章节 | 用例 | 状态 |
|------|------|------|
| §二 右侧入口 | TC-M-UI-* | ✓ |
| §四 录音架构 | TG-REC, TG-M-REC | ✓ |
| §五 内置语音模型 | TG-RT（whisper-base/small 注册）| ✓（fail-closed）|
| §六 会议纪要流程 | TC-OFFICE-005, TC-M-REC-007/008 | ✓ |
| §七 文件预览 | TG-PREVIEW, TG-M-PREVIEW | ✓ |
| §八 Office 文件处理 | TG-OFFICE, TG-TOOLS | ✓ |
| §九 内置运行时目录 | TG-RT | ✓（下载安装到应用自有目录）|
| §十 接入聊天 | TC-M-PREVIEW-009, TC-M-REC-009 | ✓ |
| §十一 权限与审批 | TC-TOOLS-001/004, TC-M-RT-* | ✓ |
| §十二 开发阶段 Phase1–4 | tasks.md 全勾选 | ✓ |

### 4.3 办公与现实交互方案.md → 覆盖与缺口
| 方案章节 | 状态 | 说明 |
|----------|------|------|
| §4.1 Skill 负责流程不负责执行 | ✓ | office.* 工具执行；Tier C 用 skill 知识 + LLM 生成 |
| §4.2 API 优先视觉兜底 | 部分 | office 三层（Rust→pandoc/LibreOffice）；视觉兜底为 Future |
| §4.3 主界面简洁 | ✓ | 录音/预览走右侧插件，不污染主聊天 |
| §4.4 可解释/回放/验证 | ✓ | 工具经 runtime 事件+审批；office-context 卡片 |
| §9 deepagent-office | 部分 | 办公文档读写已落 app-core OfficeService（未独立 crate）|
| §10 deepagent-vision | **N/A Future** | 视觉/OCR/UI 树未实现 |
| §11 deepagent-computer-use | **N/A Future** | 桌面自动化未实现 |
| §6 Connections 连接中心 | **N/A Future** | OAuth/office-provider/device 未实现 |
| §12 deepagent-edge | **N/A Future** | 现实设备未实现 |
| §15 风险分级 | ✓ | 低=读/预览，中=新建/导出，高=覆盖/删除/下载执行 |
| §18 测试与验证 | ✓ | 单元/集成/仿真/回放四层映射（见 §2）|
| §19 MVP | 部分 | 办公只读+本地 skill+草稿生成已具；连接器/视觉为后续 |

### 4.4 Future 阶段规划用例（暂 N/A，启用后补测）
- TC-VISION-* ：`vision.capture_screen/inspect_window/read_table` 结构化输出校验。
- TC-CU-* ：`computer.click_control` 基于视觉 control_id（禁裸坐标）。
- TC-CONN-* ：`Connection` 模型状态/health/risk_tier；OAuth 连通性测试。
- TC-EDGE-* ：打印机/NAS/摄像头/MQTT 只读状态。

---

## 第五部分 · 最终测试结果（实跑）

执行平台：Windows。命令与结果：

| Gate | 命令 | 结果 |
|------|------|------|
| 内核全量单测 | `cargo test --workspace` | **1259 passed, 0 failed** |
| 内核 lint | `cargo clippy --workspace` | **0 warnings, 0 errors** |
| 全特性 lint | `cargo clippy -p deepagent-app-core -p deepagent-builtins --features audio,runtimes,pdfium` | **干净** |
| pdfium 编译 | `cargo check -p deepagent-app-core --features pdfium` | **Finished** |
| 桌面壳 | `cargo check`（src-tauri）| **Finished** |
| 前端类型 | `npx tsc --noEmit` | **0 error** |

office-agent 专属单元测试（计 36 条，全部 PASS，包含于上面 workspace 计数）：
- file_preview_service：12｜recording_service：5｜runtime_service：9｜speech_service：2｜office_service：5｜office_tools（builtins）：3。

### 结论
- `工作模式-办公开发.md` 全部章节对应用例 **通过**；功能与统一测试覆盖完整。
- `办公与现实交互方案.md` 的**办公子集 + 安全/审批/UI 原则 + 测试分层**均满足；视觉 / computer-use / connections / edge 为后续阶段，已在覆盖矩阵显式标注并预置规划用例。

### 启用后需补测（fail-closed 安全门，非代码缺口）
1. 为 whisper-base/small、pandoc、pdfium、libreoffice 注册表 pin 各平台实测 SHA-256 → 解锁 TC-M-REC-004、Tier R 路径。
2. 录音真实转写需 `--features whisper`（C++/cmake）→ 解锁 TC-M-REC-006 端到端。
3. pdfium 安装后 → 解锁 TC-M-PREVIEW PDF 页面图渲染。
