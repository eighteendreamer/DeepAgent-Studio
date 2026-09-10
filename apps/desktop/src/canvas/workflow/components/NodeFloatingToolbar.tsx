import { Fragment, useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import {
  Play,
  Paintbrush,
  Eraser,
  Sparkles,
  Maximize,
  Library,
  Clapperboard,
  Move3d,
  Lightbulb,
  Crop,
  Images,
  Download,
  PenLine,
  Copy,
  Trash2,
  Languages,
  RefreshCw,
  ChevronRight,
  Grid2x2,
  Grid3x3,
  LayoutGrid,
  PersonStanding,
  Users,
  IdCard,
  Smile,
  Laugh,
  Clock,
  Sun,
  Wind,
  Layers3,
  FastForward,
  Rewind,
  Video,
  Edit3,
  type LucideIcon,
} from "lucide-react";
import type { WorkflowNodeKind } from "../types";

interface SubmenuItem {
  key: string;
  label: string;
  desc?: string;
  icon?: LucideIcon;
  hasMore?: boolean;
  children?: SubmenuItem[];
  disabled?: boolean;
}

export interface ToolbarAction {
  key: string;
  label: string;
  icon: LucideIcon;
  group: "edit" | "creative" | "finalize" | "system";
  tooltip?: string;
  disabled?: boolean;
  danger?: boolean;
  submenu?: SubmenuItem[];
}

// 分镜大师子菜单（照抄 Penguin-Magic NodeFloatingToolbar）
const STORYBOARD_SUBMENU: SubmenuItem[] = [
  { key: "plot-4", label: "4宫格剧情推演", icon: Grid2x2 },
  { key: "multi-cam-9", label: "9宫格多机位", icon: Grid3x3 },
  { key: "continuous-25", label: "25宫格连贯分镜", icon: LayoutGrid },
  { key: "char-3view", label: "角色3视图", icon: PersonStanding },
  { key: "char-4view", label: "角色4视图", icon: Users },
  { key: "char-design-sheet", label: "角色设定图", icon: IdCard },
  { key: "emoji-grid-9", label: "9宫格表情包", icon: Smile },
  { key: "scene-after-3", label: "画面推演·3秒后", icon: FastForward },
  { key: "scene-before-5", label: "画面回推·5秒前", icon: Rewind },
  { key: "camera-control", label: "运镜控制", icon: Video, hasMore: true, children: [
    { key: "cam-dolly-in", label: "推镜头（Dolly In）" },
    { key: "cam-dolly-out", label: "拉镜头（Dolly Out）" },
    { key: "cam-pan-left", label: "左摇（Pan Left）" },
    { key: "cam-pan-right", label: "右摇（Pan Right）" },
    { key: "cam-tilt-up", label: "上仰（Tilt Up）" },
    { key: "cam-tilt-down", label: "下俯（Tilt Down）" },
    { key: "cam-zoom-in", label: "变焦推近（Zoom In）" },
    { key: "cam-zoom-out", label: "变焦拉远（Zoom Out）" },
    { key: "cam-orbit-left", label: "环绕左移（Orbit Left）" },
    { key: "cam-orbit-right", label: "环绕右移（Orbit Right）" },
    { key: "cam-crane-up", label: "升镜头（Crane Up）" },
    { key: "cam-crane-down", label: "降镜头（Crane Down）" },
    { key: "cam-move", label: "移镜头（Track）" },
    { key: "cam-follow", label: "跟镜头（Follow）" },
    { key: "cam-static", label: "固定镜头（Static）" },
    { key: "cam-arc", label: "弧线运动（Arc Shot）" },
    { key: "cam-dutch", label: "荷兰角倾斜（Dutch Angle）" },
  ]},
  { key: "film-light", label: "电影光影", icon: Sun, hasMore: true, children: [
    { key: "film-light-key", label: "主光" },
    { key: "film-light-fill", label: "辅光" },
    { key: "film-light-rim", label: "逆光/轮廓光" },
    { key: "film-light-hair", label: "发丝光" },
    { key: "film-light-side", label: "侧光" },
    { key: "film-light-edge", label: "边缘光" },
    { key: "film-light-top", label: "顶光" },
    { key: "film-light-bottom", label: "底光" },
    { key: "film-light-eye", label: "眼神光" },
    { key: "film-light-chiaroscuro", label: "明暗对比法" },
    { key: "film-light-rembrandt", label: "伦勃朗布光" },
    { key: "film-light-butterfly", label: "蝴蝶光" },
    { key: "film-light-loop", label: "环形光" },
    { key: "film-light-split", label: "二分光" },
    { key: "film-light-highkey", label: "高调光" },
    { key: "film-light-flat", label: "平光" },
  ]},
  { key: "time-design", label: "时间设计", icon: Clock, hasMore: true, children: [
    { key: "time-spring-dawn", label: "春·清晨" },
    { key: "time-spring-noon", label: "春·正午" },
    { key: "time-spring-dusk", label: "春·傍晚" },
    { key: "time-spring-night", label: "春·夜晚" },
    { key: "time-summer-dawn", label: "夏·清晨" },
    { key: "time-summer-noon", label: "夏·正午" },
    { key: "time-summer-dusk", label: "夏·傍晚" },
    { key: "time-summer-night", label: "夏·夜晚" },
    { key: "time-autumn-dawn", label: "秋·清晨" },
    { key: "time-autumn-noon", label: "秋·正午" },
    { key: "time-autumn-dusk", label: "秋·傍晚" },
    { key: "time-autumn-night", label: "秋·夜晚" },
    { key: "time-winter-dawn", label: "冬·清晨" },
    { key: "time-winter-noon", label: "冬·正午" },
    { key: "time-winter-dusk", label: "冬·傍晚" },
    { key: "time-winter-night", label: "冬·夜晚" },
  ]},
  { key: "emotion-reset", label: "情绪重塑", icon: Laugh, hasMore: true, children: [
    { key: "emo-angry", label: "生气" },
    { key: "emo-smile", label: "微笑" },
    { key: "emo-think", label: "思考" },
    { key: "emo-confused", label: "疑惑" },
    { key: "emo-scared", label: "吓到" },
    { key: "emo-peek", label: "偷瞄" },
    { key: "emo-evil", label: "邪恶" },
    { key: "emo-cry", label: "哭泣" },
  ]},
  { key: "depth-parallax", label: "深度视差", icon: Layers3 },
  { key: "dynamic-effect", label: "动态场效", icon: Wind, hasMore: true, children: [
    { key: "fx-rain", label: "雨天" },
    { key: "fx-snow", label: "雪天" },
    { key: "fx-sand", label: "沙尘" },
    { key: "fx-god-rays", label: "丁达尔效应" },
    { key: "fx-ember", label: "余烬" },
    { key: "fx-biolum", label: "荧光浮游生物效果" },
    { key: "fx-petal-fall", label: "花瓣飘落" },
    { key: "fx-firefly", label: "萤火虫微光" },
    { key: "fx-dust-beam", label: "阳光尘埃" },
    { key: "fx-fallen-leaves", label: "落叶" },
    { key: "fx-fog", label: "晨雾/薄雾" },
    { key: "fx-sparkle", label: "星光/金箔碎片" },
    { key: "fx-steam", label: "蒸汽/热气" },
    { key: "fx-wind", label: "风吹动效" },
  ]},
];

// 裁剪：自由裁剪与宫格裁剪作为二级能力（照抄 Penguin-Magic）
const CROP_SUBMENU: SubmenuItem[] = [
  { key: "free", label: "自由裁剪", desc: "自由调整裁剪框与比例", icon: Crop },
  { key: "4", label: "4宫格裁剪", desc: "2×2 网格", icon: Grid2x2 },
  { key: "9", label: "9宫格裁剪", desc: "3×3 网格", icon: Grid3x3 },
  { key: "16", label: "16宫格裁剪", desc: "4×4 网格", icon: LayoutGrid },
  { key: "25", label: "25宫格裁剪", desc: "5×5 网格", icon: LayoutGrid },
  { key: "custom", label: "自定义宫格裁剪", desc: "自由拖入横竖线", icon: Edit3 },
];

const IMAGE_ACTIONS: ToolbarAction[] = [
  { key: "repaint", label: "标注", icon: Paintbrush, group: "edit", tooltip: "在图片上添加矩形、文字、箭头、序号等标注" },
  { key: "erase", label: "擦除", icon: Eraser, group: "edit", tooltip: "点编辑：擦除模式" },
  { key: "enhance", label: "高清放大", icon: Sparkles, group: "edit", tooltip: "无损高清放大" },
  { key: "outpaint", label: "扩图", icon: Maximize, group: "edit", tooltip: "智能扩图（拖拽扩展画布边界）" },
  { key: "creative-library", label: "创意库", icon: Library, group: "creative", tooltip: "从创意库选择模板" },
  { key: "storyboard", label: "分镜大师", icon: Clapperboard, group: "creative", tooltip: "一图扩成多机位/分镜组", submenu: STORYBOARD_SUBMENU },
  { key: "angle", label: "角度", icon: Move3d, group: "creative", tooltip: "多角度三视图生成（暂未接入）", disabled: true },
  { key: "lighting", label: "打光", icon: Lightbulb, group: "creative", tooltip: "场景重打光（暂未接入）", disabled: true },
  { key: "crop", label: "裁剪", icon: Crop, group: "finalize", tooltip: "自由裁剪或宫格裁剪", submenu: CROP_SUBMENU },
  { key: "save-asset", label: "存入资产", icon: Images, group: "finalize", tooltip: "把当前图片保存到我的资产" },
  { key: "download", label: "下载", icon: Download, group: "finalize" },
  { key: "rename", label: "重命名", icon: PenLine, group: "system", tooltip: "重命名节点" },
  { key: "duplicate", label: "复制", icon: Copy, group: "system", tooltip: "复制节点" },
  { key: "delete", label: "删除", icon: Trash2, group: "system", danger: true },
];

const VIDEO_ACTIONS: ToolbarAction[] = [
  { key: "enhance", label: "增强", icon: Sparkles, group: "edit", tooltip: "视频画质增强（暂未接入）", disabled: true },
  { key: "translate", label: "翻译", icon: Languages, group: "edit" },
  { key: "regen", label: "重新生成", icon: RefreshCw, group: "edit" },
  { key: "creative-library", label: "创意库", icon: Library, group: "finalize", tooltip: "从创意库选择模板" },
  { key: "download", label: "下载", icon: Download, group: "finalize" },
  { key: "rename", label: "重命名", icon: PenLine, group: "system", tooltip: "重命名节点" },
  { key: "duplicate", label: "复制", icon: Copy, group: "system", tooltip: "复制节点" },
  { key: "delete", label: "删除", icon: Trash2, group: "system", danger: true },
];

const TEXT_ACTIONS: ToolbarAction[] = [
  { key: "creative-library", label: "创意库", icon: Library, group: "creative", tooltip: "从创意库选择模板" },
  { key: "translate", label: "翻译", icon: Languages, group: "edit" },
  { key: "regen", label: "重新生成", icon: RefreshCw, group: "edit" },
  { key: "download", label: "下载", icon: Download, group: "finalize" },
  { key: "rename", label: "重命名", icon: PenLine, group: "system", tooltip: "重命名节点" },
  { key: "duplicate", label: "复制", icon: Copy, group: "system", tooltip: "复制节点" },
  { key: "delete", label: "删除", icon: Trash2, group: "system", danger: true },
];

const PRO_ACTIONS: ToolbarAction[] = [
  { key: "rename", label: "重命名", icon: PenLine, group: "system", tooltip: "重命名节点" },
  { key: "duplicate", label: "复制", icon: Copy, group: "system", tooltip: "复制节点" },
  { key: "delete", label: "删除", icon: Trash2, group: "system", danger: true },
];

const PRO_KINDS: Set<string> = new Set([
  "start", "end", "if-else", "iteration", "llm", "agent", "question-classifier",
  "parameter-extractor", "knowledge-retrieval", "code", "http-request",
  "template-transform", "variable-aggregator", "tool", "human-input",
]);

function getActions(kind: WorkflowNodeKind): ToolbarAction[] {
  if (PRO_KINDS.has(kind)) return PRO_ACTIONS;
  switch (kind) {
    case "image-gen":
    case "image-edit":
    case "image-compare":
      return IMAGE_ACTIONS;
    case "video-gen":
    case "video-stitch":
      return VIDEO_ACTIONS;
    case "text-gen":
    case "script-gen":
      return TEXT_ACTIONS;
    default:
      return PRO_ACTIONS;
  }
}

const ICON_COLOR = "rgba(255,255,255,0.82)";
const LABEL_COLOR = "rgba(255,255,255,0.85)";
const DIVIDER_COLOR = "rgba(255,255,255,0.12)";
const SUB_ITEM_COLOR = "rgba(255,255,255,0.88)";
const SUB_DESC_COLOR = "rgba(255,255,255,0.5)";
const OPEN_BG = "rgba(255,255,255,0.1)";

// 二级/三级菜单与主工具栏同一套悬浮玻璃（Penguin getNodeCanvasSecondaryMenuStyle 同材质同圆角）
const SUBMENU_SURFACE: React.CSSProperties = {
  background: "rgba(76,80,82,0.55)",
  border: "1px solid rgba(255,255,255,0.08)",
  boxShadow: "0 6px 24px rgba(0,0,0,0.24)",
  backdropFilter: "blur(40px)",
  WebkitBackdropFilter: "blur(40px)",
  borderRadius: 12,
};

const SUBMENU_Z_INDEX = 10001;

function resolveSubmenuPosition({
  triggerRect,
  menuWidth,
  menuHeight,
  viewportWidth,
  viewportHeight,
  gap = 8,
}: {
  triggerRect: DOMRect;
  menuWidth: number;
  menuHeight: number;
  viewportWidth: number;
  viewportHeight: number;
  gap?: number;
}): { left: number; top: number } {
  let left = triggerRect.left + triggerRect.width / 2 - menuWidth / 2;
  left = Math.max(gap, Math.min(left, viewportWidth - menuWidth - gap));
  const spaceAbove = triggerRect.top - gap;
  const spaceBelow = viewportHeight - triggerRect.bottom - gap;
  let top = spaceBelow < menuHeight && spaceAbove > spaceBelow
    ? triggerRect.top - menuHeight - gap
    : triggerRect.bottom + gap;
  top = Math.max(gap, Math.min(top, viewportHeight - menuHeight - gap));
  return { left, top };
}

const GROUP_ORDER: Array<ToolbarAction["group"]> = ["edit", "creative", "finalize", "system"];

interface NodeFloatingToolbarProps {
  kind: WorkflowNodeKind;
  onRun: () => void;
  onDelete: () => void;
  onRename: () => void;
  onDuplicate: () => void;
  onDownload: () => void;
}

export function NodeFloatingToolbar({ kind, onRun, onDelete, onRename, onDuplicate, onDownload }: NodeFloatingToolbarProps) {
  const actions = getActions(kind);
  const [openSubmenu, setOpenSubmenu] = useState<string | null>(null);
  const [openNestedKey, setOpenNestedKey] = useState<string | null>(null);
  const [submenuPos, setSubmenuPos] = useState<{ left: number; top: number } | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const submenuRef = useRef<HTMLDivElement>(null);
  const triggerRectRef = useRef<DOMRect | null>(null);
  const nestedRectRef = useRef<DOMRect | null>(null);

  // Portal 到 body 后按触发按钮实测定位（layout effect 在绘制前修正）
  useLayoutEffect(() => {
    if (!openSubmenu || !submenuRef.current || !triggerRectRef.current) return;
    const menuRect = submenuRef.current.getBoundingClientRect();
    setSubmenuPos(
      resolveSubmenuPosition({
        triggerRect: triggerRectRef.current,
        menuWidth: menuRect.width || 230,
        menuHeight: menuRect.height || 360,
        viewportWidth: window.innerWidth,
        viewportHeight: window.innerHeight,
      }),
    );
  }, [openSubmenu]);

  // 点击外部关闭（排除工具栏本体与 Portal 渲染的子菜单）
  useEffect(() => {
    if (!openSubmenu) return;
    const handler = (e: MouseEvent) => {
      const target = e.target as Node;
      if (rootRef.current && rootRef.current.contains(target)) return;
      if (target instanceof HTMLElement && target.closest?.('[data-wf-popover="true"]')) return;
      setOpenSubmenu(null);
    };
    window.addEventListener("mousedown", handler);
    return () => window.removeEventListener("mousedown", handler);
  }, [openSubmenu]);

  useEffect(() => {
    if (!openSubmenu) setOpenNestedKey(null);
  }, [openSubmenu]);

  const renderSubmenuPortal = (action: ToolbarAction) => {
    if (!action.submenu?.length || openSubmenu !== action.key) return null;
    const trigger = triggerRectRef.current;
    const fallbackLeft = trigger ? trigger.left + trigger.width / 2 - 115 : 0;
    const fallbackTop = trigger ? trigger.bottom + 8 : 0;
    return createPortal(
      <div
        ref={submenuRef}
        data-wf-popover="true"
        className="overflow-hidden"
        style={{
          ...SUBMENU_SURFACE,
          position: "fixed",
          left: submenuPos?.left ?? fallbackLeft,
          top: submenuPos?.top ?? fallbackTop,
          zIndex: SUBMENU_Z_INDEX,
        }}
      >
        <div className="max-h-[360px] overflow-y-auto overscroll-contain" style={{ scrollbarWidth: "none" }}>
          <div className="flex min-w-[230px] flex-col gap-0.5 p-1.5">
            {action.submenu.map((child) => {
              const ChildIcon = child.icon;
              const hasNested = !!(child.hasMore && child.children?.length);
              const isNestedOpen = openNestedKey === child.key;
              return (
                <div key={child.key} className="relative">
                  <button
                    type="button"
                    disabled={child.disabled}
                    onClick={(e) => {
                      e.stopPropagation();
                      if (hasNested) {
                        nestedRectRef.current = e.currentTarget.getBoundingClientRect();
                        setOpenNestedKey((prev) => (prev === child.key ? null : child.key));
                      } else {
                        setOpenSubmenu(null);
                      }
                    }}
                    onMouseEnter={(e) => {
                      if (child.disabled) return;
                      if (hasNested) {
                        nestedRectRef.current = e.currentTarget.getBoundingClientRect();
                        setOpenNestedKey(child.key);
                      } else {
                        setOpenNestedKey(null);
                      }
                    }}
                    className="flex w-full items-center gap-2 rounded-lg px-2.5 py-2 text-left transition-all hover:bg-white/10"
                    style={{
                      color: SUB_ITEM_COLOR,
                      background: "transparent",
                      opacity: child.disabled ? 0.45 : 1,
                      cursor: child.disabled ? "not-allowed" : "pointer",
                    }}
                  >
                    {ChildIcon && <ChildIcon className="h-3.5 w-3.5 flex-shrink-0 opacity-70" strokeWidth={1.8} />}
                    <div className="min-w-0 flex-1">
                      <div className="truncate text-[12.5px] font-medium leading-snug">{child.label}</div>
                      {child.desc && (
                        <div className="mt-0.5 truncate text-[10.5px]" style={{ color: SUB_DESC_COLOR }}>
                          {child.desc}
                        </div>
                      )}
                    </div>
                    {child.hasMore && <ChevronRight className="h-3 w-3 flex-shrink-0 opacity-50" />}
                  </button>
                  {hasNested && isNestedOpen && nestedRectRef.current && createPortal(
                    (() => {
                      const rect = nestedRectRef.current;
                      const nestedW = 230;
                      const nestedH = Math.min((child.children?.length || 0) * 36 + 16, 400);
                      let left = rect.right + 4;
                      let top = rect.top;
                      if (left + nestedW > window.innerWidth) left = rect.left - nestedW - 4;
                      if (top + nestedH > window.innerHeight) top = Math.max(8, window.innerHeight - nestedH - 8);
                      return (
                        <div
                          data-wf-popover="true"
                          className="overflow-hidden"
                          style={{ ...SUBMENU_SURFACE, position: "fixed", left, top, zIndex: SUBMENU_Z_INDEX }}
                          onMouseLeave={() => setOpenNestedKey(null)}
                        >
                          <div className="max-h-[400px] overflow-y-auto overscroll-contain" style={{ scrollbarWidth: "none" }}>
                            <div className="flex min-w-[230px] flex-col gap-0.5 p-1.5">
                              {child.children!.map((grandChild) => (
                                <button
                                  key={grandChild.key}
                                  type="button"
                                  onClick={(e) => {
                                    e.stopPropagation();
                                    setOpenNestedKey(null);
                                    setOpenSubmenu(null);
                                  }}
                                  className="flex w-full items-center whitespace-nowrap rounded-lg px-2.5 py-2 text-left transition-all hover:bg-white/10"
                                  style={{ color: SUB_ITEM_COLOR, background: "transparent" }}
                                >
                                  <div className="text-[12.5px] font-medium leading-snug">{grandChild.label}</div>
                                </button>
                              ))}
                            </div>
                          </div>
                        </div>
                      );
                    })(),
                    document.body,
                  )}
                </div>
              );
            })}
            <div className="mt-0.5 pt-1.5">
              <button
                type="button"
                onClick={(e) => {
                  e.stopPropagation();
                  setOpenSubmenu(null);
                }}
                className="flex w-full items-center rounded-lg px-2.5 py-2 text-left text-[12px] font-medium transition-all hover:bg-white/10"
                style={{ color: SUB_DESC_COLOR, background: "transparent" }}
              >
                取消
              </button>
            </div>
          </div>
        </div>
      </div>,
      document.body,
    );
  };

  const renderAction = (action: ToolbarAction) => {
    const Icon = action.icon;
    const wired: Record<string, (() => void) | undefined> = {
      delete: onDelete,
      rename: onRename,
      duplicate: onDuplicate,
      download: onDownload,
    };
    const handler = wired[action.key];
    const hasSubmenu = !!action.submenu?.length;
    const isOpen = openSubmenu === action.key;
    return (
      <Fragment key={action.key}>
        <button
          type="button"
          title={action.tooltip ?? action.label}
          disabled={action.disabled}
          className="flex h-7 items-center gap-1.5 rounded-lg px-2 transition-colors duration-200 hover:bg-white/10 disabled:cursor-not-allowed disabled:opacity-45 disabled:hover:bg-transparent"
          style={{ background: isOpen ? OPEN_BG : undefined }}
          onMouseDown={(e) => e.stopPropagation()}
          onClick={(e) => {
            e.stopPropagation();
            if (hasSubmenu) {
              triggerRectRef.current = e.currentTarget.getBoundingClientRect();
              setOpenNestedKey(null);
              setOpenSubmenu((prev) => (prev === action.key ? null : action.key));
              return;
            }
            setOpenSubmenu(null);
            handler?.();
          }}
        >
          <Icon
            className="h-3.5 w-3.5"
            strokeWidth={1.8}
            style={{ color: action.danger ? "rgba(248,113,113,0.9)" : ICON_COLOR }}
          />
          <span
            className="whitespace-nowrap text-xs font-medium"
            style={{ color: action.danger ? "rgba(248,113,113,0.9)" : LABEL_COLOR }}
          >
            {action.label}
          </span>
          {hasSubmenu && <ChevronRight className="h-3 w-3" style={{ color: "rgba(255,255,255,0.45)" }} />}
        </button>
        {renderSubmenuPortal(action)}
      </Fragment>
    );
  };

  const groups = GROUP_ORDER.map((group) => actions.filter((a) => a.group === group)).filter(
    (items) => items.length > 0,
  );

  return (
    <div
      ref={rootRef}
      className="flex items-center gap-px rounded-[12px] px-1.5"
      style={{
        height: 40,
        background: "rgba(76,80,82,0.55)",
        border: "1px solid rgba(255,255,255,0.08)",
        boxShadow: "0 6px 24px rgba(0,0,0,0.24)",
        backdropFilter: "blur(40px)",
        WebkitBackdropFilter: "blur(40px)",
      }}
      onMouseDown={(e) => e.stopPropagation()}
      onClick={(e) => e.stopPropagation()}
    >
      <button
        type="button"
        title="运行此节点"
        className="flex h-7 items-center gap-1.5 rounded-lg px-2 transition-colors duration-200 hover:bg-white/10"
        onMouseDown={(e) => e.stopPropagation()}
        onClick={(e) => {
          e.stopPropagation();
          onRun();
        }}
      >
        <Play className="h-3.5 w-3.5" strokeWidth={1.8} style={{ color: "#8b7cf7" }} />
        <span className="whitespace-nowrap text-xs font-medium" style={{ color: LABEL_COLOR }}>
          运行
        </span>
      </button>
      {groups.map((items) => (
        <div key={items[0].group} className="flex items-center gap-px">
          <div className="mx-1 h-4 w-px" style={{ background: DIVIDER_COLOR }} />
          {items.map(renderAction)}
        </div>
      ))}
    </div>
  );
}
