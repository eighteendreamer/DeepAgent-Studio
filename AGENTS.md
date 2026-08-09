# DeepAgent 项目规则（AGENTS.md）

本文件是 DeepAgent（monorepo，核心为 `apps/desktop` Tauri 桌面应用）的开发规则。所有开发行为必须遵守。

## 核心规则：前端改动必须「运行」才能看到效果

桌面应用是 **Tauri v2** 打包的 —— 前端代码会被编译进 Rust 应用本体。**只改代码、不运行，用户在旧窗口/旧安装包里永远看不到改动**。任何前端修改完成后，必须把改动跑起来让用户验证，不允许直接说"完成了"。

## 开发改动规范（必须先控范围）

所有开发都要先判断用户要的是「修复原行为」还是「新增能力」。如果用户描述的是性能、报错、轻量化、查询慢、体验卡顿等问题，默认按 **保持原功能和原调用方式不变** 处理，优先在现有接口、现有数据流、现有组件内部优化。

- **禁止未经要求扩功能**：用户没要求新入口、新页面、新交互、新前端状态时，不要主动新增。性能优化尤其不能把“轻量化”做成“新增一套协议/状态/UI”。
- **优先改现有接口实现**：能在当前 Tauri command、service 方法、API wrapper 内部变轻，就不要新增 command、DTO、前端 API。只有现有接口语义无法承载时，才允许新增接口。
- **新增接口必须收口**：确实新增接口时，要同步处理旧接口的去留：要么旧接口委托新实现并保留兼容理由，要么迁移调用点后删除旧接口。禁止新旧两套长期并存、调用链不清。
- **禁止无授权改前端**：用户只要求后端、数据查询、Rust、规则文档或方案时，不要改 React 组件、交互、样式和 i18n。除非前端调用方式必须配合变化，且要先说明原因。
- **小步修改，避免堆代码**：优先删除不必要工作、减少读取、减少扫描、减少重复查询；不要用大量缓存结构、分页状态、DTO 扩散来掩盖原问题。代码量增加必须和收益成比例。
- **先定位再修复**：性能问题要先用 `rg`、日志、SQL、调用链或已有代码确认耗时来源，再下手改。不要凭感觉同时改 sessions、skills、前端和接口层。
- **保持行为兼容**：原本已经能用的功能，优化后默认仍然能用；如果为了性能改成懒加载、截断、分页或默认不查完整内容，必须保证用户触发原功能时还能拿到完整数据。
- **清理临时和废弃代码**：调试代码、临时 DTO、未使用 API、未接入命令、废弃函数和无用类型必须随改动一起清掉。提交前用 `rg`/类型检查确认没有悬空实现。
- **改动说明要说边界**：最终说明里必须明确改了哪些层、哪些没动、为什么这样做；如果刻意没有改前端或没有新增接口，也要说明。

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
- 改完 ui 组件后务必 `git status` 确认文件已进入暂存区，否则 commit 会漏文件（常见遗漏：`MorphingMenuShell.tsx`、`morphingMenuMotion.ts` 等新文件）

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

**浮层 Morph 动效**（Framer Motion `layoutId`，参数集中在 `morphingMenuMotion.ts`）：
- Spring：`stiffness: 250` · `damping: 20` · `mass: 1` · 约 **0.62s**
- 内容 stagger（仅 `staggerContent={true}` 时）：`delayChildren: 0.1` · `staggerChildren: 0.02` · 单项 `opacity + y + blur(4px)`
- **含 `SlidingMenuList` 的菜单一律 `staggerContent={false}`** —— stagger 延迟 + 形变期药丸逻辑叠加会产生「阻塞感」
- 尊重 `prefers-reduced-motion`（`useReducedMotion`，时长归零）

**组件化（强制）**：新 UI 一律使用 `components/ui/` 组件库，禁止手写这些类名组合：
- `Panel` — 浮层容器
- `ListItem`（selected 属性）— 列表行/下拉行
- `SlidingMenuList` — 浮层内滑动列表药丸（行须 `data-menu-item` + `relative z-[1]`）
- `MorphingMenuShell` — 通用触发器 ↔ 浮层 morph（Composer 胶囊、自定义 trigger）
- `MorphingToolbarMenu` — Footer 工具栏 morph（`ToolbarMenuTrigger` + `MorphingMenuShell`）
- `MorphingSelect` — 独立 pill→card 选择器（Morphing Select 风格）
- `ToolbarMenuTrigger` — Footer 项目/环境/Git 触发器（无 morph 时的纯触发器）
- `TintButton`（variant="primary"）— 着色按钮
- `IconButton` — 图标按钮
- `InputSurface` — 输入区容器
- `Slider` — 滑块（如推理强度）
- `DropdownMenu` / `SlidingTabs` — 下拉与标签切换
- `useSlidingIndicator` + `SlidingPill` — **仅侧栏**滑动药丸
- `ComposerAttachButton`（`components/` 非 `ui/`）— Composer「+」悬停 morph 附件按钮
组件用 `cn()`（components/shadcn/utils.ts）合并类，差异通过 className 覆盖。

