# Design Document

## Overview

知识库 (Knowledge Base, KB) 让 DeepAgent Studio 把日常使用中沉淀的经验——坑、解决方法、常用命令、关键配置——以 **Markdown 笔记（Obsidian 风格，磁盘真源）** 保存，并在后续对话中**自动且精准地**注入相关知识，避免重复踩坑。

核心是**双通道检索注入**，直接回答“怎么保证精准命中”：

1. **被动自动注入（主通道，默认开启）**：每轮用户提问前，系统用 query 检索 KB，命中度 ≥ 阈值的条目作为带来源标注的上下文块注入（类似 L5 语义检索）。模型无需记得去查，命中是默认保证的。
2. **主动工具查询（补充通道）**：暴露 `knowledge_search`（只读检索）与 `knowledge_write`（写入条目）两个工具，模型/子代理在被动注入不足时按需深查或沉淀新知识。

设计的最大杠杆是**复用**：检索引擎（chunk → contextualize → embed → BM25 → RRF → rerank）已存在于 `deepagent-memory`（`ContextualRetriever`、`HybridRetriever`），持久化模式存在于 `MemoryRepository`，vault 扫描/frontmatter 解析模式存在于 `deepagent-skills`。本特性新增一个薄 crate `deepagent-knowledge` 负责 **vault（markdown 真源）+ 捕获 + 检索编排**，并在 `deepagent-app-core` 加 `KnowledgeService` 把它接到 ChatService（注入 + 工具）与桌面 UI。

### 已解决的待定决策（来自 requirements 的开放问题）

- **命中阈值 / 注入条数**：被动注入默认 `min_score = 0.30`（rerank 归一化分数），`max_inject = 3` 条，`max_inject_tokens ≈ 1200`。三者均可配置（设置面板 + `KnowledgeConfig`）。
- **作用域**：先做**项目级**（`<project>/.deepagent/knowledge/`）+ **用户级全局**（`~/.deepagent/knowledge/`）。两个 vault 都加载进同一检索索引，命中结果标注来源 scope。后续可扩展更多 scope。
- **捕获方式**：Phase 1 做 **`knowledge_write` 工具** + **用户显式“记住这个”**（前端发一条带指令的消息，模型据系统提示调用 `knowledge_write`）。**会话结束自动总结沉淀**作为 Phase 2 的可选增强，设计上预留 `KnowledgeService::capture_from_session` 接口，但不在首期实现以控制范围与避免噪声。

## Architecture

### 组件关系

```text
┌────────────────────────── Desktop (Tauri + React) ──────────────────────────┐
│  KnowledgeView.tsx (列表/搜索/编辑/删除)   Composer "记住这个"                  │
│        │ invoke                                  │ run_chat                    │
└────────┼─────────────────────────────────────────┼──────────────────────────┘
         │  kb_list / kb_search / kb_get / kb_save / kb_delete / kb_reload
         ▼                                          ▼
┌──────────────────────── deepagent-app-core ─────────────────────────────────┐
│  KnowledgeService                          ChatService                        │
│   - vault(s) 加载/保存/扫描                   - run 前: passive inject ────┐    │
│   - search()/write()/list()/get()/delete()   - 注册 knowledge_search ──┐  │    │
│   - KnowledgeDto / KnowledgeHitDto            - 注册 knowledge_write ─┐ │  │    │
└───────────────┬───────────────────────────────────────────────────┼─┼──┼────┘
                │ uses                                                │ │  │
                ▼                                                     ▼ ▼  ▼
┌──────────────────────── deepagent-knowledge (new) ──────────────────────────┐
│  KnowledgeBase                                                                │
│   - Vault: markdown 文件 I/O + frontmatter (复用 deepagent-skills::frontmatter)│
│   - KnowledgeEntry: 元数据 + 正文 (kind/tags/created/source_session)           │
│   - 索引: ContextualRetriever (chunk-level)  +  HybridRetriever (entry-level)  │
│   - 持久化: 复用 deepagent-persistence 文档存储 (embedding 缓存)               │
└──────────────────────────────────────────────────────────────────────────────┘
                │ reuses
                ▼
   deepagent-memory (ContextualRetriever/HybridRetriever/Embedder/BM25)
   deepagent-persistence (DocumentStore — embedding 缓存)
   deepagent-skills::frontmatter (YAML frontmatter 解析)
```

