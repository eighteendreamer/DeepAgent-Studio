import { useEffect, useState } from "react";
import { pickProjectFolder } from "../api";
import { Button } from "./shadcn/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "./shadcn/dialog";
import { Input } from "./shadcn/input";

export type EditingProject = { path: string; name: string };

export function EditProjectDialog({ open, project, onClose, onSave }: {
  open: boolean;
  project: EditingProject | null;
  onClose: () => void;
  onSave: (name: string, folder: string) => Promise<void>;
}) {
  const [name, setName] = useState("");
  const [folder, setFolder] = useState("");
  const [error, setError] = useState("");
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (!open || !project) return;
    setName(project.name);
    setFolder(project.path);
    setError("");
    setSaving(false);
  }, [open, project]);

  const folderChanged = Boolean(project) && folder !== project?.path;
  const canSave = Boolean(name.trim()) && Boolean(folder) && !saving;

  return <Dialog open={open} onOpenChange={(next) => { if (!next && !saving) onClose(); }}>
    <DialogContent className="max-w-md">
      <DialogHeader>
        <DialogTitle>编辑项目</DialogTitle>
        <DialogDescription className="break-all">{project?.path}</DialogDescription>
      </DialogHeader>
      <form onSubmit={async (event) => {
        event.preventDefault();
        if (!canSave) return;
        setSaving(true);
        setError("");
        try {
          await onSave(name.trim(), folder);
          onClose();
        } catch (cause) {
          setError(String(cause));
          setSaving(false);
        }
      }}>
        <label className="block text-[13px] text-text-secondary" htmlFor="edit-project-name">项目名称</label>
        <Input id="edit-project-name" className="mt-1.5" autoFocus maxLength={64} value={name}
          onChange={(event) => setName(event.target.value)} />
        <label className="mt-4 block text-[13px] text-text-secondary" htmlFor="edit-project-folder">源文件夹</label>
        <div className="mt-1.5 flex items-center gap-2">
          <Input id="edit-project-folder" readOnly value={folder}
            className="min-w-0 flex-1 cursor-default truncate" title={folder} />
          <Button type="button" variant="outline" disabled={saving} onClick={async () => {
            setError("");
            try {
              const picked = await pickProjectFolder();
              if (picked) setFolder(picked);
            } catch (cause) {
              setError(String(cause));
            }
          }}>浏览…</Button>
        </div>
        {folderChanged && <p className="mt-2 text-xs text-amber-600">
          保存后该项目下的会话会一并归到新文件夹。
        </p>}
        {error && <p className="mt-2 text-xs text-red-500">{error}</p>}
        <DialogFooter className="mt-5">
          <Button type="button" variant="ghost" disabled={saving} onClick={onClose}>取消</Button>
          <Button type="submit" disabled={!canSave}>保存</Button>
        </DialogFooter>
      </form>
    </DialogContent>
  </Dialog>;
}
