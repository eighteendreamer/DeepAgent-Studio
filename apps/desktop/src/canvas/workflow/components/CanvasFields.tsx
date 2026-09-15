import type { ComponentProps, ReactNode } from "react";
import { Check, ChevronDown } from "lucide-react";
import { Button } from "../../../components/shadcn/button";
import { Input } from "../../../components/shadcn/input";
import { Textarea } from "../../../components/shadcn/textarea";
import { Label } from "../../../components/shadcn/label";
import { cn } from "../../../components/shadcn/utils";
import { DropdownMenu, DropdownMenuTrigger, DropdownMenuContent, DropdownMenuItem } from "../../../components/shadcn/dropdown-menu";

export const CANVAS_MENU_CLASS = "!min-w-0 !max-h-72 overflow-y-auto !rounded-xl !p-1 !text-[12px] !shadow-[0_8px_28px_rgba(0,0,0,0.45)] !border !border-white/[0.08] !bg-[rgba(24,24,27,0.96)] backdrop-blur-[40px]";
export const CANVAS_BUTTON_CLASS = "!text-white/75 !bg-white/[0.05] hover:!bg-white/[0.10] !border-white/10";
const FIELD_CLASS = "!rounded-lg !border !border-white/[0.08] !bg-white/[0.05] !px-2.5 !text-[12px] !text-white/85 !shadow-none placeholder:!text-white/30 focus:!border-white/25 focus:!ring-0 disabled:!opacity-40 [color-scheme:dark]";

export function CanvasInput({ className, ...props }: ComponentProps<typeof Input>) {
  return <Input className={cn(FIELD_CLASS, "!h-8", className)} {...props} />;
}

export function CanvasTextarea({ className, style, ...props }: ComponentProps<typeof Textarea>) {
  return <Textarea className={cn(FIELD_CLASS, "!py-1.5", className)} style={{ minHeight: 72, ...style }} {...props} />;
}

export function CanvasField({ label, htmlFor, children }: { label: string; htmlFor?: string; children: ReactNode }) {
  return <div className="flex min-w-0 flex-col gap-1.5"><Label htmlFor={htmlFor} className="!text-[11px] !font-medium !text-white/55">{label}</Label>{children}</div>;
}

export function CanvasSelect({ value, options, onChange, label, disabled, id }: {
  value: string;
  options: Array<{ value: string; label: string }>;
  onChange: (value: string) => void;
  label?: string;
  disabled?: boolean;
  id?: string;
}) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button id={id} size="sm" variant="ghost" disabled={disabled} aria-label={label} className={cn(CANVAS_BUTTON_CLASS, "w-full !justify-between !font-normal")}>
          <span className="truncate">{options.find((option) => option.value === value)?.label ?? (value || "请选择")}</span>
          <ChevronDown className="h-3 w-3 shrink-0 opacity-60" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" sideOffset={6} className={CANVAS_MENU_CLASS}>
        {options.map((option) => (
          <DropdownMenuItem key={option.value} onSelect={() => onChange(option.value)} className="!rounded-lg !px-2.5 !py-1.5 !text-[12px] !text-white/85 data-[highlighted]:!bg-white/10">
            <Check className={cn("mr-1.5 h-3 w-3 text-violet-400", value !== option.value && "invisible")} />{option.label}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
