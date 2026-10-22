import { Popover as PopoverPrimitive } from "@base-ui/react/popover";
import { cn } from "./utils";
import { SpringPopupContext, SpringSurface, usePopupMotionRoot } from "../ui/SpringSurface";

// Base UI composition from the shadcn registry, with this app's theme/motion.
export function Popover({ actionsRef, onOpenChange, ...props }: PopoverPrimitive.Root.Props) {
  const motion = usePopupMotionRoot(actionsRef);
  return <SpringPopupContext.Provider value={motion.context}>
    <PopoverPrimitive.Root {...props} actionsRef={motion.actionsRef} onOpenChange={(open, details) => {
      onOpenChange?.(open, details);
      motion.onChange(open, details);
    }} />
  </SpringPopupContext.Provider>;
}
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
          render={<SpringSurface />}
          data-slot="popover-content"
          className={cn(
            "origin-[var(--transform-origin)] rounded-2xl border border-border-theme bg-elevated-bg text-text-base shadow-[0_12px_40px_rgb(0,0,0,0.10)] outline-none",
            className,
          )}
          {...props}
        />
      </PopoverPrimitive.Positioner>
    </PopoverPrimitive.Portal>
  );
}
