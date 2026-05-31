# Requirements Document

## Introduction

DeepAgent Studio 在日常使用中会反复遇到同类问题：踩过的坑、好用的解决方法、常用命令、关键配置。今天这些经验随会话结束而流失，下次只能重新踩坑。本特性引入一个**可累积、可精准命中的知识库**：把使用过程中沉淀的知识以 Markdown 笔记（Codex + Obsidian 风格——纯文本、人可读、可被外部工具编辑）落盘保存，并在后续对话中**自动、精准地**把相关知识注入到模型上下文，从而避免重复踩坑。

本特性**复用**现有的 `deepagent-memory` 检索栈（chunking → contextualize → embed → BM25 → hybrid RRF fusion → rerank，见 `contextual_retrieval.rs`）和文档存储持久化（`repository.rs`），不重新发明检索引擎。新增的是：知识的**捕获/写入**、**Markdown vault 作为真源**、以及**“怎么保证精准命中”**的检索注入策略。

核心设计取舍（针对用户的核心疑问“后续怎么确保精准命中”）：检索**不只**在“遇到难题时开子代理去查”，而是采用**双通道**：
1. **被动自动注入（主通道）**：每一轮用户提问，系统先用 query 去知识库做混合检索，命中度超过阈值的条目作为上下文层（类似 L5 语义检索）注入，模型无需显式调用。这是“无感、默认开启、保证命中”的关键。
2. **主动工具查询（补充通道）**：暴露一个 `knowledge_search` 工具，模型在被动注入不够、或需要更深入查阅时显式调用；也可由子代理在复杂任务中调用。

> 说明：用户提到的“调用工具时出现乱码”是**独立的编码 Bug**（疑似 Windows 命令输出 GBK/UTF-8 问题），不在本知识库特性范围内，将单独作为 bugfix 处理。

## Glossary

- **知识条目 (Knowledge Entry / Note)**：知识库中的一篇 Markdown 笔记，带 frontmatter 元数据（标题、标签、类型、来源会话、时间）。
- **Vault**：知识库的磁盘目录（默认 `<project>/.deepagent/knowledge/` 或用户级 `~/.deepagent/knowledge/`），存放所有 Markdown 笔记——真源 (source of truth)。
- **被动注入 (Passive Injection)**：每轮对话前，自动检索并把高相关知识拼入系统/上下文，模型无需调用工具。
- **主动查询 (Active Query)**：模型/子代理通过 `knowledge_search` 工具显式检索知识库。
- **条目类型 (Entry Kind)**：pitfall（坑）、solution（解决方法）、command（常用命令）、config（重要配置）、note（一般笔记）。
- **命中阈值 (Hit Threshold)**：被动注入时，rerank 后的相关度分数下限；低于阈值的条目不注入，避免噪声污染上下文。

## Requirements

### Requirement 1: 知识捕获与写入

**User Story:** 作为用户，我希望系统能把使用过程中的坑、解决方法、常用命令、重要配置沉淀成知识库条目，这样我就不必重复踩坑。

#### Acceptance Criteria
1. WHEN 用户在会话中显式要求“记住这个 / 存到知识库 / remember this” THEN 系统 SHALL 创建一条知识条目并以 Markdown 文件写入 vault。
2. WHEN 模型在运行中调用 `knowledge_write` 工具（含 title、content、kind、tags 参数） THEN 系统 SHALL 将该条目写入 vault 并返回条目 id 与文件路径。
3. WHEN 写入一条知识条目 THEN 系统 SHALL 在文件头写入 YAML frontmatter，至少包含 `title`、`kind`、`tags`、`created_at`、`source_session`（来源会话 id）字段。
4. WHEN 写入的条目缺少 `kind` THEN 系统 SHALL 默认归类为 `note`。
5. IF 新条目与已有条目标题高度重复（同 title 或高相似度） THEN 系统 SHALL 更新已有条目（追加/合并）而非创建重复文件，并记录更新时间。
6. WHERE 写入操作 THE 系统 SHALL 把知识文件限定在 vault 目录内（禁止路径穿越），与现有 `WorkspaceRoot` 路径守卫一致。
7. WHEN 一条知识条目被写入或更新 THEN 系统 SHALL 同步更新检索索引（embedding + BM25），以便立即可被检索到。

### Requirement 2: Markdown Vault 作为真源（Obsidian 风格）

**User Story:** 作为用户，我希望知识库就是一个普通的 Markdown 文件夹，我能用 Obsidian / VS Code 等外部工具直接查看和编辑，不被锁死在应用内。

#### Acceptance Criteria
1. WHERE 知识库存储 THE 系统 SHALL 以每条目一份 `.md` 文件的形式存放于 vault 目录，文件名由标题 slug 化生成。
2. WHEN 应用启动或 vault 目录发生外部变更 THEN 系统 SHALL（重新）扫描 vault 并把所有 Markdown 条目载入检索索引。
3. IF 用户用外部编辑器修改/新增/删除了 vault 中的 Markdown 文件 THEN 系统 SHALL 在下次扫描时反映这些变更（以磁盘文件为准）。
4. WHERE frontmatter 缺失或格式不合法 THE 系统 SHALL 仍能把正文作为可检索内容载入（容错），并用文件名推断标题。
5. WHEN 渲染或导出知识条目 THEN 系统 SHALL 保持 Markdown 原文不做破坏性改写。

