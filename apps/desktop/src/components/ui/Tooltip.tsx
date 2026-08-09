import * as React from "react";
import * as TooltipPrimitive from "@radix-ui/react-tooltip";

import { cn } from "../shadcn/utils";

export function TooltipProvider({
  delayDuration = 250,
  ...props
}: React.ComponentProps<typeof TooltipPrimitive.Provider>) {
  return <TooltipPrimitive.Provider delayDuration={delayDuration} {...props} />;
}

export function Tooltip(props: React.ComponentProps<typeof TooltipPrimitive.Root>) {
  return <TooltipPrimitive.Root {...props} />;
}

export function TooltipTrigger(props: React.ComponentProps<typeof TooltipPrimitive.Trigger>) {
  return <TooltipPrimitive.Trigger {...props} />;
}

export function TooltipContent({
  className,
  sideOffset = 6,
  children,
  ...props
}: React.ComponentProps<typeof TooltipPrimitive.Content>) {
  return (
    <TooltipPrimitive.Portal>
      <TooltipPrimitive.Content
        sideOffset={sideOffset}
        className={cn(
          "z-[70] w-fit max-w-sm rounded-md bg-text-base px-3 py-2 text-[11px] leading-snug text-bg-base shadow-[0_6px_24px_rgba(0,0,0,0.16)]",
          className,
        )}
        {...props}
      >
        {children}
        <TooltipPrimitive.Arrow className="h-2.5 w-2.5 fill-text-base" />
      </TooltipPrimitive.Content>
    </TooltipPrimitive.Portal>
  );
}
