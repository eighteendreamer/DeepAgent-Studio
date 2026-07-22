import {
  memo,
  useCallback,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type RefObject,
} from "react";
import type { ChatMessage, ComposerMention, ComposerSkillSelection } from "../../types";
import { chatMessagesToBlocks } from "./chatMessageMapper";
import { groupTurns, stableTurnKey } from "./groupTurns";
import { MessageTurn } from "./MessageTurn";
import type { Turn } from "./timelineTypes";

const VIRTUALIZE_TURN_THRESHOLD = 48;
const ESTIMATED_TURN_HEIGHT = 260;
const TURN_GAP_PX = 32;
const OVERSCAN_PX = 900;

type TimelineTurn = {
  turn: Turn;
  key: string;
};

type ViewportState = {
  top: number;
  height: number;
};

function estimateTurnHeight(turn: Turn): number {
  const assistantTextLength = turn.blocks.reduce((sum, block) => {
    if (block.kind !== "assistant" && block.kind !== "reasoning") return sum;
    return sum + block.text.length;
  }, 0);
  const toolCount = turn.blocks.filter((block) => block.kind === "tool").length;
  return Math.max(
    ESTIMATED_TURN_HEIGHT,
    120 + Math.ceil(assistantTextLength / 110) * 22 + toolCount * 72,
  );
}

export function ChatTimeline({
  messages,
  busy,
  onResend,
  onOpenUrl,
  scrollContainerRef,
}: {
  messages: ChatMessage[];
  busy: boolean;
  onResend: (text: string, skills?: ComposerSkillSelection[], mentions?: ComposerMention[]) => void;
  onOpenUrl?: (url: string) => void;
  scrollContainerRef?: RefObject<HTMLDivElement>;
}) {
  const turns = useMemo(() => groupTurns(chatMessagesToBlocks(messages)), [messages]);
  const timelineTurns = useMemo<TimelineTurn[]>(
    () => turns.map((turn, index) => ({ turn, key: stableTurnKey(turn, index) })),
    [turns],
  );

  if (timelineTurns.length <= VIRTUALIZE_TURN_THRESHOLD || !scrollContainerRef) {
    return (
      <div className="mx-auto flex w-full max-w-4xl flex-col gap-8">
        {timelineTurns.map(({ turn, key }, index) => (
          <MessageTurn
            key={key}
            turn={turn}
            processing={busy && index === timelineTurns.length - 1}
            busy={busy}
            onResend={onResend}
            onOpenUrl={onOpenUrl}
          />
        ))}
      </div>
    );
  }

  return (
    <VirtualizedChatTimeline
      turns={timelineTurns}
      busy={busy}
      onResend={onResend}
      onOpenUrl={onOpenUrl}
      scrollContainerRef={scrollContainerRef}
    />
  );
}

function VirtualizedChatTimeline({
  turns,
  busy,
  onResend,
  onOpenUrl,
  scrollContainerRef,
}: {
  turns: TimelineTurn[];
  busy: boolean;
  onResend: (text: string, skills?: ComposerSkillSelection[], mentions?: ComposerMention[]) => void;
  onOpenUrl?: (url: string) => void;
  scrollContainerRef: RefObject<HTMLDivElement>;
}) {
  const sizeByKeyRef = useRef(new Map<string, number>());
  const [measureVersion, setMeasureVersion] = useState(0);
  const [viewport, setViewport] = useState<ViewportState>(() => ({
    top: scrollContainerRef.current?.scrollTop ?? 0,
    height: scrollContainerRef.current?.clientHeight ?? 0,
  }));

  useLayoutEffect(() => {
    const container = scrollContainerRef.current;
    if (!container) return;

    let frame = 0;
    const updateViewport = () => {
      frame = 0;
      setViewport({
        top: container.scrollTop,
        height: container.clientHeight,
      });
    };
    const scheduleViewportUpdate = () => {
      if (frame) return;
      frame = window.requestAnimationFrame(updateViewport);
    };

    updateViewport();
    container.addEventListener("scroll", scheduleViewportUpdate, { passive: true });
    const observer = new ResizeObserver(scheduleViewportUpdate);
    observer.observe(container);

    return () => {
      container.removeEventListener("scroll", scheduleViewportUpdate);
      observer.disconnect();
      if (frame) window.cancelAnimationFrame(frame);
    };
  }, [scrollContainerRef]);

  useLayoutEffect(() => {
    const liveKeys = new Set(turns.map((turn) => turn.key));
    for (const key of sizeByKeyRef.current.keys()) {
      if (!liveKeys.has(key)) sizeByKeyRef.current.delete(key);
    }
  }, [turns]);

  const updateMeasuredHeight = useCallback((key: string, height: number) => {
    const rounded = Math.ceil(height);
    const previous = sizeByKeyRef.current.get(key);
    if (previous && Math.abs(previous - rounded) < 2) return;
    sizeByKeyRef.current.set(key, rounded);
    setMeasureVersion((version) => version + 1);
  }, []);

  const layout = useMemo(() => {
    void measureVersion;
    let cursor = 0;
    const items = turns.map((item, index) => {
      const height = sizeByKeyRef.current.get(item.key) ?? estimateTurnHeight(item.turn);
      const top = cursor;
      cursor += height + (index === turns.length - 1 ? 0 : TURN_GAP_PX);
      return { ...item, index, top, height };
    });
    return { items, totalHeight: cursor };
  }, [measureVersion, turns]);

  const visibleItems = useMemo(() => {
    const min = Math.max(0, viewport.top - OVERSCAN_PX);
    const max = viewport.top + viewport.height + OVERSCAN_PX;
    return layout.items.filter((item) => item.top + item.height >= min && item.top <= max);
  }, [layout.items, viewport.height, viewport.top]);

  return (
    <div className="mx-auto w-full max-w-4xl">
      <div className="relative w-full" style={{ height: layout.totalHeight }}>
        {visibleItems.map((item) => (
          <MeasuredTurn
            key={item.key}
            itemKey={item.key}
            top={item.top}
            turn={item.turn}
            processing={busy && item.index === turns.length - 1}
            busy={busy}
            onResend={onResend}
            onOpenUrl={onOpenUrl}
            onHeightChange={updateMeasuredHeight}
          />
        ))}
      </div>
    </div>
  );
}

const MeasuredTurn = memo(function MeasuredTurn({
  itemKey,
  top,
  turn,
  processing,
  busy,
  onResend,
  onOpenUrl,
  onHeightChange,
}: {
  itemKey: string;
  top: number;
  turn: Turn;
  processing: boolean;
  busy: boolean;
  onResend: (text: string, skills?: ComposerSkillSelection[], mentions?: ComposerMention[]) => void;
  onOpenUrl?: (url: string) => void;
  onHeightChange: (key: string, height: number) => void;
}) {
  const ref = useRef<HTMLDivElement>(null);

  useLayoutEffect(() => {
    const element = ref.current;
    if (!element) return;

    const measure = () => onHeightChange(itemKey, element.getBoundingClientRect().height);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, [itemKey, onHeightChange]);

  return (
    <div
      ref={ref}
      className="absolute left-0 right-0"
      style={{ transform: `translateY(${top}px)` }}
    >
      <MessageTurn
        turn={turn}
        processing={processing}
        busy={busy}
        onResend={onResend}
        onOpenUrl={onOpenUrl}
      />
    </div>
  );
});
