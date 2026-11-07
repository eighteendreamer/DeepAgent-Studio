# 画布模型场景选择

模型的当前 `scenarios` 声明决定能否用于该场景；绑定只表示候选优先顺序，不能增加模型资格。
这是本地选型策略，不是供应商能力证明、沙箱权限或已发送请求的撤销机制。

## 修改场景后会发生什么

- 新请求先从当前 enabled provider/model 中建立声明了目标场景的候选集合，再应用全局或
  workspace 绑定顺序；剩余候选保持原 priority/provider-name 排序。
- 旧绑定、重复绑定、已删除身份或未知旧场景标签不能向候选集合添加模型。
  撤销后不会自动删除保存的绑定偏好；重新声明支持后，可恢复原有顺序。
- `binding.enabled = false` 只撤销该优先行，不等于禁用模型。模型若仍声明该场景且 enabled，
  可以作为未绑定候选参与；真正禁用应修改 provider/model enabled 或场景声明。
- 显式 `providerId::modelId` 只绕过优先排序，不绕过当前场景声明。无资格时在请求前明确报错，
  不静默选另一家。旧配置中空 scenarios 的显式执行因此会被拒绝，需先保存真实支持的场景。
- 无合格候选时返回 `NoCandidateModel`，不调用供应商；合法候选的 HTTP 400/401 仍是原非瞬时错误，
  不通过扩大发送范围来补救失效绑定。
- 视频 submit/status 的自动与显式模型解析也使用此规则；自动选择消费当前 run workspace 的绑定。
  URL 第二跳仍走原批准工具，不因场景选型而放开网络边界。

## 配置快照与并发

复用既有 provider commit gate，把 provider/model 编辑、完整 binding 列表验证/写入和候选准备串行化。
一次候选准备在同一边界内固定 provider、model、场景、endpoint 和凭据，不在 HTTP await 期间持锁。
已经准备或在途的调用保留其快照，配置撤销影响后续新请求，不承诺取消远端已接收的工作或退款。

`CanvasProviderService::resolve_model_for_scenario` 提供同一边界上的显式执行解析。
原 `resolve_model` 保留为 enabled 身份/诊断解析，不隐式猜场景；正式 gateway 执行入口使用前者。
场景校验先于凭据物化。没有新增存储、路由器、迁移、前端字段或第二套审批/取消机制。
此一致性针对使用既有 service/gate 的协作读写，不声称任意进程直接修改 SQLite 文档也受该锁约束。

## 回归入口

测试只用合成回环 provider、合成内存凭据和临时 SQLite，不下载视频结果 URL：

```text
cargo test -p deepagent-app-core --test canvas_scenario_binding_audit --offline -- --test-threads=1
cargo test -p deepagent-app-core --lib canvas_model_gateway::workflow_media_audit::h292_ --offline -- --test-threads=1
```

六场景、两种更新 API、重开、显式拒绝、恢复、旧绑定与快照验收，以及完整门禁的实际结果/残余限制，
见原审计 H-292 和修复进度同编号条目；本说明不替代实际执行证据。
