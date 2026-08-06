
import { useCallback, useEffect, useRef, useState } from "react";
import { cn } from "../shadcn/utils";

/** 方案 E：简单 / 中等 / 深度 对应粒子速度（越大越快） */
const REASONING_MOTION_STOPS = [0.32, 1.05, 3.0] as const;

function reasoningMotionAt(index: number, maxIndex: number): number {
  if (maxIndex <= 0) return REASONING_MOTION_STOPS[0];
  const scaled = (index / maxIndex) * (REASONING_MOTION_STOPS.length - 1);
  const lo = Math.min(REASONING_MOTION_STOPS.length - 1, Math.floor(scaled));
  const hi = Math.min(REASONING_MOTION_STOPS.length - 1, Math.ceil(scaled));
  const t = scaled - lo;
  return REASONING_MOTION_STOPS[lo] + (REASONING_MOTION_STOPS[hi] - REASONING_MOTION_STOPS[lo]) * t;
}

export type SliderStop<T extends string = string> = {
  value: T;
  label: string;
};

type Props<T extends string> = {
  stops: SliderStop<T>[];
  value: T;
  onChange: (value: T) => void;
  ariaLabel?: string;
};

export function Slider<T extends string>({ stops, value, onChange, ariaLabel }: Props<T>) {
  const thumbRadius = 14;
  const activeIndex = Math.max(0, stops.findIndex((s) => s.value === value));
  const maxIndex = Math.max(0, stops.length - 1);
  const previousIndex = useRef(activeIndex);
  const settleTimer = useRef<number | null>(null);
  const dragFrame = useRef<number | null>(null);
  const pendingPreviewIndex = useRef(activeIndex);
  const pointerStartX = useRef(0);
  const didDrag = useRef(false);
  const trackRef = useRef<HTMLDivElement | null>(null);
  const draggingRef = useRef(false);
  const previewIndexRef = useRef(activeIndex);
  const [dragging, setDragging] = useState(false);
  const [settling, setSettling] = useState(false);
  const [directSettling, setDirectSettling] = useState(false);
  const [previewIndex, setPreviewIndex] = useState(activeIndex);
  const [motionEpoch, setMotionEpoch] = useState(0);
  const previewActiveIndex = Math.max(0, Math.min(maxIndex, Math.round(previewIndex)));
  const positionRatio = stops.length <= 1 ? 0.5 : previewIndex / maxIndex;
  const reasoningMotion = reasoningMotionAt(previewIndex, maxIndex);
  const position = `calc(${positionRatio * 100}% + ${thumbRadius - positionRatio * thumbRadius * 2}px)`;

  const updatePreview = useCallback(
    (nextIndex: number) => {
      const clamped = Math.max(0, Math.min(maxIndex, nextIndex));
      previewIndexRef.current = clamped;
      setPreviewIndex(clamped);
    },
    [maxIndex],
  );

  const schedulePreview = useCallback(
    (nextIndex: number) => {
      pendingPreviewIndex.current = nextIndex;
      if (dragFrame.current !== null) return;
      dragFrame.current = window.requestAnimationFrame(() => {
        dragFrame.current = null;
        updatePreview(pendingPreviewIndex.current);
      });
    },
    [updatePreview],
  );

  const triggerSettle = useCallback((source: "drag" | "direct" = "direct") => {
    setSettling(true);
    setDirectSettling(source === "direct");
    if (settleTimer.current !== null) window.clearTimeout(settleTimer.current);
    settleTimer.current = window.setTimeout(() => {
      setSettling(false);
      setDirectSettling(false);
      settleTimer.current = null;
    }, source === "direct" ? 600 : 420);
  }, []);

  const settleAt = useCallback(
    (nextIndex: number, source: "drag" | "direct" = "direct") => {
      const snapped = Math.max(0, Math.min(maxIndex, Math.round(nextIndex)));
      updatePreview(snapped);
      triggerSettle(source);
      const next = stops[snapped];
      if (next && next.value !== value) onChange(next.value);
    },
    [maxIndex, onChange, stops, triggerSettle, updatePreview, value],
  );

  const indexFromClientX = useCallback(
    (clientX: number) => {
      const rect = trackRef.current?.getBoundingClientRect();
      if (!rect || rect.width === 0 || maxIndex === 0) return 0;
      const usableWidth = Math.max(1, rect.width - thumbRadius * 2);
      const ratio = Math.max(
        0,
        Math.min(1, (clientX - rect.left - thumbRadius) / usableWidth),
      );
      return ratio * maxIndex;
    },
    [maxIndex, thumbRadius],
  );

  useEffect(() => {
    if (!draggingRef.current) updatePreview(activeIndex);
    if (previousIndex.current === activeIndex) return;
    previousIndex.current = activeIndex;
    triggerSettle();
    setMotionEpoch((epoch) => epoch + 1);
  }, [activeIndex, triggerSettle, updatePreview]);

  useEffect(() => {
    const handlePointerMove = (event: PointerEvent) => {
      if (!draggingRef.current) return;
      event.preventDefault();
      if (!didDrag.current) {
        if (Math.abs(event.clientX - pointerStartX.current) < 3) return;
        didDrag.current = true;
        setDragging(true);
      }
      schedulePreview(indexFromClientX(event.clientX));
    };

    const handlePointerEnd = (event: PointerEvent) => {
      if (!draggingRef.current) return;
      const finalIndex =
        event.type === "pointercancel"
          ? previewIndexRef.current
          : indexFromClientX(event.clientX);
      const source = didDrag.current ? "drag" : "direct";
      if (dragFrame.current !== null) {
        window.cancelAnimationFrame(dragFrame.current);
        dragFrame.current = null;
      }
      draggingRef.current = false;
      didDrag.current = false;
      setDragging(false);
      settleAt(finalIndex, source);
    };

    window.addEventListener("pointermove", handlePointerMove, { passive: false });
    window.addEventListener("pointerup", handlePointerEnd);
    window.addEventListener("pointercancel", handlePointerEnd);
    return () => {
      window.removeEventListener("pointermove", handlePointerMove);
      window.removeEventListener("pointerup", handlePointerEnd);
      window.removeEventListener("pointercancel", handlePointerEnd);
    };
  }, [indexFromClientX, schedulePreview, settleAt]);

  useEffect(
    () => () => {
      if (settleTimer.current !== null) window.clearTimeout(settleTimer.current);
      if (dragFrame.current !== null) window.cancelAnimationFrame(dragFrame.current);
    },
    [],
  );

  return (
    <div className="flex flex-col gap-1">
      <div className="relative h-9 touch-none select-none">
        <button
          type="button"
          role="slider"
          aria-label={ariaLabel}
          aria-valuemin={0}
          aria-valuemax={maxIndex}
          aria-valuenow={previewActiveIndex}
          aria-valuetext={stops[previewActiveIndex]?.label}
          onPointerDown={(event) => {
            event.preventDefault();
            event.currentTarget.focus();
            pointerStartX.current = event.clientX;
            didDrag.current = false;
            draggingRef.current = true;
            setDragging(false);
          }}
          onKeyDown={(event) => {
            let next = previewActiveIndex;
            if (event.key === "ArrowRight" || event.key === "ArrowUp") next += 1;
            else if (event.key === "ArrowLeft" || event.key === "ArrowDown") next -= 1;
            else if (event.key === "Home") next = 0;
            else if (event.key === "End") next = maxIndex;
            else return;
            event.preventDefault();
            settleAt(next);
          }}
          className="absolute inset-x-0 top-0 z-10 h-9 w-full cursor-grab rounded-full bg-transparent p-0 outline-none active:cursor-grabbing focus-visible:ring-2 focus-visible:ring-primary/20"
        />

        <div
          ref={trackRef}
          className="reasoning-slider-track absolute inset-x-0 top-1.5 h-6 overflow-hidden rounded-full"
        >
          <div
            className={cn(
              "reasoning-slider-fill absolute inset-0 rounded-full",
              dragging
                ? "transition-none"
                : directSettling
                  ? "transition-[clip-path,--reasoning-motion] duration-[560ms] ease-[cubic-bezier(0.16,1,0.3,1)]"
                  : "transition-[clip-path,--reasoning-motion] duration-300 ease-out",
            )}
            style={{
              clipPath: `inset(0 calc(${(1 - positionRatio) * 100}% - ${
                thumbRadius - positionRatio * thumbRadius * 2
              }px) 0 0 round 9999px)`,
              ["--reasoning-motion" as string]: String(reasoningMotion),
            }}
          >
            <span className="reasoning-slider-d-energy absolute inset-0" />
            <span key={`flow-${motionEpoch}`} className="reasoning-slider-d-flow absolute inset-0" aria-hidden="true" />
            <span key={`breathe-${motionEpoch}`} className="reasoning-slider-d-breathe absolute inset-0" aria-hidden="true" />
            <span key={`p1-${motionEpoch}`} className="reasoning-slider-d-particles reasoning-slider-d-particles-1 absolute inset-0" aria-hidden="true" />
            <span key={`p2-${motionEpoch}`} className="reasoning-slider-d-particles reasoning-slider-d-particles-2 absolute inset-0" aria-hidden="true" />
            <span key={`p3-${motionEpoch}`} className="reasoning-slider-d-particles reasoning-slider-d-particles-3 absolute inset-0" aria-hidden="true" />
            <span key={`p4-${motionEpoch}`} className="reasoning-slider-d-particles reasoning-slider-d-particles-4 absolute inset-0" aria-hidden="true" />
            <span key={`p5-${motionEpoch}`} className="reasoning-slider-d-particles reasoning-slider-d-particles-5 absolute inset-0" aria-hidden="true" />
          </div>

          {stops.map((stop, index) => {
            const dotRatio = stops.length <= 1 ? 0.5 : index / (stops.length - 1);
            const dotPosition = `calc(${dotRatio * 100}% + ${thumbRadius - dotRatio * thumbRadius * 2}px)`;
            return (
              <span
                key={stop.value}
                aria-hidden="true"
                className={cn(
                  "absolute top-1/2 z-[2] h-1 w-1 -translate-x-1/2 -translate-y-1/2 rounded-full transition-colors duration-150",
                  index <= previewActiveIndex
                    ? "bg-white/60 shadow-[0_0_5px_rgba(255,255,255,0.28)]"
                    : "bg-primary/25",
                )}
                style={{ left: dotPosition }}
              />
            );
          })}
        </div>

        <span
          aria-hidden="true"
          className={cn(
            "reasoning-slider-thumb absolute top-[18px] z-[5] h-7 -translate-x-1/2 -translate-y-1/2 rounded-full",
            "w-7",
            settling ? "reasoning-slider-thumb-settle" : "",
            dragging
              ? "transition-[box-shadow,background-color] duration-150 ease-out"
              : directSettling
                ? "transition-[left,width,box-shadow,background-color] duration-[560ms] ease-[cubic-bezier(0.16,1,0.3,1)]"
                : "transition-[left,width,box-shadow,background-color] duration-300 ease-out",
          )}
          style={{ left: position }}
        />
      </div>

      <div className="flex justify-between px-0.5 text-[10px] text-text-secondary">
        {stops.map((stop) => {
          const selected = stop.value === stops[previewActiveIndex]?.value;
          return (
            <button
              type="button"
              key={stop.value}
              onClick={() => settleAt(stops.findIndex((item) => item.value === stop.value))}
              className={cn(
                "cursor-pointer bg-transparent p-0 transition-colors duration-150",
                selected ? "font-medium text-text-base" : "text-text-secondary hover:text-text-base",
              )}
            >
              {stop.label}
            </button>
          );
        })}
      </div>
    </div>
  );
}
