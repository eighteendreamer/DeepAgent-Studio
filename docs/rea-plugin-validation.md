# REA 插件制作与验收

日期：2026-10-13。首版：包装 0.1.0 / REA 6.4.0 / Windows x64 / Node 24.21.0。

## 交付

- 内置源文件：[REA README](../apps/desktop/src-tauri/resources/plugins/rea/README.md)。
- 制作脚本：[package-rea-plugin.ps1](../scripts/package-rea-plugin.ps1)。
- 安装制品：`dist/plugins/rea-plugin-0.1.0-windows-x64.zip`，约 22 MiB。
- 同名 `.zip.json` 记录 REA 注册表完整性、两层 ZIP 的 SHA-256、条目数和展开大小；以本次制品旁的记录为准。
- 运行包包含 144 个生产依赖包，6,557 个条目，展开大小 91,148,579 字节，符合系统 100,000 条目 / 512 MiB 上限。

首版独立制品验证后，现已将 REA 包装移入桌面内置资源目录，沿用通用插件扫描。构建阶段从锁文件准备 runtime.zip，最终用户无需外部安装，也不运行 npm 或访问开发 checkout；Node 与 Ghidra 等外部引擎另行提供。包装、锁文件和验收源码提交；依赖树、临时目录和 ZIP 遵循构建产物忽略规则。

## 本轮内置接入验证

本轮纠正分发方式：REA 作为 `rea@builtin` 随桌面提供，未向真实用户的个人插件目录安装。桌面继续使用已有资源扫描与 MCP 投影，不增加 REA 专属入口。

| 命令或检查 | 本轮结果 |
|---|---|
| `pnpm --dir apps/desktop prebundle-rea` | 使用托管 Node 24.21.0，144 个生产依赖安装成功；原生制品验证和实际加载成功；生成内置运行 ZIP 与元数据 |
| `cargo test -p deepagent-app-core --offline --test plugin_real_fixtures` | 7 项通过；真实内置资源共 10 个插件，REA 默认启用、来源 builtin、1 个 Skill、1 个 MCP；不需要个人安装 |
| `cargo test -p deepagent-app-core --offline --test rea_plugin_artifact rea_builtin_discovers_and_analyzes_without_external_installation -- --ignored --nocapture` | 1 项通过；直接加载桌面内置资源，真实 MCP 发现 170 个工具，JS/ASAR 与 .NET 静态分析通过；Evidence、权限拒绝、目标未执行、禁用投影通过 |
| `pnpm build`（`apps/desktop`） | 通过：运行包 SHA 与锁文件复用校验、前端测试、10 个插件许可门禁、TypeScript 检查、Vite 构建；保留依赖浏览器兼容与大 chunk 警告 |
| `rustfmt --edition 2021 --check`（本轮两个 Rust 测试文件） | 通过 |
| 许可证与依赖记录 | 内置的三份上游/依赖声明与本轮制作 ZIP 内容一致（统一换行后比较）；源目录采用 LF，避免 Windows 与 CI 的内容哈希差异 |

内置 `runtime.zip` SHA-256：`b43f70e32da309147e49b865ed6de06979d016f2002f0e97106d097c29671731`。运行 ZIP 与校验记录为生成产物，未提交。

本轮未构建 MSI/NSIS 安装器、未执行全量 workspace 测试或 clippy；生产 Rust、Tauri command 和前端 DTO 未改动。页面手动刷新/截图验证因用户 Esc 停止电脑操作而未完成，未声称已看到 REA 卡片。真实模型选工具、动态分析及其他平台继续未验收。Codex 工具加载目标设计仍见 [设计文档](codex-plugin-tool-loading-design.md)，本轮内置分发不等于该目标已全部实现。

## 已运行的验收

| 验证 | 实际结果 |
|---|---|
| npm 固定锁文件生产安装 | `npm ci --omit=dev --no-audit --no-fund` 成功；固定 `rea-agents@6.4.0` 的 SHA-512 integrity |
| Windows 原生制品 | 上游验证脚本通过，REA 6.4.0 / Node-API 8 / ABI 1；在托管 Node 24 下实际加载成功 |
| 上游生产 MCP 自检 | `rea.mjs mcp doctor --json` healthy=true；170 个工具、6 个 prompts；版本与 catalog digest 匹配 |
| 干净 ZIP 安装 | 在临时中文、空格路径解包外层 ZIP，再走 `PluginService::scan_plugin/install_from_dir/runtime_projection`；清单、1 个 Skill、1 个 MCP 服务可用 |
| 运行环境 | PATH 上 Node 22 未满足插件要求；现有 RuntimeBroker 选择已安装的托管 Node 24.21.0 |
| 系统工具发现 | 现有 McpService 连接真实 REA，注册 170 个工具；现有 ToolSearchTool 搜索并激活 JavaScript 分析工具 |
| Skill 发现 | 系统 Skills loader 实际加载 `rea-analysis`；无 REA 专属 Skill 解析代码 |
| JavaScript / ASAR | 使用自制 Electron 样本目录及由包内 `@electron/asar` 生成的 ASAR；找到了 `sample-channel` IPC 证据 |
| .NET | `inspect_managed_artifact` 静态检查 PowerShell 7 随附的 `System.Console.dll`，返回该程序集身份 |
| 证据保留 | JS 两次调用的 structuredContent 与返回文本中的 JSON Evidence 相同，含 evidence_id；.NET 返回结构化结果 |
| 目标不执行 | 样本源码带有执行后写文件的标记，分析结束后未出现标记文件 |
| 权限 / 禁用 | 无 Network 权限时路由检查拒绝调用；禁用插件后 Skill 与 MCP 投影均移除 |