### Requirement 3: 被动自动注入（保证精准命中的主通道）

**User Story:** 作为用户，我希望相关知识在我提问时被自动、精准地用上，而不需要我或模型记得去查。

#### Acceptance Criteria
1. WHEN 用户提交一条对话消息 THEN 系统 SHALL 用该消息（必要时结合近期上下文）作为 query 对知识库执行混合检索（embedding + BM25 + RRF 融合 + rerank）。
2. WHEN 检索返回候选条目 THEN 系统 SHALL 仅注入 rerank 分数 ≥ 命中阈值的条目，且最多注入 N 条（N 可配置，默认值在 design 中确定）。
3. IF 没有任何候选超过命中阈值 THEN 系统 SHALL 不注入任何知识，且不向上下文添加占位/噪声内容。
4. WHEN 注入知识到上下文 THEN 系统 SHALL 以独立、带来源标注的上下文块呈现（标明来自知识库及条目标题），与现有 L5 语义检索块风格一致。
5. WHERE 被动注入的知识块 THE 系统 SHALL 将其放在 prompt 的可缓存边界处理逻辑允许的位置，避免破坏既有 prompt 缓存前缀（与 `SYSTEM_PROMPT_DYNAMIC_BOUNDARY` 约定一致）。
6. WHEN 注入的总 token 数可能超出预算 THEN 系统 SHALL 依据相关度截断，优先保留高分条目。
7. WHERE 被动注入功能 THE 系统 SHALL 提供开关（默认开启），用户可在设置中关闭。

### Requirement 4: 主动工具查询（补充通道）

**User Story:** 作为模型/子代理，我希望在被动注入不足时能主动、按需地查询知识库，以便深入获取解决方案。

#### Acceptance Criteria
1. WHERE 工具集 THE 系统 SHALL 提供一个 `knowledge_search` 工具，入参含 `query` 和可选 `kind`、`limit`，返回命中的条目（标题、kind、正文片段、来源、相关度）。
2. WHEN 模型调用 `knowledge_search` THEN 系统 SHALL 执行与被动注入相同的混合检索并返回结构化结果。
3. WHERE 系统提示词 THE 系统 SHALL 告知模型：遇到不熟悉的报错、重复性问题或需要项目特定约定时，应调用 `knowledge_search` 查阅既有经验，而不是凭空猜测。
4. WHEN `knowledge_search` 无命中 THEN 系统 SHALL 返回明确的空结果（而非报错），使模型知道知识库暂无相关经验。
5. WHERE 子代理（`task` 工具派生） THE 系统 SHALL 允许子代理也能调用 `knowledge_search`（只读检索，安全）。
6. WHERE `knowledge_search` THE 系统 SHALL 把它归类为只读、并发安全（`RiskLevel::Safe`），无需审批即可调用。

### Requirement 5: 知识库可视化管理

**User Story:** 作为用户，我希望能在应用里浏览、搜索、编辑、删除知识条目，掌握知识库里有什么。

#### Acceptance Criteria
1. WHERE 桌面应用 THE 系统 SHALL 提供一个知识库视图，列出所有条目（标题、kind、标签、更新时间）。
2. WHEN 用户在知识库视图中搜索 THEN 系统 SHALL 用混合检索返回排序后的条目列表。
3. WHEN 用户选中一条条目 THEN 系统 SHALL 展示其 Markdown 正文与元数据。
4. WHEN 用户在视图中编辑并保存一条条目 THEN 系统 SHALL 写回对应 Markdown 文件并更新索引。
5. WHEN 用户删除一条条目 THEN 系统 SHALL 删除对应文件并从索引移除。
6. WHERE 知识库视图的所有提示/反馈 THE 系统 SHALL 使用既有 message 组件，不用内联红字。

### Requirement 6: 检索质量与精准度保障

**User Story:** 作为用户，我担心知识库越大越容易“答非所问”，我希望命中是精准的、可解释的、可调的。

#### Acceptance Criteria
1. WHERE 检索 THE 系统 SHALL 同时使用稠密向量（语义近似）与 BM25（精确关键词，如命令名、配置键、报错原文）并用 RRF 融合，确保“查命令/配置原文”这类精确匹配也能命中。
2. WHEN 检索对较长条目执行 THEN 系统 SHALL 在 chunk 级检索并对 chunk 做上下文化（contextualize），复用 `ContextualRetriever`，以提升长文档的命中精度。
3. WHERE 命中阈值与注入条数 THE 系统 SHALL 提供可配置参数，使用户能在“宁缺毋滥”与“尽量召回”之间权衡。
4. WHEN 一条知识被实际检索命中并注入/返回 THEN 系统 SHALL 更新该条目的访问统计（recency/access_count），供排序参考（复用 `MemoryItem::touch`）。
5. WHERE 检索结果 THE 系统 SHALL 为每条命中附带来源（条目标题/文件）与相关度分数，便于用户判断与调试。

