# REA 插件

DeepAgent Studio 插件包装 0.1.0；上游 `rea-agents@6.4.0`；Windows x64；Node 24.21.0。

## 内置与使用

REA 随 Windows x64 桌面版内置，在插件页的“DeepAgent 提供”和“开发工具”中显示。无需单独安装插件 ZIP；保持 REA 启用，下一轮对话通过 Skill 和工具搜索发现能力。系统解包随附的生产运行树，使用兼容的本地 Node 或托管 Node 24。

主程序需要使用包含 REA 内置资源和 MCP 通用修复的构建，并具备 Node 24.21.0 运行环境。旧构建存在插件工具路由和中文路径问题。插件声明不会代替执行时的权限和审批。

示例：“使用 REA 分析这个 ASAR，说明入口、IPC 和证据来源。”或“使用 REA 检查这个 .NET 程序集，列出类型和方法。”

用户安装时无需 npm install。独立运行包包含编译产物、生产依赖、桥接资源和 Windows 原生模块；不包含 Node 本身或外部分析引擎。

## 制作

从仓库根目录使用 PowerShell 7 执行：

```powershell
pwsh -File scripts/package-rea-plugin.ps1 -NodePath '<Node 24.21.0 的绝对路径>'
```

插件清单与 Skill 的唯一源目录为 `apps/desktop/src-tauri/resources/plugins/rea/`；运行包的 package.json 和锁文件位于 `plugins/rea/runtime/`。制作脚本从固定锁文件安装生产依赖、验证原生制品，生成内置 `runtime.zip` 和校验记录，同时保留 `dist/plugins/` 的可选独立 ZIP 输出。

Windows x64 的 `pnpm dev` / `pnpm build` 会先执行 `prebundle-rea`：已有有效运行包时直接复用，否则使用当前 Node 24.21.0 或已安装的托管 SDK 调用同一制作脚本。首次准备需要 npm 注册表和 PowerShell 7；最终用户使用随包产物，不运行 npm。`node_modules`、runtime ZIP 和校验产物不提交。

其他平台当前只包含插件说明与 Skill，尚未生成或验证该平台的 REA 运行包；不能据此宣称跨平台分析能力可用。

## 外部依赖与边界

- Ghidra 使用 `GHIDRA_INSTALL_DIR` 和 `JAVA_HOME`；完整 JDK 与 Ghidra 由用户提供，当前系统托管 JDK 17 不等于 REA 文档所用 JDK 21。
- IDA、JADX、设备连接、浏览器观察、Frida 等能力按各自环境要求工作。缺少可选依赖不会被声明为已经验证。
- 本系统现有 MCP 调用通过工具注册与权限检查；当前 MCP 工具统一采用 Medium 风险和 Network 权限，不提供 REA 各项动态操作的独立风险分类。插件清单的 permissions 字段用于展示和安装扫描，不能代替执行时的权限判定。
- REA MCP 服务持有会话状态；不同聊天不能盲目复用 active target。静态接入通过后，动态执行、注入、设备写入与多聊天隔离需要分别验收。

验证结果记录在仓库的 `docs/rea-plugin-validation.md`；它区分已运行的静态分析与尚未验证的能力。

插件包装代码使用项目 Apache-2.0 许可；REA 使用 MIT，具体见随包的 `LICENSE-REA` 和第三方声明。
