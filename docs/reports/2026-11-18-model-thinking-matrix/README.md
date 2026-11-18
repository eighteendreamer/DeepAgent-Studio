# 模型思考参数实测报告（2026-11-18）

本轮按用户要求只测试、记录并报告，没有修复产品代码。测试对象是 2026-11-18 18:41:30 编译的现有桌面程序；工作区正在进行、尚未完成的 deepseek-harness 思考能力改造没有编译进本次程序，因此本报告不代表新改造已经验收。

测试使用同一原生进程完成矩阵，期间没有重启。测试结束复核时发现磁盘同路径程序已被 19:59:12 的另一份构建替换，SHA256 为 `dfe45d00c4db4ab54a6eb327bcc4a9e6200a8011ed5dbbb83f9bfebde1db8cc7`；本轮结果只适用于启动时校验的构建，不适用于这份新文件。

## 范围与方法

- 原用户数据库的已激活模型快照共 14 个，分别测试界面现有 off / low / medium / high / max 五档，共 70 个组合。
- 当前程序没有 minimal / xhigh / ultra 的可选控件，本轮没有将这些值绕过界面注入测试。参数传递一致性按当前界面所选值核对，不按模型名称猜测供应商的映射规则。
- 使用真实 Tauri / WebView2 桌面程序，Playwright 通过 CDP 操作模型选择、思考档位和发送消息；IPC 只用于读取选择与设置以确认保存结果，未用 IPC 直接代替模型切换或发消息。
- 使用独立测试数据目录，复制模型配置和加密凭据，没有复制原聊天。仅测试副本的供应商 URL 指向本机透明流式代理；代理原样转发 JSON 请求字节，不改模型和思考字段。原供应商为 DeepSeek 官方及 Layso，协议均为 Responses。
- 统一发送简短问候并要求不调用工具，每个组合从新对话入口发起。150 秒未终止或出现工具调用时，脚本取消该测试运行并保留已发出的参数证据。UI 禁用档位不绕过限制强行发送。
- 记录界面选择、保存值、运行态 model_request_started、实际 HTTP 请求中的 reasoning.effort、响应中的相关参数、HTTP 状态与耗时。标题及会话记忆的后台请求单独识别，不计入主对话参数判断。
- 报告、CSV、JSON 和本机 wire 元数据不包含密钥、请求正文、回答正文或思考正文。原始应用测试数据库与本机诊断文件留在忽略的 target 目录。
- 可执行文件 SHA256：`205d2ae55242bf909238b2042fb93c83cb0d8d89e3549f9bbad4f8184b5bf65b`。

## 结果

- 14 个模型 / 70 个组合；69 个组合实际发出主对话请求，1 个组合在界面被禁用。
- 对话终态：61 完成、6 失败、2 因工具调用被脚本取消、0 测试超时、0 脚本异常。
- 已发送组合中，界面档位与实际 wire 参数不一致 0 个；有参数回显的 24 个组合中，发送与回显不一致 2 个。
- 运行态 model_request_started 与界面所选模型/档位不一致：0 个。
- 45 个已发送组合没有回显思考档位。这些只能确认本系统发出的参数，不能据此证明供应商内部使用了哪个档位。
- 关闭档位下观察到 reasoning delta 或非零 reasoning token 的组合：4 个。返回模型标识不同于请求标识的组合：0 个。

表中为“实际发送 → 服务端回显”。off 的保存与 wire 拼写为 none。“未回显”不是通过或失败结论；HTTP 成功和回显一致也不能证明供应商内部模型执行强度。