### Requirement 7: 离线、隐私与作用域

**User Story:** 作为用户，我希望知识库默认在本地、可按项目隔离，且不依赖联网。

#### Acceptance Criteria
1. WHERE 默认嵌入与检索 THE 系统 SHALL 可在完全离线下工作（复用现有 `HashingEmbedder` 与本地 BM25），不强制外部 embedding API。
2. WHERE vault 作用域 THE 系统 SHALL 支持“项目级”知识库（随项目目录），并为后续“用户级/全局”知识库预留扩展位。
3. WHERE 知识库内容 THE 系统 SHALL 不把知识条目正文写入任何外部网络端点（除非用户显式发起需要联网的操作）。
4. WHERE 索引数据 THE 系统 SHALL 把 embedding/索引持久化到既有文档存储（`MemoryRepository` 模式），使重启后无需全量重算即可检索。

### Requirement 8: 不回归现有质量门禁

**User Story:** 作为维护者，我希望本特性遵守既有工程纪律，不破坏现有测试与构建。

#### Acceptance Criteria
1. WHEN 本特性实现完成 THEN 系统 SHALL 保持 `cargo test --workspace --offline` 全绿。
2. WHEN 本特性实现完成 THEN 系统 SHALL 保持 `cargo fmt --all -- --check` 与 `cargo clippy --workspace --all-targets --offline -- -D warnings` 干净。
3. WHEN 前端改动完成 THEN 系统 SHALL 保持 `pnpm build`（tsc + vite）通过。
4. WHERE 网络相关能力 THE 系统 SHALL 走 feature gate，默认 offline 可构建。
5. WHERE 新增后端能力 THE 系统 SHALL 附带单元测试覆盖捕获、检索命中/未命中、阈值过滤、vault 扫描容错等关键路径。

### Requirement 9: 会话自动沉淀（草稿 + 人工确认）

**User Story:** 作为用户，当系统在一次对话中经历了「报错 → 多步排查 → 最终解决」的过程，我希望这段经验能被自动总结并保存下来，但不要把噪声直接塞进知识库，而是先给我一个草稿让我一键采纳或丢弃。

> 背景：被动注入开关只控制「读」（检索注入），不控制「写」。本需求补上「写」侧的确定性自动沉淀——方案 A（轻、可控）：只在确实发生错误恢复时触发一次小的总结调用，产出**待确认草稿**，绝不直接污染正式知识库。

#### Acceptance Criteria
1. WHEN 一次会话 run 正常完成（任务到达 `Completed`） THEN 系统 SHALL 检查该 run 的事件序列是否构成「错误恢复」模式（至少一次工具调用失败 `ok=false`，且任务最终成功完成）。
2. IF 该 run 未发生任何工具失败（没有踩坑） THEN 系统 SHALL 不触发自动总结（避免对平凡对话产生噪声）。
3. WHEN 检测到错误恢复模式 THEN 系统 SHALL 发起一次小的、**非流式**的总结模型调用，产出一条结构化草稿（标题、kind、正文、标签）。
4. IF 总结模型判定本次经历不值得沉淀（无可复用价值） THEN 系统 SHALL 允许其返回「跳过」，系统据此不生成草稿。
5. WHEN 生成一条草稿 THEN 系统 SHALL 将其存为**草稿状态**（`pending`），且草稿 SHALL NOT 进入被动注入与 `knowledge_search` 的检索结果（草稿不影响正式知识库）。
6. WHERE 草稿存储 THE 系统 SHALL 把草稿与正式条目隔离（独立子目录/状态标记），使其仍为可读的 Markdown，但不被当作真源知识检索。
7. WHEN 用户在知识库视图中查看草稿 THEN 系统 SHALL 清晰标识其「待确认」状态，并提供「采纳」与「丢弃」两个操作。
8. WHEN 用户「采纳」一条草稿 THEN 系统 SHALL 将其转为正式条目（落入正式 vault、进入检索索引），并从草稿区移除。
9. WHEN 用户「丢弃」一条草稿 THEN 系统 SHALL 删除该草稿且不影响正式知识库。
10. WHERE 自动沉淀功能 THE 系统 SHALL 提供开关（可独立于被动注入开关），默认行为不得在未配置模型/离线时报错（无 API key 或总结失败时静默跳过，不打断主 run）。
11. WHERE 自动总结调用 THE 系统 SHALL 在主 run 完成后异步进行，不得阻塞或延迟用户看到主回答。
12. WHEN 采纳一条由会话生成的草稿 THEN 系统 SHALL 在其 `source_session` 字段记录来源会话 id，便于回溯。