### 双通道在一次 run 中的位置

```text
用户提问
  │
  ├─(被动通道) ChatService.run 开始
  │     query = 用户消息(+近期上下文)
  │     hits = KnowledgeService.search(query, min_score, max_inject)
  │     if !hits.empty(): system_prompt += "# 相关知识(检索)\n[source: …]\n…"
  │           ↑ 放在 SYSTEM_PROMPT_DYNAMIC_BOUNDARY 之后(易变段)，不破坏可缓存前缀
  │
  ├─(主动通道) 模型在 turn 中可调用:
  │     knowledge_search({query, kind?, limit?})  -> 结构化命中
  │     knowledge_write({title, content, kind, tags})  -> 写入 vault + 更新索引
  │
  └─ run 继续 (THINK→TOOL→OBSERVE…)
```

## Components and Interfaces

### 1. 新 crate：`deepagent-knowledge`

放检索/存储无关 UI 的核心逻辑，默认 offline（与现有 crate 一致）。

```rust
// crates/deepagent-knowledge/src/entry.rs

/// 一条知识条目的类型（驱动归类与 UI 标签）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind { Pitfall, Solution, Command, Config, Note }

impl EntryKind {
    pub fn label(&self) -> &'static str;          // "pitfall" | "solution" | ...
    pub fn parse(s: &str) -> EntryKind;           // 容错；未知 → Note
    /// 映射到既有 memory 的 ObservationType（复用 tier/排序）。
    pub fn observation_type(&self) -> deepagent_memory::ObservationType;
}

/// vault 作用域（命中结果带回此标注）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope { Project, Global }

/// 一条知识条目（= 一份 .md 文件）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KnowledgeEntry {
    pub id: String,                 // 稳定 id（= 文件名 slug）
    pub title: String,
    pub kind: EntryKind,
    pub tags: Vec<String>,
    pub created_at: i64,            // Unix ms
    pub updated_at: i64,
    pub source_session: Option<String>,
    pub scope: Scope,
    pub body: String,              // markdown 正文（frontmatter 之后的部分）
}

impl KnowledgeEntry {
    /// 渲染成完整 .md 文件文本（YAML frontmatter + body）。
    pub fn to_markdown(&self) -> String;
    /// 从 .md 文件文本解析（容错：frontmatter 缺失则用文件名/默认值）。
    pub fn from_markdown(id: &str, scope: Scope, raw: &str) -> KnowledgeEntry;
    /// 供检索索引使用的可搜索文本（title + tags + body）。
    pub fn searchable_text(&self) -> String;
}
```

```rust
// crates/deepagent-knowledge/src/vault.rs

/// 一个 markdown vault 目录（真源）。每条目一份 .md 文件。
pub struct Vault { root: PathBuf, scope: Scope }

impl Vault {
    pub fn new(root: impl Into<PathBuf>, scope: Scope) -> Self;
    pub fn scope(&self) -> Scope;
    /// 扫描目录下所有 *.md，解析成条目（容错跳过坏文件，记 warn）。
    pub fn scan(&self) -> Result<Vec<KnowledgeEntry>>;
    /// 写入/覆盖一条目对应的 .md（路径限定在 root 内，禁止穿越）。
    pub fn write(&self, entry: &KnowledgeEntry) -> Result<PathBuf>;
    /// 删除一条目的 .md。
    pub fn delete(&self, id: &str) -> Result<bool>;
    /// 由标题生成稳定、安全的文件名 slug。
    pub fn slug(title: &str) -> String;
}
```

