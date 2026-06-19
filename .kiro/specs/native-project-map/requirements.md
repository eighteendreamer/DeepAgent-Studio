# Requirements Document

## Introduction

DeepAgent Studio 需要让 AI **通过结构化的代码图谱来理解项目**，而不是逐个文件地 grep/read。当前的"项目地图"能力存在两个根本问题：

1. **生成依赖外部 Node.js 项目**：`project_map_service.rs` 的 `refresh_deep()` 通过 `node` 子进程调用外部 `understand-anything-plugin`，导致 `ERR_MODULE_NOT_FOUND: zod` 报错，没有那个外部项目就无法生成图谱。
2. **图谱粒度太粗**：仅文件级 imports，缺少符号级调用关系，AI 无法回答"X 如何调用到 Y""改动这个函数影响哪些地方"这类结构/流程问题，只能退回到 grep/read。

本特性用**纯 Rust 原生引擎**复现并融合两套业界方案：

- **Understand-Anything 风格**（给人看）：可视化项目地图面板，展示文件/符号节点、层级、导览。
- **CodeGraph 风格**（给 AI 看）：基于 tree-sitter AST 的精确符号图谱，提供 `explore/callers/callees/impact/node/search` 查询，让 AI 一次拿到答案、零文件读取。

**核心架构——一次提取，两个消费者**：新增 `deepagent-codegraph` crate，用 tree-sitter 提取一次 AST，存入 SQLite（nodes/edges/files + FTS5），再向两个方向输出：
- **AI 路**：`codegraph_*` 工具直接查 SQLite，快且精确。
- **人路**：投影器把富图谱降维成现有 `.understand-anything/knowledge-graph.json`，喂给现有前端面板，零改动。

**目标**：
1. 用 tree-sitter + SQLite 的纯 Rust 引擎彻底替换 Node.js 子进程，修复生成报错。
2. 构建符号级图谱（含调用边），让 AI 能追踪调用链、分析变更影响。
3. 保持现有前端面板与查询 API 完全兼容。
4. 新增 AI 代码图谱查询工具，并引导 agent 优先查图谱而非 grep/read。

**代码归属**：
- 新增 `deepagent-codegraph` crate（提取 / 存储 / 解析 / 查询 / 投影）。
- 改造 `deepagent-app-core::project_map_service`（`refresh_deep()` 改调原生引擎，删除 Node.js 逻辑，保留查询 API）。
- `deepagent-builtins` 新增 `codegraph_*` AI 工具。

**分期**：
- **一期（修复 + 地基）**：新 crate 骨架 + tree-sitter 提取 5 语言（Rust/TS/JS/Python/Go）+ SQLite 存储 + import 解析 + 投影 UA JSON + `refresh_deep` 去 Node.js。
- **二期（AI 杀手锏）**：函数级调用边 + `codegraph_explore/callers/callees/impact/node/search` 工具 + FTS5 搜索 + 系统提示引导。
- **三期（保鲜 + 广度）**：文件 watcher 自动增量同步 + 框架路由识别 + 扩展到 20+ 语言。

**范围外**：
- `deepagent-knowledge` 知识库模块：完全不涉及，两者独立。
- 三期的 watcher、框架路由、20+ 语言不在一期/二期验收范围内。
- 跨语言桥接（Swift↔ObjC、RN bridge 等 CodeGraph 高级特性）：本期不做。

## Glossary

