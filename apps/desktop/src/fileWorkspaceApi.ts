import { getInvoke } from "./api";
import type { FileEndpoint, FilePanelListing } from "./components/remote/filePanelModel";

export interface ChmodRequest { version: number; connectionId: string; connectionGeneration: string; path: string; mode: number; recursive: boolean; fingerprint: string }
export interface ChmodPreview extends ChmodRequest { totalEntries: number; totalFiles: number; totalDirectories: number; totalSymlinks: number }
export interface CopySpec { sourceConnectionId: string; destinationConnectionId: string; sourcePath: string; destinationPath: string; conflict: "ask" | "skip" | "replace" | "keep_both"; method: "relay" | "scp" | "rsync" }
export interface CopyRequest { version: number; spec: CopySpec; sourceGeneration: string; destinationGeneration: string; fingerprint: string }
export interface CopyPreview extends CopyRequest { totalFiles: number; totalBytes: string; conflicts: number; entries: { relativePath: string }[] }
export interface ArchiveSpec { connectionId: string; archivePath: string; destinationPath: string; conflict: "ask" | "skip" | "replace" }
export interface ArchiveRequest { version: number; spec: ArchiveSpec; connectionGeneration: string; fingerprint: string }
export interface ArchivePreview extends ArchiveRequest { details: { format: string; totalBytes: string; conflicts: number; maxEntries: number; maxBytes: string; entries: { path: string; directory: boolean; sizeBytes: string }[] } }
export interface DirectProbe { scp: boolean; rsync: boolean; authenticated: boolean; destinationFingerprint: string | null; reason: string | null }
export interface EditorSession { id: string; connectionId: string; remotePath: string; localPath: string; autoUpload: boolean; state: string; error: string | null; taskId: string | null }
export type WorkbenchOperation = { operation: "chmod"; request: ChmodRequest } | { operation: "copy"; request: CopyRequest } | { operation: "extract"; request: ArchiveRequest };
export function fileCommand<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const invoke = getInvoke(); if (!invoke) return Promise.reject(new Error("File management requires the desktop app"));
  return invoke<T>(command, args);
}
export const previewChmod = (connectionId: string, path: string, mode: number, recursive: boolean) => fileCommand<ChmodPreview>("file_chmod_preview", { connectionId, path, mode, recursive });
export const previewCopy = (spec: CopySpec) => fileCommand<CopyPreview>("file_copy_preview", { spec });
export const probeCopy = (sourceId: string, destinationId: string) => fileCommand<DirectProbe>("file_copy_probe", { sourceId, destinationId });
export const previewArchive = (spec: ArchiveSpec) => fileCommand<ArchivePreview>("file_archive_preview", { spec });
export const submitOperation = (operation: WorkbenchOperation) => fileCommand("file_operation_submit", { operation });

export async function filePanelList(endpoint: FileEndpoint, path: string): Promise<FilePanelListing> {
  const invoke = getInvoke();
  if (!invoke) throw new Error("File management requires the desktop app");
  return invoke("file_panel_list", { endpoint, path });
}
