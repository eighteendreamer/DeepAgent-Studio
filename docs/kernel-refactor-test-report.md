# DeepAgent 内核重构（阶段 A–G）完整测试用例与结果报告

- **日期**：2026-07-28
- **环境**：Windows（PS5 `5.1.26100` / PS7 `7.6.3` / CMD `10.0.26200` / WSL2 `6.18.33.1-microsoft-standard-WSL2`）、Rust stable、特性组合 `web,runtimes,keychain`（桌面端等价）
- **覆盖范围**：阶段 A（工具装配收口）、B（配置统一）、C（上下文唯一入口）、D（检查点/崩溃恢复）、E（Hook/压缩/CompletionGate）、F（日志脱敏/旧链退役）、G（golden trace/故障注入/硬指标/Shell 矩阵）
- **总判定**：✅ **全部通过**（960+ 用例；2 例并行计时竞争经单线程复核通过，见 TC-R2）

---

## 一、执行方式（测试流程总览）

```powershell
# 单元/集成（按 crate）
cargo test -p deepagent-models   --lib
cargo test -p deepagent-runtime  --lib
cargo test -p deepagent-runtime  --test e2e_stabilization
cargo test -p deepagent-mcp      --lib
cargo test -p deepagent-context  --lib
cargo test -p deepagent-persistence --lib
cargo test -p deepagent-builtins --lib          # 计时敏感项见 TC-R2
cargo test -p deepagent-hooks    --lib          # 计时敏感项见 TC-R2
cargo test -p deepagent-app-core --features web,runtimes,keychain --lib
cargo test -p deepagent-app-core --features web,runtimes,keychain --test kernel_v2_e2e
cargo test -p deepagent-app-core --features web,runtimes,keychain --test golden_trace
# 构建/静态
cargo check --manifest-path apps/desktop/src-tauri/Cargo.toml
cd apps/desktop && npx tsc --noEmit
cargo fmt --all -- --check
```

真实工作区 e2e fixture 统一创建于 `G:\Code\Kotlin_code\_deepagent-e2e\<run-id>` 并在测试尾部清理（未触碰 `del_dir.py` 与 `新建文件夹 (3)`）。

---

## 二、分阶段测试用例

### TC-A 工具装配三层收口（阶段 A）

| 项 | 内容 |
|---|---|
| 覆盖修改 | `tool_runtime.rs` 单入口 `build_main_run_toolset`（base→MCP→task→knowledge_write→plan→skill→manifest 固定序）；`subagent_runner.rs` 拆分（1,065 行迁出）；`base_registry_request` 共享构建 |
| 测试流程 | app-core lib 中 `tool_search`(5)/`skill`(49)/`mcp`(25)/`subagent` 过滤集 + `kernel_v2_e2e` 全量（子代理同步/后台/worktree/恢复经新装配路径） |
| 预期 | 注册顺序字节等价；子代理不可递归（隔离注册表无 `task`）；全部既有行为回归通过 |
| 结果 | ✅ app-core lib **477 passed / 0 failed / 1 ignored**；kernel_v2_e2e **9 passed** |

### TC-B 配置统一入口（阶段 B）

| 项 | 内容 |
|---|---|
| 覆盖修改 | `DualConfigLoader` 托管层（`managed-settings.json` + `managed-settings.d/` 字母序 drop-in）+ `ConfigLayer` 分层输出；`merged_permission_rules`（多来源并集 + 全局 deny>ask>allow + 托管至上）；`model_override`；hooks 分层追加聚合 |
| 测试流程 | context lib `config_overlay` 2 例；app-core `run_config` 6 例（含新增 3 例） |
| 预期 | 同层 `.deepagent` 压 `.claude`；drop-in 覆盖托管 base 且全压低层；项目 deny 不可撤销托管 allow、托管 deny 压项目 allow；`model` 标量经优先级链生效 |
| 结果 | ✅ context lib **37 passed**；关键用例：`managed_dir_base_and_drop_ins_win_over_all_lower_scopes`、`layered_rules_union_across_scopes_with_deny_over_ask_over_allow`、`managed_rules_are_supreme_over_lower_scopes`、`model_override_reads_trimmed_scalar_via_precedence_chain` 全过 |

