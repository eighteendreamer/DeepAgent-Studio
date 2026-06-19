# Implementation Plan

## Overview

本计划将 CodeGraph 双引擎设计落实为可增量执行、每步可编译/可测试的编码任务。实施策略按三期推进：

- **一期（任务 1-13）**：搭建 `deepagent-codegraph` crate，实现 tree-sitter 提取（5 语言）、SQLite 存储、import 解析、UA JSON 投影，并把 `ProjectMapService::refresh_deep()` 从 Node.js 子进程切换到原生引擎——**修复前端报错，恢复项目地图面板**。
- **二期（任务 14-22）**：实现跨文件调用边解析、QueryManager（explore/callers/callees/impact/node/search）、FTS5 搜索、6 个 AI 工具、系统提示引导——**让 AI 用图谱替代 grep/read**。
- **三期（任务 23-26）**：文件 watcher 自动增量同步、框架路由识别、扩展语言——**保鲜与广度**。

每步保持 `cargo test --workspace` 全绿、`cargo clippy -D warnings` 无警告。复用工作区现有的 `rusqlite 0.31 bundled` 依赖（启用 fts5 特性），新增 tree-sitter grammar 依赖。

## Tasks

### 一期：提取 + 存储 + 投影（修复报错，地基）

- [x] 1. 创建 `deepagent-codegraph` crate 骨架并接入 workspace
  - 在 `crates/deepagent-codegraph/` 创建 `Cargo.toml`，依赖 `rusqlite`（启用 `fts5`）、`tree-sitter`、`tree-sitter-rust`、`tree-sitter-typescript`、`tree-sitter-javascript`、`tree-sitter-python`、`tree-sitter-go`、`ignore`、`serde`、`serde_json`、`thiserror`、`tracing`、`deepagent-core`
  - 在根 `Cargo.toml` 的 `[workspace] members` 加入新 crate，在 `[workspace.dependencies]` 登记 tree-sitter 相关版本
  - 创建 `src/lib.rs` 暴露 `CodeGraph` 门面占位和子模块 `pub mod`（types/scanner/extraction/resolution/store/query/projection）
  - 验证：`cargo build -p deepagent-codegraph --offline` 通过
  - _Requirements: 13.1, 13.2_

- [x] 2. 实现核心数据类型 `types.rs`
  - 实现 `NodeKind` 枚举（File/Module/Class/Struct/Interface/Trait/Function/Method/Property/Field/Variable/Constant/Enum/EnumMember/TypeAlias/Namespace/Import/Route）含 `as_str()`/`parse()` 容错
  - 实现 `EdgeKind` 枚举（Contains/Calls/Imports/Exports/Extends/Implements/References/TypeOf）
  - 实现 `Language` 枚举（Rust/TypeScript/JavaScript/Python/Go/Other）含 `from_extension`/`from_path`
  - 实现 `Node`、`Edge`、`FileRecord`、`UnresolvedRef` 结构体
  - 单元测试：枚举往返、扩展名映射、无扩展名文件（Dockerfile/Makefile）识别、未知扩展名归 Other
  - _Requirements: 2.2, 3.5, 13.1_

- [x] 3. 实现 `FileScanner`（扫描 + 过滤 + 语言识别 + hash）
  - 在 `scanner.rs` 实现 `ScannedFile` 结构（path/relative_path/language/size/content_hash）
  - 实现 `FileScanner::new`（解析 .gitignore，用 `ignore` crate）和 `scan`
  - 跳过 `.git`/`node_modules`/`target`/`dist`/`build`/`__pycache__`/`.venv`；过滤二进制扩展名；过滤 >1.5MB；遵守 .gitignore
  - 计算 content_hash（用 blake3 或 xxhash，加依赖）
  - 单元测试（用 tempfile）：跳过指定目录、过滤二进制/大文件、遵守 gitignore、content_hash 稳定
  - _Requirements: 1.1, 1.2, 1.3, 1.4, 1.5, 1.6_

- [x] 4. 实现 GraphStore schema 与建表迁移
  - 在 `store/schema.rs` 用 rusqlite 创建 nodes/edges/files/unresolved_refs/schema_versions/project_metadata 表
  - 创建 nodes_fts FTS5 虚拟表 + nodes_ai/nodes_ad/nodes_au 同步触发器
  - 创建性能索引（idx_nodes_kind/name/file_path、idx_edges_source_kind/target_kind、idx_unresolved_name）
  - 实现 schema 版本检查与迁移框架
  - 单元测试：建表成功、FTS5 表可用、触发器存在、重复 open 幂等
  - _Requirements: 5.1, 5.2, 13.1_

