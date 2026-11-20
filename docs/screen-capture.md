# 截图：窗口、浏览器与执行步骤

本地对话的 `screen_capture` 按窗口标题截取单个应用窗口。在支持窗口选择的 Wayland 桌面，由系统对话框选择窗口。Windows 使用 Windows Graphics Capture；macOS 使用 ScreenCaptureKit（需要 macOS 14 或更新版本及系统屏幕录制授权）；Linux 分别使用 X11 窗口图像或支持窗口选择的 Wayland ScreenCast portal。

工具通过现有单次审批，向支持工具图片的模型发送截图。应用按图片引用在执行步骤中显示缩略图、打开大图；重载会话时从历史恢复同一文件。

## 浏览器截图

目前可以通过设置中的 MCP 服务接入 Microsoft Playwright MCP。使用隔离浏览器会话，测试不会使用日常浏览器的登录状态。可用的测试配置（Windows 上选择实际可执行的 `npx.cmd`）：

- 传输：`stdio`
- 启动命令：`npx`（Windows：`npx.cmd`）
- 参数逐个添加：`-y`、`@playwright/mcp@0.0.86`、`--browser`、`msedge`、`--headless`、`--isolated`、`--image-responses`、`allow`

等该服务显示已连接后，先在该浏览器会话打开目标页面，再调用 `browser_take_screenshot`。传入 `scale: "css"`；`fullPage: true` 截整页，省略则截当前视口。工具返回的图片沿用与窗口截图相同的模型和执行步骤图片链。该入口暂未接入内置 `screen_capture`，也没有接管已经打开的普通浏览器标签页。

后续按目标绑定补齐明确选择的显示器、浏览器标签页及内置终端会话，继续共用图片结果和历史投影。

## 验收

Windows 以隔离用户目录运行重新编译的桌面程序。桌面场景使用脚本创建的彩色窗口；浏览器场景使用隔离的 Edge 和本地页面。两者都用本地 Responses 测试服务检查模型请求中的图片，并比较图片字节与执行步骤显示的附件。刷新和重开会话后应从 SQLite 恢复同一引用，不能再发起截图。macOS 和 Linux 仍需在各自的桌面环境验收。