```rust
// crates/deepagent-knowledge/src/lib.rs

/// 检索/注入调参。
#[derive(Debug, Clone)]
pub struct KnowledgeConfig {
    pub min_score: f32,        // 被动注入命中阈值，默认 0.30
    pub max_inject: usize,     // 被动注入最多条数，默认 3
    pub max_inject_tokens: usize, // 注入 token 上限，默认 ~1200
    pub passive_enabled: bool, // 被动注入开关，默认 true
}
impl Default for KnowledgeConfig { /* 上述默认值 */ }

/// 一次检索命中。
#[derive(Debug, Clone)]
pub struct KnowledgeHit {
    pub entry: KnowledgeEntry,
    pub score: f32,            // rerank 归一化分数 [0,1]
    pub excerpt: String,       // 命中片段（chunk 文本）
}

/// 知识库门面：组合多个 vault + 检索索引 + 捕获。
pub struct KnowledgeBase<E: Embedder> {
    vaults: Vec<Vault>,
    config: KnowledgeConfig,
    // chunk 级上下文检索（长文档命中精度），见 design 取舍
    retriever: ContextualRetriever<E, HeadingContextualizer, ScoreReranker>,
    // id -> entry 的内存映射（doc_id == entry.id）
    entries: BTreeMap<String, KnowledgeEntry>,
}

impl<E: Embedder + Clone> KnowledgeBase<E> {
    pub fn new(vaults: Vec<Vault>, embedder: E, config: KnowledgeConfig) -> Self;
    /// 扫描所有 vault，重建索引。返回载入条目数。
    pub fn load_all(&mut self) -> Result<usize>;
    /// 检索（被动注入与 knowledge_search 共用）。
    pub fn search(&self, query: &str, kind: Option<EntryKind>, limit: usize) -> Vec<KnowledgeHit>;
    /// 被动注入用：检索 + 阈值过滤 + token 预算截断 → 上下文块字符串（空则返回空串）。
    pub fn passive_block(&self, query: &str) -> String;
    /// 写入新条目或更新已有（标题/高相似度去重），落盘并更新索引。返回最终条目。
    pub fn write(&mut self, draft: KnowledgeDraft) -> Result<KnowledgeEntry>;
    pub fn list(&self) -> Vec<KnowledgeEntry>;
    pub fn get(&self, id: &str) -> Option<&KnowledgeEntry>;
    pub fn delete(&mut self, id: &str) -> Result<bool>;
    pub fn config(&self) -> &KnowledgeConfig;
    pub fn set_config(&mut self, config: KnowledgeConfig);
}

/// 写入草稿（来自工具或 UI）。
pub struct KnowledgeDraft {
    pub title: String,
    pub body: String,
    pub kind: EntryKind,
    pub tags: Vec<String>,
    pub source_session: Option<String>,
    pub scope: Scope,   // 默认 Project
}
```

**检索器选择**：被动注入与 `knowledge_search` 都走 `ContextualRetriever`（chunk 级 + 上下文化），因为知识条目正文可能较长（一段坑+解决方法+命令），chunk 级命中比整条目命中更精准（满足 Req 6.2）。条目级 `HybridRetriever` 不再单独引入，避免重复索引。`excerpt` 取命中 chunk 文本；阈值过滤用 `RetrievedChunk::score`。

**精确匹配保证**（Req 6.1）：`ContextualRetriever` 内部已是 dense + BM25 + RRF，BM25 分支保证命令名/配置键/报错原文这类精确 token 能命中。

### 2. 持久化 / embedding 缓存

复用 `deepagent-persistence::DocumentStore`：在 collection `"knowledge_index"` 下缓存 `entry.id -> embedding`（按 chunk 可选）。启动时若磁盘 md 的 `updated_at` 与缓存一致则复用 embedding，否则重算（满足 Req 7.4：重启不必全量重算）。MVP 可先简单实现为“每次 load_all 重算 embedding”（`HashingEmbedder` 很快、纯本地），缓存作为 design 记录的后续优化；**真源永远是磁盘 md，缓存只是加速**。

### 3. `KnowledgeService`（app-core）

```rust
// crates/deepagent-app-core/src/knowledge_service.rs

pub struct KnowledgeDto {        // UI 列表/详情
    pub id: String, pub title: String, pub kind: String,
    pub tags: Vec<String>, pub scope: String,
    pub created_at: i64, pub updated_at: i64,
    pub source_session: Option<String>, pub body: String,
}
pub struct KnowledgeHitDto {     // 搜索/工具结果
    pub id: String, pub title: String, pub kind: String,
    pub scope: String, pub score: f32, pub excerpt: String,
}

pub struct KnowledgeService {
    inner: Mutex<KnowledgeBase<HashingEmbedder>>,
}

impl KnowledgeService {
    /// 用项目 vault + 全局 vault 打开并载入。
    pub fn open(project_root: &Path, global_root: &Path) -> Result<Self>;
    pub fn reload(&self) -> Result<usize>;
    pub fn list(&self) -> Vec<KnowledgeDto>;
    pub fn get(&self, id: &str) -> Option<KnowledgeDto>;
    pub fn search(&self, query: &str, kind: Option<&str>, limit: usize) -> Vec<KnowledgeHitDto>;
    pub fn save(&self, dto: KnowledgeDraftDto) -> Result<KnowledgeDto>; // 新建或更新
    pub fn delete(&self, id: &str) -> Result<bool>;
    /// 被动注入块（ChatService 调用）。
    pub fn passive_block(&self, query: &str) -> String;
    pub fn set_passive_enabled(&self, on: bool);
}
```