### TC-C 上下文唯一入口 + 嵌套指令发现（阶段 C）

| 项 | 内容 |
|---|---|
| 覆盖修改 | `NestedInstructionsDecorator`（文件工具触达目录时懒发现 `CLAUDE.md`/`AGENTS.md`，浅→深注入、去重、24KB/3 文件上限、越界不爬树）+ `InstructionsLoaded` hook 派发 |
| 测试流程 | app-core lib `nested_instructions` 4 例 |
| 预期 | 首次触达注入且父目录先于子目录；同 run 不重复；根 manifest 已载文件不再注入；失败工具/非触发工具/根级文件忽略；相对路径按 workspace root 解析 |
| 结果 | ✅ 4/4：`discovers_nested_instructions_once_per_run`、`root_manifest_paths_are_never_reinjected`、`workspace_root_files_and_failed_tools_are_ignored`、`relative_paths_resolve_against_workspace_root` |

### TC-D 检查点崩溃一致性 + 配对重放（阶段 D）

| 项 | 内容 |
|---|---|
| 覆盖修改 | `capture_before` 增量落库（崩溃后 manifest 可查）；子代理检查点继承父 `session_sequence`；`conversation_with_tool_pairs_from_events`（配对重建 + 孤儿合成失败 + 2000 字符截断） |
| 测试流程 | runtime lib `checkpoint` 5 例；app-core lib `input_runtime` 6 例；kernel_v2_e2e 写任务/删除证据/worktree 用例 |
| 预期 | drop 不 commit 仍可 restore（模拟崩溃）；rewind 还原 改/删/建 三态；重放绝无悬空 tool_use/tool_result；超大结果有界 |
| 结果 | ✅ 全过，关键：`incremental_commit_survives_crash_without_final_commit`、`restore_reverts_modified_deleted_and_created_files`、`tool_pair_replay_synthesizes_failures_for_orphaned_calls`、`tool_pair_replay_rebuilds_batches_and_results`、`tool_pair_replay_bounds_oversized_results` |

### TC-E Hook 全派发 + 压缩闭环 + CompletionGate（阶段 E）

| 项 | 内容 |
|---|---|
| 覆盖修改 | `Notification`/`CwdChanged`/`BeforePlan` 补派发（26 个 HookPoint 全接线）；设置页测试支持 5 种 hook 类型/23 事件（`test_hook_action` 复用生产 dispatch）；micro-compact 第一阶段 + 重注入块（修改文件/失败检查/Skills）+ `CompactionBreaker`（15s 冷却/2 次无效熔断）；`completion_plan` 自动 build/test 验收计划 |
| 测试流程 | app-core lib `context_runtime` 5 新例 + `completion_plan` 5 例 + 既有 hook 套件；runtime loop_engine completion/stop 套件 |
| 预期 | micro-compact 只清旧大工具结果、保尾窗与配对；重注入块含文件/失败/Skills；断路器冷却+熔断；读任务/非代码写任务不触发构建计划，Rust→cargo check、TS→tsc |
| 结果 | ✅ 全过，关键：`micro_compact_clears_only_old_large_tool_results`、`reinjection_block_carries_files_failures_and_skills`、`compaction_breaker_cooldown_and_trip`、`rust_code_task_gets_cargo_check_plan`、`read_only_prompts_produce_no_plan` |

### TC-F 日志递归脱敏 + 旧链退役（阶段 F）

