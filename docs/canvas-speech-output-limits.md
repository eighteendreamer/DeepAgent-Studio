# 画布语音合成输出限额

`CanvasModelGateway` 对供应商 `speech_synthesize` 的成功响应施加独立限额。默认
`DEFAULT_SPEECH_OUTPUT_LIMIT = 32 * 1024 * 1024` 字节，是宿主本地保守策略，
不是供应商标准、音频输入限额或完整进程内存保证。

## 可信 Rust 宿主配置

共享 gateway 前可使用已有构造链上的两个 additive builder：

```rust,ignore
let gateway = CanvasModelGateway::new(providers)
    .with_artifacts(artifacts)
    .with_speech_output_limit(32 * 1024 * 1024)?
    .with_speech_output_limit_for("provider-id", "voice-model", None, 48 * 1024 * 1024)?
    .with_speech_output_limit_for("provider-id", "voice-model", Some("wav"), 64 * 1024 * 1024)?;
```

按**实际选中**的 provider/model 与请求格式匹配：精确格式覆盖 → 同模型覆盖 → 默认值。
格式仅在政策匹配时去首尾空白、转 ASCII 小写；不改发送给供应商的参数。
零值、不可索引或不可寻址的限额及空作用域标识明确拒绝。每个 run scope 继承已配置政策，
不修改共享父 gateway。没有新增未接线的桌面开关、wire 字段或数据库 schema。

## 读取、发布与兼容

- 同一 reader 预检声明长度，并在每个 chunk 拷贝或写盘前累计检查；允许恰好到界。
- 正式 `run_audio` 直接使用既有 `CanvasArtifactStream` staging；EOF 前不发布，
  完整接收后才经既有原子发布与 SQLite 索引返回 `artifact://`。
- 公开 `execute` / `execute_on` 的 `CanvasModelOutput::Audio { bytes, mime, ... }`
  形状不变，无 artifact service 也可调用；同一 reader 使用有界 Vec sink，不生成制品。
- 容器签名优先于响应声明或请求后缀。明确请求的 raw PCM/Opus 仅在响应声明对应
  raw 类型或 `application/octet-stream` 时保留有限兼容；未验证完整 codec、采样率或声道。
  空正文、未知内容不能仅凭请求 `mp3` 标签成为音频制品。
- 容量错误 `SpeechArtifactTooLarge`、格式/存储错误不是自动换模型的理由。
  成功头之后 body 截断/超时属于远端结果未知，不自动重派；不承诺远端未执行或未计费。
  原非成功响应有界读取和小型 429/5xx 候选回退保持不变。
- 取消与整体 deadline 仍由原 WorkflowAgent/Kernel owner 管理，丢弃 future 清理原 staging；
  不新增第二套取消、审批、dispatch 或 store。本地导入、转写输入和其他模态不受此政策限额约束。

限额约束接收的单次音频字节，不约束压缩解码后的内存、底层 HTTP chunk 开销、并发总量或供应商成本。
调高限额仍需评估资源。未提供完整音频解码有效性、OOM、断电或其他操作系统验收保证。

## 回归入口

在仓库根运行（测试仅使用合成回环 HTTP 与临时 SQLite）：

```text
cargo test -p deepagent-app-core --lib canvas_model_gateway::speech_output_tests:: --offline -- --test-threads=1
cargo test -p deepagent-app-core --lib canvas_model_gateway::tests::audit_speech_synthesis_output_rejects_oversized_artifact --offline -- --exact
cargo test -p deepagent-app-core --lib canvas_model_gateway::workflow_media_audit::h248_ --offline -- --test-threads=1
```

实际红绿证据、完整验收命令、基线失败与残余限制记录在原审计 H-248 和修复进度同编号条目；
示例配置不是已运行的验收。
