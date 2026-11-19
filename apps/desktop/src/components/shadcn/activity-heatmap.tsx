import { useEffect, useId, useMemo, useRef, useState, type CSSProperties, type KeyboardEvent } from "react";
import { motion, useInView, useReducedMotion } from "framer-motion";
import { cn } from "./utils";

export interface ActivityDay { date: string; count: number }
interface ActivityHeatmapProps {
  days: readonly ActivityDay[];
  year: number;
  today: string;
  locale: string;
  label: string;
  selectedDate: string | null;
  onSelectDate: (date: string) => void;
  formatValue: (value: number) => string;
  labels: { less: string; more: string; chooseDay: string; future: string;
    level: (level: number, days: number) => string };
}

const DAY_MS = 86_400_000;

/** Activity calendar with one shared tooltip and a single keyboard tab stop. */
export function ActivityHeatmap({ days, year, today, locale, label, selectedDate,
  onSelectDate, formatValue, labels }: ActivityHeatmapProps) {
  const rootRef = useRef<HTMLDivElement>(null);
  const gridRef = useRef<HTMLDivElement>(null);
  const inView = useInView(rootRef, { once: true, amount: .25 });
  const reduced = useReducedMotion();
  const tooltipId = useId();
  const [tip, setTip] = useState<{ date: string; count: number; x: number; y: number } | null>(null);
  const [preview, setPreview] = useState<number | null>(null);
  const [pinned, setPinned] = useState<number | null>(null);
  const [focused, setFocused] = useState<string | null>(null);
  const highlight = preview ?? pinned;
  const dates = useMemo(() => new Intl.DateTimeFormat(locale,
    { year: "numeric", month: "long", day: "numeric", weekday: "short", timeZone: "UTC" }), [locale]);
  const model = useMemo(() => {
    const start = Date.UTC(year, 0, 1), end = Date.UTC(year + 1, 0, 1);
    const length = Math.round((end - start) / DAY_MS);
    const lead = (new Date(start).getUTCDay() + 6) % 7;
    const weeks = Math.ceil((length + lead) / 7);
    const byDate = new Map(days.map(day => [day.date, Math.max(0, day.count)]));
    const max = Math.max(1, ...byDate.values());
    const counts = [0, 0, 0, 0, 0];
    const cells = Array.from({ length: weeks * 7 }, (_, index) => {
      const day = index - lead;
      const date = new Date(start + day * DAY_MS).toISOString().slice(0, 10);
      const count = byDate.get(date) ?? 0;
      const level = count === 0 ? 0 : Math.min(4, Math.ceil(count / max * 4));
      const empty = day < 0 || day >= length;
      const future = date > today;
      if (!empty && !future) counts[level]++;
      return { date, count, level, empty, future, column: Math.floor(index / 7), row: index % 7 };
    });
    const months = Array.from({ length: 12 }, (_, month) => ({
      column: Math.floor(((Date.UTC(year, month, 1) - start) / DAY_MS + lead) / 7) + 1,
      label: new Intl.DateTimeFormat(locale, { month: "short", timeZone: "UTC" }).format(Date.UTC(year, month, 1)),
    }));
    return { cells, weeks, counts, months };
  }, [days, year, today, locale]);
  const lastDate = year === Number(today.slice(0, 4)) ? today : `${year}-12-31`;
  const tabDate = focused ?? selectedDate ?? lastDate;
  const selected = model.cells.find(cell => cell.date === selectedDate && !cell.empty);
  useEffect(() => { setTip(null); setPinned(null); setPreview(null); setFocused(null); }, [year, days]);

  function showTip(button: HTMLButtonElement, cell: (typeof model.cells)[number]) {
    const root = rootRef.current?.getBoundingClientRect();
    if (!root) return;
    const box = button.getBoundingClientRect();
    setTip({ date: cell.date, count: cell.count,
      x: Math.max(95, Math.min(root.width - 95, box.left - root.left + box.width / 2)),
      y: box.top - root.top - 9 });
  }

  function onKeyDown(event: KeyboardEvent<HTMLButtonElement>, date: string) {
    const offsets: Record<string, number> = { ArrowLeft: -7, ArrowRight: 7, ArrowUp: -1, ArrowDown: 1 };
    if (!(event.key in offsets)) return;
    event.preventDefault();
    const next = new Date(Date.parse(`${date}T00:00:00Z`) + offsets[event.key] * DAY_MS).toISOString().slice(0, 10);
    gridRef.current?.querySelector<HTMLButtonElement>(`button[data-date="${next}"]:not(:disabled)`)?.focus();
  }

  return <div ref={rootRef} className="activity-heatmap">
    <div className="activity-heatmap-scroll">
      <div className="activity-heatmap-canvas" style={{ "--activity-weeks": model.weeks } as CSSProperties}>
        <div className="activity-heatmap-months" aria-hidden="true">
          {model.months.map(month => <span key={month.column} style={{ gridColumn: month.column }}>{month.label}</span>)}
        </div>
        <div className="activity-heatmap-weekdays" aria-hidden="true">
          {Array.from({ length: 7 }, (_, day) => <span key={day}>{day % 2 === 0
            ? new Intl.DateTimeFormat(locale, { weekday: "narrow", timeZone: "UTC" }).format(Date.UTC(2024, 0, day + 1)) : ""}</span>)}
        </div>
        <div ref={gridRef} className="activity-heatmap-grid" role="group" aria-label={label}
          onPointerLeave={() => setTip(null)}>
          {model.cells.map(cell => cell.empty ? <span key={cell.date} className="activity-heatmap-cell empty" />
            : <button key={cell.date} type="button" data-date={cell.date} data-level={cell.level}
                disabled={cell.future} tabIndex={cell.date === tabDate ? 0 : -1}
                aria-label={`${dates.format(Date.parse(`${cell.date}T00:00:00Z`))}, ${cell.future ? labels.future : `${formatValue(cell.count)} Token`}`}
                aria-pressed={cell.date === selectedDate} aria-describedby={tip?.date === cell.date ? tooltipId : undefined}
                className={cn("activity-heatmap-cell", cell.future && "future",
                  inView && !reduced && "reveal", highlight !== null && highlight !== cell.level && "muted")}
                style={{ "--activity-wave": `${(cell.column + cell.row) * 6}ms` } as CSSProperties}
                onPointerEnter={event => showTip(event.currentTarget, cell)}
                onFocus={event => { setFocused(cell.date); showTip(event.currentTarget, cell); }}
                onBlur={() => setTip(null)} onKeyDown={event => onKeyDown(event, cell.date)}
                onClick={() => onSelectDate(cell.date)} />)}
        </div>
      </div>
    </div>
    <div className="activity-heatmap-footer">
      <span aria-live="polite">{highlight !== null ? labels.level(highlight, model.counts[highlight])
        : selected ? `${dates.format(Date.parse(`${selected.date}T00:00:00Z`))} · ${formatValue(selected.count)} Token` : labels.chooseDay}</span>
      <div className="activity-heatmap-legend" aria-label={`${labels.less} — ${labels.more}`}>
        <span>{labels.less}</span>
        {[0, 1, 2, 3, 4].map(level => <button key={level} type="button" data-level={level}
          className="activity-heatmap-swatch" aria-pressed={pinned === level}
          aria-label={labels.level(level, model.counts[level])}
          onPointerEnter={() => setPreview(level)} onPointerLeave={() => setPreview(null)}
          onFocus={() => setPreview(level)} onBlur={() => setPreview(null)}
          onClick={() => setPinned(current => current === level ? null : level)} />)}
        <span>{labels.more}</span>
      </div>
    </div>
    {tip && <motion.div id={tooltipId} role="tooltip" className="activity-heatmap-tooltip"
      initial={false} animate={{ left: tip.x, top: tip.y }}
      transition={reduced ? { duration: 0 } : { type: "spring", stiffness: 450, damping: 35 }}>
      <strong key={`${tip.date}-value`}>{formatValue(tip.count)} Token</strong>
      <span>{dates.format(Date.parse(`${tip.date}T00:00:00Z`))}</span>
    </motion.div>}
  </div>;
}
