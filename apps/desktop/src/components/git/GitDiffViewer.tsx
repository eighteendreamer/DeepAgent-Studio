import { useCallback, useEffect, useMemo, useState } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { gitApplyHunk, gitDiff } from "../../api";
import type { GitChangedFile, GitDiff } from "../../types";

interface Props {
  projectPath: string;
  file: GitChangedFile;
  onRefresh?: () => Promise<void> | void;
}

type DiffMode = "unified" | "split";

type SplitRow =
  | {
      kind: "meta" | "hunk";
      text: string;
      oldLine: null;
      newLine: null;
      oldText: "";
      newText: "";
    }
  | {
      kind: "context" | "add" | "remove";
      text: "";
      oldLine: number | null;
      newLine: number | null;
      oldText: string;
      newText: string;
    };

interface UnifiedHunk {
  header: string;
  lines: string[];
}

interface UnifiedPatchParts {
  headerLines: string[];
  hunks: UnifiedHunk[];
}

export function GitDiffViewer({ projectPath, file, onRefresh }: Props) {
  const [diff, setDiff] = useState<GitDiff | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [hunkBusy, setHunkBusy] = useState<number | null>(null);
  const [mode, setMode] = useState<DiffMode>("unified");
  const staged = file.category === "staged";

  const loadDiff = useCallback(() => {
    setLoading(true);
    setError(null);
    return gitDiff(projectPath, file.path, staged)
      .then((next) => {
        setDiff(next);
      })
      .catch((err) => {
        setError(err instanceof Error ? err.message : String(err));
      })
      .finally(() => {
        setLoading(false);
      });
  }, [file.path, projectPath, staged]);

  useEffect(() => {
    let cancelled = false;
    loadDiff().finally(() => {
      if (cancelled) return;
    });
    return () => {
      cancelled = true;
    };
  }, [loadDiff]);

  const applyHunk = async (hunkIndex: number, patch: string) => {
    if (hunkBusy !== null) return;
    setHunkBusy(hunkIndex);
    setError(null);
    try {
      const result = await gitApplyHunk(projectPath, file.path, patch, staged);
      if (!result.ok) {
        setError(result.stderr || result.stdout || "Failed to apply hunk.");
        return;
      }
      await loadDiff();
      await onRefresh?.();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setHunkBusy(null);
    }
  };

  return (
    <div className="flex h-full min-h-0 flex-col bg-white">
      <div className="flex h-10 flex-shrink-0 items-center justify-between border-b border-border-theme px-3">
        <div className="flex min-w-0 items-center text-[13px] text-text-base">
          <FontAwesomeIcon icon={["far", "file-lines"]} className="mr-2 text-text-secondary" />
          <span className="truncate font-medium">{file.path}</span>
          {diff?.truncated && (
            <span className="ml-2 rounded bg-amber-50 px-1.5 py-0.5 text-[10px] text-amber-700">
              truncated
            </span>
          )}
        </div>
        <div className="ml-3 flex shrink-0 items-center gap-2">
          <DiffModeToggle mode={mode} onChange={setMode} />
          <div className="flex items-center gap-1.5 text-[12px] font-medium tabular-nums">
            <span className="text-green-600">+{file.additions}</span>
            <span className="text-red-500">-{file.deletions}</span>
          </div>
        </div>
      </div>

      <div className="min-h-0 flex-1 overflow-auto bg-[#fbfbfb]">
        {loading ? (
          <div className="flex h-full items-center justify-center text-[13px] text-text-secondary">
            Loading diff...
          </div>
        ) : error ? (
          <div className="p-4 text-[13px] text-red-500">{error}</div>
        ) : diff?.text ? (
          mode === "split" ? (
            <SplitDiffText text={diff.text} />
          ) : (
            <InteractiveDiffText
              text={diff.text}
              staged={staged}
              busyHunk={hunkBusy}
              onApplyHunk={(hunkIndex, patch) => void applyHunk(hunkIndex, patch)}
            />
          )
        ) : (
          <div className="flex h-full flex-col items-center justify-center px-6 text-center text-[13px] text-text-secondary">
            <FontAwesomeIcon icon={["fas", "circle-info"]} className="mb-2 text-[15px]" />
            {file.category === "untracked"
              ? "No text diff is available for this untracked file."
              : "This file has no displayable diff."}
          </div>
        )}
      </div>
    </div>
  );
}

