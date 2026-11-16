import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { SshKnownHostsImportPreview } from "../../api";
import { Button } from "../shadcn/button";

/** Large reviewed files must not mount thousands of fingerprint rows at once. */
export function KnownHostsImportReport({ preview, imported }: {
  preview: SshKnownHostsImportPreview; imported?: number;
}) {
  const { t } = useTranslation();
  const [page, setPage] = useState(0);
  const pageSize = 100;
  const pages = Math.max(1, Math.ceil(preview.rows.length / pageSize));
  const current = Math.min(page, pages - 1);
  const visible = preview.rows.slice(current * pageSize, (current + 1) * pageSize);
  const label = (key: string) => t(`settings.connections.trustManager.${key}`);
  return <section className="space-y-2" aria-label={label("fileReport")}>
    <p role="status">{label(imported === undefined ? "importable" : "imported")}: {imported ?? preview.importable} · {label("skipped")}: {preview.skipped}</p>
    <p className="break-all text-text-secondary">{preview.known_hosts_path}</p>
    <p className="break-all font-mono text-[10px]">SHA256: {preview.source_sha256}</p>
    <ul className="space-y-2">
      {visible.map((row, index) => <li key={`${current}:${index}`} data-import-row={row.line_number} className="space-y-1 rounded-lg border border-border-theme p-2">
        <p>{label("trustLine")} {row.line_number} · {row.host ? `${row.host}:${row.port}` : "—"}</p>
        {row.fingerprint && <p className="break-all font-mono">{row.algorithm} · {row.fingerprint}</p>}
        <p className="text-text-secondary">{row.status === "ready" && imported !== undefined ? label("imported") : label(`fileStatus.${row.status}`)}</p>
      </li>)}
    </ul>
    {pages > 1 && <div className="flex items-center justify-between gap-2">
      <Button size="sm" variant="outline" disabled={current === 0} onClick={() => setPage(current - 1)}>{label("previousPage")}</Button>
      <span>{current + 1} / {pages}</span>
      <Button size="sm" variant="outline" data-import-next disabled={current + 1 === pages} onClick={() => setPage(current + 1)}>{label("nextPage")}</Button>
    </div>}
  </section>;
}
