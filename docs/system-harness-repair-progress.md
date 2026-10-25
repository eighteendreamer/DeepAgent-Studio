# Harness 问题修复进度

## 当前状态与计数口径（2026-10-26）

本任务是已授权的本地项目修改与测试，不对第三方开展测试，不访问真实内网资源、真实凭据或用户数据。依据 [原审计报告](system-harness-audit-2026-10-21.md) 逐项修复，每轮只处理一个独立根因；H-286/H-72/H-73/H-75 已交付，本轮仅 H-76。审计时已修复有验收的条目跳过；归并、撤销和补充证据不重复计数。

本台账按 **H-01～H-306 的 306 个历史编号槽位一一对应**，不是 306 个有效缺陷。原文有 299 个不同编号的主/归档标题；7 个没有独立主标题的旧编号从原报告归并说明及 Git 撤回记录补齐，仍各占一个追溯行，不新增有效任务。同一 H 的补充证据、真实进程补强或复核章节共用一行。表中标题直接来自当前原报告；无独立标题的旧编号用带“重复/撤销/归并”说明的追溯标题。原文链接指向同一报告，按 H 编号定位对应章节；旧编号按“归并目标/跳过依据”定位。

| 状态 | 数量 | 含义 |
|---|---|---|
| 本阶段已修复并验收 | 5 | H-286、H-72、H-73、H-75、H-76，均限所列本机验收；前四提交为 `56b9e7b9`、`d08c776c`、`d40eef9a`、`8eea7bd5`，H-76 本轮独立提交后回填 |
| 历史已修复跳过 | 17 | 原报告有实现及修复后验收；本轮不重复修复/冒称重验 |
| 归并 | 7 | 只在唯一有效项修复，不增加完成数 |
| 撤销 | 2 | H-263/H-305，不作为缺陷或修复成果 |
| 修复中 | 0 | H-76 所列本机验收结束，不将全量既有失败或其他平台未验算通过 |
| 待修复 | 275 | 执行前仍要核对当前代码、触发条件和原始测试；不是全部条目本轮重新验证未修 |
| 阻塞待处理 | 0 | 整套验证的环境/基线失败另列，不能据此假称通过 |

有效未关闭条目当前为 **275（275 待修复、0 修复中）**；不提供完成率。五项的本机范围验收不表示整个系统、全部平台或完整 workspace 已通过。历史修复依原报告状态跳过，但不虚构其 commit ID：尚未逐项重新定位的历史 hash 明确留为“未重新定位”，保留原文验收来源。

## 总体工时、排序与不确定性

暂估剩余 **200–400 小时有效修复、测试和文档交付时间**，是基于当前有效未关闭规模、跨层 owner/授权/持久化问题与多数单项需原输入回归的初始规划区间，不是将每项假定为相同难度。前五项复杂度不同，且尚无独立计量的有效工时，不能据此虚构可靠吞吐或机械扣减工时；继续按实际样本与剩余根因分组滚动校准，不编造精确结束日期。不以对话跨度或编译等待伪造工时样本。

设备、其他 OS/SDK、真实供应商或 UI 环境的等待时间 **尚无法量化，另计，不混入有效工时承诺**。不可取得的验收必须按具体问题报告阻塞或限制，不静默关闭。优先队列按原报告 A/B 组：文件损坏/授权一致性 → 未知结果副作用 → 接纳/审批/持久提交 → provider/计量/画布 → 历史/缓存/客户端。下一项拟核对 H-20（DAG 成功节点的工作树被清理，产出未交付），因其已有直接数据损失证据，优先于纯展示/历史投影问题；按当前代码复核后再动手，不按 H 编号机械选容易项。H-33/H-23/H-78/H-238/H-260/H-287 仍独立待修，不因本项或 H-75 的锁关闭。

## H-286 本轮实施与验收明细

- 原问题：授权 overwrite 移动到源的祖先目录时，先递归删除目标，连源和无关 sibling 一起删除，再 rename 失败。
- 范围：deepagent-builtins 原 MovePathTool 的私有 native move seam、回归测试和正式 AppCore 原探针；不改其他 H 项，不新建权限/检查点/运行存储，不改前端/协议 DTO，不操作用户目录。
- 根因修复：先以实际路径身份检查同路径、workspace root 及祖先/后代关系；旧目标先 rename 到专属 sibling staging，源发布失败则恢复；所有发布/恢复使用 no-replace 原语，避免覆盖期间新出现的目标。恢复失败保留旧目标且返回 recovery_path/restore_error；发布已成功但清理失败仍准确报告 moved/verified 加 warning，避免诱导重试。staging 在容纳业务数据前取消自动析构删除，panic 不会自动删掉唯一旧副本。
- 实现边界：同一 blocking 本地操作包含 staging/发布/恢复，丢弃异步等待不在两次 await 间隐式清理旧数据；这不等于停止已启动的 native 调用，也不等于 crash-atomic/断电事务。Windows MoveFileExW 不启用 REPLACE_EXISTING/COPY_ALLOWED；Linux/Android renameat2 NOREPLACE、macOS renamex_np EXCL；不支持该能力的系统/文件系统显式失败，不降级为先删后移。未增加依赖，保留 ToolPipeline 权限、检查点和结果通道。

| 实际命令 | 结果 | 证明范围 / 限制 |
|---|---|---|
| `cargo test -p deepagent-app-core --test kernel_v2_e2e move_failure:: --offline -- --include-ignored --nocapture`（修复前） | 1 passed / 1 failed | 原始正式祖先覆盖输入仍破坏临时 source/sibling；显式检查点恢复成功 |
| `cargo test -p deepagent-builtins --lib file_move:: --offline -- --nocapture`（首次修复后） | 11 passed | 文件/非空目录替换、关系拒绝、stage/publish/rollback/cleanup/panic/竞争目标；仅此时版本，后续加的正常竞争回归另验 |
| `cargo test -p deepagent-app-core --test kernel_v2_e2e move_failure:: --offline -- --nocapture` | 2 passed / 0 ignored | 原故障探针已移除 ignore，正式模型工具链、下一 wire、文件 SQLite 重开与检查点恢复通过 |
| `cargo test -p deepagent-builtins --offline`（补最后一条竞争回归前） | 291 passed / 1 ignored；doc-tests 0 | 包含真实 Windows source 占用导致 native 发布失败、恢复旧目标的默认 Tool 回归；ignored 是原有其他 H 探针，不作通过 |
| `cargo test -p deepagent-builtins --offline -j 1 -- --quiet`（最终版本） | 292 passed / 0 failed / 1 ignored；doc-tests 0 | 包含最后追加的“absence 检查后出现新目标”正常拒绝回归，全部 default builtins 测试通过；不执行其他问题 ignored 探针 |
| `cargo test -p deepagent-app-core --test kernel_v2_e2e --offline -j 1 -- --quiet` | 102 passed / 0 failed / 92 ignored | 正式 AppCore 工具链整组默认回归；本项原探针默认执行，不以 ignored 冒充通过 |
| `cargo clippy -p deepagent-builtins --all-targets --offline -- -D warnings` | 未通过 | 未修改的 models/client.rs:355 clone_on_copy 阻断依赖 lint，不伪称本轮引入或顺手修复 |
| `cargo clippy -p deepagent-builtins --all-targets --offline --no-deps -- -D warnings` | 退出 0 | 本问题 crate 全部 target 严格 lint；不替代全依赖 lint |
| `cargo test --workspace --offline -- --quiet` | 未通过 | 默认并行编译内存分配失败/编译进程异常退出，不能列为功能通过 |
| `cargo test --workspace --offline -j 1 -- --quiet` | 编译完成；AppCore lib 1100 passed / 1 failed / 80 ignored，退出 1 | 既有 `completion_plan::audit_tests::discovered_real_build_completes_after_release` 的临时 Cargo 构建未在 4 秒内创建启动标记；Cargo 早停，其他未运行 target 不算通过 |
| `cargo test -p deepagent-app-core --lib completion_plan::audit_tests::discovered_real_build_completes_after_release --offline -j 1 -- --exact --nocapture` | 1 passed，1.18 秒 | 独立复测正常启动/收尾；该夹具注册表为空，不经过 MovePathTool。具体调度/资源成因未进一步确认，不把独立绿色覆盖前述全量失败 |

本轮未执行 Linux/macOS/Android 动态或交叉编译、真实跨卷故障、掉电/崩溃恢复和真实 Desktop UI；主机仅安装 Windows 与 Android Rust targets，不能由 Windows 测试推断这些平台通过。case-alias 已在 Windows 测试；Unix symlink-parent 回归存在但本机不执行。没有修改 SandboxBackend、数据库 schema 或公共 command/DTO。

`cargo fmt --all -- --check`、`git diff --cached --check` 与 306 行台账唯一性/集合/列数校验通过。最终源码移除重复前置判断后，重新运行 builtins/AppCore 两组测试，仍分别为 292 passed/1 ignored、102 passed/92 ignored；格式与 no-deps 严格 lint 再次通过。新增文档首次暂存检查发现一个 EOF 空行，移除后暂存检查通过。代码、测试及两份文档已作为本项独立提交 `56b9e7b9`；本次仅补文档记录，不反复 amend。H-286 标记为“已修复并验收”限上述 Windows 本机正式路径；完整 workspace、全依赖 lint 和其他平台没有通过状态，整个修复任务仍未完成。文档技能的证据链要求用于区分原失败、本机验收及未验范围；人工核对归并/撤销，不把历史证据冒充本轮重验。

## H-72 本轮实施与验收明细

H-72 当前实施记录（2026-10-26）：修复前执行 `cargo test -p deepagent-app-core --test checkpoint_audit audit_missing_backup_preserves_current_file_on_failed_rewind_and_fork --offline -j 1 -- --include-ignored --exact --nocapture`，**0 passed / 1 failed**，测试体 0.16 秒。rewind/fork 均为 failed=true、current_file_exists=false、healthy_target=second-before、history_unchanged=true、checkpoint_unchanged=true、session_count=1。仅使用原临时 fixture；不以本次重验当作新发现。

恢复实现现位于既有 CheckpointManager 的私有 restore 模块：解析全部 checkpoint 后先构建最终替换树，重叠路径保持原恢复顺序；当前工作副本以 no-replace rename 保留到历史提交成功，普通失败则反向补偿，补偿失败保留 recovery 目录并报错。no-replace 原语从 H-286 私有模块提至现有 persistence 共享模块，两处共用，不复制平台分支。AppService 两入口接入同一准备/发布/提交边界；fork 的完整前缀先暂存，在同一 SQLite 事务中写投影后发布，COMMIT 失败撤下派生分支；rewind 以原有文件代际发布为事实，避免发布后 reload 失败被误当作未提交。没有迁移生产数据库或修改公共 DTO。

