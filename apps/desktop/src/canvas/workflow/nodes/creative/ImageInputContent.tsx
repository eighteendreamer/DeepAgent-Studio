import { useRef } from "react";
import { ImagePlus, RefreshCw, Upload } from "lucide-react";
import type { CreativeNodeData } from "../../types";
import { useCreativeStore } from "../../store/creativeStore";
import { importCanvasMedia, useCanvasMediaSrc } from "../../utils/canvasMedia";

interface Props {
  id: string;
  data: CreativeNodeData;
}

function stopNodeGesture(event: React.SyntheticEvent) {
  event.stopPropagation();
}

function readFileAsDataUrl(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result ?? ""));
    reader.onerror = () => reject(reader.error ?? new Error("读取图片失败"));
    reader.readAsDataURL(file);
  });
}

export function ImageInputContent({ id, data }: Props) {
  const inputRef = useRef<HTMLInputElement | null>(null);
  const imageUrl = data.imageUrl ?? "";
  const src = useCanvasMediaSrc(imageUrl);
  const sourceFileName = typeof data.sourceFileName === "string" ? data.sourceFileName : "";

  return (
    <div className="flex flex-col gap-2">
      <input
        ref={inputRef}
        type="file"
        accept="image/*"
        className="hidden"
        onChange={async (event) => {
          const file = event.currentTarget.files?.[0];
          event.currentTarget.value = "";
          if (!file) return;
          const url = await importCanvasMedia("image", {
            dataUrl: await readFileAsDataUrl(file),
            fileName: file.name,
          });
          useCreativeStore.getState().updateNodeData(id, {
            imageUrl: url,
            mediaUrl: url,
            mediaType: "image",
            sourceFileName: file.name,
            label: "图片输入",
          });
        }}
      />

      {src ? (
        <div className="relative overflow-hidden rounded-lg" style={{ height: 124, background: "rgba(255,255,255,0.035)" }}>
          <img
            src={src}
            alt={sourceFileName || "输入图片"}
            draggable={false}
            onDragStart={(event) => event.preventDefault()}
            className="pointer-events-none h-full w-full select-none object-cover"
          />
          <button
            type="button"
            title="替换图片"
            onPointerDown={stopNodeGesture}
            onMouseDown={stopNodeGesture}
            onClick={(event) => {
              stopNodeGesture(event);
              inputRef.current?.click();
            }}
            className="absolute right-2 top-2 flex h-7 w-7 items-center justify-center rounded-md border border-white/10 bg-black/60 text-white/75 backdrop-blur transition hover:bg-black/80 hover:text-white"
          >
            <RefreshCw className="h-3.5 w-3.5" />
          </button>
        </div>
      ) : (
        <button
          type="button"
          onPointerDown={stopNodeGesture}
          onMouseDown={stopNodeGesture}
          onClick={(event) => {
            stopNodeGesture(event);
            inputRef.current?.click();
          }}
          className="flex h-[124px] flex-col items-center justify-center gap-2 rounded-lg border border-dashed border-white/10 bg-white/[0.025] text-white/45 transition hover:border-white/20 hover:bg-white/[0.05] hover:text-white/75"
        >
          <ImagePlus className="h-5 w-5" />
          <span className="text-[11px]">上传图片</span>
        </button>
      )}

      <div className="flex min-w-0 items-center gap-1.5 text-[10px] text-white/40">
        <Upload className="h-3 w-3 shrink-0" />
        <span className="truncate">{sourceFileName || "支持 JPG、PNG、WebP"}</span>
      </div>
    </div>
  );
}
