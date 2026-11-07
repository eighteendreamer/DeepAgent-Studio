# 画布供应商图片输出限额

> 图片输出默认上限为 **32 MiB/张**，不是供应商标准或输入附件限额。
> 超限现在会明确失败；不会截断图片后伪造成功，也不会自动换模型重发。
> 这是单输出资源边界，不是总磁盘、并发、像素解码或全宿主内存配额。

## 调整可信宿主策略

在现有 Rust 宿主组装 `CanvasModelGateway`、转为共享 `Arc` 之前调用
`with_image_output_limit(max_bytes)`。它返回 `CoreResult<Self>`，拒绝零值、
不能进入制品索引的数值以及无法在本机寻址的派生 JSON 上限。
`scoped_for_run` 继承不可变限额，不读取活动项目的可变设置。

新增常量 `DEFAULT_IMAGE_OUTPUT_LIMIT` 为32 MiB。需要更大合法图片的可信宿主可以
显式提高限额；没有新增桌面设置项、wire DTO、SQL 字段或依赖。通用本地导入、
输入附件、视频输出和音频输入的既有数值不变，语音合成输出不在本项范围内。

## 哪些内容受限

| 路径 | 边界 |
|---|---|
| 供应商 URL | 仍须原审批与公共网络 broker；声明长度只作提前拒绝，实际 chunk 在写入前累计 |
| `b64_json`、`data`、Gemini `inlineData`/`inline_data` | 解码前估算原始字节，解码后再次核对同一图片限额 |
| 图片生成/编辑的成功 JSON | UTF-8/JSON 解析前，最多 `4 × ceil(图片限额 / 3) + 1 MiB` |
| 非成功 provider 正文 | 继续复用原64 KiB错误正文边界，不受图片配置扩大 |

成功 JSON 的1 MiB余量用于 URL、metadata 和信封，不是给每个返回图片另加额度；
当前仍只请求/消费第一张图片。额外图片、很大的 metadata、JSON 转义或非规范编码可能
触及整个正文上限，均明确失败。此取舍是本地有限策略，未宣称兼容所有真实供应商。

图片 URL 复用原单次句柄、权限、取消、deadline、`CanvasArtifactStream` staging、
摘要及原子发布。超限块不写入，失败/drop清理尚未发布的 `.part`，不会落制品记录或
成功节点。`ImageArtifactTooLarge`/`ResponseBodyTooLarge` 是非 transient 错误，
不因大小拒绝启动候选模型 fallback；未知远端执行结果不能理解为“供应商未执行或未计费”。

HTTP framing 仍由网络库解释，实际计数对象是其交付的 response body；不能把服务端
成功 write 或客户端未读取 framing 之外的字节说成完整下载。URL 下载流式写 staging；
inline 路径仍需容纳有限的 JSON、解析对象和解码字节。底层单chunk、解析开销和多个
并发请求会叠加，压缩图片的解码像素、OOM及真实磁盘耗尽未因此得到保证。

## 本机回归

仓库根执行，使用已缓存Rust依赖；只使用合成临时文件/SQLite及自建回环服务，
公网域名在 `cfg(test)` 接缝固定到回环，不修改生产网络门禁。

```powershell
cargo test -p deepagent-app-core --lib canvas_model_gateway::image_output_tests:: --offline
cargo test -p deepagent-app-core --lib canvas_model_gateway::workflow_media_audit::audit_provider_image_download_rejects_oversized_artifact --offline -- --exact
cargo test -p deepagent-app-core --lib canvas_model_gateway::workflow_media_audit::audit_provider_inline_image_rejects_oversized_artifact --offline -- --exact
cargo test -p deepagent-app-core --lib canvas_model_gateway::video_download_tests:: --offline
cargo test -p deepagent-app-core --lib canvas_model_gateway::audio_input_tests:: --offline
```

原失败、修复状态、实际执行结果和未验范围以
[原审计报告](system-harness-audit-2026-10-21.md)与
[修复台账](system-harness-repair-progress.md)的 H-167 为准。
