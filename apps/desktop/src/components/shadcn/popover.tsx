import { useContext } from "react";
import { Popover as PopoverPrimitive } from "@base-ui/react/popover";
import { cn } from "./utils";
import { SpringPopupContext, SpringSurface, usePopupMotionRoot } from "../ui/SpringSurface";

// Base UI composition from the shadcn registry, with this app's theme/motion.
export function Popover({ actionsRef, onOpenChange, morph = true, ...props }: PopoverPrimitive.Root.Props & { morph?: boolean }) {
  const motion = usePopupMotionRoot(morph ? actionsRef : undefined);
  return <SpringPopupContext.Provider value={morph ? motion.context : null}>
    <PopoverPrimitive.Root {...props} actionsRef={morph ? motion.actionsRef : actionsRef} onOpenChange={(open, details) => {
      onOpenChange?.(open, details);
      if (morph) motion.onChange(open, details);
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
  const motion = useContext(SpringPopupContext);
  return (
    <PopoverPrimitive.Portal>
      <PopoverPrimitive.Positioner anchor={anchor} align={align} side={side} sideOffset={sideOffset} className="z-[90]">
        <PopoverPrimitive.Popup
          render={motion ? <SpringSurface /> : undefined}
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
