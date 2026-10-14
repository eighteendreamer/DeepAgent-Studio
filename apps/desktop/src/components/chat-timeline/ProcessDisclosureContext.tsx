import { createContext, useCallback, useContext, useState, useSyncExternalStore } from "react";
import { ProcessDisclosureStore, type DisclosureSnapshot } from "./processDisclosure";

export const ProcessStoreContext = createContext<ProcessDisclosureStore | null>(null);
export const ProcessDetailsContext = createContext<{
  snapshot: DisclosureSnapshot;
  setDetail: (id: string, open: boolean) => void;
} | null>(null);

export function useTurnDisclosure(key: string, epoch: string, defaultOpen: boolean) {
  const shared = useContext(ProcessStoreContext);
  const [local] = useState(() => new ProcessDisclosureStore());
  const store = shared ?? local;
  const subscribe = useCallback((listener: () => void) => store.subscribe(key, listener), [store, key]);
  const getSnapshot = useCallback(() => store.read(key, epoch, defaultOpen), [store, key, epoch, defaultOpen]);
  const snapshot = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
  const setOpen = useCallback((open: boolean) => store.setOpen(key, open), [store, key]);
  const setDetail = useCallback((id: string, open: boolean) => store.setDetail(key, id, open), [store, key]);
  return { snapshot, setOpen, setDetail };
}

export function useProcessDetail(id: string, defaultOpen = false): [boolean, () => void] {
  const context = useContext(ProcessDetailsContext);
  const [localOpen, setLocalOpen] = useState(defaultOpen);
  const open = context
    ? context.snapshot.details[id] ?? (context.snapshot.detailsReset ? false : defaultOpen)
    : localOpen;
  const toggle = () => context ? context.setDetail(id, !open) : setLocalOpen(!open);
  return [open, toggle];
}