- **CodeGraph_Engine**: 新增 `deepagent-codegraph` crate，原生 Rust 代码图谱引擎，负责 tree-sitter 提取、SQLite 存储、引用解析、图谱查询与 UA JSON 投影。
- **Extractor**: 提取器，用 tree-sitter 把源文件解析为 AST，按语言查询规则提取符号节点和边。
- **Resolver**: 引用解析器，在全量提取后解析 import 目标文件、函数调用到定义、继承/实现关系。
- **Graph_Store**: SQLite 存储层，包含 nodes/edges/files 表和 nodes_fts FTS5 全文索引。
- **Projector**: 投影器，把 SQLite 富图谱降维成现有 `.understand-anything/knowledge-graph.json`（file/symbol 节点 + contains/imports 边 + layers + tour）。
- **Project_Map_Service**: 现有 `project_map_service.rs` 服务层，提供项目地图查询 API。
- **Node**: 图谱节点，含 id、kind、name、qualified_name、file_path、language、start_line、end_line、signature、docstring、visibility 等字段。
- **NodeKind**: 节点类型枚举：file、module、class、struct、interface、trait、function、method、property、field、variable、constant、enum、enum_member、type_alias、namespace、import、route 等。
- **Edge**: 图谱边，含 source、target、kind、metadata、provenance、line 字段。
- **EdgeKind**: 边类型枚举：contains、calls、imports、exports、extends、implements、references、type_of 等。
- **Call_Edge**: `calls` 类型边，表示函数/方法间调用关系，是 callers/callees/impact 查询的基础。
- **AI_Query_Tool**: `deepagent-builtins` 中面向 AI 的只读工具：codegraph_explore / codegraph_callers / codegraph_callees / codegraph_impact / codegraph_node / codegraph_search。
- **Explore**: AI 主力查询，输入符号名集合，一次返回相关符号源码 + 调用路径，按文件分组。
- **Impact_Radius**: 影响半径，从某符号出发追踪所有直接/间接调用者，分析改动影响范围。
- **Incremental_Sync**: 增量同步，通过文件 content_hash 比对检测变更文件，仅重新提取变更文件并合并回图谱。
- **Layer**: 逻辑架构层，按目录路径模式将文件节点分组（API/Service/Data/UI/Utility/Test/Config/Core）。
- **Tour**: 图谱导览序列，按拓扑排序或层级分组，帮助按逻辑顺序理解代码库。
- **Change_Marker**: 变更标记，通过 git diff 检测变更文件，在对应节点打 `changed` 标签。
- **UA_JSON**: `.understand-anything/knowledge-graph.json`，现有前端面板消费的图谱格式。

## Requirements

### Requirement 1: 文件扫描与过滤

**User Story:** 作为开发者，我希望引擎能递归扫描项目目录并智能过滤非源码文件，以便只提取有意义的源代码。

#### Acceptance Criteria

1. WHEN 启动项目扫描时, THE CodeGraph_Engine SHALL 递归扫描项目根目录下的所有子目录。
2. WHILE 扫描目录时, THE CodeGraph_Engine SHALL 跳过 `.git`、`node_modules`、`target`、`dist`、`build`、`__pycache__`、`.venv` 等已知非源码目录。
3. WHEN 遇到二进制文件（图片、字体、压缩包、可执行文件）时, THE CodeGraph_Engine SHALL 跳过该文件。
4. WHEN 遇到文件大小超过 1.5MB 的文件时, THE CodeGraph_Engine SHALL 跳过该文件。
5. WHILE 扫描目录时, THE CodeGraph_Engine SHALL 遵守项目根目录的 `.gitignore` 规则。
6. WHEN 扫描到源文件时, THE CodeGraph_Engine SHALL 根据文件扩展名识别其编程语言；IF 语言不在当前支持列表内, THEN THE CodeGraph_Engine SHALL 仅将其登记为 file 节点而不提取符号。

### Requirement 2: tree-sitter 多语言提取

**User Story:** 作为开发者，我希望引擎用 tree-sitter 精确解析源代码 AST，以便提取可靠的符号和关系，而非脆弱的正则匹配。

#### Acceptance Criteria

