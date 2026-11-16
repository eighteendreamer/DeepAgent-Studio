import * as React from "react";
import { Checkbox as CheckboxPrimitive } from "radix-ui";
import { Check } from "lucide-react";
import { cn } from "./utils";

const Checkbox = React.forwardRef<React.ElementRef<typeof CheckboxPrimitive.Root>, React.ComponentPropsWithoutRef<typeof CheckboxPrimitive.Root>>(
  ({ className, ...props }, ref) => <CheckboxPrimitive.Root ref={ref} className={cn(
    "peer h-4 w-4 shrink-0 rounded border border-border-theme bg-white outline-none focus-visible:ring-2 focus-visible:ring-primary/30 disabled:cursor-not-allowed disabled:opacity-50 data-[state=checked]:border-primary data-[state=checked]:bg-primary data-[state=checked]:text-white",
    className,
  )} {...props}><CheckboxPrimitive.Indicator className="flex items-center justify-center"><Check className="h-3 w-3" /></CheckboxPrimitive.Indicator></CheckboxPrimitive.Root>,
);
Checkbox.displayName = "Checkbox";
export { Checkbox };
