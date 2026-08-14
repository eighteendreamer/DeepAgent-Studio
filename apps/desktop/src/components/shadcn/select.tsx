import * as React from "react";
import * as DropdownMenuPrimitive from "@radix-ui/react-dropdown-menu";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { faCheck, faChevronDown } from "@fortawesome/free-solid-svg-icons";

import { cn } from "./utils";

type SelectContextValue = {
  value: string;
  onValueChange?: (value: string) => void;
  labels: Map<string, React.ReactNode>;
  registerLabel: (value: string, label: React.ReactNode) => void;
};

const SelectContext = React.createContext<SelectContextValue | null>(null);

function useSelectContext(component: string) {
  const context = React.useContext(SelectContext);
  if (!context) {
    throw new Error(`${component} must be used inside Select`);
  }
  return context;
}

type SelectProps = {
  value: string;
  onValueChange?: (value: string) => void;
  children: React.ReactNode;
};

function Select({ value, onValueChange, children }: SelectProps) {
  const [labels, setLabels] = React.useState(() => new Map<string, React.ReactNode>());
  const registerLabel = React.useCallback((itemValue: string, label: React.ReactNode) => {
    setLabels((previous) => {
      if (previous.get(itemValue) === label) return previous;
      const next = new Map(previous);
      next.set(itemValue, label);
      return next;
    });
  }, []);

  const context = React.useMemo<SelectContextValue>(
    () => ({ value, onValueChange, labels, registerLabel }),
    [labels, onValueChange, registerLabel, value],
  );

  return (
    <SelectContext.Provider value={context}>
      <DropdownMenuPrimitive.Root>{children}</DropdownMenuPrimitive.Root>
    </SelectContext.Provider>
  );
}

const SelectTrigger = React.forwardRef<
  React.ElementRef<typeof DropdownMenuPrimitive.Trigger>,
  React.ComponentPropsWithoutRef<typeof DropdownMenuPrimitive.Trigger>
>(({ className, children, ...props }, ref) => (
  <DropdownMenuPrimitive.Trigger
    ref={ref}
    className={cn(
      "flex h-10 w-full items-center justify-between gap-2 rounded-xl bg-ui-tint px-3 py-2 text-[13px] text-text-base outline-none transition-[color,box-shadow] data-[placeholder]:text-text-secondary focus-visible:bg-ui-tint-strong focus-visible:ring-2 focus-visible:ring-primary/10 disabled:cursor-not-allowed disabled:opacity-50 [&>span]:line-clamp-1",
      className,
    )}
    {...props}
  >
    {children}
    <FontAwesomeIcon icon={faChevronDown} className="text-[11px] text-text-secondary" />
  </DropdownMenuPrimitive.Trigger>
));
SelectTrigger.displayName = "SelectTrigger";

function SelectValue({ placeholder }: { placeholder?: React.ReactNode }) {
  const { value, labels } = useSelectContext("SelectValue");
  const label = labels.get(value) ?? placeholder ?? value;
  return <span>{label}</span>;
}

const SelectContent = React.forwardRef<
  React.ElementRef<typeof DropdownMenuPrimitive.Content>,
  React.ComponentPropsWithoutRef<typeof DropdownMenuPrimitive.Content>
>(({ className, sideOffset = 4, align = "start", ...props }, ref) => (
  <DropdownMenuPrimitive.Portal>
    <DropdownMenuPrimitive.Content
      ref={ref}
      sideOffset={sideOffset}
      align={align}
      className={cn(
        "z-[80] min-w-[var(--radix-dropdown-menu-trigger-width)] overflow-hidden rounded-xl bg-elevated-bg p-1 text-text-base shadow-[0_6px_24px_rgba(0,0,0,0.10)]",
        "data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0 data-[state=closed]:zoom-out-95 data-[state=open]:zoom-in-95",
        className,
      )}
      {...props}
    />
  </DropdownMenuPrimitive.Portal>
));
SelectContent.displayName = "SelectContent";

type SelectItemProps = Omit<
  React.ComponentPropsWithoutRef<typeof DropdownMenuPrimitive.Item>,
  "onSelect"
> & {
  value: string;
  children: React.ReactNode;
};

const SelectItem = React.forwardRef<
  React.ElementRef<typeof DropdownMenuPrimitive.Item>,
  SelectItemProps
>(({ className, value, children, ...props }, ref) => {
  const { value: selectedValue, onValueChange, registerLabel } = useSelectContext("SelectItem");
  React.useEffect(() => {
    registerLabel(value, children);
  }, [children, registerLabel, value]);

  const selected = selectedValue === value;
  return (
    <DropdownMenuPrimitive.Item
      ref={ref}
      onSelect={() => onValueChange?.(value)}
      className={cn(
        "relative flex w-full cursor-default select-none items-center rounded-lg py-2 pl-8 pr-2 text-[13px] outline-none focus:bg-ui-tint data-[highlighted]:bg-ui-tint data-[disabled]:pointer-events-none data-[disabled]:opacity-50",
        selected && "font-medium",
        className,
      )}
      {...props}
    >
      {selected && (
        <span className="absolute left-2 flex h-3.5 w-3.5 items-center justify-center">
          <FontAwesomeIcon icon={faCheck} className="text-[11px]" />
        </span>
      )}
      {children}
    </DropdownMenuPrimitive.Item>
  );
});
SelectItem.displayName = "SelectItem";

export { Select, SelectTrigger, SelectContent, SelectItem, SelectValue };
