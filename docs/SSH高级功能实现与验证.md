# SSH 高级功能实现与验证（R069）

日期：2026-11-20。范围：完整升级方案的 S09、S10、S14、S23、S24、S25。

## 功能入口与行为

| 项目 | 已接入的行为 | 边界 |
|---|---|---|
| S09 加入 Agent 身份 | 连接设置和侧栏编辑器提供私钥文件、口令、驻留秒数、指纹预览和明确授权；后端同时支持私钥正文 | 驻留时间为 1～86400 秒；指纹重新核验；已存在的身份保留原约束；拒绝 constrained add 时不退回无限驻留；不清除用户已有身份 |
| S10 Agent 转发 | 每连接独立开关与 provider；PTY 创建时先检查本机 Agent、请求并等待服务端确认；复用当前 SSH transport 的 Agent channel | 默认关闭，与登录认证独立；远端仅可查询身份、请求签名和 session-bind，不能增删或解锁身份；最后一个 PTY 关闭、断线、transport 换代后结束桥接 |
| S14 旧算法兼容及协商信息 | 每连接显式兼容 DSA、RSA/SHA1、SHA1 DH/KEX 和 MAC；工作台展示实际 KEX、主机密钥、cipher、双向 MAC | 现代算法优先，默认移除弱算法；私钥和 Agent 登录共同遵守兼容开关；不做失败后的自动降级；协商回调不保存 shared secret |
| S23 本地转发 | 工作台“SSH 转发与协商信息”面板创建、停止本地监听，复用 direct-tcpip channel；显示实际端口、状态、活动连接、累计字节和有界记录 | 默认回环地址、端口 0 自动分配；非回环须明确确认；端口冲突直接报错；转发任务保有连接资源租约 |
| S24 远程转发与 SOCKS | 远程 tcpip-forward/cancel，端口 0 回显实际分配；SOCKS5 CONNECT 支持 IPv4、IPv6、域名 | 域名交给远端解析；拒绝 UDP/BIND 和不支持的认证；远程回调只匹配已分配或正在发送的任务；停止失败的任务不会撤销其他监听 |
| S25 X11 | 本机 X server/Xauthority 探测；PTY 请求确认；每 transport 假 cookie；校验后替换成本机真实 cookie 并转发 | 仅本机 display；不可用和服务端拒绝明确报错；错误 cookie 不连接本机 X server；大小端 setup 均处理；最后一个 PTY 关闭后结束通道 |

保存配置不会打开监听或向 Agent 加入私钥。转发任务只属于当前 transport generation，不在重连时自动重放。每连接最多 64 份任务记录、32 条活动转发通道，每任务最多 32 条事件。

私钥与 Xauthority 读取上限为 1 MiB，Agent 帧上限为 256 KiB；私钥、口令、真实 X11 cookie 不进入返回 DTO、配置新增字段或新日志。

## 源码与跨层契约

- `crates/deepagent-ssh/src/advanced.rs`：默认策略与旧算法控制、协商投影。
- `agent.rs`、`extensions.rs`、`forwarding.rs`、`x11.rs`：身份加入及 transport 资源边界。
- `config.rs`、`service.rs`、`transport.rs`、`pty.rs`：持久化、generation、协议回调和 PTY 接入。
- `apps/desktop/src-tauri/src/lib.rs`、`apps/desktop/src/api.ts`：同步请求/返回契约和七个高级能力命令。
- `SshAdvancedFields.tsx`：两个既有编辑器共用；`SshForwardPanel.tsx`：既有远程工作台入口，保留终端标签挂载。

新增字段使用 serde 默认值；旧配置默认关闭高级选项，旧更新请求省略字段时保留原配置。前端同步四语言资源，敏感操作只使用桌面 IPC，浏览器环境不会伪造后端成功。

## 已取得的验证

- `tsc --noEmit --incremental false`：当前工作树和独立提交版本均通过。
- 前端相关专项：70 项通过；完整 `pnpm build` 的前置测试合计 582 项通过、0 失败，随后 Vite 打包因堆内存上限退出。
- SSH 专项：真实 `deepagent-ssh` 包的 lib、advanced_configuration、forwarding、protocol_extensions，69 项通过、0 失败。
- 当前工作树完整 SSH 包回归：406 项通过、0 失败、1 个需外部服务器的 PTY 用例忽略；不与上述专项重复相加。
- 从 HEAD 与本轮差量生成的独立提交版本：完整 SSH 回归 377 项通过、0 失败、1 个外部服务器 PTY 用例忽略；前端专项 70 项与类型检查通过。隔离版本排除了此前尚未提交的删除和 Terminal 等改动，因此与当前工作树的测试数不同，不能合并计数。
- 实际组件手动检查：使用真实组件和语言资源、隔离模拟 IPC，检查指纹授权、X11 不可用提示、转发启动/停止、非回环确认、SOCKS 表单及协商信息。此证据不等于原生 IPC 或外部 SSH 验收。
- 相关 Rust 文件 rustfmt 检查通过；Tauri 大文件仅解析成功，未将其整体重排，也未视为原生编译通过。独立拟提交源码副本的离线锁定 metadata 通过。

首次 Rust 编译发现 Agent 公钥比较的 `Cow<PublicKey>` 类型错误，已修正后取得上述通过结果；编译错误不计作行为回归 RED。

## 尚需补齐的验证

当前工作树 163 份和独立提交版本 159 份冻结 Rust 输入在测试后逐一复核无漂移。独立版本仅验证本轮差量及必要前置修正，不代表整个 workspace 已通过。

Vite 先后触及设定的 1024 MiB、2048 MiB Node 堆上限并退出，完整前端构建未通过；没有继续提高内存上限。桌面后端离线 metadata 因锁定 `async-trait 0.1.92` 在既有 vendor 中缺失而失败；没有修改锁文件绕过此问题。Tauri 原生编译、真实 OpenSSH/Pageant 新增身份及远端二次 SSH/Git、外部 X server/旧设备、三平台和性能验收尚未完成，六项不能标为整体验收完成。

## 资源与修改隔离

本轮源码位于 `D:\code\DeepAgent-Studio-worktrees\remote-transfer-notices`。Cargo home、target、TEMP、日志、审查副本均在 D 盘；复用 `r057-ssh-target`，单任务构建，禁用 incremental 和 debug symbols。启动仍要求可用虚拟内存至少 12 GiB、物理内存至少 4 GiB、D 盘空间至少 12 GiB；运行中低于 6 GiB 虚拟或 4 GiB 物理时只停止已核验的本轮进程子树。

既有 Terminal、安全删除、AppCore、模型等未提交修改保留。拟提交副本从 HEAD 与本轮基线差量生成，仅额外保留此前用户已授权的 AppCore russh 测试依赖 `0.55.0 → 0.65.2` 前置修正；尚未据此宣称整个工作树已验证。

证据：`D:\DeepAgent-Build\remote-upgrade\evidence\r069-ssh-six\`。
