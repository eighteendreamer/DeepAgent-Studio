import { Popover as PopoverPrimitive } from "@base-ui/react/popover";
import { cn } from "./utils";

// Base UI composition from the shadcn registry, with this app's theme/motion.
export const Popover = PopoverPrimitive.Root;
export const PopoverTrigger = PopoverPrimitive.Trigger;
export const PopoverTitle = PopoverPrimitive.Title;
export const PopoverClose = PopoverPrimitive.Close;

export function PopoverContent({
  className,
  align = "center",
  side = "bottom",
  sideOffset = 8,
  anchor,
  ...props
}: PopoverPrimitive.Popup.Props &
  Pick<PopoverPrimitive.Positioner.Props, "align" | "side" | "sideOffset" | "anchor">) {
  return (
    <PopoverPrimitive.Portal>
      <PopoverPrimitive.Positioner anchor={anchor} align={align} side={side} sideOffset={sideOffset} className="z-[90]">
        <PopoverPrimitive.Popup
          data-slot="popover-content"
          className={cn(
            "origin-[var(--transform-origin)] rounded-2xl border border-border-theme bg-elevated-bg text-text-base shadow-[0_12px_40px_rgb(0,0,0,0.10)] outline-none transition-[opacity,transform] duration-150 data-[starting-style]:translate-y-1 data-[starting-style]:opacity-0 data-[ending-style]:opacity-0 motion-reduce:transition-none",
            className,
          )}
          {...props}
        />
      </PopoverPrimitive.Positioner>
    </PopoverPrimitive.Portal>
  );
}
