import type { ComponentProps } from "react";
import { cn } from "./utils";

// shadcn Item composition, adapted to the workbench's existing theme tokens.
// https://ui.shadcn.com/docs/components/item
export function ItemGroup({ className, ...props }: ComponentProps<"div">) {
  return <div role="list" data-slot="item-group" className={cn("flex flex-col", className)} {...props} />;
}

export function Item({
  className,
  variant = "default",
  size = "default",
  ...props
}: ComponentProps<"div"> & {
  variant?: "default" | "outline" | "muted";
  size?: "default" | "sm" | "xs";
}) {
  return (
    <div
      role="listitem"
      data-slot="item"
      data-variant={variant}
      data-size={size}
      className={cn(
        "flex items-center rounded-lg border border-transparent text-sm transition-colors",
        variant === "outline" && "border-border-theme",
        variant === "muted" && "bg-hover-bg",
        size === "default" ? "gap-4 p-4" : size === "sm" ? "gap-3 px-3 py-2.5" : "gap-2 px-2 py-2",
        className,
      )}
      {...props}
    />
  );
}

export function ItemMedia({ className, ...props }: ComponentProps<"div">) {
  return <div data-slot="item-media" className={cn("flex shrink-0 items-center justify-center", className)} {...props} />;
}

export function ItemContent({ className, ...props }: ComponentProps<"div">) {
  return <div data-slot="item-content" className={cn("flex min-w-0 flex-1 flex-col gap-1", className)} {...props} />;
}

export function ItemTitle({ className, ...props }: ComponentProps<"div">) {
  return <div data-slot="item-title" className={cn("text-[13px] font-medium leading-5", className)} {...props} />;
}

export function ItemDescription({ className, ...props }: ComponentProps<"p">) {
  return <p data-slot="item-description" className={cn("text-[11px] leading-4 text-text-secondary", className)} {...props} />;
}

export function ItemActions({ className, ...props }: ComponentProps<"div">) {
  return <div data-slot="item-actions" className={cn("flex shrink-0 items-center gap-2", className)} {...props} />;
}