- [x] 5. 实现 GraphStore CRUD 与增量比对
  - 在 `store/mod.rs` 实现 `GraphStore::open`（建表+迁移）
  - 实现 `upsert_file`/`insert_nodes`/`insert_edges`（批量，事务包裹）
  - 实现 `delete_file_cascade`（删文件的节点，外键级联删边）
  - 实现 `changed_files`（content_hash 比对，返回 added/modified/deleted）
  - 实现读取方法：`node_by_id`/`edges_from`/`edges_to`/`all_file_nodes`/项目元数据读写
  - 单元测试：插入/查询往返、级联删除一致性、content_hash 增量检测、批量写入事务
  - _Requirements: 5.3, 5.4, 5.5_

- [x] 6. 实现 tree-sitter 提取框架与 Language grammar 注册
  - 在 `extraction/language.rs` 实现 `Language::ts_language()` 返回各语言 tree-sitter Language
  - 在 `extraction/mod.rs` 实现 `Extractor` 结构、`ExtractedFile` 结构、`ExtractorImpl` trait
  - 实现 tree-sitter Parser 按语言缓存复用
  - 实现稳定 id 生成方案：`{kind}:{file}:{qualified_name}:{start_line}`
  - 实现解析失败容错（跳过 + warn，不中断）
  - 单元测试：parser 可解析、id 生成稳定、坏文件不 panic
  - _Requirements: 2.1, 2.5, 2.6_

- [x] 7. 实现 Rust 语言提取器
  - 在 `extraction/rust.rs` 编写 tree-sitter Query 提取：fn/方法、struct/enum/trait、mod、const/static、use 声明、impl 块内函数调用
  - 提取节点的 name/qualified_name/signature/visibility/start_line/end_line/docstring
  - 生成 contains 边（文件→符号、impl→方法）、同文件 calls 边、imports 边、impl→trait 的 implements 边
  - 文件内无法解析的调用 → 生成 UnresolvedRef
  - 单元测试：用 Rust 代码样本验证各类节点和边提取正确
  - _Requirements: 2.3, 2.4, 3.1, 3.2, 3.3, 3.4, 3.6_

- [x] 8. 实现 TypeScript/JavaScript 提取器
  - 在 `extraction/typescript.rs` 编写 TS/JS 的 tree-sitter Query：function/箭头函数/方法、class/interface/type、import/require、调用表达式
  - 生成 contains/calls/imports/extends/implements 边
  - 处理 ES6 import 与 CommonJS require
  - 单元测试：TS 和 JS 代码样本验证节点/边提取
  - _Requirements: 2.3, 2.4, 3.1, 3.2, 3.3, 3.4, 3.6_

- [x] 9. 实现 Python 与 Go 提取器
  - 在 `extraction/python.rs` 提取 def/async def、class、import/from import、调用；继承关系生成 extends 边
  - 在 `extraction/go.rs` 提取 func/method、struct/interface、import、调用；interface 实现生成 implements 边
  - 单元测试：Python 和 Go 代码样本各验证节点/边提取
  - _Requirements: 2.3, 2.4, 3.1, 3.2, 3.3, 3.4, 3.6_

- [x] 10. 实现 import 解析器（一期跨文件 imports）
  - 在 `resolution/import_resolver.rs` 实现 import 路径解析
  - 处理相对路径（./、../）拼接解析到目标文件节点
  - 处理包/模块路径（Rust crate::/super::、JS @scope/pkg）
  - 处理 tsconfig paths 别名、cargo workspace member（读 Cargo.toml）
  - 生成 imports 边（文件→文件）
  - 单元测试：相对路径、包路径、别名解析各验证
  - _Requirements: 4.1_

- [x] 11. 实现 Projector 层级分类与导览生成
  - 在 `projection/layers.rs` 实现路径启发式分层（API/Service/Data/UI/Utility/Test/Configuration/Core）
  - 在 `projection/tour.rs` 实现拓扑排序导览（有层级按层分组，无层级每3节点一组），TourStep 含 order/title/description/nodeIds
  - 单元测试：路径分类正确、Core 兜底、layer id kebab-case、tour order 连续
  - _Requirements: 8.1, 8.2, 8.3, 8.4, 8.5, 8.6_

