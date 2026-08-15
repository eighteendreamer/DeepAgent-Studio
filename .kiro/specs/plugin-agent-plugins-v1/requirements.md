## Introduction

DeepAgent Studio 现有插件系统是照 Codex 早期 `.codex-plugin/plugin.json` 格式实现的私有超集，同时能读 `.claude-plugin/plugin.json`。这套模型能用，但它是"我们自己定义的兼容层"，随三方演进要持续追赶。

调研确认了一个更好的锚点：**Agent Plugins 规范 1.0.0** 是由 Amazon / Cursor / Microsoft / OpenAI / Vercel 组成的 TSC 发布的开放、厂商中立标准（`https://agent-plugins.org/schemas/1.0.0/plugin.schema.json`），Claude Code、Codex、Cursor 均已支持。`awslabs/agent-plugins`（Apache-2.0）是同一份插件同时携带 Claude 与 Codex 两套 manifest 的权威范例。

本特性把内部统一模型换成 Agent Plugins v1，把 Codex / Claude / Cursor 三家降级为该模型的输入**方言**，并把 DeepAgent 专有能力收进规范第 8 节留出的 `com.deepagent.studio` 扩展命名空间。目标是：**装得上三家的插件，且我们自己的插件也能被三家装走**。

本特性范围是方案里的 M0–M2（合规底座 + v1 加载器 + 方言适配）。M3（扩展组件投影）、M4（市场）、M5（内置插件）、M6/M7（DeepSeek Harness 的 Cordis 侧车）不在本 spec 内，但本 spec 的数据模型必须为它们留出接口。

### 契约盘点结论（已查证，决定本 spec 的实际工作量）

盘点现有代码后有三项发现改变了原估算：

1. **hooks 已经完整支持 Claude Code 格式。** `crates/deepagent-hooks/src/external_hooks.rs` 的 `HookEvent` 枚举直接采用 Claude Code 命名（23 个事件，涵盖 Claude 全部 `PreToolUse` / `PostToolUse` / `UserPromptSubmit` / `Stop` / `SessionStart`），`HookEvent::parse` 从 `hooks.json` 的键解析，`point()` 映射到内部 `HookPoint`，`matcher_matches` 还实现了工具名别名（`Write` → `write_file`、`Edit|Bash` → `shell`）与通配。已有 `parses_claude_code_schema` 测试。**hooks 无需新建映射层。**
2. **占位符展开已是单次非递归**（`deepagent-mcp/src/config.rs` 的 `expand`），这点已符合规范 §9.2。
3. **但有三处明确的规范违背**：未知占位符被展开成空串（`unknown_var_expands_empty` 测试固定了此行为）；`command` 会被展开；`url` 与 `headers` 会被展开。规范 §7.2.1 与 §9.2 明确禁止这三者。同时缺少规范 §9.1 要求的无前缀 `PLUGIN_ROOT` / `PLUGIN_DATA`（现有只有 `DEEPAGENT_PLUGIN_ROOT` 与 `CLAUDE_PLUGIN_ROOT` 变体）。

关键约束：`expand` 是全局 MCP 配置共用的，用户手工配置的 MCP server 可能依赖 `${API_TOKEN}` 之类的环境变量展开。**因此不能直接修改通用 `expand` 的行为**，插件来源的 MCP 配置必须走独立的 v1 合规展开路径。

## Glossary

- **Agent Plugins v1 / 规范**：Agent Plugins Specification 1.0.0。本文档中"§N"均指该规范章节。
- **可移植核心 (Portable Core)**：规范定义的插件根 `plugin.json` 与两种标准组件（`skills/`、`mcp.json`）。跨客户端可移植的部分。
- **方言 (Dialect)**：客户端特有的 manifest 形态与组件位置约定。本 spec 涉及四种：`AgentPluginV1`、`Codex`、`Claude`、`Cursor`，外加历史遗留的 `DeepAgentLegacy`。
- **扩展命名空间 (Extension Namespace)**：规范 §8 定义的反向域名标识符，承载客户端专有数据。DeepAgent 使用 `com.deepagent.studio`。
- **扩展组件 (Extended Component)**：不在 v1 内的组件类型：commands、agents、hooks、output-styles、apps。
- **Overlay**：当插件采用 v1 根 manifest 时，`.codex-plugin/plugin.json` 作为补充层提供 `interface` 等展示元数据，不覆盖核心字段。
- **Presentation**：插件在 UI 上的展示元数据（显示名、分类、图标、品牌色、长描述）。
- **诊断 (Diagnostic)**：加载过程中被跳过或降级的组件的结构化记录，必须对用户可见。
- **PLUGIN_ROOT / PLUGIN_DATA**：规范 §9.1 要求客户端为插件子进程提供的两个环境变量。前者是插件根绝对路径，后者是客户端管理的、跨插件更新保留的可写目录。

