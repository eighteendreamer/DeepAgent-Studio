import { useState } from "react";
import type { SessionSummary } from "../types";
import { ChatSessionMenu, type SessionMenuOptions } from "./ChatSessionMenu";
import { RenameSessionDialog } from "./RenameSessionDialog";

export interface SidebarSessionActions {
  rename: (id: string, title: string) => Promise<void>;
  copy: (id: string) => void;
  fork: (id: string) => void;
  openInNewWindow: (id: string) => void;
  automation: (id: string) => void;
}

export function SidebarSessionMenu({ session, position, open, onOpenChange, actions, onPin, onArchive, options }: {
  session: SessionSummary;
  options: SessionMenuOptions;
  position: { left: number; top: number };
  open: boolean;
  onOpenChange: (open: boolean) => void;
  actions: SidebarSessionActions;
  onPin: () => void;
  onArchive: () => void;
}) {
  const [renaming, setRenaming] = useState(false);
  const [saving, setSaving] = useState(false);
  const [renameError, setRenameError] = useState<string | null>(null);
  return <>
    <ChatSessionMenu {...options} title={session.title} pinned={session.pinned} open={open}
      contextMenuPosition={position} onOpenChange={onOpenChange}
      onPin={onPin} onArchive={onArchive}
      onRename={() => { setRenameError(null); setRenaming(true); }}
      onCopy={() => actions.copy(session.id)}
      onFork={() => actions.fork(session.id)}
      onOpenAutomation={() => actions.automation(session.id)}
      onOpenInNewWindow={() => actions.openInNewWindow(session.id)}/>
    <RenameSessionDialog open={renaming} initialValue={session.title ?? ""} loading={saving}
      error={renameError} onClose={() => setRenaming(false)} onConfirm={async (title) => {
        setSaving(true);
        setRenameError(null);
        try { await actions.rename(session.id, title); setRenaming(false); }
        catch (error) { setRenameError(String(error)); }
        finally { setSaving(false); }
      }}/>
  </>;
}