| 供应商 / 模型 | 关闭 off | low | medium | high | max |
|---|---|---|---|---|---|
| deepseek / `deepseek-flash` | none → none | low → low | 禁用，未发请求 | high → high | max → max |
| layso / `claude-fable-5-2` | none → 未回显；仍有思考输出 | low → 未回显 | medium → 未回显 | high → 未回显 | max → 未回显 |
| layso / `claude-haiku-5-5` | none → 未回显 | low → 未回显 | medium → 未回显 | high → 未回显 | max → 未回显 |
| layso / `claude-opus-5` | none → 未回显 | low → 未回显 | medium → 未回显 | high → 未回显 | max → 未回显 |
| layso / `claude-opus-5-5` | none → 未回显；仍有思考输出 | low → 未回显 | medium → 未回显 | high → 未回显 | max → 未回显 |
| layso / `deepseek-v4.1-flash-1017` | none → 未回显 | low → 未回显 | medium → 未回显 | high → high | max → max |
| layso / `doubao-seedream-5-0-pro` | none → 未回显；失败 HTTP 404 | low → 未回显；失败 HTTP 404 | medium → 未回显；失败 HTTP 404 | high → 未回显；失败 HTTP 404 | max → 未回显；失败 HTTP 404 |
| layso / `gemini-3-flash-preview` | none → none | low → low | medium → medium | high → high | max → max |
| layso / `glm-5.3-flash` | none → 未回显 | low → 未回显 | medium → 未回显 | high → 未回显 | max → 未回显 |
| layso / `gpt-6.2-sol` | none → medium；回显不符 | low → low | medium → medium | high → high；工具后取消 | max → max；工具后取消 |
| layso / `grok-4.7` | none → 未回显；仍有思考输出 | low → low | medium → medium | high → high | max → high；回显不符 |
| layso / `kimi-k3.1-pro` | none → 未回显 | low → 未回显 | medium → 未回显 | high → 未回显 | max → 未回显 |
| layso / `minimax-m3-1-flash` | none → 未回显 | low → 未回显 | medium → 未回显 | high → 未回显；失败 HTTP 无响应头/502 | max → 未回显 |
| layso / `qwen4-max` | none → 未回显；仍有思考输出 | low → low | medium → medium | high → high | max → max |

## 异常及限制

- `layso/doubao-seedream-5-0-pro/high`：high → 未回显；失败 HTTP 404。
- `layso/gpt-6.2-sol/high`：high → high；工具后取消。主对话共 2 次 HTTP 请求，状态 200 → 无响应头。
- `layso/kimi-k3.1-pro/high`：high → 未回显。主对话共 2 次 HTTP 请求，状态 502 → 200。
- `layso/minimax-m3-1-flash/high`：high → 未回显；失败 HTTP 无响应头/502。主对话共 3 次 HTTP 请求，状态 无响应头 → 无响应头 → 502。
- `layso/claude-fable-5-2/off`：none → 未回显；仍有思考输出。返回了 reasoning 增量；reasoning tokens：未提供。
- `layso/claude-opus-5-5/off`：none → 未回显；仍有思考输出。返回了 reasoning 增量；reasoning tokens：未提供。
- `layso/doubao-seedream-5-0-pro/off`：none → 未回显；失败 HTTP 404。
- `layso/glm-5.3-flash/off`：none → 未回显。主对话共 3 次 HTTP 请求，状态 无响应头 → 无响应头 → 200。
- `layso/gpt-6.2-sol/off`：none → medium；回显不符。
- `layso/grok-4.7/off`：none → 未回显；仍有思考输出。返回了 reasoning 增量；reasoning tokens：0。
- `layso/kimi-k3.1-pro/off`：none → 未回显。主对话共 2 次 HTTP 请求，状态 502 → 200。
- `layso/qwen4-max/off`：none → 未回显；仍有思考输出。返回了 reasoning 增量；reasoning tokens：55。
- `layso/doubao-seedream-5-0-pro/low`：low → 未回显；失败 HTTP 404。
- `layso/kimi-k3.1-pro/low`：low → 未回显。主对话共 2 次 HTTP 请求，状态 502 → 200。
- `deepseek/deepseek-flash/medium`：禁用，未发请求。
- `layso/doubao-seedream-5-0-pro/medium`：medium → 未回显；失败 HTTP 404。
- `layso/doubao-seedream-5-0-pro/max`：max → 未回显；失败 HTTP 404。
- `layso/gpt-6.2-sol/max`：max → max；工具后取消。主对话共 2 次 HTTP 请求，状态 200 → 无响应头。
- `layso/grok-4.7/max`：max → high；回显不符。

本轮是每个组合一次问候样本，不是稳定性、压力测试或模型推理质量评测。服务端可接受或回显字段，不等同于其后端模型实际执行该档位；尤其未回显的模型仍需供应商文档或服务端日志才能进一步验证。简单问候没有产生 reasoning token，也不能反推高档未生效。

完整参数与状态见同目录 results.json；便于筛选的逐项记录见 results.csv。产品修复及未完成的思考能力改造均保持暂停，等待用户审阅。
