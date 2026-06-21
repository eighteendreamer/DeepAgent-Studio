# 需求文档

## 引言

DeepAgent Studio 目前的行为是一个编码（coding）代理。产品愿景（见 `DeepAgent Studio 办公与现实交互开发方案.md`）是把它扩展为一个"日常办公"助手。本 spec 只交付**第一个地基步骤**：让现有的工作模式开关真正改变代理行为，并把面向办公的技能（skill）接入该行为。

应用已经在 `OnboardingWizard.tsx`、`GeneralSettings.tsx` 和主视图（`StartView.tsx`）中暴露了一个"工作模式"（Work Mode）开关（`code` / `daily`），但它目前仅停留在 UI 层：所选值存储在 `localStorage` 中，从未到达后端 `ChatService`，因此切换模式不会改变系统提示、可用技能或任何行为。

本 spec 以**纯增量改动**的方式填补这一空缺：
- 把工作模式作为真正的设置持久化，并让主视图能够切换并记住它。
- 把模式透传到 `ChatService.run` / `run_in_session`。
- 按模式切换系统提示人设（`daily` 人设以分层方式叠加，且不破坏前缀缓存边界；`code` 保持不变）。
- 用来源于 `SKILL.md` 的分类/标签（`office` / `coding` / `general`）扩展技能元数据。
- 按模式偏好并被动激活技能（`daily` 下偏向办公，`code` 下偏向编码）。
- 在 `daily` 模式下自动发现并激活本地办公技能。
- 让 UI 模式切换变得可用，并配一个影响下一轮对话的轻量模式徽章。
- 保持完全向后兼容（默认 `code` 与今天的行为一致）。
- 遵守非功能约束（不新增 crate、clippy 无告警、核心逻辑有单元测试、系统提示前缀缓存完好）。

**明确的范围外（推迟到后续 spec）：** 诸如 `deepagent-office` / `vision` / `computer-use` 等新 crate；Connections 系统；OAuth；办公系统 API；MCP 办公连接器；视觉 / computer-use / 物理设备；以及对邮件和日历的任何真实读写。

## 术语表

- **System**：整个 DeepAgent Studio 应用（桌面应用加后端 crate）。
- **Work_Mode**：一个被持久化的用户偏好，恰有两个取值 `code` 和 `daily`，用于选择代理的行为画像。
- **Chat_Service**：`deepagent-app-core` 中的后端 `ChatService`，其 `run` 和 `run_in_session` 方法驱动一轮代理对话。
- **System_Prompt_Builder**：生成有效系统提示的组件（`build_system_prompt` 以及 `deepagent-prompts` 和 `crate::system_prompt::system_prompt_base` 中的分层组装逻辑）。
- **Prompt_Boundary**：`SYSTEM_PROMPT_DYNAMIC_BOUNDARY` 标记，用于分隔系统提示中可前缀缓存的静态部分与每次请求动态变化的部分。
- **Persona_Segment**：一段系统提示文本，为给定 Work_Mode 定义代理的语气和取向。
- **Skill**：由 `SKILL.md` 文件定义、被解析为 `SkillMeta` 加正文与资源的可复用能力。
- **Skill_Category**：技能上的分类标签，取值为 `office`、`coding` 或 `general`，来源于 `SKILL.md` 的 frontmatter。
- **Skills_Service**：后端 `SkillsService`，暴露 `SkillDto` 和被动激活预览（`SkillActivationDto`）。
- **Passive_Activation**：在无需用户显式调用的情况下，对与当前对话相关的技能进行打分并呈现的机制。
- **Settings_Service**：持久化用户偏好的后端服务。
- **Mode_Badge**：主视图中显示当前 Work_Mode 的轻量 UI 指示器。

## 需求

### 需求 1：工作模式持久化

**User Story:** 作为用户，我希望我所选的工作模式被保存下来，以便应用在多个会话间记住我的偏好。

#### 验收标准