## Requirements

### Requirement 1: Agent Plugins v1 作为统一内部模型

**User Story:** 作为 DeepAgent 的维护者，我希望内部只有一套插件模型且它就是开放标准，这样三方插件的兼容不再依赖我们自己发明的映射规则。

#### Acceptance Criteria

1. WHEN 系统解析任意来源的插件 THEN 系统 SHALL 产出唯一的归一化结构，其可移植核心字段与规范 §5.2 的 closed schema 一一对应（`$schema`、`name`、`version`、`description`、`author`、`homepage`、`repository`、`license`、`keywords`、`extensions`）。
2. WHERE 插件根存在 `plugin.json` THE 系统 SHALL 依据其 `$schema` 值判定三种状态：值等于规范 canonical 标识符时为 Supported；值以 `https://agent-plugins.org/schemas/` 为前缀但版本不受支持时为 Unsupported；其余为 Unrelated。
3. WHEN `$schema` 判定为 Unsupported THEN 系统 SHALL 拒绝该插件并报告不受支持的版本。
4. WHERE 加载插件 THE 系统 SHALL NOT 通过网络获取 schema 文档（规范 §5.2 明确禁止）。
5. WHEN `author` 对象包含除 `name`、`email`、`url` 之外的字段，或任一字段值非字符串 THEN 系统 SHALL 判定 manifest 非法并拒绝该插件。
6. WHERE 元数据字段 THE 系统 SHALL NOT 仅因 `version` 不是合法 SemVer、`homepage`/`repository`/`author.url` 不是可识别 URL、`author.email` 不是可识别邮箱、或 `license` 不是 SPDX 标识符而拒绝 manifest（规范 §5.4）。
7. WHEN DeepAgent 专有能力（apps、permissions、runtime 偏好）需要表达 THEN 系统 SHALL 将其置于 `extensions` 的 `com.deepagent.studio` 键下或同名顶层目录中，而非新增顶层 manifest 字段。
8. WHERE 归一化结构 THE 系统 SHALL 记录该插件的来源方言，以便 UI 展示与故障诊断。

### Requirement 2: 插件名称约束

**User Story:** 作为插件安装者，我希望非法插件名在加载时就被拒绝，而不是在后续拼接路径或 ID 时引发意外行为。

#### Acceptance Criteria

1. WHERE 插件 manifest 的 `name` THE 系统 SHALL 校验：长度 1–64 字符；字符集仅限小写字母、数字、连字符、句点；首尾字符为字母或数字；不含连续连字符或连续句点（规范 §5.5）。
2. WHEN `name` 违反上述任一约束 THEN 系统 SHALL 拒绝该插件并报告是哪一项约束不满足。
3. WHEN `name` 缺失、类型错误或为空 THEN 系统 SHALL 拒绝该插件（规范 §5.3）。
4. WHERE 名称校验 THE 系统 SHALL 接受规范给出的合法样例（`my-plugin`、`acme.tools`、`lint3r`、`a`）并拒绝规范给出的非法样例（`My-Plugin`、`-start`、`has--double`、`too.many..dots`、空字符串）。

### Requirement 3: manifest 发现顺序与方言归一

**User Story:** 作为使用三方插件的用户，我希望不管插件是给 Codex、Claude Code 还是 Cursor 打包的，DeepAgent 都能识别它。

#### Acceptance Criteria

