import { forwardRef } from "react";
import { ContextMenu as MenuPrimitive } from "@base-ui/react/context-menu";
import { cn } from "./utils";
import "../ui/menuSurface.css";

// shadcn Base UI context menu; use the desktop theme tokens and Tailwind v3 syntax.
const ContextMenu = MenuPrimitive.Root;
const ContextMenuTrigger = MenuPrimitive.Trigger;

function ContextMenuContent({ className, collisionBoundary, ...props }: MenuPrimitive.Popup.Props &
  Pick<MenuPrimitive.Positioner.Props, "collisionBoundary">) {
  return (
    <MenuPrimitive.Portal>
      <MenuPrimitive.Positioner className="isolate z-[20000]" side="right" sideOffset={4}
        collisionBoundary={collisionBoundary} collisionPadding={8} collisionAvoidance={{ side: "flip", align: "shift", fallbackAxisSide: "end" }}>
        <MenuPrimitive.Popup data-slot="context-menu-content" {...props} className={state => cn(
          "ui-menu-surface max-h-[var(--available-height)] overflow-x-hidden overflow-y-auto rounded-lg bg-elevated-bg p-1 text-text-base shadow-md",
          typeof className === "function" ? className(state) : className,
        )} />
      </MenuPrimitive.Positioner>
    </MenuPrimitive.Portal>
  );
}

const ContextMenuItem = forwardRef<HTMLElement, MenuPrimitive.Item.Props>(({ className, ...props }, ref) => (
  <MenuPrimitive.Item ref={ref} data-slot="context-menu-item" {...props} className={state => cn(
    "relative flex min-h-7 cursor-default select-none items-center gap-1.5 rounded-md px-2 py-1 text-[13px] leading-5 outline-none [overflow-wrap:anywhere] focus:bg-hover-bg data-[highlighted]:bg-hover-bg data-[disabled]:pointer-events-none data-[disabled]:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0",
    typeof className === "function" ? className(state) : className,
  )} />
));
ContextMenuItem.displayName = "ContextMenuItem";

function ContextMenuSeparator() {
  return <MenuPrimitive.Separator data-slot="context-menu-separator" className="my-1 h-px bg-border-theme" />;
}

export { ContextMenu, ContextMenuTrigger, ContextMenuContent, ContextMenuItem, ContextMenuSeparator };