1. THE Settings_Service SHALL 将 Work_Mode 持久化为一个设置，其值为 `code` 或 `daily` 之一。
2. WHEN 用户在主视图中选择某个 Work_Mode 时, THE Settings_Service SHALL 把所选值存储为被持久化的 Work_Mode。
3. WHEN 应用启动且存在被持久化的 Work_Mode 时, THE System SHALL 把被持久化的 Work_Mode 加载为活动 Work_Mode。
4. IF 应用启动时不存在被持久化的 Work_Mode, THEN THE System SHALL 使用 `code` 作为活动 Work_Mode。
5. WHEN 用户在任一提供该开关的位置更改 Work_Mode 时, THE System SHALL 在所有其他显示该值的位置反映同一活动 Work_Mode。

### 需求 2：模式向后端传播

**User Story:** 作为用户，我希望我的工作模式能到达后端，以便切换模式时真正改变代理的响应方式。

#### 验收标准

1. THE Chat_Service SHALL 接受一个 Work_Mode 值作为一次运行的输入。
2. WHEN 主视图启动一轮代理对话时, THE System SHALL 把活动 Work_Mode 传递给 `ChatService.run`。
3. WHEN 主视图继续一个已有会话时, THE System SHALL 把活动 Work_Mode 传递给 `ChatService.run_in_session`。
4. IF 一次运行在没有 Work_Mode 值的情况下被启动, THEN THE Chat_Service SHALL 为该次运行使用 `code` 作为 Work_Mode。
5. WHEN 活动 Work_Mode 在两轮对话之间发生变化时, THE Chat_Service SHALL 使用为当前这一轮提供的 Work_Mode 值。

### 需求 3：模式专属的系统提示人设

**User Story:** 作为处于 daily 模式的用户，我希望助手使用平实、面向办公的语言，以便响应贴合日常工作而非开发者术语。

#### 验收标准

1. WHILE 某次运行的 Work_Mode 为 `daily`, THE System_Prompt_Builder SHALL 在系统提示中包含 daily Persona_Segment。
2. WHILE 某次运行的 Work_Mode 为 `code`, THE System_Prompt_Builder SHALL 生成与当前 `code` 系统提示逐字节相同的系统提示。
3. THE System_Prompt_Builder SHALL 把 Persona_Segment 放在 Prompt_Boundary 的静态一侧。
4. WHEN System_Prompt_Builder 为某个固定的 Work_Mode 组装提示时, THE Prompt_Boundary 之前的静态部分 SHALL 在使用同一 Work_Mode 的多次运行间逐字节相同。
5. THE daily Persona_Segment SHALL 指示代理使用面向办公、低术语含量的语言。

### 需求 4：技能分类元数据

**User Story:** 作为技能作者，我希望给技能打上分类标签，以便系统能将技能匹配到相关的工作模式。

#### 验收标准

1. THE Skill SHALL 支持一个 Skill_Category，其值为 `office`、`coding` 或 `general` 之一。
2. WHEN 一个 `SKILL.md` 文件在其 frontmatter 中声明了 Skill_Category 时, THE Skills_Service SHALL 把该值解析进该技能的 `SkillMeta`。
3. IF 一个 `SKILL.md` 文件未声明 Skill_Category, THEN THE Skills_Service SHALL 指定 `general` 作为 Skill_Category。
4. IF 一个 `SKILL.md` 文件声明的 Skill_Category 值不在 `office`、`coding` 或 `general` 之内, THEN THE Skills_Service SHALL 指定 `general` 作为 Skill_Category 并记录一条解析告警。
5. THE Skills_Service SHALL 在返回给用户界面的 `SkillDto` 中包含 Skill_Category。

### 需求 5：基于模式的技能偏好

**User Story:** 作为用户，我希望助手偏好契合我当前模式的技能，以便建议的能力与我的任务相关。

#### 验收标准

