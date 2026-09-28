import {
  Compass,
  FileText,
  FolderOpen,
  LayoutGrid,
  MessageSquare,
  Mic,
  Network,
  Puzzle,
  SquareTerminal,
  type LucideIcon,
} from "lucide-react";

const TOOL_ICONS: Record<string, LucideIcon> = {
  browser: Compass,
  canvas: LayoutGrid,
  chat: MessageSquare,
  file_preview: FileText,
  files: FolderOpen,
  project_map: Network,
  recording: Mic,
  terminal: SquareTerminal,
};

/** Visual icons for right-sidebar tools; plugin identifiers and launch behavior stay unchanged. */
export function toolIconComponent(type: string): LucideIcon {
  return TOOL_ICONS[type] ?? Puzzle;
}
