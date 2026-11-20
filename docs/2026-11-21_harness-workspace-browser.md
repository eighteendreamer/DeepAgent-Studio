# Harness 控制右侧浏览器

浏览器标签由 Tauri 创建和维护，地址栏与 `workspace_browser` 工具指向同一个子 WebView。关闭右侧浏览器标签会销毁 WebView。本地会话打开并显示唯一的内置浏览器标签后，下一轮对话会注册工具。调用进入现有 Harness 权限管线；关闭标签或关闭浏览器后，已经注册的工具也会拒绝后续调用。远程对话和 Windows Sandbox 的运行不注册此工具。

「设置 → 浏览器」管理浏览器开关、截图和域名范围。审批默认跟随系统权限，也可以只对浏览器工具要求逐次人工审批；全局禁用规则仍然有效。允许名单为空时不缩小范围；有条目时只允许这些域名及其子域名，阻止名单优先。域名设置用于内置浏览器的顶层页面及工具访问，不拦截页面子资源。

工具接受 `navigate`、`snapshot`、`click`、`fill`、`scroll` 和 `screenshot`。`snapshot` 返回页面文字与元素引用。新快照、切换文档或元素被移除后，旧引用失效。截图是浏览器视口，走常规图片附件，执行步骤保存调用参数和结果并显示预览。仅支持图片工具结果的模型路由会看到 `screenshot` 操作。首版对页面的操作限定在顶层文档中的常用元素；跨域 iframe、验证码、文件上传和弹窗不在范围内。

Windows 原型先通过 Playwright MCP 的 CDP 连接验证可控性，正式功能使用进程内的 Tauri WebView 能力，无需安装 Playwright 或开启调试端口。截图分别调用 WebView2、WKWebView、WebKitGTK 的视口截图接口；Windows 已通过原生测试，macOS 和 Linux 需要在对应系统上验收。

Windows 复测：先在 `apps/desktop` 运行 `pnpm build`，在仓库根目录运行 `cargo build --offline --manifest-path apps/desktop/src-tauri/Cargo.toml --features custom-protocol`。用 `DEEPAGENT_TEST_PLAYWRIGHT` 指向测试环境已安装的 Playwright 包，然后运行：

```text
node apps/desktop/scripts/check-workspace-browser.cjs apps/desktop/src-tauri/target/debug/deepagent-desktop.exe
```

脚本启动独立数据目录、仅本机访问的网页和模拟 Responses 供应商。Playwright 负责触发对话和查看应用结果；对测试网页的导航、读取、填写、点击、滚动与截图由 DeepAgent Harness 调用工具。脚本核对工具请求与响应、地址栏、系统审批、域名及工具拒绝规则、禁用开关、图片字节一致性和关闭页面后重新载入的历史记录。