即时验收时间线（保留中间失败，不代表最终版本反复失败）：原 H-72 探针修复后 **1 passed**，两入口当前两文件均为 after，历史与 checkpoint 未改。恢复层首批 **7 passed**；公开 AppCore `checkpoint_audit` **8 passed / 2 ignored**，覆盖完整备份、缺失旧 checkpoint、文件 SQLite 重开/重试、fork INSERT 与延迟 FK 的真实 COMMIT 失败及无残留子会话。两个 ignored 是未独立编号的 capture 同对象重试契约探针及 H-73，不作本项通过，也不误记为 H-65。首次恢复单测暴露空 suffix 被拼成文件尾斜杠，已改正后原 5 项全部通过。一条历史目录阻断测试实际命中 public preflight，已改名避免假称晚期补偿，并另补私有共享提交 seam 的发布后故障用例。

最终本机结果如下；没有把局部绿色写成 workspace 全绿：

| 实际命令 | 结果 | 覆盖与限制 |
|---|---|---|
| `cargo test -p deepagent-runtime --lib checkpoint:: --offline -j 1 -- --nocapture` | 13 passed / 0 failed / 0 ignored | 原 5 项及新增 8 项；含目录部分复制失败、Windows 占用导致发布失败、完整/非法计划、重叠目录、多检查点、提交失败补偿、补偿失败保留副本与 panic；预期 panic 被 catch_unwind 捕获，不是测试失败 |
| `cargo test -p deepagent-app-core --test checkpoint_audit --offline -j 1 -- --nocapture` | 8 passed / 0 failed / 2 ignored | 原故障默认执行；其他 ignored 不作通过；最终源码也在下一行 workspace 中同结果 |
| `cargo test --workspace --offline -j 1 -- --quiet` | 退出 1，未全绿；编译 6m23s | AppCore lib 1102 passed/80 ignored，kernel_v2_e2e 102 passed/92 ignored，builtins 292 passed/1 ignored，persistence 67 passed；后到 prompts lib 27 passed/1 failed，被既有 H-306 空 `creative.video-edit.v1.defaultUserPrompt` 阻断。之后未运行 target 不算通过；没有修改该问题 |
| `cargo test -p deepagent-runtime -p deepagent-session --offline -j 1 -- --quiet` | 退出 0 | 补跑前述早停未执行模块：runtime lib 282 passed/16 ignored、e2e_stabilization 5 passed、tokenizer 正常项 1 passed/1 ignored、schedule 2 ignored；session 14 passed；两 crate doc-tests 均 0 |
| `cargo test -p deepagent-app-core --lib restore_commit_seam_compensates_files_when_history_rewrite_fails_after_publication --offline -j 1 -- --nocapture` | 1 passed；最终源码另在 workspace AppCore lib 通过 | 共享 private seam 在发布文件后才引入真实 history 故障并验证补偿，不冒称 public 并发故障注入或 SQLite 投影错误 |
| `cargo clippy -p deepagent-persistence -p deepagent-runtime -p deepagent-builtins -p deepagent-app-core --all-targets --offline --no-deps -j 1 -- -D warnings` | 退出 1，未通过 | 未修改的 persistence/run_store.rs:127 `explicit_counter_loop` 阻断 |
| 上条命令尾部追加 `-A clippy::explicit_counter_loop` | 退出 1，未通过 | 继续遇到未修改的 runtime/model_agent.rs:225 `clone_on_copy` |
| 再追加 `-A clippy::clone_on_copy` | 退出 1，未通过 | 继续遇到未修改的 AppCore/plugin_preparation.rs:14 `type_complexity`、run_assembler.rs:1687 `too_many_arguments`；三次后停止叠加豁免，源码没有加入 lint allow，不伪称四 crate 严格 lint 通过 |
| `cargo fmt --all -- --check`、`git diff --check` | 退出 0 | 最终源码格式与差异检查；暂存后仍须检查新文件 |

兼容性与残余：不新增外部版本或生产 schema/DTO，仅将已锁定 tempfile 提为 runtime 正式依赖，把既有 libc/windows-sys 发布原语收敛到 persistence。Windows 已验，Linux/macOS/Android 构建运行、真实跨卷、磁盘写满、强杀/断电与真实 UI 未验。未知平台/不支持 no-replace 的文件系统显式失败；symbolic-link 恢复树、workspace root 及与 checkpoint storage 重叠的恢复目标明确拒绝，不危险降级。文件系统和 SQLite 不是崩溃原子事务；回滚也失败会保留旧当前副本并报告恢复目录，panic 只保证不由析构删除唯一副本，不保证自动完成补偿；fork COMMIT 失败且撤下派生分支也失败时，错误标出保留目录，可能需人工清理/恢复。清理失败只记录 warning，不把已提交恢复假报失败。外部并发/活动运行、分支身份、非法锚点及 Windows DACL 仍分别受 H-75/H-73/H-285/H-287 约束，未在本项关闭。

本项已完成上述 Windows 行为验收，代码、测试和两份文档一起独立提交为 `d08c776c`；本次另做文档 hash 回填，不 amend、不推送。暂存差异检查通过，306 行编号唯一、无缺号、列数正确，状态为已修复 2/历史跳过 17/归并 7/撤销 2/待修复 278。整体修复 goal 仍 active。code-audit 要求人工核对源头和真实故障，docs-generator 要求区分原始失败、共享 seam 和 public 入口证据，避免将前置拒绝或 ignored 算成补偿通过。

## H-73 本轮实施与验收明细（2026-10-26）

当前状态：**已修复并验收，限下述 Windows 本机范围**。以下按执行顺序保留中间失败和阶段性结果，出现“尚未验收”表示当时状态，不覆盖最终范围限制。本轮代码、测试和两份文档已一并独立提交 `d40eef9a`；本次只回填文档，不 amend、不推送。

- 原探针在 HEAD `78fbbba6` 实跑：`cargo test -p deepagent-app-core --test checkpoint_audit branch --offline -j 1 -- --include-ignored --nocapture`，1 passed / 1 failed / 0 ignored，测试体 0.32s；实际文件 old-first、restored_paths=1。原始证据保留，不重复计新发现。
- 首版根因修改在现有 manifest 绑定稳定 EventId；选择恢复时要求当前权威日志同序号仍是同一事件，而非只比较数字或只允许最新 generation。旧 run/checkpoint 继续供审计查询，H-72 文件准备/发布/补偿契约不变；不新增 store、schema migration、UI/协议 DTO。
- 首版 `cargo test -p deepagent-app-core --test checkpoint_audit --offline -j 1 -- --nocapture`：9 passed / 0 failed / 1 ignored，原 H-73 已取消 ignore，实际 current-branch/restored_paths=0；这是新增分支/旧格式回归前的中间结果，尚未完成最终验收。
- 新回归首次编译因 fixture 将临时 FixedClock 借给长期 Session 而报 E0716，测试未执行；已改为命名 clock 绑定后重跑。不把夹具编译错误记作产品缺陷或行为验收。
- 上述修正后同一命令为 16 passed / 0 failed / 1 ignored（测试体 5.55s）；最后新增 source 缺失拒绝/淘汰备份缺失输入之后仍需重跑，当前不关闭 H-73。
- 最终分支专项重跑同一命令：17 passed / 0 failed / 1 ignored，测试体 5.61s，source 缺失拒绝及淘汰备份缺失输入已通过。当前进入 workspace/runtime/lint 验证；不把唯一未处理的 ignored 探针计通过。
- workspace 实跑 `cargo test --workspace --offline -j 1 -- --quiet`：编译 6m56s，AppCore lib 1102 passed/80 ignored、checkpoint_audit 17 passed/1 ignored、kernel_v2_e2e 102 passed/92 ignored、builtins 292 passed/1 ignored、persistence 67 passed，各组 0 failed；到 prompts 时既有 H-306 再次失败（27 passed/1 failed，creative.video-edit.v1.defaultUserPrompt 为空），命令退出 1。后续未运行 target 不算通过；正在补跑 runtime/session，不将局部绿色说成全量成功。
- 补跑 `cargo test -p deepagent-runtime -p deepagent-session --offline -j 1 -- --quiet` 退出 0：runtime lib 283 passed/16 ignored、e2e_stabilization 5 passed、tokenizer 正常项 1 passed/1 ignored、schedule 2 ignored、session 14 passed、两 crate doc-tests 均 0。未将 ignored 或其他未运行 crate 当作通过。
- `cargo test -p deepagent-runtime --lib checkpoint:: --offline -j 1 -- --nocapture`：14 passed / 0 failed / 0 ignored（0.14s），包含本项 source 前置拒绝与 H-72 故障补偿；预期 panic 已 catch，不是失败。严格 `cargo clippy -p deepagent-persistence -p deepagent-runtime -p deepagent-app-core --all-targets --offline --no-deps -j 1 -- -D warnings` 退出 1，未修改的 run_store.rs:127 `explicit_counter_loop` 阻断；不伪称通过，不新增 lint allow。
- 普通 lint（上条不带 `-- -D warnings`）仍退出 1：三个生产库检查后，在未修改的 `tests/kernel_v2_e2e/mcp_discovery_restore.rs:433` 命中默认 deny 的 `panicking_unwrap`；生产库另有既有 run_store/model_agent/context_runtime/plugin_preparation/run_assembler 告警。没有将普通 lint 当作严格通过，没有修改或豁免无关问题；最后仅核对本项 checkpoint_audit target。
- 聚焦检查 `cargo clippy -p deepagent-app-core --test checkpoint_audit --offline --no-deps -j 1` 退出 0，未报告本项修改文件告警，但仍带既有生产库 warnings；只证明该选定 target 完成普通 lint，**不替代严格/all-targets lint**。没有继续叠加豁免。
- 兼容策略已按所列回归验证：未重写 generation 1 可在现有 metadata 事务内补齐全部旧格式检查点（包括保留前缀）身份，保留未知 manifest 字段；历史已重写且缺身份的相关备份明确拒绝自动恢复，不靠时间戳猜归属。即使只是旧版 header/project 重写也会保守拒绝；新格式按 EventId 验证，不受 header 代际改变影响。显式按 checkpoint ID 的恢复入口保留供人工核对，不把它当作自动分支选择。已登记 run 若 source 不存在，manager 构造在创建 checkpoint 目录前失败；无 run 的 detached evidence 不获得已证明的自动恢复资格。
- 范围限制：本项不解决活动 turn 与 rewind 的提交竞争（H-75），也没有引入 fork 对所有祖先文件快照的继承机制；fork 验证是 source 有效分支筛选、复制事件前缀与 child 自有 run/checkpoint 隔离，不能冒称完整祖先快照回放。文件 SQLite 重开不是跨进程强杀/断电验收；其他 OS、真实 Desktop UI、完整特性组合/全依赖 lint 未验。本轮未改前端/桥接 DTO/权限/沙箱，未运行 pnpm build 或生产数据迁移。不改变 H-72 的补偿/人工恢复限制。
- 参考与技能影响：重新读取 Claude 的稳定 messageId 快照定位、Codex 对有效历史应用 rollback 的逻辑，仅借鉴稳定来源/当前有效性，不复制业务代码、不假称运行其测试。冻结 source EventId、原日志旧格式补齐与未知归属拒绝是复用现有 CheckpointStore/EventStore 的本地设计。code-audit 用于人工追溯与故障验证；docs-generator 要求即时分离原失败、中间编译错误、实际通过和未验范围。