1. WHERE 插件根目录 THE 系统 SHALL 按以下顺序查找 manifest：根 `plugin.json`（仅当其 `$schema` 判定非 Unrelated）、`.codex-plugin/plugin.json`、`.claude-plugin/plugin.json`、`.cursor-plugin/plugin.json`。
2. WHEN 根 `plugin.json` 是符号链接，或不是常规文件 THEN 系统 SHALL 不将其作为 manifest 使用。
3. WHEN 采用根 `plugin.json` 作为 manifest 且 `.codex-plugin/plugin.json` 同时存在 THEN 系统 SHALL 将后者作为 overlay 叠加，仅用于补充展示元数据，且 SHALL NOT 用它覆盖可移植核心字段。
4. WHEN 插件仅有 `.claude-plugin/plugin.json` 且该 manifest 未声明任何组件路径 THEN 系统 SHALL 回退到 Claude 约定目录扫描（`skills/`、`commands/`、`agents/`、`hooks/hooks.json`、`.mcp.json`）。
5. WHERE Presentation 元数据 THE 系统 SHALL 按以下优先级取值：Codex `interface` 字段、Claude 市场清单条目、可移植核心 `description`、插件目录名。
6. WHEN 插件同时提供多种方言 manifest THEN 系统 SHALL 按第 1 条的顺序选定唯一一个作为权威来源，并在归一化结构中记录被选中的方言。
7. WHERE 现有 `.codex-plugin` 私有超集字段（`commands`、`agents`、`output-styles`、`runtime`、`interface.permissions`） THE 系统 SHALL 继续识别，以保证既有内置插件与已安装插件不失效。

### Requirement 4: 标准组件发现

**User Story:** 作为插件作者，我希望我的 skills 和 MCP 服务器按规范固定位置被发现，行为与其他客户端一致。

#### Acceptance Criteria

1. WHERE `skills/` 目录 THE 系统 SHALL 仅把每个**直接子目录**中存在且解析为常规文件的 `SKILL.md` 视为一个 skill，且 SHALL NOT 递归搜索更深层级（规范 §7.1）。
2. WHEN 固定组件位置不存在 THEN 系统 SHALL NOT 将其视为错误（规范 §6.2）。
3. WHEN 固定组件位置存在但文件类型不符（如 `skills` 不是目录、`mcp.json` 不是常规文件） THEN 系统 SHALL 将该组件类型标记为无效，并继续加载其他组件类型。
4. WHERE 插件根 `mcp.json` THE 系统 SHALL 校验其为对象且仅含 `$schema` 与 `mcpServers` 两个顶层字段，`mcpServers` 为对象（可为空对象）。
5. WHEN `mcp.json` 的 `$schema` 版本与 `plugin.json` 声明的版本不一致 THEN 系统 SHALL 禁用该插件的 MCP 组件并报告版本不匹配，且 SHALL NOT 影响其他组件类型。
6. WHERE 每个 MCP server 条目 THE 系统 SHALL 按 `type` 匹配闭合联合的唯一变体：`stdio`（`command` 必需，`args`/`env`/`cwd` 可选）、`streamable-http`（`url` 必需，`headers` 可选）、`sse`（`url` 必需，`headers` 可选）；出现未知字段、未知 `type` 值或属于其他变体的字段时，SHALL 判定该 server 条目无效。
7. WHEN 单个 MCP server 条目无效或其 transport 不被支持 THEN 系统 SHALL 跳过该条目并继续加载同文件中其他 server 与其他组件类型。
8. WHERE stdio 的 `command` THE 系统 SHALL 将其视为单个可执行 token（裸名或以 `./` 开头的插件相对路径），且 SHALL NOT 将其解析为 shell 命令串。
9. WHERE 远程 transport 的 `url` THE 系统 SHALL 要求绝对 HTTP/HTTPS URL、不含用户信息与片段；非回环主机 SHALL 要求 HTTPS。

### Requirement 5: 占位符展开与子进程环境

**User Story:** 作为插件作者，我希望 `${PLUGIN_ROOT}` 与 `${PLUGIN_DATA}` 的行为和规范一致，不会因为客户端差异导致我的插件在 DeepAgent 上出错。

#### Acceptance Criteria

