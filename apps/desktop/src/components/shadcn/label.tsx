import * as React from "react";

import { cn } from "./utils";

export interface LabelProps extends React.LabelHTMLAttributes<HTMLLabelElement> {}

const Label = React.forwardRef<HTMLLabelElement, LabelProps>(({ className, ...props }, ref) => (
  <label
    ref={ref}
    className={cn("text-[12px] font-medium text-text-secondary", className)}
    {...props}
  />
));
Label.displayName = "Label";

export { Label };