证据链：E-H73-01 是修复前 HEAD `78fbbba6` 的原 branch 命令与 old-first/1 path 失败观察；E-H73-02 是最终 checkpoint_audit 命令的 17 passed/1 ignored，以及 checkpoint 专项 14 passed/0 ignored（source_type=command，observed_at=2026-10-26，content_hash/artifact_path=n/a；未另存敏感请求或命令日志）。F-H73（severity=P1，status=validated，confidence=high，location=checkpoint_store.rs::list_for_session_after_sequence）由两条证据确认缺陷与本机修复，不扩为整个系统通过。P-H73（path_type=callflow）：manager 创建冻结源身份 → 既有 manifest 落盘 → 对照当前权威事件筛选 → H-72 恢复事务 → 原 rewind/fork 提交；步骤关联 E-H73-02，原错选路径关联 E-H73-01。范围/授权见本文开头，timeline 为本节逐步追加记录。

最终 `cargo fmt --all -- --check`、`git diff --check`、`git diff --cached --check` 退出 0；台账校验 Rows=306、Unique=306、Missing=[]、BadColumns=0，已修复 3 / 历史跳过 17 / 归并 7 / 撤销 2 / 待修复 277，修复中与阻塞待处理均 0。独立提交 `d40eef9a` 仅含本项 6 个文件；用户原有未跟踪项不读、不改、不暂存。下一项 H-75 仍须原探针复核，整体 goal 保持 active。

## H-75 本轮实施与验收明细（2026-10-26）

当前状态：**已修复并验收，限 Windows 本机后端行为及前端生产回调/构建；真实 Tauri 手动交互未验**。以下为逐步追加的当时记录，不抹去中间失败；代码、测试和当时两份文档已一并独立提交 `8eea7bd5`，本次只回填文档，不 amend、不推送。原探针在 HEAD `1a3c7192` 已复现（1 passed / 1 failed / 0 ignored，0.28s），不重复登记。额外源码核对确认 run_inner 的输入租约先释放，外层仍可能补写 Session RunTerminal，因此仅保护模型请求或内层运行不足。

本地设计首版：通过现有 session 文件锁设施提供共享活动/独占控制 scope；保护附着现有 RunCoordinator admission 与 ActiveRunRegistration 生命周期，不新增输入调度 registry、运行/事件存储或数据库迁移。AppService 在恢复文件前获取独占 scope，原始 truncate 通过同一边界，append 与直连 Kernel/RuntimeEngine 也接入；忙时拒绝，不自动取消。fork 也会恢复同一工作区，故复用同一互斥规则，但不关闭 H-74 等独立问题。

即时验证：`cargo test -p deepagent-persistence --test session_control --offline -j 1 -- --nocapture` 首跑 **6 passed / 1 failed / 0 ignored**。跨 handle、真实子进程共享/独占、强制退出后释放均通过；故障回归在仍保留故意创建的 g2 阻断目录时读取日志而失败（5000ms 打开超时），已改为移除该空临时目录后核对历史，待重跑。即时丢弃 must_use guard 的测试警告已调整，尚未称严格 lint 通过。原失败及中间结果保留，其他平台、真实 UI、完整 workspace 仍未验收。

首版原探针复测：`cargo test -p deepagent-app-core --test kernel_v2_e2e active_rewind --offline -j 1 -- --nocapture`，**2 passed / 0 failed / 0 ignored**，0.33s；忙时明确拒绝且历史未动，实际 invocation 结束后回退成功，两种回答投影均被移除，原 run 单终态保留。持久化同一专项修正后 **7 passed / 0 failed / 0 ignored**，5.09s。并发编译期间一次 cargo fmt 遇 Windows 源码映射 error 1224，未完成，待 rustc 退出后单独重跑；不把该次随后的测试成功算作格式成功。新增直接 RuntimeEngine/Kernel、接纳与检查点回归尚待验。

调用方补强：桌面生产回调在后端返回之前删除 liveTranscripts 并清屏，busy 错误只提示、不恢复原屏幕。先执行 `node --test --test-name-pattern='rewind refusal' scripts/check-session-control.test.cjs`（apps/desktop）得到 **0 passed / 1 failed**，实际 messages=[] 而非原对象。只将失效缓存/清屏移到 await rewindSession 成功之后；同一文件 AST 提取、转译、实际执行回调，拒绝/请求中不变、成功刷新、等待时切换会话、空会话四项 **4 passed / 0 failed / 0 skipped**。该配套修复归属 H-75，不另计问题。已经接入前端 build 回归门，尚待完整 pnpm build；没有改 Tauri/SDK DTO、组件样式或添加新控件，真实 Desktop 手动交互未验。

四 crate 实跑 `cargo test -p deepagent-persistence -p deepagent-session -p deepagent-runtime -p deepagent-app-core --offline -j 1 -- --quiet` 退出 **0**，编译 5m20s：AppCore lib 1104 passed/71 ignored，checkpoint_audit 18 passed/1 ignored，kernel_v2_e2e 103 passed/91 ignored，persistence lib 67 passed，session_control 8 passed（含子进程 helper 测试，另有 16 次真实线程抢占），runtime lib 284 passed/16 ignored，e2e_stabilization 5 passed，session lib 14 passed，四 crate doc-tests 均 0；各组 0 failed。feature 合并范围不同，不把与 workspace 的测试数量差异当作删测试。本命令编译的 E2E 是追加 provider failure/cancel 两项之前的版本，因此新两项正在专项重跑；不冒称该命令已覆盖后加测试。源码格式随后单独 cargo fmt 已退出 0，前端完整 build 与 workspace/lint 尚待。

最终追加故障项首次专项 `cargo test -p deepagent-app-core --test kernel_v2_e2e active_rewind --offline -j 1 -- --nocapture` 为 **3 passed / 1 failed / 0 ignored**：正常/回退/供应商 400 失败正常，取消用例在未释放离线 transport 时等待 5s 超时。核对 HttpTransport::stream_cancelled，兼容默认仅在 stream 之前检查取消；本屏障夹具此前没有实现实时 flag 处理，不能据此宣称真实 HTTP 取消有缺陷。已在同一离线 transport 实现取消感知 seam（不改生产取消机制），待重跑；保留该夹具失败证据，不通过释放成功响应冒充取消。前端 `pnpm build` 已退出 **0**（包括新增回调 4 项、tsc 与 Vite），Vite 46.70s，有第三方浏览器 externalization/大 chunk 警告，未声称零告警或实际 Tauri 点击验收。

取消感知夹具修正后，最终 active_rewind 专项 **4 passed / 0 failed / 0 ignored**（0.33s）：正常、不取消的 busy 拒绝、供应商 400 failed、实时 flag 取消四条正式 ChatService 链均通过；后两组无回答落盘，原 RunStore 为 failed/cancelled 且 run_events 单终态，实际调用退出后回退成功。已开始 `cargo test --workspace --offline -j 1 -- --quiet`，仍不将本项先标验收完成。

最终 workspace 实录：上述命令编译 **6m52s** 后运行，退出 **1**。AppCore lib **1104 passed/80 ignored**、checkpoint_audit **18 passed/1 ignored**、kernel_v2_e2e **105 passed/91 ignored**（已含最终四种 active_rewind 场景）、builtins **292 passed/1 ignored**、persistence lib **67 passed**、session_control **8 passed**，这些组均 0 failed；之后 prompts lib **27 passed/1 failed**，既有 H-306 的 `creative.video-edit.v1.defaultUserPrompt` 为空。Cargo 早停，后续 target 不计本次 workspace 通过；前述四 crate 退出 0 已独立验证 runtime/session，但不能替代全 workspace 结果。

最终 lint 实录：`cargo clippy -p deepagent-persistence -p deepagent-session -p deepagent-runtime -p deepagent-app-core --all-targets --offline --no-deps -j 1 -- -D warnings` 退出 **1**，被未修改的 `run_store.rs:127 explicit_counter_loop` 阻断。随后四 crate `--lib` 普通 lint（不带 `-- -D warnings`）退出 **0**，仍有既有 run_store/model_agent/context_runtime/plugin_preparation/run_assembler warnings；`cargo clippy -p deepagent-persistence --test session_control --offline --no-deps -j 1` 退出 **0**，仅带同一生产库 run_store 告警，没有本项测试文件告警。没有叠加豁免、顺手修基线或冒称严格/all-targets/全依赖 lint 通过。

补充目标 lint：`cargo clippy -p deepagent-runtime --tests --offline --no-deps -j 1` 退出 **0**，带既有 model_agent/schedule_audit warnings；`cargo clippy -p deepagent-app-core --test checkpoint_audit --test kernel_v2_e2e --offline --no-deps -j 1` 退出 **1**，被未修改的 `mcp_discovery_restore.rs:433 panicking_unwrap` 阻断，另有既有 responses_custom_tool 布尔断言 warning，不把两个 target 记通过。最终 `cargo fmt --all -- --check`、`git diff --check` 退出 **0**；暂存后仍须检查新文件与台账。

边界复核：共享活动 scope 只协调同一 session 的运行与 rewind/会恢复工作区的 AppService fork，不替代 InputLeaseRegistry，不解决 H-33 多宿主之间的运行互斥。取消只是设置状态，所有权必须由真实执行退出后释放；等待中的 admission 仍算忙。直接嵌入方若跨调用持有历史快照，应在恢复快照前获取 EventStore activity 并持有到最后写入；本项不使任意旧 Session 自动刷新，也没有给离线/外部直接写日志的代码补分支身份。不同 session 共享 workspace、后台笔记/知识派生状态（H-23）、脱离 Future 生命周期的本地进程或 native 副作用，不在本次动态证明范围。锁是 fs2 协作式并发边界，不是沙箱；旧版本宿主不识别新控制锁，因此不能混跑新旧二进制获得本项保证。不改事件格式、DB schema、供应商字段或 DTO，新增空 control.lock 文件沿用稳定锁目录保留。

证据链：E-H75-01 为原 HEAD `1a3c7192` active_rewind 命令与迟到回答失败观察；E-H75-02 为最终同一专项 4 passed/0 ignored、四 crate 回归及持久化控制 8 项；E-H75-03 为桌面回调原拒绝场景 0 passed/1 failed、修正后 4 passed/0 skipped 和 pnpm build 退出 0；E-H75-04 为上述全量及 lint 实际退出和基线阻断。source_type=command，observed_at=2026-10-26，content_hash/artifact_path=n/a，未额外保存敏感模型日志。F-H75（severity=P1，category=design，status=validated，confidence=high，location=RunCoordinator::admit / AppService::rewind_session / EventStore 控制 scope）由 E-H75-01 证明缺陷，E-H75-02/03 证明上述本机行为，E-H75-04 保留全量未通过边界。P-H75（path_type=callflow）：接纳取得共享 scope → 原调度/快照恢复 → 原 Kernel/工具循环 → 外层失败/终态投影 → 释放活动保护 → 独占检查点恢复与 truncate；每步关联 E-H75-02，桌面成功后刷新关联 E-H75-03，原失败路径关联 E-H75-01。范围/授权见文首，时间线为本节按执行顺序追加记录。code-audit 影响是人工复核完整写入生命周期；docs-generator 影响是将原失败、夹具错误、本机验收与未验范围分离留档，未用扫描结果代替复现。