| 项 | 内容 |
|---|---|
| 覆盖修改 | `runtime::redaction`（run_events/子代理事件/run_terminal 三个落库口仅密钥脱敏、正文完整）；删除 `run_chat` 命令（111 行），前端 `runChat` 包装内切 `start_chat_v2`+`session://completed` |
| 测试流程 | runtime lib `redaction` 2 例；前端 `tsc --noEmit`；Tauri check；grep 确认 `run_chat` 零生产残留 |
| 预期 | 秘密键/Bearer/sk- 递归清除；普通文本字节不变（重放完整性）；前端类型 0 错误；唯一主链无双执行 |
| 结果 | ✅ `scrubs_secret_keys_and_literals_recursively`、`ordinary_text_passes_through_unchanged` 过；tsc **0 错误**；Tauri check 通过 |

### TC-G1 Golden Trace 状态机契约（阶段 G）

| 项 | 内容 |
|---|---|
| 测试流程 | `tests/golden_trace.rs`：真实 `run_in_session` → `run_events` 投影为 `phase:event_type` 序列 → 与仓库内 fixture 逐行对比 |
| 预期 | 简单问答固定为 7 事件序列：`accepted:run_accepted → preparing:run_started → preparing:session_registered → running_turn:turn_started → verifying:completion_evidence → finalizing:run_completed → terminal:run_terminal`；工具往返 run 中 `tool_started` 先于 `tool_completed` 且以 `terminal:run_terminal` 收尾 |
| 结果 | ✅ 3/3（含 TC-G3 的取消用例共用文件） |

### TC-G2 故障注入矩阵（阶段 G，12 场景）

| 场景 | 用例 | 预期 | 结果 |
|---|---|---|---|
| SSE 空响应 | `retries_empty_200_stream_without_recording_failed_turn` | 静默重试，不记失败轮 | ✅ |
| 部分响应断流 | `retry_resets_visible_deltas_from_failed_attempt` | 发 AttemptReset，丢弃可见增量 | ✅ |
| 429 限流 | `rate_limited_429_retries_and_recovers_within_budget`（新） | 同模型退避重试恢复，不切 fallback | ✅ |
| 503 服务端 | `server_503_retries_then_fails_terminally_when_budget_exhausted`（新） | 恰好 max_attempts 次后干净终态失败 | ✅ |
| 529 过载 | `overload_switches_once_to_configured_fallback_model` | 切换一次 fallback 模型 | ✅ |
| max output | `max_output_retries_same_turn_with_larger_limit_and_aborts_old_tools` | 放大 max_tokens 同轮重试 + 中止旧工具尝试 | ✅ |
| context overflow | `context_overflow_compacts_once_then_retries_clean_request` + 断路器用例 | 压缩一次重试；重复溢出熔断 | ✅ |
| 畸形 tool_calls JSON | `malformed_tool_call_arguments_do_not_panic_and_yield_a_decision`（新，**修复真实缺陷**：原行为整 run 失败并孤儿化同批调用，现降级哨兵→pipeline 校验层拦截→配对失败反馈重试） | 不 panic、不孤儿、可继续 | ✅ |
| 无效工具 schema | `invalid_schema_never_runs_hooks_permissions_or_tool` | 不跑 Hook/权限/进程 | ✅ |
| Hook 超时/进程挂起 | `system_runner_timeout_terminates_process_tree_promptly` + `system_executor_times_out_and_kills_running_process` | 2s 内杀进程树 | ✅（串行，见 TC-R2） |
| **MCP 运行中掉线** | `adapter_fails_fast_when_server_dies_mid_run`（新） | 首调成功→掉线后 1s 内结构化失败，不悬挂 | ✅ |
| **子代理失败传播** | `failed_subagent_propagates_paired_failure_and_parent_still_terminates`（新） | 子 503 终态失败 → 父收 `subagent_completed{failed}`、task 调用配对 `ok=false`、父仍 `succeeded` 终态、无 running 残留 | ✅ |

### TC-G3 硬指标（阶段 G）