- [x] 12. 实现 Projector 主体（富图谱 → UA JSON）
  - 在 `projection/mod.rs` 实现 `Projector::project`
  - 节点降维：NodeKind → UA type；复杂度按行数（≥500 complex/≥150 moderate/else simple）
  - 边降维：contains→contains、imports→imports，calls/extends/implements 降为 related 或省略
  - 扫描 manifest（package.json/Cargo.toml/pyproject.toml/go.mod）提取 name/description/languages/frameworks
  - 调 layers/tour，组装含 version="1.0.0"/project/nodes/edges/layers/tour 的 JSON，写 `.understand-anything/knowledge-graph.json`
  - 单元测试：投影 JSON 符合前端 schema、节点 id 唯一、边引用有效、POSIX 路径、降维正确
  - _Requirements: 7.1, 7.2, 7.3, 7.4, 7.5_

- [x] 13. 实现 CodeGraph 门面 + IndexOrchestrator + 集成到 ProjectMapService
  - 在 `lib.rs` 实现 `CodeGraph::open`/`index_all`/`sync`/`project_ua_json`/`has_existing_index`
  - `index_all`：FileScanner → 并行 Extractor → GraphStore 批量写 → import Resolver → 返回 IndexStats
  - `sync`：changed_files → 删旧节点边 → 重新提取变更文件 → 局部 import 解析
  - 变更高亮：git diff 获取变更文件，标记（一期可先在 sync 中标 changed tag）
  - 改造 `deepagent-app-core::project_map_service::refresh_deep`：改调 CodeGraph，删除 `locate_understand_plugin_root`/`ensure_understand_core_built`/`pnpm_command`/Node 子进程逻辑/`understand-deep-map.mjs`/硬编码路径
  - 保持 status/overview/search/node/neighbors/graph/impact 不变
  - 集成测试：小型多语言样本项目全量索引→投影→前端格式校验；refresh_deep 不再依赖 Node.js
  - _Requirements: 2.1, 5.4, 9.1, 9.2, 9.3, 9.4, 10.1, 10.2, 10.3, 10.4, 10.5, 12.1, 12.2_

### 二期：调用图 + AI 查询工具（杀手锏）

- [x] 14. 实现调用引用名称匹配解析器（跨文件 calls）
  - 在 `resolution/name_matcher.rs` 实现未解析调用引用 → 定义节点匹配
  - 按 name/qualified_name 查候选，多候选按（同文件 > 同模块 > 限定名匹配）排序
  - 生成 calls 边，启发式得出的标 provenance="heuristic"
  - 无法解析的保留 unresolved_ref，不生成错误边
  - 实现 `Resolver::resolve_all` 和增量 `resolve_for_files`
  - 单元测试：单候选解析、多候选排序、跨文件调用连通、无候选不报错
  - _Requirements: 4.2, 4.3, 4.4, 3.2_

- [x] 15. 实现图遍历器 Traverser
  - 在 `query/traverser.rs` 实现基于 edges 表的 BFS/DFS
  - 实现反向遍历（callers：沿 calls 边找 source）
  - 实现正向遍历（callees：沿 calls 边找 target）
  - 实现 impact：反向 BFS 收集直接/间接调用者，按深度分层，深度可配
  - 单元测试：调用链遍历、impact 深度分层、环检测不死循环
  - _Requirements: 6.3, 6.4, 6.5_

- [x] 16. 实现 FTS5 搜索与 explore 算法
  - 在 GraphStore 实现 `search_fts`（FTS5 查询 name/qualified_name/docstring/signature，可选 kind 过滤）
  - 在 `query/explore.rs` 实现 explore：符号集→FTS5/精确定位→calls 子图找调用路径（≤1 未命名桥接）→取源码→按文件分组→输出 Flow + FileGroup
  - 实现 explore 输出预算随 file count 缩放
  - 单元测试：FTS5 检索命中、explore 连通命名符号、按文件分组、预算缩放
  - _Requirements: 6.1, 6.2, 12.4_

