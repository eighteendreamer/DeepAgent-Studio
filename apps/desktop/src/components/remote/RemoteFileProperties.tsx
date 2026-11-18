import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { sshFileMetadata, sshRealpath, type RemoteFileMetadata } from "../../api";
import { Button } from "../shadcn/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "../shadcn/dialog";

export async function loadRemoteFileProperties(connectionId: string, path: string): Promise<RemoteFileMetadata> {
  const result = await sshFileMetadata(connectionId, path, false);
  if (result.connection_id !== connectionId || result.path !== path || result.followed_symlinks !== false) {
    throw new Error("Unexpected remote file metadata identity");
  }
  return result;
}

export function RemoteFilePropertiesBody({ metadata, canonicalPath, label }: {
  metadata: RemoteFileMetadata; canonicalPath: string | null; label: (key: string) => string;
}) {
  const unknown = label("unknown");
  const fields = [
    ["path", metadata.path],
    ["type", label(`kind.${metadata.kind}`)],
    ["size", metadata.size_bytes == null ? unknown : `${metadata.size_bytes} B`],
    ["permissions", metadata.permissions == null ? unknown : (metadata.permissions & 0o7777).toString(8).padStart(4, "0")],
    ["owner", metadata.user ?? (metadata.uid == null ? unknown : String(metadata.uid))],
    ["group", metadata.group ?? (metadata.gid == null ? unknown : String(metadata.gid))],
    ["uid", metadata.uid == null ? unknown : String(metadata.uid)],
    ["gid", metadata.gid == null ? unknown : String(metadata.gid)],
    ["modified", metadata.modified_ms == null ? unknown : new Date(metadata.modified_ms).toLocaleString()],
    ["accessed", metadata.accessed_ms == null ? unknown : new Date(metadata.accessed_ms).toLocaleString()],
    ["canonical", canonicalPath ?? label("notResolved")],
  ];
  return <dl className="grid grid-cols-[7rem_minmax(0,1fr)] gap-x-3 gap-y-2 text-[13px]">
    {fields.map(([key, value]) => <div key={key} className="contents"><dt className="text-text-secondary">{label(key)}</dt><dd className="min-w-0 break-all font-mono">{value}</dd></div>)}
  </dl>;
}

/** Read-only lstat by default. Resolving a link is a separate explicit action. */
export function RemoteFileProperties({ connectionId, path, onClose }: {
  connectionId: string; path: string; onClose: () => void;
}) {
  const { t } = useTranslation();
  const label = (key: string) => t(`remote.properties.${key}`);
  const [metadata, setMetadata] = useState<RemoteFileMetadata | null>(null);
  const [canonicalPath, setCanonicalPath] = useState<string | null>(null);
  const [busy, setBusy] = useState(true);
  const [resolving, setResolving] = useState(false);
  const [error, setError] = useState("");
  const [refresh, setRefresh] = useState(0);
  const generation = useRef(0);
  useEffect(() => {
    const owner = ++generation.current;
    setBusy(true); setMetadata(null); setCanonicalPath(null); setError(""); setResolving(false);
    void loadRemoteFileProperties(connectionId, path).then(result => {
      if (owner === generation.current) setMetadata(result);
    }).catch(error => {
      if (owner === generation.current) setError(String(error));
    }).finally(() => {
      if (owner === generation.current) setBusy(false);
    });
    return () => { generation.current++; };
  }, [connectionId, path, refresh]);

  async function resolvePath() {
    const owner = generation.current;
    setResolving(true); setError("");
    try {
      const result = await sshRealpath(connectionId, path);
      if (result.connection_id !== connectionId || result.path !== path) throw new Error("Unexpected remote realpath identity");
      if (owner === generation.current) setCanonicalPath(result.canonical_path);
    } catch (error) {
      if (owner === generation.current) setError(String(error));
    } finally {
      if (owner === generation.current) setResolving(false);
    }
  }

  return <Dialog open onOpenChange={open => { if (!open) onClose(); }}><DialogContent className="flex max-h-[85vh] w-[min(640px,calc(100vw-32px))] flex-col overflow-hidden" aria-label={label("title")}>
    <DialogHeader><DialogTitle>{label("title")}</DialogTitle><DialogDescription className="break-all">{path}</DialogDescription></DialogHeader>
    <div className="min-h-0 overflow-y-auto px-5 py-4">
    {busy && <p role="status">{label("loading")}</p>}
    {error && <p role="alert" className="break-words text-red-500">{label("failed")}: {error}</p>}
    {metadata && <RemoteFilePropertiesBody metadata={metadata} canonicalPath={canonicalPath} label={label} />}
    </div>
    <DialogFooter className="flex-wrap gap-2">
      <Button variant="outline" disabled={busy || resolving} onClick={() => void resolvePath()}>{resolving ? label("loading") : label("resolve")}</Button>
      <Button variant="outline" disabled={busy || resolving} onClick={() => setRefresh(value => value + 1)}>{label("refresh")}</Button>
      <Button onClick={onClose}>{label("close")}</Button>
    </DialogFooter>
  </DialogContent></Dialog>;
}