| 指标 | 用例 | 结果 |
|---|---|---|
| 取消 200ms 内确认 | `cancel_request_acknowledged_within_200ms`（模型流挂起中） | ✅ |
| 取消后 2s 内模型流终态 | `cancel_by_run_id_stops_model_stream_and_reaches_terminal_under_two_seconds` | ✅ |
| 进程取消/超时 2s 内杀树 | `system_executor_cancels_running_process_within_two_seconds` 等 | ✅（串行） |
| 终态不变量（无缺口序列/唯一 terminal/工具双向配对/无 running 子代理） | `assert_terminal_invariants`（挂 3 个 e2e 尾部） | ✅ |

### TC-G4 Shell 矩阵（本机实测）

| Shell | 版本 | 执行验证 | 结果 |
|---|---|---|---|
| PowerShell 5 | 5.1.26100.8875 | SystemExecutor 集成测试（Unicode 路径/长脚本临时文件/取消/超时） | ✅ |
| PowerShell 7 | 7.6.3 | `pwsh` 可用（`powershell_executable()` 优先选择） | ✅ |
| CMD | 10.0.26200.8875 | `system_executor_cmd_deletes_quoted_unicode_path` | ✅ |
| WSL2 | 6.18.33.1-WSL2 | `wsl.exe -e sh -c` 执行验证（`wsl-ok`） | ✅ |

### TC-R1 稳定性回归（既有基线）

`e2e_stabilization`：600 次工具调用一致性 / 崩溃后事件日志精确重建 / 25 会话并发无污染 / 序列无缺口 / 10 次恢复幂等 —— ✅ **5 passed**。

### TC-R2 已知偏差（如实记录）

`system_executor_times_out_and_kills_running_process` 与 `system_runner_timeout_terminates_process_tree_promptly` 在**全 crate 并行**跑时偶发超 2s 断言（209/51 个测试同时抢占 pwsh 冷启动）。`--test-threads=1` 串行复跑 **6/6、3/3 全过**（3.80s/2.86s）。判定：测试环境竞争，非产品回归；生产路径单进程无此竞争。

---

## 三、结果汇总

| 套件 | 结果 |
|---|---|
| deepagent-models --lib | ok. **50 passed; 0 failed** |
| deepagent-runtime --lib | ok. **102 passed; 0 failed** |
| deepagent-runtime e2e_stabilization | ok. **5 passed; 0 failed** |
| deepagent-mcp --lib | ok. **36 passed; 0 failed** |
| deepagent-context --lib | ok. **37 passed; 0 failed** |
| deepagent-persistence --lib | ok. **31 passed; 0 failed** |
| deepagent-builtins --lib | **208 passed; 1 环境竞争**（串行复核 ✅，TC-R2） |
| deepagent-hooks --lib | **50 passed; 1 环境竞争**（串行复核 ✅，TC-R2） |
| deepagent-app-core --lib (web,runtimes,keychain) | ok. **477 passed; 0 failed; 1 ignored** |
| kernel_v2_e2e | ok. **9 passed; 0 failed** |
| golden_trace（新） | ok. **3 passed; 0 failed** |
| Tauri check | Finished ✅ |
| 前端 tsc --noEmit | **0 错误** |
| cargo fmt --all --check | **0 差异** |

**合计：1008 个用例通过，0 个产品缺陷级失败。**

---

## 四、输出日志（真实采集摘录）

### 4.1 Golden trace / 硬指标 / e2e

```text
test golden_trace_simple_answer_matches_fixture ... ok
test cancel_request_acknowledged_within_200ms ... ok
test golden_trace_tool_call_run_holds_invariants ... ok
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.08s

test cancel_by_run_id_stops_model_stream_and_reaches_terminal_under_two_seconds ... ok
test native_move_and_delete_are_verified_and_recorded_as_completion_evidence ... ok
test delete_task_cannot_succeed_without_deleted_path_evidence ... ok
test write_turn_reaches_terminal_with_checkpoint_and_real_side_effect ... ok
test background_subagent_outlives_parent_and_can_be_cancelled_independently ... ok
test subagent_run_persists_terminal_state_and_independent_transcript ... ok
test terminal_subagent_resumes_across_parent_runs_in_the_same_session ... ok
test worktree_subagent_rebinds_file_tools_without_touching_main_checkout ... ok
test failed_subagent_propagates_paired_failure_and_parent_still_terminates ... ok
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.07s
```

