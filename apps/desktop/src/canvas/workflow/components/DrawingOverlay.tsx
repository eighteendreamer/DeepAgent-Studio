import { useCallback, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Check, X, Square, Type, Hash, ArrowUpRight, Eraser } from "lucide-react";

type DrawMode = "annotate" | "erase";
type AnnotateTool = "rect" | "arrow" | "text" | "marker";

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

interface DrawObject {
  type: AnnotateTool | "brush";
  points: Point[];
  color: string;
  size: number;
  text?: string;
  markerNum?: number;
}

const COLORS = ["#ef4444", "#3b82f6", "#22c55e", "#eab308", "#ffffff"];
const BRUSH_SIZES = [4, 8, 16, 32];

export function DrawingOverlay({ imageUrl, itemName, mode, onConfirm, onCancel }: Props) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const [objects, setObjects] = useState<DrawObject[]>([]);
  const [currentTool, setCurrentTool] = useState<AnnotateTool>("rect");
  const [currentColor, setCurrentColor] = useState("#ef4444");
  const [brushSize, setBrushSize] = useState(8);
  const [drawing, setDrawing] = useState(false);
  const [startPos, setStartPos] = useState<Point | null>(null);
  const [currentPoints, setCurrentPoints] = useState<Point[]>([]);
  const [markerCounter, setMarkerCounter] = useState(1);
  const [imgSize, setImgSize] = useState<{ w: number; h: number } | null>(null);

  // Load image and setup canvas
  useEffect(() => {
    const img = new Image();
    img.crossOrigin = "anonymous";
    img.onload = () => {
      setImgSize({ w: img.naturalWidth, h: img.naturalHeight });
    };
    img.src = imageUrl;
  }, [imageUrl]);

  // Render canvas
  const renderCanvas = useCallback(() => {
    const canvas = canvasRef.current;
    const container = containerRef.current;
    if (!canvas || !container || !imgSize) return;

    const rect = container.getBoundingClientRect();
    const scale = Math.min(rect.width / imgSize.w, rect.height / imgSize.h, 1);
    const dw = imgSize.w * scale;
    const dh = imgSize.h * scale;
    const ox = (rect.width - dw) / 2;
    const oy = (rect.height - dh) / 2;

    canvas.width = rect.width;
    canvas.height = rect.height;
    canvas.style.left = `${ox}px`;
    canvas.style.top = `${oy}px`;
    canvas.style.width = `${dw}px`;
    canvas.style.height = `${dh}px`;

    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    ctx.clearRect(0, 0, canvas.width, canvas.height);

    // Draw base image
    const img = new Image();
    img.onload = () => {
      ctx.drawImage(img, 0, 0, dw, dh);

      // Draw all objects
      for (const obj of objects) {
        drawObject(ctx, obj, scale, ox, oy);
      }

      // Draw current in-progress object
      if (currentPoints.length > 0 && startPos) {
        const currentObj: DrawObject = {
          type: mode === "erase" ? "brush" : currentTool,
          points: currentPoints,
          color: currentColor,
          size: brushSize,
        };
        drawObject(ctx, currentObj, scale, ox, oy);
      }
    };
    img.src = imageUrl;
  }, [objects, currentPoints, startPos, currentTool, currentColor, brushSize, mode, imgSize, imageUrl]);

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

  function drawObject(ctx: CanvasRenderingContext2D, obj: DrawObject, scale: number, ox: number, oy: number) {
    ctx.strokeStyle = obj.color;
    ctx.fillStyle = obj.color;
    ctx.lineWidth = obj.size;
    ctx.lineCap = "round";
    ctx.lineJoin = "round";

    const pts = obj.points.map((p) => ({ x: p.x * scale + ox, y: p.y * scale + oy }));

    if (obj.type === "brush") {
      if (pts.length < 2) return;
      ctx.globalCompositeOperation = "destination-out";
      ctx.beginPath();
      ctx.moveTo(pts[0].x, pts[0].y);
      for (let i = 1; i < pts.length; i++) {
        ctx.lineTo(pts[i].x, pts[i].y);
      }
      ctx.stroke();
      ctx.globalCompositeOperation = "source-over";
    } else if (obj.type === "rect" && pts.length >= 2) {
      const x = Math.min(pts[0].x, pts[1].x);
      const y = Math.min(pts[0].y, pts[1].y);
      const w = Math.abs(pts[1].x - pts[0].x);
      const h = Math.abs(pts[1].y - pts[0].y);
      ctx.strokeRect(x, y, w, h);
    } else if (obj.type === "arrow" && pts.length >= 2) {
      const [start, end] = [pts[0], pts[pts.length - 1]];
      ctx.beginPath();
      ctx.moveTo(start.x, start.y);
      ctx.lineTo(end.x, end.y);
      ctx.stroke();
      // Arrowhead
      const angle = Math.atan2(end.y - start.y, end.x - start.x);
      const headLen = 12;
      ctx.beginPath();
      ctx.moveTo(end.x, end.y);
      ctx.lineTo(end.x - headLen * Math.cos(angle - 0.4), end.y - headLen * Math.sin(angle - 0.4));
      ctx.moveTo(end.x, end.y);
      ctx.lineTo(end.x - headLen * Math.cos(angle + 0.4), end.y - headLen * Math.sin(angle + 0.4));
      ctx.stroke();
    } else if (obj.type === "text" && pts.length >= 1 && obj.text) {
      ctx.font = `bold ${16 * scale}px sans-serif`;
      ctx.fillText(obj.text, pts[0].x, pts[0].y);
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

  const handleMouseDown = (e: React.MouseEvent) => {
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

    if (mode === "erase") {
      setObjects((prev) => [
        ...prev,
        { type: "brush", points: currentPoints, color: "#000", size: brushSize },
      ]);
    } else if (currentTool === "marker") {
      setObjects((prev) => [
        ...prev,
        { type: "marker", points: [startPos], color: currentColor, size: 2, markerNum: markerCounter },
      ]);
      setMarkerCounter((c) => c + 1);
    } else if (currentTool === "text") {
      const text = prompt("输入标注文字:");
      if (text) {
        setObjects((prev) => [
          ...prev,
          { type: "text", points: [startPos], color: currentColor, size: 2, text },
        ]);
      }
    } else {
      // rect or arrow - use start and end points
      const endPos = currentPoints[currentPoints.length - 1];
      if (endPos) {
        setObjects((prev) => [
          ...prev,
          { type: currentTool, points: [startPos, endPos], color: currentColor, size: 2 },
        ]);
      }
    }
    setCurrentPoints([]);
    setStartPos(null);
  };

  const handleConfirm = async () => {
    if (!imgSize) return;
    const canvas = document.createElement("canvas");
    canvas.width = imgSize.w;
    canvas.height = imgSize.h;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    // Draw original image
    const img = new Image();
    img.crossOrigin = "anonymous";
    await new Promise<void>((resolve) => {
      img.onload = () => resolve();
      img.src = imageUrl;
    });
    ctx.drawImage(img, 0, 0);

    // Draw all objects at full resolution
    for (const obj of objects) {
      drawObject(ctx, obj, 1, 0, 0);
    }

    onConfirm(canvas.toDataURL("image/png"));
  };

  const handleUndo = () => {
    setObjects((prev) => prev.slice(0, -1));
  };

  if (!imgSize) return null;

  return createPortal(
    <div
      className="fixed inset-0 flex flex-col"
      style={{ zIndex: 10000, background: "rgba(0,0,0,0.9)" }}
      onMouseDown={(e) => e.stopPropagation()}
      onDoubleClick={(e) => e.stopPropagation()}
      onContextMenu={(e) => e.stopPropagation()}
    >
      {/* Title bar */}
      <div className="flex items-center justify-between px-6 py-3" style={{ borderBottom: "1px solid rgba(255,255,255,0.08)" }}>
        <div className="flex items-center gap-2">
          <div className="h-3 w-3 rounded-sm" style={{ background: mode === "erase" ? "#ef4444" : "#3b82f6" }} />
          <span className="text-sm font-medium" style={{ color: "rgba(255,255,255,0.85)" }}>
            {mode === "erase" ? "擦除" : "标注"} · {itemName}
          </span>
        </div>
        <button onClick={onCancel} className="flex h-8 w-8 items-center justify-center rounded-lg transition-colors hover:bg-white/10">
          <X className="h-4 w-4" style={{ color: "rgba(255,255,255,0.7)" }} />
        </button>
      </div>

      {/* Canvas workspace */}
      <div ref={containerRef} className="relative flex-1 overflow-hidden">
        <canvas
          ref={canvasRef}
          className="absolute cursor-crosshair"
          onMouseDown={handleMouseDown}
          onMouseMove={handleMouseMove}
          onMouseUp={handleMouseUp}
          onMouseLeave={handleMouseUp}
        />
      </div>

      {/* Bottom toolbar */}
      <div
        className="flex items-center justify-between px-6 py-3"
        style={{ borderTop: "1px solid rgba(255,255,255,0.08)", background: "rgba(24,24,27,0.95)" }}
      >
        {/* Tools */}
        {mode === "annotate" && (
          <div className="flex items-center gap-1.5">
            {([
              { tool: "rect", icon: Square, label: "矩形" },
              { tool: "arrow", icon: ArrowUpRight, label: "箭头" },
              { tool: "text", icon: Type, label: "文字" },
              { tool: "marker", icon: Hash, label: "序号" },
            ] as const).map(({ tool, icon: Icon, label }) => (
              <button
                key={tool}
                onClick={() => setCurrentTool(tool)}
                className="flex items-center gap-1.5 rounded-lg px-2.5 py-1.5 text-xs font-medium transition-colors"
                style={{
                  background: currentTool === tool ? "rgba(59,130,246,0.2)" : "transparent",
                  color: currentTool === tool ? "#3b82f6" : "rgba(255,255,255,0.6)",
                }}
              >
                <Icon className="h-3.5 w-3.5" />
                {label}
              </button>
            ))}
          </div>
        )}

        {mode === "erase" && (
          <div className="flex items-center gap-1.5">
            <Eraser className="h-4 w-4" style={{ color: "rgba(255,255,255,0.6)" }} />
            <span className="text-xs" style={{ color: "rgba(255,255,255,0.6)" }}>画笔大小:</span>
            {BRUSH_SIZES.map((size) => (
              <button
                key={size}
                onClick={() => setBrushSize(size)}
                className="rounded-lg px-2.5 py-1.5 text-xs font-medium transition-colors"
                style={{
                  background: brushSize === size ? "rgba(239,68,68,0.2)" : "transparent",
                  color: brushSize === size ? "#ef4444" : "rgba(255,255,255,0.6)",
                }}
              >
                {size}px
              </button>
            ))}
          </div>
        )}

        {/* Colors (annotate mode only) */}
        {mode === "annotate" && (
          <div className="flex items-center gap-1.5">
            {COLORS.map((color) => (
              <button
                key={color}
                onClick={() => setCurrentColor(color)}
                className="h-6 w-6 rounded-full transition-transform"
                style={{
                  background: color,
                  border: currentColor === color ? "2px solid white" : "2px solid transparent",
                  transform: currentColor === color ? "scale(1.2)" : "scale(1)",
                }}
              />
            ))}
          </div>
        )}

        {/* Actions */}
        <div className="flex items-center gap-2">
          <button
            onClick={handleUndo}
            disabled={objects.length === 0}
            className="rounded-lg px-3 py-1.5 text-xs font-medium transition-colors disabled:opacity-40 hover:bg-white/10"
            style={{ color: "rgba(255,255,255,0.7)" }}
          >
            撤销
          </button>
          <button
            onClick={onCancel}
            className="rounded-lg px-4 py-2 text-sm font-medium transition-colors hover:bg-white/10"
            style={{ color: "rgba(255,255,255,0.7)" }}
          >
            取消
          </button>
          <button
            onClick={handleConfirm}
            className="flex items-center gap-1.5 rounded-lg px-4 py-2 text-sm font-medium transition-colors"
            style={{ background: mode === "erase" ? "#ef4444" : "#3b82f6", color: "white" }}
          >
            <Check className="h-4 w-4" />
            确认
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
