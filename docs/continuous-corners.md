# 全局连续圆角

桌面 UI 沿用确认稿的 24px 输入框参考半径和 60% 平滑度。其他控件保留原来的大小层级；圆形按钮、胶囊和比例形变滑块保留其轮廓。

## 单一规则

`apps/desktop/src/theme/continuousCorners.ts` 保存参数，并供以下入口共用：

- `scripts/continuous-corners.postcss.mjs` 在 Tailwind 生成工具类后转换所有 CSS 圆角，包括任意值、单角覆盖、伪元素和画布样式。
- 行内样式调用 `continuousCornerStyle`，动画中的新半径调用 `continuousRadius`。
- 从 `getComputedStyle` 读到的半径已经转换，不得再次缩放。菜单动画同时过渡曲率，保证圆形/胶囊触发器可以还原。
- 按钮、菜单项和滑动 hover 高亮共用 `styles.css` 中直接声明的 `--ui-interactive-radius`（12px），使用处保留 12px 兜底。这个 UI 令牌不由构建插件注入，避免旧开发服务未重载插件时出现变量缺失、半径变成 0 的情况。原先 2–12px 的标准小圆角统一到这一层级；显式圆形、胶囊、直角和更大的卡片圆角保留。预览 HTML 的新增 CSS 也必须经过同一 PostCSS 转换，不能直接拼在已编译的 CSS 后面。

## 网页实现与兼容

使用浏览器原生 `corner-shape: superellipse(...)`，让背景、边框、阴影和原有 overflow/focus 行为一起工作。不使用全局 DOM 观察器或给组件统一套 `clip-path`。

这是本地的原生 CSS 近似映射，和确认稿中的 SVG/Figma 路径并非同一条数学曲线：将圆角作用范围扩大至半径的 `1 + smoothing` 倍，并求解超椭圆指数，使 45° 处的内缩量与原 SVG 相同。平滑度 0.6 对应约 1.77747 的 `superellipse` 参数，不直接把 0.6 当作 CSS 指数。控件较短时，浏览器按盒子尺寸约束圆角，局部轮廓可能与 SVG 保留平滑度算法存在差异。

不支持 `corner-shape` 的旧 WebView 保留原有普通圆角及尺寸，不改变布局。当前 Windows 桌面使用 WebView2，打包本身不锁定用户运行时版本。

## 验证

在 `apps/desktop` 运行 `pnpm test:continuous-corners`。测试覆盖实际 Tailwind 输出、普通 CSS、单角覆盖、继承、圆形/胶囊例外、行内样式和旧运行时回退，并检查原生近似映射的 45° 内缩量。

运行中的页面可用 `python scripts/check-interactive-corners.browser.py --url <页面地址>` 检查共用 hover/按钮半径，同时确认静态卡片、输入框、显式直角与胶囊没有被统一规则改写。

界面验证需覆盖输入框、普通按钮、浮层、菜单、弹窗、画布控件和主题；检查 375 / 768 / 1440 宽度、键盘焦点、Esc 关闭及减少动态效果。Tauri 生产包还需在目标 WebView2 上复查。

参考：[Chrome corner-shape 发布说明](https://developer.chrome.com/release-notes/139)、[CSS 超椭圆定义](https://www.w3.org/TR/css-borders-4/#corner-shape)、[Figma 平滑度说明](https://developers.figma.com/docs/plugins/api/properties/nodes-cornersmoothing/)。