1. THE CodeGraph_Engine SHALL 使用 tree-sitter 及其各语言 grammar 解析源文件为 AST，纯 Rust 编译集成，不依赖 Node.js 或运行时下载 grammar。
2. WHEN 一期实现时, THE CodeGraph_Engine SHALL 支持 Rust、TypeScript、JavaScript、Python、Go 五种语言的 AST 提取。
3. WHEN 解析代码文件时, THE CodeGraph_Engine SHALL 提取函数/方法、类/结构体/接口/trait、模块/命名空间、枚举、类型别名、常量/变量等符号节点。
4. WHEN 提取符号时, THE CodeGraph_Engine SHALL 记录每个节点的 name、qualified_name、file_path、language、start_line、end_line、signature、docstring（如有）、visibility（如有）。
5. WHEN 某文件解析失败（语法错误或 grammar 异常）时, THE CodeGraph_Engine SHALL 跳过该文件、记录警告并继续处理其余文件，不中断整体提取。
6. THE CodeGraph_Engine SHALL 使用稳定的 id 生成方案，确保同一符号在多次提取间 id 一致。

### Requirement 3: 符号级图谱构建（含调用边）

**User Story:** 作为 AI，我希望图谱包含符号间的调用、包含、继承关系，以便追踪完整的代码流程而无需读源文件。

#### Acceptance Criteria

1. WHEN 文件包含符号定义时, THE CodeGraph_Engine SHALL 生成 `contains` 边（文件/容器 → 子符号）。
2. WHEN 函数/方法内出现对其他函数/方法的调用时, THE CodeGraph_Engine SHALL 生成 `calls` 边（调用者 → 被调用者）。
3. WHEN 文件包含 import/use/require 声明时, THE CodeGraph_Engine SHALL 生成 `imports` 边。
4. WHEN 类/结构体继承或实现其他类型时, THE CodeGraph_Engine SHALL 生成 `extends` 或 `implements` 边。
5. THE CodeGraph_Engine SHALL 为每条边记录 source、target、kind，并在边由启发式推断而非精确解析得出时记录 `provenance` 标记。
6. WHEN 调用目标无法在当前文件内解析时, THE CodeGraph_Engine SHALL 将其登记为未解析引用，留待 Resolver 在全量提取后解析。

### Requirement 4: 引用解析

**User Story:** 作为 AI，我希望 import 和函数调用能解析到真实的定义节点，以便调用链跨文件连通。

#### Acceptance Criteria

1. WHEN 全量提取完成后, THE Resolver SHALL 解析 import 声明到目标文件（处理相对路径、包路径、tsconfig/cargo workspace 别名）。
2. WHEN 存在未解析的函数调用引用时, THE Resolver SHALL 通过名称匹配和限定名匹配将其解析到候选定义节点并生成 `calls` 边。
3. WHEN 一个引用匹配到多个候选定义时, THE Resolver SHALL 保留候选集合并按限定名/同文件/同模块优先级排序。
4. WHEN 引用无法解析到任何定义时, THE Resolver SHALL 保留该未解析引用记录而不生成错误边。

### Requirement 5: SQLite + FTS5 存储与增量同步

**User Story:** 作为开发者，我希望图谱存储在本地 SQLite 中并支持增量更新，以便大项目不爆内存且重新分析快速。

#### Acceptance Criteria

1. THE Graph_Store SHALL 使用 SQLite（复用工作区 rusqlite bundled 依赖）存储 nodes、edges、files 三张核心表及 schema 版本表。
2. THE Graph_Store SHALL 创建 nodes_fts FTS5 虚拟表，索引节点的 name、qualified_name、docstring、signature，并通过触发器与 nodes 表保持同步。
3. THE Graph_Store SHALL 在 files 表中记录每个文件的 content_hash、language、size、modified_at、indexed_at。
4. WHEN 执行增量同步时, THE CodeGraph_Engine SHALL 通过比对 content_hash 检测新增/修改/删除的文件，仅重新提取变更文件。
5. WHEN 重新提取某文件时, THE CodeGraph_Engine SHALL 删除该文件的旧节点和相关边（级联删除），再插入新提取结果。
6. THE Graph_Store SHALL 将图谱数据库存放于项目本地目录（如 `.codegraph/` 或 `.understand-anything/`），并加入 `.gitignore` 忽略建议。

