import * as React from "react";
import {
  Focusable,
  OverlayArrow,
  Tooltip as TooltipPrimitive,
  TooltipTrigger as TooltipTriggerPrimitive,
} from "react-aria-components";

import { cn } from "../shadcn/utils";

export function TooltipTrigger({
  delay = 250,
  children,
  ...props
}: React.ComponentProps<typeof TooltipTriggerPrimitive>) {
  const [trigger, tooltip] = React.Children.toArray(children);

  return (
    <TooltipTriggerPrimitive delay={delay} {...props}>
      <Focusable>
        {trigger as React.ComponentProps<typeof Focusable>["children"]}
      </Focusable>
      {tooltip}
    </TooltipTriggerPrimitive>
  );
}

export function Tooltip({
  className,
  placement = "top",
  offset = 6,
  crossOffset = 0,
  children,
  ...props
}: Omit<React.ComponentProps<typeof TooltipPrimitive>, "children" | "className"> & {
  className?: string;
  children?: React.ReactNode;
}) {
  return (
    <TooltipPrimitive
      placement={placement}
      offset={offset}
      crossOffset={crossOffset}
      className={cn(
        "z-[70] w-fit max-w-sm rounded-md bg-text-base px-3 py-2 text-[11px] leading-snug text-bg-base shadow-[0_6px_24px_rgba(0,0,0,0.16)]",
        className,
      )}
      {...props}
    >
      {children}
      <OverlayArrow
        className="z-[70] h-2.5 w-2.5 rotate-45 rounded-[2px] bg-text-base"
      />
    </TooltipPrimitive>
  );
}
