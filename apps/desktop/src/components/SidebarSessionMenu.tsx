import { useEffect, useState } from "react";
import { getSessionDetail } from "../api";
import type { SessionSummary, TimelineEntry } from "../types";
import { ChatSessionMenu, formatSessionRewindTimestamp, sessionRewindEntries } from "./ChatSessionMenu";
import { RenameSessionDialog } from "./RenameSessionDialog";
import { message } from "./message";

export interface SidebarSessionActions {
  rename: (id: string, title: string) => Promise<void>;
  copy: (id: string) => void;
  export: (id: string) => void;
  fork: (id: string) => void;
  rewind: (id: string, sequence: number) => void;
  openInNewWindow: (id: string) => void;
  automation: (id: string) => void;
}

export function SidebarSessionMenu({ session, position, open, onOpenChange, actions, onPin, onArchive }: {
  session: SessionSummary;
  position: { left: number; top: number };
  open: boolean;
  onOpenChange: (open: boolean) => void;
  actions: SidebarSessionActions;
  onPin: () => void;
  onArchive: () => void;
}) {
  const [rewindEntries, setRewindEntries] = useState<TimelineEntry[]>([]);
  const [renaming, setRenaming] = useState(false);
  const [saving, setSaving] = useState(false);
  const [renameError, setRenameError] = useState<string | null>(null);
  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setRewindEntries([]);
    getSessionDetail(session.id).then((detail) => {
      if (!cancelled) setRewindEntries(sessionRewindEntries(detail.timeline));
    }).catch((error) => {
      if (!cancelled) message.error(`加载会话回退点失败：${String(error)}`);
    });
    return () => { cancelled = true; };
  }, [session.id, open]);
  return <>
    <ChatSessionMenu title={session.title} pinned={session.pinned} open={open}
      contextMenuPosition={position} onOpenChange={onOpenChange}
      rewindEntries={rewindEntries} formatRewindTimestamp={formatSessionRewindTimestamp}
      onPin={onPin} onArchive={onArchive}
      onRename={() => { setRenameError(null); setRenaming(true); }}
      onCopy={() => actions.copy(session.id)} onExport={() => actions.export(session.id)}
      onFork={() => actions.fork(session.id)} onRewindEntry={(seq) => actions.rewind(session.id, seq)}
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