### Requirement 6: AI 代码图谱查询工具

**User Story:** 作为 AI，我希望有专门的工具一次性获得结构化答案，以便用极少的工具调用理解代码而不退回 grep/read。

#### Acceptance Criteria

1. THE deepagent-builtins SHALL 提供 `codegraph_search` 工具，按符号名/限定名/docstring 全文检索节点，返回匹配节点及其位置。
2. THE deepagent-builtins SHALL 提供 `codegraph_explore` 工具，输入符号名集合，一次返回相关符号的源码 + 它们之间的调用路径，按文件分组。
3. THE deepagent-builtins SHALL 提供 `codegraph_callers` 工具，返回指定符号的所有调用点。
4. THE deepagent-builtins SHALL 提供 `codegraph_callees` 工具，返回指定符号调用的所有符号。
5. THE deepagent-builtins SHALL 提供 `codegraph_impact` 工具，返回改动指定符号的直接和间接影响半径。
6. THE deepagent-builtins SHALL 提供 `codegraph_node` 工具，返回单个符号的完整源码 + 调用者/被调用者轨迹，或按文件路径读取文件内容。
7. THE AI_Query_Tool SHALL 全部为只读、Safe 风险等级、只需读权限，并通过 backend trait 接到 host 服务（沿用现有 ProjectMapBackend 模式）。
8. WHEN 项目尚未建立图谱索引时, THE AI_Query_Tool SHALL 返回成功形态的提示信息（而非 error），引导而不阻断。

### Requirement 7: UA JSON 投影与前端兼容

**User Story:** 作为开发者，我希望现有项目地图面板无需修改即可展示新引擎生成的图谱，以便保持可视化体验。

#### Acceptance Criteria

1. WHEN 图谱构建完成后, THE Projector SHALL 把 SQLite 富图谱降维投影为 `.understand-anything/knowledge-graph.json`，格式与现有前端完全兼容。
2. THE Projector SHALL 在投影中包含 file 节点和符号节点，contains/imports 边，以及 project 元数据（name、description、languages、frameworks、analyzedAt、gitCommitHash）。
3. THE Projector SHALL 确保所有节点 id 唯一，边的 source/target 引用有效节点 id，文件路径使用 POSIX 风格相对路径。
4. THE Projector SHALL 在投影 JSON 中包含顶层 `layers` 数组、`tour` 数组和 `version` 字段（值为 `"1.0.0"`）。
5. WHEN 富图谱包含 calls/extends 等 UA 格式不支持的边时, THE Projector SHALL 将其降维或省略，确保投影结果符合现有前端 schema。

### Requirement 8: 层级检测与导览生成

**User Story:** 作为开发者，我希望图谱按架构层次分组并提供逻辑阅读顺序，以便直观理解项目结构。

#### Acceptance Criteria

1. WHEN 投影图谱时, THE Projector SHALL 根据文件路径目录名启发式地将 file 节点分配到层级。
2. THE Projector SHALL 识别层级模式：`routes/controller/handler/endpoint/api`→API层；`service/usecase/business`→Service层；`model/entity/schema/db/repository/migration`→Data层；`component/view/page/screen/ui/widget`→UI层；`util/helper/lib/common/shared`→Utility层；`test/spec/__tests__`→Test层；`config/setting/env`→Configuration层。
3. WHEN 文件路径不匹配任何层级模式时, THE Projector SHALL 将该节点归入 `Core` 层。
4. THE Projector SHALL 为每个 Layer 输出 id（kebab-case）、name、description、nodeIds 数组。
5. WHEN 生成导览时, THE Projector SHALL 基于拓扑排序生成启发式导览（不依赖 LLM）；IF 存在 Layer 信息, THEN 按层级分组生成 TourStep；ELSE 每 3 个节点为一组。
6. THE Projector SHALL 为每个 TourStep 输出 order（从 1 开始）、title、description、nodeIds 数组。

