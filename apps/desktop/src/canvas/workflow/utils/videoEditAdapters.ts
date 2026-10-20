import { invoke } from "@tauri-apps/api/core";
import {
  readCanvasPreference,
  whenCanvasPreferencesReady,
  writeCanvasPreferenceAndWait,
} from "./canvasPreferences";

const PROJECTS_KEY = "video-edit-projects" as const;
const UPLOAD_CHUNK_BYTES = 256 * 1024;
let projectWriteQueue: Promise<void> = Promise.resolve();

export interface VideoEditProject {
  id: string;
  inputRef: string;
  trimStart: number;
  trimEnd: number;
  duration: number;
  updatedAt: number;
}

interface CanvasArtifactDto {
  uri: string;
}

async function readProjects(): Promise<Record<string, VideoEditProject>> {
  await whenCanvasPreferencesReady();
  return readCanvasPreference<Record<string, VideoEditProject>>(PROJECTS_KEY, {});
}

export function saveVideoEditProject(project: VideoEditProject): Promise<void> {
  let persisted: Promise<void>;
  const pending = projectWriteQueue.catch(() => undefined).then(async () => {
    const projects = await readProjects();
    persisted = writeCanvasPreferenceAndWait(PROJECTS_KEY, {
      ...projects,
      [project.id]: { ...project, updatedAt: Date.now() },
    });
  });
  projectWriteQueue = pending;
  return pending.then(() => persisted);
}

export async function loadVideoEditProject(id: string): Promise<VideoEditProject | null> {
  return (await readProjects())[id] ?? null;
}

/** Upload encoded MediaRecorder chunks directly; the whole export stays off the JS heap. */
export async function uploadRecordedVideo(
  chunks: AsyncIterable<Blob>,
  mime: string,
  name: string,
): Promise<string> {
  const id = await invoke<string>("canvas_artifact_upload", {
    action: { action: "begin_streaming", kind: "video", mime, name },
  });
  let offset = 0;
  try {
    for await (const blob of chunks) {
      for (let cursor = 0; cursor < blob.size; cursor += UPLOAD_CHUNK_BYTES) {
        const chunk = new Uint8Array(
          await blob.slice(cursor, Math.min(cursor + UPLOAD_CHUNK_BYTES, blob.size)).arrayBuffer(),
        );
        await invoke("canvas_artifact_upload", {
          action: { action: "write", id, offset, bytes: Array.from(chunk) },
        });
        offset += chunk.length;
      }
    }
    if (offset === 0) throw new Error("编码结果为空");
    const artifact = await invoke<CanvasArtifactDto>("canvas_artifact_upload", {
      action: { action: "finish", id },
    });
    return artifact.uri;
  } catch (error) {
    await invoke("canvas_artifact_upload", { action: { action: "abort", id } }).catch(
      () => undefined,
    );
    throw error;
  }
}