`KnowledgeService` 用 `Mutex<KnowledgeBase>`（与 `AppService`/`SkillsService` 一致），可 `Arc` 共享给 ChatService 和 Tauri 命令。

### 4. 两个新工具（deepagent-builtins）

与现有 `ask_user_tool`/`task_tool` 同模式：trait 抽象 + 注入实现，便于离线测试。

```rust
// crates/deepagent-builtins/src/knowledge_tools.rs

pub const KNOWLEDGE_SEARCH_TOOL_NAME: &str = "knowledge_search";
pub const KNOWLEDGE_WRITE_TOOL_NAME: &str = "knowledge_write";

/// 工具到知识库的后端桥（app-core 注入真实实现；headless 用 stub）。
#[async_trait]
pub trait KnowledgeBackend: Send + Sync {
    async fn search(&self, query: &str, kind: Option<String>, limit: usize)
        -> Result<Vec<KnowledgeToolHit>>;
    async fn write(&self, draft: KnowledgeToolDraft) -> Result<String>; // 返回 id
}

pub struct KnowledgeSearchTool<B: KnowledgeBackend> { backend: B }
pub struct KnowledgeWriteTool<B: KnowledgeBackend>  { backend: B }
```

- `knowledge_search`：`RiskLevel::Safe`、只读、并发安全、无需审批（Req 4.6/4.5）；schema `{ query, kind?, limit? }`。
- `knowledge_write`：`RiskLevel::Low`、需要 `WorkspaceWrite`（写文件）；schema `{ title, content, kind?, tags? }`。写入限定 vault 内（Req 1.6）。

注册时机与 `task` 工具一致：在 `ChatService::run_in_session` 组装主 registry 时注入（子代理只给 `knowledge_search`，不给 `knowledge_write`，避免子代理乱写——与 Claude Code 的 agent-disallowed-tools 思路一致）。

### 5. ChatService 接线

- **被动注入**：在 `build_system_prompt(root)` 之后，把 `knowledge_service.passive_block(prompt)` 的结果**追加到动态段**（`SYSTEM_PROMPT_DYNAMIC_BOUNDARY` 之后），不破坏可缓存前缀（Req 3.5）。空串则不加（Req 3.3）。受 `KnowledgeConfig.passive_enabled` 控制（Req 3.7）。
- **工具注册**：把 `KnowledgeSearchTool`/`KnowledgeWriteTool` 用真实 `KnowledgeBackend`（包一层 `Arc<KnowledgeService>`）注册进主 registry。
- **系统提示补强**（Req 4.3）：在 `SYSTEM_PROMPT_BASE` 的 “Using your tools” 段加一句：遇到不熟悉报错/重复性问题/项目约定时，先 `knowledge_search` 查既有经验；解决了值得复用的问题后，用 `knowledge_write` 沉淀。
- ChatService 加 `with_knowledge(Arc<KnowledgeService>)` builder（与 `with_mcp`/`with_projects` 一致）；未接时被动注入为空、工具不注册（向后兼容，现有测试不受影响）。

### 6. Tauri 命令 + 前端

`apps/desktop/src-tauri/src/lib.rs` 新增命令：`kb_list`、`kb_search`、`kb_get`、`kb_save`、`kb_delete`、`kb_reload`、`kb_set_passive`，均薄封装 `KnowledgeService`。`AppState` 加 `knowledge: Arc<KnowledgeService>`，并 `chat = ...with_knowledge(knowledge.clone())`。项目 vault 用活跃项目根的 `.deepagent/knowledge/`，全局 vault 用 app 数据目录的 `knowledge/`。

