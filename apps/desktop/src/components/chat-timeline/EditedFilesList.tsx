import { useState } from "react";
import {
  ChevronDown,
  ChevronUp,
  File,
  FileCode,
  FileJson,
  FileText,
  FileType,
  Palette,
  type LucideIcon,
} from "lucide-react";
import type { EditedFile } from "../../types";
import { HoverInfo } from "../ui/HoverInfo";

const COLLAPSED_ROWS = 4;

function getFileIcon(path: string): LucideIcon {
  const ext = path.split(".").pop()?.toLowerCase();
  switch (ext) {
    case "ts":
    case "tsx":
    case "js":
    case "jsx":
    case "rs":
    case "py":
    case "go":
    case "java":
    case "c":
    case "cpp":
    case "h":
    case "rb":
    case "swift":
    case "kt":
    case "lua":
    case "sh":
    case "bash":
    case "zsh":
    case "fish":
    case "ps1":
    case "toml":
    case "yaml":
    case "yml":
      return FileCode;
    case "json":
    case "jsonc":
    case "json5":
      return FileJson;
    case "md":
    case "mdx":
    case "txt":
    case "log":
    case "csv":
      return FileText;
    case "css":
    case "scss":
    case "less":
    case "sass":
      return Palette;
    case "html":
    case "htm":
    case "xml":
    case "svg":
      return FileType;
    default:
      return File;
  }
}

function basename(path: string): string {
  const parts = path.replace(/\\/g, "/").split("/");
  return parts[parts.length - 1] || path;
}

function Counts({
  added,
  deleted,
}: {
  added: number;
  deleted: number;
}) {
  return (
    <span className="inline-flex gap-1.5 font-mono text-[11px]">
      <span className="text-green-600 dark:text-green-400">+{added}</span>
      <span className="text-red-600 dark:text-red-400">-{deleted}</span>
    </span>
  );
}

export function EditedFilesList({ files }: { files: EditedFile[] }) {
  const [expanded, setExpanded] = useState(false);

  if (!files || files.length === 0) return null;

  const totalAdded = files.reduce((s, f) => s + f.lines_added, 0);
  const totalDeleted = files.reduce((s, f) => s + f.lines_removed, 0);
  const singleFile = files.length === 1 ? files[0] : undefined;
  const foldable = files.length > COLLAPSED_ROWS;
  const rows = foldable && !expanded ? files.slice(0, COLLAPSED_ROWS) : files;

  const SingleIcon = singleFile ? getFileIcon(singleFile.path) : FileCode;

  return (
    <div className="mt-3 flex flex-col overflow-hidden rounded-lg border border-[var(--theme-border)]">
      {/* Header */}
      <div className="flex h-[52px] items-center gap-2.5 bg-[var(--theme-elevated,var(--theme-bg))] px-3">
        <span className="grid h-9 w-9 flex-none place-items-center rounded-[8px] border border-[var(--theme-border)] bg-[var(--theme-bg)]">
          <SingleIcon size={18} className="text-[var(--theme-text-secondary)]" />
        </span>
        <span className="flex min-w-0 flex-1 flex-col">
          <span className="truncate text-[13px] font-medium leading-5 text-[var(--theme-fg)]">
            {singleFile
              ? `已编辑 ${basename(singleFile.path)}`
              : `已编辑 ${files.length} 个文件`}
          </span>
          <span className="text-[10px] leading-4 text-[var(--theme-text-secondary)]">
            <Counts added={totalAdded} deleted={totalDeleted} />
          </span>
        </span>
      </div>

      {/* File list (multi-file only) */}
      {!singleFile && (
        <ul className="m-0 list-none border-t border-[var(--theme-border)] p-0">
          {rows.map((file) => {
            const Icon = getFileIcon(file.path);
            return (
              <li key={file.path}>
                <HoverInfo content={file.path}>
                  <div className="flex w-full items-center justify-between gap-2.5 px-3 py-[7px] text-left text-[12px] leading-[18px] text-[var(--theme-text-secondary)] transition-colors hover:bg-[var(--theme-hover,rgba(0,0,0,0.04))]">
                    <span className="flex min-w-0 flex-1 items-center gap-2">
                      <Icon
                        size={14}
                        className="flex-none opacity-60"
                      />
                      <span className="truncate font-mono">
                        {file.path}
                      </span>
                    </span>
                    <span className="flex-none">
                      <Counts
                        added={file.lines_added}
                        deleted={file.lines_removed}
                      />
                    </span>
                  </div>
                </HoverInfo>
              </li>
            );
          })}
        </ul>
      )}

      {/* Fold toggle */}
      {foldable && (
        <button
          type="button"
          onClick={() => setExpanded((v) => !v)}
          className="flex w-full items-center justify-start gap-1 border-t border-[var(--theme-border)] px-3 py-2.5 text-left text-[12px] leading-[18px] text-[var(--theme-text-secondary)] transition-colors hover:bg-[var(--theme-hover,rgba(0,0,0,0.04))]"
        >
          <span>
            {expanded
              ? "收起"
              : `显示全部 ${files.length} 个文件`}
          </span>
          {expanded ? (
            <ChevronUp size={14} />
          ) : (
            <ChevronDown size={14} />
          )}
        </button>
      )}
    </div>
  );
}