### Requirement 9: 变更高亮

**User Story:** 作为开发者，我希望重新分析时高亮显示变更文件，以便快速定位改动影响范围。

#### Acceptance Criteria

1. WHEN 执行增量分析时, THE CodeGraph_Engine SHALL 通过 `git diff` 获取自上次分析（gitCommitHash）以来变更的文件列表。
2. WHEN 检测到文件变更时, THE Projector SHALL 在对应节点的 `tags` 字段追加 `changed` 标签。
3. WHEN 无法执行 git diff（非 git 仓库或首次分析）时, THE CodeGraph_Engine SHALL 执行全量分析，不标记任何变更节点。
4. WHEN 分析完成时, THE CodeGraph_Engine SHALL 更新图谱中的 gitCommitHash 为当前 HEAD commit hash。

### Requirement 10: 集成替换（去 Node.js）

**User Story:** 作为开发者，我希望用原生 Rust 引擎无缝替换 Node.js 子进程调用，以便消除外部依赖并修复生成报错。

#### Acceptance Criteria

1. WHEN 调用 `refresh_deep()` 时, THE Project_Map_Service SHALL 使用本地 CodeGraph_Engine 执行提取、存储与投影，不再调用外部 Node 进程。
2. THE Project_Map_Service SHALL 删除 `locate_understand_plugin_root()`、`ensure_understand_core_built()`、`pnpm_command()` 等 Node.js 相关函数。
3. THE Project_Map_Service SHALL 删除 `understand-deep-map.mjs` 桥接脚本及所有硬编码的外部项目路径。
4. THE Project_Map_Service SHALL 保持所有现有公开查询 API 不变（status/overview/search/node/neighbors/graph/impact/refresh_deep）。
5. WHEN 没有任何外部 Node.js 项目存在时, THE Project_Map_Service SHALL 仍能成功生成项目地图（修复 ERR_MODULE_NOT_FOUND 报错）。

### Requirement 11: 系统提示引导

**User Story:** 作为 AI，我希望被引导在面对结构/流程问题时优先使用代码图谱工具，以便减少 grep/read 的工具调用和 token 消耗。

#### Acceptance Criteria

1. WHEN 构建系统提示时, THE 系统 SHALL 在工具使用指引中加入：遇到"X 如何工作""X 如何调用到 Y""改动 X 影响什么"等结构/流程问题时，优先使用 codegraph_explore/callers/callees/impact，而非 grep/read。
2. THE 系统提示 SHALL 引导 agent 把 codegraph 工具返回的源码视为"已读取"，不重复 grep 验证。
3. WHEN 子代理（subagent）注册工具时, THE 系统 SHALL 同样为其提供只读的 codegraph 查询工具。

### Requirement 12: 性能要求

**User Story:** 作为开发者，我希望分析过程快速且不阻塞 UI，以便获得流畅体验。

#### Acceptance Criteria

1. WHEN 分析中型项目（约 1000 个源文件）时, THE CodeGraph_Engine SHALL 在 30 秒以内完成全量提取与投影。
2. WHILE 执行分析过程时, THE CodeGraph_Engine SHALL 异步执行不阻塞 UI 线程，并尽可能并行解析多个文件。
3. WHEN 执行增量同步时, THE CodeGraph_Engine SHALL 仅重新提取变更文件，耗时显著低于全量分析。
4. WHEN AI 查询工具被调用时, THE Graph_Store SHALL 通过 SQLite 索引和 FTS5 在亚秒级返回结果。

### Requirement 13: 非功能约束

**User Story:** 作为开发者，我希望实现遵循工程质量标准且依赖可控，以便代码可维护、可分期交付。

#### Acceptance Criteria