前端 `api.ts` 加对应函数（含 browser fallback），`types.ts` 加 `KnowledgeEntry`/`KnowledgeHit`。新增 `KnowledgeView.tsx`（列表 + 搜索框 + 详情/编辑 + 删除），挂到 Sidebar（“知识库”入口，紧邻“技能”）。所有提示用既有 `message` 组件（Req 5.6）。保留既有 Tailwind 风格。

## Data Models

### Markdown 文件格式（vault 真源）

```markdown
---
title: PowerShell 管道命令被 ^C 中断
kind: pitfall
tags: [windows, powershell, cargo]
created_at: 1730000000000
updated_at: 1730000000000
source_session: ses_019e7bxx
---

## 现象
`cargo test | Select-String` 经常 exit -1，是 UI artifact 不是真失败。

## 解决
改用 `> out.txt 2>&1` 重定向后再读，或看 `$LASTEXITCODE`。
```

- 文件名：`<slug(title)>.md`，slug 规则：小写、非字母数字转 `-`、压缩连续 `-`、截断长度；冲突追加短哈希。
- frontmatter 解析复用 `deepagent_skills::frontmatter`（已有、无外部依赖）。容错：缺失 frontmatter → 用文件名当 title、kind=note（Req 2.4）。

### 去重/更新逻辑（Req 1.5）

`write` 时：若存在同 `id`（slug 相同）或与现有条目标题归一化后相等 → 视为更新（覆盖 body、刷新 `updated_at`、保留 `created_at`）。否则新建。高相似度（可选，MVP 用标题精确匹配，design 记录“相似度合并”为后续增强）。

## Error Handling

- vault 路径穿越：`Vault::write`/`delete` 复用 `WorkspaceRoot` 同款校验（在 vault root 内），越界返回 `CoreError::invalid`（Req 1.6）。
- 坏 md 文件：`scan` 跳过并 `tracing::warn`，不让单个坏文件阻断整库加载（Req 2.4）。
- 工具失败：`knowledge_search` 无命中返回**成功的空结果**（`{ "hits": [], "count": 0 }`），不报错（Req 4.4）。`knowledge_write` 失败返回 `ToolOutput::failure`，由现有“失败结构化回喂”让模型重试。
- 检索为空/query 为空：`search`/`passive_block` 返回空，调用方安全处理（不注入噪声）。

## Correctness Properties

These invariants must hold and are the backbone of the test suite.

### Property 1: Markdown 是唯一真源
索引、embedding、缓存都是磁盘 `.md` 的派生物。删除/编辑磁盘文件后重扫，索引必须与磁盘一致；缓存与磁盘不一致时以磁盘为准。

**Validates: Requirements 2.2, 2.3, 7.4**

### Property 2: vault 封闭性
任何写入/删除的解析路径必须落在某个 vault root 内；`..` 穿越或越界绝对路径一律拒绝（与 `WorkspaceRoot` 同款保证）。

**Validates: Requirements 1.6**

### Property 3: 阈值单调性
被动注入只包含 rerank 归一化分数 ≥ `min_score` 的条目；无任何条目达标时注入块为空字符串（绝不注入占位/噪声）。

**Validates: Requirements 3.2, 3.3**

### Property 4: 注入预算上界
被动注入条数 ≤ `max_inject` 且总 token ≤ `max_inject_tokens`；超出时按分数降序保留高分条目。

**Validates: Requirements 3.2, 3.6**

### Property 5: 缓存前缀不破坏
被动注入块只出现在 `SYSTEM_PROMPT_DYNAMIC_BOUNDARY` 之后；静态前缀（含工具 schema）逐字节稳定，保持 DeepSeek prefix 缓存命中。

**Validates: Requirements 3.5**

### Property 6: 精确匹配可命中
对命令名/配置键/报错原文等罕见 token 的查询，BM25 分支保证对应条目进入候选（dense-only 不得漏掉）。

**Validates: Requirements 6.1, 6.2**

### Property 7: 空安全
空 query、空库、无命中三种情况下 `search`/`passive_block`/`knowledge_search` 均返回安全空结果，不 panic、不报错。

**Validates: Requirements 3.3, 4.4**

### Property 8: 去重幂等
用同一 `title` 连续 `write` 两次只产生一份 `.md`，`created_at` 不变、`updated_at` 刷新。

**Validates: Requirements 1.5**

### Property 9: 向后兼容
ChatService 未 `with_knowledge` 时，行为与本特性引入前完全一致（被动注入为空、两个工具不注册），现有测试不回归。