function DiffModeToggle({ mode, onChange }: { mode: DiffMode; onChange: (mode: DiffMode) => void }) {
  return (
    <div className="inline-flex h-7 overflow-hidden rounded-md border border-border-theme bg-white text-[11px]">
      <button
        type="button"
        className={`px-2.5 font-medium ${
          mode === "unified" ? "bg-gray-100 text-text-base" : "text-text-secondary hover:bg-gray-50"
        }`}
        onClick={() => onChange("unified")}
      >
        Unified
      </button>
      <button
        type="button"
        className={`border-l border-border-theme px-2.5 font-medium ${
          mode === "split" ? "bg-gray-100 text-text-base" : "text-text-secondary hover:bg-gray-50"
        }`}
        onClick={() => onChange("split")}
      >
        Split
      </button>
    </div>
  );
}

export function DiffText({ text }: { text: string }) {
  return (
    <pre className="min-w-full text-[12px] leading-5">
      {text.split("\n").map((line, index) => (
        <DiffLine key={`${index}-${line.slice(0, 12)}`} line={line} lineNo={index + 1} />
      ))}
    </pre>
  );
}

function InteractiveDiffText({
  text,
  staged,
  busyHunk,
  onApplyHunk,
}: {
  text: string;
  staged: boolean;
  busyHunk: number | null;
  onApplyHunk: (hunkIndex: number, patch: string) => void;
}) {
  const parts = useMemo(() => parseUnifiedPatch(text), [text]);
  if (parts.hunks.length === 0) return <DiffText text={text} />;
  return (
    <div className="min-w-full text-[12px] leading-5">
      {parts.headerLines.map((line, index) => (
        <DiffLine key={`header-${index}-${line.slice(0, 12)}`} line={line} lineNo={index + 1} />
      ))}
      {parts.hunks.map((hunk, index) => {
        const patch = buildSingleHunkPatch(parts.headerLines, hunk);
        const busy = busyHunk === index;
        return (
          <div key={`${index}:${hunk.header}`}>
            <div className="grid grid-cols-[52px_minmax(0,1fr)_auto] bg-blue-50 text-blue-700">
              <span className="select-none border-r border-black/5 pr-2 text-right text-[11px] text-text-secondary">
                {parts.headerLines.length + index + 1}
              </span>
              <code className="px-3 font-mono">{hunk.header}</code>
              <button
                type="button"
                className="mr-2 self-center rounded border border-blue-200 bg-white px-2 py-0.5 text-[11px] font-medium text-blue-700 hover:bg-blue-100 disabled:cursor-not-allowed disabled:opacity-50"
                disabled={busyHunk !== null}
                onClick={() => onApplyHunk(index, patch)}
              >
                <FontAwesomeIcon
                  icon={busy ? ["fas", "spinner"] : ["fas", staged ? "minus" : "plus"]}
                  className={`mr-1 text-[10px] ${busy ? "animate-spin" : ""}`}
                />
                {staged ? "Unstage hunk" : "Stage hunk"}
              </button>
            </div>
            {hunk.lines.map((line, lineIndex) => (
              <DiffLine
                key={`${index}:${lineIndex}:${line.slice(0, 12)}`}
                line={line}
                lineNo={parts.headerLines.length + lineIndex + 2}
              />
            ))}
          </div>
        );
      })}
    </div>
  );
}

function DiffLine({ line, lineNo }: { line: string; lineNo: number }) {
  const kind = diffLineKind(line);
  const cls = lineClass(kind);

  return (
    <div className={`grid grid-cols-[52px_minmax(0,1fr)] whitespace-pre ${cls}`}>
      <span className="select-none border-r border-black/5 pr-2 text-right text-[11px] text-text-secondary">
        {lineNo}
      </span>
      <code className="px-3 font-mono">{line || " "}</code>
    </div>
  );
}

function SplitDiffText({ text }: { text: string }) {
  const rows = useMemo(() => parseSplitDiff(text), [text]);
  return (
    <div className="min-w-[920px] text-[12px] leading-5">
      <div className="sticky top-0 z-10 grid grid-cols-[64px_minmax(0,1fr)_64px_minmax(0,1fr)] border-b border-border-theme bg-gray-100 text-[11px] font-medium text-text-secondary">
        <div className="border-r border-border-theme px-2 py-1 text-right">Old</div>
        <div className="border-r border-border-theme px-3 py-1">Before</div>
        <div className="border-r border-border-theme px-2 py-1 text-right">New</div>
        <div className="px-3 py-1">After</div>
      </div>
      {rows.map((row, index) =>
        isFullWidthRow(row) ? (
          <div
            key={`${index}:${row.text}`}
            className={`grid grid-cols-[64px_minmax(0,1fr)] ${lineClass(row.kind)}`}
          >
            <div className="border-r border-black/5" />
            <code className="px-3 font-mono">{row.text || " "}</code>
          </div>
        ) : (
          <SplitDiffRow key={`${index}:${row.oldLine ?? ""}:${row.newLine ?? ""}`} row={row} />
        ),
      )}
    </div>
  );
}

