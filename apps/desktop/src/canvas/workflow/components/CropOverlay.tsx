import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Check, X } from "lucide-react";
import { Input } from "../../../components/shadcn/input";
import { cropImageToRect } from "../utils/gridCrop";

interface Props {
  imageUrl: string;
  itemName: string;
  initialRatio?: string;
  onConfirm: (dataUrl: string) => void;
  onCancel: () => void;
}

type Handle = "nw" | "n" | "ne" | "w" | "e" | "sw" | "s" | "se" | "drag";

interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

const MIN_CROP = 40;
const PADDING = 40;

const RATIOS: Array<{ key: string; label: string; value: number | null }> = [
  { key: "free", label: "自由", value: null },
  { key: "original", label: "原图", value: null },
  { key: "1:1", label: "1:1", value: 1 },
  { key: "4:5", label: "4:5", value: 4 / 5 },
  { key: "3:4", label: "3:4", value: 3 / 4 },
  { key: "16:9", label: "16:9", value: 16 / 9 },
  { key: "5:4", label: "5:4", value: 5 / 4 },
  { key: "4:3", label: "4:3", value: 4 / 3 },
  { key: "3:2", label: "3:2", value: 3 / 2 },
  { key: "2:3", label: "2:3", value: 2 / 3 },
  { key: "9:16", label: "9:16", value: 9 / 16 },
  { key: "21:9", label: "21:9", value: 21 / 9 },
  { key: "4:1", label: "4:1", value: 4 },
  { key: "1:4", label: "1:4", value: 1 / 4 },
];

function clamp(v: number, min: number, max: number) {
  return Math.max(min, Math.min(max, v));
}

