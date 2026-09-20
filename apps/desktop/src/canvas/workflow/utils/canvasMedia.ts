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
export function resolveCanvasMediaSrc(reference: string): Promise<string> {
  const trimmed = reference.trim();
  if (!isArtifactReference(trimmed)) return Promise.resolve(trimmed);
  const cached = displaySrcs.get(trimmed);
  if (cached) return Promise.resolve(cached);
  const pending = inFlight.get(trimmed);
  if (pending) return pending;

  const task = (async () => {
    const dto = await invoke<CanvasArtifactDto | null>("canvas_artifact_url", {
      reference: trimmed,
    });
    if (!dto) return trimmed;
    const { convertFileSrc } = await import("@tauri-apps/api/core");
    const src = convertFileSrc(dto.path);
    displaySrcs.set(trimmed, src);
    return src;
  })().finally(() => inFlight.delete(trimmed));
  inFlight.set(trimmed, task);
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
 * A one-off `data:` URL for the crop / annotation overlays, which need pixels.
 * Never store the result on a node.
 */
export async function loadCanvasMediaPixels(reference: string): Promise<string> {
  const resolved = await invoke<string | null>("canvas_artifact_data_url", {
    reference: reference.trim(),
  });
  // 旧图里的 asset:/http:/data: 引用后端不接管，原样交给浏览器解码，行为与迁移前一致。
  return resolved ?? reference;
}

/** Component hook: keep node data as the reference, render the resolved src. */
export function useCanvasMediaSrc(reference?: string): string | undefined {
  const [src, setSrc] = useState<string | undefined>(() =>
    reference && !isArtifactReference(reference) ? reference : undefined,
  );

  useEffect(() => {
    if (!reference) {
      setSrc(undefined);
      return;
    }
    if (!isArtifactReference(reference)) {
      setSrc(reference);
      return;
    }
    let active = true;
    void resolveCanvasMediaSrc(reference)
      .then((value) => {
        if (active) setSrc(value);
      })
      .catch((error: unknown) => {
        console.error("[canvas] 解析媒体引用失败:", error);
        if (active) setSrc(undefined);
      });
    return () => {
      active = false;
    };
  }, [reference]);

  return src;
}
