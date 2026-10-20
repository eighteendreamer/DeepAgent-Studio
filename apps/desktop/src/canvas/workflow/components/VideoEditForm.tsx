import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { FileVideo, Play, Scissors, Upload, X } from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { Button } from "../../../components/shadcn/button";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "../../../components/shadcn/dialog";
import { Slider } from "../../../components/shadcn/slider";
import { message } from "../../../components/message";
import type { CreativeNodeData } from "../types";
import { importCanvasMedia, useCanvasMediaSrc } from "../utils/canvasMedia";
import { loadVideoEditProject, saveVideoEditProject } from "../utils/videoEditAdapters";
const clamp = (value: number, min: number, max: number) => Math.min(max, Math.max(min, value));
const formatTime = (time: number) => {
  const safe = Number.isFinite(time) ? Math.max(0, time) : 0;
  return `${Math.floor(safe / 60).toString().padStart(2, "0")}:${Math.floor(safe % 60).toString().padStart(2, "0")}.${Math.floor((safe % 1) * 10)}`;
};

export function VideoEditForm({ data, onUpdate }: {
  data: CreativeNodeData;
  onUpdate: (patch: Record<string, unknown>) => void;
}) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const editorVideoRef = useRef<HTMLVideoElement>(null);
  const src = useCanvasMediaSrc(data.videoEditInputRefs?.[0]);
  const [busy, setBusy] = useState(false);
  const [trimProgress, setTrimProgress] = useState(0);
  const [trimOperationId, setTrimOperationId] = useState<string>();
  const [editorOpen, setEditorOpen] = useState(false);
  const [duration, setDuration] = useState(data.videoDuration ?? 0);
  const [trimStart, setTrimStart] = useState(0);
  const [trimEnd, setTrimEnd] = useState(0);
  const [playhead, setPlayhead] = useState(0);
  const [projectReady, setProjectReady] = useState(false);

  useEffect(() => {
    if (!data.videoEditProjectId || !data.videoEditInputRefs?.[0]) {
      setProjectReady(true);
      return;
    }
    let active = true;
    void loadVideoEditProject(data.videoEditProjectId).then((project) => {
        if (!active) return;
        if (project && project.inputRef === data.videoEditInputRefs?.[0]) {
          setDuration(project.duration || data.videoDuration || 0);
          setTrimStart(project.trimStart || 0);
          setTrimEnd(project.trimEnd || project.duration || data.videoDuration || 0);
          onUpdate({ videoEditTrimStart: project.trimStart, videoEditTrimEnd: project.trimEnd });
        } else {
          setDuration(0);
          setTrimStart(0);
          setTrimEnd(0);
          onUpdate({
            videoEditTrimStart: 0,
            videoEditTrimEnd: 0,
            videoEditStatus: "dirty",
            videoEditOutputRef: undefined,
            videoEditInputFingerprint: undefined,
            videoUrl: undefined,
            result: undefined,
            status: "idle",
          });
        }
        setProjectReady(true);
      }).catch((error) => {
      console.error("[video-edit] 恢复工程失败:", error);
      if (active) setProjectReady(true);
    });
    return () => { active = false; };
  }, [data.videoEditProjectId, data.videoEditInputRefs?.[0], data.videoDuration, onUpdate]);

  useEffect(() => {
    const video = videoRef.current;
    if (!video) return;
    const update = () => {
      const nextDuration = Number.isFinite(video.duration) ? video.duration : 0;
      setDuration(nextDuration);
      setTrimEnd((current) => current > 0 ? Math.min(current, nextDuration) : nextDuration);
      onUpdate({ videoDuration: nextDuration });
    };
    video.addEventListener("loadedmetadata", update);
    return () => video.removeEventListener("loadedmetadata", update);
  }, [src, onUpdate]);

  useEffect(() => {
    const projectId = data.videoEditProjectId;
    const inputRef = data.videoEditInputRefs?.[0];
    if (!projectReady || !projectId || !inputRef || !duration) return;
    void saveVideoEditProject({
      id: projectId,
      inputRef,
      duration,
      trimStart,
      trimEnd: trimEnd || duration,
      updatedAt: Date.now(),
    }).catch((error) => message.error(`保存编辑工程失败：${String(error)}`));
  }, [projectReady, data.videoEditProjectId, data.videoEditInputRefs, duration, trimStart, trimEnd]);

  useEffect(() => {
    if (!projectReady || !data.videoEditOutputRef || !data.videoEditInputFingerprint) return;
    const current = `${data.videoEditInputRefs?.join("|") ?? ""}:${trimStart.toFixed(3)}:${trimEnd.toFixed(3)}`;
    if (current !== data.videoEditInputFingerprint) {
      onUpdate({
        videoEditStatus: "dirty",
        videoEditOutputRef: undefined,
        videoEditInputFingerprint: undefined,
        videoUrl: undefined,
        result: undefined,
        status: "idle",
      });
    }
  }, [projectReady, data.videoEditOutputRef, data.videoEditInputFingerprint, data.videoEditInputRefs, trimStart, trimEnd, onUpdate]);

  async function importVideo() {
    setBusy(true);
    try {
      const path = await open({
        multiple: false,
        title: "导入视频素材",
        filters: [{ name: "视频", extensions: ["mp4", "webm", "mov", "mkv"] }],
      });
      if (!path) return;
      const reference = await importCanvasMedia("video", { localPath: path });
      const projectId = crypto.randomUUID();
      setDuration(0);
      setTrimStart(0);
      setTrimEnd(0);
      setProjectReady(true);
      onUpdate({
        videoEditProjectId: projectId,
        videoEditInputRefs: [reference],
        videoEditTrimStart: 0,
        videoEditTrimEnd: 0,
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

  function changeRange(values: number[]) {
    const [start, end] = values;
    setTrimStart(start);
    setTrimEnd(end);
    onUpdate({
      videoEditStatus: "dirty",
      videoEditTrimStart: start,
      videoEditTrimEnd: end,
      videoEditOutputRef: undefined,
      videoEditInputFingerprint: undefined,
      videoUrl: undefined,
      result: undefined,
      status: "idle",
    });
  }

  async function exportSelection() {
    if (!data.videoEditInputRefs?.[0] || trimEnd <= trimStart) return;
    const operationId = crypto.randomUUID();
    setTrimOperationId(operationId);
    setTrimProgress(0);
    setBusy(true);
    let unlisten: (() => void) | undefined;
    try {
      unlisten = await listen<{ operationId: string; fraction: number }>("canvas-video-trim-progress", (event) => {
        if (event.payload.operationId === operationId) {
          setTrimProgress(Math.max(0, Math.min(1, event.payload.fraction)));
        }
      });
      const artifact = await invoke<{ uri: string }>("canvas_video_trim", {
        operationId,
        reference: data.videoEditInputRefs[0],
        startSeconds: trimStart,
        endSeconds: trimEnd,
      });
      const projectFingerprint = `${data.videoEditInputRefs?.join("|") ?? ""}:${trimStart.toFixed(3)}:${trimEnd.toFixed(3)}`;
      onUpdate({
        videoEditOutputRef: artifact.uri,
        videoEditTrimStart: trimStart,
        videoEditTrimEnd: trimEnd,
        videoEditInputFingerprint: projectFingerprint,
        videoEditStatus: "exported",
        videoUrl: artifact.uri,
        result: artifact.uri,
        status: "completed",
      });
      message.success("已导出为新视频素材");
    } catch (error) {
      const detail = String(error);
      if (detail.includes("已取消视频导出")) message.info("已取消视频导出");
      else message.error(`导出失败：${detail}`);
    } finally {
      unlisten?.();
      setTrimOperationId(undefined);
      setBusy(false);
    }
  }

  async function cancelExport() {
    if (!trimOperationId) return;
    try {
      await invoke("canvas_video_trim_cancel", { operationId: trimOperationId });
    } catch (error) {
      message.error(`取消视频导出失败：${String(error)}`);
    }
  }

  const validRange = duration > 0 && trimEnd > trimStart;
  return (
    <div className="space-y-3 text-xs text-white/75">
      <div className="flex items-center justify-between">
        <span className="flex items-center gap-2"><FileVideo className="h-4 w-4" />视频素材</span>
        <div className="flex gap-1.5">
          {src && <Button size="sm" variant="secondary" onClick={() => setEditorOpen(true)}><Scissors className="h-3.5 w-3.5" />打开编辑器</Button>}
          <Button size="sm" disabled={busy} onClick={() => void importVideo()}><Upload className="h-3.5 w-3.5" />{busy ? "处理中…" : "导入"}</Button>
        </div>
      </div>
      {src ? (
        <div className="overflow-hidden rounded-lg bg-black/30">
          <video ref={videoRef} src={src} controls className="max-h-36 w-full object-contain" />
          <div className="px-2.5 py-2 text-[10px] text-white/50">
            {data.videoEditOutputRef && data.videoEditStatus === "exported" ? "已有导出版本" : "素材已载入 · 编辑草稿自动保存"}
          </div>
        </div>
      ) : <p role="status" className="text-white/50">导入视频后可设置裁剪范围并导出为新素材。</p>}

      <Dialog open={editorOpen} onOpenChange={setEditorOpen}>
        <DialogContent className="w-[min(1080px,calc(100vw-48px))] max-w-none overflow-hidden border border-white/10 bg-[#151518] p-0 text-white shadow-2xl">
          <DialogHeader className="flex-row items-center justify-between border-b border-white/10 px-5 py-3">
            <DialogTitle className="text-sm font-medium">视频编辑</DialogTitle>
            <button type="button" aria-label="关闭" onClick={() => setEditorOpen(false)} className="rounded-md p-1.5 text-white/55 hover:bg-white/10 hover:text-white"><X className="h-4 w-4" /></button>
          </DialogHeader>
          <div className="grid min-h-[560px] grid-cols-[minmax(0,1fr)_280px]">
            <div className="flex min-w-0 flex-col bg-black/25 p-5">
              <div className="flex min-h-0 flex-1 items-center justify-center overflow-hidden rounded-lg bg-black">
                {src && <video ref={editorVideoRef} src={src} controls className="max-h-[430px] max-w-full" onTimeUpdate={(event) => setPlayhead(event.currentTarget.currentTime)} />}
              </div>
              <div className="mt-4 flex items-center gap-3 text-[11px] text-white/55">
                <span className="w-14 text-right tabular-nums">{formatTime(playhead)}</span>
                <Slider min={0} max={Math.max(duration, 0.1)} step={0.05} value={[playhead]} onValueChange={([value]) => {
                  setPlayhead(value);
                  if (editorVideoRef.current) editorVideoRef.current.currentTime = value;
                }} aria-label="播放位置" />
                <span className="w-14 tabular-nums">{formatTime(duration)}</span>
              </div>
              <div className="mt-5 rounded-lg border border-white/10 bg-white/[0.025] p-4">
                <div className="mb-3 flex items-center justify-between text-xs"><span className="font-medium text-white/80">视频轨道</span><span className="tabular-nums text-white/45">{formatTime(trimEnd - trimStart)} 时长</span></div>
                <div className="h-9 rounded-md border border-sky-300/15 bg-gradient-to-r from-sky-400/10 via-sky-300/20 to-sky-400/10" />
                <div className="mt-3 flex items-center gap-3 text-[10px] text-white/45">
                  <span>{formatTime(trimStart)}</span>
                  <Slider min={0} max={Math.max(duration, 0.1)} step={0.05} value={[trimStart, trimEnd]} onValueChange={changeRange} aria-label="裁剪范围" />
                  <span>{formatTime(trimEnd)}</span>
                </div>
              </div>
            </div>
            <aside className="border-l border-white/10 p-5">
              <h3 className="text-xs font-medium text-white/80">片段设置</h3>
              <p className="mt-1 text-[11px] leading-5 text-white/45">设置导出片段的起止时间。导出保留 MP4、MOV、WebM 或 MKV 容器，新素材会保存到素材库，源视频保持不变。</p>
              <div className="mt-5 space-y-4">
                <label className="block text-[11px] text-white/55">开始时间
                  <input className="mt-1.5 h-8 w-full rounded-md border border-white/10 bg-black/20 px-2 text-xs text-white" type="number" min={0} max={trimEnd} step={0.1} value={trimStart.toFixed(1)} onChange={(event) => changeRange([clamp(Number(event.target.value), 0, Math.max(0, trimEnd - 0.1)), trimEnd])} />
                </label>
                <label className="block text-[11px] text-white/55">结束时间
                  <input className="mt-1.5 h-8 w-full rounded-md border border-white/10 bg-black/20 px-2 text-xs text-white" type="number" min={trimStart} max={duration} step={0.1} value={trimEnd.toFixed(1)} onChange={(event) => changeRange([trimStart, clamp(Number(event.target.value), trimStart + 0.1, duration)])} />
                </label>
              </div>
              <div className="mt-6 rounded-md bg-white/[0.04] p-3 text-[10px] leading-5 text-white/45">
                {busy ? `正在处理视频 ${Math.round(trimProgress * 100)}%` : "使用应用管理的 FFmpeg，按原视频容器导出选中片段"}
              </div>
              {busy && <div className="mt-3 h-1.5 overflow-hidden rounded-full bg-white/10" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(trimProgress * 100)}><div className="h-full bg-sky-300 transition-[width] duration-150" style={{ width: `${Math.round(trimProgress * 100)}%` }} /></div>}
              <div className="mt-5 flex gap-2">
                <Button className="flex-1" disabled={busy || !validRange} onClick={() => void exportSelection()}>
                  {busy ? <><Play className="h-3.5 w-3.5 animate-pulse" />处理中…</> : <><Scissors className="h-3.5 w-3.5" />导出选中片段</>}
                </Button>
                {busy && <Button variant="secondary" aria-label="取消导出" title="取消导出" onClick={() => void cancelExport()}><X className="h-3.5 w-3.5" />取消</Button>}
              </div>
              <p className="mt-2 text-center text-[10px] text-white/35">需要先在设置的按需下载资源中安装 FFmpeg 与 FFprobe。</p>
            </aside>
          </div>
        </DialogContent>
      </Dialog>
    </div>
  );
}
