import { cloneElement, createContext, isValidElement, useCallback, useContext, useEffect, useMemo, useRef, useState, type HTMLAttributes, type ReactElement, type ReactNode } from "react";

import { createHoverCardHandle, HoverCard, HoverCardContent, HoverCardTrigger, type HoverCardHandle } from "../shadcn/hover-card";
import { MenuTriggerOpenContext } from "./MorphPanelLayoutContext";

interface HoverInfoProps {
  content: ReactNode;
  children: ReactElement;
  side?: "top" | "right" | "bottom" | "left";
}

type HintPayload = { content: ReactNode; side?: HoverInfoProps["side"] };
const HoverInfoGroupContext = createContext<{
  handle: HoverCardHandle<HintPayload>;
  dismiss: () => void;
  delay: number;
  closeDelay: number;
} | null>(null);

/** One information surface for one navigation/toolbar region. First hover waits;
 * sibling triggers update the existing payload and anchor without reopening it. */
export function HoverInfoGroup({ children, side, delay = 350, closeDelay = 150 }: {
  children: ReactElement<HTMLAttributes<HTMLElement>>;
  side?: HoverInfoProps["side"];
  delay?: number;
  closeDelay?: number;
}) {
  const [handle] = useState(() => createHoverCardHandle<HintPayload>());
  const [open, setOpen] = useState(false);
  const inside = useRef(false);
  const dismiss = useCallback(() => handle.close(), [handle]);
  const context = useMemo(() => ({ handle, dismiss, delay, closeDelay }), [handle, dismiss, delay, closeDelay]);
  const region = cloneElement(children, {
    onPointerEnter: event => { inside.current = true; children.props.onPointerEnter?.(event); },
    onPointerLeave: event => { inside.current = false; dismiss(); children.props.onPointerLeave?.(event); },
    onPointerDownCapture: event => { dismiss(); children.props.onPointerDownCapture?.(event); },
    onBlurCapture: event => {
      if (!event.currentTarget.contains(event.relatedTarget)) dismiss();
      children.props.onBlurCapture?.(event);
    },
  });
  return <HoverInfoGroupContext.Provider value={context}>
    <HoverCard handle={handle} open={open} onOpenChange={(next, details) => {
      if ((next && details.trigger?.hasAttribute("data-hover-info-disabled"))
        || (!next && details.reason === "trigger-hover" && inside.current)) {
        details.cancel(); return;
      }
      setOpen(next);
    }}>
      {({ payload }) => <>{region}<HoverCardContent side={payload?.side ?? side} continuous>{payload?.content}</HoverCardContent></>}
    </HoverCard>
  </HoverInfoGroupContext.Provider>;
}

/** A short, non-underlined information card for an existing UI trigger. */
export function HoverInfo({ content, children, side }: HoverInfoProps) {
  const menuOpen = useContext(MenuTriggerOpenContext);
  const group = useContext(HoverInfoGroupContext);
  const [open, setOpen] = useState(false);
  useEffect(() => { if (menuOpen) { setOpen(false); group?.dismiss(); } }, [menuOpen, group]);
  if (content == null || content === "") return children;
  const disabled = isValidElement<{ disabled?: boolean; className?: string }>(children)
    && children.props.disabled === true;
  const ariaLabel = typeof content === "string"
    && children.type === "button"
    && isValidElement<{ "aria-label"?: string }>(children)
    && !children.props["aria-label"]
    ? content
    : undefined;
  const trigger = disabled ? (
    <span
      className={children.props.className?.includes("w-full") ? "flex w-full" : "inline-flex max-w-full"}
      tabIndex={0}
      aria-label={typeof content === "string" ? content : undefined}
    >
      {children}
    </span>
  ) : children;

  if (group) return <HoverCardTrigger
    handle={group.handle}
    payload={{ content, side }}
    render={trigger}
    delay={group.delay}
    closeDelay={group.closeDelay}
    aria-label={ariaLabel}
    data-hover-info-disabled={menuOpen || undefined}
    className="!no-underline hover:!no-underline"
  />;

  return (
    <HoverCard open={open && !menuOpen} onOpenChange={(next, details) => {
      if (details.reason === "trigger-press" || (next && menuOpen)) details.cancel();
      else setOpen(next);
    }}>
      <HoverCardTrigger
        render={trigger}
        delay={350}
        closeDelay={150}
        aria-label={ariaLabel}
        className="!no-underline hover:!no-underline"
      />
      <HoverCardContent side={side}>{content}</HoverCardContent>
    </HoverCard>
  );
}
