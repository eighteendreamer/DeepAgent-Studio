import React from "react";
import {
  MenuTrigger as AriaMenuTrigger,
  Menu as AriaMenu,
  MenuItem as AriaMenuItem,
  Separator as AriaSeparator,
  Keyboard as AriaKeyboard,
  SubmenuTrigger as AriaSubmenuTrigger,
  Popover as AriaPopover,
  MenuProps,
  MenuItemProps,
  SeparatorProps,
} from "react-aria-components";

export const DropdownMenuTrigger = AriaMenuTrigger;

export function DropdownMenu<T extends object>(props: MenuProps<T> & { className?: string }) {
  const { className = "", ...rest } = props;
  return (
    <AriaPopover className="z-50 min-w-[180px] overflow-hidden rounded-xl border border-border-theme bg-elevated-bg p-1 shadow-[0_10px_28px_rgba(0,0,0,0.14)] animate-in fade-in-0 zoom-in-95">
      <AriaMenu className={`outline-none ${className}`} {...rest} />
    </AriaPopover>
  );
}

export function DropdownMenuItem(props: MenuItemProps & { className?: string }) {
  const { className = "", ...rest } = props;
  return (
    <AriaMenuItem
      className={`group/menu-item flex cursor-pointer items-center rounded-lg px-2.5 py-1.5 text-xs text-text-base outline-none transition-colors data-[focused]:bg-hover-bg data-[disabled]:opacity-50 ${className}`}
      {...rest}
    />
  );
}

export function DropdownMenuSeparator(props: SeparatorProps) {
  return <AriaSeparator className="my-1 h-px bg-border-theme" {...props} />;
}

export function DropdownMenuShortcut({ children, className = "" }: { children: React.ReactNode; className?: string }) {
  return <AriaKeyboard className={`ml-auto text-[10px] tracking-widest text-text-secondary ${className}`}>{children}</AriaKeyboard>;
}

export const DropdownMenuSub = AriaSubmenuTrigger;

export function DropdownMenuSubContent<T extends object>(props: MenuProps<T> & { className?: string }) {
  const { className = "", ...rest } = props;
  return (
    <AriaPopover className="z-50 min-w-[140px] overflow-hidden rounded-xl border border-border-theme bg-elevated-bg p-1 shadow-[0_10px_28px_rgba(0,0,0,0.14)]">
      <AriaMenu className={`outline-none ${className}`} {...rest} />
    </AriaPopover>
  );
}
