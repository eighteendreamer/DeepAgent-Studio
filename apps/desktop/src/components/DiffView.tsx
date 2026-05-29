import { useEffect, useState } from "react";
import { computeDiff } from "../api";
import type { DiffResult } from "../types";

interface Props {
  open: boolean;
  onClose: () => void;
}

const SAMPLE_OLD = `fn retry(n: u32) {
    for _ in 0..n {
        attempt();
    }
}`;

const SAMPLE_NEW = `fn retry(n: u32) {
    let mut delay = 100;
    for _ in 0..n {
        attempt();
        sleep(delay);
        delay *= 2;
    }
}`;

export function DiffView({ open, onClose }: Props) {
  const [oldText, setOldText] = useState(SAMPLE_OLD);
  const [newText, setNewText] = useState(SAMPLE_NEW);
  const [result, setResult] = useState<DiffResult | null>(null);

  useEffect(() => {
    if (!open) return;
    computeDiff(oldText, newText).then(setResult).catch(() => setResult(null));
  }, [open, oldText, newText]);

  if (!open) return null;

  return (
    <div className="dialog-backdrop" onClick={onClose}>
      <div className="diff-modal" onClick={(e) => e.stopPropagation()}>
        <div className="diff-header">
          <h3>Diff view</h3>
          {result && (
            <span className="diff-summary">
              <span className="added">+{result.added}</span>{" "}
              <span className="removed">-{result.removed}</span>
            </span>
          )}
          <button className="btn" onClick={onClose}>
            Close
          </button>
        </div>
        <div className="diff-inputs">
          <textarea value={oldText} onChange={(e) => setOldText(e.target.value)} spellCheck={false} />
          <textarea value={newText} onChange={(e) => setNewText(e.target.value)} spellCheck={false} />
        </div>
        <div className="diff-body">
          {result?.lines.map((l, idx) => (
            <div key={idx} className={`diff-line ${l.kind}`}>
              <span className="ln">{l.old_line ?? ""}</span>
              <span className="ln">{l.new_line ?? ""}</span>
              <span className="sign">{l.kind === "added" ? "+" : l.kind === "removed" ? "-" : " "}</span>
              <span className="code">{l.content}</span>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
