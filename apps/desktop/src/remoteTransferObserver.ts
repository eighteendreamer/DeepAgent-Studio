import { remoteTransfersWatch, type TransferSnapshot } from "./remoteTransfersApi";

interface Listener {
  next: (snapshot: TransferSnapshot) => void;
  error: (error: unknown) => void;
}

/** One bounded IPC wait per WebView; no task execution or second state store. */
export function createTransferObserver(read: typeof remoteTransfersWatch) {
  const listeners = new Set<Listener>();
  let latest: TransferSnapshot | undefined;
  let cursor: string | undefined;
  let running = false;
  let timer: ReturnType<typeof setTimeout> | undefined;

  async function load() {
    timer = undefined;
    running = true;
    let delay = 100;
    try {
      const snapshot = await read(cursor);
      const changed = snapshot.revision !== cursor;
      cursor = snapshot.revision;
      latest = snapshot;
      if (changed) for (const listener of listeners) listener.next(snapshot);
    } catch (error) {
      cursor = undefined;
      latest = undefined;
      delay = 1000;
      for (const listener of listeners) listener.error(error);
    } finally {
      running = false;
      // Throttle progress bursts, not the transfer itself. Slow IPC stays serial.
      if (listeners.size) timer = setTimeout(() => void load(), delay);
    }
  }

  return (listener: Listener) => {
    listeners.add(listener);
    if (latest) listener.next(latest);
    if (!running && timer === undefined) void load();
    return () => {
      listeners.delete(listener);
      if (!listeners.size && timer !== undefined) {
        clearTimeout(timer);
        timer = undefined;
      }
    };
  };
}

export const observeRemoteTransfers = createTransferObserver(remoteTransfersWatch);

/** Invalidate published parents, bounded by the current queue, once per attempt. */
export function createUploadInvalidator(connectionId: string, refresh: (paths: Set<string>) => void) {
  let seen = new Set<string>();
  return (snapshot: TransferSnapshot) => {
    const next = new Set<string>();
    const paths = new Set<string>();
    for (const task of snapshot.tasks) {
      if (task.request.connectionId !== connectionId || task.request.transfer.direction !== "upload"
        || task.state !== "completed" || !task.receipt || task.receipt.skipped) continue;
      const key = `${task.id}:${task.attempt}`;
      next.add(key);
      if (!seen.has(key)) {
        const path = task.request.transfer.remotePath;
        paths.add(path.slice(0, path.lastIndexOf("/")) || "/");
      }
    }
    seen = next;
    if (paths.size) refresh(paths);
  };
}
