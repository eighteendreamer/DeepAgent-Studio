/** View state only. Run outcomes remain owned by the existing runtime/event log. */
export type ProcessPhase = "working" | "answering" | "settled";

export interface TurnActivity {
  id: string;
  phase: ProcessPhase;
  generation: number;
  finalAnswer?: string;
}

export function advanceTurnActivity(
  activity: TurnActivity | undefined,
  phase: ProcessPhase,
  reset = false,
): TurnActivity | undefined {
  if (!activity || activity.phase === "settled" || (!reset && activity.phase === phase)) return activity;
  return { ...activity, phase, generation: activity.generation + 1 };
}

export interface DisclosureSnapshot {
  epoch: string;
  open: boolean;
  detailsReset: boolean;
  details: Readonly<Record<string, boolean>>;
}

/** Owned by App, outside both session frames and virtualized rows. */
export class ProcessDisclosureStore {
  private entries = new Map<string, DisclosureSnapshot>();
  private listeners = new Map<string, Set<() => void>>();

  read(key: string, epoch: string, defaultOpen: boolean): DisclosureSnapshot {
    const previous = this.entries.get(key);
    if (previous?.epoch === epoch) return previous;
    // A subscription from the previous render must not rewind a newer phase.
    if (previous && (previous.epoch === "settled" ||
      (epoch !== "settled" && Number.parseInt(previous.epoch, 10) > Number.parseInt(epoch, 10)))) return previous;
    const snapshot = { epoch, open: defaultOpen, detailsReset: !defaultOpen, details: {} };
    this.entries.set(key, snapshot);
    return snapshot;
  }

  subscribe(key: string, listener: () => void): () => void {
    const listeners = this.listeners.get(key) ?? new Set();
    listeners.add(listener);
    this.listeners.set(key, listeners);
    return () => {
      listeners.delete(listener);
      if (!listeners.size) this.listeners.delete(key);
    };
  }

  setOpen(key: string, open: boolean): void {
    const previous = this.entries.get(key);
    if (!previous || previous.open === open) return;
    this.publish(key, {
      ...previous,
      open,
      detailsReset: previous.detailsReset || !open,
      details: open ? previous.details : {},
    });
  }

  setDetail(key: string, id: string, open: boolean): void {
    const previous = this.entries.get(key);
    if (!previous || previous.details[id] === open) return;
    this.publish(key, { ...previous, details: { ...previous.details, [id]: open } });
  }

  private publish(key: string, snapshot: DisclosureSnapshot): void {
    this.entries.set(key, snapshot);
    this.listeners.get(key)?.forEach((listener) => listener());
  }
}