交付前校验：Rows=306、Unique=306、Missing=[]、BadColumns=0；已修复 4 / 历史跳过 17 / 归并 7 / 撤销 2 / 待修复 276 / 修复中 0。显式暂存本项 15 个文件（含两个新增测试文件），`git diff --cached --check` 通过；用户原有五类未跟踪项不读、不改、不暂存。尚未创建提交，此刻不猜填 hash；提交后另做文档回填，不 amend、不推送。下一项 H-76 只排队，未开始复现或修改；整体 goal 保持 active。

提交后回填：本项独立修复提交为 `8eea7bd5`，恰含上述 15 个文件；此段与表头、H-75 行、原审计状态同步更新，保留前段提交前的真实时间线。未推送、未创建 PR，未改动用户原有未跟踪项。剩余有效待修复 276；总工时仍为 200–400 有效小时初估，环境等待另计，没有足以机械校准吞吐的有效工时记录。

## H-76 本轮实施与验收明细（2026-10-26）

当前状态：**已修复并验收，限所列 Windows 本机行为**。三种 mutation 共用当前字节版本/已读窗口/暂存后复核/发布边界，拒绝陈旧覆盖而不替代权限/检查点。代码、测试和当时两份文档将独立提交，提交后另做 hash 回填；此刻不猜写提交号。以下保留阶段性失败和当时尚待项，不覆盖本节最终限制。

交付前核对：`cargo fmt --all -- --check`、`git diff --check`、显式暂存 9 个本项文件后的 `git diff --cached --check` 均退出 0；台账 306 行、306 唯一编号、无缺号、8 列正确，已修复 5/历史跳过 17/归并 7/撤销 2/待修复 275/修复中 0。原有五类未跟踪项未读、未改、未暂存。所有本轮测试进程已结束；下一项 H-20 仅排队，尚未开始其原探针复核或生产修改。

最终补跑 `cargo test -p deepagent-runtime -p deepagent-session --offline -j 1 -- --quiet` **退出 0**：runtime lib **284 passed/16 ignored**、e2e_stabilization **5 passed**（54.68s）、tokenizer 正常项 **1 passed/1 ignored**、schedule **2 ignored**、session **14 passed**，两 crate doc-tests 均 0。未执行项不作通过；完整 workspace/严格 lint 的基线失败仍保留，不影响原 H-76 本机不变量已验的事实，也不等于整个任务完成。

以下先列最近一次专项/契约校验进展，随后保留修复前与首版实录；“尚待”均指该条记录当时状态，最终验收另列。

修正契约断言后同一 AppCore 专项：**3 passed / 0 failed / 0 ignored**（1.74s，编译 1m03s）。三种 mutation 的陈旧拒绝、外部全文保留、重读成功，下一 wire 与重开 SQLite 的唯一原生基础 envelope 完整相等、action failed/completed、单 succeeded run 终态均通过。不要求一次工具冲突使整个 run 失败，不证明真实供应商行为；workspace/lint 收尾仍待。

严格 lint 实录：`cargo clippy -p deepagent-persistence -p deepagent-builtins -p deepagent-app-core --all-targets --offline --no-deps -j 1 -- -D warnings` 退出 1，被未修改的 `persistence/src/run_store.rs:127 explicit_counter_loop` 阻断。不增加豁免、不顺手修改，不能列为通过；workspace 仍在执行。

查证首跑断言差异：Session `response_items_for_payload` 持久化基础 status/error，ModelAgent observation 与既有 `failed_observation_is_fed_back_with_recovery_hint` 测试明确为热请求追加 tool/recovery_hint。因此测试改为成功完整相等、失败完整基础 envelope 相等且额外核对 tool/非空 hint，不以 substring 代替、不改生产投影；待重跑。这不等于已验所有热/冷历史投影差异。

最终严格相等 E2E 首跑：`cargo test -p deepagent-app-core --test kernel_v2_e2e file_version --offline -j 1 -- --nocapture`，**1 passed / 2 failed / 0 ignored**（0.86s，编译 1m54s）。持久化保留 status/error，下一 wire 多了 tool/recovery_hint，导致完整 JSON 相等断言失败；正常 write 两组通过。先核对既有投影契约，不以修改生产实现来迁就测试，仍未最终验收。

修复前在 HEAD `4c885044` 执行原命令 `cargo test -p deepagent-app-core --test kernel_v2_e2e file_version --offline -j 1 -- --include-ignored --nocapture`，**1 passed / 1 failed / 0 ignored**，0.88s。陈旧读取的 write_ok=true、外部 owner 被覆盖；未修改和重新读取对照均成功。授权/网络范围同文首，所有样本为临时目录虚构内容，不新增发现或操作用户数据。

首版根因方案（本地设计，尚未最终验收）：三种 mutation 共用一个观察版本/暂存/最终比较/发布边界，读工具按真实字节而非 mtime/长度决定是否返回空桩。缓存区分已加载字节和已交付窗口，整文件替换要求完整覆盖，局部编辑仅改已读范围；多次编辑追踪内存替换后的范围。原子替换仍复用旧平台原语，新路径使用既有 no-replace 发布。借鉴 Claude FileWriteTool 的实际写入前版本复核及无 await 区段，不复制业务代码，也不声称参考实现解决任意外部并发。

复用 persistence 的 fs2/sha2 实现稳定路径协作锁，四种 read/write/edit/multi_edit 从同一临界区进入，缓存与宿主可独立；锁位于相同 OS 临时根的散列命名空间，不删除活动锁文件，覆盖普通父目录别名，不是授权/沙箱或任意文件系统 CAS。检查/暂存/最终重读/发布/缓存失效放在一个 blocking 操作内；非协作写入在最终比较与 rename 的 syscall 间隙仍可竞争，旧二进制/不同临时根不受该锁协调，不能夸成通用多写入者事务。H-238 actor 归属、H-260 压缩后可见性、H-287 DACL、H-78 路径授权及 delete/move/native 后台副作用仍是独立范围。

首版实际 `cargo test -p deepagent-builtins --lib file_ --offline -j 1 -- --nocapture`：**56 passed / 0 failed / 0 ignored**，测试体 0.73s；筛选也命中一个既有 bash 临时文件测试，不把它说成 56 个新场景。出现本项移除旧 helper 后残留的 PathBuf 未使用告警，已移除，待后续重跑；既有预期 panic 已 catch，不是测试失败。此时仅既有 file 筛选回归，尚未新增故障/并发回归或复测正式原探针，不能关闭 H-76。

