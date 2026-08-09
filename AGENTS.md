# DeepAgent-Studio 开发铁律：对齐成熟架构与官方文档，不缝补丁、不摸瞎

本项目是 **monorepo**，核心产品为 `apps/desktop` 下的 **Tauri v2 桌面应用**；后端为 Rust，前端为 React/TypeScript，目标是为 DeepSeek 提供 Claude Code 级执行内核。**任何架构、选型、技术点、代码生成，都必须有权威依据（成熟参考项目 / 官方文档）；严禁凭直觉自创。自己想的永远不如官方技术文档权威。**

**重点是系统底层（后端 Rust 内核）；前端并非不管——凡后端改动涉及前后端交互（Tauri command 签名、事件、数据契约），必须同步更新前端并保持一致，避免前后端脱节。**

## 信源优先级（按此顺序查证，命中即停）

**A. 行为机制 / 架构**（工具循环、权限、Hook、Skill/CLAUDE.md 注入、压缩、子代理、取消、状态机、系统提示词）——本地 `G:\Code_Warehouse\DeepAgent-Studio\借鉴`：
1. **Claude Code**（`借鉴\claudecode`，restored-src / package）—— 行为机制第一信源。
2. **Codex**（`借鉴\codex\codex-rs`，Rust）—— 与本项目同语言，架构/状态机/取消/沙箱执行直接对照；**代码审查触发链（`/review` 命令、审查专用流程）以它为准**。
3. **Grok**（`借鉴\grok-build`，Rust）—— 模型交互、流式、工具协议。
4. **其它本地参考**：DeepSeek-Reasonix（Go 桌面 Agent，会话/恢复/心跳/沙箱）、Kun。
5. **能力增强参考**（为本系统增强而引入，接入时同样先读源码再动手）：
    - **open-code-review**（`借鉴\open-code-review`，Go）—— 代码审查能力：审查规则引擎、diff 审查、`delegate` 宿主代理委托模式、plugins/skills 接入形态。
    - **better-harness**（`借鉴\better-harness`）—— 让 AI 写好代码的 harness 工程：hooks、skills、agent 资产组织、case-studies 经验库。

**B. Rust 技术点 / 库选型 / 语言用法**：
- 优先看参考项目（codex-rs、grok-build）里同类技术用哪个 crate、怎么组织；
- 再查 **Rust 官方文档 / std / docs.rs / 该 crate 官方文档**；
- **禁止**凭记忆臆造 API、crate 名、feature、版本行为——以官方文档为准。

**C. 参考项目都没有的技术点**（如 Windows 沙箱用的是**微软官方 Sandboxie / Windows Sandbox**、Job Object、Win32 API 等）：
- **必须查对应官方文档**（Microsoft Learn / Sandboxie 官方文档 / Win32 API 手册）；
- 官方怎么规定就怎么做，不得用"应该是这样"的猜测实现系统底层能力。

**D. 模型侧**：以 **DeepSeek 官方手册**（联网）为准——模型能力、API 参数、Thinking/reasoning_effort、function calling、上下文窗口、限流等，不得沿用 Claude/OpenAI 的假设。

**E. 联网检索**：仅当上述本地信源与官方文档都无对应时才用，优先官方/一手来源。

> 查证结论必须写进方案：`{信源项目/官方文档 + 具体位置} 的做法 → 本项目的对齐方式`。冲突裁决：行为逻辑以 Claude Code 为准、Rust 工程实现以 Codex 为准、模型侧以 DeepSeek 官方为准、OS/沙箱以微软官方为准。

## 能力增强接入要求（open-code-review / better-harness）