实际 REA 6.4.0 静态响应没有统一附带 `_meta.provenance.untrusted_content`。通用 MCP 层会保留服务器实际返回的 `_meta`、structuredContent、文本 annotations、图片及 resource 等原始字段；相关成功/错误回归使用明确的协议 fixture，不声称 REA 每次都产生该标记。

参考目录的旧 Skill 提及 SQLite，但此发布包的实际 MCP catalogue 未发现专门的 SQLite 检查工具；首版没有宣称 SQLite 工具验收通过。Node 24 的 `node:sqlite` 读写属于前一轮 SDK 验证。

## 通用根因修复

1. MCP 字符串展开按字节转字符，第二次展开已替换的中文路径后造成 Windows 工作目录无效。改为按 UTF-8 字符串切片展开，保留中文、emoji 与空格，补重复展开回归。
2. 插件服务名 `plugin__rea__rea` 被工具路由按第一个 `__` 截断成 `plugin`。查找和调用使用已注册 RemoteTool 的所属服务与 local_name；拒绝歧义的重复全名，补插件服务名与前缀服务并存回归。
3. MCP 协议类型与适配器丢弃结构化 Evidence、元数据和非文本内容块。保留扩展字段与原始块，成功仍提供 text/content，失败仍提供 error，补成功与失败结果回归。
4. runtime.zip 缓存只比较大小，同尺寸更新仍保留旧内容。真实回归先复现失败，再复用已有流式 SHA-256，使用 `.payload-sha256` 缓存标记。旧 `.payload-size` 自动失效，第一次使用新构建会重解包一次。

这些修复均位于现有通用层；没有新增 REA Tauri command、协议代理、执行链或审批实现。协议字段依据 [MCP Tools](https://modelcontextprotocol.io/specification/2025-06-18/server/tools)。

## 复跑

```powershell
pwsh -File scripts/package-rea-plugin.ps1 -NodePath '<托管 Node 24.21.0/node.exe>'
$env:REA_PLUGIN_ZIP = (Resolve-Path 'dist/plugins/rea-plugin-0.1.0-windows-x64.zip').Path
$env:REA_RUNTIME_ROOT = '<系统运行环境根目录，包含 sdk/node-24>'
$env:REA_MANAGED_FIXTURE = Join-Path $PSHOME 'System.Console.dll'
cargo test -p deepagent-app-core --test rea_plugin_artifact --offline -- --ignored --nocapture
cargo test --workspace --offline
```

真实制品测试默认 ignored，避免其他开发者运行 workspace 测试时隐式下载安装引擎。它要求已制作的 ZIP、已安装的 Node SDK 和自备的无害程序集；整个测试有 120 秒上限。

本轮最终验证结果：

- 最终 ZIP 的真实制品验收：1 项通过。
- `cargo test --workspace --offline`：2,843 项通过，30 项 ignored；真实制品测试另外显式运行并通过。
- `cargo test -p deepagent-mcp --offline`：55 项单元测试、2 项真实进程测试通过。
- `cargo clippy -p deepagent-mcp -p deepagent-app-core --all-targets --offline -- -D warnings`：通过；没有运行全 workspace 的 all-features clippy。
- 修改的 Rust 文件 `rustfmt --edition 2021 --check` 和 `git diff --check`：通过。
- `cargo fmt --all -- --check`：仅原有 `kernel.rs:384` 格式差异，见下节。
- 没有修改 Tauri command、前端类型或 UI，本轮未运行 `pnpm build` 和桌面安装截图验收。

## 适用范围与尚未验证

- 主程序必须更新到包含本轮通用修复的构建；已有桌面可执行程序不会因修改仓库源码而自动升级。
- Ghidra/JDK 21、IDA、JADX、设备、浏览器、Frida 注入及目标动态执行未验收；上游目录中的可选资源不等于引擎已安装。
- 现有 MCP 适配统一为 Medium / Network，安装扫描对 process.execute 提示 high。安装确认不代表所有动态操作都受逐项审批或沙箱隔离；本次只验证了静态样本和现有权限路由。
- 未完成真实模型自动选工具、桌面内置页面手动显示验证、多聊天隔离、长任务取消/重连、设备写入、跨平台原生运行验证。
- 验收是在网络可用机器上运行；运行树随包完整且启动无需 npm，但没有通过禁网验证宣称整机离线能力。
- skill-creator 的 quick_validate.py 已尝试执行，因工具 Python 缺少 PyYAML 未能运行；系统自身 Skill loader 验证通过。
- 仓库全量格式检查已有无关阻塞：`crates/deepagent-runtime/src/kernel.rs:384` 的长类型需要 rustfmt 换行；本轮不修改该用户工作范围外的文件。

## 安装与使用

使用包含内置 REA 的 Windows x64 桌面构建，具备 Node 24.21.0 SDK 后，在插件页面“DeepAgent 提供”中确认 REA 已启用。无需外部 ZIP 安装。下一轮可以输入：

> 使用 REA 分析这个 ASAR，列出入口、IPC 和对应 Evidence。

完整使用说明与许可证见插件随附 README、LICENSE、LICENSE-REA 和第三方依赖声明。