1. THE CodeGraph_Engine SHALL 作为独立 crate `deepagent-codegraph` 实现，依赖边界清晰（提取/存储/解析/查询/投影分模块）。
2. THE CodeGraph_Engine SHALL 仅引入纯 Rust 可编译的 tree-sitter grammar crate，不依赖外部进程、不在运行时下载 grammar。
3. THE CodeGraph_Engine SHALL 通过 `cargo clippy -D warnings` 编译检查无警告。
4. THE CodeGraph_Engine SHALL 为核心逻辑（扫描、提取、图构建、引用解析、存储、增量同步、投影、查询）提供单元测试覆盖。
5. THE 实现 SHALL 按一期（提取+存储+import解析+投影+去Node.js）、二期（调用边+AI查询工具+FTS5+系统提示）、三期（watcher+框架路由+扩展语言）分期交付，一期与二期为本 spec 的主要验收范围。

### Requirement 14: codegraph 优先读取引导（软引导）

**User Story:** 作为 AI，我希望在理解代码时被引导优先使用 codegraph 查询工具而非裸读文件，以便用更少的工具调用获得更精确的代码理解，同时在 codegraph 不适用时仍能裸读兜底。

#### Acceptance Criteria

1. WHEN 构建系统提示时, THE 系统 SHALL 引导：理解代码（某符号如何工作、调用链、影响面、定位符号）时优先使用 codegraph_explore/node/search/callers/callees/impact，而非 read_file/grep。
2. THE read_file 工具描述 SHALL 注明：为理解代码逻辑而读取时优先用 codegraph_node/explore；read_file 用于非代码文件、编辑前置精确读、以及 codegraph 不适用时的兜底。
3. THE grep 工具描述 SHALL 注明：搜索代码符号优先用 codegraph_search；grep 用于搜索字符串字面量/注释/配置等非符号文本。
4. WHEN 项目已建立 codegraph 索引 AND AI 即将裸读代码文件以理解逻辑时, THE 系统 SHALL 通过提示与工具描述引导其先尝试 codegraph，但 SHALL NOT 阻断裸读（软引导，保留兜底）。
5. THE 系统 SHALL 在以下场景始终允许裸读且不强行引导走 codegraph：非代码文件、未索引项目、解析失败文件、不支持的语言、编辑类工具（edit_file/multi_edit/write_file）的前置 read_file。
6. THE 系统 SHALL 引导 AI 把 codegraph 返回的源码视为"已读取"，不重复 grep/read 验证。

### Requirement 15: 报错信息/截图 → 代码定位

**User Story:** 作为开发者，我希望粘贴报错信息或截图后，系统能自动定位到出问题的文件和具体位置（符号/行）并取出相关上下文，以便快速诊断，而无需我手动指出位置。

#### Acceptance Criteria

1. WHEN 用户粘贴包含堆栈轨迹或报错的文本时, THE 系统 SHALL 解析其中的文件路径、行号、列号、符号名和错误码。
2. WHEN 用户粘贴截图（图片）时, THE 系统 SHALL 由多模态模型从图像提取报错文本，再按文本路径处理，不新增 OCR 依赖。
3. WHEN 解析出"文件:行号"引用且该文件在 codegraph 索引中时, THE 系统 SHALL 通过 codegraph 定位到 [start_line, end_line] 包含该行的符号节点，并返回其源码及调用者/被调用者。
4. WHEN 解析出符号名但缺少精确"文件:行号"时, THE 系统 SHALL 通过 codegraph_search 定位候选定义节点。
5. WHEN 堆栈帧引用的文件不在项目图谱中（外部依赖或运行时内部路径）时, THE 系统 SHALL 将其标记为外部帧、聚焦于项目内帧，不将外部路径误报为项目代码位置。
6. THE 错误解析 SHALL 支持常见格式：Rust panic/backtrace、Node.js 错误栈、Python traceback、Java 异常栈、Go panic。
7. WHEN 无法解析出任何可定位引用时, THE 系统 SHALL 优雅降级（提示补充信息或退回关键词搜索），不报错。
