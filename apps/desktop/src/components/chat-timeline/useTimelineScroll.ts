import { useCallback, useLayoutEffect, useRef, type RefObject } from "react";

/** Follow streamed output only while the reader is already at the tail. */
export function useTimelineScroll(
  containerRef: RefObject<HTMLDivElement>,
  contentRef: RefObject<HTMLDivElement>,
  contentVersion: unknown,
  bottomPadding: number,
) {
  const following = useRef(true);
  const userScrolling = useRef(false);
  const expectedTop = useRef<number | null>(null);
  const anchor = useRef<{ key: string; offset: number } | null>(null);

  const preservePosition = useCallback(() => {
    const container = containerRef.current;
    if (!container) return;
    let top = container.scrollTop;
    if (following.current) {
      top = container.scrollHeight - container.clientHeight;
    } else if (anchor.current) {
      const element = Array.from(container.querySelectorAll<HTMLElement>("[data-process-turn]"))
        .find((node) => node.dataset.processTurn === anchor.current!.key);
      if (element) top += element.getBoundingClientRect().top - container.getBoundingClientRect().top - anchor.current.offset;
    }
    const next = Math.max(0, Math.min(top, container.scrollHeight - container.clientHeight));
    if (Math.abs(next - container.scrollTop) > 1) {
      expectedTop.current = next;
      container.scrollTop = next;
    }
  }, [containerRef]);

  useLayoutEffect(() => {
    const container = containerRef.current;
    const content = contentRef.current;
    if (!container || !content) return;
    const onScroll = () => {
      if (!userScrolling.current && expectedTop.current !== null && Math.abs(container.scrollTop - expectedTop.current) <= 1) {
        expectedTop.current = null;
        return;
      }
      expectedTop.current = null;
      // Resize/virtual-list clamping also emits scroll. Only actual user input
      // may disable following or replace the reading anchor.
      if (!userScrolling.current) {
        preservePosition();
        return;
      }
      following.current = container.scrollHeight - container.clientHeight - container.scrollTop <= 24;
      const bounds = container.getBoundingClientRect();
      const element = Array.from(container.querySelectorAll<HTMLElement>("[data-process-turn]"))
        .find((node) => node.getBoundingClientRect().bottom > bounds.top && node.getBoundingClientRect().top < bounds.bottom);
      anchor.current = element ? { key: element.dataset.processTurn!, offset: element.getBoundingClientRect().top - bounds.top } : null;
    };
    const onInput = () => { userScrolling.current = true; };
    const onScrollEnd = () => { userScrolling.current = false; };
    const onPointerDown = (event: PointerEvent) => { if (event.target === container) onInput(); };
    const onKeyDown = (event: KeyboardEvent) => {
      if (["ArrowUp", "ArrowDown", "PageUp", "PageDown", "Home", "End", " "].includes(event.key)) onInput();
    };
    container.addEventListener("scroll", onScroll, { passive: true });
    container.addEventListener("scrollend", onScrollEnd);
    container.addEventListener("wheel", onInput, { passive: true });
    container.addEventListener("touchmove", onInput, { passive: true });
    container.addEventListener("pointerdown", onPointerDown);
    container.addEventListener("keydown", onKeyDown);
    const observer = new ResizeObserver(preservePosition);
    observer.observe(container);
    observer.observe(content);
    preservePosition();
    return () => {
      container.removeEventListener("scroll", onScroll);
      container.removeEventListener("scrollend", onScrollEnd);
      container.removeEventListener("wheel", onInput);
      container.removeEventListener("touchmove", onInput);
      container.removeEventListener("pointerdown", onPointerDown);
      container.removeEventListener("keydown", onKeyDown);
      observer.disconnect();
    };
  }, [containerRef, contentRef, preservePosition]);

  useLayoutEffect(preservePosition, [contentVersion, bottomPadding, preservePosition]);
}
