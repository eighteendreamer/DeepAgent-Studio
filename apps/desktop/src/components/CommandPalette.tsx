import { useEffect, useRef, useState } from "react";
import { getCommands } from "../api";
import type { Command } from "../types";

interface Props {
  open: boolean;
  onClose: () => void;
  onRun: (command: Command) => void;
}

export function CommandPalette({ open, onClose, onRun }: Props) {
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<Command[]>([]);
  const [selected, setSelected] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (open) {
      setQuery("");
      setSelected(0);
      inputRef.current?.focus();
    }
  }, [open]);

  useEffect(() => {
    if (!open) return;
    getCommands(query)
      .then((r) => {
        setResults(r);
        setSelected(0);
      })
      .catch(() => setResults([]));
  }, [query, open]);

  if (!open) return null;

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") onClose();
    else if (e.key === "ArrowDown") {
      e.preventDefault();
      setSelected((s) => Math.min(s + 1, results.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setSelected((s) => Math.max(s - 1, 0));
    } else if (e.key === "Enter" && results[selected]) {
      onRun(results[selected]);
      onClose();
    }
  };

  return (
    <div className="palette-backdrop" onClick={onClose}>
      <div className="palette" onClick={(e) => e.stopPropagation()} onKeyDown={onKeyDown}>
        <input
          ref={inputRef}
          className="palette-input"
          placeholder="Type a command…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <div className="palette-results">
          {results.length === 0 && <div className="palette-empty">No matching commands</div>}
          {results.map((c, i) => (
            <div
              key={c.id}
              className={`palette-item${i === selected ? " active" : ""}`}
              onMouseEnter={() => setSelected(i)}
              onClick={() => {
                onRun(c);
                onClose();
              }}
            >
              <span className="palette-cat">{c.category}</span>
              <span className="palette-title">{c.title}</span>
              {c.shortcut && <span className="palette-shortcut">{c.shortcut}</span>}
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