### 4.2 故障注入 / 检查点 / 脱敏（runtime lib 摘录）

```text
test model_agent::tests::rate_limited_429_retries_and_recovers_within_budget ... ok
test model_agent::tests::server_503_retries_then_fails_terminally_when_budget_exhausted ... ok
test model_agent::tests::malformed_tool_call_arguments_do_not_panic_and_yield_a_decision ... ok
test model_agent::tests::overload_switches_once_to_configured_fallback_model ... ok
test model_agent::tests::context_overflow_compacts_once_then_retries_clean_request ... ok
test model_agent::tests::repeated_context_overflow_trips_compaction_circuit_breaker ... ok
test model_agent::tests::max_output_retries_same_turn_with_larger_limit_and_aborts_old_tools ... ok
test model_agent::tests::retries_empty_200_stream_without_recording_failed_turn ... ok
test model_agent::tests::retry_resets_visible_deltas_from_failed_attempt ... ok
test model_agent::tests::aborts_failed_tool_attempt_and_commits_only_retry_calls ... ok
test model_agent::tests::kernel_feedback_without_call_id_is_not_an_orphan_tool_result ... ok
test checkpoint::tests::incremental_commit_survives_crash_without_final_commit ... ok
test checkpoint::tests::restore_reverts_modified_deleted_and_created_files ... ok
test checkpoint::tests::capture_rejects_workspace_escape ... ok
test redaction::tests::scrubs_secret_keys_and_literals_recursively ... ok
test redaction::tests::ordinary_text_passes_through_unchanged ... ok
test result: ok. 102 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.05s
```

### 4.3 套件级汇总行

```text
deepagent-models      test result: ok. 50 passed;  0 failed; finished in 0.02s
deepagent-runtime     test result: ok. 102 passed; 0 failed; finished in 1.05s
e2e_stabilization     test result: ok. 5 passed;   0 failed; finished in 0.16s
deepagent-mcp         test result: ok. 36 passed;  0 failed; finished in 0.00s
deepagent-context     test result: ok. 37 passed;  0 failed; finished in 0.02s
deepagent-persistence test result: ok. 31 passed;  0 failed; finished in 0.19s
deepagent-app-core    test result: ok. 477 passed; 0 failed; 1 ignored; finished in 1.51s
kernel_v2_e2e         test result: ok. 9 passed;   0 failed; finished in 1.07s
golden_trace          test result: ok. 3 passed;   0 failed; finished in 0.08s
builtins(串行复核)     test result: ok. 6 passed;   0 failed; finished in 3.80s
hooks(串行复核)        test result: ok. 3 passed;   0 failed; finished in 2.86s
Tauri check           Finished `dev` profile ... in 16.61s
前端 tsc --noEmit      0 errors
cargo fmt --check      0 diffs
```

### 4.4 Shell 矩阵检测原始输出

```text
PS5: 5.1.26100.8875
PS7: 7.6.3
CMD: Microsoft Windows [版本 10.0.26200.8875]
WSL exec: wsl-ok
Linux EIghteen 6.18.33.1-microsoft-standard-WSL2 #1 SMP PREEMPT_DYNAMIC ... x86_64 GNU/Linux
```

---

## 五、未纳入本报告的项（需真实环境）

1. 小/写任务各 20 次、复杂/大任务各 10 次成功率统计（需桌面端 + DeepSeek API key）
2. Java 多文件工程 javac 构建/重构/回归
3. 20–30 文件大任务 + 压缩触发 + 中断重启恢复全链路
4. UI 点停止的真实进程树 2 秒终止演练
5. 数据库升级/回滚（需历史版本 deepagent.db）
