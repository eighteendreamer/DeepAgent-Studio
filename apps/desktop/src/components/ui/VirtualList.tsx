import { memo, useCallback, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { virtualListOffsets, virtualListRange } from "./virtualListLayout";

export interface VirtualListPosition {
  filter: string;
  top: number;
  width: number;
  heights: Map<string, number>;
}

interface Props<T> {
  items: readonly T[];
  itemKey: (item: T) => string;
  renderItem: (item: T) => ReactNode;
  estimatedHeight: number;
  position: VirtualListPosition;
  label: string;
  className?: string;
  empty: ReactNode;
}

/** Variable-height windowing, following ChatTimeline's measured rows and rAF scroll updates.
 * The owner retains geometry only; item data continues to come from its existing store.
 */
function VirtualListInner<T>({ items, itemKey, renderItem, estimatedHeight, position, label, className, empty }: Props<T>) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [viewport, setViewport] = useState({ top: position.top, height: 0 });
  const [measurement, setMeasurement] = useState(0);
  const [focusedKey, setFocusedKey] = useState<string | null>(null);
  const keys = useMemo(() => items.map(itemKey), [items, itemKey]);
  const offsets = useMemo(() => {
    void measurement;
    return virtualListOffsets(keys, position.heights, estimatedHeight);
  }, [keys, position, estimatedHeight, measurement]);
  const range = virtualListRange(offsets, viewport.top, viewport.height, estimatedHeight * 3);

  useLayoutEffect(() => {
    const live = new Set(keys);
    for (const key of position.heights.keys()) if (!live.has(key)) position.heights.delete(key);
  }, [keys, position]);

  useLayoutEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    let frame = 0;
    const read = () => {
      frame = 0;
      position.top = container.scrollTop;
      setViewport({ top: container.scrollTop, height: container.clientHeight });
    };
    const schedule = () => { if (!frame) frame = requestAnimationFrame(read); };
    const resize = () => {
      if (position.width !== container.clientWidth) {
        position.width = container.clientWidth;
        position.heights.clear();
        setMeasurement(value => value + 1);
      }
      schedule();
    };
    container.scrollTop = position.top;
    read();
    resize();
    container.addEventListener("scroll", schedule, { passive: true });
    const observer = new ResizeObserver(resize);
    observer.observe(container);
    return () => {
      position.top = container.scrollTop;
      container.removeEventListener("scroll", schedule);
      observer.disconnect();
      if (frame) cancelAnimationFrame(frame);
    };
  }, [position]);

  // Clamp after filtering/deletion or viewport resize rather than showing a blank window.
  useLayoutEffect(() => {
    const container = containerRef.current;
    if (container && container.scrollTop !== range.scrollTop) {
      container.scrollTop = range.scrollTop;
      position.top = range.scrollTop;
      setViewport(value => ({ ...value, top: range.scrollTop }));
    }
  }, [range.scrollTop, position]);

  const measure = useCallback((key: string, height: number) => {
    const next = Math.ceil(height);
    if (next > 0 && position.heights.get(key) !== next) {
      position.heights.set(key, next);
      setMeasurement(value => value + 1);
    }
  }, [position]);

  const indexes = Array.from({ length: range.end - range.start }, (_, index) => range.start + index);
  // Keep a focused control mounted when the user scrolls with the mouse.
  const focusedIndex = focusedKey ? keys.indexOf(focusedKey) : -1;
  if (focusedIndex >= 0 && (focusedIndex < range.start || focusedIndex >= range.end)) {
    indexes.push(focusedIndex);
    indexes.sort((left, right) => left - right);
  }

  return (
    <div ref={containerRef} role="list" aria-label={label} className={className}
      style={{ overflowAnchor: "none", scrollbarGutter: "stable" }}>
      {items.length === 0 ? empty : (
        <div className="relative" style={{ height: offsets[keys.length] }}>
          {indexes.map(index => (
            <MeasuredRow key={keys[index]} itemKey={keys[index]} index={index} count={items.length}
              top={offsets[index]} onMeasure={measure}
              onFocus={setFocusedKey}>
              {renderItem(items[index])}
            </MeasuredRow>
          ))}
        </div>
      )}
    </div>
  );
}

const MeasuredRow = memo(function MeasuredRow({ itemKey, index, count, top, onMeasure, onFocus, children }: {
  itemKey: string; index: number; count: number; top: number;
  onMeasure: (key: string, height: number) => void;
  onFocus: (key: string | null) => void;
  children: ReactNode;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const element = ref.current;
    if (!element) return;
    const measure = () => onMeasure(itemKey, element.getBoundingClientRect().height);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, [itemKey, onMeasure]);
  return (
    <div ref={ref} role="listitem" aria-posinset={index + 1} aria-setsize={count}
      className="absolute left-0 right-0" style={{ top }}
      onFocusCapture={() => onFocus(itemKey)}
      onBlurCapture={event => { if (!event.currentTarget.contains(event.relatedTarget)) onFocus(null); }}>
      {children}
    </div>
  );
});

export const VirtualList = memo(VirtualListInner) as typeof VirtualListInner;
