# 旧模型链路删除与数据库时间戳

日期：2026-11-20。

## 范围与结果

主对话、CLI、SDK 和 Desktop 只使用 SQLite harness 注册表。删除 `ModelCatalog`、`ModelRole`、双角色选择、旧默认模型导入、`deepseek-official` 别名及旧配置回退；模型发现只返回元数据，不决定用户选择。画布既有独立供应商体系不在此轮合并范围。

删除旧初始化、刷新、全局密钥/余额和 `set_chat_model` 入口。首次连接和既有模型页使用相同的供应商保存、模型获取、明确激活、明确选择链路。获取模型列表不自动激活、不选择第一项。没有有效选择时要求用户配置模型，运行链不补默认模型。

思考强度由新注册表选择与显式运行覆盖决定，旧全局设置不再影响路由。当前 turn 冻结选择，下一轮用户输入重新读取。技能安全复审使用明确的供应商/模型身份；官方 DeepSeek 搜索只读取官方地址对应供应商的凭据。

## 数据库迁移

- V26 删除 `settings/app` 内 `catalog`、`discovered_at`、`thinking_depth`、`reasoning_effort`、`reasoning_effort_explicit`，以及旧凭据生命周期文档、种子导入标记；不复制旧默认模型。
- V27 为缺字段的 17 张业务表补 `created_at`、`updated_at`，加上原有 8 张，合计 25 张业务表；单位为 Unix 毫秒。
- 有原始时钟时按原始值回填，其他记录使用迁移时刻。新增字段通过数据库触发器维护；已有字段继续由现有 writer 维护。搜索投影重建也恢复时间戳规则。
- SQLite 系统表、FTS 虚拟表及影子表由 SQLite 管理，不直接 ALTER；FTS 的业务源表 `session_search_docs` 包含时间戳。
- 迁移在事务内执行；补了失败回滚、重复运行、查询投影修复及历史迁移重放验证。

实际升级数据库：`C:\Users\ameth\.deepagent\deepagent.db`，V25 → V27。

升级前在线备份：`C:\Users\ameth\.deepagent\backups\deepagent-before-final-cutover-20261120-104836.db`。

升级后检查：完整性 `ok`，外键违规 0，25/25 业务表均有两个时间戳；无空值、无非正数时间戳。对升级前后的原有业务列计算哈希，除明确删除的旧配置和对应设置更新时间外，全部保持一致，包括新供应商、模型、选择、密钥引用和加密记录。

已有 `api_key_ref` 是不透明标识。被新供应商引用的既有凭据不因名称含旧字符串而改名、复制或删除；认证加密的 AAD 绑定凭据名，改数据库名称会破坏解密。

维护工具 `cargo run -p deepagent-persistence --example migrate_schema --offline -- <已存在数据库路径>` 调用正式产品迁移，不另写一套 SQL 升级器，也不创建不存在的数据库。

## 验证

- `cargo test -p deepagent-persistence --offline -j 1 --target-dir target/legacy-cutover-audit/migrate-target`：244 项通过。
- `pnpm build`：完整前端门禁、TypeScript、Vite 构建通过。
- `cargo check --manifest-path apps/desktop/src-tauri/Cargo.toml --offline -j 2`：桌面 Rust 检查通过。
- `cargo build --manifest-path apps/desktop/src-tauri/Cargo.toml --features custom-protocol --offline -j 2`：原生桌面构建通过。
- `cargo fmt --all -- --check`：通过。

真实原生桌面采用隔离数据目录、Playwright/CDP 和本机合成 HTTP/SSE 服务验收，没有使用桌面控制插件：

1. 全新数据库没有选择、没有激活模型，`configured=false`。
2. 五个已删除的旧命令调用全部得到 command not found。
3. 获取两个模型后仍没有选择，两项均未激活。
4. 明确激活、选择后，刷新恢复相同模型与思考档位；模型发现接口累计只调用一次。
5. 点击真实胶囊切换模型、在真实输入框发送消息，服务端收到所选模型和预期 `reasoning.effort=high`，页面呈现回复；无 JS 异常。

