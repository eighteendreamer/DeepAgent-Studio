import { isValidElement, type ReactElement, type ReactNode } from "react";

import { HoverCard, HoverCardContent, HoverCardTrigger } from "../shadcn/hover-card";

interface HoverInfoProps {
  content: ReactNode;
  children: ReactElement;
  side?: "top" | "right" | "bottom" | "left";
}

/** A short, non-underlined information card for an existing UI trigger. */
export function HoverInfo({ content, children, side = "top" }: HoverInfoProps) {
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

  return (
    <HoverCard onOpenChange={(_open, details) => {
      if (details.reason === "trigger-press") details.cancel();
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
