import * as React from "react";
import * as DropdownMenuPrimitive from "@radix-ui/react-dropdown-menu";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { FLOATING_MENU, MOTION } from "../ui/motion";
import { cn } from "./utils";
import { SpringSurface } from "../ui/SpringSurface";

const MenuMotionContext = React.createContext<{ open: boolean; trigger: React.MutableRefObject<HTMLElement | null>; pointerHeld: React.MutableRefObject<boolean> } | null>(null);
function useMenuState(open: boolean | undefined, defaultOpen: boolean | undefined, onOpenChange?: (open: boolean) => void) {
  const [localOpen, setLocalOpen] = React.useState(defaultOpen ?? false);
  const trigger = React.useRef<HTMLElement | null>(null);
  const pointerHeld = React.useRef(false);
  const actual = open ?? localOpen;
  return { context: React.useMemo(() => ({ open: actual, trigger, pointerHeld }), [actual]), change: (next: boolean) => {
    if (!next) pointerHeld.current = false;
    if (open === undefined) setLocalOpen(next);
    onOpenChange?.(next);
  } };
}
function DropdownMenu({ open, defaultOpen, onOpenChange, ...props }: React.ComponentPropsWithoutRef<typeof DropdownMenuPrimitive.Root>) {
  const motion = useMenuState(open, defaultOpen, onOpenChange);
  return <MenuMotionContext.Provider value={motion.context}>
    <DropdownMenuPrimitive.Root {...props} open={motion.context.open} onOpenChange={motion.change} />
  </MenuMotionContext.Provider>;
}
const DropdownMenuTrigger = React.forwardRef<React.ElementRef<typeof DropdownMenuPrimitive.Trigger>, React.ComponentPropsWithoutRef<typeof DropdownMenuPrimitive.Trigger>>(
  (props, ref) => {
    const motion = React.useContext(MenuMotionContext);
    return <DropdownMenuPrimitive.Trigger {...props} onPointerDown={(event) => {
      if (motion) motion.pointerHeld.current = event.button === 0;
      props.onPointerDown?.(event);
    }} onPointerUp={(event) => {
      if (motion) motion.pointerHeld.current = false;
      props.onPointerUp?.(event);
    }} onKeyDown={(event) => {
      if (motion) motion.pointerHeld.current = false;
      props.onKeyDown?.(event);
    }} ref={(node) => {
      if (motion) motion.trigger.current = node;
      if (typeof ref === "function") ref(node); else if (ref) ref.current = node;
    }} />;
  },
);
const DropdownMenuGroup = DropdownMenuPrimitive.Group;
const DropdownMenuPortal = DropdownMenuPrimitive.Portal;
function DropdownMenuSub({ open, defaultOpen, onOpenChange, ...props }: React.ComponentPropsWithoutRef<typeof DropdownMenuPrimitive.Sub>) {
  const motion = useMenuState(open, defaultOpen, onOpenChange);
  return <MenuMotionContext.Provider value={motion.context}>
    <DropdownMenuPrimitive.Sub {...props} open={motion.context.open} onOpenChange={motion.change} />
  </MenuMotionContext.Provider>;
}
const DropdownMenuRadioGroup = DropdownMenuPrimitive.RadioGroup;

const menuPanelClass = cn(
  "z-[20000] min-w-[8rem] overflow-hidden rounded-2xl bg-elevated-bg p-1.5 text-text-base shadow-[0_6px_24px_rgba(0,0,0,0.10)]",
  FLOATING_MENU.panel,
);

const menuItemClass = cn(
  "relative flex cursor-default select-none items-center rounded-lg px-2 py-1.5 text-[12px] outline-none",
  FLOATING_MENU.item,
  MOTION.fast,
  "focus:bg-black/5 data-[highlighted]:bg-black/5 data-[disabled]:pointer-events-none data-[disabled]:opacity-50",
);

// 二级面板锚在子菜单项上，而该项比一级面板边缘内缩 p-1.5（6px）；不补这 6px，
// Radix 默认 sideOffset=0 会让二级压进一级面板，两块白底看起来粘成一片。
const SUB_SIDE_OFFSET = 14;

const DropdownMenuSubTrigger = React.forwardRef<
  React.ElementRef<typeof DropdownMenuPrimitive.SubTrigger>,
  React.ComponentPropsWithoutRef<typeof DropdownMenuPrimitive.SubTrigger> & {
    inset?: boolean;
  }
>(({ className, inset, children, ...props }, ref) => {
  const motion = React.useContext(MenuMotionContext);
  return (
  <DropdownMenuPrimitive.SubTrigger
    ref={(node) => {
      if (motion) motion.trigger.current = node;
      if (typeof ref === "function") ref(node); else if (ref) ref.current = node;
    }}
    className={cn(menuItemClass, "data-[state=open]:bg-black/5", inset && "pl-8", className)}
    {...props}
  >
    {children}
    <FontAwesomeIcon icon={["fas", "chevron-right"]} className="ml-auto text-[10px] text-text-secondary" />
  </DropdownMenuPrimitive.SubTrigger>
  );
});
DropdownMenuSubTrigger.displayName = DropdownMenuPrimitive.SubTrigger.displayName;

