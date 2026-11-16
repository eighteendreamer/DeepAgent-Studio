import * as React from "react";
import { Progress as ProgressPrimitive } from "radix-ui";
import { cn } from "./utils";

const Progress = React.forwardRef<React.ElementRef<typeof ProgressPrimitive.Root>, React.ComponentPropsWithoutRef<typeof ProgressPrimitive.Root>>(
  ({ className, value, ...props }, ref) => <ProgressPrimitive.Root ref={ref} value={value} className={cn("relative h-2 w-full overflow-hidden rounded-full bg-gray-200", className)} {...props}>
    <ProgressPrimitive.Indicator className="h-full w-full bg-primary transition-transform duration-300 motion-reduce:transition-none" style={{ transform: `translateX(-${100 - (value ?? 0)}%)` }} />
  </ProgressPrimitive.Root>,
);
Progress.displayName = "Progress";
export { Progress };