function SplitDiffRow({ row }: { row: Extract<SplitRow, { kind: "context" | "add" | "remove" }> }) {
  const oldCls = row.kind === "remove" ? "bg-red-50 text-red-800" : "text-text-base";
  const newCls = row.kind === "add" ? "bg-green-50 text-green-800" : "text-text-base";
  return (
    <div className="grid grid-cols-[64px_minmax(0,1fr)_64px_minmax(0,1fr)] whitespace-pre">
      <div className={`${oldCls} select-none border-r border-black/5 px-2 text-right text-[11px] text-text-secondary`}>
        {row.oldLine ?? ""}
      </div>
      <code className={`${oldCls} border-r border-black/5 px-3 font-mono`}>{row.oldText || " "}</code>
      <div className={`${newCls} select-none border-r border-black/5 px-2 text-right text-[11px] text-text-secondary`}>
        {row.newLine ?? ""}
      </div>
      <code className={`${newCls} px-3 font-mono`}>{row.newText || " "}</code>
    </div>
  );
}

function isFullWidthRow(row: SplitRow): row is Extract<SplitRow, { kind: "meta" | "hunk" }> {
  return row.kind === "meta" || row.kind === "hunk";
}

function parseUnifiedPatch(text: string): UnifiedPatchParts {
  const lines = text.endsWith("\n") ? text.slice(0, -1).split("\n") : text.split("\n");
  const headerLines: string[] = [];
  const hunks: UnifiedHunk[] = [];
  let current: UnifiedHunk | null = null;
  for (const line of lines) {
    if (line.startsWith("@@")) {
      if (current) hunks.push(current);
      current = { header: line, lines: [] };
      continue;
    }
    if (current) {
      current.lines.push(line);
    } else {
      headerLines.push(line);
    }
  }
  if (current) hunks.push(current);
  return { headerLines, hunks };
}

function buildSingleHunkPatch(headerLines: string[], hunk: UnifiedHunk): string {
  return [...headerLines, hunk.header, ...hunk.lines].join("\n") + "\n";
}

function parseSplitDiff(text: string): SplitRow[] {
  const rows: SplitRow[] = [];
  let oldLine = 0;
  let newLine = 0;
  for (const line of text.split("\n")) {
    if (line.startsWith("@@")) {
      const hunk = parseHunkHeader(line);
      oldLine = hunk?.oldStart ?? oldLine;
      newLine = hunk?.newStart ?? newLine;
      rows.push({ kind: "hunk", text: line, oldLine: null, newLine: null, oldText: "", newText: "" });
      continue;
    }
    if (isMetaLine(line)) {
      rows.push({ kind: "meta", text: line, oldLine: null, newLine: null, oldText: "", newText: "" });
      continue;
    }
    if (line.startsWith("+")) {
      rows.push({
        kind: "add",
        text: "",
        oldLine: null,
        newLine,
        oldText: "",
        newText: line.slice(1),
      });
      newLine += 1;
      continue;
    }
    if (line.startsWith("-")) {
      rows.push({
        kind: "remove",
        text: "",
        oldLine,
        newLine: null,
        oldText: line.slice(1),
        newText: "",
      });
      oldLine += 1;
      continue;
    }
    const content = line.startsWith(" ") ? line.slice(1) : line;
    rows.push({
      kind: "context",
      text: "",
      oldLine,
      newLine,
      oldText: content,
      newText: content,
    });
    oldLine += 1;
    newLine += 1;
  }
  return rows;
}

function parseHunkHeader(line: string): { oldStart: number; newStart: number } | null {
  const match = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(line);
  if (!match) return null;
  return {
    oldStart: Number(match[1]),
    newStart: Number(match[2]),
  };
}

function diffLineKind(line: string): "add" | "remove" | "hunk" | "meta" | "context" {
  if (line.startsWith("@@")) return "hunk";
  if (line.startsWith("+") && !line.startsWith("+++")) return "add";
  if (line.startsWith("-") && !line.startsWith("---")) return "remove";
  if (isMetaLine(line)) return "meta";
  return "context";
}

function isMetaLine(line: string): boolean {
  return (
    line.startsWith("diff --git") ||
    line.startsWith("index ") ||
    line.startsWith("---") ||
    line.startsWith("+++") ||
    line.startsWith("new file mode") ||
    line.startsWith("deleted file mode") ||
    line.startsWith("similarity index") ||
    line.startsWith("rename from") ||
    line.startsWith("rename to")
  );
}

function lineClass(kind: "add" | "remove" | "hunk" | "meta" | "context"): string {
  return kind === "add"
    ? "bg-green-50 text-green-800"
    : kind === "remove"
      ? "bg-red-50 text-red-800"
      : kind === "hunk"
        ? "bg-blue-50 text-blue-700"
        : kind === "meta"
          ? "bg-gray-100 text-text-secondary"
          : "text-text-base";
}