**Validates: Requirements 8.1, 8.5**

## Testing Strategy

后端单元测试（覆盖 Req 8.5 关键路径）：
- `entry`: frontmatter 往返、容错解析（缺 frontmatter/坏 kind）、slug 生成与冲突。
- `vault`: scan 多文件、跳过坏文件、write/delete 往返、路径穿越被拒。
- `KnowledgeBase`: load_all 计数、search 命中相关条目、BM25 精确命中命令/配置键、阈值过滤（低分不注入）、passive_block 空/非空、token 预算截断、write 去重更新。
- `knowledge_tools`: search 命中/空结果、write 返回 id、schema 校验、子代理只读（write 不可用）路径。
- `KnowledgeService`: open + 双 vault 合并、save 新建/更新、delete。
- ChatService：接 KnowledgeService 后被动注入出现在 system prompt 动态段；未接时行为不变（回归保护）。

前端：`pnpm build`（tsc+vite）通过；KnowledgeView 无诊断。

质量门禁（Req 8）：`cargo test --workspace --offline`、`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --offline -- -D warnings`、`pnpm build` 全绿；新 crate 加入 workspace `Cargo.toml`；网络相关（无）默认 offline 可构建。

## 实施阶段（建议）

1. **新 crate `deepagent-knowledge`**：entry + vault + KnowledgeBase + 测试（纯后端、离线）。
2. **工具**：`knowledge_search` / `knowledge_write` + 测试。
3. **app-core `KnowledgeService`** + DTO + 测试。
4. **ChatService 接线**：被动注入 + 工具注册 + 系统提示补强 + 回归测试。
5. **Tauri 命令 + AppState 接线**。
6. **前端 KnowledgeView + Sidebar 入口 + api/types + i18n**。
7. **设置**：被动注入开关 + 阈值/条数（可放设置面板）。

> 范围控制：会话结束“自动总结沉淀”、embedding 磁盘缓存、相似度合并去重、用户级 vault 之外的更多 scope —— 均为后续增强，本设计预留接口但不在首期实现。

## 增补设计：会话自动沉淀（方案 A — 草稿 + 人工确认）

> 对应 Requirement 9。这是 Phase 2 预留的 `capture_from_session` 的落地，采用「轻、可控」方案：只在错误恢复时触发，产出待确认草稿，绝不直接写正式库。

### 数据模型：条目状态

`KnowledgeEntry` 增加状态维度，区分**正式（active）**与**草稿（draft）**：

```rust
// crates/deepagent-knowledge/src/entry.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryStatus { Active, Draft }
```

`KnowledgeEntry` 增加 `status: EntryStatus`（frontmatter `status: draft|active`，缺省 `active` 以兼容既有文件）。

### Vault 隔离：草稿子目录

草稿存到各 vault 的 `.drafts/` 子目录（项目级 `<project>/.deepagent/knowledge/.drafts/`），与正式条目物理隔离。`Vault` 增加 `scan_drafts()` / `write_draft()` / `delete_draft()`，规则与正式区相同（路径封闭、容错扫描）。

**检索隔离（Property 关键）**：`KnowledgeBase::load_all` 只把**正式区**条目载入检索索引；草稿仅载入一个独立的 `drafts: BTreeMap<uid, KnowledgeEntry>`，**不进** `ContextualRetriever`、不进 `passive_block`、不进 `search`。这保证草稿绝不污染被动注入与 `knowledge_search`（Req 9.5/9.6）。

新增 API：
- `KnowledgeBase::add_draft(draft) -> KnowledgeEntry`：写 `.drafts/`，登记到 `drafts` 映射，**不重建索引**。
- `KnowledgeBase::list_drafts() -> Vec<KnowledgeEntry>`。
- `KnowledgeBase::accept_draft(uid) -> KnowledgeEntry`：删草稿文件 + 以正式 `write` 落入正式 vault（进索引），`source_session` 透传（Req 9.8/9.12）。
- `KnowledgeBase::discard_draft(uid) -> bool`：删草稿文件 + 移除映射（Req 9.9）。

### 错误恢复检测：`SessionCapture`

纯函数，无网络、可离线单测。输入一个 run 的事件序列（`&[Event]`），输出是否值得总结 + 一份精炼上下文：