- [x] 17b. 实现 GraphStore 按位置查询
  - 实现 `node_at_location(file_path, line) -> Option<Node>`：按 file_path 过滤，返回 start_line<=line<=end_line 且行范围最小（最内层）的节点
  - 单元测试：行落在函数体内命中函数节点、嵌套取最内层、行不在任何符号返回 None 或 file 节点
  - _Requirements: 15.3_

- [x] 19c. 实现 ErrorParser（报错文本 → 栈帧引用）
  - 在 `deepagent-codegraph/src/error_locator.rs` 实现 `Frame { file, line, col, symbol, error_code }` 和 `parse(text) -> Vec<Frame>`
  - 支持 Rust panic/backtrace、Node.js 错误栈、Python traceback、Java 异常栈、Go panic 格式
  - 实现项目帧/外部帧区分（按文件是否在项目根内/files 表中）
  - 单元测试：各语言栈样本解析正确、提取 file:line:col/symbol/error_code、外部帧识别、无引用时返回空
  - _Requirements: 15.1, 15.5, 15.6, 15.7_

- [x] 19d. 实现 codegraph_locate 工具
  - 在 `deepagent-builtins/src/codegraph_tools.rs` 新增 `CodeGraphLocateTool`：输入 file:line 或原始报错文本块
  - 内部走 ErrorParser → 过滤项目帧 → 对每帧调 backend.node_at_location → 返回符号源码 + callers/callees + imports；外部帧单独标记
  - 在 CodeGraphBackend trait 增加 locate/node_at_location 方法，host 端桥接实现
  - 无法解析时返回成功形态引导（非 error）
  - 单元测试（stub backend）：file:line 定位、原始栈解析定位、外部帧标记、无引用降级
  - _Requirements: 15.1, 15.3, 15.4, 15.5, 15.7_


- [x] 18. 在 CodeGraph 门面暴露查询 API
  - 在 `lib.rs` 暴露 `search`/`explore`/`callers`/`callees`/`impact`/`node`，委托 QueryManager
  - 定义查询结果类型（NodeHit/ExploreResult/FlowHop/FileGroup/SymbolSource/ImpactResult/CallSite/NodeDetail）
  - 二期在 index_all/sync 中加入 name_matcher 调用解析阶段
  - 集成测试：样本项目索引后 explore 一次连通完整调用链、impact 找到所有调用者
  - _Requirements: 6.1, 6.2, 6.3, 6.4, 6.5, 6.6_

- [x] 19. 定义 CodeGraphBackend trait 与 6 个 AI 工具
  - 在 `deepagent-builtins/src/codegraph_tools.rs` 定义 `CodeGraphBackend` async trait（search/explore/callers/callees/impact/node）
  - 实现 6 个工具：CodeGraphSearchTool/ExploreTool/CallersTool/CalleesTool/ImpactTool/NodeTool
  - 全部 RiskLevel::Safe + PermissionSet::read_only()，定义 JSON schema 参数
  - 未索引时返回 ToolOutput::success 带引导文本（非 error）
  - 在 deepagent-builtins lib 导出新模块
  - 单元测试（stub backend）：各工具 schema 校验、调用返回、未索引引导、风险等级正确
  - _Requirements: 6.7, 6.8_

- [x] 20. 实现 host 端 CodeGraphBackend 桥接
  - 在 `deepagent-app-core` 实现把 CodeGraph（或其服务封装）适配成 `CodeGraphBackend`
  - 处理图谱未打开/未索引的情况，返回引导而非错误
  - 单元测试：桥接各方法映射正确、未索引降级
  - _Requirements: 6.7, 6.8_

- [x] 21. 注册 AI 工具并补强系统提示（codegraph 优先引导）
  - 在主 registry 注册 6 个 codegraph 工具（真实 backend）
  - 子代理 registry 同样注册只读 codegraph 查询工具
  - 在系统提示工具指引段补充：理解代码（X 如何工作/调用链/影响面）优先用 codegraph_explore/node/search/callers/callees/impact 而非 read_file/grep；把返回源码视为已读取，不重复验证
  - _Requirements: 11.1, 11.2, 11.3, 14.1, 14.6_