组件用 `cn()`（components/shadcn/utils.ts）合并类，差异通过 className 覆盖。Morph 相关共享参数见 `morphingMenuMotion.ts`；形变期药丸上下文见 `MorphPanelLayoutContext.tsx`（由 `MorphingMenuShell` 注入）。

### 浮层 Morph 菜单（layoutId 形变）

**一级菜单**（触发器 pill → 面板）用 morph，**二级 flyout**（如环境 → SSH 子列表）**不用**行 morph，保持独立 `Panel` + `floating-menu-panel-in`。

| 场景 | 组件 | 参考 |
|------|------|------|
| Footer 项目 / 环境 | `MorphingToolbarMenu` | `StartView.tsx` |
| Footer Git | `MorphingToolbarMenu` + `unstyled`（内容自带 `Panel`） | `GitBranchChip.tsx` |
| Composer 权限 / 模型 | `MorphingMenuShell` | `Composer.tsx` · `ModelThinkingSelector.tsx` |

**接入要点**：
- 每个菜单实例 `const layoutId = useId().replace(/:/g, "")` —— `layoutId` 必须唯一
- 打开时用 **invisible 占位 trigger**（`MorphingMenuShell` 内置）防止 Footer 项「挤上去」
- 面板对齐：Composer 权限 `panelAlign="left"`；模型选择 `panelAlign="right"`
- Git 等自定义定位：`unstyled` + `panelClassName` 自带 `bottom-full` / `top-full` 等
- 依赖 **`framer-motion@^11`**（已在 `apps/desktop/package.json`）

**二级 flyout 规范**（`StartView.tsx` 环境 → 远程 SSH）：
- 「远程模式」行留在主 `SlidingMenuList` 内，hover 药丸正常跟随
- 子菜单为右侧独立 `Panel`（`absolute left-full`），主 Panel `overflow-visible`
- 行与 flyout 之间加 **invisible bridge** 防 hover 闪断
- **禁止**对行做 `layoutId` hover morph（已验证体验不对）

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

**形变期滑动药丸**（`SlidingPill.tsx` + `MorphPanelLayoutContext`）：
- 药丸从第一帧即可见，**禁止**形变结束后再 `opacity` 淡入
- 形变中、**无 hover**：rAF ~700ms 跟激活项，`transition: none`（行位随 panel 缩放变化）
- 形变中、**有 hover**：恢复 `MOTION.standard`（300ms ease-out）行间滑动；ResizeObserver 同步 hover 行
- **禁止**形变期间 rAF 每帧覆盖 hover 位置（会导致药丸「卡住」或变僵硬）

### 浮层互斥

- **Footer**（`StartView.tsx`）：项目 / 环境 / Git 三菜单互斥，同时只开一个。
- **Composer**：模型 / 权限 / 上下文互斥。
- **跨区**：Composer 浮层打开时通过 `overlayCloseSignal` + `onOverlayOpenChange` 关闭 Footer 浮层，反之亦然。

### 特殊交互：Composer 附件按钮

用户明确不要「点击展开 Pill 菜单」或「上方独立 tooltip 卡片」，要 **悬停时按钮自身 morph 展开**（`ComposerAttachButton.tsx`：32px 圆 → 胶囊 + 文案，点击仍打开文件选择器）。此类交互与浮层菜单是不同模式，须先出 HTML 预览（≥6 方案）再落地。

**禁止**：`border border-border-theme` 用于交互元素（浮层/按钮/选项卡片）、`hover:bg-gray-50/100`、激活态 `bg-gray-100`、浮层 `bg-white`（用 `bg-elevated-bg` 否则深色刺眼）。颜色一律 Tailwind 类，不写死 hex。
- 导航/设置标签文案走 i18n：`t('settings.tabs.' + id)`，中文文案集中在语言文件里，不要硬编码。
- 交互样式/动效类需求：先出 HTML 选择预览页（`choice-preview-YYYY-MM-DD-HHMM.html`，≥6 个方案），用户选定落地并验证后删除临时文件；探索过程文件（如 `sliding_hover_variations.html`）用户保留时不要擅自删。Morph 菜单参考预览：`choice-preview-2026-08-07-morphing-select-framer.html`（Framer Motion 真实现，非 CSS 近似）。

## 验证闭环

- 改动后：`npx tsc --noEmit` 类型检查 + 实际运行/浏览器验证 + 关键交互断言，证据齐全才算完成。
- 桌面端环境类问题（pnpm/node_modules）参见本文件「包管理器警告」，不要重复踩坑。