- **代码审查触发方式**：先对照 **codex 的 `/review` 实现**（触发入口、前端命令、审查会话形态）再设计本系统方案。基线：**手动触发**走前端 `@`/斜杠命令（对齐 codex）；**AI 写代码任务的自动触发**（写任务完成后自动审查一轮）作为可选增强，需先在 codex/claudecode 中查证是否有对应机制，无对应则按"自创从宽"处理——审查结果只作反馈，不得变成误杀 run 的门卫。
- **审查执行通道**：优先用 open-code-review 的 `delegate` 模式（输出结构化审查规格，由 DeepSeek 模型执行审查），CLI 已作为 devDependency 存在（`pnpm review` / `npx ocr`）。
- **better-harness 的用法**：其 hooks/skills/资产组织作为"让 AI 写好代码"的参考蓝本，接入本系统时必须遵守"附加物是参考不是指令"基线，不得压制内置工具。

## 开发改动边界（动手前必须判断）

先判断用户要的是“修复原行为”还是“新增能力”。性能、报错、轻量化、查询慢、体验卡顿等问题，默认按 **保持原功能、原接口和原调用方式不变** 处理；只有调用链、真实失败样本或参考架构证明当前子系统存在系统性缺失时，才扩大到架构调整。

- **禁止未经要求扩功能**：用户未要求新入口、新页面、新交互、新前端状态时，不得主动新增。不能把性能优化做成一套新的协议、DTO、command、缓存状态或 UI。
- **优先优化现有实现**：能在当前 Tauri command、service、repository、API wrapper 或组件内部减少读取、扫描和重复查询，就不要新增接口。
- **新增接口必须收口**：确需新增接口时，同步处理旧接口去留；旧接口要么有明确兼容理由并委托新实现，要么迁移调用点后删除。禁止新旧两套悬空并存。
- **禁止无授权改前端**：用户只要求后端、Rust、数据查询、规则文档或方案时，不得修改 React 组件、交互、样式和 i18n。前端必须配合契约变化时，先说明原因和改动边界。
- **代码量与收益成比例**：优先删除不必要工作，避免用大量缓存结构、分页状态或 DTO 扩散掩盖根因。不要顺手重构无关模块。
- **性能问题先定位**：先用 `rg`、日志、SQL、调用链和已有测试确认耗时来源；禁止凭感觉同时修改 sessions、skills、前端和接口层。
- **保持行为兼容**：若改成懒加载、截断或分页，用户触发原功能时仍必须能取得完整数据，不能静默改变接口语义。
- **清理悬空实现**：调试代码、临时 DTO、未使用 API、未接入 command、废弃函数和无用类型必须随本次改动清理，并用 `rg`、编译和类型检查确认。
- **最终说明边界**：明确改了哪些层、哪些层未动、为什么；刻意未改前端或未新增接口时也要说明。

> “不要局部补丁”与“小步修改”并不冲突：一次性偶发问题优先在现有正确架构内做最小完整修复；同类问题反复出现或证据表明架构错位时，再对照上游重构对应子系统。

## 强制工作流

1. **先查证再动手**：任何行为机制、库选型、系统底层技术点，先按 A–E 找到权威依据并引用具体文件/文档位置，禁止无依据就写代码。
2. **要完整架构，不要补丁**：修复前先判断"这是补丁还是架构缺失"。同类问题反复出现（如 CompletionGate 连环误杀）即为架构错位——**必须对照信源重构该子系统，而非在旧结构上再加一层判断**。
3. **选型有据**：引入/更换任何 crate、系统 API、脚手架、算法时，必须说明依据来源（参考项目在用 / 官方文档推荐），不得只凭"常见""应该"。
4. **前后端同步**：改动 Tauri command、事件名、payload/DTO 结构时，必须同时更新前端调用与类型（`api.ts`/`types.ts` 等），并核对一致，禁止只改一侧造成脱节。
5. **无对应才自创**：所有信源与官方文档都无对应时才允许自设计，代码中标注 `// No upstream counterpart (checked: claudecode/codex/grok/official docs):` + 理由。
6. **自创机制默认从宽**：自创的"门卫/校验/强制"逻辑（如 CompletionGate）宁可漏过、不可误杀——误杀正确执行的 run 比没有门卫更糟。
7. **真实失败样本回归**：修行为缺陷时，回归测试必须用日志/会话抓到的真实失败 prompt 与数据，不允许只造理想化用例。

## Tauri 前端运行与验证

