import { Menu as MenuPrimitive } from "@base-ui/react/menu";
import { Check } from "lucide-react";
import { cn } from "./utils";

// shadcn Base UI composition: https://ui.shadcn.com/docs/components/base/dropdown-menu
// Kept separate from the existing Radix API, whose callers use asChild/onSelect.
function DropdownMenu(props: MenuPrimitive.Root.Props) {
  return <MenuPrimitive.Root {...props} />;
}

function DropdownMenuTrigger(props: MenuPrimitive.Trigger.Props) {
  return <MenuPrimitive.Trigger data-slot="dropdown-menu-trigger" {...props} />;
}

function DropdownMenuContent({
  align = "start", alignOffset = 0, side = "bottom", sideOffset = 6,
  className, ...props
}: MenuPrimitive.Popup.Props & Pick<MenuPrimitive.Positioner.Props,
  "align" | "alignOffset" | "side" | "sideOffset">) {
  return (
    <MenuPrimitive.Portal>
      <MenuPrimitive.Positioner className="isolate z-[20000] outline-none"
        align={align} alignOffset={alignOffset} side={side} sideOffset={sideOffset}>
        <MenuPrimitive.Popup data-slot="dropdown-menu-content"
          className={state => cn(
            "ui-menu-surface min-w-[var(--anchor-width)] max-h-[var(--available-height)] origin-[var(--transform-origin)] overflow-y-auto rounded-xl bg-elevated-bg p-1.5 text-text-base shadow-[0_6px_24px_rgba(0,0,0,0.10)] outline-none",
            typeof className === "function" ? className(state) : className,
          )} {...props} />
      </MenuPrimitive.Positioner>
    </MenuPrimitive.Portal>
  );
}

function DropdownMenuGroup(props: MenuPrimitive.Group.Props) {
  return <MenuPrimitive.Group data-slot="dropdown-menu-group" {...props} />;
}

function DropdownMenuLabel({ className, ...props }: MenuPrimitive.GroupLabel.Props) {
  return <MenuPrimitive.GroupLabel data-slot="dropdown-menu-label"
    className={state => cn("px-2 py-1.5 text-[11px] text-text-secondary",
      typeof className === "function" ? className(state) : className)} {...props} />;
}

function DropdownMenuRadioGroup(props: MenuPrimitive.RadioGroup.Props) {
  return <MenuPrimitive.RadioGroup data-slot="dropdown-menu-radio-group" {...props} />;
}

function DropdownMenuRadioItem({ className, children, ...props }: MenuPrimitive.RadioItem.Props) {
  return (
    <MenuPrimitive.RadioItem data-slot="dropdown-menu-radio-item" closeOnClick
      className={state => cn(
        "relative flex cursor-default select-none items-center gap-2 rounded-lg py-2 pl-8 pr-3 text-[12px] outline-none data-[highlighted]:bg-hover-bg data-[disabled]:pointer-events-none data-[disabled]:opacity-50",
        typeof className === "function" ? className(state) : className,
      )} {...props}>
      <span className="pointer-events-none absolute left-2 flex size-4 items-center justify-center">
        <MenuPrimitive.RadioItemIndicator><Check className="size-3.5" /></MenuPrimitive.RadioItemIndicator>
      </span>
      {children}
    </MenuPrimitive.RadioItem>
  );
}

export { DropdownMenu, DropdownMenuTrigger, DropdownMenuContent, DropdownMenuGroup,
  DropdownMenuLabel, DropdownMenuRadioGroup, DropdownMenuRadioItem };
