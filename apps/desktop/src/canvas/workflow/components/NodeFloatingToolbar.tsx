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
  type LucideIcon,
} from "lucide-react";
import type { WorkflowNodeKind } from "../types";

export interface ToolbarAction {
  key: string;
  label: string;
  icon: LucideIcon;
  group: "edit" | "creative" | "finalize" | "system";
  tooltip?: string;
  disabled?: boolean;
  danger?: boolean;
  chevron?: boolean;
}

const IMAGE_ACTIONS: ToolbarAction[] = [
  { key: "repaint", label: "标注", icon: Paintbrush, group: "edit", tooltip: "在图片上添加矩形、文字、箭头、序号等标注" },
  { key: "erase", label: "擦除", icon: Eraser, group: "edit", tooltip: "点编辑：擦除模式" },
  { key: "enhance", label: "高清放大", icon: Sparkles, group: "edit", tooltip: "无损高清放大" },
  { key: "outpaint", label: "扩图", icon: Maximize, group: "edit", tooltip: "智能扩图（拖拽扩展画布边界）" },
  { key: "creative-library", label: "创意库", icon: Library, group: "creative", tooltip: "从创意库选择模板" },
  { key: "storyboard", label: "分镜大师", icon: Clapperboard, group: "creative", tooltip: "一图扩成多机位/分镜组", chevron: true },
  { key: "angle", label: "角度", icon: Move3d, group: "creative", tooltip: "多角度三视图生成（暂未接入）", disabled: true },
  { key: "lighting", label: "打光", icon: Lightbulb, group: "creative", tooltip: "场景重打光（暂未接入）", disabled: true },
  { key: "crop", label: "裁剪", icon: Crop, group: "finalize", tooltip: "自由裁剪或宫格裁剪", chevron: true },
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

const GROUP_ORDER: Array<ToolbarAction["group"]> = ["edit", "creative", "finalize", "system"];

interface NodeFloatingToolbarProps {
  kind: WorkflowNodeKind;
  onRun: () => void;
  onDelete: () => void;
}

export function NodeFloatingToolbar({ kind, onRun, onDelete }: NodeFloatingToolbarProps) {
  const actions = getActions(kind);

  const renderAction = (action: ToolbarAction) => {
    const Icon = action.icon;
    const wired = action.key === "delete" ? onDelete : undefined;
    return (
      <button
        key={action.key}
        type="button"
        title={action.tooltip ?? action.label}
        disabled={action.disabled}
        className="flex h-7 items-center gap-1.5 rounded-lg px-2 transition-colors duration-200 hover:bg-white/10 disabled:cursor-not-allowed disabled:opacity-45 disabled:hover:bg-transparent"
        onMouseDown={(e) => e.stopPropagation()}
        onClick={(e) => {
          e.stopPropagation();
          wired?.();
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
        {action.chevron && <ChevronRight className="h-3 w-3" style={{ color: "rgba(255,255,255,0.45)" }} />}
      </button>
    );
  };

  const groups = GROUP_ORDER.map((group) => actions.filter((a) => a.group === group)).filter(
    (items) => items.length > 0,
  );

  return (
    <div
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
