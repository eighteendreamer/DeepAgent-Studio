/**
 * 统一动效语言（全局唯一来源）：
 * - fast：150ms —— 按钮、列表行等小元素的 hover/激活反馈
 * - standard：300ms ease-out（cubic-bezier(0.4,0,0.2,1)）—— 滑块、面板等中等动效
 * - smooth：400ms —— 大面板展开、较大过渡
 * ease-out 即 Tailwind 默认的 cubic-bezier(0.4, 0, 0.2, 1)。
 */
export const MOTION = {
  fast: "transition-colors duration-150",
  standard: "transition-all duration-300 ease-out",
  smooth: "transition-all duration-[400ms] ease-out",
} as const;

/** 二级浮层菜单（A 视觉 + H 动效）—— 配合 styles.css 中 .floating-menu-panel */
export const FLOATING_MENU = {
  panel: "floating-menu-panel",
  item: "floating-menu-item",
  shell:
    "floating-menu-panel rounded-2xl bg-elevated-bg p-1.5 shadow-[0_6px_24px_rgba(0,0,0,0.10)]",
  row:
    "floating-menu-item flex w-full cursor-pointer items-center rounded-lg px-2 py-1.5 text-[12px] transition-colors duration-150 hover:bg-black/5",
} as const;

/** 浮层内列表（方案 A）—— scroll/actions 同 gutter，行同 padding，hover 等宽 */
export const MENU_LIST = {
  block: "p-2",
  gutter: "px-1.5 py-1",
  /** 滑动药丸内缩 —— 与 shell p-1.5 叠加，行 hover 不贴边 */
  pillInset: "left-1.5 right-1.5",
  row: "w-full items-center gap-2 rounded-lg px-2.5 py-2 text-left text-[12px]",
  rowCompact: "w-full items-center gap-2 rounded-lg px-2 py-1.5 text-left text-[11px]",
  section: "px-2.5 pb-0.5 pt-1.5 text-[10px] font-medium text-text-secondary",
  icon: "mr-2 w-3.5 shrink-0 text-[13px] text-text-secondary",
  empty: "px-2.5 py-2 text-[12px] text-text-secondary",
  search: "flex items-center text-text-secondary",
  searchInput: "w-full bg-transparent outline-none placeholder:text-text-secondary",
  /** 搜索行 —— 与项目下拉对齐：13px、充足上下留白 */
  searchBar: "flex items-center text-[13px] text-text-secondary",
  searchBarPad: "px-3 pb-2.5 pt-3",
  searchBarPadInShell: "px-2 py-2",
  /** 方案 D：内缩短线，弱化全宽 border-t */
  divider: "mx-3.5 my-1.5 h-px shrink-0 bg-border-theme opacity-[0.55]",
} as const;