1. WHEN 系统启动插件提供的 stdio MCP server 子进程 THEN 系统 SHALL 在其环境中提供 `PLUGIN_ROOT`（插件根绝对路径）与 `PLUGIN_DATA`（该已安装插件专属的、客户端管理的可写目录绝对路径）。
2. WHERE `PLUGIN_DATA` 目录 THE 系统 SHALL 在启动子进程前创建它、保证对该子进程可写、并在插件更新后保留其内容。
3. WHERE 插件来源的配置值 THE 系统 SHALL 仅展开 `${PLUGIN_ROOT}` 与 `${PLUGIN_DATA}`，且展开为单次非递归的文本替换；替换产生的文本 SHALL NOT 被再次扫描占位符。
4. WHERE 占位符展开 THE 系统 SHALL 只作用于 stdio server 的 `args` 每个元素、`env` 的每个值、以及 `cwd`；SHALL NOT 作用于 `env` 的键、`command`、远程 transport 的 `url` 与 `headers`、以及固定组件位置。
5. WHEN 插件来源配置中出现无法识别的占位符样式文本 THEN 系统 SHALL 保持其字面量不变（规范 §9.2），而非展开为空字符串。
6. WHEN 某个 stdio server 的 `env` 对象包含名为 `PLUGIN_ROOT` 或 `PLUGIN_DATA` 的条目 THEN 系统 SHALL 判定该 server 配置无效。
7. WHERE 子进程环境组装顺序 THE 系统 SHALL 先应用配置的 `env` 覆盖基础环境，再设置 `PLUGIN_ROOT` 与 `PLUGIN_DATA`（后者优先，规范 §9.1）。
8. WHEN `cwd` 省略 THEN 系统 SHALL 以插件根作为子进程工作目录。
9. WHERE 显式 `cwd` THE 系统 SHALL 仅接受三种形式：以 `./` 开头的插件相对路径、`${PLUGIN_ROOT}` 或以 `${PLUGIN_ROOT}/` 开头、`${PLUGIN_DATA}` 或以 `${PLUGIN_DATA}/` 开头；展开并解析后越出对应根目录的 SHALL 判定该 server 条目无效。
10. WHERE 非插件来源的 MCP 配置（用户手工配置、项目配置） THE 系统 SHALL 保持现有环境变量展开行为不变，以免破坏既有用户配置。
11. WHERE 方言别名 THE 系统 SHALL 在归一化阶段把 `${CLAUDE_PLUGIN_ROOT}` 与 `${CODEX_PLUGIN_ROOT}` 重写为 `${PLUGIN_ROOT}`（对应的 DATA 变体同理），使其后续走统一的 v1 展开语义。
12. WHERE 兼容性 THE 系统 SHALL 在子进程环境中同时保留现有的 `DEEPAGENT_PLUGIN_*` 与 `CLAUDE_PLUGIN_*` 变量，以免既有内置插件失效。

### Requirement 6: 路径containment 与失败边界分层

**User Story:** 作为用户，我希望一个插件里某个组件坏掉时其余部分照常工作，并且我能在界面上看到到底哪里被跳过了。

#### Acceptance Criteria

1. WHERE 插件包提供的任何文件或目录 THE 系统 SHALL 保证其文件系统解析后的路径仍在插件根之内，并拒绝解析到根之外的包路径。
2. WHERE manifest 中定义为插件相对路径的字段 THE 系统 SHALL 要求其以 `./` 开头、相对插件根解析、且解析后仍在根内。
3. WHEN `plugin.json` 无法在插件根内解析 THEN 系统 SHALL 拒绝该插件。
4. WHEN 固定组件位置无法在插件根内解析 THEN 系统 SHALL 将该组件类型标记为无效。
5. WHEN 某个发现到的 `SKILL.md` 无法在插件根内解析 THEN 系统 SHALL 跳过该 skill。
6. WHEN MCP server 的 `command` 或 `cwd` 未通过 containment 校验 THEN 系统 SHALL 判定该 server 条目无效。
7. WHEN manifest 出现未知顶层字段 THEN 系统 SHALL 报告并忽略该字段、且 SHALL 继续加载该插件（非致命，规范 §5.2）。
8. WHEN `extensions` 字段不是对象 THEN 系统 SHALL 报告并忽略该字段、且 SHALL 继续加载组件（非致命，规范 §8.1）。
9. WHEN manifest 出现除上述两类之外的任何 schema 违规 THEN 系统 SHALL 拒绝该插件且 SHALL NOT 发现或执行其任何组件。
10. WHERE 加载结果 THE 系统 SHALL 以结构化诊断记录每一次跳过或降级，至少区分：未知 manifest 字段、extensions 非对象、skill 被跳过、MCP 整体禁用、单个 MCP server 被跳过、扩展组件被跳过。
11. WHERE 诊断信息 THE 系统 SHALL 通过应用服务层 DTO 暴露到前端，并在插件详情界面可见。
12. WHERE 未实现的 `extensions` 命名空间 THE 系统 SHALL 忽略其条目且 SHALL NOT 校验其值的内容（规范 §8.1）。

