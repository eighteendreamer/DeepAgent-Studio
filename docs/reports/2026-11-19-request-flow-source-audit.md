# 请求流程与参数拼接动画审计

日期：2026-11-19。

交付：[docs/ui/request-flow.html](../ui/request-flow.html)。独立 HTML，无外部依赖，可离线打开。

## 范围与版本

依据当前工作区源码，而非方案中的预期架构。基线提交为 `08b772f3cc38816cde5443059681f0cb31e4c4f8`；部分源文件有本轮之前的未提交改动。图中每段源码均附路径、行号、完整文件 SHA-256 和相对基线的改动标记，包含 36 个源文件、58 段摘录。源码后续变化不会自动更新这份快照。

主图跟踪桌面正常消息的 12 步：

1. App 处理文本和附件。
2. 前端注册事件监听并调用 `start_chat_v2`。
3. Tauri 创建/沿用会话、准入并返回 ACK。
4. worker 解析执行配置、模型路由和凭据。
5. 输入租约、历史恢复、控制命令及 UserPromptSubmit Hook。
6. 上下文清单排序、预算裁剪、稳定指令和动态 snapshot。
7. ModelAgent 原生 Surface、工具结果、图片解析和请求构造。
8. `PROTOCOLS → codec_for` 工厂分发。
9. Responses / Chat Completions / Messages 字段转换。
10. URL、鉴权、Files 条件分支和最终 HTTP 请求。
11. SSE、协议 decoder、usage 和 RuntimeEvent。
12. 工具权限/审批/执行及续写，或持久终态并通知 UI。

每一步提供“字段与拼接 / 输入 / 输出 / 源码”。另列工具续写、模型 Hook、压缩/prefire、输出截断、传输重试、Files、stall/验证、标题和模型发现分支。CLI/Harness prepared admission 与桌面普通消息的冻结时机有单独边界说明；Canvas、远程终端和 MCP 业务 API 不冒充模型 HTTP Body。

## 样本如何生成

使用临时 Rust 捕获程序调用当前 `deepagent-models::ModelClient::stream_response`，在自定义 `HttpTransport::stream` 收到 `TransportRequest` 时截获实际 URL、auth 枚举、Body 和 UTF-8 字节数，然后主动结束；不执行网络请求。

- 5 种模拟路由：DeepSeek、通用 GPT 标识、Claude adaptive、Claude budget、Grok 标识。
- 3 种协议、6 种选择（应用默认/off/low/medium/high/max）、4 个场景（新文本/续聊/工具结果/图片）。
- 合计 360 组；348 组实际生成 Body，12 组由于该通用 GPT 路由没有 off 声明而被本地拒绝。拒绝是捕获矩阵中的预期结果，不是供应商返回。
- 用户界面依据当前能力的 `distinct_levels` 提供等价合并后的选项，能操作的组合为 300 组。
- 样本容量为合成值：context_window=131072，output_limit=16384；设置为 temperature=0.4、top_p=0.7、max_output_tokens=8192、tool_choice=auto、user=demo-user。
- 系统规则、环境、snapshot 摘要、历史、工具 schema 和工具结果均为合成内容；图片为程序生成的 1×1 PNG。
- Body 为真实 Rust 编码结果。其他步骤的示例对象是依据源码构造的结构说明，没有伪称截获了完整桌面/AppCore 运行。
- 请求头来自 reqwest transport 源码核对；这是 TransportRequest 捕获，不是 TLS/HTTP 抓包。Key 统一展示为 `<REDACTED>`。
- 图片样本关闭 Files 上传，展示真实 inline 编码；生产链路的上传、缓存和受限回退另行说明。
- 模拟路由的协议配置不证明远端支持；未使用用户数据库、真实凭据或真实聊天。

临时程序在生成后移出 crate，保存在忽略目录 `target/request-flow-audit/capture.rs`；生产源码与依赖未因本轮修改。

## 从源码确认的核对重点

1. **模型选择和发送是两条 IPC。** `apps/desktop/src/api.ts:1516` 的发送参数没有模型/供应商/思考档位；胶囊另行调用 `harness_model_select`。普通桌面消息先 ACK，worker 准备时读取并冻结模型；不把 ACK 误画成完成模型/工具准备。仅凭这个边界不能判定已经存在竞态。
2. **Messages 提升 system。** `crates/deepagent-models/src/messages/request.rs:220` 起把 instructions 和历史 system 项集中到顶层 system；历史 snapshot 因而离开原有时间位置。“最新替换旧”由本地 envelope 文本表达，并非供应商自动删除历史。
3. **跨协议字段不完全等价。** Chat 将内部 user 写为 user_id；Messages 当前编码器没有投影 user/text/top_logprobs；Chat 明确拒绝 top_logprobs。图中展示差异，未擅自改业务代码。
4. **图片可能先走 Files。** Responses/Chat 在 transport 支持上传且存在缓存时可先上传；根地址末尾 /v1 被移除后再拼 /files。Messages 保持 image source，不走此 Files 分支。上传失败及明确无效 file_id 的回退条件不同。
5. **过载不重试不等于所有情况不重发。** Overloaded 不可重试；RateLimited/Server/Timeout/Transport 仍可受限重试；Length/ContextOverflow 有上限提升、续写或压缩分支。不存在这里自动切换到其他模型的 fallback。
6. **Responses SDK 与本地扩展并存。** 标准字段使用类型层；reasoning/custom 等保留的 DeepSeek 扩展不能据此认定适配所有第三方。图没有将现状改画成通用理想协议。

以上是源码行为及需核对的边界，不是本轮已通过远端复现的缺陷清单。

## 实际验证

离线 Rust 捕获：

```text
cargo run -p deepagent-models --example request_flow_audit_20261119 --offline --locked --target-dir D:/DeepAgent-Build/p2-p4-target -j 1
```

实际复用既有 Cargo home/target；最终执行成功，生成 360 条记录。存在既有 workspace 清单和 MSVC LIBCMT 链接警告。临时 example 已清理，不作为生产示例提交。

浏览器脚本：

```text
node target/request-flow-audit/verify.cjs
```

使用已安装 Playwright 1.62.1 和本机 Edge，无桌面控制插件。脚本验证：

- 300 个可选组合均找到匹配记录，页面 Body 与截获 Body 逐项相同，URL 与记录相同。
- 三协议鉴权展示、stream=true、Chat usage/user_id、Messages max_tokens/字段省略。
- 12 个节点 × 4 个详情页、上/下步、播放和暂停。
- URL 根路径、已有 /v1、错误填写 /messages 时的实际字符串拼接。
- 320/736/1024/1440 宽度无页面水平溢出，表格在窄屏内滚动。
- 浅色/深色、减少动画设置；截图人工查看。
- 无页面脚本异常，无外部 HTTP 请求。

证据保存在忽略目录 `target/request-flow-audit/`：projections.json、source-manifest.json、capture-final.log、verification.json 和截图。

没有进行真实模型联网验收、完整桌面重编译、完整 workspace 测试、SSE 响应回放或性能计时。本轮只交付流程审计文档，不能据此判断当前运行二进制版本、真实首 token 延迟或所有供应商兼容性。无数据库迁移、权限或沙箱配置变更。
