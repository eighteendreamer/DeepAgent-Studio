# P5 Messages 图片与原生请求头

本轮补齐 P5 的图片和原生 Anthropic 请求头；P2 继续使用已经提交的完整协议表与工厂。没有变更模型思考策略、缓存断点、权限或 UI。

## 实现依据

- DeepSeek Harness：`llm-pi-ai/src/provider.ts` 的完整 provider 工厂；`llm-deepseek-api-key/src/index.ts` 的 API Key 鉴权；`llm-deepseek/tests/serialize.spec.ts` 的工具结果图片与顺序约束。
- 已固定的 `anthropic-sdk` revision `16dc3e424e7c5041bcc952ebb54d9ec4b2b1ced7`：`ContentBlockParam::Image`、`ImageSource::{Base64, Url}` 及默认 `anthropic-version: 2023-06-01`。
- Anthropic 官方 API overview 与 vision 文档，2026-11-19 查证：支持 API Key / Bearer 以及 base64 / URL 图片来源。官网同时保留 2023-06-01 与 2026-10-21 版本；本轮沿用已固定 SDK 的版本，避免隐式迁移 API 版本。
- 已锁定的 reqwest 0.12.28 `redirect.rs::remove_sensitive_headers` 不剥离 `x-api-key`，因此携带该头的客户端必须限制重定向来源。

## 生产边界

- `MessagesCodec` 复用现有异步图片字节校验与取消边界，用户图片投影为 SDK image block；工具返回图片保留在 `tool_result.content` 内，文本、重复图片和工具结果配对顺序保持；纯图片输入省略空文本块。
- JPEG、PNG、GIF、WebP 保留原始 base64 字符串；远程图片保留 URL，系统不主动下载或重编码。未解析引用、无视觉能力、错误 MIME、损坏字节及不支持的角色在发送前报错。
- 序列化继续经过既有 48 MiB 完整请求预算。供应商自身的图片数量、文件大小等限制仍由供应商执行；没有将该本地预算宣称为所有供应商的配额。
- 工厂拥有鉴权类型，`TransportRequest.auth` 传递给共用传输层；模型发现通过 `ModelDiscovery.with_wire_mode` 读取同一个 codec 的鉴权类型。
- **本地兼容设计**：Messages 同时向已配置来源发送 `x-api-key`、Bearer 和版本头，兼容已有 Bearer 网关以及原生 API Key 入口，不按模型名称或密钥格式推测鉴权。
- `ReqwestTransport::new` 和 `with_client_builder` 允许同源重定向并阻止跨源及协议降级，保护密钥与 POST 正文。已有 `with_client` 保持 Bearer 用法；Messages 必须改用 `with_client_builder`，因为已构建客户端的重定向策略无法检查或修改。
- HTTP 错误正文中回显的实际密钥会先被替换；请求 Debug 仅保留鉴权类型和正文长度。没有新增过载重试。

## 验证

初始回归已复现：图片测试 1 通过、3 失败，错误为图片尚未实现；真实 HTTP 头测试 2 项均失败。实现后首轮新增图片、HTTP 与工厂专项共 11 项通过；追加纯图片空文本块回归先失败，再修正投影。

- 集成工作树新增专项：`cargo test -p deepagent-models --features http --test messages_images --test messages_http --test protocol_factory --offline -j 1`，最终 **12 通过**。
- `cargo test -p deepagent-models --features http --offline -j 1 --no-fail-fast` 在 lib test 编译时因 Windows `memory allocation of 2097152 bytes failed` 退出，没有完整执行该命令的测试。
- 已成功构建的 33 个模型集成测试二进制逐个实际运行（每个 `--test-threads=2`）：155 通过、0 失败、4 忽略。该结果不包含未完成编译的 lib test。
- 独立快照：`cargo clippy -p deepagent-models --features http --all-targets --offline -j 1 -- -D warnings` 通过。
- 本轮文件在集成工作树及独立快照的 `rustfmt --check` 均通过；`cargo fmt --all -- --check` 仍被未修改 `crates/deepagent-core/src/id.rs` 的四处既有排版差异阻断。
- `cargo test --workspace --offline --no-fail-fast -j 1` 在 `kernel_v2_e2e` 编译阶段遇到 `rustc-LLVM ERROR: out of memory`，未进入完整测试执行，不能宣称 workspace 通过。
- 第一轮独立完整模型测试发现 UTF-8 协议夹具仍使用不具备重定向保护的旧客户端构造器；Messages 因明确鉴权门禁报错，夹具等待连接超时。已同步夹具到安全构建器，重新运行完整模型测试。
- 最终独立提交快照：`cargo test -p deepagent-models --features http --offline --config 'profile.test.package.deepagent-models.debug=0' -j 1 --no-fail-fast`，**420 通过、0 失败、4 忽略**。其中库测试 280 项；包含新增纯图片回归及修正后的 UTF-8 夹具。
- 最终版本严格 Clippy 再次通过。本轮 Rust 文件在集成工作树与独立快照的格式检查均通过。
- 集成工作树最终专项 12 项通过（图片 5、真实 HTTP 4、工厂 3）；`cargo test -p deepagent-app-core --test harness_model_service --offline -j 1`，**25 通过、0 失败**，包括协议感知模型发现调用。

日志留在忽略目录 `target/p5-messages-audit/`，不提交请求、响应、密钥或机器配置。

## 兼容与验收范围

`TransportRequest` 的 Rust 字面量增加 `auth` 字段，仓库内构造点已同步。自定义 `HttpTransport` 若需支持 Messages 模型发现，应实现 `get_json_authenticated`；默认实现对未声明支持的 Messages 发现明确报错。AppCore 测试传输已同步验证鉴权类型。

本轮使用真实回环 HTTP/SSE 和真实图片字节验证。没有使用原生 Anthropic 账户发起远程验收，也没有执行桌面 UI 验收；本轮未修改 Tauri/前端接口。