首版正式原探针同命令复测 **2 passed / 0 failed / 0 ignored**（0.66s，编译 2m05s），外部 owner 保留、陈旧写入返回失败，两个正常对照保持成功。此时二进制尚未包含后来追加的独立 mutation 回归及扩展 E2E；不能冒称已经覆盖 edit/multi_edit、文件重开或多宿主。追加测试使用 `FileTimes::set_modified`/`File::set_times`，已查 [Rust 官方文档](https://doc.rust-lang.org/std/fs/struct.FileTimes.html) 为 1.75 起可用；当前 workspace 声明 Rust 1.80，尚未执行该 MSRV 编译。仍不关闭 H-76。

追加首批实录：`cargo test -p deepagent-builtins --lib file_mutation:: --offline -j 1 -- --nocapture`，**18 passed/0 failed/0 ignored**（0.24s，含一个子进程 helper，并非 18 个独立故障）；同内容改时间、同长度同 mtime 改内容、实际窗口与完整覆盖、Unicode/CRLF 多次编辑、暂存后更改、no-replace 竞争、发布故障/panic 清理、12 次双缓存竞争和独立子进程正式文件工具均通过。预期 panic 由 catch_unwind 捕获。之后同一 E2E 命令移除 include-ignored 为 **3 passed/0 failed/0 ignored**（1.55s），原探针已取消 ignore，write/edit/multi 的陈旧拒绝和重读成功、下一 wire 错误、durable action 和新文件 SQLite 句柄均已核对。

上述二进制尚未包含随后增加的强杀 owner/实际排他锁测试、等待期间取消回归，以及原生结果与 wire 的严格相等断言，不能用它们证明后加项已过。源码进一步缩短 cache mutex 持有范围，磁盘临界区使用路径 gate 而非串行化所有无关文件。复用现有 ToolExecutionContext 在 blocking 开始、取得锁后和最终提交前检查取消，不另建取消 flag；正在运行的 native syscall 不能撤销，Future 被丢弃也不等于停止 blocking 线程。记录的观察窗口是文件工具产出的窗口，不证明后续 Runtime 截断/压缩或其他 actor 的最终可见性，H-238/H-260 不关闭。新增源码和故障项还需重跑。

H-76 交接前最新专项实录（2026-10-26）：`cargo test -p deepagent-builtins --lib file_mutation:: --offline -j 1 -- --nocapture`，**20 passed / 0 failed / 0 ignored**，测试体 5.07s、编译 17.24s；含一个 subprocess helper。独立进程持锁/5 秒竞争超时/强杀回收后重取、入口前和等待 gate 期间取消不发布/正常重试均通过，覆盖最后 context/cache mutex 修改。预期 panic 已捕获，不是失败。AppCore 严格 wire/persisted equality、workspace、lint 尚待，仍为修复中。

H-76 人工复核与兼容边界：四工具 `invoke` 统一进入各自 `invoke_with_context`，三种 mutation 再进入同一 `file_mutation::apply`，无递归调用或第二条写链。路径 gate 先于 cache mutex，cache 锁只取观察快照/失效，不跨整个磁盘暂存；范围按反向替换维护，不扩到未读后缀。已有工具参数、成功结果字段、权限/审批流水线、provider、协议 DTO、数据库 schema 和前端未改；新增 persistence Rust 协作函数，不是新权限中心。大文件整写必须分页读全、外部变化明确冲突，是有意收紧的执行契约。

Workspace 实录：`cargo test --workspace --offline -j 1 -- --quiet` **退出 1**；Cargo Finished 为 7m01s（含构建锁排队，不作有效工时）。AppCore lib **1104 passed/80 ignored**、kernel_v2_e2e **107 passed/90 ignored**、checkpoint_audit **18 passed/1 ignored**、builtins **312 passed/1 ignored**、persistence lib **67 passed**、session_control **8 passed**，以上各组 0 failed。之后 prompts lib **27 passed/1 failed**，仍为既有 H-306 的 `creative.video-edit.v1.defaultUserPrompt` 为空；没有修改该问题，早停后未运行 target 不算通过，另补 runtime/session。

普通生产 lint：`cargo clippy -p deepagent-persistence -p deepagent-app-core --lib --offline --no-deps -j 1` **退出 0**，带既有 run_store explicit_counter_loop 及 AppCore context_runtime clone_on_copy、plugin_preparation type_complexity、run_assembler too_many_arguments 警告；本项修改文件无新增告警，但不替代严格或所有 test target lint，不增加 allow。

选定 E2E lint：`cargo clippy -p deepagent-app-core --test kernel_v2_e2e --offline --no-deps -j 1` **退出 1**，被未修改的 `mcp_discovery_restore.rs:433 panicking_unwrap` 默认 deny 阻断，另有 `responses_custom_tool.rs:58 bool_assert_comparison` 既有 warning；没有报告 file_version 修改告警，但该 target lint 未通过，不加豁免、不顺手修复。

聚焦严格 lint：`cargo clippy -p deepagent-builtins --all-targets --offline --no-deps -j 1 -- -D warnings` **退出 0**，包含本项 builtins 全 target；不是 persistence/AppCore/全依赖/全 workspace 严格通过。普通生产 lib lint 与 workspace 此时仍在执行，未提前记通过；Cargo 等待构建锁的时间不算有效修复工时。

限制仍保留：每次 read/stub 实际读取字节，增加磁盘 I/O，未做性能 benchmark；OS 临时根下零内容 sidecar 为稳定锁身份保留，不能在其他 owner 活动时直接删除，未实现通用回收器。5 秒锁争用会返回 busy，不无限等待。gate 持有期间的取消测试观测未结束 Future 并设置取消，不声称精确定位 blocking 线程已进入哪条 native 指令；超时丢弃 Future 或已进入 syscall 不保证后台工作被撤销。最后比较/rename 间隙、不同临时根/旧二进制/非协作 delete/move/shell、hardlink/网络文件系统、H-78/H-238/H-260/H-287 均未因此关闭。未跑其他 OS、MSRV、断电事务、真实编辑器/DeepSeek/参考工程或 UI；本项无前端变更，不虚构 UI 验收。

## 按 H 编号的一一对应台账

“待修复”仅表示未关闭，既有条件性风险和未验范围仍按原项保留；不是预先认定本轮能复现，也不是授权跳过先复核步骤。历史/归并/撤销行的修复摘要和验收字段说明其不适用原因，不冒充新修复。

<!-- repair-ledger:start -->
| H 编号、原标题与原报告链接 | 优先级 | 当前状态 | 归并目标 / 跳过依据 | 根因与修复摘要 | 测试命令、结果 / 未执行验收 | 修复 commit ID | 残余、下一步与剩余工时 |
|---|---|---|---|---|---|---|---|
| [H-01：stdio 事件队列满时丢弃关键通知，连接仍表现正常](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-02：Harness ACK 与运行接纳分离，立即中断可能被虚假确认](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-03：Harness 的逐次沙箱 backend 选择未进入实际执行路由](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-04：Harness 恢复与 steer 替换运行会清空显式模型和权限配置](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-05：Chat Completions 往返投影拆散同一次 assistant 工具响应](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-06：MCP 断线恢复自动重发结果未知的工具调用，可能重复产生副作用](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-07：MCP 工具发现忽略分页，只发布首页工具且不报不完整](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-08：MCP 初始化缺少 initialized 通知，严格服务端无法进入就绪状态](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-09：stdio MCP 并发回包按读取者筛选，其他请求的响应会被永久丢弃](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-10：运行结束后预取压缩任务仍继续执行，结果及 usage 无处归集](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-11：以消息内容反查压缩边界，重复回答会让恢复重新带回已摘要的历史](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-12：Streamable HTTP 未保存初始化分配的 session header，有状态远程服务后续请求失败](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-13：AppCore 插件就绪快照与共享技能注册表不同步，新请求可继续声明已禁用技能](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-14：无效压缩保留原上下文时，已完成摘要请求的 usage 一同被丢弃](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-15：审批等待被取消后未清理，运行终态后仍接受迟到回答并授予跨调用权限](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-16：审批直接以 provider call_id 为全库身份，不同运行之间产生幂等冲突](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-17：桌面收到取消后的保存失败，却因对话已 settled 而静默丢弃错误](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-18：输出上限重试后仍截断时，未完成工具调用被补参数并提交](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-19：子 Agent 的工具限制只过滤 schema，实际执行仍可调用被排除的工具](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-20：DAG 成功节点的工作树被强制删除，文件未交付给主目录或下游](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-21：子 Agent 恢复遗漏原始输出契约，schema、phase、label 全部丢失](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-22：DAG 子运行只改变声明的工作目录，文件工具仍绑定父工作区](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-23：回退会话后旧笔记仍按复用序号宣称覆盖，压缩候选会替换新分支](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-24：较长的非 ASCII 供应商错误触发 panic，结构化错误处理被中断（已修复）](system-harness-audit-2026-10-21.md) | P2 | 历史已修复跳过 | 原报告明确记录修复实现与修复后验收 | 保留现有实现，不重复修复 | 历史命令与结果见原项；本轮未重跑，不声称重新验收 | 历史提交未在本轮重新定位，不猜填 hash | 保留原项未验限制；本阶段不另估修复工时 |
| [H-25：丢弃供应商 Retry-After，在服务恢复前耗尽限流重试（已修复）](system-harness-audit-2026-10-21.md) | P2 | 历史已修复跳过 | 原报告明确记录修复实现与修复后验收 | 保留现有实现，不重复修复 | 历史命令与结果见原项；本轮未重跑，不声称重新验收 | 历史提交未在本轮重新定位，不猜填 hash | 保留原项未验限制；本阶段不另估修复工时 |
| [H-26：标准 PreToolUse 输出中的 deny/ask 被忽略，实际工具照常执行](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-27：项目 hooks 信任开关只覆盖独立文件，settings.json 内联 hook 仍执行宿主命令](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-28：未信任项目的权限配置覆盖用户只读设置，实际写文件没有请求审批](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-29：HTTP 错误正文挂起时取消检查缺失，停止等待退化到正文完成或总超时（已修复）](system-harness-audit-2026-10-21.md) | P2 | 历史已修复跳过 | 原报告明确记录修复实现与修复后验收 | 保留现有实现，不重复修复 | 历史命令与结果见原项；本轮未重跑，不声称重新验收 | 历史提交未在本轮重新定位，不猜填 hash | 保留原项未验限制；本阶段不另估修复工时 |
| [H-30：SSE 实际按 data 行解析，合法多行事件失败，未闭合终态帧却被接受](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-31：同步原生压缩的摘要请求没有运行取消信号，停止必须等摘要返回（已修复）](system-harness-audit-2026-10-21.md) | P1 | 历史已修复跳过 | 原报告明确记录修复实现与修复后验收 | 保留现有实现，不重复修复 | 历史命令与结果见原项；本轮未重跑，不声称重新验收 | 历史提交未在本轮重新定位，不猜填 hash | 保留原项未验限制；本阶段不另估修复工时 |
| [H-32：备用宿主打开数据库先执行无所有权恢复，误终结主宿主的工具动作和子 Agent](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-33：输入租约仅限 ChatService 内存，两宿主可同时推进同一会话](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-34：并行工具绕过统一执行流水线，单工具超时保护在批量调用时失效](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-35：并行调度跨越串行写屏障，新增无关读操作会改变已有调用的执行语义](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-36：完整工具结果在 turn 结束时被删除，持久化历史与 artifact 索引留下失效引用](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-37：定时任务丢弃审批通知却仍等待人工回答，无人值守运行停在不可达审批上](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-38：正式工作流工具注册表为空，工具失败被包装成节点完成并令整次运行成功](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-39：条件分支只生成标签，未选中的互斥路径仍被实际派发](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-40：专业工作流配置在前端序列化被删除或被后端按另一套字段读取](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-41：其他会话完成会清掉当前画布运行监听和停止入口](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-42：定时调度没有跨进程所有权，同一触发时段可重复派发一次性和循环任务](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-43：运行启动先于事件订阅就绪，提前终态丢失后前端持续等待](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-44：启动恢复绕过统一终态边界，运行表已失败但会话重放仍没有终态](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-45：父 Kernel 总时限直接丢弃执行 Future，前台子 Agent 没有取消和持久化收尾](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-46：Windows MCP transport 只终止直接进程，非 libuv 服务的普通后代仍继续运行](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-47：新增无关 MCP 配置会取消未变服务的初始化，并将宿主取消缓存为该服务失败](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-48：stdio MCP 探活耗尽后只停止探针，旧能力仍可发现，后续每次调用重新启动完整恢复预算](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-49：备用模型切换只更新模型和预算，实际请求仍携带主模型的输出上限](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-50：移动输入未校验界面快照，已过期和未知引用仍下发设备命令](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-51：项目激活改写共享知识服务，既有运行的记忆读写会被重定向到另一个项目](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-52：知识库锁仅保护最终文件写入，独立宿主的旧索引可把不同笔记静默覆盖](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-53：原生压缩移除相关记忆后，后台检索去重与预算仍保留旧记录](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-54：有效模型参数被桌面深度覆盖，显式思考强度与输出上限没有进入真实请求](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-55：简单思考模式直接携带 Responses 的 none 进入 Chat 适配器，本地校验拒绝整个运行](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-56：宿主就绪快照合并未检查工具重名，已连接 MCP 可让普通聊天在请求前整体失败](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-57：MCP 说明与技能目录仅按已发送身份去重，正文未进入可回放模型历史，后续无状态请求丢失内容](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-58：附件清理把字符替换当作身份验证，空 ID/点目录可删除整桶，父目录 session 可删除附件 root 外数据](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-59：WASM 调用完成后遗留的 watchdog 中断共享引擎上的后续调用](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-60：命令输出管道的等待超时仅丢弃句柄，读取任务仍保留缓冲并继续收集迟到输出](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-61：对抗复核脱离主运行取消，取消后仍可落成功终态，总时限也不清理复核请求](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-62：停滞分类等待没有运行取消，停止要等分类结束，但最终取消映射正确](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-63：对抗复核和停滞分类丢弃已返回的 usage，现有会话成本账本漏记附属请求](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-64：自动完成验证没有运行取消与进程归属，总时限终态后真实 build.rs 仍能写文件](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-65：两处自动验证绕过已配置命令后端，显式构建被拒绝后仍可在宿主执行项目脚本并写出工作区](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-66：Flash 开启 DeepSeek 原生搜索后，本地搜索函数被替换为该模型会忽略的原生工具](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-67：当前轮图片未进入实时模型 Surface，原生镜像也丢附件，已有会话的图片在下一轮仍不出现](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-68：历史图片文件丢失时静默退化为纯文字请求，续聊仍成功](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-69：只限制单张图片，缺少多图序列化后的请求体总预算](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-70：导入哈希未绑定模型历史引用，图片替换后同一附件 ID 静默发送新内容](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-71：发现目录丢弃官方输入模态，文字模型仍收到原生图片请求](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-72：检查点备份缺失时，失败的回退或分叉仍删除当前文件](system-harness-audit-2026-10-21.md) | P1 | 已修复并验收 | 原探针修复前失败、修复后默认通过；不重复登记 | 全 checkpoint 准备/发布/历史提交与补偿边界；共用 no-replace 原语 | 详见本节命令表：checkpoint 13 passed，AppCore audit 8 passed/2 ignored，相关库/端到端通过；workspace/H-306 和 lint 基线限制保留 | `d08c776c` | Windows 本机行为验收；断电/其他 OS/UI 未验，补偿失败有恢复路径；不关闭 H-73/H-75/H-285/H-287；本项本地剩余 0，环境另计 |
| [H-73：回退后的旧分支检查点仍可覆盖新分支文件，调用却返回成功](system-harness-audit-2026-10-21.md) | P1 | 已修复并验收 | 原探针修复前失败，修复后取消 ignore 并通过 | manifest 冻结稳定 source EventId，按权威有效历史筛选；保留旧记录/前缀恢复，旧格式身份不明时拒绝 | audit 17 passed/1 unrelated ignored；checkpoint 14 passed；runtime/session、AppCore/持久化相关回归通过；workspace H-306 与 lint 基线未绿，详见本节 | `d40eef9a` | Windows 本机验收；旧格式可能需人工核对，H-75/祖先快照继承/其他 OS/UI 不在关闭范围；本项本地剩余 0，环境另计 |
| [H-74：分支边界截在源消息与原生镜像之间，保留的消息不进入下一请求](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-75：运行中回退没有取消或提交栅栏，已丢弃 turn 的回答仍写入新历史](system-harness-audit-2026-10-21.md) | P1 | 已修复并验收 | 原 HEAD 1a3c7192 为 1 passed/1 failed，原证据保留 | 复用会话锁：admission 至外层投影持共享 scope，rewind/fork 持独占 scope，忙时拒绝；桌面成功后才清屏 | active_rewind 默认 4 passed/0 ignored；四 crate 测试及 pnpm build 退出 0；workspace/严格 lint 既有失败及实际命令见 H-75 明细 | `8eea7bd5` | 限 Windows 后端及回调/构建；真实 UI/其他 OS 未验；H-33/H-23/跨 session 副作用未解决；下一项 H-76，总区间 200–400 有效工时 |
| [H-76：文件写入只验证“曾经读过”，外部已保存的改动仍被旧内容成功覆盖](system-harness-audit-2026-10-21.md) | P1 | 已修复并验收 | 原 HEAD 4c885044 复现失败，修复后原探针已转默认且通过，保留中间断言失败 | 观察真实字节与已交付窗口，三 mutation 统一提交、暂存后复核；稳定路径协作 gate，成功失效 | mutation 20 passed；正式 file_version 3 passed/0 ignored；workspace 内 builtins312、AppCore1104/正式链107、persistence67；runtime/session补跑通过；完整命令与既有失败见本节 | 提交后另做文档回填 | Windows 本机范围；非协作 syscall 间隙、actor/compaction/路径/DACL及其他OS仍有限制；下一项 H-20，总区间200–400有效工时 |
| [H-77：grep 未继承敏感文件读取拒绝，普通搜索可将被禁止的内容送入模型请求](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-78：Windows 工作区目录联接绕过词法路径限制，read_file 读取根目录外文件](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-79：glob/list_dir 返回被直接读取策略拒绝的敏感文件名](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-80：Responses repair 对外部混合工具类型缺少不变量校验（条件性）](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-81：Responses 完成文本 item 被错误补充全局 fallback（含空 assistant）](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-82：Responses 内容过滤被误判为输出到限，触发预算升级和长度续写](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-83：最终钩子报错前已发布完成，stdio 同一 turn 收到相互冲突的两个终态](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-84：审批请求在 gate 决策前投影，造成重复发送或自动放行误报](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-85：模型目录与余额 GET 没有总时限，响应头或响应体停滞时请求可无限 pending（已修复）](system-harness-audit-2026-10-21.md) | P2 | 历史已修复跳过 | 原报告明确记录修复实现与修复后验收 | 保留现有实现，不重复修复 | 历史命令与结果见原项；本轮未重跑，不声称重新验收 | 历史提交未在本轮重新定位，不猜填 hash | 保留原项未验限制；本阶段不另估修复工时 |
| [H-86：模型发现 GET 的错误正文可把反射的 API Key 写入 `/doctor` 会话历史（已修复）](system-harness-audit-2026-10-21.md) | P3 | 历史已修复跳过 | 原报告明确记录修复实现与修复后验收 | 保留现有实现，不重复修复 | 历史命令与结果见原项；本轮未重跑，不声称重新验收 | 历史提交未在本轮重新定位，不猜填 hash | 保留原项未验限制；本阶段不另估修复工时 |
| [H-87：模型刷新用宿主本地旧快照覆盖另一宿主新设置](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-88：旧模型刷新在退出登录或换 Key 后重新写入旧凭据](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-89：Harness 显式选择未知模型时静默回退到默认模型](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-90：Harness SDK 关闭 writer 后的失败请求残留 pending reply](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-91：Harness 并发事件生产可使实时 eventSequence 倒退](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-92：stdio EOF 直接退出运行时，活动 turn 没有有界收尾](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-93：Chat Completions 路由未转换 Responses 命名工具选择的字段形状](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-94：Chat 兼容投影覆盖相邻 reasoning item，只保留最后一段](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-95：启动恢复把同一 session 的最新中断原因套用到所有未完成 run](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-96：定时任务已派发但落账失败，重启后会再次派发且调度循环退出](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-97：调度器停止不取消或等待已派发的定时运行](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-98：DeepSeek Chat Completions 的 JSON 输出模式被拒绝或静默丢弃](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-99：Chat Completions 静默忽略配置中的 top_logprobs 默认值](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-100：Harness SDK initialize 接受缺少 result/error 的 JSON-RPC response](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-101：Harness SDK 忽略畸形 stdout 行并让请求持续 pending](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-102：InstructionsLoaded 外部 hook 缺少兼容输入必需字段](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-103：工具生命周期 hook 缺少调用 ID 与真实工具结果](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-104：Claude 兼容 hook 公共输入缺少 transcript_path 和 cwd](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-105：Stop hook 缺少必需的 stop_hook_active 字段](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-106：StopFailure hook 缺少必需的 error 分类](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-107：续聊的每个 turn 结束都会触发 SessionEnd hook](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-108：SessionStart/SessionEnd payload 缺少必需的事件元数据](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-109：SubagentStop hook 缺少必需的停止状态与 transcript 路径](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-110：PreCompact/PostCompact payload 不符合事件输入 schema](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-111：PreCompact hook 的 stdout 指令没有进入摘要请求](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-112：工作树生命周期 hook 缺少事件专属字段](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-113：FileChanged hook 字段名与变化类型不符合 schema](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-114：后台子 Agent Notification hook 缺少必需的通知类别](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-115：PermissionDenied hook 丢失工具调用关联 ID](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-116：步数上限终止的失败任务仍触发 TaskCompleted hook](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-117：API 错误终止未派发 StopFailure，反而只在完成被拒绝时触发](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-118：Responses 流式失败丢弃供应商错误码，上下文溢出恢复依赖消息措辞（已修复）](system-harness-audit-2026-10-21.md) | P2 | 历史已修复跳过 | 原报告明确记录修复实现与修复后验收 | 保留现有实现，不重复修复 | 历史命令与结果见原项；本轮未重跑，不声称重新验收 | 历史提交未在本轮重新定位，不猜填 hash | 保留原项未验限制；本阶段不另估修复工时 |
| [H-119：视频状态 HTTP 等待不响应运行取消，停止已受理仍需等待供应商返回（已修复）](system-harness-audit-2026-10-21.md) | P1 | 历史已修复跳过 | 原报告明确记录修复实现与修复后验收 | 保留现有实现，不重复修复 | 历史命令与结果见原项；本轮未重跑，不声称重新验收 | 历史提交未在本轮重新定位，不猜填 hash | 保留原项未验限制；本阶段不另估修复工时 |
| [H-120：WorkflowAgent 的节点 publisher 绕过持久化事件通道，实时状态与输出无法从 run_events 重放（已修复）](system-harness-audit-2026-10-21.md) | P1 | 历史已修复跳过 | 原报告明确记录修复实现与修复后验收 | 保留现有实现，不重复修复 | 历史命令与结果见原项；本轮未重跑，不声称重新验收 | 历史提交未在本轮重新定位，不猜填 hash | 保留原项未验限制；本阶段不另估修复工时 |
| [H-121：前端忽略视频进度事件的 job_id，完成前停止后重跑丢失续跑身份（已修复）](system-harness-audit-2026-10-21.md) | P1 | 历史已修复跳过 | 原报告明确记录修复实现与修复后验收 | 保留现有实现，不重复修复 | 历史命令与结果见原项；本轮未重跑，不声称重新验收 | 历史提交未在本轮重新定位，不猜填 hash | 保留原项未验限制；本阶段不另估修复工时 |
| [H-122：视频生成成功后修改提示词仍隐式续跑旧任务，新提示词没有发给供应商却再次成功](system-harness-audit-2026-10-21.md) | P1 | 历史已修复跳过 | 原报告明确记录修复实现与修复后验收 | 保留现有实现，不重复修复 | 历史命令与结果见原项；本轮未重跑，不声称重新验收 | 历史提交未在本轮重新定位，不猜填 hash | 保留原项未验限制；本阶段不另估修复工时 |
| [H-123：本地与 SSH 终端的分片读取推进到缓存末尾，未返回的输出被游标跳过](system-harness-audit-2026-10-21.md) | P2 | 历史已修复跳过 | 原报告明确记录修复实现与修复后验收 | 保留现有实现，不重复修复 | 历史命令与结果见原项；本轮未重跑，不声称重新验收 | 历史提交未在本轮重新定位，不猜填 hash | 保留原项未验限制；本阶段不另估修复工时 |
| [H-124：系统视觉预处理未纳入运行生命周期，点击停止后仍处理附件并提交主模型（已修复）](system-harness-audit-2026-10-21.md) | P1 | 历史已修复跳过 | 原报告明确记录修复实现与修复后验收 | 保留现有实现，不重复修复 | 历史命令与结果见原项；本轮未重跑，不声称重新验收 | 历史提交未在本轮重新定位，不猜填 hash | 保留原项未验限制；本阶段不另估修复工时 |
| [H-125：SSH 客户端不验证远端主机密钥，连接到未信任服务器仍成功](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-126：MCP stdio 与 HTTP transport 在完整读取/解析前没有响应字节上限](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-127：MCP image 工具结果被降格成 Responses 普通 JSON 字符串，模型收不到图像输入](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-128：停止运行后活动 MCP 工具仍等待响应，取消信号没有到达 MCP adapter/transport](system-harness-audit-2026-10-21.md) | P1 | 历史已修复跳过 | 原报告明确记录修复实现与修复后验收 | 保留现有实现，不重复修复 | 历史命令与结果见原项；本轮未重跑，不声称重新验收 | 历史提交未在本轮重新定位，不猜填 hash | 保留原项未验限制；本阶段不另估修复工时 |
| [H-129：设置页的一次性 MCP 测试连接没有启动截止时间（已修复）](system-harness-audit-2026-10-21.md) | P2 | 历史已修复跳过 | 原报告明确记录修复实现与修复后验收 | 保留现有实现，不重复修复 | 历史命令与结果见原项；本轮未重跑，不声称重新验收 | 历史提交未在本轮重新定位，不猜填 hash | 保留原项未验限制；本阶段不另估修复工时 |
| [H-130：Harness `turn/steer` 可从同一个已替代 turn 重复派生多个运行](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-131：固定总时限会截断仍在持续输出的 provider stream](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-132：Chat Completions 的合法内容过滤终态被当作未知 provider 错误，usage 未进入运行账本](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-133：Responses `response.failed` 携带 usage 时未进入运行用量](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-134：被替代的画布 run 迟到节点事件可覆盖当前状态](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-135：stdio MCP 隐式继承宿主凭据，配置环境不是授权边界](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-136：stdio MCP 请求写入受背压阻塞时，停止信号无法结束发送等待](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-137：MCP 工具目录变更通知被丢弃，后续运行继续发布旧工具](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-138：MCP 结果整对象进入模型上下文，客户端元数据没有独立可见性边界](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-139：后置 hook 报错跳过已执行工具的结果提交，重开后成功写入变成结果缺失](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-140：输入 hook 成功输出的上下文被丢弃，主模型只收到原始输入](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-141：周期 todo reminder 在独立用户回合之间被重置，长期会话永不触发](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-142：运行事件持久化失败被吞掉，实时事件与回放事件发生分叉](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-143：终态持久化失败前已经发布实时完成事件](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-144：DeepSeek Chat 合法终态 `aborted` / `insufficient_system_resource` 被当作未知错误](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-145：Thinking 模式下不支持的 Chat `tool_choice` 组合未在请求前拒绝](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-146：DeepSeek Chat 用户隔离字段被序列化为 Responses 的 `user`](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-147：DeepSeek 工具 schema 名称未按 provider 约束校验，非法、超长和重复名称直接越过请求边界](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-148：Responses 工具调用的空 `call_id` 未在请求边界拒绝](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-149：Responses 重复 `call_id` 被静默去重并跨过 provider boundary](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-150：Responses `input` 与 `instructions` 同时为空仍发送请求](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-151：Responses malformed function call 被填充默认身份并继续提交](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-152：官方 `tool_search_call` 在 Responses adapter 中被静默丢弃](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-153：Responses 规范思考档位贯通缺陷（规范值链路已修复；别名见 H-155）](system-harness-audit-2026-10-21.md) | P2 | 历史已修复跳过 | 原项规范值链已有修复/验收；别名缺口仍属 H-155 | 保留现有实现，不重复修复 | 历史命令与结果见原项；本轮未重跑，不声称重新验收 | 历史提交未在本轮重新定位，不猜填 hash | 保留原项未验限制；本阶段不另估修复工时 |
| [H-154：Responses `user` 只检查非空，非法标识符进入真实 provider 请求（已修复）](system-harness-audit-2026-10-21.md) | P2 | 历史已修复跳过 | 原报告明确记录修复实现与修复后验收 | 保留现有实现，不重复修复 | 历史命令与结果见原项；本轮未重跑，不声称重新验收 | 历史提交未在本轮重新定位，不猜填 hash | 保留原项未验限制；本阶段不另估修复工时 |
| [H-155：Responses 设置拒绝官方接受的 reasoning effort 兼容别名](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-156：同名扩展工具可能误入 `apply_patch` custom 序列化分支](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-157：模型目录丢弃 model-level reasoning effort，Responses 请求不校验所选模型支持档位](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-158：Responses named `tool_choice` 接受官方不支持的 `custom` / `web_search` 类型并进入 wire](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-159：MCP 非法 `inputSchema` 在注册与发布阶段未校验，作为 function `parameters` 进入每次模型请求](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-160：Canvas gateway 把非法显式模型引用解释为自动路由，修通专业节点后可静默改用另一供应商](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-161：画布 LLM 节点的温度和输出上限在 provider adapter 前被丢弃](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-162：前端标为可用的四类专业 AI 节点没有可达的正式模型执行链](system-harness-audit-2026-10-21.md) | P0 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-163：工作流模型调用丢失 DeepSeek usage，成本账本、缓存观测和 token budget 与实际请求分裂](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-164：画布文本请求不在运行取消树内，节点完成态可与运行取消终态分裂](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-165：图片供应商可令宿主下载任意私网 URL，并把响应作为成功 artifact 保存](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-166：DeepSeek Responses 思考模式下 top_p 会被忽略或规范化，设置值不等于有效采样值](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-167：画布图片下载无响应字节上限，超预算数据被完整缓冲并持久化](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-168：易变 instructions 位于历史之前，动态变化截断多轮 KV cache 前缀](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-169：桌面接受任意 `image/*`，不支持的 SVG 被原样发送给 DeepSeek Responses](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-170：历史图片每轮重发完整 base64，未接入 DeepSeek Files API 的持久引用](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-171：Responses 图片角色沿用 Chat 规则，合法 `developer` 图丢失且非法 `assistant` 图静默成功](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-172：Responses 数值参数只在设置层校验，公开 ModelClient 仍发送越界值](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-173：Responses 工具输出图片无法保留，公共消息边界静默降为纯文本](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-174：公开 `replace_messages` 绕过 Responses 工具调用配对修复](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-175：工作流 `http-request` 节点绕过统一网络权限与审批边界](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-176：工作流 `knowledgeBaseId` 是死字段，知识检索节点无条件混合 project/global vault](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-177：未知 `target_node_id` 被当作空工作流成功，重跑/恢复目标没有 admission 校验](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-178：专业画布 `code` 节点标记为 enabled，但脚本从未执行且错误被伪造成成功](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-179：专业画布 `document-extractor` 标记为 enabled，但正式运行没有 executor](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-180：专业画布 `human-input` 节点不进入统一人工输入/审批生命周期](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-181：专业画布 `iteration` 不消费前端输入数组，也没有真正调度迭代体](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-182：专业画布 `answer` 节点忽略 `answerTemplate`，成功运行却输出空答案](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-183：专业画布 `variable-aggregator` 忽略 `aggregatorVariables`，成功运行却输出空数组](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-184：工作流 `start` 的必填输入缺少 admission 校验，缺失值被静默填成 null](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-185：工作流 `start` 不校验输入类型，声明 number 却接受任意字符串](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-186：工作流 `start` 没有输入 allowlist，未声明字段进入执行上下文并成功传播](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-187：工作流声明的输入默认值被静默丢弃，缺失字段变成 `null` 仍成功](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-188：工作流字符串 `options` 不参与 admission，非法枚举值成功进入执行上下文](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-189：工作流 `minimum/maximum` 不参与 admission，越界数值成功进入执行上下文](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-190：工作流 `minLength/maxLength` 不参与 admission，越界字符串成功传播](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-191：工作流图没有入口节点仍可成功完成，结构 admission 缺少 start/end 不变量](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-192：工作流图没有 `end` 节点仍以成功终态结束](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-193：start 与 end 彼此不可达时仍按全图拓扑执行并成功](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-194：拓扑等价图仅改变节点数组顺序，执行范围便发生变化](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-195：多个可达 end 没有 reject/join/choice 语义，第一个 end 截断正常分支](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-196：合法 target-node 运行跳过必需祖先，正常节点因缺少中间输出失败](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-197：end 的输出类型声明只校验类型名称，成功结果可违反公开契约](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-198：OR 条件组从 true 开始累加，所有子条件为 false 仍选择 true 分支](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-199：多 case 条件删除 false 端口，但 executor 无匹配时仍返回 false](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-200：Responses 生命周期事件丢弃响应身份、服务端模型与序号，durable run 无法关联完整流](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-201：工作流画布缩放控件改变 zoom 却保留平移偏移，视口焦点随每次缩放跳动](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-202：工作流图持久化不包含视口，重启后用户的画布导航状态无法恢复](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-203：Responses `message` 的公开字符串角色缺少 provider 边界校验](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-204：Harness SDK 对无响应但未断开的 RPC 无期限等待，调用取消后 pending 不回收](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-205：Rust Harness SDK 丢弃通知外层 `eventSequence`，无法可靠做 ACK 或缺口恢复](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-206：Harness SDK 接受协议版本不兼容的 initialize 结果并进入已初始化状态](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-207：Harness SDK 对 `thread/start` 成功结果缺少必填 `threadId` 校验](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-208：Responses `response.completed` 缺少必填 response 字段仍被当作成功](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-209：Responses 输出 `message` item 缺少 `role` 仍被静默补成 assistant](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-210：Responses 流非单调 `sequence_number` 被静默接受](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-211：Harness SDK 将字符串 JSON-RPC response ID 强制折叠为整数，导致请求无法关联](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-212：Harness SDK 未校验 JSON-RPC response 的协议版本](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-213：Harness SDK 忽略 notification method，任意通知可伪装成 Harness 事件](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-214：Harness SDK 请求返回后才订阅会永久丢失先于 response 到达的合法通知](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-215：Harness 归档状态未进入活动列表与运行接纳，已归档 thread 仍可创建 turn](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-216：Harness 冷分叉返回成功但新 thread 未加载，首个 `turn/start` 被拒绝](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-217：未知 namespaced JSON-RPC method 被误报为 Invalid params](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-218：`thread/read` 无界全量物化历史，缺少分页与响应预算](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-219：`tool/list` 是无作用域的静态目录，无法描述当前 run 的实际能力](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-220：`thread/list` 无界返回全部 session，缺少列表分页与服务端过滤](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-221：`config/read` 只返回静态协议 banner，无法读取有效执行配置](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-222：`thread/start` 的 `reasoningEffort` 被静默丢弃，后续 turn 回退到默认思考配置](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-223：刷新相同模型目录会覆盖仍有效的用户选型，下一次 Responses 请求静默换模型](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-224：日/月费用使用错误时间桶，真实预算接纳漏计本期历史费用后继续请求模型](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-225：首次登录只持久化报价，未激活既有 CostService，正常模型有用量却无费用且预算继续放行](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-226：备用模型切换后仍按初始模型结算，完整 usage 被错误归价且下一轮预算误放行或误拒绝](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-227：日/月限额仅驻留内存，宿主服务重建后历史费用保留但预算静默变为无限](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-228：会话搜索在可见性过滤前截断候选，隐藏记录可挤掉全部有效历史](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-229：单会话文件损坏经全库搜索修复阻断无关项目搜索与数据库启动](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-230：单文件验证只按扩展名选择严格 JSON，合法 TypeScript 配置被误报并在 Strict 模式记为失败动作](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-231：可选 ruff 未安装时真实 shell 返回非零结果，被 Python 验证器误报为源代码失败](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-232：回退删除发现事件后，原宿主仍公开旧工具，重建宿主则隐藏](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-233：无 hooks 的公共 Runtime 将并发安全等同于可丢弃预执行，失败响应留下未记录的 Todo 状态](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-234：声明 isolated/read-only 的 agent-hook 共享主运行 TodoStore，正常完成仍替换父任务列表](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-235：模型型生命周期 hook 的成功 usage 未交回会话账本，漏计费用后下一轮预算仍放行](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-236：hook 缺少父运行取消上下文，工具流水线仍等待，MCP 对端未获取消](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-237：agent-hook 未继承主运行有效权限，被规则或工作区边界拒绝的读取仍将正文交给 hook 模型](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-238：agent-hook 与主模型共享“已读”状态，首次读取变空桩且写前凭证串用](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-239：hook 去重忽略条件与模型，不适用的声明可吞掉实际校验器](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-240：一次性 hook 被不匹配的工具提前消耗，首次实际写入没有经过校验](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-241：普通 task 的取消操作遗漏会话归属校验，另一会话可停止不属于自己的后台子 Agent](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-242：子 Agent 的 resume 用状态摘要重启空会话，完整已保存结果及工具观察未恢复](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-243：原生 compaction 移除 memory 摘录后，memory 去重状态未释放，后续检索无法重新注入](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-244：CanvasWorkflowStore 接受陈旧完整图快照，两个窗口的后写会静默丢失先写编辑](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-245：画布偏好读取失败后仍完成 hydration，后续 workspace 设置写回会覆盖未读字段](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-246：画布偏好 hydration 完成后会覆盖读取期间的用户编辑](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-247：画布 legacy 偏好迁移在后端写入失败时先删源数据，导致不可重试丢失](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-248：画布语音合成响应无字节上限，超预算音频被完整缓冲并持久化](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-249：画布语音转写的远程音频输入绕过统一网络权限与审批](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-250：画布 artifact 读取不校验持久化摘要，旧 URI 会静默投影替换后的字节](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-251：命名画布工作流保存了 workspace_id，但库查询、读取和删除没有 workspace 访问边界](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-252：Canvas artifact 的读取、provider 投影、显示和删除按全局 ID 操作，绕过 workspace 隔离](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-253：分块 canvas artifact 上传丢失 workspace 所有权，发布出无主媒体](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-254：artifact 删除先删文件后删索引，数据库失败会留下永久 dangling URI](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-255：命名工作流 ID 可占用 `current:<mode>:<workspace>`，覆盖或删除 live canvas slot](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-256：SDK reader 已确认 EOF，事件订阅仍无期限等待，断线后新订阅也无法得知生产者消失](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-257：SSE 把确定非法的 UTF-8 当成未收齐字符，后续字节持续滞留且错误被误报为缺少终态](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-258：CLI TUI 忽略模型 attempt 回滚，重试成功后仍把废弃答案和推理拼入最终显示](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-259：流式预处理缓存覆盖最终工具参数，实际写入与原生调用历史及续轮上下文发生错配](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-260：上下文压缩清掉模型可见文件结果后，旧 FileStateCache 仍授权覆盖已有文件](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-261：Responses 历史中乱序工具结果被 repair 静默替换为合成失败](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-262：附件身份路径边界（H-58 的重复登记）](system-harness-audit-2026-10-21.md) | — | 归并 | 唯一有效项 H-58 | 只关联原证据，不新增修复任务 | 验收由 H-58 统一记录 | 无独立修复提交 | 不独立计数或估时 |
| [H-263：实时持久化分叉重复登记（已撤销）](system-harness-audit-2026-10-21.md) | — | 撤销 | 原报告 H-265 明确撤销；Git b1912f53 为撤回提交 | 不作为有效缺陷或修复成果 | 不重复运行已撤销探针 | 撤销证据：b1912f53 | 不独立计数或估时；编号不复用 |
| [H-264：MCP 响应无界缓冲（H-126 的重复登记）](system-harness-audit-2026-10-21.md) | — | 归并 | 唯一有效项 H-126 | 只关联原证据，不新增修复任务 | 验收由 H-126 统一记录 | 无独立修复提交 | 不独立计数或估时 |
| [H-265：TypeScript SDK 自动替换已退出进程，却沿用旧握手状态，新进程首条业务请求被拒绝](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-266：TypeScript SDK 的 CLI JSONL 迭代忽略非零退出，部分输出或空输出均无错误结束](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-267：TypeScript SDK 的 CLI JSONL 缺少进程 owner 收尾，停止迭代、解析失败和 close 后仍执行后台工作](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-268：结构化摘要接受空对象与无关 JSON，绕过降级后持续遮蔽原历史](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-269：摘要草稿清理在解析 JSON 前删除字符串内的合法 analysis 标签及正文](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-270：一条计划记录解码失败使全部健康任务不可见，正常创建随后覆盖并删除旧任务](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-271：正常创建的日历不可能计划触发日期搜索 panic，终止整个调度任务且重建后再次触发](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-272：日/月限额只有 run 准入保护，已取得越限用量后正常工具循环仍继续请求模型](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-273：Responses cached_tokens 超过 input_tokens 仍被接受并按低价记账](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-274：Responses token 计数超过宿主 u32 宽度后静默截断，账本可被压成零](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-275：MCP hook 直接调用绕过 discovered inputSchema，非法参数仍发送到工具](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-276：MCP hook 在解析前截断完整拒绝 JSON，超过展示上限后静默放行真实写入](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-277：MCP initialize 结果未校验，缺失必填字段或协商不支持版本仍发布 Ready 并执行工具](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-278：MCP 未知结果重放的真实进程补强（归 H-06）](system-harness-audit-2026-10-21.md) | — | 归并 | 唯一有效项 H-06 | 只关联原证据，不新增修复任务 | 验收由 H-06 统一记录 | 无独立修复提交 | 不独立计数或估时 |
| [H-279：junction 物理路径边界重复登记（归 H-78）](system-harness-audit-2026-10-21.md) | — | 归并 | 唯一有效项 H-78 | 只关联原证据，不新增修复任务 | 验收由 H-78 统一记录 | 无独立修复提交 | 不独立计数或估时 |
| [H-280：敏感路径发现重复登记（归 H-79）](system-harness-audit-2026-10-21.md) | — | 归并 | 唯一有效项 H-79 | 只关联原证据，不新增修复任务 | 验收由 H-79 统一记录 | 无独立修复提交 | 不独立计数或估时 |
| [H-281：中间 system serializer 补强（归 H-168）](system-harness-audit-2026-10-21.md) | — | 归并 | 唯一有效项 H-168 | 只关联原证据，不新增修复任务 | 验收由 H-168 统一记录 | 无独立修复提交 | 不独立计数或估时 |
| [H-282：Git 状态整体 trim 破坏首条未暂存路径，正式模型上下文可把脏工作区报告为 clean](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-283：Read deny 只拦工具，自动工作区上下文仍在首次请求发送 README 与清单摘要](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-284：同一 Harness thread 的第二个 `turn/start` 被确认成独立 started，实际输入租约随后取消第一个运行](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-285：Harness 接受不存在的分叉锚点，超出有符号范围后恢复旧文件却复制完整成功历史](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-286：`move_path` 覆盖目标前先递归删除，移到源的祖先目录时失败却删除源和无关文件](system-harness-audit-2026-10-21.md) | P1 | 已修复并验收 | 原正式探针修复前 1 passed / 1 failed；修复后默认 2 passed | 原生 no-replace 发布；旧目标 staging/失败恢复；失败恢复保留 recovery_path | 详见本文件 H-286 命令表：builtins 292 passed/1 ignored，AppCore 正式链 102 passed/92 ignored；全量/lint 限制保留 | `56b9e7b9` | Windows 本机范围验收；其他 OS、真实跨卷/断电/UI 未验，不等于全局通过；本项本地剩余工时 0，环境验收另计 |
| [H-287：Windows 内容原子替换丢失目标文件 DACL，成功编辑及检查点恢复均未保留原访问控制策略](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-288：计划模式只存在服务内存，恢复同一会话后静默失去写入限制](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-289：CodeGraph 定义文件变更后不会重新连接未修改 caller 的调用边](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-290：画布供应商保存先提交配置、后写密钥，密钥持久化失败后发布新配置与旧凭据的混合身份](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-291：重复编号撤销，归并 H-126（2026-10-25 更正）](system-harness-audit-2026-10-21.md) | — | 归并 | 唯一有效项 H-126 | 只关联原证据，不新增修复任务 | 验收由 H-126 统一记录 | 无独立修复提交 | 不独立计数或估时 |
| [H-292：画布场景声明撤销后旧绑定仍参与实际调用，非瞬时失败阻断健康候选](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-293：共享终端租约过期后真实 PTY 存活但没有续租、接管或清理路径](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-294：完成门把每次成功文件操作强制转换为最终净变化，临时创建后删除被误判未完成并额外请求模型](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-295：TypeScript SDK 以客户端全局审批屏障结束单个 turn，另一会话的待答审批会阻塞已完成流](system-harness-audit-2026-10-21.md) | P2 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-296：模型能力解析以 context_window 是否存在决定整份元数据来源，单独公布的输出上限被 fallback 覆盖](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-297：默认字符密度计数低估官方离线文本，冷计量可漏过既定压缩压力阈值](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-298：Harness 超出存储表示域的读取游标回绕，错误重放全部旧 run events](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-299：桌面端工具循环的实时用量与会话重载显示不一致](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-300：Embedding 工作流执行器存在，但画布契约/图注册表没有注册 embedding 节点](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-301：Audio 合成取消没有打断共享 Canvas HTTP 响应](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-302：画布视频下载没有类型级字节上限](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-303：画布视频 provider URL 第二跳绕过网络审批](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-304：Canvas provider 非 2xx 错误正文在分类前无界缓冲](system-harness-audit-2026-10-21.md) | P1 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
| [H-305 撤销：SDK Drop 行为符合已有文档，原探针不能证明独立缺陷](system-harness-audit-2026-10-21.md) | — | 撤销 | f37da4fe 撤销误报；Drop 契约及探针隔离/清理不足 | 不作为有效缺陷或修复成果 | 不重复运行已撤销探针 | 撤销证据：f37da4fe | 不独立计数或估时；编号不复用 |
| [H-306：本地视频编辑的空默认提示词使公开画布提示词目录整体不可读](system-harness-audit-2026-10-21.md) | P3 | 待修复 | 无跳过依据；动手前复核当前代码 | 根因见原项；本阶段尚未实施 | 本轮未执行该项验收；原探针/限制见原文 | 无本阶段修复提交 | 按风险/依赖排队；逐项工时待评估，计入总区间 |
<!-- repair-ledger:end -->
