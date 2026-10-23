import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { Slider as SliderPrimitive } from "../shadcn/slider";
import { Button } from "../shadcn/button";
import type { SliderStop } from "./Slider";
import { createEffortMotion } from "./effortMotion";

type Gesture = { id: number; startX: number; lastX: number; time: number; moved: boolean };

type Props<T extends string> = {
  stops: SliderStop<T>[];
  value: T;
  onChange: (value: T) => void;
  ariaLabel: string;
  disabled?: boolean;
};

export function EffortSlider<T extends string>({ stops, value, onChange, ariaLabel, disabled }: Props<T>) {
  const [reducedMotion, setReducedMotion] = useState(() => window.matchMedia("(prefers-reduced-motion: reduce)").matches);
  const active = Math.max(0, stops.findIndex(stop => stop.value === value));
  const max = Math.max(0, stops.length - 1);
  const [preview, setPreview] = useState(active);
  const [dragging, setDragging] = useState(false);
  const previewRef = useRef(active);
  const draggingRef = useRef(false);
  const gesture = useRef<Gesture | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const motion = useRef<ReturnType<typeof createEffortMotion> | null>(null);
  const selected = Math.min(max, Math.max(0, Math.round(preview)));
  const ratio = max ? preview / max : 0;

  const updatePreview = (next: number) => {
    previewRef.current = Math.max(0, Math.min(max, next));
    setPreview(previewRef.current);
  };
  const commit = (next: number) => {
    const index = Math.max(0, Math.min(max, Math.round(next)));
    updatePreview(index);
    if (stops[index] && stops[index].value !== value) onChange(stops[index].value);
  };
  const cancel = () => {
    draggingRef.current = false;
    gesture.current = null;
    setDragging(false);
    updatePreview(active);
    motion.current?.release();
  };

  useEffect(() => {
    const media = window.matchMedia("(prefers-reduced-motion: reduce)");
    const update = () => setReducedMotion(media.matches);
    update();
    media.addEventListener("change", update);
    return () => media.removeEventListener("change", update);
  }, []);

  useEffect(() => {
    // Respect optimistic updates and a parent's rollback after a failed save.
    if (!draggingRef.current) {
      previewRef.current = active;
      setPreview(active);
    }
  }, [active]);

  useLayoutEffect(() => {
    const root = rootRef.current;
    const canvas = canvasRef.current;
    if (!root || !canvas) return;
    const controller = createEffortMotion(root, canvas, ratio, reducedMotion);
    motion.current = controller;
    return () => {
      controller.dispose();
      motion.current = null;
    };
  }, []);

  useLayoutEffect(() => { motion.current?.moveTo(ratio, dragging); }, [ratio, dragging]);
  useLayoutEffect(() => { motion.current?.setReducedMotion(reducedMotion); }, [reducedMotion]);

  return (
    <div ref={rootRef} className="reasoning-effort" data-dragging={dragging || undefined}
      data-maximum={selected === max || undefined} data-disabled={disabled || undefined}>
      <SliderPrimitive appearance="effort" min={0} max={Math.max(1, max)} step={0.001}
        value={[preview]} disabled={disabled || max === 0}
        thumbProps={{ "aria-label": ariaLabel, "aria-valuenow": selected, "aria-valuetext": stops[selected]?.label }}
        onValueChange={([next]) => updatePreview(next)} onValueCommit={([next]) => commit(next)}
        onPointerDown={event => { if (!event.isPrimary || event.button !== 0) event.preventDefault(); }}
        onPointerDownCapture={event => {
          if (disabled || max === 0 || !event.isPrimary || event.button !== 0) return;
          gesture.current = { id: event.pointerId, startX: event.clientX, lastX: event.clientX, time: event.timeStamp, moved: false };
          motion.current?.press();
        }}
        onPointerMoveCapture={event => {
          const current = gesture.current;
          if (!current || event.pointerId !== current.id) return;
          if (!current.moved && Math.abs(event.clientX - current.startX) < 4) return;
          current.moved = true;
          draggingRef.current = true;
          setDragging(true);
          const velocity = (event.clientX - current.lastX) / Math.max(8, event.timeStamp - current.time);
          motion.current?.drag(velocity);
          current.lastX = event.clientX;
          current.time = event.timeStamp;
        }}
        onPointerUpCapture={event => {
          if (event.pointerId !== gesture.current?.id) return;
          gesture.current = null;
          draggingRef.current = false;
          setDragging(false);
          updatePreview(Math.round(previewRef.current));
          motion.current?.release();
        }}
        onPointerCancel={cancel}
        onLostPointerCapture={() => { if (gesture.current) cancel(); }}
        onKeyDownCapture={event => {
          let next = selected;
          if (["ArrowRight", "ArrowUp", "PageUp"].includes(event.key)) next++;
          else if (["ArrowLeft", "ArrowDown", "PageDown"].includes(event.key)) next--;
          else if (event.key === "Home") next = 0;
          else if (event.key === "End") next = max;
          else return;
          event.preventDefault();
          event.stopPropagation();
          if (!disabled) commit(next);
        }}>
        <div className="reasoning-effort-fill" aria-hidden="true"><canvas ref={canvasRef} /></div>
        <div className="reasoning-effort-dots" aria-hidden="true">
          {stops.map(stop => <span key={stop.value} />)}
        </div>
        <span className="reasoning-effort-thumb" aria-hidden="true" />
      </SliderPrimitive>
      <div className="reasoning-effort-stops">
        {stops.map((stop, index) => (
          <Button key={stop.value} variant="ghost" size="sm" disabled={disabled} aria-pressed={index === selected}
            className="h-auto rounded px-0.5 py-[3px] text-[11px] font-normal hover:bg-transparent"
            onClick={() => commit(index)}>{stop.label}</Button>
        ))}
      </div>
    </div>
  );
}