1. WHILE 某次运行的 Work_Mode 为 `daily`, THE Skills_Service SHALL 在 Passive_Activation 期间将 Skill_Category 为 `office` 的技能排在 Skill_Category 为 `coding` 的技能之上。
2. WHILE 某次运行的 Work_Mode 为 `code`, THE Skills_Service SHALL 在 Passive_Activation 期间将 Skill_Category 为 `coding` 的技能排在 Skill_Category 为 `office` 的技能之上。
3. THE Skills_Service SHALL 在每一种 Work_Mode 下都把 Skill_Category 为 `general` 的技能纳入 Passive_Activation 的候选范围。
4. WHEN Skills_Service 为某次运行计算 Passive_Activation 结果时, THE Skills_Service SHALL 应用与该次运行所提供的 Work_Mode 相对应的排序。

### 需求 6：本地办公技能发现与激活

**User Story:** 作为 daily 模式用户，我希望相关的办公技能无需手动设置即可就绪，以便我能立即开始工作。

#### 验收标准

1. THE Skills_Service SHALL 从已配置的技能根目录中发现本地可用、Skill_Category 为 `office` 的技能。
2. WHILE 某次运行的 Work_Mode 为 `daily`, THE Skills_Service SHALL 让所发现的办公技能在该次运行中可用于 Passive_Activation。
3. WHILE 某次运行的 Work_Mode 为 `code`, THE Skills_Service SHALL NOT 为 Passive_Activation 自动优先办公技能。
4. WHEN 一个新的办公技能被添加到某个技能根目录且 Skills_Service 重新加载时, THE Skills_Service SHALL 在后续的发现结果中包含该新办公技能。

### 需求 7：可用的模式切换与徽章

**User Story:** 作为用户，我希望能从主视图切换模式并看到当前激活的模式，以便我知道助手在我的下一条消息中会如何表现。

#### 验收标准

1. THE 主视图 SHALL 显示一个 Mode_Badge，呈现活动 Work_Mode。
2. WHEN 用户在主视图中切换 Work_Mode 时, THE System SHALL 把 Mode_Badge 更新为新选择的 Work_Mode。
3. WHEN 用户切换 Work_Mode 之后发送一条消息时, THE System SHALL 把新选择的 Work_Mode 应用于该消息的运行。
4. WHEN 用户切换 Work_Mode 时, THE System SHALL 在无需重启应用的情况下应用该变更。

### 需求 8：向后兼容

**User Story:** 作为现有用户，我希望当前行为保持不变，以便这一改动不会扰乱我的编码工作流。

#### 验收标准

1. WHILE 活动 Work_Mode 为 `code`, THE System SHALL 在系统提示、可用工具和技能处理方面产生与本特性之前相同的代理行为。
2. IF 本特性之前的某个已存储配置不包含 Work_Mode 值, THEN THE System SHALL 把活动 Work_Mode 视为 `code`。
3. WHEN 一个在本特性之前创建的已有会话被继续时, THE Chat_Service SHALL 以 Work_Mode `code` 运行，除非提供了不同的 Work_Mode。
4. THE System SHALL 保持所有现有技能可用，无论其 Skill_Category 为何。

### 需求 9：非功能约束

**User Story:** 作为维护者，我希望本特性在现有 crate 内实现并经过验证，以便代码库保持整洁、提示缓存保持有效。

#### 验收标准

1. THE System SHALL 在不向工作区新增 crate 的前提下实现本特性。
2. WHEN 项目用 clippy 进行 lint 时, THE System SHALL 不产生任何来自为本特性新增或修改的代码的 clippy 告警。
3. THE System SHALL 为 Work_Mode 人设选择、Skill_Category 解析以及基于模式的技能排序提供单元测试。
4. WHEN System_Prompt_Builder 为某个固定的 Work_Mode 生成提示时, THE Prompt_Boundary 之前的部分 SHALL 按现有 Prompt_Boundary 约定保持可前缀缓存。
