import * as React from "react";

import {
  GlobalModal,
  GlobalModalDescription,
  GlobalModalFooter,
  GlobalModalHeader,
  GlobalModalTitle,
  modalOriginFromElement,
  type ModalTriggerOrigin,
} from "../ui/GlobalModal";
import { MOTION } from "../ui/motion";
import { cn } from "./utils";

interface DialogContextValue {
  open: boolean;
  setOpen: (open: boolean) => void;
  origin: ModalTriggerOrigin | null;
  setOrigin: (origin: ModalTriggerOrigin | null) => void;
}

const DialogContext = React.createContext<DialogContextValue | null>(null);

function useDialogContext() {
  const context = React.useContext(DialogContext);
  if (!context) {
    throw new Error("Dialog components must be used inside Dialog");
  }
  return context;
}

export interface DialogProps {
  open?: boolean;
  defaultOpen?: boolean;
  onOpenChange?: (open: boolean) => void;
  children: React.ReactNode;
}

function Dialog({ open, defaultOpen = false, onOpenChange, children }: DialogProps) {
  const [internalOpen, setInternalOpen] = React.useState(defaultOpen);
  const [origin, setOrigin] = React.useState<ModalTriggerOrigin | null>(null);
  const controlled = typeof open === "boolean";
  const actualOpen = controlled ? open : internalOpen;
  const setOpen = React.useCallback(
    (next: boolean) => {
      if (!controlled) setInternalOpen(next);
      onOpenChange?.(next);
    },
    [controlled, onOpenChange],
  );

  return (
    <DialogContext.Provider value={{ open: actualOpen, setOpen, origin, setOrigin }}>
      {children}
    </DialogContext.Provider>
  );
}

interface DialogTriggerProps extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  asChild?: boolean;
}

const DialogTrigger = React.forwardRef<HTMLButtonElement, DialogTriggerProps>(
  ({ asChild: _asChild, onClick, ...props }, ref) => {
    const { setOpen, setOrigin } = useDialogContext();
    return (
      <button
        ref={ref}
        type="button"
        onClick={(event) => {
          onClick?.(event);
          if (!event.defaultPrevented) {
            setOrigin(modalOriginFromElement(event.currentTarget));
            setOpen(true);
          }
        }}
        {...props}
      />
    );
  },
);
DialogTrigger.displayName = "DialogTrigger";

/** @deprecated Prefer GlobalModal overlay; kept for API compatibility. */
const DialogOverlay = React.forwardRef<HTMLDivElement, React.HTMLAttributes<HTMLDivElement>>(
  ({ className, ...props }, ref) => (
    <div ref={ref} className={cn("hidden", className)} {...props} />
  ),
);
DialogOverlay.displayName = "DialogOverlay";

const DialogContent = React.forwardRef<HTMLDivElement, React.HTMLAttributes<HTMLDivElement>>(
  ({ className, children, ...props }, ref) => {
    const { open, setOpen, origin } = useDialogContext();

    return (
      <GlobalModal
        open={open}
        origin={origin}
        onClose={() => setOpen(false)}
        panelClassName={className}
      >
        <div ref={ref} role="dialog" aria-modal="true" className="flex min-h-0 flex-1 flex-col" {...props}>
          {children}
        </div>
      </GlobalModal>
    );
  },
);
DialogContent.displayName = "DialogContent";

const DialogHeader = ({ className, ...props }: React.HTMLAttributes<HTMLDivElement>) => (
  <GlobalModalHeader className={className} {...props} />
);
DialogHeader.displayName = "DialogHeader";

const DialogFooter = ({ className, ...props }: React.HTMLAttributes<HTMLDivElement>) => (
  <GlobalModalFooter className={className} {...props} />
);
DialogFooter.displayName = "DialogFooter";

const DialogTitle = React.forwardRef<HTMLHeadingElement, React.HTMLAttributes<HTMLHeadingElement>>(
  ({ className, ...props }, ref) => (
    <GlobalModalTitle ref={ref} className={className} {...props} />
  ),
);
DialogTitle.displayName = "DialogTitle";

const DialogDescription = React.forwardRef<
  HTMLParagraphElement,
  React.HTMLAttributes<HTMLParagraphElement>
>(({ className, ...props }, ref) => (
  <GlobalModalDescription ref={ref} className={className} {...props} />
));
DialogDescription.displayName = "DialogDescription";

interface DialogCloseProps extends React.ButtonHTMLAttributes<HTMLButtonElement> {
  asChild?: boolean;
}

const DialogClose = React.forwardRef<HTMLButtonElement, DialogCloseProps>(
  ({ asChild: _asChild, onClick, ...props }, ref) => {
    const { setOpen } = useDialogContext();
    return (
      <button
        ref={ref}
        type="button"
        onClick={(event) => {
          onClick?.(event);
          if (!event.defaultPrevented) setOpen(false);
        }}
        {...props}
      />
    );
  },
);
DialogClose.displayName = "DialogClose";

/** 弹窗右上角关闭 —— 6px 圆角 tint 底（非正圆 pill） */
const DialogCloseIcon = React.forwardRef<HTMLButtonElement, DialogCloseProps>(
  ({ className, children, onClick, ...props }, ref) => {
    const { setOpen } = useDialogContext();
    return (
      <button
        ref={ref}
        type="button"
        className={cn(
          "flex h-8 w-8 flex-shrink-0 appearance-none items-center justify-center rounded-md border-0 bg-transparent text-text-secondary",
          MOTION.fast,
          "hover:bg-ui-tint-strong hover:text-text-base active:bg-ui-tint-strong",
          className,
        )}
        onClick={(event) => {
          onClick?.(event);
          if (!event.defaultPrevented) setOpen(false);
        }}
        {...props}
      >
        {children}
      </button>
    );
  },
);
DialogCloseIcon.displayName = "DialogCloseIcon";

export {
  Dialog,
  DialogClose,
  DialogCloseIcon,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogOverlay,
  DialogTitle,
  DialogTrigger,
};
