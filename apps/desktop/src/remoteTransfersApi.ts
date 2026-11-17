import { getInvoke } from "./api";

export type TransferDirection = "upload" | "download";
export type ConflictPolicy = "ask" | "skip" | "replace";
export interface TransferInput {
  direction: TransferDirection;
  localPath: string;
  remotePath: string;
  conflict?: ConflictPolicy;
}
export interface TransferTask {
  id: string;
  request: { connectionId: string; transfer: TransferInput };
  attempt: number;
  state: "queued" | "running" | "cancelling" | "completed" | "failed" | "cancelled";
  bytes: string;
  total: string | null;
  revision: number;
  updatedMs: number;
  error: { code: string; message: string; cleanupWarnings: string[] } | null;
  receipt: {
    bytes: string;
    sha256: string;
    skipped: boolean;
    publishStrategy: string;
    cleanupWarnings: string[];
  } | null;
}

export interface TransferSnapshot {
  /** Queue-local readiness cursor, kept exact across the Rust/JavaScript boundary. */
  revision: string;
  tasks: TransferTask[];
}

export async function remoteTransfersWatch(afterRevision?: string): Promise<TransferSnapshot> {
  const invoke = getInvoke();
  if (!invoke) throw Error("transfers require the desktop app");
  return invoke("remote_transfer_watch", { afterRevision: afterRevision ?? null });
}

export async function remoteTransferSubmit(connectionId: string, transfer: TransferInput): Promise<TransferTask> {
  const invoke = getInvoke();
  if (!invoke) throw Error("transfers require the desktop app");
  return invoke("remote_transfer_submit", {
    request: { connectionId, transfer: { ...transfer, conflict: transfer.conflict ?? "ask" } },
  });
}
export async function remoteTransfersSnapshot(): Promise<TransferTask[]> {
  const invoke = getInvoke();
  if (!invoke) throw Error("transfers require the desktop app");
  return invoke("remote_transfer_snapshot");
}
export async function remoteTransferCancel(id: string): Promise<TransferTask> {
  const invoke = getInvoke();
  if (!invoke) throw Error("transfers require the desktop app");
  return invoke("remote_transfer_cancel", { id });
}
export async function remoteTransferRetry(id: string, conflict: ConflictPolicy = "ask"): Promise<TransferTask> {
  const invoke = getInvoke();
  if (!invoke) throw Error("transfers require the desktop app");
  return invoke("remote_transfer_retry", { id, conflict });
}
export async function remoteTransfersClear(): Promise<void> {
  const invoke = getInvoke();
  if (!invoke) throw Error("transfers require the desktop app");
  return invoke("remote_transfer_clear");
}
