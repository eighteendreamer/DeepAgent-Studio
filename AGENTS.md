# DeepAgent 项目规则（AGENTS.md）

本文件是 DeepAgent（monorepo，核心为 `apps/desktop` Tauri 桌面应用）的开发规则。所有开发行为必须遵守。

## 核心规则：前端改动必须「运行」才能看到效果

桌面应用是 **Tauri v2** 打包的 —— 前端代码会被编译进 Rust 应用本体。**只改代码、不运行，用户在旧窗口/旧安装包里永远看不到改动**。任何前端修改完成后，必须把改动跑起来让用户验证，不允许直接说"完成了"。

### 开发预览（日常开发用）

```bash
cd apps/desktop && npm run tauri dev
```

- 首次运行会弹 pnpm 确认（重建 node_modules 布局），选 **Y**。
- 改动后 **vite HMR 热更新即时生效**，窗口无需重启。

### 非交互环境（CI / 后台任务）绕过 beforeDevCommand

`pnpm dev` 的依赖校验在无 TTY 环境会失败（`ERR_PNPM_ABORTED_REMOVE_MODULES_DIR_NO_TTY`）。绕过方式：

```bash
# 终端 1：起 vite（端口必须 1420，与 tauri.conf.json 的 devUrl 一致）
cd apps/desktop && npx vite --port 1420 --strictPort

# 终端 2：编译并运行 Rust 外壳（devUrl 由应用自身读取，不依赖 tauri CLI）
cd apps/desktop/src-tauri && cargo run --no-default-features
```

### 打包 / 发布

- 本地出安装包：`cd apps/desktop && npm run tauri build`
- 正式发布：打 tag 走 GitHub Actions（详见 `apps/desktop/RELEASE.md`），前端改动必须重新打包才进安装包

## 包管理器警告（重要，踩过坑）

`apps/desktop` 同时存在 `package-lock.json`（npm）和 `pnpm-lock.yaml`（pnpm）：

- 项目内 `node_modules/pnpm` 是 **pnpm v11**（tauri CLI 用它），全局 PATH 可能是 **pnpm v10** —— 版本分裂。
- **非交互环境下不要跑 `pnpm install`**：v11 校验失败时可能清空 `node_modules` 顶层（只留 `.pnpm` store），导致 vite 模块丢失。
- node_modules 损坏时恢复：`cd apps/desktop && npm install`（按 package-lock 重建扁平布局），随后 vite 即可用。

## 浏览器端验证技巧（无需 Tauri 也可测真实前端）

用 Playwright（Python）打开 vite dev 地址，注入 mock 后端即可驱动完整应用：

```python
ctx.add_init_script("""
    localStorage.setItem('onboarding_complete', 'true');   # 跳过 onboarding
    window.__TAURI_INTERNALS__ = {                          # mock Tauri 后端
        invoke: async (cmd) => {
            if (cmd === 'get_settings') return { configured: true };
            if (cmd === 'get_balance') return { is_available: false, infos: [] };
            if (cmd === 'get_active_project') return null;
            return [];
        }
    };
""")
```

注意：某些组件对 mock 返回值形状敏感（如 BalanceChip 需要 `infos` 数组），缺字段会白屏，先加特例再跑。

## 样式与组件规范

- **颜色必须用主题 token**（`tailwind.config.js` 定义），禁止硬编码色值：`bg-sidebar-bg`、`bg-elevated-bg`、`bg-hover-bg`、`border-border-theme`、`text-text-base`、`text-text-secondary`、`text-primary` 等。
- 滑动药丸指示器（`SettingsSidebar.tsx` 设置侧栏 + `Sidebar.tsx` 主侧栏顶部导航）：按钮必须 `relative z-[1]`（在药丸 `z-0` 之上），否则白块会遮住文字 —— 这是踩过的坑。主侧栏无 surface 激活时药丸停靠「新对话」。

## 统一界面语言（2026-08 全局改造，新代码必须遵守）

用户明确偏好：**无边框、无浮起阴影、静默着色（bg-black/5）**。四条映射：

1. **浮层容器**（下拉菜单/弹层面板）：无 border，`bg-elevated-bg` + `shadow-[0_6px_24px_rgba(0,0,0,0.10)]`
2. **触发按钮/胶囊**：无 border，`bg-black/5`，hover 同色
3. **列表项 hover/激活**：统一 `bg-black/5`（hover 与激活同色；激活可加 font-medium）
4. **输入区**（Composer）：无 border，`bg-elevated-bg` + `shadow-[0_2px_12px_rgba(0,0,0,0.07)]`，聚焦 `focus-within:shadow-[0_4px_18px_rgba(0,0,0,0.12)]`

**动效语言**：150ms（按钮/行 hover）· 300ms ease-out（滑块/面板）· 400ms（大面板），定义在 `components/ui/motion.ts`（MOTION.fast / standard / smooth）。

**组件化（强制）**：新 UI 一律使用 `components/ui/` 组件库，禁止手写这些类名组合：
- `Panel` — 浮层容器
- `ListItem`（selected 属性）— 列表行/下拉行
- `TintButton`（variant="primary"）— 着色按钮
- `IconButton` — 图标按钮
- `InputSurface` — 输入区容器
- `useSlidingIndicator` + `SlidingPill` — 侧栏滑动药丸（hoverSelector/activeSelector 参数）
组件用 `cn()`（components/shadcn/utils.ts）合并类，差异通过 className 覆盖。

**禁止**：`border border-border-theme` 用于交互元素（浮层/按钮/选项卡片）、`hover:bg-gray-50/100`、激活态 `bg-gray-100`、浮层 `bg-white`（用 `bg-elevated-bg` 否则深色刺眼）。颜色一律 Tailwind 类，不写死 hex。
- 导航/设置标签文案走 i18n：`t('settings.tabs.' + id)`，中文文案集中在语言文件里，不要硬编码。
- 交互样式/动效类需求：先出 HTML 选择预览页（`choice-preview-YYYY-MM-DD-HHMM.html`，≥6 个方案），用户选定落地并验证后删除临时文件；探索过程文件（如 `sliding_hover_variations.html`）用户保留时不要擅自删。

## 验证闭环

- 改动后：`npx tsc --noEmit` 类型检查 + 实际运行/浏览器验证 + 关键交互断言，证据齐全才算完成。
- 桌面端环境类问题（pnpm/node_modules）参见本文件「包管理器警告」，不要重复踩坑。
