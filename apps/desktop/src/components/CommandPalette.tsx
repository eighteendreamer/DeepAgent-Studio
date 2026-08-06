import { useEffect, useRef, useState } from "react";
import { getCommands } from "../api";
import type { Command } from "../types";
import { useTranslation } from "react-i18next";
import { Panel } from "./ui/Panel";
import { ListItem } from "./ui/ListItem";

interface Props {
  open: boolean;
  onClose: () => void;
  onRun: (command: Command) => void;
}

export function CommandPalette({ open, onClose, onRun }: Props) {
  const { t } = useTranslation();
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
    <div
      className="fixed inset-0 z-[80] flex items-start justify-center bg-black/20 px-4 pt-[12vh]"
      onClick={onClose}
    >
      <Panel
        menu={false}
        className="w-full max-w-xl overflow-hidden rounded-2xl"
        onClick={(e) => e.stopPropagation()}
        onKeyDown={onKeyDown}
      >
        <input
          ref={inputRef}
          className="w-full border-b border-border-theme bg-elevated-bg px-4 py-3 text-sm text-text-base placeholder:text-text-secondary"
          placeholder={t("commandPalette.placeholder")}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <div className="max-h-[420px] overflow-y-auto py-1">
          {results.length === 0 && (
            <div className="px-4 py-6 text-center text-sm text-text-secondary">
              {t("commandPalette.noMatch")}
            </div>
          )}
          {results.map((c, i) => (
            <ListItem
              key={c.id}
              selected={i === selected}
              className="grid cursor-pointer grid-cols-[96px_minmax(0,1fr)_auto] items-stretch gap-x-3 gap-y-1 px-4 py-2.5 text-left"
              onMouseEnter={() => setSelected(i)}
              onClick={() => {
                onRun(c);
                onClose();
              }}
            >
              <span className="truncate text-[11px] font-medium uppercase tracking-wide text-text-secondary">
                {c.category}
              </span>
              <span className="truncate text-sm font-medium text-text-base">{c.title}</span>
              {c.shortcut && (
                <span className="rounded border border-border-theme px-1.5 py-0.5 text-[11px] text-text-secondary">
                  {c.shortcut}
                </span>
              )}
              <span className="col-start-2 col-end-4 line-clamp-2 text-xs leading-snug text-text-secondary">
                {c.description}
              </span>
            </ListItem>
          ))}
        </div>
      </Panel>
    </div>
  );
}