export function CropOverlay({ imageUrl, itemName, initialRatio, onConfirm, onCancel }: Props) {
  const workspaceRef = useRef<HTMLDivElement>(null);
  const imgRef = useRef<HTMLImageElement>(null);
  const [imgRect, setImgRect] = useState<Rect | null>(null);
  const [crop, setCrop] = useState<Rect | null>(null);
  const cropRef = useRef<Rect | null>(null);
  const [ratioKey, setRatioKey] = useState("free");
  const [dragging, setDragging] = useState<{ handle: Handle; startX: number; startY: number; startCrop: Rect } | null>(null);
  const [confirming, setConfirming] = useState(false);
  const appliedInitialRef = useRef(false);
  const [customW, setCustomW] = useState("");
  const [customH, setCustomH] = useState("");

  useEffect(() => { cropRef.current = crop; }, [crop]);

  // Measure image and fit to workspace — runs after portal DOM is committed
  useLayoutEffect(() => {
    const el = workspaceRef.current;
    const img = imgRef.current;
    if (!el || !img) return;

    const fit = () => {
      const ww = el.clientWidth - PADDING * 2;
      const wh = el.clientHeight - PADDING * 2;
      const iw = img.naturalWidth;
      const ih = img.naturalHeight;
      if (!iw || !ih) return;
      const scale = Math.min(ww / iw, wh / ih, 1);
      const dw = iw * scale;
      const dh = ih * scale;
      const ox = (el.clientWidth - dw) / 2;
      const oy = (el.clientHeight - dh) / 2;
      setImgRect({ x: ox, y: oy, w: dw, h: dh });
      setCrop({ x: ox, y: oy, w: dw, h: dh });
    };

    if (img.complete) {
      fit();
    } else {
      img.onload = fit;
      img.onerror = () => { /* image failed to load */ };
    }

    const ro = new ResizeObserver(fit);
    ro.observe(el);
    return () => ro.disconnect();
  }, [imageUrl]);

  const applyRatio = useCallback(
    (key: string) => {
      setRatioKey(key);
      if (!imgRect) return;
      const ratio = RATIOS.find((r) => r.key === key)?.value;
      if (ratio === null || ratio === undefined) return;

      const cx = imgRect.x + imgRect.w / 2;
      const cy = imgRect.y + imgRect.h / 2;
      let w = imgRect.w;
      let h = imgRect.h;

      if (w / h > ratio) {
        w = h * ratio;
      } else {
        h = w / ratio;
      }

      const nx = clamp(cx - w / 2, imgRect.x, imgRect.x + imgRect.w - w);
      const ny = clamp(cy - h / 2, imgRect.y, imgRect.y + imgRect.h - h);
      setCrop({ x: nx, y: ny, w, h });
    },
    [imgRect],
  );

  const applyCustomSize = useCallback(() => {
    const pw = parseInt(customW, 10);
    const ph = parseInt(customH, 10);
    if (!imgRect || !imgRef.current || !pw || !ph || pw <= 0 || ph <= 0) return;
    const scaleX = imgRef.current.naturalWidth / imgRect.w;
    const scaleY = imgRef.current.naturalHeight / imgRect.h;
    const dw = pw / scaleX;
    const dh = ph / scaleY;
    if (dw > imgRect.w || dh > imgRect.h) return;
    const cx = imgRect.x + imgRect.w / 2;
    const cy = imgRect.y + imgRect.h / 2;
    const nx = clamp(cx - dw / 2, imgRect.x, imgRect.x + imgRect.w - dw);
    const ny = clamp(cy - dh / 2, imgRect.y, imgRect.y + imgRect.h - dh);
    setCrop({ x: nx, y: ny, w: dw, h: dh });
    setRatioKey("custom");
  }, [imgRect, customW, customH]);

  useEffect(() => {
    if (!initialRatio || appliedInitialRef.current) return;
    if (!imgRect || !crop) return;
    appliedInitialRef.current = true;
    if (initialRatio === "original") {
      setRatioKey("original");
      setCrop({ x: imgRect.x, y: imgRect.y, w: imgRect.w, h: imgRect.h });
      return;
    }
    if (initialRatio === "free") {
      setRatioKey("free");
      return;
    }
    applyRatio(initialRatio);
  }, [initialRatio, imgRect, crop, applyRatio]);

  useEffect(() => {
    if (!dragging || !imgRect) return;
    const onMove = (e: MouseEvent) => {
      const dx = e.clientX - dragging.startX;
      const dy = e.clientY - dragging.startY;
      const sc = dragging.startCrop;
      const ratio = RATIOS.find((r) => r.key === ratioKey)?.value;
      let newCrop = { ...sc };
      if (dragging.handle === "drag") {
        newCrop.x = clamp(sc.x + dx, imgRect.x, imgRect.x + imgRect.w - sc.w);
        newCrop.y = clamp(sc.y + dy, imgRect.y, imgRect.y + imgRect.h - sc.h);
      } else {
        if (dragging.handle.includes("e")) newCrop.w = Math.max(MIN_CROP, sc.w + dx);
        if (dragging.handle.includes("w")) {
          const nw = Math.max(MIN_CROP, sc.w - dx);
          newCrop.x = sc.x + sc.w - nw;
          newCrop.w = nw;
        }
        if (dragging.handle.includes("s")) newCrop.h = Math.max(MIN_CROP, sc.h + dy);
        if (dragging.handle.includes("n")) {
          const nh = Math.max(MIN_CROP, sc.h - dy);
          newCrop.y = sc.y + sc.h - nh;
          newCrop.h = nh;
        }
        newCrop.x = Math.max(imgRect.x, newCrop.x);
        newCrop.y = Math.max(imgRect.y, newCrop.y);
        newCrop.w = Math.min(newCrop.w, imgRect.x + imgRect.w - newCrop.x);
        newCrop.h = Math.min(newCrop.h, imgRect.y + imgRect.h - newCrop.y);
        if (ratio !== null && ratio !== undefined) {
          if (dragging.handle === "e" || dragging.handle === "w") {
            newCrop.h = newCrop.w / ratio;
          } else if (dragging.handle === "n" || dragging.handle === "s") {
            newCrop.w = newCrop.h * ratio;
          } else {
            const aspect = newCrop.w / newCrop.h;
            if (aspect > ratio) newCrop.h = newCrop.w / ratio;
            else newCrop.w = newCrop.h * ratio;
          }
          newCrop.x = Math.max(imgRect.x, newCrop.x);
          newCrop.y = Math.max(imgRect.y, newCrop.y);
          newCrop.w = Math.min(newCrop.w, imgRect.x + imgRect.w - newCrop.x);
          newCrop.h = Math.min(newCrop.h, imgRect.y + imgRect.h - newCrop.y);
        }
      }
      setCrop(newCrop);
    };
    const onUp = () => setDragging(null);
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    return () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
  }, [dragging, imgRect, ratioKey]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onCancel();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onCancel]);

  const handleConfirm = async () => {
    if (!crop || !imgRect || !imgRef.current) return;
    setConfirming(true);
    try {
      const scaleX = imgRef.current.naturalWidth / imgRect.w;
      const scaleY = imgRef.current.naturalHeight / imgRect.h;
      const sx = (crop.x - imgRect.x) * scaleX;
      const sy = (crop.y - imgRect.y) * scaleY;
      const sw = crop.w * scaleX;
      const sh = crop.h * scaleY;
      const dataUrl = await cropImageToRect(imageUrl, sx, sy, sw, sh);
      onConfirm(dataUrl);
    } finally {
      setConfirming(false);
    }
  };

  const getHandleCursor = (h: Handle) => {
    const map: Record<Handle, string> = {
      nw: "nw-resize", n: "n-resize", ne: "ne-resize",
      w: "w-resize", e: "e-resize",
      sw: "sw-resize", s: "s-resize", se: "se-resize",
      drag: "move",
    };
    return map[h];
  };

  const ready = imgRect !== null && crop !== null;

  return createPortal(
    <div
      className="fixed inset-0 flex flex-col"
      style={{ zIndex: 10000, background: "rgba(0,0,0,0.85)" }}
      onMouseDown={(e) => e.stopPropagation()}
      onDoubleClick={(e) => e.stopPropagation()}
      onContextMenu={(e) => e.stopPropagation()}
    >
      {/* Title bar */}
      <div className="flex items-center justify-between px-6 py-3" style={{ borderBottom: "1px solid rgba(255,255,255,0.08)" }}>
        <div className="flex items-center gap-2">
          <div className="h-3 w-3 rounded-sm" style={{ background: "#3b82f6" }} />
          <span className="text-sm font-medium" style={{ color: "rgba(255,255,255,0.85)" }}>
            裁剪 · {itemName}
          </span>
        </div>
        <button
          onClick={onCancel}
          className="flex h-8 w-8 items-center justify-center rounded-lg transition-colors hover:bg-white/10"
        >
          <X className="h-4 w-4" style={{ color: "rgba(255,255,255,0.7)" }} />
        </button>
      </div>

      {/* Workspace — always rendered so refs attach */}
      <div ref={workspaceRef} className="relative flex-1 overflow-hidden">
        <img
          ref={imgRef}
          src={imageUrl}
          className="absolute"
          style={{
            left: ready ? imgRect.x : "50%",
            top: ready ? imgRect.y : "50%",
            width: ready ? imgRect.w : "auto",
            height: ready ? imgRect.h : "auto",
            maxWidth: "90%",
            maxHeight: "90%",
            transform: ready ? "none" : "translate(-50%, -50%)",
            pointerEvents: "none",
            opacity: ready ? 1 : 0.3,
          }}
          draggable={false}
        />

        {!ready && (
          <div className="absolute inset-0 flex items-center justify-center">
            <span className="text-sm" style={{ color: "rgba(255,255,255,0.5)" }}>加载中...</span>
          </div>
        )}

        {ready && (
          <>
            {/* Discard masks */}
            <div className="absolute pointer-events-none" style={{ left: 0, top: 0, right: 0, height: crop.y, background: "rgba(0,0,0,0.6)" }} />
            <div className="absolute pointer-events-none" style={{ left: 0, top: crop.y + crop.h, right: 0, bottom: 0, background: "rgba(0,0,0,0.6)" }} />
            <div className="absolute pointer-events-none" style={{ left: 0, top: crop.y, width: crop.x, height: crop.h, background: "rgba(0,0,0,0.6)" }} />
            <div className="absolute pointer-events-none" style={{ left: crop.x + crop.w, top: crop.y, right: 0, height: crop.h, background: "rgba(0,0,0,0.6)" }} />

            {/* Crop rect */}
            <div
              className="absolute"
              style={{
                left: crop.x,
                top: crop.y,
                width: crop.w,
                height: crop.h,
                border: "2px solid #3b82f6",
                boxShadow: "0 0 0 1px rgba(59,130,246,0.3)",
              }}
            >
              <div className="absolute inset-0 pointer-events-none">
                <div className="absolute left-1/3 top-0 bottom-0 w-px" style={{ background: "rgba(255,255,255,0.2)" }} />
                <div className="absolute left-2/3 top-0 bottom-0 w-px" style={{ background: "rgba(255,255,255,0.2)" }} />
                <div className="absolute top-1/3 left-0 right-0 h-px" style={{ background: "rgba(255,255,255,0.2)" }} />
                <div className="absolute top-2/3 left-0 right-0 h-px" style={{ background: "rgba(255,255,255,0.2)" }} />
              </div>

              <div
                className="absolute inset-0 cursor-move"
                style={{ zIndex: 2 }}
                onMouseDown={(e) => {
                  e.preventDefault();
                  setDragging({ handle: "drag", startX: e.clientX, startY: e.clientY, startCrop: { ...crop } });
                }}
              />

              {(["nw", "n", "ne", "w", "e", "sw", "s", "se"] as const).map((h) => {
                const isCorner = h.length === 2;
                const size = isCorner ? 12 : 8;
                const pos: Record<string, React.CSSProperties> = {
                  nw: { top: -6, left: -6 }, n: { top: -4, left: "50%", transform: "translateX(-50%)" },
                  ne: { top: -6, right: -6 }, w: { top: "50%", left: -4, transform: "translateY(-50%)" },
                  e: { top: "50%", right: -4, transform: "translateY(-50%)" },
                  sw: { bottom: -6, left: -6 }, s: { bottom: -4, left: "50%", transform: "translateX(-50%)" },
                  se: { bottom: -6, right: -6 },
                };
                return (
                  <div
                    key={h}
                    className="absolute rounded-full"
                    style={{
                      ...pos[h],
                      width: size,
                      height: size,
                      background: "#3b82f6",
                      border: "2px solid white",
                      cursor: getHandleCursor(h),
                      zIndex: 3,
                    }}
                    onMouseDown={(e) => {
                      e.preventDefault();
                      e.stopPropagation();
                      setDragging({ handle: h, startX: e.clientX, startY: e.clientY, startCrop: { ...crop } });
                    }}
                  />
                );
              })}
            </div>
          </>
        )}
      </div>

      {/* Bottom panel */}
      <div
        className="flex items-center justify-between px-6 py-3"
        style={{ borderTop: "1px solid rgba(255,255,255,0.08)", background: "rgba(24,24,27,0.95)" }}
      >
        <div className="flex items-center gap-1.5 flex-wrap">
          {RATIOS.map((r) => (
            <button
              key={r.key}
              onClick={() => applyRatio(r.key)}
              className="rounded-lg px-2.5 py-1.5 text-xs font-medium transition-colors"
              style={{
                background: ratioKey === r.key ? "rgba(59,130,246,0.2)" : "transparent",
                color: ratioKey === r.key ? "#3b82f6" : "rgba(255,255,255,0.6)",
                border: ratioKey === r.key ? "1px solid rgba(59,130,246,0.3)" : "1px solid transparent",
              }}
            >
              {r.label}
            </button>
          ))}
          <div className="mx-1 h-5 w-px" style={{ background: "rgba(255,255,255,0.12)" }} />
          <style>{`
            .crop-input::-webkit-outer-spin-button,
            .crop-input::-webkit-inner-spin-button { -webkit-appearance: none; margin: 0; }
            .crop-input[type=number] { -moz-appearance: textfield; }
          `}</style>
          <Input
            type="number"
            min={1}
            placeholder="宽"
            value={customW}
            onChange={(e) => setCustomW(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && applyCustomSize()}
            className="crop-input !h-7 !w-12 !rounded-l !rounded-r-none !border-r-0 !bg-white/5 !px-2 !py-1 !text-center !text-xs !text-white/85 !shadow-none placeholder:!text-white/30 focus:!border-blue-500/40 focus:!ring-0"
          />
          <Input
            type="number"
            min={1}
            placeholder="高"
            value={customH}
            onChange={(e) => setCustomH(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && applyCustomSize()}
            className="crop-input !h-7 !w-12 !rounded-l-none !rounded-r !bg-white/5 !px-2 !py-1 !text-center !text-xs !text-white/85 !shadow-none placeholder:!text-white/30 focus:!border-blue-500/40 focus:!ring-0"
          />
          <button
            onClick={applyCustomSize}
            className="ml-1 rounded px-2 py-1 text-xs transition-colors hover:bg-white/10"
            style={{ color: "rgba(255,255,255,0.45)" }}
          >
            应用
          </button>
          {crop && imgRect && imgRef.current && (
            <span className="ml-2 text-xs tabular-nums" style={{ color: "rgba(255,255,255,0.35)" }}>
              {Math.round(crop.w * (imgRef.current.naturalWidth / imgRect.w))} ×{" "}
              {Math.round(crop.h * (imgRef.current.naturalHeight / imgRect.h))} px
            </span>
          )}
        </div>

        <div className="flex items-center gap-2">
          <button
            onClick={onCancel}
            className="rounded-lg px-4 py-2 text-sm font-medium transition-colors hover:bg-white/10"
            style={{ color: "rgba(255,255,255,0.7)" }}
          >
            取消
          </button>
          <button
            onClick={handleConfirm}
            disabled={confirming || !ready}
            className="flex items-center gap-1.5 rounded-lg px-4 py-2 text-sm font-medium transition-colors disabled:opacity-50"
            style={{ background: "#3b82f6", color: "white" }}
          >
            <Check className="h-4 w-4" />
            {confirming ? "处理中..." : "确认裁剪"}
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
