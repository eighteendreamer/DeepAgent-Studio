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

## Git 提交注意：`components/ui/` 被 `.gitignore` 忽略

根目录 `.gitignore` 第 51 行有 `ui/` 规则，会误伤 **`apps/desktop/src/components/ui/`** 整个组件库目录。普通 `git add` 不会纳入 `Panel.tsx`、`SlidingMenuList.tsx` 等文件。

- 提交 ui 组件时必须：`git add -f apps/desktop/src/components/ui/`
- 改完 ui 组件后务必 `git status` 确认文件已进入暂存区，否则 commit 会漏文件

## 样式与组件规范

- **颜色必须用主题 token**（`tailwind.config.js` 定义），禁止硬编码色值：`bg-sidebar-bg`、`bg-elevated-bg`、`bg-hover-bg`、`border-border-theme`、`text-text-base`、`text-text-secondary`、`text-primary` 等。
- **侧栏**滑动药丸（`SettingsSidebar.tsx` 设置侧栏 + `Sidebar.tsx` 主侧栏顶部导航）：按钮必须 `relative z-[1]`（在药丸 `z-0` 之上），否则白块会遮住文字 —— 这是踩过的坑。主侧栏无 surface 激活时药丸停靠「新对话」。侧栏用 `useSlidingIndicator` + `SlidingPill`（hoverSelector/activeSelector 参数）。
- **浮层菜单**滑动药丸用 `SlidingMenuList`（见下节），与侧栏是两套入口，不要混用。

## 统一界面语言（2026-08 全局改造，新代码必须遵守）

用户明确偏好：**无边框、无浮起阴影、静默着色（bg-black/5）**。四条映射：

1. **浮层容器**（下拉菜单/弹层面板）：无 border，`bg-elevated-bg` + `shadow-[0_6px_24px_rgba(0,0,0,0.10)]`
2. **触发按钮/胶囊**：无 border，`bg-black/5`，hover 同色
3. **列表项 hover/激活**：统一 `bg-black/5`（hover 与激活同色；激活可加 font-medium）
4. **输入区**（Composer）：无 border，`bg-elevated-bg` + `shadow-[0_2px_12px_rgba(0,0,0,0.07)]`，聚焦 `focus-within:shadow-[0_4px_18px_rgba(0,0,0,0.12)]`

**动效语言**：150ms（按钮/行 hover）· 300ms ease-out（滑块/面板/浮层药丸）· 400ms（大面板），定义在 `components/ui/motion.ts`（MOTION.fast / standard / smooth）。**特例**：用户选定的触发器 morph（如 Composer「+」悬停展开）可用更长时长（当前 500ms），须先 HTML 预览、用户确认后再落地。

**组件化（强制）**：新 UI 一律使用 `components/ui/` 组件库，禁止手写这些类名组合：
- `Panel` — 浮层容器
- `ListItem`（selected 属性）— 列表行/下拉行
- `SlidingMenuList` — 浮层内滑动列表药丸（行须 `data-menu-item` + `relative z-[1]`）
- `ToolbarMenuTrigger` — Footer 项目/环境/Git 触发器
- `TintButton`（variant="primary"）— 着色按钮
- `IconButton` — 图标按钮
- `InputSurface` — 输入区容器
- `Slider` — 滑块（如推理强度）
- `DropdownMenu` / `SlidingTabs` — 下拉与标签切换
- `useSlidingIndicator` + `SlidingPill` — **仅侧栏**滑动药丸
- `ComposerAttachButton`（`components/` 非 `ui/`）— Composer「+」悬停 morph 附件按钮
组件用 `cn()`（components/shadcn/utils.ts）合并类，差异通过 className 覆盖。

### 浮层菜单规范（SlidingMenuList）

Footer 项目/环境/Git、Composer 模型/权限等下拉菜单已统一为以下布局。**参考实现**：`GitBranchMenuContent.tsx`（`GIT_MENU`）、`StartView.tsx`（`PROJECT_MENU` / `ENV_MENU`）、`ModelThinkingSelector.tsx`（`MODEL_MENU`）、`Composer.tsx`（权限下拉）。

**推荐 layout**：

```
Panel（无 p-1.5）
  └─ div.padX（px-2）
       └─ SlidingMenuList（pillClassName="left-0 right-0 rounded-lg"）
            └─ 行（px-2.5，relative z-[1]，sliding 模式 hover:bg-transparent）
```

**反模式（hover 药丸偏窄一圈）**：Panel 带 `p-1.5` **且** 默认 `MENU_LIST.pillInset`（`left-1.5 right-1.5`）→ 双重内缩约 24px。新浮层勿用 `motion.ts` 里 `FLOATING_MENU.shell` 的 `p-1.5` 写法。

**行标记**：`data-menu-item={id}` + `relative z-[1]`；`activeId` 须匹配真实 id（如无项目行用 `__none__`，不要用空字符串 `""` 期望匹配）。

**嵌套子菜单**：外包 `position: relative` wrapper 的行（如环境「远程模式」）会导致 `SlidingPill` 用 `offsetTop` 算错位置；`SlidingPill.tsx` 已改为 `getBoundingClientRect`，新代码勿回退。有 `absolute left-full` 三级子 Panel 时，主 Panel 须 `overflow-visible`（否则 SSH 子菜单被裁切）。

**分区标题**：不要用 `pl-8` 空出图标位却不放图标；用 `flex` + 分区图标（如 Git 本地 `folder`、远程 `cloud`）。

### 浮层互斥

- **Footer**（`StartView.tsx`）：项目 / 环境 / Git 三菜单互斥，同时只开一个。
- **Composer**：模型 / 权限 / 上下文互斥。
- **跨区**：Composer 浮层打开时通过 `overlayCloseSignal` + `onOverlayOpenChange` 关闭 Footer 浮层，反之亦然。

### 特殊交互：Composer 附件按钮

用户明确不要「点击展开 Pill 菜单」或「上方独立 tooltip 卡片」，要 **悬停时按钮自身 morph 展开**（`ComposerAttachButton.tsx`：32px 圆 → 胶囊 + 文案，点击仍打开文件选择器）。此类交互与浮层菜单是不同模式，须先出 HTML 预览（≥6 方案）再落地。

**禁止**：`border border-border-theme` 用于交互元素（浮层/按钮/选项卡片）、`hover:bg-gray-50/100`、激活态 `bg-gray-100`、浮层 `bg-white`（用 `bg-elevated-bg` 否则深色刺眼）。颜色一律 Tailwind 类，不写死 hex。
- 导航/设置标签文案走 i18n：`t('settings.tabs.' + id)`，中文文案集中在语言文件里，不要硬编码。
- 交互样式/动效类需求：先出 HTML 选择预览页（`choice-preview-YYYY-MM-DD-HHMM.html`，≥6 个方案），用户选定落地并验证后删除临时文件；探索过程文件（如 `sliding_hover_variations.html`）用户保留时不要擅自删。

## 验证闭环

- 改动后：`npx tsc --noEmit` 类型检查 + 实际运行/浏览器验证 + 关键交互断言，证据齐全才算完成。
- 桌面端环境类问题（pnpm/node_modules）参见本文件「包管理器警告」，不要重复踩坑。
