import { useRef, useState, type ReactNode } from "react";
import { Archive, ChevronDown, Ellipsis, Layers, Pencil, Pin, SquarePen, X } from "lucide-react";
import type { SidebarSection } from "../types";
import { message } from "./message";
import { changeSidebarPreferences } from "./sidebarPreferences";
import { SidebarSectionDialog } from "./SidebarSectionDialog";
import { Button } from "./shadcn/button";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "./shadcn/dialog";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "./shadcn/dropdown-menu";
import { PinThumbtackIcon } from "./ui/PinThumbtackIcon";
import { HoverInfo } from "./ui/HoverInfo";
import { cn } from "./shadcn/utils";

interface Props {
  section: SidebarSection;
  collapsed: boolean;
  sessionCount: number;
  onToggle: () => void;
  onNewChat?: () => void;
  onAddSection: () => void;
  onArchive?: () => Promise<void>;
  children: ReactNode;
}

const actionClass = "h-5 w-5 rounded p-0 hover:bg-sidebar-highlight";
const iconClass = "h-3.5 w-3.5 shrink-0 text-text-secondary";

/** Custom sections share the top-level heading style and project-row actions. */
export function SidebarSectionGroup({ section, collapsed, sessionCount, onToggle, onNewChat, onAddSection, onArchive, children }: Props) {
  const [menuOpen, setMenuOpen] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [confirmation, setConfirmation] = useState<"archive" | "remove" | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const openingDialog = useRef(false);
  const togglePin = () => {
    void changeSidebarPreferences({ type: "set_section_pinned", section_id: section.id, pinned: !section.pinned })
      .catch((cause) => message.error(`更新分区置顶失败：${String(cause)}`));
  };
  const confirm = (action: "archive" | "remove") => { openingDialog.current = true; setError(""); setConfirmation(action); };
  return <div className="flex flex-col">
    <div data-custom-contextmenu="1" className="group mb-1 flex min-w-0 items-center justify-between px-2 text-text-secondary"
      onContextMenu={(event) => { event.preventDefault(); event.stopPropagation(); setMenuOpen(true); }}>
      <Button variant="ghost" aria-expanded={!collapsed} onClick={onToggle}
        className="h-auto min-w-0 flex-1 justify-start gap-0.5 rounded-none bg-transparent p-0 text-[12px] font-normal hover:bg-transparent hover:text-text-secondary">
        <span className="truncate">{section.name}</span>
        <ChevronDown className={cn("h-3 w-3 shrink-0 transition-transform", collapsed && "-rotate-90")}/>
      </Button>
      <div className={cn("ml-1 flex shrink-0 items-center space-x-0.5 transition-opacity group-focus-within:opacity-100",
        menuOpen || section.pinned ? "opacity-100" : "opacity-0 group-hover:opacity-100")}>
        <HoverInfo content={section.pinned ? "取消置顶分区" : "置顶分区"}>
          <Button variant="ghost" size="icon" className={actionClass} aria-label={`${section.name}：${section.pinned ? "取消置顶" : "置顶"}`} onClick={togglePin}>
            <PinThumbtackIcon pinned={!!section.pinned}/>
          </Button>
        </HoverInfo>
        <DropdownMenu open={menuOpen} onOpenChange={setMenuOpen} modal={false}>
          <HoverInfo content="分区选项"><DropdownMenuTrigger asChild>
            <Button variant="ghost" size="icon" className={actionClass} aria-label={`${section.name}：分区选项`}><Ellipsis className="h-3 w-3"/></Button>
          </DropdownMenuTrigger></HoverInfo>
          <DropdownMenuContent align="end" className="w-48" data-custom-contextmenu="1"
            onCloseAutoFocus={(event) => { if (openingDialog.current) { event.preventDefault(); openingDialog.current = false; } }}>
            <DropdownMenuItem className="gap-2" onSelect={togglePin}><Pin className={iconClass}/>{section.pinned ? "取消置顶" : "置顶"}</DropdownMenuItem>
            <DropdownMenuItem className="gap-2" onSelect={() => { openingDialog.current = true; setRenaming(true); }}><Pencil className={iconClass}/>重命名</DropdownMenuItem>
            <DropdownMenuItem className="gap-2" onSelect={() => { openingDialog.current = true; onAddSection(); }}><Layers className={iconClass}/>添加分区</DropdownMenuItem>
            <DropdownMenuSeparator/>
            <DropdownMenuItem className="gap-2" disabled={!onArchive || sessionCount === 0} onSelect={() => confirm("archive")}><Archive className={iconClass}/>归档聊天</DropdownMenuItem>
            <DropdownMenuItem className="gap-2" onSelect={() => confirm("remove")}><X className={iconClass}/>移除分区</DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
        <HoverInfo content="新建会话"><Button variant="ghost" size="icon" className={actionClass} disabled={!onNewChat}
          aria-label={`${section.name}：新建会话`} onClick={onNewChat}><SquarePen className="h-3 w-3"/></Button></HoverInfo>
      </div>
    </div>
    {!collapsed && <div className="space-y-0.5">{children}</div>}
    <SidebarSectionDialog open={renaming} onClose={() => setRenaming(false)} title="重命名分区" confirmLabel="保存" initialName={section.name}
      onCreate={(name) => changeSidebarPreferences({ type: "rename_section", section_id: section.id, name })}/>
    <Dialog open={confirmation !== null} onOpenChange={(next) => { if (!next && !saving) setConfirmation(null); }}>
      <DialogContent className="max-w-sm">
        <DialogHeader><DialogTitle>{confirmation === "remove" ? "移除分区" : "归档聊天"}</DialogTitle></DialogHeader>
        <p className="px-6 text-[13px] leading-relaxed text-text-secondary">
          {confirmation === "remove" ? `移除“${section.name}”后，其中的会话会回到原项目或最近对话。` : `归档“${section.name}”中的 ${sessionCount} 个会话？`}
        </p>
        {error && <p role="alert" className="mt-2 px-6 text-xs text-red-500">{error}</p>}
        <DialogFooter>
          <Button variant="ghost" disabled={saving} onClick={() => setConfirmation(null)}>取消</Button>
          <Button disabled={saving} onClick={async () => {
            setSaving(true); setError("");
            try {
              if (confirmation === "remove") await changeSidebarPreferences({ type: "remove_section", section_id: section.id });
              else await onArchive?.();
              setConfirmation(null);
            } catch (cause) { setError(String(cause)); }
            finally { setSaving(false); }
          }}>{confirmation === "remove" ? "移除" : "归档"}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </div>;
}
