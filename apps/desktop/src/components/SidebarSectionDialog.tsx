import { useEffect, useState } from "react";
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter } from "./shadcn/dialog";
import { Input } from "./shadcn/input";
import { Button } from "./shadcn/button";

export function SidebarSectionDialog({ open, onClose, onCreate, initialName = "", title = "创建分区", confirmLabel = "创建" }: {
  open: boolean; onClose: () => void; onCreate: (name: string) => Promise<void>;
  initialName?: string; title?: string; confirmLabel?: string;
}) {
  const [name, setName] = useState("");
  const [error, setError] = useState("");
  const [saving, setSaving] = useState(false);
  useEffect(() => { if (open) { setName(initialName); setError(""); } }, [open, initialName]);
  return <Dialog open={open} onOpenChange={(next) => { if (!next && !saving) onClose(); }}>
    <DialogContent className="max-w-sm"><DialogHeader><DialogTitle>{title}</DialogTitle></DialogHeader>
      <form className="px-6 pb-6" onSubmit={async (event) => {
        event.preventDefault();
        if (!name.trim() || saving) return;
        setSaving(true); setError("");
        try { await onCreate(name.trim()); onClose(); }
        catch (e) { setError(String(e)); }
        finally { setSaving(false); }
      }}>
        <Input autoFocus aria-label="分区名称" placeholder="分区名称" maxLength={64} value={name} onChange={(e) => setName(e.target.value)}/>
        {error && <p className="mt-2 text-xs text-red-500">{error}</p>}
        <DialogFooter className="mt-5 p-0"><Button type="button" variant="ghost" disabled={saving} onClick={onClose}>取消</Button>
          <Button type="submit" disabled={saving || !name.trim()}>{confirmLabel}</Button></DialogFooter>
      </form>
    </DialogContent>
  </Dialog>;
}
