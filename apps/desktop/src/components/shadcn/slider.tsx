import * as React from "react";
import * as SliderPrimitive from "@radix-ui/react-slider";
import { cn } from "./utils";

type SliderProps = React.ComponentPropsWithoutRef<typeof SliderPrimitive.Root> & {
  appearance?: "default" | "effort";
  thumbProps?: React.ComponentPropsWithoutRef<typeof SliderPrimitive.Thumb>;
};

const Slider = React.forwardRef<
  React.ElementRef<typeof SliderPrimitive.Root>,
  SliderProps
>(({ className, appearance = "default", children, thumbProps, ...props }, ref) => (
  <SliderPrimitive.Root
    ref={ref}
    className={cn("relative flex w-full touch-none select-none items-center", appearance === "effort" && "reasoning-effort-input", className)}
    {...props}
  >
    <SliderPrimitive.Track className={appearance === "effort" ? "reasoning-effort-track" : "relative h-1.5 w-full grow overflow-hidden rounded-full bg-white/10"}>
      <SliderPrimitive.Range className={appearance === "effort" ? "sr-only" : "absolute h-full bg-white/80"} />
      {children}
    </SliderPrimitive.Track>
    <SliderPrimitive.Thumb
      {...thumbProps}
      className={appearance === "effort" ? "reasoning-effort-input-thumb" : cn(
        "block h-3.5 w-3.5 rounded-full border border-white/40 bg-white shadow-[0_2px_8px_rgba(0,0,0,0.35)] transition-transform",
        "hover:scale-110 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-white/40",
        "disabled:pointer-events-none disabled:opacity-50",
      )}
    />
  </SliderPrimitive.Root>
));
Slider.displayName = SliderPrimitive.Root.displayName;

export { Slider };
