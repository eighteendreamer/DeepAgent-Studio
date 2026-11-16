# Windows 品牌安装引导程序

基于用户选定的 `A-abyss.html`：一键安装、二级自定义、进度、完成四页。
改用主程序一致的浅色灰阶主题（primary `#111827` / hover `#000000` /
text `#1F2937` / secondary `#6B7280` / border `#E5E7EB`），760×480 固定无边框窗口。
Button、Input、Label、Checkbox、Progress 复用桌面端 shadcn/ui 边界。

## 边界

- 仅 Windows x64；原 NSIS 负责安装、注册卸载与升级，未另造安装数据库。
- `currentUser` 与原 NSIS 默认一致，默认 `%LOCALAPPDATA%/DeepAgent Studio`。
  不做管理员提权；以管理员身份运行会拒绝，避免把后续智能体/终端也提权。
  不把当前用户安装迁移为全用户安装。已有安装只允许原位置升级。
- 创建任何 WebView 之前检查 Microsoft 公布的 HKCU/HKLM WebView2 `pv` 注册表值。
  缺失时直接释放并等待原 NSIS 向导。此路径不依赖 HTML。
  原 NSIS 默认会联网下载缺少的运行时，不承诺离线或策略阻断时也能成功安装。
- 静默参数为 `/S /NS /D=绝对路径`；`/D` 不带引号且位于最后。不传 `/R`，
  避免 NSIS 与完成页重复启动。拒绝网络、盘符根目录、越界路径与重解析点。
- 进度是安装清单文件大小观察值，不是 NSIS 的精确百分比；升级时旧文件可能使估值提前到 95%。
  只有 NSIS 退出成功、所有清单文件 SHA-256 校验成功、选项应用成功才显示完成。
  无伪造文件名、运行时间进度或固定“420 MB”。
- `check_env`、`install(options)`、`apply_options`、`launch_app` 使用同一 Rust 状态；
  选项应用与启动不能绕过安装，安装/启动期间拒绝关闭和重复安装。
  选项失败可单独重试，不重跑 NSIS。启动失败保留安装器显示错误；进程成功创建且
  没有立即退出后，安装器退出，主程序现有 splash 负责完整启动动画。
- PATH 仅修改 HKCU，保留 REG_SZ/REG_EXPAND_SZ 类型；登录启动默认关闭，打开的是正常窗口，
  不声称有尚不存在的“托盘常驻”。自动更新沿用主程序，不新增无效开关。
- 原 `installer/hooks.nsh` 逐字不变。新增 `bootstrapper-hooks.nsh` 引用原文件，
  打包小型 `deepagent-install-options.exe`，显式卸载后只撤销本安装器记录为自己新增的
  PATH/Run 值，不覆盖其他程序/用户后续变更；`/UPDATE` 升级保留选项。
  失败保留归属记录和 helper 以便修复；不删除用户数据。
- 不读取模型密钥，不调用模型，不包含远程网页、外部字体或任意命令执行 IPC。

## 开发与验证

```powershell
cd apps/installer
pnpm install --frozen-lockfile
pnpm build
pnpm test
pnpm tauri dev
```

从仓库根运行原生测试（NSIS 测试只操作独立临时目录）：

```powershell
cargo test --locked -p deepagent-installer --features gui --lib
# 先安装 NSIS，或将 DEEPAGENT_NSIS 指向已有 makensis.exe
cargo test --locked -p deepagent-installer --features gui --test nsis -- --ignored --nocapture
```

NSIS 夹具验证中文/空格路径、非零退出、缺文件，以及扩展卸载 callback 的
显式卸载清理与 `/UPDATE` 保留分支；它使用不访问注册表的测试 helper，
不会执行原有用户数据清理 hook。注册表归属算法另用内存后端测试。

未提供 payload 的 debug 构建可预览界面，但真实安装按钮禁用；release 构建缺包直接失败。
内核 workspace 默认只编译/测试安装器的轻量编排与 helper；`gui` 是 Windows 可选特性，
不把 Linux WebKit 开发依赖强加给内核 CI。

## 发布

1. 正常执行 `apps/desktop` 的 `pnpm tauri build`。前置步骤同时构建 CLI 与选项卸载 helper。
2. 安装本目录前端依赖，执行（路径以实际产物为准）：

```powershell
./apps/installer/scripts/build-bootstrapper.ps1 `
  -Nsis './apps/desktop/src-tauri/target/release/bundle/nsis/DeepAgent Studio_0.0.8_x64-setup.exe' `
  -NsisScript './apps/desktop/src-tauri/target/release/nsis/x64/installer.nsi'
```

脚本从当前 Tauri 生成的 NSIS 文件清单生成大小/摘要 manifest，将原 NSIS 嵌入 EXE。
无法识别指令、版本/安装范围不一致、缺文件时停止，不静默打空包。
输出 `target/installer-package/output/DeepAgent-Studio-Setup-x64.exe`。
二进制、payload、缓存、日志不提交。

Release workflow 将它作为附加资产上传到原 Release，不覆盖原 NSIS 或 updater JSON。
可选 Secrets `WINDOWS_CERTIFICATE`（base64 PFX）、`WINDOWS_CERTIFICATE_PASSWORD`
用于对外层 Setup 做 Authenticode 签名；未配置则明确输出 unsigned。Tauri updater 签名仍独立保留。
本地不自动发布、不读取签名私钥、不在用户机器实际安装/覆盖现有产品来验证。

依据：[Tauri Windows 安装配置](https://v2.tauri.app/distribute/windows-installer/)、
[NSIS 命令行](https://nsis.sourceforge.io/Docs/Chapter3.html)、
[Microsoft WebView2 部署检测](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/distribution)。
品牌引导状态机、文件观察估值、选项归属记录是本项目本地设计；通过 Rust、NSIS 夹具及浏览器测试验证。
