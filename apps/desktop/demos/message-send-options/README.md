# 消息发送连贯动效 · 六方案预览

本地设计预览。A–F 在同一输入框/气泡布局上对比真实动作，F 可调时长、回弹、弧线。用户已确认 F 微调版，正式聊天与此预览现在共用 `src/components/ui/messageSendFlight.ts`，消息发送和持久化仍沿用原链路。

在 `apps/desktop` 运行 `node demos/message-send-options/build.cjs` 生成 `.codex/previews/choice-preview-2026-10-22-0652-message-send.html`。HTML 内嵌 React、GSAP、库图标和样式，可离线打开。

复用项目 `Button`、Radix Slider、连续圆角构建规则、`springEase` 和减少动态效果设置。图标来自项目已安装的 Lucide。轨迹/黏连 SVG 是过渡连接几何，不是图标。

F 微调版：运行 `node demos/message-send-options/build.cjs --refined`，生成 `choice-preview-2026-10-22-0652-message-send-v2.html`。已交付的 HTML 保留原样。左侧是原 F，右侧复用模型菜单的 `SHARE_MOTION.open` 双轴弹簧和 57ms 高度延迟，减少起步压缩，送达时只有轻微过冲。两版共用 `SendCard`；确认版的实际时间线已提取到正式组件，避免两处手感不一致。

正式接入只关联 `Composer` 本次提交与 App 实际生成的 user message id。首页切换聊天页时保留输入文字快照；气泡出现后隐藏真实气泡，飞行完成原子交接。Framer Motion 11 的 `popLayout` 会覆盖直属子元素的 ref，注册因此放在稳定的消息行上；布局投影还会写入 inline visibility，隐藏状态使用独立属性与 CSS 规则维护。导航、缩放、卸载、超时和减少动态效果均会清理快照及隐藏状态。附件沿用原有附件行。

浏览器回归：启动前端后运行 `python scripts/check-message-send.browser.py`，使用项目现有浏览器后端回退，仅替换未配置的设置入口，不连接真实模型。覆盖首页首条、相同文字的后续消息、长文字/换行/emoji、IME、窗口缩放、导航清理、减少动态效果与 375/768/1440 截图，并逐帧检查飞行期间只有一份可见文字及落位交接。

实现参考：[GSAP 时间线](https://gsap.com/docs/v3/GSAP/gsap.timeline()/)、[GSAP 对位置/尺寸变化的 FLIP 说明](https://gsap.com/docs/v3/Plugins/Flip/)。本地预览使用首尾坐标测量与 GSAP 时间线，不引入新插件。