- [x] 21b. read_file/grep 工具描述补充 codegraph 优先引导（软引导）
  - 更新 `read_file` 工具 description：为理解代码逻辑而读取时优先 codegraph_node/explore；read_file 留作非代码文件、编辑前置读、codegraph 不适用兜底
  - 更新 `grep` 工具 description：搜代码符号优先 codegraph_search；grep 留作字符串/注释/配置等非符号文本
  - 确认不改动 read_file/grep/glob/list_dir 的功能实现与编辑不变量（纯引导层）
  - 验证兜底场景（非代码/未索引/编辑前置读）裸读仍正常工作
  - 单元测试：工具描述含引导文案；裸读功能与编辑链路回归不变
  - 补充引导：用户粘贴报错信息或截图时，先用 codegraph_locate 抽取栈帧并定位项目内出问题的符号，而非 grep 全仓
  - _Requirements: 14.2, 14.3, 14.4, 14.5_, 15.2_


- [x] 22. 二期性能优化与质量门禁
  - Extractor 并行解析（rayon/spawn_blocking），批量写入事务化
  - grammar parser 与 query 编译缓存复用
  - 验证 ~1000 文件项目全量索引 < 30 秒、增量显著更快、AI 查询亚秒级
  - 运行 `cargo fmt --all -- --check`、`cargo clippy --workspace -D warnings`、`cargo test --workspace`
  - _Requirements: 12.1, 12.2, 12.3, 12.4, 13.3, 13.4_

### 三期：保鲜 + 广度（可选增强）

- [x] 23. 实现文件 watcher 自动增量同步
  - 用 notify crate 监听文件变更（FSEvents/inotify/ReadDirectoryChangesW）
  - 防抖（2 秒静默窗口）后触发增量 sync
  - 过滤仅源文件，忽略非源码目录
  - 单元测试：变更触发 sync、防抖合并、忽略过滤
  - _Requirements: 13.5_

- [x] 24. 实现框架路由识别
  - 识别常见 Web 框架路由文件（Axum/Actix/Express/FastAPI/Django 等）
  - 生成 route 节点 + references 边链接到 handler
  - 单元测试：各框架路由样本生成 route 节点
  - _Requirements: 13.5_

- [x] 25. 扩展语言支持到 20+
  - 新增 Java/C#/C++/Ruby/PHP/Swift/Kotlin 等 tree-sitter grammar 与提取器
  - 每语言至少支持函数/类/import/调用提取
  - 单元测试：每新增语言基础提取验证
  - _Requirements: 13.5_

- [x] 26. 三期全量回归与文档
  - 运行完整质量门禁（fmt/clippy/test）
  - 为 deepagent-codegraph 补 crate 级与模块级文档
  - 更新相关 README，说明双引擎架构与 AI 工具用法
  - _Requirements: 13.3, 13.4, 13.5_

## 依赖关系

```
一期: 1 → 2 → 3,4 → 5 → 6 → 7,8,9 → 10 → 11 → 12 → 13 (集成,修复报错)
二期: 14 → 15 → 16 → 17 → 18 → 19,20 → 21 → 22
三期: 23 / 24 / 25 (并行) → 26
```

**关键路径**：
- 任务 1-2 是地基（crate + 类型），必须最先
- 任务 4-5（存储）与 6-9（提取）是一期核心，提取依赖类型
- 任务 13 是一期里程碑：集成后前端报错修复
- 任务 14（跨文件 calls）是二期地基，explore/impact 都依赖它
- 任务 19-21 是二期里程碑：AI 工具上线 + 引导

## 验收标准

**一期**：refresh_deep 不再依赖 Node.js，前端面板正常显示项目地图，5 语言符号提取 + import 图 + 投影正确。
**二期**：AI 通过 codegraph_explore 一次连通调用链，callers/callees/impact 准确，系统提示引导生效。
**三期**：watcher 自动保鲜，20+ 语言，框架路由识别。

## 风险与缓解

| 风险 | 缓解 |
|------|------|
| tree-sitter grammar 编译问题（Windows） | 选用纯 Rust 可编译的 grammar crate，CI 验证三平台 |
| 跨文件调用解析精度不足 | 标 provenance=heuristic，多候选保留排序，宁缺勿错 |
| 大项目索引慢 | 并行提取 + 批量事务 + 增量同步 |
| explore 在 god-function 发散 | ≤1 未命名桥接限制，预算缩放 |
| UA 投影前端不兼容 | 任务 12 严格按现有 schema 降维 + 回归测试 |
