import { PreviewCard as PreviewCardPrimitive } from "@base-ui/react/preview-card";

import { cn } from "./utils";

function HoverCard(props: PreviewCardPrimitive.Root.Props) {
  return <PreviewCardPrimitive.Root data-slot="hover-card" {...props} />;
}

function HoverCardTrigger(props: PreviewCardPrimitive.Trigger.Props) {
  return <PreviewCardPrimitive.Trigger data-slot="hover-card-trigger" {...props} />;
}

function HoverCardContent({
  className,
  side = "top",
  align = "center",
  sideOffset = 6,
  alignOffset = 0,
  ...props
}: PreviewCardPrimitive.Popup.Props &
  Pick<PreviewCardPrimitive.Positioner.Props, "side" | "align" | "sideOffset" | "alignOffset">) {
  return (
    <PreviewCardPrimitive.Portal data-slot="hover-card-portal">
      <PreviewCardPrimitive.Positioner
        side={side}
        align={align}
        sideOffset={sideOffset}
        alignOffset={alignOffset}
        className="isolate z-[70]"
      >
        <PreviewCardPrimitive.Popup
          data-slot="hover-card-content"
          className={cn(
            "z-[70] max-w-sm origin-(--transform-origin) rounded-xl border border-border-theme bg-elevated-bg px-3 py-2 text-[12px] leading-snug text-text-base shadow-[0_6px_24px_rgba(0,0,0,0.12)] outline-none",
            "data-[starting-style]:opacity-0 data-[ending-style]:opacity-0",
            className,
          )}
          {...props}
        />
      </PreviewCardPrimitive.Positioner>
    </PreviewCardPrimitive.Portal>
  );
}

export { HoverCard, HoverCardTrigger, HoverCardContent };
