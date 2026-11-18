import * as React from "react";
import { Checkbox as CheckboxPrimitive } from "radix-ui";
import { Check } from "lucide-react";
import { cn } from "./utils";

const Checkbox = React.forwardRef<React.ElementRef<typeof CheckboxPrimitive.Root>, React.ComponentPropsWithoutRef<typeof CheckboxPrimitive.Root>>(
  ({ className, ...props }, ref) => <CheckboxPrimitive.Root ref={ref} className={cn(
    "peer h-4 w-4 shrink-0 rounded border border-text-secondary bg-elevated-bg outline-none focus-visible:ring-2 focus-visible:ring-primary/30 disabled:cursor-not-allowed disabled:opacity-50 data-[state=checked]:border-text-base data-[state=checked]:bg-text-base data-[state=checked]:text-bg-base",
    className,
  )} {...props}><CheckboxPrimitive.Indicator className="flex items-center justify-center"><Check className="h-3 w-3" strokeWidth={3} /></CheckboxPrimitive.Indicator></CheckboxPrimitive.Root>,
);
Checkbox.displayName = "Checkbox";
export { Checkbox };
