import { useEffect, useState } from "react";
import { FileCode } from "lucide-react";
import { HoverInfo } from "../ui/HoverInfo";
import { computeDiff } from "../../api";
import type { DiffResult } from "../../types";
import { CodeBlock, CodeBlockActions, CodeBlockContent, CodeBlockCopyButton, CodeBlockHeader, CodeBlockLine, CodeBlockTitle } from "../shadcn/code-block";
import { canPreviewChange, type FileChange } from "./toolPresentation";

export function FileChangeBlock({ change, path }: { change: FileChange; path: string }) {
  const [result, setResult] = useState<{ change: FileChange; diff?: DiffResult; error?: string }>();
  const bounded = canPreviewChange(change);
  useEffect(() => {
    if (!bounded) return;
    let cancelled = false;
    computeDiff(change.before, change.after).then(diff => {
      if (!cancelled) setResult({ change, diff });
    }).catch(error => {
      if (!cancelled) setResult({ change, error: String(error) });
    });
    return () => { cancelled = true; };
  }, [change, bounded]);
  const current = result?.change === change ? result : undefined;
  const diff = current?.diff;
  const line = (value: number | null) => value === null || change.startLine === undefined ? "" : value + change.startLine - 1;
  return <CodeBlock size="sm">
    <CodeBlockHeader>
      <FileCode size={13} className="shrink-0 text-text-secondary" />
      <HoverInfo content={path}><CodeBlockTitle>{path.split(/[\\/]/).pop() || "代码变更"}</CodeBlockTitle></HoverInfo>
      {diff && <span className="flex shrink-0 gap-2 font-mono text-[11px] tabular-nums">
        <span className="text-emerald-700 dark:text-emerald-400">+{diff.added}</span>
        <span className="text-red-600 dark:text-red-400">−{diff.removed}</span>
      </span>}
      <CodeBlockActions><CodeBlockCopyButton value={change.after} aria-label="复制修改后代码" /></CodeBlockActions>
    </CodeBlockHeader>
    {diff ? <CodeBlockContent aria-label="代码变更">
      {diff.lines.map((item, index) => <CodeBlockLine key={index}
        variant={item.kind === "context" ? "default" : item.kind}
        gutter={<span aria-hidden="true" className="mr-3 inline-flex shrink-0 select-none gap-2 text-right tabular-nums text-text-secondary/60">
          {change.startLine !== undefined && <><span className="w-7">{line(item.old_line)}</span><span className="w-7">{line(item.new_line)}</span></>}
          <span className="w-3">{item.kind === "added" ? "+" : item.kind === "removed" ? "−" : " "}</span>
        </span>}>{item.content}</CodeBlockLine>)}
    </CodeBlockContent> : <p role="status" className="px-3 py-2 text-xs text-text-secondary">
      {!bounded ? "变更过大，未展开代码预览" : current?.error ? `代码差异加载失败：${current.error}` : "正在计算代码差异…"}
    </p>}
  </CodeBlock>;
}