```rust
// crates/deepagent-knowledge/src/capture.rs
pub struct RecoverySignal {
    pub had_failure: bool,        // 至少一次 ToolCallCompleted{ok:false}
    pub completed: bool,          // 任务最终 TaskStateChanged -> Completed
    pub failed_tools: Vec<String>,// 失败过的工具名（去重）
    pub user_goal: String,        // 首个用户消息（任务目标）
    pub final_answer: String,     // 末条 assistant 文本
    pub transcript_digest: String,// 提交给总结模型的精炼材料（限长）
}
pub fn detect_recovery(events: &[Event]) -> RecoverySignal;
pub fn is_worth_capturing(sig: &RecoverySignal) -> bool; // had_failure && completed
```

`transcript_digest` 只取关键骨架（用户目标、失败的工具调用及其错误摘要、最终答案），并按字符上限截断，避免把整段长对话喂给总结模型。

### 总结调用：app-core 的 `KnowledgeAutoCapture`

放在 app-core（需要 `ModelClient`），不污染纯 crate 的离线性：

```rust
// crates/deepagent-app-core/src/knowledge_service.rs（或新模块）
impl KnowledgeService {
    /// 由 ChatService 在 run 完成后异步调用。无 failure / 模型判定不值得 / 调用失败
    /// 一律静默返回 None（不打断主 run）。成功则写入一条草稿并返回。
    pub async fn capture_from_session(
        &self,
        client: Arc<ModelClient>,
        model: String,
        events: &[Event],
        session_id: &str,
    ) -> Option<KnowledgeDto>;
    pub fn list_drafts(&self) -> Vec<KnowledgeDto>;
    pub fn accept_draft(&self, id: &str) -> Result<KnowledgeDto>;
    pub fn discard_draft(&self, id: &str) -> Result<bool>;
    pub fn set_auto_capture(&self, on: bool);
    pub fn auto_capture_enabled(&self) -> bool;
}
```

总结 prompt 要求模型**只输出 JSON**：`{ "worth_saving": bool, "title", "kind", "tags":[], "body" }`。`worth_saving=false` → 不建草稿（Req 9.4）。解析失败 → 静默跳过（容错）。

### ChatService 接线

`run_in_session` 在 `engine.run(...)` 成功返回后、函数返回前：若挂了 knowledge 且 `auto_capture_enabled`，**spawn 一个后台任务**（用 `state.rt` 同款 tokio runtime / `tokio::spawn`）调用 `capture_from_session`，**不 await**，从而不延迟用户看到回答（Req 9.11）。后台任务从该 session 的事件日志读取事件序列。

> 边界：自动总结用 Chat 模型角色；无 API key 时 `build_model` 失败 → 直接不 spawn（Req 9.10）。草稿写入失败仅 `tracing::warn`。

### Tauri 命令 + 前端

新增命令：`kb_list_drafts`、`kb_accept_draft`、`kb_discard_draft`、`kb_set_auto_capture`、`kb_auto_capture_enabled`。前端 `KnowledgeView` 增加「待确认草稿」分区（顶部、醒目标识），每条带「采纳 / 丢弃」按钮；头部增加「自动沉淀」开关（与「自动注入」并列）。提示统一 `message` 组件，保留既有 Tailwind 风格。

### Correctness Properties（增补）

#### Property 10: 草稿检索隔离
草稿条目绝不出现在 `search` / `passive_block` / `knowledge_search` 的结果中；只有 `accept_draft` 后转成的正式条目才可被检索。

**Validates: Requirements 9.5, 9.6**

#### Property 11: 触发保守性
`is_worth_capturing` 当且仅当「至少一次工具失败 且 任务最终完成」为真；无失败的平凡 run 不产生草稿。

**Validates: Requirements 9.1, 9.2**

#### Property 12: 主 run 不受影响
自动沉淀在主 run 完成后异步进行；其失败（无 key、模型错误、解析失败）一律静默，不改变主 run 的返回值与时序。未挂 knowledge 或关闭开关时，行为与本增补引入前完全一致。

**Validates: Requirements 9.10, 9.11**

#### Property 13: 采纳幂等与来源
采纳一条草稿后，草稿区不再有它、正式区恰好多一条（遵循既有同标题去重），且其 `source_session` 等于来源会话 id。

**Validates: Requirements 9.8, 9.12**
