import { useState } from "react";
import { ChevronDown, ChevronRight, FileEdit, RotateCcw } from "lucide-react";
import type { EditedFile } from "../../types";
import { HoverInfo } from "../ui/HoverInfo";

export function EditedFilesList({ files }: { files: EditedFile[] }) {
  const [expanded, setExpanded] = useState(false);

  if (!files || files.length === 0) return null;

  const visibleFiles = expanded ? files : files.slice(0, 3);
  const hiddenCount = files.length - 3;

  const handleRevert = (filePath: string) => {
    // TODO: 实现 git revert 功能
    console.log("Revert file:", filePath);
  };

  return (
    <div className="mt-3 rounded-md border border-border-primary bg-bg-secondary/30 px-3 py-2">
      <div className="mb-2 flex items-center gap-1.5 text-[11.5px] font-medium text-text-secondary">
        <FileEdit size={12} />
        <span>已编辑 {files.length} 个文件</span>
      </div>
      <div className="space-y-1">
        {visibleFiles.map((file, idx) => (
          <div
            key={idx}
            className="group flex items-center justify-between gap-2 rounded px-2 py-1.5 text-[11px] hover:bg-bg-secondary"
          >
            <div className="flex min-w-0 flex-1 items-center gap-2">
              <HoverInfo content={file.path}>
                <span className="truncate font-mono text-text-base">
                  {file.path}
                </span>
              </HoverInfo>
              <span className="shrink-0 text-[10px] text-text-tertiary">
                +{file.lines_added} -{file.lines_removed}
              </span>
            </div>
            <HoverInfo content="撤销修改">
              <button
                type="button"
                onClick={() => handleRevert(file.path)}
                className="invisible flex h-6 shrink-0 items-center gap-1 rounded px-2 text-[10px] text-text-tertiary transition hover:bg-bg-tertiary hover:text-text-base group-hover:visible"
              >
                <RotateCcw size={10} />
                <span>撤销</span>
              </button>
            </HoverInfo>
          </div>
        ))}
      </div>
      {hiddenCount > 0 && (
        <button
          type="button"
          onClick={() => setExpanded(!expanded)}
          className="mt-2 flex w-full items-center justify-center gap-1 rounded py-1 text-[11px] text-text-tertiary transition hover:bg-bg-secondary hover:text-text-base"
        >
          {expanded ? (
            <>
              <ChevronDown size={12} />
              <span>收起</span>
            </>
          ) : (
            <>
              <ChevronRight size={12} />
              <span>显示更多 ({hiddenCount} 个文件)</span>
            </>
          )}
        </button>
      )}
    </div>
  );
}
