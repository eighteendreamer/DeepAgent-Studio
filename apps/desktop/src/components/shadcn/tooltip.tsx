import { Tooltip as TooltipPrimitive } from "@base-ui/react/tooltip";
import { cn } from "./utils";

// shadcn Base UI tooltip, adapted to the desktop theme and Tailwind v3.
export const TooltipProvider = TooltipPrimitive.Provider;
export const Tooltip = TooltipPrimitive.Root;
export const TooltipTrigger = TooltipPrimitive.Trigger;

export function TooltipContent({
  className, side = "top", sideOffset = 6, align = "center", children, ...props
}: TooltipPrimitive.Popup.Props & Pick<TooltipPrimitive.Positioner.Props, "side" | "sideOffset" | "align">) {
  return <TooltipPrimitive.Portal>
    <TooltipPrimitive.Positioner className="isolate z-[100]" side={side} sideOffset={sideOffset} align={align}>
      <TooltipPrimitive.Popup data-slot="tooltip-content" {...props} className={state => cn(
        "rounded-md bg-text-base px-2 py-1 text-xs text-bg-base", typeof className === "function" ? className(state) : className,
      )}>
        {children}
        <TooltipPrimitive.Arrow className="size-2 rotate-45 rounded-[1px] bg-text-base data-[side=top]:-bottom-1 data-[side=bottom]:-top-1 data-[side=left]:-right-1 data-[side=right]:-left-1" />
      </TooltipPrimitive.Popup>
    </TooltipPrimitive.Positioner>
  </TooltipPrimitive.Portal>;
}
