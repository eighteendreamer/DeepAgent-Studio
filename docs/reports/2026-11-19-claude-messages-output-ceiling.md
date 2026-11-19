# Claude Messages 输出上限排查与修复（2026-11-19）

隔离桌面添加并激活 Layso 的 `claude-opus-5-5` 后，模型列表和余额可用，但新对话在发送 `high` 时返回 HTTP 400 `invalid_request_error`。模型注册表中的输出上限是 `131072`。代码在模型发现缺少 `max_output_tokens` 时补入该值，Messages 适配器随后按注册表发送 `max_tokens`。

## 对照结果

使用验收指定的 Messages 地址和模型，输入固定为一条合成问候。保持 `thinking.type=adaptive`、`output_config.effort=high` 及 Bearer 认证，只改 `max_tokens`：

| `max_tokens` | HTTP | 收到 `message_stop` |
| ---: | ---: | --- |
| 2048 | 200 | 是 |
| 8192 | 200 | 是 |
| 32768 | 200 | 是 |
| 65536 | 200 | 是 |
| 65537 | 200 | 是 |
| 127999 | 200 | 是 |
| 128000 | 200 | 是 |
| 128001 | 400 | 否 |
| 131071 | 400 | 否 |
| 131072 | 400 | 否 |

低上限时，Bearer、`x-api-key` 加版本头的基础请求分别成功；带 `adaptive/high` 的请求也成功。再使用单个合成工具、稳定系统块并各设一个显式缓存断点，仍有 `message_stop`。响应包含缓存用量字段，但未确认非零命中，也没有档位回显。这些结果只证明所测路由和请求形态。

首次探针错误地把验收任务中用于 Markdown 换行的反斜杠附到了测试密钥，四次 401 不属于有效对照。改为使用原始密钥后才取得本报告中的结果。

## 修复和验证

- 新发现的 Messages 模型在未声明输出上限时，使用十进制 128K（`128000`）；供应商声明的上限优先。其他协议的默认值、已保存模型的配置、生产请求序列化和重试策略保持原有行为。
- 新增 `messages_discovery_uses_128k_when_the_provider_omits_the_ceiling`。旧代码以 `131072` 对 `128000` 失败；同一用例也检查供应商明确返回 `131072` 时仍按原值入库。修复后 `cargo test -p deepagent-app-core --test harness_model_service --locked --offline -j 1` 的 25 项通过，两处改动文件的 `rustfmt --check` 通过。
- 旧桌面程序中，通过既有 Tauri 模型设置命令把隔离数据目录内的该模型输出上限保存为 `128000`，再用 Playwright 点击新对话并发送合成消息。页面出现预期回复、无失败提示，后端日志为 `run_completed`。后端准备 2080 ms、调用传输层至首个 token 5285 ms、到运行完成共 7579 ms。此项验证现有配置修复和完整发送链路，不等同于重编译后的新发现默认值验收。

已保存为 `131072` 的旧配置可在模型设置单独改为 `128000`，不自动覆盖用户配置。不得把本路由的上限作为所有 Messages 供应商的硬上限。
