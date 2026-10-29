import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

/**
 * Canvas media references.
 *
 * A node stores an image, video or audio as `artifact://<id>`; the bytes live in
 * the kernel's artifact store. Only this layer turns a reference into something
 * the webview can load, and the resolved address is never written back into the
 * graph — otherwise the reference would stop meaning anything.
 */

export const ARTIFACT_URI_PREFIX = "artifact://";

export type CanvasMediaKind = "image" | "video" | "audio" | "document";

interface CanvasArtifactDto {
  id: string;
  uri: string;
  workspaceId?: string | null;
  kind: string;
  mediaType?: string | null;
  byteSize: number;
  path: string;
}

export interface CanvasMediaSource {
  dataUrl?: string;
  localPath?: string;
  fileName?: string;
  mediaType?: string;
  workspaceId?: string;
}

export function isArtifactReference(value: string): boolean {
  return value.trim().startsWith(ARTIFACT_URI_PREFIX);
}

const displaySrcs = new Map<string, string>();
const inFlight = new Map<string, Promise<string>>();

/** Reference → an address an `<img>` can load. Non-artifacts pass through. */
export function resolveCanvasMediaSrc(reference: string, workspaceId: string | null = null): Promise<string> {
  const trimmed = reference.trim();
  if (!isArtifactReference(trimmed)) return Promise.resolve(trimmed);
  const scope = workspaceId?.trim() ?? null;
  const key = JSON.stringify([scope, trimmed]);
  const cached = displaySrcs.get(key);
  if (cached) return Promise.resolve(cached);
  const pending = inFlight.get(key);
  if (pending) return pending;

  const task = (async () => {
    const dto = await invoke<CanvasArtifactDto | null>("canvas_artifact_url", {
      reference: trimmed,
      workspaceId: scope,
    });
    if (!dto) throw new Error("artifact 制品在当前工作区不可用");
    const { convertFileSrc } = await import("@tauri-apps/api/core");
    const src = convertFileSrc(dto.path);
    displaySrcs.set(key, src);
    return src;
  })().finally(() => inFlight.delete(key));
  inFlight.set(key, task);
  return task;
}

/**
 * Store bytes as an artifact and hand back the reference the node should keep.
 *
 * The `dataUrl` is a transient carrier between the webview and the kernel; the
 * caller must persist only the returned URI.
 */
export async function importCanvasMedia(
  kind: CanvasMediaKind,
  source: CanvasMediaSource,
): Promise<string> {
  const dto = await invoke<CanvasArtifactDto>("canvas_artifact_import", {
    artifact: {
      kind,
      dataUrl: source.dataUrl ?? null,
      localPath: source.localPath ?? null,
      fileName: source.fileName ?? null,
      mediaType: source.mediaType ?? null,
      workspaceId: source.workspaceId ?? null,
    },
  });
  return dto.uri;
}

/**
 * Read a webview `File` and store its bytes.
 *
 * Callers that start from a File use this instead of handling the data URL
 * themselves, so an ephemeral `data:`/`blob:` value cannot reach node data.
 */
export async function importCanvasMediaFile(
  kind: CanvasMediaKind,
  file: File,
  workspaceId?: string,
): Promise<string> {
  const dataUrl = await new Promise<string>((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result));
    reader.onerror = () => reject(reader.error ?? new Error(`读取 ${file.name} 失败`));
    reader.readAsDataURL(file);
  });
  return importCanvasMedia(kind, {
    dataUrl,
    fileName: file.name,
    mediaType: file.type || undefined,
    workspaceId,
  });
}

/**
 * A one-off `data:` URL for the crop / annotation overlays, which need pixels.
 * Never store the result on a node.
 */
export async function loadCanvasMediaPixels(reference: string, workspaceId: string | null = null): Promise<string> {
  const resolved = await invoke<string | null>("canvas_artifact_data_url", {
    reference: reference.trim(),
    workspaceId: workspaceId?.trim() ?? null,
  });
  if (!resolved && isArtifactReference(reference)) throw new Error("artifact 制品在当前工作区不可用");
  // 旧图里的 asset:/http:/data: 引用后端不接管，原样交给浏览器解码，行为与迁移前一致。
  return resolved ?? reference;
}

/** Component hook: keep node data as the reference, render the resolved src. */
export function useCanvasMediaSrc(reference?: string, workspaceId: string | null = null): string | undefined {
  const key = JSON.stringify([workspaceId?.trim() ?? null, reference?.trim()]);
  const [resolved, setResolved] = useState<{ key: string; src?: string } | null>(null);

  useEffect(() => {
    if (!reference) {
      setResolved(null);
      return;
    }
    if (!isArtifactReference(reference)) {
      setResolved(null);
      return;
    }
    let active = true;
    void resolveCanvasMediaSrc(reference, workspaceId)
      .then((value) => {
        if (active) setResolved({ key, src: value });
      })
      .catch((error: unknown) => {
        console.error("[canvas] 解析媒体引用失败:", error);
        if (active) setResolved({ key });
      });
    return () => {
      active = false;
    };
  }, [reference, workspaceId, key]);

  // Effects run after render: never show the prior namespace's resolved path
  // even for one frame while a scope/reference change is waiting to load.
  if (!reference) return undefined;
  if (!isArtifactReference(reference)) return reference;
  return resolved?.key === key ? resolved.src : undefined;
}
