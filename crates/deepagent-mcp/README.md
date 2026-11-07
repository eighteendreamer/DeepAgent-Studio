# MCP 响应字节预算

DeepAgent 在 stdio/HTTP 读取边界限制原始响应，避免完整缓冲后才做结果裁剪。

> 这是 DeepAgent 本地策略，不是 MCP 标准上限。默认 **16 MiB**；每 server 可设为
> **1 字节～64 MiB**，不支持零值或无限值。老配置无需修改；Rust 的
> `McpServerConfig` / `McpServerDto` 完整字面量需补 `response_limit_bytes: None`。
> 大结果被拒绝不代表远端工具没有执行，不能因此自动重试业务调用。

## 调整一个 server 的预算

在现有 MCP 配置的 server 项中添加可选的 `response_limit_bytes`：

```json
{
  "mcpServers": {
    "local-example": {
      "command": "node",
      "args": ["server.cjs"],
      "response_limit_bytes": 8388608
    }
  }
}
```

这是完整 JSON 配置示例；启动仍需要用户自己配置的可用 `server.cjs`。
同字段适用于 HTTP/SSE server。省略或 `null` 使用默认值；零、负数、非整数和超过
64 MiB 的值拒绝配置，不会关闭限制。预算计入整个 JSON-RPC envelope，而非仅结果文本：
stdio 还包括行末 LF/CRLF，HTTP 为实际返回的 body 字节。

用户配置、内置/传统插件 MCP 配置、AppCore 持久化与 DTO 均保留该字段；桌面现有编辑
序列化保留它，但本项没有新增数值输入控件。标准 Agent Plugins 的封闭 MCP schema
不接受该本地扩展，仍严格拒绝未知字段；其投影使用默认预算，不偷偷扩展外部协议。

## 超限后的行为

- **stdio**：按有限块累加，超限立即停止 reader 并永久隔离该连接，不等 LF/EOF，
  不丢弃部分数据后猜测下一帧。原 dispatcher 释放所有 pending waiter；等待/后续请求
  发现超限时复用原 close/owned-process/job 关闭。无 waiter 的主动超限先隔离，仍由
  原 owner 的 close/drop 收尾。没有新增进程所有者或后台重启链。
- **HTTP/SSE**：成功 JSON、SSE、非成功状态与通知回执共用实际 chunk 计数。
  不信任 Content-Length 作为唯一依据；超过预算不追加该块、不继续 drain、不解析。
  丢弃本次 body，后续显式的小请求仍可使用 HTTP transport；不暗中重发失败请求。
- **错误**：`CoreError::McpResponseTooLarge { limit_bytes, observed_bytes }` 保留结构化
  计数。observed 是截至当前块的已观测下界，不声称读完远端正文。此限额错误不回显正文、
  stdio stderr 或 URL 凭据；普通 HTTP 非成功/解码错误同样省略正文。
- **副作用**：原 `ReconnectingTransport` 不把限额错误当临时断网，不自动恢复并重试它。
  `tools/call` 按原 `RemoteOutcomeUnknown` 返回，内含限额原因；先核对远端状态。
  直接使用底层 transport 的调用方也不能把限额错误解释为“未执行”。

## 大型合法资源与限制

原始 envelope 上限不按尚未解析的 stdio method/id 猜测，所有消息先受同一个安全边界约束。
工具目录既有分页/条目/游标上限、运行时模型可见工具结果预算继续生效，不能用它们替代
读取上限。正常的 2 MiB 图片/资源载荷仍可在默认值内传递；需要更大内容时采用服务端
分页、文件/资源引用或显式调大预算。超过上限返回错误，不能截断 JSON 或伪造成功。

这是每消息/每 body 的限额，不是全宿主内存、并发、CPU 或沙箱配额。解析对象、SSE
投影和单个底层网络 chunk 仍有额外开销，多连接/多请求会叠加。stdio 累加缓冲不超过预算，
HTTP 在追加前检查实际 chunk；不会宣称底层网络栈完全不预读额外字节。既有 HTTP 60 秒
请求超时、取消、环境策略、URL 验证及工具权限不变。

## 本机回归检查

在仓库根目录执行；HTTP 测试只启动自建 loopback，stdio 测试只启动合成临时 Node 服务。
需要本机 Node 和已缓存的 Rust 依赖，不需要真实 MCP server、凭据或模型：

```powershell
cargo test -p deepagent-mcp --offline
cargo test -p deepagent-mcp --features http --offline
cargo test -p deepagent-app-core --lib h126_ --offline
pnpm -C apps/desktop test:mcp-response-budget
```

原审计 1 MiB 阈值不是产品默认；原探针现在显式配置 1 MiB，保留小响应对照并断言大响应
被 typed error 拒绝。原始未修复失败和完整验收/未验范围保存在
[审计记录](../../docs/system-harness-audit-2026-10-21.md)与
[修复台账](../../docs/system-harness-repair-progress.md)的 H-126 项。