前端代码会被编译进 Tauri 应用。**只改代码、不运行，旧窗口和旧安装包不会出现改动。**任何前端修改都必须实际运行供用户验证，不能只通过类型检查就宣称完成。

日常开发：

```bash
cd apps/desktop && npm run tauri dev
```

非交互环境中，`pnpm dev` 可能因无 TTY 触发 `ERR_PNPM_ABORTED_REMOVE_MODULES_DIR_NO_TTY`，应绕过 `beforeDevCommand`：

```bash
# 终端 1：端口必须与 tauri.conf.json 的 devUrl 一致
cd apps/desktop && npx vite --port 1420 --strictPort

# 终端 2：运行 Rust 外壳
cd apps/desktop/src-tauri && cargo run --no-default-features
```

- 前端改动至少执行 `npx tsc --noEmit`，并完成实际运行或浏览器验证及关键交互断言。
- 浏览器验证可用 Playwright 打开 Vite 地址，并通过 `window.__TAURI_INTERNALS__.invoke` 注入 mock；mock 返回值必须符合真实 DTO 形状，例如 balance 结果必须包含 `infos` 数组。
- 本地打包：`cd apps/desktop && npm run tauri build`。正式发布按 `apps/desktop/RELEASE.md` 打 tag 走 GitHub Actions；前端改动必须重新打包才会进入安装包。

## 包管理器与 Git 已知坑

- `apps/desktop` 同时存在 `package-lock.json` 和 `pnpm-lock.yaml`；项目内 `node_modules/pnpm` 为 pnpm v11，全局 PATH 可能为 v10。
- **非交互环境禁止运行 `pnpm install`**：版本校验失败可能清空 `node_modules` 顶层。依赖布局损坏时使用 `cd apps/desktop && npm install` 恢复。
- 根目录 `.gitignore` 的 `ui/` 规则会误伤 `apps/desktop/src/components/ui/`。提交该目录时必须使用 `git add -f apps/desktop/src/components/ui/`，随后用 `git status` 确认文件没有遗漏。

## 前端样式与组件规范

现行界面语言是 **无边框、无浮起阴影、静默着色**。颜色必须使用 `tailwind.config.js` 中的主题 token，禁止硬编码 hex、`bg-white`、`hover:bg-gray-50/100` 或 `bg-gray-100` 激活态。

- 浮层容器：使用 `Panel`，`bg-elevated-bg` + `shadow-[0_6px_24px_rgba(0,0,0,0.10)]`，不加 border。
- 触发按钮/胶囊：无 border，使用 `bg-black/5`；列表 hover/激活同样使用 `bg-black/5`。
- Composer 输入区：使用 `InputSurface`，无 border；默认 `shadow-[0_2px_12px_rgba(0,0,0,0.07)]`，聚焦使用 `focus-within:shadow-[0_4px_18px_rgba(0,0,0,0.12)]`。
- 动效统一使用 `components/ui/motion.ts`：150ms hover、300ms ease-out 滑块/面板/滑动药丸、400ms 大面板，并尊重 `prefers-reduced-motion`。
- 新 UI 必须复用 `components/ui/`：`Panel`、`ListItem`、`SlidingMenuList`、`MorphingMenuShell`、`MorphingToolbarMenu`、`MorphingSelect`、`ToolbarMenuTrigger`、`TintButton`、`IconButton`、`InputSurface`、`Slider`、`DropdownMenu`、`SlidingTabs`。使用 `cn()` 合并类，禁止重复手写已有组件的类名组合。
- 侧栏滑动药丸使用 `useSlidingIndicator` + `SlidingPill`；按钮必须 `relative z-[1]`。浮层列表使用 `SlidingMenuList`，二者不能混用。
- 一级菜单可用 `MorphingMenuShell`/`MorphingToolbarMenu` 的 `layoutId` 形变；每个实例的 `layoutId` 必须唯一。二级 flyout 使用独立 `Panel`，不得给菜单行增加 hover `layoutId` morph。
- 含 `SlidingMenuList` 的 morph 菜单必须 `staggerContent={false}`。行需带 `data-menu-item={id}` 与 `relative z-[1]`，`activeId` 必须匹配真实 id。
- Footer 项目/环境/Git 菜单互斥；Composer 模型/权限/上下文菜单互斥；两区之间通过既有 overlay 信号保持互斥。
- Composer 附件按钮沿用 `ComposerAttachButton` 的“悬停时按钮自身 morph 展开”模式，不改成点击展开菜单或独立 tooltip 卡片。
- 导航和设置文案必须走 i18n，不得在组件中硬编码中文。
- 新交互样式/动效在落地前先制作 `choice-preview-YYYY-MM-DD-HHMM.html`，至少提供 6 个方案；用户确认并完成验证后删除临时选择页，用户明确保留的探索文件不得擅自删除。