该原生验收证明新配置、Tauri 桥接、真实 UI 和传输链路闭环；本轮没有重新运行真实第三方供应商的全协议行为矩阵。

严格检查和独立提交源码检查均已完成：

- `cargo clippy --all-targets --all-features --offline -j 2 -- -D warnings`：退出码 0。Cargo 仍提示既有 manifest 未使用依赖警告，未将其描述为零警告。
- `cargo check --workspace --all-targets --offline -j 2 --manifest-path target/legacy-cutover-audit/commit-source/Cargo.toml --target-dir target`：独立提交源码快照通过，退出码 0；验证没有依赖未纳入提交的 Skill 缓存及性能改动。
- 相关前端回归 44 项、首次连接真实回调测试 3 项通过。

完整 workspace 命令为 `cargo test --workspace --offline --no-fail-fast -j 2 -- --test-threads=4`，退出码 1：**5755 passed / 25 failed / 131 ignored**，205 个目标返回测试结果，8 个目标失败。保留完整命令的原始结果，不用后续专项复测改写这个统计。

| 失败目标 | passed / failed / ignored |
| --- | --- |
| app-core lib | 1703 / 2 / 45 |
| kernel_v2_e2e | 368 / 5 / 7 |
| app-core mcp_environment | 0 / 1 / 1 |
| builtins lib | 376 / 3 / 3 |
| CLI bin | 225 / 2 / 7 |
| SDK e2e_stdio | 13 / 2 / 0 |
| MCP env_inheritance_audit | 0 / 5 / 1 |
| MCP process_lifecycle_audit | 0 / 5 / 0 |

全量运行暴露的本轮相关旧断言已按新行为修正，并分别复测：

- H219：普通应用设置可以存在，但模型注册表仍无选择，`configured=false`；准备工具前后设置不变。专项 1 项通过。
- Kernel 思考参数：DeepSeek 的 medium 映射为 high，minimal 映射为 low，xhigh 映射为 max；编辑模型初始档位不能覆盖用户已经保存的选择。思考参数专项 6 项和 H54 专项 2 项通过。
- CLI H222：保存用户原始覆盖值，`config/read` 和实际 wire 展示供应商映射后的有效值；minimal/xhigh 是有效输入，非法值仍在发送前拒绝。专项 3 项通过。
- SDK stdio：空注册表使用明确的选模错误，`config/read` 返回 invalid，不再以普通应用设置是否存在推断旧初始化状态。首次专项复跑为 14 passed / 1 failed，暴露线程范围内遗漏的同类旧断言；同步修正后完整 15 项通过，退出码 0。

尚未解决的其他失败包含：运行环境 Local/Managed 断言不符 1 项；真实构建及 MCP 背压取消审计点超时 2 项；Windows 创建符号链接缺少特权（OS 1314）3 项；MCP 环境隔离及进程生命周期 11 项，其中日志可见 PowerShell 执行策略拒绝脚本及子进程启动等待超时。没有为了通过测试改动系统执行策略、权限或沙箱。未重跑最终完整 workspace，不宣称全仓通过，也不将这些失败一概归为基线。

## 提交边界

删除旧运行链需要连同工作区已存在的新思考参数映射、原生图片路径及对应消费者一起提交，避免独立提交源码依赖旧接口。这些依赖已包含在独立源码检查中。

未纳入本轮提交的内容包括 Skill 缓存、请求性能诊断、data root 改动、既有 bundled Skill 删除、先前报告和并发宠物功能。`src-tauri/src/lib.rs` 只选取已验证的旧模型清理改动，工作区的宠物模块、命令和生命周期接线保留；执行记录中此前未提交的 H027～H030 也保持未暂存。

脱敏检查结果、编译日志、合成桌面截图保存在忽略目录 `target/legacy-cutover-audit/`；不提交真实数据库、凭据、模型正文或运行日志。