const DropdownMenuSubContent = React.forwardRef<
  React.ElementRef<typeof DropdownMenuPrimitive.SubContent>,
  React.ComponentPropsWithoutRef<typeof DropdownMenuPrimitive.SubContent>
>(({ className, sideOffset = SUB_SIDE_OFFSET, children, ...props }, ref) => {
  const motion = React.useContext(MenuMotionContext);
  const [present, setPresent] = React.useState(motion?.open ?? false);
  React.useLayoutEffect(() => { if (motion?.open) setPresent(true); }, [motion?.open]);
  if (!motion?.open && !present) return null;
  return (
  <DropdownMenuPrimitive.Portal>
    <DropdownMenuPrimitive.SubContent
      forceMount
      asChild
      ref={ref}
      sideOffset={sideOffset}
      className={cn(menuPanelClass, "z-[80]", className)}
      {...props}
    ><SpringSurface anchor={motion?.trigger} openingPointer={motion?.pointerHeld} onMotionExit={() => setPresent(false)}>{children}</SpringSurface></DropdownMenuPrimitive.SubContent>
  </DropdownMenuPrimitive.Portal>
  );
});
DropdownMenuSubContent.displayName = DropdownMenuPrimitive.SubContent.displayName;

const DropdownMenuContent = React.forwardRef<
  React.ElementRef<typeof DropdownMenuPrimitive.Content>,
  React.ComponentPropsWithoutRef<typeof DropdownMenuPrimitive.Content>
>(({ className, sideOffset = 8, children, onCloseAutoFocus, ...props }, ref) => {
  const motion = React.useContext(MenuMotionContext);
  const [present, setPresent] = React.useState(motion?.open ?? false);
  React.useLayoutEffect(() => { if (motion?.open) setPresent(true); }, [motion?.open]);
  if (!motion?.open && !present) return null;
  return (
  <DropdownMenuPrimitive.Portal>
    <DropdownMenuPrimitive.Content
      forceMount
      asChild
      ref={ref}
      sideOffset={sideOffset}
      className={cn(menuPanelClass, className)}
      onCloseAutoFocus={(event) => {
        onCloseAutoFocus?.(event);
        const active = document.activeElement;
        // A menu action may already have focused a dialog or another control.
        if (active && active !== document.body && active !== motion?.trigger.current
          && !active.closest('.spring-surface[data-state="closed"]')) event.preventDefault();
      }}
      {...props}
    ><SpringSurface anchor={motion?.trigger} openingPointer={motion?.pointerHeld} onMotionExit={() => setPresent(false)}>{children}</SpringSurface></DropdownMenuPrimitive.Content>
  </DropdownMenuPrimitive.Portal>
  );
});
DropdownMenuContent.displayName = DropdownMenuPrimitive.Content.displayName;

const DropdownMenuItem = React.forwardRef<
  React.ElementRef<typeof DropdownMenuPrimitive.Item>,
  React.ComponentPropsWithoutRef<typeof DropdownMenuPrimitive.Item> & {
    inset?: boolean;
  }
>(({ className, inset, ...props }, ref) => (
  <DropdownMenuPrimitive.Item
    ref={ref}
    className={cn(menuItemClass, inset && "pl-8", className)}
    {...props}
  />
));
DropdownMenuItem.displayName = DropdownMenuPrimitive.Item.displayName;

const DropdownMenuSeparator = React.forwardRef<
  React.ElementRef<typeof DropdownMenuPrimitive.Separator>,
  React.ComponentPropsWithoutRef<typeof DropdownMenuPrimitive.Separator>
>(({ className, ...props }, ref) => (
  <DropdownMenuPrimitive.Separator ref={ref} className={cn("-mx-1 my-1 h-px bg-border-theme", className)} {...props} />
));
DropdownMenuSeparator.displayName = DropdownMenuPrimitive.Separator.displayName;

const DropdownMenuLabel = React.forwardRef<
  React.ElementRef<typeof DropdownMenuPrimitive.Label>,
  React.ComponentPropsWithoutRef<typeof DropdownMenuPrimitive.Label> & {
    inset?: boolean;
  }
>(({ className, inset, ...props }, ref) => (
  <DropdownMenuPrimitive.Label
    ref={ref}
    className={cn("px-2 py-1 text-[11px] font-medium text-text-secondary", inset && "pl-8", className)}
    {...props}
  />
));
DropdownMenuLabel.displayName = DropdownMenuPrimitive.Label.displayName;

const DropdownMenuShortcut = ({ className, ...props }: React.HTMLAttributes<HTMLSpanElement>) => {
  return (
    <span
      className={cn("ml-auto text-xs tracking-widest text-text-secondary opacity-60", className)}
      {...props}
    />
  );
};
DropdownMenuShortcut.displayName = "DropdownMenuShortcut";

export {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuGroup,
  DropdownMenuPortal,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuRadioGroup,
  DropdownMenuShortcut,
};
