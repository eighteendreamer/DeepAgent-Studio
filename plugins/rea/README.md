# REA 插件

DeepAgent Studio 插件包装 0.1.0；上游 `rea-agents@6.4.0`；Windows x64；Node 24.21.0。

## 安装与使用

在插件页面选择“从 ZIP 安装”，安装 `rea-plugin-0.1.0-windows-x64.zip` 并启用。系统解包随附的生产运行树，使用兼容的本地 Node 或托管 Node 24。下一轮对话通过 Skill 和工具搜索发现 REA。

主程序需要使用包含本轮 MCP 通用修复的构建，并在运行环境页面安装 Node 24.21.0 SDK。旧构建存在插件工具路由和中文路径问题。安装扫描会提示 `process.execute` 高风险声明，按现有安装页面审阅确认。

示例：“使用 REA 分析这个 ASAR，说明入口、IPC 和证据来源。”或“使用 REA 检查这个 .NET 程序集，列出类型和方法。”

用户安装时无需 npm install。独立运行包包含编译产物、生产依赖、桥接资源和 Windows 原生模块；不包含 Node 本身或外部分析引擎。

## 制作

从仓库根目录使用 PowerShell 7 执行：

```powershell
pwsh -File scripts/package-rea-plugin.ps1 -NodePath '<Node 24.21.0 的绝对路径>'
```

制作阶段从固定锁文件安装生产依赖并验证原生制品；输出到 `dist/plugins/`，同时生成 ZIP 校验信息和依赖声明。`node_modules` 和输出 ZIP 不提交。

## 外部依赖与边界

- Ghidra 使用 `GHIDRA_INSTALL_DIR` 和 `JAVA_HOME`；完整 JDK 与 Ghidra 由用户提供，当前系统托管 JDK 17 不等于 REA 文档所用 JDK 21。
- IDA、JADX、设备连接、浏览器观察、Frida 等能力按各自环境要求工作。缺少可选依赖不会被声明为已经验证。
- 本系统现有 MCP 调用通过工具注册与权限检查；当前 MCP 工具统一采用 Medium 风险和 Network 权限，不提供 REA 各项动态操作的独立风险分类。插件清单的 permissions 字段用于展示和安装扫描，不能代替执行时的权限判定。
- REA MCP 服务持有会话状态；不同聊天不能盲目复用 active target。静态接入通过后，动态执行、注入、设备写入与多聊天隔离需要分别验收。

验证结果记录在仓库的 `docs/rea-plugin-validation.md`；它区分已运行的静态分析与尚未验证的能力。

插件包装代码使用项目 Apache-2.0 许可；REA 使用 MIT，具体见随包的 `LICENSE-REA` 和第三方声明。
