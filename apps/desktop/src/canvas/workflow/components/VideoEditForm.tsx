import { useState } from "react";
import { FileVideo, Upload } from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { Button } from "../../../components/shadcn/button";
import { message } from "../../../components/message";
import type { CreativeNodeData } from "../types";
import { importCanvasMedia } from "../utils/canvasMedia";

export function VideoEditForm({ data, onUpdate }: {
  data: CreativeNodeData;
  onUpdate: (patch: Record<string, unknown>) => void;
}) {
  const [busy, setBusy] = useState(false);

  async function importVideo() {
    setBusy(true);
    try {
      const path = await open({
        multiple: false,
        title: "导入视频素材",
        filters: [{ name: "视频", extensions: ["mp4", "webm", "mov", "mkv"] }],
      });
      if (!path) return;
      // Only the path crosses IPC; the artifact service streams the file.
      const reference = await importCanvasMedia("video", { localPath: path });
      onUpdate({
        videoEditProjectId: data.videoEditProjectId ?? crypto.randomUUID(),
        videoEditInputRefs: [reference],
        videoEditStatus: "dirty",
        videoEditOutputRef: undefined,
        videoEditInputFingerprint: undefined,
        videoUrl: undefined,
        result: undefined,
        status: "idle",
      });
    } catch (error) {
      message.error(`导入视频失败：${String(error)}`);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="space-y-3 text-xs text-white/75">
      <div className="flex items-center justify-between">
        <span className="flex items-center gap-2">
          <FileVideo className="h-4 w-4" />视频素材
        </span>
        <Button size="sm" disabled={busy} onClick={() => void importVideo()}>
          <Upload className="h-3.5 w-3.5" />{busy ? "导入中…" : "导入"}
        </Button>
      </div>
      <p role="status" className="text-white/50">
        时间线编辑器尚未接通，当前仅支持保存素材引用。
      </p>
    </div>
  );
}
