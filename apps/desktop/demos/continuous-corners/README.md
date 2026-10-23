# 连续圆角确认稿

仅预览首页输入框和底部工具栏的圆角；未接入正式页面。

- 默认输入框半径 24px、工具栏 20px，与当前 `Composer.tsx` 保持一致。
- 在相同半径下切换普通圆弧与连续圆角，默认平滑度 60%。
- 可调节半径、平滑度、显示轮廓；支持键盘和减少动态效果设置。
- 复用项目的 shadcn Button、Slider、InputSurface、按压动效和 Lucide 图标。
- 圆形按钮与胶囊不改变。示意界面不执行模型请求。

## 预览和导出

从 `apps/desktop` 启动项目已有 Vite 服务后，访问 `/demos/continuous-corners/`。

在 `apps/desktop` 执行 `node demos/continuous-corners/build.mjs`，生成可离线直接打开的单文件 HTML：

`target/continuous-corners-preview/continuous-corners.html`

生成文件内嵌脚本和样式；无需安装新依赖，不修改主应用构建入口。

## 依据与限制

- [Apple CALayerCornerCurve](https://developer.apple.com/documentation/quartzcore/calayercornercurve) 区分 circular / continuous。
- [Figma cornerSmoothing](https://developers.figma.com/docs/plugins/api/properties/nodes-cornersmoothing/) 给出 0.6 对应 iOS 7 图标 squircle 的参考。
- `curve.ts` 适配 [figma-squircle](https://github.com/phamfoo/figma-squircle/blob/main/src/draw.ts) 的等半径路径算法，保留 MIT 许可证。

这是用于确认视觉方向的网页近似，不宣称与苹果当前原生控件逐像素一致。最终半径与平滑度属于本地设计参数；尚未更改 Tauri 页面或全局圆角规则。
