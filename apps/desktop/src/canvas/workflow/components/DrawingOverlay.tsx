import { useCallback, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { loadCanvasMediaPixels } from "../utils/canvasMedia";
import {
  Check,
  X,
  Square,
  Type,
  Hash,
  ArrowUpRight,
  Paintbrush,
  Eraser,
  Undo2,
  Redo2,
  Trash2,
  Pencil,
} from "lucide-react";
import { Slider } from "../../../components/shadcn/slider";

type DrawMode = "annotate" | "erase";
type AnnotateTool = "rect" | "arrow" | "text" | "marker" | "brush";

interface Props {
  imageUrl: string;
  itemName: string;
  mode: DrawMode;
  onConfirm: (dataUrl: string) => void;
  onCancel: () => void;
}

interface Point {
  x: number;
  y: number;
}

type ShapeObject = {
  id: string;
  type: "brush" | "rect" | "arrow" | "marker";
  points: Point[];
  color: string;
  size: number;
  markerNum?: number;
  erase?: boolean;
};

type TextObject = {
  id: string;
  type: "text";
  x: number;
  y: number;
  width: number;
  height: number;
  text: string;
  color: string;
  size: number;
};

type DrawObject = ShapeObject | TextObject;

const COLORS = [
  { label: "红", solid: "#ef4444" },
  { label: "蓝", solid: "#3b82f6" },
  { label: "绿", solid: "#22c55e" },
  { label: "黄", solid: "#eab308" },
  { label: "白", solid: "#ffffff" },
];

const ANNOTATE_TOOLS = [
  { tool: "brush", icon: Paintbrush, label: "画笔" },
  { tool: "rect", icon: Square, label: "矩形" },
  { tool: "arrow", icon: ArrowUpRight, label: "箭头" },
  { tool: "text", icon: Type, label: "文字" },
  { tool: "marker", icon: Hash, label: "序号" },
] as const;

const ERASE_TOOLS = [
  { tool: "brush", icon: Eraser, label: "擦除画笔" },
] as const;

const DEFAULT_TEXT_WIDTH = 240;
const MAX_TEXT_WIDTH = 720;
const DEFAULT_TEXT_HEIGHT = 56;
const MIN_FONT_SIZE = 16;
const MAX_FONT_SIZE = 120;
const DEFAULT_TEXT_SIZE = 32;

function getTextWidth(fontSize: number): number {
  return Math.min(DEFAULT_TEXT_WIDTH * (fontSize / DEFAULT_TEXT_SIZE), MAX_TEXT_WIDTH);
}

function measureTextHeight(text: string, width: number, fontSize: number): number {
  const lineHeight = fontSize * 1.25;
  const baseHeight = DEFAULT_TEXT_HEIGHT * (fontSize / DEFAULT_TEXT_SIZE);
  const canvas = document.createElement("canvas");
  const ctx = canvas.getContext("2d");
  if (!ctx) return Math.max(baseHeight, lineHeight + 8);
  ctx.font = `${fontSize}px sans-serif`;

  const shouldWrap = width >= MAX_TEXT_WIDTH;
  let lines = 0;
  for (const paragraph of text.split("\n")) {
    if (!shouldWrap || paragraph.length === 0) {
      lines += 1;
      continue;
    }

    let line = "";
    for (const ch of paragraph) {
      const nextLine = line + ch;
      if (line && ctx.measureText(nextLine).width > width) {
        lines += 1;
        line = ch;
      } else {
        line = nextLine;
      }
    }
    lines += line ? 1 : 1;
  }

  return Math.max(baseHeight, lines * lineHeight + 8);
}

// ─── 视觉 token：与 PM InpaintingOverlay 同源（暗色场景）────────────────
const TOOLBAR_BG = "rgba(10,10,14,0.82)";
const TOOLBAR_BORDER = "1px solid rgba(255,255,255,0.10)";
const TOOLBAR_BLUR = "blur(20px)";
const TOOLBAR_SHADOW =
  "0 12px 40px rgba(0,0,0,0.5), inset 0 1px 0 rgba(255,255,255,0.06)";

const TOOLBAR_STYLE: React.CSSProperties = {
  background: TOOLBAR_BG,
  border: TOOLBAR_BORDER,
  backdropFilter: TOOLBAR_BLUR,
  WebkitBackdropFilter: TOOLBAR_BLUR,
  boxShadow: TOOLBAR_SHADOW,
};

const ICON_BTN =
  "flex h-7 w-7 items-center justify-center rounded-lg transition-all hover:bg-white/[0.06] disabled:opacity-40 disabled:hover:bg-transparent";

const newId = () => `obj_${Date.now().toString(36)}_${Math.random().toString(36).slice(2, 7)}`;

function drawText(
  ctx: CanvasRenderingContext2D,
  text: string,
  bounds: { x: number; y: number; width: number; height: number },
  fontSize: number,
  wrap: boolean,
) {
  const lineHeight = fontSize * 1.25;
  let y = bounds.y;
  for (const paragraph of text.split("\n")) {
    if (y > bounds.y + bounds.height - fontSize) return;

    if (!wrap || paragraph.length === 0) {
      ctx.fillText(paragraph, bounds.x, y);
      y += lineHeight;
      continue;
    }

    let line = "";
    for (const ch of paragraph) {
      const nextLine = line + ch;
      if (line && ctx.measureText(nextLine).width > bounds.width) {
        ctx.fillText(line, bounds.x, y);
        y += lineHeight;
        if (y > bounds.y + bounds.height - fontSize) return;
        line = ch;
      } else {
        line = nextLine;
      }
    }
    if (line) {
      ctx.fillText(line, bounds.x, y);
      y += lineHeight;
    }
  }
}

export function DrawingOverlay({ imageUrl, itemName, mode, onConfirm, onCancel }: Props) {
  const [source, setSource] = useState("");

  useEffect(() => {
    let active = true;
    void loadCanvasMediaPixels(imageUrl)
      .then((pixels) => {
        if (active) setSource(pixels);
      })
      .catch((error) => console.error("[canvas] 读取标注源图失败:", error));
    return () => {
      active = false;
    };
  }, [imageUrl]);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const baseImageRef = useRef<HTMLImageElement | null>(null);
  const [objects, setObjects] = useState<DrawObject[]>([]);
  const [redoStack, setRedoStack] = useState<DrawObject[]>([]);
  const [currentTool, setCurrentTool] = useState<AnnotateTool>(mode === "erase" ? "brush" : "brush");
  const [currentColor, setCurrentColor] = useState("#ef4444");
  const [brushSize, setBrushSize] = useState(mode === "erase" ? 32 : DEFAULT_TEXT_SIZE);
  const [drawing, setDrawing] = useState(false);
  const [startPos, setStartPos] = useState<Point | null>(null);
  const [currentPoints, setCurrentPoints] = useState<Point[]>([]);
  const [markerCounter, setMarkerCounter] = useState(1);
  const [imgSize, setImgSize] = useState<{ w: number; h: number } | null>(null);

  // 文字编辑态（PM 风格：内联 textarea 替代 window.prompt）
  const [editingId, setEditingId] = useState<string | null>(null);
  const [selectedTextId, setSelectedTextId] = useState<string | null>(null);
  const [editingValue, setEditingValue] = useState("");
  const editingOriginalRef = useRef<{ id: string; text: string; isNew: boolean } | null>(null);

  // 1) 一次性预加载 base image 到 ref
  useEffect(() => {
    const img = new Image();
    img.crossOrigin = "anonymous";
    img.onload = () => {
      baseImageRef.current = img;
      setImgSize({ w: img.naturalWidth, h: img.naturalHeight });
    };
    if (!source) return;
    img.src = source;
  }, [source]);

  // 2) renderCanvas 同步绘制（base image 来自 ref，不重复异步加载）
  const renderCanvas = useCallback(() => {
    const canvas = canvasRef.current;
    const container = containerRef.current;
    const baseImg = baseImageRef.current;
    if (!canvas || !container || !imgSize || !baseImg) return;

    const rect = container.getBoundingClientRect();
    const scale = Math.min(rect.width / imgSize.w, rect.height / imgSize.h, 1);
    const dw = imgSize.w * scale;
    const dh = imgSize.h * scale;
    const ox = (rect.width - dw) / 2;
    const oy = (rect.height - dh) / 2;

    if (canvas.width !== rect.width || canvas.height !== rect.height) {
      canvas.width = rect.width;
      canvas.height = rect.height;
    }
    canvas.style.left = `${ox}px`;
    canvas.style.top = `${oy}px`;
    canvas.style.width = `${dw}px`;
    canvas.style.height = `${dh}px`;

    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    ctx.drawImage(baseImg, ox, oy, dw, dh);

    for (const obj of objects) {
      if (obj.type === "text" && obj.id === editingId) continue;
      drawObject(ctx, obj, scale, ox, oy);
    }
    if (currentPoints.length > 0 && startPos) {
      const preview: DrawObject = {
        id: "preview",
        type: "brush" as const,
        points: currentPoints,
        color: currentColor,
        size: brushSize,
        erase: isEraseMode,
      };
      drawObject(ctx, preview, scale, ox, oy);
    }
  }, [objects, currentPoints, startPos, currentColor, brushSize, imgSize, editingId]);

  useEffect(() => {
    renderCanvas();
  }, [renderCanvas]);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const ro = new ResizeObserver(renderCanvas);
    ro.observe(container);
    return () => ro.disconnect();
  }, [renderCanvas]);

  // Esc 优先：编辑中先取消文字编辑；否则关闭 overlay
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        if (editingId) {
          e.preventDefault();
          e.stopPropagation();
          cancelTextEdit();
        } else {
          onCancel();
        }
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [editingId, onCancel]);

  function drawObject(
    ctx: CanvasRenderingContext2D,
    obj: DrawObject,
    scale: number,
    ox: number,
    oy: number,
    renderMode: "preview" | "export" = "preview",
  ) {
    if (obj.type === "text") {
      const fontSize = Math.max(MIN_FONT_SIZE, Math.min(MAX_FONT_SIZE, obj.size));
      ctx.fillStyle = obj.color;
      ctx.font = `${fontSize * scale}px sans-serif`;
      ctx.textBaseline = "top";
      const bounds = {
        x: obj.x * scale + ox,
        y: obj.y * scale + oy,
        width: obj.width * scale,
        height: obj.height * scale,
      };
      drawText(ctx, obj.text, bounds, fontSize * scale, obj.width >= MAX_TEXT_WIDTH);
      return;
    }

    ctx.strokeStyle = obj.color;
    ctx.fillStyle = obj.color;
    ctx.lineCap = "round";
    ctx.lineJoin = "round";

    const pts = obj.points.map((p) => ({ x: p.x * scale + ox, y: p.y * scale + oy }));

    if (obj.type === "brush") {
      if (pts.length < 2) return;
      const isExportErase = obj.erase && renderMode === "export";
      ctx.globalCompositeOperation = isExportErase ? "destination-out" : "source-over";
      ctx.globalAlpha = obj.erase && !isExportErase ? 0.72 : 1;
      ctx.strokeStyle = obj.erase ? "#ef4444" : obj.color;
      ctx.lineWidth = obj.size * scale;
      ctx.beginPath();
      ctx.moveTo(pts[0].x, pts[0].y);
      for (let i = 1; i < pts.length; i++) ctx.lineTo(pts[i].x, pts[i].y);
      ctx.stroke();
      ctx.globalAlpha = 1;
      ctx.globalCompositeOperation = "source-over";
    } else if (obj.type === "rect" && pts.length >= 2) {
      ctx.lineWidth = 2;
      const x = Math.min(pts[0].x, pts[1].x);
      const y = Math.min(pts[0].y, pts[1].y);
      const w = Math.abs(pts[1].x - pts[0].x);
      const h = Math.abs(pts[1].y - pts[0].y);
      ctx.strokeRect(x, y, w, h);
    } else if (obj.type === "arrow" && pts.length >= 2) {
      ctx.lineWidth = 2;
      const [start, end] = [pts[0], pts[pts.length - 1]];
      ctx.beginPath();
      ctx.moveTo(start.x, start.y);
      ctx.lineTo(end.x, end.y);
      ctx.stroke();
      const angle = Math.atan2(end.y - start.y, end.x - start.x);
      const headLen = 12;
      ctx.beginPath();
      ctx.moveTo(end.x, end.y);
      ctx.lineTo(end.x - headLen * Math.cos(angle - 0.4), end.y - headLen * Math.sin(angle - 0.4));
      ctx.moveTo(end.x, end.y);
      ctx.lineTo(end.x - headLen * Math.cos(angle + 0.4), end.y - headLen * Math.sin(angle + 0.4));
      ctx.stroke();
    } else if (obj.type === "marker" && pts.length >= 1 && obj.markerNum) {
      const r = 14 * scale;
      ctx.beginPath();
      ctx.arc(pts[0].x, pts[0].y, r, 0, Math.PI * 2);
      ctx.fill();
      ctx.fillStyle = "white";
      ctx.font = `bold ${12 * scale}px sans-serif`;
      ctx.textAlign = "center";
      ctx.textBaseline = "middle";
      ctx.fillText(String(obj.markerNum), pts[0].x, pts[0].y);
      ctx.textAlign = "start";
      ctx.textBaseline = "alphabetic";
    }
  }

  const getPos = (e: React.MouseEvent): Point => {
    const canvas = canvasRef.current;
    if (!canvas) return { x: 0, y: 0 };
    const rect = canvas.getBoundingClientRect();
    const scale = imgSize ? Math.min(rect.width / imgSize.w, rect.height / imgSize.h, 1) : 1;
    const ox = (rect.width - imgSize!.w * scale) / 2;
    const oy = (rect.height - imgSize!.h * scale) / 2;
    return {
      x: (e.clientX - rect.left - ox) / scale,
      y: (e.clientY - rect.top - oy) / scale,
    };
  };

  // 命中检测：text 类型用矩形 hit，其他类型简单跳过
  const hitTest = (imageX: number, imageY: number): DrawObject | null => {
    for (let i = objects.length - 1; i >= 0; i--) {
      const o = objects[i];
      if (o.type === "text") {
        if (imageX >= o.x && imageX <= o.x + o.width && imageY >= o.y && imageY <= o.y + o.height) {
          return o;
        }
      }
    }
    return null;
  };

  const startTextEdit = (obj: TextObject, isNew: boolean) => {
    editingOriginalRef.current = { id: obj.id, text: obj.text, isNew };
    setSelectedTextId(obj.id);
    setEditingId(obj.id);
    setEditingValue(obj.text);
  };

  const commitTextEdit = () => {
    if (!editingId) return;
    const id = editingId;
    setObjects((prev) =>
      prev.map((o) => (o.id === id && o.type === "text" ? { ...o, text: editingValue } : o)),
    );
    setSelectedTextId(id);
    setEditingId(null);
    setEditingValue("");
    editingOriginalRef.current = null;
  };

  const cancelTextEdit = () => {
    const original = editingOriginalRef.current;
    if (!original) {
      setEditingId(null);
      setSelectedTextId(null);
      return;
    }
    if (original.isNew) {
      // 新建未保存 → 删除占位对象
      setObjects((prev) => prev.filter((o) => o.id !== original.id));
    } else {
      // 二次编辑 → 还原原文字
      setObjects((prev) =>
        prev.map((o) => (o.id === original.id && o.type === "text" ? { ...o, text: original.text } : o)),
      );
    }
    setEditingId(null);
    setSelectedTextId(null);
    setEditingValue("");
    editingOriginalRef.current = null;
  };

  const handleMouseDown = (e: React.MouseEvent) => {
    if (editingId) return; // 编辑中不响应
    const pos = getPos(e);
    setDrawing(true);
    setStartPos(pos);
    setCurrentPoints([pos]);
  };

  const handleMouseMove = (e: React.MouseEvent) => {
    if (!drawing) return;
    const pos = getPos(e);
    setCurrentPoints((prev) => [...prev, pos]);
  };

  const handleMouseUp = () => {
    if (!drawing || !startPos) return;
    setDrawing(false);

    if (mode === "erase" || currentTool === "brush") {
      const newObj: ShapeObject = {
        id: newId(),
        type: "brush",
        points: currentPoints,
        color: currentColor,
        size: brushSize,
        erase: mode === "erase",
      };
      setObjects((prev) => [...prev, newObj]);
      setRedoStack([]);
    } else if (currentTool === "marker") {
      const newObj: ShapeObject = {
        id: newId(),
        type: "marker",
        points: [startPos],
        color: currentColor,
        size: 2,
        markerNum: markerCounter,
      };
      setObjects((prev) => [...prev, newObj]);
      setMarkerCounter((c) => c + 1);
      setRedoStack([]);
    } else if (currentTool === "text") {
      // PM 风格：直接创建默认文字框 + 立即进入编辑态（不再 window.prompt）
      const fontSize = Math.max(MIN_FONT_SIZE, Math.min(MAX_FONT_SIZE, brushSize));
      const width = getTextWidth(fontSize);
      const newText: TextObject = {
        id: newId(),
        type: "text",
        x: startPos.x,
        y: startPos.y,
        width,
        height: measureTextHeight("", width, fontSize),
        text: "",
        color: currentColor,
        size: fontSize,
      };
      setObjects((prev) => [...prev, newText]);
      setRedoStack([]);
      startTextEdit(newText, true);
    } else {
      // rect or arrow
      const endPos = currentPoints[currentPoints.length - 1];
      if (endPos) {
        const newObj: ShapeObject = {
          id: newId(),
          type: currentTool,
          points: [startPos, endPos],
          color: currentColor,
          size: 2,
        };
        setObjects((prev) => [...prev, newObj]);
        setRedoStack([]);
      }
    }
    setCurrentPoints([]);
    setStartPos(null);
  };

  const handleDoubleClick = (e: React.MouseEvent) => {
    if (mode === "erase") return;
    const pos = getPos(e);
    const hit = hitTest(pos.x, pos.y);
    if (hit && hit.type === "text") {
      startTextEdit(hit, false);
    }
  };

  const handleConfirm = async () => {
    if (!imgSize) return;
    // 编辑中未提交的文字直接丢弃
    if (editingId) cancelTextEdit();
    const canvas = document.createElement("canvas");
    canvas.width = imgSize.w;
    canvas.height = imgSize.h;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    const img = new Image();
    const pixels = await loadCanvasMediaPixels(imageUrl);
    await new Promise<void>((resolve) => {
      img.onload = () => resolve();
      img.src = pixels;
    });
    ctx.drawImage(img, 0, 0);
    for (const obj of objects) drawObject(ctx, obj, 1, 0, 0, "export");
    onConfirm(canvas.toDataURL("image/png"));
  };

  const handleUndo = () => {
    if (objects.length === 0) return;
    const last = objects[objects.length - 1];
    setObjects((prev) => prev.slice(0, -1));
    setRedoStack((s) => [...s, last]);
  };

  const handleRedo = () => {
    if (redoStack.length === 0) return;
    const last = redoStack[redoStack.length - 1];
    setRedoStack((s) => s.slice(0, -1));
    setObjects((o) => [...o, last]);
  };

  const handleClear = () => {
    if (objects.length === 0) return;
    setRedoStack((s) => [...s, ...objects]);
    setObjects([]);
  };

  if (!imgSize) return null;

  const isEraseMode = mode === "erase";
  const title = isEraseMode ? "擦除编辑" : "图片标注";
  const subtitle = isEraseMode ? "涂抹要移除的区域" : itemName;
  const confirmLabel = isEraseMode ? "确认擦除" : "确认标记";
  const editingObj = editingId ? objects.find((o) => o.id === editingId) : null;
  const editingText = editingObj && editingObj.type === "text" ? editingObj : null;

  // 计算编辑框在画布坐标系中的位置（容器坐标系）
  const containerBox = containerRef.current?.getBoundingClientRect();
  const editingBox = (() => {
    if (!editingText || !containerBox) return null;
    const scale = Math.min(containerBox.width / imgSize.w, containerBox.height / imgSize.h, 1);
    const dw = imgSize.w * scale;
    const dh = imgSize.h * scale;
    const ox = (containerBox.width - dw) / 2;
    const oy = (containerBox.height - dh) / 2;
    return {
      left: editingText.x * scale + ox,
      top: editingText.y * scale + oy,
      width: editingText.width * scale,
      height: editingText.height * scale,
      fontSize: editingText.size * scale,
    };
  })();

  return createPortal(
    <div
      className="fixed inset-0 z-[10000] flex flex-col items-center justify-center"
      onMouseDown={(e) => e.stopPropagation()}
      onDoubleClick={(e) => e.stopPropagation()}
      onContextMenu={(e) => e.stopPropagation()}
    >
      {/* 全屏暗色遮罩 + 毛玻璃 */}
      <div className="absolute inset-0 bg-black/70 backdrop-blur-sm" onClick={onCancel} />

      {/* 顶部标题胶囊 */}
      <div
        className="relative z-10 mb-3 flex items-center gap-3 rounded-2xl px-5 py-2.5"
        style={TOOLBAR_STYLE}
      >
        {isEraseMode ? (
          <Eraser className="h-4 w-4" style={{ color: "#ef4444" }} />
        ) : (
          <Pencil className="h-4 w-4" style={{ color: "#3b82f6" }} />
        )}
        <span className="text-[14px] font-semibold" style={{ color: "rgba(255,255,255,0.92)" }}>
          {title}
        </span>
        <span className="text-[12px]" style={{ color: "rgba(255,255,255,0.55)" }}>
          {subtitle}
        </span>
        <button
          onClick={onCancel}
          className="ml-3 rounded-lg p-1.5 transition-all hover:bg-white/10"
          style={{ color: "rgba(255,255,255,0.65)" }}
          title="关闭 (Esc)"
        >
          <X className="h-4 w-4" />
        </button>
      </div>

      {/* 中部画布区域 */}
      <div
        ref={containerRef}
        className="relative z-10"
        style={{
          width: "min(90vw, 1400px)",
          height: "min(80vh, 900px)",
        }}
      >
        <img
          src={source || imageUrl}
          alt={itemName}
          className="absolute inset-0 h-full w-full rounded-2xl object-contain"
          style={{ background: "rgba(0,0,0,0.4)" }}
          draggable={false}
        />
        <canvas
          ref={canvasRef}
          className="absolute"
          style={{ cursor: editingId ? "default" : "crosshair" }}
          onMouseDown={handleMouseDown}
          onMouseMove={handleMouseMove}
          onMouseUp={handleMouseUp}
          onMouseLeave={handleMouseUp}
          onDoubleClick={handleDoubleClick}
        />
        {/* 文字编辑：PM 风格内联 textarea（替代 window.prompt） */}
        {editingText && editingBox && (
          <textarea
            autoFocus
            value={editingValue}
            onChange={(e) => {
              const value = e.target.value;
              setEditingValue(value);
              if (editingId) {
                setObjects((prev) =>
                  prev.map((o) =>
                    o.id === editingId && o.type === "text"
                      ? { ...o, height: measureTextHeight(value, o.width, o.size) }
                      : o,
                  ),
                );
              }
            }}
            onBlur={commitTextEdit}
            wrap={editingText.width >= MAX_TEXT_WIDTH ? "soft" : "off"}
            onKeyDown={(e) => {
              if (e.key === "Escape") {
                e.preventDefault();
                e.stopPropagation();
                cancelTextEdit();
              } else if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                commitTextEdit();
              }
            }}
            className="absolute z-20 resize-none rounded-md outline-none"
            style={{
              left: editingBox.left,
              top: editingBox.top,
              width: editingBox.width,
              height: editingBox.height,
              color: editingText.color,
              fontSize: Math.max(MIN_FONT_SIZE, editingBox.fontSize),
              lineHeight: 1.25,
              background: "rgba(10,10,14,0.86)",
              border: `1px solid ${editingText.color}`,
              padding: 2,
              boxSizing: "border-box",
              overflow: "hidden",
              whiteSpace: editingText.width >= MAX_TEXT_WIDTH ? "pre-wrap" : "pre",
              overflowWrap: "normal",
              fontFamily: "sans-serif",
            }}
          />
        )}
      </div>

      {/* 底部工具胶囊 */}
      <div
        className="relative z-10 mt-3 flex items-center gap-2 rounded-2xl px-3 py-2.5"
        style={TOOLBAR_STYLE}
      >
        {/* 工具按钮 */}
        <div className="flex items-center gap-0.5">
          {(isEraseMode ? ERASE_TOOLS : ANNOTATE_TOOLS).map(({ tool, icon: Icon, label }) => {
            const active = currentTool === tool;
            return (
              <button
                key={tool}
                onClick={() => {
                  setCurrentTool(tool);
                  if (tool === "text") {
                    setBrushSize((size) => Math.max(size, DEFAULT_TEXT_SIZE));
                  }
                }}
                className={ICON_BTN}
                style={{
                  background: active ? "rgba(255,255,255,0.12)" : "transparent",
                  color: active ? "rgba(255,255,255,0.95)" : "rgba(255,255,255,0.6)",
                }}
                title={label}
              >
                <Icon className="h-3.5 w-3.5" />
              </button>
            );
          })}
        </div>

        <div className="h-5 w-px" style={{ background: "rgba(255,255,255,0.10)" }} />

        {/* 颜色（仅 annotate） */}
        {!isEraseMode && (
          <>
            <div className="flex items-center gap-0.5">
              {COLORS.map((c) => {
                const active = currentColor === c.solid;
                return (
                  <button
                    key={c.solid}
                    onClick={() => setCurrentColor(c.solid)}
                    className="flex h-7 w-7 items-center justify-center rounded-lg transition-all"
                    style={{
                      background: active ? "rgba(255,255,255,0.12)" : "transparent",
                    }}
                    title={`${c.label}色`}
                  >
                    <div
                      className="h-3.5 w-3.5 rounded-full transition-all"
                      style={{
                        background: c.solid,
                        boxShadow: active
                          ? `0 0 0 2px rgba(248,250,252,1), 0 0 8px ${c.solid}`
                          : "none",
                      }}
                    />
                  </button>
                );
              })}
            </div>
            <div className="h-5 w-px" style={{ background: "rgba(255,255,255,0.10)" }} />
          </>
        )}

        {/* 大小（文字 = 字号；其他 = 画笔粗细） */}
        <div className="flex items-center gap-2 px-1">
          <span className="text-[11px] tabular-nums" style={{ color: "rgba(255,255,255,0.55)" }}>
            {currentTool === "text" && !isEraseMode
              ? `${brushSize}pt`
              : `${brushSize}px`}
          </span>
          <Slider
            value={[brushSize]}
            min={currentTool === "text" && !isEraseMode ? MIN_FONT_SIZE : 4}
            max={currentTool === "text" && !isEraseMode ? MAX_FONT_SIZE : isEraseMode ? 120 : 64}
            step={1}
            onValueChange={(v) => {
              const nextSize = v[0] ?? DEFAULT_TEXT_SIZE;
              setBrushSize(nextSize);
              const targetId = editingId ?? selectedTextId;
              if (currentTool === "text" && !isEraseMode && targetId) {
                setObjects((prev) =>
                  prev.map((o) =>
                    o.id === targetId && o.type === "text"
                      ? {
                          ...o,
                          size: nextSize,
                          width: getTextWidth(nextSize),
                          height: measureTextHeight(
                            o.text,
                            getTextWidth(nextSize),
                            nextSize,
                          ),
                        }
                      : o,
                  ),
                );
              }
            }}
            className="w-28"
          />
        </div>

        <div className="h-5 w-px" style={{ background: "rgba(255,255,255,0.10)" }} />

        {/* 撤销 / 重做 / 清空 */}
        <div className="flex items-center gap-0.5">
          <button onClick={handleUndo} disabled={objects.length === 0} className={ICON_BTN}
            style={{ color: "rgba(255,255,255,0.65)" }} title="撤销 (Ctrl+Z)">
            <Undo2 className="h-3.5 w-3.5" />
          </button>
          <button onClick={handleRedo} disabled={redoStack.length === 0} className={ICON_BTN}
            style={{ color: "rgba(255,255,255,0.65)" }} title="重做 (Ctrl+Shift+Z)">
            <Redo2 className="h-3.5 w-3.5" />
          </button>
          <button onClick={handleClear} disabled={objects.length === 0} className={ICON_BTN}
            style={{ color: "rgba(255,255,255,0.65)" }} title="清空">
            <Trash2 className="h-3.5 w-3.5" />
          </button>
        </div>

        <div className="h-5 w-px" style={{ background: "rgba(255,255,255,0.10)" }} />

        {/* 取消 + 确认 */}
        <button
          onClick={onCancel}
          className="rounded-xl px-3 py-1.5 text-[12px] font-medium transition-opacity hover:opacity-80"
          style={{ background: "rgba(255,255,255,0.06)", color: "rgba(255,255,255,0.85)" }}
        >
          取消
        </button>
        <button
          onClick={handleConfirm}
          className="flex items-center gap-1.5 rounded-xl px-4 py-1.5 text-[12px] font-semibold transition-all hover:scale-105 active:scale-95"
          style={{
            background: isEraseMode ? "#ef4444" : "#f8fafc",
            color: isEraseMode ? "#ffffff" : "#0f172a",
            boxShadow: isEraseMode
              ? "0 4px 12px rgba(239,68,68,0.35)"
              : "0 4px 12px rgba(255,255,255,0.10)",
          }}
        >
          <Check className="h-3.5 w-3.5" />
          {confirmLabel}
        </button>
      </div>
    </div>,
    document.body,
  );
}
