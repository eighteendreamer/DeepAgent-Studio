import { Archive, ArrowUpRightFromSquare, Check, Clock, Copy, Ellipsis, Eye, EyeOff, Folder, GitBranch, Layers, Pencil, Pin, Plus, Share, SquareArrowOutUpRight } from "lucide-react";
import type { Project, SidebarSection } from "../types";
import { OPEN_TARGET_OPTIONS } from "./openTargets";
import {
  DropdownMenu, DropdownMenuTrigger, DropdownMenuContent, DropdownMenuItem,
  DropdownMenuSeparator, DropdownMenuSub, DropdownMenuSubContent, DropdownMenuSubTrigger,
} from "./shadcn/dropdown-menu";
import { Button } from "./shadcn/button";

export interface SessionMenuOptions {
  projects?: Project[];
  sections?: SidebarSection[];
  sectionId?: string;
  unread?: boolean;
  defaultOpenTarget?: string;
  onSetUnread?: (unread: boolean) => void;
  onOpenProject?: (path: string) => void;
  onSetSection?: (id: string | null) => void;
  onCreateSection?: () => void;
  onCopyDeepLink?: () => void;
  onCopyDirectory?: () => void;
  onOpenTarget?: (target: string) => void;
}

type Props = SessionMenuOptions & {
  title?: string | null; pinned?: boolean; open: boolean;
  onOpenChange: (open: boolean) => void;
  contextMenuPosition?: { left: number; top: number };
  onPin?: () => void; onRename?: () => void; onArchive?: () => void;
  onCopy?: () => void; onFork?: () => void;
  onOpenAutomation?: () => void; onOpenInNewWindow?: () => void;
};

export function ChatSessionMenu({ title, pinned, open, onOpenChange, contextMenuPosition,
  onPin, onRename, onArchive, onCopy, onFork, onOpenAutomation, onOpenInNewWindow,
  projects = [], sections = [], sectionId, unread, defaultOpenTarget,
  onSetUnread, onOpenProject, onSetSection, onCreateSection, onCopyDeepLink, onCopyDirectory, onOpenTarget,
}: Props) {
  const item = "gap-2 text-[13px]";
  const icon = "h-3.5 w-3.5 text-text-secondary";
  return <DropdownMenu open={open} onOpenChange={onOpenChange} modal={false}>
    <DropdownMenuTrigger asChild>
      {contextMenuPosition ? <span aria-hidden className="pointer-events-none fixed h-px w-px"
        style={{ left: contextMenuPosition.left, top: contextMenuPosition.top }}/>
        : <Button variant="ghost" className="h-8 max-w-full min-w-0 gap-2 pl-1 pr-2 font-medium">
          <span className="truncate">{title?.trim() || "对话"}</span><Ellipsis className="h-3 w-3 shrink-0"/>
        </Button>}
    </DropdownMenuTrigger>
    <DropdownMenuContent align="start" sideOffset={contextMenuPosition ? 0 : 6}
      className="w-48" data-custom-contextmenu="1" onContextMenu={(event) => event.preventDefault()}>
      <DropdownMenuItem className={item} onSelect={onRename}><Pencil className={icon}/>重命名</DropdownMenuItem>
      <DropdownMenuItem className={item} onSelect={onPin}><Pin className={icon}/>{pinned ? "取消置顶" : "置顶"}</DropdownMenuItem>
      <DropdownMenuItem className={item} disabled={!onSetUnread} onSelect={() => onSetUnread?.(!unread)}>
        {unread ? <Eye className={icon}/> : <EyeOff className={icon}/>}{unread ? "标记为已读" : "标记为未读"}
      </DropdownMenuItem>
      <DropdownMenuSub><DropdownMenuSubTrigger className={item}><Folder className={icon}/>项目</DropdownMenuSubTrigger>
        <DropdownMenuSubContent className="max-h-80 min-w-56 overflow-y-auto" data-custom-contextmenu="1">
          {projects.length === 0 && <DropdownMenuItem disabled>暂无项目</DropdownMenuItem>}
          {projects.map((project) => <DropdownMenuItem key={project.path} className={item} onSelect={() => onOpenProject?.(project.path)}>
            <Folder className={icon}/><span className="max-w-64 truncate">{project.name}</span>
          </DropdownMenuItem>)}
        </DropdownMenuSubContent>
      </DropdownMenuSub>
      <DropdownMenuSub><DropdownMenuSubTrigger className={item}><Layers className={icon}/>分区</DropdownMenuSubTrigger>
        <DropdownMenuSubContent className="max-h-80 min-w-48 overflow-y-auto" data-custom-contextmenu="1">
          {sections.map((section) => <DropdownMenuItem key={section.id} onSelect={() => onSetSection?.(section.id)}>
            {section.name}{sectionId === section.id && <Check className="ml-auto h-3.5 w-3.5"/>}
          </DropdownMenuItem>)}
          {sections.length > 0 && <DropdownMenuSeparator/>}
          <DropdownMenuItem className={item} onSelect={onCreateSection}><Plus className={icon}/>创建分区…</DropdownMenuItem>
        </DropdownMenuSubContent>
      </DropdownMenuSub>
      <DropdownMenuSeparator/>
      <DropdownMenuItem className={item} onSelect={onFork}><GitBranch className={icon}/>分支</DropdownMenuItem>
      <DropdownMenuSeparator/>
      <DropdownMenuItem className={item} disabled><Share className={icon}/>分享</DropdownMenuItem>
      <DropdownMenuSub><DropdownMenuSubTrigger className={item}><Copy className={icon}/>复制</DropdownMenuSubTrigger>
        <DropdownMenuSubContent className="min-w-48" data-custom-contextmenu="1">
          <DropdownMenuItem disabled={!onCopyDeepLink} onSelect={onCopyDeepLink}>复制深度链接</DropdownMenuItem>
          <DropdownMenuItem onSelect={onCopy}>复制为 Markdown</DropdownMenuItem>
          <DropdownMenuItem disabled={!onCopyDirectory} onSelect={onCopyDirectory}>复制工作目录</DropdownMenuItem>
        </DropdownMenuSubContent>
      </DropdownMenuSub>
      <DropdownMenuSeparator/>
      <DropdownMenuItem className={item} onSelect={onOpenInNewWindow}><ArrowUpRightFromSquare className={icon}/>在新窗口中打开</DropdownMenuItem>
      <DropdownMenuSub><DropdownMenuSubTrigger className={item}><SquareArrowOutUpRight className={icon}/>打开方式</DropdownMenuSubTrigger>
        <DropdownMenuSubContent className="min-w-48" data-custom-contextmenu="1">
          {OPEN_TARGET_OPTIONS.map((target) => <DropdownMenuItem key={target.title} disabled={!onOpenTarget} onSelect={() => onOpenTarget?.(target.title)}>
            {target.title}{target.title === defaultOpenTarget && <Check className="ml-auto h-3.5 w-3.5"/>}
          </DropdownMenuItem>)}
        </DropdownMenuSubContent>
      </DropdownMenuSub>
      <DropdownMenuItem className={item} onSelect={onOpenAutomation}><Clock className={icon}/>添加自动化…</DropdownMenuItem>
      <DropdownMenuSeparator/>
      <DropdownMenuItem className={item} onSelect={onArchive}><Archive className={icon}/>归档</DropdownMenuItem>
    </DropdownMenuContent>
  </DropdownMenu>;
}
