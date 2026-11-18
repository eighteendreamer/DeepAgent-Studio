import { getInvoke } from "./api";

export type TransferDirection = "upload" | "download";
export type ConflictPolicy = "ask" | "skip" | "replace" | "keep_both";
export type TransferKind = "file" | "directory";
export type DirectoryConflictPolicy = "ask" | "skip" | "merge" | "keep_both" | "replace";
export interface FileConflictDecision {
  policy: Exclude<ConflictPolicy, "ask">;
  applyToRemainingFiles: boolean;
}
export interface PendingFileConflict {
  version: number;
  id: string;
  attempt: number;
  conflict: {
    relativePath: string;
    destinationPath: string;
    direction: TransferDirection;
    sourceSize: string;
  };
}
export interface FileConflictResponse {
  version: number;
  id: string;
  attempt: number;
  decision: FileConflictDecision;
}
export interface PendingDirectoryConflict {
  version: number;
  id: string;
  attempt: number;
  conflict: {
    destinationPath: string;
    direction: TransferDirection;
    totalFiles: number;
    totalBytes: string;
  };
}
export interface DirectoryConflictResponse {
  version: number;
  id: string;
  attempt: number;
  decision: Exclude<DirectoryConflictPolicy, "ask">;
}
export interface TransferOptions {
  kind?: TransferKind;
  directoryConflict?: DirectoryConflictPolicy;
}
export interface TransferInput {
  direction: TransferDirection;
  localPath: string;
  remotePath: string;
  conflict?: ConflictPolicy;
}
export interface TransferTask {
  id: string;
  request: TransferOptions & { connectionId: string; transfer: TransferInput };
  attempt: number;
  state: "queued" | "running" | "cancelling" | "completed" | "failed" | "cancelled";
  bytes: string;
  total: string | null;
  currentFile?: string | null;
  totalFiles?: number | null;
  completedFiles: number;
  skippedFiles?: number;
  speedBps?: string | null;
  revision: number;
  updatedMs: number;
  pendingFileConflict?: PendingFileConflict | null;
  lastFileConflict?: { pending: PendingFileConflict; decision: FileConflictDecision } | null;
  pendingDirectoryConflict?: PendingDirectoryConflict | null;
  lastDirectoryConflict?: { pending: PendingDirectoryConflict; decision: DirectoryConflictResponse["decision"] } | null;
  error: { code: string; message: string; cleanupWarnings: string[]; relativePath?: string; destinationPath?: string } | null;
  directoryReceipt?: {
    bytes: string; totalBytes: string; totalFiles: number; completedFiles: number;
    skippedFiles: number; createdDirectories: number; cleanupWarnings: string[];
    destinationPath?: string; renamedFiles?: number;
    publishStrategy?: "directory_created" | "directory_backup_replace";
  } | null;
  receipt: {
    bytes: string;
    sha256: string;
    skipped: boolean;
    publishStrategy: string;
    cleanupWarnings: string[];
    destinationPath?: string;
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

export async function remoteTransferSubmit(connectionId: string, transfer: TransferInput, options: TransferOptions = {}): Promise<TransferTask> {
  const invoke = getInvoke();
  if (!invoke) throw Error("transfers require the desktop app");
  return invoke("remote_transfer_submit", {
    request: { connectionId, kind: options.kind ?? "file", directoryConflict: options.directoryConflict ?? "ask",
      transfer: { ...transfer, conflict: transfer.conflict ?? "ask" } },
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
export async function remoteTransferResolveFileConflict(id: string, response: FileConflictResponse): Promise<TransferTask> {
  const invoke = getInvoke();
  if (!invoke) throw Error("transfers require the desktop app");
  return invoke("remote_transfer_resolve_file_conflict", { id, response });
}
export async function remoteTransferResolveDirectoryConflict(id: string, response: DirectoryConflictResponse): Promise<TransferTask> {
  const invoke = getInvoke();
  if (!invoke) throw Error("transfers require the desktop app");
  return invoke("remote_transfer_resolve_directory_conflict", { id, response });
}
export async function remoteTransferRetry(id: string, conflict: ConflictPolicy = "ask", directoryConflict?: DirectoryConflictPolicy): Promise<TransferTask> {
  const invoke = getInvoke();
  if (!invoke) throw Error("transfers require the desktop app");
  return invoke("remote_transfer_retry", { id, conflict, directoryConflict: directoryConflict ?? null });
}
export async function remoteTransfersClear(): Promise<void> {
  const invoke = getInvoke();
  if (!invoke) throw Error("transfers require the desktop app");
  return invoke("remote_transfer_clear");
}