## 日志纪律（排查问题的生命线）

- **习惯性打日志**：新增/修改任何核心路径（状态机转换、工具执行、Hook 派发、权限判定、配置加载、压缩、恢复、取消、降级/熔断）时，**必须同步补结构化日志**——判定标准：该路径出问题时，仅凭日志能还原"发生了什么、为什么走到这一步"。
- **格式统一，走既有双路日志，不得另起炉灶**：
    - 诊断日志 → `runtime-logs.db`：统一经 `append_runtime_log` + `NewRuntimeLogEntry`，字段齐全（`level` / `category` / `event` / `message` / `data_json`，并尽量带 `run_id` / `session_id` / `source`）；
    - 产品事件 → `run_events`：可回放的状态机事实，经内核事件通道落库。
- **命名与结构**：`event` 用 snake_case 动词短语（如 `registry_ready`、`input_queued`）；上下文一律放 `data` 结构化 JSON 字段，**禁止把变量拼进 message 字符串**了事。
- **失败路径必留痕**：任何 `Err` 返回、静默降级、fallback、熔断触发点，都必须有一条可检索的日志（含原因与关键参数），禁止吞错。
- **脱敏红线**：日志不得记录 API key/密钥/prompt 原文（runtime-logs 只记长度；密钥经 redaction 清洗），新日志点必须遵守。

## 已验证的对齐基线（违反即回归）

- **错误可见性**：任何执行器（含沙箱）必须把子进程完整 stdout/stderr 回传给模型。自纠错完全依赖看得见报错——空输出 + exit code 不可接受。
- **附加物永远是参考，不是指令**：Skill/规则正文注入用参考性措辞，附"环境中反复失败即放弃该路线"逃生条款；附加物（skill/MCP/rules）不得门禁或压制内置工具。
- **失败必须升级**：同一工具滑动窗口内失败 ≥4 次 → 反馈升级为强制换根本路线（APPROACH CHANGE REQUIRED），优先内置能力与零依赖路线。
- **外部配置须校验**：兼容读取 `~/.claude/*` 等外部配置时，值必须验证对本 provider 有效（如 model 名必须存在于目录），无效则忽略而非透传。
- **完成校验以事实为准、从用户短指令推导**：CompletionPolicy 只能从用户原始短指令提取要求，绝不扫描附件/粘贴正文/文件内容；提取结果必须有数量上限与合法性过滤（中文全角标点、CJK 粘连已多次误伤）。
- **模型侧一律以 DeepSeek 官方为准**：Thinking/reasoning_effort、max_tokens、function calling 格式、上下文窗口、限流重试策略等，不得照搬 Claude/OpenAI 的取值。

## 已知踩坑速查（详见 memory）

- Sandboxie `Start.exe` 不中继沙箱内输出 → 已用工作区重定向回读修复，勿回退。
- 中文 prompt 全角标点（：，；）粘连进"必须路径" → 路径提取按路径合法字符切段，勿改回空格分词。
- skill 命令式全文重注入曾锁死模型策略 → 保持参考性措辞。
- 测试须隔离用户主目录：`DualConfigLoader` 会读真实 `~/.claude/settings.json`，测试用 `with_user_home(None)`/临时目录。
- 写工作区外文件（如本规则文件）受沙箱写限制 → 需提权执行。