### Requirement 7: 扩展组件的接纳与可见降级

**User Story:** 作为安装 Claude Code 插件的用户，我希望它的 commands、agents、hooks 能在 DeepAgent 里真的生效；如果某类确实接不上，我要知道，而不是被静默丢掉。

#### Acceptance Criteria

1. WHERE 扩展组件类型（commands、agents、hooks、output-styles、apps） THE 系统 SHALL 在归一化结构中为每一类保留独立集合，供后续里程碑投影到运行时。
2. WHEN 插件提供 `hooks/hooks.json` 或 manifest 内联 hooks THEN 系统 SHALL 复用现有 `HookDefinitions` 解析与 `HookEvent` 映射，且 SHALL NOT 新建平行的 hooks 事件映射层。
3. WHEN hooks 中出现无法映射到内部生命周期点的事件名 THEN 系统 SHALL 以诊断记录该事件被跳过，且 SHALL NOT 静默丢弃。
4. WHERE 扩展组件的路径 THE 系统 SHALL 应用与标准组件相同的 containment 规则。
5. WHEN 某类扩展组件解析失败 THEN 系统 SHALL 跳过该类并继续加载其余组件类型。

### Requirement 8: 既有插件零回归

**User Story:** 作为现有用户，我希望这次改造不让我已经在用的插件失效。

#### Acceptance Criteria

1. WHEN 新加载器加载现有 9 个内置插件（browser、computer-use、files、meeting-recorder、office-agent、project-map、side-chat、terminal、wedecode） THEN 系统 SHALL 对每个插件产出与改造前逐一相等的 skill、command、app、hook、MCP server、output-style 计数。
2. WHERE 已安装的个人插件与市场插件 THE 系统 SHALL 在改造后继续被发现、启用状态继续生效。
3. WHERE `state.json` 的启用状态与已安装记录 THE 系统 SHALL 保持向后兼容，既有文件 SHALL NOT 需要用户手工迁移。
4. WHERE 插件来源优先级与覆盖关系（内置 / 工作区 / 个人 / 市场 / 会话） THE 系统 SHALL 保持现有语义不变。
5. WHERE 应用服务层与 Tauri 命令签名 THE 系统 SHALL 保持现有命令名与参数不变，仅允许在 DTO 上新增字段。
6. WHEN 跨层契约新增字段 THEN 系统 SHALL 同步更新前端类型定义、API 包装与界面消费点。

### Requirement 9: 许可合规

**User Story:** 作为项目负责人，我需要确保随包分发的第三方插件不带来许可风险。

#### Acceptance Criteria

1. WHERE 随包内置分发的第三方插件 THE 系统 SHALL 仅包含明确授予再分发权利的开源许可（如 Apache-2.0）的插件。
2. WHERE `anthropics/claude-code` 仓库中的官方插件 THE 系统 SHALL NOT 将其复制进本仓库分发（其许可为保留所有权利并受商业条款约束）。
3. WHEN 内置第三方插件 THEN 系统 SHALL 在插件目录内保留其原始 LICENSE 文件，并在 `THIRD_PARTY_NOTICES.md` 记录来源、版本与许可。
4. WHERE 测试固件 THE 系统 SHALL 仅把可再分发许可的插件复制进仓库；引用不可再分发插件的测试 SHALL 在其本地不存在时跳过而非失败。

### Requirement 10: 模块可维护性

**User Story:** 作为维护者，我希望插件子系统的代码结构能支撑后续三个里程碑，而不是继续在几个数千行的文件里加分支。

#### Acceptance Criteria

1. WHERE 插件子系统源码 THE 系统 SHALL 按职责组织为模块树：规范解析、方言适配、组件发现、加载、市场、依赖、安全扫描、运行时投影、状态持久化、服务门面。
2. WHEN 进行模块重构 THEN 系统 SHALL 保持 crate 对外公共 API 不变，且 SHALL NOT 在同一变更中夹带行为改动。
3. WHERE 重构后 THE 系统 SHALL 使既有全部测试在无修改的情况下通过。
4. WHERE 本特性 THE 系统 SHALL NOT 新增 crate（统一模型继续留在应用服务层 crate 内，因下游 skills / hooks / mcp crate 由该层向下投影，不存在反向依赖需求）。
