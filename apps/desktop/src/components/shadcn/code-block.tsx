import * as React from "react";
import { Check, Copy } from "lucide-react";
import { Button } from "./button";
import { cn } from "./utils";
import { HoverInfo } from "../ui/HoverInfo";

// Adapted from shadcn Base Code Block's public slots and line variants:
// https://ui.shadcn.com/docs/components/base/code-block
// Uses this app's theme tokens and Button sizes (Tailwind 3).
export function CodeBlock({ className, variant = "default", size = "default", ...props }:
  React.ComponentProps<"div"> & { variant?: "default" | "destructive"; size?: "default" | "sm" }) {
  return <div data-slot="code-block" data-size={size} data-variant={variant}
    className={cn("group/code-block flex min-w-0 flex-col overflow-hidden rounded-lg border bg-elevated-bg text-text-base",
      variant === "destructive" ? "border-red-500/30" : "border-border-theme", className)} {...props} />;
}
export function CodeBlockHeader({ className, ...props }: React.ComponentProps<"div">) {
  return <div data-slot="code-block-header" className={cn("flex min-w-0 items-center gap-2 border-b border-border-theme bg-hover-bg/40 px-3 py-1.5 text-text-secondary", className)} {...props} />;
}
export const CodeBlockTitle = React.forwardRef<HTMLDivElement, React.ComponentPropsWithoutRef<"div">>(
  ({ className, ...props }, ref) => <div ref={ref} data-slot="code-block-title"
    className={cn("min-w-0 truncate text-xs font-medium", className)} {...props} />,
);
CodeBlockTitle.displayName = "CodeBlockTitle";
export function CodeBlockActions({ className, ...props }: React.ComponentProps<"div">) {
  return <div data-slot="code-block-actions" className={cn("ml-auto flex shrink-0 items-center gap-2", className)} {...props} />;
}
export function CodeBlockCopyButton({ value, className, ...props }: Omit<React.ComponentProps<typeof Button>, "onClick"> & { value: string }) {
  const [state, setState] = React.useState<"idle" | "copied" | "error">("idle");
  React.useEffect(() => {
    if (state === "idle") return;
    const timer = setTimeout(() => setState("idle"), 2000);
    return () => clearTimeout(timer);
  }, [state]);
  return <>
    <HoverInfo content={state === "error" ? "复制失败，请重试" : state === "copied" ? "已复制" : "复制代码"}><Button variant="ghost" size="icon" {...props} data-slot="code-block-copy-button"
      aria-label={state === "copied" ? "已复制" : props["aria-label"] || "复制代码"}
      className={cn("h-6 w-6 rounded-md hover:bg-hover-bg", className)}
      onClick={async () => {
        try { await navigator.clipboard.writeText(value); setState("copied"); }
        catch { setState("error"); }
      }}>{state === "copied" ? <Check size={13} /> : <Copy size={13} />}</Button></HoverInfo>
    <span aria-live="polite" className={state === "error" ? "text-xs text-red-600 dark:text-red-400" : "sr-only"}>
      {state === "error" ? "复制失败，请重试" : state === "copied" ? "已复制" : ""}
    </span>
  </>;
}
export function CodeBlockContent({ className, children, wrap = false, lineNumbers = false, startLine = 1, ...props }:
  React.ComponentProps<"pre"> & { wrap?: boolean; lineNumbers?: boolean; startLine?: number }) {
  return <pre data-slot="code-block-content" dir="ltr" tabIndex={0}
    className={cn("m-0 max-h-80 min-w-0 overflow-auto py-2 font-mono text-[12px] leading-5 outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-primary/30",
      wrap ? "whitespace-pre-wrap break-words" : "whitespace-pre", className)} {...props}>
    <code className={cn("block min-w-full", !wrap && "w-max")}>
      {typeof children === "string" ? children.replace(/\n$/, "").split("\n").map((line, index) =>
        <CodeBlockLine key={index} lineNumber={lineNumbers ? startLine + index : undefined}>{line}</CodeBlockLine>) : children}
    </code>
  </pre>;
}
export function CodeBlockLine({ className, variant = "default", children, lineNumber, gutter, ...props }:
  React.ComponentProps<"span"> & { variant?: "default" | "added" | "removed" | "highlighted"; lineNumber?: number; gutter?: React.ReactNode }) {
  return <span data-slot="code-block-line" data-variant={variant}
    className={cn("flex min-h-5 px-3", variant === "added" && "bg-emerald-500/10 text-emerald-800 dark:text-emerald-200",
      variant === "removed" && "bg-red-500/10 text-red-800 dark:text-red-200", variant === "highlighted" && "bg-hover-bg", className)} {...props}>
    {gutter ?? (lineNumber !== undefined && <span aria-hidden="true" className="mr-4 w-8 shrink-0 select-none text-right text-text-secondary/60">{lineNumber}</span>)}
    <span data-slot="code-block-line-content" className="min-w-0 flex-1">{children || "\n"}</span>
  </span>;
}
export function CodeBlockFooter({ className, ...props }: React.ComponentProps<"div">) {
  return <div data-slot="code-block-footer" className={cn("border-t border-border-theme px-3 py-1.5 text-[11px] text-text-secondary", className)} {...props} />;
}
