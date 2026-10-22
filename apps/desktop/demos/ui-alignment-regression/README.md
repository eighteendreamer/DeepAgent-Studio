# 界面对齐与消息编辑回归场景

复用真实的 App、CustomTitleBar、Composer、ChatTimeline 和消息组件，不调用模型。

在桌面前端目录运行已有的浏览器夹具服务器：

```text
node demos/sidebar-d-regression/serve.mjs
```

- `/demos/ui-alignment-regression/index.html#session=layout-d`：完整界面，模拟 decorum 1.1.1 的 Windows 标题栏 DOM；窗口控制按钮只作占位。
- `/demos/ui-alignment-regression/edit.html`：短消息、长消息、忙碌限制、取消及重发元数据验证。

检查项目：

1. 顶部首按钮和功能栏共用中心线，桌面按钮外围顶部/左侧各 10px。
2. 功能栏上方六个按钮均为 36×36px，图标框 20×20px，按钮间隔 10px；底部两个按钮保持相同规格。
3. 输入 `/` 或 `@`，弹出面板左右边缘与输入框一致，滚动条位于圆角外壳内部。375/768/1440 宽度不横向溢出。
4. 矮窗口中，面板最高不超出工作区顶部的 8px 留白。方向键滚动至选项后，Tab/Enter 可以选中；Escape 关闭面板。
5. 编辑、取消和重新编辑只出现一个输入框，回答保持原始文字尺寸；不在展开后因移除旧操作行再次跳动。
6. 长消息输入框不超过 144px；空白内容及忙碌状态禁止重发；Escape 恢复原文并把焦点交回编辑按钮；Ctrl+Enter 重发保留技能和提及。

这些是浏览器夹具，不能替代 Tauri 原生窗口拖拽、缩放和窗口控制验证。
